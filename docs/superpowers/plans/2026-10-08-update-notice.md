# Update Notice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a newer stable Postbode release is out, the status bar shows a peach "↑ vX.Y.Z available" link to that release's GitHub notes, after the version label.

**Architecture:** A new `update` module (GUI feature only) holds pure helpers (version parsing and comparison, the once-a-day rule, the cache file) and one `check` function that takes the network fetch as a `fn` seam; the real `fetch` is a hand-written HTTP/1.0 GET over the `rustls` and `webpki-roots` the IMAP client already uses. `App` runs `check` once on a background thread at the first frame, the same way it runs `reconnect`, and the status bar draws the link from the result.

**Tech Stack:** Rust 2024, eframe/egui 0.36, egui_kittest, rustls 0.23 (`StreamOwned`, `ClientConnection`), webpki-roots 1, serde_json. No new dependencies.

**Spec:** design canvas https://claude.ai/artifact/B5W17bKLqpKUw9z3bCw65Q (Inbox · Mocha board, status bar) + docs/superpowers/specs/2026-10-06-postbode-core-design.md

## Decisions (the owner can veto any of these before Task 1)

1. **Postbode never updates itself.** Homebrew, cargo and the AppImage own the install; the notice only links to `https://github.com/pataar/postbode/releases/tag/vX.Y.Z`.
2. **Nothing shows when up to date, offline, or when the check fails.** Failures are silent apart from a `log::debug!` line.
3. **The check runs in the GUI process, not the daemon**, on a background thread, at most once a day. The last result and its time are cached in `<cache_dir>/update-check.json`, so restarting the window does not ask again. A failed fetch still counts as that day's check (no hammering while offline) and keeps the last known version.
4. **Source:** `GET https://api.github.com/repos/pataar/postbode/releases/latest`. That endpoint already excludes drafts and prereleases; the code checks both flags anyway and ignores any tag that is not plain `X.Y.Z` (with or without `v`). It compares numerically with `env!("CARGO_PKG_VERSION")`.
5. **On by default; `[ui] check_updates = false` turns it off.** The only data sent is a normal HTTPS request to `api.github.com` with `User-Agent: postbode/<version>`; GitHub sees the IP address, as with any request. Documented in `docs/src/accounts.md`. See open decision A below.
6. **No HTTP client dependency.** None is in `Cargo.toml`; the TLS stack is. A ~25-line HTTP/1.0 request over `rustls::StreamOwned` replaces `ureq` and its tree. HTTP/1.0 rules out chunked replies, so the body is everything after the headers. Checked on 2026-10-08: `curl --http1.0` against the endpoint returns `200`, `connection: close`, a plain body with `tag_name`, `draft`, `prerelease`.
7. **The arrow is drawn in monospace.** egui's default proportional fonts (Ubuntu-Light, NotoEmoji, emoji-icon-font) lack U+2191 "↑"; Hack, the monospace font, has it (checked in the cmap tables of `epaint_default_fonts-0.36.2`). Monospace also fits the terminal look.

### Open decisions

- **A. On by default in a privacy-minded mail client.** I would keep it on but am not sure: no account or mail data leaves, but it is a daily request to a third party the user did not ask for, and Homebrew and cargo users already learn about updates from their package manager. The users who gain most are AppImage and download users. Alternatives: off by default, or on only for the AppImage (`APPIMAGE` set). The plan implements on-by-default as asked; flipping it is one line in `UiConfig::default` plus the docs sentence.
- **B. HTTP/1.0 by hand vs. `ureq`.** If GitHub ever stops serving HTTP/1.0, the check fails silently and nobody notices. A `ureq` dependency would be sturdier but adds a crate tree for one GET a day.
- **C. Release timing.** dist creates the GitHub release and then pushes the Homebrew formula in the same run, so for a few minutes a Homebrew user can see a notice that `brew upgrade` cannot satisfy yet. Accepted.

## Scope

One PR off `origin/main`, three commits (one per task). It assumes the GUI redesign plan's Task 8 (the `v{version}` label in the status bar) has landed; Task 3 adds that one line itself if not.

