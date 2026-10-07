# Postbode daemon design

Date: 2026-10-07. Status: draft for review. Phase 4. It builds on:
- the core spec `2026-10-06-postbode-core-design.md` (§17 "Daemon split");
- the GUI spec `2026-10-06-postbode-gui-design.md` (the `Command` and `Event` types);
- the MCP spec `2026-10-07-postbode-mcp-design.md` (§10, and `Backend` as the switch-over point).

## 1. Purpose and scope

One process owns IMAP. The daemon runs every account's sync thread, holds the account locks, applies rules, sends notifications and runs every command that needs the server. The GUI, CLI and MCP server become clients over a private Unix socket, so they can run side by side with one sync per account and rules applied once.

In scope:
- **Daemon:** `postbode run` as the daemon, auto-start with idle exit, and single-instance locking.
- **Wire protocol:** a JSON-lines protocol carrying the existing `Command` and `Event` types.
- **Clients:** the GUI, the CLI's IMAP commands and the MCP backend move to the daemon.
- **Service:** `postbode service install|remove` for launchd (macOS) and systemd user units (Linux).
- **Cleanup:** removal of the code that exists only because several processes could sync.
- **Docs:** a "Background sync" page and updates elsewhere.

Out of scope, recorded in §11: Windows, reads over the socket, remote or multi-user daemons, click-to-open notifications.

## 2. Decisions log

| Decision | Choice | Rejected |
|---|---|---|
| Lifecycle | Clients auto-start the daemon; `service install` adds a login service | Service only; auto-start only; manual `run` with fallback |
| Idle | An auto-started daemon exits 60 s after its last client leaves; a service or terminal `run` never idles out | Always keep running; ask on first close |
| IMAP routing | Every command that needs the server goes through the daemon, auto-starting it | Daemon when running, else direct; daemon for MCP only |
| Reads | Clients read the SQLite stores directly (WAL allows many readers) | Reads over the socket |
| Transport | JSON lines over a 0600 Unix socket, one thread per client, no new dependency | JSON-RPC via rmcp; tarpc or tonic |
| Correlation | Request ids travel with each command to the account thread; completion events carry them back | FIFO matching per account |
| Versions | `hello` exchanges protocol and crate version; a client replaces an older daemon, or one of another protocol and the same version, with its own binary, and refuses a newer one | Backward-compatible protocol versions |
| Notifications | The daemon alone sends them | GUI and daemon both |
| Attribution | `Apply` carries `by` (`cli`, `gui`, `mcp:<client>`) into the action log | Logging everything as `cli` |

## 3. Process layout

- **The daemon** is the only process that calls `Engine::start`, takes account locks or opens IMAP connections for an existing account. `account add` still logs in directly, because it tests an account that does not exist yet.
- **Clients** read the stores, rules.toml and config.toml themselves. Previews, `--dry-run`, rule edits (propose, approve, reject, set enabled) and every read stay local; they need neither IMAP nor a lock.
- **Notifications:** the daemon sends new-mail notifications using each account's `notify` setting. The GUI no longer sends any.

## 4. Lifecycle

**Commands.**

| Command | Behaviour |
|---|---|
| `postbode run` | The daemon in the foreground. It never idles out, logs to stderr, and Ctrl-C stops it. It still prints `[account] new mail from …` and `synced: N new` lines. |
| `postbode run --idle-exit SECS` (hidden) | What auto-start spawns: a new process group, stdin from `/dev/null`, stdout and stderr appended to `<state>/daemon.log`. That log is truncated at start once it exceeds 1 MB. |
| `postbode daemon status` | Shows pid, version, uptime, connected clients, and each account's state (running, offline with its reason and next retry, or failed with its reason). |
| `postbode daemon stop` | Sends `shutdown` and waits up to 5 s for the socket to close. |
| `postbode service install\|remove [--dry-run]` | Described in §8. |

**Single instance.**
- The daemon takes an exclusive try-lock on `<state>/daemon.lock` and writes its pid there.
- A second daemon exits with "already running (pid N)".
- The lock holder removes a leftover `<state>/daemon.sock`, then binds it with mode 0600.
- A socket path longer than the OS limit (`sun_path`: 104 bytes on macOS, 108 on Linux) fails at start with an error naming the path.

**Auto-start.**
- `Client::connect_or_start` connects. If the socket is missing or refuses, it spawns `postbode run --idle-exit 60` from its own `current_exe` and retries the connection for up to 5 s.
- Clients that race resolve through the daemon lock: one daemon wins and all clients connect to it.
- If no connection succeeds, the client reports the last 20 lines of `daemon.log`.

**Idle exit.**
- With `--idle-exit`, the daemon exits once no client has been connected for that many seconds.
- It stops the engine as `Engine::drop` does: a pass stops at its next checkpoint, and a chunked first sync resumes next time.

