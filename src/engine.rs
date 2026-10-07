//! Runs one sync thread per account and routes commands to them; the entry point for front ends.
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};

use crate::config::{AccountConfig, Config};
use crate::mail_ops::MailOps;
use crate::paths::Paths;
use crate::sync::{self, Command, Event, Job, RequestId, SyncError};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StartState {
    Running,
    /// Another process holds the account's lock; `pid` is what it wrote there.
    Locked {
        pid: Option<u32>,
    },
    Failed(String),
}

pub type Connector =
    Arc<dyn Fn(&AccountConfig) -> Result<Box<dyn MailOps>, SyncError> + Send + Sync>;

/// Connects to the account's real IMAP server.
pub fn imap_connector() -> Connector {
    Arc::new(|account| Ok(Box::new(sync::connect(account)?)))
}

struct AccountThread {
    config: AccountConfig,
    state: StartState,
    commands: Option<Sender<Job>>,
    wake: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    lock: Option<File>,
}

impl AccountThread {
    fn not_started(config: AccountConfig, state: StartState) -> AccountThread {
        AccountThread {
            config,
            state,
            commands: None,
            wake: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
            handle: None,
            lock: None,
        }
    }

    fn signal_stop(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.wake.store(true, Ordering::Release);
    }

    /// Joins the thread, then releases the lock.
    fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        self.lock = None;
    }
}

struct Spawner {
    paths: Paths,
    events: Sender<Event>,
    connect: Connector,
}

impl Spawner {
    /// Takes the account's lock and spawns its sync thread; a held lock or a failure is recorded as its state.
    fn spawn(&self, config: &AccountConfig) -> AccountThread {
        let name = &config.name;
        let lock = match lock_account(&self.paths, name) {
            Ok(Ok(file)) => file,
            Ok(Err(pid)) => {
                return AccountThread::not_started(config.clone(), StartState::Locked { pid });
            }
            Err(e) => {
                return AccountThread::not_started(
                    config.clone(),
                    StartState::Failed(e.to_string()),
                );
            }
        };
        let (commands_tx, commands) = mpsc::channel();
        let mut thread = AccountThread::not_started(config.clone(), StartState::Running);
        let spawned = std::thread::Builder::new()
            .name(format!("sync-{name}"))
            .spawn({
                let (account, paths, events) =
                    (config.clone(), self.paths.clone(), self.events.clone());
                let (shutdown, wake) = (thread.shutdown.clone(), thread.wake.clone());
                let connect = self.connect.clone();
                move || {
                    let target = account.clone();
                    sync::run_loop(
                        account,
                        paths,
                        events,
                        shutdown,
                        commands,
                        wake,
                        move || connect(&target),
                    )
                }
            });
        match spawned {
            Ok(handle) => {
                thread.commands = Some(commands_tx);
                thread.handle = Some(handle);
                thread.lock = Some(lock);
                thread
            }
            Err(e) => AccountThread::not_started(config.clone(), StartState::Failed(e.to_string())),
        }
    }
}

enum Route {
    Threads {
        threads: Vec<AccountThread>,
        spawner: Spawner,
    },
    Detached {
        accounts: Vec<String>,
        sent: Sender<(String, Job)>,
    },
}

pub struct Engine {
    route: Route,
}

impl Engine {
    /// Takes each account's lock and spawns its sync thread; accounts whose lock is held are not started.
    pub fn start(config: &Config, paths: &Paths) -> (Engine, Receiver<Event>) {
        Engine::start_with(config, paths, imap_connector())
    }

    pub fn start_with(
        config: &Config,
        paths: &Paths,
        connect: Connector,
    ) -> (Engine, Receiver<Event>) {
        let (events, received) = mpsc::channel();
        let spawner = Spawner {
            paths: paths.clone(),
            events,
            connect,
        };
        let threads = config
            .accounts
            .iter()
            .map(|account| spawner.spawn(account))
            .collect();
        let engine = Engine {
            route: Route::Threads { threads, spawner },
        };
        (engine, received)
    }

    /// Stops accounts that were removed or whose settings changed, starts new and changed ones, and leaves the rest.
    pub fn apply_config(&mut self, config: &Config) {
        let Route::Threads { threads, spawner } = &mut self.route else {
            return;
        };
        let (kept, mut stale): (Vec<_>, Vec<_>) = std::mem::take(threads)
            .into_iter()
            .partition(|thread| config.accounts.contains(&thread.config));
        stale.iter().for_each(AccountThread::signal_stop);
        stale.iter_mut().for_each(AccountThread::join);
        let mut kept: Vec<Option<AccountThread>> = kept.into_iter().map(Some).collect();
        *threads = config
            .accounts
            .iter()
            .map(|account| {
                let existing = kept.iter_mut().find(|slot| {
                    slot.as_ref()
                        .is_some_and(|thread| thread.config == *account)
                });
                existing
                    .and_then(Option::take)
                    .unwrap_or_else(|| spawner.spawn(account))
            })
            .collect();
    }

