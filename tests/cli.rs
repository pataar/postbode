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
