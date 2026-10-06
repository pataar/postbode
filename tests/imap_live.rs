//! Live IMAP tests against tests/dovecot/compose.yml, skipped unless POSTBODE_TEST_IMAP_HOST is set.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use postbode::config::{AccountConfig, PasswordSource};
use postbode::credentials::Secret;
use postbode::mail_ops::imap::ImapOps;
use postbode::mail_ops::{IdleOutcome, MailOps};

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
    let new = ops.fetch_new(1).unwrap();
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
    let uid = ops.fetch_new(1).unwrap()[0].uid;
    ops.move_message(uid, "Archive").unwrap();
    assert!(
        ops.fetch_new(1).unwrap().is_empty(),
        "message is still in INBOX"
    );
    ops.select("Archive").unwrap();
    assert_eq!(ops.fetch_new(1).unwrap().len(), 1);
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
        let uids: Vec<u32> = ops.fetch_new(1).unwrap().iter().map(|e| e.uid).collect();
        ops.add_flags(uids[0], &["\\Deleted"]).unwrap();
        ops.expunge(uids[0]).unwrap();
        let left: Vec<u32> = ops.fetch_new(1).unwrap().iter().map(|e| e.uid).collect();
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
