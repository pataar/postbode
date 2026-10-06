-- One above the highest uid already on the server when the folder was first tracked; lower uids never notify.
ALTER TABLE folders ADD COLUMN initial_uid_next INTEGER NOT NULL DEFAULT 0;
-- The highest uid the rules have evaluated; higher uids are fresh, so they notify and get bodies for body rules.
ALTER TABLE folders ADD COLUMN rules_uid INTEGER NOT NULL DEFAULT 0;
UPDATE folders SET rules_uid = last_uid;
