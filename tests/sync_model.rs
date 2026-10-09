//! Model-based tests for sync and rules against `RecordingOps` (issue #24).
//!
//! Proptest generates bounded runs of server-side events (new mail, deletes, moves, flag changes, UIDVALIDITY
//! resets), clock ticks, a rule added later, and sync passes. A pass may get one injected failure (a NO answer, or a
//! dropped connection, optionally after the server already ran the command) and one server event between its sync and
//! its rules. The test drives the same public steps a daemon pass takes (`load_rules_for`, `sync_all` or
//! `sync_folder` on INBOX, then `run_rules`) through `Harness`, a `MailOps` wrapper around the fake that injects the
//! failures and checks every command the code sends. All mail is made up.
//!
//! Not modelled: the daemon's command queue and IDLE (the checkpoint closure is a no-op here), body rules, restore
//! from trash, user deletes, folders deleted on the server, and more than one account.
//!
//! The default case count keeps `cargo test` fast. For a deeper search:
//!
//! ```sh
//! PROPTEST_CASES=2000 cargo test --features testing --test sync_model
//! ```

// Test code: unwrap, expect and panic are how a test fails.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use postvak::config::{AccountConfig, Identity, PasswordSource};
use postvak::mail_ops::{
    Envelope, FlagUpdate, IdleOutcome, MailError, MailOps, MailResult, RecordingOps, RemoteFolder,
    SelectInfo,
};
use postvak::rules::engine::Mode;
use postvak::store::Store;
use postvak::sync::{Event, RulesRun, load_rules_for, run_rules, sync_all, sync_folder};
use postvak::trash::Trash;
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};

const T0: i64 = 1_790_000_000;
const HOUR: i64 = 3600;
/// Server folders by index. Newsletters exists only once the `newsletters` rule created it.
const FOLDERS: [&str; 4] = ["INBOX", "Lists", "Archive", "Newsletters"];

const BASE_RULES: &str = r#"
[[rules]]
name = "newsletters"
match.from = { equals = "news@shop.example" }
actions = [{ move = "Newsletters" }]

[[rules]]
name = "ci-read"
match.from = { equals = "bot@ci.example" }
actions = ["mark_read"]

[[rules]]
name = "codes"
match.subject = { contains = "sign-in code" }
match.older_than = "1h"
actions = ["delete"]

[[rules]]
name = "news-read"
folder = "Newsletters"
match.from = { equals = "news@shop.example" }
actions = ["mark_read"]
"#;

/// Added by a step, so it starts its clock in the middle of a run with older mail already present.
const ALERTS_RULE: &str = r#"
[[rules]]
name = "alerts"
match.from = { equals = "alerts@mon.example" }
actions = ["flag", "archive"]
"#;

