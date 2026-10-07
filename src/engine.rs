//! Runs one sync thread per account and routes commands to them; the entry point for front ends.
use std::collections::HashMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::paths::Paths;
use crate::sync::{self, Command, Event, Job, RequestId};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StartState {
    Running,
    /// Another process holds the account's lock; `pid` is what it wrote there.
    Locked {
        pid: Option<u32>,
    },
    Failed(String),
}

struct AccountThread {
    commands: Sender<Job>,
    wake: Arc<AtomicBool>,
}

enum Route {
    Threads(HashMap<String, AccountThread>),
    Detached(Sender<(String, Job)>),
}

pub struct Engine {
    accounts: Vec<(String, StartState)>,
    route: Route,
    shutdown: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
    _locks: Vec<File>,
}

impl Engine {
    /// Takes each account's lock and spawns its sync thread; accounts whose lock is held are not started.
    pub fn start(config: &Config, paths: &Paths) -> (Engine, Receiver<Event>) {
        let shutdown = Arc::new(AtomicBool::new(false));
        let (events, received) = mpsc::channel();
        let mut threads = HashMap::new();
        let mut engine = Engine {
            accounts: Vec::new(),
            route: Route::Threads(HashMap::new()),
            shutdown: shutdown.clone(),
            handles: Vec::new(),
            _locks: Vec::new(),
        };
        for account in &config.accounts {
            let name = account.name.clone();
            let lock = match lock_account(paths, &name) {
                Ok(Ok(file)) => file,
                Ok(Err(pid)) => {
                    engine.accounts.push((name, StartState::Locked { pid }));
                    continue;
                }
                Err(e) => {
                    engine
                        .accounts
                        .push((name, StartState::Failed(e.to_string())));
                    continue;
                }
            };
            let (commands_tx, commands) = mpsc::channel();
            let wake = Arc::new(AtomicBool::new(false));
            let spawned = std::thread::Builder::new()
                .name(format!("sync-{name}"))
                .spawn({
                    let (account, paths, events) = (account.clone(), paths.clone(), events.clone());
                    let (shutdown, wake) = (shutdown.clone(), wake.clone());
                    move || sync::run_loop(account, paths, events, shutdown, commands, wake)
                });
            let handle = match spawned {
                Ok(handle) => handle,
                Err(e) => {
                    engine
                        .accounts
                        .push((name, StartState::Failed(e.to_string())));
                    continue;
                }
            };
            engine._locks.push(lock);
            engine.handles.push(handle);
            threads.insert(
                name.clone(),
                AccountThread {
                    commands: commands_tx,
                    wake,
                },
            );
            engine.accounts.push((name, StartState::Running));
        }
        engine.route = Route::Threads(threads);
        (engine, received)
    }

    pub fn accounts(&self) -> &[(String, StartState)] {
        &self.accounts
    }

    /// Queues the command and wakes the account's IDLE; false when the account is not running.
    pub fn send(&self, account: &str, request: RequestId, command: Command) -> bool {
        let job = Job { request, command };
        match &self.route {
            Route::Threads(threads) => threads.get(account).is_some_and(|thread| {
                let sent = thread.commands.send(job).is_ok();
                thread.wake.store(true, Ordering::Release);
                sent
            }),
            Route::Detached(sent) => sent.send((account.to_string(), job)).is_ok(),
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
            accounts: accounts
                .iter()
                .map(|name| (name.to_string(), StartState::Running))
                .collect(),
            route: Route::Detached(sent),
            shutdown: Arc::new(AtomicBool::new(false)),
            handles: Vec::new(),
            _locks: Vec::new(),
        };
        (engine, received)
    }
}

impl Drop for Engine {
    /// Sets shutdown and every wake flag, then joins the threads; the locks are released after, with the fields.
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Route::Threads(threads) = &self.route {
            for thread in threads.values() {
                thread.wake.store(true, Ordering::Release);
            }
        }
        for handle in std::mem::take(&mut self.handles) {
            let _ = handle.join();
        }
    }
}

/// The held lock file, or the pid written by the process holding it.
pub fn lock_account(paths: &Paths, name: &str) -> io::Result<Result<File, Option<u32>>> {
    paths.ensure_account(name)?;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(paths.account_dir(name).join("sync.lock"))?;
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
