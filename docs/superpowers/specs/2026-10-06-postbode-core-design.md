# Postbode core design

Date: 2026-10-06. Status: draft for review. Covers phase 1 only, but records the decisions that constrain later phases.

## 1. Purpose

Postbode is a fast, simple desktop mail client for powerusers and developers, written in Rust with an egui GUI. Its differentiator is automatic mailbox hygiene: ordered rules that move mail into folders and delete transient mail such as sign-in codes and magic links once they are no longer needed. Rules are written by humans in a text file or the UI, and proposed by agents through a built-in MCP server.

Open source under `MIT OR Apache-2.0`, published at `github.com/pataar/postbode`.

## 2. Phases

Each phase gets its own spec and plan. This document is phase 1.

1. **Core** (this spec): IMAP sync into a local SQLite store, rules engine, trash with retention, direct message actions, search, threading, attachments, aliases, thin CLI. Usable daily as a mailbox cleaner and a scriptable mail backend. Everything the GUI will call exists and is tested here first.
2. **GUI**: egui reader on top of the store, rule editing, the sync loop in a background thread.
3. **MCP server**: `postbode mcp` over stdio exposing the core API with scopes and rule proposals.
4. **Compose and send** (SMTP), OS integration (notifications, mailto handler, tray), packaging for macOS and Linux (KDE Plasma).

Later, undated: daemon split (always-on sync, GUI and CLI as clients), provider adapters (Gmail, Outlook OAuth2), HTML bodies rendered natively by Blitz (`2026-10-07-postbode-html-design.md`), Windows.

## 3. Decisions log

| Decision | Choice | Rejected |
|---|---|---|
| First milestone | Headless core plus CLI | UI first; both at once |
| Process model | Single process; rules run while Postbode runs | Daemon now |
| Deletion timing | Rules re-evaluated every sync; `older_than` and `seen` conditions | Delete on arrival; delete on read only |
| Rules storage | `rules.toml` in the config dir, human-editable | Database only |
| Local store | Full envelope cache for all folders, bodies on demand | Last N days; no cache |
| Store layout | One SQLite file per account | One shared file |
| Backups | Deleted mail kept as `.eml` for 30 days | Full local archive |
| Credentials | OS keyring or password command, per account | Plaintext config |
| HTML mail | Text-first in egui, body panel isolated so an HTML view can join it; that view is Blitz, see `2026-10-07-postbode-html-design.md` | Embedded webview now; drop egui |
| Crate layout | One package, lib plus bin | Workspace now; async core |
| IMAP crate | `async-imap` on a per-account tokio runtime, hidden behind a sync trait | `imap` (last release 2025-02, 3.0 alpha for years) |
| SQLite tooling | `rusqlite` + `rusqlite_migration` + `serde_rusqlite` | diesel; sqlx (async) |
| MVP engine scope | Direct actions, special-use folders, FTS5 search, threading, attachments, aliases, Dovecot CI | SMTP send, offline action queue, contacts |
| Releases | release-plz for versions and crates.io, cargo-dist for binaries and the Homebrew tap | Hand-rolled scripts |
| Dependency updates | Dependabot with `.github/dependabot.yml` | Renovate |
| Notifications | Engine decides via rules and emits an event; the running front end delivers | GUI-only feature |
| Agent control | Self-describing CLI (`guide`, `rules schema`, `rules propose/approve`), shipped `SKILL.md`; MCP reuses the same text | Separate MCP-only docs |
| Documentation | `docs/` is the single source: embedded in the binary, tested in CI, mdBook-ready | Docs in README and doc comments only |
| MCP transport | stdio only | localhost listener with token |
| MCP safety | Rule proposals need human approval; per-server scopes | Full access once connected |

The IMAP crate choice amends the "sync core" decision slightly: async lives only inside the IMAP module, on a current-thread tokio runtime owned by each account thread. Nothing outside that module is async.

## 4. Architecture

One Cargo package `postbode`, edition 2024, `rust-version` pinned in `rust-toolchain.toml` and `.mise.toml`.

