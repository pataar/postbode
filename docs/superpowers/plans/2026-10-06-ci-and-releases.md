# CI, Releases and Live IMAP Tests Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Postbode releasable and continuously checked. This covers IMAP timeouts, live tests against Dovecot, CI on Linux and macOS, release-plz and cargo-dist releases with a Homebrew tap, Renovate, and the open plan-1 and plan-2 follow-ups that touch safety or offline use.

**Architecture:**
- **Engine changes stay small.** The IMAP client gets a connect timeout, TCP keepalive and an optional extra trusted CA (`ca_file`). Four follow-up fixes land in `paths`, `rules::edit`, `trash`, `credentials` and the CLI.
- **Live tests.** They run against two Dovecot 2.4 containers from one compose file: one with MOVE and UIDPLUS, one without. Each test logs in as its own throwaway user, so tests never share a mailbox.
- **Releases.** CI, release and dependency automation is GitHub Actions plus config files: `ci.yml`, release-plz, cargo-dist, `renovate.json`. No hand-written release scripts.

**Tech Stack:**
- Rust 1.99 (edition 2024), async-imap 0.12 on tokio, rustls 0.23, socket2 0.6 (new).
- Dovecot 2.4.5 (`dovecot/dovecot` image) via docker compose.
- GitHub Actions: `kunobi-ninja/kache-action@v1`, `Swatinem/rust-cache@v2`, `taiki-e/install-action@v2`, `release-plz/action@v0.5`.
- dist (cargo-dist) 0.33.0, Renovate, mise.

**Spec:** `docs/superpowers/specs/2026-10-06-postbode-core-design.md`, sections 12–15. Also read `.brainstorm_projects/plan-1-followups.md` and `.brainstorm_projects/plan-2-followups.md`; they are git-ignored, so read them in the main checkout.

## Global Constraints

- Rust 1.99, edition 2024. `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings` and `cargo test` pass after every task.
- Every new dependency gets a one-line reason comment in `Cargo.toml`. This plan adds exactly one runtime dependency: `socket2 = { version = "0.6", features = ["all"] }`.
- Migration 001 is live. This plan changes no schema; any schema change would need a migration 002.
- Never read message bodies from a user's store, and never log bodies or secrets. Live tests talk only to the local Dovecot containers.
- Do not touch anything outside the repository:
  - Do not run `kache init`, which edits `~/.cargo/config.toml`.
  - Do not create GitHub repos or secrets, and do not publish to crates.io.
  - Do not push. The user pushes; a hook blocks agents from pushing to `main`.
- Report platform coverage honestly: "compiled on" vs "ran on". GitHub workflows can only be linted locally (actionlint, `dist plan`), so say they have not run until the user pushes a branch.
- Comments: none by default; one line when the why is non-obvious; no history references.
- Use conventional commits, one commit per task step that says "Commit". Do not broaden a task into adjacent features.
- After changing CLI flags or rule types, run `POSTBODE_BLESS=1 cargo test` and commit `docs/src/cli.md` and `docs/src/rules.schema.json`. No task here is expected to change either.
- Prose lives in `docs/src/`, and `README.md` contains `docs/src/index.md` verbatim (`tests/docs.rs` checks this).

## Review Focus

1. **A server that accepts TCP but never answers TLS** (a captive portal, a half-dead NAT): `connect` fails within the timeout with a clear "did not answer" error and doesn't hang. Pinned by Task 1, `connect_gives_up_on_a_server_that_never_answers`.
2. **A `ca_file` that is missing, empty or not PEM:** the error names `ca_file` and the path; it doesn't surface later as a generic TLS "unknown issuer". A relative path is rejected at config load. Pinned by Task 2, `root_store_rejects_a_missing_or_empty_ca_file` and `relative_ca_file_is_rejected`.
3. **`POSTBODE_TEST_IMAP_HOST` set to an empty string:** the macOS CI job does this, and the live tests must skip, not fail. Pinned by Task 3's `host()` helper (it treats empty as unset), and exercised by every macOS CI run.
4. **The published crate shipping the test TLS key, design docs or CI files.** Pinned by Task 8's `cargo package --list` check against the `include` list.
5. **A `rules.toml` that is a symlink** into a dotfiles repo, or a dangling one: `rules approve` writes through the link, and a dangling link is an error rather than being silently replaced by a regular file. Pinned by Task 5, `write_atomic_writes_through_a_symlink` and `write_atomic_refuses_a_dangling_symlink`.

## Out of scope (recorded so reviewers don't flag them)

- **Code signing.** Unsigned release binaries mean macOS asks for Keychain access again after every upgrade. Developer ID signing needs a paid Apple account, so it is the user's decision later. Task 6 adds a hint when the keyring is slow, so a hidden prompt no longer looks like a hang.
- **mold.** Rust's default linker on x86_64 Linux has been lld since 1.90, so mold adds little. Task 7 records this in the spec.
- **A per-command IMAP timeout.** TCP keepalive catches dead connections. A live server that stops answering mid-command still blocks; this is marked with a `ponytail:` comment.
- **STARTTLS on port 143, a shell installer, `.deb`/`.rpm`, a docs site, and the remaining deferred minors** in the follow-up files that this plan does not name.

---

### Task 1: IMAP connect timeout and TCP keepalive

**Files:**
- Modify: `Cargo.toml` (add socket2)
- Modify: `src/mail_ops/imap.rs` (`ImapOps::connect` and its tests)
- Modify: `docs/superpowers/specs/2026-10-06-postbode-core-design.md` §12

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `ImapOps::connect(account: &AccountConfig, secret: &Secret) -> MailResult<ImapOps>`, with the same signature, now bounded by `CONNECT_TIMEOUT` (30s).
  - The private `ImapOps::connect_within(account, secret, limit: Duration)`.
  - The private `async fn open_session(account: &AccountConfig, secret: &Secret) -> MailResult<(ImapSession, bool, bool, bool)>`, which Task 2 edits.
  - The private `fn enable_keepalive(tcp: &TcpStream) -> std::io::Result<()>`.

- [ ] **Step 1: Add socket2.** In `Cargo.toml` `[dependencies]`, in alphabetical position after `serde_rusqlite`:

```toml
socket2 = { version = "0.6", features = ["all"] }     # TCP keepalive on the IMAP socket so dead connections fail; "all" exposes the retry count
```

- [ ] **Step 2: Write the failing tests.** Replace the `live_round_trip` test (Task 3 supersedes it) in `src/mail_ops/imap.rs`'s test module with a shared account helper and two tests:

```rust
    fn test_account(host: &str, port: u16) -> AccountConfig {
        AccountConfig {
            name: "test".into(),
            host: host.into(),
            port,
            username: "me@example.com".into(),
            password: PasswordSource::Keyring { keyring: true },
            address: None,
            aliases: vec![],
            sync_interval_secs: 120,
            trash_retention_days: 30,
            notify: true,
        }
    }

    #[test]
    fn connect_gives_up_on_a_server_that_never_answers() {
        // The kernel completes the TCP handshake from the backlog; nobody ever answers the TLS hello.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let started = Instant::now();
        let result = ImapOps::connect_within(
            &test_account("127.0.0.1", port),
            &Secret::new("x".into()),
            Duration::from_millis(300),
        );
        match result {
            Err(MailError::Connect(message)) => assert!(message.contains("did not answer"), "{message}"),
            Err(other) => panic!("wrong error: {other}"),
            Ok(_) => panic!("connected to a silent server"),
        }
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(listener);
    }

    #[test]
    fn keepalive_is_enabled_on_the_socket() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let tcp = TcpStream::connect(addr).await.unwrap();
            enable_keepalive(&tcp).unwrap();
            let socket = socket2::SockRef::from(&tcp);
            assert!(socket.keepalive().unwrap());
            assert_eq!(socket.tcp_keepalive_time().unwrap(), KEEPALIVE_IDLE);
        });
    }
```

