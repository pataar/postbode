use std::io;
use std::path::Path;

use include_dir::{Dir, include_dir};
use rusqlite::{Connection, OptionalExtension, params};
use rusqlite_migration::Migrations;
use serde::{Deserialize, Serialize};

static MIGRATIONS_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/migrations");

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database migration: {0}")]
    Migration(#[from] rusqlite_migration::Error),
    #[error("database row: {0}")]
    Serde(#[from] serde_rusqlite::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub name: String,
    pub uidvalidity: u32,
    pub last_uid: u32,
    pub special_use: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub folder: String,
    pub uid: u32,
    pub message_id: Option<String>,
    pub from_addr: Option<String>,
    pub to_addr: Option<String>,
    pub cc_addr: Option<String>,
    pub delivered_to: Option<String>,
    pub in_reply_to: Option<String>,
    pub refs: Option<String>,
    pub thread_id: String,
    pub subject: Option<String>,
    pub date: Option<i64>,
    pub internaldate: i64,
    pub flags: String,
    pub size: Option<u32>,
    #[serde(skip_serializing, deserialize_with = "deserialize_blob")]
    pub headers: Vec<u8>,
    pub body_text: Option<String>,
}

impl Message {
    pub fn is_seen(&self) -> bool {
        self.flags.split(' ').any(|f| f == "\\Seen")
    }

    /// How deep in its thread the message sits, judged from its own reply headers.
    pub fn thread_depth(&self) -> usize {
        let refs = self
            .refs
            .as_deref()
            .map_or(0, |r| r.split_whitespace().count());
        refs.max(usize::from(self.in_reply_to.is_some()))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    #[serde(default)]
    pub id: i64,
    pub at: i64,
    pub rule_name: String,
    pub folder: String,
    pub uid: u32,
    pub message_id: Option<String>,
    pub subject: Option<String>,
    pub action: String,
    pub trash_file: Option<String>,
}

/// serde's `Vec<u8>` only accepts sequences; rusqlite offers BLOBs as bytes. Accept both.
fn deserialize_blob<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    struct BlobVisitor;
    impl<'de> serde::de::Visitor<'de> for BlobVisitor {
        type Value = Vec<u8>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("bytes")
        }
        fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Vec<u8>, E> {
            Ok(v.to_vec())
        }
        fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Vec<u8>, E> {
            Ok(v)
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u8>, A::Error> {
            let mut out = Vec::new();
            while let Some(b) = seq.next_element::<u8>()? {
                out.push(b);
            }
            Ok(out)
        }
    }
    d.deserialize_byte_buf(BlobVisitor)
}

pub struct Store {
    conn: Connection,
}

const MESSAGE_COLUMNS: &str = "folder, uid, message_id, from_addr, to_addr, cc_addr, delivered_to, in_reply_to, refs, thread_id, subject, date, internaldate, flags, size, headers, body_text";

impl Store {
    pub fn migrations() -> Migrations<'static> {
        Migrations::from_directory(&MIGRATIONS_DIR).expect("migrations directory is valid")
    }

    pub fn open(path: &Path) -> Result<Store, StoreError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        Store::init(conn)
    }

    pub fn open_in_memory() -> Result<Store, StoreError> {
        Store::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Store, StoreError> {
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // The CLI writes (direct actions, body fetches) while `postbode run` may hold the write lock.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Store::migrations().to_latest(&mut conn)?;
        Ok(Store { conn })
    }

    /// Runs `f` in one transaction; any error rolls back everything `f` wrote.
    pub fn transaction<T, E: From<StoreError>>(
        &self,
        f: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(StoreError::from)?;
        let out = f()?;
        tx.commit().map_err(StoreError::from)?;
        Ok(out)
    }

    pub fn folders(&self) -> Result<Vec<Folder>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT name, uidvalidity, last_uid, special_use FROM folders ORDER BY name",
        )?;
        let rows = serde_rusqlite::from_rows::<Folder>(stmt.query([])?);
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn folder(&self, name: &str) -> Result<Option<Folder>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT name, uidvalidity, last_uid, special_use FROM folders WHERE name = ?1",
        )?;
        let mut rows = stmt.query(params![name])?;
        match rows.next()? {
            Some(row) => Ok(Some(serde_rusqlite::from_row(row)?)),
            None => Ok(None),
        }
    }

    pub fn upsert_folder(&self, folder: &Folder) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO folders (name, uidvalidity, last_uid, special_use) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET uidvalidity = excluded.uidvalidity, last_uid = excluded.last_uid, special_use = excluded.special_use",
            params![folder.name, folder.uidvalidity, folder.last_uid, folder.special_use],
        )?;
        Ok(())
    }

    pub fn reset_folder(&self, name: &str, uidvalidity: u32) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM messages WHERE folder = ?1", params![name])?;
        self.conn.execute(
            "UPDATE folders SET uidvalidity = ?2, last_uid = 0 WHERE name = ?1",
            params![name, uidvalidity],
        )?;
        Ok(())
    }

    pub fn insert_message(&self, m: &Message) -> Result<(), StoreError> {
        self.conn.execute(
            &format!(
                "INSERT OR IGNORE INTO messages ({MESSAGE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"
            ),
            params![
                m.folder, m.uid, m.message_id, m.from_addr, m.to_addr, m.cc_addr, m.delivered_to, m.in_reply_to,
                m.refs, m.thread_id, m.subject, m.date, m.internaldate, m.flags, m.size, m.headers, m.body_text
            ],
        )?;
        Ok(())
    }

    pub fn update_flags(&self, folder: &str, uid: u32, flags: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE messages SET flags = ?3 WHERE folder = ?1 AND uid = ?2 AND flags IS NOT ?3",
            params![folder, uid, flags],
        )?;
        Ok(())
    }

    pub fn remove_message(&self, folder: &str, uid: u32) -> Result<(), StoreError> {
        self.conn.execute(
            "DELETE FROM messages WHERE folder = ?1 AND uid = ?2",
            params![folder, uid],
        )?;
        Ok(())
    }

    /// Removes rows with uid <= `upto_uid` whose uid is not in `present`.
    pub fn remove_missing(
        &self,
        folder: &str,
        upto_uid: u32,
        present: &[u32],
    ) -> Result<usize, StoreError> {
        let existing: Vec<u32> = {
            let mut stmt = self
                .conn
                .prepare("SELECT uid FROM messages WHERE folder = ?1 AND uid <= ?2")?;
            let rows = stmt.query_map(params![folder, upto_uid], |r| r.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        let present: std::collections::HashSet<u32> = present.iter().copied().collect();
        let mut removed = 0;
        for uid in existing.into_iter().filter(|u| !present.contains(u)) {
            self.remove_message(folder, uid)?;
            removed += 1;
        }
        Ok(removed)
    }

    /// Moves the row to `new_folder` under `new_uid`, or deletes it when the new uid is unknown (next sync re-adds it).
    pub fn move_message_row(
        &self,
        folder: &str,
        uid: u32,
        new_folder: &str,
        new_uid: Option<u32>,
    ) -> Result<(), StoreError> {
        match new_uid {
            Some(new_uid) => {
                self.conn.execute(
                    "UPDATE messages SET folder = ?3, uid = ?4 WHERE folder = ?1 AND uid = ?2",
                    params![folder, uid, new_folder, new_uid],
                )?;
                Ok(())
            }
            None => self.remove_message(folder, uid),
        }
    }

    pub fn messages(&self, folder: &str, limit: u32) -> Result<Vec<Message>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages WHERE folder = ?1 ORDER BY internaldate DESC, uid DESC LIMIT ?2"
        ))?;
        let rows = serde_rusqlite::from_rows::<Message>(stmt.query(params![folder, limit])?);
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Threads in `folder`, the most recently active first; each thread's messages oldest first.
    pub fn threads(&self, folder: &str, limit: u32) -> Result<Vec<Vec<Message>>, StoreError> {
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT thread_id FROM messages WHERE folder = ?1 GROUP BY thread_id
                 ORDER BY MAX(internaldate) DESC, MAX(uid) DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![folder, limit], |r| r.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages WHERE folder = ?1 AND thread_id = ?2 ORDER BY internaldate, uid"
        ))?;
        ids.iter()
            .map(|id| {
                let rows = serde_rusqlite::from_rows::<Message>(stmt.query(params![folder, id])?);
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .collect()
    }

    pub fn messages_in_folder(&self, folder: &str) -> Result<Vec<Message>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages WHERE folder = ?1 ORDER BY uid"
        ))?;
        let rows = serde_rusqlite::from_rows::<Message>(stmt.query(params![folder])?);
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// FTS5 search, newest first. A query FTS5 cannot parse, such as a bare email address, is retried with every word quoted.
    pub fn search(
        &self,
        query: &str,
        folder: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Message>, StoreError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        self.search_fts(query, folder, limit)
            .or_else(|_| self.search_fts(&quote_words(query), folder, limit))
    }

    fn search_fts(
        &self,
        query: &str,
        folder: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Message>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages
             WHERE id IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?1)
               AND (?2 IS NULL OR folder = ?2)
             ORDER BY internaldate DESC, uid DESC LIMIT ?3"
        ))?;
        let rows = serde_rusqlite::from_rows::<Message>(stmt.query(params![query, folder, limit])?);
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn message(&self, folder: &str, uid: u32) -> Result<Option<Message>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages WHERE folder = ?1 AND uid = ?2"
        ))?;
        let mut rows = stmt.query(params![folder, uid])?;
        match rows.next()? {
            Some(row) => Ok(Some(serde_rusqlite::from_row(row)?)),
            None => Ok(None),
        }
    }

    pub fn raw(&self, folder: &str, uid: u32) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT raw FROM messages WHERE folder = ?1 AND uid = ?2",
                params![folder, uid],
                |r| r.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()?
            .flatten())
    }

    pub fn set_raw(
        &self,
        folder: &str,
        uid: u32,
        raw: &[u8],
        body_text: &str,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE messages SET raw = ?3, body_text = ?4 WHERE folder = ?1 AND uid = ?2",
            params![folder, uid, raw, body_text],
        )?;
        Ok(())
    }

    /// Returns when the rule name was first seen, inserting `now` if it is new.
    pub fn rule_first_seen(&self, name: &str, now: i64) -> Result<i64, StoreError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO rules_seen (name, first_seen_at) VALUES (?1, ?2)",
            params![name, now],
        )?;
        Ok(self.conn.query_row(
            "SELECT first_seen_at FROM rules_seen WHERE name = ?1",
            params![name],
            |r| r.get(0),
        )?)
    }

    /// Starts the rule's clock at `now`, replacing any stale first-seen time.
    pub fn restart_rule_clock(&self, name: &str, now: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO rules_seen (name, first_seen_at) VALUES (?1, ?2)",
            params![name, now],
        )?;
        Ok(())
    }

    /// Drops first-seen times of rules no longer in the file, so a re-added rule starts fresh instead of acting on history.
    pub fn forget_rules_except(&self, names: &[&str]) -> Result<(), StoreError> {
        let names = serde_json::to_string(names).expect("a list of strings serializes");
        self.conn.execute(
            "DELETE FROM rules_seen WHERE name NOT IN (SELECT value FROM json_each(?1))",
            params![names],
        )?;
        Ok(())
    }

    pub fn log_action(&self, entry: &LogEntry) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO rule_log (at, rule_name, folder, uid, message_id, subject, action, trash_file)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![entry.at, entry.rule_name, entry.folder, entry.uid, entry.message_id, entry.subject, entry.action, entry.trash_file],
        )?;
        Ok(())
    }

    pub fn log(&self, limit: u32) -> Result<Vec<LogEntry>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, at, rule_name, folder, uid, message_id, subject, action, trash_file FROM rule_log ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = serde_rusqlite::from_rows::<LogEntry>(stmt.query(params![limit])?);
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn message_count(&self, folder: &str) -> Result<u32, StoreError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE folder = ?1",
            params![folder],
            |r| r.get(0),
        )?)
    }

    pub fn unread_count(&self, folder: &str) -> Result<u32, StoreError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE folder = ?1 AND instr(flags, '\\Seen') = 0",
            params![folder],
            |r| r.get(0),
        )?)
    }
}

