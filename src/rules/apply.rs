use std::io;

use crate::mail_ops::{MailError, MailOps};
use crate::message::body_text;
use crate::rules::Action;
use crate::rules::engine::{Plan, PlannedAction};
use crate::store::{Folder, LogEntry, Message, Store, StoreError};
use crate::trash::Trash;

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error(transparent)]
    Mail(#[from] MailError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("could not write trash file: {0}")]
    Trash(#[from] io::Error),
    #[error("the server has no Archive folder")]
    NoArchiveFolder,
    #[error("message {folder}/{uid} could not be fetched for backup")]
    RawUnavailable { folder: String, uid: u32 },
}

pub fn ensure_raw(
    msg: &Message,
    ops: &mut dyn MailOps,
    store: &Store,
) -> Result<Vec<u8>, ApplyError> {
    if let Some(raw) = store.raw(&msg.folder, msg.uid)? {
        return Ok(raw);
    }
    let raw = ops
        .fetch_raw(msg.uid)?
        .ok_or_else(|| ApplyError::RawUnavailable {
            folder: msg.folder.clone(),
            uid: msg.uid,
        })?;
    store.set_raw(&msg.folder, msg.uid, &raw, &body_text(&raw))?;
    Ok(raw)
}

/// A delete or user delete wins over everything else in the plan. Otherwise flag actions run first, then only the first
/// move or archive. Returns how many actions ran; a flag already in the wanted state is skipped without a log row.
pub fn apply(
    plan: &Plan,
    msg: &Message,
    ops: &mut dyn MailOps,
    store: &Store,
    trash: &Trash,
    now: i64,
) -> Result<usize, ApplyError> {
    if let Some(planned) = plan
        .actions
        .iter()
        .find(|planned| matches!(planned.action, Action::Delete | Action::Trash))
    {
        match trash_target(planned, msg, store)? {
            Some(folder) => {
                store.log_action(&log_entry(planned, msg, now))?;
                move_to(msg, &folder, ops, store)?;
            }
            None => delete(planned, msg, ops, store, trash, now)?,
        }
        return Ok(1);
    }
    let first_move = first_move(plan, msg, store)?;
    let mut current = msg.clone();
    let mut executed = 0;
    for planned in &plan.actions {
        let (flag, wanted) = match planned.action {
            Action::Flag => ("\\Flagged", true),
            Action::MarkRead => ("\\Seen", true),
            Action::MarkUnread => ("\\Seen", false),
            Action::Unflag => ("\\Flagged", false),
            _ => continue,
        };
        if has_flag(&current, flag) == wanted {
            continue;
        }
        store.log_action(&log_entry(planned, msg, now))?;
        set_flag(&mut current, flag, wanted, ops, store)?;
        executed += 1;
    }
    if let Some((planned, target)) = first_move {
        store.log_action(&log_entry(planned, msg, now))?;
        move_to(&current, &target, ops, store)?;
        executed += 1;
    }
    Ok(executed)
}

/// The first move or archive in the plan with its resolved target. Resolved before anything runs so a failure leaves no
/// partial effects; a move into the message's own folder is dropped because the next sync would re-add and re-move it.
fn first_move<'a>(
    plan: &'a Plan,
    msg: &Message,
    store: &Store,
) -> Result<Option<(&'a PlannedAction, String)>, ApplyError> {
    let Some(planned) = plan
        .actions
        .iter()
        .find(|planned| matches!(planned.action, Action::Move(_) | Action::Archive))
    else {
        return Ok(None);
    };
    let target = match &planned.action {
        Action::Move(target) => target.clone(),
        _ => archive_folder(store)?,
    };
    Ok((target != msg.folder).then_some((planned, target)))
}

fn log_entry(planned: &PlannedAction, msg: &Message, now: i64) -> LogEntry {
    LogEntry {
        id: 0,
        at: now,
        rule_name: planned.rule.clone(),
        folder: msg.folder.clone(),
        uid: msg.uid,
        message_id: msg.message_id.clone(),
        subject: msg.subject.clone(),
        action: planned.action.label(),
        trash_file: None,
    }
}

