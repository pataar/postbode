# Postbode GUI design

Date: 2026-10-06. Status: draft for review. Phase 2 of `2026-10-06-postbode-core-design.md`; that spec's §17 constraints apply and are restated where this design builds on them.

## 1. Purpose and scope

A daily mail reader: keyboard-driven triage of the local store, with the sync loop running inside the GUI process. Rule management is secondary and minimal. Sending stays in another client until phase 4.

In scope:
- `postbode gui`: three-column window (folders, collapsed threads, text body), keyboard actions, local search.
- A library `engine` that both `postbode run` and the GUI use to run the account threads.
- Commands from the UI into the account threads, one IMAP connection per account.
- Live activity and progress, including chunked, resumable first sync.
- Rules view (proposals, enable switch, open in editor), activity log, trash with restore.
- A single-instance lock per account.

Out of scope, recorded in §11: compose and send, HTML rendering, unified inbox, account setup in the GUI, live reload of `config.toml`, applying a rule to existing mail from the GUI, undo, app bundle and tray.

## 2. Decisions log

| Decision | Choice | Rejected |
|---|---|---|
| Day-one job | Daily reader; rules UI secondary | Rules dashboard; both at equal weight |
| SQLCipher | No. Stores stay 0600 in the state dir; full-disk encryption covers data at rest | Encrypt now; defer |
| Layout | Three columns: folders, list, body | List over body; list then body |
| List | Collapsed threads, expand in place | Flat; per-folder toggle |
| Accounts | One tree per account | Unified inbox |
| Rules UI | Proposals, enable switch, open in editor, log, trash | Rule-from-message form; full form editor |
| UI to IMAP | Commands into the existing account thread; IDLE woken by a flag | Second action connection per account; connection per action |
| Packaging | `gui` cargo feature, on by default; `postbode gui` | Separate binary; workspace |
| Thread spawning | `postbode::engine`, shared by `run` and `gui` | Each front end spawns its own threads |
| Concurrent processes | Per-account lock file; a second process gets that account read-only | Allow, and run rules twice |
| First sync | `UID SEARCH`, then envelopes 500 messages at a time, one transaction each, resumable | One fetch per folder; 500-UID ranges (sparse UIDs) |
| Mark read | 1 s after the user opens a message, text on screen | On selection; never automatically |
| Theme | System by default; `[ui] theme` in `config.toml`, switchable in the app | Fixed dark; eframe's own persistence file |
| GUI tests | `egui_kittest` (headless, AccessKit queries) | Hand-rolled `Context::run_ui` harness |

The SQLCipher row settles the open question in core §17. The `egui_kittest` row refines core §17's "headless egui layout tests via `Context::run_ui`": kittest drives the same headless context and adds key events and label queries.

## 3. Engine and packaging

`Cargo.toml` gains feature `gui = ["dep:eframe"]`, in `default`. `eframe` 0.36 with its default features (wgpu backend, X11 and Wayland loaded at runtime) is the only new runtime dependency. `egui_kittest` 0.36 is a dev-dependency. `cargo build --no-default-features` builds the CLI without the GUI; CI builds it once.

`postbode gui` opens the window. Bare `postbode` keeps printing help.

New module `src/engine.rs`:

```rust
pub struct Engine { /* per-account command senders and wake flags, shutdown flag, join handles */ }

pub enum StartState { Running, Locked { pid: Option<u32> }, Failed(String) }

impl Engine {
    /// Takes each account's lock and spawns its sync thread; accounts whose lock is held are not started.
    pub fn start(config: &Config, paths: &Paths) -> (Engine, Receiver<Event>);
    pub fn accounts(&self) -> &[(String, StartState)];
    /// Queues the command and wakes the account's IDLE; false when the account is not running.
    pub fn send(&self, account: &str, command: Command) -> bool;
    /// Sets shutdown and every wake flag, then joins the threads.
    pub fn stop(self);
    /// An engine with no threads whose commands arrive on the returned receiver, for front-end tests.
    pub fn detached(accounts: &[&str]) -> (Engine, Receiver<(String, Command)>);
}
```

`cmd_run` becomes `Engine::start` plus its existing event printing. It prints a warning for each locked account and exits non-zero when no account could start.

**Lock.** `<state>/<account>/sync.lock`, held with `std::fs::File::try_lock` for the engine's lifetime, containing the holder's pid for the message. One-shot CLI commands (`sync`, `archive`, `delete`, …) do not take it, as today.

