use super::*;
use crate::config::PasswordSource;
use crate::mail_ops::RecordingOps;
use crate::rules::{compile, parse};

const H: i64 = 3600;

fn account() -> AccountConfig {
    let password = PasswordSource::Keyring { keyring: true };
    AccountConfig::new("work", "h", "pieter@example.com", password)
}

fn headers(from: &str, subject: &str, id: &str) -> String {
    format!(
        "From: {from}\r\nTo: pieter@example.com\r\nSubject: {subject}\r\nMessage-ID: <{id}>\r\n\r\n"
    )
}

fn ops_with_inbox() -> RecordingOps {
    let mut ops = RecordingOps::new()
        .with_folder("INBOX", None)
        .with_folder("Trash", Some("Trash"));
    ops.add_mail(
        "INBOX",
        1,
        10 * H,
        &headers("alice@x", "hello", "m1@x"),
        Some("From: alice@x\r\n\r\nhi"),
    );
    ops.add_mail(
        "INBOX",
        2,
        11 * H,
        &headers("noreply@login.x", "Your sign-in code", "m2@x"),
        Some("From: noreply@login.x\r\n\r\ncode 1234"),
    );
    ops
}

fn rules_from(toml: &str, store: &Store, now: i64) -> Vec<CompiledRule> {
    let mut compiled = compile(&parse(toml).unwrap()).unwrap();
    for r in &mut compiled {
        r.first_seen_at = store
            .rule_first_seen(&r.rule.name, &r.rule.definition().unwrap(), now)
            .unwrap();
    }
    compiled
}

#[test]
fn thousands_groups_digits() {
    assert_eq!(thousands(0), "0");
    assert_eq!(thousands(999), "999");
    assert_eq!(thousands(1_000), "1,000");
    assert_eq!(thousands(48_213), "48,213");
    assert_eq!(thousands(1_234_567), "1,234,567");
}

#[test]
fn activity_text_matches_the_spec_table() {
    let folder = || "INBOX".to_string();
    let cases = [
        (Activity::Connecting, "connecting…".to_string()),
        (Activity::ListingFolders, "listing folders".into()),
        (
            Activity::SyncingFolder {
                folder: "Archive".into(),
                index: 4,
                of: 12,
            },
            "Archive (4/12)".into(),
        ),
        (
            Activity::FetchingHeaders {
                folder: folder(),
                done: 12_500,
                total: 48_213,
            },
            "INBOX headers 12,500 / 48,213".into(),
        ),
        (
            Activity::FetchingBodies {
                folder: folder(),
                done: 30,
                total: 210,
            },
            "INBOX bodies 30 / 210".into(),
        ),
        (
            Activity::RunningRules { folder: folder() },
            "running rules on INBOX".into(),
        ),
        (
            Activity::RunningCommand {
                what: "archiving 3 messages".into(),
            },
            "archiving 3 messages".into(),
        ),
        (
            Activity::Idle {
                since: 1_790_000_000,
            },
            format!("up to date · {}", clock(1_790_000_000)),
        ),
        (
            Activity::Offline {
                reason: "timeout".into(),
                retry_at: 1_790_000_300,
            },
            format!("offline (timeout) · retry {}", clock(1_790_000_300)),
        ),
        (
            Activity::NotRunning {
                reason: "out of threads".into(),
            },
            "not running (out of threads)".into(),
        ),
    ];
    for (activity, text) in cases {
        assert_eq!(activity.to_string(), text);
    }
}

#[test]
fn first_sync_inserts_folders_and_messages() {
    let mut ops = ops_with_inbox();
    let store = Store::open_in_memory().unwrap();
    let new = sync_all(&mut ops, &store).unwrap().0;
    assert_eq!(new.len(), 2);
    let inbox = store.folder("INBOX").unwrap().unwrap();
    assert_eq!((inbox.uidvalidity, inbox.last_uid), (1, 2));
    assert_eq!(
        store
            .folder("Trash")
            .unwrap()
            .unwrap()
            .special_use
            .as_deref(),
        Some("Trash")
    );
    let m = store.message("INBOX", 2).unwrap().unwrap();
    assert_eq!(m.subject.as_deref(), Some("Your sign-in code"));
    assert_eq!(m.thread_id, "m2@x");
    assert_eq!(m.internaldate, 11 * H);
}

#[test]
fn first_sync_does_not_notify() {
    let mut ops = ops_with_inbox();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let trash = Trash::new(dir.path().to_path_buf());
    let acc = account();
    let identity = acc.identity().unwrap();
    sync_all(&mut ops, &store).unwrap();
    let run = run_rules(
        &mut ops,
        &store,
        &trash,
        &[],
        &acc,
        &identity,
        Mode::Normal,
        12 * H,
    )
    .unwrap();
    assert_eq!(run.evaluated, 2);
    assert!(
        run.events.is_empty(),
        "mail already in a folder on its first sync never notifies"
    );
}

#[test]
fn first_mail_into_empty_tracked_folder_notifies() {
    let mut ops = RecordingOps::new().with_folder("INBOX", None);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let trash = Trash::new(dir.path().to_path_buf());
    let acc = account();
    let identity = acc.identity().unwrap();
    assert!(sync_all(&mut ops, &store).unwrap().0.is_empty());
    ops.add_mail("INBOX", 1, 12 * H, &headers("bob@x", "first", "f1@x"), None);
    sync_all(&mut ops, &store).unwrap();
    let run = run_rules(
        &mut ops,
        &store,
        &trash,
        &[],
        &acc,
        &identity,
        Mode::Normal,
        12 * H,
    )
    .unwrap();
    assert_eq!(run.events.len(), 1, "{:?}", run.events);
    assert!(matches!(&run.events[0], Event::NewMail { uid: 1, .. }));
}

#[test]
fn second_sync_only_fetches_new_and_updates_flags_and_removals() {
    let mut ops = ops_with_inbox();
    let store = Store::open_in_memory().unwrap();
    sync_all(&mut ops, &store).unwrap();
    ops.select("INBOX").unwrap();
    ops.add_flags(1, &["\\Seen"]).unwrap();
    ops.add_flags(2, &["\\Deleted"]).unwrap();
    ops.expunge(2).unwrap();
    ops.add_mail("INBOX", 3, 12 * H, &headers("bob@x", "new", "m3@x"), None);
    ops.calls.clear();
    let new = sync_all(&mut ops, &store).unwrap().0;
    assert_eq!(
        new,
        vec![NewMessageRef {
            folder: "INBOX".into(),
            uid: 3,
        }]
    );
    assert!(store.message("INBOX", 1).unwrap().unwrap().is_seen());
    assert_eq!(store.message("INBOX", 2).unwrap(), None);
    assert!(ops.calls.iter().any(|c| c == "search_uids INBOX 3"));
}

#[test]
fn sync_ignores_uids_below_last_uid() {
    let mut ops = ops_with_inbox();
    let store = Store::open_in_memory().unwrap();
    sync_all(&mut ops, &store).unwrap();
    let new = sync_all(&mut ops, &store).unwrap().0;
    assert!(
        new.is_empty(),
        "server answered 3:* with uid 2, which must not count as new"
    );
}

#[test]
fn uidvalidity_change_wipes_and_resyncs() {
    let mut ops = ops_with_inbox();
    let store = Store::open_in_memory().unwrap();
    sync_all(&mut ops, &store).unwrap();
    ops.uidvalidity.insert("INBOX".into(), 9);
    let new = sync_all(&mut ops, &store).unwrap().0;
    assert_eq!(store.folder("INBOX").unwrap().unwrap().uidvalidity, 9);
    assert_eq!(new.len(), 2);
    assert_eq!(store.messages_in_folder("INBOX").unwrap().len(), 2);
}

#[test]
fn placeholder_folder_adopts_server_uidvalidity_quietly() {
    let mut ops = ops_with_inbox().with_folder("Archive", Some("Archive"));
    ops.uidvalidity.insert("Archive".into(), 7);
    ops.add_mail(
        "Archive",
        1,
        10 * H,
        &headers("carol@x", "old", "a1@x"),
        None,
    );
    let store = Store::open_in_memory().unwrap();
    store
        .upsert_folder(&Folder {
            name: "Archive".into(),
            uidvalidity: 0,
            last_uid: 0,
            special_use: None,
            delimiter: None,
        })
        .unwrap();
    sync_all(&mut ops, &store).unwrap();
    assert_eq!(store.folder("Archive").unwrap().unwrap().uidvalidity, 7);
    assert_eq!(store.messages_in_folder("Archive").unwrap().len(), 1);
}

#[test]
fn clean_pass_forgets_folders_the_server_no_longer_lists() {
    let mut ops = ops_with_inbox().with_folder("[Gmail]/All Mail", None);
    ops.add_mail(
        "[Gmail]/All Mail",
        1,
        10 * H,
        &headers("alice@x", "hello", "m1@x"),
        None,
    );
    let store = Store::open_in_memory().unwrap();
    sync_all(&mut ops, &store).unwrap();
    assert_eq!(
        store.messages_in_folder("[Gmail]/All Mail").unwrap().len(),
        1
    );
    ops.folders.retain(|f| f.name != "[Gmail]/All Mail");
    sync_all(&mut ops, &store).unwrap();
    assert_eq!(store.folder("[Gmail]/All Mail").unwrap(), None);
    assert!(
        store
            .messages_in_folder("[Gmail]/All Mail")
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .search("hello", None, 10)
            .unwrap()
            .iter()
            .all(|m| m.folder == "INBOX")
    );
    assert_eq!(store.messages_in_folder("INBOX").unwrap().len(), 2);
}

#[test]
fn clean_pass_forgets_a_virtual_folder_synced_before_it_was_skipped() {
    let mut ops = ops_with_inbox().with_folder("[Gmail]/All Mail", Some("All"));
    let store = Store::open_in_memory().unwrap();
    store
        .upsert_folder(&Folder {
            name: "[Gmail]/All Mail".into(),
            uidvalidity: 1,
            last_uid: 5,
            special_use: Some("All".into()),
            delimiter: None,
        })
        .unwrap();
    sync_all(&mut ops, &store).unwrap();
    assert_eq!(store.folder("[Gmail]/All Mail").unwrap(), None);
}

#[test]
fn stopped_pass_keeps_folders_it_did_not_list() {
    let mut ops = ops_with_inbox().with_folder("Old", None);
    let store = Store::open_in_memory().unwrap();
    sync_all(&mut ops, &store).unwrap();
    ops.folders.retain(|f| f.name != "Old");
    let mut stop_at_second = |_: &mut dyn MailOps, activity| match activity {
        Activity::SyncingFolder { index: 2, .. } => Err(SyncError::Stopped),
        _ => Ok(false),
    };
    assert!(sync_all_with(&mut ops, &store, &mut stop_at_second).is_err());
    assert!(store.folder("Old").unwrap().is_some());
}

#[test]
fn unlisted_rule_move_placeholder_survives_a_pass() {
    let mut ops = ops_with_inbox();
    let store = Store::open_in_memory().unwrap();
    store
        .upsert_folder(&Folder {
            name: "Newsletters".into(),
            uidvalidity: 0,
            last_uid: 0,
            special_use: None,
            delimiter: None,
        })
        .unwrap();
    sync_all(&mut ops, &store).unwrap();
    assert!(store.folder("Newsletters").unwrap().is_some());
}

#[test]
fn rules_run_deletes_old_seen_code_and_notifies_untouched_new_mail() {
    let mut ops = RecordingOps::new()
        .with_folder("INBOX", None)
        .with_folder("Trash", Some("Trash"));
    ops.add_mail(
        "INBOX",
        2,
        11 * H,
        &headers("noreply@login.x", "Your sign-in code", "m2@x"),
        Some("From: noreply@login.x\r\n\r\ncode 1234"),
    );
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let trash = Trash::new(dir.path().to_path_buf());
    let acc = account();
    let identity = acc.identity().unwrap();
    let toml = "[[rules]]\nname = \"purge\"\nmatch.from = { contains = \"noreply@\" }\nmatch.older_than = \"1h\"\nmatch.seen = true\nactions = [\"delete\"]\n";
    let rules = rules_from(toml, &store, 0);
    sync_all(&mut ops, &store).unwrap();
    ops.select("INBOX").unwrap();
    ops.add_flags(2, &["\\Seen"]).unwrap();
    ops.add_mail(
        "INBOX",
        3,
        12 * H,
        &headers("alice@x", "hello", "m1@x"),
        None,
    );
    sync_all(&mut ops, &store).unwrap();
    let run = run_rules(
        &mut ops,
        &store,
        &trash,
        &rules,
        &acc,
        &identity,
        Mode::Normal,
        13 * H,
    )
    .unwrap();
    assert_eq!(run.actions, 1);
    assert_eq!(store.message("INBOX", 2).unwrap(), None);
    assert_eq!(trash.list().unwrap().len(), 1);
    assert_eq!(run.events.len(), 1);
    assert!(matches!(&run.events[0], Event::NewMail { uid: 3, from, .. } if from == "alice@x"));
}

