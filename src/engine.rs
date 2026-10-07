//! Runs one sync thread per account and routes commands to them; the daemon runs it.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use crate::config::{AccountConfig, Config};
use crate::mail_ops::MailOps;
use crate::paths::Paths;
use crate::sync::{self, Command, Event, Job, RequestId, SyncError};

pub type Connector =
    Arc<dyn Fn(&AccountConfig) -> Result<Box<dyn MailOps>, SyncError> + Send + Sync>;

/// Connects to the account's real IMAP server.
pub fn imap_connector() -> Connector {
    Arc::new(|account| Ok(Box::new(sync::connect(account)?)))
}

struct AccountThread {
    config: AccountConfig,
    commands: Option<Sender<Job>>,
    wake: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl AccountThread {
    fn not_started(config: AccountConfig) -> AccountThread {
        AccountThread {
            config,
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
    /// Spawns the account's sync thread; one that cannot spawn is reported once and stays without a thread.
    fn spawn(&self, config: &AccountConfig) -> AccountThread {
        let name = &config.name;
        let (commands_tx, commands) = mpsc::channel();
        let mut thread = AccountThread::not_started(config.clone());
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
            }
            Err(e) => {
                let _ = self.events.send(Event::Error {
                    account: name.clone(),
                    message: format!("could not start its sync thread: {e}"),
                });
            }
        }
        thread
    }
}

pub struct Engine {
    spawner: Spawner,
    threads: Vec<AccountThread>,
    /// The configured accounts in order; one whose old thread is still in `stopping` starts once that thread ended.
    wanted: Vec<AccountConfig>,
    /// Signalled threads of removed or changed accounts, by account name, until they end.
    stopping: Vec<(String, JoinHandle<()>)>,
}

impl Engine {
    /// Spawns a sync thread per account.
    pub fn start(config: &Config, paths: &Paths, connect: Connector) -> (Engine, Receiver<Event>) {
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
            spawner,
            threads,
            wanted: config.accounts.clone(),
            stopping: Vec::new(),
        };
        (engine, received)
    }

    /// Signals the threads of removed and changed accounts, starts new ones, and leaves the rest; never waits. A changed
    /// account starts again from `start_ready` once its old thread ended.
    pub fn apply_config(&mut self, config: &Config) {
        let (kept, stale): (Vec<_>, Vec<_>) = std::mem::take(&mut self.threads)
            .into_iter()
            .partition(|thread| config.accounts.contains(&thread.config));
        for mut thread in stale {
            thread.signal_stop();
            if let Some(handle) = thread.handle.take() {
                self.stopping.push((thread.config.name, handle));
            }
        }
        self.threads = kept;
        self.wanted = config.accounts.clone();
        self.start_ready();
    }

    /// Starts each configured account that has no thread and no old one still ending; true once every one runs.
    pub fn start_ready(&mut self) -> bool {
        if self.threads.len() == self.wanted.len() {
            return true;
        }
        self.stopping.retain(|(_, handle)| !handle.is_finished());
        let mut idle = std::mem::take(&mut self.threads);
        let mut threads = Vec::new();
        for account in &self.wanted {
            if let Some(index) = idle.iter().position(|thread| thread.config == *account) {
                threads.push(idle.swap_remove(index));
            } else if !self.stopping.iter().any(|(name, _)| *name == account.name) {
                threads.push(self.spawner.spawn(account));
            }
        }
        self.threads = threads;
        self.threads.len() == self.wanted.len()
    }

    pub fn accounts(&self) -> Vec<String> {
        self.threads
            .iter()
            .map(|thread| thread.config.name.clone())
            .collect()
    }

    /// Whether the account is configured but has no thread yet, as while its old thread ends after a config change.
    pub fn is_restarting(&self, account: &str) -> bool {
        self.wanted.iter().any(|wanted| wanted.name == account)
            && !self
                .threads
                .iter()
                .any(|thread| thread.config.name == account)
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
}

impl Drop for Engine {
    /// Signals every thread first so they stop together, then joins them.
    fn drop(&mut self) {
        self.threads.iter().for_each(AccountThread::signal_stop);
        self.threads.iter_mut().for_each(AccountThread::join);
        for (_, handle) in self.stopping.drain(..) {
            let _ = handle.join();
        }
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
        let config = config_of(&["home", "work"]);
        let (engine, _events) = Engine::start(&config, &Paths::under(dir.path()), imap_connector());
        assert_eq!(engine.accounts(), ["home", "work"]);
    }