**Notifications.** The `notify-rust` call and `notification_text` move from `cli/mod.rs` to `src/notify.rs`, used by `run` and `gui`.

## 4. Sync changes

### Commands

```rust
pub enum Command {
    Apply { folder: String, uids: Vec<u32>, action: rules::Action },
    FetchBody { folder: String, uid: u32 },
    Restore { file: PathBuf },
    SyncNow,
}
```

- `Apply` runs `actions::run`, so a delete writes its `.eml` backup first, exactly like a rule delete or `postbode delete`.
- `FetchBody` downloads the raw message, stores it and indexes `body_text`, as `ensure_raw` does for rules.
- `Restore` runs `Trash::restore(ops, file) -> folder`: APPEND with the restored keyword, then remove the file. This logic moves from `cli/mod.rs` into `trash.rs`, and `postbode trash restore` calls it too.
- `SyncNow` makes the next pass a full one.

`run_loop` takes a `Receiver<Command>` and a per-account `Arc<AtomicBool>` wake flag. The flag is the `interrupt` passed to `MailOps::idle`, so a command ends IDLE within 500 ms. `Engine::stop` sets the shared shutdown flag and every wake flag; the loop tells the two apart by checking shutdown.

Commands are drained, in arrival order:
- before every pass and after IDLE returns;
- between folders of a full pass;
- between header chunks (below). After draining, the chunk loop selects its folder again and stops that folder's pass if UIDVALIDITY changed.

While an account is offline, commands wait in the channel and run after reconnecting. A failed command produces an `Error` event; the session continues unless the error is a connection error.

### Events

`Event` keeps `NewMail`, `Synced` and `Error`, and gains:

```rust
BodyReady { account: String, folder: String, uid: u32 },
ActionDone { account: String, folder: String, results: Vec<(u32, Result<usize, String>)> },
Restored { account: String, folder: String },
Activity { account: String, activity: Activity },
```

```rust
pub enum Activity {
    Connecting,
    ListingFolders,
    SyncingFolder { folder: String, index: usize, of: usize },
    FetchingHeaders { folder: String, done: usize, total: usize },
    FetchingBodies { folder: String, done: usize, total: usize },
    RunningRules { folder: String },
    RunningCommand { what: String },
    Idle { since: i64 },
    Offline { reason: String, retry_at: i64 },
}
```

`ActionDone` carries error text rather than `ActionError`, because `Event` is `Clone + PartialEq`.

### Chunked envelope fetch

UIDs can be sparse (Gmail and Exchange folders span millions of UID values for a few thousand messages), so chunks are counted in messages, not UID ranges.
- `MailOps::fetch_new` is replaced by `search_uids(from_uid) -> Vec<u32>` (`UID SEARCH UID n:*`, ascending) and `fetch_envelopes(first, last)` (`UID FETCH first:last`).
- `sync_folder` lists the UIDs above `last_uid`, then fetches them 500 at a time, each chunk spanning its first to last UID. Each chunk is written in its own transaction, which also advances `last_uid`. The flag refresh for already-stored messages stays one fetch, written before the chunks.
- `FetchingHeaders.total` is the number of UIDs listed, so the counter is exact.
- An interrupted first sync resumes after the last committed chunk. Mail that arrives during a pass is picked up by the next one.

### Migration 002

```sql
ALTER TABLE folders ADD COLUMN initial_uid_next INTEGER NOT NULL DEFAULT 0;
ALTER TABLE folders ADD COLUMN rules_uid INTEGER NOT NULL DEFAULT 0;
UPDATE folders SET rules_uid = last_uid;
```

When a folder is newly tracked, a placeholder is adopted, or a UIDVALIDITY change resets it, `initial_uid_next` is set to one above the highest UID that pass's `search_uids` returned, or 0 when the folder is empty, and `rules_uid` is set to 0 in the same transaction. `rules_uid` is the highest UID the rules have evaluated. Existing rows migrate with `initial_uid_next` 0 and `rules_uid = last_uid`, so an upgrade treats no existing mail as fresh.

In Normal mode a message is fresh (it may notify, and body rules fetch its body) when `initial_uid_next <= uid` and `rules_uid < uid <= last_uid`; other modes treat nothing as fresh. Freshness is stored, not taken from the pass's in-memory list, so mail committed by a pass that aborts (failed chunk, lost connection, stop, crash) is still fresh on the next pass, and a resumed chunked first sync stays silent. The `uid <= last_uid` bound keeps a row a UIDPLUS move stored above `last_uid` from marking unsynced mail below it as seen. After a folder's rules complete, `rules_uid` advances to the highest UID evaluated, capped at `last_uid`; synced folders the rules do not look at advance to `last_uid`, so a rule added for one later finds no old mail fresh. It never moves down.

