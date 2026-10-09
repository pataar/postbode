# Background sync

One process, the daemon, syncs every account, runs your rules and sends the notifications for new mail. The mail window, the command line and agents over MCP all talk to it, so they never fight over a mailbox.

## How it starts

You do not start it. Any command that needs the mail server starts it, and it stops a minute after the last client leaves. `postvak run` runs it in the foreground instead and never stops it by itself; Ctrl-C does. Only one daemon runs at a time.

## Always on

To keep mail syncing while no window is open, run the daemon at login:

```sh
postvak service install
```

This writes a launchd agent (`~/Library/LaunchAgents/io.github.postvak_app.postvak.plist`) on macOS or a systemd user unit (`~/.config/systemd/user/postvak.service`) on Linux, and starts it. A daemon that was already running is stopped first, so the service's daemon takes over. Running the command again is safe. `--dry-run` shows the file and the commands and changes nothing. `postvak service remove` stops the service and deletes the file. The service runs the postvak binary from the path it was installed from: for Homebrew that is the `opt/` link, so `brew upgrade` needs nothing more, and for an AppImage it is the `.AppImage` file. After moving postvak or the AppImage, run `postvak service install` again. With `POSTVAK_HOME` set, the service file passes it on, so the service's daemon uses that home too.

## On a server

Postvak runs headless on any Linux machine, such as a NAS, a VPS or a Raspberry Pi. Rules act on the IMAP server, so your phone and other mail clients see the sorted mailbox. Two things differ from a desktop:

- **Passwords:** a server has no keyring, so write `config.toml` by hand (see [Accounts](accounts.md)) with `password = { command = "cat /path/to/secret" }` instead of `account add`. Set `notify = false`; there is no desktop to notify.
- **Starting at boot:** `postvak service install` writes a systemd *user* unit, and user units only run while you are logged in. Run `loginctl enable-linger $USER` once to start it at boot.

### Docker

There is no published image. To run Postvak in a container, mount the Linux binary from a [release](https://github.com/postvak-app/postvak/releases/latest) into a plain Debian container. On Unraid, use this file with the Compose Manager plugin:

```yaml
services:
  postvak:
    image: debian:stable-slim
    command: ["postvak", "run"]
    environment:
      POSTVAK_HOME: /data
    volumes:
      - ./postvak:/usr/local/bin/postvak:ro  # the binary extracted from the release archive
      - ./data:/data
    restart: unless-stopped
```

With `POSTVAK_HOME` set, `config.toml` and `rules.toml` go in `./data/config/`, and the mail store and `daemon.log` in `./data/state/`. Put the password in a file under `./data/`, for example `password = { command = "cat /data/imap-work" }`. To use the CLI against the running daemon, run `docker compose exec postvak postvak rules test`.

## Status and stopping

```sh
postvak daemon status    # pid, version, uptime, clients and what each account is doing
postvak daemon stop
```

Both only look for a running daemon; they never start one, and say `no daemon running` when there is none. After `stop`, the next command that needs the server starts it again, and an open mail window does so within 5 s. A service restarts it on its own.

## Logs

The daemon writes `daemon.log` in its state directory: `~/Library/Application Support/postvak/` on macOS, `~/.local/state/postvak/` on Linux. The log is emptied at start when it is over 1 MB. It holds the daemon's errors, warnings and notes such as reconnects, but never senders or subjects; `postvak run` in a terminal also prints each sync's counts and each new mail's sender and subject. The socket `daemon.sock` and the lock `daemon.lock` live there too. `postvak log` shows what rules and actions did.

## Troubleshooting

- `already running (pid N)`: a daemon holds the lock. Use it, or stop it with `postvak daemon stop` before running another.
- `socket path too long`: the state directory is nested too deep for a Unix socket. Shorten `POSTVAK_HOME`.
- `the daemon stopped; see <log>`: the daemon quit while a command was waiting. The log says why.
- `<account> is offline (<reason>); retrying at HH:MM`: that account could not reach its server. Everything else keeps syncing.
- `<account> is restarting`: `config.toml` changed that account and its old sync thread is still finishing. Try again in a moment.
- After an upgrade, the next command notices the old daemon, stops it and starts the new one. A daemon started by the service is restarted by launchd or systemd.
- `the daemon is version X, newer than this postvak (Y); restart this program`: a window or MCP server from before an upgrade is still open. It leaves the newer daemon alone; restart it.