const BASE_RULE_NAMES: [&str; 4] = ["newsletters", "ci-read", "codes", "news-read"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sender {
    News,
    Bot,
    Alerts,
    Codes,
    Friend,
}

impl Sender {
    const ALL: [Sender; 5] = [
        Sender::News,
        Sender::Bot,
        Sender::Alerts,
        Sender::Codes,
        Sender::Friend,
    ];

    fn address(self) -> &'static str {
        match self {
            Sender::News => "news@shop.example",
            Sender::Bot => "bot@ci.example",
            Sender::Alerts => "alerts@mon.example",
            Sender::Codes => "noreply@login.example",
            Sender::Friend => "alice@friends.example",
        }
    }

    fn subject(self) -> &'static str {
        match self {
            Sender::Codes => "Your sign-in code",
            Sender::News => "This week's deals",
            Sender::Bot => "Build passed",
            Sender::Alerts => "Disk almost full",
            Sender::Friend => "Lunch?",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flag {
    Seen,
    Flagged,
}

impl Flag {
    fn imap(self) -> &'static str {
        match self {
            Flag::Seen => "\\Seen",
            Flag::Flagged => "\\Flagged",
        }
    }
}

/// Something another client or the server does. `pick` chooses a message by index over all server mail, wrapping.
#[derive(Clone, Debug)]
enum ServerEvent {
    Arrive {
        sender: Sender,
        folder: usize,
        age_min: i64,
        seen: bool,
    },
    Delete {
        pick: usize,
    },
    Move {
        pick: usize,
        to: usize,
    },
    SetFlag {
        pick: usize,
        flag: Flag,
        on: bool,
    },
    /// The folder gets a new UIDVALIDITY and its messages new uids, reusing low ones in a different order.
    Reset {
        folder: usize,
    },
}

#[derive(Clone, Copy, Debug)]
enum Failure {
    /// The `after`-th command of the pass gets NO and does nothing; the connection stays usable.
    Command { after: usize },
    /// The connection drops at the `after`-th command; with `effect` the server ran it first. Every later command in
    /// the pass fails too, and the next pass starts on a new connection.
    Lost { after: usize, effect: bool },
}

#[derive(Clone, Debug)]
enum Step {
    Server(ServerEvent),
    Tick {
        minutes: i64,
    },
    AddAlertsRule,
    /// `full` syncs every folder, else INBOX only, like a pass after an IDLE push. `mid` happens between sync and rules.
    Pass {
        full: bool,
        failure: Option<Failure>,
        mid: Option<ServerEvent>,
    },
}

/// What the test knows about a generated message, keyed by its unique Message-ID.
struct Meta {
    sender: Sender,
    internaldate: i64,
    raw: Vec<u8>,
}

enum Trip {
    Pass,
    Fail(MailError),
    EffectThenFail,
}

fn lost() -> MailError {
    MailError::Io("connection lost (injected)".into())
}

fn message_id(headers: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(headers);
    let start = text.find("Message-ID: <")? + "Message-ID: <".len();
    let end = start + text[start..].find('>')?;
    Some(text[start..end].to_string())
}

/// The fake server plus the model: what every message is, which rules exist since when, and what the code did.
struct Harness<'a> {
    ops: RecordingOps,
    store: &'a Store,
    trash_dir: PathBuf,
    now: i64,
    passes: usize,
    next_id: usize,
    metas: HashMap<String, Meta>,
    /// Bumped by every server event touching the message; re-applying an action is allowed only after one.
    epoch: HashMap<String, u32>,
    /// Counts each message's arrivals in INBOX; notifications are judged per arrival.
    inbox_entry: HashMap<String, u32>,
    /// Arrivals in INBOX that may notify: after the first pass started and not swept up by a later INBOX reset.
    may_notify: HashSet<(String, u32)>,
    notified: HashSet<(String, u32)>,
    /// When each rule was first loaded, by the model's clock: the now of the first pass that saw it in the file.
    first_seen: HashMap<&'static str, i64>,
    alerts_in_file: bool,
    /// Every (folder, uidvalidity, uid) the code fetched an envelope for, and whose it was. IMAP never reuses one.
    uid_map: HashMap<(String, u32, u32), String>,
    /// Successful rule commands per (message, what, epoch).
    applied: HashMap<(String, String, u32), usize>,
    /// Messages a successful `\Deleted` store was sent for.
    deleted: HashSet<String>,
    /// Messages a rule moved or deleted in the current pass.
    moved_or_deleted: HashSet<String>,
    // Connection state.
    selected: Option<String>,
    failure: Option<Failure>,
    calls: usize,
    tripped: bool,
    dead: bool,
}

