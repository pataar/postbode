# Postbode

Rules for your IMAP inbox. Test them with a dry run and undo any delete. Postbode syncs your mail into a local store and runs your rules: move mail into folders, mark it read, or delete transient mail such as sign-in codes and magic links once you no longer need it. Without rules, it leaves your mail alone.

It runs headless as a background service, at login or on a server. On top come a keyboard-driven mail window for reading (`postbode gui`), a full command line, and an MCP server for agents (see [Agents over MCP](https://postbode.pataar.nl/mcp.html)). Postbode reads and sorts mail; it does not send it. It runs on macOS and Linux.

![The Postbode mail window](https://raw.githubusercontent.com/pataar/postbode/main/tests/snapshots/gui_inbox_light.png#gh-light-mode-only)
![The Postbode mail window](https://raw.githubusercontent.com/pataar/postbode/main/tests/snapshots/gui_inbox_dark.png#gh-dark-mode-only)

## Quickstart

```sh
brew install pataar/tap/postbode    # or cargo, mise, the macOS app or an AppImage: see Install
postbode account add                # asks for host, user and password, then tests the login
postbode sync                       # first sync of every folder
postbode list                       # newest mail in INBOX
postbode gui                        # the mail window
postbode mcp install claude-desktop # let Claude read your inbox and propose rules
```

Postbode ships with no rules. Add your own to `rules.toml` next to `config.toml`, in `~/.config/postbode/` on Linux or `~/Library/Application Support/postbode/` on macOS. For example, this one deletes read sign-in codes after an hour:

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

## Why not server-side filters?

- Works with any IMAP server, without Sieve or a webmail settings page.
- Rules run on every sync, not once on delivery, so they can act later: an hour after you read a sign-in code, or once a newsletter is a week old.
- `rules test` shows what every rule would do before it does it, and deleted mail can be restored.
- Agents over MCP get only the scopes you grant, so it stays privacy-friendly: by default they see headers but no message bodies, and propose rules instead of acting on mail.
- Passwords stay in the macOS Keychain or the Secret Service, or come from a command such as `pass`.

Postbode is licensed under MIT or Apache-2.0, at your option. The mail window's HTML view includes Stylo, the CSS engine from Servo and Firefox, which is under MPL-2.0.
