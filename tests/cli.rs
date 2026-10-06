use std::process::Command;

fn postbode(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_postbode"))
        .args(args)
        .env("POSTBODE_HOME", home)
        .env("RUST_LOG", "error")
        .output()
        .unwrap()
}

#[test]
fn rules_check_reports_bad_file_and_exits_nonzero() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("config")).unwrap();
    std::fs::write(
        home.path().join("config/rules.toml"),
        "[[rules]]\nname = \"x\"\nmatch.from = { regex = \"(\" }\nactions = [\"delete\"]\n",
    )
    .unwrap();
    let out = postbode(home.path(), &["rules", "check"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("rule 'x'"), "{stderr}");
}

#[test]
fn rules_check_passes_on_valid_file_and_lists() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("config")).unwrap();
    std::fs::write(
        home.path().join("config/rules.toml"),
        "[[rules]]\nname = \"ok\"\nmatch.seen = true\nactions = [\"flag\"]\n",
    )
    .unwrap();
    assert!(postbode(home.path(), &["rules", "check"]).status.success());
    let out = postbode(home.path(), &["rules", "list"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("ok"));
}

#[test]
fn list_on_unknown_account_fails_and_empty_config_lists_nothing() {
    let home = tempfile::tempdir().unwrap();
    let out = postbode(home.path(), &["list", "--account", "nope"]);
    assert!(!out.status.success());
    let out = postbode(home.path(), &["folders"]);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}

#[test]
fn account_add_rejects_invalid_name() {
    use std::io::Write;
    use std::process::Stdio;

    let home = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_postbode"))
        .args(["account", "add"])
        .env("POSTBODE_HOME", home.path())
        .env("RUST_LOG", "error")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"my work\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("may only contain letters, digits"),
        "{stderr}"
    );
    assert!(!home.path().join("config/config.toml").exists());
}

