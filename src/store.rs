use std::io;
use std::path::Path;

use include_dir::{Dir, include_dir};
use rusqlite::{Connection, OptionalExtension, params};
use rusqlite_migration::Migrations;
use serde::{Deserialize, Serialize};

use crate::paths::Paths;

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
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub name: String,
    pub uidvalidity: u32,
    pub last_uid: u32,
    pub special_use: Option<String>,
    #[serde(default)]
    pub delimiter: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
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

/// Whether the space-separated `flags` contain `flag`.
pub(crate) fn has_flag(flags: &str, flag: &str) -> bool {
    flags.split_whitespace().any(|f| f == flag)
}

impl Message {
    pub fn is_seen(&self) -> bool {
        has_flag(&self.flags, "\\Seen")
    }

    pub fn is_flagged(&self) -> bool {
        has_flag(&self.flags, "\\Flagged")
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

/// What a list row shows; no headers or body.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageSummary {
    pub uid: u32,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub date: i64,
    pub flags: String,
}

impl MessageSummary {
    pub fn is_seen(&self) -> bool {
        has_flag(&self.flags, "\\Seen")
    }

    pub fn is_flagged(&self) -> bool {
        has_flag(&self.flags, "\\Flagged")
    }
}

/// A collapsed thread row: its latest message plus counts over all members in the folder.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadSummary {
    pub thread_id: String,
    pub latest: MessageSummary,
    pub count: u32,
    pub unread: bool,
    pub flagged: bool,
}

fn summary_from_row(row: &rusqlite::Row, offset: usize) -> rusqlite::Result<MessageSummary> {
    Ok(MessageSummary {
        uid: row.get(offset)?,
        from: row.get(offset + 1)?,
        to: row.get(offset + 2)?,
        subject: row.get(offset + 3)?,
        date: row.get(offset + 4)?,
        flags: row.get(offset + 5)?,
    })
}

