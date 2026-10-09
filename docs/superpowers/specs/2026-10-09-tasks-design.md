# Tasks over IMAP — design

Status: draft, 2026-10-09

## Goal

Replace TickTick with tasks that live in the user's own IMAP account. Each task is an email in a `Tasks/` folder, so tasks sync through any IMAP server, survive without Postvak and can be read in any mail client. A future Android app must be able to reuse the engine through Kotlin bindings.

## Decisions

- Works on **any IMAP server**: all task data lives in the message, not in keywords or flags.
- Task data is an **iCalendar VTODO** (RFC 5545) part in the message.
- **Agents write tasks directly** over MCP, with no proposal step.
- **Reminders are desktop-only** (daemon notifications). Phone reminders come with the Android app.
- **Recurrence and subtasks are v2.** The format already allows them.
- **No TickTick importer.** An agent reads the TickTick CSV export and calls `task_create`.
- **Done-task retention is a rule**, not a setting.

## Storage

### Folders

- The root folder is `Tasks`, configurable as `[tasks] folder` in `config.toml`. It is created when the first task is added.
- Tasks directly in the root form the **Inbox** list. Each subfolder (`Tasks/Work`) is a list. List create, rename and delete map to IMAP folder operations; deleting a list that isn't empty is refused.
- `Tasks/Done` is reserved. Completing a task moves it there and records its list as `X-POSTVAK-LIST`. Reopening moves it back, or to Inbox when that list no longer exists.

### Message

```
From: <account address>
Subject: <title>
Content-Type: multipart/alternative
  ├─ text/plain                       notes, readable in any client
  └─ text/calendar; component=VTODO   source of truth
```

VTODO properties in the MVP:

| Property | Use |
|---|---|
| `UID` | Stable task identity across rewrites |
| `SEQUENCE`, `LAST-MODIFIED`, `DTSTAMP` | Conflict resolution |
| `SUMMARY`, `DESCRIPTION` | Title, notes |
| `DUE` | `DATE` or `DATE-TIME`, never a date silently turned into midnight |
| `PRIORITY` | 1 / 5 / 9 = high / medium / low, 0 = none |
| `CATEGORIES` | Tags |
| `STATUS`, `COMPLETED` | `NEEDS-ACTION` / `COMPLETED` and when |
| `VALARM` | Reminder |
| `X-POSTVAK-LIST` | Original list of a done task |

Properties Postvak doesn't understand (`RRULE`, `RELATED-TO`, other apps' `X-` properties) are kept unchanged on every rewrite.

### Plain mail as a task

A message in `Tasks/` without a VTODO is an open task titled by its subject. Nothing is written until it is edited. The first edit writes a task message with the original attached as `message/rfc822`.

### Edits

IMAP messages can't change, so every edit writes a new version:

1. `APPEND` the new version with `SEQUENCE` incremented. With UIDPLUS (RFC 4315) the new UID comes from `APPENDUID`. Without it, a `SEARCH` on the VTODO UID finds it.
2. Mark the old version `\Deleted` and `UID EXPUNGE` it (or `EXPUNGE` without UIDPLUS).

Write first, delete after: a failure in between leaves a duplicate, never a lost task. Deleting a task uses the existing delete path, so its `.eml` goes to the local trash for 30 days.

### Sync and conflicts

- Existing folder sync fetches task messages. **Migration 008** adds a `tasks` table, filled by parsing the VTODO when a message in `Tasks/` is synced.
- When two messages share a VTODO `UID`, the one with the highest `SEQUENCE`, then the latest `LAST-MODIFIED`, wins and the other is expunged. This covers both a failed expunge and two offline devices editing one task. There is no field-level merge.
- An unparseable VTODO shows as a read-only task with a warning. Editing it is refused; deleting it is allowed.

### Retention of done tasks

A documented rule example, no new code:

```toml
[[rules]]
name = "forget done tasks"
folder = "Tasks/Done"
match.older_than = "90days"
actions = ["delete"]
```

Completing a task rewrites it, so its arrival date in `Tasks/Done` is the completion date. **Verify during planning:** that `older_than` measures the server's INTERNALDATE and accepts `90days`.

## Engine

`postvak::engine::tasks` holds all task logic. GUI, CLI, MCP and the daemon only call it.

**Kotlin-ready constraints:** the public API uses plain owned types (structs, enums, `String`, `Vec`, `Option`) with no generics or lifetimes, so UniFFI can expose it later. No platform code (paths, keychain, notifications) in this module. **Verify during planning** that the engine has no desktop-only dependencies on this path. UniFFI itself is not added now.

