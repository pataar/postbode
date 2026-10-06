# Mail window

`postbode gui` opens a window with three columns: your accounts and folders, the threads in the chosen folder, and the selected message as text. It runs the same sync and rules as `postbode run`, so use one or the other. If another Postbode process already syncs an account, the window shows that account read-only and says that another Postbode process holds it.

## Keys

| Key | Action |
|---|---|
| `j` / `k`, Down / Up | next or previous row |
| Right / Left | expand or collapse a thread |
| `x` | add the row to the selection, or take it out |
| `e` | archive |
| `#`, Delete | delete: move to Trash; from Trash, or with no Trash folder, delete for good after saving an `.eml` backup |
| `m` | move: type to filter the folders, Enter |
| `u` | mark read or unread |
| `s` | flag or unflag |
| `/` | search this account |
| Esc | close a popup, leave search, clear the selection |
| Tab | next pane: folders, list, body |
| Ctrl+R (Cmd+R on macOS) | sync every account now |
| `?` | show these keys |

Actions apply to the selection when there is one, else to the current row; on a thread row they apply to the whole thread in that folder. A message is marked read after it has been on screen for a second.

## Status bar

One line per account says what its sync is doing: connecting, which folder, how many headers or bodies of how many, up to date, or offline and when it retries. Click it for the last 50 lines. The switch on the right picks the System, Light or Dark theme and saves it as `[ui] theme` in `config.toml`. The light and dark themes are Catppuccin Latte and Mocha.

## Rules, Activity and Trash

**Rules** lists proposals with Approve and Reject, then every rule with a switch. Changes to `rules.toml`, from the window or from your editor, take effect within about two seconds for new mail. **Activity** is the log of what rules and your actions did. **Trash** lists the `.eml` backups of mail deleted for good (deleted from Trash, from an account without a Trash folder, or by a rule), with Restore. Mail moved to the server's Trash folder is in that folder in the tree.

Changes to `config.toml` need a restart; the window says so.

Bodies show as text, and only `http`, `https` and `mailto` links are clickable. HTML rendering and sending mail come later.