#[test]
fn resync_after_uidvalidity_change_does_not_notify() {
    let mut ops = ops_with_inbox();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let trash = Trash::new(dir.path().to_path_buf());
    let acc = account();
    let identity = acc.identity().unwrap();
    sync_all(&mut ops, &store).unwrap();
    ops.add_mail("INBOX", 3, 12 * H, &headers("bob@x", "new", "m3@x"), None);
    sync_all(&mut ops, &store).unwrap();
    let run = run_rules(
        &mut ops,
        &store,
        &trash,
        &[],
        &acc,
        &identity,
        Mode::Normal,
        12 * H,
    )
    .unwrap();
    assert_eq!(
        run.events.len(),
        1,
        "mail arriving after the first sync notifies"
    );
    ops.uidvalidity.insert("INBOX".into(), 9);
    let resync = sync_all(&mut ops, &store).unwrap().0;
    let run = run_rules(
        &mut ops,
        &store,
        &trash,
        &[],
        &acc,
        &identity,
        Mode::Normal,
        12 * H,
    )
    .unwrap();
    assert_eq!(resync.len(), 3);
    assert!(run.events.is_empty(), "a UIDVALIDITY resync never notifies");
}

#[test]
fn body_rule_fetches_bodies_for_new_messages() {
    let mut ops = RecordingOps::new()
        .with_folder("INBOX", None)
        .with_folder("Trash", Some("Trash"));
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let trash = Trash::new(dir.path().to_path_buf());
    let acc = account();
    let identity = acc.identity().unwrap();
    let toml =
        "[[rules]]\nname = \"codes\"\nmatch.body = { contains = \"code\" }\nactions = [\"flag\"]\n";
    let rules = rules_from(toml, &store, 0);
    sync_all(&mut ops, &store).unwrap();
    ops.add_mail(
        "INBOX",
        2,
        11 * H,
        &headers("noreply@login.x", "Your sign-in code", "m2@x"),
        Some("From: noreply@login.x\r\n\r\ncode 1234"),
    );
    sync_all(&mut ops, &store).unwrap();
    let run = run_rules(
        &mut ops,
        &store,
        &trash,
        &rules,
        &acc,
        &identity,
        Mode::Normal,
        12 * H,
    )
    .unwrap();
    assert_eq!(run.actions, 1);
    let m = store.message("INBOX", 2).unwrap().unwrap();
    assert!(m.body_text.is_some());
    assert!(m.flags.contains("\\Flagged"));
}

#[test]
fn a_body_rule_added_later_skips_mail_its_folder_already_had() {
    let mut ops = RecordingOps::new()
        .with_folder("INBOX", None)
        .with_folder("Lists", None);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let trash = Trash::new(dir.path().to_path_buf());
    let acc = account();
    let identity = acc.identity().unwrap();
    let pass = |ops: &mut RecordingOps, rules: &[CompiledRule]| {
        sync_all(ops, &store).unwrap();
        let run = run_rules(
            ops,
            &store,
            &trash,
            rules,
            &acc,
            &identity,
            Mode::Normal,
            12 * H,
        );
        run.unwrap();
    };
    pass(&mut ops, &[]);
    let raw = "From: a@x\r\n\r\nyour code 1";
    ops.add_mail(
        "Lists",
        1,
        10 * H,
        &headers("a@x", "code", "l1@x"),
        Some(raw),
    );
    pass(&mut ops, &[]);
    let rules = rules_from(
        "[[rules]]\nname = \"codes\"\nfolder = \"Lists\"\nmatch.body = { contains = \"code\" }\nactions = [\"flag\"]\n",
        &store,
        0,
    );
    ops.add_mail(
        "Lists",
        2,
        11 * H,
        &headers("a@x", "code", "l2@x"),
        Some(raw),
    );
    ops.calls.clear();
    pass(&mut ops, &rules);
    let fetched: Vec<&String> = ops
        .calls
        .iter()
        .filter(|c| c.starts_with("fetch_raw"))
        .collect();
    assert_eq!(fetched, ["fetch_raw Lists 2"]);
    assert!(
        store
            .message("Lists", 2)
            .unwrap()
            .unwrap()
            .flags
            .contains("\\Flagged")
    );
}

#[test]
fn body_rule_skips_body_fetch_on_first_sync() {
    let mut ops = ops_with_inbox();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let trash = Trash::new(dir.path().to_path_buf());
    let acc = account();
    let identity = acc.identity().unwrap();
    let toml =
        "[[rules]]\nname = \"codes\"\nmatch.body = { contains = \"code\" }\nactions = [\"flag\"]\n";
    let rules = rules_from(toml, &store, 0);
    sync_all(&mut ops, &store).unwrap();
    run_rules(
        &mut ops,
        &store,
        &trash,
        &rules,
        &acc,
        &identity,
        Mode::Normal,
        12 * H,
    )
    .unwrap();
    assert!(
        !ops.calls.iter().any(|c| c.starts_with("fetch_raw")),
        "{:?}",
        ops.calls
    );
}

#[test]
fn disabled_rule_starts_its_clock_when_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    let store = Store::open_in_memory().unwrap();
    let rule = |enabled: bool| {
        format!(
            "[[rules]]\nname = \"codes\"\nenabled = {enabled}\nmatch.seen = true\nactions = [\"delete\"]\n"
        )
    };
    std::fs::write(&path, rule(false)).unwrap();
    load_rules_for(&store, &path, 100).unwrap();
    std::fs::write(&path, rule(true)).unwrap();
    assert_eq!(
        load_rules_for(&store, &path, 500).unwrap()[0].first_seen_at,
        500
    );
    std::fs::write(&path, rule(false)).unwrap();
    load_rules_for(&store, &path, 600).unwrap();
    std::fs::write(&path, rule(true)).unwrap();
    assert_eq!(
        load_rules_for(&store, &path, 900).unwrap()[0].first_seen_at,
        900,
        "re-enabling restarts the clock"
    );
}

#[test]
fn editing_a_rule_in_place_restarts_its_clock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.toml");
    let store = Store::open_in_memory().unwrap();
    let rule = |from: &str| {
        format!(
            "[[rules]]\nname = \"purge\"\nmatch.from = {{ contains = \"{from}\" }}\nactions = [\"delete\"]\n"
        )
    };
    std::fs::write(&path, rule("noreply@shop.x")).unwrap();
    load_rules_for(&store, &path, 100).unwrap();
    assert_eq!(
        load_rules_for(&store, &path, 200).unwrap()[0].first_seen_at,
        100
    );
    std::fs::write(&path, rule("@")).unwrap();
    assert_eq!(
        load_rules_for(&store, &path, 300).unwrap()[0].first_seen_at,
        300,
        "a widened rule must not act on mail that arrived before the edit"
    );
}

#[test]
fn removed_rule_forgets_its_first_seen() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let path = dir.path().join("rules.toml");
    let rule = |name: &str| {
        format!("[[rules]]\nname = \"{name}\"\nmatch.seen = true\nactions = [\"flag\"]\n")
    };
    std::fs::write(&path, rule("a") + &rule("b")).unwrap();
    load_rules_for(&store, &path, 100).unwrap();
    std::fs::write(&path, rule("b")).unwrap();
    load_rules_for(&store, &path, 200).unwrap();
    std::fs::write(&path, rule("a") + &rule("b")).unwrap();
    let rules = load_rules_for(&store, &path, 300).unwrap();
    let first_seen: Vec<(&str, i64)> = rules
        .iter()
        .map(|r| (r.rule.name.as_str(), r.first_seen_at))
        .collect();
    assert_eq!(first_seen, [("a", 300), ("b", 100)]);
}

#[test]
fn invalid_rules_file_keeps_previous_rules() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let path = dir.path().join("rules.toml");
    std::fs::write(
        &path,
        "[[rules]]\nname = \"a\"\nmatch.seen = true\nactions = [\"flag\"]\n",
    )
    .unwrap();
    let good = load_rules_for(&store, &path, 100).unwrap();
    assert_eq!(good.len(), 1);
    assert_eq!(good[0].first_seen_at, 100);
    std::fs::write(
        &path,
        "[[rules]]\nname = \"a\"\nmatch.from = { regex = \"(\" }\nactions = [\"flag\"]\n",
    )
    .unwrap();
    let (events, reported) = std::sync::mpsc::channel();
    let account = account();
    let mut rules = LoadedRules {
        last_good: Some(good),
        ..LoadedRules::default()
    };
    rules.reload(&store, &path, &account, &events);
    rules.reload(&store, &path, &account, &events);
    let kept = rules.last_good.unwrap();
    assert_eq!(kept.len(), 1);
    assert!(
        kept[0].rule.matches.seen.is_some(),
        "previous rules kept after a bad edit"
    );
    let errors: Vec<Event> = reported.try_iter().collect();
    assert!(
        matches!(errors.as_slice(), [Event::Error { message, .. }] if message.ends_with("; keeping the previous rules")),
        "{errors:?}"
    );
}

#[test]
fn run_loop_stops_on_shutdown_after_idle() {
    let mut ops = ops_with_inbox();
    ops.idle_outcomes.push_back(IdleOutcome::NewMail);
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    paths.ensure_account("work").unwrap();
    sync_all(&mut ops, &Store::open(&paths.mail_db("work")).unwrap()).unwrap();
    ops.add_mail("INBOX", 3, 12 * H, &headers("bob@x", "new", "m3@x"), None);
    let (tx, rx) = std::sync::mpsc::channel();
    let shutdown = Arc::new(AtomicBool::new(false));
    ops.shutdown_when_idle_empty = Some(shutdown.clone());
    let stop = shutdown.clone();
    let mut connects = 0;
    run_loop_with(
        account(),
        paths,
        tx,
        shutdown,
        std::sync::mpsc::channel::<Job>().1,
        Arc::new(AtomicBool::new(false)),
        || {
            connects += 1;
            Ok(Box::new(std::mem::take(&mut ops)) as Box<dyn MailOps>)
        },
        |_| stop.store(true, Ordering::Relaxed),
    );
    assert_eq!(connects, 1);
    let events: Vec<Event> = rx.try_iter().collect();
    assert!(events.iter().any(|e| matches!(e, Event::Synced { .. })));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::NewMail { uid: 3, .. }))
    );
    assert!(
        !events.iter().any(|e| matches!(e, Event::Error { .. })),
        "{events:?}"
    );
}

#[test]
fn an_invalid_rules_file_syncs_without_rules_and_says_so_once() {
    let mut ops = ops_with_inbox();
    ops.idle_outcomes.push_back(IdleOutcome::NewMail);
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    paths.ensure_account("work").unwrap();
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::write(
        paths.rules_file(),
        "[[rules]]\nname = \"x\"\nmatch.from = { regex = \"(\" }\nactions = [\"delete\"]\n",
    )
    .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let shutdown = Arc::new(AtomicBool::new(false));
    ops.shutdown_when_idle_empty = Some(shutdown.clone());
    let stop = shutdown.clone();
    run_loop_with(
        account(),
        paths,
        tx,
        shutdown,
        std::sync::mpsc::channel::<Job>().1,
        Arc::new(AtomicBool::new(false)),
        || Ok(Box::new(std::mem::take(&mut ops)) as Box<dyn MailOps>),
        |_| stop.store(true, Ordering::Relaxed),
    );
    let events: Vec<Event> = rx.try_iter().collect();
    let errors: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            Event::Error { message, .. } => Some(message),
            _ => None,
        })
        .collect();
    assert_eq!(errors.len(), 1, "{events:?}");
    assert!(errors[0].contains("rule 'x'"), "{errors:?}");
    let passes = events
        .iter()
        .filter(|e| matches!(e, Event::Synced { .. }))
        .count();
    assert_eq!(passes, 2, "{events:?}");
}

