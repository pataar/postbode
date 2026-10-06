# Postbode — notes for agents

Spec: `docs/superpowers/specs/2026-10-06-postbode-core-design.md`. Read it before changing behaviour.

## Module map
- `actions` direct actions on chosen messages (mark, move, archive, delete); same `apply` as rules
- `paths` platform dirs, atomic writes
- `config` accounts and identity (address + aliases)
- `credentials` keyring or password command; `Secret` has no Debug
- `engine` one sync thread per account, the per-account lock, commands in and events out; what `run` and the GUI use
- `message` header parsing, thread id, body text
- `notify` desktop notifications for new mail
- `store` one SQLite file per account; migrations in `migrations/`
- `rules` parse + validate + schema (`mod.rs`), pure `evaluate` (`engine.rs`), side effects (`apply.rs`), the only writer of rules.toml (`edit.rs`)
- `mail_ops` `MailOps` trait; `imap.rs` is the real client; `RecordingOps` is the test fake
- `trash` `.eml` backups before any rule delete
- `sync` per-account loop: sync folders in chunks, run rules, run commands, IDLE
- `main` + `cli/` clap only; no logic

## Definition of done
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `cargo machete` and `cargo audit` all pass
- New logic has a test; a bug fix has a regression test
- Platform coverage reported honestly: say "compiled on" vs "ran on"
- Every new dependency has a one-line reason in `Cargo.toml`
- Do not broaden a task into adjacent features
- After changing CLI flags or rule types, run `POSTBODE_BLESS=1 cargo test` to regenerate `docs/src/cli.md` and `docs/src/rules.schema.json`, and commit them
- Prose lives in `docs/src/`; `README.md` contains `docs/src/index.md` verbatim

## Privacy
- Never read message bodies from a user's store, and never log bodies or secrets
- Schema, counts and your own test fixtures are fine

## Live IMAP tests
`tests/imap_live.rs` runs against two local Dovecot servers and is skipped unless `POSTBODE_TEST_IMAP_HOST` is set:

```sh
docker compose -f tests/dovecot/compose.yml up -d
POSTBODE_TEST_IMAP_HOST=localhost cargo test --test imap_live
```

Port 10993 advertises MOVE and UIDPLUS, port 11993 neither. Each test logs in as its own throwaway user. `tests/dovecot/gen-certs.sh` regenerates the test-only CA.

## Build speed
`mise install` brings kache, cargo-nextest and actionlint. Run `kache init` once per machine to make kache your `RUSTC_WRAPPER`; it edits your own `~/.cargo/config.toml`, so the repo does not do it for you. Edit loop: `cargo check`, `cargo nextest run`.

## Releasing
Conventional commits on `main` drive everything. release-plz keeps a release PR open; merging it publishes to crates.io and pushes the `vX.Y.Z` tag, and dist's `release.yml` builds the binaries, creates the GitHub release and updates `pataar/homebrew-tap`. Regenerate `release.yml` with `dist generate` after changing `dist-workspace.toml`; never edit it by hand.

One-time setup, by the repo owner:
1. `pataar/homebrew-tap` already exists (it also holds `gast`); dist adds `Formula/postbode.rb` beside the other formulas.
2. Add repo secret `HOMEBREW_TAP_TOKEN` to `pataar/postbode`: a fine-grained token with Contents read/write on `pataar/homebrew-tap` (the token gast uses for its tap works too).
3. Add repo secret `RELEASE_PLZ_TOKEN`: a fine-grained token with Contents and Pull requests read/write on `pataar/postbode`.
4. Publish 0.1.0 by hand (crates.io requires the first publish with a token): `cargo publish`, then `git tag v0.1.0 && git push origin v0.1.0` to run the first dist release.
5. On crates.io, add a trusted publisher for `postbode`: repository `pataar/postbode`, workflow `release-plz.yml`.
6. Under Settings → Code security, enable Dependabot alerts and security updates; version updates come from `.github/dependabot.yml`.
