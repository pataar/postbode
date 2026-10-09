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
fn status_lists_each_account() {
    let daemon = TestDaemon::start();
    let status = status_of(&mut daemon.client());
    let accounts: Vec<String> = status
        .accounts
        .into_iter()
        .map(|AccountStatus { name, .. }| name)
        .collect();
    assert_eq!(accounts, ["work", "play"]);
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
fn every_store_is_migrated_before_the_first_hello() {
    // Offline accounts never reach their own store open, so only the daemon's start can have made these.
    let daemon = TestDaemon::start_with(options(offline_connector(), None));
    let _client = daemon.client();
    for name in ["play", "work"] {
        assert!(daemon.paths.mail_db(name).exists(), "{name} has no store");
    }
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
        status.accounts.iter().any(|account| account.name == "new")
    });
}

#[test]
fn a_daemon_without_accounts_serves_and_starts_one_added_later() {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::under(home.path());
    let daemon = TestDaemon::serve(home, paths, options(recording_connector(), None));
    assert!(status_of(&mut daemon.client()).accounts.is_empty());

    write_config(&daemon.paths);
    fs::File::options()
        .write(true)
        .open(daemon.paths.config_file())
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(10))
        .unwrap();
    status_until(&mut daemon.client(), |status| {
        status.accounts.iter().any(|account| account.name == "work")
    });
}

#[test]
fn a_rules_change_syncs_every_account() {
    let daemon = TestDaemon::start();
    let mut watcher = subscribed(daemon.client());
    let rules = daemon.paths.rules_file();
    fs::write(&rules, "").unwrap();
    fs::File::options()
        .write(true)
        .open(&rules)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(10))
        .unwrap();

    let mut synced = std::collections::BTreeSet::new();
    while synced.len() < 2 {
        if let Event::Synced { account, .. } = watcher.event_where(
            |event| matches!(event, Event::Synced { requests, .. } if requests.contains(&0)),
        ) {
            synced.insert(account);
        }
    }
    assert_eq!(synced, ["play".to_string(), "work".to_string()].into());
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
        errors: vec![],
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
    assert!(foreground.notify);
    let auto = Options::auto_started(Duration::from_secs(60));
    assert_eq!(auto.idle_exit, Some(Duration::from_secs(60)));
    assert!(auto.notify);
}

#[test]
fn reporting_errors_leaves_senders_and_subjects_out_of_the_log() {
    let new_mail = Event::NewMail {
        account: "work".into(),
        folder: "INBOX".into(),
        uid: 1,
        from: "bob@example.com".into(),
        subject: "hi".into(),
    };
    let synced = Event::Synced {
        account: "work".into(),
        new_messages: 1,
        actions: 0,
        requests: vec![],
        errors: vec![],
    };
    let error = Event::Error {
        account: "work".into(),
        message: "INBOX: gone".into(),
    };
    let failed = Event::CommandFailed {
        account: "work".into(),
        request: 1,
        message: "no".into(),
    };
    for event in [&new_mail, &synced, &error, &failed] {
        assert!(super::reported(event, true), "{event:?}");
    }
    assert!(!super::reported(&new_mail, false));
    assert!(!super::reported(&synced, false));
    assert!(super::reported(&error, false));
    assert!(super::reported(&failed, false));
}

mod client {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::Arc;
    use std::time::Duration;

    use super::super::Client;
    use super::super::client::{LazyClient, NO_REPLY, connect_as, reply_timeout};
    use super::super::test_support::{TestDaemon, WAIT, options, recording_connector};
    use super::super::wire::{self, DaemonMessage};
    use crate::engine::Connector;
    use crate::mail_ops::{MailOps, RecordingOps};
    use crate::paths::Paths;
    use crate::store::Store;
    use crate::sync::{Command, Event};

    /// Each connection takes a second, so commands queue behind it.
    fn slow_connector() -> Connector {
        Arc::new(|_| {
            std::thread::sleep(Duration::from_secs(1));
            Ok(Box::new(RecordingOps::new().with_folder("INBOX", None)) as Box<dyn MailOps>)
        })
    }

