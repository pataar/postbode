-- The IMAP hierarchy delimiter, so the window can show folders as a tree. NULL until the next full sync lists it.
ALTER TABLE folders ADD COLUMN delimiter TEXT;