#[test]
fn a_panicking_session_fails_its_requests_and_goes_offline() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let (queue, commands) = std::sync::mpsc::channel();
    queue
        .send(Job {
            request: 5,
            command: Command::SyncNow,
        })
        .unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let stop = shutdown.clone();
    run_loop_with(
        account(),
        Paths::under(dir.path()),
        tx,
        shutdown,
        commands,
        Arc::new(AtomicBool::new(false)),
        || -> Result<Box<dyn MailOps>, SyncError> { panic!("a bug") },
        |_| stop.store(true, Ordering::Relaxed),
    );
    let events: Vec<Event> = rx.try_iter().collect();
    assert!(
        events.iter().any(|e| matches!(e, Event::Activity { activity: Activity::Offline { reason, .. }, .. } if reason == "crashed: a bug")),
        "{events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, Event::CommandFailed { request: 5, message, .. } if message.starts_with("work is offline (crashed: a bug)"))),
        "{events:?}"
    );
}

#[test]
fn a_command_that_panics_still_gets_its_answer() {
    let mut ops = ops_with_inbox();
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    sync_all(&mut ops, &Store::open_account(&paths, "work").unwrap()).unwrap();
    ops.panic_on_add_flags = true;
    let (queue, commands) = std::sync::mpsc::channel();
    queue.send(mark_read(1)).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let shutdown = Arc::new(AtomicBool::new(false));
    let stop = shutdown.clone();
    run_loop_with(
        account(),
        paths,
        tx,
        shutdown,
        commands,
        Arc::new(AtomicBool::new(false)),
        || Ok(Box::new(std::mem::take(&mut ops)) as Box<dyn MailOps>),
        |_| stop.store(true, Ordering::Relaxed),
    );
    let events: Vec<Event> = rx.try_iter().collect();
    let answers: Vec<&Event> = events
        .iter()
        .filter(|e| e.request_ids().contains(&1))
        .collect();
    assert!(
        matches!(answers.as_slice(), [Event::CommandFailed { message, .. }] if message.starts_with("work is offline (crashed: add_flags panicked)")),
        "{events:?}"
    );
}

const INVALID_RULES: &str =
    "[[rules]]\nname = \"x\"\nmatch.from = { regex = \"(\" }\nactions = [\"delete\"]\n";

#[test]
fn mail_that_arrives_while_the_rules_file_is_invalid_notifies_once_across_restarts_and_gets_the_rules_once_fixed()
 {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let acct = account();
    let mut state = AccountSync::open(&acct, &paths).unwrap();
    let mut ops = ops_with_inbox();
    sync_all(&mut ops, &state.store).unwrap();
    // The rule ran before the bad edit, so its clock already started.
    state.store.restart_rule_clock("codes", 0).unwrap();
    write_rules(&paths.config_dir, INVALID_RULES);
    ops.add_mail(
        "INBOX",
        3,
        12 * H,
        &headers("bob@x", "your code", "m3@x"),
        Some("From: bob@x\r\n\r\ncode 99"),
    );
    let commands = std::sync::mpsc::channel::<Job>().1;
    let stop = AtomicBool::new(false);
    let (tx, rx) = std::sync::mpsc::channel();
    // Each pass starts from nothing carried over, like the first pass of a restarted daemon or account thread.
    for _ in 0..2 {
        state
            .pass(
                &mut ops,
                true,
                &mut Carried::default(),
                &tx,
                &commands,
                &stop,
            )
            .unwrap();
    }
    write_rules(
        &paths.config_dir,
        "[[rules]]\nname = \"codes\"\nmatch.body = { contains = \"code\" }\nactions = [\"flag\"]\n",
    );
    state
        .pass(
            &mut ops,
            true,
            &mut Carried::default(),
            &tx,
            &commands,
            &stop,
        )
        .unwrap();
    let message = state.store.message("INBOX", 3).unwrap().unwrap();
    assert!(message.flags.contains("\\Flagged"), "{message:?}");
    let events: Vec<Event> = rx.try_iter().collect();
    assert_eq!(notified(&events), [3], "{events:?}");
}

#[test]
fn the_last_good_rules_survive_a_reconnect() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    // The rule ran before, so its clock covers the mail already on the server.
    let store = Store::open_account(&paths, "work").unwrap();
    store.restart_rule_clock("codes", 0).unwrap();
    write_rules(&paths.config_dir, FLAG_NOREPLY);
    let shutdown = Arc::new(AtomicBool::new(false));
    let mut dropped = ops_with_inbox();
    dropped.fail_fetch_after = Some(0);
    let mut reconnected = ops_with_inbox();
    reconnected.shutdown_when_idle_empty = Some(shutdown.clone());
    let mut servers = vec![reconnected, dropped];
    let config_dir = paths.config_dir.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    run_loop_with(
        account(),
        paths,
        tx,
        shutdown,
        std::sync::mpsc::channel::<Job>().1,
        Arc::new(AtomicBool::new(false)),
        || Ok(Box::new(servers.pop().unwrap()) as Box<dyn MailOps>),
        |_| {
            write_rules(&config_dir, INVALID_RULES);
        },
    );
    let events: Vec<Event> = rx.try_iter().collect();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Synced { actions: 1, .. })),
        "{events:?}"
    );
    assert!(
        store
            .message("INBOX", 2)
            .unwrap()
            .unwrap()
            .flags
            .contains("\\Flagged")
    );
}

#[test]
fn failed_apply_skips_message_and_continues() {
    let mut ops = RecordingOps::new().with_folder("INBOX", None);
    ops.add_mail(
        "INBOX",
        1,
        10 * H,
        &headers("noreply@a.x", "one", "n1@x"),
        None,
    );
    ops.add_mail(
        "INBOX",
        2,
        11 * H,
        &headers("noreply@b.x", "two", "n2@x"),
        Some("From: noreply@b.x\r\n\r\ntwo"),
    );
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let trash = Trash::new(dir.path().to_path_buf());
    let acc = account();
    let identity = acc.identity().unwrap();
    let rules = rules_from(
        "[[rules]]\nname = \"purge\"\nmatch.from = { contains = \"noreply@\" }\nactions = [\"delete\"]\n",
        &store,
        0,
    );
    sync_all(&mut ops, &store).unwrap();
    let run = run_rules(
        &mut ops,
        &store,
        &trash,
        &rules,
        &acc,
        &identity,
        Mode::Normal,
        12 * H,
    )
    .unwrap();
    assert!(
        store.message("INBOX", 1).unwrap().is_some(),
        "message without raw is left alone"
    );
    assert_eq!(
        store.message("INBOX", 2).unwrap(),
        None,
        "later message still deleted"
    );
    assert_eq!(run.events.len(), 1);
    assert!(
        matches!(&run.events[0], Event::Error { message, .. } if message.contains("INBOX/1")),
        "{:?}",
        run.events
    );
}

#[test]
fn missing_rule_folder_is_skipped() {
    let mut ops = ops_with_inbox();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let trash = Trash::new(dir.path().to_path_buf());
    let acc = account();
    let identity = acc.identity().unwrap();
    store
        .upsert_folder(&Folder {
            name: "Gone".into(),
            uidvalidity: 3,
            last_uid: 0,
            special_use: None,
            delimiter: None,
        })
        .unwrap();
    let rules = rules_from(
        "[[rules]]\nname = \"old\"\nfolder = \"Gone\"\nmatch.seen = true\nactions = [\"flag\"]\n",
        &store,
        0,
    );
    // Only INBOX: a full pass would forget Gone, and a rule on a folder the store does not know is skipped silently.
    let inbox = ops.folders[0].clone();
    sync_folder(&mut ops, &store, &inbox).unwrap();
    let run = run_rules(
        &mut ops,
        &store,
        &trash,
        &rules,
        &acc,
        &identity,
        Mode::Normal,
        12 * H,
    )
    .unwrap();
    assert_eq!(run.evaluated, 2, "INBOX still processed");
    assert_eq!(run.events.len(), 1);
    assert!(
        matches!(&run.events[0], Event::Error { message, .. } if message.contains("Gone")),
        "{:?}",
        run.events
    );
}

#[test]
fn rules_run_selects_each_folder_once() {
    let mut ops = ops_with_inbox();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let trash = Trash::new(dir.path().to_path_buf());
    let acc = account();
    let identity = acc.identity().unwrap();
    let rules = rules_from(
        "[[rules]]\nname = \"all\"\nmatch.seen = false\nactions = [\"flag\"]\n",
        &store,
        0,
    );
    sync_all(&mut ops, &store).unwrap();
    ops.calls.clear();
    let run = run_rules(
        &mut ops,
        &store,
        &trash,
        &rules,
        &acc,
        &identity,
        Mode::Normal,
        12 * H,
    )
    .unwrap();
    assert_eq!(run.actions, 2);
    assert_eq!(
        ops.calls.iter().filter(|c| c.starts_with("select")).count(),
        1,
        "{:?}",
        ops.calls
    );
}

#[test]
fn special_use_falls_back_to_folder_name() {
    let mut ops = RecordingOps::new()
        .with_folder("INBOX", None)
        .with_folder("archive", None)
        .with_folder("Deleted Items", Some("Trash"))
        .with_folder("Trash", None);
    ops.add_mail("INBOX", 1, 10 * H, &headers("a@x", "old", "a1@x"), None);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let trash = Trash::new(dir.path().to_path_buf());
    let acc = account();
    let identity = acc.identity().unwrap();
    let rules = rules_from(
        "[[rules]]\nname = \"a\"\nmatch.from = { contains = \"a@x\" }\nactions = [\"archive\"]\n",
        &store,
        0,
    );
    sync_all(&mut ops, &store).unwrap();
    let special_use = |name: &str| store.folder(name).unwrap().unwrap().special_use;
    assert_eq!(special_use("archive").as_deref(), Some("Archive"));
    assert_eq!(special_use("Trash"), None, "a marked Trash folder wins");
    let run = run_rules(
        &mut ops,
        &store,
        &trash,
        &rules,
        &acc,
        &identity,
        Mode::Normal,
        12 * H,
    )
    .unwrap();
    assert!(run.events.is_empty(), "{:?}", run.events);
    assert_eq!(ops.mail["archive"].len(), 1);
}

#[test]
fn flag_rule_acts_and_logs_once_across_passes() {
    let mut ops = RecordingOps::new().with_folder("INBOX", None);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let trash = Trash::new(dir.path().to_path_buf());
    let acc = account();
    let identity = acc.identity().unwrap();
    let rules = rules_from(
        "[[rules]]\nname = \"boss\"\nmatch.from = { contains = \"boss@\" }\nactions = [\"flag\"]\n",
        &store,
        0,
    );
    sync_all(&mut ops, &store).unwrap();
    ops.add_mail("INBOX", 1, 10 * H, &headers("boss@x", "hi", "b1@x"), None);
    let mut actions = 0;
    for _ in 0..5 {
        sync_all(&mut ops, &store).unwrap();
        actions += run_rules(
            &mut ops,
            &store,
            &trash,
            &rules,
            &acc,
            &identity,
            Mode::Normal,
            12 * H,
        )
        .unwrap()
        .actions;
    }
    assert_eq!(store.log(100).unwrap().len(), 1);
    assert_eq!(
        ops.calls
            .iter()
            .filter(|c| c.starts_with("add_flags"))
            .count(),
        1
    );
    assert_eq!(actions, 1);
}

