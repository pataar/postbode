//! Live IMAP tests against tests/dovecot/compose.yml, skipped unless POSTBODE_TEST_IMAP_HOST is set.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use postbode::config::{AccountConfig, Config, PasswordSource};
use postbode::credentials::Secret;
use postbode::mail_ops::imap::ImapOps;
use postbode::mail_ops::{Envelope, IdleOutcome, MailOps};
use postbode::paths::Paths;
use postbode::store::Store;
use postbode::sync::{self, Event};

const PASSWORD: &str = "postbode-test";
const PORT: u16 = 10993;
const PORT_WITHOUT_MOVE: u16 = 11993;

/// CI sets the variable to an empty string where no server runs; that counts as unset.
fn host() -> Option<String> {
    let host = std::env::var("POSTBODE_TEST_IMAP_HOST")
        .ok()
        .filter(|h| !h.is_empty());
    if host.is_none() {
        eprintln!("POSTBODE_TEST_IMAP_HOST unset; live IMAP test skipped");
    }
    host
}

fn all_envelopes(ops: &mut ImapOps) -> Vec<Envelope> {
    let uids = ops.search_uids(1).unwrap();
    match (uids.first(), uids.last()) {
        (Some(&first), Some(&last)) => ops.fetch_envelopes(first, last).unwrap(),
        _ => Vec::new(),
    }
}

/// A fresh user per call, so no two tests (or reruns) share a mailbox; Dovecot accepts any name.
fn account(host: &str, port: u16, test: &str) -> AccountConfig {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    AccountConfig {
        name: "live".into(),
        host: host.into(),
        port,
        username: format!("{test}-{nanos}@example.com"),
        password: PasswordSource::Command {
            command: format!("printf {PASSWORD}"),
        },
        address: None,
        aliases: vec![],
        sync_interval_secs: 120,
        trash_retention_days: 30,
        notify: false,
        ca_file: Some(PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/dovecot/certs/ca.pem"
        ))),
    }
}

fn connect(account: &AccountConfig) -> ImapOps {
    ImapOps::connect(account, &Secret::new(PASSWORD.into())).unwrap_or_else(|e| panic!("{e}"))
}

fn mail(subject: &str) -> Vec<u8> {
    format!(
        "From: Sender <sender@example.com>\r\nTo: user@example.com\r\nSubject: {subject}\r\n\
         Message-ID: <{}@example.com>\r\nDate: Tue, 06 Oct 2026 10:00:00 +0000\r\n\r\nBody of {subject}\r\n",
        subject.replace(' ', ".")
    )
    .into_bytes()
}

#[test]
fn lists_special_use_folders() {
    let Some(host) = host() else { return };
    let folders = connect(&account(&host, PORT, "special-use"))
        .list_folders()
        .unwrap();
    for role in ["Archive", "Drafts", "Junk", "Sent", "Trash"] {
        assert!(
            folders
                .iter()
                .any(|f| f.special_use.as_deref() == Some(role)),
            "{role}: {folders:?}"
        );
    }
}

#[test]
fn append_fetch_and_flags_round_trip() {
    let Some(host) = host() else { return };
    let mut ops = connect(&account(&host, PORT, "flags"));
    ops.append("INBOX", &mail("hello"), &["$PostbodeRestored"])
        .unwrap();
    ops.select("INBOX").unwrap();
    let new = all_envelopes(&mut ops);
    assert_eq!(new.len(), 1);
    assert!(
        new[0].flags.iter().any(|f| f == "$PostbodeRestored"),
        "{:?}",
        new[0].flags
    );
    let uid = new[0].uid;
    ops.add_flags(uid, &["\\Seen", "\\Flagged"]).unwrap();
    ops.remove_flags(uid, &["\\Flagged"]).unwrap();
    let flags = ops.fetch_flags(uid).unwrap().remove(0).flags;
    assert!(flags.iter().any(|f| f == "\\Seen"), "{flags:?}");
    assert!(!flags.iter().any(|f| f == "\\Flagged"), "{flags:?}");
    assert_eq!(ops.fetch_raw(uid).unwrap().unwrap(), mail("hello"));
}

