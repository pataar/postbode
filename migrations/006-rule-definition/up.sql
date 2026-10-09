-- The rule's match and actions as JSON; a change restarts its clock. NULL until the rule is next loaded, which adopts it.
ALTER TABLE rules_seen ADD COLUMN definition TEXT;