impl<'a> Harness<'a> {
    fn new(store: &'a Store, trash_dir: PathBuf) -> Harness<'a> {
        let ops = RecordingOps::new()
            .with_folder("INBOX", None)
            .with_folder("Lists", None)
            .with_folder("Archive", Some("Archive"));
        Harness {
            ops,
            store,
            trash_dir,
            now: T0,
            passes: 0,
            next_id: 1,
            metas: HashMap::new(),
            epoch: HashMap::new(),
            inbox_entry: HashMap::new(),
            may_notify: HashSet::new(),
            notified: HashSet::new(),
            first_seen: HashMap::new(),
            alerts_in_file: false,
            uid_map: HashMap::new(),
            applied: HashMap::new(),
            deleted: HashSet::new(),
            moved_or_deleted: HashSet::new(),
            selected: None,
            failure: None,
            calls: 0,
            tripped: false,
            dead: false,
        }
    }

    // ---- the server side -------------------------------------------------------------------------------------

    fn identity(&self, folder: &str, uid: u32) -> Option<String> {
        self.ops
            .mail
            .get(folder)?
            .iter()
            .find(|e| e.uid == uid)
            .and_then(|e| message_id(&e.headers))
    }

    fn all_mail(&self) -> Vec<(String, u32, String)> {
        let mut all: Vec<(String, u32, String)> = self
            .ops
            .mail
            .iter()
            .flat_map(|(folder, list)| {
                list.iter()
                    .map(|e| (folder.clone(), e.uid, message_id(&e.headers).unwrap()))
            })
            .collect();
        all.sort();
        all
    }

    fn pick(&self, pick: usize) -> Option<(String, u32, String)> {
        let all = self.all_mail();
        (!all.is_empty()).then(|| all[pick % all.len()].clone())
    }

    fn bump(&mut self, id: &str) {
        *self.epoch.entry(id.to_string()).or_default() += 1;
    }

    fn enter_inbox(&mut self, id: &str) {
        let entry = self.inbox_entry.entry(id.to_string()).or_default();
        *entry += 1;
        let key = (id.to_string(), *entry);
        if self.passes > 0 {
            self.may_notify.insert(key);
        }
    }

    /// Another client's commands leave our connection's selected folder alone.
    fn reselect(&mut self) {
        if let Some(folder) = self.selected.clone() {
            self.ops.select(&folder).unwrap();
        }
    }

    fn server(&mut self, event: &ServerEvent) {
        match *event {
            ServerEvent::Arrive {
                sender,
                folder,
                age_min,
                seen,
            } => {
                let folder = FOLDERS[folder];
                if !self.ops.mail.contains_key(folder) {
                    return;
                }
                let id = format!("m{}@model.example", self.next_id);
                self.next_id += 1;
                let raw = format!(
                    "From: {}\r\nTo: me@model.example\r\nSubject: {}\r\nMessage-ID: <{id}>\r\n\r\nMade-up body of {id}.\r\n",
                    sender.address(),
                    sender.subject()
                );
                let flags: &[&str] = if seen { &["\\Seen"] } else { &[] };
                self.ops.append(folder, raw.as_bytes(), flags).unwrap();
                let internaldate = self.now - age_min * 60;
                self.ops
                    .mail
                    .get_mut(folder)
                    .unwrap()
                    .last_mut()
                    .unwrap()
                    .internaldate = internaldate;
                self.metas.insert(
                    id.clone(),
                    Meta {
                        sender,
                        internaldate,
                        raw: raw.into_bytes(),
                    },
                );
                if folder == "INBOX" {
                    self.enter_inbox(&id);
                }
            }
            ServerEvent::Delete { pick } => {
                let Some((folder, uid, id)) = self.pick(pick) else {
                    return;
                };
                self.ops
                    .mail
                    .get_mut(&folder)
                    .unwrap()
                    .retain(|e| e.uid != uid);
                self.ops.raw.remove(&(folder, uid));
                self.bump(&id);
            }
            ServerEvent::Move { pick, to } => {
                let Some((folder, uid, id)) = self.pick(pick) else {
                    return;
                };
                let to = FOLDERS[to];
                if to == folder || !self.ops.mail.contains_key(to) {
                    return;
                }
                self.ops.select(&folder).unwrap();
                self.ops.move_message(uid, to).unwrap();
                self.reselect();
                self.bump(&id);
                if to == "INBOX" {
                    self.enter_inbox(&id);
                }
            }
            ServerEvent::SetFlag { pick, flag, on } => {
                let Some((folder, uid, id)) = self.pick(pick) else {
                    return;
                };
                let env = self
                    .ops
                    .mail
                    .get_mut(&folder)
                    .unwrap()
                    .iter_mut()
                    .find(|e| e.uid == uid)
                    .unwrap();
                env.flags.retain(|f| f != flag.imap());
                if on {
                    env.flags.push(flag.imap().to_string());
                }
                self.bump(&id);
            }
            ServerEvent::Reset { folder } => {
                let folder = FOLDERS[folder];
                let Some(list) = self.ops.mail.get_mut(folder) else {
                    return;
                };
                // The same messages, renumbered from 1 in reverse order, so old uids now name other messages.
                list.sort_by_key(|e| std::cmp::Reverse(e.uid));
                let mut moved_raw = Vec::new();
                for (i, env) in list.iter_mut().enumerate() {
                    let raw = self.ops.raw.remove(&(folder.to_string(), env.uid));
                    env.uid = i as u32 + 1;
                    moved_raw.push((env.uid, raw));
                }
                list.sort_by_key(|e| e.uid);
                for (uid, raw) in moved_raw {
                    if let Some(raw) = raw {
                        self.ops.raw.insert((folder.to_string(), uid), raw);
                    }
                }
                *self.ops.uidvalidity.get_mut(folder).unwrap() += 1;
                if folder == "INBOX" {
                    // Spec section 8: mail found by a UIDVALIDITY resync never notifies.
                    let ids: Vec<String> = self.ops.mail["INBOX"]
                        .iter()
                        .map(|e| message_id(&e.headers).unwrap())
                        .collect();
                    for id in ids {
                        let entry = self.inbox_entry.get(&id).copied().unwrap_or(0);
                        self.may_notify.remove(&(id, entry));
                    }
                }
                self.reselect();
            }
        }
    }

    // ---- failure injection -----------------------------------------------------------------------------------

    fn arm(&mut self, failure: Option<Failure>) {
        self.failure = failure;
        self.calls = 0;
        self.tripped = false;
        self.dead = false;
    }

    /// The next pass runs on a fresh connection.
    fn disarm(&mut self) {
        self.failure = None;
        self.dead = false;
        self.selected = None;
    }

    fn trip(&mut self) -> Trip {
        if self.dead {
            return Trip::Fail(lost());
        }
        let Some(failure) = self.failure else {
            return Trip::Pass;
        };
        let (Failure::Command { after } | Failure::Lost { after, .. }) = failure;
        if self.calls < after {
            self.calls += 1;
            return Trip::Pass;
        }
        self.failure = None;
        self.tripped = true;
        match failure {
            Failure::Command { .. } => Trip::Fail(MailError::Protocol("NO (injected)".into())),
            Failure::Lost { effect, .. } => {
                self.dead = true;
                if effect {
                    Trip::EffectThenFail
                } else {
                    Trip::Fail(lost())
                }
            }
        }
    }

    fn run<T>(&mut self, f: impl FnOnce(&mut RecordingOps) -> MailResult<T>) -> MailResult<T> {
        match self.trip() {
            Trip::Pass => f(&mut self.ops),
            Trip::Fail(e) => Err(e),
            Trip::EffectThenFail => {
                let _ = f(&mut self.ops);
                Err(lost())
            }
        }
    }

    /// Like a real server, a command that needs a selected folder fails without one, e.g. after a failed SELECT.
    fn run_selected<T>(
        &mut self,
        f: impl FnOnce(&mut RecordingOps) -> MailResult<T>,
    ) -> MailResult<T> {
        if self.selected.is_none() && !self.dead {
            return Err(MailError::Protocol("no mailbox selected".into()));
        }
        self.run(f)
    }

    // ---- per-command checks ----------------------------------------------------------------------------------

    /// Checks a rule command before it is sent. `what` is a flag being set or `move:<folder>`.
    fn check_rule_command(&self, uid: u32, what: &str) {
        let folder = self
            .selected
            .clone()
            .expect("a rule command needs a selected folder");
        // A command for a uid the server does not have gets NO from the fake and changes nothing.
        let Some(id) = self.identity(&folder, uid) else {
            return;
        };
        // Invariant: rules act on the message they evaluated, never on whatever now carries that uid.
        let row = self.store.message(&folder, uid).unwrap();
        assert_eq!(
            row.and_then(|m| m.message_id).as_deref(),
            Some(id.as_str()),
            "{what} on {folder}/{uid} hits a different message than the store row"
        );
        let (rule, sender) = match (folder.as_str(), what) {
            ("INBOX", "\\Seen") => ("ci-read", Sender::Bot),
            ("Newsletters", "\\Seen") => ("news-read", Sender::News),
            ("INBOX", "\\Flagged") => ("alerts", Sender::Alerts),
            ("INBOX", "\\Deleted") => ("codes", Sender::Codes),
            ("INBOX", "move:Newsletters") => ("newsletters", Sender::News),
            ("INBOX", "move:Archive") => ("alerts", Sender::Alerts),
            _ => panic!("no rule does {what} in {folder} (message {id})"),
        };
        let meta = &self.metas[&id];
        assert_eq!(meta.sender, sender, "{rule} acted on {id}");
        // Invariant (spec section 6): a rule only acts on mail whose internaldate is at or after its first_seen_at.
        let first_seen = *self
            .first_seen
            .get(rule)
            .unwrap_or_else(|| panic!("{rule} acted before it was loaded"));
        assert!(
            meta.internaldate >= first_seen,
            "{rule} acted on {id} from {} although the rule exists since {first_seen}",
            meta.internaldate
        );
        if what == "\\Deleted" {
            assert!(
                self.now - meta.internaldate >= HOUR,
                "codes deleted {id} before it was an hour old"
            );
            // Invariant (spec section 8): the .eml backup exists, with this message's bytes, before the delete.
            let suffix = format!("-{folder}-{uid}.eml");
            let backed_up = std::fs::read_dir(&self.trash_dir)
                .map(|dir| {
                    dir.flatten().any(|f| {
                        f.file_name().to_string_lossy().ends_with(&suffix)
                            && std::fs::read(f.path()).is_ok_and(|bytes| bytes == meta.raw)
                    })
                })
                .unwrap_or(false);
            assert!(
                backed_up,
                "no backup of {id} before deleting {folder}/{uid}"
            );
        }
    }

    /// Records a rule command the server accepted. Invariant: within one epoch of a message, each action succeeds once.
    fn record(&mut self, folder: &str, uid: u32, what: &str, id: Option<String>) {
        let Some(id) = id else { return };
        let epoch = self.epoch.get(&id).copied().unwrap_or(0);
        let count = self
            .applied
            .entry((id.clone(), format!("{folder}:{what}"), epoch))
            .or_default();
        *count += 1;
        assert_eq!(
            *count, 1,
            "{what} on {id} ({folder}/{uid}) succeeded twice with no server change in between"
        );
        if what == "expunge" {
            self.deleted.insert(id.clone());
        }
        if what == "expunge" || what.starts_with("move:") {
            self.moved_or_deleted.insert(id);
        }
    }

    // ---- checks after a step ---------------------------------------------------------------------------------

    /// Invariant 1, exact: after a sync that saw no failure and no concurrent change, the store mirrors the server:
    /// same folders (full pass), same UIDVALIDITY, same uids, same messages under them, same flags.
    fn check_mirror(&self, full: bool) {
        let server_folders: BTreeSet<String> = self.ops.mail.keys().cloned().collect();
        if full {
            let stored: BTreeSet<String> = self
                .store
                .folders()
                .unwrap()
                .into_iter()
                .map(|f| f.name)
                .collect();
            assert_eq!(
                stored, server_folders,
                "folders differ after a clean full sync"
            );
        }
        let checked: Vec<&str> = if full {
            server_folders.iter().map(String::as_str).collect()
        } else {
            vec!["INBOX"]
        };
        for folder in checked {
            let stored = self
                .store
                .folder(folder)
                .unwrap()
                .expect("synced folder is stored");
            assert_eq!(
                stored.uidvalidity, self.ops.uidvalidity[folder],
                "{folder}: UIDVALIDITY after a clean sync"
            );
            let server: BTreeMap<u32, (String, BTreeSet<String>)> = self.ops.mail[folder]
                .iter()
                .map(|e| {
                    let flags = e.flags.iter().cloned().collect();
                    (e.uid, (message_id(&e.headers).unwrap(), flags))
                })
                .collect();
            let local: BTreeMap<u32, (String, BTreeSet<String>)> = self
                .store
                .messages_in_folder(folder)
                .unwrap()
                .into_iter()
                .map(|m| {
                    let flags = m
                        .flags
                        .split(' ')
                        .filter(|f| !f.is_empty())
                        .map(str::to_string)
                        .collect();
                    (m.uid, (m.message_id.unwrap_or_default(), flags))
                })
                .collect();
            assert_eq!(
                local, server,
                "{folder}: store differs from server after a clean sync"
            );
        }
    }

    /// Holds after every step, failures or not: in a folder whose stored UIDVALIDITY is the server's, every row names
    /// the message the server had at that uid when it was fetched. Rows may be stale (deleted or moved since), never
    /// wrong.
    ///
    /// A folder whose stored UIDVALIDITY differs from the server's is exempt: the store already knows it is stale,
    /// rules and direct actions refuse to touch it, and its next sync wipes it.
    fn check_rows_name_their_messages(&self) {
        for folder in self.store.folders().unwrap() {
            if self.ops.uidvalidity.get(&folder.name) != Some(&folder.uidvalidity) {
                continue;
            }
            for m in self.store.messages_in_folder(&folder.name).unwrap() {
                let key = (folder.name.clone(), folder.uidvalidity, m.uid);
                assert_eq!(
                    self.uid_map.get(&key),
                    m.message_id.as_ref(),
                    "{}/{} under UIDVALIDITY {} is stored as the wrong message",
                    folder.name,
                    m.uid,
                    folder.uidvalidity
                );
            }
        }
    }

    /// Invariant 4 on the rule log, which also records attempts that failed: no rule touched mail older than itself.
    fn check_rule_log(&self) {
        for entry in self.store.log(u32::MAX).unwrap() {
            let id = entry.message_id.expect("generated mail has a Message-ID");
            let first_seen = self.first_seen[entry.rule_name.as_str()];
            assert!(
                self.metas[&id].internaldate >= first_seen,
                "rule_log: {} {} on {id}, older than the rule",
                entry.rule_name,
                entry.action
            );
        }
    }

    /// Notifications (spec section 8): only INBOX, only mail that arrived after the first sync and not swept up by a
    /// resync, at most once per arrival, never for mail a rule moved or deleted.
    fn check_events(&mut self, events: &[Event]) {
        let Some(inbox) = self.store.folder("INBOX").unwrap() else {
            return;
        };
        for event in events {
            let Event::NewMail { folder, uid, .. } = event else {
                continue;
            };
            assert_eq!(folder, "INBOX");
            let id = self.uid_map[&(folder.clone(), inbox.uidvalidity, *uid)].clone();
            let entry = (id.clone(), self.inbox_entry.get(&id).copied().unwrap_or(0));
            assert!(
                self.may_notify.contains(&entry),
                "notified {id}, which was in INBOX before the first sync or a resync"
            );
            assert!(self.notified.insert(entry), "notified {id} twice");
            assert!(
                !self.moved_or_deleted.contains(&id),
                "notified {id} although a rule moved or deleted it"
            );
            let meta = &self.metas[&id];
            assert!(
                !(meta.sender == Sender::News
                    && meta.internaldate >= self.first_seen["newsletters"]),
                "notified {id}, which the newsletters rule moves"
            );
        }
    }

    // ---- final checks ----------------------------------------------------------------------------------------

    /// After quiet passes the rules have done everything they promise for the mail on the server.
    fn check_rules_caught_up(&self) {
        for (folder, list) in &self.ops.mail {
            for env in list {
                let id = message_id(&env.headers).unwrap();
                let meta = &self.metas[&id];
                let after = |rule: &str| {
                    self.first_seen
                        .get(rule)
                        .is_some_and(|&t| meta.internaldate >= t)
                };
                let seen = env.flags.iter().any(|f| f == "\\Seen");
                let stuck = match (folder.as_str(), meta.sender) {
                    ("INBOX", Sender::News) => after("newsletters"),
                    ("INBOX", Sender::Bot) => after("ci-read") && !seen,
                    ("Newsletters", Sender::News) => after("news-read") && !seen,
                    ("INBOX", Sender::Alerts) => after("alerts"),
                    ("INBOX", Sender::Codes) => {
                        after("codes") && self.now - meta.internaldate >= HOUR
                    }
                    _ => false,
                };
                assert!(
                    !stuck,
                    "{id} in {folder} still waits for its rule after quiet passes"
                );
            }
        }
    }

    /// Every message a rule deleted still has its backup, with its bytes.
    fn check_backups_kept(&self) {
        let backups: Vec<Vec<u8>> = std::fs::read_dir(&self.trash_dir)
            .map(|dir| {
                dir.flatten()
                    .filter_map(|f| std::fs::read(f.path()).ok())
                    .collect()
            })
            .unwrap_or_default();
        for id in &self.deleted {
            let present = self.all_mail().iter().any(|(_, _, other)| other == id);
            if present {
                continue;
            }
            assert!(
                backups.contains(&self.metas[id].raw),
                "the backup of deleted {id} is gone"
            );
        }
    }
}

impl MailOps for Harness<'_> {
    fn list_folders(&mut self) -> MailResult<Vec<RemoteFolder>> {
        self.run(|ops| ops.list_folders())
    }

