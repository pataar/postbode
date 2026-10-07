use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use super::test_support::{
    TestClient, TestDaemon, WAIT, offline_connector, options, recording_connector, subscribed,
    write_config,
};
use super::wire::{self, AccountStatus, ClientMessage, DaemonMessage, Outcome, Payload, Status};
use super::{Options, run};
use crate::engine::StartState;
use crate::paths::Paths;
use crate::sync::{Activity, Command, Event};

fn command(id: u64, account: &str, command: Command) -> ClientMessage {
    ClientMessage::Command {
        id,
        account: account.into(),
        command,
    }
}

fn status_of(client: &mut TestClient) -> Status {
    match client.request(ClientMessage::Status { id: 1 }) {
        Outcome::Ok(Payload::Status(status)) => status,
        other => panic!("expected a status, got {other:?}"),
    }
}

/// Asks for the status until it satisfies `wanted`, for up to 5 s.
fn status_until(client: &mut TestClient, wanted: impl Fn(&Status) -> bool) {
    let deadline = Instant::now() + WAIT;
    while !wanted(&status_of(client)) {
        assert!(Instant::now() < deadline, "the status never matched");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn mode_of(path: &std::path::Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn hello_is_answered_with_protocol_version_and_pid() {
    let daemon = TestDaemon::start();
    let mut client = TestClient::connect(&daemon.paths.daemon_socket());
    client.send(&ClientMessage::Hello {
        protocol: wire::PROTOCOL,
        version: wire::VERSION.into(),
    });
    assert_eq!(
        client.recv(),
        Some(DaemonMessage::Hello {
            protocol: 1,
            version: env!("CARGO_PKG_VERSION").into(),
            pid: std::process::id(),
        })
    );
}

#[test]
fn a_first_message_other_than_hello_closes_the_connection() {
    let daemon = TestDaemon::start();
    let mut client = TestClient::connect(&daemon.paths.daemon_socket());
    client.send(&ClientMessage::Status { id: 1 });
    assert_eq!(client.recv(), None);
}

#[test]
fn status_lists_each_account_with_its_state() {
    let daemon = TestDaemon::start();
    let status = status_of(&mut daemon.client());
    let accounts: Vec<(String, StartState)> = status
        .accounts
        .into_iter()
        .map(|AccountStatus { name, state, .. }| (name, state))
        .collect();
    assert_eq!(
        accounts,
        [
            ("work".to_string(), StartState::Running),
            ("play".to_string(), StartState::Running)
        ]
    );
    assert_eq!(status.pid, std::process::id());
    assert_eq!(status.version, wire::VERSION);
    assert_eq!(status.clients, 1);
}

#[test]
fn an_unknown_account_is_refused_at_once() {
    let daemon = TestDaemon::start();
    assert_eq!(
        daemon
            .client()
            .request(command(2, "nope", Command::SyncNow)),
        Outcome::Error("no account named 'nope'".into())
    );
}

#[test]
fn a_malformed_message_is_refused_by_its_id_or_closes_the_connection() {
    let daemon = TestDaemon::start();
    let mut client = daemon.client();
    client.send_raw(r#"{"command": {"id": 6, "account": "work"}}"#);
    match client.reply(6) {
        Outcome::Error(message) => assert!(message.starts_with("bad message: "), "{message}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    client.send_raw("not json");
    assert_eq!(client.recv(), None);
}

#[test]
fn a_command_reply_carries_the_clients_own_id() {
    let daemon = TestDaemon::start();
    let mut client = daemon.client();
    let outcome = client.request(command(42, "work", Command::SyncNow));
    assert!(
        matches!(outcome, Outcome::Ok(Payload::Event(Event::Synced { .. }))),
        "{outcome:?}"
    );

    let (mut first, mut second) = (daemon.client(), daemon.client());
    first.send(&command(1, "work", Command::SyncNow));
    second.send(&command(1, "work", Command::SyncNow));
    for client in [&mut first, &mut second] {
        let outcome = client.reply(1);
        assert!(
            matches!(outcome, Outcome::Ok(Payload::Event(Event::Synced { .. }))),
            "{outcome:?}"
        );
    }
}

#[test]
fn subscribers_see_another_clients_action() {
    let daemon = TestDaemon::start();
    let mut watcher = subscribed(daemon.client());
    let Outcome::Ok(Payload::Event(done)) =
        daemon
            .client()
            .request(command(5, "work", Command::SyncNow))
    else {
        panic!("the sync did not succeed");
    };
    assert_eq!(watcher.event_where(|event| *event == done), done);
}

#[test]
fn an_offline_account_refuses_commands_with_its_reason() {
    let daemon = TestDaemon::start_with(options(offline_connector(), None));
    let mut client = daemon.client();
    status_until(&mut client, |status| {
        status.accounts.iter().any(|account| {
            account.name == "work" && matches!(account.activity, Some(Activity::Offline { .. }))
        })
    });
    match client.request(command(3, "work", Command::SyncNow)) {
        Outcome::Error(message) => assert!(
            message.starts_with("work is offline (no route to host); retrying at "),
            "{message}"
        ),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_second_daemon_on_the_same_home_exits_already_running() {
    let daemon = TestDaemon::start();
    let error = run(&daemon.paths, options(recording_connector(), None)).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("already running (pid {})", std::process::id())
    );
    daemon.client();
}

#[test]
fn a_refused_second_daemon_leaves_the_log_alone() {
    let daemon = TestDaemon::start();
    let log = daemon.paths.daemon_log();
    fs::write(&log, vec![b'x'; 2 * 1_048_576]).unwrap();
    run(&daemon.paths, options(recording_connector(), None)).unwrap_err();
    assert_eq!(fs::metadata(&log).unwrap().len(), 2 * 1_048_576);
}

#[test]
fn a_failed_account_refuses_commands_with_its_reason() {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::under(home.path());
    write_config(&paths);
    fs::create_dir_all(paths.state_dir.join("accounts")).unwrap();
    fs::write(paths.account_dir("work"), "not a directory").unwrap();
    let daemon = TestDaemon::serve(home, paths, options(recording_connector(), None));
    let mut client = daemon.client();
    let state = status_of(&mut client)
        .accounts
        .into_iter()
        .find(|account| account.name == "work")
        .unwrap()
        .state;
    let StartState::Failed(reason) = state else {
        panic!("work started: {state:?}");
    };
    assert_eq!(
        client.request(command(4, "work", Command::SyncNow)),
        Outcome::Error(reason)
    );
}

#[test]
fn a_command_in_flight_at_shutdown_gets_exactly_one_reply() {
    let mut daemon = TestDaemon::start();
    let mut client = daemon.client();
    client.send(&command(1, "work", Command::SyncNow));
    client.send(&ClientMessage::Shutdown { id: 2 });
    let mut replies = Vec::new();
    while let Some(message) = client.recv() {
        if let DaemonMessage::Reply { id, outcome } = message {
            replies.push((id, outcome));
        }
    }
    daemon.finished().unwrap();
    replies.sort_by_key(|(id, _)| *id);
    let [(1, answer), (2, Outcome::Ok(Payload::Done))] = replies.as_slice() else {
        panic!("expected one reply to each request: {replies:?}");
    };
    assert!(
        matches!(answer, Outcome::Ok(Payload::Event(Event::Synced { .. })))
            || *answer == Outcome::Error("work stopped".into()),
        "{answer:?}"
    );
}

#[test]
fn a_client_leaving_with_requests_in_flight_leaves_the_daemon_serving() {
    let daemon = TestDaemon::start();
    let mut leaving = daemon.client();
    leaving.send(&command(1, "work", Command::SyncNow));
    leaving.send(&command(2, "play", Command::SyncNow));
    drop(leaving);
    let mut staying = daemon.client();
    let outcome = staying.request(command(1, "work", Command::SyncNow));
    assert!(
        matches!(outcome, Outcome::Ok(Payload::Event(Event::Synced { .. }))),
        "{outcome:?}"
    );
    status_until(&mut staying, |status| status.clients == 1);
}

#[test]
fn a_stale_socket_file_is_replaced() {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::under(home.path());
    write_config(&paths);
    fs::create_dir_all(&paths.state_dir).unwrap();
    drop(UnixListener::bind(paths.daemon_socket()).unwrap());
    assert!(paths.daemon_socket().exists());
    let daemon = TestDaemon::serve(home, paths, options(recording_connector(), None));
    daemon.client();
}

#[test]
fn the_socket_is_private() {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::under(home.path());
    write_config(&paths);
    fs::create_dir_all(&paths.state_dir).unwrap();
    fs::set_permissions(&paths.state_dir, fs::Permissions::from_mode(0o755)).unwrap();
    let daemon = TestDaemon::serve(home, paths, options(recording_connector(), None));
    assert_eq!(mode_of(&daemon.paths.daemon_socket()), 0o600);
    assert_eq!(mode_of(&daemon.paths.state_dir), 0o700);
}

#[test]
fn shutdown_replies_then_removes_the_socket() {
    let mut daemon = TestDaemon::start();
    let mut client = daemon.client();
    assert_eq!(
        client.request(ClientMessage::Shutdown { id: 9 }),
        Outcome::Ok(Payload::Done)
    );
    daemon.finished().unwrap();
    assert!(!daemon.paths.daemon_socket().exists());
    assert_eq!(client.recv(), None);
}

#[test]
fn idle_exit_stops_a_daemon_nobody_uses() {
    let mut daemon =
        TestDaemon::start_with(options(recording_connector(), Some(Duration::from_secs(1))));
    drop(daemon.client());
    daemon.finished().unwrap();
    assert!(!daemon.paths.daemon_socket().exists());
}

#[test]
fn a_config_change_starts_an_added_account() {
    let daemon = TestDaemon::start();
    let added = "\n[[accounts]]\nname = \"new\"\nhost = \"127.0.0.1\"\nusername = \"me@example.net\"\npassword = { command = \"printf x\" }\n";
    let config = daemon.paths.config_file();
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(added);
    fs::write(&config, text).unwrap();
    fs::File::options()
        .write(true)
        .open(&config)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(10))
        .unwrap();

    status_until(&mut daemon.client(), |status| {
        status
            .accounts
            .iter()
            .any(|account| account.name == "new" && account.state == StartState::Running)
    });
}

#[test]
fn a_socket_path_too_long_is_refused_naming_it() {
    let home = tempfile::tempdir().unwrap();
    let long: PathBuf = home.path().join("x".repeat(120));
    let paths = Paths::under(&long);
    let error = run(&paths, options(recording_connector(), None)).unwrap_err();
    let socket = paths.daemon_socket();
    assert!(
        error.to_string().contains(&socket.display().to_string()),
        "{error}"
    );
}

#[test]
fn wire_messages_round_trip() {
    let event = Event::Synced {
        account: "work".into(),
        new_messages: 2,
        actions: 1,
        requests: vec![3],
    };
    let clients = [
        ClientMessage::Hello {
            protocol: 1,
            version: "0.2.0".into(),
        },
        ClientMessage::Subscribe { id: 1 },
        ClientMessage::Status { id: 2 },
        ClientMessage::Shutdown { id: 3 },
        command(4, "work", Command::SyncNow),
    ];
    let daemons = [
        DaemonMessage::Hello {
            protocol: 1,
            version: "0.2.0".into(),
            pid: 42,
        },
        DaemonMessage::Reply {
            id: 1,
            outcome: Outcome::Ok(Payload::Done),
        },
        DaemonMessage::Reply {
            id: 2,
            outcome: Outcome::Ok(Payload::Event(event.clone())),
        },
        DaemonMessage::Reply {
            id: 3,
            outcome: Outcome::Ok(Payload::Status(Status {
                pid: 42,
                version: "0.2.0".into(),
                uptime_secs: 5,
                clients: 1,
                accounts: vec![AccountStatus {
                    name: "work".into(),
                    state: StartState::Failed("bad".into()),
                    activity: Some(Activity::Idle { since: 7 }),
                }],
            })),
        },
        DaemonMessage::Reply {
            id: 4,
            outcome: Outcome::Error("no".into()),
        },
        DaemonMessage::Event(event),
    ];
    for message in clients {
        let mut line = Vec::new();
        wire::write_line(&mut line, &message).unwrap();
        assert_eq!(line.last(), Some(&b'\n'));
        assert_eq!(
            serde_json::from_slice::<ClientMessage>(&line).unwrap(),
            message
        );
    }
    for message in daemons {
        let mut line = Vec::new();
        wire::write_line(&mut line, &message).unwrap();
        assert_eq!(
            serde_json::from_slice::<DaemonMessage>(&line).unwrap(),
            message
        );
    }
}

#[test]
fn options_presets_match_their_use() {
    let foreground = Options::foreground();
    assert_eq!(foreground.idle_exit, None);
    assert!(foreground.report && foreground.notify);
    let auto = Options::auto_started(Duration::from_secs(60));
    assert_eq!(auto.idle_exit, Some(Duration::from_secs(60)));
    assert!(auto.report && auto.notify);
}
