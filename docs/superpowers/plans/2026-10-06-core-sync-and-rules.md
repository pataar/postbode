# Postbode core: sync and rules — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `postbode` binary that syncs IMAP accounts into per-account SQLite files, applies `rules.toml` on every sync, keeps deleted mail as `.eml` for 30 days, and notifies on new mail, driven by `postbode run`.

**Architecture:** One Cargo package, library plus binary. The library holds paths, config, credentials, header parsing, the SQLite store, rules parsing and the pure `evaluate`, a `MailOps` trait with a real `async-imap` implementation and a recording fake, `apply` for side effects, and the per-account sync loop. The binary is a thin clap CLI. Async exists only inside the IMAP module, on a current-thread tokio runtime owned by each account thread.

**Tech Stack:** Rust edition 2024, `async-imap` 0.12 + `tokio-rustls` 0.26, `rusqlite` 0.40 + `rusqlite_migration` 2.6 + `serde_rusqlite` 0.43, `mail-parser` 0.11, `keyring-core` 1 with the Apple and zbus stores, `clap` 4, `regex`, `globset`, `humantime`, `html2text`, `notify-rust` 4, `log` + `env_logger`.

**Spec:** `docs/superpowers/specs/2026-10-06-postbode-core-design.md`

This is plan 1 of 3 for the spec. Plan 2 adds search, threads listing, attachments, direct actions, agent commands (`rules propose/approve/reject/schema`, `guide`) and the mdBook skeleton. Plan 3 adds the Dovecot live tests, full CI, kache, release-plz, cargo-dist and Renovate.

## Global Constraints

- Rust edition `2024`, toolchain pinned to `1.91` in `rust-toolchain.toml` and `.mise.toml`. License `MIT OR Apache-2.0`.
- One package named `postbode`, `src/lib.rs` + `src/main.rs`. No workspace.
- Every dependency in `Cargo.toml` carries a one-line comment with its reason.
- `[profile.dev.package."*"] opt-level = 2`. Release: `lto = "thin"`, `codegen-units = 1`, `strip = true`.
- Secrets never implement `Debug` or `Display`. Provider error text from the keyring goes to `log::debug!` only.
- All file writes go through `paths::write_atomic` (tmp in the same dir, then rename). Directories are created with mode `0700` on Unix.
- A rule delete never happens unless the `.eml` was written to trash first.
- A rule acts only on messages whose `internaldate >= rule.first_seen_at`, unless mode is `ApplyExisting`.
- Only messages new in the current sync pass can produce a `NewMail` event.
- Logging through the `log` facade; default filter `warn,postbode=info`; `RUST_LOG` overrides.
- Conventional commits, one commit per task step that says commit. Do not push.
- Run `cargo fmt` and `cargo clippy --all-targets -- -D warnings` before every commit.

## Review Focus

1. **A `UIDVALIDITY` change on a folder with pending rule state**: rows are wiped and resynced; the resync must not fire notifications or re-run deletes on mail that was already handled. Pinned in Task 13 (`resync_after_uidvalidity_change_does_not_notify`).
2. **Server returns the last message for `N:*` when `N` exceeds the max UID**: the sync must not re-insert an existing UID or fail on the primary key. Pinned in Task 13 (`fetch_new_ignores_uids_below_last_uid`).
3. **A message with no `Message-ID`**: threading and the log must still work; the synthetic id must be stable across syncs. Pinned in Task 5 (`thread_id_without_message_id_is_synthetic_and_stable`).
4. **`rules.toml` is edited to an invalid state while `run` is going**: the loop must keep the last good rules and log the error, not stop syncing or run with half a file. Pinned in Task 13 (`invalid_rules_file_keeps_previous_rules`).
5. **Password command prints a trailing newline or nothing**: the trailing newline is stripped; empty output is an error, not an empty password sent to the server. Pinned in Task 4 (`command_output_is_trimmed` and `empty_command_output_is_error`).

---

## File structure

```
Cargo.toml                 package, deps with reasons, profiles
rust-toolchain.toml        channel 1.91, rustfmt, clippy
.mise.toml                 rust 1.91
.gitignore                 target/, *.db*, .brainstorm_projects/
LICENSE-MIT, LICENSE-APACHE
AGENTS.md                  module map, definition of done, privacy rule
migrations/001-initial/up.sql
src/lib.rs                 pub mod list
src/paths.rs               Paths, write_atomic
src/config.rs              Config, AccountConfig, PasswordSource, Identity
src/credentials.rs         Secret, CredentialError, resolve, store
src/message.rs             Parsed headers, thread_id, body_text, address helpers
src/store.rs               Store, Folder, Message, LogEntry, queries
src/rules/mod.rs           RuleFile, Rule, Match, Action, parse, load, compile, RulesError
src/rules/engine.rs        evaluate, Context, Mode, Plan
src/rules/apply.rs         apply
src/mail_ops.rs            MailOps trait, types, RecordingOps (cfg(test) + feature "testing")
src/mail_ops/imap.rs       ImapOps on async-imap
src/trash.rs               Trash
src/sync.rs                sync_folder, sync_all, run_rules, run_once, run_loop, Event
src/main.rs                clap CLI
src/cli/mod.rs             command handlers
```

---

### Task 1: Scaffold

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `.gitignore`, `LICENSE-MIT`, `LICENSE-APACHE`, `AGENTS.md`, `src/lib.rs`, `src/main.rs`
- Modify: `.mise.toml`

**Interfaces:**
- Produces: the package and the dependency set every later task uses.

- [ ] **Step 1: Write Cargo.toml**

```toml
[package]
name = "postbode"
version = "0.1.0"
edition = "2024"
rust-version = "1.91"
license = "MIT OR Apache-2.0"
description = "A fast, simple mail client with automatic mailbox rules"
repository = "https://github.com/pataar/postbode"

[dependencies]
anyhow = "1"                                         # error context in the binary and sync loop
async-imap = { version = "0.12", default-features = false, features = ["runtime-tokio"] }  # IMAP client, maintained, IDLE support
chrono = { version = "0.4", default-features = false, features = ["std", "clock"] }         # INTERNALDATE comes back as chrono; also list output
clap = { version = "4", features = ["derive"] }      # CLI
directories = "6"                                    # platform config/state/cache dirs
env_logger = "0.11"                                  # log output to stderr, RUST_LOG filter
futures-util = { version = "0.3", default-features = false }  # collect async-imap streams
globset = "0.4"                                      # alias globs
html2text = "0.17"                                   # HTML-only mail to text for body rules
humantime = "2"                                      # "1h", "2d" in rules
include_dir = "0.7"                                  # embed migrations/ in the binary
keyring-core = "1"                                   # OS keyring API
log = "0.4"                                          # logging facade
mail-parser = "0.11"                                 # RFC 5322 header and body parsing
notify-rust = "4"                                    # desktop notifications from `run`
regex = "1"                                          # rule regex conditions
rpassword = "7"                                      # hidden password prompt in `account add`
rusqlite = { version = "0.40", features = ["bundled"] }       # SQLite, bundled build includes FTS5
rusqlite_migration = { version = "2", features = ["from-directory"] }  # numbered SQL migrations
rustls = { version = "0.23", default-features = false, features = ["ring", "std", "tls12", "logging"] }  # TLS without a C toolchain
serde = { version = "1", features = ["derive"] }     # config, rules, rows
serde_json = "1"                                     # --json output
serde_rusqlite = "0.43"                              # rows to structs
thiserror = "2"                                      # user-facing error enums
tokio = { version = "1", features = ["rt", "net", "time", "macros", "sync"] }  # runtime for async-imap only
tokio-rustls = { version = "0.26", default-features = false, features = ["ring", "tls12", "logging"] }  # TLS stream for tokio
toml = "1"                                           # config.toml and rules.toml
webpki-roots = "1"                                   # root certificates
zeroize = "1"                                        # wipe secrets on drop

[target.'cfg(target_os = "macos")'.dependencies]
apple-native-keyring-store = "1"                     # macOS Keychain backend for keyring-core

[target.'cfg(target_os = "linux")'.dependencies]
zbus-secret-service-keyring-store = { version = "1", features = ["rt-tokio-crypto-rust"] }  # Secret Service backend for keyring-core

[dev-dependencies]
tempfile = "3"                                       # temp dirs for store and paths tests

[features]
testing = []                                         # exposes RecordingOps to integration tests

[profile.dev.package."*"]
opt-level = 2

[profile.release]
lto = "thin"
codegen-units = 1
strip = true
```

- [ ] **Step 2: Write rust-toolchain.toml, .mise.toml, .gitignore**

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "1.91"
components = ["rustfmt", "clippy"]
```

`.mise.toml` (replace the existing file):
```toml
[tools]
rust = { version = "1.91", components = ["rustfmt", "clippy"] }
```

`.gitignore`:
```
/target
*.db
*.db-wal
*.db-shm
.brainstorm_projects/
mise.lock
```

- [ ] **Step 3: Write licenses and AGENTS.md**

Fetch the Apache text and write the MIT text:
```bash
curl -s https://www.apache.org/licenses/LICENSE-2.0.txt -o LICENSE-APACHE
```

`LICENSE-MIT`:
```
MIT License

Copyright (c) 2026 Pieter Willekens

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

`AGENTS.md`:
```markdown
# Postbode — notes for agents

Spec: `docs/superpowers/specs/2026-10-06-postbode-core-design.md`. Read it before changing behaviour.

## Module map
- `paths` platform dirs, atomic writes
- `config` accounts and identity (address + aliases)
- `credentials` keyring or password command; `Secret` has no Debug
- `message` header parsing, thread id, body text
- `store` one SQLite file per account; migrations in `migrations/`
- `rules` parse + validate (`mod.rs`), pure `evaluate` (`engine.rs`), side effects (`apply.rs`)
- `mail_ops` `MailOps` trait; `imap.rs` is the real client; `RecordingOps` is the test fake
- `trash` `.eml` backups before any rule delete
- `sync` per-account loop: sync folders, run rules, IDLE
- `main` + `cli/` clap only; no logic

## Definition of done
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` all pass
- New logic has a test; a bug fix has a regression test
- Platform coverage reported honestly: say "compiled on" vs "ran on"
- Every new dependency has a one-line reason in `Cargo.toml`
- Do not broaden a task into adjacent features

## Privacy
- Never read message bodies from a user's store, and never log bodies or secrets
- Schema, counts and your own test fixtures are fine
```

- [ ] **Step 4: Write lib.rs and main.rs stubs**

`src/lib.rs`:
```rust
pub mod paths;
```

`src/main.rs`:
```rust
fn main() {
    println!("postbode");
}
```

`src/paths.rs` (placeholder that Task 2 replaces):
```rust
```

- [ ] **Step 5: Build and verify**

Run: `mise install && cargo build`
Expected: compiles with no errors. First build downloads and compiles all deps; it takes a few minutes.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock rust-toolchain.toml .mise.toml .gitignore LICENSE-MIT LICENSE-APACHE AGENTS.md src/ docs/
git commit -m "chore: scaffold postbode package with spec and plan"
```

---

### Task 2: Paths and atomic writes

**Files:**
- Create: `src/paths.rs`

**Interfaces:**
- Produces:
  - `pub struct Paths { pub config_dir: PathBuf, pub state_dir: PathBuf, pub cache_dir: PathBuf }`
  - `Paths::discover() -> anyhow::Result<Paths>`
  - `Paths::under(root: &Path) -> Paths`
  - `Paths::config_file(&self) -> PathBuf`, `rules_file`, `account_dir(&self, name: &str)`, `mail_db(&self, name)`, `trash_dir(&self, name)`
  - `Paths::ensure_account(&self, name: &str) -> io::Result<()>`
  - `pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()>`

- [ ] **Step 1: Write the failing tests**

```rust
// src/paths.rs
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_root_lays_out_three_dirs() {
        let root = tempfile::tempdir().unwrap();
        let p = Paths::under(root.path());
        assert_eq!(p.config_file(), root.path().join("config/config.toml"));
        assert_eq!(p.rules_file(), root.path().join("config/rules.toml"));
        assert_eq!(p.mail_db("work"), root.path().join("state/accounts/work/mail.db"));
        assert_eq!(p.trash_dir("work"), root.path().join("state/accounts/work/trash"));
    }

    #[test]
    fn ensure_account_creates_dirs() {
        let root = tempfile::tempdir().unwrap();
        let p = Paths::under(root.path());
        p.ensure_account("work").unwrap();
        assert!(p.trash_dir("work").is_dir());
        assert!(p.config_dir.is_dir());
    }

    #[test]
    fn write_atomic_replaces_content_and_leaves_no_tmp() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("a.txt");
        write_atomic(&file, b"one").unwrap();
        write_atomic(&file, b"two").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"two");
        let leftovers: Vec<_> = fs::read_dir(root.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test paths`
Expected: compile error, `Paths::under` not found.

- [ ] **Step 3: Implement**

Append below the struct:

```rust
impl Paths {
    pub fn discover() -> anyhow::Result<Paths> {
        let dirs = directories::ProjectDirs::from("", "", "postbode")
            .ok_or_else(|| anyhow::anyhow!("no home directory found"))?;
        let state_dir = dirs
            .state_dir()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| dirs.data_local_dir().to_path_buf());
        Ok(Paths {
            config_dir: dirs.config_dir().to_path_buf(),
            state_dir,
            cache_dir: dirs.cache_dir().to_path_buf(),
        })
    }

    pub fn under(root: &Path) -> Paths {
        Paths {
            config_dir: root.join("config"),
            state_dir: root.join("state"),
            cache_dir: root.join("cache"),
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn rules_file(&self) -> PathBuf {
        self.config_dir.join("rules.toml")
    }

    pub fn account_dir(&self, name: &str) -> PathBuf {
        self.state_dir.join("accounts").join(name)
    }

    pub fn mail_db(&self, name: &str) -> PathBuf {
        self.account_dir(name).join("mail.db")
    }

    pub fn trash_dir(&self, name: &str) -> PathBuf {
        self.account_dir(name).join("trash")
    }

    pub fn ensure_account(&self, name: &str) -> io::Result<()> {
        create_private_dir(&self.config_dir)?;
        create_private_dir(&self.cache_dir)?;
        create_private_dir(&self.trash_dir(name))
    }
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::other("path has no parent"))?;
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file")
    ));
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test paths`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add src/paths.rs
git commit -m "feat: platform paths and atomic file writes"
```

---

### Task 3: Config and identity

**Files:**
- Create: `src/config.rs`
- Modify: `src/lib.rs` (add `pub mod config;`)

**Interfaces:**
- Produces:
  - `pub struct Config { pub accounts: Vec<AccountConfig> }` with `Config::load(path) -> Result<Config, ConfigError>`, `Config::save(&self, path) -> Result<(), ConfigError>`, `Config::account(&self, name) -> Option<&AccountConfig>`
  - `pub struct AccountConfig { name, host, port, username, password: PasswordSource, address: Option<String>, aliases: Vec<String>, sync_interval_secs: u64, trash_retention_days: u64, notify: bool }`
  - `pub enum PasswordSource { Keyring { keyring: bool }, Command { command: String } }`
  - `AccountConfig::address(&self) -> &str`, `AccountConfig::identity(&self) -> Result<Identity, ConfigError>`
  - `pub struct Identity` with `Identity::is_me(&self, addr: &str) -> bool`
  - `pub enum ConfigError { Parse(String), Invalid(String), Io(io::Error) }`

- [ ] **Step 1: Write the failing tests**

```rust
// src/config.rs
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::paths::write_atomic;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config.toml: {0}")]
    Parse(String),
    #[error("config.toml: {0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub accounts: Vec<AccountConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountConfig {
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub username: String,
    pub password: PasswordSource,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default = "default_sync_interval")]
    pub sync_interval_secs: u64,
    #[serde(default = "default_retention")]
    pub trash_retention_days: u64,
    #[serde(default = "default_true")]
    pub notify: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PasswordSource {
    Keyring { keyring: bool },
    Command { command: String },
}

fn default_port() -> u16 { 993 }
fn default_sync_interval() -> u64 { 120 }
fn default_retention() -> u64 { 30 }
fn default_true() -> bool { true }

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[[accounts]]
name = "work"
host = "imap.example.com"
username = "pieter@example.com"
password = { keyring = true }
aliases = ["p@example.org", "*@shop.example.com"]