Not in this plan: a check in the CLI, daemon or MCP server; dismissing or snoozing the notice; a changelog view; any self-update.

## Global Constraints

- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `cargo machete` and `cargo audit` all pass before the PR.
- No `unwrap`, `expect`, `panic!`, `todo!` or `unimplemented!` outside tests; where a call cannot fail, a narrow `#[allow]` says why.
- Every new dependency has a one-line reason in `Cargo.toml` (this plan adds none).
- Tests never touch the network: `update::check` takes the fetch as a `fn() -> anyhow::Result<String>`, and the GUI test `Fixture` replaces `App::fetch_release` before the first frame.
- GUI views draw and return `UiAction`s; only `app.rs` changes state (`tests/architecture.rs` checks this).
- Colours come from `theme::Palette`; the notice uses `highlight` (peach in Mocha, #fe640b in Latte).
- Never log bodies or secrets; the debug line carries only the error.
- Prose lives in `docs/src/`: the setting in `accounts.md`, the status bar in `gui.md`. `README.md` contains `docs/src/index.md` verbatim (not touched here).
- After GUI work, run `TZ=UTC UPDATE_SNAPSHOTS=1 cargo test gui::snapshots` on Linux with lavapipe; expect no changed PNGs (the test fixture never has a newer release).
- Branch: `git switch -c feat/update-notice --no-track origin/main`, first push `git push -u origin HEAD`. Conventional commits.
- Report platform coverage honestly: "compiled on" vs "ran on". `update::fetch` is not run by any test; say so in the PR.

## Review Focus

1. **Version `0.10.0` against `0.9.0`**: compared as numbers, so `0.10.0` is newer; a string compare would call it older. Test in Task 1 (`newer_compares_numbers_not_strings`).
2. **A prerelease, draft or hostile tag** (`v0.3.0-rc.1`, `draft: true`, `v1.2.3/../../evil`, a 30-digit number): never announced, and only numbers rebuilt as `X.Y.Z` ever reach the link. Test in Task 1 (`only_stable_plain_versions_are_announced`).
3. **Offline or rate-limited (403)**: no notice, no second request the same day, and a version learned yesterday is still shown. Test in Task 1 (`a_failed_fetch_counts_as_the_check_and_keeps_the_last_version`).
4. **The user upgraded since the cache was written** (cache says `0.3.0`, running `0.3.0`): nothing shows. Test in Task 1 (`a_cached_version_the_user_already_runs_is_not_announced`).
5. **A corrupt cache file, or a clock that went back past the cached time**: the check runs instead of being stuck until the clock catches up. Test in Task 1 (`due_when_missing_old_or_in_the_future` and `a_corrupt_cache_file_means_a_new_check`).

---

### Task 1: The `update` module

**Files:**
- Create: `src/update.rs`
- Modify: `src/lib.rs` (`#[cfg(feature = "gui")] pub mod update;` after `pub mod trash;`), `src/paths.rs` (`update_check` path), `AGENTS.md` (module map line)

**Interfaces:**
- Consumes: `crate::paths::{Paths, write_atomic}`; `rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned}`, `rustls::pki_types::ServerName` (rustls 0.23.45, pki-types 1.15.1, verified in `~/.cargo/registry/src`); `webpki_roots::TLS_SERVER_ROOTS`.
- Produces:
  - `Paths::update_check(&self) -> PathBuf` (`<cache_dir>/update-check.json`)
  - `update::Cached { pub checked_at: i64, pub latest: Option<String> }` (`Serialize`, `Deserialize`, `Debug`, `Clone`, `PartialEq`)
  - `update::parse_version(text: &str) -> Option<[u64; 3]>`
  - `update::latest_from_json(json: &str) -> Option<String>` (normalised `X.Y.Z`)
  - `update::newer(latest: &str, current: &str) -> bool`
  - `update::release_url(version: &str) -> String`
  - `update::due(cached: Option<&Cached>, now: i64) -> bool`
  - `update::load(path: &Path) -> Option<Cached>`, `update::save(path: &Path, cached: &Cached) -> io::Result<()>`
  - `update::check(cache: &Path, now: i64, current: &str, fetch: fn() -> anyhow::Result<String>) -> Option<String>` (the version to announce)
  - `update::fetch() -> anyhow::Result<String>` (the real request; untested)
  - `update::body_of(response: &str) -> anyhow::Result<&str>`

- [ ] **Step 1: Write the failing tests** in a new `src/update.rs`, and add `#[cfg(feature = "gui")] pub mod update;` to `src/lib.rs` after `pub mod trash;`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const DAY_AGO: i64 = 1_790_000_000;
    const NOW: i64 = DAY_AGO + DAY;

    fn release(tag: &str, draft: bool, prerelease: bool) -> String {
        format!(r#"{{"html_url":"https://github.com/pataar/postbode/releases/tag/{tag}","tag_name":"{tag}","draft":{draft},"prerelease":{prerelease},"assets":[]}}"#)
    }

    fn release_0_3_0() -> anyhow::Result<String> {
        Ok(release("v0.3.0", false, false))
    }

    fn release_5_0_0() -> anyhow::Result<String> {
        Ok(release("v5.0.0", false, false))
    }

    fn offline() -> anyhow::Result<String> {
        Err(anyhow::anyhow!("offline"))
    }

    fn cache_in(dir: &tempfile::TempDir) -> std::path::PathBuf {
        crate::paths::Paths::under(dir.path()).update_check()
    }

    #[test]
    fn parse_version_takes_plain_versions_with_or_without_v() {
        assert_eq!(parse_version("v0.3.0"), Some([0, 3, 0]));
        assert_eq!(parse_version("1.12.7"), Some([1, 12, 7]));
        assert_eq!(parse_version("v0.3.0-rc.1"), None);
        assert_eq!(parse_version("v1.2"), None);
        assert_eq!(parse_version("v1.2.3.4"), None);
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version(" v1.2.3"), None);
    }

    #[test]
    fn newer_compares_numbers_not_strings() {
        assert!(newer("0.10.0", "0.9.0"));
        assert!(newer("1.0.0", "0.99.99"));
        assert!(!newer("0.2.0", "0.2.0"));
        assert!(!newer("0.1.9", "0.2.0"));
        assert!(!newer("garbage", "0.2.0"));
    }

    #[test]
    fn only_stable_plain_versions_are_announced() {
        assert_eq!(latest_from_json(&release("v0.3.0", false, false)).as_deref(), Some("0.3.0"));
        assert_eq!(latest_from_json(&release("0.3.0", false, false)).as_deref(), Some("0.3.0"));
        assert_eq!(latest_from_json(&release("v0.3.0", true, false)), None);
        assert_eq!(latest_from_json(&release("v0.3.0", false, true)), None);
        assert_eq!(latest_from_json(&release("v0.3.0-rc.1", false, false)), None);
        assert_eq!(latest_from_json(&release("v1.2.3/../../evil", false, false)), None);
        assert_eq!(latest_from_json(&release("v123456789012345678901234567890.0.0", false, false)), None);
        assert_eq!(latest_from_json(r#"{"message":"API rate limit exceeded"}"#), None);
        assert_eq!(latest_from_json("<html>"), None);
        assert_eq!(release_url("0.3.0"), "https://github.com/pataar/postbode/releases/tag/v0.3.0");
    }

    #[test]
    fn due_when_missing_old_or_in_the_future() {
        let cached = |checked_at| Cached { checked_at, latest: None };
        assert!(due(None, NOW));
        assert!(!due(Some(&cached(NOW - 60 * 60)), NOW));
        assert!(due(Some(&cached(DAY_AGO)), NOW));
        assert!(due(Some(&cached(NOW + 60)), NOW), "a clock that went back must not block checks");
    }

    #[test]
    fn a_fresh_cache_answers_without_fetching() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        save(&cache, &Cached { checked_at: NOW - 60, latest: Some("0.3.0".into()) }).unwrap();
        assert_eq!(check(&cache, NOW, "0.2.0", release_5_0_0).as_deref(), Some("0.3.0"));
    }

    #[test]
    fn a_due_check_fetches_and_caches_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        assert_eq!(check(&cache, NOW, "0.2.0", release_0_3_0).as_deref(), Some("0.3.0"));
        assert_eq!(load(&cache), Some(Cached { checked_at: NOW, latest: Some("0.3.0".into()) }));
        assert_eq!(check(&cache, NOW + 60, "0.2.0", release_5_0_0).as_deref(), Some("0.3.0"));
        assert_eq!(check(&cache, NOW + DAY, "0.2.0", release_5_0_0).as_deref(), Some("5.0.0"));
    }

    #[test]
    fn a_failed_fetch_counts_as_the_check_and_keeps_the_last_version() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        assert_eq!(check(&cache, NOW, "0.2.0", offline), None);
        assert_eq!(load(&cache), Some(Cached { checked_at: NOW, latest: None }));
        save(&cache, &Cached { checked_at: DAY_AGO, latest: Some("0.3.0".into()) }).unwrap();
        assert_eq!(check(&cache, NOW, "0.2.0", offline).as_deref(), Some("0.3.0"));
        assert_eq!(load(&cache), Some(Cached { checked_at: NOW, latest: Some("0.3.0".into()) }));
    }

    #[test]
    fn a_cached_version_the_user_already_runs_is_not_announced() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        save(&cache, &Cached { checked_at: NOW - 60, latest: Some("0.3.0".into()) }).unwrap();
        assert_eq!(check(&cache, NOW, "0.3.0", release_5_0_0), None);
        assert_eq!(check(&cache, NOW, "0.4.0", release_5_0_0), None);
    }

    #[test]
    fn a_corrupt_cache_file_means_a_new_check() {
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(&dir);
        crate::paths::write_atomic(&cache, b"{not json").unwrap();
        assert_eq!(load(&cache), None);
        assert_eq!(check(&cache, NOW, "0.2.0", release_0_3_0).as_deref(), Some("0.3.0"));
    }

    #[test]
    fn body_of_takes_a_200_body_and_rejects_other_statuses() {
        let ok = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nconnection: close\r\n\r\n{\"tag_name\":\"v0.3.0\"}";
        assert_eq!(body_of(ok).unwrap(), "{\"tag_name\":\"v0.3.0\"}");
        let limited = "HTTP/1.1 403 rate limit exceeded\r\n\r\n{\"message\":\"API rate limit exceeded\"}";
        assert!(body_of(limited).unwrap_err().to_string().contains("403"));
        assert!(body_of("HTTP/1.1 200 OK\r\nContent-Type: application/json").is_err());
        assert!(body_of("").is_err());
    }
}
```

and in `src/paths.rs` tests, extend `under_root_lays_out_three_dirs` with:

```rust
        assert_eq!(p.update_check(), root.path().join("cache/update-check.json"));
