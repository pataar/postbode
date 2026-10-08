//! Direct actions on chosen messages: mark, move, archive and delete, through the same `apply` as rules, which the
//! daemon's sync loop runs; plus the dry-run and rule preview helpers the CLI and the MCP server share.
use crate::config::{AccountConfig, Identity};
use crate::mail_ops::{MailError, MailOps};
use crate::message::clean;
use crate::rules::apply::{ApplyError, apply, ensure_raw, trash_destination};
use crate::rules::engine::{Context, Mode, Plan, PlannedAction, evaluate};
use crate::rules::{Action, CompiledRule};
use crate::store::{Message, Store, StoreError};
use crate::trash::Trash;

/// The rule name the CLI logs direct actions under.
pub const RULE_NAME: &str = "cli";

#[derive(Debug, thiserror::Error)]
pub enum ActionError {
    #[error(transparent)]
    Apply(#[from] ApplyError),
    #[error("{0} changed on the server since the last sync; run `postbode sync` first")]
    FolderChanged(String),
    #[error(transparent)]
    Mail(#[from] MailError),
    #[error("no message {folder}/{uid} in the local store; run `postbode sync` first")]
    NotFound { folder: String, uid: u32 },
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{0} has not been synced yet; run `postbode sync` first")]
    UnknownFolder(String),
}

/// The outcome for one uid: how many actions ran, or why none did.
pub type UidResult = (u32, Result<usize, ActionError>);

/// Selects `folder`, refusing when the stored uids belong to another UIDVALIDITY and would hit unrelated messages.
pub fn select_synced(
    ops: &mut dyn MailOps,
    store: &Store,
    folder: &str,
) -> Result<(), ActionError> {
    let stored = store
        .folder(folder)?
        .ok_or_else(|| ActionError::UnknownFolder(folder.to_string()))?;
    if ops.select(folder)?.uidvalidity != stored.uidvalidity {
        return Err(ActionError::FolderChanged(folder.to_string()));
    }
    Ok(())
}

/// Runs `action` on each uid in `folder`, logged under `rule_name`. One result per uid, so a missing message does not
/// stop the others.
#[allow(clippy::too_many_arguments)]
pub fn run(
    ops: &mut dyn MailOps,
    store: &Store,
    trash: &Trash,
    folder: &str,
    uids: &[u32],
    action: &Action,
    rule_name: &str,
    now: i64,
) -> Result<Vec<UidResult>, ActionError> {
    select_synced(ops, store, folder)?;
    let plan = Plan {
        actions: vec![PlannedAction {
            rule: rule_name.to_string(),
            action: action.clone(),
        }],
        notify: false,
    };
    let mut results = Vec::new();
    for &uid in uids {
        let result = match store.message(folder, uid) {
            Ok(Some(msg)) => apply(&plan, &msg, ops, store, trash, now).map_err(ActionError::from),
            Ok(None) => Err(ActionError::NotFound {
                folder: folder.to_string(),
                uid,
            }),
            Err(e) => Err(e.into()),
        };
        results.push((uid, result));
    }
    Ok(results)
}

/// Downloads and indexes every body `folder` lacks. Returns how many arrived; a message that cannot be fetched is
/// logged and skipped.
pub fn fetch_bodies(
    ops: &mut dyn MailOps,
    store: &Store,
    folder: &str,
) -> Result<usize, ActionError> {
    let missing: Vec<Message> = store
        .messages_in_folder(folder)?
        .into_iter()
        .filter(|m| m.body_text.is_none())
        .collect();
    if missing.is_empty() {
        return Ok(0);
    }
    select_synced(ops, store, folder)?;
    let mut fetched = 0;
    for msg in &missing {
        match ensure_raw(msg, ops, store) {
            Ok(_) => fetched += 1,
            Err(e) => log::warn!("{}/{}: body fetch failed: {e}", msg.folder, msg.uid),
        }
    }
    Ok(fetched)
}

/// Downloads and indexes one message body, refusing when the folder changed on the server since the last sync.
pub fn fetch_body(
    ops: &mut dyn MailOps,
    store: &Store,
    folder: &str,
    uid: u32,
) -> Result<(), ActionError> {
    select_synced(ops, store, folder)?;
    let msg = store
        .message(folder, uid)?
        .ok_or_else(|| ActionError::NotFound {
            folder: folder.to_string(),
            uid,
        })?;
    ensure_raw(&msg, ops, store)?;
    Ok(())
}

/// What a dry run would do to `msg`; a user delete names its outcome, because an expunge cannot be undone on the server.
pub fn planned_effect(store: &Store, msg: &Message, action: &Action) -> Result<String, StoreError> {
    if *action != Action::Trash {
        return Ok(format!("would {}", clean(&action.label(), false)));
    }
    let effect = match trash_destination(store, msg)? {
        Some(trash) => format!("would move to {}", clean(&trash, false)),
        None => "would delete (expunge, .eml backup kept)".to_string(),
    };
    Ok(effect)
}

/// One action a rule would take on a cached message.
#[derive(Debug, Clone)]
pub struct Planned {
    pub rule: String,
    pub message: Message,
    pub action: Action,
}

/// What `rules` would do to every message cached in `store`, evaluated as `rules apply-existing` would.
pub fn planned(
    rules: &[CompiledRule],
    store: &Store,
    account: &AccountConfig,
    identity: &Identity,
    now: i64,
) -> Result<Vec<Planned>, StoreError> {
    let ctx = Context {
        account: &account.name,
        identity,
        now,
        mode: Mode::ApplyExisting,
        notify_default: false,
    };
    let mut planned = Vec::new();
    for folder in store.folders()? {
        for message in store.messages_in_folder(&folder.name)? {
            for PlannedAction { rule, action } in evaluate(rules, &message, &ctx).actions {
                planned.push(Planned {
                    rule,
                    message: message.clone(),
                    action,
                });
            }
        }
    }
    Ok(planned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail_ops::RecordingOps;
    use crate::store::{Folder, Message};

    fn setup() -> (RecordingOps, Store, Trash, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut ops = RecordingOps::new().with_folder("INBOX", None);
        ops.add_mail(
            "INBOX",
            5,
            100,
            "Subject: hi\r\n\r\n",
            Some("Subject: hi\r\n\r\nbody"),
        );
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_folder(&Folder {
                name: "INBOX".into(),
                uidvalidity: 1,
                last_uid: 5,
                special_use: None,
            })
            .unwrap();
        store
            .insert_message(&Message {
                folder: "INBOX".into(),
                uid: 5,
                thread_id: "t".into(),
                subject: Some("hi".into()),
                internaldate: 100,
                headers: b"Subject: hi\r\n\r\n".to_vec(),
                ..Default::default()
            })
            .unwrap();
        (ops, store, Trash::new(dir.path().join("trash")), dir)
    }

    #[test]
    fn planned_effect_names_a_delete_without_trash() {
        let (_ops, store, _trash, _dir) = setup();
        let msg = store.message("INBOX", 5).unwrap().unwrap();
        assert_eq!(
            planned_effect(&store, &msg, &Action::Trash).unwrap(),
            "would delete (expunge, .eml backup kept)"
        );
        assert_eq!(
            planned_effect(&store, &msg, &Action::Archive).unwrap(),
            "would archive"
        );
    }

    #[test]
    fn run_applies_to_each_uid_and_reports_missing_ones() {
        let (mut ops, store, trash, _dir) = setup();
        let results = run(
            &mut ops,
            &store,
            &trash,
            "INBOX",
            &[5, 99],
            &Action::Flag,
            "mcp:test-host",
            500,
        )
        .unwrap();
        assert!(matches!(results[0], (5, Ok(1))));
        assert!(matches!(
            results[1],
            (99, Err(ActionError::NotFound { .. }))
        ));
        assert_eq!(ops.mail["INBOX"][0].flags, ["\\Flagged"]);
        assert_eq!(store.log(10).unwrap()[0].rule_name, "mcp:test-host");
    }

    #[test]
    fn run_refuses_a_folder_whose_uidvalidity_changed() {
        let (mut ops, store, trash, _dir) = setup();
        ops.uidvalidity.insert("INBOX".into(), 2);
        let err = run(
            &mut ops,
            &store,
            &trash,
            "INBOX",
            &[5],
            &Action::Trash,
            RULE_NAME,
            500,
        )
        .unwrap_err();
        assert!(matches!(err, ActionError::FolderChanged(_)));
        assert_eq!(ops.mail["INBOX"].len(), 1);
        assert_eq!(ops.calls, ["select INBOX"]);
    }

    #[test]
    fn run_refuses_a_folder_that_was_never_synced() {
        let (mut ops, store, trash, _dir) = setup();
        let err = run(
            &mut ops,
            &store,
            &trash,
            "Receipts",
            &[5],
            &Action::Flag,
            RULE_NAME,
            500,
        )
        .unwrap_err();
        assert!(matches!(err, ActionError::UnknownFolder(_)));
        assert!(ops.calls.is_empty());
    }

    #[test]
    fn fetch_bodies_indexes_only_missing_bodies() {
        let (mut ops, store, _trash, _dir) = setup();
        assert_eq!(fetch_bodies(&mut ops, &store, "INBOX").unwrap(), 1);
        assert_eq!(store.search("body", None, 10).unwrap().len(), 1);
        let calls = ops.calls.len();
        assert_eq!(fetch_bodies(&mut ops, &store, "INBOX").unwrap(), 0);
        assert_eq!(ops.calls.len(), calls, "nothing left to fetch");
    }
}