```
src/
  lib.rs            pub mod for everything below
  paths.rs          ProjectDirs split into config/state/cache; `Paths::under(root)` for tests
  config.rs         accounts: load config.toml
  credentials.rs    Secret type (no Debug), keyring and command backends, error enum
  rules/
    mod.rs          RuleFile parsing and validation, regex compiled once
    engine.rs       evaluate(rules, message, now, mode) -> Vec<Action>   pure
    apply.rs        apply(actions, message, mail_ops, store, trash)       side effects
  store.rs          SQLite open, migrations, typed queries
  mail_ops.rs       trait MailOps + ImapOps (async-imap) + RecordingOps (cfg(test))
  sync.rs           per-account loop: sync folders, run rules, IDLE
  trash.rs          write .eml before delete, list, restore, purge
  main.rs           clap CLI, thin: parse args, call lib, print
migrations/
  001-initial/up.sql
```

The library is the API boundary. The GUI, MCP server and daemon later consume `postbode::*` and never reach into SQLite or IMAP directly.

## 5. Configuration

Config dir: `~/Library/Application Support/postbode/` on macOS, `~/.config/postbode/` on Linux. State dir holds `accounts/<name>/mail.db` and `accounts/<name>/trash/`.

`config.toml`:

```toml
[[accounts]]
name = "work"
host = "imap.example.com"
port = 993
username = "pieter@example.com"
password = { keyring = true }           # or { command = "pass show mail/work" }
address = "pieter@example.com"          # primary address, default: username if it contains @
aliases = ["p@example.org", "*@shop.example.com"]   # optional, glob on the whole address
sync_interval_secs = 120                # default 120
trash_retention_days = 30               # default 30
notify = true                           # default true: notify for new INBOX mail not handled by a rule
ca_file = "/etc/ssl/private-ca.pem"     # optional, absolute path to a PEM with an extra trusted root, for a server with a private CA
```

`address` plus `aliases` define "me" for the account. Globs use `*` only, matched case-insensitively against the full address. Later phases use the same list for the From picker in compose and to reply from the alias a mail was sent to.

`rules.toml`:

```toml
[[rules]]
name = "purge sign-in codes"
account = "work"                        # optional, default: all accounts
folder = "INBOX"                        # optional, default: INBOX
enabled = true                          # optional, default: true
match.from = { regex = "no-?reply@" }
match.subject = { regex = "(?i)sign.?in|verification code|magic link" }
match.older_than = "1h"
match.seen = true
actions = ["delete"]

[[rules]]
name = "github to folder"
match.header = { name = "List-Id", contains = "github.com" }
actions = [{ move = "Lists/GitHub" }, "mark_read"]
```

Match keys: `from`, `to`, `cc`, `subject`, `body`, `header` (with `name`), each taking exactly one of `contains` (case-insensitive substring), `equals` (exact, case-insensitive for addresses), `regex` (`regex` crate syntax). Plus `older_than` (humantime duration), `seen` (bool), `to_me` (bool: any of To, Cc or Delivered-To matches the account address or an alias; `false` catches list and bcc mail) and `alias` (string: the mail was addressed to this specific alias, glob allowed). All conditions in a rule are ANDed. A rule with no match keys is a validation error.