    pub fn accounts(&self) -> Vec<(String, StartState)> {
        match &self.route {
            Route::Threads { threads, .. } => threads
                .iter()
                .map(|thread| (thread.config.name.clone(), thread.state.clone()))
                .collect(),
            Route::Detached { accounts, .. } => accounts
                .iter()
                .map(|name| (name.clone(), StartState::Running))
                .collect(),
        }
    }

    /// Queues the command and wakes the account's IDLE; false when the account is not running.
    pub fn send(&self, account: &str, request: RequestId, command: Command) -> bool {
        let job = Job { request, command };
        match &self.route {
            Route::Threads { threads, .. } => threads
                .iter()
                .find(|thread| thread.config.name == account)
                .and_then(|thread| thread.commands.as_ref().map(|commands| (thread, commands)))
                .is_some_and(|(thread, commands)| {
                    let sent = commands.send(job).is_ok();
                    thread.wake.store(true, Ordering::Release);
                    sent
                }),
            Route::Detached { sent, .. } => sent.send((account.to_string(), job)).is_ok(),
        }
    }

    /// Stops the threads and releases the locks, like dropping the engine.
    pub fn stop(self) {
        drop(self);
    }

    /// An engine with no threads whose commands arrive on the returned receiver, for front-end tests.
    pub fn detached(accounts: &[&str]) -> (Engine, Receiver<(String, Job)>) {
        let (sent, received) = mpsc::channel();
        let engine = Engine {
            route: Route::Detached {
                accounts: accounts.iter().map(|name| name.to_string()).collect(),
                sent,
            },
        };
        (engine, received)
    }
}

impl Drop for Engine {
    /// Signals every thread first so they stop together, then joins them and releases their locks.
    fn drop(&mut self) {
        if let Route::Threads { threads, .. } = &mut self.route {
            threads.iter().for_each(AccountThread::signal_stop);
            threads.iter_mut().for_each(AccountThread::join);
        }
    }
}

/// The held lock file, or the pid written by the process holding it.
pub fn lock_account(paths: &Paths, name: &str) -> io::Result<Result<File, Option<u32>>> {
    paths.ensure_account(name)?;
    lock_pid_file(&paths.account_dir(name).join("sync.lock"))
}

