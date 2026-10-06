# Rules

Rules live in `rules.toml` next to `config.toml`. Postbode reads the file on every sync. A file that fails to validate is rejected as a whole, and the previous rules stay active. `postbode rules check` validates the file; `postbode rules test` shows what each rule would do to the mail Postbode has cached.

```toml
[[rules]]
name = "github to folder"
match.header = { name = "List-Id", contains = "github.com" }
actions = [{ move = "Lists/GitHub" }, "mark_read"]
```

## Rule keys

| Key | Default | Meaning |
|---|---|---|
| `name` | required | Unique. Renaming a rule makes it a new rule. |
| `account` | every account | Only for this account. |
| `folder` | `INBOX` | The folder the rule watches. |
| `enabled` | `true` | `false` skips the rule. Proposals start disabled. |
| `proposed_by` | none | Set by `postbode rules propose`. |
| `match` | required | Conditions that must all hold. At least one. |
| `actions` | required | What to do. At least one. |

## Conditions

Text conditions take exactly one of:

- `contains`: a case-insensitive substring.
- `equals`: the whole value, case-insensitive. On `from`, `to` and `cc` it also matches any single address in the field, so `equals = "a@example.com"` matches `Alice <a@example.com>, b@example.com`.
- `regex`: Rust `regex` syntax. Start with `(?i)` for case-insensitive.

| Key | Takes | Matches |
|---|---|---|
| `from`, `to`, `cc`, `subject` | text condition | That header. |
| `body` | text condition | The plain-text body; HTML mail is converted. Postbode downloads the body of new mail in the rule's folder for this. |
| `header` | text condition plus `name` | Any header, such as `List-Id`. |
| `older_than` | duration: `30m`, `1h`, `2days` | Mail that arrived at least this long ago. |
| `seen` | `true` or `false` | Read or unread mail. |
| `to_me` | `true` or `false` | To, Cc or Delivered-To holds your address or an alias. `false` catches list and bcc mail. |
| `alias` | address, `*` as wildcard | Mail sent to that alias. |

## Actions

| Action | Effect |
|---|---|
| `"delete"` | Saves the message as `.eml` in the local trash, then removes it from the server. Later rules don't run for that message. |
| `"mark_read"` | Marks it read. |
| `"flag"` | Flags it. |
| `"archive"` | Moves it to the server's Archive folder. |
| `{ move = "Folder/Sub" }` | Moves it to that folder, creating the folder if needed. |
| `"notify"` | Notifies even when the message was moved. |
| `"silent"` | Never notifies. |

Flags are set before a move. Only the first `move` or `archive` that matches a message runs.

## When rules act

- On every sync, in file order. Because rules run again on each sync, `older_than` and `seen` can fire later, for example an hour after you read a sign-in code.
- A rule acts only on mail that arrived after the rule was enabled, so adding a rule never touches your history. `postbode rules apply-existing NAME` is the explicit opt-in. Run it with `--dry-run` first.
- Renaming a rule, or disabling and enabling it again, restarts that clock.
- Mail restored with `postbode trash restore` carries the `$PostbodeRestored` keyword. Rules never act on it again.

## Notifications

New INBOX mail notifies unless a rule moved or deleted it, or a matching rule says `silent`. `notify` forces a notification for moved mail. With `notify = false` on the account, only rules that say `notify` notify. Deleted mail, and mail found by the first sync of a folder, never notifies.

## Examples

Delete sign-in codes and magic links an hour after you read them:

```toml
[[rules]]
name = "purge sign-in codes"
match.from = { regex = "no-?reply@" }
match.subject = { regex = "(?i)sign.?in|verification code|magic link" }
match.older_than = "1h"
match.seen = true
actions = ["delete"]
```

Move list mail that is not addressed to you, without a notification:

```toml
[[rules]]
name = "list mail"
match.to_me = false
match.header = { name = "List-Unsubscribe", regex = "." }
actions = [{ move = "Lists" }, "silent"]
```

Give a shop alias its own folder, but still notify:

```toml
[[rules]]
name = "shop alias"
match.alias = "*@shop.example.com"
actions = [{ move = "Shopping" }, "notify"]
```

Archive read mail after 30 days:

```toml
[[rules]]
name = "archive old read mail"
match.seen = true
match.older_than = "30days"
actions = ["archive"]
```

## Proposals

Agents never edit `rules.toml`. They run `postbode rules propose`, which appends the rule with `enabled = false` and `proposed_by` set. To review a proposal:

- `postbode rules list` shows every rule with its state and proposer; a pending proposal is `off` with a proposer.
- `postbode rules test NAME` previews what a proposal would do.
- `postbode rules approve NAME` enables it.
- `postbode rules reject NAME` removes it.

`postbode rules schema` prints the JSON Schema of this file, and `rules.schema.json` in these docs holds the same schema.