Actions: `delete`, `mark_read`, `flag`, `archive` (move to the server's `\Archive` folder), `{ move = "Folder/Sub" }`, `notify`, `silent`.

Notification policy: a new message in INBOX produces a `NewMail` event unless a rule moved or deleted it, or a matching rule said `silent`. `notify` forces the event even when the message was moved, for example "move GitHub mail to a folder but still tell me". Per account, `notify = false` in `config.toml` turns the default off so only explicit `notify` rules fire.

Rules with `enabled = false` are skipped. This is also how MCP proposals are stored later: an agent appends a rule with `enabled = false` and `proposed_by = "mcp:<client name>"`, and a human flips it.

Files are written as tmp then rename. `rules.toml` is only written by `postbode rules` subcommands, the GUI and the MCP server; the sync loop never writes it.

## 6. Data model

One SQLite file per account at `state/accounts/<name>/mail.db`, WAL mode, `synchronous=NORMAL`. The account's sync thread is the main writer. The CLI also writes, for direct actions (mark, move, archive, delete), body fetches and `rules approve`. A writer that finds the database busy waits for it, up to 5 seconds, instead of failing. Cross-account views (`list` without `--account`, later a unified inbox and global search) iterate accounts or `ATTACH` the files with `UNION ALL`. Removing an account is removing its directory.

```sql
CREATE TABLE folders (
  name         TEXT PRIMARY KEY,
  uidvalidity  INTEGER NOT NULL,
  last_uid     INTEGER NOT NULL DEFAULT 0,
  special_use  TEXT                  -- Trash, Sent, Junk, Drafts, Archive (RFC 6154), NULL otherwise
);

CREATE TABLE messages (
  folder        TEXT NOT NULL REFERENCES folders(name) ON DELETE CASCADE,
  uid           INTEGER NOT NULL,
  message_id    TEXT,
  from_addr     TEXT,
  to_addr       TEXT,
  cc_addr       TEXT,
  delivered_to  TEXT,
  in_reply_to   TEXT,
  refs          TEXT,                -- References header, space-separated
  thread_id     TEXT NOT NULL,       -- root message-id of the thread, or own message-id
  subject       TEXT,
  date          INTEGER,            -- Date header, unix seconds
  internaldate  INTEGER NOT NULL,   -- server arrival time, unix seconds
  flags         TEXT NOT NULL,      -- space-separated IMAP flags
  size          INTEGER,
  headers       BLOB NOT NULL,      -- raw header block, parsed on demand for header rules
  raw           BLOB,               -- full RFC 5322 message, NULL until fetched
  body_text     TEXT,               -- plain text extracted from raw, for rules and search
  PRIMARY KEY (folder, uid)
);
CREATE INDEX messages_internaldate ON messages (folder, internaldate);
CREATE INDEX messages_thread ON messages (thread_id);
CREATE INDEX messages_message_id ON messages (message_id);

CREATE VIRTUAL TABLE messages_fts USING fts5 (
  subject, from_addr, to_addr, body_text,
  content='messages', content_rowid='rowid'
);
-- plus the three standard external-content triggers keeping messages_fts in sync

CREATE TABLE rules_seen (
  name           TEXT PRIMARY KEY,
  first_seen_at  INTEGER NOT NULL
);

CREATE TABLE rule_log (
  id          INTEGER PRIMARY KEY,
  at          INTEGER NOT NULL,
  rule_name   TEXT NOT NULL,
  folder      TEXT NOT NULL,
  uid         INTEGER NOT NULL,
  message_id  TEXT,
  subject     TEXT,
  action      TEXT NOT NULL,
  trash_file  TEXT                  -- set for delete actions
);
```

Migrations are numbered SQL files under `migrations/`, embedded with `include_dir`, applied by `rusqlite_migration` on open using `user_version`. Every account file runs the same migrations. Rows map to structs via `serde_rusqlite`.

Threading: on insert, `thread_id` is the first id in `References` if present, else `In-Reply-To`, else the own `message_id`. When a message arrives whose `message_id` is some existing row's `thread_id` ancestor, nothing is rewritten; threads are keyed on the root id, which is stable. Messages without a `message_id` get a synthetic `<uid>@<folder>.postbode` id.

Special-use folders come from `LIST (SPECIAL-USE)` or the folder attributes in a plain `LIST`. If the server marks none, `Trash`, `Sent`, `Junk`, `Drafts`, `Archive` by name are used as a fallback.

`rules_seen` records when a rule name was first loaded by this account. A rule only acts on messages whose `internaldate` is at or after its `first_seen_at`, so adding a rule never mass-deletes history. Renaming a rule resets this. `rules apply-existing` is the explicit opt-in to older mail.

Disabled rules have no entry: a rule's clock starts the first time it is loaded enabled, or at approval time when `rules approve` enables it, so a proposal approved a week later does not act on that week's mail, and disabling then enabling a rule restarts it.

## 7. Sync

One std thread per account owning one IMAP connection on a current-thread tokio runtime. Loop:

1. **Full sync** on start. `LIST` folders. For each folder `SELECT`, compare `UIDVALIDITY`; on change, delete the folder's rows and resync from UID 1. Otherwise `UID FETCH last_uid+1:* (UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER])` for new rows. The full header block is parsed locally with `mail-parser` and stored, so `header` rules and threading never need a body fetch, `UID FETCH 1:last_uid (UID FLAGS)` to update flags and detect removed UIDs. Store in one transaction per folder.
2. **Rules pass** for the account (section 8).
3. **IDLE** on INBOX. Wake on server push, on a timer every `sync_interval_secs`, or on shutdown. IDLE is re-issued before 29 minutes regardless.
4. On push: sync INBOX only. On timer: sync all folders. Then rules pass. `NewMail` events from the pass go out over an `std::sync::mpsc` channel the account thread was given at start. Back to 3.

`postbode run` owns the receiving end in phase 1 and delivers each event with `notify-rust`: title is the from address, body is the subject, no click handling. The GUI replaces this receiver in phase 2.

The timer stays even with push, because `older_than` rules fire without new mail. Servers without IDLE degrade to the timer loop.

Bodies are fetched on demand with `UID FETCH n BODY.PEEK[]` and stored in `messages.raw`, with `body_text` extracted at the same time: when a rule has a `body` condition, when the CLI shows a message, when `search` is asked to include bodies, and before any delete. The flag-update fetch over `1:last_uid` is O(folder size) per timer tick; ponytail: upgrade to CONDSTORE/QRESYNC when a large folder makes ticks slow.

Per-account failures log with the account name and retry with exponential backoff capped at 5 minutes. One account failing never stops the others.

## 8. Rules engine

```
evaluate(rules: &[Rule], msg: &Message, now: Timestamp, mode: Mode) -> Vec<(RuleName, Action)>
```

Pure. For each rule in file order:

0. Skip the message entirely, with no actions and no notification, if it carries the `$PostbodeRestored` keyword.
1. Skip if disabled, or `account`/`folder` don't match.
2. Skip if `msg.internaldate < rule.first_seen_at`, unless `mode == ApplyExisting`. `first_seen_at` is filled from `rules_seen` when the rules file is loaded for an account.
3. Evaluate each match condition. `body` requires `msg.raw`; the caller fetches it beforehand when any enabled rule for that folder has a body condition. Body text is the `text/plain` part via `mail-parser`, else the HTML part converted to text by `mail-parser`.
4. `older_than`: `now - internaldate >= duration`. `seen`: presence of `\Seen`.
5. Collect actions. After a `delete`, stop evaluating further rules for this message.
6. Decide notification: `notify` if any matched rule says `notify`; `silent` if any says `silent` or the message was moved or deleted; else the account default. `notify` beats `silent` only when both are explicit, in which case the later rule wins. Only messages new in this sync pass are considered. Messages found by a folder's first sync or a UIDVALIDITY resync never notify.

```
apply(plan, msg, mail_ops, store, trash) -> Result<()>
```

Dry runs (`rules test`, `apply-existing --dry-run`) stop after `evaluate` and print the plan; they never call `apply` and never touch `rule_log`. Each real action is written to `rule_log` first, then executed:

- `delete`: fetch raw if missing, write `accounts/<name>/trash/<unix>-<folder>-<uid>.eml`, then `UID STORE +FLAGS \Deleted` and `UID EXPUNGE`. If the trash write fails, the delete does not happen.
- `move`: `UID MOVE` if the server advertises `MOVE`, else `COPY` + delete flags + expunge. Create the target folder if missing. Update the local row to the new folder and UID from `COPYUID`, else let the next sync reconcile.
- `archive`: `move` to the folder with `special_use = Archive`; error if the server has none.
- `mark_read`, `flag`: `UID STORE +FLAGS`.

`notify` and `silent` are not applied through `MailOps`; `apply` returns them as a `NewMail { account, folder, uid, from, subject }` event the sync loop forwards to whoever is listening.

Direct actions from the CLI (`mark`, `move`, `delete`, `archive`) build the same `Action` values and go through the same `apply`, logged in `rule_log` with `rule_name = "cli"`. One difference: a user-initiated `delete` moves to the `\Trash` folder when the server has one, like other clients, and only falls back to expunge-plus-`.eml` when there is no Trash folder. Rule deletes always expunge-plus-`.eml`, because the point is a clean mailbox. Direct actions also offer `mark_unread`, `unflag` and the user delete (`trash`); these are not valid in `rules.toml`. `--dry-run` previews any direct action from the local store.

`MailOps` is the trait that `apply` and `sync` call: `list_folders`, `select`, `fetch_envelopes`, `fetch_flags`, `fetch_raw`, `store_flags`, `expunge`, `move_message`, `create_folder`, `idle`. Real impl wraps `async-imap`; `RecordingOps` in tests records calls and serves canned data. The trait exists for the fake.

Rules file validation fails as a whole on any error (bad regex, unknown action, unknown key, no match keys), reporting rule name and TOML span. The engine never runs with a partially parsed file.

## 9. Trash

`accounts/<name>/trash/` holds `.eml` files. `postbode trash list` reads the directory plus `rule_log` for context. `restore FILE` does `APPEND` into the original folder with the `$PostbodeRestored` keyword, which every rule skips, and removes the file. `purge` removes files older than `trash_retention_days`; the sync loop runs purge once per hour.

## 10. Credentials

`Secret(String)` with no `Debug`/`Display`, wrapped in `zeroize::Zeroizing`. Backends:

- `keyring = true`: `keyring-core` with `apple-native-keyring-store` on macOS and `zbus-secret-service-keyring-store` on Linux. Service `postbode`, user `<account name>`.
- `command = "..."`: run via `sh -c`, trim trailing newline, non-zero exit is an error.

Errors map to `CredentialError::{NotFound, Locked, Unavailable, CommandFailed}` with user-facing messages. Provider error text goes to the log at debug level only.

## 11. CLI

```
postbode run                                  sync loops for all accounts, foreground until Ctrl-C
postbode sync [--account NAME]                one-shot sync + rules pass
postbode rules check                          validate rules.toml
postbode rules test [NAME | --stdin] [--account NAME]   dry run; NAME previews a disabled proposal too, --stdin a draft JSON rule
postbode rules apply-existing NAME [--dry-run]
postbode rules schema                         JSON Schema for rules.toml, from the Rust types via schemars
postbode rules propose [--by WHO]             read one rule as JSON on stdin, append with enabled = false
postbode rules approve NAME | reject NAME     flip enabled, or remove the proposal
postbode rules list [--json]                  name, enabled, proposed_by
postbode guide                                print docs/src/agent-guide.md
postbode folders [--account NAME] [--json]
postbode list [--account NAME] [--folder INBOX] [--limit 50] [--threads] [--json]
postbode show UID [--account NAME] [--folder INBOX] [--raw] [--json]
postbode log [--limit 50] [--json]
postbode search QUERY [--account NAME] [--folder NAME] [--bodies] [--limit 50] [--json]
postbode mark read|unread|flag|unflag UID... [--account NAME] [--folder INBOX] [--dry-run]
postbode move UID... --to FOLDER [--account NAME] [--folder INBOX] [--dry-run]
postbode archive UID... [--account NAME] [--folder INBOX] [--dry-run]
postbode delete UID... [--account NAME] [--folder INBOX] [--dry-run]     to Trash, or expunge + .eml if none
postbode attachment list UID [--json] | save UID N [--dir DIR]   [--account NAME] [--folder INBOX]
postbode trash list | restore FILE | purge
postbode account add                          interactive; tests login before saving
```

`rules propose` is the only write path agents use; they never edit `rules.toml` directly. A proposed rule carries `proposed_by = "cli:<who>"` now and `"mcp:<client>"` later. `rules check` errors name the rule and the TOML span, which is what lets an agent self-correct.

`list --threads` groups rows by `thread_id`, newest thread first, with a depth indicator. `folders` shows total and unread counts. `search` uses FTS5 syntax (`invoice from_addr:acme`); `--bodies` fetches and indexes missing bodies in the matching folders first, which can be slow on a large folder and says so. A query FTS5 cannot parse, such as a bare address, is retried with each word quoted.

Plain text, one record per line, so it pipes into grep and fzf. `--json` output shapes are the ones the MCP server returns later. Logging via `log` + `env_logger` to stderr, default filter `warn,postbode=info`, `RUST_LOG` overrides. `account add` is the only interactive command.

## 12. Errors, logging, paths

- `anyhow` in `main.rs` and `sync.rs`. `thiserror` enums where a human reads the text: `RulesError`, `CredentialError`, `StoreError`.
- Secrets never implement `Debug`. Status enums for logging use a `log_label()` rather than `Debug`.
- `Paths` from `directories::ProjectDirs("", "", "postbode")`, directories created with mode 0700. `Paths::under(tempdir)` roots everything for tests. `Paths::account(name)` gives the per-account state dir holding `mail.db` and `trash/`.
- All file writes: tmp in the same dir, then rename.
- IMAP connections give up after 30s while connecting (TCP, TLS and login together) and use TCP keepalive (60s idle, then 4 probes 15s apart), so a dead connection fails within about two minutes and the run loop reconnects. A live server that stops answering mid-command is not bounded yet.

## 13. Testing

- `rules::engine`: table-driven unit tests over sample messages: sign-in code deleted only after 1h and only when seen, move + mark_read chain, delete stops the chain, rule added after a message does not touch it, `ApplyExisting` does.
- `rules` parsing: invalid regex, unknown action, no match keys all fail with the rule name.
- `store`: temp file, migrations `validate()`, insert/update/flag/removal, `rules_seen` behaviour.
- `rules::apply` and `sync` with `RecordingOps`: trash file exists before expunge is called; `MOVE` vs `COPY` fallback; `UIDVALIDITY` change wipes and resyncs.
- `trash`: restore removes the file and calls `APPEND`; purge respects retention.
- CLI: black-box tests running `CARGO_BIN_EXE_postbode` with `HOME` and `XDG_*` pointed at a temp dir: `rules check` on a bad file exits non-zero, `list` on an empty store prints nothing.
- Threading: table-driven tests over References/In-Reply-To combinations, missing message-id, out-of-order arrival.
- Search: FTS triggers keep the index in sync on insert, update of `body_text`, and delete.
- Aliases: glob matching, `to_me` against To, Cc and Delivered-To, `alias` with and without wildcard.
- Notifications: default fires for untouched INBOX mail, moved mail is silent unless `notify`, `silent` suppresses, account `notify = false` leaves only explicit rules, old messages on a resync never notify.
- **Integration tests against Dovecot** in `tests/imap_live.rs`. `tests/dovecot/compose.yml` runs two Dovecot 2.4 servers on localhost: one advertising MOVE and UIDPLUS (port 10993), one without either (port 11993). Their certificate comes from a test-only CA in `tests/dovecot/certs/`, which test accounts trust through `ca_file`. Each test logs in as its own throwaway user and seeds mail with APPEND, so tests never share a mailbox. They cover special-use detection, flags and keywords, move with and without MOVE, expunge with and without UIDPLUS, IDLE wake, a UIDVALIDITY change, a 1,200-message first sync in chunks, a command waking IDLE through the engine, a rule delete keeping its backup, `delete` to Trash, and `sync` exiting non-zero when a rule fails. The Ubuntu CI job starts the compose file. Locally, run `docker compose -f tests/dovecot/compose.yml up -d` and set `POSTBODE_TEST_IMAP_HOST=localhost`. The tests are skipped, not failed, when that variable is unset or empty.

## 14. Repo conventions and CI

- `rust-toolchain.toml` with `channel`, `rustfmt`, `clippy`; `.mise.toml` pinning the same Rust.
- `Cargo.toml`: every dependency has a one-line comment stating why. Dev profile: `[profile.dev.package."*"] opt-level = 2`. Release: `lto = "thin"`, `codegen-units = 1`, `strip = true`.
- Build speed: dependency weight is watched with `cargo build --timings` and `cargo-machete` in CI. `.mise.toml` installs kache. Each developer runs `kache init` once, which makes it the `RUSTC_WRAPPER` in their own Cargo config: compiler outputs are content-addressed and shared across git worktrees, restored with reflinks on APFS, so a fresh worktree starts warm. No mold: Rust's default linker on x86_64 Linux has been lld since 1.90. CI uses `kunobi-ninja/kache-action` through the GitHub cache service, plus `Swatinem/rust-cache` for the registry and git deps only, both saving on pushes to `main`. Edit loop is `cargo check` and `cargo nextest`.
- `AGENTS.md`: module map, definition of done (fmt, clippy -D warnings, tests, platform coverage reported honestly), one privacy rule: agents never read message bodies from a user's store, and never log bodies or secrets. `CLAUDE.md` is one line pointing to it.
- GitHub Actions `ci.yml`: fmt, clippy `-D warnings`, `cargo-machete` and `cargo audit` on Ubuntu, and tests on `ubuntu-latest` and `macos-latest`. The Ubuntu test job starts `tests/dovecot/compose.yml` for the live IMAP tests; a compose step rather than a service container, so CI and local runs share one config. Concurrency with cancel-in-progress, `permissions: contents: read`.
- `LICENSE-MIT`, `LICENSE-APACHE`, README with a 20-line quickstart.

Dependencies for phase 1: `anyhow`, `async-imap` (runtime-tokio), `clap` (derive), `directories`, `env_logger`, `globset` (alias globs), `humantime`, `include_dir`, `keyring-core` plus the two platform stores, `log`, `mail-parser`, `notify-rust`, `regex`, `rusqlite` (bundled, FTS5 is included in the bundled build), `rusqlite_migration`, `rustls` via `tokio-rustls` and `webpki-roots`, `schemars`, `serde`, `serde_json`, `serde_rusqlite`, `thiserror`, `tokio` (rt, net, time, macros), `toml`, `zeroize`. Dev: `tempfile`, `clap-markdown`.

## 15. Release and distribution

Two tools, both driven by conventional commits and tags, no hand-written release scripts.

**release-plz** runs on every push to `main`. It keeps a release PR open that bumps `Cargo.toml`, updates `CHANGELOG.md` from conventional commits, and when merged, tags `vX.Y.Z` and publishes to crates.io. Publishing uses crates.io trusted publishing (OIDC from GitHub Actions), so no API token is stored. `cargo install postbode` and `mise use cargo:postbode` work from that moment.

release-plz does not create the GitHub release (`git_release_enable = false`); dist does, from the tag. release-plz pushes with a `RELEASE_PLZ_TOKEN` fine-grained token, because tags and PRs created with the default `GITHUB_TOKEN` start no other workflows. The package `include` list keeps tests, the test CA and the design docs out of the published crate.

**cargo-dist** runs on the tag. It builds release binaries for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`, attaches them with checksums to a GitHub release, and pushes a formula to `pataar/homebrew-tap`. The `.deb` and `.rpm` outputs are left off until the GUI phase. `dist-workspace.toml` is the only config, and `dist generate` writes the workflow from it.

What each channel needs:

| Channel | Install command | Setup |
|---|---|---|
| crates.io | `cargo install postbode` | Trusted publisher configured once on crates.io for `pataar/postbode` |
| mise | `mise use ubi:pataar/postbode` or `cargo:postbode` | Nothing; `ubi` reads GitHub release assets, `cargo` reads crates.io. An aqua-registry PR comes later once releases are stable, which makes plain `mise use postbode` work |
| Homebrew | `brew install pataar/tap/postbode` | The existing `pataar/homebrew-tap` repo (shared with gast) and a `HOMEBREW_TAP_TOKEN` fine-grained secret with contents write on that repo. homebrew-core is a later submission when the project has users |

Human steps, one time: add the `HOMEBREW_TAP_TOKEN` and `RELEASE_PLZ_TOKEN` secrets, publish 0.1.0 by hand with `cargo publish` (crates.io requires the first publish with a token), push the first tag `v0.1.0` by hand to run the first dist release, then register the trusted publisher on crates.io. `pataar/homebrew-tap` already exists and is shared with gast, so there is no tap repo to create.

Release binaries are not code-signed yet. On macOS every new binary asks again for Keychain access. Developer ID signing is a later decision.

**Dependabot**, configured in `.github/dependabot.yml`: weekly updates for `cargo` (minor and patch grouped), GitHub Actions (grouped) and `rust-toolchain.toml`, monthly for the Dovecot image in `tests/dovecot/compose.yml`; a 3-day cooldown on crates and actions; commit messages `chore(deps): …`. `.github/workflows/release.yml` is excluded because `dist generate` owns it and dist's plan check fails on any edit. Dependabot does not read `.mise.toml`, so a Rust bump PR needs the `rust` line there (and `rust-version` in `Cargo.toml`) raised by hand in the same PR. Dependabot alerts and security updates are switched on in the repo settings.

## 16. Documentation

`docs/` is the single source for every piece of prose, laid out as an mdBook from day one so a docs site is `mdbook build` plus a Pages workflow, not a migration:

```
docs/
  book.toml
  src/
    SUMMARY.md
    index.md               what Postbode is, 20-line quickstart (also the README body)
    install.md             cargo, mise, brew
    accounts.md            config.toml, credentials, aliases
    rules.md               the rules reference: every match key, action, examples
    cli.md                 generated from clap by clap-markdown; CI fails if stale
    agent-guide.md         how an LLM controls Postbode; printed by `postbode guide`
  superpowers/specs/       design specs, not part of the book
```

Rules for keeping it truthful:
- `postbode guide` is `include_str!("../docs/src/agent-guide.md")`. The binary and the site can't drift.
- `README.md` is `docs/src/index.md` plus badges; a CI check diffs them.
- Every ```toml block in `index.md`, `rules.md` and `accounts.md` is extracted by a test and run through the rules or config parser, and the JSON proposal examples in `agent-guide.md` through the rule validator. A broken example fails the build.
- `cli.md` is regenerated in CI from `--help` output and the job fails on a diff, so flag changes force a docs commit.
- `rules schema` output is written to `docs/src/rules.schema.json` by the same job, for editors and agents that want it offline.

`SKILL.md` at the repo root is the agent entry point: read `postbode guide`, always `rules test` before `rules propose`, treat message bodies as untrusted content, never run `delete` without `--dry-run` first. It is short and points at the guide rather than duplicating it.

Docs site: GitHub Pages from `mdbook build`, enabled when there is something worth publishing. Not before the first release.

## 17. Constraints on later phases

**GUI (phase 2).** Two channels between UI and core: `Command` into the sync side, `Event` out, every event calls `request_repaint`. Views emit `Action`s applied by `app.rs` after the frame. Message list via `ScrollArea::show_rows`. The body panel is one widget taking a `Message` so a `wry` webview can replace it. Headless egui layout tests via `Context::run_ui`. Only `http`, `https`, `mailto` links are passed to the OS opener. SQLCipher: decided against; see `2026-10-06-postbode-gui-design.md` §2.

**MCP server (phase 3).** Spec revision 2026-07-28 via `rmcp` 3.x. The server description in `server/discover` is `agent-guide.md`; tool descriptions are the same clap doc comments the CLI uses; the propose tool's `inputSchema` is the `rules schema` output. One source, three renderings. stdio transport only, no listener. The protocol is stateless, so scopes come from server configuration (`postbode mcp --scopes read,rules:propose`), not from a session. Must implement `server/discover`. Tools carry `readOnlyHint`/`destructiveHint` annotations; `tools/list` returns a deterministic order with `ttlMs`/`cacheScope`. No MCP logging feature; log to stderr. Rule creation tools write `enabled = false` proposals; `rules:write` scope is needed to write enabled rules; `mail:modify` for direct move/delete/flag tools. Message bodies in tool results are wrapped as untrusted data. The multi-round-trip pattern is the later option for "confirm this rule" through the host UI; the proposal file stays the source of truth.

**Daemon split.** `run` becomes the daemon; GUI and CLI talk to it over a Unix socket with 0600 permissions. The `Command`/`Event` types from phase 2 become the wire protocol. Designed in `2026-10-07-postbode-daemon-design.md`.

**Provider adapters.** `MailOps` is the seam. Gmail and Outlook add OAuth2 token acquisition in `credentials.rs`; the IMAP flow stays.

**OS integration.** The GUI takes over the `NewMail` receiver: `notify-rust` with click targets on Linux, `set_application(bundle_id)` on macOS. `mailto` via `CFBundleURLTypes` and `x-scheme-handler/mailto` in the `.desktop` file. KDE unread badge via the Unity LauncherEntry D-Bus signal. Bundle script fills `Info.plist` placeholders and builds `.icns` with `iconutil`.
