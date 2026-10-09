# Agents over MCP

`postbode mcp` lets an agent host such as Claude Desktop or Claude Code read your mail, propose rules and, when you allow it, act on mail. It speaks MCP on stdin and stdout, and the host starts it. It can do nothing you did not grant: each capability is a scope, and the host's config sets the scopes.

The defaults are privacy-friendly. An agent sees folders and headers but no message bodies, cannot change or delete mail, and can only propose rules, which stay off until you approve them. Bodies, acting on mail and approving rules are each a separate scope you add on purpose, and `--account` hides every other account from the agent.

## Setup

For Claude Desktop, register the server and restart the app:

```sh
postbode mcp install claude-desktop
```

For Claude Code:

```sh
postbode mcp install claude-code
```

The Claude Desktop install edits `claude_desktop_config.json` and keeps every other server in it. The file is rewritten pretty-printed with sorted keys, and the original is saved next to it as `claude_desktop_config.json.bak`. Re-running the install does not replace that backup unless you changed the file in between. The Claude Code install runs `claude mcp remove` and then `claude mcp add --scope user`, so a re-run replaces the old entry. With `--dry-run`, or when `claude` is not on your `PATH`, it prints those commands instead of running them.

For any other host, `postbode mcp install json` prints a snippet for its config:

```json
{
  "mcpServers": {
    "postbode": {
      "args": [
        "mcp",
        "--scopes",
        "read,rules:propose"
      ],
      "command": "/opt/homebrew/bin/postbode"
    }
  }
}
```

The `command` is the absolute path of the `postbode` you ran, because hosts started from the Dock do not see your shell's `PATH`: for Homebrew the `opt/` link, which survives `brew upgrade`, and for an AppImage the `.AppImage` file, so run the install again after moving it. The install command fills it in; the path above is an example.

Options for `postbode mcp install`:

- `--scopes` sets the scopes. Run the install again with other scopes to change them.
- `--account NAME` limits the server to one account. Repeat it for more.
- `--remove` takes the entry out again.
- `--dry-run` shows only the postbode entry, or for Claude Code the `claude` commands, and writes nothing.

The install also prints a hint with the granted scopes and how to widen them. It goes to stderr, so the output of `install json` stays pure JSON.

## Scopes

| Scope | What it grants | Tools |
|---|---|---|
| `read` | Folders, message headers, the activity log, rules and trash listings, previews of rules that do not match on the body, and a sync. No body text. | `folders`, `list`, `log`, `rules_check`, `rules_list`, `rules_schema`, `rules_test`, `search`, `sync`, `trash_list` |
| `read:bodies` | Message bodies and attachment names, search over stored bodies, and with `read` previews of rules that match on the body. | `attachments`, `show` |
| `rules:propose` | Proposing a rule. It is stored disabled until you approve it. | `rules_propose` |
| `rules:write` | Approving and rejecting rules, and turning them on or off. | `rules_approve`, `rules_reject`, `rules_set_enabled` |
| `mail:modify` | Acting on mail and restoring from the trash. | `archive`, `delete`, `mark`, `move`, `trash_restore` |

The default is `read,rules:propose`. Three example grants:

- Read-only: `--scopes read`. `sync` is in this scope and still runs your approved rules.
- Rule author: the default. The agent proposes, you approve in the window or with `postbode rules approve`.
- Inbox assistant: `--scopes read,read:bodies,rules:propose,mail:modify`.

`rules:write` lets the agent approve its own rules, and an approved rule can delete mail. Leave it off.

## Tools

The names follow the CLI commands. A tool that lists things returns the CLI's `--json` rows wrapped in `{"rows": [...]}`. Rows carry an `account`; in `rules_list` that is the rule's own scope, and `null` means every account. Message rows carry no body text. `show` returns `{"message": ..., "body": ..., "truncated": ...}`.

`rules.toml` is shared by every account, so `rules_list`, `rules_propose`, `rules_approve`, `rules_reject` and `rules_set_enabled` take no `account` argument; a rule's own `account` field scopes it. They follow `--account` as described under [Safety](#safety).