```

- [ ] **Step 2: Run them**

Run: `cargo test --lib update:: paths::`
Expected: FAIL to compile (`parse_version`, `Cached`, `DAY`, `update_check` not defined).

- [ ] **Step 3: Implement**

`src/paths.rs`, after `daemon_log`:

```rust
    pub fn update_check(&self) -> PathBuf {
        self.cache_dir.join("update-check.json")
    }
```

`src/update.rs`, above the tests:

```rust
//! Whether a newer Postbode release is out: GitHub's latest release, asked at most once a day. Postbode never updates
//! itself; the window only links to the release notes.
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::paths::write_atomic;

const DAY: i64 = 24 * 60 * 60;
const HOST: &str = "api.github.com";
const LATEST: &str = "/repos/pataar/postbode/releases/latest";
const TIMEOUT: Duration = Duration::from_secs(10);

/// The last check: when it ran, and the newest stable version it saw as `X.Y.Z`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cached {
    pub checked_at: i64,
    pub latest: Option<String>,
}

#[derive(Deserialize)]
struct Release {
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    tag_name: String,
}

/// `X.Y.Z` with or without a leading `v`; anything else, prereleases included, is None.
pub fn parse_version(text: &str) -> Option<[u64; 3]> {
    let text = text.strip_prefix('v').unwrap_or(text);
    let mut parts = text.split('.').map(|part| part.parse::<u64>().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

/// The stable version in a `releases/latest` reply, rebuilt from its numbers so nothing else reaches a URL.
pub fn latest_from_json(json: &str) -> Option<String> {
    let release: Release = serde_json::from_str(json).ok()?;
    if release.draft || release.prerelease {
        return None;
    }
    let [major, minor, patch] = parse_version(&release.tag_name)?;
    Some(format!("{major}.{minor}.{patch}"))
}

pub fn newer(latest: &str, current: &str) -> bool {
    matches!((parse_version(latest), parse_version(current)), (Some(latest), Some(current)) if latest > current)
}

pub fn release_url(version: &str) -> String {
    format!("https://github.com/pataar/postbode/releases/tag/v{version}")
}

/// Due with no earlier check, after a day, or when the clock went back past the last one.
pub fn due(cached: Option<&Cached>, now: i64) -> bool {
    cached.is_none_or(|cached| now - cached.checked_at >= DAY || cached.checked_at > now)
}

pub fn load(path: &Path) -> Option<Cached> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

pub fn save(path: &Path, cached: &Cached) -> io::Result<()> {
    write_atomic(path, serde_json::to_string(cached)?.as_bytes())
}

/// The version to announce, if one newer than `current` is out. Calls `fetch` only when a check is due; a failed fetch
/// still counts as the day's check and keeps the version seen before.
pub fn check(cache: &Path, now: i64, current: &str, fetch: fn() -> anyhow::Result<String>) -> Option<String> {
    let cached = load(cache);
    let latest = if due(cached.as_ref(), now) {
        let latest = match fetch() {
            Ok(body) => latest_from_json(&body),
            Err(e) => {
                log::debug!("update check failed: {e:#}");
                cached.and_then(|cached| cached.latest)
            }
        };
        let fresh = Cached { checked_at: now, latest: latest.clone() };
        if let Err(e) = save(cache, &fresh) {
            log::debug!("could not save the update check: {e}");
        }
        latest
    } else {
        cached.and_then(|cached| cached.latest)
    };
    latest.filter(|latest| newer(latest, current))
}

/// Asks GitHub for the latest release over HTTP/1.0, which rules out a chunked reply.
pub fn fetch() -> anyhow::Result<String> {
    let tcp = TcpStream::connect((HOST, 443))?;
    tcp.set_read_timeout(Some(TIMEOUT))?;
    tcp.set_write_timeout(Some(TIMEOUT))?;
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::try_from(HOST)?;
    let mut tls = rustls::StreamOwned::new(rustls::ClientConnection::new(Arc::new(config), name)?, tcp);
    let request = format!(
        "GET {LATEST} HTTP/1.0\r\nHost: {HOST}\r\nUser-Agent: postbode/{}\r\nAccept: application/vnd.github+json\r\n\r\n",
        env!("CARGO_PKG_VERSION")
    );
    tls.write_all(request.as_bytes())?;
    let mut response = Vec::new();
    match tls.read_to_end(&mut response) {
        Ok(_) => {}
        // A server that closes without TLS close_notify; a cut-off body then fails to parse as JSON.
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof && !response.is_empty() => {}
        Err(e) => return Err(e.into()),
    }
    Ok(body_of(&String::from_utf8(response)?)?.to_string())
}

/// The body of a 200 reply; any other status is an error.
pub fn body_of(response: &str) -> anyhow::Result<&str> {
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("no end of headers"))?;
    let status = head.lines().next().unwrap_or_default();
    anyhow::ensure!(status.split(' ').nth(1) == Some("200"), "GitHub answered {status}");
    Ok(body)
}
```

Verified against source: `rustls::ClientConfig::builder()` uses the process-default provider, which is `ring` because only that feature is on (as `mail_ops/imap.rs` relies on); `ClientConnection::new(Arc<ClientConfig>, ServerName<'static>)` (`client/client_conn.rs:912`, re-exported at `rustls::ClientConnection`); `StreamOwned::new(conn, sock)` implements `Read` and `Write` (`stream.rs:183,217,248`); `ServerName: TryFrom<&'a str>` with `InvalidDnsNameError: std::error::Error` (pki-types `server_name.rs:117,315`); `impl From<serde_json::Error> for io::Error` (serde_json `error.rs:189`); `Option::is_none_or` is already used in this crate.

`AGENTS.md` module map, after `trash`:

```markdown
- `update` whether a newer release is out (GUI only): GitHub's latest release at most once a day, cached; never updates itself
```

- [ ] **Step 4: Run them**

Run: `cargo test --lib update:: paths:: && cargo clippy --all-targets --all-features -- -D warnings && cargo build --no-default-features`
Expected: PASS; the CLI-only build compiles without `update`.

- [ ] **Step 5: Commit**

```bash
git add src/update.rs src/lib.rs src/paths.rs AGENTS.md
git commit -m "feat: check GitHub for a newer release at most once a day"
```

---

### Task 2: The window runs the check, unless `[ui] check_updates = false`

**Files:**
- Modify: `src/config.rs` (`UiConfig`, tests), `src/gui/app.rs` (`UpdateCheck`, two `App` fields, `check_release`, `newer_release`, tests), `src/gui/test_support.rs` (stub the fetch), `docs/src/accounts.md` (Appearance section)

**Interfaces:**
- Consumes: `update::{check, fetch}`, `Paths::update_check` (Task 1); `crate::time::now() -> i64`.
- Produces:
  - `UiConfig.check_updates: bool` (default `true`)
  - `pub(crate) enum UpdateCheck { Off, Waiting, Running(Receiver<Option<String>>), Done(Option<String>) }` in `app.rs`
  - `App.fetch_release: fn() -> anyhow::Result<String>`, `App.update: UpdateCheck`
  - `App::newer_release(&self) -> Option<&str>`

- [ ] **Step 1: Write the failing tests**

In `config.rs` tests:

```rust
    #[test]
    fn check_updates_is_on_unless_turned_off() {
        assert!(Config::default().ui.check_updates);
        assert!(Config::parse(SAMPLE).unwrap().ui.check_updates);
        let theme_only = format!("{SAMPLE}\n[ui]\ntheme = \"dark\"\n");
        assert!(Config::parse(&theme_only).unwrap().ui.check_updates);
        let off = format!("{SAMPLE}\n[ui]\ncheck_updates = false\n");
        assert!(!Config::parse(&off).unwrap().ui.check_updates);
    }
```

In `app.rs` tests (add `use crate::update;` to the test imports):

```rust
    /// Steps frames until the background release check has answered.
    fn finish_update_check(harness: &mut egui_kittest::Harness<'_, App>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(harness.state().update, UpdateCheck::Done(_)) && Instant::now() < deadline {
            harness.step();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(harness.state().update, UpdateCheck::Done(_)));
    }

    #[test]
    fn a_cached_newer_release_reaches_the_window() {
        let fx = Fixture::new(&["work"]);
        let cached = update::Cached { checked_at: crate::time::now(), latest: Some("999.0.0".into()) };
        update::save(&fx.paths.update_check(), &cached).unwrap();
        let (mut harness, _wires) = fx.harness();
        finish_update_check(&mut harness);
        assert_eq!(harness.state().newer_release(), Some("999.0.0"));
    }

    #[test]
    fn a_failed_check_shows_nothing_and_is_remembered() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        finish_update_check(&mut harness);
        assert_eq!(harness.state().newer_release(), None);
        assert!(update::load(&fx.paths.update_check()).is_some());
    }

    #[test]
    fn check_updates_false_starts_no_check() {
        let fx = Fixture::new(&["work"]);
        fx.append_config("[ui]\ncheck_updates = false\n");
        let (mut harness, _wires) = fx.harness();
        harness.run();
        assert!(matches!(harness.state().update, UpdateCheck::Off));
        assert!(!fx.paths.update_check().exists());
    }