#[test]
fn failing_folder_is_reported_and_others_still_sync() {
    let mut ops = ops_with_inbox();
    ops.folders.insert(
        0,
        RemoteFolder {
            name: "Bogus".into(),
            special_use: None,
            delimiter: None,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let (tx, rx) = std::sync::mpsc::channel();
    let shutdown = Arc::new(AtomicBool::new(false));
    ops.shutdown_when_idle_empty = Some(shutdown.clone());
    run_loop_with(
        account(),
        paths.clone(),
        tx,
        shutdown,
        std::sync::mpsc::channel::<Job>().1,
        Arc::new(AtomicBool::new(false)),
        || Ok(Box::new(std::mem::take(&mut ops)) as Box<dyn MailOps>),
        |_| panic!("the session must not fail"),
    );
    let store = Store::open(&paths.mail_db("work")).unwrap();
    assert_eq!(store.messages_in_folder("INBOX").unwrap().len(), 2);
    let events: Vec<Event> = rx.try_iter().collect();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Error { message, .. } if message.contains("Bogus"))),
        "{events:?}"
    );
    assert!(events.iter().any(|e| matches!(e, Event::Synced { .. })));
}

#[test]
fn a_lost_connection_ends_a_full_pass_instead_of_skipping_every_folder() {
    let mut ops = ops_with_inbox();
    ops.fail_fetch_after = Some(0);
    let store = Store::open_in_memory().unwrap();
    let result = sync_all(&mut ops, &store);
    assert!(
        matches!(result, Err(SyncError::Mail(MailError::Io(_)))),
        "{result:?}"
    );
    assert!(
        !ops.calls.iter().any(|c| c == "select Trash"),
        "{:?}",
        ops.calls
    );
}

#[test]
fn interrupted_first_sync_stays_initial_on_retry() {
    let mut ops = RecordingOps::new().with_folder("INBOX", None);
    for uid in 1..=3 {
        ops.add_mail(
            "INBOX",
            uid,
            10 * H,
            &headers("a@x", "old", &format!("o{uid}@x")),
            None,
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let trash = Trash::new(dir.path().to_path_buf());
    let acc = account();
    let identity = acc.identity().unwrap();
    ops.fail_fetch_after = Some(0);
    let inbox = RemoteFolder {
        name: "INBOX".into(),
        special_use: None,
        delimiter: None,
    };
    assert!(sync_folder(&mut ops, &store, &inbox).is_err());
    assert_eq!(store.folder("INBOX").unwrap().unwrap().last_uid, 0);
    assert_eq!(store.message_count("INBOX").unwrap(), 0);
    ops.fail_fetch_after = None;
    sync_folder(&mut ops, &store, &inbox).unwrap();
    let run = run_rules(
        &mut ops,
        &store,
        &trash,
        &[],
        &acc,
        &identity,
        Mode::Normal,
        12 * H,
    )
    .unwrap();
    assert!(run.events.is_empty(), "{:?}", run.events);
}

/// Runs no rules over the store and returns the uids that notify.
fn notify_uids(ops: &mut RecordingOps, store: &Store) -> Vec<u32> {
    let dir = tempfile::tempdir().unwrap();
    let trash = Trash::new(dir.path().to_path_buf());
    let acc = account();
    let identity = acc.identity().unwrap();
    let run = run_rules(
        ops,
        store,
        &trash,
        &[],
        &acc,
        &identity,
        Mode::Normal,
        12 * H,
    );
    notified(&run.unwrap().events)
}

fn inbox() -> RemoteFolder {
    RemoteFolder {
        name: "INBOX".into(),
        special_use: None,
        delimiter: None,
    }
}

#[test]
fn sparse_uids_are_fetched_in_chunks_of_500_messages() {
    let mut ops = RecordingOps::new().with_folder("INBOX", None);
    for i in 1..=1200u32 {
        ops.add_mail(
            "INBOX",
            i * 1000,
            10 * H,
            &headers("a@x", "old", &format!("o{i}@x")),
            None,
        );
    }
    let store = Store::open_in_memory().unwrap();
    let new = sync_folder(&mut ops, &store, &inbox()).unwrap();
    assert_eq!(new.len(), 1200);
    let fetches: Vec<&String> = ops
        .calls
        .iter()
        .filter(|c| c.starts_with("fetch_envelopes"))
        .collect();
    assert_eq!(
        fetches,
        [
            "fetch_envelopes INBOX 1000 500000",
            "fetch_envelopes INBOX 501000 1000000",
            "fetch_envelopes INBOX 1001000 1200000",
        ]
    );
    assert_eq!(store.folder("INBOX").unwrap().unwrap().last_uid, 1_200_000);
}

#[test]
fn interrupted_chunked_first_sync_resumes_and_stays_silent() {
    let mut ops = RecordingOps::new().with_folder("INBOX", None);
    for uid in 1..=1200 {
        ops.add_mail(
            "INBOX",
            uid,
            10 * H,
            &headers("a@x", "old", &format!("o{uid}@x")),
            None,
        );
    }
    let store = Store::open_in_memory().unwrap();
    ops.fail_fetch_after = Some(1);
    assert!(sync_folder(&mut ops, &store, &inbox()).is_err());
    assert_eq!(store.folder("INBOX").unwrap().unwrap().last_uid, 500);
    assert_eq!(store.message_count("INBOX").unwrap(), 500);
    assert_eq!(store.initial_uid_next("INBOX").unwrap(), 1201);

    ops.fail_fetch_after = None;
    ops.add_mail("INBOX", 1201, 11 * H, &headers("b@x", "new", "n@x"), None);
    let new = sync_folder(&mut ops, &store, &inbox()).unwrap();
    assert_eq!(new.len(), 701);
    assert_eq!(notify_uids(&mut ops, &store), [1201]);
}

#[test]
fn a_folder_that_starts_empty_notifies_its_first_mail() {
    let mut ops = RecordingOps::new().with_folder("INBOX", None);
    let store = Store::open_in_memory().unwrap();
    assert!(sync_folder(&mut ops, &store, &inbox()).unwrap().is_empty());
    assert_eq!(store.initial_uid_next("INBOX").unwrap(), 0);
    ops.add_mail("INBOX", 1, 10 * H, &headers("a@x", "first", "f@x"), None);
    sync_folder(&mut ops, &store, &inbox()).unwrap();
    assert_eq!(notify_uids(&mut ops, &store), [1]);
}

#[test]
fn checkpoints_report_progress_in_order() {
    let mut ops = ops_with_inbox();
    let store = Store::open_in_memory().unwrap();
    let mut seen = Vec::new();
    sync_all_with(&mut ops, &store, &mut |_, activity| {
        seen.push(activity);
        Ok(false)
    })
    .unwrap();
    let folder = |name: &str, index| Activity::SyncingFolder {
        folder: name.into(),
        index,
        of: 2,
    };
    let headers = |done| Activity::FetchingHeaders {
        folder: "INBOX".into(),
        done,
        total: 2,
    };
    assert_eq!(
        seen,
        [
            Activity::ListingFolders,
            folder("INBOX", 1),
            headers(0),
            headers(2),
            folder("Trash", 2),
        ]
    );
}

#[test]
fn a_checkpoint_that_used_the_connection_makes_the_chunk_loop_reselect() {
    let mut ops = ops_with_inbox();
    let store = Store::open_in_memory().unwrap();
    sync_folder_with(&mut ops, &store, &inbox(), &mut |ops, activity| {
        if matches!(activity, Activity::FetchingHeaders { done: 0, .. }) {
            ops.select("Trash")?;
            return Ok(true);
        }
        Ok(false)
    })
    .unwrap();
    let tail: Vec<&String> = ops.calls.iter().rev().take(3).rev().collect();
    assert_eq!(
        tail,
        ["select Trash", "select INBOX", "fetch_envelopes INBOX 1 2"]
    );
    assert_eq!(store.message_count("INBOX").unwrap(), 2);
}

#[test]
fn a_uidvalidity_change_between_chunks_stops_the_folder_until_the_next_pass_resets_it() {
    let mut ops = RecordingOps::new().with_folder("INBOX", None);
    for uid in 1..=600 {
        let id = format!("o{uid}@x");
        ops.add_mail("INBOX", uid, 10 * H, &headers("a@x", "old", &id), None);
    }
    let next_uidvalidity = Arc::new(std::sync::atomic::AtomicU32::new(0));
    ops.next_inbox_uidvalidity = Some(next_uidvalidity.clone());
    let store = Store::open_in_memory().unwrap();
    let mut changed = false;
    sync_folder_with(&mut ops, &store, &inbox(), &mut |_, activity| {
        if !changed && matches!(activity, Activity::FetchingHeaders { done: 500, .. }) {
            changed = true;
            next_uidvalidity.store(9, Ordering::Release);
            return Ok(true);
        }
        Ok(false)
    })
    .unwrap();
    let fetches = |ops: &RecordingOps| {
        let calls = ops.calls.iter();
        calls.filter(|c| c.starts_with("fetch_envelopes")).count()
    };
    assert_eq!(fetches(&ops), 1, "{:?}", ops.calls);
    assert_eq!(store.message_count("INBOX").unwrap(), 500);

    sync_folder(&mut ops, &store, &inbox()).unwrap();
    let folder = store.folder("INBOX").unwrap().unwrap();
    assert_eq!((folder.uidvalidity, folder.last_uid), (9, 600));
    assert_eq!(store.message_count("INBOX").unwrap(), 600);
    assert_eq!(store.initial_uid_next("INBOX").unwrap(), 601);
}

#[test]
fn a_stopping_checkpoint_ends_the_pass() {
    let mut ops = ops_with_inbox();
    let store = Store::open_in_memory().unwrap();
    let result = sync_all_with(&mut ops, &store, &mut |_, _| Err(SyncError::Stopped));
    assert!(matches!(result, Err(SyncError::Stopped)));
}

#[test]
fn rules_report_running_and_body_progress() {
    let mut ops = ops_with_inbox();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let trash = Trash::new(dir.path().to_path_buf());
    let acc = account();
    let identity = acc.identity().unwrap();
    sync_all(&mut ops, &store).unwrap();
    ops.add_mail(
        "INBOX",
        3,
        12 * H,
        &headers("bob@x", "code", "m3@x"),
        Some("From: bob@x\r\n\r\nyour code 99"),
    );
    sync_all(&mut ops, &store).unwrap();
    let rules = rules_from(
        "[[rules]]\nname = \"codes\"\nmatch.body = { contains = \"code\" }\nactions = [\"flag\"]\n",
        &store,
        0,
    );
    let mut seen = Vec::new();
    run_rules_with(
        &mut ops,
        &store,
        &trash,
        &rules,
        &acc,
        &identity,
        Mode::Normal,
        13 * H,
        &mut |a| seen.push(a),
    )
    .unwrap();
    assert!(seen.contains(&Activity::RunningRules {
        folder: "INBOX".into()
    }));
    assert!(seen.contains(&Activity::FetchingBodies {
        folder: "INBOX".into(),
        done: 1,
        total: 1
    }));
}

#[test]
fn changed_uidvalidity_blocks_rules_on_stale_rows() {
    let mut ops = RecordingOps::new()
        .with_folder("INBOX", None)
        .with_folder("Codes", None);
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_in_memory().unwrap();
    let trash = Trash::new(dir.path().to_path_buf());
    let acc = account();
    let identity = acc.identity().unwrap();
    let rules = rules_from(
        "[[rules]]\nname = \"p\"\nfolder = \"Codes\"\nmatch.subject = { contains = \"code\" }\nactions = [\"delete\"]\n",
        &store,
        0,
    );
    sync_all(&mut ops, &store).unwrap();
    ops.add_mail(
        "Codes",
        1,
        10 * H,
        &headers("n@x", "your code", "c1@x"),
        Some("Subject: your code\r\n\r\nOLD"),
    );
    sync_all(&mut ops, &store).unwrap();
    // Folder recreated elsewhere: new UIDVALIDITY and uid 1 is now an unrelated message; only INBOX is synced.
    ops.uidvalidity.insert("Codes".into(), 9);
    ops.mail.get_mut("Codes").unwrap().clear();
    ops.raw.clear();
    ops.add_mail(
        "Codes",
        1,
        11 * H,
        &headers("boss@x", "contract", "k1@x"),
        Some("Subject: contract\r\n\r\nIMPORTANT"),
    );
    let inbox = RemoteFolder {
        name: "INBOX".into(),
        special_use: None,
        delimiter: None,
    };
    sync_folder(&mut ops, &store, &inbox).unwrap();
    let run = run_rules(
        &mut ops,
        &store,
        &trash,
        &rules,
        &acc,
        &identity,
        Mode::Normal,
        12 * H,
    )
    .unwrap();
    assert!(!ops.calls.iter().any(|c| c.starts_with("expunge")));
    assert_eq!(ops.mail["Codes"].len(), 1, "server message intact");
    assert!(trash.list().unwrap().is_empty());
    assert!(
        run.events.iter().any(
            |e| matches!(e, Event::Error { message, .. } if message.contains("resync needed"))
        ),
        "{:?}",
        run.events
    );
}

#[test]
fn backoff_grows_caps_and_resets_after_a_full_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let (tx, _rx) = std::sync::mpsc::channel();
    let shutdown = Arc::new(AtomicBool::new(false));
    let stop = shutdown.clone();
    let mut connects = 0;
    let mut sleeps = Vec::new();
    run_loop_with(
        account(),
        paths,
        tx,
        shutdown,
        std::sync::mpsc::channel::<Job>().1,
        Arc::new(AtomicBool::new(false)),
        || {
            connects += 1;
            match connects {
                1..=8 => Err(SyncError::Mail(MailError::Connect("refused".into()))),
                // No folders: the pass completes, then selecting INBOX before IDLE fails.
                9 => Ok(Box::new(RecordingOps::new()) as Box<dyn MailOps>),
                // INBOX is selectable but not listed, so the full pass completes; after an IDLE wake the INBOX-only pass fails.
                _ => {
                    let mut ops = RecordingOps::new().with_folder("INBOX", None);
                    ops.folders.clear();
                    ops.add_mail("INBOX", 1, 0, "Subject: a\r\n\r\n", None);
                    ops.fail_fetch_after = Some(0);
                    ops.idle_outcomes.push_back(IdleOutcome::NewMail);
                    Ok(Box::new(ops) as Box<dyn MailOps>)
                }
            }
        },
        |delay| {
            sleeps.push(delay.as_secs());
            if sleeps.len() == 10 {
                stop.store(true, Ordering::Relaxed);
            }
        },
    );
    assert_eq!(connects, 10);
    assert_eq!(sleeps, vec![5, 10, 20, 40, 80, 160, 300, 300, 300, 5]);
}

/// Receives until `wanted` matches, keeping every event in `log`.
fn wait_for(
    rx: &std::sync::mpsc::Receiver<Event>,
    log: &mut Vec<Event>,
    wanted: impl Fn(&Event) -> bool,
) -> Event {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let event = rx
            .recv_timeout(left)
            .unwrap_or_else(|e| panic!("no matching event within 5 s: {e}; got {log:?}"));
        log.push(event.clone());
        if wanted(&event) {
            return event;
        }
    }
}

