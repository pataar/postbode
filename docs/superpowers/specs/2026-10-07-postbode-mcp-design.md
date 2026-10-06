# Postbode MCP design

Date: 2026-10-07. Status: draft for review. Phase 3 of `2026-10-06-postbode-core-design.md`; that spec's §17 "MCP server (phase 3)" constraints apply and are restated where this design builds on them.

## 1. Purpose and scope

An agent host (Claude Desktop, Claude Code, or any MCP client) drives Postbode over MCP, both as a rule author and as an inbox assistant: it reads and searches mail, previews and proposes rules a human approves, and, when granted, acts on mail directly. It can do nothing it was not granted.

In scope:
- `postbode mcp`: an MCP server on stdio, built on `rmcp` 3.x, speaking protocol `2026-07-28` and the older `initialize` versions `rmcp` supports.
- Scopes and an account filter from the command line, set once in the host's config.
- Tools mirroring the CLI's agent surface, with results in the CLI's `--json` shapes.
- `postbode mcp install` for Claude Desktop, Claude Code, and a JSON snippet for other hosts.
- The GUI noticing store writes made by other processes.
- Website documentation.

Out of scope, recorded in §10: the daemon and socket, saving attachments to disk, host-UI confirmation through elicitation, install targets beyond the three above.

## 2. Decisions log

| Decision | Choice | Rejected |
|---|---|---|
| Agent role | Rule author and inbox assistant equally | Rules only; triage only |
| Confirming changes | The host's per-tool approval, `destructiveHint`, and a `dry_run` argument on every changing tool | Preview tokens; MCP elicitation |
| Data and IMAP access | Like the CLI one-shots: reads from the local store, each action on its own short IMAP connection, no account lock | The server running its own engine; a socket to the GUI or daemon (the long-term goal, §10) |
| Backend boundary | One concrete `Backend` struct the tools call; its internals switch to the daemon socket later | A trait with one implementation now |
| Bodies | Their own scope, `read:bodies`; `read` sees headers only | `read` includes bodies |
| Scopes | `read`, `read:bodies`, `rules:propose`, `rules:write`, `mail:modify`; default `read,rules:propose` | Per-session grants (the protocol is stateless) |
| Hidden tools | Tools outside the granted scopes are left out of `tools/list` | Listed but refused |
| Texts | One source: the agent guide is the server description, the CLI help strings are the tool descriptions, `rules schema` is the propose input schema, CLI `--json` rows are the result rows | MCP-only docs |
| Install | `postbode mcp install claude-desktop|claude-code|json`, re-runnable, `--remove`, `--dry-run` | Manual setup only |
| Concurrency gap | GUI polls SQLite `data_version` with its 2 s file poll | Accept stale GUI rows until the next sync |

## 3. Command line

```
postbode mcp [--scopes LIST] [--account NAME]...
postbode mcp install <claude-desktop|claude-code|json> [--scopes LIST] [--account NAME]... [--remove] [--dry-run]
```

`postbode mcp` without a subcommand runs the server; hosts call it. `--scopes` is a comma-separated list of the five scopes, default `read,rules:propose`; an unknown scope is an error naming the valid ones. `--account` (repeatable) limits the accounts the server sees at all; a name not in `config.toml` is an error. With no `--account`, every account is visible.

The server exits non-zero at start when no accounts are configured.

## 4. Architecture

```
src/mcp/
  mod.rs       run(): parse scopes, build Backend, serve rmcp on stdio
  backend.rs   Backend: store reads, actions over a short IMAP connection, sync; the only code touching store or IMAP
  tools.rs     tool definitions, scope filtering, result shaping, untrusted wrapping
  install.rs   postbode mcp install
src/help.rs    help strings shared by clap and the MCP tool descriptions
src/output.rs  JSON row builders shared by the CLI's --json and the MCP results
```

