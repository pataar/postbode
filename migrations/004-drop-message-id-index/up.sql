-- No query looks messages up by Message-ID, so the index only slowed every insert.
DROP INDEX IF EXISTS messages_message_id;
