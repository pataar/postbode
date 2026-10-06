[![CI](https://github.com/pataar/postbode/actions/workflows/ci.yml/badge.svg)](https://github.com/pataar/postbode/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](LICENSE-MIT)

# Postbode

A fast, simple IMAP mail client for powerusers and developers, with rules that keep your mailbox clean. Postbode syncs your mail into a local store, moves it into folders and deletes transient mail such as sign-in codes and magic links once you no longer need it. Agents can propose rules; you approve them.

Postbode is a command-line engine today. A GUI and an MCP server follow.

## Quickstart

```sh
cargo install --git https://github.com/pataar/postbode
postbode account add      # asks for host, user and password, then tests the login
postbode sync             # first sync of every folder
postbode list             # newest mail in INBOX
```

Add rules to `rules.toml` next to `config.toml`, in `~/.config/postbode/` on Linux or `~/Library/Application Support/postbode/` on macOS:

```toml
[[rules]]
name = "purge sign-in codes"
match.subject = { regex = "(?i)sign.?in|verification code|magic link" }
match.older_than = "1h"
match.seen = true
actions = ["delete"]
```

```sh
postbode rules test       # dry run: what would each rule do?
postbode run              # keep syncing, apply rules, notify on new mail
```

Deleted mail is kept as `.eml` for 30 days: `postbode trash list` and `postbode trash restore FILE`.

Postbode is licensed under MIT or Apache-2.0, at your option.