### Store queries for the list

`store.threads()` returns full `Message`s with headers and body. The list uses light rows:

```rust
pub struct MessageSummary { pub uid: u32, pub from: String, pub to: String, pub subject: String, pub date: i64, pub flags: String }
pub struct ThreadSummary { pub thread_id: String, pub latest: MessageSummary, pub count: u32, pub unread: bool, pub flagged: bool }

pub fn thread_summaries(&self, folder: &str, limit: u32) -> Result<Vec<ThreadSummary>, StoreError>;
pub fn thread_members(&self, folder: &str, thread_id: &str) -> Result<Vec<MessageSummary>, StoreError>;
```

The list loads at most 10,000 threads per folder, marked with a `ponytail:` comment: older mail is reachable through search; page by date if that is not enough.

## 5. GUI structure

```
src/gui/
  mod.rs       launch: eframe::run_native, Engine::start, event forwarder thread
  app.rs       App state, eframe::App, applies UiActions after the frame
  folders.rs   account and folder tree, plus the Rules/Activity/Trash entries
  list.rs      thread list via ScrollArea::show_rows, search field
  body.rs      one widget taking &Message; headers, text, links, attachments
  rules.rs     Rules, Activity and Trash views
  status.rs    status bar and its history popup
```

- Views take `&App` state and return `Vec<UiAction>`; `app.rs` applies them after the frame: store queries, `engine.send`, optimistic edits. `UiAction` is named to stay clear of `rules::Action`.
- A forwarder thread owns the event receiver from `Engine::start`, moves each event into a channel the app drains each frame, and calls `ctx.request_repaint()` per event. Tests skip the forwarder and hand the app a receiver directly.
- Reads use one `Store` per account opened on the UI thread; the store is in WAL mode, so reads never wait for the sync writer. Rows are cached in app state and reloaded on: folder change, `Synced`, `ActionDone`, `BodyReady`, `Restored`, search. Never per frame.
- The app also asks for a repaint every 2 s to poll file modification times (§8).

### Appearance

egui ships light and dark themes, and eframe follows the OS setting when the preference is `System`, switching live when the OS switches.
- `config.toml` gains an optional `[ui]` table: `theme = "system" | "light" | "dark"`, default `"system"`. Unknown values fail `Config::load` like any other bad field.
- The status bar has a three-way switch (system, light, dark). Choosing one applies at once through `ctx.set_theme(ThemePreference)` and writes `[ui] theme` with `toml_edit`, keeping comments. The app records the file's new modification time, so its own write does not raise the "config.toml changed" banner.
- `postbode` without the `gui` feature parses and ignores `[ui]`.

### Visual identity: Ossenbloed

Chosen 2026-10-09 over the first look (Catppuccin Mocha and Latte, peach and mint), which read as a stock terminal theme. Mock-ups: https://claude.ai/artifact/6T24ycUywqkcN5ghd8DhQP (boards `Ossenbloed` and `OssenbloedDark`).
- An oxblood frame (toolbar, folder pane, status bar) around the panes, which are rounded (`theme::RADIUS`) where they meet it, like mail in a letterbox. The frame keeps cream text in both modes; the panes are warm paper in light mode and near-black brown in dark mode.
- `theme.rs` has four palettes: `PAPER` and `NIGHT` for the panes, `FRAME` and `FRAME_NIGHT` for the frame. `theme::chrome(ui)` switches a ui to its mode's frame palette, and `theme::palette(ui)` finds a ui's palette by its panel colour, so views stay unaware of which surface they are on.
- The selected row and folder take the frame colour (`selected`, a lighter oxblood in dark mode). Brass (`highlight`) marks unread mail, flags and the frame's focus ring. Every palette passes WCAG AA for text, checked by tests.
- The list has the folder's name as a serif title with its thread and unread counts. Rows are two lines, ruled apart: sender and date, then the subject; unread senders are SemiBold.
- The reader opens with the subject as a serif heading, then the sender's initials in a circle beside the sender's name (SemiBold), address, recipients and a short date (the full date on hover). Message text is 15 px, larger than the interface.
- Dates and the version are monospace. The status bar marks each account with a brass dot when up to date, a brass ring while offline and a ring in the error colour on an error. Account names are muted SemiBold; the selected folder's icon is brass and its unread count a brass pill.
- The window names the INBOX "Inbox" (`folders::label`); commands and the status line keep the server's name.
- The app icon, `assets/logo.svg`, the social preview and the docs theme use the same colours: a cream envelope with a brass flap on oxblood.
- Fonts, bundled and under the OFL: Hanken Grotesk Medium for text and SemiBold for emphasis, Spline Sans Mono Medium for monospace with egui's Hack behind it for the arrows it lacks, and Newsreader Medium for headings and the wordmark. All are static instances of Google Fonts' variable fonts, since egui cannot pick a weight.