[[accounts]]
name = "home"
host = "mail.home.test"
port = 143
username = "pieter"
address = "pieter@home.test"
password = { command = "pass show mail/home" }
notify = false
"#;

    #[test]
    fn parses_accounts_with_defaults() {
        let cfg = Config::parse(SAMPLE).unwrap();
        let work = cfg.account("work").unwrap();
        assert_eq!(work.port, 993);
        assert_eq!(work.sync_interval_secs, 120);
        assert_eq!(work.trash_retention_days, 30);
        assert!(work.notify);
        assert!(matches!(work.password, PasswordSource::Keyring { keyring: true }));
        let home = cfg.account("home").unwrap();
        assert_eq!(home.port, 143);
        assert!(!home.notify);
        assert!(matches!(&home.password, PasswordSource::Command { command } if command == "pass show mail/home"));
    }

    #[test]
    fn address_defaults_to_username_when_it_is_an_email() {
        let cfg = Config::parse(SAMPLE).unwrap();
        assert_eq!(cfg.account("work").unwrap().address(), "pieter@example.com");
        assert_eq!(cfg.account("home").unwrap().address(), "pieter@home.test");
    }

    #[test]
    fn identity_matches_address_and_aliases_case_insensitively() {
        let cfg = Config::parse(SAMPLE).unwrap();
        let id = cfg.account("work").unwrap().identity().unwrap();
        assert!(id.is_me("Pieter@Example.com"));
        assert!(id.is_me("p@example.org"));
        assert!(id.is_me("orders@shop.example.com"));
        assert!(!id.is_me("someone@example.com"));
    }

    #[test]
    fn rejects_duplicate_and_unsafe_names() {
        let dup = SAMPLE.replace("name = \"home\"", "name = \"work\"");
        assert!(matches!(Config::parse(&dup), Err(ConfigError::Invalid(_))));
        let bad = SAMPLE.replace("name = \"home\"", "name = \"ho/me\"");
        assert!(matches!(Config::parse(&bad), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_username_without_at_and_no_address() {
        let bad = SAMPLE.replace("address = \"pieter@home.test\"\n", "");
        assert!(matches!(Config::parse(&bad), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn load_missing_file_is_empty_and_save_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = Config::load(&path).unwrap();
        assert!(cfg.accounts.is_empty());
        let cfg = Config::parse(SAMPLE).unwrap();
        cfg.save(&path).unwrap();
        let again = Config::load(&path).unwrap();
        assert_eq!(again.accounts.len(), 2);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test config`
Expected: compile error, `Config::parse` not found.

- [ ] **Step 3: Implement**

Insert between the `fn default_*` helpers and the tests:

```rust
impl Config {
    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let cfg: Config = toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let text = toml::to_string_pretty(self).map_err(|e| ConfigError::Invalid(e.to_string()))?;
        write_atomic(path, text.as_bytes())?;
        Ok(())
    }

    pub fn account(&self, name: &str) -> Option<&AccountConfig> {
        self.accounts.iter().find(|a| a.name == name)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let mut seen = std::collections::HashSet::new();
        for account in &self.accounts {
            let name = &account.name;
            let safe = !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if !safe {
                return Err(ConfigError::Invalid(format!(
                    "account name '{name}' may only contain letters, digits, '-' and '_'"
                )));
            }
            if !seen.insert(name) {
                return Err(ConfigError::Invalid(format!("duplicate account name '{name}'")));
            }
            if account.address.is_none() && !account.username.contains('@') {
                return Err(ConfigError::Invalid(format!(
                    "account '{name}': set `address` because the username is not an email address"
                )));
            }
            account.identity()?;
        }
        Ok(())
    }
}

impl AccountConfig {
    pub fn address(&self) -> &str {
        self.address.as_deref().unwrap_or(&self.username)
    }

    pub fn identity(&self) -> Result<Identity, ConfigError> {
        let mut builder = globset::GlobSetBuilder::new();
        for pattern in &self.aliases {
            let glob = globset::GlobBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .map_err(|e| ConfigError::Invalid(format!("account '{}': alias '{pattern}': {e}", self.name)))?;
            builder.add(glob);
        }
        let aliases = builder
            .build()
            .map_err(|e| ConfigError::Invalid(format!("account '{}': aliases: {e}", self.name)))?;
        Ok(Identity { address: self.address().to_ascii_lowercase(), aliases })
    }
}

#[derive(Debug, Clone)]
pub struct Identity {
    address: String,
    aliases: globset::GlobSet,
}

impl Identity {
    pub fn is_me(&self, addr: &str) -> bool {
        let addr = addr.trim().to_ascii_lowercase();
        addr == self.address || self.aliases.is_match(&addr)
    }
}
```

Add `pub mod config;` to `src/lib.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test config`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs src/lib.rs
git commit -m "feat: account config with aliases and identity matching"
```

---

### Task 4: Credentials

**Files:**
- Create: `src/credentials.rs`
- Modify: `src/lib.rs` (add `pub mod credentials;`)

**Interfaces:**
- Consumes: `AccountConfig`, `PasswordSource` from Task 3.
- Produces:
  - `pub struct Secret` (no Debug), `Secret::new(String)`, `Secret::expose(&self) -> &str`
  - `pub enum CredentialError { NotFound(String), Locked, Unavailable, CommandFailed { account: String, status: i32 }, CommandEmpty(String), CommandSpawn(String) }`
  - `pub fn resolve(account: &AccountConfig) -> Result<Secret, CredentialError>`
  - `pub fn store(account_name: &str, secret: &Secret) -> Result<(), CredentialError>`

- [ ] **Step 1: Write the failing tests**

```rust
// src/credentials.rs
use std::process::Command;
use std::sync::OnceLock;

use zeroize::Zeroizing;

use crate::config::{AccountConfig, PasswordSource};

const SERVICE: &str = "postbode";

pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: String) -> Secret {
        Secret(Zeroizing::new(value))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("no password stored for account '{0}'; run `postbode account add`")]
    NotFound(String),
    #[error("the keyring is locked or denied access")]
    Locked,
    #[error("no keyring is available on this system; use `password = {{ command = \"...\" }}`")]
    Unavailable,
    #[error("password command for account '{account}' exited with status {status}")]
    CommandFailed { account: String, status: i32 },
    #[error("password command for account '{0}' printed nothing")]
    CommandEmpty(String),
    #[error("password command could not be started: {0}")]
    CommandSpawn(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(command: &str) -> AccountConfig {
        AccountConfig {
            name: "t".into(),
            host: "h".into(),
            port: 993,
            username: "u@example.com".into(),
            password: PasswordSource::Command { command: command.into() },
            address: None,
            aliases: vec![],
            sync_interval_secs: 120,
            trash_retention_days: 30,
            notify: true,
        }
    }

    #[test]
    fn command_output_is_trimmed() {
        let secret = resolve(&account("printf 'hunter2\\n'")).unwrap();
        assert_eq!(secret.expose(), "hunter2");
    }

    #[test]
    fn empty_command_output_is_error() {
        assert!(matches!(resolve(&account("true")), Err(CredentialError::CommandEmpty(_))));
    }

    #[test]
    fn failing_command_reports_status() {
        assert!(matches!(
            resolve(&account("exit 3")),
            Err(CredentialError::CommandFailed { status: 3, .. })
        ));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test credentials`
Expected: compile error, `resolve` not found.

- [ ] **Step 3: Implement**

Insert before the tests:

```rust
pub fn resolve(account: &AccountConfig) -> Result<Secret, CredentialError> {
    match &account.password {
        PasswordSource::Command { command } => run_command(&account.name, command),
        PasswordSource::Keyring { .. } => {
            let entry = keyring_entry(&account.name)?;
            match entry.get_password() {
                Ok(password) => Ok(Secret::new(password)),
                Err(e) => Err(map_keyring_error(e, &account.name)),
            }
        }
    }
}

pub fn store(account_name: &str, secret: &Secret) -> Result<(), CredentialError> {
    let entry = keyring_entry(account_name)?;
    entry
        .set_password(secret.expose())
        .map_err(|e| map_keyring_error(e, account_name))
}

fn run_command(account_name: &str, command: &str) -> Result<Secret, CredentialError> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .map_err(|e| CredentialError::CommandSpawn(e.to_string()))?;
    if !output.status.success() {
        return Err(CredentialError::CommandFailed {
            account: account_name.to_string(),
            status: output.status.code().unwrap_or(-1),
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let trimmed = text.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return Err(CredentialError::CommandEmpty(account_name.to_string()));
    }
    Ok(Secret::new(trimmed.to_string()))
}

fn keyring_entry(account_name: &str) -> Result<keyring_core::Entry, CredentialError> {
    static STORE: OnceLock<Result<(), String>> = OnceLock::new();
    let init = STORE.get_or_init(|| platform_store().map(keyring_core::set_default_store));
    if let Err(detail) = init {
        log::debug!("keyring store init failed: {detail}");
        return Err(CredentialError::Unavailable);
    }
    keyring_core::Entry::new(SERVICE, account_name).map_err(|e| map_keyring_error(e, account_name))
}

#[cfg(target_os = "macos")]
fn platform_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>, String> {
    apple_native_keyring_store::keychain::Store::new()
        .map(|s| s as std::sync::Arc<keyring_core::CredentialStore>)
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "linux")]
fn platform_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>, String> {
    zbus_secret_service_keyring_store::Store::new()
        .map(|s| s as std::sync::Arc<keyring_core::CredentialStore>)
        .map_err(|e| e.to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>, String> {
    Err("no keyring store for this platform".to_string())
}

fn map_keyring_error(e: keyring_core::Error, account_name: &str) -> CredentialError {
    log::debug!("keyring error for '{account_name}': {e:?}");
    match e {
        keyring_core::Error::NoEntry => CredentialError::NotFound(account_name.to_string()),
        keyring_core::Error::NoStorageAccess(_) => CredentialError::Locked,
        _ => CredentialError::Unavailable,
    }
}
```

Add `pub mod credentials;` to `src/lib.rs`.

Note: `keyring_core::CredentialStore` is the type alias for `dyn CredentialStoreApi + Send + Sync`; `Store::new()` on both platform crates returns `Result<Arc<Self>>`, and the `as` cast unsizes it. If the compiler rejects the cast, write `.map(|s| { let s: Arc<keyring_core::CredentialStore> = s; s })`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test credentials`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add src/credentials.rs src/lib.rs
git commit -m "feat: credentials from keyring or password command"
```

---

### Task 5: Header parsing, thread id, body text

**Files:**
- Create: `src/message.rs`
- Modify: `src/lib.rs` (add `pub mod message;`)

**Interfaces:**
- Produces:
  - `pub struct Parsed { pub message_id: Option<String>, pub from: Option<String>, pub to: Option<String>, pub cc: Option<String>, pub delivered_to: Option<String>, pub subject: Option<String>, pub date: Option<i64>, pub in_reply_to: Option<String>, pub references: Vec<String> }`
  - `pub fn parse_headers(raw: &[u8]) -> Parsed`
  - `pub fn header_value(raw_headers: &[u8], name: &str) -> Option<String>`
  - `pub fn thread_id(parsed: &Parsed, folder: &str, uid: u32) -> String`
  - `pub fn synthetic_message_id(folder: &str, uid: u32) -> String`
  - `pub fn body_text(raw: &[u8]) -> String`
  - `pub fn bare_addresses(field: &str) -> Vec<String>`

Address fields are stored as display strings: `Name <addr>` or `addr`, comma-joined for lists. `bare_addresses` extracts the lowercase addresses back out for `equals`, `to_me` and `alias` matching.

- [ ] **Step 1: Write the failing tests**

```rust
// src/message.rs
use mail_parser::{Address, HeaderValue, MessageParser};

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Parsed {
    pub message_id: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub cc: Option<String>,
    pub delivered_to: Option<String>,
    pub subject: Option<String>,
    pub date: Option<i64>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADERS: &[u8] = b"From: Alice <alice@example.com>\r\n\
To: Bob <bob@example.com>, carol@example.com\r\n\
Cc: dave@example.com\r\n\
Delivered-To: bob@example.com\r\n\
Subject: Re: lunch\r\n\
Date: Mon, 6 Oct 2026 12:00:00 +0200\r\n\
Message-ID: <m3@example.com>\r\n\
In-Reply-To: <m2@example.com>\r\n\
References: <m1@example.com> <m2@example.com>\r\n\
List-Id: Dev <dev.lists.example.com>\r\n\
\r\n";

    #[test]
    fn parses_common_headers() {
        let p = parse_headers(HEADERS);
        assert_eq!(p.from.as_deref(), Some("Alice <alice@example.com>"));
        assert_eq!(p.to.as_deref(), Some("Bob <bob@example.com>, carol@example.com"));
        assert_eq!(p.cc.as_deref(), Some("dave@example.com"));
        assert_eq!(p.delivered_to.as_deref(), Some("bob@example.com"));
        assert_eq!(p.subject.as_deref(), Some("Re: lunch"));
        assert_eq!(p.message_id.as_deref(), Some("m3@example.com"));
        assert_eq!(p.in_reply_to.as_deref(), Some("m2@example.com"));
        assert_eq!(p.references, vec!["m1@example.com", "m2@example.com"]);
        assert_eq!(p.date, Some(1_791_280_800));
    }

    #[test]
    fn header_value_is_case_insensitive() {
        assert_eq!(header_value(HEADERS, "list-id").as_deref(), Some("Dev <dev.lists.example.com>"));
        assert_eq!(header_value(HEADERS, "X-Missing"), None);
    }

    #[test]
    fn thread_id_prefers_first_reference_then_in_reply_to_then_self() {
        let p = parse_headers(HEADERS);
        assert_eq!(thread_id(&p, "INBOX", 1), "m1@example.com");
        let no_refs = Parsed { references: vec![], ..p.clone() };
        assert_eq!(thread_id(&no_refs, "INBOX", 1), "m2@example.com");
        let root = Parsed { references: vec![], in_reply_to: None, ..p.clone() };
        assert_eq!(thread_id(&root, "INBOX", 1), "m3@example.com");
    }

    #[test]
    fn thread_id_without_message_id_is_synthetic_and_stable() {
        let p = Parsed::default();
        assert_eq!(thread_id(&p, "INBOX", 42), "42@INBOX.postbode");
        assert_eq!(thread_id(&p, "INBOX", 42), synthetic_message_id("INBOX", 42));
    }

    #[test]
    fn body_text_prefers_plain_and_falls_back_to_html() {
        let plain = b"From: a@b\r\nContent-Type: text/plain\r\n\r\nhello plain\r\n";
        assert_eq!(body_text(plain).trim(), "hello plain");
        let html = b"From: a@b\r\nContent-Type: text/html\r\n\r\n<p>hello <b>html</b></p>\r\n";
        assert!(body_text(html).contains("hello html"));
    }

    #[test]
    fn bare_addresses_extracts_lowercase_addresses() {
        assert_eq!(
            bare_addresses("Bob <Bob@Example.com>, carol@example.com"),
            vec!["bob@example.com", "carol@example.com"]
        );
        assert!(bare_addresses("").is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test message`
Expected: compile error, `parse_headers` not found.

- [ ] **Step 3: Implement**

Insert before the tests:

```rust
pub fn parse_headers(raw: &[u8]) -> Parsed {
    let Some(msg) = MessageParser::default().parse_headers(raw) else {
        return Parsed::default();
    };
    Parsed {
        message_id: msg.message_id().map(str::to_string),
        from: msg.from().map(format_address),
        to: msg.to().map(format_address),
        cc: msg.cc().map(format_address),
        delivered_to: msg.header_raw("Delivered-To").map(|v| v.trim().to_string()),
        subject: msg.subject().map(str::to_string),
        date: msg.date().map(|d| d.to_timestamp()),
        in_reply_to: text_list(msg.in_reply_to()).into_iter().next(),
        references: text_list(msg.references()),
    }
}

pub fn header_value(raw_headers: &[u8], name: &str) -> Option<String> {
    let msg = MessageParser::default().parse_headers(raw_headers)?;
    msg.header_raw(name).map(|v| v.trim().to_string())
}

pub fn synthetic_message_id(folder: &str, uid: u32) -> String {
    format!("{uid}@{folder}.postbode")
}

pub fn thread_id(parsed: &Parsed, folder: &str, uid: u32) -> String {
    parsed
        .references
        .first()
        .cloned()
        .or_else(|| parsed.in_reply_to.clone())
        .or_else(|| parsed.message_id.clone())
        .unwrap_or_else(|| synthetic_message_id(folder, uid))
}

pub fn body_text(raw: &[u8]) -> String {
    let Some(msg) = MessageParser::default().parse(raw) else {
        return String::new();
    };
    if let Some(text) = msg.body_text(0) {
        return text.into_owned();
    }
    match msg.body_html(0) {
        Some(html) => html2text::from_read(html.as_bytes(), 100).unwrap_or_default(),
        None => String::new(),
    }
}

pub fn bare_addresses(field: &str) -> Vec<String> {
    field
        .split(',')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            let addr = match (part.rfind('<'), part.rfind('>')) {
                (Some(start), Some(end)) if end > start => &part[start + 1..end],
                _ => part,
            };
            Some(addr.trim().to_ascii_lowercase())
        })
        .collect()
}

fn format_address(address: &Address<'_>) -> String {
    address
        .iter()
        .map(|a| match (a.name(), a.address()) {
            (Some(name), Some(addr)) => format!("{name} <{addr}>"),
            (None, Some(addr)) => addr.to_string(),
            (Some(name), None) => name.to_string(),
            (None, None) => String::new(),
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn text_list(value: &HeaderValue<'_>) -> Vec<String> {
    match value {
        HeaderValue::Text(t) => vec![t.to_string()],
        HeaderValue::TextList(list) => list.iter().map(|t| t.to_string()).collect(),
        _ => vec![],
    }
}
```

Add `pub mod message;` to `src/lib.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test message`
Expected: 6 passed. If `parses_common_headers` fails on `date`, compute the expected epoch for `2026-10-06T12:00:00+02:00` with `date -u -j -f '%Y-%m-%dT%H:%M:%S%z' '2026-10-06T12:00:00+0200' +%s` on macOS and fix the constant; the test checks parsing, not arithmetic.

- [ ] **Step 5: Commit**

```bash
git add src/message.rs src/lib.rs
git commit -m "feat: header parsing, thread ids and body text extraction"
```

---

### Task 6: SQLite store and migrations

**Files:**
- Create: `migrations/001-initial/up.sql`, `src/store.rs`
- Modify: `src/lib.rs` (add `pub mod store;`)

**Interfaces:**
- Consumes: `message::{parse_headers, thread_id}` is NOT called here; callers fill `Message` fields.
- Produces:
  - `pub struct Store`, `Store::open(path: &Path) -> Result<Store, StoreError>`, `Store::open_in_memory() -> Result<Store, StoreError>`
  - `pub struct Folder { pub name: String, pub uidvalidity: u32, pub last_uid: u32, pub special_use: Option<String> }`
  - `pub struct Message { pub folder: String, pub uid: u32, pub message_id: Option<String>, pub from_addr: Option<String>, pub to_addr: Option<String>, pub cc_addr: Option<String>, pub delivered_to: Option<String>, pub in_reply_to: Option<String>, pub refs: Option<String>, pub thread_id: String, pub subject: Option<String>, pub date: Option<i64>, pub internaldate: i64, pub flags: String, pub size: Option<u32>, pub headers: Vec<u8>, pub body_text: Option<String> }`
  - `Message::is_seen(&self) -> bool`
  - `pub struct LogEntry { pub id: i64, pub at: i64, pub rule_name: String, pub folder: String, pub uid: u32, pub message_id: Option<String>, pub subject: Option<String>, pub action: String, pub trash_file: Option<String> }`
  - `Store` methods: `folders`, `folder`, `upsert_folder`, `reset_folder`, `set_last_uid`, `insert_message`, `update_flags`, `remove_message`, `remove_missing`, `move_message_row`, `messages`, `messages_in_folder`, `message`, `raw`, `set_raw`, `rule_first_seen`, `log_action`, `log`, `unread_count` — signatures in Step 3.
  - `pub enum StoreError { Sqlite(rusqlite::Error), Migration(rusqlite_migration::Error), Serde(serde_rusqlite::Error), Io(io::Error) }`

- [ ] **Step 1: Write the migration**

`migrations/001-initial/up.sql`:
```sql
CREATE TABLE folders (
  name         TEXT PRIMARY KEY,
  uidvalidity  INTEGER NOT NULL,
  last_uid     INTEGER NOT NULL DEFAULT 0,
  special_use  TEXT
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
  refs          TEXT,
  thread_id     TEXT NOT NULL,
  subject       TEXT,
  date          INTEGER,
  internaldate  INTEGER NOT NULL,
  flags         TEXT NOT NULL,
  size          INTEGER,
  headers       BLOB NOT NULL,
  raw           BLOB,
  body_text     TEXT,
  PRIMARY KEY (folder, uid)
);
CREATE INDEX messages_internaldate ON messages (folder, internaldate);
CREATE INDEX messages_thread ON messages (thread_id);
CREATE INDEX messages_message_id ON messages (message_id);

CREATE VIRTUAL TABLE messages_fts USING fts5 (
  subject, from_addr, to_addr, body_text,
  content='messages', content_rowid='rowid'
);
CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN
  INSERT INTO messages_fts(rowid, subject, from_addr, to_addr, body_text)
  VALUES (new.rowid, new.subject, new.from_addr, new.to_addr, new.body_text);
END;
CREATE TRIGGER messages_ad AFTER DELETE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, subject, from_addr, to_addr, body_text)
  VALUES ('delete', old.rowid, old.subject, old.from_addr, old.to_addr, old.body_text);
END;
CREATE TRIGGER messages_au AFTER UPDATE ON messages BEGIN
  INSERT INTO messages_fts(messages_fts, rowid, subject, from_addr, to_addr, body_text)
  VALUES ('delete', old.rowid, old.subject, old.from_addr, old.to_addr, old.body_text);
  INSERT INTO messages_fts(rowid, subject, from_addr, to_addr, body_text)
  VALUES (new.rowid, new.subject, new.from_addr, new.to_addr, new.body_text);
END;

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
  trash_file  TEXT
);
```

- [ ] **Step 2: Write the failing tests**

```rust
// src/store.rs
use std::io;
use std::path::Path;

use include_dir::{Dir, include_dir};
use rusqlite::{Connection, OptionalExtension, params};
use rusqlite_migration::Migrations;
use serde::{Deserialize, Serialize};

static MIGRATIONS_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/migrations");

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database migration: {0}")]
    Migration(#[from] rusqlite_migration::Error),
    #[error("database row: {0}")]
    Serde(#[from] serde_rusqlite::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Folder {
    pub name: String,
    pub uidvalidity: u32,
    pub last_uid: u32,
    pub special_use: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub folder: String,
    pub uid: u32,
    pub message_id: Option<String>,
    pub from_addr: Option<String>,
    pub to_addr: Option<String>,
    pub cc_addr: Option<String>,
    pub delivered_to: Option<String>,
    pub in_reply_to: Option<String>,
    pub refs: Option<String>,
    pub thread_id: String,
    pub subject: Option<String>,
    pub date: Option<i64>,
    pub internaldate: i64,
    pub flags: String,
    pub size: Option<u32>,
    #[serde(skip_serializing, deserialize_with = "deserialize_blob")]
    pub headers: Vec<u8>,
    pub body_text: Option<String>,
}

impl Message {
    pub fn is_seen(&self) -> bool {
        self.flags.split(' ').any(|f| f == "\\Seen")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    #[serde(default)]
    pub id: i64,
    pub at: i64,
    pub rule_name: String,
    pub folder: String,
    pub uid: u32,
    pub message_id: Option<String>,
    pub subject: Option<String>,
    pub action: String,
    pub trash_file: Option<String>,
}

/// serde's `Vec<u8>` only accepts sequences; rusqlite offers BLOBs as bytes. Accept both.
fn deserialize_blob<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    struct BlobVisitor;
    impl<'de> serde::de::Visitor<'de> for BlobVisitor {
        type Value = Vec<u8>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("bytes")
        }
        fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Vec<u8>, E> {
            Ok(v.to_vec())
        }
        fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Vec<u8>, E> {
            Ok(v)
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u8>, A::Error> {
            let mut out = Vec::new();
            while let Some(b) = seq.next_element::<u8>()? {
                out.push(b);
            }
            Ok(out)
        }
    }
    d.deserialize_byte_buf(BlobVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(folder: &str, uid: u32, internaldate: i64) -> Message {
        Message {
            folder: folder.into(),
            uid,
            message_id: Some(format!("m{uid}@x")),
            from_addr: Some("Alice <alice@x>".into()),
            to_addr: Some("bob@x".into()),
            cc_addr: None,
            delivered_to: None,
            in_reply_to: None,
            refs: None,
            thread_id: format!("m{uid}@x"),
            subject: Some(format!("subject {uid}")),
            date: Some(internaldate),
            internaldate,
            flags: String::new(),
            size: Some(100),
            headers: b"From: alice@x\r\n\r\n".to_vec(),
            body_text: None,
        }
    }

    fn store_with_inbox() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_folder(&Folder { name: "INBOX".into(), uidvalidity: 1, last_uid: 0, special_use: None }).unwrap();
        s
    }

    #[test]
    fn migrations_are_valid() {
        Store::migrations().validate().unwrap();
    }

    #[test]
    fn open_creates_file_and_applies_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("acc").join("mail.db");
        let s = Store::open(&path).unwrap();
        assert!(path.exists());
        assert!(s.folders().unwrap().is_empty());
    }

    #[test]
    fn folder_upsert_and_reset() {
        let s = store_with_inbox();
        s.insert_message(&msg("INBOX", 1, 10)).unwrap();
        s.set_last_uid("INBOX", 1).unwrap();
        assert_eq!(s.folder("INBOX").unwrap().unwrap().last_uid, 1);
        s.reset_folder("INBOX", 2).unwrap();
        let f = s.folder("INBOX").unwrap().unwrap();
        assert_eq!((f.uidvalidity, f.last_uid), (2, 0));
        assert!(s.messages_in_folder("INBOX").unwrap().is_empty());
    }

    #[test]
    fn messages_round_trip_and_order_newest_first() {
        let s = store_with_inbox();
        s.insert_message(&msg("INBOX", 1, 10)).unwrap();
        s.insert_message(&msg("INBOX", 2, 20)).unwrap();
        let list = s.messages("INBOX", 10).unwrap();
        assert_eq!(list.iter().map(|m| m.uid).collect::<Vec<_>>(), vec![2, 1]);
        assert_eq!(list[1], msg("INBOX", 1, 10));
        assert_eq!(s.message("INBOX", 3).unwrap(), None);
    }

    #[test]
    fn flags_raw_and_removal() {
        let s = store_with_inbox();
        s.insert_message(&msg("INBOX", 1, 10)).unwrap();
        s.update_flags("INBOX", 1, "\\Seen \\Flagged").unwrap();
        assert!(s.message("INBOX", 1).unwrap().unwrap().is_seen());
        assert_eq!(s.raw("INBOX", 1).unwrap(), None);
        s.set_raw("INBOX", 1, b"raw bytes", "body words").unwrap();
        assert_eq!(s.raw("INBOX", 1).unwrap().as_deref(), Some(&b"raw bytes"[..]));
        assert_eq!(s.message("INBOX", 1).unwrap().unwrap().body_text.as_deref(), Some("body words"));
        s.remove_message("INBOX", 1).unwrap();
        assert_eq!(s.message("INBOX", 1).unwrap(), None);
    }

    #[test]
    fn remove_missing_drops_uids_not_present() {
        let s = store_with_inbox();
        for uid in 1..=4 {
            s.insert_message(&msg("INBOX", uid, uid as i64)).unwrap();
        }
        let removed = s.remove_missing("INBOX", 4, &[1, 3]).unwrap();
        assert_eq!(removed, 2);
        let left: Vec<u32> = s.messages_in_folder("INBOX").unwrap().iter().map(|m| m.uid).collect();
        assert_eq!(left, vec![1, 3]);
    }

    #[test]
    fn move_row_with_and_without_new_uid() {
        let s = store_with_inbox();
        s.upsert_folder(&Folder { name: "Archive".into(), uidvalidity: 1, last_uid: 0, special_use: Some("Archive".into()) }).unwrap();
        s.insert_message(&msg("INBOX", 1, 10)).unwrap();
        s.insert_message(&msg("INBOX", 2, 20)).unwrap();
        s.move_message_row("INBOX", 1, "Archive", Some(7)).unwrap();
        assert_eq!(s.message("Archive", 7).unwrap().unwrap().subject.as_deref(), Some("subject 1"));
        s.move_message_row("INBOX", 2, "Archive", None).unwrap();
        assert_eq!(s.message("INBOX", 2).unwrap(), None);
        assert_eq!(s.messages_in_folder("Archive").unwrap().len(), 1);
    }

    #[test]
    fn rule_first_seen_is_sticky() {
        let s = store_with_inbox();
        assert_eq!(s.rule_first_seen("purge", 100).unwrap(), 100);
        assert_eq!(s.rule_first_seen("purge", 200).unwrap(), 100);
        assert_eq!(s.rule_first_seen("other", 200).unwrap(), 200);
    }

    #[test]
    fn log_round_trip_newest_first() {
        let s = store_with_inbox();
        for i in 1..=3 {
            s.log_action(&LogEntry {
                id: 0,
                at: i,
                rule_name: "r".into(),
                folder: "INBOX".into(),
                uid: i as u32,
                message_id: None,
                subject: None,
                action: "delete".into(),
                trash_file: Some(format!("{i}.eml")),
            })
            .unwrap();
        }
        let log = s.log(2).unwrap();
        assert_eq!(log.iter().map(|e| e.at).collect::<Vec<_>>(), vec![3, 2]);
    }

    #[test]
    fn unread_count_counts_unseen() {
        let s = store_with_inbox();
        s.insert_message(&msg("INBOX", 1, 10)).unwrap();
        s.insert_message(&msg("INBOX", 2, 20)).unwrap();
        s.update_flags("INBOX", 2, "\\Seen").unwrap();
        assert_eq!(s.unread_count("INBOX").unwrap(), 1);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test store`
Expected: compile error, `Store` has no `open_in_memory`.

- [ ] **Step 4: Implement**

Insert before the tests:

```rust
pub struct Store {
    conn: Connection,
}

const MESSAGE_COLUMNS: &str = "folder, uid, message_id, from_addr, to_addr, cc_addr, delivered_to, in_reply_to, refs, thread_id, subject, date, internaldate, flags, size, headers, body_text";

impl Store {
    pub fn migrations() -> Migrations<'static> {
        Migrations::from_directory(&MIGRATIONS_DIR).expect("migrations directory is valid")
    }

    pub fn open(path: &Path) -> Result<Store, StoreError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        Store::init(conn)
    }

    pub fn open_in_memory() -> Result<Store, StoreError> {
        Store::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Store, StoreError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        Store::migrations().to_latest(&mut conn)?;
        Ok(Store { conn })
    }

    pub fn folders(&self) -> Result<Vec<Folder>, StoreError> {
        let mut stmt = self.conn.prepare("SELECT name, uidvalidity, last_uid, special_use FROM folders ORDER BY name")?;
        let rows = serde_rusqlite::from_rows::<Folder>(stmt.query([])?);
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn folder(&self, name: &str) -> Result<Option<Folder>, StoreError> {
        let mut stmt = self.conn.prepare("SELECT name, uidvalidity, last_uid, special_use FROM folders WHERE name = ?1")?;
        let mut rows = stmt.query(params![name])?;
        match rows.next()? {
            Some(row) => Ok(Some(serde_rusqlite::from_row(row)?)),
            None => Ok(None),
        }
    }

    pub fn upsert_folder(&self, folder: &Folder) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO folders (name, uidvalidity, last_uid, special_use) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET uidvalidity = excluded.uidvalidity, last_uid = excluded.last_uid, special_use = excluded.special_use",
            params![folder.name, folder.uidvalidity, folder.last_uid, folder.special_use],
        )?;
        Ok(())
    }

    pub fn reset_folder(&self, name: &str, uidvalidity: u32) -> Result<(), StoreError> {
        self.conn.execute("DELETE FROM messages WHERE folder = ?1", params![name])?;
        self.conn.execute(
            "UPDATE folders SET uidvalidity = ?2, last_uid = 0 WHERE name = ?1",
            params![name, uidvalidity],
        )?;
        Ok(())
    }

    pub fn set_last_uid(&self, name: &str, uid: u32) -> Result<(), StoreError> {
        self.conn.execute("UPDATE folders SET last_uid = ?2 WHERE name = ?1", params![name, uid])?;
        Ok(())
    }

    pub fn insert_message(&self, m: &Message) -> Result<(), StoreError> {
        self.conn.execute(
            &format!(
                "INSERT OR IGNORE INTO messages ({MESSAGE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"
            ),
            params![
                m.folder, m.uid, m.message_id, m.from_addr, m.to_addr, m.cc_addr, m.delivered_to, m.in_reply_to,
                m.refs, m.thread_id, m.subject, m.date, m.internaldate, m.flags, m.size, m.headers, m.body_text
            ],
        )?;
        Ok(())
    }

    pub fn update_flags(&self, folder: &str, uid: u32, flags: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE messages SET flags = ?3 WHERE folder = ?1 AND uid = ?2",
            params![folder, uid, flags],
        )?;
        Ok(())
    }

    pub fn remove_message(&self, folder: &str, uid: u32) -> Result<(), StoreError> {
        self.conn.execute("DELETE FROM messages WHERE folder = ?1 AND uid = ?2", params![folder, uid])?;
        Ok(())
    }

    /// Removes rows with uid <= `upto_uid` whose uid is not in `present`.
    pub fn remove_missing(&self, folder: &str, upto_uid: u32, present: &[u32]) -> Result<usize, StoreError> {
        let existing: Vec<u32> = {
            let mut stmt = self.conn.prepare("SELECT uid FROM messages WHERE folder = ?1 AND uid <= ?2")?;
            let rows = stmt.query_map(params![folder, upto_uid], |r| r.get(0))?;
            rows.collect::<Result<_, _>>()?
        };
        let present: std::collections::HashSet<u32> = present.iter().copied().collect();
        let mut removed = 0;
        for uid in existing.into_iter().filter(|u| !present.contains(u)) {
            self.remove_message(folder, uid)?;
            removed += 1;
        }
        Ok(removed)
    }

    /// Moves the row to `new_folder` under `new_uid`, or deletes it when the new uid is unknown (next sync re-adds it).
    pub fn move_message_row(&self, folder: &str, uid: u32, new_folder: &str, new_uid: Option<u32>) -> Result<(), StoreError> {
        match new_uid {
            Some(new_uid) => {
                self.conn.execute(
                    "UPDATE messages SET folder = ?3, uid = ?4 WHERE folder = ?1 AND uid = ?2",
                    params![folder, uid, new_folder, new_uid],
                )?;
                Ok(())
            }
            None => self.remove_message(folder, uid),
        }
    }

    pub fn messages(&self, folder: &str, limit: u32) -> Result<Vec<Message>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages WHERE folder = ?1 ORDER BY internaldate DESC, uid DESC LIMIT ?2"
        ))?;
        let rows = serde_rusqlite::from_rows::<Message>(stmt.query(params![folder, limit])?);
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn messages_in_folder(&self, folder: &str) -> Result<Vec<Message>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages WHERE folder = ?1 ORDER BY uid"
        ))?;
        let rows = serde_rusqlite::from_rows::<Message>(stmt.query(params![folder])?);
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn message(&self, folder: &str, uid: u32) -> Result<Option<Message>, StoreError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages WHERE folder = ?1 AND uid = ?2"
        ))?;
        let mut rows = stmt.query(params![folder, uid])?;
        match rows.next()? {
            Some(row) => Ok(Some(serde_rusqlite::from_row(row)?)),
            None => Ok(None),
        }
    }

    pub fn raw(&self, folder: &str, uid: u32) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT raw FROM messages WHERE folder = ?1 AND uid = ?2",
                params![folder, uid],
                |r| r.get::<_, Option<Vec<u8>>>(0),
            )
            .optional()?
            .flatten())
    }

    pub fn set_raw(&self, folder: &str, uid: u32, raw: &[u8], body_text: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE messages SET raw = ?3, body_text = ?4 WHERE folder = ?1 AND uid = ?2",
            params![folder, uid, raw, body_text],
        )?;
        Ok(())
    }

    /// Returns when the rule name was first seen, inserting `now` if it is new.
    pub fn rule_first_seen(&self, name: &str, now: i64) -> Result<i64, StoreError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO rules_seen (name, first_seen_at) VALUES (?1, ?2)",
            params![name, now],
        )?;
        Ok(self.conn.query_row(
            "SELECT first_seen_at FROM rules_seen WHERE name = ?1",
            params![name],
            |r| r.get(0),
        )?)
    }

    pub fn log_action(&self, entry: &LogEntry) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO rule_log (at, rule_name, folder, uid, message_id, subject, action, trash_file)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![entry.at, entry.rule_name, entry.folder, entry.uid, entry.message_id, entry.subject, entry.action, entry.trash_file],
        )?;
        Ok(())
    }

    pub fn log(&self, limit: u32) -> Result<Vec<LogEntry>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, at, rule_name, folder, uid, message_id, subject, action, trash_file FROM rule_log ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = serde_rusqlite::from_rows::<LogEntry>(stmt.query(params![limit])?);
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn unread_count(&self, folder: &str) -> Result<u32, StoreError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE folder = ?1 AND instr(flags, '\\Seen') = 0",
            params![folder],
            |r| r.get(0),
        )?)
    }
}
```

Add `pub mod store;` to `src/lib.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test store`
Expected: 10 passed. `deserialize_blob` accepts both the bytes and the sequence form; if the round trip still fails, map `Message` rows by hand with `r.get::<_, Vec<u8>>("headers")`.

- [ ] **Step 6: Commit**

```bash
git add migrations/ src/store.rs src/lib.rs
git commit -m "feat: per-account sqlite store with migrations"
```

---

### Task 7: Rules file parsing and validation

**Files:**
- Create: `src/rules/mod.rs`
- Modify: `src/lib.rs` (add `pub mod rules;`)

**Interfaces:**
- Produces:
  - `pub struct RuleFile { pub rules: Vec<Rule> }`
  - `pub struct Rule { pub name: String, pub account: Option<String>, pub folder: Option<String>, pub enabled: bool, pub proposed_by: Option<String>, pub matches: Match, pub actions: Vec<Action> }`
  - `pub struct Match { pub from, to, cc, subject, body: Option<TextMatch>, pub header: Option<HeaderMatch>, pub older_than: Option<String>, pub seen: Option<bool>, pub to_me: Option<bool>, pub alias: Option<String> }`
  - `pub struct TextMatch { pub contains: Option<String>, pub equals: Option<String>, pub regex: Option<String> }`
  - `pub struct HeaderMatch { pub name: String, pub contains, equals, regex }`
  - `pub enum Action { Delete, MarkRead, Flag, Archive, Notify, Silent, Move(String) }` with `Action::label(&self) -> String`
  - `pub struct CompiledRule { pub rule: Rule, pub first_seen_at: i64, ...matchers }` with `CompiledRule::folder(&self) -> &str` (defaults `"INBOX"`), `CompiledRule::applies_to_account(&self, account: &str) -> bool`, `CompiledRule::needs_body(&self) -> bool`
  - `pub fn parse(text: &str) -> Result<RuleFile, RulesError>`
  - `pub fn load(path: &Path) -> Result<RuleFile, RulesError>` (missing file = empty)
  - `pub fn compile(file: &RuleFile) -> Result<Vec<CompiledRule>, RulesError>` with `first_seen_at = 0`; callers set it from the store.
  - `pub enum RulesError { Parse(String), Invalid { rule: String, reason: String }, Io(io::Error) }`
  - `pub(crate) enum Matcher { Contains(String), Equals(String), Regex(regex::Regex) }` with `Matcher::is_match(&self, text: &str) -> bool`

- [ ] **Step 1: Write the failing tests**

```rust
// src/rules/mod.rs
pub mod apply;
pub mod engine;

