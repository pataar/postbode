-- When flags were last brought up to date: the folder's HIGHESTMODSEQ then (0 without CONDSTORE) and the Unix time.
ALTER TABLE folders ADD COLUMN highest_modseq INTEGER NOT NULL DEFAULT 0;
ALTER TABLE folders ADD COLUMN flags_synced_at INTEGER NOT NULL DEFAULT 0;
