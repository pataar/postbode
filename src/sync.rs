use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::actions::{self, ActionError};
use crate::config::{AccountConfig, ConfigError, Identity};
use crate::credentials::{self, CredentialError};
use crate::mail_ops::imap::ImapOps;
use crate::mail_ops::{Envelope, IdleOutcome, MailError, MailOps, RemoteFolder};
use crate::message::{body_text, parse_headers, thread_id};
use crate::paths::Paths;
use crate::rules::apply::{ApplyError, apply, ensure_raw};
use crate::rules::engine::{Context, Mode, evaluate, folder_needs_body};
use crate::rules::{Action, CompiledRule, RulesError};
use crate::store::{Folder, Message, Store, StoreError};
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Activity {
        account: String,
        activity: Activity,
    },
    ActionDone {
        account: String,
        folder: String,
        results: EventResults,
    },
    BodyReady {
        account: String,
        folder: String,
        uid: u32,
    },
    Restored {
        account: String,
        folder: String,
    },
    NewMail {
        account: String,
        folder: String,
        uid: u32,
        from: String,
        subject: String,
    },
    Synced {
        account: String,
        new_messages: usize,
        actions: usize,
    },
    Error {
        account: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewMessageRef {
    pub folder: String,
    pub uid: u32,
    /// The uid was already on the server when the folder was first tracked or reset; such messages never notify.
    pub initial: bool,
}

#[derive(Debug, Default)]
pub struct RulesRun {
    pub evaluated: usize,
    pub actions: usize,
    /// NewMail notifications and per-folder or per-message errors that did not stop the run.
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, PartialEq)]
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
}

