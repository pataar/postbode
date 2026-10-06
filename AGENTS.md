# Postbode — notes for agents

Spec: `docs/superpowers/specs/2026-10-06-postbode-core-design.md`. Read it before changing behaviour.

## Module map
- `actions` direct actions on chosen messages (mark, move, archive, delete); same `apply` as rules
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
