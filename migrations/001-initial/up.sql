CREATE TABLE folders (
  name         TEXT PRIMARY KEY,
  uidvalidity  INTEGER NOT NULL,
  last_uid     INTEGER NOT NULL DEFAULT 0,
  special_use  TEXT
);

CREATE TABLE messages (
  folder        TEXT NOT NULL REFERENCES folders(name) ON DELETE CASCADE,
  uid           INTEGER NOT NULL,
  message_id    TEXT,
  from_addr     TEXT,
  to_addr       TEXT,
  cc_addr       TEXT,
  delivered_to  TEXT,
  in_reply_to   TEXT,
  refs          TEXT,
  thread_id     TEXT NOT NULL,
  subject       TEXT,
  date          INTEGER,
  internaldate  INTEGER NOT NULL,
  flags         TEXT NOT NULL,
  size          INTEGER,
  headers       BLOB NOT NULL,
  raw           BLOB,
  body_text     TEXT,
  PRIMARY KEY (folder, uid)
);
CREATE INDEX messages_internaldate ON messages (folder, internaldate);
CREATE INDEX messages_thread ON messages (thread_id);
CREATE INDEX messages_message_id ON messages (message_id);

CREATE VIRTUAL TABLE messages_fts USING fts5 (
  subject, from_addr, to_addr, body_text,
  content='messages', content_rowid='rowid'
);
CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN
  INSERT INTO messages_fts(rowid, subject, from_addr, to_addr, body_text)
  VALUES (new.rowid, new.subject, new.from_addr, new.to_addr, new.body_text);
END;
CREATE TRIGGER messages_ad AFTER DELETE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, subject, from_addr, to_addr, body_text)
  VALUES ('delete', old.rowid, old.subject, old.from_addr, old.to_addr, old.body_text);
END;
CREATE TRIGGER messages_au AFTER UPDATE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, subject, from_addr, to_addr, body_text)
  VALUES ('delete', old.rowid, old.subject, old.from_addr, old.to_addr, old.body_text);
  INSERT INTO messages_fts(rowid, subject, from_addr, to_addr, body_text)
  VALUES (new.rowid, new.subject, new.from_addr, new.to_addr, new.body_text);
END;

CREATE TABLE rules_seen (
  name           TEXT PRIMARY KEY,
  first_seen_at  INTEGER NOT NULL
);

CREATE TABLE rule_log (
  id          INTEGER PRIMARY KEY,
  at          INTEGER NOT NULL,
  rule_name   TEXT NOT NULL,
  folder      TEXT NOT NULL,
  uid         INTEGER NOT NULL,
  message_id  TEXT,
  subject     TEXT,
  action      TEXT NOT NULL,
  trash_file  TEXT
);