/// Work a front end asks an account's sync thread to do on its connection.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Apply {
        folder: String,
        uids: Vec<u32>,
        action: Action,
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

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

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

    let updates = if last_uid > 0 {
        ops.fetch_flags(last_uid)?
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
        }
        let present: Vec<u32> = updates.iter().map(|u| u.uid).collect();
        for u in &updates {
            store.update_flags(&folder.name, u.uid, &u.flags.join(" "))?;
        }
        if last_uid > 0 {
            store.remove_missing(&folder.name, last_uid, &present)?;
        }
        Ok::<_, SyncError>(())
    })?;

    let initial_uid_next = store.initial_uid_next(&folder.name)?;
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
        if checkpoint(ops, progress)? && ops.select(&folder.name)?.uidvalidity != info.uidvalidity {
            break;
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
                    initial: uid < initial_uid_next,
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

pub fn sync_all(
    ops: &mut dyn MailOps,
    store: &Store,
) -> Result<(Vec<NewMessageRef>, Vec<String>), SyncError> {
    sync_all_with(ops, store, &mut |_, _| Ok(false))
}

/// Syncs every listed folder. A folder that fails is skipped and reported in the second list; the others still sync.
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
            Err(SyncError::Stopped) => return Err(SyncError::Stopped),
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

/// Reloads the rules file; on any error logs it and returns `previous` unchanged.
pub fn reload_rules(
    store: &Store,
    path: &Path,
    now: i64,
    previous: Vec<CompiledRule>,
) -> Vec<CompiledRule> {
    match load_rules_for(store, path, now) {
        Ok(rules) => rules,
        Err(e) => {
            log::error!("{e}; keeping the previous rules");
            previous
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_rules(
    ops: &mut dyn MailOps,
    store: &Store,
    trash: &Trash,
    rules: &[CompiledRule],
    account: &AccountConfig,
    identity: &Identity,
    new: &[NewMessageRef],
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
        new,
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
    new: &[NewMessageRef],
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

    // Only mail new since the folder was first tracked can notify or earn a body fetch.
    let fresh: HashSet<(&str, u32)> = new
        .iter()
        .filter(|n| !n.initial)
        .map(|n| (n.folder.as_str(), n.uid))
        .collect();
    let mut run = RulesRun::default();
    for folder in folders {
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
        let messages = store.messages_in_folder(&folder)?;
        let bodies_total = if needs_body {
            messages
                .iter()
                .filter(|m| m.body_text.is_none() && fresh.contains(&(m.folder.as_str(), m.uid)))
                .count()
        } else {
            0
        };
        let mut bodies_done = 0;
        for mut msg in messages {
            let is_fresh = fresh.contains(&(msg.folder.as_str(), msg.uid));
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
            if is_fresh && plan.notify && folder == "INBOX" && mode == Mode::Normal {
                run.events.push(Event::NewMail {
                    account: account.name.clone(),
                    folder: msg.folder.clone(),
                    uid: msg.uid,
                    from: msg.from_addr.clone().unwrap_or_default(),
                    subject: msg.subject.clone().unwrap_or_default(),
                });
            }
        }
    }
    Ok(run)
}

fn account_error(account: &AccountConfig, message: String) -> Event {
    log::warn!("{}: {message}", account.name);
    Event::Error {
        account: account.name.clone(),
        message,
    }
}

/// Runs every queued command in arrival order. A failed command is reported and the next one runs; a lost
/// connection ends the drain with the error, so the session reconnects.
pub fn run_commands(
    ops: &mut dyn MailOps,
    store: &Store,
    trash: &Trash,
    account: &AccountConfig,
    commands: &Receiver<Command>,
    events: &Sender<Event>,
) -> Result<CommandsRun, SyncError> {
    let name = || account.name.clone();
    let mut run = CommandsRun::default();
    while let Ok(command) = commands.try_recv() {
        match command {
            Command::SyncNow => run.wants_full_pass = true,
            Command::Apply {
                folder,
                uids,
                action,
            } => {
                run.used_connection = true;
                let _ = events.send(Event::Activity {
                    account: name(),
                    activity: Activity::RunningCommand {
                        what: describe(&action, uids.len()),
                    },
                });
                let outcome = actions::run(ops, store, trash, &folder, &uids, &action, now());
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
                });
                if let Some(e) = lost {
                    return Err(e.into());
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
                        });
                    }
                    Err(e) if connection_lost(&e) => return Err(e.into()),
                    Err(e) => {
                        let _ = events.send(account_error(account, format!("{folder}/{uid}: {e}")));
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
                        });
                    }
                    Err(RestoreError::Mail(e)) if is_connection_error(&e) => {
                        return Err(SyncError::Mail(e));
                    }
                    Err(e) => {
                        let _ = events.send(account_error(account, e.to_string()));
                    }
                }
            }
        }
    }
    Ok(run)
}

fn is_connection_error(e: &MailError) -> bool {
    matches!(e, MailError::Io(_) | MailError::Connect(_))
}

fn connection_lost(e: &ActionError) -> bool {
    match e {
        ActionError::Mail(e) | ActionError::Apply(ApplyError::Mail(e)) => is_connection_error(e),
        _ => false,
    }
}

type EventResults = Vec<(u32, Result<usize, String>)>;

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

/// What one account's passes share: its store, trash, identity and the last good rules.
struct AccountSync<'a> {
    account: &'a AccountConfig,
    store: Store,
    trash: Trash,
    identity: Identity,
    rules_path: PathBuf,
    rules: Vec<CompiledRule>,
    pending_full: bool,
}