```

- [ ] **Step 2: Run them**

Run: `cargo test --lib config:: gui::app::`
Expected: FAIL to compile (`check_updates`, `UpdateCheck`, `newer_release` not defined).

- [ ] **Step 3: Implement**

`config.rs`: `UiConfig` drops `Default` from its derive, so the default turns the check on; fields stay alphabetical.

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiConfig {
    /// Ask GitHub once a day whether a newer release is out.
    #[serde(default = "default_true")]
    pub check_updates: bool,
    #[serde(default)]
    pub theme: Theme,
}

impl Default for UiConfig {
    fn default() -> UiConfig {
        UiConfig {
            check_updates: true,
            theme: Theme::default(),
        }
    }
}
```

`save_round_trip_leaves_out_a_default_ui_table` keeps passing: `is_default` compares against this `Default`.

`app.rs`, beside the other state types:

```rust
/// The release check: off in config, waiting for the first frame, running on its thread, or done with the version to
/// announce.
pub(crate) enum UpdateCheck {
    Off,
    Waiting,
    Running(Receiver<Option<String>>),
    Done(Option<String>),
}
```

`App` fields, in alphabetical place (`fetch_release` after `events`, `update` after `trash`):

```rust
    pub(crate) fetch_release: fn() -> anyhow::Result<String>,
    ...
    pub(crate) update: UpdateCheck,
```

