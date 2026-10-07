-- The highest uid new-mail notifications have covered; it can pass rules_uid while no valid rules run.
ALTER TABLE folders ADD COLUMN notified_uid INTEGER NOT NULL DEFAULT 0;
UPDATE folders SET notified_uid = rules_uid;