    fn select(&mut self, folder: &str) -> MailResult<SelectInfo> {
        let result = self.run(|ops| ops.select(folder));
        self.selected = result.is_ok().then(|| folder.to_string());
        result
    }

    fn search_uids(&mut self, from_uid: u32) -> MailResult<Vec<u32>> {
        self.run_selected(|ops| ops.search_uids(from_uid))
    }

    fn fetch_envelopes(&mut self, first: u32, last: u32) -> MailResult<Vec<Envelope>> {
        let envelopes = self.run_selected(|ops| ops.fetch_envelopes(first, last))?;
        let folder = self.selected.clone().unwrap();
        let uidvalidity = self.ops.uidvalidity[&folder];
        for env in &envelopes {
            let id = message_id(&env.headers).unwrap();
            let previous = self
                .uid_map
                .insert((folder.clone(), uidvalidity, env.uid), id.clone());
            assert!(
                previous.is_none_or(|p| p == id),
                "the fake reused {folder}/{} under one UIDVALIDITY",
                env.uid
            );
        }
        Ok(envelopes)
    }

    fn fetch_flags(
        &mut self,
        upto_uid: u32,
        changed_since: Option<u64>,
    ) -> MailResult<Vec<FlagUpdate>> {
        self.run_selected(|ops| ops.fetch_flags(upto_uid, changed_since))
    }