#[test]
fn a_command_sent_during_idle_runs_on_the_same_session() {
    let ops = ops_with_inbox();
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let (tx, rx) = std::sync::mpsc::channel();
    let (commands_tx, commands) = std::sync::mpsc::channel();
    let shutdown = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(AtomicBool::new(false));
    let worker = {
        let (shutdown, wake) = (shutdown.clone(), wake.clone());
        let mut ops = Some(ops);
        std::thread::spawn(move || {
            let mut connects = 0;
            run_loop_with(
                account(),
                paths,
                tx,
                shutdown,
                commands,
                wake,
                || {
                    connects += 1;
                    Ok(Box::new(ops.take().expect("one connection")) as Box<dyn MailOps>)
                },
                |_| panic!("the session must not fail"),
            );
            connects
        })
    };
    let mut log = Vec::new();
    wait_for(&rx, &mut log, |e| {
        matches!(
            e,
            Event::Activity {
                activity: Activity::Idle { .. },
                ..
            }
        )
    });
    commands_tx.send(mark_read(1)).unwrap();
    wake.store(true, Ordering::Relaxed);
    let done = wait_for(&rx, &mut log, |e| matches!(e, Event::ActionDone { .. }));
    assert!(matches!(done, Event::ActionDone { results, .. } if results == [(1, Ok(1))]));
    wait_for(&rx, &mut log, |e| {
        matches!(
            e,
            Event::Activity {
                activity: Activity::Idle { .. },
                ..
            }
        )
    });
    shutdown.store(true, Ordering::Relaxed);
    wake.store(true, Ordering::Relaxed);
    assert_eq!(worker.join().unwrap(), 1);
    log.extend(rx.try_iter());
    let passes = log
        .iter()
        .filter(|e| matches!(e, Event::Synced { .. }))
        .count();
    assert_eq!(
        passes, 1,
        "only the first pass; the command ran without one: {log:?}"
    );
}

#[test]
fn a_command_sent_while_offline_runs_after_reconnecting() {
    let mut ops = ops_with_inbox();
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    paths.ensure_account("work").unwrap();
    sync_all(&mut ops, &Store::open(&paths.mail_db("work")).unwrap()).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let (commands_tx, commands) = std::sync::mpsc::channel();
    let shutdown = Arc::new(AtomicBool::new(false));
    ops.shutdown_when_idle_empty = Some(shutdown.clone());
    let mut connects = 0;
    run_loop_with(
        account(),
        paths,
        tx,
        shutdown,
        commands,
        Arc::new(AtomicBool::new(false)),
        || {
            connects += 1;
            if connects == 1 {
                return Err(SyncError::Mail(MailError::Connect("refused".into())));
            }
            Ok(Box::new(std::mem::take(&mut ops)) as Box<dyn MailOps>)
        },
        |_| commands_tx.send(mark_read(1)).unwrap(),
    );
    assert_eq!(connects, 2);
    let events: Vec<Event> = rx.try_iter().collect();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::ActionDone { results, .. } if results == &[(1, Ok(1))])),
        "{events:?}"
    );
}

#[test]
fn sync_now_during_a_pass_runs_another_full_pass() {
    let mut ops = ops_with_inbox();
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let (tx, rx) = std::sync::mpsc::channel();
    let (commands_tx, commands) = std::sync::mpsc::channel();
    commands_tx.send(sync_now(1)).unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    ops.shutdown_when_idle_empty = Some(shutdown.clone());
    run_loop_with(
        account(),
        paths,
        tx,
        shutdown,
        commands,
        Arc::new(AtomicBool::new(false)),
        || Ok(Box::new(std::mem::take(&mut ops)) as Box<dyn MailOps>),
        |_| panic!("the session must not fail"),
    );
    let events: Vec<Event> = rx.try_iter().collect();
    let passes = events
        .iter()
        .filter(|e| matches!(e, Event::Synced { .. }))
        .count();
    let listings = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                Event::Activity {
                    activity: Activity::ListingFolders,
                    ..
                }
            )
        })
        .count();
    assert_eq!((passes, listings), (2, 2), "{events:?}");
}

#[test]
fn stop_interrupts_the_reconnect_wait() {
    let mut offline = account();
    offline.host = "127.0.0.1".into();
    offline.port = 1;
    offline.password = PasswordSource::Command {
        command: "printf x".into(),
    };
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let shutdown = Arc::new(AtomicBool::new(false));
    let wake = Arc::new(AtomicBool::new(false));
    let worker = {
        let (paths, shutdown, wake) = (Paths::under(dir.path()), shutdown.clone(), wake.clone());
        let commands = std::sync::mpsc::channel::<Job>().1;
        std::thread::spawn(move || {
            let account = offline.clone();
            run_loop(offline, paths, tx, shutdown, commands, wake, move || {
                Ok(Box::new(connect(&account)?) as Box<dyn MailOps>)
            })
        })
    };
    wait_for(&rx, &mut Vec::new(), |e| {
        matches!(
            e,
            Event::Activity {
                activity: Activity::Offline { .. },
                ..
            }
        )
    });
    let started = std::time::Instant::now();
    shutdown.store(true, Ordering::Relaxed);
    wake.store(true, Ordering::Relaxed);
    worker.join().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

/// Queues commands and a connection failure when the pass selects `folder`, i.e. before its chunk checkpoint.
struct QueuesCommandsOnSelect {
    inner: RecordingOps,
    commands: std::sync::mpsc::Sender<Job>,
    queued: Vec<Job>,
    folder: &'static str,
}

impl MailOps for QueuesCommandsOnSelect {
    fn list_folders(&mut self) -> crate::mail_ops::MailResult<Vec<RemoteFolder>> {
        self.inner.list_folders()
    }
    fn select(&mut self, folder: &str) -> crate::mail_ops::MailResult<crate::mail_ops::SelectInfo> {
        if folder != self.folder {
            return self.inner.select(folder);
        }
        if !self.queued.is_empty() {
            self.inner.fail_next = Some(MailError::Io("reset".into()));
        }
        for command in self.queued.drain(..) {
            self.commands.send(command).unwrap();
        }
        self.inner.select(folder)
    }
    fn search_uids(&mut self, from_uid: u32) -> crate::mail_ops::MailResult<Vec<u32>> {
        self.inner.search_uids(from_uid)
    }
    fn fetch_envelopes(
        &mut self,
        first: u32,
        last: u32,
    ) -> crate::mail_ops::MailResult<Vec<Envelope>> {
        self.inner.fetch_envelopes(first, last)
    }
    fn fetch_flags(
        &mut self,
        upto_uid: u32,
        changed_since: Option<u64>,
    ) -> crate::mail_ops::MailResult<Vec<crate::mail_ops::FlagUpdate>> {
        self.inner.fetch_flags(upto_uid, changed_since)
    }
    fn fetch_raw(&mut self, uid: u32) -> crate::mail_ops::MailResult<Option<Vec<u8>>> {
        self.inner.fetch_raw(uid)
    }
    fn add_flags(&mut self, uid: u32, flags: &[&str]) -> crate::mail_ops::MailResult<()> {
        self.inner.add_flags(uid, flags)
    }
    fn remove_flags(&mut self, uid: u32, flags: &[&str]) -> crate::mail_ops::MailResult<()> {
        self.inner.remove_flags(uid, flags)
    }
    fn expunge(&mut self, uid: u32) -> crate::mail_ops::MailResult<()> {
        self.inner.expunge(uid)
    }
    fn move_message(&mut self, uid: u32, to: &str) -> crate::mail_ops::MailResult<()> {
        self.inner.move_message(uid, to)
    }
    fn create_folder(&mut self, name: &str) -> crate::mail_ops::MailResult<()> {
        self.inner.create_folder(name)
    }
    fn append(
        &mut self,
        folder: &str,
        raw: &[u8],
        flags: &[&str],
    ) -> crate::mail_ops::MailResult<()> {
        self.inner.append(folder, raw, flags)
    }
    fn idle(
        &mut self,
        timeout: Duration,
        interrupt: &AtomicBool,
    ) -> crate::mail_ops::MailResult<IdleOutcome> {
        self.inner.idle(timeout, interrupt)
    }
}

fn mark_read(uid: u32) -> Job {
    Job {
        request: RequestId::from(uid),
        command: Command::Apply {
            folder: "INBOX".into(),
            uids: vec![uid],
            action: Action::MarkRead,
            by: "test".into(),
        },
    }
}

fn sync_now(request: RequestId) -> Job {
    Job {
        request,
        command: Command::SyncNow,
    }
}

#[test]
fn a_lost_connection_at_a_chunk_checkpoint_ends_the_pass_and_keeps_later_commands() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let acct = account();
    let mut state = AccountSync::open(&acct, &paths).unwrap();
    let mut inner = ops_with_inbox();
    sync_all(&mut inner, &state.store).unwrap();
    inner.add_mail("INBOX", 3, 12 * H, &headers("bob@x", "new", "m3@x"), None);
    let (commands_tx, commands) = std::sync::mpsc::channel();
    let mut ops = QueuesCommandsOnSelect {
        inner,
        commands: commands_tx,
        queued: vec![mark_read(1), mark_read(2)],
        folder: "INBOX",
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let result = state.pass(
        &mut ops,
        true,
        &mut Carried::default(),
        &tx,
        &commands,
        &AtomicBool::new(false),
    );
    let error = result.unwrap_err();
    assert!(error.to_string().contains("reset"), "{error}");
    let events: Vec<Event> = rx.try_iter().collect();
    let done = events
        .iter()
        .filter(|e| matches!(e, Event::ActionDone { .. }))
        .count();
    assert_eq!(done, 1, "{events:?}");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::Error { .. } | Event::Synced { .. })),
        "{events:?}"
    );
    assert_eq!(commands.try_recv().unwrap(), mark_read(2));
}