/// Every whitespace-separated word as an FTS5 string, which FTS5 always parses.
fn quote_words(query: &str) -> String {
    query
        .split_whitespace()
        .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(folder: &str, uid: u32, internaldate: i64) -> Message {
        Message {
            folder: folder.into(),
            uid,
            message_id: Some(format!("m{uid}@x")),
            from_addr: Some("Alice <alice@x>".into()),
            to_addr: Some("bob@x".into()),
            cc_addr: None,
            delivered_to: None,
            in_reply_to: None,
            refs: None,
            thread_id: format!("m{uid}@x"),
            subject: Some(format!("subject {uid}")),
            date: Some(internaldate),
            internaldate,
            flags: String::new(),
            size: Some(100),
            headers: b"From: alice@x\r\n\r\n".to_vec(),
            body_text: None,
        }
    }

    fn store_with_inbox() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_folder(&Folder {
            name: "INBOX".into(),
            uidvalidity: 1,
            last_uid: 0,
            special_use: None,
        })
        .unwrap();
        s
    }

    #[test]
    fn migrations_are_valid() {
        Store::migrations().validate().unwrap();
    }

    #[test]
    fn open_creates_file_and_applies_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("acc").join("mail.db");
        let s = Store::open(&path).unwrap();
        assert!(path.exists());
        assert!(s.folders().unwrap().is_empty());
    }

    #[test]
    fn folder_upsert_and_reset() {
        let s = store_with_inbox();
        s.insert_message(&msg("INBOX", 1, 10)).unwrap();
        s.upsert_folder(&Folder {
            name: "INBOX".into(),
            uidvalidity: 1,
            last_uid: 1,
            special_use: None,
        })
        .unwrap();
        assert_eq!(s.folder("INBOX").unwrap().unwrap().last_uid, 1);
        s.reset_folder("INBOX", 2).unwrap();
        let f = s.folder("INBOX").unwrap().unwrap();
        assert_eq!((f.uidvalidity, f.last_uid), (2, 0));
        assert!(s.messages_in_folder("INBOX").unwrap().is_empty());
    }

    #[test]
    fn threads_group_by_thread_most_recent_activity_first() {
        let s = store_with_inbox();
        let mut root = msg("INBOX", 1, 10);
        root.thread_id = "root@x".into();
        let mut reply = msg("INBOX", 3, 30);
        reply.thread_id = "root@x".into();
        let other = msg("INBOX", 2, 20);
        for m in [&root, &reply, &other] {
            s.insert_message(m).unwrap();
        }
        let uids: Vec<Vec<u32>> = s
            .threads("INBOX", 10)
            .unwrap()
            .iter()
            .map(|thread| thread.iter().map(|m| m.uid).collect())
            .collect();
        assert_eq!(uids, [vec![1, 3], vec![2]]);
        assert_eq!(s.threads("INBOX", 1).unwrap().len(), 1);
    }

    #[test]
    fn thread_depth_follows_reply_headers() {
        let mut m = msg("INBOX", 1, 10);
        assert_eq!(m.thread_depth(), 0);
        m.in_reply_to = Some("a@x".into());
        assert_eq!(m.thread_depth(), 1);
        m.refs = Some("a@x b@x".into());
        assert_eq!(m.thread_depth(), 2);
    }

    #[test]
    fn messages_round_trip_and_order_newest_first() {
        let s = store_with_inbox();
        s.insert_message(&msg("INBOX", 1, 10)).unwrap();
        s.insert_message(&msg("INBOX", 2, 20)).unwrap();
        let list = s.messages("INBOX", 10).unwrap();
        assert_eq!(list.iter().map(|m| m.uid).collect::<Vec<_>>(), vec![2, 1]);
        assert_eq!(list[1], msg("INBOX", 1, 10));
        assert_eq!(s.message("INBOX", 3).unwrap(), None);
    }

    #[test]
    fn flags_raw_and_removal() {
        let s = store_with_inbox();
        s.insert_message(&msg("INBOX", 1, 10)).unwrap();
        s.update_flags("INBOX", 1, "\\Seen \\Flagged").unwrap();
        assert!(s.message("INBOX", 1).unwrap().unwrap().is_seen());
        assert_eq!(s.raw("INBOX", 1).unwrap(), None);
        s.set_raw("INBOX", 1, b"raw bytes", "body words").unwrap();
        assert_eq!(
            s.raw("INBOX", 1).unwrap().as_deref(),
            Some(&b"raw bytes"[..])
        );
        assert_eq!(
            s.message("INBOX", 1).unwrap().unwrap().body_text.as_deref(),
            Some("body words")
        );
        s.remove_message("INBOX", 1).unwrap();
        assert_eq!(s.message("INBOX", 1).unwrap(), None);
    }

    fn fts_hits(s: &Store, query: &str) -> i64 {
        s.conn
            .query_row(
                "SELECT count(*) FROM messages_fts WHERE messages_fts MATCH ?1",
                params![query],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn fts_integrity_check(s: &Store) {
        s.conn
            .execute(
                "INSERT INTO messages_fts(messages_fts) VALUES ('integrity-check')",
                [],
            )
            .unwrap();
    }

    #[test]
    fn fts_follows_rows_and_ignores_flag_updates() {
        let s = store_with_inbox();
        s.insert_message(&msg("INBOX", 1, 10)).unwrap();
        assert_eq!(fts_hits(&s, "alice"), 1);
        s.set_raw("INBOX", 1, b"raw", "pineapple").unwrap();
        assert_eq!(fts_hits(&s, "pineapple"), 1);
        let before = s.conn.total_changes();
        s.update_flags("INBOX", 1, "").unwrap();
        assert_eq!(
            s.conn.total_changes(),
            before,
            "unchanged flags write nothing"
        );
        s.update_flags("INBOX", 1, "\\Seen").unwrap();
        assert_eq!(
            s.conn.total_changes(),
            before + 1,
            "a flag change does not touch the FTS index"
        );
        s.upsert_folder(&Folder {
            name: "Archive".into(),
            uidvalidity: 1,
            last_uid: 0,
            special_use: None,
        })
        .unwrap();
        s.move_message_row("INBOX", 1, "Archive", Some(9)).unwrap();
        assert_eq!(fts_hits(&s, "pineapple"), 1);
        fts_integrity_check(&s);
        s.remove_message("Archive", 9).unwrap();
        assert_eq!(fts_hits(&s, "pineapple"), 0);
        fts_integrity_check(&s);
    }

    #[test]
    fn insert_ignores_duplicate_folder_uid() {
        let s = store_with_inbox();
        s.insert_message(&msg("INBOX", 1, 10)).unwrap();
        let mut again = msg("INBOX", 1, 99);
        again.subject = Some("other".into());
        s.insert_message(&again).unwrap();
        assert_eq!(
            s.messages_in_folder("INBOX").unwrap(),
            vec![msg("INBOX", 1, 10)]
        );
    }

    #[test]
    fn remove_missing_drops_uids_not_present() {
        let s = store_with_inbox();
        for uid in 1..=4 {
            s.insert_message(&msg("INBOX", uid, uid as i64)).unwrap();
        }
        let removed = s.remove_missing("INBOX", 4, &[1, 3]).unwrap();
        assert_eq!(removed, 2);
        let left: Vec<u32> = s
            .messages_in_folder("INBOX")
            .unwrap()
            .iter()
            .map(|m| m.uid)
            .collect();
        assert_eq!(left, vec![1, 3]);
    }

    #[test]
    fn move_row_with_and_without_new_uid() {
        let s = store_with_inbox();
        s.upsert_folder(&Folder {
            name: "Archive".into(),
            uidvalidity: 1,
            last_uid: 0,
            special_use: Some("Archive".into()),
        })
        .unwrap();
        s.insert_message(&msg("INBOX", 1, 10)).unwrap();
        s.insert_message(&msg("INBOX", 2, 20)).unwrap();
        s.move_message_row("INBOX", 1, "Archive", Some(7)).unwrap();
        assert_eq!(
            s.message("Archive", 7).unwrap().unwrap().subject.as_deref(),
            Some("subject 1")
        );
        s.move_message_row("INBOX", 2, "Archive", None).unwrap();
        assert_eq!(s.message("INBOX", 2).unwrap(), None);
        assert_eq!(s.messages_in_folder("Archive").unwrap().len(), 1);
    }

    #[test]
    fn rule_first_seen_is_sticky() {
        let s = store_with_inbox();
        assert_eq!(s.rule_first_seen("purge", 100).unwrap(), 100);
        assert_eq!(s.rule_first_seen("purge", 200).unwrap(), 100);
        assert_eq!(s.rule_first_seen("other", 200).unwrap(), 200);
    }

    #[test]
    fn restart_rule_clock_overwrites_the_first_seen_time() {
        let s = store_with_inbox();
        assert_eq!(s.rule_first_seen("purge", 100).unwrap(), 100);
        s.restart_rule_clock("purge", 300).unwrap();
        assert_eq!(s.rule_first_seen("purge", 400).unwrap(), 300);
        s.restart_rule_clock("fresh", 500).unwrap();
        assert_eq!(s.rule_first_seen("fresh", 600).unwrap(), 500);
    }

    #[test]
    fn log_round_trip_newest_first() {
        let s = store_with_inbox();
        for i in 1..=3 {
            s.log_action(&LogEntry {
                id: 0,
                at: i,
                rule_name: "r".into(),
                folder: "INBOX".into(),
                uid: i as u32,
                message_id: None,
                subject: None,
                action: "delete".into(),
                trash_file: Some(format!("{i}.eml")),
            })
            .unwrap();
        }
        let log = s.log(2).unwrap();
        assert_eq!(log.iter().map(|e| e.at).collect::<Vec<_>>(), vec![3, 2]);
    }

    #[test]
    fn unread_and_message_counts() {
        let s = store_with_inbox();
        s.insert_message(&msg("INBOX", 1, 10)).unwrap();
        s.insert_message(&msg("INBOX", 2, 20)).unwrap();
        s.update_flags("INBOX", 2, "\\Seen").unwrap();
        assert_eq!(s.unread_count("INBOX").unwrap(), 1);
        assert_eq!(s.message_count("INBOX").unwrap(), 2);
        assert_eq!(s.message_count("Other").unwrap(), 0);
    }

    #[test]
    fn search_matches_words_newest_first_and_filters_by_folder() {
        let s = store_with_inbox();
        s.upsert_folder(&Folder {
            name: "Archive".into(),
            uidvalidity: 1,
            last_uid: 0,
            special_use: None,
        })
        .unwrap();
        s.insert_message(&msg("INBOX", 1, 10)).unwrap();
        s.insert_message(&msg("INBOX", 2, 30)).unwrap();
        s.insert_message(&msg("Archive", 3, 20)).unwrap();
        let uids = |found: Vec<Message>| found.iter().map(|m| m.uid).collect::<Vec<_>>();
        assert_eq!(uids(s.search("subject", None, 10).unwrap()), [2, 3, 1]);
        assert_eq!(
            uids(s.search("subject", Some("INBOX"), 10).unwrap()),
            [2, 1]
        );
        assert_eq!(uids(s.search("subject", None, 1).unwrap()), [2]);
        assert_eq!(
            uids(s.search("from_addr:alice", None, 10).unwrap()),
            [2, 3, 1]
        );
        assert!(s.search("pineapple", None, 10).unwrap().is_empty());
        assert!(s.search("   ", None, 10).unwrap().is_empty());
    }

    #[test]
    fn search_retries_unparsable_queries_as_quoted_words() {
        let s = store_with_inbox();
        s.insert_message(&msg("INBOX", 1, 10)).unwrap();
        assert_eq!(s.search("alice@x", None, 10).unwrap().len(), 1);
        assert!(s.search("re: lunch", None, 10).unwrap().is_empty());
        assert!(s.search("\"unbalanced", None, 10).unwrap().is_empty());
    }

    #[test]
    fn search_covers_fetched_bodies() {
        let s = store_with_inbox();
        s.insert_message(&msg("INBOX", 1, 10)).unwrap();
        assert!(s.search("pineapple", None, 10).unwrap().is_empty());
        s.set_raw("INBOX", 1, b"raw", "pineapple pie").unwrap();
        assert_eq!(s.search("pineapple", None, 10).unwrap().len(), 1);
    }
}