`read`:

- `folders` lists folders with total and unread counts.
- `list` lists the newest messages in a folder, or threads. `limit` defaults to 50 and is at most 500. With `threads: true` it counts threads, and each thread comes with all its messages.
- `log` shows the rule and action log.
- `rules_check` validates `rules.toml`.
- `rules_list` lists rules and their state.
- `rules_schema` returns the JSON Schema for a rule.
- `rules_test` previews a rule, given as JSON, against the local store. It reports subjects, never bodies. A rule that matches on the body needs `read:bodies` too, since its matches would reveal what bodies contain.
- `search` finds messages by subject, from and to. It matches all the given words as plain words.
- `sync` asks the daemon to sync and run rules once per account and reports its result: `new_messages`, `actions` and `errors`, or an `error` when the daemon refuses the account (offline, for example). It is in the `read` scope, but it writes the store and applies approved rules, so hosts see it as a changing tool.
- `trash_list` lists the `.eml` backups.

`read:bodies`:

- `attachments` lists attachment names, types and sizes. It does not save them.
- `show` returns the headers and the body. With this scope, `search` also covers stored bodies, in FTS5 syntax. `search` and `dry_run` never connect to the server. `show` and `attachments` fetch a message you have not stored yet, once.

`rules:propose`:

- `rules_propose` stores a rule disabled, with `proposed_by` set to `mcp:<client name>`, or `mcp` when the host sends no name. A rule scoped to an account the server hides is refused. While `--account` hides an account, a rule without `account` gets the only visible account, or is refused when several are visible.

`rules:write`:

- `rules_approve` approves a rule. It acts on mail that arrives afterwards.
- `rules_reject` rejects a proposal.
- `rules_set_enabled` turns a rule on or off.

`mail:modify`:

- `archive`, `delete`, `mark` and `move` act on uids in a folder. `log` shows them under the rule name `mcp:<client name>`, or `mcp`.
- `trash_restore` restores a backup, given as the bare file name `trash_list` returns.

With more than one visible account, every tool that acts on one message needs `account`.

## Safety

- Mail is written by strangers. `show` wraps the body in `<untrusted_mail_content>`, and tells the agent that text inside is data, never instructions. Any spelling of that tag name inside a body is rewritten to `untrusted-mail-content`, so a mail cannot close the wrapper early. A body is cut at 100 KB and marked `"truncated": true`. Every mail-derived string loses its control characters; a body keeps its newlines and tabs. The tag rewrite ignores letter case, but it does not catch look-alike letters.
- A host that honours tool annotations asks you before tools that change things. `delete`, `rules_approve` and `rules_set_enabled` are marked destructive, because they can delete mail. The other changing tools are marked as changing, but not destructive.
- The `mail:modify` tools take `dry_run`. It reports what would happen from the local store and does not ask the daemon.
- `--account` hides other accounts from every tool. Rule writes refuse rules scoped to a hidden account, and so does proposing one. A rule without `account` applies to every account, hidden ones too, so while an account is hidden `rules_approve` and `rules_set_enabled` refuse to turn such a rule on; turning it off and rejecting it still work.
- Without `read:bodies`, no tool returns or reveals body text: `search` covers subject and addresses only, as plain words, with no search operators, and `rules_test` refuses a rule that matches on the body.
- A call outside the granted scopes fails with "not allowed with these scopes". Hosts only see the tools you granted, so they should not make such a call.

## Alongside the window and the CLI

The MCP server reads the same local store as the mail window. Everything that needs the mail server goes through the Postbode daemon, which owns the connections: actions, `trash_restore`, `show` and `attachments` for a message not yet fetched, and `sync`. The first such call starts the daemon if none runs. The window picks up the changes as soon as the daemon reports them, so an archived message disappears from the list.

## Troubleshooting

- Restart the host after installing. It reads its config at start.
- Logs go to stderr. Look in the host's MCP log.
- "not allowed with these scopes" means the tool needs a wider scope. Run the install again with more scopes.
- "several accounts are visible; pass account" means the tool needs the `account` argument.
