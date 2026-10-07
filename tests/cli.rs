// Test code: unwrap, expect and panic are how a test fails.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::Command;
use std::time::Duration;

fn postbode(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_postbode"))
        .args(args)
        .env("POSTBODE_HOME", home)
        .env("POSTBODE_IDLE_EXIT_SECS", "1")
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
fn list_shows_local_time() {
    let mut m = message(42, "billing@example.com", "Your invoice");
    m.internaldate = 0;
    let (home, _store) = seeded_home(&[m]);
    let out = Command::new(env!("CARGO_BIN_EXE_postbode"))
        .arg("list")
        .env("POSTBODE_HOME", home.path())
        .env("TZ", "Etc/GMT-3")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("1970-01-01 03:00"), "{stdout}");
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
fn an_invalid_rules_file_still_lets_actions_reach_the_daemon() {
    let (home, _store) = seeded_home(&[message(1, "a@example.com", "Hello")]);
    std::fs::write(
        Paths::under(home.path()).rules_file(),
        "[[rules]]\nname = \"x\"\nmatch.from = { regex = \"(\" }\nactions = [\"delete\"]\n",
    )
    .unwrap();
    let out = postbode(home.path(), &["archive", "1"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("work is offline ("), "{stderr}");
    assert!(postbode(home.path(), &["daemon", "stop"]).status.success());
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
        "would delete (expunge, .eml backup kept)  INBOX/42  Old newsletter\n"
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
fn delete_dry_run_names_the_trash_folder() {
    let (home, store) = seeded_home(&[message(42, "a@example.com", "Old newsletter")]);
    store
        .upsert_folder(&Folder {
            name: "Trash".into(),
            uidvalidity: 1,
            last_uid: 0,
            special_use: Some("Trash".into()),
        })
        .unwrap();
    let out = postbode(home.path(), &["delete", "42", "--dry-run"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "would move to Trash  INBOX/42  Old newsletter\n"
    );
}

#[test]
fn apply_existing_refuses_a_disabled_rule() {
    let (home, _store) = seeded_home(&[message(42, "a@example.com", "Your code")]);
    std::fs::write(
        Paths::under(home.path()).rules_file(),
        "[[rules]]\nname = \"codes\"\nenabled = false\nmatch.subject = { contains = \"code\" }\nactions = [\"flag\"]\n",
    )
    .unwrap();
    for args in [
        &["rules", "apply-existing", "codes", "--dry-run"][..],
        &["rules", "apply-existing", "codes"][..],
    ] {
        let out = postbode(home.path(), args);
        assert!(!out.status.success(), "{args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("rule 'codes' is disabled; approve or enable it first"),
            "{args:?}: {stderr}"
        );
    }
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

#[test]
fn search_bodies_works_offline() {
    let mut stored = message(42, "billing@example.com", "Your invoice");
    stored.body_text = Some("the total is due".into());
    let (home, _store) = seeded_home(&[stored]);
    let out = postbode(home.path(), &["search", "--bodies", "invoice"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        !stderr.contains("offline"),
        "asked the daemon with nothing to fetch: {stderr}"
    );
    assert!(
        !Paths::under(home.path()).daemon_socket().exists(),
        "started a daemon with nothing to fetch"
    );

    let (home, _store) = seeded_home(&[message(43, "friend@example.com", "Lunch?")]);
    let out = postbode(home.path(), &["search", "--bodies", "Lunch"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("work: work is offline (")
            && stderr.contains("searching the bodies already stored"),
        "{stderr}"
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("INBOX/43"));
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_speaks_only_json_rpc_on_stdout() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::time::Duration;

    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("config")).unwrap();
    std::fs::write(
        home.path().join("config/config.toml"),
        "[[accounts]]\nname = \"work\"\nhost = \"127.0.0.1\"\nport = 1\nusername = \"me@example.com\"\npassword = { command = \"printf x\" }\n",
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_postbode"))
        .args(["mcp", "--scopes", "read"])
        .env("POSTBODE_HOME", home.path())
        .env("RUST_LOG", "info")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let requests = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"folders","arguments":{}}}"#,
    ];
    let mut stdin = child.stdin.take().unwrap();
    for line in requests {
        writeln!(stdin, "{line}").unwrap();
    }
    // A reader thread lets the wait for id 3 time out instead of hanging the suite.
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in stdout.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let json_rpc = |line: &str| {
        let reply: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON-RPC: {line}: {e}"));
        assert_eq!(reply["jsonrpc"], "2.0", "{line}");
        reply
    };
    loop {
        let line = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("no reply with id 3");
        if json_rpc(&line)["id"] == 3 {
            break;
        }
    }
    drop(stdin);
    child.wait().unwrap();
    for line in rx {
        json_rpc(&line);
    }
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_install_rejects_an_unknown_account_and_accepts_none() {
    let home = tempfile::tempdir().unwrap();
    let out = postbode(
        home.path(),
        &["mcp", "install", "json", "--account", "nope"],
    );
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no account named 'nope'"));
    let out = postbode(home.path(), &["mcp", "install", "json"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("mcpServers"));
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_install_json_keeps_stdout_machine_readable() {
    let home = tempfile::tempdir().unwrap();
    let out = postbode(home.path(), &["mcp", "install", "json"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect(&stdout);
    assert_eq!(parsed["mcpServers"]["postbode"]["args"][0], "mcp");
    assert!(String::from_utf8_lossy(&out.stderr).contains("Scopes: read, rules:propose"));
}

/// A foreground `postbode run`, killed if a test fails before it is stopped.
struct Daemon(std::process::Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_daemon(home: &std::path::Path) -> Daemon {
    let child = Command::new(env!("CARGO_BIN_EXE_postbode"))
        .arg("run")
        .env("POSTBODE_HOME", home)
        .env("RUST_LOG", "error")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait_for_status(home, "work");
    Daemon(child)
}

/// Polls `postbode daemon status` for up to 10 s until its output contains `needle`.
fn wait_for_status(home: &std::path::Path, needle: &str) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let out = postbode(home, &["daemon", "status"]);
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        if stdout.contains(needle) {
            return stdout;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "status never showed {needle:?}: {stdout}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn archive_through_an_offline_account_reports_the_offline_reason() {
    let (home, _store) = seeded_home(&[message(1, "a@example.com", "Hello")]);
    let _daemon = start_daemon(home.path());
    wait_for_status(home.path(), "offline");
    for args in [
        &["archive", "1"][..],
        &["delete", "1"],
        &["mark", "unread", "1"],
    ] {
        let out = postbode(home.path(), args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?}: {stderr}");
        assert!(stderr.contains("work is offline ("), "{args:?}: {stderr}");
    }
    assert!(postbode(home.path(), &["daemon", "stop"]).status.success());
}

#[test]
fn daemon_status_shows_pid_version_and_accounts() {
    let (home, _store) = seeded_home(&[]);
    let daemon = start_daemon(home.path());
    let stdout = wait_for_status(home.path(), "work");
    let first = stdout.lines().next().unwrap();
    assert!(
        first.starts_with(&format!(
            "pid {}, version {}, up ",
            daemon.0.id(),
            env!("CARGO_PKG_VERSION")
        )) && first.ends_with(" clients"),
        "{stdout}"
    );
    assert!(
        stdout
            .lines()
            .skip(1)
            .any(|line| line.starts_with("work  ") && !line.contains("running")),
        "{stdout}"
    );
    assert!(postbode(home.path(), &["daemon", "stop"]).status.success());
}

#[test]
fn daemon_stop_ends_the_daemon() {
    let (home, _store) = seeded_home(&[]);
    let mut daemon = start_daemon(home.path());
    let out = postbode(home.path(), &["daemon", "stop"]);
    assert!(out.status.success());
    assert!(daemon.0.wait().unwrap().success());
    assert!(!Paths::under(home.path()).daemon_socket().exists());
    let out = postbode(home.path(), &["daemon", "status"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "no daemon running"
    );
}

#[test]
fn daemon_stop_without_a_daemon_says_so() {
    let (home, _store) = seeded_home(&[]);
    let out = postbode(home.path(), &["daemon", "stop"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "no daemon running"
    );
}

#[test]
fn run_refuses_a_second_daemon() {
    let (home, _store) = seeded_home(&[]);
    let daemon = start_daemon(home.path());
    let out = postbode(home.path(), &["run"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(
        stderr.contains(&format!("already running (pid {})", daemon.0.id())),
        "{stderr}"
    );
    assert!(postbode(home.path(), &["daemon", "stop"]).status.success());
}

#[test]
fn sync_through_an_offline_account_fails_with_the_offline_reason() {
    let (home, _store) = seeded_home(&[]);
    let out = postbode(home.path(), &["sync"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(
        stderr.contains("[work] error: work is offline ("),
        "{stderr}"
    );
}

const FAILED_PASS: &str = "rule 'x': invalid regex; running no rules until it is fixed";

/// Answers like a daemon of `version`: hello, subscribe, status, shutdown (which removes its socket), and a sync whose
/// pass failed with FAILED_PASS. Subscribers also get an error from an unrelated pass.
fn serve_fake_daemon(paths: &Paths, version: &'static str) -> std::thread::JoinHandle<()> {
    use postbode::daemon::wire::{
        self, ClientMessage, DaemonMessage, Outcome, PROTOCOL, Payload, Status,
    };
    use postbode::sync::Event;
    use std::io::{BufRead, BufReader};

    std::fs::create_dir_all(&paths.state_dir).unwrap();
    let socket = paths.daemon_socket();
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            for line in BufReader::new(stream.try_clone().unwrap()).lines() {
                let reply = match serde_json::from_str(&line.unwrap()).unwrap() {
                    ClientMessage::Hello { .. } => DaemonMessage::Hello {
                        protocol: PROTOCOL,
                        version: version.into(),
                        pid: 4242,
                    },
                    ClientMessage::Subscribe { id } => {
                        let done = DaemonMessage::Reply {
                            id,
                            outcome: Outcome::Ok(Payload::Done),
                        };
                        wire::write_line(&mut stream, &done).unwrap();
                        DaemonMessage::Event(Event::Error {
                            account: "work".into(),
                            message: "an unrelated pass failed".into(),
                        })
                    }
                    ClientMessage::Command { id, account, .. } => DaemonMessage::Reply {
                        id,
                        outcome: Outcome::Ok(Payload::Event(Event::Synced {
                            account,
                            new_messages: 0,
                            actions: 0,
                            requests: Vec::new(),
                            errors: vec![FAILED_PASS.into()],
                        })),
                    },
                    ClientMessage::Status { id } => DaemonMessage::Reply {
                        id,
                        outcome: Outcome::Ok(Payload::Status(Status {
                            pid: 4242,
                            version: version.into(),
                            uptime_secs: 0,
                            clients: 1,
                            accounts: Vec::new(),
                        })),
                    },
                    ClientMessage::Shutdown { id } => {
                        std::fs::remove_file(&socket).unwrap();
                        let done = DaemonMessage::Reply {
                            id,
                            outcome: Outcome::Ok(Payload::Done),
                        };
                        wire::write_line(&mut stream, &done).unwrap();
                        return;
                    }
                };
                wire::write_line(&mut stream, &reply).unwrap();
            }
        }
    })
}

#[test]
fn daemon_status_and_stop_reach_a_daemon_of_another_version() {
    let (home, _store) = seeded_home(&[]);
    let paths = Paths::under(home.path());
    let stale = serve_fake_daemon(&paths, "0.0.0-old");
    let out = postbode(home.path(), &["daemon", "status"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("pid 4242, version 0.0.0-old, up "),
        "{stdout}"
    );
    let out = postbode(home.path(), &["daemon", "stop"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    stale.join().unwrap();
    assert!(!paths.daemon_socket().exists());
}

#[test]
fn sync_prints_the_errors_of_its_own_pass_and_fails() {
    let (home, _store) = seeded_home(&[]);
    serve_fake_daemon(&Paths::under(home.path()), postbode::daemon::wire::VERSION);
    let out = postbode(home.path(), &["sync"]);
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(!out.status.success(), "{stdout}{stderr}");
    assert!(
        stdout.contains("[work] synced: 0 new, 0 rule actions"),
        "{stdout}"
    );
    assert!(
        stderr.contains(&format!("[work] error: {FAILED_PASS}")),
        "{stderr}"
    );
    assert!(!stderr.contains("unrelated"), "{stderr}");
}
