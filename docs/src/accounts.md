# Accounts

`postbode account add` asks for the details (including an extra CA file, if your server needs one), tests the login and writes `config.toml`. You can also edit the file by hand:

| | Linux | macOS |
|---|---|---|
| `config.toml`, `rules.toml` | `~/.config/postbode/` | `~/Library/Application Support/postbode/` |
| Mail store and trash | `~/.local/state/postbode/accounts/<name>/` | `~/Library/Application Support/postbode/accounts/<name>/` |

```toml
[[accounts]]
name = "work"
host = "imap.example.com"
port = 993
username = "me@example.com"
password = { keyring = true }
address = "me@example.com"
aliases = ["me@example.org", "*@shop.example.com"]
sync_interval_secs = 120
trash_retention_days = 30
notify = true
```

| Key | Default | Meaning |
|---|---|---|
| `name` | required | Letters, digits, `-` and `_`. Used in `--account` and for the store directory. |
| `host`, `port` | port 993 | IMAP over TLS. STARTTLS on port 143 is not supported yet. |
| `username` | required | The IMAP login. |
| `password` | required | `{ keyring = true }` or `{ command = "pass show mail/work" }`. |
| `address` | the username | Your address, when the username is not one. |
| `aliases` | none | Other addresses that are you. `*` is a wildcard over the whole address. |
| `sync_interval_secs` | 120 | Full sync interval. New INBOX mail arrives sooner through IMAP IDLE. |
| `trash_retention_days` | 30 | How long deleted mail is kept as `.eml`. |
| `notify` | true | Desktop notification for new INBOX mail no rule handled. |
| `ca_file` | none | Absolute path to a PEM file with an extra trusted root certificate, for a server with a private CA. |

## Appearance

```toml
[ui]
theme = "system"
```

`theme` is `"system"` (follow the OS, the default), `"light"` or `"dark"`. The mail window's theme switch writes it.

## Passwords

`{ keyring = true }` keeps the password in the macOS Keychain or the Secret Service, under service `postbode` and the account name. `account add` stores it there.

`{ command = "..." }` runs the command with `sh -c` and uses its output, without the trailing newline. A non-zero exit is an error, and the command's own error output shows in your terminal.

## Aliases

`address` plus `aliases` define "me". Rules use them through `to_me` and `alias`.
