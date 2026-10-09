# Contributing

Thanks for helping out. Bug reports, rule ideas and pull requests are all welcome. For anything bigger than a small fix, open an issue first so we can agree on the approach before you write the code.

## Setup

The toolchain is pinned in `rust-toolchain.toml`. With [mise](https://mise.jdx.dev), `mise install` brings the pinned Rust plus cargo-nextest, kache and actionlint. You also need `cargo-machete` and `cargo-audit`:

```sh
cargo install cargo-machete cargo-audit
```

Fast edit loop: `cargo check` and `cargo nextest run`.

## Before you open a pull request

All of these must pass; CI runs the same set:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo machete
cargo audit
```

- New logic comes with a test; a bug fix comes with a regression test.
- Every new dependency gets a one-line comment in `Cargo.toml` saying why it is needed.
- After changing CLI flags or rule types, run `POSTVAK_BLESS=1 cargo test` and commit the regenerated `docs/src/cli.md` and `docs/src/rules.schema.json`.
- User-facing prose lives in `docs/src/`. `README.md` contains `docs/src/index.md` verbatim, so edit both together.
- Keep a pull request to one change.

## Property tests

`tests/fuzz.rs` throws arbitrary and mutated input at the message, rules and daemon wire parsers and checks that none of them panic. `cargo test` runs a few cases; for a deeper search:

```sh
PROPTEST_CASES=20000 cargo test --release --test fuzz
```

If it finds a panic, add the input as a named test next to the fix.

## Live IMAP tests

`tests/imap_live.rs` runs against two local Dovecot servers and is skipped unless `POSTVAK_TEST_IMAP_HOST` is set:

```sh
docker compose -f tests/dovecot/compose.yml up -d
POSTVAK_TEST_IMAP_HOST=localhost cargo test --features testing --test imap_live
```

Port 10993 advertises MOVE and UIDPLUS, port 11993 neither.

## Commit messages and pull request titles

Use [Conventional Commits](https://www.conventionalcommits.org): `feat: …`, `fix: …`, `docs: …`, `chore: …`. Pull requests are squash-merged, so the PR title becomes the commit on `main`, and release-plz uses it for the changelog and the version bump.

## Privacy

Never paste real message bodies, passwords or tokens into issues, tests or logs. Build test fixtures from made-up mail.

## Licence

By contributing, you agree that your contribution is dual-licensed under MIT and Apache-2.0, like the rest of the project, without any additional terms.
