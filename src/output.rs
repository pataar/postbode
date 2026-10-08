//! JSON rows shared by the CLI's `--json` and the MCP results; each row carries its account.
use serde_json::{Value, json};

use crate::rules::Rule;
use crate::store::{Folder, Message};

pub fn with_account(account: &str, row: &impl serde::Serialize) -> serde_json::Result<Value> {
    let mut value = serde_json::to_value(row)?;
    if let Some(object) = value.as_object_mut() {
        object.insert("account".into(), account.into());
    }
    Ok(value)
}

pub fn folder(account: &str, folder: &Folder, total: u32, unread: u32) -> Value {
    json!({ "account": account, "folder": folder.name, "total": total, "unread": unread, "special_use": folder.special_use })
}

/// Each message's indent in its thread, relative to the thread's shallowest message and capped at 4.
pub fn depths(thread: &[Message]) -> Vec<usize> {
    let base = thread.iter().map(Message::thread_depth).min().unwrap_or(0);
    thread
        .iter()
        .map(|m| (m.thread_depth() - base).min(4))
        .collect()
}

/// A rule's listing row; `account` is the rule's own scope, `null` for every account.
pub fn rule(rule: &Rule) -> Value {
    json!({ "name": rule.name, "enabled": rule.enabled, "proposed_by": rule.proposed_by, "account": rule.account, "folder": rule.folder })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Message;

    fn message(uid: u32, refs: Option<&str>) -> Message {
        Message {
            folder: "INBOX".into(),
            uid,
            refs: refs.map(str::to_string),
            thread_id: "t".into(),
            ..Default::default()
        }
    }

    #[test]
    fn with_account_adds_the_account_key() {
        let row = with_account("work", &serde_json::json!({ "uid": 1 })).unwrap();
        assert_eq!(row, serde_json::json!({ "account": "work", "uid": 1 }));
    }

    #[test]
    fn depths_are_relative_to_the_shallowest_and_capped_at_four() {
        let thread = vec![
            message(1, Some("a")),
            message(2, Some("a b")),
            message(3, Some("a b c d e f g")),
        ];
        assert_eq!(depths(&thread), vec![0, 1, 4]);
    }
}