    fn fetch_raw(&mut self, uid: u32) -> MailResult<Option<Vec<u8>>> {
        self.run_selected(|ops| ops.fetch_raw(uid))
    }

    fn add_flags(&mut self, uid: u32, flags: &[&str]) -> MailResult<()> {
        if self.selected.is_some() {
            for flag in flags {
                self.check_rule_command(uid, flag);
            }
        }
        let folder = self.selected.clone();
        let id = folder.as_deref().and_then(|f| self.identity(f, uid));
        self.run_selected(|ops| ops.add_flags(uid, flags))?;
        // `\Deleted` alone deletes nothing, and a delete whose expunge failed is rightly sent again: the delete
        // counts at the expunge that removes the message.
        for flag in flags.iter().filter(|f| **f != "\\Deleted") {
            self.record(folder.as_deref().unwrap(), uid, flag, id.clone());
        }
        Ok(())
    }

    fn remove_flags(&mut self, _uid: u32, flags: &[&str]) -> MailResult<()> {
        panic!("no rule removes flags, yet {flags:?} was removed");
    }

    fn expunge(&mut self, uid: u32) -> MailResult<()> {
        let folder = self.selected.clone();
        let id = folder.as_deref().and_then(|f| self.identity(f, uid));
        if id.is_some() {
            self.check_rule_command(uid, "\\Deleted");
        }
        self.run_selected(|ops| ops.expunge(uid))?;
        let folder = folder.unwrap();
        if id.is_some() && self.identity(&folder, uid).is_none() {
            self.record(&folder, uid, "expunge", id);
        }
        Ok(())
    }