const SUMMARY_COLUMNS: &str = "uid, COALESCE(from_addr, ''), COALESCE(to_addr, ''), COALESCE(subject, ''), internaldate, flags";

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
    pub fn migrations() -> Result<Migrations<'static>, StoreError> {
        Ok(Migrations::from_directory(&MIGRATIONS_DIR)?)
    }

    pub fn open(path: &Path) -> Result<Store, StoreError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        Store::init(conn)
    }

    /// The account's store, in its private directory.
    pub fn open_account(paths: &Paths, account: &str) -> Result<Store, StoreError> {
        paths.ensure_account(account)?;
        Store::open(&paths.mail_db(account))
    }

    pub fn open_in_memory() -> Result<Store, StoreError> {
        Store::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Store, StoreError> {
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // Clients migrate and write rule clocks while the daemon may hold the write lock.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        Store::migrations()?.to_latest(&mut conn)?;
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
            "SELECT name, uidvalidity, last_uid, special_use, delimiter FROM folders ORDER BY name",
        )?;
        let rows = serde_rusqlite::from_rows::<Folder>(stmt.query([])?);
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn folder(&self, name: &str) -> Result<Option<Folder>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT name, uidvalidity, last_uid, special_use, delimiter FROM folders WHERE name = ?1",
        )?;
        let mut rows = stmt.query(params![name])?;
        match rows.next()? {
            Some(row) => Ok(Some(serde_rusqlite::from_row(row)?)),
            None => Ok(None),
        }
    }

    pub fn upsert_folder(&self, folder: &Folder) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO folders (name, uidvalidity, last_uid, special_use, delimiter) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(name) DO UPDATE SET uidvalidity = excluded.uidvalidity, last_uid = excluded.last_uid,
             special_use = excluded.special_use, delimiter = COALESCE(excluded.delimiter, folders.delimiter)",
            params![folder.name, folder.uidvalidity, folder.last_uid, folder.special_use, folder.delimiter],
        )?;
        Ok(())
    }

    /// One above the highest uid already on the server when `folder` was first tracked; 0 when unknown.
    pub fn initial_uid_next(&self, folder: &str) -> Result<u32, StoreError> {
        self.folder_uid(folder, "initial_uid_next")
    }

    pub fn set_initial_uid_next(&self, folder: &str, uid_next: u32) -> Result<(), StoreError> {
        self.set_folder_uid(folder, "initial_uid_next", uid_next)
    }

    /// The highest uid the rules have evaluated in `folder`; above it mail is fresh. 0 when unknown.
    pub fn rules_uid(&self, folder: &str) -> Result<u32, StoreError> {
        self.folder_uid(folder, "rules_uid")
    }

    pub fn set_rules_uid(&self, folder: &str, uid: u32) -> Result<(), StoreError> {
        self.set_folder_uid(folder, "rules_uid", uid)
    }

    /// The highest uid new-mail notifications have covered in `folder`; 0 when unknown.
    pub fn notified_uid(&self, folder: &str) -> Result<u32, StoreError> {
        self.folder_uid(folder, "notified_uid")
    }

    pub fn set_notified_uid(&self, folder: &str, uid: u32) -> Result<(), StoreError> {
        self.set_folder_uid(folder, "notified_uid", uid)
    }

    /// A uid column of the `folders` row; 0 when the folder is untracked.
    fn folder_uid(&self, folder: &str, column: &'static str) -> Result<u32, StoreError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {column} FROM folders WHERE name = ?1"),
                params![folder],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    fn set_folder_uid(
        &self,
        folder: &str,
        column: &'static str,
        uid: u32,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            &format!("UPDATE folders SET {column} = ?2 WHERE name = ?1"),
            params![folder, uid],
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

    /// One row per thread in `folder`, the most recently active first. With exactly one MAX() in the query, SQLite fills
    /// the bare columns from the row holding it: the thread's latest message. A second MAX() anywhere breaks that.
    pub fn thread_summaries(
        &self,
        folder: &str,
        limit: u32,
    ) -> Result<Vec<ThreadSummary>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            r"SELECT thread_id, {SUMMARY_COLUMNS}, MAX(internaldate) AS latest, COUNT(*),
                     SUM(instr(' ' || flags || ' ', ' \Seen ') = 0), SUM(instr(' ' || flags || ' ', ' \Flagged ') > 0)
              FROM messages WHERE folder = ?1 GROUP BY thread_id
              ORDER BY latest DESC, uid DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![folder, limit], |r| {
            Ok(ThreadSummary {
                thread_id: r.get(0)?,
                latest: summary_from_row(r, 1)?,
                count: r.get(8)?,
                unread: r.get::<_, i64>(9)? > 0,
                flagged: r.get::<_, i64>(10)? > 0,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn thread_members(
        &self,
        folder: &str,
        thread_id: &str,
    ) -> Result<Vec<MessageSummary>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SUMMARY_COLUMNS} FROM messages WHERE folder = ?1 AND thread_id = ?2 ORDER BY internaldate, uid"
        ))?;
        let rows = stmt.query_map(params![folder, thread_id], |r| summary_from_row(r, 0))?;
        Ok(rows.collect::<Result<_, _>>()?)
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

    /// Search over subject and addresses only; every word is quoted, because FTS5 syntax could escape the column filter.
    pub fn search_headers(
        &self,
        query: &str,
        folder: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Message>, StoreError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let scoped = format!("{{subject from_addr to_addr}} : ({})", quote_words(query));
        self.search_fts(&scoped, folder, limit)
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

    /// Returns when the rule was first seen with this definition, starting its clock at `now` if it is new or changed.
    pub fn rule_first_seen(
        &self,
        name: &str,
        definition: &str,
        now: i64,
    ) -> Result<i64, StoreError> {
        self.conn.execute(
            "INSERT INTO rules_seen (name, first_seen_at, definition) VALUES (?1, ?2, ?3)
             ON CONFLICT (name) DO UPDATE SET
               first_seen_at = CASE WHEN definition IS NULL OR definition = excluded.definition
                                    THEN first_seen_at ELSE excluded.first_seen_at END,
               definition = excluded.definition",
            params![name, now, definition],
        )?;
        Ok(self.conn.query_row(
            "SELECT first_seen_at FROM rules_seen WHERE name = ?1",
            params![name],
            |r| r.get(0),
        )?)
    }

    /// Starts the rule's clock at `now`, replacing any stale first-seen time; the next load adopts its definition.
    pub fn restart_rule_clock(&self, name: &str, now: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO rules_seen (name, first_seen_at) VALUES (?1, ?2)",
            params![name, now],
        )?;
        Ok(())
    }

    /// Moves `rules_uid` up to `last_uid` in every folder not in `except`, so their mail stops counting as fresh.
    pub fn mark_rules_seen_except(&self, except: &[String]) -> Result<(), StoreError> {
        let except = serde_json::to_string(except)?;
        self.conn.execute(
            "UPDATE folders SET rules_uid = last_uid
             WHERE last_uid > rules_uid AND name NOT IN (SELECT value FROM json_each(?1))",
            params![except],
        )?;
        Ok(())
    }

    /// Drops first-seen times of rules no longer in the file, so a re-added rule starts fresh instead of acting on history.
    pub fn forget_rules_except(&self, names: &[&str]) -> Result<(), StoreError> {
        let names = serde_json::to_string(names)?;
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

    #[test]
    fn a_folder_keeps_its_delimiter_through_an_upsert_without_one() {
        let store = Store::open_in_memory().unwrap();
        let mut folder = Folder {
            name: "Projects/Postbode".into(),
            uidvalidity: 1,
            last_uid: 0,
            special_use: None,
            delimiter: Some("/".into()),
        };
        store.upsert_folder(&folder).unwrap();
        folder.delimiter = None;
        folder.last_uid = 5;
        store.upsert_folder(&folder).unwrap();
        let stored = store.folder("Projects/Postbode").unwrap().unwrap();
        assert_eq!(stored.delimiter.as_deref(), Some("/"));
        assert_eq!(stored.last_uid, 5);
        assert_eq!(store.folders().unwrap()[0].delimiter.as_deref(), Some("/"));
    }

    #[test]
    fn open_account_makes_the_account_directory_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        Store::open_account(&paths, "work").unwrap();
        let meta = std::fs::metadata(paths.account_dir("work")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    }

    fn msg(folder: &str, uid: u32, internaldate: i64) -> Message {
        Message {
            folder: folder.into(),
            uid,
            message_id: Some(format!("m{uid}@x")),
            from_addr: Some("Alice <alice@x>".into()),
            to_addr: Some("bob@x".into()),
            thread_id: format!("m{uid}@x"),
            subject: Some(format!("subject {uid}")),
            date: Some(internaldate),
            internaldate,
            size: Some(100),
            headers: b"From: alice@x\r\n\r\n".to_vec(),
            ..Default::default()
        }
    }

    fn store_with_inbox() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_folder(&Folder {
            name: "INBOX".into(),
            uidvalidity: 1,
            last_uid: 0,
            special_use: None,
            delimiter: None,
        })
        .unwrap();
        s
    }

    #[test]
    fn migrations_are_valid() {
        Store::migrations().unwrap().validate().unwrap();
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
            delimiter: None,
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
        s.remove_message("INBOX", 1).unwrap();
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
    fn rule_first_seen_is_sticky_while_the_definition_is_unchanged() {
        let s = store_with_inbox();
        assert_eq!(s.rule_first_seen("purge", "a", 100).unwrap(), 100);
        assert_eq!(s.rule_first_seen("purge", "a", 200).unwrap(), 100);
        assert_eq!(s.rule_first_seen("other", "a", 200).unwrap(), 200);
    }

    #[test]
    fn rule_first_seen_restarts_when_the_definition_changes() {
        let s = store_with_inbox();
        assert_eq!(s.rule_first_seen("purge", "a", 100).unwrap(), 100);
        assert_eq!(s.rule_first_seen("purge", "b", 200).unwrap(), 200);
        assert_eq!(s.rule_first_seen("purge", "b", 300).unwrap(), 200);
    }

    #[test]
    fn restart_rule_clock_overwrites_the_first_seen_time() {
        let s = store_with_inbox();
        assert_eq!(s.rule_first_seen("purge", "a", 100).unwrap(), 100);
        s.restart_rule_clock("purge", 300).unwrap();
        assert_eq!(s.rule_first_seen("purge", "a", 400).unwrap(), 300);
        s.restart_rule_clock("fresh", 500).unwrap();
        assert_eq!(s.rule_first_seen("fresh", "a", 600).unwrap(), 500);
    }

    #[test]
    fn migration_006_keeps_rule_clocks_and_adopts_the_next_definition() {
        let mut conn = Connection::open_in_memory().unwrap();
        Store::migrations()
            .unwrap()
            .to_version(&mut conn, 5)
            .unwrap();
        conn.execute(
            "INSERT INTO rules_seen (name, first_seen_at) VALUES ('purge', 100)",
            [],
        )
        .unwrap();
        let s = Store::init(conn).unwrap();
        assert_eq!(s.rule_first_seen("purge", "a", 200).unwrap(), 100);
        assert_eq!(s.rule_first_seen("purge", "b", 300).unwrap(), 300);
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
            delimiter: None,
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
    fn header_search_ignores_bodies_even_through_fts_syntax() {
        let store = store_with_inbox();
        let mut m = msg("INBOX", 1, 100);
        m.subject = Some("hello".into());
        m.body_text = Some("secret word".into());
        store.insert_message(&m).unwrap();
        assert_eq!(store.search_headers("hello", None, 10).unwrap().len(), 1);
        assert!(store.search_headers("secret", None, 10).unwrap().is_empty());
        assert!(
            store
                .search_headers("x) OR (body_text:secret", None, 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .search_headers("x) OR body_text:secret", None, 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(store.search("secret", None, 10).unwrap().len(), 1);
    }

    #[test]
    fn search_covers_fetched_bodies() {
        let s = store_with_inbox();
        s.insert_message(&msg("INBOX", 1, 10)).unwrap();
        assert!(s.search("pineapple", None, 10).unwrap().is_empty());
        s.set_raw("INBOX", 1, b"raw", "pineapple pie").unwrap();
        assert_eq!(s.search("pineapple", None, 10).unwrap().len(), 1);
    }

    #[test]
    fn migration_002_keeps_rows_and_marks_existing_mail_as_processed() {
        let mut conn = Connection::open_in_memory().unwrap();
        Store::migrations()
            .unwrap()
            .to_version(&mut conn, 1)
            .unwrap();
        conn.execute(
            "INSERT INTO folders (name, uidvalidity, last_uid) VALUES ('INBOX', 7, 42)",
            [],
        )
        .unwrap();
        let s = Store::init(conn).unwrap();
        let inbox = s.folder("INBOX").unwrap().unwrap();
        assert_eq!((inbox.uidvalidity, inbox.last_uid), (7, 42));
        assert_eq!(s.initial_uid_next("INBOX").unwrap(), 0);
        assert_eq!(s.rules_uid("INBOX").unwrap(), 42);
    }

    #[test]
    fn migration_003_counts_mail_the_rules_saw_as_notified() {
        let mut conn = Connection::open_in_memory().unwrap();
        Store::migrations()
            .unwrap()
            .to_version(&mut conn, 2)
            .unwrap();
        conn.execute(
            "INSERT INTO folders (name, uidvalidity, last_uid, rules_uid) VALUES ('INBOX', 7, 42, 40)",
            [],
        )
        .unwrap();
        let s = Store::init(conn).unwrap();
        assert_eq!(s.rules_uid("INBOX").unwrap(), 40);
        assert_eq!(s.notified_uid("INBOX").unwrap(), 40);
        s.set_notified_uid("INBOX", 42).unwrap();
        assert_eq!(s.notified_uid("INBOX").unwrap(), 42);
    }

    #[test]
    fn migration_004_drops_the_message_id_index() {
        let s = store_with_inbox();
        let indexes: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'messages_message_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(indexes, 0);
    }

    #[test]
    fn rules_uid_round_trips_and_survives_upsert() {
        let s = store_with_inbox();
        assert_eq!(s.rules_uid("INBOX").unwrap(), 0);
        assert_eq!(s.rules_uid("Nope").unwrap(), 0);
        s.set_rules_uid("INBOX", 77).unwrap();
        s.upsert_folder(&Folder {
            name: "INBOX".into(),
            uidvalidity: 1,
            last_uid: 500,
            special_use: None,
            delimiter: None,
        })
        .unwrap();
        assert_eq!(s.rules_uid("INBOX").unwrap(), 77);
    }

    #[test]
    fn mark_rules_seen_except_skips_listed_folders_and_never_lowers() {
        let s = store_with_inbox();
        for (name, last_uid) in [("INBOX", 50), ("Lists", 30), ("Old", 10)] {
            s.upsert_folder(&Folder {
                name: name.into(),
                uidvalidity: 1,
                last_uid,
                special_use: None,
                delimiter: None,
            })
            .unwrap();
        }
        s.set_rules_uid("Old", 20).unwrap();
        s.mark_rules_seen_except(&["INBOX".to_string()]).unwrap();
        assert_eq!(s.rules_uid("INBOX").unwrap(), 0);
        assert_eq!(s.rules_uid("Lists").unwrap(), 30);
        assert_eq!(s.rules_uid("Old").unwrap(), 20);
    }

    #[test]
    fn initial_uid_next_round_trips_and_survives_upsert() {
        let s = store_with_inbox();
        assert_eq!(s.initial_uid_next("INBOX").unwrap(), 0);
        assert_eq!(s.initial_uid_next("Nope").unwrap(), 0);
        s.set_initial_uid_next("INBOX", 1201).unwrap();
        s.upsert_folder(&Folder {
            name: "INBOX".into(),
            uidvalidity: 1,
            last_uid: 500,
            special_use: None,
            delimiter: None,
        })
        .unwrap();
        assert_eq!(s.initial_uid_next("INBOX").unwrap(), 1201);
    }

    #[test]
    fn thread_summaries_describe_the_latest_message() {
        let s = store_with_inbox();
        let mut first = msg("INBOX", 1, 10);
        first.thread_id = "t1".into();
        first.flags = "\\Seen".into();
        let mut reply = msg("INBOX", 2, 30);
        reply.thread_id = "t1".into();
        reply.flags = "\\Flagged".into();
        let mut other = msg("INBOX", 3, 20);
        other.thread_id = "t2".into();
        other.flags = "\\Seen".into();
        for m in [&first, &reply, &other] {
            s.insert_message(m).unwrap();
        }
        let threads = s.thread_summaries("INBOX", 10).unwrap();
        assert_eq!(
            threads,
            vec![
                ThreadSummary {
                    thread_id: "t1".into(),
                    latest: MessageSummary {
                        uid: 2,
                        from: "Alice <alice@x>".into(),
                        to: "bob@x".into(),
                        subject: "subject 2".into(),
                        date: 30,
                        flags: "\\Flagged".into(),
                    },
                    count: 2,
                    unread: true,
                    flagged: true,
                },
                ThreadSummary {
                    thread_id: "t2".into(),
                    latest: MessageSummary {
                        uid: 3,
                        from: "Alice <alice@x>".into(),
                        to: "bob@x".into(),
                        subject: "subject 3".into(),
                        date: 20,
                        flags: "\\Seen".into(),
                    },
                    count: 1,
                    unread: false,
                    flagged: false,
                },
            ]
        );
        assert_eq!(s.thread_summaries("INBOX", 1).unwrap().len(), 1);
        let members: Vec<u32> = s
            .thread_members("INBOX", "t1")
            .unwrap()
            .iter()
            .map(|m| m.uid)
            .collect();
        assert_eq!(members, [1, 2]);
    }

    #[test]
    fn summaries_tolerate_missing_headers() {
        let s = store_with_inbox();
        let mut bare = msg("INBOX", 1, 10);
        bare.from_addr = None;
        bare.to_addr = None;
        bare.subject = None;
        s.insert_message(&bare).unwrap();
        let latest = &s.thread_summaries("INBOX", 10).unwrap()[0].latest;
        assert_eq!((latest.from.as_str(), latest.subject.as_str()), ("", ""));
        assert!(!latest.is_seen() && !latest.is_flagged());
    }
}
