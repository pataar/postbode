# Mail window

`postbode gui` opens a window with three columns: your accounts and folders, the threads in the chosen folder, and the selected message. Sync, rules and notifications run in the [daemon](daemon.md), which the window starts when none is running, so the window, the CLI and agents over MCP can all be open at once.

When the window cannot start, for instance because no account is set up yet, a small window says why and what to do; the error also goes to stderr. Closing the window does not stop sync. A daemon the window started stops a minute after its last client goes; one you run with `postbode run` keeps going. If the daemon goes away while the window is open, the status bar says "background sync stopped — reconnecting" and the window tries again every five seconds.

The window remembers its size, its position and the width of its columns in `window.ron`, next to `daemon.log` in the state directory. Delete the file to start from the defaults.

## Keys

| Key | Action |
|---|---|
| `j` / `k`, Down / Up | next or previous row |
| Right / Left | expand or collapse a thread |
| `x` | add the row to the selection, or take it out |
| `e` | archive |
| `#`, Delete (Backspace on macOS) | delete: move to Trash; from Trash, or with no Trash folder, delete for good after saving an `.eml` backup |
| `m` | move: type to filter the folders, Enter |
| `u` | mark read or unread |
| `s` | flag or unflag |
| `v` | show HTML mail as text, or as HTML again |
| `/` | search this account; mail whose body is not downloaded yet matches on sender, recipients and subject only |
| Esc | close a popup, leave search, clear the selection |
| Tab | next pane: folders, list, body |
| Ctrl+R (Cmd+R on macOS) | sync every account now |
| `?` | show these keys |

Each attachment of the open message has a Save button, which saves it to your Downloads folder, or to your home folder when there is none.

Actions apply to the selection when there is one, else to the current row; on a thread row they apply to the whole thread in that folder. A message you open, with the keys or a click, is marked read after its text has been on screen for a second; the one a folder opens on stays unread.

## Status bar

One line per account says what its sync is doing: connecting, which folder, how many headers or bodies of how many, up to date, or offline and when it retries. Click it for the last 50 lines. The switch on the right picks the System, Light or Dark theme and saves it as `[ui] theme` in `config.toml`. The light and dark themes are Catppuccin Latte and Mocha.

## Rules, Activity and Trash

**Rules** lists proposals with Approve and Reject, then every rule with a switch. Changes to `rules.toml`, from the window or from your editor, take effect within about two seconds for new mail. **Activity** is the log of what rules and your actions did. **Trash** lists the `.eml` backups of mail deleted for good (deleted from Trash, from an account without a Trash folder, or by a rule), with Restore. Mail moved to the server's Trash folder is in that folder in the tree.

The daemon applies changes to `config.toml` by itself; the window's account list catches up when you reopen it, and the window says so.

## HTML mail

Mail with an HTML part shows as its sender laid it out, on a white page in both themes; `v` switches that message to its text and back. Nothing is fetched over the network: images and stylesheets from the web are not loaded, and the line "Remote content not loaded." says when a message asked for some. Images sent inside the message show. No scripts run and forms do nothing. Hover over a link to see where it goes; in HTML and in text, only `http`, `https` and `mailto` links open, in your browser or mail app.

HTML over 2 MB, or a page too tall or too broken to lay out, shows as text with a note saying why. Sending mail comes later.