fn notified(events: &[Event]) -> Vec<u32> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::NewMail { uid, .. } => Some(*uid),
            _ => None,
        })
        .collect()
}

/// A pass commits new INBOX mail (uid 3), then loses the connection at a later folder's chunk checkpoint; a clean
/// pass follows. Returns the clean pass's events and the account's store.
fn clean_pass_after_an_aborted_one(rules: &str) -> (Vec<Event>, Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    if !rules.is_empty() {
        std::fs::create_dir_all(paths.rules_file().parent().unwrap()).unwrap();
        std::fs::write(paths.rules_file(), rules).unwrap();
    }
    let acct = account();
    let mut state = AccountSync::open(&acct, &paths).unwrap();
    let mut inner = ops_with_inbox().with_folder("Later", None);
    sync_all(&mut inner, &state.store).unwrap();
    inner.add_mail(
        "INBOX",
        3,
        now(),
        &headers("bob@x", "your code", "m3@x"),
        Some("From: bob@x\r\n\r\ncode 99"),
    );
    inner.add_mail("Later", 1, 12 * H, &headers("c@x", "later", "l1@x"), None);
    let (commands_tx, commands) = std::sync::mpsc::channel();
    let mut ops = QueuesCommandsOnSelect {
        inner,
        commands: commands_tx,
        queued: vec![mark_read(1)],
        folder: "Later",
    };
    let stop = AtomicBool::new(false);
    let (tx, _) = std::sync::mpsc::channel();
    assert!(
        state
            .pass(
                &mut ops,
                true,
                &mut Carried::default(),
                &tx,
                &commands,
                &stop
            )
            .is_err()
    );
    assert_eq!(state.store.folder("INBOX").unwrap().unwrap().last_uid, 3);
    let (tx, rx) = std::sync::mpsc::channel();
    state
        .pass(
            &mut ops.inner,
            true,
            &mut Carried::default(),
            &tx,
            &commands,
            &stop,
        )
        .unwrap();
    (rx.try_iter().collect(), state.store, dir)
}

#[test]
fn mail_committed_by_a_pass_aborted_at_a_checkpoint_notifies_on_the_next_pass() {
    let (events, _store, _dir) = clean_pass_after_an_aborted_one("");
    assert_eq!(notified(&events), [3], "{events:?}");
}

#[test]
fn a_body_rule_fetches_the_body_of_mail_committed_by_an_aborted_pass() {
    let rules =
        "[[rules]]\nname = \"codes\"\nmatch.body = { contains = \"code\" }\nactions = [\"flag\"]\n";
    let (events, store, _dir) = clean_pass_after_an_aborted_one(rules);
    let message = store.message("INBOX", 3).unwrap().unwrap();
    assert!(message.body_text.is_some(), "{events:?}");
    assert!(message.flags.contains("\\Flagged"));
}

#[test]
fn mail_committed_before_a_failed_chunk_notifies_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let acct = account();
    let mut state = AccountSync::open(&acct, &paths).unwrap();
    let mut ops = RecordingOps::new().with_folder("INBOX", None);
    let commands = std::sync::mpsc::channel::<Job>().1;
    let stop = AtomicBool::new(false);
    let (tx, rx) = std::sync::mpsc::channel();
    state
        .pass(
            &mut ops,
            true,
            &mut Carried::default(),
            &tx,
            &commands,
            &stop,
        )
        .unwrap();
    for uid in 1..=600 {
        let id = format!("n{uid}@x");
        ops.add_mail("INBOX", uid, 12 * H, &headers("bob@x", "new", &id), None);
    }
    ops.fail_fetch_after = Some(1);
    let _ = state.pass(
        &mut ops,
        true,
        &mut Carried::default(),
        &tx,
        &commands,
        &stop,
    );
    assert_eq!(state.store.message_count("INBOX").unwrap(), 500);
    ops.fail_fetch_after = None;
    state
        .pass(
            &mut ops,
            true,
            &mut Carried::default(),
            &tx,
            &commands,
            &stop,
        )
        .unwrap();
    let mut uids = notified(&rx.try_iter().collect::<Vec<_>>());
    uids.sort_unstable();
    assert_eq!(uids, (1..=600).collect::<Vec<u32>>());
}

#[test]
fn processed_mail_does_not_notify_again_on_later_passes() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let acct = account();
    let mut state = AccountSync::open(&acct, &paths).unwrap();
    let mut ops = ops_with_inbox();
    let commands = std::sync::mpsc::channel::<Job>().1;
    let stop = AtomicBool::new(false);
    let mut pass = |ops: &mut RecordingOps| {
        let (tx, rx) = std::sync::mpsc::channel();
        state
            .pass(ops, true, &mut Carried::default(), &tx, &commands, &stop)
            .unwrap();
        notified(&rx.try_iter().collect::<Vec<_>>())
    };
    assert!(pass(&mut ops).is_empty());
    ops.add_mail("INBOX", 3, 12 * H, &headers("bob@x", "new", "m3@x"), None);
    assert_eq!(pass(&mut ops), [3]);
    assert!(pass(&mut ops).is_empty());
}

#[test]
fn an_inbox_only_pass_runs_queued_commands_before_syncing() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let acct = account();
    let mut state = AccountSync::open(&acct, &paths).unwrap();
    let mut ops = ops_with_inbox();
    sync_all(&mut ops, &state.store).unwrap();
    ops.add_mail("INBOX", 3, 12 * H, &headers("bob@x", "new", "m3@x"), None);
    let (commands_tx, commands) = std::sync::mpsc::channel();
    commands_tx.send(mark_read(1)).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    state
        .pass(
            &mut ops,
            false,
            &mut Carried::default(),
            &tx,
            &commands,
            &AtomicBool::new(false),
        )
        .unwrap();
    let events: Vec<Event> = rx.try_iter().collect();
    let position = |wanted: fn(&Event) -> bool| events.iter().position(wanted).unwrap();
    let command_done = position(|e| matches!(e, Event::ActionDone { .. }));
    let first_fetch = position(|e| {
        matches!(
            e,
            Event::Activity {
                activity: Activity::FetchingHeaders { .. },
                ..
            }
        )
    });
    assert!(command_done < first_fetch, "{events:?}");
}

fn synced() -> (RecordingOps, Store, tempfile::TempDir) {
    let mut ops = ops_with_inbox();
    let store = Store::open_in_memory().unwrap();
    sync_all(&mut ops, &store).unwrap();
    (ops, store, tempfile::tempdir().unwrap())
}

/// Runs the commands as requests 1, 2, ... against a missing rules file.
fn drain(
    ops: &mut RecordingOps,
    store: &Store,
    trash: &Trash,
    commands: Vec<Command>,
) -> (Result<CommandsRun, SyncError>, Vec<Event>) {
    let missing_rules = Path::new("/nonexistent/rules.toml");
    drain_with_rules(ops, store, trash, missing_rules, commands)
}

fn drain_with_rules(
    ops: &mut RecordingOps,
    store: &Store,
    trash: &Trash,
    rules_path: &Path,
    commands: Vec<Command>,
) -> (Result<CommandsRun, SyncError>, Vec<Event>) {
    let (tx, rx) = std::sync::mpsc::channel();
    for (index, command) in commands.into_iter().enumerate() {
        let request = index as RequestId + 1;
        tx.send(Job { request, command }).unwrap();
    }
    let (events, seen) = std::sync::mpsc::channel();
    let acc = account();
    let identity = acc.identity().unwrap();
    let mut carried = Carried::default();
    let run = run_commands(
        ops,
        store,
        trash,
        &acc,
        &identity,
        rules_path,
        &rx,
        &mut carried,
        &events,
    );
    (run, seen.try_iter().collect())
}

#[test]
fn apply_reports_a_result_per_uid_and_keeps_going() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    let apply = Command::Apply {
        folder: "INBOX".into(),
        uids: vec![1, 99],
        action: Action::MarkRead,
        by: "test".into(),
    };
    let (run, events) = drain(&mut ops, &store, &trash, vec![apply, Command::SyncNow]);
    assert_eq!(
        run.unwrap(),
        CommandsRun {
            used_connection: true,
            wants_full_pass: true,
        }
    );
    let done = events
        .iter()
        .find_map(|e| match e {
            Event::ActionDone { results, .. } => Some(results.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(done[0], (1, Ok(1)));
    assert!(
        matches!(&done[1], (99, Err(e)) if e.contains("no message INBOX/99")),
        "{done:?}"
    );
    assert!(store.message("INBOX", 1).unwrap().unwrap().is_seen());
    assert!(events.contains(&Event::Activity {
        account: "work".into(),
        activity: Activity::RunningCommand {
            what: "marking read 2 messages".into()
        }
    }));
}

#[test]
fn a_move_syncs_the_target_folder_before_it_reports_done_then_asks_for_a_pass() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    let apply = Command::Apply {
        folder: "INBOX".into(),
        uids: vec![1],
        action: Action::Move("Done".into()),
        by: "test".into(),
    };
    let (run, _) = drain(&mut ops, &store, &trash, vec![apply]);
    assert!(run.unwrap().wants_full_pass);
    assert_eq!(store.messages_in_folder("Done").unwrap().len(), 1);
}

#[test]
fn a_trash_syncs_the_trash_folder_before_it_reports_done() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    let apply = Command::Apply {
        folder: "INBOX".into(),
        uids: vec![1],
        action: Action::Trash,
        by: "test".into(),
    };
    let (run, _) = drain(&mut ops, &store, &trash, vec![apply]);
    assert!(run.unwrap().wants_full_pass);
    assert_eq!(store.messages_in_folder("Trash").unwrap().len(), 1);
}

#[test]
fn a_failed_move_syncs_nothing() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    let apply = Command::Apply {
        folder: "INBOX".into(),
        uids: vec![99],
        action: Action::Move("Done".into()),
        by: "test".into(),
    };
    let (run, _) = drain(&mut ops, &store, &trash, vec![apply]);
    assert!(!run.unwrap().wants_full_pass);
    assert!(!ops.calls.iter().any(|call| call == "select Done"));
}

#[test]
fn fetch_body_stores_the_body_and_reports_it() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    let (run, events) = drain(
        &mut ops,
        &store,
        &trash,
        vec![Command::FetchBody {
            folder: "INBOX".into(),
            uid: 1,
        }],
    );
    assert!(run.unwrap().used_connection);
    assert!(store.raw("INBOX", 1).unwrap().is_some());
    assert!(events.contains(&Event::BodyReady {
        account: "work".into(),
        folder: "INBOX".into(),
        uid: 1,
        request: 1
    }));
}

#[test]
fn restore_appends_the_backup_syncs_its_folder_and_asks_for_a_full_pass() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    let file = trash
        .save("INBOX", 7, b"Subject: back\r\n\r\nhi", 100)
        .unwrap();
    let restore = Command::Restore { file: file.clone() };
    let (run, events) = drain(&mut ops, &store, &trash, vec![restore]);
    assert!(run.unwrap().wants_full_pass);
    assert_eq!(store.messages_in_folder("INBOX").unwrap().len(), 3);
    assert!(!file.exists());
    assert!(ops.calls.iter().any(|c| c.starts_with("append INBOX")));
    assert!(events.contains(&Event::Restored {
        account: "work".into(),
        folder: "INBOX".into(),
        request: 1
    }));
}

#[test]
fn restore_refuses_a_file_outside_the_trash() {
    let (mut ops, store, dir) = synced();
    let trash_dir = dir.path().join("trash");
    std::fs::create_dir(&trash_dir).unwrap();
    let trash = Trash::new(trash_dir);
    let outside = dir.path().join("100-INBOX-7.eml");
    std::fs::write(&outside, b"Subject: x\r\n\r\n").unwrap();
    let restore = Command::Restore {
        file: outside.clone(),
    };
    let (run, events) = drain(&mut ops, &store, &trash, vec![restore]);
    assert!(run.is_ok());
    assert!(outside.exists());
    assert!(!ops.calls.iter().any(|c| c.starts_with("append")));
    assert!(events.iter().any(
        |e| matches!(e, Event::CommandFailed { request: 1, message, .. } if message.contains("not a backup"))
    ));
}