/// Takes an exclusive lock on `path` and writes our pid there, or reads the pid of the process holding it.
pub fn lock_pid_file(path: &Path) -> io::Result<Result<File, Option<u32>>> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => {
            file.set_len(0)?;
            write!(file, "{}", std::process::id())?;
            Ok(Ok(file))
        }
        Err(TryLockError::WouldBlock) => {
            let mut pid = String::new();
            file.read_to_string(&mut pid)?;
            Ok(Err(pid.trim().parse().ok()))
        }
        Err(TryLockError::Error(e)) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AccountConfig, PasswordSource};
    use crate::sync::Activity;

    fn offline_account(name: &str) -> AccountConfig {
        AccountConfig {
            name: name.into(),
            host: "127.0.0.1".into(),
            port: 1,
            username: "me@example.com".into(),
            password: PasswordSource::Command {
                command: "printf x".into(),
            },
            address: None,
            aliases: vec![],
            sync_interval_secs: 120,
            trash_retention_days: 30,
            notify: false,
            ca_file: None,
        }
    }

    /// A concurrent test spawning a password command briefly inherits a lock's descriptor until its exec closes it.
    fn lock_soon(paths: &Paths, name: &str) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while lock_account(paths, name).unwrap().is_err() {
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        true
    }

    #[test]
    fn an_account_locked_elsewhere_is_reported_and_not_started() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let held = lock_account(&paths, "work").unwrap().unwrap();
        let config = Config {
            accounts: vec![offline_account("work")],
            ..Default::default()
        };
        let (engine, _events) = Engine::start(&config, &paths);
        assert_eq!(
            engine.accounts(),
            [(
                "work".to_string(),
                StartState::Locked {
                    pid: Some(std::process::id())
                }
            )]
        );
        assert!(!engine.send("work", 1, Command::SyncNow));
        engine.stop();
        drop(held);
        assert!(lock_soon(&paths, "work"));
    }

    #[test]
    fn stop_returns_promptly_while_an_account_is_offline() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            accounts: vec![offline_account("work")],
            ..Default::default()
        };
        let (engine, events) = Engine::start(&config, &Paths::under(dir.path()));
        assert_eq!(
            engine.accounts(),
            [("work".to_string(), StartState::Running)]
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if let Event::Activity {
                activity: Activity::Offline { .. },
                ..
            } = events.recv_timeout(left).unwrap()
            {
                break;
            }
        }
        assert!(engine.send("work", 1, Command::SyncNow));
        assert!(!engine.send("nope", 2, Command::SyncNow));
        let started = std::time::Instant::now();
        engine.stop();
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn dropping_the_engine_stops_its_threads_before_releasing_the_locks() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let config = Config {
            accounts: vec![offline_account("work")],
            ..Default::default()
        };
        let (engine, events) = Engine::start(&config, &paths);
        let started = std::time::Instant::now();
        drop(engine);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        events.try_iter().for_each(drop);
        assert_eq!(
            events.try_recv(),
            Err(mpsc::TryRecvError::Disconnected),
            "every sync thread has ended"
        );
        assert!(lock_soon(&paths, "work"));
    }

    #[test]
    fn a_command_for_an_offline_account_fails_with_its_request_id() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            accounts: vec![offline_account("work")],
            ..Default::default()
        };
        let (engine, events) = Engine::start(&config, &Paths::under(dir.path()));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut failed = None;
        while failed.is_none() {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match events.recv_timeout(left).unwrap() {
                Event::Activity {
                    activity: Activity::Offline { .. },
                    ..
                } => assert!(engine.send("work", 4, Command::SyncNow)),
                Event::CommandFailed { request, .. } => failed = Some(request),
                _ => {}
            }
        }
        assert_eq!(failed, Some(4));
    }

    use crate::mail_ops::RecordingOps;
    use std::sync::Mutex;

    const WAIT: std::time::Duration = std::time::Duration::from_secs(5);

    /// A connector whose fake server idles until woken; every connection attempt is announced by account name.
    fn recording_connector() -> (Connector, Receiver<String>) {
        let (connected, announced) = mpsc::channel();
        let connected = Mutex::new(connected);
        let connector: Connector = Arc::new(move |account: &AccountConfig| {
            let _ = connected.lock().unwrap().send(account.name.clone());
            Ok(Box::new(RecordingOps::new().with_folder("INBOX", None)) as Box<dyn MailOps>)
        });
        (connector, announced)
    }

    fn config_of(names: &[&str]) -> Config {
        Config {
            accounts: names.iter().map(|name| offline_account(name)).collect(),
            ..Default::default()
        }
    }

    fn names_of(engine: &Engine) -> Vec<String> {
        engine
            .accounts()
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    #[test]
    fn the_engine_can_move_to_another_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<Engine>();
    }

    #[test]
    fn apply_config_restarts_only_changed_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let (connector, connected) = recording_connector();
        let mut config = config_of(&["a", "b"]);
        let (mut engine, _events) =
            Engine::start_with(&config, &Paths::under(dir.path()), connector);
        let mut first_connections = [
            connected.recv_timeout(WAIT).unwrap(),
            connected.recv_timeout(WAIT).unwrap(),
        ];
        first_connections.sort();
        assert_eq!(first_connections, ["a", "b"]);

        config.accounts[1].sync_interval_secs = 300;
        engine.apply_config(&config);
        assert_eq!(connected.recv_timeout(WAIT).unwrap(), "b");
        assert_eq!(names_of(&engine), ["a", "b"]);

        engine.apply_config(&config_of(&["a"]));
        assert_eq!(names_of(&engine), ["a"]);
        assert!(!engine.send("b", 1, Command::SyncNow));

        engine.apply_config(&config_of(&["a", "c"]));
        assert_eq!(connected.recv_timeout(WAIT).unwrap(), "c");
        assert_eq!(names_of(&engine), ["a", "c"]);
        assert_eq!(
            connected.try_recv(),
            Err(mpsc::TryRecvError::Empty),
            "a was never restarted"
        );
    }

    #[test]
    fn a_restarted_account_takes_its_lock_again() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let (connector, connected) = recording_connector();
        let mut config = config_of(&["a"]);
        let (mut engine, _events) = Engine::start_with(&config, &paths, connector);
        connected.recv_timeout(WAIT).unwrap();

        config.accounts[0].sync_interval_secs = 300;
        engine.apply_config(&config);
        connected.recv_timeout(WAIT).unwrap();
        assert_eq!(engine.accounts(), [("a".to_string(), StartState::Running)]);
        assert!(
            lock_account(&paths, "a").unwrap().is_err(),
            "the new thread holds the lock"
        );

        engine.apply_config(&config_of(&[]));
        assert!(lock_soon(&paths, "a"));
    }

    #[test]
    fn a_command_reaches_the_account_thread_through_start_with() {
        let dir = tempfile::tempdir().unwrap();
        let (connector, _connected) = recording_connector();
        let (engine, events) =
            Engine::start_with(&config_of(&["a"]), &Paths::under(dir.path()), connector);
        assert!(engine.send("a", 11, Command::SyncNow));
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if let Event::Synced { requests, .. } = events.recv_timeout(left).unwrap()
                && requests == vec![11]
            {
                break;
            }
        }
    }

    #[test]
    fn a_detached_engine_hands_commands_to_the_test() {
        let (engine, sent) = Engine::detached(&["work"]);
        assert_eq!(
            engine.accounts(),
            [("work".to_string(), StartState::Running)]
        );
        assert!(engine.send("work", 1, Command::SyncNow));
        assert_eq!(
            sent.try_recv().unwrap(),
            (
                "work".to_string(),
                Job {
                    request: 1,
                    command: Command::SyncNow
                }
            )
        );
    }
}
