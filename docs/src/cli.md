# Command-Line Help for `postbode`

This document contains the help content for the `postbode` command-line program.

**Command Overview:**

* [`postbode`↴](#postbode)
* [`postbode run`↴](#postbode-run)
* [`postbode daemon`↴](#postbode-daemon)
* [`postbode daemon status`↴](#postbode-daemon-status)
* [`postbode daemon stop`↴](#postbode-daemon-stop)
* [`postbode gui`↴](#postbode-gui)
* [`postbode service`↴](#postbode-service)
* [`postbode service install`↴](#postbode-service-install)
* [`postbode service remove`↴](#postbode-service-remove)
* [`postbode sync`↴](#postbode-sync)
* [`postbode attachment`↴](#postbode-attachment)
* [`postbode attachment list`↴](#postbode-attachment-list)
* [`postbode attachment save`↴](#postbode-attachment-save)
* [`postbode rules`↴](#postbode-rules)
* [`postbode rules check`↴](#postbode-rules-check)
* [`postbode rules test`↴](#postbode-rules-test)
* [`postbode rules schema`↴](#postbode-rules-schema)
* [`postbode rules propose`↴](#postbode-rules-propose)
* [`postbode rules approve`↴](#postbode-rules-approve)
* [`postbode rules reject`↴](#postbode-rules-reject)
* [`postbode rules list`↴](#postbode-rules-list)
* [`postbode rules apply-existing`↴](#postbode-rules-apply-existing)
* [`postbode folders`↴](#postbode-folders)
* [`postbode list`↴](#postbode-list)
* [`postbode search`↴](#postbode-search)
* [`postbode show`↴](#postbode-show)
* [`postbode mark`↴](#postbode-mark)
* [`postbode move`↴](#postbode-move)
* [`postbode archive`↴](#postbode-archive)
* [`postbode delete`↴](#postbode-delete)
* [`postbode log`↴](#postbode-log)
* [`postbode trash`↴](#postbode-trash)
* [`postbode trash list`↴](#postbode-trash-list)
* [`postbode trash restore`↴](#postbode-trash-restore)
* [`postbode trash purge`↴](#postbode-trash-purge)
* [`postbode account`↴](#postbode-account)
* [`postbode account add`↴](#postbode-account-add)
* [`postbode guide`↴](#postbode-guide)
* [`postbode mcp`↴](#postbode-mcp)
* [`postbode mcp install`↴](#postbode-mcp-install)

## `postbode`

A fast, simple mail client with automatic mailbox rules

**Usage:** `postbode <COMMAND>`

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
* `guide` — Print the agent guide: how an LLM should drive Postbode
* `mcp` — Serve Postbode to an agent host over MCP on stdio; hosts start this, see `postbode mcp install`



## `postbode run`

Run the daemon in the foreground: sync all accounts continuously and apply rules; fails when a daemon already runs; Ctrl-C stops

**Usage:** `postbode run`



## `postbode daemon`

Inspect or stop the daemon that syncs your accounts; other commands start it when needed

**Usage:** `postbode daemon <COMMAND>`

###### **Subcommands:**

* `status` — Print the daemon's pid, version, uptime and what each account is doing
* `stop` — Stop the daemon; it starts again when a command needs it



## `postbode daemon status`

Print the daemon's pid, version, uptime and what each account is doing

**Usage:** `postbode daemon status`



## `postbode daemon stop`

Stop the daemon; it starts again when a command needs it

**Usage:** `postbode daemon stop`



## `postbode gui`

Open the mail window; starts the daemon when needed

**Usage:** `postbode gui`



## `postbode service`

Start the daemon at login: a launchd agent on macOS, a systemd user unit on Linux

**Usage:** `postbode service <COMMAND>`

###### **Subcommands:**

* `install` — Install and start the service; stops a daemon that is already running so the service's takes over
* `remove` — Stop and remove the service



## `postbode service install`

Install and start the service; stops a daemon that is already running so the service's takes over

**Usage:** `postbode service install [OPTIONS]`

###### **Options:**

* `--dry-run` — Print the file and the commands and change nothing



## `postbode service remove`

Stop and remove the service

**Usage:** `postbode service remove [OPTIONS]`

###### **Options:**

* `--dry-run` — Print the commands and change nothing



## `postbode sync`

Ask the daemon to sync now and apply rules; waits for the result

**Usage:** `postbode sync [OPTIONS]`

###### **Options:**

* `--account <ACCOUNT>`



## `postbode attachment`

List or save a message's attachments

**Usage:** `postbode attachment <COMMAND>`

###### **Subcommands:**

* `list` — Index, type, size and name of each attachment
* `save` — Save attachment N, as numbered by `attachment list`, into --dir



## `postbode attachment list`

Index, type, size and name of each attachment

**Usage:** `postbode attachment list [OPTIONS] <UID>`

###### **Arguments:**

* `<UID>`

###### **Options:**

* `--account <ACCOUNT>`
* `--folder <FOLDER>`

  Default value: `INBOX`
* `--json`



## `postbode attachment save`

Save attachment N, as numbered by `attachment list`, into --dir

**Usage:** `postbode attachment save [OPTIONS] <UID> <N>`

###### **Arguments:**

* `<UID>`
* `<N>`

###### **Options:**

* `--dir <DIR>`

  Default value: `.`
* `--account <ACCOUNT>`
* `--folder <FOLDER>`

  Default value: `INBOX`



## `postbode rules`

Inspect and test rules.toml

**Usage:** `postbode rules <COMMAND>`

###### **Subcommands:**

* `check` — Validate rules.toml
* `test` — Dry run: print what each rule would do to the cached messages; naming a rule previews it even while disabled
* `schema` — JSON Schema for rules.toml; a proposal is one entry of `rules`
* `propose` — Read one rule as JSON on stdin and add it disabled, for a human to approve
* `approve` — Enable a disabled rule, such as a proposal
* `reject` — Remove a pending proposal
* `list` — Names, enabled state and who proposed them
* `apply-existing` — Run one rule against mail that predates it



## `postbode rules check`

Validate rules.toml

**Usage:** `postbode rules check`



## `postbode rules test`

Dry run: print what each rule would do to the cached messages; naming a rule previews it even while disabled

**Usage:** `postbode rules test [OPTIONS] [NAME]`

###### **Arguments:**

* `<NAME>`

###### **Options:**

* `--account <ACCOUNT>`
* `--stdin` — Preview one rule read as JSON from stdin instead of rules.toml



## `postbode rules schema`

JSON Schema for rules.toml; a proposal is one entry of `rules`

**Usage:** `postbode rules schema`



## `postbode rules propose`

Read one rule as JSON on stdin and add it disabled, for a human to approve

**Usage:** `postbode rules propose [OPTIONS]`

###### **Options:**

* `--by <BY>` — Who proposes it, recorded as proposed_by = "cli:WHO"



## `postbode rules approve`

Enable a disabled rule, such as a proposal

**Usage:** `postbode rules approve <NAME>`

###### **Arguments:**

* `<NAME>`



## `postbode rules reject`

Remove a pending proposal

**Usage:** `postbode rules reject <NAME>`

###### **Arguments:**

* `<NAME>`



## `postbode rules list`

Names, enabled state and who proposed them

**Usage:** `postbode rules list [OPTIONS]`

###### **Options:**

* `--json`



## `postbode rules apply-existing`

Run one rule against mail that predates it

**Usage:** `postbode rules apply-existing [OPTIONS] <NAME>`

###### **Arguments:**

* `<NAME>`

###### **Options:**

* `--account <ACCOUNT>`
* `--dry-run`



## `postbode folders`

List folders with message and unread counts

**Usage:** `postbode folders [OPTIONS]`

###### **Options:**

* `--account <ACCOUNT>`
* `--json`



## `postbode list`

List recent messages, newest first

**Usage:** `postbode list [OPTIONS]`

###### **Options:**

* `--account <ACCOUNT>`
* `--folder <FOLDER>`

  Default value: `INBOX`
* `--limit <LIMIT>`

  Default value: `50`
* `--json`
* `--threads` — Group by conversation, the most recently active thread first



## `postbode search`

Full-text search (FTS5 syntax) over subject, addresses and fetched bodies, newest first

**Usage:** `postbode search [OPTIONS] <QUERY>`

###### **Arguments:**

* `<QUERY>`

###### **Options:**

* `--account <ACCOUNT>`
* `--folder <FOLDER>`
* `--bodies` — Fetch and index missing bodies first; slow on a large folder
* `--limit <LIMIT>`

  Default value: `50`
* `--json`



## `postbode show`

Show one message

**Usage:** `postbode show [OPTIONS] <UID>`

###### **Arguments:**

* `<UID>`

###### **Options:**

* `--account <ACCOUNT>`
* `--folder <FOLDER>`

  Default value: `INBOX`
* `--raw` — Print the raw RFC 5322 message instead of the text body
* `--json`



## `postbode mark`

Mark messages read or unread, flagged or unflagged

**Usage:** `postbode mark [OPTIONS] <HOW> <UIDS>...`

###### **Arguments:**

* `<HOW>`

  Possible values: `flag`, `read`, `unflag`, `unread`

* `<UIDS>` — Message uids in --folder, as `list` prints them

###### **Options:**

* `--account <ACCOUNT>`
* `--folder <FOLDER>`

  Default value: `INBOX`
* `--dry-run` — Print what would happen without touching the server



## `postbode move`

Move messages to another folder, creating it if needed

**Usage:** `postbode move [OPTIONS] --to <TO> <UIDS>...`

###### **Arguments:**

* `<UIDS>` — Message uids in --folder, as `list` prints them

###### **Options:**

* `--to <TO>`
* `--account <ACCOUNT>`
* `--folder <FOLDER>`

  Default value: `INBOX`
* `--dry-run` — Print what would happen without touching the server



## `postbode archive`

Move messages to the Archive folder

**Usage:** `postbode archive [OPTIONS] <UIDS>...`

###### **Arguments:**

* `<UIDS>` — Message uids in --folder, as `list` prints them

###### **Options:**

* `--account <ACCOUNT>`
* `--folder <FOLDER>`

  Default value: `INBOX`
* `--dry-run` — Print what would happen without touching the server



## `postbode delete`

Move messages to Trash; inside Trash, or without one, delete them keeping a local .eml backup

**Usage:** `postbode delete [OPTIONS] <UIDS>...`

###### **Arguments:**

* `<UIDS>` — Message uids in --folder, as `list` prints them

###### **Options:**

* `--account <ACCOUNT>`
* `--folder <FOLDER>`

  Default value: `INBOX`
* `--dry-run` — Print what would happen without touching the server



## `postbode log`

Show what rules did, newest first

**Usage:** `postbode log [OPTIONS]`

###### **Options:**

* `--account <ACCOUNT>`
* `--limit <LIMIT>`

  Default value: `50`
* `--json`



## `postbode trash`

Deleted mail kept for the retention period

**Usage:** `postbode trash <COMMAND>`

###### **Subcommands:**

* `list` — 
* `restore` — Append a trashed .eml back into its original folder
* `purge` — Remove trash files older than the retention period



## `postbode trash list`

**Usage:** `postbode trash list [OPTIONS]`

###### **Options:**

* `--account <ACCOUNT>`



## `postbode trash restore`

Append a trashed .eml back into its original folder

**Usage:** `postbode trash restore [OPTIONS] <FILE>`

###### **Arguments:**

* `<FILE>`

###### **Options:**

* `--account <ACCOUNT>`



## `postbode trash purge`

Remove trash files older than the retention period

**Usage:** `postbode trash purge [OPTIONS]`

###### **Options:**

* `--account <ACCOUNT>`



## `postbode account`

Manage accounts

**Usage:** `postbode account <COMMAND>`

###### **Subcommands:**

* `add` — Interactively add an IMAP account and test the login



## `postbode account add`

Interactively add an IMAP account and test the login

**Usage:** `postbode account add`



## `postbode guide`

Print the agent guide: how an LLM should drive Postbode

**Usage:** `postbode guide`



## `postbode mcp`

Serve Postbode to an agent host over MCP on stdio; hosts start this, see `postbode mcp install`

**Usage:** `postbode mcp [OPTIONS]
       mcp <COMMAND>`

###### **Subcommands:**

* `install` — Register `postbode mcp` with an agent host; re-run it to change the scopes

###### **Options:**

* `--scopes <SCOPES>` — Comma-separated: read, read:bodies, rules:propose, rules:write, mail:modify

  Default value: `read,rules:propose`
* `--account <ACCOUNT>` — Only this account; repeatable; default every account



## `postbode mcp install`

Register `postbode mcp` with an agent host; re-run it to change the scopes

**Usage:** `postbode mcp install [OPTIONS] <TARGET>`

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