    #[test]
    fn stop_returns_promptly_while_an_account_is_offline() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            accounts: vec![offline_account("work")],
            ..Default::default()
        };
        let (engine, events) = Engine::start(&config, &Paths::under(dir.path()), imap_connector());
        assert_eq!(engine.accounts(), ["work"]);
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
        drop(engine);
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
        let (engine, events) = Engine::start(&config, &paths, imap_connector());
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
        let (engine, events) = Engine::start(&config, &Paths::under(dir.path()), imap_connector());
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

    /// Starts what `apply_config` left waiting, for up to 5 s.
    fn settle(engine: &mut Engine) {
        let deadline = std::time::Instant::now() + WAIT;
        while !engine.start_ready() {
            assert!(
                std::time::Instant::now() < deadline,
                "an old thread never ended"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn apply_config_never_waits_and_restarts_an_account_after_its_old_thread_ended() {
        let dir = tempfile::tempdir().unwrap();
        let (said, log) = mpsc::channel();
        let said = Mutex::new(said);
        let connector: Connector = Arc::new(move |account: &AccountConfig| {
            let say = |what: &str| {
                let line = format!("{what} {}", account.sync_interval_secs);
                let _ = said.lock().unwrap().send(line);
            };
            say("connecting");
            std::thread::sleep(std::time::Duration::from_secs(1));
            say("connected");
            Ok(Box::new(RecordingOps::new().with_folder("INBOX", None)) as Box<dyn MailOps>)
        });
        let mut config = config_of(&["a"]);
        let (mut engine, _events) = Engine::start(&config, &Paths::under(dir.path()), connector);
        assert_eq!(log.recv_timeout(WAIT).unwrap(), "connecting 120");
        config.accounts[0].sync_interval_secs = 300;
        let started = std::time::Instant::now();
        engine.apply_config(&config);
        assert!(started.elapsed() < std::time::Duration::from_millis(200));
        assert!(
            !engine.send("a", 1, Command::SyncNow),
            "a is between threads"
        );
        settle(&mut engine);
        assert_eq!(log.recv_timeout(WAIT).unwrap(), "connected 120");
        assert_eq!(log.recv_timeout(WAIT).unwrap(), "connecting 300");
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
        let (mut engine, _events) = Engine::start(&config, &Paths::under(dir.path()), connector);
        let mut first_connections = [
            connected.recv_timeout(WAIT).unwrap(),
            connected.recv_timeout(WAIT).unwrap(),
        ];
        first_connections.sort();
        assert_eq!(first_connections, ["a", "b"]);

        config.accounts[1].sync_interval_secs = 300;
        engine.apply_config(&config);
        settle(&mut engine);
        assert_eq!(connected.recv_timeout(WAIT).unwrap(), "b");
        assert_eq!(engine.accounts(), ["a", "b"]);

        engine.apply_config(&config_of(&["a"]));
        assert_eq!(engine.accounts(), ["a"]);
        assert!(!engine.send("b", 1, Command::SyncNow));

        engine.apply_config(&config_of(&["a", "c"]));
        assert_eq!(connected.recv_timeout(WAIT).unwrap(), "c");
        assert_eq!(engine.accounts(), ["a", "c"]);
        drop(engine);
        assert_eq!(
            connected.try_iter().collect::<Vec<_>>(),
            Vec::<String>::new(),
            "a was never restarted, and no thread connects after the engine drops"
        );
    }

    #[test]
    fn a_command_reaches_the_account_thread() {
        let dir = tempfile::tempdir().unwrap();
        let (connector, _connected) = recording_connector();
        let (engine, events) =
            Engine::start(&config_of(&["a"]), &Paths::under(dir.path()), connector);
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