use std::io;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum RulesError {
    #[error("rules.toml: {0}")]
    Parse(String),
    #[error("rule '{rule}': {reason}")]
    Invalid { rule: String, reason: String },
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RuleFile {
    #[serde(default)]
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_by: Option<String>,
    #[serde(rename = "match")]
    pub matches: Match,
    pub actions: Vec<Action>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Match {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<TextMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<TextMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<TextMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<TextMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<TextMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<HeaderMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub older_than: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_me: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextMatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderMatch {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Delete,
    MarkRead,
    Flag,
    Archive,
    Notify,
    Silent,
    #[serde(rename = "move")]
    Move(String),
}

impl Action {
    pub fn label(&self) -> String {
        match self {
            Action::Delete => "delete".into(),
            Action::MarkRead => "mark_read".into(),
            Action::Flag => "flag".into(),
            Action::Archive => "archive".into(),
            Action::Notify => "notify".into(),
            Action::Silent => "silent".into(),
            Action::Move(folder) => format!("move:{folder}"),
        }
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const SAMPLE: &str = r#"
[[rules]]
name = "purge sign-in codes"
account = "work"
match.from = { regex = "no-?reply@" }
match.subject = { regex = "(?i)sign.?in|verification code|magic link" }
match.older_than = "1h"
match.seen = true
actions = ["delete"]

[[rules]]
name = "github to folder"
match.header = { name = "List-Id", contains = "github.com" }
actions = [{ move = "Lists/GitHub" }, "mark_read"]

[[rules]]
name = "shop alias"
enabled = false
proposed_by = "cli:test"
match.alias = "*@shop.example.com"
actions = [{ move = "Shopping" }, "notify"]
"#;

    #[test]
    fn parses_sample() {
        let file = parse(SAMPLE).unwrap();
        assert_eq!(file.rules.len(), 3);
        let github = &file.rules[1];
        assert_eq!(github.actions, vec![Action::Move("Lists/GitHub".into()), Action::MarkRead]);
        assert_eq!(github.matches.header.as_ref().unwrap().name, "List-Id");
        assert!(!file.rules[2].enabled);
        assert_eq!(file.rules[2].proposed_by.as_deref(), Some("cli:test"));
    }

    #[test]
    fn compile_sets_defaults() {
        let compiled = compile(&parse(SAMPLE).unwrap()).unwrap();
        assert_eq!(compiled[0].folder(), "INBOX");
        assert!(compiled[0].applies_to_account("work"));
        assert!(!compiled[0].applies_to_account("home"));
        assert!(compiled[1].applies_to_account("anything"));
        assert!(!compiled[0].needs_body());
        assert_eq!(compiled[0].older_than, Some(Duration::from_secs(3600)));
    }

    #[test]
    fn rejects_bad_regex_with_rule_name() {
        let bad = SAMPLE.replace("no-?reply@", "(unclosed");
        match parse(&bad).and_then(|f| compile(&f)) {
            Err(RulesError::Invalid { rule, reason }) => {
                assert_eq!(rule, "purge sign-in codes");
                assert!(reason.contains("regex"), "{reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn rejects_text_match_without_exactly_one_operator() {
        let two = SAMPLE.replace(r#"{ regex = "no-?reply@" }"#, r#"{ regex = "a", contains = "b" }"#);
        assert!(matches!(parse(&two).and_then(|f| compile(&f)), Err(RulesError::Invalid { .. })));
        let none = SAMPLE.replace(r#"{ regex = "no-?reply@" }"#, "{ }");
        assert!(matches!(parse(&none).and_then(|f| compile(&f)), Err(RulesError::Invalid { .. })));
    }

    #[test]
    fn rejects_rule_without_conditions_or_actions_or_unknown_action() {
        let empty_match = "[[rules]]\nname = \"x\"\nmatch = {}\nactions = [\"delete\"]\n";
        assert!(matches!(parse(empty_match).and_then(|f| compile(&f)), Err(RulesError::Invalid { .. })));
        let no_actions = "[[rules]]\nname = \"x\"\nmatch.seen = true\nactions = []\n";
        assert!(matches!(parse(no_actions).and_then(|f| compile(&f)), Err(RulesError::Invalid { .. })));
        let unknown = "[[rules]]\nname = \"x\"\nmatch.seen = true\nactions = [\"explode\"]\n";
        assert!(matches!(parse(unknown), Err(RulesError::Parse(_))));
    }

    #[test]
    fn rejects_bad_duration_and_duplicate_names() {
        let bad = SAMPLE.replace("\"1h\"", "\"soon\"");
        assert!(matches!(parse(&bad).and_then(|f| compile(&f)), Err(RulesError::Invalid { .. })));
        let dup = SAMPLE.replace("name = \"github to folder\"", "name = \"purge sign-in codes\"");
        assert!(matches!(parse(&dup).and_then(|f| compile(&f)), Err(RulesError::Invalid { .. })));
    }

    #[test]
    fn load_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(&dir.path().join("rules.toml")).unwrap().rules.is_empty());
    }

    #[test]
    fn matcher_semantics() {
        assert!(Matcher::Contains("github".into()).is_match("Lists GitHub Dev"));
        assert!(Matcher::Equals("a@b.c".into()).is_match("A@B.C"));
        assert!(!Matcher::Equals("a@b.c".into()).is_match("xa@b.c"));
        assert!(Matcher::Regex(regex::Regex::new("^no-?reply").unwrap()).is_match("noreply@x"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test rules::tests`
Expected: compile error, `parse` not found. (`apply` and `engine` modules do not exist yet; create empty `src/rules/apply.rs` and `src/rules/engine.rs` files so the module declarations compile.)

- [ ] **Step 3: Implement**

Insert before the tests:

```rust
pub fn parse(text: &str) -> Result<RuleFile, RulesError> {
    toml::from_str(text).map_err(|e| RulesError::Parse(e.to_string()))
}

pub fn load(path: &Path) -> Result<RuleFile, RulesError> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(RuleFile::default()),
        Err(e) => Err(e.into()),
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Matcher {
    Contains(String),
    Equals(String),
    Regex(regex::Regex),
}

impl Matcher {
    pub(crate) fn is_match(&self, text: &str) -> bool {
        match self {
            Matcher::Contains(needle) => text.to_lowercase().contains(needle),
            Matcher::Equals(wanted) => text.trim().eq_ignore_ascii_case(wanted),
            Matcher::Regex(re) => re.is_match(text),
        }
    }

    fn build(rule: &str, field: &str, contains: &Option<String>, equals: &Option<String>, regex: &Option<String>) -> Result<Matcher, RulesError> {
        let invalid = |reason: String| RulesError::Invalid { rule: rule.to_string(), reason };
        match (contains, equals, regex) {
            (Some(c), None, None) => Ok(Matcher::Contains(c.to_lowercase())),
            (None, Some(e), None) => Ok(Matcher::Equals(e.clone())),
            (None, None, Some(r)) => regex::Regex::new(r)
                .map(Matcher::Regex)
                .map_err(|e| invalid(format!("match.{field}: invalid regex: {e}"))),
            _ => Err(invalid(format!("match.{field} needs exactly one of contains, equals, regex"))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompiledRule {
    pub rule: Rule,
    pub first_seen_at: i64,
    pub(crate) from: Option<Matcher>,
    pub(crate) to: Option<Matcher>,
    pub(crate) cc: Option<Matcher>,
    pub(crate) subject: Option<Matcher>,
    pub(crate) body: Option<Matcher>,
    pub(crate) header: Option<(String, Matcher)>,
    pub older_than: Option<Duration>,
    pub(crate) alias: Option<globset::GlobMatcher>,
}

impl CompiledRule {
    pub fn folder(&self) -> &str {
        self.rule.folder.as_deref().unwrap_or("INBOX")
    }

    pub fn applies_to_account(&self, account: &str) -> bool {
        self.rule.account.as_deref().is_none_or(|a| a == account)
    }

    pub fn needs_body(&self) -> bool {
        self.body.is_some()
    }
}

pub fn compile(file: &RuleFile) -> Result<Vec<CompiledRule>, RulesError> {
    let mut names = std::collections::HashSet::new();
    file.rules.iter().map(|rule| compile_rule(rule, &mut names)).collect()
}

fn compile_rule(rule: &Rule, names: &mut std::collections::HashSet<String>) -> Result<CompiledRule, RulesError> {
    let invalid = |reason: &str| RulesError::Invalid { rule: rule.name.clone(), reason: reason.to_string() };
    if rule.name.trim().is_empty() {
        return Err(invalid("name must not be empty"));
    }
    if !names.insert(rule.name.clone()) {
        return Err(invalid("duplicate rule name"));
    }
    if rule.actions.is_empty() {
        return Err(invalid("actions must not be empty"));
    }
    let m = &rule.matches;
    let has_condition = m.from.is_some() || m.to.is_some() || m.cc.is_some() || m.subject.is_some()
        || m.body.is_some() || m.header.is_some() || m.older_than.is_some() || m.seen.is_some()
        || m.to_me.is_some() || m.alias.is_some();
    if !has_condition {
        return Err(invalid("match needs at least one condition"));
    }
    let text = |field: &str, t: &Option<TextMatch>| -> Result<Option<Matcher>, RulesError> {
        t.as_ref()
            .map(|t| Matcher::build(&rule.name, field, &t.contains, &t.equals, &t.regex))
            .transpose()
    };
    let header = m
        .header
        .as_ref()
        .map(|h| Matcher::build(&rule.name, "header", &h.contains, &h.equals, &h.regex).map(|mm| (h.name.clone(), mm)))
        .transpose()?;
    let older_than = m
        .older_than
        .as_ref()
        .map(|s| humantime::parse_duration(s).map_err(|e| invalid(&format!("match.older_than: {e}"))))
        .transpose()?;
    let alias = m
        .alias
        .as_ref()
        .map(|pattern| {
            globset::GlobBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .map(|g| g.compile_matcher())
                .map_err(|e| invalid(&format!("match.alias: {e}")))
        })
        .transpose()?;
    Ok(CompiledRule {
        rule: rule.clone(),
        first_seen_at: 0,
        from: text("from", &m.from)?,
        to: text("to", &m.to)?,
        cc: text("cc", &m.cc)?,
        subject: text("subject", &m.subject)?,
        body: text("body", &m.body)?,
        header,
        older_than,
        alias,
    })
}
```

Add `pub mod rules;` to `src/lib.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test rules::tests`
Expected: 8 passed.

- [ ] **Step 5: Commit**

```bash
git add src/rules/ src/lib.rs
git commit -m "feat: rules.toml parsing, validation and compilation"
```

---

### Task 8: Rules engine — pure evaluate

**Files:**
- Create: `src/rules/engine.rs`

**Interfaces:**
- Consumes: `CompiledRule`, `Action`, `Matcher` (Task 7); `Message` (Task 6); `Identity` (Task 3); `message::{header_value, bare_addresses}` (Task 5).
- Produces:
  - `pub enum Mode { Normal, ApplyExisting }`
  - `pub struct Context<'a> { pub account: &'a str, pub identity: &'a Identity, pub now: i64, pub mode: Mode, pub notify_default: bool }`
  - `pub struct PlannedAction { pub rule: String, pub action: Action }`
  - `pub struct Plan { pub actions: Vec<PlannedAction>, pub notify: bool }` with `Plan::moves_or_deletes(&self) -> bool`
  - `pub fn evaluate(rules: &[CompiledRule], msg: &Message, ctx: &Context) -> Plan`
  - `pub fn folder_needs_body(rules: &[CompiledRule], account: &str, folder: &str) -> bool`

- [ ] **Step 1: Write the failing tests**

```rust
// src/rules/engine.rs
use crate::config::Identity;
use crate::message::{bare_addresses, header_value};
use crate::rules::{Action, CompiledRule};
use crate::store::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    ApplyExisting,
}

pub struct Context<'a> {
    pub account: &'a str,
    pub identity: &'a Identity,
    pub now: i64,
    pub mode: Mode,
    pub notify_default: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlannedAction {
    pub rule: String,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    pub actions: Vec<PlannedAction>,
    pub notify: bool,
}

impl Plan {
    pub fn moves_or_deletes(&self) -> bool {
        self.actions
            .iter()
            .any(|a| matches!(a.action, Action::Delete | Action::Move(_) | Action::Archive))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AccountConfig, PasswordSource};
    use crate::rules::{compile, parse};

    const HOUR: i64 = 3600;

    fn identity() -> Identity {
        AccountConfig {
            name: "work".into(),
            host: "h".into(),
            port: 993,
            username: "pieter@example.com".into(),
            password: PasswordSource::Keyring { keyring: true },
            address: None,
            aliases: vec!["*@shop.example.com".into()],
            sync_interval_secs: 120,
            trash_retention_days: 30,
            notify: true,
        }
        .identity()
        .unwrap()
    }

    fn rules(toml: &str, first_seen_at: i64) -> Vec<CompiledRule> {
        let mut compiled = compile(&parse(toml).unwrap()).unwrap();
        for r in &mut compiled {
            r.first_seen_at = first_seen_at;
        }
        compiled
    }

    fn msg(from: &str, to: &str, subject: &str, internaldate: i64, seen: bool) -> Message {
        let list_id = if from.contains("github") { "List-Id: dev <dev.github.com>\r\n" } else { "" };
        let headers = format!("From: {from}\r\nTo: {to}\r\nSubject: {subject}\r\n{list_id}\r\n");
        Message {
            folder: "INBOX".into(),
            uid: 1,
            message_id: Some("m1@x".into()),
            from_addr: Some(from.into()),
            to_addr: Some(to.into()),
            cc_addr: None,
            delivered_to: None,
            in_reply_to: None,
            refs: None,
            thread_id: "m1@x".into(),
            subject: Some(subject.into()),
            date: Some(internaldate),
            internaldate,
            flags: if seen { "\\Seen".into() } else { String::new() },
            size: None,
            headers: headers.into_bytes(),
            body_text: None,
        }
    }

    fn ctx<'a>(id: &'a Identity, now: i64) -> Context<'a> {
        Context { account: "work", identity: id, now, mode: Mode::Normal, notify_default: true }
    }

    const PURGE: &str = r#"
[[rules]]
name = "purge"
match.from = { regex = "no-?reply@" }
match.subject = { regex = "(?i)sign.?in|verification code" }
match.older_than = "1h"
match.seen = true
actions = ["delete"]
"#;

    #[test]
    fn sign_in_code_deleted_only_when_old_and_seen() {
        let id = identity();
        let rules = rules(PURGE, 0);
        let now = 10 * HOUR;
        let fresh = msg("noreply@login.example", "pieter@example.com", "Your sign-in code", now - 600, true);
        assert!(evaluate(&rules, &fresh, &ctx(&id, now)).actions.is_empty());
        let old_unseen = msg("noreply@login.example", "pieter@example.com", "Your sign-in code", now - 2 * HOUR, false);
        assert!(evaluate(&rules, &old_unseen, &ctx(&id, now)).actions.is_empty());
        let old_seen = msg("noreply@login.example", "pieter@example.com", "Your sign-in code", now - 2 * HOUR, true);
        let plan = evaluate(&rules, &old_seen, &ctx(&id, now));
        assert_eq!(plan.actions, vec![PlannedAction { rule: "purge".into(), action: Action::Delete }]);
        assert!(!plan.notify, "deleted mail is silent");
    }

    #[test]
    fn rule_ignores_mail_older_than_its_first_seen_unless_apply_existing() {
        let id = identity();
        let rules = rules(PURGE, 5 * HOUR);
        let now = 10 * HOUR;
        let before_rule = msg("noreply@x", "pieter@example.com", "sign in", 4 * HOUR, true);
        assert!(evaluate(&rules, &before_rule, &ctx(&id, now)).actions.is_empty());
        let mut c = ctx(&id, now);
        c.mode = Mode::ApplyExisting;
        assert_eq!(evaluate(&rules, &before_rule, &c).actions.len(), 1);
    }

    #[test]
    fn move_and_mark_read_chain_and_header_match() {
        let id = identity();
        let toml = r#"
[[rules]]
name = "github"
match.header = { name = "List-Id", contains = "github.com" }
actions = [{ move = "Lists/GitHub" }, "mark_read"]
"#;
        let plan = evaluate(&rules(toml, 0), &msg("bot@github.com", "pieter@example.com", "PR", 100, false), &ctx(&id, 200));
        assert_eq!(plan.actions.iter().map(|a| a.action.clone()).collect::<Vec<_>>(), vec![Action::Move("Lists/GitHub".into()), Action::MarkRead]);
        assert!(!plan.notify, "moved mail is silent by default");
    }

    #[test]
    fn delete_stops_the_chain() {
        let id = identity();
        let toml = r#"
[[rules]]
name = "first"
match.subject = { contains = "spam" }
actions = ["delete"]

[[rules]]
name = "second"
match.subject = { contains = "spam" }
actions = ["flag"]
"#;
        let plan = evaluate(&rules(toml, 0), &msg("a@x", "pieter@example.com", "SPAM offer", 100, false), &ctx(&id, 200));
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.actions[0].rule, "first");
    }

    #[test]
    fn account_and_folder_scoping() {
        let id = identity();
        let toml = r#"
[[rules]]
name = "other account"
account = "home"
match.seen = false
actions = ["flag"]

[[rules]]
name = "other folder"
folder = "Archive"
match.seen = false
actions = ["flag"]
"#;
        let plan = evaluate(&rules(toml, 0), &msg("a@x", "pieter@example.com", "s", 100, false), &ctx(&id, 200));
        assert!(plan.actions.is_empty());
    }

    #[test]
    fn to_me_and_alias() {
        let id = identity();
        let toml = r#"
[[rules]]
name = "lists"
match.to_me = false
actions = [{ move = "Lists" }]

[[rules]]
name = "shop"
match.alias = "*@shop.example.com"
actions = [{ move = "Shopping" }]
"#;
        let r = rules(toml, 0);
        let direct = msg("a@x", "Pieter <pieter@example.com>", "s", 100, false);
        assert!(evaluate(&r, &direct, &ctx(&id, 200)).actions.is_empty());
        let list = msg("a@x", "dev@lists.example", "s", 100, false);
        assert_eq!(evaluate(&r, &list, &ctx(&id, 200)).actions[0].rule, "lists");
        let shop = msg("a@x", "orders@shop.example.com", "s", 100, false);
        let plan = evaluate(&r, &shop, &ctx(&id, 200));
        assert_eq!(plan.actions.iter().map(|a| a.rule.as_str()).collect::<Vec<_>>(), vec!["shop"]);
    }

    #[test]
    fn body_rule_needs_body_and_matches_when_present() {
        let id = identity();
        let toml = r#"
[[rules]]
name = "unsubscribe"
match.body = { contains = "unsubscribe" }
actions = ["flag"]
"#;
        let r = rules(toml, 0);
        assert!(folder_needs_body(&r, "work", "INBOX"));
        assert!(!folder_needs_body(&r, "work", "Sent"));
        let mut m = msg("a@x", "pieter@example.com", "s", 100, false);
        assert!(evaluate(&r, &m, &ctx(&id, 200)).actions.is_empty());
        m.body_text = Some("Click here to UNSUBSCRIBE".into());
        assert_eq!(evaluate(&r, &m, &ctx(&id, 200)).actions.len(), 1);
    }

    #[test]
    fn notification_policy() {
        let id = identity();
        let toml = r#"
[[rules]]
name = "github"
match.header = { name = "List-Id", contains = "github.com" }
actions = [{ move = "Lists/GitHub" }, "notify"]

[[rules]]
name = "quiet"
match.subject = { contains = "newsletter" }
actions = ["silent"]
"#;
        let r = rules(toml, 0);
        let untouched = msg("a@x", "pieter@example.com", "hello", 100, false);
        assert!(evaluate(&r, &untouched, &ctx(&id, 200)).notify);
        let mut off = ctx(&id, 200);
        off.notify_default = false;
        assert!(!evaluate(&r, &untouched, &off).notify);
        let moved_but_notify = msg("bot@github.com", "pieter@example.com", "PR", 100, false);
        assert!(evaluate(&r, &moved_but_notify, &ctx(&id, 200)).notify);
        let quiet = msg("a@x", "pieter@example.com", "Weekly newsletter", 100, false);
        assert!(!evaluate(&r, &quiet, &ctx(&id, 200)).notify);
    }

    #[test]
    fn disabled_rules_are_skipped() {
        let id = identity();
        let toml = "[[rules]]\nname = \"off\"\nenabled = false\nmatch.seen = false\nactions = [\"flag\"]\n";
        assert!(evaluate(&rules(toml, 0), &msg("a@x", "p@x", "s", 100, false), &ctx(&id, 200)).actions.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test rules::engine`
Expected: compile error, `evaluate` not found.

- [ ] **Step 3: Implement**

Insert before the tests:

```rust
pub fn folder_needs_body(rules: &[CompiledRule], account: &str, folder: &str) -> bool {
    rules
        .iter()
        .any(|r| r.rule.enabled && r.applies_to_account(account) && r.folder() == folder && r.needs_body())
}

pub fn evaluate(rules: &[CompiledRule], msg: &Message, ctx: &Context) -> Plan {
    let mut plan = Plan::default();
    let mut explicit_notify: Option<bool> = None;
    for rule in rules {
        if !rule.rule.enabled || !rule.applies_to_account(ctx.account) || rule.folder() != msg.folder {
            continue;
        }
        if ctx.mode == Mode::Normal && msg.internaldate < rule.first_seen_at {
            continue;
        }
        if !matches(rule, msg, ctx) {
            continue;
        }
        let mut deleted = false;
        for action in &rule.rule.actions {
            match action {
                Action::Notify => explicit_notify = Some(true),
                Action::Silent => explicit_notify = Some(false),
                _ => plan.actions.push(PlannedAction { rule: rule.rule.name.clone(), action: action.clone() }),
            }
            if *action == Action::Delete {
                deleted = true;
            }
        }
        if deleted {
            break;
        }
    }
    plan.notify = match explicit_notify {
        Some(explicit) => explicit,
        None => ctx.notify_default && !plan.moves_or_deletes(),
    };
    plan
}

fn matches(rule: &CompiledRule, msg: &Message, ctx: &Context) -> bool {
    let field = |value: &Option<String>| value.as_deref().unwrap_or("");
    if let Some(m) = &rule.from {
        if !m.is_match(field(&msg.from_addr)) {
            return false;
        }
    }
    if let Some(m) = &rule.to {
        if !m.is_match(field(&msg.to_addr)) {
            return false;
        }
    }
    if let Some(m) = &rule.cc {
        if !m.is_match(field(&msg.cc_addr)) {
            return false;
        }
    }
    if let Some(m) = &rule.subject {
        if !m.is_match(field(&msg.subject)) {
            return false;
        }
    }
    if let Some(m) = &rule.body {
        match &msg.body_text {
            Some(body) if m.is_match(body) => {}
            _ => return false,
        }
    }
    if let Some((name, m)) = &rule.header {
        match header_value(&msg.headers, name) {
            Some(value) if m.is_match(&value) => {}
            _ => return false,
        }
    }
    if let Some(min_age) = rule.older_than {
        if ctx.now - msg.internaldate < min_age.as_secs() as i64 {
            return false;
        }
    }
    if let Some(seen) = rule.rule.matches.seen {
        if msg.is_seen() != seen {
            return false;
        }
    }
    if rule.rule.matches.to_me.is_some() || rule.alias.is_some() {
        let recipients: Vec<String> = [&msg.to_addr, &msg.cc_addr, &msg.delivered_to]
            .into_iter()
            .flat_map(|f| bare_addresses(field(f)))
            .collect();
        if let Some(to_me) = rule.rule.matches.to_me {
            if recipients.iter().any(|r| ctx.identity.is_me(r)) != to_me {
                return false;
            }
        }
        if let Some(glob) = &rule.alias {
            if !recipients.iter().any(|r| glob.is_match(r)) {
                return false;
            }
        }
    }
    true
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test rules::engine`
Expected: 9 passed.

- [ ] **Step 5: Commit**

```bash
git add src/rules/engine.rs
git commit -m "feat: pure rules evaluation with notification policy"
```

---

### Task 9: MailOps trait and recording fake

**Files:**
- Create: `src/mail_ops.rs`
- Modify: `src/lib.rs` (add `pub mod mail_ops;`)

**Interfaces:**
- Produces:
  - `pub struct RemoteFolder { pub name: String, pub special_use: Option<String> }`
  - `pub struct SelectInfo { pub uidvalidity: u32 }`
  - `pub struct Envelope { pub uid: u32, pub flags: Vec<String>, pub internaldate: i64, pub size: Option<u32>, pub headers: Vec<u8> }`
  - `pub struct FlagUpdate { pub uid: u32, pub flags: Vec<String> }`
  - `pub enum IdleOutcome { NewMail, Timeout, Interrupted }`
  - `pub enum MailError { Connect(String), Auth(String), Protocol(String), Io(String) }`
  - `pub trait MailOps` with: `list_folders`, `select(folder) -> SelectInfo`, `fetch_new(from_uid) -> Vec<Envelope>`, `fetch_flags(upto_uid) -> Vec<FlagUpdate>`, `fetch_raw(uid) -> Option<Vec<u8>>`, `add_flags(uid, &[&str])`, `remove_flags(uid, &[&str])`, `expunge(uid)`, `move_message(uid, to) -> Option<u32>`, `create_folder(name)`, `append(folder, raw)`, `idle(timeout, interrupt: &AtomicBool) -> IdleOutcome`
  - `pub struct RecordingOps` (available under `#[cfg(any(test, feature = "testing"))]`) with `RecordingOps::new()`, `pub folders: Vec<RemoteFolder>`, `pub uidvalidity: HashMap<String, u32>`, `pub mail: HashMap<String, Vec<Envelope>>` (per folder), `pub raw: HashMap<(String, u32), Vec<u8>>`, `pub supports_move: bool`, `pub calls: Vec<String>` (one line per call, e.g. `"add_flags INBOX 3 \\Deleted"`), `pub idle_outcomes: VecDeque<IdleOutcome>`.

- [ ] **Step 1: Write the trait, types and fake**

```rust
// src/mail_ops.rs
pub mod imap;

use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteFolder {
    pub name: String,
    pub special_use: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectInfo {
    pub uidvalidity: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub uid: u32,
    pub flags: Vec<String>,
    pub internaldate: i64,
    pub size: Option<u32>,
    pub headers: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FlagUpdate {
    pub uid: u32,
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleOutcome {
    NewMail,
    Timeout,
    Interrupted,
}

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("login failed: {0}")]
    Auth(String),
    #[error("server error: {0}")]
    Protocol(String),
    #[error("connection lost: {0}")]
    Io(String),
}

pub type MailResult<T> = Result<T, MailError>;

pub trait MailOps {
    fn list_folders(&mut self) -> MailResult<Vec<RemoteFolder>>;
    fn select(&mut self, folder: &str) -> MailResult<SelectInfo>;
    /// Envelopes with uid >= `from_uid` in the selected folder.
    fn fetch_new(&mut self, from_uid: u32) -> MailResult<Vec<Envelope>>;
    /// Current flags for every uid <= `upto_uid` in the selected folder.
    fn fetch_flags(&mut self, upto_uid: u32) -> MailResult<Vec<FlagUpdate>>;
    fn fetch_raw(&mut self, uid: u32) -> MailResult<Option<Vec<u8>>>;
    fn add_flags(&mut self, uid: u32, flags: &[&str]) -> MailResult<()>;
    fn remove_flags(&mut self, uid: u32, flags: &[&str]) -> MailResult<()>;
    fn expunge(&mut self, uid: u32) -> MailResult<()>;
    /// Moves the message; returns the new uid when the server reports it.
    fn move_message(&mut self, uid: u32, to: &str) -> MailResult<Option<u32>>;
    fn create_folder(&mut self, name: &str) -> MailResult<()>;
    fn append(&mut self, folder: &str, raw: &[u8]) -> MailResult<()>;
    /// Waits for new mail in the selected folder, the timeout, or `interrupt` becoming true.
    fn idle(&mut self, timeout: Duration, interrupt: &AtomicBool) -> MailResult<IdleOutcome>;
}

#[cfg(any(test, feature = "testing"))]
pub use recording::RecordingOps;

#[cfg(any(test, feature = "testing"))]
mod recording {
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::*;

    #[derive(Debug, Default)]
    pub struct RecordingOps {
        pub folders: Vec<RemoteFolder>,
        pub uidvalidity: HashMap<String, u32>,
        pub mail: HashMap<String, Vec<Envelope>>,
        pub raw: HashMap<(String, u32), Vec<u8>>,
        pub supports_move: bool,
        pub calls: Vec<String>,
        pub idle_outcomes: VecDeque<IdleOutcome>,
        selected: String,
        next_uid: HashMap<String, u32>,
    }

    impl RecordingOps {
        pub fn new() -> RecordingOps {
            RecordingOps { supports_move: true, ..Default::default() }
        }

        pub fn with_folder(mut self, name: &str, special_use: Option<&str>) -> RecordingOps {
            self.folders.push(RemoteFolder { name: name.into(), special_use: special_use.map(str::to_string) });
            self.uidvalidity.insert(name.into(), 1);
            self.mail.entry(name.into()).or_default();
            self
        }

        pub fn add_mail(&mut self, folder: &str, uid: u32, internaldate: i64, headers: &str, raw: Option<&str>) {
            self.mail.entry(folder.into()).or_default().push(Envelope {
                uid,
                flags: vec![],
                internaldate,
                size: raw.map(|r| r.len() as u32),
                headers: headers.as_bytes().to_vec(),
            });
            if let Some(raw) = raw {
                self.raw.insert((folder.into(), uid), raw.as_bytes().to_vec());
            }
            let next = self.next_uid.entry(folder.into()).or_insert(1);
            *next = (*next).max(uid + 1);
        }

        fn envelope_mut(&mut self, uid: u32) -> MailResult<&mut Envelope> {
            let folder = self.selected.clone();
            self.mail
                .get_mut(&folder)
                .and_then(|list| list.iter_mut().find(|e| e.uid == uid))
                .ok_or_else(|| MailError::Protocol(format!("no uid {uid} in {folder}")))
        }
    }

    impl MailOps for RecordingOps {
        fn list_folders(&mut self) -> MailResult<Vec<RemoteFolder>> {
            self.calls.push("list_folders".into());
            Ok(self.folders.clone())
        }

        fn select(&mut self, folder: &str) -> MailResult<SelectInfo> {
            self.calls.push(format!("select {folder}"));
            let uidvalidity = *self
                .uidvalidity
                .get(folder)
                .ok_or_else(|| MailError::Protocol(format!("no folder {folder}")))?;
            self.selected = folder.to_string();
            Ok(SelectInfo { uidvalidity })
        }

        fn fetch_new(&mut self, from_uid: u32) -> MailResult<Vec<Envelope>> {
            self.calls.push(format!("fetch_new {} {from_uid}", self.selected));
            let list = self.mail.get(&self.selected).cloned().unwrap_or_default();
            let max = list.iter().map(|e| e.uid).max().unwrap_or(0);
            // Real servers answer `N:*` with the highest message when N exceeds it.
            let out: Vec<Envelope> = if from_uid > max {
                list.into_iter().filter(|e| e.uid == max).collect()
            } else {
                list.into_iter().filter(|e| e.uid >= from_uid).collect()
            };
            Ok(out)
        }

        fn fetch_flags(&mut self, upto_uid: u32) -> MailResult<Vec<FlagUpdate>> {
            self.calls.push(format!("fetch_flags {} {upto_uid}", self.selected));
            Ok(self
                .mail
                .get(&self.selected)
                .map(|l| l.iter().filter(|e| e.uid <= upto_uid).map(|e| FlagUpdate { uid: e.uid, flags: e.flags.clone() }).collect())
                .unwrap_or_default())
        }

        fn fetch_raw(&mut self, uid: u32) -> MailResult<Option<Vec<u8>>> {
            self.calls.push(format!("fetch_raw {} {uid}", self.selected));
            Ok(self.raw.get(&(self.selected.clone(), uid)).cloned())
        }

        fn add_flags(&mut self, uid: u32, flags: &[&str]) -> MailResult<()> {
            self.calls.push(format!("add_flags {} {uid} {}", self.selected, flags.join(" ")));
            let env = self.envelope_mut(uid)?;
            for f in flags {
                if !env.flags.iter().any(|x| x == f) {
                    env.flags.push(f.to_string());
                }
            }
            Ok(())
        }

        fn remove_flags(&mut self, uid: u32, flags: &[&str]) -> MailResult<()> {
            self.calls.push(format!("remove_flags {} {uid} {}", self.selected, flags.join(" ")));
            let env = self.envelope_mut(uid)?;
            env.flags.retain(|f| !flags.contains(&f.as_str()));
            Ok(())
        }

        fn expunge(&mut self, uid: u32) -> MailResult<()> {
            self.calls.push(format!("expunge {} {uid}", self.selected));
            let folder = self.selected.clone();
            if let Some(list) = self.mail.get_mut(&folder) {
                list.retain(|e| e.uid != uid);
            }
            self.raw.remove(&(folder, uid));
            Ok(())
        }

        fn move_message(&mut self, uid: u32, to: &str) -> MailResult<Option<u32>> {
            self.calls.push(format!("move {} {uid} -> {to}", self.selected, ));
            if !self.mail.contains_key(to) {
                return Err(MailError::Protocol(format!("no folder {to}")));
            }
            let folder = self.selected.clone();
            let mut env = self.envelope_mut(uid)?.clone();
            self.mail.get_mut(&folder).unwrap().retain(|e| e.uid != uid);
            let raw = self.raw.remove(&(folder, uid));
            let next = self.next_uid.entry(to.into()).or_insert(1);
            let new_uid = *next;
            *next += 1;
            env.uid = new_uid;
            self.mail.get_mut(to).unwrap().push(env);
            if let Some(raw) = raw {
                self.raw.insert((to.into(), new_uid), raw);
            }
            Ok(if self.supports_move { Some(new_uid) } else { None })
        }

        fn create_folder(&mut self, name: &str) -> MailResult<()> {
            self.calls.push(format!("create_folder {name}"));
            self.folders.push(RemoteFolder { name: name.into(), special_use: None });
            self.uidvalidity.insert(name.into(), 1);
            self.mail.entry(name.into()).or_default();
            Ok(())
        }

        fn append(&mut self, folder: &str, raw: &[u8]) -> MailResult<()> {
            self.calls.push(format!("append {folder} {} bytes", raw.len()));
            let uid = *self.next_uid.get(folder).unwrap_or(&1);
            let end = raw.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
            self.add_mail(folder, uid, 0, &String::from_utf8_lossy(&raw[..end]), Some(&String::from_utf8_lossy(raw)));
            Ok(())
        }

        fn idle(&mut self, _timeout: Duration, interrupt: &AtomicBool) -> MailResult<IdleOutcome> {
            self.calls.push(format!("idle {}", self.selected));
            if interrupt.load(Ordering::Relaxed) {
                return Ok(IdleOutcome::Interrupted);
            }
            Ok(self.idle_outcomes.pop_front().unwrap_or(IdleOutcome::Interrupted))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_fetch_new_mimics_star_semantics() {
        let mut ops = RecordingOps::new().with_folder("INBOX", None);
        ops.add_mail("INBOX", 1, 10, "Subject: a\r\n\r\n", None);
        ops.add_mail("INBOX", 2, 20, "Subject: b\r\n\r\n", None);
        ops.select("INBOX").unwrap();
        assert_eq!(ops.fetch_new(2).unwrap().len(), 1);
        assert_eq!(ops.fetch_new(5).unwrap()[0].uid, 2);
    }

    #[test]
    fn fake_move_assigns_new_uid_and_records_call() {
        let mut ops = RecordingOps::new().with_folder("INBOX", None).with_folder("Archive", Some("Archive"));
        ops.add_mail("INBOX", 1, 10, "Subject: a\r\n\r\n", Some("raw"));
        ops.select("INBOX").unwrap();
        assert_eq!(ops.move_message(1, "Archive").unwrap(), Some(1));
        assert!(ops.mail["INBOX"].is_empty());
        assert_eq!(ops.raw.get(&("Archive".into(), 1)).unwrap(), b"raw");
        assert_eq!(ops.calls.last().unwrap(), "move INBOX 1 -> Archive");
    }
}
```

Create an empty `src/mail_ops/imap.rs` so the module compiles (Task 10 fills it). Add `pub mod mail_ops;` to `src/lib.rs`.

- [ ] **Step 2: Run tests**

Run: `cargo test mail_ops`
Expected: 2 passed.

- [ ] **Step 3: Commit**

```bash
git add src/mail_ops.rs src/mail_ops/imap.rs src/lib.rs
git commit -m "feat: MailOps trait with recording fake"
```

---

### Task 10: IMAP implementation

**Files:**
- Create: `src/mail_ops/imap.rs`

**Interfaces:**
- Consumes: `AccountConfig` (Task 3), `Secret` (Task 4), the `MailOps` trait and types (Task 9).
- Produces: `pub struct ImapOps` with `ImapOps::connect(account: &AccountConfig, secret: &Secret) -> MailResult<ImapOps>` implementing `MailOps`.

There is no unit test for this file; it's exercised by the manual round-trip test at the end of this task and by the Dovecot tests in plan 3. Keep every `async` inside this file.

- [ ] **Step 1: Implement**

```rust
// src/mail_ops/imap.rs
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_imap::Session;
use async_imap::types::{Fetch, Flag, NameAttribute};
use futures_util::TryStreamExt;
use tokio::net::TcpStream;
use tokio::runtime::Runtime;
use tokio_rustls::client::TlsStream;

use crate::config::AccountConfig;
use crate::credentials::Secret;
use crate::mail_ops::{Envelope, FlagUpdate, IdleOutcome, MailError, MailOps, MailResult, RemoteFolder, SelectInfo};

type ImapSession = Session<TlsStream<TcpStream>>;

pub struct ImapOps {
    rt: Runtime,
    session: Option<ImapSession>,
    has_move: bool,
    has_uidplus: bool,
}

impl ImapOps {
    pub fn connect(account: &AccountConfig, secret: &Secret) -> MailResult<ImapOps> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| MailError::Io(e.to_string()))?;
        let (session, has_move, has_uidplus) = rt.block_on(async {
            let tcp = TcpStream::connect((account.host.as_str(), account.port))
                .await
                .map_err(|e| MailError::Connect(e.to_string()))?;
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
            let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
            let name = rustls::pki_types::ServerName::try_from(account.host.clone())
                .map_err(|e| MailError::Connect(e.to_string()))?;
            let tls = connector.connect(name, tcp).await.map_err(|e| MailError::Connect(e.to_string()))?;
            let client = async_imap::Client::new(tls);
            let mut session = client
                .login(&account.username, secret.expose())
                .await
                .map_err(|(e, _)| MailError::Auth(e.to_string()))?;
            let caps = session.capabilities().await.map_err(proto)?;
            let has_move = caps.has_str("MOVE");
            let has_uidplus = caps.has_str("UIDPLUS");
            Ok::<_, MailError>((session, has_move, has_uidplus))
        })?;
        Ok(ImapOps { rt, session: Some(session), has_move, has_uidplus })
    }

    fn session(&mut self) -> MailResult<&mut ImapSession> {
        self.session.as_mut().ok_or_else(|| MailError::Io("session closed".into()))
    }
}

fn proto(e: async_imap::error::Error) -> MailError {
    match e {
        async_imap::error::Error::Io(e) => MailError::Io(e.to_string()),
        other => MailError::Protocol(other.to_string()),
    }
}

fn flag_to_string(flag: &Flag<'_>) -> String {
    match flag {
        Flag::Seen => "\\Seen".into(),
        Flag::Answered => "\\Answered".into(),
        Flag::Flagged => "\\Flagged".into(),
        Flag::Deleted => "\\Deleted".into(),
        Flag::Draft => "\\Draft".into(),
        Flag::Recent => "\\Recent".into(),
        Flag::MayCreate => "\\*".into(),
        Flag::Custom(s) => s.to_string(),
    }
}

fn special_use(attrs: &[NameAttribute<'_>]) -> Option<String> {
    attrs.iter().find_map(|a| match a {
        NameAttribute::Trash => Some("Trash".into()),
        NameAttribute::Sent => Some("Sent".into()),
        NameAttribute::Junk => Some("Junk".into()),
        NameAttribute::Drafts => Some("Drafts".into()),
        NameAttribute::Archive => Some("Archive".into()),
        NameAttribute::Extension(s) => match s.trim_start_matches('\\') {
            x @ ("Trash" | "Sent" | "Junk" | "Drafts" | "Archive") => Some(x.to_string()),
            _ => None,
        },
        _ => None,
    })
}

fn envelope_from(fetch: &Fetch) -> Option<Envelope> {
    Some(Envelope {
        uid: fetch.uid?,
        flags: fetch.flags().map(|f| flag_to_string(&f)).collect(),
        internaldate: fetch.internal_date().map(|d| d.timestamp()).unwrap_or(0),
        size: fetch.size,
        headers: fetch.header().map(<[u8]>::to_vec).unwrap_or_default(),
    })
}

impl MailOps for ImapOps {
    fn list_folders(&mut self) -> MailResult<Vec<RemoteFolder>> {
        let session = self.session()?;
        self.rt.block_on(async {
            let names: Vec<_> = session.list(Some(""), Some("*")).await.map_err(proto)?.try_collect().await.map_err(proto)?;
            Ok(names
                .iter()
                .filter(|n| !n.attributes().iter().any(|a| matches!(a, NameAttribute::NoSelect)))
                .map(|n| RemoteFolder { name: n.name().to_string(), special_use: special_use(n.attributes()) })
                .collect())
        })
    }

    fn select(&mut self, folder: &str) -> MailResult<SelectInfo> {
        let session = self.session()?;
        self.rt.block_on(async {
            let mailbox = session.select(folder).await.map_err(proto)?;
            Ok(SelectInfo { uidvalidity: mailbox.uid_validity.unwrap_or(0) })
        })
    }

    fn fetch_new(&mut self, from_uid: u32) -> MailResult<Vec<Envelope>> {
        let session = self.session()?;
        self.rt.block_on(async {
            let fetches: Vec<Fetch> = session
                .uid_fetch(format!("{from_uid}:*"), "(UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER])")
                .await
                .map_err(proto)?
                .try_collect()
                .await
                .map_err(proto)?;
            Ok(fetches.iter().filter_map(envelope_from).filter(|e| e.uid >= from_uid).collect())
        })
    }

    fn fetch_flags(&mut self, upto_uid: u32) -> MailResult<Vec<FlagUpdate>> {
        if upto_uid == 0 {
            return Ok(vec![]);
        }
        let session = self.session()?;
        self.rt.block_on(async {
            let fetches: Vec<Fetch> = session
                .uid_fetch(format!("1:{upto_uid}"), "(UID FLAGS)")
                .await
                .map_err(proto)?
                .try_collect()
                .await
                .map_err(proto)?;
            Ok(fetches
                .iter()
                .filter_map(|f| Some(FlagUpdate { uid: f.uid?, flags: f.flags().map(|x| flag_to_string(&x)).collect() }))
                .collect())
        })
    }

    fn fetch_raw(&mut self, uid: u32) -> MailResult<Option<Vec<u8>>> {
        let session = self.session()?;
        self.rt.block_on(async {
            let fetches: Vec<Fetch> = session
                .uid_fetch(uid.to_string(), "(UID BODY.PEEK[])")
                .await
                .map_err(proto)?
                .try_collect()
                .await
                .map_err(proto)?;
            Ok(fetches.iter().find(|f| f.uid == Some(uid)).and_then(|f| f.body().map(<[u8]>::to_vec)))
        })
    }

    fn add_flags(&mut self, uid: u32, flags: &[&str]) -> MailResult<()> {
        store_flags(self, uid, '+', flags)
    }

    fn remove_flags(&mut self, uid: u32, flags: &[&str]) -> MailResult<()> {
        store_flags(self, uid, '-', flags)
    }

    fn expunge(&mut self, uid: u32) -> MailResult<()> {
        let has_uidplus = self.has_uidplus;
        let session = self.session()?;
        self.rt.block_on(async {
            if has_uidplus {
                let _: Vec<_> = session.uid_expunge(uid.to_string()).await.map_err(proto)?.try_collect().await.map_err(proto)?;
            } else {
                let _: Vec<_> = session.expunge().await.map_err(proto)?.try_collect().await.map_err(proto)?;
            }
            Ok(())
        })
    }

    fn move_message(&mut self, uid: u32, to: &str) -> MailResult<Option<u32>> {
        if self.has_move {
            let session = self.session()?;
            self.rt.block_on(async { session.uid_mv(uid.to_string(), to).await.map_err(proto) })?;
            return Ok(None);
        }
        {
            let session = self.session()?;
            self.rt.block_on(async { session.uid_copy(uid.to_string(), to).await.map_err(proto) })?;
        }
        self.add_flags(uid, &["\\Deleted"])?;
        self.expunge(uid)?;
        Ok(None)
    }

    fn create_folder(&mut self, name: &str) -> MailResult<()> {
        let session = self.session()?;
        self.rt.block_on(async { session.create(name).await.map_err(proto) })
    }

    fn append(&mut self, folder: &str, raw: &[u8]) -> MailResult<()> {
        let session = self.session()?;
        self.rt.block_on(async { session.append(folder, None, None, raw).await.map_err(proto) })
    }

    fn idle(&mut self, timeout: Duration, interrupt: &AtomicBool) -> MailResult<IdleOutcome> {
        let session = self.session.take().ok_or_else(|| MailError::Io("session closed".into()))?;
        let (outcome, session) = self.rt.block_on(async {
            let mut handle = session.idle();
            handle.init().await.map_err(proto)?;
            let (wait, stop) = handle.wait_with_timeout(timeout.min(Duration::from_secs(29 * 60)));
            let outcome = {
                let mut wait = std::pin::pin!(wait);
                loop {
                    tokio::select! {
                        res = &mut wait => break match res.map_err(proto)? {
                            async_imap::extensions::idle::IdleResponse::NewData(_) => IdleOutcome::NewMail,
                            async_imap::extensions::idle::IdleResponse::Timeout => IdleOutcome::Timeout,
                            async_imap::extensions::idle::IdleResponse::ManualInterrupt => IdleOutcome::Interrupted,
                        },
                        _ = tokio::time::sleep(Duration::from_millis(500)) => {
                            if interrupt.load(Ordering::Relaxed) {
                                drop(stop);
                                let _ = (&mut wait).await;
                                break IdleOutcome::Interrupted;
                            }
                        }
                    }
                }
            };
            let session = handle.done().await.map_err(proto)?;
            Ok::<_, MailError>((outcome, session))
        })?;
        self.session = Some(session);
        Ok(outcome)
    }
}

fn store_flags(ops: &mut ImapOps, uid: u32, sign: char, flags: &[&str]) -> MailResult<()> {
    let session = ops.session()?;
    ops.rt.block_on(async {
        let query = format!("{sign}FLAGS.SILENT ({})", flags.join(" "));
        let _: Vec<_> = session.uid_store(uid.to_string(), query).await.map_err(proto)?.try_collect().await.map_err(proto)?;
        Ok(())
    })
}
```

Notes for the implementer:
- `session.expunge()` and `uid_expunge()` return streams that must be drained; that's what the `try_collect` into `Vec<_>` does.
- If `tokio::select!` with a pinned `wait` fights the borrow checker because `stop` must be dropped inside the loop, restructure: spawn nothing, just `let stop = Some(stop)` and `stop.take()` in the sleep branch.
- `drop(stop)` interrupts the IDLE stream; awaiting `wait` once more lets it return `ManualInterrupt` before `done()`.

- [ ] **Step 2: Compile with clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean. Fix any API mismatch against the vendored crate source in `~/.cargo/registry/src/*/async-imap-0.12.0/src/`.

- [ ] **Step 3: Manual round trip (optional, needs a real account)**

Add this ignored test at the bottom of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PasswordSource;

    /// POSTBODE_TEST_IMAP_HOST, _USER, _PASS must be set. Lists folders and selects INBOX.
    #[test]
    #[ignore]
    fn live_round_trip() {
        let account = AccountConfig {
            name: "live".into(),
            host: std::env::var("POSTBODE_TEST_IMAP_HOST").unwrap(),
            port: 993,
            username: std::env::var("POSTBODE_TEST_IMAP_USER").unwrap(),
            password: PasswordSource::Keyring { keyring: true },
            address: Some("x@example.com".into()),
            aliases: vec![],
            sync_interval_secs: 120,
            trash_retention_days: 30,
            notify: true,
        };
        let secret = Secret::new(std::env::var("POSTBODE_TEST_IMAP_PASS").unwrap());
        let mut ops = ImapOps::connect(&account, &secret).unwrap();
        let folders = ops.list_folders().unwrap();
        assert!(folders.iter().any(|f| f.name.eq_ignore_ascii_case("INBOX")));
        let info = ops.select("INBOX").unwrap();
        assert!(info.uidvalidity > 0);
        let new = ops.fetch_new(1).unwrap();
        eprintln!("{} messages, first headers {} bytes", new.len(), new.first().map(|e| e.headers.len()).unwrap_or(0));
    }
}
```

Run: `POSTBODE_TEST_IMAP_HOST=... POSTBODE_TEST_IMAP_USER=... POSTBODE_TEST_IMAP_PASS=... cargo test live_round_trip -- --ignored --nocapture`
Expected: passes against a real server. Report honestly if not run.

- [ ] **Step 4: Commit**

```bash
git add src/mail_ops/imap.rs
git commit -m "feat: IMAP MailOps on async-imap with IDLE"
```

---

### Task 11: Trash

**Files:**
- Create: `src/trash.rs`
- Modify: `src/lib.rs` (add `pub mod trash;`)

**Interfaces:**
- Produces:
  - `pub struct Trash` with `Trash::new(dir: PathBuf) -> Trash`
  - `Trash::save(&self, folder: &str, uid: u32, raw: &[u8], now: i64) -> io::Result<PathBuf>`
  - `pub struct TrashEntry { pub path: PathBuf, pub folder: String, pub uid: u32, pub saved_at: i64 }`
  - `Trash::list(&self) -> io::Result<Vec<TrashEntry>>` (newest first)
  - `Trash::purge(&self, retention_secs: i64, now: i64) -> io::Result<usize>`
  - `Trash::parse_name(name: &str) -> Option<(i64, String, u32)>`

File name: `<unix>-<folder percent-encoded>-<uid>.eml`, where `%` becomes `%25` and `/` becomes `%2F` in the folder.

- [ ] **Step 1: Write the failing tests**

```rust
// src/trash.rs
use std::io;
use std::path::{Path, PathBuf};

use crate::paths::write_atomic;

pub struct Trash {
    dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrashEntry {
    pub path: PathBuf,
    pub folder: String,
    pub uid: u32,
    pub saved_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_list_and_parse_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let trash = Trash::new(dir.path().join("trash"));
        let path = trash.save("Lists/GitHub", 7, b"raw", 1000).unwrap();
        assert_eq!(path.file_name().unwrap(), "1000-Lists%2FGitHub-7.eml");
        assert_eq!(std::fs::read(&path).unwrap(), b"raw");
        trash.save("INBOX", 3, b"raw2", 2000).unwrap();
        let list = trash.list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].saved_at, list[0].folder.as_str(), list[0].uid), (2000, "INBOX", 3));
        assert_eq!(list[1].folder, "Lists/GitHub");
    }

    #[test]
    fn purge_removes_only_old_files() {
        let dir = tempfile::tempdir().unwrap();
        let trash = Trash::new(dir.path().to_path_buf());
        trash.save("INBOX", 1, b"a", 1000).unwrap();
        trash.save("INBOX", 2, b"b", 5000).unwrap();
        assert_eq!(trash.purge(3000, 6000).unwrap(), 1);
        assert_eq!(trash.list().unwrap().len(), 1);
    }

    #[test]
    fn parse_name_rejects_garbage() {
        assert_eq!(Trash::parse_name("notes.txt"), None);
        assert_eq!(Trash::parse_name("12-INBOX-x.eml"), None);
        assert_eq!(Trash::parse_name("12-A%25B-4.eml"), Some((12, "A%B".into(), 4)));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test trash`
Expected: compile error, `Trash::new` not found.

- [ ] **Step 3: Implement**

Insert before the tests:

```rust
impl Trash {
    pub fn new(dir: PathBuf) -> Trash {
        Trash { dir }
    }

    pub fn save(&self, folder: &str, uid: u32, raw: &[u8], now: i64) -> io::Result<PathBuf> {
        let encoded = folder.replace('%', "%25").replace('/', "%2F");
        let path = self.dir.join(format!("{now}-{encoded}-{uid}.eml"));
        write_atomic(&path, raw)?;
        Ok(path)
    }

    pub fn list(&self) -> io::Result<Vec<TrashEntry>> {
        let mut entries = Vec::new();
        let dir = match std::fs::read_dir(&self.dir) {
            Ok(d) => d,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(entries),
            Err(e) => return Err(e),
        };
        for entry in dir {
            let entry = entry?;
            let name = entry.file_name();
            let Some((saved_at, folder, uid)) = name.to_str().and_then(Trash::parse_name) else {
                continue;
            };
            entries.push(TrashEntry { path: entry.path(), folder, uid, saved_at });
        }
        entries.sort_by(|a, b| b.saved_at.cmp(&a.saved_at).then(b.uid.cmp(&a.uid)));
        Ok(entries)
    }

    pub fn purge(&self, retention_secs: i64, now: i64) -> io::Result<usize> {
        let mut removed = 0;
        for entry in self.list()? {
            if now - entry.saved_at > retention_secs {
                std::fs::remove_file(&entry.path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub fn parse_name(name: &str) -> Option<(i64, String, u32)> {
        let stem = name.strip_suffix(".eml")?;
        let (ts, rest) = stem.split_once('-')?;
        let (encoded, uid) = rest.rsplit_once('-')?;
        let folder = encoded.replace("%2F", "/").replace("%25", "%");
        Some((ts.parse().ok()?, folder, uid.parse().ok()?))
    }
}

pub fn read_eml(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
}
```

Add `pub mod trash;` to `src/lib.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test trash`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add src/trash.rs src/lib.rs
git commit -m "feat: trash directory for deleted mail backups"
```

---

### Task 12: Apply actions

**Files:**
- Create: `src/rules/apply.rs`

**Interfaces:**
- Consumes: `Plan`, `PlannedAction` (Task 8); `Action` (Task 7); `Store`, `Message`, `LogEntry`, `Folder` (Task 6); `MailOps` (Task 9); `Trash` (Task 11); `message::body_text` (Task 5).
- Produces:
  - `pub enum ApplyError { Mail(MailError), Store(StoreError), Trash(io::Error), NoArchiveFolder, RawUnavailable { folder: String, uid: u32 } }`
  - `pub fn apply(plan: &Plan, msg: &Message, ops: &mut dyn MailOps, store: &Store, trash: &Trash, now: i64) -> Result<(), ApplyError>`
  - `pub fn ensure_raw(msg: &Message, ops: &mut dyn MailOps, store: &Store) -> Result<Vec<u8>, ApplyError>`

Precondition: `ops` has the message's folder selected. `apply` logs each action to `rule_log` before executing it.

- [ ] **Step 1: Write the failing tests**

```rust
// src/rules/apply.rs
use std::io;

use crate::mail_ops::{MailError, MailOps};
use crate::message::body_text;
use crate::rules::Action;
use crate::rules::engine::Plan;
use crate::store::{Folder, LogEntry, Message, Store, StoreError};
use crate::trash::Trash;

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error(transparent)]
    Mail(#[from] MailError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("could not write trash file: {0}")]
    Trash(#[from] io::Error),
    #[error("the server has no Archive folder")]
    NoArchiveFolder,
    #[error("message {folder}/{uid} could not be fetched for backup")]
    RawUnavailable { folder: String, uid: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail_ops::RecordingOps;
    use crate::rules::engine::PlannedAction;

    fn setup() -> (RecordingOps, Store, Trash, tempfile::TempDir, Message) {
        let dir = tempfile::tempdir().unwrap();
        let mut ops = RecordingOps::new().with_folder("INBOX", None).with_folder("Archive", Some("Archive"));
        ops.add_mail("INBOX", 5, 100, "Subject: hi\r\n\r\n", Some("Subject: hi\r\n\r\nbody text"));
        ops.select("INBOX").unwrap();
        let store = Store::open_in_memory().unwrap();
        store.upsert_folder(&Folder { name: "INBOX".into(), uidvalidity: 1, last_uid: 5, special_use: None }).unwrap();
        store.upsert_folder(&Folder { name: "Archive".into(), uidvalidity: 1, last_uid: 0, special_use: Some("Archive".into()) }).unwrap();
        let msg = Message {
            folder: "INBOX".into(),
            uid: 5,
            message_id: Some("m5@x".into()),
            from_addr: Some("a@x".into()),
            to_addr: None,
            cc_addr: None,
            delivered_to: None,
            in_reply_to: None,
            refs: None,
            thread_id: "m5@x".into(),
            subject: Some("hi".into()),
            date: None,
            internaldate: 100,
            flags: String::new(),
            size: None,
            headers: b"Subject: hi\r\n\r\n".to_vec(),
            body_text: None,
        };
        store.insert_message(&msg).unwrap();
        let trash = Trash::new(dir.path().join("trash"));
        (ops, store, trash, dir, msg)
    }

    fn plan(actions: Vec<Action>) -> Plan {
        Plan { actions: actions.into_iter().map(|action| PlannedAction { rule: "r".into(), action }).collect(), notify: false }
    }

    #[test]
    fn delete_backs_up_before_expunge_and_logs() {
        let (mut ops, store, trash, _dir, msg) = setup();
        apply(&plan(vec![Action::Delete]), &msg, &mut ops, &store, &trash, 500).unwrap();
        let calls = ops.calls.clone();
        let fetch = calls.iter().position(|c| c.starts_with("fetch_raw")).unwrap();
        let expunge = calls.iter().position(|c| c.starts_with("expunge")).unwrap();
        assert!(fetch < expunge);
        let saved = trash.list().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(std::fs::read(&saved[0].path).unwrap(), b"Subject: hi\r\n\r\nbody text");
        assert!(ops.mail["INBOX"].is_empty());
        assert_eq!(store.message("INBOX", 5).unwrap(), None);
        let log = store.log(10).unwrap();
        assert_eq!(log[0].action, "delete");
        assert!(log[0].trash_file.as_deref().unwrap().ends_with("-INBOX-5.eml"));
    }

    #[test]
    fn delete_is_refused_when_raw_cannot_be_fetched() {
        let (mut ops, store, trash, _dir, msg) = setup();
        ops.raw.clear();
        let err = apply(&plan(vec![Action::Delete]), &msg, &mut ops, &store, &trash, 500).unwrap_err();
        assert!(matches!(err, ApplyError::RawUnavailable { .. }));
        assert!(!ops.calls.iter().any(|c| c.starts_with("expunge")));
        assert_eq!(ops.mail["INBOX"].len(), 1);
    }

    #[test]
    fn move_creates_missing_folder_and_updates_row() {
        let (mut ops, store, trash, _dir, msg) = setup();
        apply(&plan(vec![Action::Move("Lists/GitHub".into()), Action::MarkRead]), &msg, &mut ops, &store, &trash, 500).unwrap();
        assert!(ops.calls.iter().any(|c| c == "create_folder Lists/GitHub"));
        assert!(store.folder("Lists/GitHub").unwrap().is_some());
        assert_eq!(store.message("INBOX", 5).unwrap(), None);
        let moved = store.message("Lists/GitHub", 1).unwrap().unwrap();
        assert!(moved.is_seen(), "mark_read applied after the move to the new location");
        assert!(ops.mail["Lists/GitHub"][0].flags.contains(&"\\Seen".to_string()));
    }

    #[test]
    fn move_without_move_capability_drops_row_for_resync() {
        let (mut ops, store, trash, _dir, msg) = setup();
        ops.supports_move = false;
        apply(&plan(vec![Action::Move("Archive".into())]), &msg, &mut ops, &store, &trash, 500).unwrap();
        assert_eq!(store.message("INBOX", 5).unwrap(), None);
        assert!(store.messages_in_folder("Archive").unwrap().is_empty());
    }

    #[test]
    fn archive_uses_special_use_folder_or_errors() {
        let (mut ops, store, trash, _dir, msg) = setup();
        apply(&plan(vec![Action::Archive]), &msg, &mut ops, &store, &trash, 500).unwrap();
        assert_eq!(ops.mail["Archive"].len(), 1);
        let (mut ops2, store2, trash2, _dir2, msg2) = setup();
        store2.upsert_folder(&Folder { name: "Archive".into(), uidvalidity: 1, last_uid: 0, special_use: None }).unwrap();
        assert!(matches!(apply(&plan(vec![Action::Archive]), &msg2, &mut ops2, &store2, &trash2, 500), Err(ApplyError::NoArchiveFolder)));
    }

    #[test]
    fn flag_and_mark_read_update_server_and_store() {
        let (mut ops, store, trash, _dir, msg) = setup();
        apply(&plan(vec![Action::Flag, Action::MarkRead]), &msg, &mut ops, &store, &trash, 500).unwrap();
        let m = store.message("INBOX", 5).unwrap().unwrap();
        assert_eq!(m.flags, "\\Flagged \\Seen");
        assert_eq!(store.log(10).unwrap().len(), 2);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test rules::apply`
Expected: compile error, `apply` not found.

- [ ] **Step 3: Implement**

Insert before the tests:

```rust
pub fn ensure_raw(msg: &Message, ops: &mut dyn MailOps, store: &Store) -> Result<Vec<u8>, ApplyError> {
    if let Some(raw) = store.raw(&msg.folder, msg.uid)? {
        return Ok(raw);
    }
    let raw = ops
        .fetch_raw(msg.uid)?
        .ok_or_else(|| ApplyError::RawUnavailable { folder: msg.folder.clone(), uid: msg.uid })?;
    store.set_raw(&msg.folder, msg.uid, &raw, &body_text(&raw))?;
    Ok(raw)
}

pub fn apply(plan: &Plan, msg: &Message, ops: &mut dyn MailOps, store: &Store, trash: &Trash, now: i64) -> Result<(), ApplyError> {
    // A move changes where the message lives; later actions in the same plan follow it.
    let mut current = msg.clone();
    for planned in &plan.actions {
        let mut entry = LogEntry {
            id: 0,
            at: now,
            rule_name: planned.rule.clone(),
            folder: current.folder.clone(),
            uid: current.uid,
            message_id: current.message_id.clone(),
            subject: current.subject.clone(),
            action: planned.action.label(),
            trash_file: None,
        };
        match &planned.action {
            Action::Delete => {
                let raw = ensure_raw(&current, ops, store)?;
                let path = trash.save(&current.folder, current.uid, &raw, now)?;
                entry.trash_file = Some(path.to_string_lossy().into_owned());
                store.log_action(&entry)?;
                ops.add_flags(current.uid, &["\\Deleted"])?;
                ops.expunge(current.uid)?;
                store.remove_message(&current.folder, current.uid)?;
                return Ok(());
            }
            Action::Move(target) => {
                store.log_action(&entry)?;
                current = move_to(&current, target, ops, store)?;
            }
            Action::Archive => {
                let target = store
                    .folders()?
                    .into_iter()
                    .find(|f| f.special_use.as_deref() == Some("Archive"))
                    .ok_or(ApplyError::NoArchiveFolder)?;
                store.log_action(&entry)?;
                current = move_to(&current, &target.name, ops, store)?;
            }
            Action::MarkRead => {
                store.log_action(&entry)?;
                set_flag(&mut current, "\\Seen", ops, store)?;
            }
            Action::Flag => {
                store.log_action(&entry)?;
                set_flag(&mut current, "\\Flagged", ops, store)?;
            }
            Action::Notify | Action::Silent => {}
        }
    }
    Ok(())
}

fn set_flag(current: &mut Message, flag: &str, ops: &mut dyn MailOps, store: &Store) -> Result<(), ApplyError> {
    // uid 0 means a COPY-fallback move lost track of the message; the next sync re-adds it with server flags.
    if current.uid == 0 || current.flags.split(' ').any(|f| f == flag) {
        return Ok(());
    }
    ops.add_flags(current.uid, &[flag])?;
    let mut flags: Vec<&str> = current.flags.split(' ').filter(|f| !f.is_empty()).collect();
    flags.push(flag);
    current.flags = flags.join(" ");
    store.update_flags(&current.folder, current.uid, &current.flags)?;
    Ok(())
}

/// Moves on the server and in the store. When the server does not report the new uid, the local row is
/// dropped and the next sync of the target folder re-adds it. Flags set after that point in the same plan
/// cannot be applied locally, so the function returns a message whose uid is 0 and callers skip store updates.
fn move_to(current: &Message, target: &str, ops: &mut dyn MailOps, store: &Store) -> Result<Message, ApplyError> {
    if store.folder(target)?.is_none() {
        ops.create_folder(target)?;
        store.upsert_folder(&Folder { name: target.to_string(), uidvalidity: 0, last_uid: 0, special_use: None })?;
    }
    let new_uid = ops.move_message(current.uid, target)?;
    store.move_message_row(&current.folder, current.uid, target, new_uid)?;
    let mut moved = current.clone();
    moved.folder = target.to_string();
    moved.uid = new_uid.unwrap_or(0);
    if new_uid.is_some() {
        ops.select(target)?;
    }
    Ok(moved)
}
```

After a `Move`/`Archive` whose server did not report the new uid, the remaining flag actions in the plan are no-ops; the message keeps the flags it had before the move.

Note: `move_to` re-selects the target folder when it knows the new uid so that follow-up `add_flags` calls hit the right mailbox. The sync loop re-selects its own folder before continuing (Task 13).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test rules::apply`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
git add src/rules/apply.rs
git commit -m "feat: apply rule actions with trash-first delete"
```

---

### Task 13: Sync loop

**Files:**
- Create: `src/sync.rs`
- Modify: `src/lib.rs` (add `pub mod sync;`)

**Interfaces:**
- Consumes: everything above.
- Produces:
  - `pub enum Event { NewMail { account: String, folder: String, uid: u32, from: String, subject: String }, Synced { account: String, new_messages: usize, actions: usize }, Error { account: String, message: String } }`
  - `pub struct NewMessageRef { pub folder: String, pub uid: u32 }`
  - `pub fn sync_folder(ops: &mut dyn MailOps, store: &Store, folder: &RemoteFolder) -> Result<Vec<NewMessageRef>, SyncError>`
  - `pub fn sync_all(ops: &mut dyn MailOps, store: &Store) -> Result<Vec<NewMessageRef>, SyncError>`
  - `pub struct RulesRun { pub evaluated: usize, pub actions: usize, pub notifications: Vec<Event> }`
  - `pub fn run_rules(ops: &mut dyn MailOps, store: &Store, trash: &Trash, rules: &[CompiledRule], account: &AccountConfig, identity: &Identity, new: &[NewMessageRef], mode: Mode, now: i64) -> Result<RulesRun, SyncError>`
  - `pub fn load_rules_for(store: &Store, path: &Path, now: i64) -> Result<Vec<CompiledRule>, RulesError>` (compile + fill `first_seen_at`)
  - `pub fn run_once(account: &AccountConfig, paths: &Paths, events: &Sender<Event>) -> Result<(), SyncError>`
  - `pub fn run_loop(account: AccountConfig, paths: Paths, events: Sender<Event>, shutdown: Arc<AtomicBool>)`
  - `pub fn now() -> i64`
  - `pub enum SyncError { Mail(MailError), Store(StoreError), Apply(ApplyError), Rules(RulesError), Credentials(CredentialError), Config(ConfigError), Io(io::Error) }`

- [ ] **Step 1: Write the failing tests**

```rust
// src/sync.rs
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::{AccountConfig, ConfigError, Identity};
use crate::credentials::{self, CredentialError};
use crate::mail_ops::{IdleOutcome, MailError, MailOps, RemoteFolder};
use crate::message::{body_text, parse_headers, thread_id};
use crate::paths::Paths;
use crate::rules::apply::{ApplyError, apply, ensure_raw};
use crate::rules::engine::{Context, Mode, evaluate, folder_needs_body};
use crate::rules::{CompiledRule, RulesError};
use crate::store::{Folder, Message, Store, StoreError};
use crate::trash::Trash;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Mail(#[from] MailError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Apply(#[from] ApplyError),
    #[error(transparent)]
    Rules(#[from] RulesError),
    #[error(transparent)]
    Credentials(#[from] CredentialError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    NewMail { account: String, folder: String, uid: u32, from: String, subject: String },
    Synced { account: String, new_messages: usize, actions: usize },
    Error { account: String, message: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewMessageRef {
    pub folder: String,
    pub uid: u32,
}

#[derive(Debug, Default)]
pub struct RulesRun {
    pub evaluated: usize,
    pub actions: usize,
    pub notifications: Vec<Event>,
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PasswordSource;
    use crate::mail_ops::RecordingOps;
    use crate::rules::{compile, parse};

    const H: i64 = 3600;

    fn account() -> AccountConfig {
        AccountConfig {
            name: "work".into(),
            host: "h".into(),
            port: 993,
            username: "pieter@example.com".into(),
            password: PasswordSource::Keyring { keyring: true },
            address: None,
            aliases: vec![],
            sync_interval_secs: 120,
            trash_retention_days: 30,
            notify: true,
        }
    }

    fn headers(from: &str, subject: &str, id: &str) -> String {
        format!("From: {from}\r\nTo: pieter@example.com\r\nSubject: {subject}\r\nMessage-ID: <{id}>\r\n\r\n")
    }

    fn ops_with_inbox() -> RecordingOps {
        let mut ops = RecordingOps::new().with_folder("INBOX", None).with_folder("Trash", Some("Trash"));
        ops.add_mail("INBOX", 1, 10 * H, &headers("alice@x", "hello", "m1@x"), Some("From: alice@x\r\n\r\nhi"));
        ops.add_mail("INBOX", 2, 11 * H, &headers("noreply@login.x", "Your sign-in code", "m2@x"), Some("From: noreply@login.x\r\n\r\ncode 1234"));
        ops
    }

    fn rules_from(toml: &str, store: &Store, now: i64) -> Vec<CompiledRule> {
        let mut compiled = compile(&parse(toml).unwrap()).unwrap();
        for r in &mut compiled {
            r.first_seen_at = store.rule_first_seen(&r.rule.name, now).unwrap();
        }
        compiled
    }

    #[test]
    fn first_sync_inserts_folders_and_messages() {
        let mut ops = ops_with_inbox();
        let store = Store::open_in_memory().unwrap();
        let new = sync_all(&mut ops, &store).unwrap();
        assert_eq!(new.len(), 2);
        let inbox = store.folder("INBOX").unwrap().unwrap();
        assert_eq!((inbox.uidvalidity, inbox.last_uid), (1, 2));
        assert_eq!(store.folder("Trash").unwrap().unwrap().special_use.as_deref(), Some("Trash"));
        let m = store.message("INBOX", 2).unwrap().unwrap();
        assert_eq!(m.subject.as_deref(), Some("Your sign-in code"));
        assert_eq!(m.thread_id, "m2@x");
        assert_eq!(m.internaldate, 11 * H);
    }

    #[test]
    fn second_sync_only_fetches_new_and_updates_flags_and_removals() {
        let mut ops = ops_with_inbox();
        let store = Store::open_in_memory().unwrap();
        sync_all(&mut ops, &store).unwrap();
        ops.select("INBOX").unwrap();
        ops.add_flags(1, &["\\Seen"]).unwrap();
        ops.expunge(2).unwrap();
        ops.add_mail("INBOX", 3, 12 * H, &headers("bob@x", "new", "m3@x"), None);
        ops.calls.clear();
        let new = sync_all(&mut ops, &store).unwrap();
        assert_eq!(new, vec![NewMessageRef { folder: "INBOX".into(), uid: 3 }]);
        assert!(store.message("INBOX", 1).unwrap().unwrap().is_seen());
        assert_eq!(store.message("INBOX", 2).unwrap(), None);
        assert!(ops.calls.iter().any(|c| c == "fetch_new INBOX 3"));
    }

    #[test]
    fn fetch_new_ignores_uids_below_last_uid() {
        let mut ops = ops_with_inbox();
        let store = Store::open_in_memory().unwrap();
        sync_all(&mut ops, &store).unwrap();
        let new = sync_all(&mut ops, &store).unwrap();
        assert!(new.is_empty(), "server answered 3:* with uid 2, which must not count as new");
    }

    #[test]
    fn uidvalidity_change_wipes_and_resyncs() {
        let mut ops = ops_with_inbox();
        let store = Store::open_in_memory().unwrap();
        sync_all(&mut ops, &store).unwrap();
        ops.uidvalidity.insert("INBOX".into(), 9);
        let new = sync_all(&mut ops, &store).unwrap();
        assert_eq!(store.folder("INBOX").unwrap().unwrap().uidvalidity, 9);
        assert_eq!(new.len(), 2);
        assert_eq!(store.messages_in_folder("INBOX").unwrap().len(), 2);
    }

    #[test]
    fn rules_run_deletes_old_seen_code_and_notifies_untouched_new_mail() {
        let mut ops = ops_with_inbox();
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory().unwrap();
        let trash = Trash::new(dir.path().to_path_buf());
        let acc = account();
        let identity = acc.identity().unwrap();
        let toml = "[[rules]]\nname = \"purge\"\nmatch.from = { contains = \"noreply@\" }\nmatch.older_than = \"1h\"\nmatch.seen = true\nactions = [\"delete\"]\n";
        let rules = rules_from(toml, &store, 0);
        let new = sync_all(&mut ops, &store).unwrap();
        ops.select("INBOX").unwrap();
        ops.add_flags(2, &["\\Seen"]).unwrap();
        sync_all(&mut ops, &store).unwrap();
        let run = run_rules(&mut ops, &store, &trash, &rules, &acc, &identity, &new, Mode::Normal, 13 * H).unwrap();
        assert_eq!(run.actions, 1);
        assert_eq!(store.message("INBOX", 2).unwrap(), None);
        assert_eq!(trash.list().unwrap().len(), 1);
        assert_eq!(run.notifications.len(), 1);
        assert!(matches!(&run.notifications[0], Event::NewMail { uid: 1, from, .. } if from == "alice@x"));
    }

    #[test]
    fn resync_after_uidvalidity_change_does_not_notify() {
        let mut ops = ops_with_inbox();
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory().unwrap();
        let trash = Trash::new(dir.path().to_path_buf());
        let acc = account();
        let identity = acc.identity().unwrap();
        let first = sync_all(&mut ops, &store).unwrap();
        let run = run_rules(&mut ops, &store, &trash, &[], &acc, &identity, &first, Mode::Normal, 12 * H).unwrap();
        assert_eq!(run.notifications.len(), 2, "first sync notifies");
        ops.uidvalidity.insert("INBOX".into(), 9);
        let resync = sync_all(&mut ops, &store).unwrap();
        let run = run_rules(&mut ops, &store, &trash, &[], &acc, &identity, &resync, Mode::Normal, 12 * H).unwrap();
        assert!(run.notifications.is_empty(), "already-known message ids never notify again");
    }

    #[test]
    fn body_rule_fetches_bodies_for_new_messages() {
        let mut ops = ops_with_inbox();
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory().unwrap();
        let trash = Trash::new(dir.path().to_path_buf());
        let acc = account();
        let identity = acc.identity().unwrap();
        let toml = "[[rules]]\nname = \"codes\"\nmatch.body = { contains = \"code\" }\nactions = [\"flag\"]\n";
        let rules = rules_from(toml, &store, 0);
        let new = sync_all(&mut ops, &store).unwrap();
        let run = run_rules(&mut ops, &store, &trash, &rules, &acc, &identity, &new, Mode::Normal, 12 * H).unwrap();
        assert_eq!(run.actions, 1);
        assert!(store.message("INBOX", 2).unwrap().unwrap().body_text.is_some());
        assert!(store.message("INBOX", 2).unwrap().unwrap().flags.contains("\\Flagged"));
    }

    #[test]
    fn invalid_rules_file_keeps_previous_rules() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_in_memory().unwrap();
        let path = dir.path().join("rules.toml");
        std::fs::write(&path, "[[rules]]\nname = \"a\"\nmatch.seen = true\nactions = [\"flag\"]\n").unwrap();
        let good = load_rules_for(&store, &path, 100).unwrap();
        assert_eq!(good.len(), 1);
        assert_eq!(good[0].first_seen_at, 100);
        std::fs::write(&path, "[[rules]]\nname = \"a\"\nmatch.from = { regex = \"(\" }\nactions = [\"flag\"]\n").unwrap();
        let current = reload_rules(&store, &path, 200, good.clone());
        assert_eq!(current.len(), 1);
        assert!(current[0].rule.matches.seen.is_some(), "previous rules kept after a bad edit");
    }

    #[test]
    fn run_loop_stops_on_shutdown_after_idle() {
        let mut ops = ops_with_inbox();
        ops.idle_outcomes.push_back(IdleOutcome::NewMail);
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        let (tx, rx) = std::sync::mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let stop = shutdown.clone();
        let mut connects = 0;
        run_loop_with(account(), paths, tx, shutdown, || {
            connects += 1;
            Ok(Box::new(std::mem::replace(&mut ops, RecordingOps::new())) as Box<dyn MailOps>)
        }, |_| stop.store(true, Ordering::Relaxed));
        assert_eq!(connects, 1);
        let events: Vec<Event> = rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(e, Event::Synced { .. })));
        assert!(events.iter().any(|e| matches!(e, Event::NewMail { .. })));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test sync`
Expected: compile error, `sync_all` not found.

- [ ] **Step 3: Implement**

Insert before the tests:

```rust
pub fn sync_folder(ops: &mut dyn MailOps, store: &Store, folder: &RemoteFolder) -> Result<Vec<NewMessageRef>, SyncError> {
    let info = ops.select(&folder.name)?;
    let mut local = match store.folder(&folder.name)? {
        Some(f) if f.uidvalidity == info.uidvalidity => f,
        Some(f) => {
            log::info!("{}: UIDVALIDITY changed {} -> {}, resyncing", folder.name, f.uidvalidity, info.uidvalidity);
            store.reset_folder(&folder.name, info.uidvalidity)?;
            Folder { uidvalidity: info.uidvalidity, last_uid: 0, ..f }
        }
        None => Folder { name: folder.name.clone(), uidvalidity: info.uidvalidity, last_uid: 0, special_use: folder.special_use.clone() },
    };
    local.special_use = folder.special_use.clone();
    store.upsert_folder(&local)?;

    if local.last_uid > 0 {
        let updates = ops.fetch_flags(local.last_uid)?;
        let present: Vec<u32> = updates.iter().map(|u| u.uid).collect();
        for u in &updates {
            store.update_flags(&folder.name, u.uid, &u.flags.join(" "))?;
        }
        store.remove_missing(&folder.name, local.last_uid, &present)?;
    }

    let mut new = Vec::new();
    let mut highest = local.last_uid;
    for env in ops.fetch_new(local.last_uid + 1)? {
        if env.uid <= local.last_uid {
            continue;
        }
        let parsed = parse_headers(&env.headers);
        let message = Message {
            folder: folder.name.clone(),
            uid: env.uid,
            thread_id: thread_id(&parsed, &folder.name, env.uid),
            message_id: parsed.message_id,
            from_addr: parsed.from,
            to_addr: parsed.to,
            cc_addr: parsed.cc,
            delivered_to: parsed.delivered_to,
            in_reply_to: parsed.in_reply_to,
            refs: if parsed.references.is_empty() { None } else { Some(parsed.references.join(" ")) },
            subject: parsed.subject,
            date: parsed.date,
            internaldate: env.internaldate,
            flags: env.flags.join(" "),
            size: env.size,
            headers: env.headers,
            body_text: None,
        };
        store.insert_message(&message)?;
        highest = highest.max(env.uid);
        new.push(NewMessageRef { folder: folder.name.clone(), uid: env.uid });
    }
    if highest != local.last_uid {
        store.set_last_uid(&folder.name, highest)?;
    }
    Ok(new)
}

pub fn sync_all(ops: &mut dyn MailOps, store: &Store) -> Result<Vec<NewMessageRef>, SyncError> {
    let mut new = Vec::new();
    for folder in ops.list_folders()? {
        new.extend(sync_folder(ops, store, &folder)?);
    }
    Ok(new)
}

pub fn load_rules_for(store: &Store, path: &Path, now: i64) -> Result<Vec<CompiledRule>, RulesError> {
    let file = crate::rules::load(path)?;
    let mut compiled = crate::rules::compile(&file)?;
    for rule in &mut compiled {
        rule.first_seen_at = store.rule_first_seen(&rule.rule.name, now).map_err(|e| RulesError::Parse(e.to_string()))?;
    }
    Ok(compiled)
}

/// Reloads the rules file; on any error logs it and returns `previous` unchanged.
pub fn reload_rules(store: &Store, path: &Path, now: i64, previous: Vec<CompiledRule>) -> Vec<CompiledRule> {
    match load_rules_for(store, path, now) {
        Ok(rules) => rules,
        Err(e) => {
            log::error!("{e}; keeping the previous rules");
            previous
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_rules(
    ops: &mut dyn MailOps,
    store: &Store,
    trash: &Trash,
    rules: &[CompiledRule],
    account: &AccountConfig,
    identity: &Identity,
    new: &[NewMessageRef],
    mode: Mode,
    now: i64,
) -> Result<RulesRun, SyncError> {
    let ctx = Context { account: &account.name, identity, now, mode, notify_default: account.notify };
    let mut folders: Vec<String> = rules
        .iter()
        .filter(|r| r.rule.enabled && r.applies_to_account(&account.name))
        .map(|r| r.folder().to_string())
        .collect();
    folders.push("INBOX".to_string());
    folders.sort();
    folders.dedup();

    let mut run = RulesRun::default();
    for folder in folders {
        if store.folder(&folder)?.is_none() {
            continue;
        }
        ops.select(&folder)?;
        let needs_body = folder_needs_body(rules, &account.name, &folder);
        for mut msg in store.messages_in_folder(&folder)? {
            let is_new = new.iter().any(|n| n.folder == msg.folder && n.uid == msg.uid);
            if needs_body && msg.body_text.is_none() && is_new {
                match ensure_raw(&msg, ops, store) {
                    Ok(raw) => msg.body_text = Some(body_text(&raw)),
                    Err(e) => log::warn!("{}/{}: body fetch failed: {e}", msg.folder, msg.uid),
                }
            }
            let plan = evaluate(rules, &msg, &ctx);
            run.evaluated += 1;
            if !plan.actions.is_empty() {
                run.actions += plan.actions.len();
                apply(&plan, &msg, ops, store, trash, now)?;
                ops.select(&folder)?;
            }
            if is_new && plan.notify && folder == "INBOX" && mode == Mode::Normal && !already_notified(store, &msg)? {
                run.notifications.push(Event::NewMail {
                    account: account.name.clone(),
                    folder: msg.folder.clone(),
                    uid: msg.uid,
                    from: msg.from_addr.clone().unwrap_or_default(),
                    subject: msg.subject.clone().unwrap_or_default(),
                });
                mark_notified(store, &msg)?;
            }
        }
    }
    Ok(run)
}

/// Notification bookkeeping lives in `rule_log` with rule_name "notify", keyed on message_id, so a
/// resync after a UIDVALIDITY change never re-notifies.
fn already_notified(store: &Store, msg: &Message) -> Result<bool, SyncError> {
    let Some(id) = &msg.message_id else { return Ok(false) };
    Ok(store.log(10_000)?.iter().any(|e| e.rule_name == "notify" && e.message_id.as_deref() == Some(id)))
}

fn mark_notified(store: &Store, msg: &Message) -> Result<(), SyncError> {
    store.log_action(&crate::store::LogEntry {
        id: 0,
        at: now(),
        rule_name: "notify".into(),
        folder: msg.folder.clone(),
        uid: msg.uid,
        message_id: msg.message_id.clone(),
        subject: msg.subject.clone(),
        action: "notify".into(),
        trash_file: None,
    })?;
    Ok(())
}

pub fn run_once(account: &AccountConfig, paths: &Paths, events: &Sender<Event>) -> Result<(), SyncError> {
    let secret = credentials::resolve(account)?;
    let mut ops = crate::mail_ops::imap::ImapOps::connect(account, &secret)?;
    paths.ensure_account(&account.name)?;
    let store = Store::open(&paths.mail_db(&account.name))?;
    let trash = Trash::new(paths.trash_dir(&account.name));
    let identity = account.identity()?;
    let rules = load_rules_for(&store, &paths.rules_file(), now())?;
    let new = sync_all(&mut ops, &store)?;
    let run = run_rules(&mut ops, &store, &trash, &rules, account, &identity, &new, Mode::Normal, now())?;
    for n in run.notifications {
        let _ = events.send(n);
    }
    let _ = events.send(Event::Synced { account: account.name.clone(), new_messages: new.len(), actions: run.actions });
    Ok(())
}

pub fn run_loop(account: AccountConfig, paths: Paths, events: Sender<Event>, shutdown: Arc<AtomicBool>) {
    let name = account.name.clone();
    run_loop_with(
        account.clone(),
        paths,
        events,
        shutdown,
        move || {
            let secret = credentials::resolve(&account)?;
            Ok(Box::new(crate::mail_ops::imap::ImapOps::connect(&account, &secret)?) as Box<dyn MailOps>)
        },
        |delay| {
            log::warn!("{name}: reconnecting in {}s", delay.as_secs());
            std::thread::sleep(delay);
        },
    );
}

/// The loop body, with connection and back-off sleeping injected so tests can drive it with the fake.
pub fn run_loop_with(
    account: AccountConfig,
    paths: Paths,
    events: Sender<Event>,
    shutdown: Arc<AtomicBool>,
    mut connect: impl FnMut() -> Result<Box<dyn MailOps>, SyncError>,
    mut sleep: impl FnMut(Duration),
) {
    let mut backoff = Duration::from_secs(5);
    while !shutdown.load(Ordering::Relaxed) {
        match run_session(&account, &paths, &events, &shutdown, &mut connect) {
            Ok(()) => return,
            Err(e) => {
                let _ = events.send(Event::Error { account: account.name.clone(), message: e.to_string() });
                sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(300));
            }
        }
    }
}

fn run_session(
    account: &AccountConfig,
    paths: &Paths,
    events: &Sender<Event>,
    shutdown: &AtomicBool,
    connect: &mut impl FnMut() -> Result<Box<dyn MailOps>, SyncError>,
) -> Result<(), SyncError> {
    let mut ops = connect()?;
    paths.ensure_account(&account.name)?;
    let store = Store::open(&paths.mail_db(&account.name))?;
    let trash = Trash::new(paths.trash_dir(&account.name));
    let identity = account.identity()?;
    let rules_path = paths.rules_file();
    let mut rules = load_rules_for(&store, &rules_path, now())?;
    let interval = Duration::from_secs(account.sync_interval_secs.max(10));
    let mut last_purge = 0i64;
    let mut full = true;

    while !shutdown.load(Ordering::Relaxed) {
        let new = if full {
            sync_all(ops.as_mut(), &store)?
        } else {
            let inbox = RemoteFolder { name: "INBOX".into(), special_use: None };
            sync_folder(ops.as_mut(), &store, &inbox)?
        };
        rules = reload_rules(&store, &rules_path, now(), rules);
        let run = run_rules(ops.as_mut(), &store, &trash, &rules, account, &identity, &new, Mode::Normal, now())?;
        for n in run.notifications {
            let _ = events.send(n);
        }
        let _ = events.send(Event::Synced { account: account.name.clone(), new_messages: new.len(), actions: run.actions });
        if now() - last_purge > 3600 {
            let removed = trash.purge(account.trash_retention_days as i64 * 86_400, now())?;
            if removed > 0 {
                log::info!("{}: purged {removed} trash files", account.name);
            }
            last_purge = now();
        }
        ops.select("INBOX")?;
        full = match ops.idle(interval, shutdown)? {
            IdleOutcome::NewMail => false,
            IdleOutcome::Timeout => true,
            IdleOutcome::Interrupted => return Ok(()),
        };
    }
    Ok(())
}
```

Add `pub mod sync;` to `src/lib.rs`.

Note on `already_notified`: scanning the log is O(log size) per new message. ponytail: add an index on `rule_log(message_id)` in a later migration if a large inbox makes a sync pass slow.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test sync`
Expected: 9 passed. The `run_loop_stops_on_shutdown_after_idle` test drives one session: full sync, rules, idle returns `NewMail`, INBOX sync, rules, second idle returns `Interrupted` (queue empty) so the session ends and the loop returns.

- [ ] **Step 5: Commit**

```bash
git add src/sync.rs src/lib.rs
git commit -m "feat: per-account sync loop with IDLE, rules pass and trash purge"
```

---

### Task 14: CLI

**Files:**
- Create: `src/cli/mod.rs`, `tests/cli.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Consumes: the library.
- Produces: the `postbode` binary with `run`, `sync`, `rules check|test|list|apply-existing`, `folders`, `list`, `show`, `log`, `trash list|restore|purge`, `account add`.

- [ ] **Step 1: Write the failing black-box tests**

```rust
// tests/cli.rs
use std::process::Command;

fn postbode(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_postbode"))
        .args(args)
        .env("POSTBODE_HOME", home)
        .env("RUST_LOG", "error")
        .output()
        .unwrap()
}

#[test]
fn rules_check_reports_bad_file_and_exits_nonzero() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("config")).unwrap();
    std::fs::write(home.path().join("config/rules.toml"), "[[rules]]\nname = \"x\"\nmatch.from = { regex = \"(\" }\nactions = [\"delete\"]\n").unwrap();
    let out = postbode(home.path(), &["rules", "check"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("rule 'x'"), "{stderr}");
}

#[test]
fn rules_check_passes_on_valid_file_and_lists() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join("config")).unwrap();
    std::fs::write(home.path().join("config/rules.toml"), "[[rules]]\nname = \"ok\"\nmatch.seen = true\nactions = [\"flag\"]\n").unwrap();
    assert!(postbode(home.path(), &["rules", "check"]).status.success());
    let out = postbode(home.path(), &["rules", "list"]);
    assert!(String::from_utf8_lossy(&out.stdout).contains("ok"));
}

#[test]
fn list_on_unknown_account_fails_and_empty_config_lists_nothing() {
    let home = tempfile::tempdir().unwrap();
    let out = postbode(home.path(), &["list", "--account", "nope"]);
    assert!(!out.status.success());
    let out = postbode(home.path(), &["folders"]);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test cli`
Expected: failures, the binary only prints `postbode`.

- [ ] **Step 3: Implement main.rs**

```rust
// src/main.rs
mod cli;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,postbode=info")).init();
    if let Err(e) = cli::run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}
```

- [ ] **Step 4: Implement cli/mod.rs**

```rust
// src/cli/mod.rs
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};

use postbode::config::{AccountConfig, Config, PasswordSource};
use postbode::credentials::{self, Secret};
use postbode::mail_ops::MailOps;
use postbode::paths::Paths;
use postbode::rules::engine::{Context, Mode, evaluate};
use postbode::store::Store;
use postbode::sync::{self, Event};
use postbode::trash::Trash;

#[derive(Parser)]
#[command(name = "postbode", version, about = "A fast, simple mail client with automatic mailbox rules")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Sync all accounts continuously and apply rules; Ctrl-C stops
    Run,
    /// Sync once, apply rules, exit
    Sync {
        #[arg(long)]
        account: Option<String>,
    },
    /// Inspect and test rules.toml
    Rules {
        #[command(subcommand)]
        command: RulesCommand,
    },
    /// List folders with message and unread counts
    Folders {
        #[arg(long)]
        account: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List recent messages, newest first
    List {
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value = "INBOX")]
        folder: String,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Show one message
    Show {
        uid: u32,
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value = "INBOX")]
        folder: String,
        /// Print the raw RFC 5322 message instead of the text body
        #[arg(long)]
        raw: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show what rules did, newest first
    Log {
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Deleted mail kept for the retention period
    Trash {
        #[command(subcommand)]
        command: TrashCommand,
    },
    /// Manage accounts
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
}

#[derive(Subcommand)]
enum RulesCommand {
    /// Validate rules.toml
    Check,
    /// Dry run: print what each rule would do to the cached messages
    Test {
        name: Option<String>,
        #[arg(long)]
        account: Option<String>,
    },
    /// Names, enabled state and first-seen time
    List {
        #[arg(long)]
        json: bool,
    },
    /// Run one rule against mail that predates it
    ApplyExisting {
        name: String,
        #[arg(long)]
        account: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum TrashCommand {
    List {
        #[arg(long)]
        account: Option<String>,
    },
    /// Append a trashed .eml back into its original folder
    Restore {
        file: String,
        #[arg(long)]
        account: Option<String>,
    },
    /// Remove trash files older than the retention period
    Purge {
        #[arg(long)]
        account: Option<String>,
    },
}

#[derive(Subcommand)]
enum AccountCommand {
    /// Interactively add an IMAP account and test the login
    Add,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let paths = match std::env::var_os("POSTBODE_HOME") {
        Some(home) => Paths::under(Path::new(&home)),
        None => Paths::discover()?,
    };
    let config = Config::load(&paths.config_file())?;
    match cli.command {
        Command::Run => cmd_run(&config, &paths),
        Command::Sync { account } => {
            let (tx, rx) = mpsc::channel();
            for acc in select_accounts(&config, account.as_deref())? {
                sync::run_once(acc, &paths, &tx)?;
            }
            drop(tx);
            for event in rx {
                print_event(&event);
            }
            Ok(())
        }
        Command::Rules { command } => cmd_rules(command, &config, &paths),
        Command::Folders { account, json } => {
            for acc in select_accounts(&config, account.as_deref())? {
                let store = open_store(&paths, &acc.name)?;
                for f in store.folders()? {
                    let total = store.messages_in_folder(&f.name)?.len();
                    let unread = store.unread_count(&f.name)?;
                    if json {
                        println!("{}", serde_json::json!({ "account": acc.name, "folder": f.name, "total": total, "unread": unread, "special_use": f.special_use }));
                    } else {
                        println!("{}\t{}\t{total}\t{unread}", acc.name, f.name);
                    }
                }
            }
            Ok(())
        }
        Command::List { account, folder, limit, json } => {
            for acc in select_accounts(&config, account.as_deref())? {
                let store = open_store(&paths, &acc.name)?;
                for m in store.messages(&folder, limit)? {
                    if json {
                        println!("{}", serde_json::to_string(&m)?);
                    } else {
                        let date = chrono::DateTime::from_timestamp(m.internaldate, 0).map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
                        let flag = if m.is_seen() { " " } else { "*" };
                        println!("{flag} {:>6}  {date}  {:<30}  {}", m.uid, truncate(m.from_addr.as_deref().unwrap_or(""), 30), m.subject.as_deref().unwrap_or(""));
                    }
                }
            }
            Ok(())
        }
        Command::Show { uid, account, folder, raw, json } => {
            let acc = single_account(&config, account.as_deref())?;
            let store = open_store(&paths, &acc.name)?;
            let msg = store.message(&folder, uid)?.with_context(|| format!("no message {folder}/{uid}"))?;
            let body = match store.raw(&folder, uid)? {
                Some(raw) => raw,
                None => {
                    let secret = credentials::resolve(acc)?;
                    let mut ops = postbode::mail_ops::imap::ImapOps::connect(acc, &secret)?;
                    ops.select(&folder)?;
                    postbode::rules::apply::ensure_raw(&msg, &mut ops, &store)?
                }
            };
            if raw {
                io::stdout().write_all(&body)?;
            } else if json {
                println!("{}", serde_json::json!({ "message": msg, "body_text": postbode::message::body_text(&body) }));
            } else {
                println!("From: {}\nTo: {}\nSubject: {}\n", msg.from_addr.as_deref().unwrap_or(""), msg.to_addr.as_deref().unwrap_or(""), msg.subject.as_deref().unwrap_or(""));
                println!("{}", postbode::message::body_text(&body));
            }
            Ok(())
        }
        Command::Log { account, limit, json } => {
            for acc in select_accounts(&config, account.as_deref())? {
                let store = open_store(&paths, &acc.name)?;
                for e in store.log(limit)? {
                    if json {
                        println!("{}", serde_json::to_string(&e)?);
                    } else {
                        let at = chrono::DateTime::from_timestamp(e.at, 0).map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
                        println!("{at}  {:<20} {:<12} {}/{}  {}", e.rule_name, e.action, e.folder, e.uid, e.subject.as_deref().unwrap_or(""));
                    }
                }
            }
            Ok(())
        }
        Command::Trash { command } => cmd_trash(command, &config, &paths),
        Command::Account { command } => match command {
            AccountCommand::Add => cmd_account_add(config, &paths),
        },
    }
}

fn select_accounts<'a>(config: &'a Config, name: Option<&str>) -> Result<Vec<&'a AccountConfig>> {
    match name {
        Some(n) => Ok(vec![config.account(n).with_context(|| format!("no account named '{n}'"))?]),
        None => Ok(config.accounts.iter().collect()),
    }
}

fn single_account<'a>(config: &'a Config, name: Option<&str>) -> Result<&'a AccountConfig> {
    match (name, config.accounts.len()) {
        (Some(n), _) => config.account(n).with_context(|| format!("no account named '{n}'")),
        (None, 1) => Ok(&config.accounts[0]),
        (None, 0) => bail!("no accounts configured; run `postbode account add`"),
        (None, _) => bail!("several accounts configured; pass --account"),
    }
}

fn open_store(paths: &Paths, account: &str) -> Result<Store> {
    Ok(Store::open(&paths.mail_db(account))?)
}

fn truncate(s: &str, width: usize) -> String {
    let mut out: String = s.chars().take(width).collect();
    if s.chars().count() > width {
        out.pop();
        out.push('…');
    }
    out
}

fn print_event(event: &Event) {
    match event {
        Event::NewMail { account, from, subject, .. } => println!("[{account}] new mail from {from}: {subject}"),
        Event::Synced { account, new_messages, actions } => println!("[{account}] synced: {new_messages} new, {actions} rule actions"),
        Event::Error { account, message } => eprintln!("[{account}] error: {message}"),
    }
}

fn cmd_run(config: &Config, paths: &Paths) -> Result<()> {
    if config.accounts.is_empty() {
        bail!("no accounts configured; run `postbode account add`");
    }
    // Ctrl-C ends the process through the default SIGINT handler; WAL and trash-before-delete leave nothing half done.
    let shutdown = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let mut handles = Vec::new();
    for account in config.accounts.clone() {
        let (paths, tx, shutdown) = (paths.clone(), tx.clone(), shutdown.clone());
        handles.push(std::thread::Builder::new().name(format!("sync-{}", account.name)).spawn(move || sync::run_loop(account, paths, tx, shutdown))?);
    }
    drop(tx);
    for event in rx {
        print_event(&event);
        if let Event::NewMail { from, subject, .. } = &event {
            let _ = notify_rust::Notification::new().summary(from).body(subject).appname("Postbode").show();
        }
    }
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}

fn cmd_rules(command: RulesCommand, config: &Config, paths: &Paths) -> Result<()> {
    match command {
        RulesCommand::Check => {
            let file = postbode::rules::load(&paths.rules_file())?;
            let compiled = postbode::rules::compile(&file)?;
            println!("{} rules ok", compiled.len());
            Ok(())
        }
        RulesCommand::List { json } => {
            let file = postbode::rules::load(&paths.rules_file())?;
            for r in &file.rules {
                if json {
                    println!("{}", serde_json::json!({ "name": r.name, "enabled": r.enabled, "proposed_by": r.proposed_by, "account": r.account, "folder": r.folder }));
                } else {
                    println!("{}\t{}\t{}", if r.enabled { "on " } else { "off" }, r.name, r.proposed_by.as_deref().unwrap_or(""));
                }
            }
            Ok(())
        }
        RulesCommand::ApplyExisting { name, account, dry_run } => {
            for acc in select_accounts(config, account.as_deref())? {
                let store = open_store(paths, &acc.name)?;
                let rules: Vec<_> = sync::load_rules_for(&store, &paths.rules_file(), sync::now())?.into_iter().filter(|r| r.rule.name == name).collect();
                if rules.is_empty() {
                    bail!("no rule named '{name}'");
                }
                let identity = acc.identity()?;
                if dry_run {
                    let ctx = Context { account: &acc.name, identity: &identity, now: sync::now(), mode: Mode::ApplyExisting, notify_default: false };
                    for msg in store.messages_in_folder(rules[0].folder())? {
                        for a in evaluate(&rules, &msg, &ctx).actions {
                            println!("{}\t{}/{}\t{}\t{}", a.rule, msg.folder, msg.uid, a.action.label(), msg.subject.as_deref().unwrap_or(""));
                        }
                    }
                    continue;
                }
                let secret = credentials::resolve(acc)?;
                let mut ops = postbode::mail_ops::imap::ImapOps::connect(acc, &secret)?;
                let trash = Trash::new(paths.trash_dir(&acc.name));
                let run = sync::run_rules(&mut ops, &store, &trash, &rules, acc, &identity, &[], Mode::ApplyExisting, sync::now())?;
                println!("{}: {} actions on {} messages", acc.name, run.actions, run.evaluated);
            }
            Ok(())
        }
        RulesCommand::Test { name, account } => {
            for acc in select_accounts(config, account.as_deref())? {
                let store = open_store(paths, &acc.name)?;
                let rules = sync::load_rules_for(&store, &paths.rules_file(), sync::now())?;
                let rules: Vec<_> = rules.into_iter().filter(|r| name.as_deref().is_none_or(|n| n == r.rule.name)).collect();
                let identity = acc.identity()?;
                let ctx = Context { account: &acc.name, identity: &identity, now: sync::now(), mode: Mode::Normal, notify_default: acc.notify };
                for folder in store.folders()? {
                    for msg in store.messages_in_folder(&folder.name)? {
                        let plan = evaluate(&rules, &msg, &ctx);
                        for a in plan.actions {
                            println!("{}\t{}/{}\t{}\t{}", a.rule, msg.folder, msg.uid, a.action.label(), msg.subject.as_deref().unwrap_or(""));
                        }
                    }
                }
            }
            Ok(())
        }
    }
}

fn cmd_trash(command: TrashCommand, config: &Config, paths: &Paths) -> Result<()> {
    match command {
        TrashCommand::List { account } => {
            for acc in select_accounts(config, account.as_deref())? {
                for e in Trash::new(paths.trash_dir(&acc.name)).list()? {
                    let at = chrono::DateTime::from_timestamp(e.saved_at, 0).map(|d| d.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default();
                    println!("{at}  {}/{}  {}", e.folder, e.uid, e.path.display());
                }
            }
            Ok(())
        }
        TrashCommand::Restore { file, account } => {
            let acc = single_account(config, account.as_deref())?;
            let path = Path::new(&file);
            let name = path.file_name().and_then(|n| n.to_str()).context("bad file name")?;
            let (_, folder, _) = Trash::parse_name(name).context("not a postbode trash file")?;
            let raw = std::fs::read(path)?;
            let secret = credentials::resolve(acc)?;
            let mut ops = postbode::mail_ops::imap::ImapOps::connect(acc, &secret)?;
            ops.append(&folder, &raw)?;
            std::fs::remove_file(path)?;
            println!("restored to {folder}; run `postbode sync` to see it");
            Ok(())
        }
        TrashCommand::Purge { account } => {
            for acc in select_accounts(config, account.as_deref())? {
                let removed = Trash::new(paths.trash_dir(&acc.name)).purge(acc.trash_retention_days as i64 * 86_400, sync::now())?;
                println!("{}: removed {removed}", acc.name);
            }
            Ok(())
        }
    }
}

fn cmd_account_add(mut config: Config, paths: &Paths) -> Result<()> {
    let name = prompt("Account name (letters, digits, - _)")?;
    let host = prompt("IMAP host")?;
    let port: u16 = prompt("Port [993]")?.parse().unwrap_or(993);
    let username = prompt("Username")?;
    let address = {
        let a = prompt(&format!("Email address [{username}]"))?;
        if a.is_empty() { None } else { Some(a) }
    };
    let storage = prompt("Password storage: (k)eyring or (c)ommand [k]")?;
    let (password, secret) = if storage.starts_with('c') {
        let command = prompt("Password command")?;
        let src = PasswordSource::Command { command };
        let tmp = AccountConfig { name: name.clone(), host: host.clone(), port, username: username.clone(), password: src.clone(), address: address.clone(), aliases: vec![], sync_interval_secs: 120, trash_retention_days: 30, notify: true };
        let secret = credentials::resolve(&tmp)?;
        (src, secret)
    } else {
        let pw = rpassword::prompt_password("Password: ")?;
        (PasswordSource::Keyring { keyring: true }, Secret::new(pw))
    };
    let account = AccountConfig { name, host, port, username, password, address, aliases: vec![], sync_interval_secs: 120, trash_retention_days: 30, notify: true };
    print!("Testing login... ");
    io::stdout().flush()?;
    let mut ops = postbode::mail_ops::imap::ImapOps::connect(&account, &secret)?;
    let folders = ops.list_folders()?;
    println!("ok, {} folders", folders.len());
    if matches!(account.password, PasswordSource::Keyring { .. }) {
        credentials::store(&account.name, &secret)?;
    }
    config.accounts.retain(|a| a.name != account.name);
    config.accounts.push(account);
    config.save(&paths.config_file())?;
    println!("saved to {}", paths.config_file().display());
    Ok(())
}

fn prompt(label: &str) -> Result<String> {
    print!("{label}: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}
```

Ctrl-C: the default SIGINT disposition terminates the process. The `shutdown` flag is only set by tests and, later, the GUI; add the `ctrlc` crate when a graceful stop is needed.

- [ ] **Step 5: Run all tests and clippy**

Run: `cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: all green, including the three black-box tests.

- [ ] **Step 6: Smoke test by hand**

```bash
POSTBODE_HOME=/tmp/pb cargo run -- account add
POSTBODE_HOME=/tmp/pb cargo run -- sync
POSTBODE_HOME=/tmp/pb cargo run -- folders
POSTBODE_HOME=/tmp/pb cargo run -- list --limit 5
POSTBODE_HOME=/tmp/pb cargo run -- rules check
POSTBODE_HOME=/tmp/pb cargo run -- run
```

Expected: an account is added after a successful login, `sync` prints a `synced` line, `list` shows mail, `run` keeps going and prints new mail as it arrives. Report which of these were actually run.

- [ ] **Step 7: Commit**

```bash
git add src/main.rs src/cli/ tests/cli.rs
git commit -m "feat: postbode CLI with run, sync, rules, list, show, log and trash"
```

---

## Done when

- `cargo test`, clippy with `-D warnings` and `cargo fmt --check` pass.
- `postbode run` against a real account syncs, applies a rule from `rules.toml`, writes a trash file before a delete, and prints new-mail lines.
- Plan 2 (`docs/superpowers/plans/2026-10-06-engine-features.md`) can start: search, threads, attachments, direct actions, `rules propose/approve/reject/schema`, `guide`, docs skeleton.