fn move_round_trip(port: u16, test: &str, expect_move: bool) {
    let Some(host) = host() else { return };
    let mut ops = connect(&account(&host, port, test));
    assert_eq!(
        ops.has_move(),
        expect_move,
        "server capabilities differ from compose.yml"
    );
    ops.append("INBOX", &mail("move me"), &[]).unwrap();
    ops.select("INBOX").unwrap();
    let uid = all_envelopes(&mut ops)[0].uid;
    ops.move_message(uid, "Archive").unwrap();
    assert!(
        all_envelopes(&mut ops).is_empty(),
        "message is still in INBOX"
    );
    ops.select("Archive").unwrap();
    assert_eq!(all_envelopes(&mut ops).len(), 1);
}

#[test]
fn move_uses_move_when_advertised() {
    move_round_trip(PORT, "move", true);
}

#[test]
fn move_falls_back_to_copy_without_move() {
    move_round_trip(PORT_WITHOUT_MOVE, "copy", false);
}

#[test]
fn expunge_removes_only_the_deleted_message() {
    let Some(host) = host() else { return };
    for (port, test) in [
        (PORT, "expunge-uidplus"),
        (PORT_WITHOUT_MOVE, "expunge-plain"),
    ] {
        let mut ops = connect(&account(&host, port, test));
        ops.append("INBOX", &mail("first"), &[]).unwrap();
        ops.append("INBOX", &mail("second"), &[]).unwrap();
        ops.select("INBOX").unwrap();
        let uids: Vec<u32> = all_envelopes(&mut ops).iter().map(|e| e.uid).collect();
        ops.add_flags(uids[0], &["\\Deleted"]).unwrap();
        ops.expunge(uids[0]).unwrap();
        let left: Vec<u32> = all_envelopes(&mut ops).iter().map(|e| e.uid).collect();
        assert_eq!(left, vec![uids[1]], "{test}");
    }
}

#[test]
fn idle_wakes_when_mail_arrives() {
    let Some(host) = host() else { return };
    let account = account(&host, PORT, "idle");
    let mut ops = connect(&account);
    ops.select("INBOX").unwrap();
    let sender = account.clone();
    let delivery = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        connect(&sender)
            .append("INBOX", &mail("ping"), &[])
            .unwrap();
    });
    let started = Instant::now();
    let outcome = ops
        .idle(Duration::from_secs(60), &AtomicBool::new(false))
        .unwrap();
    delivery.join().unwrap();
    assert_eq!(outcome, IdleOutcome::NewMail);
    assert!(started.elapsed() < Duration::from_secs(30));
}

/// Mail newer than a rule's first pass is what the rule acts on; INTERNALDATE has one-second resolution.
fn wait_past_the_rule_clock() {
    std::thread::sleep(Duration::from_millis(1100));
}

fn postbode(home: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_postbode"))
        .args(args)
        .env("POSTBODE_HOME", home)
        .env("RUST_LOG", "error")
        .output()
        .unwrap()
}

fn home_with(account: &AccountConfig, rules: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::under(home.path());
    Config {
        accounts: vec![account.clone()],
    }
    .save(&paths.config_file())
    .unwrap();
    if !rules.is_empty() {
        std::fs::write(paths.rules_file(), rules).unwrap();
    }
    home
}

#[test]
fn sync_resets_a_folder_whose_uidvalidity_changed() {
    let Some(host) = host() else { return };
    let mut ops = connect(&account(&host, PORT, "uidvalidity"));
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("mail.db")).unwrap();
    ops.create_folder("Lists").unwrap();
    ops.append("Lists", &mail("before"), &[]).unwrap();
    sync::sync_all(&mut ops, &store).unwrap();
    let before = store.folder("Lists").unwrap().unwrap().uidvalidity;

    ops.select("INBOX").unwrap();
    ops.delete_folder("Lists").unwrap();
    // Dovecot derives UIDVALIDITY from the clock.
    std::thread::sleep(Duration::from_millis(1100));
    ops.create_folder("Lists").unwrap();
    ops.append("Lists", &mail("after"), &[]).unwrap();
    let (_, errors) = sync::sync_all(&mut ops, &store).unwrap();
    assert!(errors.is_empty(), "{errors:?}");

    assert_ne!(store.folder("Lists").unwrap().unwrap().uidvalidity, before);
    let subjects: Vec<Option<String>> = store
        .messages_in_folder("Lists")
        .unwrap()
        .into_iter()
        .map(|m| m.subject)
        .collect();
    assert_eq!(subjects, vec![Some("after".to_string())]);
}

