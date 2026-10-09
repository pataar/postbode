# Command-Line Help for `postvak`

This document contains the help content for the `postvak` command-line program.

**Command Overview:**

* [`postvak`↴](#postvak)
* [`postvak run`↴](#postvak-run)
* [`postvak daemon`↴](#postvak-daemon)
* [`postvak daemon status`↴](#postvak-daemon-status)
* [`postvak daemon stop`↴](#postvak-daemon-stop)
* [`postvak gui`↴](#postvak-gui)
* [`postvak service`↴](#postvak-service)
* [`postvak service install`↴](#postvak-service-install)
* [`postvak service remove`↴](#postvak-service-remove)
* [`postvak sync`↴](#postvak-sync)
* [`postvak attachment`↴](#postvak-attachment)
* [`postvak attachment list`↴](#postvak-attachment-list)
* [`postvak attachment save`↴](#postvak-attachment-save)
* [`postvak rules`↴](#postvak-rules)
* [`postvak rules check`↴](#postvak-rules-check)
* [`postvak rules test`↴](#postvak-rules-test)
* [`postvak rules schema`↴](#postvak-rules-schema)
* [`postvak rules propose`↴](#postvak-rules-propose)
* [`postvak rules approve`↴](#postvak-rules-approve)
* [`postvak rules reject`↴](#postvak-rules-reject)
* [`postvak rules list`↴](#postvak-rules-list)
* [`postvak rules apply-existing`↴](#postvak-rules-apply-existing)
* [`postvak folders`↴](#postvak-folders)
* [`postvak list`↴](#postvak-list)
* [`postvak search`↴](#postvak-search)
* [`postvak show`↴](#postvak-show)
* [`postvak mark`↴](#postvak-mark)
* [`postvak move`↴](#postvak-move)
* [`postvak archive`↴](#postvak-archive)
* [`postvak delete`↴](#postvak-delete)
* [`postvak log`↴](#postvak-log)
* [`postvak trash`↴](#postvak-trash)
* [`postvak trash list`↴](#postvak-trash-list)
* [`postvak trash restore`↴](#postvak-trash-restore)
* [`postvak trash purge`↴](#postvak-trash-purge)
* [`postvak account`↴](#postvak-account)
* [`postvak account add`↴](#postvak-account-add)
* [`postvak account list`↴](#postvak-account-list)
* [`postvak guide`↴](#postvak-guide)
* [`postvak mcp`↴](#postvak-mcp)
* [`postvak mcp install`↴](#postvak-mcp-install)

## `postvak`

A fast, simple mail client with automatic mailbox rules

**Usage:** `postvak <COMMAND>`

###### **Subcommands:**

* `run` — Run the daemon in the foreground: sync all accounts continuously and apply rules; fails when a daemon already runs; Ctrl-C stops
* `daemon` — Inspect or stop the daemon that syncs your accounts; other commands start it when needed
* `gui` — Open the mail window; starts the daemon when needed
* `service` — Start the daemon at login: a launchd agent on macOS, a systemd user unit on Linux
* `sync` — Ask the daemon to sync now and apply rules; waits for the result
* `attachment` — List or save a message's attachments
* `rules` — Inspect and test rules.toml
* `folders` — List folders with message and unread counts
* `list` — List recent messages, newest first
* `search` — Full-text search (FTS5 syntax) over subject, addresses and fetched bodies, newest first
* `show` — Show one message
* `mark` — Mark messages read or unread, flagged or unflagged
* `move` — Move messages to another folder, creating it if needed
* `archive` — Move messages to the Archive folder
* `delete` — Move messages to Trash; inside Trash, or without one, delete them keeping a local .eml backup
* `log` — Show what rules did, newest first
* `trash` — Deleted mail kept for the retention period
* `account` — Manage accounts
* `guide` — Print the agent guide: how an LLM should drive Postvak
* `mcp` — Serve Postvak to an agent host over MCP on stdio; hosts start this, see `postvak mcp install`



## `postvak run`

Run the daemon in the foreground: sync all accounts continuously and apply rules; fails when a daemon already runs; Ctrl-C stops

**Usage:** `postvak run`



## `postvak daemon`

Inspect or stop the daemon that syncs your accounts; other commands start it when needed

**Usage:** `postvak daemon <COMMAND>`

###### **Subcommands:**

* `status` — Print the daemon's pid, version, uptime and what each account is doing
* `stop` — Stop the daemon; it starts again when a command needs it



## `postvak daemon status`

Print the daemon's pid, version, uptime and what each account is doing

**Usage:** `postvak daemon status`



## `postvak daemon stop`

Stop the daemon; it starts again when a command needs it

**Usage:** `postvak daemon stop`



## `postvak gui`

Open the mail window; starts the daemon when needed

**Usage:** `postvak gui`



## `postvak service`

Start the daemon at login: a launchd agent on macOS, a systemd user unit on Linux

**Usage:** `postvak service <COMMAND>`

###### **Subcommands:**

* `install` — Install and start the service; stops a daemon that is already running so the service's takes over
* `remove` — Stop and remove the service



## `postvak service install`

Install and start the service; stops a daemon that is already running so the service's takes over

**Usage:** `postvak service install [OPTIONS]`

###### **Options:**

* `--dry-run` — Print the file and the commands and change nothing



## `postvak service remove`

Stop and remove the service

**Usage:** `postvak service remove [OPTIONS]`

###### **Options:**

* `--dry-run` — Print the commands and change nothing



## `postvak sync`

Ask the daemon to sync now and apply rules; waits for the result

**Usage:** `postvak sync [OPTIONS]`

###### **Options:**

* `--account <ACCOUNT>`



## `postvak attachment`

List or save a message's attachments

**Usage:** `postvak attachment <COMMAND>`

###### **Subcommands:**

* `list` — Index, type, size and name of each attachment
* `save` — Save attachment N, as numbered by `attachment list`, into --dir



## `postvak attachment list`

Index, type, size and name of each attachment

**Usage:** `postvak attachment list [OPTIONS] <UID>`

###### **Arguments:**

* `<UID>`

###### **Options:**

* `--account <ACCOUNT>`
* `--folder <FOLDER>`

  Default value: `INBOX`
* `--json`



## `postvak attachment save`

Save attachment N, as numbered by `attachment list`, into --dir

**Usage:** `postvak attachment save [OPTIONS] <UID> <N>`

###### **Arguments:**

* `<UID>`
* `<N>`

###### **Options:**

* `--dir <DIR>`

  Default value: `.`
* `--account <ACCOUNT>`
* `--folder <FOLDER>`

  Default value: `INBOX`



## `postvak rules`

Inspect and test rules.toml

**Usage:** `postvak rules <COMMAND>`

###### **Subcommands:**

* `check` — Validate rules.toml
* `test` — Dry run: print what each rule would do to the cached messages; naming a rule previews it even while disabled
* `schema` — JSON Schema for rules.toml; a proposal is one entry of `rules`
* `propose` — Read one rule as JSON on stdin and add it disabled, for a human to approve
* `approve` — Enable a disabled rule, such as a proposal
* `reject` — Remove a pending proposal
* `list` — Names, enabled state and who proposed them
* `apply-existing` — Run one rule against mail that predates it



## `postvak rules check`

Validate rules.toml

**Usage:** `postvak rules check`



## `postvak rules test`

Dry run: print what each rule would do to the cached messages; naming a rule previews it even while disabled

**Usage:** `postvak rules test [OPTIONS] [NAME]`

###### **Arguments:**

* `<NAME>`

###### **Options:**

* `--account <ACCOUNT>`
* `--stdin` — Preview one rule read as JSON from stdin instead of rules.toml



## `postvak rules schema`

JSON Schema for rules.toml; a proposal is one entry of `rules`

**Usage:** `postvak rules schema`



## `postvak rules propose`

Read one rule as JSON on stdin and add it disabled, for a human to approve

**Usage:** `postvak rules propose [OPTIONS]`

###### **Options:**

* `--by <BY>` — Who proposes it, recorded as proposed_by = "cli:WHO"



## `postvak rules approve`

Enable a disabled rule, such as a proposal

**Usage:** `postvak rules approve <NAME>`

###### **Arguments:**

* `<NAME>`



## `postvak rules reject`

Remove a pending proposal

**Usage:** `postvak rules reject <NAME>`

###### **Arguments:**

* `<NAME>`



## `postvak rules list`

Names, enabled state and who proposed them

**Usage:** `postvak rules list [OPTIONS]`

###### **Options:**

* `--json`



## `postvak rules apply-existing`

Run one rule against mail that predates it

**Usage:** `postvak rules apply-existing [OPTIONS] <NAME>`

###### **Arguments:**

* `<NAME>`

###### **Options:**

* `--account <ACCOUNT>`
* `--dry-run`



## `postvak folders`

List folders with message and unread counts

**Usage:** `postvak folders [OPTIONS]`

###### **Options:**

* `--account <ACCOUNT>`
* `--json`



## `postvak list`

List recent messages, newest first

**Usage:** `postvak list [OPTIONS]`

###### **Options:**

* `--account <ACCOUNT>`
* `--folder <FOLDER>`

  Default value: `INBOX`
* `--limit <LIMIT>`

  Default value: `50`
* `--json`
* `--threads` — Group by conversation, the most recently active thread first



## `postvak search`

Full-text search (FTS5 syntax) over subject, addresses and fetched bodies, newest first

**Usage:** `postvak search [OPTIONS] <QUERY>`

###### **Arguments:**

* `<QUERY>`

###### **Options:**

* `--account <ACCOUNT>`
* `--folder <FOLDER>`
* `--bodies` — Fetch and index missing bodies first; slow on a large folder
* `--limit <LIMIT>`

  Default value: `50`
* `--json`



## `postvak show`

Show one message

**Usage:** `postvak show [OPTIONS] <UID>`

###### **Arguments:**

* `<UID>`

###### **Options:**

* `--account <ACCOUNT>`
* `--folder <FOLDER>`

  Default value: `INBOX`
* `--raw` — Print the raw RFC 5322 message instead of the text body
* `--json`



## `postvak mark`

Mark messages read or unread, flagged or unflagged

**Usage:** `postvak mark [OPTIONS] <HOW> <UIDS>...`

###### **Arguments:**

* `<HOW>`

  Possible values: `flag`, `read`, `unflag`, `unread`

* `<UIDS>` — Message uids in --folder, as `list` prints them

###### **Options:**

* `--account <ACCOUNT>`
* `--folder <FOLDER>`

  Default value: `INBOX`
* `--dry-run` — Print what would happen without touching the server



## `postvak move`

Move messages to another folder, creating it if needed

**Usage:** `postvak move [OPTIONS] --to <TO> <UIDS>...`

###### **Arguments:**

* `<UIDS>` — Message uids in --folder, as `list` prints them

###### **Options:**

* `--to <TO>`
* `--account <ACCOUNT>`
* `--folder <FOLDER>`

  Default value: `INBOX`
* `--dry-run` — Print what would happen without touching the server



## `postvak archive`

Move messages to the Archive folder

**Usage:** `postvak archive [OPTIONS] <UIDS>...`

###### **Arguments:**

* `<UIDS>` — Message uids in --folder, as `list` prints them

###### **Options:**

* `--account <ACCOUNT>`
* `--folder <FOLDER>`

  Default value: `INBOX`
* `--dry-run` — Print what would happen without touching the server



## `postvak delete`

Move messages to Trash; inside Trash, or without one, delete them keeping a local .eml backup

**Usage:** `postvak delete [OPTIONS] <UIDS>...`

###### **Arguments:**

* `<UIDS>` — Message uids in --folder, as `list` prints them

###### **Options:**

* `--account <ACCOUNT>`
* `--folder <FOLDER>`

  Default value: `INBOX`
* `--dry-run` — Print what would happen without touching the server



## `postvak log`

Show what rules did, newest first

**Usage:** `postvak log [OPTIONS]`

###### **Options:**

* `--account <ACCOUNT>`
* `--limit <LIMIT>`

  Default value: `50`
* `--json`



## `postvak trash`

Deleted mail kept for the retention period

**Usage:** `postvak trash <COMMAND>`

###### **Subcommands:**

* `list` — 
* `restore` — Append a trashed .eml back into its original folder
* `purge` — Remove trash files older than the retention period



## `postvak trash list`

**Usage:** `postvak trash list [OPTIONS]`

###### **Options:**

* `--account <ACCOUNT>`



## `postvak trash restore`

Append a trashed .eml back into its original folder

**Usage:** `postvak trash restore [OPTIONS] <FILE>`

###### **Arguments:**

* `<FILE>`

###### **Options:**

* `--account <ACCOUNT>`



## `postvak trash purge`

Remove trash files older than the retention period

**Usage:** `postvak trash purge [OPTIONS]`

###### **Options:**

* `--account <ACCOUNT>`



## `postvak account`

Manage accounts

**Usage:** `postvak account <COMMAND>`

###### **Subcommands:**

* `add` — Interactively add an IMAP account and test the login
* `list` — Print each configured account: name, username and server



## `postvak account add`

Interactively add an IMAP account and test the login

**Usage:** `postvak account add`



## `postvak account list`

Print each configured account: name, username and server

**Usage:** `postvak account list [OPTIONS]`

###### **Options:**

* `--json`



## `postvak guide`

Print the agent guide: how an LLM should drive Postvak

**Usage:** `postvak guide`



## `postvak mcp`

Serve Postvak to an agent host over MCP on stdio; hosts start this, see `postvak mcp install`

**Usage:** `postvak mcp [OPTIONS]
       mcp <COMMAND>`

###### **Subcommands:**

* `install` — Register `postvak mcp` with an agent host; re-run it to change the scopes

###### **Options:**

* `--scopes <SCOPES>` — Comma-separated: read, read:bodies, rules:propose, rules:write, mail:modify

  Default value: `read,rules:propose`
* `--account <ACCOUNT>` — Only this account; repeatable; default every account



## `postvak mcp install`

Register `postvak mcp` with an agent host; re-run it to change the scopes

**Usage:** `postvak mcp install [OPTIONS] <TARGET>`

###### **Arguments:**

* `<TARGET>`

  Possible values: `claude-code`, `claude-desktop`, `json`


###### **Options:**

* `--scopes <SCOPES>` — Comma-separated: read, read:bodies, rules:propose, rules:write, mail:modify

  Default value: `read,rules:propose`
* `--account <ACCOUNT>` — Only this account; repeatable; default every account
* `--remove` — Take the entry out again
* `--dry-run` — Print the change and write nothing



<hr/>

<small><i>
    This document was generated automatically by
    <a href="https://crates.io/crates/clap-markdown"><code>clap-markdown</code></a>.
</i></small>