    fn move_message(&mut self, uid: u32, to: &str) -> MailResult<()> {
        let what = format!("move:{to}");
        if self.selected.is_some() {
            self.check_rule_command(uid, &what);
        }
        let folder = self.selected.clone();
        let id = folder.as_deref().and_then(|f| self.identity(f, uid));
        self.run_selected(|ops| ops.move_message(uid, to))?;
        self.record(folder.as_deref().unwrap(), uid, &what, id);
        Ok(())
    }

    fn create_folder(&mut self, name: &str) -> MailResult<()> {
        assert_eq!(
            name, "Newsletters",
            "only the newsletters rule creates a folder"
        );
        self.run(|ops| ops.create_folder(name))
    }

    fn append(&mut self, folder: &str, _raw: &[u8], _flags: &[&str]) -> MailResult<()> {
        panic!("sync never appends, yet it appended to {folder}");
    }

    fn idle(&mut self, _timeout: Duration, _interrupt: &AtomicBool) -> MailResult<IdleOutcome> {
        panic!("the model drives passes itself; nothing idles");
    }
}

/// What a pass needs besides the harness.
struct Fixture {
    _dir: tempfile::TempDir,
    trash: Trash,
    rules_path: PathBuf,
    account: AccountConfig,
    identity: Identity,
}