`App::new`, in the struct literal:

```rust
            fetch_release: update::fetch,
            ...
            update: if config.ui.check_updates { UpdateCheck::Waiting } else { UpdateCheck::Off },
```

with `use crate::update;` at the top of `app.rs`.

`App::show`, after `self.keep_daemon(&ctx, now);`:

```rust
        self.check_release(&ctx);
```

and the methods, after `start_reconnect`:

```rust
    /// Starts the release check at the first frame, after the test fixture has swapped `fetch_release`, then takes its
    /// answer.
    fn check_release(&mut self, ctx: &egui::Context) {
        let next = match &self.update {
            UpdateCheck::Waiting => {
                let (done, answer) = mpsc::channel();
                let (cache, fetch, ctx) = (self.paths.update_check(), self.fetch_release, ctx.clone());
                std::thread::spawn(move || {
                    let _ = done.send(update::check(&cache, crate::time::now(), env!("CARGO_PKG_VERSION"), fetch));
                    ctx.request_repaint();
                });
                Some(UpdateCheck::Running(answer))
            }
            UpdateCheck::Running(answer) => match answer.try_recv() {
                Ok(newer) => Some(UpdateCheck::Done(newer)),
                Err(TryRecvError::Disconnected) => Some(UpdateCheck::Done(None)),
                Err(TryRecvError::Empty) => None,
            },
            UpdateCheck::Off | UpdateCheck::Done(_) => None,
        };
        if let Some(next) = next {
            self.update = next;
        }
    }

    /// The newer release to announce, once the check has found one.
    pub(crate) fn newer_release(&self) -> Option<&str> {
        match &self.update {
            UpdateCheck::Done(newer) => newer.as_deref(),
            _ => None,
        }
    }
```