**Config changes.**
- The daemon checks the modification time of `config.toml` every 2 s.
- An added, removed or changed account starts, stops or restarts that account's thread; the others keep running. A restarted account's new thread starts only once its old one ended, and the daemon never waits for that.
- An invalid config keeps the running accounts and logs the error.
- An invalid rules.toml never stops the daemon or its start: each account runs its last good rules, or none, and sends one `Event::Error` per bad load.

**Upgrades.**
- If `hello` shows an older crate version (dotted numbers compared; a pre-release or unparsable version sorts as older), or the same version with another protocol, the client sends `shutdown`, waits for the socket to close, and auto-starts its own binary.
- If it shows a newer crate version, the client leaves the daemon running and fails with `the daemon is version <theirs>, newer than this postbode (<ours>); restart this program`. The GUI shows that on its status line and keeps retrying every 5 s; MCP returns it as the call's error. So an old window or MCP server left open after an upgrade never stops the new daemon.
- A service-run daemon is restarted by launchd or systemd on the new binary.

## 5. Wire protocol

**Framing.**
- One JSON object per line, UTF-8, in both directions.
- `Command`, `Event` and `Activity` derive `Serialize` and `Deserialize` in serde's default externally tagged form. `Action` already does.
- The Rust types in `daemon::wire` are the protocol; there is no separate schema.

**Client to daemon.**
```json
{"hello":{"protocol":1,"version":"0.2.0"}}
{"subscribe":{"id":7}}
{"command":{"id":8,"account":"work","command":{"Apply":{"folder":"INBOX","uids":[42],"action":"archive","by":"mcp:claude-ai"}}}}
{"status":{"id":9}}
{"shutdown":{"id":10}}
```

**Daemon to client.**
```json
{"hello":{"protocol":1,"version":"0.2.0","pid":4242}}
{"reply":{"id":7,"outcome":{"ok":"done"}}}
{"reply":{"id":8,"outcome":{"ok":{"event":{"ActionDone":{"account":"work","folder":"INBOX","results":[[42,{"Ok":1}]],"request":3}}}}}}
{"reply":{"id":8,"outcome":{"error":"work is offline (no route to host); retrying at 14:02"}}}
{"reply":{"id":9,"outcome":{"ok":{"status":{"pid":4242,"version":"0.2.0","uptime_secs":61,"clients":2,"accounts":[{"name":"work","activity":{"Idle":{"since":1791374400}}}]}}}}}
{"event":{"NewMail":{"account":"work","folder":"INBOX","uid":43,"from":"alice@example.com","subject":"Hi"}}}
```

A reply carries the client's own `id`; the `request` inside an event is the daemon's id for the job.

**Rules of the exchange.**
- `hello` is the first message each way. Any other first message closes the connection.
- Every message with an `id` gets exactly one `reply`.
- Events go only to clients that sent `subscribe`.

**Correlation.**
- Inside the daemon, each command travels to its account thread with its request id.
- These events carry the request id back:
  - `ActionDone`, `BodyReady` and `Restored`;
  - a new `CommandFailed { account, request, message }`, which replaces the generic `Error` for failures of a command;
  - `Synced`, which gains `requests: Vec<u64>` because several `SyncNow`s can merge into one full pass.
- The daemon turns each such event into the reply to the requesting client and also broadcasts it to subscribers. A window therefore sees an agent's archive as an ordinary `ActionDone`.

**Commands.**

| Command | Change |
|---|---|
| `Apply { folder, uids, action, by }` | `by` is new and is logged as the rule name of the action |
| `FetchBody { folder, uid }`, `Restore { file }`, `SyncNow` | Unchanged |
| `FetchBodies { folder: Option<String> }` | New, for `search --bodies`; replies with the count fetched |
| `ApplyRule { name }` | New, for `rules apply-existing`; replies with evaluated and acted counts and per-message errors |
| `status`, `shutdown`, `subscribe`, `hello` | Daemon-level, not per account |

**Immediate error replies.**
- unknown account: `no account named 'x'`;
- an account whose sync thread could not be spawned: `x is not running` (the spawn failure itself arrives as an `Error` event);
- an account between threads after a config change, while its old thread finishes: `x is restarting`;
- an account that is offline: its reason and next retry time.

Commands already queued when an account goes offline are failed with the same text.

**Timeouts.** The CLI and the MCP backend wait up to 120 s for a reply, then report "no reply from the daemon".

## 6. Clients

**GUI.**
- `App` holds a `Client` instead of an `Engine`.
- Commands are sent without waiting; replies and other events arrive through the subscription on the existing event-forwarding thread.
- At start it connects or starts the daemon and reads account states from `status`.
- If the connection drops, the status line shows "background sync stopped — reconnecting", and the GUI retries connect-or-start every 5 s.
- `Client::in_memory()` replaces `Engine::detached` in the GUI tests.