#[cfg(unix)]
#[test]
fn restore_refuses_a_symlink_in_the_trash_pointing_outside() {
    let (mut ops, store, dir) = synced();
    let trash_dir = dir.path().join("trash");
    std::fs::create_dir(&trash_dir).unwrap();
    let trash = Trash::new(trash_dir.clone());
    let outside = dir.path().join("secret.eml");
    std::fs::write(&outside, b"Subject: x\r\n\r\n").unwrap();
    let link = trash_dir.join("100-INBOX-7.eml");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let restore = Command::Restore { file: link.clone() };
    let (run, events) = drain(&mut ops, &store, &trash, vec![restore]);
    assert!(run.is_ok());
    assert!(outside.exists());
    assert!(link.is_symlink());
    assert!(!ops.calls.iter().any(|c| c.starts_with("append")));
    assert!(events.iter().any(
        |e| matches!(e, Event::CommandFailed { request: 1, message, .. } if message.contains("not a backup"))
    ));
}

#[test]
fn a_lost_connection_during_apply_reports_then_ends_the_drain() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    ops.fail_next = Some(MailError::Io("reset".into()));
    let apply = Command::Apply {
        folder: "INBOX".into(),
        uids: vec![1],
        action: Action::MarkRead,
        by: "test".into(),
    };
    let next = Command::FetchBody {
        folder: "INBOX".into(),
        uid: 2,
    };
    let (run, events) = drain(&mut ops, &store, &trash, vec![apply, next]);
    assert!(matches!(run, Err(SyncError::Action(ref e)) if e.to_string().contains("reset")));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::ActionDone { results, .. }
        if matches!(&results[0], (1, Err(m)) if m.contains("reset"))))
    );
    assert!(!events.iter().any(|e| matches!(e, Event::BodyReady { .. })));
}

#[test]
fn a_lost_connection_during_fetch_body_ends_the_drain() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    ops.fail_next = Some(MailError::Io("reset".into()));
    let fetch = Command::FetchBody {
        folder: "INBOX".into(),
        uid: 1,
    };
    let (run, events) = drain(&mut ops, &store, &trash, vec![fetch]);
    assert!(matches!(run, Err(SyncError::Action(ref e)) if e.to_string().contains("reset")));
    assert!(
        events.iter().any(|e| matches!(e,
            Event::CommandFailed { request: 1, message, .. } if message.contains("reset"))),
        "{events:?}"
    );
}

#[test]
fn a_lost_connection_during_restore_ends_the_drain() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    let file = trash
        .save("INBOX", 7, b"Subject: back\r\n\r\n", 100)
        .unwrap();
    ops.fail_next = Some(MailError::Io("reset".into()));
    let (run, events) = drain(
        &mut ops,
        &store,
        &trash,
        vec![Command::Restore { file: file.clone() }],
    );
    assert!(matches!(run, Err(SyncError::Mail(MailError::Io(_)))));
    assert!(file.exists());
    assert!(
        events.iter().any(|e| matches!(e,
            Event::CommandFailed { request: 1, message, .. } if message.contains("reset"))),
        "{events:?}"
    );
}

#[test]
fn a_protocol_error_is_reported_and_the_next_command_runs() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    ops.fail_next = Some(MailError::Protocol("NO".into()));
    let commands = vec![
        Command::FetchBody {
            folder: "INBOX".into(),
            uid: 1,
        },
        Command::FetchBody {
            folder: "INBOX".into(),
            uid: 2,
        },
    ];
    let (run, events) = drain(&mut ops, &store, &trash, commands);
    assert!(run.is_ok());
    assert!(events.iter().any(
        |e| matches!(e, Event::CommandFailed { request: 1, message, .. } if message.contains("INBOX/1") && message.contains("NO"))
    ));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::BodyReady { uid: 2, .. }))
    );
}

#[test]
fn a_command_for_a_vanished_message_reports_and_the_next_runs() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    store.remove_message("INBOX", 1).unwrap();
    let commands = vec![
        Command::FetchBody {
            folder: "INBOX".into(),
            uid: 1,
        },
        Command::FetchBody {
            folder: "INBOX".into(),
            uid: 2,
        },
    ];
    let (run, events) = drain(&mut ops, &store, &trash, commands);
    assert!(run.is_ok());
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::CommandFailed { request: 1, message, .. } if message.contains("INBOX/1")))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::BodyReady { uid: 2, .. }))
    );
}

#[test]
fn a_command_failure_carries_its_request_id() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    let restore = Command::Restore {
        file: "/nope.eml".into(),
    };
    let (run, events) = drain(&mut ops, &store, &trash, vec![restore]);
    assert!(run.is_ok());
    let failed: Vec<&Event> = events
        .iter()
        .filter(|e| matches!(e, Event::CommandFailed { .. }))
        .collect();
    assert_eq!(failed.len(), 1, "{events:?}");
    let Event::CommandFailed {
        account,
        request,
        message,
    } = failed[0]
    else {
        unreachable!()
    };
    assert_eq!((account.as_str(), *request), ("work", 1));
    assert_eq!(message, "/nope.eml is not a backup in this account's trash");
}

#[test]
fn sync_now_ids_come_back_on_the_synced_that_answers_them() {
    let mut ops = ops_with_inbox();
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let (tx, rx) = std::sync::mpsc::channel();
    let (commands_tx, commands) = std::sync::mpsc::channel();
    commands_tx.send(sync_now(3)).unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    ops.shutdown_when_idle_empty = Some(shutdown.clone());
    run_loop_with(
        account(),
        paths,
        tx,
        shutdown,
        commands,
        Arc::new(AtomicBool::new(false)),
        || Ok(Box::new(std::mem::take(&mut ops)) as Box<dyn MailOps>),
        |_| panic!("the session must not fail"),
    );
    let answers: Vec<Vec<RequestId>> = rx
        .try_iter()
        .filter_map(|e| match e {
            Event::Synced { requests, .. } => Some(requests),
            _ => None,
        })
        .collect();
    assert_eq!(answers, vec![vec![], vec![3]]);
}

#[test]
fn queued_jobs_fail_when_the_account_goes_offline() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let (tx, rx) = std::sync::mpsc::channel();
    let (commands_tx, commands) = std::sync::mpsc::channel();
    let fetch = Command::FetchBody {
        folder: "INBOX".into(),
        uid: 1,
    };
    commands_tx
        .send(Job {
            request: 5,
            command: fetch,
        })
        .unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let stop = shutdown.clone();
    run_loop_with(
        account(),
        paths,
        tx,
        shutdown,
        commands,
        Arc::new(AtomicBool::new(false)),
        || Err(SyncError::Mail(MailError::Connect("refused".into()))),
        |_| stop.store(true, Ordering::Relaxed),
    );
    let events: Vec<Event> = rx.try_iter().collect();
    assert!(
        events.iter().any(|e| matches!(e,
            Event::CommandFailed { request: 5, message, .. }
                if message.starts_with("work is offline (could not connect: refused); retrying at "))),
        "{events:?}"
    );
}

#[test]
fn apply_logs_the_requester_as_the_rule_name() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    let apply = Command::Apply {
        folder: "INBOX".into(),
        uids: vec![1],
        action: Action::MarkRead,
        by: "mcp:host".into(),
    };
    let (_, events) = drain(&mut ops, &store, &trash, vec![apply]);
    assert_eq!(store.log(1).unwrap()[0].rule_name, "mcp:host");
    assert!(events.iter().any(|e| matches!(e,
        Event::ActionDone { request: 1, results, .. } if results == &[(1, Ok(1))])));
}

fn write_rules(dir: &Path, toml: &str) -> PathBuf {
    let path = dir.join("rules.toml");
    std::fs::write(&path, toml).unwrap();
    path
}

const FLAG_NOREPLY: &str =
    "[[rules]]\nname = \"codes\"\nmatch.from = { contains = \"noreply@\" }\nactions = [\"flag\"]\n";

#[test]
fn apply_rule_acts_on_stored_messages_and_reports_the_run() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    let rules = write_rules(dir.path(), FLAG_NOREPLY);
    let apply_rule = Command::ApplyRule {
        name: "codes".into(),
    };
    let (run, events) = drain_with_rules(&mut ops, &store, &trash, &rules, vec![apply_rule]);
    assert!(run.unwrap().used_connection);
    assert!(
        store
            .message("INBOX", 2)
            .unwrap()
            .unwrap()
            .flags
            .contains("\\Flagged")
    );
    let applied = events
        .iter()
        .find_map(|e| match e {
            Event::RuleApplied {
                request,
                evaluated,
                actions,
                errors,
                ..
            } => Some((*request, *evaluated, *actions, errors.clone())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no RuleApplied in {events:?}"));
    let (request, evaluated, actions, errors) = applied;
    assert_eq!((request, actions, errors), (1, 1, Vec::new()));
    assert!(evaluated >= 1, "evaluated {evaluated}");
}

#[test]
fn apply_rule_refuses_a_disabled_or_unknown_rule() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    let rules = write_rules(
        dir.path(),
        &format!("{FLAG_NOREPLY}enabled = false\n").replace("codes", "pending"),
    );
    let commands = ["pending", "missing"].map(|name| Command::ApplyRule { name: name.into() });
    let (run, events) = drain_with_rules(&mut ops, &store, &trash, &rules, commands.to_vec());
    assert!(run.is_ok());
    let failures: Vec<(RequestId, &str)> = events
        .iter()
        .filter_map(|e| match e {
            Event::CommandFailed {
                request, message, ..
            } => Some((*request, message.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        failures,
        [
            (1, "rule 'pending' is disabled; approve or enable it first"),
            (2, "no rule named 'missing'"),
        ]
    );
    assert!(
        !store
            .message("INBOX", 2)
            .unwrap()
            .unwrap()
            .flags
            .contains("\\Flagged")
    );
}

#[test]
fn fetch_bodies_reports_how_many_arrived() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    let fetch = Command::FetchBodies {
        folder: Some("INBOX".into()),
    };
    let (run, events) = drain(&mut ops, &store, &trash, vec![fetch]);
    assert!(run.unwrap().used_connection);
    assert!(events.contains(&Event::BodiesFetched {
        account: "work".into(),
        request: 1,
        fetched: 2
    }));
    assert!(
        store
            .message("INBOX", 1)
            .unwrap()
            .unwrap()
            .body_text
            .is_some()
    );
}

#[test]
fn fetch_bodies_without_a_folder_covers_every_stored_folder() {
    let (mut ops, store, dir) = synced();
    ops.add_mail(
        "Trash",
        1,
        10 * H,
        &headers("carol@x", "old", "t1@x"),
        Some("From: carol@x\r\n\r\nbye"),
    );
    let trash = Trash::new(dir.path().to_path_buf());
    sync_all(&mut ops, &store).unwrap();
    let (_, events) = drain(
        &mut ops,
        &store,
        &trash,
        vec![Command::FetchBodies { folder: None }],
    );
    assert!(events.contains(&Event::BodiesFetched {
        account: "work".into(),
        request: 1,
        fetched: 3
    }));
}

#[test]
fn events_and_commands_round_trip_as_json() {
    for event in [
        Event::Synced {
            account: "a".into(),
            new_messages: 1,
            actions: 0,
            requests: vec![2],
            errors: vec![],
        },
        Event::CommandFailed {
            account: "a".into(),
            request: 9,
            message: "x".into(),
        },
        Event::ActionDone {
            account: "a".into(),
            folder: "INBOX".into(),
            results: vec![(1, Ok(1)), (2, Err("e".into()))],
            request: 4,
        },
    ] {
        let text = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&text).unwrap(), event);
    }
    let job = Job {
        request: 1,
        command: Command::Apply {
            folder: "INBOX".into(),
            uids: vec![1],
            action: Action::Archive,
            by: "cli".into(),
        },
    };
    let text = serde_json::to_string(&job).unwrap();
    assert_eq!(serde_json::from_str::<Job>(&text).unwrap(), job);
}

#[test]
fn every_action_a_front_end_can_send_survives_the_wire() {
    for action in [
        Action::Archive,
        Action::Delete,
        Action::Flag,
        Action::MarkRead,
        Action::MarkUnread,
        Action::Move("Lists/rust".into()),
        Action::Notify,
        Action::Silent,
        Action::Trash,
        Action::Unflag,
    ] {
        let command = Command::Apply {
            folder: "INBOX".into(),
            uids: vec![1],
            action,
            by: "cli".into(),
        };
        let text = serde_json::to_string(&command)
            .unwrap_or_else(|e| panic!("{command:?} does not serialize: {e}"));
        assert_eq!(serde_json::from_str::<Command>(&text).unwrap(), command);
    }
}

#[test]
fn request_ids_name_what_an_event_completes() {
    let synced = Event::Synced {
        account: "a".into(),
        new_messages: 0,
        actions: 0,
        requests: vec![1, 2],
        errors: vec![],
    };
    let new_mail = Event::NewMail {
        account: "a".into(),
        folder: "INBOX".into(),
        uid: 1,
        from: String::new(),
        subject: String::new(),
    };
    let restored = Event::Restored {
        account: "a".into(),
        folder: "INBOX".into(),
        request: 8,
    };
    assert_eq!(synced.request_ids(), vec![1, 2]);
    assert_eq!(restored.request_ids(), vec![8]);
    assert!(new_mail.request_ids().is_empty());
}

#[test]
fn fetch_bodies_for_a_named_folder_fails_when_that_folder_cannot_be_fetched() {
    let (mut ops, store, dir) = synced();
    let trash = Trash::new(dir.path().to_path_buf());
    ops.uidvalidity.insert("INBOX".into(), 9);
    let fetch = Command::FetchBodies {
        folder: Some("INBOX".into()),
    };
    let (run, events) = drain(&mut ops, &store, &trash, vec![fetch]);
    assert!(run.is_ok());
    assert!(
        events.iter().any(|e| matches!(e,
            Event::CommandFailed { request: 1, message, .. } if message.contains("INBOX"))),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::BodiesFetched { .. }))
    );
}