impl Fixture {
    fn new() -> (Fixture, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let trash_dir = dir.path().join("trash");
        std::fs::create_dir_all(&trash_dir).unwrap();
        let rules_path = dir.path().join("rules.toml");
        std::fs::write(&rules_path, BASE_RULES).unwrap();
        let password = PasswordSource::Keyring { keyring: true };
        let account = AccountConfig {
            address: Some("me@model.example".into()),
            ..AccountConfig::new("model", "imap.model.example", "me@model.example", password)
        };
        let identity = account.identity().unwrap();
        let fixture = Fixture {
            trash: Trash::new(trash_dir.clone()),
            _dir: dir,
            rules_path,
            account,
            identity,
        };
        (fixture, trash_dir)
    }

    fn add_alerts_rule(&self, h: &mut Harness) {
        if h.alerts_in_file {
            return;
        }
        let text = format!("{BASE_RULES}{ALERTS_RULE}");
        std::fs::write(&self.rules_path, text).unwrap();
        h.alerts_in_file = true;
    }

    /// One pass as `AccountSync::pass` runs it: reload rules, sync, and only when the sync did not fail, run rules.
    fn pass(
        &self,
        h: &mut Harness,
        full: bool,
        failure: Option<Failure>,
        mid: Option<&ServerEvent>,
    ) -> Option<RulesRun> {
        h.passes += 1;
        h.arm(failure);
        h.moved_or_deleted.clear();
        let store = h.store;
        let rules = load_rules_for(store, &self.rules_path, h.now).expect("model rules are valid");
        let names = BASE_RULE_NAMES
            .iter()
            .chain(h.alerts_in_file.then_some(&"alerts"));
        for name in names {
            h.first_seen.entry(*name).or_insert(h.now);
        }
        assert_eq!(rules.len(), h.first_seen.len());
        let synced = if full {
            sync_all(h, store).map(|_| ())
        } else {
            let inbox = RemoteFolder {
                name: "INBOX".into(),
                special_use: None,
                delimiter: None,
            };
            sync_folder(h, store, &inbox).map(|_| ())
        };
        if synced.is_ok() && !h.tripped {
            h.check_mirror(full);
        }
        h.check_rows_name_their_messages();
        if synced.is_err() {
            // The pass ends with the error; the run loop reconnects and tries again later.
            h.disarm();
            return None;
        }
        if let Some(event) = mid {
            h.server(event);
        }
        let now = h.now;
        let run = run_rules(
            h,
            store,
            &self.trash,
            &rules,
            &self.account,
            &self.identity,
            Mode::Normal,
            now,
        )
        .expect("store errors are not modelled");
        h.check_events(&run.events);
        h.check_rule_log();
        h.check_rows_name_their_messages();
        h.disarm();
        Some(run)
    }
}