`test_support.rs`, in `Fixture::harness` after the `app.reconnect` line (before the harness is built, since building runs the first frame):

```rust
        app.fetch_release = || Err(anyhow::anyhow!("no network in tests"));
```

`docs/src/accounts.md`, the Appearance section becomes:

````markdown
## Appearance

```toml
[ui]
check_updates = true
theme = "system"
```

`theme` is `"system"` (follow the OS, the default), `"light"` or `"dark"`. The mail window's theme switch writes it.

`check_updates` (on by default) lets the mail window ask GitHub at most once a day whether a newer release is out, and link to its release notes in the status bar. The request is a plain HTTPS request to `api.github.com` with the user agent `postbode/<version>`: it carries nothing about your accounts or mail, though GitHub sees your IP address as with any request. Postbode never updates itself; upgrade the way you installed it (`brew upgrade postbode`, `cargo install postbode`, a new AppImage). Set `check_updates = false` to turn the check off.
````

- [ ] **Step 4: Run them**

Run: `cargo test --lib config:: gui:: && cargo test --test architecture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs src/gui/app.rs src/gui/test_support.rs docs/src/accounts.md
git commit -m "feat(gui): check for a newer release once a day; [ui] check_updates turns it off"
```

---

### Task 3: The status bar link

**Files:**
- Modify: `src/gui/status.rs` (`update_link`, tests), `docs/src/gui.md` (Status bar section)