fn delete(
    planned: &PlannedAction,
    msg: &Message,
    ops: &mut dyn MailOps,
    store: &Store,
    trash: &Trash,
    now: i64,
) -> Result<(), ApplyError> {
    let raw = ensure_raw(msg, ops, store)?;
    let path = trash.save(&msg.folder, msg.uid, &raw, now)?;
    let mut entry = log_entry(planned, msg, now);
    entry.trash_file = Some(path.to_string_lossy().into_owned());
    store.log_action(&entry)?;
    ops.add_flags(msg.uid, &["\\Deleted"])?;
    ops.expunge(msg.uid)?;
    store.remove_message(&msg.folder, msg.uid)?;
    Ok(())
}

fn has_flag(msg: &Message, flag: &str) -> bool {
    msg.flags.split(' ').any(|f| f == flag)
}

fn set_flag(
    current: &mut Message,
    flag: &str,
    wanted: bool,
    ops: &mut dyn MailOps,
    store: &Store,
) -> Result<(), ApplyError> {
    if wanted {
        ops.add_flags(current.uid, &[flag])?;
    } else {
        ops.remove_flags(current.uid, &[flag])?;
    }
    let mut flags: Vec<&str> = current
        .flags
        .split(' ')
        .filter(|f| !f.is_empty() && *f != flag)
        .collect();
    if wanted {
        flags.push(flag);
    }
    current.flags = flags.join(" ");
    store.update_flags(&current.folder, current.uid, &current.flags)?;
    Ok(())
}

fn special_folder(store: &Store, role: &str) -> Result<Option<String>, StoreError> {
    Ok(store
        .folders()?
        .into_iter()
        .find(|folder| folder.special_use.as_deref() == Some(role))
        .map(|folder| folder.name))
}

fn archive_folder(store: &Store) -> Result<String, ApplyError> {
    special_folder(store, "Archive")?.ok_or(ApplyError::NoArchiveFolder)
}

/// Where a user delete moves the message: the Trash folder, unless there is none or the message already sits in it.
fn trash_target(
    planned: &PlannedAction,
    msg: &Message,
    store: &Store,
) -> Result<Option<String>, ApplyError> {
    if planned.action != Action::Trash {
        return Ok(None);
    }
    Ok(special_folder(store, "Trash")?.filter(|folder| *folder != msg.folder))
}

