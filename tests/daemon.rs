//! The real binary as an auto-started daemon: one per home, idling out, replaced when stale or killed.

// Test code: unwrap, expect and panic are how a test fails.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use postbode::daemon::Client;
use postbode::paths::Paths;
use tempfile::TempDir;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const CONFIG: &str = r#"
[[accounts]]
name = "work"
host = "127.0.0.1"
port = 1
username = "me@example.com"
password = { command = "printf x" }
notify = false
"#;

/// The binary behind a script that sets the idle exit to 1 s; tests cannot set environment variables without unsafe.
fn exe() -> &'static Path {
    static SCRIPT: OnceLock<(TempDir, PathBuf)> = OnceLock::new();
    &SCRIPT
        .get_or_init(|| {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let script = dir.path().join("postbode");
            let text = format!(
                "#!/bin/sh\nPOSTBODE_IDLE_EXIT_SECS=1 exec '{}' \"$@\"\n",
                env!("CARGO_BIN_EXE_postbode")
            );
            std::fs::write(&script, text).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            (dir, script)
        })
        .1
}

fn home() -> (TempDir, Paths) {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::under(home.path());
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::write(paths.config_file(), CONFIG).unwrap();
    (home, paths)
}

fn start(paths: &Paths) -> Client {
    Client::connect_or_start_with(paths, exe(), VERSION).unwrap()
}

fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// Waits up to 10 s for `gone` to hold.
fn wait_until(what: &str, gone: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !gone() {
        assert!(Instant::now() < deadline, "{what} is still there");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn assert_gone(paths: &Paths, pid: u32) {
    wait_until(&format!("daemon {pid}"), || {
        !alive(pid) && !paths.daemon_socket().exists()
    });
}

fn stop(client: Client, paths: &Paths) {
    let pid = client.status().unwrap().pid;
    client.shutdown().unwrap();
    assert_gone(paths, pid);
}

#[test]
fn connect_or_start_starts_one_daemon_that_idles_out() {
    let (_home, paths) = home();
    let client = start(&paths);
    let status = client.status().unwrap();
    assert_ne!(status.pid, std::process::id());
    assert_eq!(status.version, VERSION);
    assert_eq!(status.accounts[0].name, "work");
    drop(client);
    assert_gone(&paths, status.pid);
}

#[test]
fn two_clients_starting_at_once_share_one_daemon() {
    let (_home, paths) = home();
    let starters: Vec<_> = (0..2)
        .map(|_| {
            let paths = paths.clone();
            std::thread::spawn(move || start(&paths))
        })
        .collect();
    let clients: Vec<Client> = starters.into_iter().map(|t| t.join().unwrap()).collect();
    let pids: Vec<u32> = clients.iter().map(|c| c.status().unwrap().pid).collect();
    assert_eq!(pids[0], pids[1]);
    let mut clients = clients.into_iter();
    let first = clients.next().unwrap();
    drop(clients);
    stop(first, &paths);
}

#[test]
fn an_older_daemon_is_replaced() {
    let (_home, paths) = home();
    let old = start(&paths);
    let old_pid = old.status().unwrap().pid;
    let new = Client::connect_or_start_with(&paths, exe(), "999.0.0").unwrap();
    let new_pid = new.status().unwrap().pid;
    assert_ne!(new_pid, old_pid);
    wait_until("the old daemon", || !alive(old_pid));
    drop(old);
    stop(new, &paths);
}

#[test]
fn a_newer_daemon_is_left_running_and_named() {
    let (_home, paths) = home();
    let daemon = start(&paths);
    let pid = daemon.status().unwrap().pid;
    let Err(error) = Client::connect_or_start_with(&paths, exe(), "0.0.0-old") else {
        panic!("an older client took over a newer daemon");
    };
    assert_eq!(
        error.to_string(),
        format!(
            "the daemon is version {VERSION}, newer than this postbode (0.0.0-old); restart this program"
        )
    );
    assert_eq!(daemon.status().unwrap().pid, pid);
    stop(daemon, &paths);
}

#[test]
fn a_killed_daemon_is_replaced_by_the_next_client() {
    let (_home, paths) = home();
    let first = start(&paths);
    let killed = first.status().unwrap().pid;
    let status = std::process::Command::new("kill")
        .args(["-9", &killed.to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    wait_until("the killed daemon", || !alive(killed));
    let error = first.status().unwrap_err();
    assert!(
        error.to_string().starts_with("the daemon stopped; see "),
        "{error}"
    );
    drop(first);
    let next = start(&paths);
    let pid = next.status().unwrap().pid;
    assert_ne!(pid, killed);
    stop(next, &paths);
}