#[test]
fn rule_delete_keeps_a_backup_then_expunges() {
    let Some(host) = host() else { return };
    let account = account(&host, PORT, "rule-delete");
    let home = home_with(
        &account,
        "[[rules]]\nname = \"codes\"\nmatch.subject = { contains = \"sign-in code\" }\nactions = [\"delete\"]\n",
    );
    let paths = Paths::under(home.path());
    let (events, received) = std::sync::mpsc::channel();
    sync::run_once(&account, &paths, &events).unwrap();
    wait_past_the_rule_clock();
    connect(&account)
        .append("INBOX", &mail("Your sign-in code"), &[])
        .unwrap();
    sync::run_once(&account, &paths, &events).unwrap();
    drop(events);
    let errors: Vec<Event> = received
        .iter()
        .filter(|e| matches!(e, Event::Error { .. }))
        .collect();
    assert!(errors.is_empty(), "{errors:?}");

    let backups = std::fs::read_dir(paths.trash_dir(&account.name))
        .unwrap()
        .count();
    assert_eq!(backups, 1, "expected one .eml backup");
    let mut ops = connect(&account);
    for folder in ["INBOX", "Trash"] {
        ops.select(folder).unwrap();
        assert!(all_envelopes(&mut ops).is_empty(), "{folder} is not empty");
    }
}

#[test]
fn cli_delete_moves_to_the_trash_folder() {
    let Some(host) = host() else { return };
    let account = account(&host, PORT, "cli-delete");
    let home = home_with(&account, "");
    connect(&account)
        .append("INBOX", &mail("old newsletter"), &[])
        .unwrap();
    for args in [&["sync"][..], &["delete", "1"][..]] {
        let out = postbode(home.path(), args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let mut ops = connect(&account);
    ops.select("INBOX").unwrap();
    assert!(all_envelopes(&mut ops).is_empty(), "still in INBOX");
    ops.select("Trash").unwrap();
    assert_eq!(all_envelopes(&mut ops).len(), 1);
}

#[test]
fn sync_exits_nonzero_when_a_rule_fails_on_a_message() {
    let Some(host) = host() else { return };
    let account = account(&host, PORT, "cli-sync-error");
    // Dovecot refuses a 300-character mailbox name, so the move fails for this message only.
    let target = "x".repeat(300);
    let home = home_with(
        &account,
        &format!(
            "[[rules]]\nname = \"impossible\"\nmatch.subject = {{ contains = \"move me\" }}\nactions = [{{ move = \"{target}\" }}]\n"
        ),
    );
    let first = postbode(home.path(), &["sync"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    wait_past_the_rule_clock();
    connect(&account)
        .append("INBOX", &mail("please move me"), &[])
        .unwrap();
    let out = postbode(home.path(), &["sync"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(
        stderr.contains("sync failed for at least one account or folder"),
        "{stderr}"
    );
}

#[test]
fn account_add_accepts_a_private_ca_file() {
    use std::io::Write;
    use std::process::Stdio;

    let Some(host) = host() else { return };
    let template = account(&host, PORT, "account-add");
    let ca_file = template.ca_file.as_ref().unwrap().display().to_string();
    let home = tempfile::tempdir().unwrap();
    let answers = format!(
        "live\n{host}\n{PORT}\n{ca_file}\n{}\n\nc\nprintf {PASSWORD}\n",
        template.username
    );
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_postbode"))
        .args(["account", "add"])
        .env("POSTBODE_HOME", home.path())
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
        .write_all(answers.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let saved = Config::load(&Paths::under(home.path()).config_file()).unwrap();
    assert_eq!(saved.accounts[0].ca_file, template.ca_file);
}