impl<'a> AccountSync<'a> {
    fn open(account: &'a AccountConfig, paths: &Paths) -> Result<AccountSync<'a>, SyncError> {
        paths.ensure_account(&account.name)?;
        let store = Store::open(&paths.mail_db(&account.name))?;
        let rules_path = paths.rules_file();
        let rules = load_rules_for(&store, &rules_path, now())?;
        Ok(AccountSync {
            account,
            trash: Trash::new(paths.trash_dir(&account.name)),
            identity: account.identity()?,
            store,
            rules_path,
            rules,
            pending_full: false,
        })
    }

    /// Syncs every folder (`full`) or only INBOX, reloads the rules, runs them and sends the events. Queued
    /// commands run at the pass's checkpoints.
    fn pass(
        &mut self,
        ops: &mut dyn MailOps,
        full: bool,
        events: &Sender<Event>,
        commands: &Receiver<Command>,
        shutdown: &AtomicBool,
    ) -> Result<(), SyncError> {
        let (account, store, trash) = (self.account, &self.store, &self.trash);
        let activity = |activity| Event::Activity {
            account: account.name.clone(),
            activity,
        };
        let mut pending_full = false;
        let mut checkpoint = |ops: &mut dyn MailOps, step: Activity| {
            let _ = events.send(activity(step));
            if shutdown.load(Ordering::Relaxed) {
                return Err(SyncError::Stopped);
            }
            let run = run_commands(ops, store, trash, account, commands, events)?;
            pending_full |= run.wants_full_pass;
            Ok(run.used_connection)
        };
        let new = if full {
            let (new, sync_errors) = sync_all_with(ops, store, &mut checkpoint)?;
            for message in sync_errors {
                let _ = events.send(account_error(account, message));
            }
            new
        } else {
            let inbox = RemoteFolder {
                name: "INBOX".into(),
                special_use: None,
            };
            sync_folder_with(ops, store, &inbox, &mut checkpoint)?
        };
        self.pending_full |= pending_full;
        let previous = std::mem::take(&mut self.rules);
        self.rules = reload_rules(&self.store, &self.rules_path, now(), previous);
        let run = run_rules_with(
            ops,
            &self.store,
            &self.trash,
            &self.rules,
            self.account,
            &self.identity,
            &new,
            Mode::Normal,
            now(),
            &mut |step| {
                let _ = events.send(activity(step));
            },
        )?;
        for event in run.events {
            let _ = events.send(event);
        }
        let _ = events.send(Event::Synced {
            account: self.account.name.clone(),
            new_messages: new.len(),
            actions: run.actions,
        });
        Ok(())
    }
}

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
        events,
        &no_commands,
        &AtomicBool::new(false),
    )
}