- [ ] **Step 3: Run the tests and confirm they fail to compile.**

Run: `cargo test --lib mail_ops::imap`
Expected: compile errors for `connect_within`, `enable_keepalive` and `KEEPALIVE_IDLE`.

- [ ] **Step 4: Implement.** In `src/mail_ops/imap.rs`, add these constants above `pub struct ImapOps`:

```rust
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// A peer that vanished (a NAT entry dropped during sleep) is noticed after about 60 + 4 × 15 seconds instead of never.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(60);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const KEEPALIVE_RETRIES: u32 = 4;
```

Then split the body of `connect`: `connect` delegates to `connect_within(account, secret, CONNECT_TIMEOUT)`, and the old async block becomes `open_session`:

```rust
impl ImapOps {
    pub fn connect(account: &AccountConfig, secret: &Secret) -> MailResult<ImapOps> {
        ImapOps::connect_within(account, secret, CONNECT_TIMEOUT)
    }

    // ponytail: only connecting is bounded; a live server that stops answering mid-command still blocks. Wrap block_on in a timeout if that shows up.
    fn connect_within(account: &AccountConfig, secret: &Secret, limit: Duration) -> MailResult<ImapOps> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| MailError::Io(e.to_string()))?;
        let (session, has_idle, has_move, has_uidplus) = rt.block_on(async {
            tokio::time::timeout(limit, open_session(account, secret))
                .await
                .map_err(|_| {
                    MailError::Connect(format!(
                        "{}:{} did not answer within {limit:?}",
                        account.host, account.port
                    ))
                })?
        })?;
        Ok(ImapOps {
            rt,
            session: Some(session),
            has_idle,
            has_move,
            has_uidplus,
        })
    }
    // parts() unchanged
}

async fn open_session(
    account: &AccountConfig,
    secret: &Secret,
) -> MailResult<(ImapSession, bool, bool, bool)> {
    let tcp = TcpStream::connect((account.host.as_str(), account.port))
        .await
        .map_err(|e| MailError::Connect(e.to_string()))?;
    enable_keepalive(&tcp).map_err(|e| MailError::Connect(e.to_string()))?;
    // ... the rest of the former async block, unchanged: roots, TLS, login, capabilities ...
    Ok((session, has_idle, has_move, has_uidplus))
}

fn enable_keepalive(tcp: &TcpStream) -> std::io::Result<()> {
    let params = socket2::TcpKeepalive::new()
        .with_time(KEEPALIVE_IDLE)
        .with_interval(KEEPALIVE_INTERVAL)
        .with_retries(KEEPALIVE_RETRIES);
    socket2::SockRef::from(tcp).set_tcp_keepalive(&params)
}
```

- [ ] **Step 5: Run the tests and confirm they pass.**

Run: `cargo test --lib mail_ops::imap`
Expected: PASS, with the timeout test finishing in well under 5 seconds.

- [ ] **Step 6: Update the spec.** Append this bullet to §12:

```markdown
- IMAP connections give up after 30s while connecting (TCP, TLS and login together) and use TCP keepalive (60s idle, then 4 probes 15s apart), so a dead connection fails within about two minutes and the run loop reconnects. A live server that stops answering mid-command is not bounded yet.
```

- [ ] **Step 7: Run the gate and commit.**

Run: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test`

```bash
git add Cargo.toml Cargo.lock src/mail_ops/imap.rs docs/superpowers/specs/2026-10-06-postbode-core-design.md
git commit -m "feat: bound IMAP connect with a timeout and enable TCP keepalive"
```

---

### Task 2: `ca_file` account option and the test CA

**Files:**
- Create: `tests/dovecot/gen-certs.sh`, `tests/dovecot/certs/ca.pem`, `tests/dovecot/certs/tls.crt`, `tests/dovecot/certs/tls.key`
- Modify: `src/config.rs` (field and validation), `src/mail_ops/imap.rs` (`root_store`, `open_session`), and every `AccountConfig { .. }` literal the compiler points at (`src/cli/mod.rs` account add, test helpers)
- Modify: `docs/src/accounts.md`, spec §5 (account keys) and §13

**Interfaces:**
- Consumes: `open_session` from Task 1.
- Produces:
  - `AccountConfig.ca_file: Option<PathBuf>`, serde default `None`, skipped when `None` on save.
  - The private `fn root_store(ca_file: Option<&Path>) -> MailResult<rustls::RootCertStore>`.
  - The files `tests/dovecot/certs/{ca.pem,tls.crt,tls.key}`, which Task 3 uses.

- [ ] **Step 1: Generate the test-only CA and server certificate.** Create `tests/dovecot/gen-certs.sh`, make it executable, and run it once:

```sh
#!/bin/sh
# Regenerates the test-only CA and the localhost certificate for the Dovecot live tests. These keys are public; never trust them elsewhere.
set -eu
cd "$(dirname "$0")"
mkdir -p certs
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
openssl req -x509 -newkey rsa:2048 -nodes -days 36500 -subj "/CN=Postbode test CA" \
  -keyout "$tmp/ca.key" -out certs/ca.pem \
  -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign"
openssl req -newkey rsa:2048 -nodes -subj "/CN=localhost" -keyout certs/tls.key -out "$tmp/tls.csr"
printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost,IP:127.0.0.1\n' > "$tmp/ext"
openssl x509 -req -in "$tmp/tls.csr" -CA certs/ca.pem -CAkey "$tmp/ca.key" -set_serial 1 \
  -days 36500 -extfile "$tmp/ext" -out certs/tls.crt
# Dovecot in the container runs as uid 1000 and must read the key.
chmod 644 certs/tls.key
```

Run: `chmod +x tests/dovecot/gen-certs.sh && tests/dovecot/gen-certs.sh && openssl verify -CAfile tests/dovecot/certs/ca.pem tests/dovecot/certs/tls.crt`
Expected: `tests/dovecot/certs/tls.crt: OK`. The CA key is thrown away, which is intended; regenerating makes a new CA.

- [ ] **Step 2: Write the failing tests.**

In `src/config.rs` tests:

```rust
    #[test]
    fn relative_ca_file_is_rejected() {
        let text = "[[accounts]]\nname = \"self\"\nhost = \"localhost\"\nusername = \"me@example.com\"\npassword = { keyring = true }\nca_file = \"certs/ca.pem\"\n";
        let err = Config::parse(text).unwrap_err().to_string();
        assert!(err.contains("ca_file must be an absolute path"), "{err}");
        let absolute = text.replace("certs/ca.pem", "/etc/ssl/self/ca.pem");
        assert!(Config::parse(&absolute).unwrap().accounts[0].ca_file.is_some());
    }
```

In `src/mail_ops/imap.rs` tests (also add `ca_file: None` to `test_account`):

```rust
    const TEST_CA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/dovecot/certs/ca.pem");

    #[test]
    fn root_store_adds_the_ca_file_to_the_public_roots() {
        let public = root_store(None).unwrap().len();
        assert_eq!(root_store(Some(Path::new(TEST_CA))).unwrap().len(), public + 1);
    }

    #[test]
    fn root_store_rejects_a_missing_or_empty_ca_file() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.pem");
        std::fs::write(&empty, "not a certificate\n").unwrap();
        for path in [dir.path().join("missing.pem"), empty] {
            match root_store(Some(&path)) {
                Err(MailError::Connect(message)) => {
                    assert!(message.contains("ca_file") && message.contains(&*path.to_string_lossy()), "{message}")
                }
                other => panic!("{path:?}: expected a ca_file error, got {:?}", other.map(|r| r.len())),
            }
        }
    }