/// When the server does not report the new uid, the local row is dropped and the next sync of the target folder re-adds it.
fn move_to(
    current: &Message,
    target: &str,
    ops: &mut dyn MailOps,
    store: &Store,
) -> Result<(), ApplyError> {
    let known = store.folder(target)?.is_some();
    // The folder may exist on the server before our first sync of it; a real create failure surfaces in the move.
    if !known && let Err(e) = ops.create_folder(target) {
        log::debug!("{target}: create failed ({e}), trying the move anyway");
    }
    let new_uid = ops.move_message(current.uid, target)?;
    if !known {
        store.upsert_folder(&Folder {
            name: target.to_string(),
            uidvalidity: 0,
            last_uid: 0,
            special_use: None,
        })?;
    }
    store.move_message_row(&current.folder, current.uid, target, new_uid)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail_ops::RecordingOps;

    fn setup() -> (RecordingOps, Store, Trash, tempfile::TempDir, Message) {
        let dir = tempfile::tempdir().unwrap();
        let mut ops = RecordingOps::new()
            .with_folder("INBOX", None)
            .with_folder("Archive", Some("Archive"));
        ops.add_mail(
            "INBOX",
            5,
            100,
            "Subject: hi\r\n\r\n",
            Some("Subject: hi\r\n\r\nbody text"),
        );
        ops.select("INBOX").unwrap();
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
            .upsert_folder(&Folder {
                name: "Archive".into(),
                uidvalidity: 1,
                last_uid: 0,
                special_use: Some("Archive".into()),
            })
            .unwrap();
        let msg = Message {
            folder: "INBOX".into(),
            uid: 5,
            message_id: Some("m5@x".into()),
            from_addr: Some("a@x".into()),
            to_addr: None,
            cc_addr: None,
            delivered_to: None,
            in_reply_to: None,
            refs: None,
            thread_id: "m5@x".into(),
            subject: Some("hi".into()),
            date: None,
            internaldate: 100,
            flags: String::new(),
            size: None,
            headers: b"Subject: hi\r\n\r\n".to_vec(),
            body_text: None,
        };
        store.insert_message(&msg).unwrap();
        let trash = Trash::new(dir.path().join("trash"));
        (ops, store, trash, dir, msg)
    }

    fn plan(actions: Vec<Action>) -> Plan {
        Plan {
            actions: actions
                .into_iter()
                .map(|action| PlannedAction {
                    rule: "r".into(),
                    action,
                })
                .collect(),
            notify: false,
        }
    }

    #[test]
    fn delete_backs_up_before_expunge_and_logs() {
        let (mut ops, store, trash, _dir, msg) = setup();
        apply(
            &plan(vec![Action::Delete]),
            &msg,
            &mut ops,
            &store,
            &trash,
            500,
        )
        .unwrap();
        let calls = ops.calls.clone();
        let fetch = calls
            .iter()
            .position(|c| c.starts_with("fetch_raw"))
            .unwrap();
        let expunge = calls.iter().position(|c| c.starts_with("expunge")).unwrap();
        assert!(fetch < expunge);
        let saved = trash.list().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(
            std::fs::read(&saved[0].path).unwrap(),
            b"Subject: hi\r\n\r\nbody text"
        );
        assert!(ops.mail["INBOX"].is_empty());
        assert_eq!(store.message("INBOX", 5).unwrap(), None);
        let log = store.log(10).unwrap();
        assert_eq!(log[0].action, "delete");
        assert!(
            log[0]
                .trash_file
                .as_deref()
                .unwrap()
                .ends_with("-INBOX-5.eml")
        );
    }

    #[test]
    fn delete_is_refused_when_raw_cannot_be_fetched() {
        let (mut ops, store, trash, _dir, msg) = setup();
        ops.raw.clear();
        let err = apply(
            &plan(vec![Action::Delete]),
            &msg,
            &mut ops,
            &store,
            &trash,
            500,
        )
        .unwrap_err();
        assert!(matches!(err, ApplyError::RawUnavailable { .. }));
        assert!(!ops.calls.iter().any(|c| c.starts_with("expunge")));
        assert_eq!(ops.mail["INBOX"].len(), 1);
    }

    #[test]
    fn delete_wins_over_other_actions() {
        let (mut ops, store, trash, _dir, msg) = setup();
        apply(
            &plan(vec![
                Action::Move("Archive".into()),
                Action::MarkRead,
                Action::Delete,
            ]),
            &msg,
            &mut ops,
            &store,
            &trash,
            500,
        )
        .unwrap();
        assert_eq!(trash.list().unwrap().len(), 1);
        assert!(ops.mail["INBOX"].is_empty());
        assert!(ops.mail["Archive"].is_empty());
        assert!(!ops.calls.iter().any(|c| c.starts_with("move")));
        assert!(
            !ops.calls
                .iter()
                .any(|c| c.starts_with("add_flags") && c.contains("\\Seen"))
        );
        assert_eq!(store.log(10).unwrap().len(), 1);
    }

    #[test]
    fn only_first_move_runs_and_flags_precede_it() {
        let (mut ops, store, trash, _dir, msg) = setup();
        apply(
            &plan(vec![
                Action::Move("Archive".into()),
                Action::MarkRead,
                Action::Move("Other".into()),
            ]),
            &msg,
            &mut ops,
            &store,
            &trash,
            500,
        )
        .unwrap();
        let flag = ops
            .calls
            .iter()
            .position(|c| c == "add_flags INBOX 5 \\Seen")
            .unwrap();
        let moved = ops
            .calls
            .iter()
            .position(|c| c == "move INBOX 5 -> Archive")
            .unwrap();
        assert!(flag < moved);
        assert!(!ops.calls.iter().any(|c| c.contains("Other")));
        let actions: Vec<String> = store
            .log(10)
            .unwrap()
            .into_iter()
            .rev()
            .map(|entry| entry.action)
            .collect();
        assert_eq!(actions, ["mark_read", "move:Archive"]);
    }

    #[test]
    fn move_creates_missing_folder_and_drops_row_for_resync() {
        let (mut ops, store, trash, _dir, msg) = setup();
        apply(
            &plan(vec![Action::Move("Lists/GitHub".into()), Action::MarkRead]),
            &msg,
            &mut ops,
            &store,
            &trash,
            500,
        )
        .unwrap();
        assert!(ops.calls.iter().any(|c| c == "create_folder Lists/GitHub"));
        assert!(store.folder("Lists/GitHub").unwrap().is_some());
        assert_eq!(store.message("INBOX", 5).unwrap(), None);
        assert!(
            store.messages_in_folder("Lists/GitHub").unwrap().is_empty(),
            "the next sync of the target folder adds the row"
        );
        assert!(
            ops.mail["Lists/GitHub"][0]
                .flags
                .contains(&"\\Seen".to_string()),
            "mark_read runs before the move, so the server copy carries it"
        );
    }

    #[test]
    fn move_into_folder_the_store_does_not_know_yet() {
        let (mut ops, store, trash, _dir, msg) = setup();
        ops = ops.with_folder("Lists", None);
        let executed = apply(
            &plan(vec![Action::Move("Lists".into())]),
            &msg,
            &mut ops,
            &store,
            &trash,
            500,
        )
        .unwrap();
        assert_eq!(executed, 1);
        assert_eq!(ops.mail["Lists"].len(), 1);
        assert!(ops.mail["INBOX"].is_empty());
        assert!(store.folder("Lists").unwrap().is_some());
    }

    #[test]
    fn move_to_own_folder_is_skipped() {
        let (mut ops, store, trash, _dir, msg) = setup();
        apply(
            &plan(vec![Action::Move("INBOX".into())]),
            &msg,
            &mut ops,
            &store,
            &trash,
            500,
        )
        .unwrap();
        assert!(!ops.calls.iter().any(|c| c.starts_with("move")));
        assert!(store.message("INBOX", 5).unwrap().is_some());
        assert!(store.log(10).unwrap().is_empty());
    }

    #[test]
    fn archive_without_folder_changes_nothing() {
        let (mut ops, store, trash, _dir, msg) = setup();
        store
            .upsert_folder(&Folder {
                name: "Archive".into(),
                uidvalidity: 1,
                last_uid: 0,
                special_use: None,
            })
            .unwrap();
        let result = apply(
            &plan(vec![Action::MarkRead, Action::Archive]),
            &msg,
            &mut ops,
            &store,
            &trash,
            500,
        );
        assert!(matches!(result, Err(ApplyError::NoArchiveFolder)));
        assert!(!ops.calls.iter().any(|c| c.starts_with("add_flags")));
        assert!(store.log(10).unwrap().is_empty());
    }

    #[test]
    fn delete_refused_when_trash_write_fails() {
        let (mut ops, store, _trash, dir, msg) = setup();
        let blocked = dir.path().join("not-a-dir");
        std::fs::write(&blocked, b"").unwrap();
        let err = apply(
            &plan(vec![Action::Delete]),
            &msg,
            &mut ops,
            &store,
            &Trash::new(blocked),
            500,
        )
        .unwrap_err();
        assert!(matches!(err, ApplyError::Trash(_)));
        assert!(
            !ops.calls
                .iter()
                .any(|c| c.starts_with("add_flags") || c.starts_with("expunge"))
        );
        assert_eq!(ops.mail["INBOX"].len(), 1);
        assert!(store.message("INBOX", 5).unwrap().is_some());
    }

    #[test]
    fn archive_uses_special_use_folder_or_errors() {
        let (mut ops, store, trash, _dir, msg) = setup();
        apply(
            &plan(vec![Action::Archive]),
            &msg,
            &mut ops,
            &store,
            &trash,
            500,
        )
        .unwrap();
        assert_eq!(ops.mail["Archive"].len(), 1);
        let (mut ops2, store2, trash2, _dir2, msg2) = setup();
        store2
            .upsert_folder(&Folder {
                name: "Archive".into(),
                uidvalidity: 1,
                last_uid: 0,
                special_use: None,
            })
            .unwrap();
        assert!(matches!(
            apply(
                &plan(vec![Action::Archive]),
                &msg2,
                &mut ops2,
                &store2,
                &trash2,
                500
            ),
            Err(ApplyError::NoArchiveFolder)
        ));
    }

    #[test]
    fn flag_and_mark_read_update_server_and_store() {
        let (mut ops, store, trash, _dir, msg) = setup();
        apply(
            &plan(vec![Action::Flag, Action::MarkRead]),
            &msg,
            &mut ops,
            &store,
            &trash,
            500,
        )
        .unwrap();
        let m = store.message("INBOX", 5).unwrap().unwrap();
        assert_eq!(m.flags, "\\Flagged \\Seen");
        assert_eq!(store.log(10).unwrap().len(), 2);
    }

    fn with_trash_folder(ops: RecordingOps, store: &Store) -> RecordingOps {
        store
            .upsert_folder(&Folder {
                name: "Trash".into(),
                uidvalidity: 1,
                last_uid: 0,
                special_use: Some("Trash".into()),
            })
            .unwrap();
        ops.with_folder("Trash", Some("Trash"))
    }

    #[test]
    fn mark_unread_and_unflag_remove_flags_once() {
        let (mut ops, store, trash, _dir, mut msg) = setup();
        ops.add_flags(5, &["\\Seen", "\\Flagged"]).unwrap();
        store.update_flags("INBOX", 5, "\\Seen \\Flagged").unwrap();
        msg.flags = "\\Seen \\Flagged".into();
        let unset = plan(vec![Action::MarkUnread, Action::Unflag]);
        assert_eq!(
            apply(&unset, &msg, &mut ops, &store, &trash, 500).unwrap(),
            2
        );
        assert!(ops.mail["INBOX"][0].flags.is_empty());
        let stored = store.message("INBOX", 5).unwrap().unwrap();
        assert_eq!(stored.flags, "");
        assert_eq!(
            apply(&unset, &stored, &mut ops, &store, &trash, 500).unwrap(),
            0
        );
        let actions: Vec<String> = store
            .log(10)
            .unwrap()
            .into_iter()
            .map(|e| e.action)
            .collect();
        assert_eq!(actions, ["unflag", "mark_unread"]);
    }

    #[test]
    fn user_delete_moves_to_the_trash_folder() {
        let (ops, store, trash, _dir, msg) = setup();
        let mut ops = with_trash_folder(ops, &store);
        assert_eq!(
            apply(
                &plan(vec![Action::Trash]),
                &msg,
                &mut ops,
                &store,
                &trash,
                500
            )
            .unwrap(),
            1
        );
        assert_eq!(ops.mail["Trash"].len(), 1);
        assert!(ops.mail["INBOX"].is_empty());
        assert!(!ops.calls.iter().any(|c| c.starts_with("expunge")));
        assert!(trash.list().unwrap().is_empty());
        assert_eq!(store.log(10).unwrap()[0].action, "trash");
    }

    #[test]
    fn user_delete_without_trash_folder_expunges_with_backup() {
        let (mut ops, store, trash, _dir, msg) = setup();
        apply(
            &plan(vec![Action::Trash]),
            &msg,
            &mut ops,
            &store,
            &trash,
            500,
        )
        .unwrap();
        assert!(ops.mail["INBOX"].is_empty());
        assert_eq!(trash.list().unwrap().len(), 1);
        assert!(store.log(10).unwrap()[0].trash_file.is_some());
    }

    #[test]
    fn user_delete_inside_the_trash_folder_expunges_with_backup() {
        let (mut ops, store, trash, _dir, msg) = setup();
        store
            .upsert_folder(&Folder {
                name: "INBOX".into(),
                uidvalidity: 1,
                last_uid: 5,
                special_use: Some("Trash".into()),
            })
            .unwrap();
        apply(
            &plan(vec![Action::Trash]),
            &msg,
            &mut ops,
            &store,
            &trash,
            500,
        )
        .unwrap();
        assert!(!ops.calls.iter().any(|c| c.starts_with("move")));
        assert!(ops.mail["INBOX"].is_empty());
        assert_eq!(trash.list().unwrap().len(), 1);
    }
}