```rust
pub struct Task {
    pub uid: String,
    pub list: String,
    pub title: String,
    pub notes: String,
    pub due: Option<Due>,
    pub priority: Priority,
    pub tags: Vec<String>,
    pub done: bool,
    pub completed_at: Option<DateTime>,
    pub reminder: Option<DateTime>,
}
pub enum Due { Date(NaiveDate), DateTime(DateTime) }
pub enum Priority { None, Low, Medium, High }
pub enum View { Inbox, List(String), Today, Overdue, Next7, Tag(String), Done }
```

Commands: `list_tasks(view)`, `create_task`, `update_task`, `complete_task`, `reopen_task`, `delete_task`, `move_task`, `lists`, `create_list`, `rename_list`, `delete_list`, `create_task_from_mail(message, draft)`, `due_reminders(before)`.

Smart views are local SQL on the `tasks` table, in local time: **Today** = due today; **Overdue** = open and due before now (a date-only due is overdue from the next day); **Next 7 days** = due within today plus six days.

### Quick add

`parse_quick_add(&str) -> Draft` understands, in English:

- dates: `today`, `tomorrow`, weekday names (next occurrence), `in N days`, ISO `2026-10-12`
- an optional time: `9:00`, `14:30`
- `#tag`, `!1`–`!3` (high to low priority), `^List`

The rest is the title. Unrecognised tokens stay in the title.

## Surfaces

### CLI

```
postvak task add "<quick add>"
postvak task list [--today|--overdue|--next7|--list NAME|--tag TAG|--done]
postvak task done|reopen|rm <id>
postvak task edit <id> [--title …] [--due …] [--priority …] [--tag …] [--list …] [--notes …]
postvak task lists
```

`<id>` is a unique prefix of the VTODO `UID`, like git hashes.

### MCP

`task_list`, `task_create` (every field, including `done`, so it serves the TickTick import), `task_update`, `task_complete`, `task_delete`, `task_lists`. Writes apply directly.

### Daemon

On each existing tick, `due_reminders(now)` is checked and each reminder fires once through the existing notification path.

### GUI

- A **Tasks** view at the top of the Rules / Activity / Backups group at the bottom of the folder pane. `T` switches between mail and tasks. `Tasks/` folders are hidden from the mail folder tree but offered in the `m` move picker.
- Three columns, as in mail: views and lists (Inbox, Today, Overdue, Next 7 days, lists, Done); task rows with checkbox, priority mark, title, tags and due date (marked when overdue); a details pane with editable title, notes, due date and time, priority, tags, reminder and list, and a link to the attached mail when there is one.
- A quick-add line above the task list, opened with `a`, previews the parsed draft live.

| Key | Action |
|---|---|
| `j` / `k`, `x`, Esc, Tab, `/`, `?` | As in mail |
| `a` | Quick add |
| Space | Complete or reopen |
| `#`, Delete | Delete |
| `m` | Move to a list |
| `s` | Cycle priority |
| `d` | Set the due date (quick-add date words) |
| Enter | Edit details |

**Create task from mail:** the mail toolbar gets a **Create task** button (hover: "Moves this mail to Tasks"), with key `t`. It opens the quick-add line pre-filled with the subject. Enter writes the task with the mail attached as `message/rfc822` and removes the original; Esc cancels.

## Errors

- Offline or server errors on task actions surface like mail actions, in the status bar. No offline queue.
- Unparseable VTODO: read-only, see Sync and conflicts.
- A failed expunge after `APPEND` is healed by the duplicate rule on the next sync.

## Testing

- Unit: VTODO parse and serialise round trip, including unknown properties kept intact; the duplicate rule; quick-add parsing; smart-view boundaries at midnight and across timezones.
- Live (Dovecot, `tests/dovecot/compose.yml`): create → complete → Done → reopen; two-device conflict; mail to task; behaviour with and without UIDPLUS if Dovecot can disable it.
- GUI snapshots in `tests/snapshots/`: the Tasks view in light and dark.

## Out of scope

- **v2:** recurrence (RRULE: completing appends the next occurrence), subtasks or checklists, start or snooze dates.
- **Android app:** UniFFI bindings, phone reminders.
- **Not planned:** a TickTick importer, habits, pomodoro, an Eisenhower matrix, a calendar view, shared lists, location reminders, drag and drop, rich-text notes, an offline action queue.