```

- [ ] **Step 3: Run the tests and confirm they fail.**

Run: `cargo test --lib config:: mail_ops::imap`
Expected: compile errors for the `ca_file` field and `root_store`.

- [ ] **Step 4: Implement.**

In `src/config.rs`, add the field to `AccountConfig` after `notify`, and add `use std::path::PathBuf;`:

```rust
    /// Extra trusted root certificate (PEM), for servers with a private CA.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_file: Option<PathBuf>,
```

In `Config::validate`, inside the per-account loop after the address check:

```rust
            if let Some(ca_file) = &account.ca_file
                && !ca_file.is_absolute()
            {
                return Err(ConfigError::Invalid(format!(
                    "account '{name}': ca_file must be an absolute path"
                )));
            }
```

In `src/mail_ops/imap.rs`, add `use std::path::Path;` and `use rustls::pki_types::{CertificateDer, pem::PemObject};`, plus:

```rust
/// The public web roots, plus the account's own CA when it sets `ca_file`.
fn root_store(ca_file: Option<&Path>) -> MailResult<rustls::RootCertStore> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let Some(path) = ca_file else {
        return Ok(roots);
    };
    let invalid = |reason: String| MailError::Connect(format!("ca_file {}: {reason}", path.display()));
    let certs = CertificateDer::pem_file_iter(path)
        .and_then(|certs| certs.collect::<Result<Vec<_>, _>>())
        .map_err(|e| invalid(e.to_string()))?;
    let (added, _) = roots.add_parsable_certificates(certs);
    if added == 0 {
        return Err(invalid("no certificate found".into()));
    }
    Ok(roots)
}
```

In `open_session`, replace the two lines that build `roots` with `let roots = root_store(account.ca_file.as_deref())?;`. Add `ca_file: None` to every `AccountConfig` literal the compiler reports.

- [ ] **Step 5: Run the tests and confirm they pass.**

Run: `cargo test`
Expected: PASS, including `tests/docs.rs` (`account_examples_parse`).

- [ ] **Step 6: Update the docs.**
  - In `docs/src/accounts.md`, add a row to the key table after `notify`:

    ```markdown
    | `ca_file` | none | Absolute path to a PEM file with an extra trusted root certificate, for a server with a private CA. |
    ```

  - In spec §5, add `ca_file` wherever the account keys are listed, with the same meaning.
  - Leave §13 for Task 3.

- [ ] **Step 7: Run the gate and commit.**

```bash
git add tests/dovecot src/config.rs src/mail_ops/imap.rs src/cli/mod.rs docs/src/accounts.md docs/superpowers/specs/2026-10-06-postbode-core-design.md
git commit -m "feat: trust an extra CA per account with ca_file"
```

---

### Task 3: Dovecot harness and live `ImapOps` tests

**Files:**
- Create: `tests/dovecot/compose.yml`, `tests/dovecot/postbode.conf`, `tests/dovecot/without-move.conf`, `tests/imap_live.rs`
- Modify: `src/mail_ops/imap.rs` (two inherent methods), `AGENTS.md`, spec §13

**Interfaces:**
- Consumes:
  - `AccountConfig.ca_file` and `tests/dovecot/certs/*` from Task 2.
  - `ImapOps::connect` and the `MailOps` trait.
- Produces:
  - `ImapOps::has_move(&self) -> bool`.
  - `ImapOps::delete_folder(&mut self, name: &str) -> MailResult<()>`.
  - The `tests/imap_live.rs` helpers `host()`, `account()`, `connect()` and `mail()`, which Task 4 adds to.
  - Live servers at `localhost:10993` (MOVE and UIDPLUS) and `localhost:11993` (neither), password `postbode-test`.

- [ ] **Step 1: Write the Dovecot config.**

`tests/dovecot/postbode.conf`:

```
namespace inbox {
  mailbox Archive {
    auto = subscribe
    special_use = \Archive
  }
}

ssl_server {
  cert_file = /etc/postbode-test/tls.crt
  key_file = /etc/postbode-test/tls.key
}
```

`tests/dovecot/without-move.conf`:

```
imap_capability {
  IMAP4rev2 = no
  MOVE = no
  UIDPLUS = no
}
```

`tests/dovecot/compose.yml`:

```yaml
# Live IMAP test servers; see AGENTS.md. Any username logs in with USER_PASSWORD; mail lives only in the container.
services:
  dovecot:
    image: dovecot/dovecot:2.4.5
    environment:
      USER_PASSWORD: postbode-test
    ports:
      - "127.0.0.1:10993:31993"
    volumes:
      - ./certs:/etc/postbode-test:ro
      - ./postbode.conf:/etc/dovecot/conf.d/postbode.conf:ro
  dovecot-without-move:
    image: dovecot/dovecot:2.4.5
    environment:
      USER_PASSWORD: postbode-test
    ports:
      - "127.0.0.1:11993:31993"
    volumes:
      - ./certs:/etc/postbode-test:ro
      - ./postbode.conf:/etc/dovecot/conf.d/postbode.conf:ro
      - ./without-move.conf:/etc/dovecot/conf.d/without-move.conf:ro
```

Run: `docker compose -f tests/dovecot/compose.yml up -d && sleep 3 && docker compose -f tests/dovecot/compose.yml logs --tail 20`
Expected: both containers running, no config errors in the logs.

If Dovecot rejects a setting, fix it against the 2.4 docs and the image's own `/etc/dovecot` config (use `docker run --rm --entrypoint cat dovecot/dovecot:2.4.5-dev /etc/dovecot/conf.d/ssl.conf`). Record the change in your report. The known risk is the `ssl_server { }` block and `UIDPLUS = no`.

Check the certificate from the host:

Run: `openssl s_client -connect localhost:10993 -CAfile tests/dovecot/certs/ca.pem -verify_return_error </dev/null 2>&1 | ggrep -E 'Verify return code|\* OK'`
Expected: `Verify return code: 0 (ok)` and the IMAP greeting.

- [ ] **Step 2: Add the two inherent methods** to `impl ImapOps` in `src/mail_ops/imap.rs`, after `parts`:

```rust
    pub fn has_move(&self) -> bool {
        self.has_move
    }

    pub fn delete_folder(&mut self, name: &str) -> MailResult<()> {
        let (rt, session) = self.parts()?;
        rt.block_on(async { session.delete(name).await.map_err(proto) })
    }
```

- [ ] **Step 3: Write the live tests.** Create `tests/imap_live.rs`:

```rust
//! Live IMAP tests against tests/dovecot/compose.yml, skipped unless POSTBODE_TEST_IMAP_HOST is set.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use postbode::config::{AccountConfig, PasswordSource};
use postbode::credentials::Secret;
use postbode::mail_ops::imap::ImapOps;
use postbode::mail_ops::{IdleOutcome, MailOps};

const PASSWORD: &str = "postbode-test";
const PORT: u16 = 10993;
const PORT_WITHOUT_MOVE: u16 = 11993;

/// CI sets the variable to an empty string where no server runs; that counts as unset.
fn host() -> Option<String> {
    let host = std::env::var("POSTBODE_TEST_IMAP_HOST")
        .ok()
        .filter(|h| !h.is_empty());
    if host.is_none() {
        eprintln!("POSTBODE_TEST_IMAP_HOST unset; live IMAP test skipped");
    }
    host
}

/// A fresh user per call, so no two tests (or reruns) share a mailbox; Dovecot accepts any name.
fn account(host: &str, port: u16, test: &str) -> AccountConfig {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    AccountConfig {
        name: "live".into(),
        host: host.into(),
        port,
        username: format!("{test}-{nanos}@example.com"),
        password: PasswordSource::Command {
            command: format!("printf {PASSWORD}"),
        },
        address: None,
        aliases: vec![],
        sync_interval_secs: 120,
        trash_retention_days: 30,
        notify: false,
        ca_file: Some(PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/dovecot/certs/ca.pem"
        ))),
    }
}

fn connect(account: &AccountConfig) -> ImapOps {
    ImapOps::connect(account, &Secret::new(PASSWORD.into())).unwrap_or_else(|e| panic!("{e}"))
}

fn mail(subject: &str) -> Vec<u8> {
    format!(
        "From: Sender <sender@example.com>\r\nTo: user@example.com\r\nSubject: {subject}\r\n\
         Message-ID: <{}@example.com>\r\nDate: Tue, 06 Oct 2026 10:00:00 +0000\r\n\r\nBody of {subject}\r\n",
        subject.replace(' ', ".")
    )
    .into_bytes()
}

#[test]
fn lists_special_use_folders() {
    let Some(host) = host() else { return };
    let folders = connect(&account(&host, PORT, "special-use")).list_folders().unwrap();
    for role in ["Archive", "Drafts", "Junk", "Sent", "Trash"] {
        assert!(
            folders.iter().any(|f| f.special_use.as_deref() == Some(role)),
            "{role}: {folders:?}"
        );
    }
}

#[test]
fn append_fetch_and_flags_round_trip() {
    let Some(host) = host() else { return };
    let mut ops = connect(&account(&host, PORT, "flags"));
    ops.append("INBOX", &mail("hello"), &["$PostbodeRestored"]).unwrap();
    ops.select("INBOX").unwrap();
    let new = ops.fetch_new(1).unwrap();
    assert_eq!(new.len(), 1);
    assert!(new[0].flags.iter().any(|f| f == "$PostbodeRestored"), "{:?}", new[0].flags);
    let uid = new[0].uid;
    ops.add_flags(uid, &["\\Seen", "\\Flagged"]).unwrap();
    ops.remove_flags(uid, &["\\Flagged"]).unwrap();
    let flags = ops.fetch_flags(uid).unwrap().remove(0).flags;
    assert!(flags.iter().any(|f| f == "\\Seen"), "{flags:?}");
    assert!(!flags.iter().any(|f| f == "\\Flagged"), "{flags:?}");
    assert_eq!(ops.fetch_raw(uid).unwrap().unwrap(), mail("hello"));
}

fn move_round_trip(port: u16, test: &str, expect_move: bool) {
    let Some(host) = host() else { return };
    let mut ops = connect(&account(&host, port, test));
    assert_eq!(ops.has_move(), expect_move, "server capabilities differ from compose.yml");
    ops.append("INBOX", &mail("move me"), &[]).unwrap();
    ops.select("INBOX").unwrap();
    let uid = ops.fetch_new(1).unwrap()[0].uid;
    ops.move_message(uid, "Archive").unwrap();
    assert!(ops.fetch_new(1).unwrap().is_empty(), "message is still in INBOX");
    ops.select("Archive").unwrap();
    assert_eq!(ops.fetch_new(1).unwrap().len(), 1);
}

#[test]
fn move_uses_move_when_advertised() {
    move_round_trip(PORT, "move", true);
}

#[test]
fn move_falls_back_to_copy_without_move() {
    move_round_trip(PORT_WITHOUT_MOVE, "copy", false);
}

#[test]
fn expunge_removes_only_the_deleted_message() {
    let Some(host) = host() else { return };
    for (port, test) in [(PORT, "expunge-uidplus"), (PORT_WITHOUT_MOVE, "expunge-plain")] {
        let mut ops = connect(&account(&host, port, test));
        ops.append("INBOX", &mail("first"), &[]).unwrap();
        ops.append("INBOX", &mail("second"), &[]).unwrap();
        ops.select("INBOX").unwrap();
        let uids: Vec<u32> = ops.fetch_new(1).unwrap().iter().map(|e| e.uid).collect();
        ops.add_flags(uids[0], &["\\Deleted"]).unwrap();
        ops.expunge(uids[0]).unwrap();
        let left: Vec<u32> = ops.fetch_new(1).unwrap().iter().map(|e| e.uid).collect();
        assert_eq!(left, vec![uids[1]], "{test}");
    }
}

#[test]
fn idle_wakes_when_mail_arrives() {
    let Some(host) = host() else { return };
    let account = account(&host, PORT, "idle");
    let mut ops = connect(&account);
    ops.select("INBOX").unwrap();
    let sender = account.clone();
    let delivery = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        connect(&sender).append("INBOX", &mail("ping"), &[]).unwrap();
    });
    let started = Instant::now();
    let outcome = ops.idle(Duration::from_secs(60), &AtomicBool::new(false)).unwrap();
    delivery.join().unwrap();
    assert_eq!(outcome, IdleOutcome::NewMail);
    assert!(started.elapsed() < Duration::from_secs(30));
}
```

`fetch_raw` equality assumes Dovecot stores APPENDed bytes unchanged. If it doesn't, assert that the raw bytes contain `Subject: hello` instead, and say so in the report.

- [ ] **Step 4: Run them against the containers.**

Run: `POSTBODE_TEST_IMAP_HOST=localhost cargo test --test imap_live`
Expected: 7 passed.

Run: `cargo test --test imap_live` (variable unset)
Expected: 7 passed, each printing "skipped" on stderr (see it with `-- --nocapture`).

If `move_falls_back_to_copy_without_move` fails on `has_move`, the capability config did not apply. Fix `without-move.conf` rather than the test.

- [ ] **Step 5: Document.**
  - **AGENTS.md:** add a section:

    ````markdown
    ## Live IMAP tests
    `tests/imap_live.rs` runs against two local Dovecot servers and is skipped unless `POSTBODE_TEST_IMAP_HOST` is set:

    ```sh
    docker compose -f tests/dovecot/compose.yml up -d
    POSTBODE_TEST_IMAP_HOST=localhost cargo test --test imap_live
    ```

    Port 10993 advertises MOVE and UIDPLUS, port 11993 neither. Each test logs in as its own throwaway user. `tests/dovecot/gen-certs.sh` regenerates the test-only CA.
    ````

  - **Spec §13:** replace the "Integration tests against Dovecot" bullet with:

    ```markdown
    - **Integration tests against Dovecot** in `tests/imap_live.rs`. `tests/dovecot/compose.yml` runs two Dovecot 2.4 servers on localhost: one advertising MOVE and UIDPLUS (port 10993), one without either (port 11993). Their certificate comes from a test-only CA in `tests/dovecot/certs/`, which test accounts trust through `ca_file`. Each test logs in as its own throwaway user and seeds mail with APPEND, so tests never share a mailbox. They cover special-use detection, flags and keywords, move with and without MOVE, expunge with and without UIDPLUS, IDLE wake, a UIDVALIDITY change, a rule delete keeping its backup, `delete` to Trash, and `sync` exiting non-zero when a rule fails. The Ubuntu CI job starts the compose file. Locally, run `docker compose -f tests/dovecot/compose.yml up -d` and set `POSTBODE_TEST_IMAP_HOST=localhost`. The tests are skipped, not failed, when that variable is unset or empty.
    ```

- [ ] **Step 6: Run the gate and commit.**

```bash
git add tests/dovecot tests/imap_live.rs src/mail_ops/imap.rs AGENTS.md docs/superpowers/specs/2026-10-06-postbode-core-design.md
git commit -m "test: live IMAP tests against Dovecot with and without MOVE"
```

---

### Task 4: Live sync and CLI tests

**Files:**
- Modify: `tests/imap_live.rs` only. If a test exposes an engine bug, stop and report it as DONE_WITH_CONCERNS with the failing test; don't fix the engine inside this task.

**Interfaces:**
- Consumes:
  - From Task 3: `host()`, `account()`, `connect()`, `mail()`, `PORT`, `ImapOps::delete_folder`.
  - `postbode::sync::{sync_all, run_once, Event}`, `postbode::store::Store`, `postbode::paths::Paths`, `postbode::config::Config`.
  - The signatures: `sync_all(&mut dyn MailOps, &Store) -> Result<(Vec<NewMessageRef>, Vec<String>), SyncError>`, `run_once(&AccountConfig, &Paths, &Sender<Event>) -> Result<(), SyncError>`, `Store::open(&Path)`, `Store::folder(&str) -> Result<Option<Folder>, _>`, `Store::messages_in_folder(&str) -> Result<Vec<Message>, _>`, `Config::save(&self, &Path)`.
- Produces: the regression test parked in plan 2: `sync` exits non-zero when a rule fails on one message.

- [ ] **Step 1: Write the tests.** Append to `tests/imap_live.rs`, and add `use std::path::Path;`, `use postbode::config::Config;`, `use postbode::paths::Paths;`, `use postbode::store::Store;` and `use postbode::sync::{self, Event};` at the top:

```rust
/// Mail newer than a rule's first pass is what the rule acts on; INTERNALDATE has one-second resolution.
fn wait_past_the_rule_clock() {
    std::thread::sleep(Duration::from_millis(1100));
}

fn postbode(home: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_postbode"))
        .args(args)
        .env("POSTBODE_HOME", home)
        .env("RUST_LOG", "error")
        .output()
        .unwrap()
}

fn home_with(account: &AccountConfig, rules: &str) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::under(home.path());
    Config { accounts: vec![account.clone()] }.save(&paths.config_file()).unwrap();
    if !rules.is_empty() {
        std::fs::write(paths.rules_file(), rules).unwrap();
    }
    home
}

#[test]
fn sync_resets_a_folder_whose_uidvalidity_changed() {
    let Some(host) = host() else { return };
    let mut ops = connect(&account(&host, PORT, "uidvalidity"));
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("mail.db")).unwrap();
    ops.create_folder("Lists").unwrap();
    ops.append("Lists", &mail("before"), &[]).unwrap();
    sync::sync_all(&mut ops, &store).unwrap();
    let before = store.folder("Lists").unwrap().unwrap().uidvalidity;

    ops.select("INBOX").unwrap();
    ops.delete_folder("Lists").unwrap();
    // Dovecot derives UIDVALIDITY from the clock.
    std::thread::sleep(Duration::from_millis(1100));
    ops.create_folder("Lists").unwrap();
    ops.append("Lists", &mail("after"), &[]).unwrap();
    let (_, errors) = sync::sync_all(&mut ops, &store).unwrap();
    assert!(errors.is_empty(), "{errors:?}");

    assert_ne!(store.folder("Lists").unwrap().unwrap().uidvalidity, before);
    let subjects: Vec<Option<String>> = store
        .messages_in_folder("Lists")
        .unwrap()
        .into_iter()
        .map(|m| m.subject)
        .collect();
    assert_eq!(subjects, vec![Some("after".to_string())]);
}

#[test]
fn rule_delete_keeps_a_backup_then_expunges() {
    let Some(host) = host() else { return };
    let account = account(&host, PORT, "rule-delete");
    let home = home_with(
        &account,
        "[[rules]]\nname = \"codes\"\nmatch.subject = { contains = \"sign-in code\" }\nactions = [\"delete\"]\n",
    );
    let paths = Paths::under(home.path());
    let (events, received) = std::sync::mpsc::channel();
    sync::run_once(&account, &paths, &events).unwrap();
    wait_past_the_rule_clock();
    connect(&account).append("INBOX", &mail("Your sign-in code"), &[]).unwrap();
    sync::run_once(&account, &paths, &events).unwrap();
    drop(events);
    let errors: Vec<Event> = received
        .iter()
        .filter(|e| matches!(e, Event::Error { .. }))
        .collect();
    assert!(errors.is_empty(), "{errors:?}");

    let backups = std::fs::read_dir(paths.trash_dir(&account.name)).unwrap().count();
    assert_eq!(backups, 1, "expected one .eml backup");
    let mut ops = connect(&account);
    for folder in ["INBOX", "Trash"] {
        ops.select(folder).unwrap();
        assert!(ops.fetch_new(1).unwrap().is_empty(), "{folder} is not empty");
    }
}

#[test]
fn cli_delete_moves_to_the_trash_folder() {
    let Some(host) = host() else { return };
    let account = account(&host, PORT, "cli-delete");
    let home = home_with(&account, "");
    connect(&account).append("INBOX", &mail("old newsletter"), &[]).unwrap();
    for args in [&["sync"][..], &["delete", "1"][..]] {
        let out = postbode(home.path(), args);
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    let mut ops = connect(&account);
    ops.select("INBOX").unwrap();
    assert!(ops.fetch_new(1).unwrap().is_empty(), "still in INBOX");
    ops.select("Trash").unwrap();
    assert_eq!(ops.fetch_new(1).unwrap().len(), 1);
}

#[test]
fn sync_exits_nonzero_when_a_rule_fails_on_a_message() {
    let Some(host) = host() else { return };
    let account = account(&host, PORT, "cli-sync-error");
    // Dovecot refuses a 300-character mailbox name, so the move fails for this message only.
    let target = "x".repeat(300);
    let home = home_with(
        &account,
        &format!("[[rules]]\nname = \"impossible\"\nmatch.subject = {{ contains = \"move me\" }}\nactions = [{{ move = \"{target}\" }}]\n"),
    );
    let first = postbode(home.path(), &["sync"]);
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    wait_past_the_rule_clock();
    connect(&account).append("INBOX", &mail("please move me"), &[]).unwrap();
    let out = postbode(home.path(), &["sync"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.contains("sync failed for at least one account or folder"), "{stderr}");
}
```

If Dovecot accepts the 300-character name, use `"bad*name"` (IMAP wildcards are refused as mailbox names), update the comment, and say which one you used in your report. Don't weaken the assertions.

- [ ] **Step 2: Run them against the containers.**

Run: `docker compose -f tests/dovecot/compose.yml up -d && POSTBODE_TEST_IMAP_HOST=localhost cargo test --test imap_live`
Expected: 11 passed.

- [ ] **Step 3: Run the gate and commit.**

```bash
git add tests/imap_live.rs
git commit -m "test: live sync, rule delete, UIDVALIDITY and CLI exit-code tests"
```

---

### Task 5: File safety follow-ups (lock mode, symlinked files, purge)

**Files:**
- Modify: `src/rules/edit.rs` (`lock`), `src/paths.rs` (`write_atomic`), `src/trash.rs` (`purge`), and their test modules.

**Interfaces:**
- Consumes: nothing new.
- Produces: no signature changes. `write_atomic` writes through a symlink and errors on a dangling one. `purge` skips files it cannot remove instead of aborting.

- [ ] **Step 1: Write the failing tests.**

`src/rules/edit.rs` tests:

```rust
    #[cfg(unix)]
    #[test]
    fn lock_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, path) = rules_file("");
        let lock_path = path.parent().unwrap().join(".rules.lock");
        std::fs::write(&lock_path, "").unwrap();
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        drop(lock(&path).unwrap());
        let mode = std::fs::metadata(&lock_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
```

`src/paths.rs` tests:

```rust
    #[cfg(unix)]
    #[test]
    fn write_atomic_writes_through_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("dotfiles/rules.toml");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "old").unwrap();
        let link = root.path().join("config/rules.toml");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, b"new").unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_refuses_a_dangling_symlink() {
        let root = tempfile::tempdir().unwrap();
        let link = root.path().join("rules.toml");
        std::os::unix::fs::symlink(root.path().join("missing/rules.toml"), &link).unwrap();
        assert!(write_atomic(&link, b"new").is_err());
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
    }
```

`src/trash.rs` tests:

```rust
    #[test]
    fn purge_skips_what_it_cannot_remove_and_continues() {
        let dir = tempfile::tempdir().unwrap();
        let trash = Trash::new(dir.path().to_path_buf());
        // A directory with a backup's name cannot be removed with remove_file.
        std::fs::create_dir(dir.path().join("1000-INBOX-9.eml")).unwrap();
        trash.save("INBOX", 1, b"a", 1000).unwrap();
        assert_eq!(trash.purge(3000, 6000).unwrap(), 1);
        assert!(!dir.path().join("1000-INBOX-1.eml").exists());
    }
```

- [ ] **Step 2: Run the tests and confirm they fail.**

Run: `cargo test --lib lock_file_is_private write_atomic_ purge_skips`
Expected: the lock mode is 0o644; the symlink is replaced by a regular file; the dangling symlink write succeeds; purge returns an error.

- [ ] **Step 3: Implement.**

In `edit.rs` `lock`, after `.open(...)?`:

```rust
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
```

In `paths.rs`, rename the current body of `write_atomic` to a private `write_file_atomic(path, bytes)`, and remove the `create_private_dir(dir)?` call from it. The new `write_atomic`:

```rust
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // A symlink (say into a dotfiles repo) is written through, not replaced; a dangling one fails in canonicalize.
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return write_file_atomic(&fs::canonicalize(path)?, bytes);
    }
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent"))?;
    create_private_dir(dir)?;
    write_file_atomic(path, bytes)
}
```

`write_file_atomic` still computes `dir` from `path.parent()` for the temp file and the directory fsync.

In `trash.rs`, `purge`:

```rust
    pub fn purge(&self, retention_secs: i64, now: i64) -> io::Result<usize> {
        let mut removed = 0;
        for entry in self.list()? {
            if now - entry.saved_at <= retention_secs {
                continue;
            }
            match std::fs::remove_file(&entry.path) {
                Ok(()) => removed += 1,
                // A concurrent `trash restore` may have taken it already.
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => log::warn!("{}: could not purge: {e}", entry.path.display()),
            }
        }
        Ok(removed)
    }
```

- [ ] **Step 4: Run the tests and confirm they pass.**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add src/rules/edit.rs src/paths.rs src/trash.rs
git commit -m "fix: private rules lock, write through symlinked config, keep purging past a bad file"
```

---

### Task 6: CLI follow-ups (keyring hint, offline `search --bodies`)

**Files:**
- Modify: `src/credentials.rs`, `src/cli/mod.rs` (`fetch_missing_bodies`), `tests/cli.rs`.

**Interfaces:**
- Consumes: the `seeded_home(&[Message]) -> (TempDir, Store)` and `message(uid, from, subject) -> Message` fixtures in `tests/cli.rs`. Their account points at the unreachable `127.0.0.1:1` with `password = { command = "printf x" }`.
- Produces: the private `credentials::hint_if_slow<T>(delay: Duration, hint: impl FnOnce() + Send + 'static, work: impl FnOnce() -> T) -> T`.

- [ ] **Step 1: Write the failing tests.**

`src/credentials.rs` tests (add a test module if none exists):

```rust
    #[test]
    fn hint_if_slow_fires_only_for_slow_work() {
        use std::sync::mpsc;
        use std::time::Duration;

        let (tx, rx) = mpsc::channel();
        let slow = hint_if_slow(
            Duration::from_millis(20),
            move || tx.send("slow").unwrap(),
            || {
                std::thread::sleep(Duration::from_millis(200));
                1
            },
        );
        assert_eq!(slow, 1);
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Ok("slow"));

        let (tx, rx) = mpsc::channel();
        assert_eq!(hint_if_slow(Duration::from_millis(500), move || tx.send("fast").unwrap(), || 2), 2);
        assert!(rx.recv_timeout(Duration::from_millis(800)).is_err());
    }
```

`tests/cli.rs`:

```rust
#[test]
fn search_bodies_works_offline() {
    let mut stored = message(42, "billing@example.com", "Your invoice");
    stored.body_text = Some("the total is due".into());
    let (home, _store) = seeded_home(&[stored]);
    let out = postbode(home.path(), &["search", "--bodies", "invoice"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(!stderr.contains("could not connect"), "connected with nothing to fetch: {stderr}");

    let (home, _store) = seeded_home(&[message(43, "friend@example.com", "Lunch?")]);
    let out = postbode(home.path(), &["search", "--bodies", "Lunch"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("could not connect"), "{stderr}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("INBOX/43"));
}
```

- [ ] **Step 2: Run the tests and confirm they fail.**

Run: `cargo test --lib credentials && cargo test --test cli search_bodies_works_offline`
Expected: `hint_if_slow` is not defined; the CLI test fails, either because the first search prints a connect error or because the second exits non-zero.

- [ ] **Step 3: Implement.**

In `credentials.rs`:

```rust
/// Runs `work`, calling `hint` once it has taken longer than `delay`.
fn hint_if_slow<T>(delay: Duration, hint: impl FnOnce() + Send + 'static, work: impl FnOnce() -> T) -> T {
    let (done, waiting) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if waiting.recv_timeout(delay) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
            hint();
        }
    });
    let result = work();
    drop(done);
    result
}
```

In `resolve`, wrap the keyring read:

```rust
        PasswordSource::Keyring { .. } => {
            let entry = keyring_entry(&account.name)?;
            let name = account.name.clone();
            // macOS asks again for Keychain access after each new unsigned binary, and the prompt may sit behind other windows.
            let password = hint_if_slow(
                Duration::from_secs(2),
                move || log::info!("{name}: waiting for the OS keyring; answer its password prompt if one is showing"),
                || entry.get_password(),
            );
            match password {
                Ok(password) => Ok(Secret::new(password)),
                Err(e) => Err(map_keyring_error(e, &account.name)),
            }
        }
```

Add `use std::time::Duration;`.

In `src/cli/mod.rs` `fetch_missing_bodies`, connect only when something is missing, and fall back to the local search when the server can't be reached:

```rust
/// Fetches the bodies `search --bodies` needs; without a connection the search uses the bodies already stored.
fn fetch_missing_bodies(
    account: &AccountConfig,
    store: &Store,
    folder: Option<&str>,
) -> Result<()> {
    let folders = match folder {
        Some(folder) => vec![folder.to_string()],
        None => store.folders()?.into_iter().map(|f| f.name).collect(),
    };
    let mut missing = Vec::new();
    for folder in folders {
        if store.messages_in_folder(&folder)?.iter().any(|m| m.body_text.is_none()) {
            missing.push(folder);
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    let mut ops = match sync::connect(account) {
        Ok(ops) => ops,
        Err(e) => {
            eprintln!(
                "{}: could not connect ({}); searching the bodies already stored",
                account.name,
                clean(&format!("{e:#}"), false)
            );
            return Ok(());
        }
    };
    for folder in missing {
        // the existing per-folder announce + fetch_bodies loop, unchanged
    }
    Ok(())
}
```

- [ ] **Step 4: Run the tests and confirm they pass.**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add src/credentials.rs src/cli/mod.rs tests/cli.rs
git commit -m "fix: hint when the keyring waits on a prompt, search bodies offline"
```

---

### Task 7: Dev tooling and the CI workflow

**Files:**
- Modify: `.mise.toml`, `AGENTS.md`, `README.md` (badge line only), spec §14
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `tests/dovecot/compose.yml` (ports 10993 and 11993) from Task 3, and the live tests' `POSTBODE_TEST_IMAP_HOST` contract (empty means skip).
- Produces: the `ci.yml` job names `lint` and `test`, which Renovate and branch protection may reference. `actionlint` is available through mise for Tasks 8 and 9.

- [ ] **Step 1: Add the dev tools through mise.** `mise use` writes the right backend key for each tool:

Run:

```bash
mise use github:kunobi-ninja/kache@1.0.0
mise use cargo-nextest@latest
mise use actionlint@latest
mise install && kache --version && cargo nextest --version && actionlint --version
```

Expected: three versions printed, and `.mise.toml` gains the three entries with pinned versions. If `cargo-nextest` or `actionlint` is not in the mise registry, use `github:nextest-rs/nextest` or `github:rhysd/actionlint`, and note it in the report.

Do NOT run `kache init`: it edits `~/.cargo/config.toml`, which is outside the repo.

- [ ] **Step 2: Write `.github/workflows/ci.yml`:**

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

permissions:
  contents: read

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always

jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v6
        with:
          persist-credentials: false
      - run: rustup toolchain install
      - uses: kunobi-ninja/kache-action@v1
        with:
          save-cache: ${{ github.ref == 'refs/heads/main' }}
          pr-comment: false
      - uses: Swatinem/rust-cache@v2
        with:
          cache-targets: false
          save-if: ${{ github.ref == 'refs/heads/main' }}
      - uses: taiki-e/install-action@v2
        with:
          tool: cargo-audit,cargo-machete
      - run: cargo fmt --check
      - run: cargo clippy --all-targets --all-features -- -D warnings
      - run: cargo machete
      - run: cargo audit

  test:
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    env:
      # The Dovecot containers only run on Linux; empty skips the live tests elsewhere.
      POSTBODE_TEST_IMAP_HOST: ${{ runner.os == 'Linux' && 'localhost' || '' }}
    steps:
      - uses: actions/checkout@v6
        with:
          persist-credentials: false
      - run: rustup toolchain install
      - uses: kunobi-ninja/kache-action@v1
        with:
          save-cache: ${{ github.ref == 'refs/heads/main' }}
          pr-comment: false
      - uses: Swatinem/rust-cache@v2
        with:
          cache-targets: false
          save-if: ${{ github.ref == 'refs/heads/main' }}
      - name: Start Dovecot
        if: runner.os == 'Linux'
        run: |
          docker compose -f tests/dovecot/compose.yml up -d
          for port in 10993 11993; do
            for _ in $(seq 30); do (echo > /dev/tcp/localhost/$port) 2>/dev/null && break; sleep 1; done
          done
      - run: cargo test --all-features
```

- [ ] **Step 3: Check the workflow and the commands it runs locally.**

Run: `actionlint .github/workflows/ci.yml`
Expected: no output.

Run: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features`
Expected: PASS.

Run: `cargo install cargo-machete --locked` (only if it is not already installed), then `cargo machete`.
Expected: no unused dependencies. If it flags a dependency that is used (for example one only referenced through a path such as `rustls::`), add it to `[package.metadata.cargo-machete] ignored = [...]` in `Cargo.toml` with a one-line comment saying why. If it flags a genuinely unused dependency, remove it.

Run: `cargo audit`
Expected: no vulnerabilities. For each advisory, report the crate and the advisory ID. Fix by updating the dependency where possible. Otherwise add `.cargo/audit.toml` with `[advisories] ignore = ["RUSTSEC-…"]` and a comment per entry saying why it does not apply. Never ignore an advisory without a reason.

- [ ] **Step 4: Docs.**
  - **README.md:** insert a CI badge on the line before the license badge, outside the `index.md` content:

    ```markdown
    [![CI](https://github.com/pataar/postbode/actions/workflows/ci.yml/badge.svg)](https://github.com/pataar/postbode/actions/workflows/ci.yml)
    ```

  - **AGENTS.md:** add a section:

    ```markdown
    ## Build speed
    `mise install` brings kache, cargo-nextest and actionlint. Run `kache init` once per machine to make kache your `RUSTC_WRAPPER`; it edits your own `~/.cargo/config.toml`, so the repo does not do it for you. Edit loop: `cargo check`, `cargo nextest run`.
    ```

  - **Spec §14:** replace the "Build speed" bullet:

    ```markdown
    - Build speed: dependency weight is watched with `cargo build --timings` and `cargo-machete` in CI. `.mise.toml` installs kache. Each developer runs `kache init` once, which makes it the `RUSTC_WRAPPER` in their own Cargo config: compiler outputs are content-addressed and shared across git worktrees, restored with reflinks on APFS, so a fresh worktree starts warm. No mold: Rust's default linker on x86_64 Linux has been lld since 1.90. CI uses `kunobi-ninja/kache-action` through the GitHub cache service, plus `Swatinem/rust-cache` for the registry and git deps only, both saving on pushes to `main`. Edit loop is `cargo check` and `cargo nextest`.
    ```

    And replace the "GitHub Actions `ci.yml`" bullet:

    ```markdown
    - GitHub Actions `ci.yml`: fmt, clippy `-D warnings`, `cargo-machete` and `cargo audit` on Ubuntu, and tests on `ubuntu-latest` and `macos-latest`. The Ubuntu test job starts `tests/dovecot/compose.yml` for the live IMAP tests; a compose step rather than a service container, so CI and local runs share one config. Concurrency with cancel-in-progress, `permissions: contents: read`.
    ```

- [ ] **Step 5: Commit.**

```bash
git add .mise.toml .github/workflows/ci.yml AGENTS.md README.md Cargo.toml docs/superpowers/specs/2026-10-06-postbode-core-design.md
git commit -m "ci: fmt, clippy, machete, audit and tests on Linux and macOS with live IMAP"
```

(Add `.cargo/audit.toml` only if Step 3 created it.)

---

### Task 8: Releases with release-plz and dist

**Files:**
- Modify: `Cargo.toml` (package metadata, `include`, `[profile.dist]`), `.mise.toml`, `AGENTS.md`, spec §15
- Create: `release-plz.toml`, `.github/workflows/release-plz.yml`, `dist-workspace.toml`, `.github/workflows/release.yml` (generated)

**Interfaces:**
- Consumes: `actionlint` from Task 7.
- Produces:
  - A tag format of `v{version}`; release-plz pushes the tag and dist's `release.yml` builds on it.
  - Repo secrets the user must create: `RELEASE_PLZ_TOKEN` and `HOMEBREW_TAP_TOKEN`.

- [ ] **Step 1: Package metadata and contents.** In `Cargo.toml` `[package]`, after `repository`:

```toml
keywords = ["cli", "email", "imap", "mail", "rules"]
categories = ["command-line-utilities", "email"]
# Only what the build needs; keeps the test CA key, design docs and CI config out of the published crate.
include = ["/Cargo.lock", "/LICENSE-*", "/README.md", "/docs/src/agent-guide.md", "/migrations/**", "/src/**"]
```

After `[profile.release]`:

```toml
[profile.dist]
inherits = "release"
```

Run: `cargo package --list --allow-dirty`
Expected: only `Cargo.toml`, `Cargo.toml.orig`, `Cargo.lock`, the licenses, `README.md`, `docs/src/agent-guide.md`, `migrations/…` and `src/…`. No `tests/`, `.github/` or `docs/superpowers/`.

Run: `cargo package --allow-dirty`
Expected: the packaged crate builds (cargo verifies it). If it fails on a missing embedded file, add that file to `include`. The current embeds are `include_dir!("$CARGO_MANIFEST_DIR/migrations")` and `include_str!("../../docs/src/agent-guide.md")`.

- [ ] **Step 2: Add release-plz.** Create `release-plz.toml`:

```toml
[workspace]
# dist creates the GitHub release from the tag release-plz pushes.
git_release_enable = false
git_tag_name = "v{{ version }}"
```

Create `.github/workflows/release-plz.yml`:

```yaml
name: Release-plz

on:
  push:
    branches: [main]

permissions: {}

jobs:
  release:
    name: Publish and tag
    if: github.repository_owner == 'pataar'
    runs-on: ubuntu-latest
    permissions:
      contents: write
      id-token: write
    steps:
      - uses: actions/checkout@v6
        with:
          fetch-depth: 0
          persist-credentials: false
      - run: rustup toolchain install
      - uses: release-plz/action@v0.5
        with:
          command: release
        env:
          # A tag pushed with the default GITHUB_TOKEN would not start release.yml.
          GITHUB_TOKEN: ${{ secrets.RELEASE_PLZ_TOKEN }}

  release-pr:
    name: Release PR
    if: github.repository_owner == 'pataar'
    runs-on: ubuntu-latest
    permissions:
      contents: write
      pull-requests: write
    concurrency:
      group: release-plz-${{ github.ref }}
      cancel-in-progress: false
    steps:
      - uses: actions/checkout@v6
        with:
          fetch-depth: 0
          persist-credentials: false
      - run: rustup toolchain install
      - uses: release-plz/action@v0.5
        with:
          command: release-pr
        env:
          # PRs opened with the default GITHUB_TOKEN would not run CI.
          GITHUB_TOKEN: ${{ secrets.RELEASE_PLZ_TOKEN }}
```

crates.io publishing uses trusted publishing (OIDC via `id-token: write`), so there is no `CARGO_REGISTRY_TOKEN`.

- [ ] **Step 3: Add dist.**

Run: `mise use github:axodotdev/cargo-dist@0.33.0 && mise install && dist --version`
Expected: `dist 0.33.0` (or `cargo-dist 0.33.0`).

Create `dist-workspace.toml`:

```toml
[workspace]
members = ["cargo:."]

[dist]
cargo-dist-version = "0.33.0"
ci = "github"
installers = ["homebrew"]
tap = "pataar/homebrew-tap"
publish-jobs = ["homebrew"]
targets = ["aarch64-apple-darwin", "aarch64-unknown-linux-gnu", "x86_64-apple-darwin", "x86_64-unknown-linux-gnu"]
pr-run-mode = "plan"
install-updater = false
```

Run: `dist generate && dist plan`
Expected:
- `.github/workflows/release.yml` is written.
- `dist plan` lists the four targets, a `postbode` Homebrew formula and the announcement tag format `v0.1.0`.
- If `dist generate` asks to rewrite `Cargo.toml` profiles or other config, accept only changes to `[profile.dist]`, and report them.

Run: `actionlint .github/workflows/release-plz.yml .github/workflows/release.yml`
Expected: no errors in `release-plz.yml`. `release.yml` is generated: report any findings there, but don't hand-edit it, because `dist generate` would overwrite the edits.

- [ ] **Step 4: Document the human steps.** Add to `AGENTS.md`:

```markdown
## Releasing
Conventional commits on `main` drive everything. release-plz keeps a release PR open; merging it publishes to crates.io and pushes the `vX.Y.Z` tag, and dist's `release.yml` builds the binaries, creates the GitHub release and updates `pataar/homebrew-tap`. Regenerate `release.yml` with `dist generate` after changing `dist-workspace.toml`; never edit it by hand.

One-time setup, by the repo owner:
1. `pataar/homebrew-tap` already exists (it also holds `gast`); dist adds `Formula/postbode.rb` beside the other formulas.
2. Add repo secret `HOMEBREW_TAP_TOKEN` to `pataar/postbode`: a fine-grained token with Contents read/write on `pataar/homebrew-tap` (the token gast uses for its tap works too).
3. Add repo secret `RELEASE_PLZ_TOKEN`: a fine-grained token with Contents and Pull requests read/write on `pataar/postbode`.
4. Publish 0.1.0 by hand (crates.io requires the first publish with a token): `cargo publish`, then `git tag v0.1.0 && git push origin v0.1.0` to run the first dist release.
5. On crates.io, add a trusted publisher for `postbode`: repository `pataar/postbode`, workflow `release-plz.yml`.
6. Install the Mend Renovate GitHub App on the repo.
```

- [ ] **Step 5: Update spec §15.**
  - Replace "`dist.toml` is the only config." with "`dist-workspace.toml` is the only config, and `dist generate` writes the workflow from it."
  - After the release-plz paragraph, add: "release-plz does not create the GitHub release (`git_release_enable = false`); dist does, from the tag. release-plz pushes with a `RELEASE_PLZ_TOKEN` fine-grained token, because tags and PRs created with the default `GITHUB_TOKEN` start no other workflows. The package `include` list keeps tests, the test CA and the design docs out of the published crate."
  - In "Human steps, one time":
    - Drop "create the tap repo": `pataar/homebrew-tap` already exists and is shared with gast.
    - Add the `RELEASE_PLZ_TOKEN` secret and the first manual tag push.
  - Add: "Release binaries are not code-signed yet. On macOS every new binary asks again for Keychain access. Developer ID signing is a later decision."

- [ ] **Step 6: Run the gate and commit.**

```bash
git add Cargo.toml Cargo.lock .mise.toml release-plz.toml dist-workspace.toml .github/workflows/release-plz.yml .github/workflows/release.yml AGENTS.md docs/superpowers/specs/2026-10-06-postbode-core-design.md
git commit -m "build: release with release-plz, dist and a Homebrew tap"
```

---

### Task 9: Renovate

**Files:**
- Create: `renovate.json`
- Modify: spec §15 (only if the config differs from its Renovate paragraph)

**Interfaces:**
- Consumes: the managers for `Cargo.toml`, `rust-toolchain.toml`, `.mise.toml` and `.github/workflows/*.yml`.
- Produces: nothing code depends on.

- [ ] **Step 1: Write `renovate.json`:**

```json
{
  "$schema": "https://docs.renovatebot.com/renovate-schema.json",
  "extends": [
    "config:recommended",
    ":maintainLockFilesMonthly",
    ":semanticCommits",
    ":semanticCommitTypeAll(chore)",
    "helpers:pinGitHubActionDigests"
  ],
  "minimumReleaseAge": "3 days",
  "osvVulnerabilityAlerts": true,
  "packageRules": [
    {
      "matchUpdateTypes": ["major"],
      "draftPR": true
    },
    {
      "description": "rust-toolchain.toml and .mise.toml move together",
      "matchDepNames": ["rust"],
      "groupName": "Rust toolchain"
    }
  ]
}
```

- [ ] **Step 2: Validate.**

Run: `pnpx --package renovate renovate-config-validator --strict renovate.json`
Expected: `Config validated successfully`. If `--strict` rewrites or rejects a preset name, follow the validator and report the change.

- [ ] **Step 3: Commit.**

```bash
git add renovate.json
git commit -m "chore: renovate config"
```

---

## Self-review notes (for the executor)

- **Spec coverage:**
  - §13's Dovecot bullet → Tasks 3 and 4.
  - §14:
    - toolchain files: already present;
    - kache and machete: Task 7;
    - CI matrix, audit and Dovecot: Task 7;
    - mold: dropped, with the reason recorded.
  - §15:
    - release-plz, trusted publishing, dist with four targets and the tap: Task 8;
    - Renovate: Task 9.
  - §12: IMAP timeouts → Task 1.
  - Linux keyring code was only ever compiled on macOS; the ubuntu CI job now compiles and tests it (plan-1 deferred minor).
- **Follow-ups already fixed in the code** (verified while writing this plan, so not repeated):
  - account names with `/`;
  - `AccountConfig` `deny_unknown_fields`;
  - config dir 0700 on `account add` (`write_atomic` creates private dirs);
  - `rules test` with an unknown name errors;
  - `write_atomic` fsyncs the directory.
