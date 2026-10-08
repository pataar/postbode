<p align="center"><img src="assets/logo.svg" alt="Postbode" width="400"></p>

[![CI](https://github.com/pataar/postbode/actions/workflows/ci.yml/badge.svg)](https://github.com/pataar/postbode/actions/workflows/ci.yml)
[![Docs](https://img.shields.io/badge/docs-postbode.pataar.nl-orange)](https://postbode.pataar.nl/)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](LICENSE-MIT)

# Postbode

A fast, simple IMAP mail client for powerusers and developers, with rules that keep your mailbox clean. Postbode syncs your mail into a local store, moves it into folders and deletes transient mail such as sign-in codes and magic links once you no longer need it. Agents can propose rules; you approve them.

Run it headless, as a background service that syncs and applies rules on a server or at login, or use it through an interface: `postbode gui` opens a keyboard-driven mail window, everything also works from the command line, and agents can use it over MCP (see [Agents over MCP](https://postbode.pataar.nl/mcp.html)).

## Quickstart

```sh
brew install pataar/tap/postbode    # or cargo, mise, the macOS app or an AppImage: see Install
postbode account add                # asks for host, user and password, then tests the login
postbode sync                       # first sync of every folder
postbode list                       # newest mail in INBOX
postbode gui                        # the mail window
postbode mcp install claude-desktop # let Claude read your inbox and propose rules
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
postbode run              # keep syncing and applying rules in the foreground
postbode service install  # run in the background at login
```

Deleted mail is kept as `.eml` for 30 days: `postbode trash list` and `postbode trash restore FILE`.

Postbode is licensed under MIT or Apache-2.0, at your option. The mail window's HTML view includes Stylo, the CSS engine from Servo and Firefox, which is under MPL-2.0.

## More

- [Documentation](https://postbode.pataar.nl/): install, accounts, rules, agent guide and CLI reference
- [Contributing](CONTRIBUTING.md) and [Code of Conduct](CODE_OF_CONDUCT.md)
- [Security policy](SECURITY.md): how to report a vulnerability
- [Changelog](CHANGELOG.md)
