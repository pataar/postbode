use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::actions::{self, ActionError};
use crate::config::{AccountConfig, ConfigError, Identity};
use crate::credentials::{self, CredentialError};
use crate::mail_ops::imap::ImapOps;
use crate::mail_ops::{Envelope, IdleOutcome, MailError, MailOps, RemoteFolder};
use crate::message::{body_text, clean, parse_headers, thread_id};
use crate::paths::Paths;
use crate::rules::apply::{ApplyError, apply, ensure_raw};
use crate::rules::engine::{Context, Mode, evaluate, folder_needs_body};
use crate::rules::{Action, CompiledRule, RulesError};
use crate::store::{Folder, Message, Store, StoreError};
use crate::time::{clock, now};
use crate::trash::{RestoreError, Trash};

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Action(#[from] ActionError),
    #[error(transparent)]
    Restore(#[from] RestoreError),
    #[error(transparent)]
    Mail(#[from] MailError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Apply(#[from] ApplyError),
    #[error(transparent)]
    Rules(#[from] RulesError),
    #[error(transparent)]
    Credentials(#[from] CredentialError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("stopped")]
    Stopped,
    #[error("crashed: {0}")]
    Crashed(String),
}

/// Names one command so the event that completes it can be matched to whoever sent it.
pub type RequestId = u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Activity {
        account: String,
        activity: Activity,
    },
    ActionDone {
        account: String,
        folder: String,
        results: EventResults,
        request: RequestId,
    },
    BodiesFetched {
        account: String,
        request: RequestId,
        fetched: usize,
    },
    BodyReady {
        account: String,
        folder: String,
        uid: u32,
        request: RequestId,
    },
    CommandFailed {
        account: String,
        request: RequestId,
        message: String,
    },
    Error {
        account: String,
        message: String,
    },
    NewMail {
        account: String,
        folder: String,
        uid: u32,
        from: String,
        subject: String,
    },
    Restored {
        account: String,
        folder: String,
        request: RequestId,
    },
    RuleApplied {
        account: String,
        request: RequestId,
        evaluated: usize,
        actions: usize,
        errors: Vec<String>,
    },
    Synced {
        account: String,
        new_messages: usize,
        actions: usize,
        requests: Vec<RequestId>,
        /// The errors this pass also sent as `Error` events, and the rules file's while it is invalid.
        errors: Vec<String>,
    },
}

impl Event {
    /// The requests this event completes, so the daemon can reply to whoever sent them.
    pub fn request_ids(&self) -> Vec<RequestId> {
        match self {
            Event::ActionDone { request, .. }
            | Event::BodiesFetched { request, .. }
            | Event::BodyReady { request, .. }
            | Event::CommandFailed { request, .. }
            | Event::Restored { request, .. }
            | Event::RuleApplied { request, .. } => vec![*request],
            Event::Synced { requests, .. } => requests.clone(),
            Event::Activity { .. } | Event::Error { .. } | Event::NewMail { .. } => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewMessageRef {
    pub folder: String,
    pub uid: u32,
}

#[derive(Debug, Default)]
pub struct RulesRun {
    pub evaluated: usize,
    pub actions: usize,
    /// NewMail notifications and per-folder or per-message errors that did not stop the run.
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Activity {
    Connecting,
    ListingFolders,
    SyncingFolder {
        folder: String,
        index: usize,
        of: usize,
    },
    FetchingHeaders {
        folder: String,
        done: usize,
        total: usize,
    },
    FetchingBodies {
        folder: String,
        done: usize,
        total: usize,
    },
    RunningRules {
        folder: String,
    },
    RunningCommand {
        what: String,
    },
    Idle {
        since: i64,
    },
    Offline {
        reason: String,
        retry_at: i64,
    },
    /// The account has no sync thread, so it never syncs until the daemon restarts or its config changes.
    NotRunning {
        reason: String,
    },
}

impl fmt::Display for Activity {
    /// The status line text the window and `postbode daemon status` show.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Activity::Connecting => "connecting…".into(),
            Activity::ListingFolders => "listing folders".into(),
            Activity::SyncingFolder { folder, index, of } => {
                format!("{} ({index}/{of})", clean(folder, false))
            }
            Activity::FetchingHeaders {
                folder,
                done,
                total,
            } => fetching(folder, "headers", *done, *total),
            Activity::FetchingBodies {
                folder,
                done,
                total,
            } => fetching(folder, "bodies", *done, *total),
            Activity::RunningRules { folder } => {
                format!("running rules on {}", clean(folder, false))
            }
            Activity::RunningCommand { what } => clean(what, false),
            Activity::Idle { since } => format!("up to date · {}", clock(*since)),
            Activity::Offline { reason, retry_at } => {
                format!(
                    "offline ({}) · retry {}",
                    clean(reason, false),
                    clock(*retry_at)
                )
            }
            Activity::NotRunning { reason } => format!("not running ({})", clean(reason, false)),
        };
        f.write_str(&text)
    }
}

/// "INBOX headers 1,200 / 5,000".
fn fetching(folder: &str, what: &str, done: usize, total: usize) -> String {
    format!(
        "{} {what} {} / {}",
        clean(folder, false),
        thousands(done),
        thousands(total)
    )
}

fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Work the daemon asks an account's sync thread to do on its connection, on a client's behalf.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// `by` is logged as the action's rule name.
    Apply {
        folder: String,
        uids: Vec<u32>,
        #[serde(with = "ActionDef")]
        action: Action,
        by: String,
    },
    ApplyRule {
        name: String,
    },
    FetchBodies {
        folder: Option<String>,
    },
    FetchBody {
        folder: String,
        uid: u32,
    },
    Restore {
        file: PathBuf,
    },
    SyncNow,
}

/// `Action` skips its user-only variants in rules.toml, so the wire derives them separately.
#[derive(Serialize, Deserialize)]
#[serde(remote = "Action", rename_all = "snake_case")]
enum ActionDef {
    Archive,
    Delete,
    Flag,
    MarkRead,
    MarkUnread,
    Move(String),
    Notify,
    Silent,
    Trash,
    Unflag,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub request: RequestId,
    pub command: Command,
}

#[derive(Debug, Default, PartialEq)]
pub struct CommandsRun {
    pub used_connection: bool,
    pub wants_full_pass: bool,
}

/// Called between steps of a pass with what is happening; may run queued commands on the connection.
/// Returns true when it used the connection, so the caller selects its folder again.
pub type Checkpoint<'a> = dyn FnMut(&mut dyn MailOps, Activity) -> Result<bool, SyncError> + 'a;

/// Envelopes are fetched this many messages at a time, each batch committed on its own.
const CHUNK: usize = 500;

#[cfg(any(test, feature = "testing"))]
pub fn sync_folder(
    ops: &mut dyn MailOps,
    store: &Store,
    folder: &RemoteFolder,
) -> Result<Vec<NewMessageRef>, SyncError> {
    sync_folder_with(ops, store, folder, &mut |_, _| Ok(false))
}

pub fn sync_folder_with(
    ops: &mut dyn MailOps,
    store: &Store,
    folder: &RemoteFolder,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<Vec<NewMessageRef>, SyncError> {
    let info = ops.select(&folder.name)?;
    let stored = store.folder(&folder.name)?;
    // Newly tracked, a placeholder, or reset: everything listed now was already on the server.
    let (last_uid, starts_tracking) = match &stored {
        Some(f) if f.uidvalidity == info.uidvalidity => (f.last_uid, false),
        // uidvalidity 0 marks a placeholder created by a rule move into a folder not synced yet.
        Some(f) if f.uidvalidity != 0 => {
            log::info!(
                "{}: UIDVALIDITY changed {} -> {}, resyncing",
                folder.name,
                f.uidvalidity,
                info.uidvalidity
            );
            (0, true)
        }
        _ => (0, true),
    };

    let flags_upto = if starts_tracking { 0 } else { last_uid };
    let updates = if flags_upto > 0 {
        ops.fetch_flags(flags_upto)?
    } else {
        Vec::new()
    };
    let uids: Vec<u32> = ops
        .search_uids(last_uid + 1)?
        .into_iter()
        .filter(|&uid| uid > last_uid)
        .collect();
    let row = |last_uid| Folder {
        name: folder.name.clone(),
        uidvalidity: info.uidvalidity,
        last_uid,
        special_use: folder.special_use.clone(),
    };

    store.transaction(|| {
        if starts_tracking && stored.is_some() {
            store.reset_folder(&folder.name, info.uidvalidity)?;
        }
        store.upsert_folder(&row(last_uid))?;
        if starts_tracking {
            store.set_initial_uid_next(&folder.name, uids.last().map_or(0, |uid| uid + 1))?;
            store.set_rules_uid(&folder.name, 0)?;
            store.set_notified_uid(&folder.name, 0)?;
        }
        let present: Vec<u32> = updates.iter().map(|u| u.uid).collect();
        for u in &updates {
            store.update_flags(&folder.name, u.uid, &u.flags.join(" "))?;
        }
        if flags_upto > 0 {
            store.remove_missing(&folder.name, flags_upto, &present)?;
        }
        Ok::<_, SyncError>(())
    })?;

    let total = uids.len();
    let mut done = 0;
    let mut new = Vec::new();
    for chunk in uids.chunks(CHUNK) {
        let progress = Activity::FetchingHeaders {
            folder: folder.name.clone(),
            done,
            total,
        };
        // A command ran on the connection and may have selected another folder.
        if checkpoint(ops, progress)? {
            let uidvalidity = ops.select(&folder.name)?.uidvalidity;
            if uidvalidity != info.uidvalidity {
                log::info!(
                    "{}: UIDVALIDITY changed {} -> {} during sync, resyncing next pass",
                    folder.name,
                    info.uidvalidity,
                    uidvalidity
                );
                break;
            }
        }
        let (first, last) = (chunk[0], chunk[chunk.len() - 1]);
        let envelopes = ops.fetch_envelopes(first, last)?;
        store.transaction(|| {
            for env in envelopes {
                let uid = env.uid;
                store.insert_message(&to_message(&folder.name, env))?;
                new.push(NewMessageRef {
                    folder: folder.name.clone(),
                    uid,
                });
            }
            store.upsert_folder(&row(last))
        })?;
        done += chunk.len();
    }
    if total > 0 {
        checkpoint(
            ops,
            Activity::FetchingHeaders {
                folder: folder.name.clone(),
                done,
                total,
            },
        )?;
    }
    Ok(new)
}

fn to_message(folder: &str, env: Envelope) -> Message {
    let parsed = parse_headers(&env.headers);
    Message {
        folder: folder.to_string(),
        uid: env.uid,
        thread_id: thread_id(&parsed, folder, env.uid),
        message_id: parsed.message_id,
        from_addr: parsed.from,
        to_addr: parsed.to,
        cc_addr: parsed.cc,
        delivered_to: parsed.delivered_to,
        in_reply_to: parsed.in_reply_to,
        refs: if parsed.references.is_empty() {
            None
        } else {
            Some(parsed.references.join(" "))
        },
        subject: parsed.subject,
        date: parsed.date,
        internaldate: env.internaldate,
        flags: env.flags.join(" "),
        size: env.size,
        headers: env.headers,
        body_text: None,
    }
}

#[cfg(any(test, feature = "testing"))]
pub fn sync_all(
    ops: &mut dyn MailOps,
    store: &Store,
) -> Result<(Vec<NewMessageRef>, Vec<String>), SyncError> {
    sync_all_with(ops, store, &mut |_, _| Ok(false))
}

/// Syncs every listed folder. A folder that fails is skipped and reported in the second list; the others still sync.
/// A lost connection or a stop ends the pass with that error instead.
pub fn sync_all_with(
    ops: &mut dyn MailOps,
    store: &Store,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<(Vec<NewMessageRef>, Vec<String>), SyncError> {
    checkpoint(ops, Activity::ListingFolders)?;
    let mut folders = ops.list_folders()?;
    special_use_by_name(&mut folders);
    let of = folders.len();
    let mut new = Vec::new();
    let mut errors = Vec::new();
    for (index, folder) in folders.iter().enumerate() {
        let syncing = Activity::SyncingFolder {
            folder: folder.name.clone(),
            index: index + 1,
            of,
        };
        checkpoint(ops, syncing)?;
        match sync_folder_with(ops, store, folder, checkpoint) {
            Ok(found) => new.extend(found),
            Err(e) if matches!(e, SyncError::Stopped) || is_lost_connection(&e) => return Err(e),
            Err(e) => errors.push(format!("{}: {e}; skipped this pass", folder.name)),
        }
    }
    Ok((new, errors))
}

/// Spec section 6: a role no folder is marked with goes to the folder carrying that name, ignoring case.
fn special_use_by_name(folders: &mut [RemoteFolder]) {
    for role in ["Archive", "Drafts", "Junk", "Sent", "Trash"] {
        if folders
            .iter()
            .any(|f| f.special_use.as_deref() == Some(role))
        {
            continue;
        }
        if let Some(folder) = folders
            .iter_mut()
            .find(|f| f.special_use.is_none() && f.name.eq_ignore_ascii_case(role))
        {
            folder.special_use = Some(role.to_string());
        }
    }
}

pub fn load_rules_for(
    store: &Store,
    path: &Path,
    now: i64,
) -> Result<Vec<CompiledRule>, RulesError> {
    let file = crate::rules::load(path)?;
    let mut compiled = crate::rules::compile(&file)?;
    // A disabled rule, such as a pending proposal, starts its clock when it is enabled, not when it was written.
    let enabled: Vec<&str> = compiled
        .iter()
        .filter(|r| r.rule.enabled)
        .map(|r| r.rule.name.as_str())
        .collect();
    store.forget_rules_except(&enabled)?;
    for rule in compiled.iter_mut().filter(|r| r.rule.enabled) {
        rule.first_seen_at = store.rule_first_seen(&rule.rule.name, now)?;
    }
    Ok(compiled)
}

#[allow(clippy::too_many_arguments)]
pub fn run_rules(
    ops: &mut dyn MailOps,
    store: &Store,
    trash: &Trash,
    rules: &[CompiledRule],
    account: &AccountConfig,
    identity: &Identity,
    mode: Mode,
    now: i64,
) -> Result<RulesRun, SyncError> {
    run_rules_with(
        ops,
        store,
        trash,
        rules,
        account,
        identity,
        mode,
        now,
        &mut |_| {},
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_rules_with(
    ops: &mut dyn MailOps,
    store: &Store,
    trash: &Trash,
    rules: &[CompiledRule],
    account: &AccountConfig,
    identity: &Identity,
    mode: Mode,
    now: i64,
    report: &mut dyn FnMut(Activity),
) -> Result<RulesRun, SyncError> {
    let ctx = Context {
        account: &account.name,
        identity,
        now,
        mode,
        notify_default: account.notify,
    };
    let mut folders: Vec<String> = rules
        .iter()
        .filter(|r| r.rule.enabled && r.applies_to_account(&account.name))
        .map(|r| r.folder().to_string())
        .collect();
    folders.push("INBOX".to_string());
    folders.sort();
    folders.dedup();

    let mut run = RulesRun::default();
    for folder in folders.iter().cloned() {
        let Some(stored) = store.folder(&folder)? else {
            continue;
        };
        let info = match ops.select(&folder) {
            Ok(info) => info,
            Err(e) => {
                run.events.push(account_error(
                    account,
                    format!("{folder}: {e}; skipping its rules"),
                ));
                continue;
            }
        };
        // Stored uids belong to another UIDVALIDITY: acting on them would hit unrelated server messages.
        if info.uidvalidity != stored.uidvalidity {
            // uidvalidity 0 is a placeholder from a rule move; the next full sync adopts the server value.
            if stored.uidvalidity != 0 {
                run.events.push(account_error(
                    account,
                    format!(
                        "{folder}: folder changed on server; resync needed, skipping its rules"
                    ),
                ));
            }
            continue;
        }
        let needs_body = folder_needs_body(rules, &account.name, &folder);
        report(Activity::RunningRules {
            folder: folder.clone(),
        });
        let (rules_uid, notified_uid) = (store.rules_uid(&folder)?, store.notified_uid(&folder)?);
        let unseen = unseen_by_rules(store, &stored)?;
        let fresh = |uid: u32| mode == Mode::Normal && unseen(uid);
        let messages = store.messages_in_folder(&folder)?;
        let highest_uid = messages.last().map(|m| m.uid.min(stored.last_uid));
        let bodies_total = if needs_body {
            messages
                .iter()
                .filter(|m| m.body_text.is_none() && fresh(m.uid))
                .count()
        } else {
            0
        };
        let mut bodies_done = 0;
        for mut msg in messages {
            let is_fresh = fresh(msg.uid);
            // Mail found by a first sync or resync already sat on the server; fetching it would download the whole folder.
            if needs_body && msg.body_text.is_none() && is_fresh {
                match ensure_raw(&msg, ops, store) {
                    Ok(raw) => msg.body_text = Some(body_text(&raw)),
                    Err(e) => log::warn!("{}/{}: body fetch failed: {e}", msg.folder, msg.uid),
                }
                bodies_done += 1;
                report(Activity::FetchingBodies {
                    folder: folder.clone(),
                    done: bodies_done,
                    total: bodies_total,
                });
            }
            let plan = evaluate(rules, &msg, &ctx);
            run.evaluated += 1;
            if !plan.actions.is_empty() {
                match apply(&plan, &msg, ops, store, trash, now) {
                    Ok(executed) => run.actions += executed,
                    Err(e) => {
                        run.events.push(account_error(
                            account,
                            format!("{}/{}: {e}", msg.folder, msg.uid),
                        ));
                        continue;
                    }
                }
            }
            if is_fresh && msg.uid > notified_uid && plan.notify && folder == "INBOX" {
                run.events.push(new_mail(account, &msg));
            }
        }
        if mode == Mode::Normal
            && let Some(highest_uid) = highest_uid
        {
            if highest_uid > rules_uid {
                store.set_rules_uid(&folder, highest_uid)?;
            }
            if highest_uid > notified_uid {
                store.set_notified_uid(&folder, highest_uid)?;
            }
        }
    }
    if mode == Mode::Normal {
        // Folders no rule looks at count as seen up to their last uid, so a rule added for one later finds no old
        // mail fresh.
        store.mark_rules_seen_except(&folders)?;
    }
    Ok(run)
}

/* Whether a uid is fresh: not yet seen by the rules, not already on the server when the folder was first tracked, and
synced. A row a move stored above last_uid waits for sync, so unsynced mail below it is not marked seen too early. */
fn unseen_by_rules(
    store: &Store,
    folder: &Folder,
) -> Result<impl Fn(u32) -> bool + use<>, StoreError> {
    let rules_uid = store.rules_uid(&folder.name)?;
    let initial_uid_next = store.initial_uid_next(&folder.name)?;
    let last_uid = folder.last_uid;
    Ok(move |uid| uid > rules_uid && uid >= initial_uid_next && uid <= last_uid)
}

fn new_mail(account: &AccountConfig, msg: &Message) -> Event {
    Event::NewMail {
        account: account.name.clone(),
        folder: msg.folder.clone(),
        uid: msg.uid,
        from: msg.from_addr.clone().unwrap_or_default(),
        subject: msg.subject.clone().unwrap_or_default(),
    }
}

fn account_error(account: &AccountConfig, message: String) -> Event {
    log::warn!("{}: {message}", account.name);
    Event::Error {
        account: account.name.clone(),
        message,
    }
}

/// Runs every queued command in arrival order. A failed command is reported and the next one runs; a lost
/// connection ends the drain with the error, so the session reconnects. `SyncNow` requests go onto
/// `carried.sync_requests` for the next full pass to answer.
#[allow(clippy::too_many_arguments)]
fn run_commands(
    ops: &mut dyn MailOps,
    store: &Store,
    trash: &Trash,
    account: &AccountConfig,
    identity: &Identity,
    rules_path: &Path,
    commands: &Receiver<Job>,
    carried: &mut Carried,
    events: &Sender<Event>,
) -> Result<CommandsRun, SyncError> {
    let mut run = CommandsRun::default();
    while let Ok(Job { request, command }) = commands.try_recv() {
        if command == Command::SyncNow {
            run.wants_full_pass = true;
            carried.sync_requests.push(request);
            continue;
        }
        carried.running = Some(request);
        let ran = run_command(
            ops, store, trash, account, identity, rules_path, request, command, events,
        );
        carried.running = None;
        let ran = ran?;
        run.used_connection |= ran.used_connection;
        run.wants_full_pass |= ran.wants_full_pass;
    }
    Ok(run)
}

/// Runs one command other than `SyncNow` and sends its answer; `Err` only for a lost connection.
#[allow(clippy::too_many_arguments)]
fn run_command(
    ops: &mut dyn MailOps,
    store: &Store,
    trash: &Trash,
    account: &AccountConfig,
    identity: &Identity,
    rules_path: &Path,
    request: RequestId,
    command: Command,
    events: &Sender<Event>,
) -> Result<CommandsRun, SyncError> {
    let name = || account.name.clone();
    let failed = |request, message: String| {
        log::warn!("{}: {message}", account.name);
        Event::CommandFailed {
            account: account.name.clone(),
            request,
            message,
        }
    };
    let mut run = CommandsRun::default();
    match command {
        Command::SyncNow => unreachable!("run_commands holds SyncNow for the next full pass"),
        Command::Apply {
            folder,
            uids,
            action,
            by,
        } => {
            run.used_connection = true;
            let _ = events.send(Event::Activity {
                account: name(),
                activity: Activity::RunningCommand {
                    what: describe(&action, uids.len()),
                },
            });
            let outcome = actions::run(ops, store, trash, &folder, &uids, &action, &by, now());
            let (results, lost) = match outcome {
                Ok(results) => split_lost(results),
                Err(e) => (
                    uids.iter().map(|&uid| (uid, Err(e.to_string()))).collect(),
                    connection_lost(&e).then_some(e),
                ),
            };
            let _ = events.send(Event::ActionDone {
                account: name(),
                folder,
                results,
                request,
            });
            if let Some(e) = lost {
                return Err(e.into());
            }
        }
        Command::ApplyRule { name: rule_name } => {
            let rule = match enabled_rule(rules_path, &rule_name) {
                Ok(rule) => rule,
                Err(message) => {
                    let _ = events.send(failed(request, message));
                    return Ok(run);
                }
            };
            run.used_connection = true;
            let rules = std::slice::from_ref(&rule);
            match run_rules(
                ops,
                store,
                trash,
                rules,
                account,
                identity,
                Mode::ApplyExisting,
                now(),
            ) {
                Ok(rules_run) => {
                    let errors = rules_run
                        .events
                        .into_iter()
                        .filter_map(|event| match event {
                            Event::Error { message, .. } => Some(message),
                            _ => None,
                        })
                        .collect();
                    let _ = events.send(Event::RuleApplied {
                        account: name(),
                        request,
                        evaluated: rules_run.evaluated,
                        actions: rules_run.actions,
                        errors,
                    });
                }
                Err(e) => {
                    let _ = events.send(failed(request, e.to_string()));
                    if is_lost_connection(&e) {
                        return Err(e);
                    }
                }
            }
        }
        Command::FetchBodies { folder } => {
            run.used_connection = true;
            match fetch_bodies_in(ops, store, folder.as_deref()) {
                Ok(fetched) => {
                    let _ = events.send(Event::BodiesFetched {
                        account: name(),
                        request,
                        fetched,
                    });
                }
                Err(e) => {
                    let _ = events.send(failed(request, e.to_string()));
                    if connection_lost(&e) {
                        return Err(e.into());
                    }
                }
            }
        }
        Command::FetchBody { folder, uid } => {
            run.used_connection = true;
            match actions::fetch_body(ops, store, &folder, uid) {
                Ok(()) => {
                    let _ = events.send(Event::BodyReady {
                        account: name(),
                        folder,
                        uid,
                        request,
                    });
                }
                Err(e) => {
                    let _ = events.send(failed(request, format!("{folder}/{uid}: {e}")));
                    if connection_lost(&e) {
                        return Err(e.into());
                    }
                }
            }
        }
        Command::Restore { file } => {
            run.used_connection = true;
            match trash.restore(ops, &file) {
                Ok(folder) => {
                    run.wants_full_pass = true;
                    let _ = events.send(Event::Restored {
                        account: name(),
                        folder,
                        request,
                    });
                }
                Err(e) => {
                    let _ = events.send(failed(request, e.to_string()));
                    if let RestoreError::Mail(e) = e
                        && is_connection_error(&e)
                    {
                        return Err(SyncError::Mail(e));
                    }
                }
            }
        }
    }
    Ok(run)
}

/// The compiled rule called `name`, or why it cannot run. It keeps `first_seen_at` 0, which `ApplyExisting` ignores.
fn enabled_rule(rules_path: &Path, name: &str) -> Result<CompiledRule, String> {
    let file = crate::rules::load(rules_path).map_err(|e| e.to_string())?;
    let rule = crate::rules::compile(&file)
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|r| r.rule.name == name)
        .ok_or_else(|| format!("no rule named '{name}'"))?;
    if !rule.rule.enabled {
        return Err(format!(
            "rule '{name}' is disabled; approve or enable it first"
        ));
    }
    Ok(rule)
}

/// Downloads the missing bodies of the named folder, failing if it cannot, or of every stored folder, skipping any
/// that fail. Only a lost connection stops the walk.
fn fetch_bodies_in(
    ops: &mut dyn MailOps,
    store: &Store,
    folder: Option<&str>,
) -> Result<usize, ActionError> {
    if let Some(folder) = folder {
        return actions::fetch_bodies(ops, store, folder);
    }
    let mut fetched = 0;
    for folder in store.folders()? {
        match actions::fetch_bodies(ops, store, &folder.name) {
            Ok(count) => fetched += count,
            Err(e) if connection_lost(&e) => return Err(e),
            Err(e) => log::warn!("{}: body fetch skipped: {e}", folder.name),
        }
    }
    Ok(fetched)
}

fn is_connection_error(e: &MailError) -> bool {
    matches!(e, MailError::Io(_) | MailError::Connect(_))
}

fn is_lost_connection(e: &SyncError) -> bool {
    match e {
        SyncError::Action(e) => connection_lost(e),
        SyncError::Mail(e) | SyncError::Restore(RestoreError::Mail(e)) => is_connection_error(e),
        _ => false,
    }
}

fn connection_lost(e: &ActionError) -> bool {
    match e {
        ActionError::Mail(e) | ActionError::Apply(ApplyError::Mail(e)) => is_connection_error(e),
        _ => false,
    }
}

pub type EventResults = Vec<(u32, Result<usize, String>)>;

/// Stringifies per-uid results for the event, keeping the first connection loss so it can end the drain.
fn split_lost(results: Vec<actions::UidResult>) -> (EventResults, Option<ActionError>) {
    let mut lost = None;
    let strings = results
        .into_iter()
        .map(|(uid, result)| match result {
            Ok(count) => (uid, Ok(count)),
            Err(e) => {
                let message = e.to_string();
                if lost.is_none() && connection_lost(&e) {
                    lost = Some(e);
                }
                (uid, Err(message))
            }
        })
        .collect();
    (strings, lost)
}

fn describe(action: &Action, count: usize) -> String {
    let verb = match action {
        Action::Archive => "archiving",
        Action::Delete | Action::Trash => "deleting",
        Action::Flag => "flagging",
        Action::MarkRead => "marking read",
        Action::MarkUnread => "marking unread",
        Action::Move(_) => "moving",
        Action::Notify | Action::Silent => "updating",
        Action::Unflag => "unflagging",
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{verb} {count} message{plural}")
}

/// Resolves the account's password and logs in.
pub fn connect(account: &AccountConfig) -> Result<ImapOps, SyncError> {
    let secret = credentials::resolve(account)?;
    Ok(ImapOps::connect(account, &secret)?)
}

/// Syncs INBOX alone, draining queued commands first like a full pass does before each folder.
fn sync_inbox_with(
    ops: &mut dyn MailOps,
    store: &Store,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<Vec<NewMessageRef>, SyncError> {
    let inbox = RemoteFolder {
        name: "INBOX".into(),
        special_use: None,
    };
    let syncing = Activity::SyncingFolder {
        folder: inbox.name.clone(),
        index: 1,
        of: 1,
    };
    checkpoint(ops, syncing)?;
    sync_folder_with(ops, store, &inbox, checkpoint)
}

/// What an account's thread carries from one session to the next.
#[derive(Default)]
struct Carried {
    /// `SyncNow` requests the next full pass answers.
    sync_requests: Vec<RequestId>,
    /// The command running now, so one that panics still gets its answer.
    running: Option<RequestId>,
    rules: LoadedRules,
}

#[derive(Default)]
struct LoadedRules {
    /// The last rules that loaded; None until some have, and while the file is invalid with no rule to fall back on.
    last_good: Option<Vec<CompiledRule>>,
    /// The error of the last load, so each bad edit is reported once rather than every pass.
    error: Option<String>,
}

impl LoadedRules {
    /// Loads the rules file again; when it is invalid keeps the last good rules and reports a new error once.
    fn reload(
        &mut self,
        store: &Store,
        path: &Path,
        account: &AccountConfig,
        events: &Sender<Event>,
    ) {
        match load_rules_for(store, path, now()) {
            Ok(rules) => {
                self.last_good = Some(rules);
                self.error = None;
            }
            Err(e) => {
                self.last_good = self.last_good.take().filter(|rules| !rules.is_empty());
                let kept = if self.last_good.is_some() {
                    "keeping the previous rules"
                } else {
                    "running no rules until it is fixed"
                };
                let error = format!("{e}; {kept}");
                if self.error.as_ref() != Some(&error) {
                    let _ = events.send(account_error(account, error.clone()));
                }
                self.error = Some(error);
            }
        }
    }

    /// Runs the last good rules. With none it only notifies, leaving the mail unseen so the rules catch up on it.
    fn run(
        &self,
        ops: &mut dyn MailOps,
        store: &Store,
        trash: &Trash,
        account: &AccountConfig,
        identity: &Identity,
        report: &mut dyn FnMut(Activity),
    ) -> Result<RulesRun, SyncError> {
        let Some(rules) = &self.last_good else {
            return Ok(RulesRun {
                events: notify_unruled(store, account, identity)?,
                ..RulesRun::default()
            });
        };
        run_rules_with(
            ops,
            store,
            trash,
            rules,
            account,
            identity,
            Mode::Normal,
            now(),
            report,
        )
    }
}

/// NewMail for fresh INBOX mail no notification covered yet, as the account's notify setting says. Moves only the
/// notified mark, so the rules still catch up on that mail.
fn notify_unruled(
    store: &Store,
    account: &AccountConfig,
    identity: &Identity,
) -> Result<Vec<Event>, SyncError> {
    let Some(inbox) = store.folder("INBOX")? else {
        return Ok(Vec::new());
    };
    let unseen = unseen_by_rules(store, &inbox)?;
    let notified_uid = store.notified_uid("INBOX")?;
    let ctx = Context {
        account: &account.name,
        identity,
        now: now(),
        mode: Mode::Normal,
        notify_default: account.notify,
    };
    let messages = store.messages_in_folder("INBOX")?;
    let events = messages
        .iter()
        .filter(|msg| unseen(msg.uid) && msg.uid > notified_uid && evaluate(&[], msg, &ctx).notify)
        .map(|msg| new_mail(account, msg))
        .collect();
    if let Some(highest_uid) = messages
        .last()
        .map(|msg| msg.uid.min(inbox.last_uid))
        .filter(|&uid| uid > notified_uid)
    {
        store.set_notified_uid("INBOX", highest_uid)?;
    }
    Ok(events)
}

/// What one account's passes share: its store, trash and identity.
struct AccountSync<'a> {
    account: &'a AccountConfig,
    store: Store,
    trash: Trash,
    identity: Identity,
    rules_path: PathBuf,
}

impl<'a> AccountSync<'a> {
    fn open(account: &'a AccountConfig, paths: &Paths) -> Result<AccountSync<'a>, SyncError> {
        Ok(AccountSync {
            store: Store::open_account(paths, &account.name)?,
            account,
            trash: Trash::new(paths.trash_dir(&account.name)),
            identity: account.identity()?,
            rules_path: paths.rules_file(),
        })
    }

    /// Reloads the rules, syncs every folder (`full`) or only INBOX, runs the rules and sends the events. A full pass
    /// answers the `SyncNow` requests in `carried` when it starts; ones drained at its checkpoints are kept there for
    /// the next. Queued commands run at the checkpoints; true when one of them wants a full pass next.
    fn pass(
        &mut self,
        ops: &mut dyn MailOps,
        full: bool,
        carried: &mut Carried,
        events: &Sender<Event>,
        commands: &Receiver<Job>,
        shutdown: &AtomicBool,
    ) -> Result<bool, SyncError> {
        carried
            .rules
            .reload(&self.store, &self.rules_path, self.account, events);
        let answering = if full { carried.sync_requests.len() } else { 0 };
        let (account, store, trash) = (self.account, &self.store, &self.trash);
        let (identity, rules_path) = (&self.identity, self.rules_path.as_path());
        let activity = |activity| Event::Activity {
            account: account.name.clone(),
            activity,
        };
        let mut pending_full = false;
        // After a command hits a lost connection nothing else may touch it this pass.
        let mut lost = None;
        let mut checkpoint = |ops: &mut dyn MailOps, step: Activity| {
            if lost.is_some() {
                return Err(SyncError::Stopped);
            }
            let _ = events.send(activity(step));
            if shutdown.load(Ordering::Acquire) {
                return Err(SyncError::Stopped);
            }
            match run_commands(
                ops, store, trash, account, identity, rules_path, commands, carried, events,
            ) {
                Ok(run) => {
                    pending_full |= run.wants_full_pass;
                    Ok(run.used_connection)
                }
                Err(e) => {
                    lost = Some(e);
                    Err(SyncError::Stopped)
                }
            }
        };
        let synced = if full {
            sync_all_with(ops, store, &mut checkpoint)
        } else {
            sync_inbox_with(ops, store, &mut checkpoint).map(|new| (new, Vec::new()))
        };
        if let Some(e) = lost {
            return Err(e);
        }
        let (new, mut errors) = synced?;
        for message in &errors {
            let _ = events.send(account_error(account, message.clone()));
        }
        errors.extend(carried.rules.error.clone());
        let run = carried.rules.run(
            ops,
            &self.store,
            &self.trash,
            self.account,
            &self.identity,
            &mut |step| {
                let _ = events.send(activity(step));
            },
        )?;
        for event in run.events {
            if let Event::Error { message, .. } = &event {
                errors.push(message.clone());
            }
            let _ = events.send(event);
        }
        let _ = events.send(Event::Synced {
            account: self.account.name.clone(),
            new_messages: new.len(),
            actions: run.actions,
            requests: carried.sync_requests.drain(..answering).collect(),
            errors,
        });
        Ok(pending_full)
    }
}

#[cfg(any(test, feature = "testing"))]
pub fn run_once(
    account: &AccountConfig,
    paths: &Paths,
    events: &Sender<Event>,
) -> Result<(), SyncError> {
    let mut ops = connect(account)?;
    let (_, no_commands) = std::sync::mpsc::channel();
    AccountSync::open(account, paths)?.pass(
        &mut ops,
        true,
        &mut Carried::default(),
        events,
        &no_commands,
        &AtomicBool::new(false),
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_loop(
    account: AccountConfig,
    paths: Paths,
    events: Sender<Event>,
    shutdown: Arc<AtomicBool>,
    commands: Receiver<Job>,
    wake: Arc<AtomicBool>,
    connect: impl FnMut() -> Result<Box<dyn MailOps>, SyncError>,
) {
    let name = account.name.clone();
    let (stop, woken) = (shutdown.clone(), wake.clone());
    run_loop_with(
        account,
        paths,
        events,
        shutdown,
        commands,
        wake,
        connect,
        |delay| {
            log::warn!("{name}: reconnecting in {}s", delay.as_secs());
            // Stop or a new command cuts this wait short; clearing the flag keeps later waits at full length.
            let until = std::time::Instant::now() + delay;
            while std::time::Instant::now() < until
                && !stop.load(Ordering::Acquire)
                && !woken.swap(false, Ordering::AcqRel)
            {
                std::thread::sleep(Duration::from_millis(100));
            }
        },
    );
}

/// The loop body, with connection and back-off sleeping injected so tests can drive it with the fake.
#[allow(clippy::too_many_arguments)]
pub fn run_loop_with(
    account: AccountConfig,
    paths: Paths,
    events: Sender<Event>,
    shutdown: Arc<AtomicBool>,
    commands: Receiver<Job>,
    wake: Arc<AtomicBool>,
    mut connect: impl FnMut() -> Result<Box<dyn MailOps>, SyncError>,
    mut sleep: impl FnMut(Duration),
) {
    let mut backoff = Duration::from_secs(5);
    let mut carried = Carried::default();
    while !shutdown.load(Ordering::Acquire) {
        let mut completed_cycle = false;
        // A bug in one session must still answer its requests and leave the account retrying, not silently dead.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_session(
                &account,
                &paths,
                &events,
                &shutdown,
                &commands,
                &wake,
                &mut carried,
                &mut connect,
                &mut completed_cycle,
            )
        }))
        .unwrap_or_else(|panic| Err(SyncError::Crashed(panic_message(&*panic))));
        if completed_cycle {
            backoff = Duration::from_secs(5);
        }
        match result {
            Ok(()) => break,
            Err(_) if shutdown.load(Ordering::Acquire) => break,
            Err(e) => {
                let _ = events.send(Event::Error {
                    account: account.name.clone(),
                    message: e.to_string(),
                });
                let (reason, retry_at) = (e.to_string(), now() + backoff.as_secs() as i64);
                let offline = offline_message(&account.name, &reason, retry_at);
                let _ = events.send(Event::Activity {
                    account: account.name.clone(),
                    activity: Activity::Offline { reason, retry_at },
                });
                fail_requests(&account, &events, &commands, &mut carried, &offline);
                sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(300));
            }
        }
    }
    let stopped = format!("{} stopped", account.name);
    fail_requests(&account, &events, &commands, &mut carried, &stopped);
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|text| text.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

/// Why an offline account cannot run a command: `<account> is offline (<reason>); retrying at <HH:MM>`, local time.
pub fn offline_message(account: &str, reason: &str, retry_at: i64) -> String {
    format!(
        "{account} is offline ({reason}); retrying at {}",
        clock(retry_at)
    )
}

/// Fails the running command, the held `SyncNow` requests and every queued job, so each request still gets its one
/// answer.
fn fail_requests(
    account: &AccountConfig,
    events: &Sender<Event>,
    commands: &Receiver<Job>,
    carried: &mut Carried,
    message: &str,
) {
    let held = carried
        .running
        .take()
        .into_iter()
        .chain(carried.sync_requests.drain(..));
    let queued = std::iter::from_fn(|| commands.try_recv().ok().map(|job| job.request));
    for request in held.chain(queued) {
        let _ = events.send(Event::CommandFailed {
            account: account.name.clone(),
            request,
            message: message.to_string(),
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn run_session(
    account: &AccountConfig,
    paths: &Paths,
    events: &Sender<Event>,
    shutdown: &AtomicBool,
    commands: &Receiver<Job>,
    wake: &AtomicBool,
    carried: &mut Carried,
    connect: &mut impl FnMut() -> Result<Box<dyn MailOps>, SyncError>,
    completed_cycle: &mut bool,
) -> Result<(), SyncError> {
    let _ = events.send(Event::Activity {
        account: account.name.clone(),
        activity: Activity::Connecting,
    });
    let mut ops = connect()?;
    let mut state = AccountSync::open(account, paths)?;
    let interval = Duration::from_secs(account.sync_interval_secs.max(10));
    let mut last_purge = 0i64;
    let mut full = true;
    let mut pass = true;
    let mut pending_full = false;

    while !shutdown.load(Ordering::Acquire) {
        if pass {
            pending_full = state.pass(ops.as_mut(), full, carried, events, commands, shutdown)?;
            if now() - last_purge > 3600 {
                match state
                    .trash
                    .purge(account.trash_retention_days as i64 * 86_400, now())
                {
                    Ok(0) => {}
                    Ok(removed) => log::info!("{}: purged {removed} trash files", account.name),
                    Err(e) => log::warn!("{}: trash purge failed: {e}", account.name),
                }
                last_purge = now();
            }
        }
        let drained = run_commands(
            ops.as_mut(),
            &state.store,
            &state.trash,
            account,
            &state.identity,
            &state.rules_path,
            commands,
            carried,
            events,
        )?;
        if drained.wants_full_pass || std::mem::take(&mut pending_full) {
            (full, pass) = (true, true);
            continue;
        }
        let _ = events.send(Event::Activity {
            account: account.name.clone(),
            activity: Activity::Idle { since: now() },
        });
        ops.select("INBOX")?;
        let outcome = ops.idle(interval, wake)?;
        // A pass plus a wait means the connection is healthy; a pass alone does not, e.g. when IDLE always fails.
        *completed_cycle = true;
        (full, pass) = match outcome {
            IdleOutcome::NewMail => (false, true),
            IdleOutcome::Timeout => (true, true),
            IdleOutcome::Interrupted if shutdown.load(Ordering::Acquire) => return Ok(()),
            // Woken for queued commands: run them, then wait again.
            IdleOutcome::Interrupted => {
                wake.swap(false, Ordering::AcqRel);
                (full, false)
            }
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests;