## 6. Reading

**Keys.** `?` shows this table in the app.

| Key | Action |
|---|---|
| `j`/`k`, `↓`/`↑` | next or previous row |
| `→`/`←` | expand or collapse a thread |
| `x` | toggle the row in the multi-selection |
| `e` | archive |
| `#`, `Delete` | delete (to Trash, with an `.eml` backup) |
| `m` | move: folder popup, type to filter, Enter |
| `u` | toggle read |
| `s` | toggle flag |
| `/` | search |
| `Esc` | leave search, close popup, clear selection |
| `Tab` | cycle focus: folders, list, body |
| `Ctrl+R` | sync now |

Actions apply to the multi-selection when there is one, else to the current row. On a thread row they apply to every member of the thread in this folder. After archive, move or delete, the next row is selected. Keys go to the focused pane; text fields take keys while focused.

**Folder tree.** Accounts in config order. Per account: Inbox, Archive, Drafts, Sent, Junk, Trash (by special-use), then the rest alphabetically, each with its unread count. A spinner beside the account while its activity is not `Idle`.

**List rows.** Unread dot, flag mark, sender (recipient in Sent and Drafts), subject, `(n)` thread count, date: time if today, weekday within the past 7 days, else the date. A thread row describes its latest message and is unread if any member is.

**Search.** `/` opens a field above the list. It runs `store.search` over the current account, at most 500 results, shown as flat rows with their folder. `Esc` returns to the folder. Bodies not yet downloaded are not searched; the result header says so.

**Body panel.** From, To, Cc, Date, Subject, then the selectable text from `body_text` (HTML-only mail shows its extracted text; with the `html` feature, mail with an HTML part shows the HTML view of `2026-10-07-postbode-html-design.md` instead). Links are found by scanning the text for `http://`, `https://` and `mailto:`; only those three schemes are clickable and passed to `ctx.open_url`. Attachments listed by name and size, each with **Save**, writing through `message::save_attachment` into the Downloads dir from `directories::UserDirs` (home if none), never overwriting, and showing the path written. If the body is not stored: "Loading…" and a `FetchBody`; when the account is offline, "Not downloaded; loads when work reconnects."

**Read state.** A message is marked read after the user opens it (by moving to it, clicking it, or landing on it after an archive or delete) and its text has been on screen for 1 s, measured with egui's input time so tests can drive it. A message shown passively (at startup, after a folder switch, or while typing a search) is not marked.

## 7. Activity and progress

The status bar has one line per account built from its latest `Activity`:

| Activity | Text |
|---|---|
| `Connecting` | work: connecting… |
| `ListingFolders` | work: listing folders |
| `SyncingFolder` | work: Archive (4/12) |
| `FetchingHeaders` | work: INBOX headers 12,500 / 48,213, with a progress bar |
| `FetchingBodies` | work: INBOX bodies 30 / 210, with a progress bar |
| `RunningRules` | work: running rules on INBOX |
| `RunningCommand` | work: archiving 3 messages |
| `Idle` | work: up to date · 12:04 |
| `Offline` | work: offline (timeout) · retry 12:09 |
| `NotRunning` | work: not running (could not start its sync thread: …), in red |

The line also shows queued commands ("2 queued"), counted by the app as sent minus finished. Clicking the bar opens the last 50 activity and error lines with times.

## 8. Rules, Activity and Trash views

Three fixed entries below the account trees; selecting one replaces the list and body columns.

**Rules.**
- Pending proposals first, each shown as its TOML source (monospace, read-only), with **Approve** and **Reject** (`rules::edit::approve`, `reject`).
- Then all rules in file order: name, enabled switch, scope (account, folder), `proposed_by`. The switch calls new `rules::edit::set_enabled(path, name, enabled)`, a `toml_edit` edit that keeps comments; `approve` becomes a check plus `set_enabled(path, name, true)`.
- **Open rules.toml** runs `open` (macOS) or `xdg-open` (Linux) on the file.
- A parse error shows as a red banner over the view. The sync threads keep the previous rules, as they do today.