**Interfaces:**
- Consumes: `App::newer_release`, `UpdateCheck` (Task 2); `update::release_url` (Task 1); `theme::palette(ui) -> &'static Palette`, `Palette.highlight`; egui 0.36 `Ui::scope`, `Ui::visuals_mut`, `Ui::hyperlink_to(label: impl Into<WidgetText>, url: impl ToString) -> Response` (`egui-0.36.2/src/ui.rs:1795`). `Link` paints with `visuals().hyperlink_color` (`widgets/hyperlink.rs:47`), which is why the colour is set on the visuals, not on the `RichText`.
- Produces: nothing new for other tasks.

- [ ] **Step 1: Write the failing tests** in `status.rs` tests (add `use crate::gui::app::UpdateCheck;`)

```rust
    #[test]
    fn a_newer_release_links_to_its_notes() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        harness.state_mut().update = UpdateCheck::Done(Some("9.9.9".into()));
        harness.run();
        harness.get_by_label("↑ v9.9.9 available").click();
        harness.step();
        let opened: Vec<_> = harness
            .output()
            .platform_output
            .commands
            .iter()
            .filter_map(|command| match command {
                egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(opened, ["https://github.com/pataar/postbode/releases/tag/v9.9.9"]);
    }

    #[test]
    fn no_newer_release_shows_only_the_version() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        harness.state_mut().update = UpdateCheck::Done(None);
        harness.run();
        assert!(harness.query_by_label_contains("available").is_none());
        assert!(harness.query_by_label(&format!("v{}", env!("CARGO_PKG_VERSION"))).is_some());
    }
```

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::status`
Expected: FAIL, `a_newer_release_links_to_its_notes` finds no "↑ v9.9.9 available" (and, without the redesign's Task 8, the second test finds no version label).

- [ ] **Step 3: Implement**

In `status.rs` `show`, in the right-to-left block after the theme buttons, replace the redesign's `ui.weak(format!("v{}", env!("CARGO_PKG_VERSION")));` with the lines below (add both if that line is not there yet). Right-to-left places later widgets further left, so the link drawn first lands right of the version:

```rust
            if let Some(version) = app.newer_release() {
                update_link(ui, version);
            }
            ui.weak(format!("v{}", env!("CARGO_PKG_VERSION")));
