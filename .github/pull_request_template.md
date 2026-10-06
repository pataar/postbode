## What and why

<!-- What does this change, and why? Link the issue it closes, e.g. "Closes #12". -->

## How it was tested

<!-- Say which platforms it was compiled on and which it actually ran on. -->

## Checklist

- [ ] The PR title follows Conventional Commits (`feat:`, `fix:`, `docs:` …)
- [ ] `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `cargo machete` and `cargo audit` pass
- [ ] New logic has a test; a bug fix has a regression test
- [ ] Docs in `docs/src/` are updated, regenerated with `POSTBODE_BLESS=1 cargo test` if CLI flags or rule types changed
- [ ] No real message contents, passwords or tokens in code, tests or logs