#[test]
fn a_session_that_fails_mid_pass_fails_the_sync_now_ids_it_holds() {
    let mut ops = ops_with_inbox();
    ops.fail_fetch_after = Some(0);
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let (tx, rx) = std::sync::mpsc::channel();
    let (commands_tx, commands) = std::sync::mpsc::channel();
    commands_tx.send(sync_now(6)).unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let stop = shutdown.clone();
    run_loop_with(
        account(),
        paths,
        tx,
        shutdown,
        commands,
        Arc::new(AtomicBool::new(false)),
        || Ok(Box::new(std::mem::take(&mut ops)) as Box<dyn MailOps>),
        |_| stop.store(true, Ordering::Relaxed),
    );
    let events: Vec<Event> = rx.try_iter().collect();
    let failed: Vec<RequestId> = events
        .iter()
        .filter_map(|e| match e {
            Event::CommandFailed { request, .. } => Some(*request),
            _ => None,
        })
        .collect();
    assert_eq!(failed, vec![6], "{events:?}");
    assert!(!events.iter().any(|e| matches!(e, Event::Synced { .. })));
}

#[test]
fn a_lost_connection_later_in_a_drain_still_fails_the_sync_now_drained_before_it() {
    let mut ops = ops_with_inbox();
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    paths.ensure_account("work").unwrap();
    sync_all(&mut ops, &Store::open(&paths.mail_db("work")).unwrap()).unwrap();
    ops.fail_next = Some(MailError::Io("reset".into()));
    let (tx, rx) = std::sync::mpsc::channel();
    let (commands_tx, commands) = std::sync::mpsc::channel();
    commands_tx.send(sync_now(7)).unwrap();
    let fetch = Command::FetchBody {
        folder: "INBOX".into(),
        uid: 1,
    };
    let fetch = Job {
        request: 8,
        command: fetch,
    };
    commands_tx.send(fetch).unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let stop = shutdown.clone();
    run_loop_with(
        account(),
        paths,
        tx,
        shutdown,
        commands,
        Arc::new(AtomicBool::new(false)),
        || Ok(Box::new(std::mem::take(&mut ops)) as Box<dyn MailOps>),
        |_| stop.store(true, Ordering::Relaxed),
    );
    let events: Vec<Event> = rx.try_iter().collect();
    let mut failed: Vec<RequestId> = events
        .iter()
        .filter_map(|e| match e {
            Event::CommandFailed { request, .. } => Some(*request),
            _ => None,
        })
        .collect();
    failed.sort_unstable();
    assert_eq!(failed, vec![7, 8], "{events:?}");
}

fn failures_of(events: &[Event]) -> Vec<(RequestId, String)> {
    let mut failed: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::CommandFailed {
                request, message, ..
            } => Some((*request, message.clone())),
            _ => None,
        })
        .collect();
    failed.sort();
    failed
}

#[test]
fn a_stop_mid_pass_fails_the_held_sync_now_as_stopped() {
    let mut ops = ops_with_inbox();
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let (commands_tx, commands) = std::sync::mpsc::channel();
    commands_tx.send(sync_now(7)).unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    ops.shutdown_on_list_folders = Some(shutdown.clone());
    run_loop_with(
        account(),
        Paths::under(dir.path()),
        tx,
        shutdown,
        commands,
        Arc::new(AtomicBool::new(false)),
        || Ok(Box::new(std::mem::take(&mut ops)) as Box<dyn MailOps>),
        |_| panic!("a stop is not a failure"),
    );
    let events: Vec<Event> = rx.try_iter().collect();
    assert_eq!(
        failures_of(&events),
        [(7, "work stopped".to_string())],
        "{events:?}"
    );
}

#[test]
fn a_stop_fails_queued_jobs_as_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let (commands_tx, commands) = std::sync::mpsc::channel();
    commands_tx.send(sync_now(3)).unwrap();
    commands_tx
        .send(Job {
            request: 4,
            command: Command::ApplyRule { name: "x".into() },
        })
        .unwrap();
    run_loop_with(
        account(),
        Paths::under(dir.path()),
        tx,
        Arc::new(AtomicBool::new(true)),
        commands,
        Arc::new(AtomicBool::new(false)),
        || panic!("a stopped loop does not connect"),
        |_| panic!("a stop is not a failure"),
    );
    let events: Vec<Event> = rx.try_iter().collect();
    assert_eq!(
        failures_of(&events),
        [
            (3, "work stopped".to_string()),
            (4, "work stopped".to_string())
        ]
    );
}

#[test]
fn a_full_pass_stores_the_delimiter_and_an_inbox_only_pass_keeps_it() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    let acct = account();
    let mut state = AccountSync::open(&acct, &paths).unwrap();
    let mut ops = ops_with_inbox();
    for folder in &mut ops.folders {
        folder.delimiter = Some("/".into());
    }
    let delimiter = |store: &Store| store.folder("INBOX").unwrap().unwrap().delimiter;
    sync_all(&mut ops, &state.store).unwrap();
    assert_eq!(delimiter(&state.store).as_deref(), Some("/"));
    let (_commands_tx, commands) = std::sync::mpsc::channel();
    let (tx, _rx) = std::sync::mpsc::channel();
    let full = false;
    state
        .pass(
            &mut ops,
            full,
            &mut Carried::default(),
            &tx,
            &commands,
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(delimiter(&state.store).as_deref(), Some("/"));
}

#[test]
fn virtual_folders_that_repeat_other_folders_are_not_synced() {
    let mut ops = ops_with_inbox()
        .with_folder("[Gmail]/All Mail", Some("All"))
        .with_folder("[Gmail]/Important", Some("Important"))
        .with_folder("[Gmail]/Starred", Some("Flagged"));
    let store = Store::open_in_memory().unwrap();
    sync_all(&mut ops, &store).unwrap();
    let synced: Vec<String> = store
        .folders()
        .unwrap()
        .into_iter()
        .map(|f| f.name)
        .collect();
    assert_eq!(synced, ["INBOX", "Trash"]);
    assert!(
        !ops.calls.iter().any(|c| c.contains("[Gmail]")),
        "{:?}",
        ops.calls
    );
}

fn calls_after_second_pass(
    ops: &mut RecordingOps,
    store: &Store,
    change: impl FnOnce(&mut RecordingOps),
) -> Vec<String> {
    sync_all(ops, store).unwrap();
    change(ops);
    ops.calls.clear();
    sync_all(ops, store).unwrap();
    std::mem::take(&mut ops.calls)
}

#[test]
fn with_condstore_an_unchanged_folder_fetches_no_flags() {
    let mut ops = ops_with_inbox();
    let store = Store::open_in_memory().unwrap();
    let calls = calls_after_second_pass(&mut ops, &store, |_| {});
    assert!(
        !calls.iter().any(|c| c.starts_with("fetch_flags")),
        "{calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c == "search_uids INBOX 1"),
        "{calls:?}"
    );
}

#[test]
fn with_condstore_only_changed_flags_are_fetched() {
    let mut ops = ops_with_inbox();
    let store = Store::open_in_memory().unwrap();
    sync_all(&mut ops, &store).unwrap();
    let (since, _) = store.flags_synced("INBOX").unwrap();
    assert!(since > 0);
    let calls = calls_after_second_pass(&mut ops, &store, |ops| {
        ops.mail.get_mut("INBOX").unwrap()[0]
            .flags
            .push("\\Seen".into());
    });
    assert!(
        calls.contains(&format!("fetch_flags INBOX 2 since {since}")),
        "{calls:?}"
    );
    assert!(store.message("INBOX", 1).unwrap().unwrap().is_seen());
    assert!(store.flags_synced("INBOX").unwrap().0 > since);
}

#[test]
fn an_expunge_is_found_by_the_message_count_without_fetching_flags() {
    let mut ops = ops_with_inbox();
    let store = Store::open_in_memory().unwrap();
    let calls = calls_after_second_pass(&mut ops, &store, |ops| {
        ops.mail.get_mut("INBOX").unwrap().retain(|e| e.uid != 1);
    });
    assert_eq!(store.message("INBOX", 1).unwrap(), None);
    assert!(store.message("INBOX", 2).unwrap().is_some());
    assert!(
        calls.iter().any(|c| c == "search_uids INBOX 1"),
        "{calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.starts_with("fetch_flags")),
        "{calls:?}"
    );
}

#[test]
fn without_condstore_all_flags_are_fetched_only_once_the_interval_passed() {
    let mut ops = ops_with_inbox();
    ops.condstore = false;
    let store = Store::open_in_memory().unwrap();
    let set_seen = |ops: &mut RecordingOps| {
        ops.mail.get_mut("INBOX").unwrap()[0]
            .flags
            .push("\\Seen".into());
    };
    let calls = calls_after_second_pass(&mut ops, &store, set_seen);
    assert!(
        !calls.iter().any(|c| c.starts_with("fetch_flags")),
        "{calls:?}"
    );
    assert!(!store.message("INBOX", 1).unwrap().unwrap().is_seen());

    let long_ago = now() - FULL_FLAGS_EVERY_SECS;
    store.set_flags_synced("INBOX", 0, long_ago).unwrap();
    sync_all(&mut ops, &store).unwrap();
    assert!(
        ops.calls.iter().any(|c| c == "fetch_flags INBOX 2"),
        "{:?}",
        ops.calls
    );
    assert!(store.message("INBOX", 1).unwrap().unwrap().is_seen());
    assert!(store.flags_synced("INBOX").unwrap().1 > long_ago);
}

#[test]
fn a_folder_without_a_stored_modseq_fetches_all_flags_once() {
    let mut ops = ops_with_inbox();
    let store = Store::open_in_memory().unwrap();
    let calls = calls_after_second_pass(&mut ops, &store, |_| {});
    assert!(
        !calls.iter().any(|c| c.starts_with("fetch_flags")),
        "{calls:?}"
    );
    store.set_flags_synced("INBOX", 0, 0).unwrap();
    sync_all(&mut ops, &store).unwrap();
    assert!(
        ops.calls.iter().any(|c| c == "fetch_flags INBOX 2"),
        "{:?}",
        ops.calls
    );
    assert!(store.flags_synced("INBOX").unwrap().0 > 0);
}