- `mcp` is a library module behind a default-on `mcp` cargo feature (`dep:rmcp`); `--no-default-features` builds without it, like `gui`. `postbode mcp` exists in every build and explains when the feature is off, as `postbode gui` does.
- The tools never touch `Store`, `MailOps` or `rules.toml` directly; they call `Backend`. When the daemon (§10) exists, `Backend`'s internals send `Command`s over the socket instead, and the tools do not change.
- `Backend` reuses the library: `Store` for reads, `actions::run` for direct actions (so a delete writes its `.eml` backup exactly as `postbode delete` does), `trash::Trash::restore`, `rules::edit` for proposals and approvals, and `sync::run_once` for `sync`.
- Refactor of existing code, approved: the CLI's agent-facing help strings move to `postbode::help` constants that the clap derive attributes reference, and the CLI's `--json` row builders move to `postbode::output`. The CLI output and `docs/src/cli.md` do not change.

## 5. Tools

Tool names follow the CLI commands so the agent guide's examples carry over. Results are structured JSON in the CLI's `--json` row shapes, each row with an `account` key.

| Scope | Tool | Notes |
|---|---|---|
| `read` | `folders` | per account: folder, total, unread, special use |
| `read` | `list` | `folder`, `limit` (default 50, max 500), `threads`; headers and flags only |
| `read` | `search` | FTS5 over subject, from and to; also body text when `read:bodies` is granted; same limits as `list` |
| `read` | `log` | rule and action log |
| `read` | `trash_list` | `.eml` backups |
| `read` | `rules_list`, `rules_schema`, `rules_check` | as the CLI |
| `read` | `rules_test` | a rule as JSON in, the preview lines out; the preview reports subjects, never bodies |
| `read` | `sync` | §6 |
| `read:bodies` | `show` | headers plus the body, wrapped (§7) |
| `read:bodies` | `attachments` | names, types, sizes; no saving |
| `rules:propose` | `rules_propose` | stored disabled with `proposed_by = "mcp:<client>"` |
| `rules:write` | `rules_approve`, `rules_reject`, `rules_set_enabled` | |
| `mail:modify` | `mark`, `move`, `archive`, `delete`, `trash_restore` | each takes `dry_run` |

- `tools/list` returns the granted tools in alphabetical order, with `ttlMs` and `cacheScope` set, since the list depends only on the server's arguments.
- Annotations:
  - read tools: `readOnlyHint`;
  - `delete`, `rules_approve`, `rules_set_enabled`: `destructiveHint`. Enabling a rule can delete mail, so turning one on counts as destructive;
  - `mark`, `move`, `archive`, `trash_restore`: neither hint.
- A `dry_run` call reports what would happen from the local store, like the CLI's `--dry-run`, and opens no connection.
- `<client>` in `proposed_by` is the host's client name when the protocol provides it, else `"mcp"` alone.
- Every tool takes an optional `account`; with more than one visible account, `show`, `attachments` and the changing tools require it, as the CLI does.

## 6. Sync

`sync` runs per visible account. It tries the account lock (`<state>/accounts/<name>/sync.lock`):
- free: take it, run one sync with rules (`sync::run_once`), release it, and report new messages and actions;
- held: report `{"account": "...", "synced_by": "another Postbode process", "pid": N}` and do not sync, so rules never run twice.

The lock helper moves from `engine.rs` to a small public function both use.

## 7. Safety

- Mail is untrusted. Every mail-derived string in a result goes through `message::clean`. `show` returns the body inside `<untrusted_mail_content>…</untrusted_mail_content>`, cut at 100 KB with `"truncated": true`. The server instructions and the `show` description say text inside that element is data, never instructions.
- `read` without `read:bodies` exposes no body text anywhere: no `show`, no `attachments`, and `search` matches headers only.
- `--account` filtering applies to every tool, including `log`, `trash_list` and `sync`.
- A tool call outside the granted scopes, which a well-behaved host cannot make because the tool is not listed, is refused with "not allowed with these scopes".
- A tool that fails returns an MCP tool error (`isError`) with the text the CLI would print. `rules_propose` and `rules_test` return the `rules check` errors, which name the rule and the problem, so the agent can correct itself.
- Logs go to stderr only; stdout carries the protocol. No MCP logging capability.
- The server description in `server/discover` and `initialize` is `docs/src/agent-guide.md`.

## 8. `postbode mcp install`