```

and below `show`:

```rust
/// Monospace because the default proportional font lacks the arrow.
fn update_link(ui: &mut egui::Ui, version: &str) {
    let highlight = theme::palette(ui).highlight;
    ui.scope(|ui| {
        ui.visuals_mut().hyperlink_color = highlight;
        ui.hyperlink_to(
            egui::RichText::new(format!("↑ v{version} available")).monospace(),
            crate::update::release_url(version),
        );
    });
}
```

Update the module doc line to: `//! The bottom bar: a line per account from its latest activity, the history window, the theme switch, the version and a newer release.`

`docs/src/gui.md`, Status bar section, append to its paragraph:

```markdown
Next to the version, a link appears when a newer release is out; it opens that release's notes on GitHub. Postbode does not update itself, and `[ui] check_updates = false` in `config.toml` turns the daily check off (see [Accounts](accounts.md#appearance)).
```

- [ ] **Step 4: Run tests, the architecture check and the full gate**

Run: `cargo test --lib gui:: && cargo test --test architecture && cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && cargo machete && cargo audit`
Expected: PASS. `status.rs` stays a view: it reads `&App`, opens a URL through egui's output, and changes no state.

- [ ] **Step 5: Snapshots on Linux**: `TZ=UTC UPDATE_SNAPSHOTS=1 cargo test gui::snapshots`; expect no changed PNGs. Then commit

```bash
git add src/gui/status.rs docs/src/gui.md
git commit -m "feat(gui): link to a newer release from the status bar"
```

Run the app once by hand with a cache file that names a newer version (`{"checked_at":<now>,"latest":"9.9.9"}` in the cache dir's `update-check.json`) and look at the link in both themes; then delete the file and run once more with `RUST_LOG=postbode=debug` to see the real request succeed or log its failure. Report which platforms this ran on.
