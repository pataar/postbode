# Mail window

![The mail window in the light theme](https://raw.githubusercontent.com/pataar/postbode/main/tests/snapshots/gui_inbox_light.png)

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

## Folders

Each account lists its INBOX and special folders (Archive, Drafts, Sent, Junk, Trash) first, then your own folders as a tree that follows the server's hierarchy. Click the caret in front of a parent to fold or unfold its branch; folding lasts until you close the window. A parent the server does not list as a folder is shown but cannot be opened.

## Toolbar and status bar

The toolbar above the panes has buttons for Archive, Move, Delete, Flag and Read or unread; hover one to see its key. They act on the selection or the current row, like the keys. The magnifier on the right starts a search, like `/`. The status bar starts with a sync button (Ctrl+R, Cmd+R on macOS) and ends with Postbode's version.

## Message list

Each row shows, in columns: a dot when it is unread, a flag (or a check when marked), the sender (the recipient in Sent and Drafts), the subject with the thread's message count, and the date on the right. Dates read "09:30" today, "Sun 18:00" within the past week, "3 Sep" earlier this year and "10 Dec 2025" before that. A long subject is cut off with "…" so the date always stays in view.

## Status bar

One line per account says what its sync is doing: connecting, which folder, how many headers or bodies of how many, up to date, or offline and when it retries. Click it for the last 50 lines. The switch on the right picks the System, Light or Dark theme and saves it as `[ui] theme` in `config.toml`. Both themes put the panes in an oxblood frame: warm paper in light mode, dark brown in dark mode. Next to the version, a link appears when a newer release is out; it opens that release's notes on GitHub. Postbode does not update itself, and the daily check runs only with `[ui] check_updates = true` in `config.toml` (see [Accounts](accounts.md#appearance)).

## Rules, Activity and Backups

![Rules view with an agent proposal](https://raw.githubusercontent.com/pataar/postbode/main/tests/snapshots/gui_rules.png)

These three views sit at the bottom of the folder pane.

**Rules** lists proposals with Approve and Reject, then every rule with a switch. Changes to `rules.toml`, from the window or from your editor, take effect within about two seconds for new mail. **Activity** is the log of what rules and your actions did. **Backups** lists the `.eml` backups of mail deleted for good (deleted from Trash, from an account without a Trash folder, or by a rule), with Restore. Mail moved to the server's Trash folder is in that folder in the tree.

The daemon applies changes to `config.toml` by itself; the window's account list catches up when you reopen it, and the window says so.

## HTML mail

![An HTML message rendered in the reader](https://raw.githubusercontent.com/pataar/postbode/main/tests/snapshots/gui_html_light.png)

Mail with an HTML part shows as its sender laid it out, on a white page in both themes; `v` switches that message to its text and back. Nothing is fetched over the network: images and stylesheets from the web are not loaded, and the line "Remote content not loaded." says when a message asked for some. Images sent inside the message show. No scripts run and forms do nothing. Hover over a link to see where it goes; in HTML and in text, only `http`, `https` and `mailto` links open, in your browser or mail app.

HTML over 2 MB, or a page too tall or too broken to lay out, shows as text with a note saying why. Sending mail comes later.

## Reader toolbar

Above the headers, **Save .eml** writes the message exactly as the server sent it to your Downloads folder, named after its subject; when that name is taken it saves beside it as "… (2).eml" and never replaces a file. The line under the attachments says where it went. Until the message is downloaded the button is greyed out and says why on hover.

**View source** opens the message as plain text in a window, with control characters left out; Copy puts that text on the clipboard. Esc or moving to another message closes it.

Mail with an HTML part also has a **Text | HTML** switch there, which does the same as `v`. View source always shows the whole message, whichever is active.
