-- One above the highest uid already on the server when the folder was first tracked; lower uids never notify.
ALTER TABLE folders ADD COLUMN initial_uid_next INTEGER NOT NULL DEFAULT 0;