fn run_model(initial: &[ServerEvent], steps: &[Step]) {
    let (fixture, trash_dir) = Fixture::new();
    let store = Store::open_in_memory().unwrap();
    let mut h = Harness::new(&store, trash_dir);
    // Mail already on the server before the first pass: older than every rule, so no rule may touch it.
    for event in initial {
        h.server(event);
    }
    for step in steps {
        match step {
            Step::Server(event) => h.server(event),
            Step::Tick { minutes } => h.now += minutes * 60,
            Step::AddAlertsRule => fixture.add_alerts_rule(&mut h),
            Step::Pass { full, failure, mid } => {
                fixture.pass(&mut h, *full, *failure, mid.as_ref());
            }
        }
    }
    // Quiet passes: no failures, no concurrent changes. Each needs at most one more pass to see the moves of the
    // last (moved rows are synced in their new folder by the next pass), so four always suffice.
    let mut quiet = false;
    for _ in 0..4 {
        let run = fixture
            .pass(&mut h, true, None, None)
            .expect("a quiet pass syncs");
        let errors: Vec<&Event> = run
            .events
            .iter()
            .filter(|e| matches!(e, Event::Error { .. }))
            .collect();
        if run.actions == 0 && errors.is_empty() {
            quiet = true;
            break;
        }
    }
    assert!(quiet, "rules still act after four quiet passes");
    h.check_rules_caught_up();
    h.check_backups_kept();
}

// ---- strategies --------------------------------------------------------------------------------------------------

fn sender() -> impl Strategy<Value = Sender> {
    (0..Sender::ALL.len()).prop_map(|i| Sender::ALL[i])
}

fn folder() -> impl Strategy<Value = usize> {
    prop_oneof![4 => Just(0usize), 1 => 1..FOLDERS.len()]
}

fn server_event() -> impl Strategy<Value = ServerEvent> {
    let flag = prop_oneof![Just(Flag::Seen), Just(Flag::Flagged)];
    prop_oneof![
        5 => (sender(), folder(), 0..=180i64, any::<bool>()).prop_map(|(sender, folder, age_min, seen)| {
            ServerEvent::Arrive { sender, folder, age_min, seen }
        }),
        2 => (0..64usize).prop_map(|pick| ServerEvent::Delete { pick }),
        2 => (0..64usize, folder()).prop_map(|(pick, to)| ServerEvent::Move { pick, to }),
        3 => (0..64usize, flag, any::<bool>()).prop_map(|(pick, flag, on)| ServerEvent::SetFlag { pick, flag, on }),
        1 => folder().prop_map(|folder| ServerEvent::Reset { folder }),
    ]
}

fn failure() -> impl Strategy<Value = Option<Failure>> {
    prop_oneof![
        3 => Just(None),
        1 => (0..40usize).prop_map(|after| Some(Failure::Command { after })),
        1 => (0..40usize, any::<bool>()).prop_map(|(after, effect)| Some(Failure::Lost { after, effect })),
    ]
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        6 => server_event().prop_map(Step::Server),
        2 => (1..=120i64).prop_map(|minutes| Step::Tick { minutes }),
        1 => Just(Step::AddAlertsRule),
        5 => (prop::bool::weighted(0.7), failure(), prop::option::weighted(0.2, server_event()))
            .prop_map(|(full, failure, mid)| Step::Pass { full, failure, mid }),
    ]
}

fn initial_mail() -> impl Strategy<Value = Vec<ServerEvent>> {
    let old = (sender(), folder(), 60..=600i64, any::<bool>()).prop_map(
        |(sender, folder, age_min, seen)| ServerEvent::Arrive {
            sender,
            folder,
            age_min,
            seen,
        },
    );
    prop::collection::vec(old, 0..8)
}

fn config() -> Config {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(64);
    Config {
        cases,
        // Shrunk failures become named regression tests below, not files next to this one.
        failure_persistence: None,
        ..Config::default()
    }
}

#[test]
fn sync_and_rules_keep_their_invariants() {
    let mut runner = TestRunner::new(config());
    let strategy = (initial_mail(), prop::collection::vec(step(), 1..30));
    let result = runner.run(&strategy, |(initial, steps)| {
        run_model(&initial, &steps);
        Ok(())
    });
    if let Err(e) = result {
        panic!("{e}");
    }
}