#[test]
fn rules_test_previews_fresh_rule() {
    use postbode::paths::Paths;
    use postbode::store::{Folder, Message, Store};

    let home = tempfile::tempdir().unwrap();
    let paths = Paths::under(home.path());
    paths.ensure_account("work").unwrap();
    std::fs::write(
        paths.config_file(),
        "[[accounts]]\nname = \"work\"\nhost = \"imap.example.com\"\nusername = \"me@example.com\"\npassword = { command = \"printf x\" }\n",
    )
    .unwrap();
    std::fs::write(
        paths.rules_file(),
        "[[rules]]\nname = \"flag-invoices\"\nmatch.subject = { contains = \"invoice\" }\nactions = [\"flag\"]\n",
    )
    .unwrap();
    let store = Store::open(&paths.mail_db("work")).unwrap();
    store
        .upsert_folder(&Folder {
            name: "INBOX".into(),
            uidvalidity: 1,
            last_uid: 42,
            special_use: None,
        })
        .unwrap();
    store
        .insert_message(&Message {
            folder: "INBOX".into(),
            uid: 42,
            message_id: None,
            from_addr: Some("billing@example.com".into()),
            to_addr: Some("me@example.com".into()),
            cc_addr: None,
            delivered_to: None,
            in_reply_to: None,
            refs: None,
            thread_id: "t42".into(),
            subject: Some("Your invoice".into()),
            date: Some(1_000),
            internaldate: 1_000,
            flags: String::new(),
            size: None,
            headers: Vec::new(),
            body_text: None,
        })
        .unwrap();

    let out = postbode(home.path(), &["rules", "test"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("flag-invoices") && stdout.contains("INBOX/42"),
        "{stdout}"
    );
    assert_eq!(store.rule_first_seen("flag-invoices", 999).unwrap(), 999);
}

const ONE_ACCOUNT: &str = "[[accounts]]\nname = \"work\"\nhost = \"127.0.0.1\"\nport = 1\nusername = \"me@example.com\"\npassword = { command = \"printf x\" }\n";

use postbode::paths::Paths;
use postbode::store::{Folder, LogEntry, Message, Store};

fn message(uid: u32, from: &str, subject: &str) -> Message {
    Message {
        folder: "INBOX".into(),
        uid,
        message_id: Some(format!("m{uid}@example.com")),
        from_addr: Some(from.into()),
        to_addr: Some("me@example.com".into()),
        cc_addr: None,
        delivered_to: None,
        in_reply_to: None,
        refs: None,
        thread_id: format!("m{uid}@example.com"),
        subject: Some(subject.into()),
        date: Some(1_000 + uid as i64),
        internaldate: 1_000 + uid as i64,
        flags: String::new(),
        size: None,
        headers: Vec::new(),
        body_text: None,
    }
}

/// A POSTBODE_HOME with account "work" whose INBOX holds `messages`; its server is unreachable, so connecting fails.
fn seeded_home(messages: &[Message]) -> (tempfile::TempDir, Store) {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::under(home.path());
    paths.ensure_account("work").unwrap();
    std::fs::write(paths.config_file(), ONE_ACCOUNT).unwrap();
    let store = Store::open(&paths.mail_db("work")).unwrap();
    store
        .upsert_folder(&Folder {
            name: "INBOX".into(),
            uidvalidity: 1,
            last_uid: messages.iter().map(|m| m.uid).max().unwrap_or(0),
            special_use: None,
        })
        .unwrap();
    for m in messages {
        store.insert_message(m).unwrap();
    }
    (home, store)
}

#[test]
fn list_and_log_show_the_account() {
    let (home, store) = seeded_home(&[message(42, "billing@example.com", "Your invoice")]);
    store
        .log_action(&LogEntry {
            id: 0,
            at: 1_000,
            rule_name: "r".into(),
            folder: "INBOX".into(),
            uid: 42,
            message_id: None,
            subject: Some("Your invoice".into()),
            action: "flag".into(),
            trash_file: None,
        })
        .unwrap();
    let stdout = String::from_utf8_lossy(&postbode(home.path(), &["list"]).stdout).to_string();
    assert!(
        stdout.starts_with("work  ")
            && stdout.contains("INBOX/42")
            && stdout.contains("Your invoice"),
        "{stdout}"
    );
    let stdout = String::from_utf8_lossy(&postbode(home.path(), &["log"]).stdout).to_string();
    assert!(stdout.starts_with("work  "), "{stdout}");
    for args in [&["list", "--json"][..], &["log", "--json"][..]] {
        let out = postbode(home.path(), args);
        let first = out.stdout.split(|b| *b == b'\n').next().unwrap();
        let line: serde_json::Value = serde_json::from_slice(first).unwrap();
        assert_eq!(line["account"], "work", "{args:?}");
    }
}

#[test]
fn trash_list_shows_subjects() {
    let (home, _store) = seeded_home(&[]);
    postbode::trash::Trash::new(Paths::under(home.path()).trash_dir("work"))
        .save(
            "INBOX",
            7,
            b"Subject: Your code is 123456\r\n\r\nbody",
            1_000,
        )
        .unwrap();
    let stdout =
        String::from_utf8_lossy(&postbode(home.path(), &["trash", "list"]).stdout).to_string();
    assert!(
        stdout.contains("INBOX/7") && stdout.contains("Your code is 123456"),
        "{stdout}"
    );
}

#[test]
fn error_lines_strip_control_characters() {
    let home = tempfile::tempdir().unwrap();
    let out = postbode(home.path(), &["list", "--account", "x\u{1b}[2Jy"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no account named 'x[2Jy'"), "{stderr}");
}

#[test]
fn rules_test_with_unknown_name_fails() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("config")).unwrap();
    std::fs::write(home.path().join("config/config.toml"), ONE_ACCOUNT).unwrap();
    std::fs::write(
        home.path().join("config/rules.toml"),
        "[[rules]]\nname = \"ok\"\nmatch.seen = true\nactions = [\"flag\"]\n",
    )
    .unwrap();
    let out = postbode(home.path(), &["rules", "test", "nope"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no rule named 'nope'"), "{stderr}");
}

#[test]
fn run_refuses_to_start_with_invalid_rules() {
    use std::time::{Duration, Instant};

    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("config")).unwrap();
    std::fs::write(home.path().join("config/config.toml"), ONE_ACCOUNT).unwrap();
    std::fs::write(
        home.path().join("config/rules.toml"),
        "[[rules]]\nname = \"x\"\nmatch.from = { regex = \"(\" }\nactions = [\"delete\"]\n",
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_postbode"))
        .arg("run")
        .env("POSTBODE_HOME", home.path())
        .env("RUST_LOG", "error")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > Duration::from_secs(10) {
            child.kill().unwrap();
            panic!("`postbode run` kept running with an invalid rules.toml");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("rule 'x'"), "{stderr}");
}

#[test]
fn delete_dry_run_reads_only_the_local_store() {
    let (home, _store) = seeded_home(&[message(42, "a@example.com", "Old newsletter")]);
    let out = postbode(home.path(), &["delete", "42", "--dry-run"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "would trash  INBOX/42  Old newsletter\n"
    );
    let out = postbode(home.path(), &["mark", "read", "42", "43", "--dry-run"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("INBOX/43: not in the local store"),
        "{stderr}"
    );
}

#[test]
fn direct_actions_need_at_least_one_uid() {
    let (home, _store) = seeded_home(&[]);
    assert_eq!(postbode(home.path(), &["archive"]).status.code(), Some(2));
}

#[test]
fn show_json_carries_the_account() {
    let (home, store) = seeded_home(&[message(42, "a@example.com", "Hello")]);
    store
        .set_raw("INBOX", 42, b"Subject: Hello\r\n\r\nhi", "hi")
        .unwrap();
    let out = postbode(home.path(), &["show", "42", "--json"]);
    let line: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(line["account"], "work");
}

#[test]
fn search_finds_by_word_and_by_address() {
    let (home, _store) = seeded_home(&[
        message(42, "billing@example.com", "Your invoice"),
        message(43, "friend@example.com", "Lunch?"),
    ]);
    for query in ["invoice", "billing@example.com"] {
        let out = postbode(home.path(), &["search", query]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.lines().count() == 1 && stdout.contains("INBOX/42"),
            "{query}: {stdout}"
        );
    }
}

#[test]
fn list_threads_indents_replies_under_their_thread() {
    let root = message(1, "alice@example.com", "Plans");
    let mut reply = message(2, "bob@example.com", "Re: Plans");
    reply.thread_id = root.thread_id.clone();
    reply.in_reply_to = root.message_id.clone();
    let (home, _store) = seeded_home(&[root, reply]);
    let out = postbode(home.path(), &["list", "--threads"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    assert!(
        lines[0].ends_with("  Plans") && lines[1].ends_with("    Re: Plans"),
        "{stdout}"
    );
}

const WITH_ATTACHMENTS: &[u8] = b"From: a@example.com\r\n\
Subject: files\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\
\r\n\
--b\r\n\
Content-Type: text/plain\r\n\
\r\n\
see attached\r\n\
--b\r\n\
Content-Type: text/plain\r\n\
Content-Disposition: attachment; filename=\"../../evil.txt\"\r\n\
\r\n\
not evil\r\n\
--b\r\n\
Content-Type: application/pdf\r\n\
Content-Disposition: attachment\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0=\r\n\
--b--\r\n";

#[test]
fn attachments_list_and_save_from_the_cached_message() {
    let (home, store) = seeded_home(&[message(42, "a@example.com", "files")]);
    store
        .set_raw("INBOX", 42, WITH_ATTACHMENTS, "see attached")
        .unwrap();
    let out = postbode(home.path(), &["attachment", "list", "42"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert!(
        lines[0].starts_with("1  text/plain  ") && lines[0].ends_with("  ../../evil.txt"),
        "{stdout}"
    );
    assert_eq!(lines[1], "2  application/pdf  5  -");
    let target = tempfile::tempdir().unwrap();
    let dir = target.path().to_str().unwrap();
    let out = postbode(
        home.path(),
        &["attachment", "save", "42", "1", "--dir", dir],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(target.path().join("evil.txt").exists());
    assert!(
        !postbode(
            home.path(),
            &["attachment", "save", "42", "1", "--dir", dir]
        )
        .status
        .success()
    );
}

fn postbode_stdin(home: &std::path::Path, args: &[&str], stdin: &str) -> std::process::Output {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new(env!("CARGO_BIN_EXE_postbode"))
        .args(args)
        .env("POSTBODE_HOME", home)
        .env("RUST_LOG", "error")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn agent_proposes_and_a_human_approves_or_rejects() {
    let (home, store) = seeded_home(&[message(42, "noreply@example.com", "Your code is 123456")]);
    let rule =
        r#"{"name": "codes", "match": {"subject": {"contains": "code"}}, "actions": ["delete"]}"#;
    let list = |home: &std::path::Path| {
        String::from_utf8_lossy(&postbode(home, &["rules", "list"]).stdout).to_string()
    };

    let out = postbode_stdin(home.path(), &["rules", "test", "--stdin"], rule);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("codes\tINBOX/42\tdelete"), "{stdout}");
    assert!(
        !home.path().join("config/rules.toml").exists(),
        "a preview writes nothing"
    );

    let out = postbode_stdin(home.path(), &["rules", "propose", "--by", "test"], rule);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(list(home.path()).contains("off\tcodes\tcli:test"));
    let stdout =
        String::from_utf8_lossy(&postbode(home.path(), &["rules", "test", "codes"]).stdout)
            .to_string();
    assert!(
        stdout.contains("INBOX/42"),
        "naming a proposal previews it: {stdout}"
    );

    assert!(
        postbode(home.path(), &["rules", "approve", "codes"])
            .status
            .success()
    );
    assert!(list(home.path()).contains("on \tcodes"));
    assert!(
        store.rule_first_seen("codes", 0).unwrap() > 1_000_000_000,
        "approval stamps the rule's clock with the current time"
    );
    assert!(
        !postbode(home.path(), &["rules", "reject", "codes"])
            .status
            .success()
    );

    let out = postbode_stdin(
        home.path(),
        &["rules", "propose"],
        &rule.replace("codes", "codes-2"),
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        postbode(home.path(), &["rules", "reject", "codes-2"])
            .status
            .success()
    );
    assert!(!list(home.path()).contains("codes-2"));

    let bad = rule
        .replace("codes", "bad")
        .replace(r#""contains": "code""#, r#""regex": "(""#);
    let out = postbode_stdin(home.path(), &["rules", "propose"], &bad);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("rule 'bad'"));

    let out = postbode(home.path(), &["rules", "schema"]);
    serde_json::from_slice::<serde_json::Value>(&out.stdout).unwrap();
}
