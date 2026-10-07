# Background sync

One process, the daemon, syncs every account, runs your rules and sends the notifications for new mail. The mail window, the command line and agents over MCP all talk to it, so they never fight over a mailbox.

## How it starts

You do not start it. Any command that needs the mail server starts it, and it stops a minute after the last client leaves. `postbode run` runs it in the foreground instead and never stops it by itself; Ctrl-C does. Only one daemon runs at a time.

## Always on

To keep mail syncing while no window is open, run the daemon at login:

```sh
postbode service install
```

This writes a launchd agent (`~/Library/LaunchAgents/nl.pataar.postbode.plist`) on macOS or a systemd user unit (`~/.config/systemd/user/postbode.service`) on Linux, and starts it. A daemon that was already running is stopped first, so the service's takes over. Running the command again is safe. `--dry-run` shows the file and the commands and changes nothing. `postbode service remove` stops the service and deletes the file. The service runs the postbode binary from the path it was installed from; after moving or reinstalling postbode, run `postbode service install` again.

## Status and stopping

```sh
postbode daemon status    # pid, version, uptime, clients and what each account is doing
postbode daemon stop
```

Both only look for a running daemon; they never start one, and say `no daemon running` when there is none. After `stop`, the next command that needs the server starts it again, and an open mail window does so within 5 s. A service restarts it on its own.

## Logs

The daemon writes `daemon.log` in its state directory: `~/Library/Application Support/postbode/` on macOS, `~/.local/state/postbode/` on Linux. The log is emptied at start when it is over 1 MB. A daemon started by a command logs only errors there; `postbode run`, and so the service, also prints each new mail's sender and subject. The socket `daemon.sock` and the lock `daemon.lock` live there too. `postbode log` shows what rules and actions did.

## Troubleshooting

- `already running (pid N)`: a daemon holds the lock. Use it, or stop it with `postbode daemon stop` before running another.
- `socket path too long`: the state directory is nested too deep for a Unix socket. Shorten `POSTBODE_HOME`.
- `the daemon stopped; see <log>`: the daemon quit while a command was waiting. The log says why.
- `<account> is offline (<reason>); retrying at HH:MM`: that account could not reach its server. Everything else keeps syncing.
- After an upgrade, the next command notices the old daemon, stops it and starts the new one. A daemon started by the service is restarted by launchd or systemd.
- `the daemon is version X, newer than this postbode (Y); restart this program`: a window or MCP server from before an upgrade is still open. It leaves the newer daemon alone; restart it.
