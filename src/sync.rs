use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::{AccountConfig, ConfigError, Identity};
use crate::credentials::{self, CredentialError};
use crate::mail_ops::{Envelope, IdleOutcome, MailError, MailOps, RemoteFolder};
use crate::message::{body_text, parse_headers, thread_id};
use crate::paths::Paths;
use crate::rules::apply::{ApplyError, apply, ensure_raw};
use crate::rules::engine::{Context, Mode, evaluate, folder_needs_body};
use crate::rules::{CompiledRule, RulesError};
use crate::store::{Folder, Message, Store, StoreError};
use crate::trash::Trash;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
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
    /// Found while the folder was newly tracked, a placeholder, or reset after a UIDVALIDITY change; such messages never notify.
    pub initial: bool,
}

#[derive(Debug, Default)]
pub struct RulesRun {
    pub evaluated: usize,
    pub actions: usize,
    /// NewMail notifications and per-folder or per-message errors that did not stop the run.
    pub events: Vec<Event>,
}

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
    let info = ops.select(&folder.name)?;
    let stored = store.folder(&folder.name)?;
    // Initial means newly tracked, a placeholder, or reset: everything fetched this pass was already on the server.
    let (last_uid, initial) = match &stored {
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

    // Network first, then one transaction, so an interrupted pass leaves the folder as it was.
    let updates = if last_uid > 0 {
        ops.fetch_flags(last_uid)?
    } else {
        Vec::new()
    };
    let envelopes: Vec<Envelope> = ops
        .fetch_new(last_uid + 1)?
        .into_iter()
        .filter(|env| env.uid > last_uid)
        .collect();
    let highest = envelopes.iter().map(|env| env.uid).fold(last_uid, u32::max);

    store.transaction(|| {
        if initial && stored.is_some() {
            store.reset_folder(&folder.name, info.uidvalidity)?;
        }
        store.upsert_folder(&Folder {
            name: folder.name.clone(),
            uidvalidity: info.uidvalidity,
            last_uid: highest,
            special_use: folder.special_use.clone(),
        })?;
        let present: Vec<u32> = updates.iter().map(|u| u.uid).collect();
        for u in &updates {
            store.update_flags(&folder.name, u.uid, &u.flags.join(" "))?;
        }
        if last_uid > 0 {
            store.remove_missing(&folder.name, last_uid, &present)?;
        }
        let mut new = Vec::new();
        for env in envelopes {
            let uid = env.uid;
            store.insert_message(&to_message(&folder.name, env))?;
            new.push(NewMessageRef {
                folder: folder.name.clone(),
                uid,
                initial,
            });
        }
        Ok(new)
    })
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

/// Syncs every listed folder. A folder that fails is skipped and reported in the second list; the others still sync.
pub fn sync_all(
    ops: &mut dyn MailOps,
    store: &Store,
) -> Result<(Vec<NewMessageRef>, Vec<String>), SyncError> {
    let mut folders = ops.list_folders()?;
    special_use_by_name(&mut folders);
    let mut new = Vec::new();
    let mut errors = Vec::new();
    for folder in folders {
        match sync_folder(ops, store, &folder) {
            Ok(found) => new.extend(found),
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
    for rule in &mut compiled {
        rule.first_seen_at = store
            .rule_first_seen(&rule.rule.name, now)
            .map_err(|e| RulesError::Parse(e.to_string()))?;
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
        for mut msg in store.messages_in_folder(&folder)? {
            let is_fresh = fresh.contains(&(msg.folder.as_str(), msg.uid));
            // Mail found by a first sync or resync already sat on the server; fetching it would download the whole folder.
            if needs_body && msg.body_text.is_none() && is_fresh {
                match ensure_raw(&msg, ops, store) {
                    Ok(raw) => msg.body_text = Some(body_text(&raw)),
                    Err(e) => log::warn!("{}/{}: body fetch failed: {e}", msg.folder, msg.uid),
                }
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

pub fn run_once(
    account: &AccountConfig,
    paths: &Paths,
    events: &Sender<Event>,
) -> Result<(), SyncError> {
    let secret = credentials::resolve(account)?;
    let mut ops = crate::mail_ops::imap::ImapOps::connect(account, &secret)?;
    paths.ensure_account(&account.name)?;
    let store = Store::open(&paths.mail_db(&account.name))?;
    let trash = Trash::new(paths.trash_dir(&account.name));
    let identity = account.identity()?;
    let rules = load_rules_for(&store, &paths.rules_file(), now())?;
    let (new, sync_errors) = sync_all(&mut ops, &store)?;
    for message in sync_errors {
        let _ = events.send(account_error(account, message));
    }
    let run = run_rules(
        &mut ops,
        &store,
        &trash,
        &rules,
        account,
        &identity,
        &new,
        Mode::Normal,
        now(),
    )?;
    for event in run.events {
        let _ = events.send(event);
    }
    let _ = events.send(Event::Synced {
        account: account.name.clone(),
        new_messages: new.len(),
        actions: run.actions,
    });
    Ok(())
}

pub fn run_loop(
    account: AccountConfig,
    paths: Paths,
    events: Sender<Event>,
    shutdown: Arc<AtomicBool>,
) {
    let name = account.name.clone();
    run_loop_with(
        account.clone(),
        paths,
        events,
        shutdown,
        move || {
            let secret = credentials::resolve(&account)?;
            Ok(
                Box::new(crate::mail_ops::imap::ImapOps::connect(&account, &secret)?)
                    as Box<dyn MailOps>,
            )
        },
        |delay| {
            log::warn!("{name}: reconnecting in {}s", delay.as_secs());
            std::thread::sleep(delay);
        },
    );
}

/// The loop body, with connection and back-off sleeping injected so tests can drive it with the fake.
pub fn run_loop_with(
    account: AccountConfig,
    paths: Paths,
    events: Sender<Event>,
    shutdown: Arc<AtomicBool>,
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
            &mut connect,
            &mut completed_cycle,
        );
        if completed_cycle {
            backoff = Duration::from_secs(5);
        }
        match result {
            Ok(()) => return,
            Err(e) => {
                let _ = events.send(Event::Error {
                    account: account.name.clone(),
                    message: e.to_string(),
                });
                sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(300));
            }
        }
    }
}

fn run_session(
    account: &AccountConfig,
    paths: &Paths,
    events: &Sender<Event>,
    shutdown: &AtomicBool,
    connect: &mut impl FnMut() -> Result<Box<dyn MailOps>, SyncError>,
    completed_cycle: &mut bool,
) -> Result<(), SyncError> {
    let mut ops = connect()?;
    paths.ensure_account(&account.name)?;
    let store = Store::open(&paths.mail_db(&account.name))?;
    let trash = Trash::new(paths.trash_dir(&account.name));
    let identity = account.identity()?;
    let rules_path = paths.rules_file();
    let mut rules = load_rules_for(&store, &rules_path, now())?;
    let interval = Duration::from_secs(account.sync_interval_secs.max(10));
    let mut last_purge = 0i64;
    let mut full = true;

    while !shutdown.load(Ordering::Relaxed) {
        let new = if full {
            let (new, sync_errors) = sync_all(ops.as_mut(), &store)?;
            for message in sync_errors {
                let _ = events.send(account_error(account, message));
            }
            new
        } else {
            let inbox = RemoteFolder {
                name: "INBOX".into(),
                special_use: None,
            };
            sync_folder(ops.as_mut(), &store, &inbox)?
        };
        rules = reload_rules(&store, &rules_path, now(), rules);
        let run = run_rules(
            ops.as_mut(),
            &store,
            &trash,
            &rules,
            account,
            &identity,
            &new,
            Mode::Normal,
            now(),
        )?;
        for event in run.events {
            let _ = events.send(event);
        }
        let _ = events.send(Event::Synced {
            account: account.name.clone(),
            new_messages: new.len(),
            actions: run.actions,
        });
        if now() - last_purge > 3600 {
            match trash.purge(account.trash_retention_days as i64 * 86_400, now()) {
                Ok(0) => {}
                Ok(removed) => log::info!("{}: purged {removed} trash files", account.name),
                Err(e) => log::warn!("{}: trash purge failed: {e}", account.name),
            }
            last_purge = now();
        }
        ops.select("INBOX")?;
        let outcome = ops.idle(interval, shutdown)?;
        // A pass plus a wait means the connection is healthy; a pass alone does not, e.g. when IDLE always fails.
        *completed_cycle = true;
        full = match outcome {
            IdleOutcome::NewMail => false,
            IdleOutcome::Timeout => true,
            IdleOutcome::Interrupted => return Ok(()),
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
        assert!(ops.calls.iter().any(|c| c == "fetch_new INBOX 3"));
    }

    #[test]
    fn fetch_new_ignores_uids_below_last_uid() {
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
        let stop = shutdown.clone();
        let mut connects = 0;
        run_loop_with(
            account(),
            paths,
            tx,
            shutdown,
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
        run_loop_with(
            account(),
            paths.clone(),
            tx,
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
        ops.fail_fetch_new = true;
        let inbox = RemoteFolder {
            name: "INBOX".into(),
            special_use: None,
        };
        assert!(sync_folder(&mut ops, &store, &inbox).is_err());
        assert_eq!(store.folder("INBOX").unwrap(), None, "nothing written");
        ops.fail_fetch_new = false;
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
            || {
                connects += 1;
                match connects {
                    1..=8 => Err(SyncError::Mail(MailError::Connect("refused".into()))),
                    // No folders: the pass completes, then selecting INBOX before IDLE fails.
                    9 => Ok(Box::new(RecordingOps::new()) as Box<dyn MailOps>),
                    // Survives a pass and an IDLE wake, then the INBOX fetch fails.
                    _ => {
                        let mut ops = RecordingOps::new().with_folder("INBOX", None);
                        ops.fail_fetch_new = true;
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
}