    /// A server holding one message in INBOX whose body only the server has.
    fn one_message_connector() -> Connector {
        Arc::new(|_| {
            let mut ops = RecordingOps::new().with_folder("INBOX", None);
            ops.add_mail(
                "INBOX",
                1,
                0,
                "From: bob@example.com\r\nSubject: hi\r\n\r\n",
                Some("From: bob@example.com\r\nSubject: hi\r\n\r\nthe body\r\n"),
            );
            Ok(Box::new(ops) as Box<dyn MailOps>)
        })
    }

    #[test]
    fn request_returns_the_completion_event() {
        let daemon = TestDaemon::start();
        let client = Client::connect(&daemon.paths).unwrap();
        let event = client.request("work", Command::SyncNow).unwrap();
        assert!(matches!(event, Event::Synced { .. }), "{event:?}");
    }

    #[test]
    fn request_errors_with_the_daemons_refusal() {
        let daemon = TestDaemon::start();
        let client = Client::connect(&daemon.paths).unwrap();
        let error = client.request("nope", Command::SyncNow).unwrap_err();
        assert!(error.to_string().contains("no account named"), "{error}");
    }

    #[test]
    fn a_timeout_says_the_command_may_still_run() {
        let daemon = TestDaemon::start_with(options(slow_connector(), None));
        let client = Client::connect(&daemon.paths).unwrap();
        let error = client
            .request_within("work", Command::SyncNow, Duration::from_millis(50))
            .unwrap_err();
        assert_eq!(error.to_string(), NO_REPLY);
        assert_eq!(
            NO_REPLY,
            "no reply from the daemon within 120 s; the command may still run, see `postbode log`"
        );
    }