**CLI.**
- These become one request each and print what they print today:
  - `archive`, `delete`, `mark`, `move`;
  - `sync`, `trash restore`, `rules apply-existing`;
  - `search --bodies`;
  - `show` and `attachment` for a message whose body is not stored.
- `run` is the daemon (§4).

**MCP.**
- `Backend` keeps one connection for the server's lifetime and reconnects on the next call after a drop.
- An MCP host that keeps the server open therefore keeps the daemon alive.
- `Apply.by` carries `mcp:<client>`.
- The MCP `sync` tool becomes `SyncNow` plus the matching `Synced` reply.

## 7. Removed code

- `StartState::Locked` and every branch that handles it: in the engine, the CLI, and the GUI's rules, body, status and folders views, plus `ensure_can_act`'s locked case.
- The MCP `sync` tool's lock handling: `lock_account` becomes private again, and the `synced_by` reply goes.
- Every IMAP connection outside the engine:
  - `sync::connect` in actions, CLI and MCP;
  - `message_raw`'s fetch, now a `FetchBody` request;
  - `fetch_missing_bodies` and `cmd_act`'s connection;
  - `trash restore`'s connection.
- The GUI's `PRAGMA data_version` poll and `Store::data_version`. Every mailbox write now happens in the daemon and arrives as an event.
- The GUI's engine start and stop: `Engine::start` in `gui/mod.rs`, `stop_within`, and the shutdown join.
- `notify::new_mail` in `cmd_run` and the GUI's `notifier`.
- `sync::run_once` callers outside the live tests.

## 8. `postbode service`

- **macOS:**
  - `install` writes `~/Library/LaunchAgents/nl.pataar.postbode.plist`: the absolute binary path plus `run`, `RunAtLoad`, `KeepAlive`, and stdout and stderr to `<state>/daemon.log`.
  - It then runs `launchctl bootstrap gui/<uid> <plist>`.
  - `remove` runs `launchctl bootout` and deletes the file.
- **Linux:**
  - `install` writes `~/.config/systemd/user/postbode.service` (`ExecStart=<binary> run`, `Restart=always`, `RestartSec=10`), so the service comes back after `daemon stop` or an upgrade, like `KeepAlive`.
  - It then runs `systemctl --user daemon-reload` and `systemctl --user enable --now postbode`.
  - `remove` runs `disable --now` and deletes the file.
- **Common:**
  - Both are re-runnable.
  - `--dry-run` prints the file and the commands and writes nothing.
  - If an auto-started daemon is running, `install` stops it first, so the service's daemon takes the lock.

## 9. Failures

| Case | Behaviour |
|---|---|
| Daemon exits mid-request | The CLI prints "the daemon stopped; see <state>/daemon.log" and exits non-zero; MCP returns an `isError` result; the GUI reconnects |
| Daemon cannot start | The client prints the tail of `daemon.log` |
| Malformed line from a client | The daemon replies with an error if it can read an `id`, and otherwise closes that connection; other clients are unaffected |
| Client disconnects with requests in flight | The commands still run; their replies are dropped; subscribers still see the events |

## 10. Testing

- **Wire.** A serde round-trip for every message type, and the JSON shapes in §5 parse as written.
- **Daemon in process on a temp home.** Accounts point at a refusing port. Tests cover:
  - hello and status;
  - unknown, failed and offline account errors;
  - request id round-trip;
  - a subscriber seeing another client's `ActionDone`;
  - the second daemon refused, a stale socket removed, idle exit with a short timeout, an older daemon replaced and a newer one left running;
  - a too-long socket path refused.
- **Live Dovecot.**
  - `postbode archive` auto-starts a daemon, archives, and the daemon exits after the idle timeout.
  - MCP `sync` and `delete` go through the daemon.
  - The action log shows `mcp:<client>` and `cli`.
- **GUI.** A kittest with `Client::in_memory()`: sending a command, applying its reply, and the reconnect line.
- **Service.** The generated plist and unit text on temp paths. Tests never run `launchctl` or `systemctl`.
- **Platforms.** Reported as ran on macOS; Linux is covered by CI.

## 11. Later

- Windows, with named pipes.
- Serving reads over the socket.
- Remote or multi-user daemons.
- Notifications that open the message when clicked.

## 12. Documentation

- **New page `docs/src/daemon.md`, "Background sync":**
  - what the daemon does and how it starts and stops;
  - idle exit;
  - `postbode service install`;
  - `daemon status` and `stop`;
  - the log location;
  - troubleshooting: stale versions, a socket path that is too long.
- **Updates elsewhere:**
  - `index.md` and `README.md`: `run` and the quickstart;
  - `gui.md`: closing the window;
  - `mcp.md`: "With the mail window open" gets simpler, and `sync` replies change;
  - the AGENTS.md module map gains `daemon`;
  - `cli.md` is regenerated;
  - core spec §17 points here.
