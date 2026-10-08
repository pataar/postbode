# Postbode — notes for agents

Spec: `docs/superpowers/specs/2026-10-06-postbode-core-design.md`. Read it before changing behaviour.

## Module map
- `actions` direct actions on chosen messages (mark, move, archive, delete); same `apply` as rules
- `config` accounts and identity (address + aliases)
- `credentials` keyring or password command; `Secret` has no Debug
- `daemon` the background process that owns sync and IMAP: `mod.rs` the server (socket, lock, log, idle exit), `wire` the JSON-lines protocol, `client` what front ends use (CLI, GUI, MCP; auto-starts the daemon), `service` launchd and systemd
- `engine` one sync thread per account, commands in and events out; applies a config change by restarting only the changed accounts, each once its old thread ended; what the daemon runs
- `gui` the egui window (feature `gui`, on by default), which talks to the daemon through `daemon::Client`: `app` holds state and is the only code that changes it, the view modules draw and return `UiAction`s; tests use `test_support::Fixture` with `egui_kittest`
- `help` help texts shared by the CLI and the MCP tool descriptions
- `mail_ops` `MailOps` trait; `imap.rs` is the real client; `RecordingOps` is the test fake
- `main` + `cli/` clap only; no logic
- `mcp` the MCP server (feature `mcp`, on by default): `tools` defines the tools and their scopes, `backend` is the only code touching store, rules or the daemon, `install` registers hosts
- `message` header parsing, thread id, body text
- `notify` desktop notifications for new mail
- `output` JSON rows shared by `--json` and MCP results
- `paths` platform dirs, atomic writes, whether this is the macOS app opened from Finder (`launched_as_app`)
- `rules` parse + validate + schema (`mod.rs`), pure `evaluate` (`engine.rs`), side effects (`apply.rs`), the only writer of rules.toml (`edit.rs`)
- `store` one SQLite file per account; migrations in `migrations/`
- `sync` per-account loop: sync folders in chunks, run rules, run commands, IDLE
- `trash` `.eml` backups before any rule delete

`tests/architecture.rs` checks the ownership rules above for `mcp`, `rules` (rules.toml writes), `gui` views and `cli`; allowed exceptions live there with a reason.

## Definition of done
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `cargo machete` and `cargo audit` all pass
- New logic has a test; a bug fix has a regression test
- No `unwrap`, `expect`, `panic!`, `todo!` or `unimplemented!` outside tests (clippy denies them); where a call cannot fail, a narrow `#[allow]` says why
- Platform coverage reported honestly: say "compiled on" vs "ran on"
- Every new dependency has a one-line reason in `Cargo.toml`
- Do not broaden a task into adjacent features
- After changing CLI flags or rule types, run `POSTBODE_BLESS=1 cargo test` to regenerate `docs/src/cli.md` and `docs/src/rules.schema.json`, and commit them
- After GUI work, run `TZ=UTC UPDATE_SNAPSHOTS=1 cargo test gui::snapshots` on Linux with lavapipe (`mesa-vulkan-drivers`) and look at the changed PNGs in `tests/snapshots/`
- Prose lives in `docs/src/`; `README.md` contains `docs/src/index.md` verbatim

## Privacy
- Never read message bodies from a user's store, and never log bodies or secrets
- Schema, counts and your own test fixtures are fine

## Live IMAP tests
`tests/imap_live.rs` runs against two local Dovecot servers and is skipped unless `POSTBODE_TEST_IMAP_HOST` is set:

```sh
docker compose -f tests/dovecot/compose.yml up -d
POSTBODE_TEST_IMAP_HOST=localhost cargo test --features testing --test imap_live
```

Port 10993 advertises MOVE and UIDPLUS, port 11993 neither. Each test logs in as its own throwaway user. `tests/dovecot/gen-certs.sh` regenerates the test-only CA.

## Property tests
`tests/fuzz.rs` feeds hostile input to the message, rules and wire parsers; the only assertion is no panic. Deeper search: `PROPTEST_CASES=20000 cargo test --release --test fuzz`. A finding becomes a named test; unfixed ones go in `tests/known_bugs.rs` as `#[ignore = "BUG: …"]`.

## Model-based sync tests
`tests/sync_model.rs` runs random server events and sync passes against `RecordingOps`; search deeper with `PROPTEST_CASES=2000 cargo test --features testing --test sync_model`.

## Build speed
`mise install` brings kache, cargo-nextest and actionlint. Run `kache init` once per machine to make kache your `RUSTC_WRAPPER`; it edits your own `~/.cargo/config.toml`, so the repo does not do it for you. Edit loop: `cargo check`, `cargo nextest run`.

Run `packaging/icons.sh` after changing `assets/icon.svg`, and commit what it writes.

## Releasing
Conventional commits on `main` drive everything. release-plz keeps a release PR open; merging it publishes to crates.io and pushes the `vX.Y.Z` tag, and dist's `release.yml` builds the binaries, creates the GitHub release and updates `pataar/homebrew-tap`. On the tag, `macos-app.yml` runs `packaging/macos/package.sh` on macOS (Postbode.app, signed and notarized, on a DMG), attaches the DMG to the release and writes `Casks/postbode.rb` in the tap from `packaging/macos/postbode.rb.in`; rerun it for a tag via workflow_dispatch. Regenerate `release.yml` with `dist generate` after changing `dist-workspace.toml`; never edit it by hand.

One-time setup, by the repo owner:
1. `pataar/homebrew-tap` already exists (it also holds `gast`); dist adds `Formula/postbode.rb` beside the other formulas.
2. Add repo secret `HOMEBREW_TAP_TOKEN` to `pataar/postbode`: a fine-grained token with Contents read/write on `pataar/homebrew-tap` (the token gast uses for its tap works too).
3. Add repo secret `RELEASE_PLZ_TOKEN`: a fine-grained token with Contents and Pull requests read/write on `pataar/postbode`.
4. Publish 0.1.0 by hand (crates.io requires the first publish with a token): `cargo publish`, then `git tag v0.1.0 && git push origin v0.1.0` to run the first dist release.
5. On crates.io, add a trusted publisher for `postbode`: repository `pataar/postbode`, workflow `release-plz.yml`.
6. For the macOS app, add repo secrets: `APPLE_CERTIFICATE` (base64 of the Developer ID Application .p12), `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD` (any random string), `APPLE_ID`, `APPLE_PASSWORD` (an app-specific password) and `APPLE_TEAM_ID`. Without them the DMG is signed ad hoc, and the cask keeps a `postflight` that clears the quarantine flag (the `@ADHOC@` lines in `packaging/macos/postbode.rb.in`); with them those lines are dropped. The packaging has never run on a Mac before its first PR run; check that DMG by hand once.
7. Under Settings → Code security, enable Dependabot alerts and security updates; version updates come from `.github/dependabot.yml`.
