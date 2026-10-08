//! Help texts shared by the CLI and the MCP tool descriptions.
pub const ARCHIVE: &str = "Move messages to the Archive folder";
pub const ATTACHMENT_LIST: &str = "Index, type, size and name of each attachment";
pub const DELETE: &str =
    "Move messages to Trash; inside Trash, or without one, delete them keeping a local .eml backup";
pub const FOLDERS: &str = "List folders with message and unread counts";
pub const LIST: &str = "List recent messages, newest first";
pub const LOG: &str = "Show what rules did, newest first";
pub const MARK: &str = "Mark messages read or unread, flagged or unflagged";
pub const MCP_DEFAULT_SCOPES: &str = "read,rules:propose";
pub const MOVE: &str = "Move messages to another folder, creating it if needed";
pub const RULES_APPROVE: &str = "Enable a disabled rule, such as a proposal";
pub const RULES_CHECK: &str = "Validate rules.toml";
pub const RULES_LIST: &str = "Names, enabled state and who proposed them";
pub const RULES_PROPOSE: &str = "Add one rule, disabled, for a human to approve; the arguments are the rule, one entry of `rules` in rules_schema";
pub const RULES_REJECT: &str = "Remove a pending proposal";
pub const RULES_SCHEMA: &str = "JSON Schema for rules.toml; a proposal is one entry of `rules`";
pub const RULES_SET_ENABLED: &str =
    "Turn a rule on or off; turning one on lets it act on new mail, deletes included";
pub const RULES_TEST: &str =
    "Dry run: what this rule would do to the cached messages; previews subjects, never bodies";
pub const SEARCH: &str =
    "Full-text search (FTS5 syntax) over subject, addresses and fetched bodies, newest first";
pub const SHOW: &str = "Show one message";
pub const SYNC: &str = "Ask the daemon to sync now and apply rules; reports new messages and actions per account, or why one was refused";
pub const TRASH_LIST: &str = "Deleted mail kept as .eml backups for the retention period";
pub const TRASH_RESTORE: &str = "Append a trashed .eml back into its original folder";