pub fn run_loop(
    account: AccountConfig,
    paths: Paths,
    events: Sender<Event>,
    shutdown: Arc<AtomicBool>,
    commands: Receiver<Command>,
    wake: Arc<AtomicBool>,
) {
    let name = account.name.clone();
    let (stop, woken) = (shutdown.clone(), wake.clone());
    run_loop_with(
        account.clone(),
        paths,
        events,
        shutdown,
        commands,
        wake,
        move || Ok(Box::new(connect(&account)?) as Box<dyn MailOps>),
        |delay| {
            log::warn!("{name}: reconnecting in {}s", delay.as_secs());
            // Stop or a new command cuts this wait short; clearing the flag keeps later waits at full length.
            let until = std::time::Instant::now() + delay;
            while std::time::Instant::now() < until
                && !stop.load(Ordering::Relaxed)
                && !woken.swap(false, Ordering::Relaxed)
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
    commands: Receiver<Command>,
    wake: Arc<AtomicBool>,
    mut connect: impl FnMut() -> Result<Box<dyn MailOps>, SyncError>,
    mut sleep: impl FnMut(Duration),
) {
    let mut backoff = Duration::from_secs(5);
    while !shutdown.load(Ordering::Relaxed) {
        let mut completed_cycle = false;
        let result = run_session(
            &account,
            &paths,
            &events,
            &shutdown,
            &commands,
            &wake,
            &mut connect,
            &mut completed_cycle,
        );
        if completed_cycle {
            backoff = Duration::from_secs(5);
        }
        match result {
            Ok(()) => return,
            Err(_) if shutdown.load(Ordering::Relaxed) => return,
            Err(e) => {
                let _ = events.send(Event::Error {
                    account: account.name.clone(),
                    message: e.to_string(),
                });
                let _ = events.send(Event::Activity {
                    account: account.name.clone(),
                    activity: Activity::Offline {
                        reason: e.to_string(),
                        retry_at: now() + backoff.as_secs() as i64,
                    },
                });
                sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(300));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_session(
    account: &AccountConfig,
    paths: &Paths,
    events: &Sender<Event>,
    shutdown: &AtomicBool,
    commands: &Receiver<Command>,
    wake: &AtomicBool,
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

    while !shutdown.load(Ordering::Relaxed) {
        if pass {
            state.pass(ops.as_mut(), full, events, commands, shutdown)?;
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
            commands,
            events,
        )?;
        if drained.wants_full_pass || std::mem::take(&mut state.pending_full) {
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
            IdleOutcome::Interrupted if shutdown.load(Ordering::Relaxed) => return Ok(()),
            // Woken for queued commands: run them, then wait again.
            IdleOutcome::Interrupted => {
                wake.store(false, Ordering::Relaxed);
                (full, false)
            }
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PasswordSource;
    use crate::mail_ops::RecordingOps;
    use crate::rules::{compile, parse};

    const H: i64 = 3600;

    fn account() -> AccountConfig {
        AccountConfig {
            name: "work".into(),
            host: "h".into(),
            port: 993,
            username: "pieter@example.com".into(),
            password: PasswordSource::Keyring { keyring: true },
            address: None,
            aliases: vec![],
            sync_interval_secs: 120,
            trash_retention_days: 30,
            notify: true,
            ca_file: None,
        }
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
            r.first_seen_at = store.rule_first_seen(&r.rule.name, now).unwrap();
        }
        compiled
    }

    #[test]
    fn first_sync_inserts_folders_and_messages() {
        let mut ops = ops_with_inbox();
        let store = Store::open_in_memory().unwrap();
        let new = sync_all(&mut ops, &store).unwrap().0;
        assert_eq!(new.len(), 2);
        assert!(new.iter().all(|n| n.initial));
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
        let new = sync_all(&mut ops, &store).unwrap().0;
        let run = run_rules(
            &mut ops,
            &store,
            &trash,
            &[],
            &acc,
            &identity,
            &new,
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
        let new = sync_all(&mut ops, &store).unwrap().0;
        let run = run_rules(
            &mut ops,
            &store,
            &trash,
            &[],
            &acc,
            &identity,
            &new,
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
                initial: false
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
            })
            .unwrap();
        sync_all(&mut ops, &store).unwrap();
        assert_eq!(store.folder("Archive").unwrap().unwrap().uidvalidity, 7);
        assert_eq!(store.messages_in_folder("Archive").unwrap().len(), 1);
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
        let new = sync_all(&mut ops, &store).unwrap().0;
        let run = run_rules(
            &mut ops,
            &store,
            &trash,
            &rules,
            &acc,
            &identity,
            &new,
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
        let second = sync_all(&mut ops, &store).unwrap().0;
        let run = run_rules(
            &mut ops,
            &store,
            &trash,
            &[],
            &acc,
            &identity,
            &second,
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
            &resync,
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
        let toml = "[[rules]]\nname = \"codes\"\nmatch.body = { contains = \"code\" }\nactions = [\"flag\"]\n";
        let rules = rules_from(toml, &store, 0);
        sync_all(&mut ops, &store).unwrap();
        ops.add_mail(
            "INBOX",
            2,
            11 * H,
            &headers("noreply@login.x", "Your sign-in code", "m2@x"),
            Some("From: noreply@login.x\r\n\r\ncode 1234"),
        );
        let new = sync_all(&mut ops, &store).unwrap().0;
        let run = run_rules(
            &mut ops,
            &store,
            &trash,
            &rules,
            &acc,
            &identity,
            &new,
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
    fn body_rule_skips_body_fetch_on_first_sync() {
        let mut ops = ops_with_inbox();
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory().unwrap();
        let trash = Trash::new(dir.path().to_path_buf());
        let acc = account();
        let identity = acc.identity().unwrap();
        let toml = "[[rules]]\nname = \"codes\"\nmatch.body = { contains = \"code\" }\nactions = [\"flag\"]\n";
        let rules = rules_from(toml, &store, 0);
        let new = sync_all(&mut ops, &store).unwrap().0;
        run_rules(
            &mut ops,
            &store,
            &trash,
            &rules,
            &acc,
            &identity,
            &new,
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
        let current = reload_rules(&store, &path, 200, good.clone());
        assert_eq!(current.len(), 1);
        assert!(
            current[0].rule.matches.seen.is_some(),
            "previous rules kept after a bad edit"
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
            std::sync::mpsc::channel::<Command>().1,
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
        let new = sync_all(&mut ops, &store).unwrap().0;
        let run = run_rules(
            &mut ops,
            &store,
            &trash,
            &rules,
            &acc,
            &identity,
            &new,
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
            })
            .unwrap();
        let rules = rules_from(
            "[[rules]]\nname = \"old\"\nfolder = \"Gone\"\nmatch.seen = true\nactions = [\"flag\"]\n",
            &store,
            0,
        );
        let new = sync_all(&mut ops, &store).unwrap().0;
        let run = run_rules(
            &mut ops,
            &store,
            &trash,
            &rules,
            &acc,
            &identity,
            &new,
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
        let new = sync_all(&mut ops, &store).unwrap().0;
        ops.calls.clear();
        let run = run_rules(
            &mut ops,
            &store,
            &trash,
            &rules,
            &acc,
            &identity,
            &new,
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
        let new = sync_all(&mut ops, &store).unwrap().0;
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
            &new,
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
            let new = sync_all(&mut ops, &store).unwrap().0;
            actions += run_rules(
                &mut ops,
                &store,
                &trash,
                &rules,
                &acc,
                &identity,
                &new,
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
            std::sync::mpsc::channel::<Command>().1,
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
        };
        assert!(sync_folder(&mut ops, &store, &inbox).is_err());
        assert_eq!(store.folder("INBOX").unwrap().unwrap().last_uid, 0);
        assert_eq!(store.message_count("INBOX").unwrap(), 0);
        ops.fail_fetch_after = None;
        let new = sync_folder(&mut ops, &store, &inbox).unwrap();
        assert!(new.iter().all(|n| n.initial), "{new:?}");
        let run = run_rules(
            &mut ops,
            &store,
            &trash,
            &[],
            &acc,
            &identity,
            &new,
            Mode::Normal,
            12 * H,
        )
        .unwrap();
        assert!(run.events.is_empty(), "{:?}", run.events);
    }

    fn inbox() -> RemoteFolder {
        RemoteFolder {
            name: "INBOX".into(),
            special_use: None,
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
        assert!(new.iter().filter(|n| n.uid <= 1200).all(|n| n.initial));
        assert!(!new.iter().find(|n| n.uid == 1201).unwrap().initial);
    }

    #[test]
    fn a_folder_that_starts_empty_notifies_its_first_mail() {
        let mut ops = RecordingOps::new().with_folder("INBOX", None);
        let store = Store::open_in_memory().unwrap();
        assert!(sync_folder(&mut ops, &store, &inbox()).unwrap().is_empty());
        assert_eq!(store.initial_uid_next("INBOX").unwrap(), 0);
        ops.add_mail("INBOX", 1, 10 * H, &headers("a@x", "first", "f@x"), None);
        let new = sync_folder(&mut ops, &store, &inbox()).unwrap();
        assert!(!new[0].initial);
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
        let new = sync_all(&mut ops, &store).unwrap().0;
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
            &new,
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
        };
        let new = sync_folder(&mut ops, &store, &inbox).unwrap();
        let run = run_rules(
            &mut ops,
            &store,
            &trash,
            &rules,
            &acc,
            &identity,
            &new,
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
            std::sync::mpsc::channel::<Command>().1,
            Arc::new(AtomicBool::new(false)),
            || {
                connects += 1;
                match connects {
                    1..=8 => Err(SyncError::Mail(MailError::Connect("refused".into()))),
                    // No folders: the pass completes, then selecting INBOX before IDLE fails.
                    9 => Ok(Box::new(RecordingOps::new()) as Box<dyn MailOps>),
                    // The full pass skips the failing INBOX; after an IDLE wake the INBOX-only pass fails.
                    _ => {
                        let mut ops = RecordingOps::new().with_folder("INBOX", None);
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
        commands_tx
            .send(Command::Apply {
                folder: "INBOX".into(),
                uids: vec![1],
                action: Action::MarkRead,
            })
            .unwrap();
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
    fn sync_now_during_a_pass_runs_another_full_pass() {
        let mut ops = ops_with_inbox();
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let (tx, rx) = std::sync::mpsc::channel();
        let (commands_tx, commands) = std::sync::mpsc::channel();
        commands_tx.send(Command::SyncNow).unwrap();
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
            let (paths, shutdown, wake) =
                (Paths::under(dir.path()), shutdown.clone(), wake.clone());
            let commands = std::sync::mpsc::channel::<Command>().1;
            std::thread::spawn(move || run_loop(offline, paths, tx, shutdown, commands, wake))
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

    fn synced() -> (RecordingOps, Store, tempfile::TempDir) {
        let mut ops = ops_with_inbox();
        let store = Store::open_in_memory().unwrap();
        sync_all(&mut ops, &store).unwrap();
        (ops, store, tempfile::tempdir().unwrap())
    }

    fn drain(
        ops: &mut RecordingOps,
        store: &Store,
        trash: &Trash,
        commands: Vec<Command>,
    ) -> (Result<CommandsRun, SyncError>, Vec<Event>) {
        let (tx, rx) = std::sync::mpsc::channel();
        for c in commands {
            tx.send(c).unwrap();
        }
        let (events, seen) = std::sync::mpsc::channel();
        let run = run_commands(ops, store, trash, &account(), &rx, &events);
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
        };
        let (run, events) = drain(&mut ops, &store, &trash, vec![apply, Command::SyncNow]);
        assert_eq!(
            run.unwrap(),
            CommandsRun {
                used_connection: true,
                wants_full_pass: true
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
            uid: 1
        }));
    }

    #[test]
    fn restore_appends_the_backup_and_asks_for_a_full_pass() {
        let (mut ops, store, dir) = synced();
        let trash = Trash::new(dir.path().to_path_buf());
        let file = trash
            .save("INBOX", 7, b"Subject: back\r\n\r\nhi", 100)
            .unwrap();
        let restore = Command::Restore { file: file.clone() };
        let (run, events) = drain(&mut ops, &store, &trash, vec![restore]);
        assert!(run.unwrap().wants_full_pass);
        assert!(!file.exists());
        assert!(ops.calls.iter().any(|c| c.starts_with("append INBOX")));
        assert!(events.contains(&Event::Restored {
            account: "work".into(),
            folder: "INBOX".into()
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
            |e| matches!(e, Event::Error { message, .. } if message.contains("not a backup"))
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
            |e| matches!(e, Event::Error { message, .. } if message.contains("not a backup"))
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
        assert!(!events.iter().any(|e| matches!(e, Event::Error { .. })));
    }

    #[test]
    fn a_lost_connection_during_restore_ends_the_drain() {
        let (mut ops, store, dir) = synced();
        let trash = Trash::new(dir.path().to_path_buf());
        let file = trash
            .save("INBOX", 7, b"Subject: back\r\n\r\n", 100)
            .unwrap();
        ops.fail_next = Some(MailError::Io("reset".into()));
        let (run, _) = drain(
            &mut ops,
            &store,
            &trash,
            vec![Command::Restore { file: file.clone() }],
        );
        assert!(matches!(run, Err(SyncError::Mail(MailError::Io(_)))));
        assert!(file.exists());
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
            |e| matches!(e, Event::Error { message, .. } if message.contains("INBOX/1") && message.contains("NO"))
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
                .any(|e| matches!(e, Event::Error { message, .. } if message.contains("INBOX/1")))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::BodyReady { uid: 2, .. }))
        );
    }
}
