# Postbode — notes for agents

Spec: `docs/superpowers/specs/2026-10-06-postbode-core-design.md`. Read it before changing behaviour.

## Module map
- `actions` direct actions on chosen messages (mark, move, archive, delete); same `apply` as rules
- `paths` platform dirs, atomic writes
- `config` accounts and identity (address + aliases)
- `credentials` keyring or password command; `Secret` has no Debug
- `message` header parsing, thread id, body text
- `store` one SQLite file per account; migrations in `migrations/`
- `rules` parse + validate + schema (`mod.rs`), pure `evaluate` (`engine.rs`), side effects (`apply.rs`), the only writer of rules.toml (`edit.rs`)
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