    #[test]
    fn the_daemon_going_away_mid_request_names_the_log() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::under(home.path());
        std::fs::create_dir_all(&paths.state_dir).unwrap();
        let listener = UnixListener::bind(paths.daemon_socket()).unwrap();
        let daemon = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut lines = BufReader::new(stream.try_clone().unwrap()).lines();
            lines.next();
            let hello = DaemonMessage::Hello {
                protocol: wire::PROTOCOL,
                version: wire::VERSION.into(),
                pid: 1,
            };
            wire::write_line(&mut stream, &hello).unwrap();
            stream.flush().unwrap();
            lines.next();
        });
        let client = Client::connect(&paths).unwrap();
        let error = client.request("work", Command::SyncNow).unwrap_err();
        daemon.join().unwrap();
        assert_eq!(
            error.to_string(),
            format!("the daemon stopped; see {}", paths.daemon_log().display())
        );
        assert!(error.to_string().ends_with("daemon.log"));
    }

    #[test]
    fn send_refusals_arrive_as_command_failed_events() {
        let daemon = TestDaemon::start();
        let client = Client::connect(&daemon.paths).unwrap();
        let events = client.subscribe().unwrap();
        assert!(client.send("nope", Command::SyncNow));
        let refusal = std::iter::from_fn(|| events.recv_timeout(WAIT).ok())
            .find(|event| matches!(event, Event::CommandFailed { .. }));
        assert_eq!(
            refusal,
            Some(Event::CommandFailed {
                account: "nope".into(),
                request: 0,
                message: "no account named 'nope'".into(),
            })
        );
    }

    #[test]
    fn a_sends_own_completion_arrives_once_as_request_zero() {
        let daemon = TestDaemon::start_with(options(one_message_connector(), None));
        let client = Client::connect(&daemon.paths).unwrap();
        client.request("work", Command::SyncNow).unwrap();
        let events = client.subscribe().unwrap();
        let apply = Command::Apply {
            folder: "INBOX".into(),
            uids: vec![1],
            action: crate::rules::Action::MarkRead,
            by: "gui".into(),
        };
        assert!(client.send("work", apply));
        // A sync of the same account after the action ends the stream of events that could carry a copy; `play` syncs
        // on its own schedule, so its `Synced` says nothing.
        assert!(client.send("work", Command::SyncNow));
        let requests: Vec<u64> = std::iter::from_fn(|| events.recv_timeout(WAIT).ok())
            .take_while(
                |event| !matches!(event, Event::Synced { account, .. } if account == "work"),
            )
            .filter_map(|event| match event {
                Event::ActionDone { request, .. } => Some(request),
                _ => None,
            })
            .collect();
        assert_eq!(requests, [0]);
    }

    #[test]
    fn a_waking_subscription_wakes_after_each_event_and_at_hang_up() {
        let mut daemon = TestDaemon::start();
        let client = Client::connect(&daemon.paths).unwrap();
        let (woke, wakes) = std::sync::mpsc::channel();
        let events = client
            .subscribe_waking(move || {
                let _ = woke.send(());
            })
            .unwrap();
        client.request("work", Command::SyncNow).unwrap();
        client.shutdown().unwrap();
        daemon.finished().unwrap();
        let mut received = 0;
        loop {
            match events.recv_timeout(WAIT) {
                Ok(_) => received += 1,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(timeout) => panic!("the subscription never ended: {timeout}"),
            }
        }
        assert!(received > 0);
        assert_eq!(
            wakes.try_iter().count(),
            received + 1,
            "one per event and one at hang-up"
        );
    }

    #[test]
    fn long_commands_wait_for_their_reply_without_a_deadline() {
        let unbounded = [
            Command::ApplyRule { name: "r".into() },
            Command::FetchBodies { folder: None },
            Command::SyncNow,
        ];
        for command in unbounded {
            assert_eq!(reply_timeout(&command), None, "{command:?}");
        }
        let fetch = Command::FetchBody {
            folder: "INBOX".into(),
            uid: 1,
        };
        assert_eq!(reply_timeout(&fetch), Some(Duration::from_secs(120)));
    }

    #[test]
    fn raw_message_fetches_once_through_the_daemon() {
        let daemon = TestDaemon::start_with(options(one_message_connector(), None));
        let lazy = LazyClient::with_client(&daemon.paths, Client::connect(&daemon.paths).unwrap());
        let client = lazy.client().unwrap();
        client.request("work", Command::SyncNow).unwrap();
        let store = Store::open(&daemon.paths.mail_db("work")).unwrap();
        let message = store.message("INBOX", 1).unwrap().unwrap();
        assert_eq!(store.raw("INBOX", 1).unwrap(), None);
        let events = client.subscribe().unwrap();

        let expected = b"From: bob@example.com\r\nSubject: hi\r\n\r\nthe body\r\n".to_vec();
        assert_eq!(
            lazy.raw_message("work", &store, &message).unwrap(),
            expected
        );
        assert_eq!(
            lazy.raw_message("work", &store, &message).unwrap(),
            expected
        );

        // The broadcast of this reply's own event ends the count. Any `Synced` cannot: play's first sync may land
        // after the subscription and before the fetch's `BodyReady`.
        let answered = client.request("work", Command::SyncNow).unwrap();
        let mut fetches = 0;
        loop {
            let event = events.recv_timeout(WAIT).unwrap();
            if event == answered {
                break;
            }
            if matches!(event, Event::BodyReady { .. }) {
                fetches += 1;
            }
        }
        assert_eq!(fetches, 1);
    }

    #[test]
    fn connect_refuses_a_daemon_of_another_version() {
        let daemon = TestDaemon::start();
        let Err(error) = connect_as(&daemon.paths, "0.0.0", false) else {
            panic!("a daemon of another version was accepted");
        };
        let error = anyhow::Error::from(error).to_string();
        assert_eq!(
            error,
            format!(
                "daemon is version {} (protocol {}); this postbode is 0.0.0",
                wire::VERSION,
                wire::PROTOCOL
            )
        );
    }

    #[test]
    fn versions_compare_by_their_dotted_numbers_and_anything_else_is_older() {
        use super::super::client::is_newer;
        assert!(is_newer("0.3.0", "0.2.9"));
        assert!(is_newer("0.2.10", "0.2.9"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("0.2.0", "0.2.0-rc.1"));
        assert!(is_newer("0.1.0", "garbage"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.2.9", "0.3.0"));
        assert!(!is_newer("0.2.0-rc.1", "0.2.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn an_in_memory_client_hands_over_commands_and_takes_injected_events() {
        let (client, commands, inject) = Client::in_memory(&["work"]);
        let events = client.subscribe().unwrap();
        assert!(client.send("work", Command::SyncNow));
        assert_eq!(
            commands.try_recv().unwrap(),
            ("work".to_string(), Command::SyncNow)
        );
        let event = Event::Error {
            account: "work".into(),
            message: "x".into(),
        };
        inject.send(event.clone()).unwrap();
        assert_eq!(events.recv_timeout(WAIT).unwrap(), event);
        assert_eq!(
            client
                .request("work", Command::SyncNow)
                .unwrap_err()
                .to_string(),
            "in-memory client has no daemon"
        );
        let status = client.status().unwrap();
        assert_eq!(status.accounts.len(), 1);
        assert_eq!(status.accounts[0].name, "work");
    }

    #[test]
    fn a_dropped_client_leaves_the_daemon() {
        let daemon = TestDaemon::start_with(options(recording_connector(), None));
        let client = Client::connect(&daemon.paths).unwrap();
        let watcher = Client::connect(&daemon.paths).unwrap();
        assert_eq!(watcher.status().unwrap().clients, 2);
        drop(client);
        let deadline = std::time::Instant::now() + WAIT;
        while watcher.status().unwrap().clients != 1 {
            assert!(
                std::time::Instant::now() < deadline,
                "the client never left"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn new_mail_notifies_even_on_an_account_with_notify_off() {
    let shared = super::Shared {
        engine: std::sync::Mutex::new(None),
        hub: std::sync::Mutex::new(super::Hub::new()),
    };
    let (events, received) = std::sync::mpsc::channel();
    events
        .send(Event::NewMail {
            account: "work".into(),
            folder: "INBOX".into(),
            uid: 1,
            from: "bob@example.com".into(),
            subject: "hi".into(),
        })
        .unwrap();
    drop(events);
    let notified = std::sync::Mutex::new(Vec::new());
    super::route_events(&shared, received, false, |from, subject| {
        notified
            .lock()
            .unwrap()
            .push((from.to_string(), subject.to_string()))
    });
    assert_eq!(
        notified.into_inner().unwrap(),
        [("bob@example.com".to_string(), "hi".to_string())]
    );
}

/// Connections take `delay`, so an account thread is busy for that long and an engine stop waits for it.
fn slow_connector(delay: Duration) -> crate::engine::Connector {
    std::sync::Arc::new(move |_| {
        std::thread::sleep(delay);
        Ok(
            Box::new(crate::mail_ops::RecordingOps::new().with_folder("INBOX", None))
                as Box<dyn crate::mail_ops::MailOps>,
        )
    })
}

#[test]
fn a_command_while_stopping_says_the_daemon_is_stopping() {
    let shared = super::Shared {
        engine: std::sync::Mutex::new(None),
        hub: std::sync::Mutex::new(super::Hub::new()),
    };
    let (out, lines) = std::sync::mpsc::channel();
    let conn = super::ClientConn {
        out,
        subscribed: false,
        writer: std::thread::spawn(|| {}),
    };
    shared.hub.lock().unwrap().clients.insert(1, conn);
    super::submit(&shared, 1, 7, "work", Command::SyncNow);
    let line = lines.recv_timeout(WAIT).unwrap();
    assert_eq!(
        serde_json::from_str::<DaemonMessage>(&line).unwrap(),
        DaemonMessage::Reply {
            id: 7,
            outcome: Outcome::Error("the daemon is stopping".into()),
        }
    );
}

#[test]
fn connecting_while_the_daemon_stops_fails_at_once() {
    let mut daemon = TestDaemon::start_with(options(slow_connector(Duration::from_secs(3)), None));
    assert_eq!(
        daemon.client().request(ClientMessage::Shutdown { id: 1 }),
        Outcome::Ok(Payload::Done)
    );
    let deadline = Instant::now() + Duration::from_secs(1);
    while std::os::unix::net::UnixStream::connect(daemon.paths.daemon_socket()).is_ok() {
        assert!(
            Instant::now() < deadline,
            "the daemon still takes connections while it stops"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    daemon.finished().unwrap();
}

/// Opens a `gated_connector`'s gate when dropped, so a failing test never leaves a sync thread stuck in its connect.
struct Gate(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Gate {
    fn open(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        self.open();
    }
}

/// Connections announce their account and then wait until the gate opens, so an account thread stays in its connect
/// for exactly as long as the test wants.
fn gated_connector() -> (
    crate::engine::Connector,
    std::sync::mpsc::Receiver<String>,
    Gate,
) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    let open = Arc::new(AtomicBool::new(false));
    let (announce, connecting) = std::sync::mpsc::channel();
    let announce = Mutex::new(announce);
    let connector: crate::engine::Connector = Arc::new({
        let open = open.clone();
        move |account| {
            let _ = announce.lock().unwrap().send(account.name.clone());
            while !open.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(
                Box::new(crate::mail_ops::RecordingOps::new().with_folder("INBOX", None))
                    as Box<dyn crate::mail_ops::MailOps>,
            )
        }
    });
    (connector, connecting, Gate(open))
}

fn account_names(status: &Status) -> Vec<&str> {
    status
        .accounts
        .iter()
        .map(|account| account.name.as_str())
        .collect()
}

/// Every answer below arrives while work's old thread is still stuck in its connect, which only the test ends; a
/// daemon that waited for that thread would not answer at all, so no wall-clock budget is needed.
#[test]
fn a_config_change_never_stalls_the_daemon_while_an_old_account_thread_finishes() {
    let (connector, connecting, gate) = gated_connector();
    let daemon = TestDaemon::start_with(options(connector, None));
    // Bound after the daemon, so the gate opens before the daemon's drop waits for its threads.
    let gate = gate;
    let mut first = [
        connecting.recv_timeout(WAIT).unwrap(),
        connecting.recv_timeout(WAIT).unwrap(),
    ];
    first.sort();
    assert_eq!(first, ["play", "work"]);

    let config = daemon.paths.config_file();
    let text = fs::read_to_string(&config).unwrap().replacen(
        "notify = false",
        "notify = false\nsync_interval_secs = 300",
        1,
    );
    fs::write(&config, text).unwrap();
    fs::File::options()
        .write(true)
        .open(&config)
        .unwrap()
        .set_modified(SystemTime::now() + Duration::from_secs(10))
        .unwrap();
    status_until(&mut daemon.client(), |status| {
        account_names(status) == ["play"]
    });

    for id in 0..3 {
        assert_eq!(account_names(&status_of(&mut daemon.client())), ["play"]);
        assert_eq!(
            daemon
                .client()
                .request(command(id, "work", Command::SyncNow)),
            Outcome::Error("work is restarting".into())
        );
    }
    assert_eq!(
        connecting.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty),
        "work's new thread waits for its old one"
    );

    gate.open();
    assert_eq!(connecting.recv_timeout(WAIT).unwrap(), "work");
    status_until(&mut daemon.client(), |status| {
        account_names(status) == ["work", "play"]
    });
}

/// Waits until work's first thread is inside its connect: a thread not yet running when the config changes sees the
/// stop flag and ends at once, and work then restarts before the command arrives.
#[test]
fn a_command_for_an_account_between_threads_says_it_is_restarting() {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::under(home.path());
    write_config(&paths);
    let (connector, connecting, gate) = gated_connector();
    let config = crate::config::Config::parse(super::test_support::CONFIG).unwrap();
    let (mut engine, _events) = crate::engine::Engine::start(&config, &paths, connector);
    while connecting.recv_timeout(WAIT).unwrap() != "work" {}
    let mut changed = crate::config::Config::parse(super::test_support::CONFIG).unwrap();
    changed.accounts[0].sync_interval_secs = 300;
    engine.apply_config(&changed);
    let shared = super::Shared {
        engine: std::sync::Mutex::new(Some(engine)),
        hub: std::sync::Mutex::new(super::Hub::new()),
    };
    // Bound after the engine, so the gate opens before the engine's drop waits for its threads.
    let gate = gate;
    let (out, lines) = std::sync::mpsc::channel();
    let conn = super::ClientConn {
        out,
        subscribed: false,
        writer: std::thread::spawn(|| {}),
    };
    shared.hub.lock().unwrap().clients.insert(1, conn);
    super::submit(&shared, 1, 7, "work", Command::SyncNow);
    gate.open();
    let line = lines.recv_timeout(WAIT).unwrap();
    assert_eq!(
        serde_json::from_str::<DaemonMessage>(&line).unwrap(),
        DaemonMessage::Reply {
            id: 7,
            outcome: Outcome::Error("work is restarting".into()),
        }
    );
}
