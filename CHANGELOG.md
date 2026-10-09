# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0](https://github.com/pataar/postbode/compare/v0.2.0...v0.3.0) - 2026-10-09

### Added

- *(gui)* reader toolbar with Save .eml, View source and a Text | HTML switch ([#91](https://github.com/pataar/postbode/pull/91))
- *(gui)* opt-in notice for a newer release in the status bar ([#90](https://github.com/pataar/postbode/pull/90))
- *(gui)* group counts by the device's region ([#82](https://github.com/pataar/postbode/pull/82))
- *(gui)* add a toolbar, a sync button and the version ([#81](https://github.com/pataar/postbode/pull/81))
- *(gui)* show the full date with seconds and offset in the reader ([#80](https://github.com/pataar/postbode/pull/80))
- *(gui)* remember window size, position and pane widths ([#83](https://github.com/pataar/postbode/pull/83))
- *(gui)* align message rows in columns with the date on the right ([#79](https://github.com/pataar/postbode/pull/79))
- *(gui)* show custom folders as a collapsible tree ([#78](https://github.com/pataar/postbode/pull/78))
- *(store)* keep each folder's IMAP hierarchy delimiter ([#77](https://github.com/pataar/postbode/pull/77))
- *(gui)* full-width folder rows with icons and counts, views pinned below ([#74](https://github.com/pataar/postbode/pull/74))
- *(gui)* add the Phosphor icon font ([#73](https://github.com/pataar/postbode/pull/73))
- *(rules)* any, none, tags and value lists in conditions ([#71](https://github.com/pataar/postbode/pull/71))
- AppImage for Linux, one file for window, CLI and MCP ([#66](https://github.com/pataar/postbode/pull/66))
- macOS app bundle, DMG and Homebrew cask ([#38](https://github.com/pataar/postbode/pull/38))
- *(gui)* render HTML mail with Blitz ([#65](https://github.com/pataar/postbode/pull/65))

### Fixed

- *(sync)* show archived, moved and trashed mail in the target folder ([#92](https://github.com/pataar/postbode/pull/92))
- *(gui)* give the macOS Dock icon Apple's margin ([#89](https://github.com/pataar/postbode/pull/89))
- *(gui)* prefer the integrated GPU on macOS ([#84](https://github.com/pataar/postbode/pull/84))
- *(gui)* keep selected text readable in text fields ([#75](https://github.com/pataar/postbode/pull/75))
- *(gui)* darken the light theme's teal and green to pass WCAG AA ([#72](https://github.com/pataar/postbode/pull/72))

### Other

- *(gui)* align panes on one 16 px grid, truncate long reader headers ([#88](https://github.com/pataar/postbode/pull/88))
- show GUI screenshots from the snapshot tests ([#85](https://github.com/pataar/postbode/pull/85))
- running headless on a server or in Docker ([#76](https://github.com/pataar/postbode/pull/76))
- refresh after the AppImage and macOS app merges ([#69](https://github.com/pataar/postbode/pull/69))

## [0.2.0](https://github.com/pataar/postbode/compare/v0.1.0...v0.2.0) - 2026-10-07

### Added

- *(gui)* Wayland app id and a Linux desktop entry ([#34](https://github.com/pataar/postbode/pull/34))
- *(daemon)* one background daemon owns sync and IMAP; GUI, CLI and MCP are clients ([#16](https://github.com/pataar/postbode/pull/16))
- *(mcp)* MCP server with scopes, account filter and host install ([#15](https://github.com/pataar/postbode/pull/15))
- *(gui)* Backspace deletes on macOS
- *(gui)* Catppuccin Mocha and Latte palettes, coloured rows and the window icon
- *(gui)* activity and trash views with restore, and a key help window
- *(gui)* rules view with proposals and switches; react to rules.toml and config.toml edits
- *(gui)* search the account from the list with /
- *(gui)* show the message body with safe links, attachments and mark-read after a second
- *(gui)* archive, delete, move, read and flag with optimistic rows and rollback
- *(gui)* show per-account activity, errors and history in a status bar with a theme switch
- *(gui)* list threads with keyboard navigation, expansion and multi-select
- *(gui)* add postbode gui with the account and folder tree
- *(config)* add [ui] theme and save_theme that keeps comments
- *(rules)* add set_enabled for toggling a rule in place
- *(engine)* run accounts behind a lock and route commands to them
- *(sync)* drain commands between passes and wake IDLE for them
- *(sync)* add commands, command results and activity events
- *(sync)* fetch envelopes in resumable 500-message chunks with progress checkpoints
- *(store)* add initial_uid_next and thread summaries

### Fixed

- *(cli)* show times in local time, from a shared time module ([#58](https://github.com/pataar/postbode/pull/58))
- *(gui)* explain startup errors in a window ([#49](https://github.com/pataar/postbode/pull/49))
- *(config)* refuse `password = { keyring = false }` ([#57](https://github.com/pataar/postbode/pull/57))
- *(service)* pass POSTBODE_HOME on to the service's daemon ([#55](https://github.com/pataar/postbode/pull/55))
- *(daemon)* start without accounts ([#47](https://github.com/pataar/postbode/pull/47))
- *(daemon)* deliver a client's own action result once ([#46](https://github.com/pataar/postbode/pull/46))
- *(gui)* readable bold text in the dark theme ([#44](https://github.com/pataar/postbode/pull/44))
- *(daemon)* sync every account when rules.toml changes ([#45](https://github.com/pataar/postbode/pull/45))
- *(sync)* ghost rows and overwritten backups, found by model-based sync tests ([#37](https://github.com/pataar/postbode/pull/37))
- *(rules)* refuse blank and overlong names and folders ([#32](https://github.com/pataar/postbode/pull/32))
- store Homebrew's stable opt path for the daemon service and MCP hosts ([#29](https://github.com/pataar/postbode/pull/29))
- *(gui)* start the read delay when the body text appears
- *(gui)* keep the cursor on its thread when the thread gets new mail
- *(gui)* a click on the shown message opens it
- *(gui)* migrate the stores before the sync threads start
- *(gui)* open rules.toml in a text editor and log a failed opener
- *(gui)* offer a search hit's other folders in the move picker
- *(gui)* visible pane focus, cached body text and safer attachment names
- *(gui)* mark mail read only after the user opens it
- *(gui)* keep the cursor on its message across reloads
- *(gui)* visible focus ring on text fields and readable light-mode selection
- *(gui)* never link a bare scheme and keep the read timer across a fetched body
- *(gui)* stack optimistic edits per row and explain why actions are off
- *(gui)* keep progress ticks out of the history and errors on the line while offline
- *(gui)* keep egui widget focus out of the way of list keys
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

- *(gui)* repaint from the client's subscription, not a relay thread ([#52](https://github.com/pataar/postbode/pull/52))
- *(sync)* move the sync tests into sync/tests.rs ([#64](https://github.com/pataar/postbode/pull/64))
- build Message and AccountConfig fixtures from their defaults ([#62](https://github.com/pataar/postbode/pull/62))
- *(daemon)* end the own-completion test at its own account's sync ([#63](https://github.com/pataar/postbode/pull/63))
- share the action, restore and approve steps of the CLI and MCP ([#51](https://github.com/pataar/postbode/pull/51))
- *(store)* drop the unused Message-ID index ([#61](https://github.com/pataar/postbode/pull/61))
- say what tokio is for in Cargo.toml ([#60](https://github.com/pataar/postbode/pull/60))
- one flag check and one source of account defaults ([#59](https://github.com/pataar/postbode/pull/59))
- *(mail_ops)* a move no longer claims to return the new uid ([#56](https://github.com/pataar/postbode/pull/56))
- *(daemon)* keep test-only client and sync code out of the binary ([#50](https://github.com/pataar/postbode/pull/50))
- *(actions)* drop fetch_bodies' unused progress callback ([#48](https://github.com/pataar/postbode/pull/48))
- describe sync and restore as the daemon does them ([#43](https://github.com/pataar/postbode/pull/43))
- *(cli)* print rule errors without faking an event ([#54](https://github.com/pataar/postbode/pull/54))
- *(daemon)* make the two timing-dependent daemon tests deterministic ([#53](https://github.com/pataar/postbode/pull/53))
- *(gui)* snapshot tests for the main views ([#40](https://github.com/pataar/postbode/pull/40))
- property tests for message, rules and wire parsing ([#36](https://github.com/pataar/postbode/pull/36))
- generate app icons from icon.svg with packaging/icons.sh ([#33](https://github.com/pataar/postbode/pull/33))
- *(mcp)* sweep every tool with junk arguments ([#31](https://github.com/pataar/postbode/pull/31))
- deny unwrap, expect and panic in shipped code ([#28](https://github.com/pataar/postbode/pull/28))
- simplify gui selection, store folder columns and sync pass state ([#14](https://github.com/pataar/postbode/pull/14))
- *(gui)* say in the key help that delete from Trash is permanent
- describe what delete does and what the Trash view lists
- describe the mail window and the [ui] theme
- link the docs site and community files from the README ([#12](https://github.com/pataar/postbode/pull/12))
- sharpen crate keywords and forbid unsafe code ([#9](https://github.com/pataar/postbode/pull/9))
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
