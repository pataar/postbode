//! Direct actions on chosen messages: the CLI's mark, move, archive and delete. They run through the same `apply` as rules.
use crate::mail_ops::{MailError, MailOps};
use crate::rules::Action;
use crate::rules::apply::{ApplyError, apply};
use crate::rules::engine::{Plan, PlannedAction};
use crate::store::{Store, StoreError};
use crate::trash::Trash;

/// The rule name direct actions are logged under.
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

/// Runs `action` on each uid in `folder`. One result per uid, so a missing message does not stop the others.
pub fn run(
    ops: &mut dyn MailOps,
    store: &Store,
    trash: &Trash,
    folder: &str,
    uids: &[u32],
    action: &Action,
    now: i64,
) -> Result<Vec<UidResult>, ActionError> {
    select_synced(ops, store, folder)?;
    let plan = Plan {
        actions: vec![PlannedAction {
            rule: RULE_NAME.to_string(),
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
                message_id: None,
                from_addr: None,
                to_addr: None,
                cc_addr: None,
                delivered_to: None,
                in_reply_to: None,
                refs: None,
                thread_id: "t".into(),
                subject: Some("hi".into()),
                date: None,
                internaldate: 100,
                flags: String::new(),
                size: None,
                headers: b"Subject: hi\r\n\r\n".to_vec(),
                body_text: None,
            })
            .unwrap();
        (ops, store, Trash::new(dir.path().join("trash")), dir)
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
            500,
        )
        .unwrap();
        assert!(matches!(results[0], (5, Ok(1))));
        assert!(matches!(
            results[1],
            (99, Err(ActionError::NotFound { .. }))
        ));
        assert_eq!(ops.mail["INBOX"][0].flags, ["\\Flagged"]);
        assert_eq!(store.log(10).unwrap()[0].rule_name, "cli");
    }

    #[test]
    fn run_refuses_a_folder_whose_uidvalidity_changed() {
        let (mut ops, store, trash, _dir) = setup();
        ops.uidvalidity.insert("INBOX".into(), 2);
        let err = run(&mut ops, &store, &trash, "INBOX", &[5], &Action::Trash, 500).unwrap_err();
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
            500,
        )
        .unwrap_err();
        assert!(matches!(err, ActionError::UnknownFolder(_)));
        assert!(ops.calls.is_empty());
    }
}