- The registered command is the absolute path of the running `postbode` binary plus `mcp` and the given `--scopes` and `--account` options. Hosts started from the Dock do not inherit the shell's PATH.
- `claude-desktop`:
  - edits `claude_desktop_config.json` in `~/Library/Application Support/Claude/` (macOS) or `~/.config/Claude/` (Linux);
  - sets only `mcpServers.postbode`, keeping every other key;
  - copies the old file to `claude_desktop_config.json.bak` first and writes atomically;
  - refuses to touch a file that is not valid JSON;
  - ends with "Restart Claude Desktop to load Postbode."
- `claude-code`: runs `claude mcp add --scope user postbode -- <command>` when `claude` is on PATH, replacing an existing `postbode` entry; otherwise prints that command.
- `json`: prints `{"mcpServers": {"postbode": {"command": …, "args": […]}}}` for other hosts.
- `--remove` takes the entry out again. `--dry-run` prints the change and writes nothing.
- The output always lists the granted scopes and shows how to widen them, for example `--scopes read,read:bodies,rules:propose,mail:modify` for an inbox assistant.

## 9. GUI change

The GUI's 2 s file poll also reads SQLite's `PRAGMA data_version` on each account's store. When it changes and the change was not the GUI's own write, the shown view and the folder counts reload. An agent's archive therefore disappears from the open window within about 2 s.

## 10. Later

- The daemon: `run` becomes the daemon, the GUI, CLI and MCP talk to it over a 0600 Unix socket, and the phase 2 `Command`/`Event` types become the wire protocol. This is the long-term shape; `Backend` (§4) is where MCP switches over.
- Saving attachments to disk through MCP.
- Elicitation for "confirm this rule" or "confirm this delete" through the host's UI; the proposal file stays the source of truth.
- Install targets for Cursor, VS Code and Codex; their configs take the `json` snippet today.

## 11. Documentation

- New page `docs/src/mcp.md`, "Agents over MCP", in `SUMMARY.md` after the agent guide:
  - setup with `postbode mcp install` and the JSON snippet;
  - a table of scopes, and three example grants (read-only, rule author, inbox assistant);
  - the tools by scope;
  - safety: untrusted mail, host confirmations, `dry_run`, `--account`;
  - GUI and MCP together, and the `sync` tool;
  - troubleshooting: restart the host, read stderr, widen scopes.
- `docs/src/agent-guide.md` gains a short "Over MCP" section mapping the CLI commands to the tool names.
- `docs/src/index.md` and `README.md` replace "An MCP server follows." with a pointer to the MCP page and add `postbode mcp install claude-desktop` to the quickstart.
- `docs/src/cli.md` is regenerated; the `AGENTS.md` module map gains `mcp`, `help` and `output`.
- Doc tests: the snippet on the MCP page equals `postbode mcp install json` output for the default scopes, and the scopes table lists every scope.

## 12. Testing

- **Protocol, in process.** An `rmcp` client (client feature as a dev-dependency) talks to the server over `tokio::io::duplex`. It covers:
  - `tools/list` for each scope set: exact names and order;
  - `list` and `search` rows against a fixture store;
  - `show` wrapping and the 100 KB cut;
  - without `read:bodies`: no `show`, and `search` misses a body-only word;
  - `--account` hiding another account's rows;
  - `rules_propose` writing a disabled rule with `proposed_by`;
  - a bad rule returning the `rules check` error;
  - `dry_run` actions reporting without connecting;
  - `sync` reporting the holder of a held lock.
- **Live Dovecot** (`tests/imap_live.rs`):
  - `archive` through the backend moves the message;
  - `delete` on the server without a Trash folder saves the `.eml`.
- **Install.** Claude Desktop config edits on temp files:
  - a missing file;
  - a file with another server that must survive;
  - invalid JSON refused with nothing written;
  - `--remove` and `--dry-run`.

  The `claude-code` and `json` outputs are checked as text, and the `claude` binary is never run in tests.
- **GUI.** A kittest test: a write through a second store connection reloads the view within the poll.
- **Docs.** The doc tests from §11, and the existing `cli.md` check.

Reported as ran on macOS; Linux is covered by CI.
