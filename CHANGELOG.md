# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/pataar/postbode/compare/v0.1.0...v0.2.0) - 2026-10-06

### Added

- *(rules)* add set_enabled for toggling a rule in place
- *(engine)* run accounts behind a lock and route commands to them
- *(sync)* drain commands between passes and wake IDLE for them
- *(sync)* add commands, command results and activity events
- *(sync)* fetch envelopes in resumable 500-message chunks with progress checkpoints
- *(store)* add initial_uid_next and thread summaries

### Fixed

- *(sync)* only count synced mail as fresh so a moved-in row cannot hide mail below it
- *(sync)* mark mail in folders without rules as seen so a later body rule skips it
- *(sync)* end a full pass on a lost connection instead of skipping every folder
- *(sync)* clear the wake flag with acquire ordering before draining commands
- *(engine)* join sync threads on drop and report a failed spawn per account
- *(sync)* log a UIDVALIDITY change found between chunks
- *(sync)* persist which mail the rules have seen so aborted passes still notify
- *(sync)* stop draining after a lost connection and drain before INBOX-only passes
- *(sync)* return the real lost-connection error and test command failure paths

### Other

- publish the mdBook docs to GitHub Pages ([#5](https://github.com/pataar/postbode/pull/5))
- add peach-and-mint terminal logo and app icon
- *(engine)* make Engine::start infallible
- *(engine)* retry taking an account lock that a concurrent spawn briefly inherited
- *(sync)* a command sent while offline runs after reconnecting
- *(mail_ops)* split fetch_new into search_uids and fetch_envelopes

## [0.1.0](https://github.com/pataar/postbode/releases/tag/v0.1.0) - 2026-10-06

### Added

- trust an extra CA per account with ca_file
- bound IMAP connect with a timeout and enable TCP keepalive
- rules schema, propose, approve, reject and test --stdin
- list and save attachments
- list --threads groups conversations
- full-text search with optional body fetch
- mark, move, archive and delete commands with --dry-run
- direct message actions through the rules apply path
- rules skip mail restored from trash
- account column, account key and backup subjects in list, log and trash list output
- postbode CLI with run, sync, rules, list, show, log and trash
- per-account sync loop with IDLE, rules pass and trash purge
- apply rule actions with trash-first delete
- trash directory for deleted mail backups
- IMAP MailOps on async-imap with IDLE
- MailOps trait with recording fake
- pure rules evaluation with notification policy
- rules.toml parsing, validation and compilation
- per-account sqlite store with migrations
- header parsing, thread ids and body text extraction
- credentials from keyring or password command
- account config with aliases and identity matching
- platform paths and atomic file writes

### Fixed

- *(imap)* do not wait for DNS past the connect timeout
- *(cli)* ask for an extra CA file in account add
- hint when the keyring waits on a prompt, search bodies offline
- private rules lock, write through symlinked config, keep purging past a bad file
- small final-review fixes
- serialize rules.toml edits and use unique temp files
- delete preview and log say when mail is expunged
- clean and validate agent-supplied rule strings
- approving a rule starts its clock at approval time
- rejecting or approving a rule keeps the user's comments
- a disabled rule starts acting from when it is enabled
- strip control characters from error lines and rule names, sync exits non-zero on folder errors
- report database errors while loading rules as database errors
- show password command errors on the terminal
- create config and rules files private
- rules test rejects unknown names and run validates rules before starting
- reject unknown keys in account config
- forget first-seen times of rules removed from rules.toml
- ignore FETCH responses without INTERNALDATE or headers
- move into a folder that already exists on the server
- drop the re-select after each applied rule
- count folder messages with a COUNT query
- fsync the directory after an atomic rename
- fall back to folder names for unmarked special-use roles
- skip body fetches for messages found by an initial sync
- stable integer id for messages FTS and skip unchanged flag writes
- strip control characters from server text on the terminal and escape notification markup
- equals on from, to and cc matches a bare address
- skip flags a message already has and count only executed actions
- timer loop for servers without IDLE and reset backoff after a full cycle
- report a failing folder and keep syncing the others
- write each folder sync in one transaction after the network calls
- skip rule actions and show on a folder whose UIDVALIDITY changed
- validate new accounts, side-effect-free rule previews, report apply errors
- only newly tracked or reset folders count as initial sync
- isolate rule failures, reset backoff, skip initial-sync notifications
- skip same-folder moves and keep failed applies side-effect free
- absolute deadline for IMAP IDLE
- deleted mail stays silent and clamp older_than
- reject unknown top-level keys and empty rule values
- create private dirs recursively and fsync atomic writes

### Other

- release with release-plz, dist and a Homebrew tap
- fmt, clippy, machete, audit and tests on Linux and macOS with live IMAP
- live sync, rule delete, UIDVALIDITY and CLI exit-code tests
- live IMAP tests against Dovecot with and without MOVE
- plan 3 — CI, releases and live IMAP tests
- CLI store writes, approval clock, restore keyword caveat and trash retention
- approval timing, account scope, dry-run caveat and spec corrections
- spec matches the engine features as built
- regeneration steps in AGENTS.md
- mdBook docs, agent guide and SKILL.md, kept honest by tests
- fake IMAP move reports no new uid, like the real client
- plan for engine features
- share the sync pass, IMAP connect and date formatting
- look up fresh messages in a HashSet during the rules run
- drop unused trash::read_eml and tokio sync feature
- make RecordingOps match IMAP expunge and append semantics
- rely on mail-parser for html body text
- ensure_account is idempotent
- pin rust 1.99 and planned crate versions
- scaffold postbode package with spec and plan