**Applying changes.**
- The app polls the modification time of `rules.toml` and `config.toml` every 2 s.
- `rules.toml` changed, or edited from the GUI: reload the Rules view and send `SyncNow` to every running account. Edits from an editor, `postbode rules propose` or an agent take effect within about 2 s. A rule acts on mail from the moment it is enabled; existing mail stays untouched unless `postbode rules apply-existing` is run.
- `config.toml` changed: a banner, "config.toml changed — restart Postbode to apply". No live reload.

**Activity.** `store.log` of every account, newest first, merged by time: time, account, rule, action, sender, subject.

**Trash.** `Trash::list` of every account: deleted-at, account, original folder, sender and subject parsed from the `.eml` headers. **Restore** sends `Command::Restore`; on `Restored` the entry disappears and the folder reloads after the next `Synced`.

## 9. Errors

- Every `Error` event lands in the status history and sets the account's line to show the error.
- Optimistic edits (row removed on archive, move or delete; flag or read toggled) are recorded per uid and rolled back when the matching `ActionDone` result is an error, or when an `Error` arrives for a command with no `ActionDone`.
- A store that fails to open shows its error under that account in the tree; other accounts work.
- An account whose lock is held elsewhere opens read-only: its tree shows "synced by another Postbode process (pid N)", reading works, action keys do nothing and say why in the status bar.
- `open_url` is only called for the three allowed schemes; anything else in a body renders as plain text.

## 10. Testing

**Engine and sync,** with `RecordingOps`:
- Commands are drained before a pass, after IDLE, between folders and between chunks; the wake flag ends IDLE; shutdown still stops the loop.
- Commands sent while offline run after reconnect; a failing command emits `Error` and the session continues.
- Chunked fetch: 1,200 messages with sparse UIDs arrive in three chunks, each committed; an error in chunk 2 leaves chunk 1 stored and the next session resumes from it.
- `initial_uid_next`: a first sync interrupted after one chunk, then resumed, sends no `NewMail`; mail with a uid at or above it notifies.
- Migration 002 on a store created by 001 keeps every row, sets `initial_uid_next` to 0 and `rules_uid` to `last_uid`.
- Activity events arrive in order for a full pass.
- `thread_summaries` and `thread_members` against `threads()` on the same fixture.
- `set_enabled` keeps comments and other rules byte-for-byte.
- Lock: a second `Engine::start` on the same paths reports `Locked` with the first pid; `run` exits non-zero when every account is locked.

**Live Dovecot,** in `tests/imap_live.rs`: a 1,200-message folder syncs in chunks; a `Command` wakes IDLE and runs within 2 s.

**GUI,** with `egui_kittest` and `Engine::detached`:
- `j`/`k` move the selection; `e` on a row sends `Apply` archive for that uid and removes the row; `e` on a thread sends every member uid.
- An `ActionDone` error puts the row back.
- The 1 s read delay: 0.9 s sends nothing, 1.1 s sends mark read.
- Body links: `https://` and `mailto:` are clickable, `javascript:` and `file:` render as text.
- A `FetchingHeaders` activity renders its counts; `Offline` renders its retry time.
- A locked account ignores action keys.
- Rules view: a proposal shows Approve; the switch calls `set_enabled`.
- Theme: `config.toml` without `[ui]` starts with `ThemePreference::System`; choosing dark writes `theme = "dark"`, keeps the file's comments, and raises no banner.

Not automated: pixels, real windowing, notifications. Reported as ran on macOS, compiled on Linux.

## 11. Later

- Compose and send (phase 4), with the OS integration and bundle. Launching from an `.app` needs either a `postbode-gui` binary target or "no args and no terminal opens the GUI".
- HTML bodies: designed in `2026-10-07-postbode-html-design.md`, rendered by Blitz inside the body panel (`wry` was dropped).
- Fonts beyond egui's defaults: CJK and other scripts render as boxes until system fonts are loaded at startup; egui has no right-to-left shaping.
- Apply a rule to existing mail from the GUI, with a dry-run count and confirmation.
- Account setup in the GUI; live reload of `config.toml`.
- Font size and other appearance settings beyond the theme.
- Unified inbox; undo; remembered window and pane sizes; tray and close-to-background; native macOS menu bar (`muda`).
