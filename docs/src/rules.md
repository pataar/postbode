# Rules

Rules live in `rules.toml` next to `config.toml`. Postbode reads the file on every sync, and the daemon syncs every account within seconds of the file changing, whoever changed it. A file that fails to validate is rejected as a whole, and the previous rules stay active until the daemon stops or that account's settings change. When there are none, no rules run and new mail notifies as the account's `notify` setting says; once the file is fixed, rules that were already enabled also run on the mail that arrived meanwhile. Each account reports the error once, and syncing and actions carry on. `postbode rules check` validates the file; `postbode rules test` shows what each rule would do to the mail Postbode has cached.

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

A tag is printable ASCII without spaces, backslashes or `( ) { } % * " ]`, and `$PostbodeRestored` is reserved. `name`, `account`, `folder` and `move` folders must not be blank, may not contain control characters, and are at most 255 characters.

## Conditions

Text conditions take exactly one of:

- `contains`: a case-insensitive substring.
- `equals`: the whole value, case-insensitive. On `from`, `to` and `cc` it also matches any single address in the field, so `equals = "a@example.com"` matches `Alice <a@example.com>, b@example.com`.
- `regex`: Rust `regex` syntax. Start with `(?i)` for case-insensitive.

`contains` and `equals` also take a list, which matches when any value does: `subject = { contains = ["receipt", "invoice"] }`.

| Key | Takes | Matches |
|---|---|---|
| `from`, `to`, `cc`, `subject` | text condition | That header. |
| `body` | text condition | The plain-text body; HTML mail is converted. Postbode downloads the body of new mail in the rule's folder for this. |
| `header` | text condition plus `name`, or a list of them | Any header, such as `List-Id`. Every header in a list must match. |
| `older_than` | duration: `30m`, `1h`, `2days` | Mail that arrived at least this long ago. |
| `seen` | `true` or `false` | Read or unread mail. |
| `to_me` | `true` or `false` | To, Cc or Delivered-To holds your address or an alias. `false` catches list and bcc mail. |
| `alias` | address, `*` as wildcard | Mail sent to that alias. |
| `tag` | IMAP keyword, such as `$label1` | Mail carrying that keyword, ignoring case. Thunderbird shows keywords as tags. |
| `any` | list of conditions | Holds when at least one entry holds. |
| `none` | list of conditions | Holds when no entry holds. |

Each entry of `any` and `none` is a set of conditions that must all hold, written like `match` itself, so `none = [{ from = …, subject = … }]` only excludes mail that matches both. To exclude either, give each its own entry. Entries can hold `any` and `none` again, up to eight levels deep. A rule whose conditions need the body never fires on a message whose body could not be downloaded, even through `none`.

## Actions

| Action | Effect |
|---|---|
| `"delete"` | Saves the message as `.eml` in the local trash, then removes it from the server. Later rules don't run for that message. |
| `"mark_read"` | Marks it read. |
| `"flag"` | Flags it. |
| `"archive"` | Moves it to the server's Archive folder. |
| `{ move = "Folder/Sub" }` | Moves it to that folder, creating the folder if needed. |
| `{ tag = "$label1" }` | Adds that IMAP keyword, which Thunderbird and other clients show as a tag. The server must accept custom keywords. |
| `"notify"` | Notifies even when the message was moved. |
| `"silent"` | Never notifies. |

A `delete` wins: when any matching rule deletes a message, no other rule's actions run for it. Otherwise flags and tags are set before a move, and only the first `move` or `archive` that matches a message runs.

## When rules act

- On every sync, in file order. Because rules run again on each sync, `older_than` and `seen` can fire later, for example an hour after you read a sign-in code.
- A rule acts only on mail that arrived after the rule was enabled, so adding a rule never touches your history. `postbode rules apply-existing NAME` is the explicit opt-in. Run it with `--dry-run` first.
- A rule approved with `postbode rules approve` acts on mail that arrives after the approval. A rule you add or enable by editing the file acts on mail that arrives after the next sync picks it up.
- Renaming a rule, or disabling and enabling it again, restarts that clock.
- Mail restored with `postbode trash restore` carries the `$PostbodeRestored` keyword. Rules never act on it again. This needs a server that accepts custom keywords; without one, the same rule can delete restored mail again.

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

Tag shop mail as Important (Thunderbird's `$label1`), except receipts and invoices:

```toml
[[rules]]
name = "tag shop mail"
match.from = { contains = "shop.example.com" }
match.none = [{ subject = { contains = ["receipt", "invoice"] } }]
actions = [{ tag = "$label1" }]
```

Flag mail from either of two people, or about an outage:

```toml
[[rules]]
name = "flag the important ones"
match.any = [
  { from = { equals = ["alice@example.com", "bob@example.com"] } },
  { subject = { contains = ["outage", "incident"] } },
]
actions = ["flag"]
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
