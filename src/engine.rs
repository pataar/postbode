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
        }
    }

    fn signal_stop(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.wake.store(true, Ordering::Release);
    }

    fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct Spawner {
    paths: Paths,
    events: Sender<Event>,
    connect: Connector,
}

impl Spawner {
    /// Spawns the account's sync thread; a spawn failure is recorded as its state.
    fn spawn(&self, config: &AccountConfig) -> AccountThread {
        let name = &config.name;
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
                thread
            }
            Err(e) => AccountThread::not_started(config.clone(), StartState::Failed(e.to_string())),
        }
    }
}

pub struct Engine {
    spawner: Spawner,
    threads: Vec<AccountThread>,
}

impl Engine {
    /// Spawns a sync thread per account.
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
        (Engine { spawner, threads }, received)
    }

    /// Stops accounts that were removed or whose settings changed, starts new and changed ones, and leaves the rest.
    pub fn apply_config(&mut self, config: &Config) {
        let (kept, mut stale): (Vec<_>, Vec<_>) = std::mem::take(&mut self.threads)
            .into_iter()
            .partition(|thread| config.accounts.contains(&thread.config));
        stale.iter().for_each(AccountThread::signal_stop);
        stale.iter_mut().for_each(AccountThread::join);
        let mut kept: Vec<Option<AccountThread>> = kept.into_iter().map(Some).collect();
        let spawner = &self.spawner;
        self.threads = config
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
        self.threads
            .iter()
            .map(|thread| (thread.config.name.clone(), thread.state.clone()))
            .collect()
    }

    /// Queues the command and wakes the account's IDLE; false when the account is not running.
    pub fn send(&self, account: &str, request: RequestId, command: Command) -> bool {
        let job = Job { request, command };
        self.threads
            .iter()
            .find(|thread| thread.config.name == account)
            .and_then(|thread| thread.commands.as_ref().map(|commands| (thread, commands)))
            .is_some_and(|(thread, commands)| {
                let sent = commands.send(job).is_ok();
                thread.wake.store(true, Ordering::Release);
                sent
            })
    }

    /// Stops the threads, like dropping the engine.
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for Engine {
    /// Signals every thread first so they stop together, then joins them.
    fn drop(&mut self) {
        self.threads.iter().for_each(AccountThread::signal_stop);
        self.threads.iter_mut().for_each(AccountThread::join);
    }
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

    #[test]
    fn every_configured_account_starts() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let config = config_of(&["home", "work"]);
        let (first, _first_events) = Engine::start(&config, &paths);
        let (second, _second_events) = Engine::start(&config, &paths);
        for engine in [&first, &second] {
            assert_eq!(
                engine.accounts(),
                [
                    ("home".to_string(), StartState::Running),
                    ("work".to_string(), StartState::Running)
                ]
            );
        }
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
    fn dropping_the_engine_stops_its_threads() {
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
        drop(engine);
        assert_eq!(
            connected.try_iter().collect::<Vec<_>>(),
            Vec::<String>::new(),
            "a was never restarted, and no thread connects after the engine drops"
        );
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
}
