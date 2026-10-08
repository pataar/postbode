# All Inboxes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An "All inboxes" entry at the top of the folder pane that lists the INBOX threads of every account in one list, newest first, with each row tagged by its account, and every action, the reader, search and the move picker working against each row's own account.

**Architecture:** Each `list::Row` carries the index of its account, and the list state keys marks, expanded threads and pending edits by account, so `app.rs` stops asking "which account does the view show" and asks the row instead. A new `View::AllInboxes` loads `thread_summaries("INBOX")` from every account's store and merges them by date in a pure `list::merge_threads`. The views stay thin and only return `UiAction`s; `app.rs` remains the only code that changes state. No new state field, no new `UiAction`, no persistence.

**Tech Stack:** Rust 2024, eframe/egui 0.36, egui_kittest, egui-phosphor 0.14, chrono, rusqlite.

**Spec:** design canvas https://claude.ai/artifact/B5W17bKLqpKUw9z3bCw65Q (Inbox · Mocha board) + docs/superpowers/specs/2026-10-06-postbode-core-design.md

## Scope

Prerequisites from `docs/superpowers/plans/2026-10-08-gui-redesign.md`, merged first:

- **Task 2** (icon font): `src/gui/icons.rs` and `egui_phosphor::regular`.
- **Task 3** (sidebar polish): `folders::folder_row(ui, selected, icon, label, indent, count, accessible) -> egui::Response`, and the scrolling account tree in `folders::show`.
- **Task 6** (list columns): `list::Columns`, `list::columns(...)`, `list::column_widths`, and the painted row in `list::show`.

If redesign Task 8 (toolbar) or Task 9 (number grouping) has landed, Tasks 2 and 5 below say what to change there; otherwise skip those steps.

| # | Task | What it delivers |
|---|---|---|
| 1 | Rows carry their account | refactor, no visible change: actions, marks, threads and the reader keyed per row |
| 2 | All inboxes view and sidebar entry | `View::AllInboxes`, merged INBOX threads, the entry with the combined unread count |
| 3 | Account tag on rows | the muted account name before the sender, in All inboxes only |
| 4 | Actions, reader and move picker per row | the move picker follows the selected rows' account; regression tests for every action |
| 5 | Search every account | `/` in All inboxes searches every account, newest first |

One branch, `feat/all-inboxes`, one commit per task, one PR. Tasks are ordered: each builds on the previous one.

Decisions taken here:

- **Shown only with two or more accounts.** With one account the entry would repeat INBOX. Its position in `j`/`k` folder stepping follows the same rule.
- **`/` in All inboxes searches every account**, all folders, merged newest first and capped at `SEARCH_LIMIT`; the hint reads "Search every account". Each hit already carries its account (Task 1), so this costs one loop, not a feature. Esc returns to All inboxes.
- **The move picker offers the folders of the account of the selected rows** (the marked rows, else the cursor row), minus the cursor row's folder. Marked rows from more than one account cannot be moved together: `m` refuses and says "move works on one account at a time; mark messages from one account". Archive, delete, flag and read work across accounts, because each account's daemon thread resolves its own Archive and Trash folders.
- **INBOX is the literal `"INBOX"`**, as `sync.rs` stores and queries it (`store.folder("INBOX")`); an account without that folder contributes no rows.
- **The window still opens on the first account's INBOX**, not on All inboxes. Changing the start view is a separate decision.
- **`RowKey` stays `(String, u32)`.** `Account.pending` is already per account and `BodyState` and `App.requested` already pair a `RowKey` with an account; only the list-wide sets (`marked`, `expanded`) gain the account. `Row::id()` gives `(usize, RowKey)` where a key must be unique across accounts.

Not in this plan: unified views of other folders (Sent, Archive), a setting to hide the entry, remembering the last view, colour per account.

## Global Constraints

- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `cargo machete` and `cargo audit` all pass before the PR.
- No `unwrap`, `expect`, `panic!`, `todo!` or `unimplemented!` outside tests; where a call cannot fail, a narrow `#[allow]` says why.
- No new dependency.
- GUI views draw and return `UiAction`s; only `app.rs` changes state (`tests/architecture.rs` checks this).
- Colours come from `theme::Palette`; Mocha stays exact Catppuccin; no blue or purple accents.
- Never read message bodies from a user's store, and never log bodies or secrets; test fixtures only.
- `Row.account` is an index into `App::accounts`, which is fixed for the window's lifetime (a config change asks to reopen the window).
- After GUI changes, regenerate snapshots on Linux with lavapipe: `TZ=UTC UPDATE_SNAPSHOTS=1 cargo test gui::snapshots`, and look at every changed PNG in `tests/snapshots/`.
- Prose changes go in `docs/src/gui.md`; `README.md` is not touched (it holds `docs/src/index.md` only).
- Branch: `git switch -c feat/all-inboxes --no-track origin/main`, first push `git push -u origin HEAD`. Conventional commits.
- Report platform coverage honestly: "compiled on" vs "ran on".

## Review Focus

1. **The same uid in the same folder of two accounts** (every account's INBOX starts at uid 1): marking, optimistic hiding and the cursor must treat them as two messages. Tests in Task 1 (`apply_pending_reads_each_rows_own_account`) and Task 4 (`marking_in_all_inboxes_marks_only_that_accounts_message`).
2. **The same thread in two accounts** (mail sent to both addresses shares its Message-IDs, so its thread id): expanding one leaves the other collapsed, and a reload does not move the cursor to the other account's copy. Tests in Task 1 (`expanding_a_thread_in_one_account_leaves_the_same_thread_in_another_alone`, `the_same_thread_in_another_account_is_another_row`).
3. **One account's store fails to open, or has no INBOX yet**: All inboxes still lists the other accounts and the count ignores the broken one, no panic. Test in Task 2 (`all_inboxes_lists_the_other_accounts_when_one_store_is_broken`).
4. **Marked rows from two accounts, then `m`**: nothing may move into a folder picked from one account's list; the picker stays shut and says why. Test in Task 4 (`moving_marked_rows_from_two_accounts_is_refused`).
5. **A refusal for one account while All inboxes shows both**: only that account's rows come back; the other account's pending edits and queue stay. Test in Task 4 (`a_refusal_for_one_account_puts_back_only_its_rows`).

---

### Task 1: Rows carry their account

**Files:**
- Modify: `src/gui/list.rs` (`Row`, `Row::id`, `Row::from_message`, `Row::from_summary`, `ListState`, `build_rows`, `apply_pending`, `show`, tests)
- Modify: `src/gui/app.rs` (`reload_view` 830–877, `arm_shown_body` 880, `sync_body` 894, `rebuild_rows` 1031, `act` 1049, `selected_rows` 1173, `targets` 1187, `expand` 1210, `collapse` 1232, `same_row` 1280, `UiAction::ToggleMark` in `apply`, tests)
- Modify: `src/gui/snapshots.rs` (`inbox_with_marks_and_an_expanded_thread`)

**Interfaces:**
- Consumes: redesign Task 6's `list::columns` test, which builds a row with `Row::from_message`; it gains the account argument here.
- Produces:

```rust
pub(crate) struct Row { pub account: usize, /* existing fields */ }
impl Row {
    pub fn key(&self) -> RowKey;                       // unchanged: (folder, uid), unique within one account
    pub fn id(&self) -> (usize, RowKey);               // unique across accounts
    pub fn from_message(account: usize, message: &Message) -> Row;
}
pub(crate) struct ListState {
    pub expanded: HashMap<(usize, String), Vec<MessageSummary>>,
    pub marked: HashSet<(usize, RowKey)>,
    pub threads: Vec<(usize, ThreadSummary)>,
    /* cursor, follow_cursor, hits, rows, viewport unchanged */
}
pub(crate) fn build_rows(folder: &str, threads: &[(usize, ThreadSummary)], expanded: &HashMap<(usize, String), Vec<MessageSummary>>) -> Vec<Row>;
/// `pending[i]` is account i's edits, as `App::accounts` is ordered.
pub(crate) fn apply_pending(rows: &mut Vec<Row>, pending: &[&HashMap<RowKey, Vec<Optimistic>>]);
// app.rs
pub(crate) fn selected_rows(&self) -> Vec<&Row>;      // now pub(crate); Task 4 uses it from list.rs
fn targets(&self) -> Vec<(usize, String, Vec<u32>)>;  // (account, folder, uids)
```

- [ ] **Step 1: Write the failing tests**

In `list.rs` tests:

```rust
    #[test]
    fn apply_pending_reads_each_rows_own_account() {
        let thread = |id: &str| ThreadSummary {
            thread_id: id.into(),
            latest: summary(1, ""),
            count: 1,
            unread: true,
            flagged: false,
        };
        let mut rows = build_rows("INBOX", &[(0, thread("a")), (1, thread("b"))], &HashMap::new());
        let home: HashMap<RowKey, Vec<Optimistic>> = HashMap::new();
        let work = HashMap::from([(("INBOX".to_string(), 1), vec![Optimistic::Hidden])]);
        apply_pending(&mut rows, &[&home, &work]);
        let left: Vec<(usize, u32)> = rows.iter().map(|r| (r.account, r.uid)).collect();
        assert_eq!(left, [(0, 1)]);
    }

    #[test]
    fn expanding_a_thread_in_one_account_leaves_the_same_thread_in_another_alone() {
        let thread = ThreadSummary {
            thread_id: "t".into(),
            latest: summary(5, ""),
            count: 2,
            unread: true,
            flagged: false,
        };
        let threads = vec![(0, thread.clone()), (1, thread)];
        let expanded = HashMap::from([((1, "t".to_string()), vec![summary(4, "\\Seen"), summary(5, "")])]);
        let rows = build_rows("INBOX", &threads, &expanded);
        let shape: Vec<(usize, u32, bool)> = rows.iter().map(|r| (r.account, r.uid, r.member)).collect();
        assert_eq!(shape, [(0, 5, false), (1, 5, false), (1, 4, true), (1, 5, true)]);
    }
```

In `app.rs` tests:

```rust
    #[test]
    fn the_same_thread_in_another_account_is_another_row() {
        let mut home = Row::from_message(0, &message("INBOX", 1, "hi"));
        home.thread_id = Some("<t@example.com>".into());
        let mut work = home.clone();
        work.account = 1;
        assert!(same_row(&home, &home.clone()));
        assert!(!same_row(&work, &home));
    }
```

`ThreadSummary` and `MessageSummary` derive `Clone` (`store.rs` lines 81 and 103).

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::list gui::app`
Expected: FAIL to compile (`Row` has no field `account`, `build_rows` takes `&[ThreadSummary]`, `from_message` takes one argument).

- [ ] **Step 3: Change `list.rs`**

```rust
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    /// Index into `App::accounts`: the account the message lives in.
    pub account: usize,
    pub count: u32,
    // … the other fields unchanged
}

impl Row {
    pub fn key(&self) -> RowKey {
        (self.folder.clone(), self.uid)
    }

    /// The key across accounts: a uid is only unique within one account's folder.
    pub fn id(&self) -> (usize, RowKey) {
        (self.account, self.key())
    }

    pub fn from_message(account: usize, message: &Message) -> Row {
        Row {
            account,
            // … the other fields unchanged
        }
    }

    fn from_summary(account: usize, folder: &str, message: &MessageSummary, member: bool) -> Row {
        Row {
            account,
            // … the other fields unchanged
        }
    }
}

#[derive(Default)]
pub(crate) struct ListState {
    pub cursor: usize,
    /// Expanded threads by account and thread id: mail sent to two accounts shares its thread id.
    pub expanded: HashMap<(usize, String), Vec<MessageSummary>>,
    /// Set when a key moved the cursor, so this frame scrolls it into view.
    pub follow_cursor: bool,
    pub hits: Vec<Row>,
    pub marked: HashSet<(usize, RowKey)>,
    pub rows: Vec<Row>,
    /// Each thread with the account it is in.
    pub threads: Vec<(usize, ThreadSummary)>,
    /// Scroll offset and height of the list from the last frame.
    pub viewport: (f32, f32),
}

/// One row per thread describing its latest message, followed by its members when expanded.
pub(crate) fn build_rows(
    folder: &str,
    threads: &[(usize, ThreadSummary)],
    expanded: &HashMap<(usize, String), Vec<MessageSummary>>,
) -> Vec<Row> {
    let mut rows = Vec::with_capacity(threads.len());
    for (account, thread) in threads {
        let mut row = Row::from_summary(*account, folder, &thread.latest, false);
        row.count = thread.count;
        row.flagged = thread.flagged;
        row.thread_id = Some(thread.thread_id.clone());
        row.unread = thread.unread;
        rows.push(row);
        if let Some(members) = expanded.get(&(*account, thread.thread_id.clone())) {
            rows.extend(members.iter().map(|m| Row::from_summary(*account, folder, m, true)));
        }
    }
    rows
}

/// Rows of sent actions as they will be: moved rows hidden, read and flag changes shown, a later edit over an earlier.
/// `pending[i]` holds account i's edits.
pub(crate) fn apply_pending(rows: &mut Vec<Row>, pending: &[&HashMap<RowKey, Vec<Optimistic>>]) {
    if pending.iter().all(|edits| edits.is_empty()) {
        return;
    }
    rows.retain(|row| !edits_of(pending, row).contains(&Optimistic::Hidden));
    for row in rows {
        for edit in edits_of(pending, row) {
            match edit {
                Optimistic::Flagged(flagged) => row.flagged = *flagged,
                Optimistic::Seen(seen) => row.unread = !seen,
                Optimistic::Hidden => {}
            }
        }
    }
}

fn edits_of<'a>(pending: &[&'a HashMap<RowKey, Vec<Optimistic>>], row: &Row) -> &'a [Optimistic] {
    pending
        .get(row.account)
        .and_then(|edits| edits.get(&row.key()))
        .map_or(&[][..], Vec::as_slice)
}
```

In `show`: `let marked = list.marked.contains(&row.id());`.

Update the existing tests: in `build_rows_puts_members_under_an_expanded_thread` wrap each `ThreadSummary` as `(0, ThreadSummary { … })` and insert into `expanded` with key `(0, "t".to_string())`; in `apply_pending_hides_and_overrides_rows` wrap the threads as `(0, thread(…))` and call `apply_pending(&mut rows, &[&pending])`; add `account: 0,` to the `Row` literal in `row_text_strips_control_characters_from_headers`; redesign Task 6's `columns_split_markers_sender_subject_and_date` calls `Row::from_message(0, &message("INBOX", 1, "Lunch?"))`.

- [ ] **Step 4: Change `app.rs`**

`reload_view`, the folder part (search and threads tag their account; the expanded loop uses the new key):

```rust
        if let Some(query) = &self.search {
            self.list.hits = match store.search(query, None, list::SEARCH_LIMIT) {
                Ok(found) => found.iter().map(|m| Row::from_message(account, m)).collect(),
                // … Err arm unchanged
            };
            self.rebuild_rows();
            return;
        }
        match store.thread_summaries(&folder, THREAD_LIMIT) {
            Ok(threads) => self.list.threads = threads.into_iter().map(|t| (account, t)).collect(),
            // … Err arm unchanged
        }
        let expanded: Vec<(usize, String)> = self.list.expanded.keys().cloned().collect();
        for key in expanded {
            match store.thread_members(&folder, &key.1) {
                Ok(members) if members.len() > 1 => {
                    self.list.expanded.insert(key, members);
                }
                _ => {
                    self.list.expanded.remove(&key);
                }
            }
        }
```

`arm_shown_body` and `sync_body` ask the row, not the view:

```rust
    fn arm_shown_body(&mut self, index: usize) {
        let Some((account, key)) = self.list.rows.get(index).map(Row::id) else {
            return;
        };
        if let Some(body) = &mut self.body
            && body.account == account
            && body.key == key
        {
            body.armed = true;
        }
    }
```

In `sync_body`: `let current = self.list.rows.get(self.list.cursor).map(Row::id);` (rows are empty outside mail views, since `select_view` resets the list).

`rebuild_rows`, after building `rows`:

```rust
        let pending: Vec<&HashMap<RowKey, Vec<Optimistic>>> = self.accounts.iter().map(|a| &a.pending).collect();
        list::apply_pending(&mut rows, &pending);
```

and the `account` binding in its `let View::Folder { account, folder }` becomes `View::Folder { folder, .. }`.

`act`, `targets` and `selected_rows`:

```rust
    /// Sends `action` for the marked rows, or the cursor row, each to its own account, and shows its effect before the
    /// server confirms it.
    fn act(&mut self, action: Action) {
        let Some(optimistic) = Optimistic::of(&action) else {
            return;
        };
        let targets = self.targets();
        if targets.is_empty() {
            return;
        }
        if matches!(action, Action::MarkRead | Action::MarkUnread)
            && let Some(body) = &mut self.body
            && targets.iter().any(|(account, folder, uids)| {
                *account == body.account && *folder == body.key.0 && uids.contains(&body.key.1)
            })
        {
            body.read_sent = true;
        }
        self.pending_arm |= optimistic == Optimistic::Hidden;
        for (account, folder, uids) in targets {
            self.send_apply(account, folder, uids, action.clone(), optimistic);
        }
        self.list.marked.clear();
        self.rebuild_rows();
    }

    /// The rows an action covers: the marked rows, or else the cursor row.
    pub(crate) fn selected_rows(&self) -> Vec<&Row> {
        if self.list.marked.is_empty() {
            self.list.rows.get(self.list.cursor).into_iter().collect()
        } else {
            self.list.rows.iter().filter(|row| self.list.marked.contains(&row.id())).collect()
        }
    }

    /// The uids each action covers, per account and folder: the marked rows or the cursor row, a thread row standing
    /// for every message of its thread in that folder.
    fn targets(&self) -> Vec<(usize, String, Vec<u32>)> {
        let mut by_folder: BTreeMap<(usize, String), BTreeSet<u32>> = BTreeMap::new();
        for row in self.selected_rows() {
            let uids = by_folder.entry((row.account, row.folder.clone())).or_default();
            let members = match (&row.thread_id, &self.accounts[row.account].store) {
                (Some(thread), Ok(store)) if row.count > 1 => store.thread_members(&row.folder, thread).ok(),
                _ => None,
            };
            match members {
                Some(members) => uids.extend(members.iter().map(|member| member.uid)),
                None => {
                    uids.insert(row.uid);
                }
            }
        }
        by_folder
            .into_iter()
            .map(|((account, folder), uids)| (account, folder, uids.into_iter().collect()))
            .collect()
    }
```

`UiAction::ToggleMark`:

```rust
            UiAction::ToggleMark => {
                if let Some(id) = self.list.rows.get(self.list.cursor).map(Row::id)
                    && !self.list.marked.remove(&id)
                {
                    self.list.marked.insert(id);
                }
            }
```

`expand` and `collapse` key threads by account:

```rust
    fn expand(&mut self) {
        let Some(row) = self.list.rows.get(self.list.cursor) else {
            return;
        };
        let (Some(thread), folder) = (row.thread_id.clone(), row.folder.clone()) else {
            return;
        };
        let key = (row.account, thread);
        if row.count < 2 || self.list.expanded.contains_key(&key) {
            return;
        }
        if let Ok(store) = &self.accounts[key.0].store
            && let Ok(members) = store.thread_members(&folder, &key.1)
        {
            self.list.expanded.insert(key, members);
            self.rebuild_rows();
        }
    }
```

In `collapse`, replace the removal with `if self.list.expanded.remove(&(rows[parent].account, thread)).is_some() {`.

`same_row` compares the account first:

```rust
fn same_row(row: &Row, cursor: &Row) -> bool {
    if row.account != cursor.account || row.member != cursor.member {
        return false;
    }
    match (&row.thread_id, &cursor.thread_id) {
        (Some(row_thread), Some(cursor_thread)) if !row.member => row_thread == cursor_thread,
        _ => row.key() == cursor.key(),
    }
}
```

`view_account` stays: `refresh`, `OpenMovePicker` and `StartSearch` still use it until Tasks 2, 4 and 5.

`snapshots.rs`, in `inbox_with_marks_and_an_expanded_thread`: `list.marked.contains(&(1, ("INBOX".to_string(), uid)))` (work is account 1 in that fixture).

Run: `cargo check --all-targets --all-features` and fix any caller the compiler still lists.

- [ ] **Step 5: Run the GUI suite and the architecture check**

Run: `cargo test --lib gui:: && cargo test --test architecture`
Expected: PASS, including every existing action, thread and body test unchanged: single-account behaviour is the same.

- [ ] **Step 6: Snapshots**

Run on Linux: `TZ=UTC UPDATE_SNAPSHOTS=1 cargo test gui::snapshots`
Expected: no PNG changes.

- [ ] **Step 7: Commit**

```bash
git add src/gui/list.rs src/gui/app.rs src/gui/snapshots.rs
git commit -m "refactor(gui): give each list row its account and key marks and threads by it"
```

---

### Task 2: All inboxes view and sidebar entry

**Files:**
- Modify: `src/gui/app.rs` (`View`, `View::lists_mail`, `App::shows_all_inboxes`, `list_folder`, `reload_view`, `rebuild_rows`, `refresh` 549, `show` 341, `tree_entries` 1251, tests)
- Modify: `src/gui/list.rs` (`INBOX`, `merge_threads`, tests)
- Modify: `src/gui/folders.rs` (`inbox_unread`, entry on top, tests)
- Modify: `src/gui/icons.rs` (`ALL_INBOXES`)
- Modify: `docs/src/gui.md`
- Modify, if redesign Task 8 landed: `src/gui/toolbar.rs`

**Interfaces:**
- Consumes: Task 1's `Row.account`, `ListState.threads: Vec<(usize, ThreadSummary)>`; redesign Task 3's `folders::folder_row`; redesign Task 2's `icons` module.
- Produces:

```rust
// app.rs
pub(crate) enum View { Activity, AllInboxes, Folder { account: usize, folder: String }, Rules, Trash }
impl View { pub(crate) fn lists_mail(&self) -> bool; }   // Folder or AllInboxes: list and reader panes
impl App { pub(crate) fn shows_all_inboxes(&self) -> bool; }   // two or more accounts
// list.rs
pub(crate) const INBOX: &str = "INBOX";
pub(crate) fn merge_threads(per_account: Vec<(usize, Vec<ThreadSummary>)>, limit: u32) -> Vec<(usize, ThreadSummary)>;
// folders.rs
pub(crate) fn inbox_unread(accounts: &[Account]) -> u32;
// icons.rs
pub(crate) const ALL_INBOXES: &str = ph::STACK;
```

- [ ] **Step 1: Write the failing tests**

In `list.rs` tests:

```rust
    #[test]
    fn merge_threads_is_newest_first_across_accounts_and_capped() {
        let thread = |uid: u32| ThreadSummary {
            thread_id: format!("t{uid}"),
            latest: summary(uid, ""),
            count: 1,
            unread: false,
            flagged: false,
        };
        let merged = merge_threads(vec![(0, vec![thread(5), thread(1)]), (1, vec![thread(3), thread(1)])], 3);
        let shape: Vec<(usize, u32)> = merged.iter().map(|(account, t)| (*account, t.latest.uid)).collect();
        assert_eq!(shape, [(0, 5), (1, 3), (0, 1)], "equal dates keep account order; the cap drops the rest");
    }
```

(`summary(uid, …)` dates each message `1_790_000_000 + uid`, so equal uids are equal dates.)

In `app.rs` tests:

```rust
    fn ids(harness: &egui_kittest::Harness<'_, App>) -> Vec<(usize, u32)> {
        harness.state().list.rows.iter().map(|r| (r.account, r.uid)).collect()
    }

    #[test]
    fn all_inboxes_lists_every_accounts_inbox_newest_first() {
        let fx = Fixture::new(&["home", "work"]);
        fx.add("home", message("INBOX", 1, "oldest"));
        fx.add("work", message("INBOX", 2, "middle"));
        fx.add("home", message("INBOX", 3, "newest"));
        fx.folder("work", "Archive", Some("Archive"));
        fx.add("work", message("Archive", 4, "not an inbox"));
        let (mut harness, _wires) = fx.harness();
        harness.state_mut().select_view(View::AllInboxes);
        harness.run();
        assert_eq!(ids(&harness), [(0, 3), (1, 2), (0, 1)]);
    }

    #[test]
    fn all_inboxes_lists_the_other_accounts_when_one_store_is_broken() {
        let fx = Fixture::new(&["broken", "work"]);
        fx.add("work", message("INBOX", 1, "still here"));
        let db = fx.paths.mail_db("broken");
        std::fs::remove_file(&db).unwrap();
        std::fs::create_dir_all(&db).unwrap();
        let (mut harness, _wires) = fx.harness();
        harness.state_mut().select_view(View::AllInboxes);
        harness.run();
        assert_eq!(ids(&harness), [(1, 1)]);
        assert!(harness.query_by_label("All inboxes").is_some());
    }
```

In `folders.rs` tests:

```rust
    #[test]
    fn all_inboxes_sits_on_top_with_every_inbox_unread_count_and_opens_on_click() {
        let fx = Fixture::new(&["home", "work"]);
        fx.folder("work", "Archive", Some("Archive"));
        for (account, folder, uid) in [("home", "INBOX", 1), ("work", "INBOX", 2), ("work", "INBOX", 3), ("work", "Archive", 4)] {
            let mut unread = message(folder, uid, "hello");
            unread.flags = String::new();
            fx.add(account, unread);
        }
        let (mut harness, _wires) = fx.harness();
        let entry = harness.get_by_label("All inboxes (3)").rect();
        assert!(entry.bottom() < harness.get_by_label("home").rect().top());
        harness.get_by_label("All inboxes (3)").click();
        harness.run();
        assert_eq!(harness.state().view, View::AllInboxes);
    }

    #[test]
    fn one_account_has_no_all_inboxes_entry() {
        let fx = Fixture::new(&["work"]);
        let (harness, _wires) = fx.harness();
        assert!(harness.query_by_label_contains("All inboxes").is_none());
    }

    #[test]
    fn k_in_the_folder_pane_steps_up_from_the_first_inbox_to_all_inboxes() {
        let fx = Fixture::new(&["home", "work"]);
        let (mut harness, _wires) = fx.harness();
        for _ in 0..2 {
            harness.key_press(egui::Key::Tab);
            harness.run();
        }
        harness.event(egui::Event::Text("k".into()));
        harness.run();
        assert_eq!(harness.state().view, View::AllInboxes);
    }
```

`folders.rs` tests need `use eframe::egui;` for the last test. The heading label "home" is the account's `ui.strong(&account.name)`; status lines read "home: …", so the exact label is unique.

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::`
Expected: FAIL to compile (`View::AllInboxes`, `merge_threads` missing).

- [ ] **Step 3: Implement the pure part** in `list.rs`

```rust
/// The folder All inboxes shows from each account; sync stores and queries it by this exact name.
pub(crate) const INBOX: &str = "INBOX";

/// Threads of several accounts in one list, the most recently active first, at most `limit`. The sort is stable, so
/// each account keeps its own order and equal dates keep account order.
pub(crate) fn merge_threads(per_account: Vec<(usize, Vec<ThreadSummary>)>, limit: u32) -> Vec<(usize, ThreadSummary)> {
    let mut threads: Vec<(usize, ThreadSummary)> = per_account
        .into_iter()
        .flat_map(|(account, found)| found.into_iter().map(move |thread| (account, thread)))
        .collect();
    threads.sort_by_key(|(_, thread)| std::cmp::Reverse(thread.latest.date));
    threads.truncate(limit as usize);
    threads
}
```

`thread.latest.date` is the latest message's `internaldate` (`SUMMARY_COLUMNS` in `store.rs`), the same value `thread_summaries` orders by.

- [ ] **Step 4: The view in `app.rs`**

```rust
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum View {
    Activity,
    /// Every account's INBOX in one list.
    AllInboxes,
    Folder { account: usize, folder: String },
    Rules,
    Trash,
}

impl View {
    /// The views with a message list and a reader.
    pub(crate) fn lists_mail(&self) -> bool {
        matches!(self, View::AllInboxes | View::Folder { .. })
    }
}
```

In `impl App`:

```rust
    /// With one account, All inboxes would only repeat its INBOX.
    pub(crate) fn shows_all_inboxes(&self) -> bool {
        self.accounts.len() > 1
    }

    /// The folder the list shows: the view's own, or INBOX in every account.
    fn list_folder(&self) -> Option<&str> {
        match &self.view {
            View::AllInboxes => Some(list::INBOX),
            View::Folder { folder, .. } => Some(folder.as_str()),
            _ => None,
        }
    }
```

`reload_view` loads from every account the view covers:

```rust
    pub(crate) fn reload_view(&mut self) {
        let Some(folder) = self.list_folder().map(str::to_string) else {
            match self.view {
                View::Activity => self.activity_log = rules::activity_log(&self.accounts),
                View::Trash => self.trash = rules::trash_rows(&self.accounts, &self.paths),
                _ => {}
            }
            return;
        };
        let accounts: Vec<usize> = match self.view {
            View::Folder { account, .. } => vec![account],
            _ => (0..self.accounts.len()).collect(),
        };
        // A store that failed to open shows its error in the folder pane and adds no rows.
        let stores: Vec<(usize, &Store)> = accounts
            .into_iter()
            .filter_map(|index| self.accounts[index].store.as_ref().ok().map(|store| (index, store)))
            .collect();
        if let Some(query) = &self.search {
            let mut hits = Vec::new();
            for &(account, store) in &stores {
                match store.search(query, None, list::SEARCH_LIMIT) {
                    Ok(found) => hits.extend(found.iter().map(|m| Row::from_message(account, m))),
                    Err(e) => log::warn!("[{}] search failed: {e}", self.accounts[account].name),
                }
            }
            self.list.hits = hits;
        } else {
            let mut found = Vec::new();
            for &(account, store) in &stores {
                match store.thread_summaries(&folder, THREAD_LIMIT) {
                    Ok(threads) => found.push((account, threads)),
                    Err(e) => log::warn!("[{}] {folder}: could not load threads: {e}", self.accounts[account].name),
                }
            }
            self.list.threads = list::merge_threads(found, THREAD_LIMIT);
            let expanded: Vec<(usize, String)> = self.list.expanded.keys().cloned().collect();
            for key in expanded {
                let members = stores
                    .iter()
                    .find(|(account, _)| *account == key.0)
                    .and_then(|(_, store)| store.thread_members(&folder, &key.1).ok());
                match members {
                    Some(members) if members.len() > 1 => {
                        self.list.expanded.insert(key, members);
                    }
                    _ => {
                        self.list.expanded.remove(&key);
                    }
                }
            }
        }
        self.rebuild_rows();
    }
```

Behaviour change to note in the PR: a failed `thread_summaries` query now empties that account's rows instead of keeping the previous ones; a failed store still shows no rows, as before. The borrows split by field (`stores` borrows `self.accounts`, the writes go to `self.list`); `stores` is dead before `self.rebuild_rows()`. If the borrow checker disagrees, collect `stores` inside a block that ends before `rebuild_rows`.

`rebuild_rows` starts with `let Some(folder) = self.list_folder() else { return; };` and calls `list::build_rows(folder, &self.list.threads, &self.list.expanded)`; take `current` before that line, and copy `folder` with `.to_string()` if the borrow of `self` conflicts with `self.list.rows = rows`.

`refresh`:

```rust
        if self.view_account() == Some(index)
            || matches!(self.view, View::AllInboxes | View::Activity | View::Trash)
        {
            self.view_dirty = true;
        }
```

`show`: the first arm becomes `View::AllInboxes | View::Folder { .. } => {`.

`tree_entries`: start from `let mut entries: Vec<View> = Vec::new(); if self.shows_all_inboxes() { entries.push(View::AllInboxes); }`, then `entries.extend(` the per-account folder views, then Rules, Activity and Trash as now. Update its doc comment to "All inboxes when shown, every folder of every account, then Rules, Activity and Trash, in tree order."

`shows_recipient` needs no change: its `let View::Folder … else { return false }` covers All inboxes.

- [ ] **Step 5: The sidebar entry**

`icons.rs`, first in the list:

```rust
pub(crate) const ALL_INBOXES: &str = ph::STACK;
```

(`egui_phosphor::regular::STACK` is `"\u{E466}"`, checked in egui-phosphor 0.14.0 `variants/codepoints.rs`.)

`folders.rs`:

```rust
/// Unread mail in every account's INBOX, for the All inboxes count.
pub(crate) fn inbox_unread(accounts: &[Account]) -> u32 {
    accounts
        .iter()
        .flat_map(|account| &account.folders)
        .filter(|folder| folder.name == list::INBOX)
        .map(|folder| folder.unread)
        .sum()
}
```

with `use super::app::Account;` and `use super::list;`. In `show`, first inside the account `ScrollArea`, before the `for (index, account)` loop:

```rust
        if app.shows_all_inboxes() {
            let label = "All inboxes";
            let (count, accessible) = match inbox_unread(&app.accounts) {
                0 => (None, label.to_string()),
                unread => (Some(unread.to_string()), format!("{label} ({unread})")),
            };
            let selected = app.view == View::AllInboxes;
            if folder_row(ui, selected, icons::ALL_INBOXES, label, 0.0, count, &accessible).clicked() {
                actions.push(UiAction::SelectView(View::AllInboxes));
            }
            ui.separator();
        }
```

If redesign Task 9 landed, the count is `Some(numbers::count(u64::from(unread)))` and the accessible name uses the same string, like the folder rows. Update the module doc to "The left column: All inboxes when there are several accounts, one tree per account, then …".

- [ ] **Step 6: Toolbar (only if redesign Task 8 landed)**

In `toolbar.rs`, `has_row` becomes `app.view.lists_mail() && !app.list.rows.is_empty()`. The search button stays `matches!(app.view, View::Folder { .. })` until Task 5.

- [ ] **Step 7: Run tests**

Run: `cargo test --lib gui:: && cargo test --test architecture`
Expected: PASS. `tab_cycles_focus_and_j_in_the_folder_pane_opens_the_next_folder` still passes: it has one account, so no All inboxes entry.

- [ ] **Step 8: Docs, snapshots, commit**

`docs/src/gui.md`, after the first paragraph: "With two or more accounts, **All inboxes** at the top of the folder pane lists the INBOX of every account in one list, newest first; its count is the unread mail in all of them. Actions, the reader and `m` work on each message in its own account."

Regenerate on Linux: every snapshot from `mailbox()` (two accounts) gains the entry and a divider above "home"; look at each PNG.

```bash
git add src/gui/ docs/src/gui.md tests/snapshots/
git commit -m "feat(gui): add All inboxes, every account's INBOX in one list"
```

---

### Task 3: Account tag on rows

**Files:**
- Modify: `src/gui/list.rs` (`row_text`, `Columns`, `columns`, row drawing in `show`, tests)
- Modify: `src/gui/snapshots.rs` (new `all_inboxes` snapshot)
- Create: `tests/snapshots/gui_all_inboxes.png` (generated)

**Interfaces:**
- Consumes: Task 2's `View::AllInboxes`; redesign Task 6's `Columns`, `columns`, `column_widths` and row painting.
- Produces:

```rust
pub(crate) struct Columns { pub account: Option<String>, /* Task 6 fields */ }
pub(crate) fn columns<Tz: TimeZone>(row: &Row, account: Option<&str>, marked: bool, recipient: bool, in_search: bool, now: &DateTime<Tz>) -> Columns where Tz::Offset: Display;
pub(crate) fn row_text<Tz: TimeZone>(row: &Row, account: Option<&str>, recipient: bool, in_search: bool, now: &DateTime<Tz>) -> String where Tz::Offset: Display;
```

`account` is `Some(name)` in All inboxes and `None` elsewhere.

- [ ] **Step 1: Write the failing tests** in `list.rs` tests

```rust
    #[test]
    fn the_account_tag_comes_before_the_sender() {
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let row = Row::from_message(1, &message("INBOX", 1, "hello"));
        let text = row_text(&row, Some("work"), false, false, &now);
        assert!(text.starts_with("   work · Sender 1 — hello"), "{text}");
        assert!(row_text(&row, None, false, false, &now).starts_with("   Sender 1 — hello"));
        assert_eq!(columns(&row, Some("work"), false, false, false, &now).account.as_deref(), Some("work"));
        assert_eq!(columns(&row, None, false, false, false, &now).account, None);
    }

    #[test]
    fn rows_name_their_account_in_all_inboxes_only() {
        let fx = Fixture::new(&["home", "work"]);
        fx.add("home", message("INBOX", 1, "from home"));
        let (mut harness, _wires) = fx.harness();
        assert!(harness.query_by_label_contains("Sender 1 — from home").is_some());
        assert!(harness.query_by_label_contains("home · Sender 1").is_none());
        harness.state_mut().select_view(View::AllInboxes);
        harness.run();
        assert!(harness.query_by_label_contains("home · Sender 1 — from home").is_some());
    }
```

The second test starts on `View::Folder { account: 0, folder: "INBOX" }`, which is home's.

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::list`
Expected: FAIL to compile (`row_text` and `columns` take one argument fewer).

- [ ] **Step 3: Implement**

`row_text`, after the member indent and before the search folder:

```rust
    if let Some(account) = account {
        text.push_str(&format!("{account} · "));
    }
```

`Columns` gains `pub account: Option<String>,` (alphabetical, first), and `columns` takes `account: Option<&str>` as its second parameter and sets `account: account.map(|name| clean(name, false)),`.

In `show`, per row:

```rust
            let account = (app.view == View::AllInboxes).then(|| app.accounts[row.account].name.as_str());
            let c = columns(row, account, marked, recipient, app.search.is_some(), &now);
```

and pass `account` to `row_text` for the accessible name. Draw the tag inside the sender column, before the sender, in `muted` at two points under the row font, capped at 80 px so a long account name cannot take the whole column:

```rust
            let mut sender_x = x + indent;
            if let Some(account) = &c.account {
                let small = egui::FontId::new(font.size - 2.0, font.family.clone());
                let mut job = egui::text::LayoutJob::simple_singleline(account.clone(), small, ink(palette.muted));
                job.wrap = egui::text::TextWrapping::truncate_at_width(80.0);
                let tag = painter.layout_job(job);
                painter.galley(egui::pos2(sender_x, rect.center().y - tag.size().y / 2.0), tag.clone(), ink(palette.muted));
                sender_x += tag.size().x + 6.0;
            }
            let sender = truncated(&c.who, (sender_w - (sender_x - x)).max(0.0));
            painter.galley(egui::pos2(sender_x, rect.center().y - sender.size().y / 2.0), sender, text_color);
```

This replaces Task 6's two sender lines (`let sender = truncated(&c.who, sender_w - indent);` and its `painter.galley`). `LayoutJob::simple_singleline`, `TextWrapping::truncate_at_width`, `Painter::layout_job`, `Painter::galley` and `Galley::size` were checked in egui/epaint 0.36.2. The canvas shows the tag as muted text; if it shows a pill behind it, add `painter.rect_filled(tag_rect.expand(2.0), 3.0, palette.faint)` before the text (not verified against the canvas).

- [ ] **Step 4: Snapshot test** in `snapshots.rs`

```rust
#[test]
fn all_inboxes() {
    let fx = mailbox("dark");
    let (mut harness, _wires) = open(&fx);
    harness.state_mut().select_view(View::AllInboxes);
    harness.run();
    assert!(harness.query_by_label_contains("home · Mum").is_some());
    assert!(harness.query_by_label_contains("work · Linus Example").is_some());
    snapshot(&mut harness, "gui_all_inboxes");
}
```

- [ ] **Step 5: Run tests, snapshots on Linux**

Run: `cargo test --lib gui::`, then on Linux `TZ=UTC UPDATE_SNAPSHOTS=1 cargo test gui::snapshots`
Expected: PASS; one new PNG, `gui_all_inboxes.png`, with "home" and "work" muted before each sender; the other PNGs unchanged.

- [ ] **Step 6: Commit**

```bash
git add src/gui/list.rs src/gui/snapshots.rs tests/snapshots/
git commit -m "feat(gui): tag each All inboxes row with its account"
```

---

### Task 4: Actions, reader and move picker per row

**Files:**
- Modify: `src/gui/app.rs` (`UiAction::OpenMovePicker` in `apply`, `MOVE_ONE_ACCOUNT`, tests)
- Modify: `src/gui/list.rs` (`show_move_picker`)
- Modify: `docs/src/gui.md`

**Interfaces:**
- Consumes: Task 1's `Row.account`, `Row::id`, `App::selected_rows` (now `pub(crate)`), `targets`; Task 2's `View::AllInboxes`.
- Produces: `app::MOVE_ONE_ACCOUNT: &str`. `m` opens the picker when the selected rows are in one account; `show_move_picker` lists that account's folders.

- [ ] **Step 1: Write the failing tests** in `app.rs` tests

```rust
    fn command(account: &str, folder: &str, uids: &[u32], action: Action) -> (String, Command) {
        let apply = Command::Apply {
            folder: folder.into(),
            uids: uids.to_vec(),
            action,
            by: "gui".into(),
        };
        (account.into(), apply)
    }

    /// home INBOX uid 1, work INBOX uids 1 and 2: rows (work 2), (home 1), (work 1), equal dates in account order.
    fn all_inboxes(fx: &Fixture) -> (egui_kittest::Harness<'static, App>, crate::gui::test_support::Wires) {
        fx.add("home", message("INBOX", 1, "home mail"));
        fx.add("work", message("INBOX", 1, "work mail"));
        fx.add("work", message("INBOX", 2, "newer work mail"));
        let (mut harness, wires) = fx.harness();
        harness.state_mut().select_view(View::AllInboxes);
        harness.run();
        assert_eq!(ids(&harness), [(1, 2), (0, 1), (1, 1)]);
        (harness, wires)
    }

    #[test]
    fn all_inboxes_sends_each_action_to_the_rows_own_account() {
        let fx = Fixture::new(&["home", "work"]);
        let (mut harness, wires) = all_inboxes(&fx);
        press(&mut harness, "j");
        press(&mut harness, "e");
        assert_eq!(wires.sent(), [command("home", "INBOX", &[1], Action::Archive)]);
        assert_eq!(ids(&harness), [(1, 2), (1, 1)], "work's uid 1 is another message and stays");
        press(&mut harness, "s");
        press(&mut harness, "u");
        assert_eq!(
            wires.sent(),
            [command("work", "INBOX", &[1], Action::Flag), command("work", "INBOX", &[1], Action::MarkUnread)]
        );
    }

    #[test]
    fn marking_in_all_inboxes_marks_only_that_accounts_message() {
        let fx = Fixture::new(&["home", "work"]);
        let (mut harness, wires) = all_inboxes(&fx);
        press(&mut harness, "j");
        press(&mut harness, "x");
        assert_eq!(harness.state().list.marked, HashSet::from([(0, ("INBOX".to_string(), 1))]));
        press(&mut harness, "j");
        press(&mut harness, "x");
        press(&mut harness, "#");
        assert_eq!(
            wires.sent(),
            [command("home", "INBOX", &[1], Action::Trash), command("work", "INBOX", &[1], Action::Trash)]
        );
        assert_eq!(ids(&harness), [(1, 2)]);
    }

    #[test]
    fn the_reader_opens_and_marks_read_in_the_rows_own_account() {
        let fx = Fixture::new(&["home", "work"]);
        let mut unread = message("INBOX", 1, "home mail");
        unread.flags = String::new();
        fx.add("home", unread);
        fx.add("work", message("INBOX", 2, "work mail"));
        let (mut harness, wires) = fx.harness();
        harness.state_mut().select_view(View::AllInboxes);
        harness.run();
        harness.input_mut().time = Some(10.0);
        harness.event(egui::Event::Text("j".into()));
        harness.step();
        assert_eq!(harness.state().body.as_ref().map(|body| body.account), Some(0));
        assert!(harness.query_by_label("Body of 1").is_some());
        harness.input_mut().time = Some(11.1);
        harness.step();
        assert_eq!(wires.sent(), [command("home", "INBOX", &[1], Action::MarkRead)]);
    }

    #[test]
    fn the_move_picker_offers_the_rows_own_account_folders() {
        let fx = Fixture::new(&["home", "work"]);
        fx.folder("home", "Receipts", None);
        fx.folder("work", "Archive", Some("Archive"));
        let (mut harness, wires) = all_inboxes(&fx);
        press(&mut harness, "j");
        press(&mut harness, "m");
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert_eq!(wires.sent(), [command("home", "INBOX", &[1], Action::Move("Receipts".into()))]);
    }

    #[test]
    fn moving_marked_rows_from_two_accounts_is_refused() {
        let fx = Fixture::new(&["home", "work"]);
        let (mut harness, wires) = all_inboxes(&fx);
        for key in ["x", "j", "x", "m"] {
            press(&mut harness, key);
        }
        assert!(harness.state().move_picker.is_none());
        assert!(harness.query_by_label(MOVE_ONE_ACCOUNT).is_some());
        assert!(wires.sent().is_empty());
        assert_eq!(harness.state().list.marked.len(), 2, "the marks stay for the user to narrow");
    }

    #[test]
    fn a_refusal_for_one_account_puts_back_only_its_rows() {
        let fx = Fixture::new(&["home", "work"]);
        let (mut harness, wires) = all_inboxes(&fx);
        for key in ["x", "j", "x", "e"] {
            press(&mut harness, key);
        }
        assert_eq!(wires.sent().len(), 2);
        assert_eq!(ids(&harness), [(1, 1)]);
        wires
            .events
            .send(Event::CommandFailed {
                account: "home".into(),
                request: 0,
                message: "home is offline (x); retrying at 10:00".into(),
            })
            .unwrap();
        harness.run();
        assert_eq!(ids(&harness), [(0, 1), (1, 1)]);
        assert_eq!(harness.state().accounts[1].queued, 1);
    }
```

`ids` comes from Task 2. Ctrl+R in All inboxes needs no new test: `command_r_syncs_every_running_account` already shows it syncs every account in any view.

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::app`
Expected: FAIL to compile (`MOVE_ONE_ACCOUNT` missing). The action, marking, reader and refusal tests need no new code: Task 1 already routes by row, so once the constant exists only the two move tests fail. If one of the other four fails, Task 1 left a `view_account()` in an action path: fix it there.

- [ ] **Step 3: Implement**

`app.rs`, beside `DAEMON_LOST`:

```rust
/// The status line when `m` is pressed with marked rows from more than one account.
pub(crate) const MOVE_ONE_ACCOUNT: &str = "move works on one account at a time; mark messages from one account";
```

In `apply`:

```rust
            UiAction::OpenMovePicker => {
                let accounts: BTreeSet<usize> = self.selected_rows().iter().map(|row| row.account).collect();
                match accounts.len() {
                    0 => {}
                    1 => self.move_picker = Some(String::new()),
                    _ => self.note_error(None, MOVE_ONE_ACCOUNT.into()),
                }
            }
```

`list.rs`, the head of `show_move_picker`:

```rust
/// The `m` popup: type to filter the folders of the selected rows' account, Enter or a click moves. It leaves out the
/// folder the cursor message is in.
pub(crate) fn show_move_picker(app: &App, ctx: &egui::Context) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let (Some(filter), Some(row)) = (&app.move_picker, app.selected_rows().first().copied()) else {
        return actions;
    };
    let folder = app.list.rows.get(app.list.cursor).map_or(&row.folder, |cursor| &cursor.folder);
    let needle = filter.to_lowercase();
    let names: Vec<&str> = app.accounts[row.account]
        .folders
        .iter()
        .map(|f| f.name.as_str())
        .filter(|name| name != folder && name.to_lowercase().contains(&needle))
        .collect();
    // … the window unchanged
```

`View` is no longer used in `show_move_picker`; keep the import only if `show` still uses it (Task 3 does).

- [ ] **Step 4: Run tests**

Run: `cargo test --lib gui:: && cargo test --test architecture`
Expected: PASS, including `m_filters_the_folders_and_enter_moves` and `the_move_picker_in_search_offers_the_inbox_for_an_archived_hit`.

- [ ] **Step 5: Docs and commit**

`docs/src/gui.md`, after the paragraph under the keys table: "In All inboxes each action goes to the message's own account. `m` offers the folders of that account; marked messages from more than one account cannot be moved together."

```bash
git add src/gui/app.rs src/gui/list.rs docs/src/gui.md
git commit -m "feat(gui): act, read and move per row in All inboxes"
```

---

### Task 5: Search every account

**Files:**
- Modify: `src/gui/app.rs` (`UiAction::StartSearch`, `reload_view` search branch, tests)
- Modify: `src/gui/list.rs` (`merge_hits`, search hint in `show`, tests)
- Modify: `src/gui/status.rs` (`KEYS` line for `/`)
- Modify: `docs/src/gui.md` (keys table)
- Modify, if redesign Task 8 landed: `src/gui/toolbar.rs`

**Interfaces:**
- Consumes: Task 2's `reload_view` (which already searches each account the view covers), `View::lists_mail`.
- Produces: `list::merge_hits(hits: Vec<Row>, limit: u32) -> Vec<Row>`.

- [ ] **Step 1: Write the failing tests**

`list.rs` tests:

```rust
    #[test]
    fn merge_hits_is_newest_first_and_capped() {
        let hit = |account, uid| Row::from_message(account, &message("INBOX", uid, "hit"));
        let merged = merge_hits(vec![hit(0, 3), hit(0, 1), hit(1, 4), hit(1, 2)], 3);
        let shape: Vec<(usize, u32)> = merged.iter().map(|r| (r.account, r.uid)).collect();
        assert_eq!(shape, [(1, 4), (0, 3), (1, 2)]);
    }
```

`app.rs` tests:

```rust
    #[test]
    fn slash_in_all_inboxes_searches_every_account_newest_first() {
        let fx = Fixture::new(&["home", "work"]);
        fx.folder("home", "Archive", Some("Archive"));
        fx.add("home", message("INBOX", 1, "invoice march"));
        fx.add("work", message("INBOX", 2, "lunch"));
        fx.add("home", message("Archive", 3, "invoice april"));
        fx.add("work", message("INBOX", 4, "invoice may"));
        let (mut harness, _wires) = fx.harness();
        harness.state_mut().select_view(View::AllInboxes);
        harness.run();
        press(&mut harness, "/");
        assert!(harness.query_by_label("Search every account").is_some());
        press(&mut harness, "invoice");
        let found: Vec<(usize, String, u32)> =
            harness.state().list.rows.iter().map(|r| (r.account, r.folder.clone(), r.uid)).collect();
        assert_eq!(
            found,
            [(1, "INBOX".to_string(), 4), (0, "Archive".to_string(), 3), (0, "INBOX".to_string(), 1)]
        );
        assert!(harness.query_by_label_contains("home · [Archive] Sender 3").is_some());
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert_eq!(harness.state().view, View::AllInboxes);
        assert_eq!(ids(&harness), [(1, 4), (1, 2), (0, 1)]);
    }
```

`query_by_label("Search every account")` finds the empty search field by its hint text; if kittest does not expose the hint as the field's label, assert `harness.state().search == Some(String::new())` instead (not verified).

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::`
Expected: FAIL (`merge_hits` missing; `/` does nothing in All inboxes).

- [ ] **Step 3: Implement**

`list.rs`:

```rust
/// Search hits of several accounts, newest first, at most `limit`; each account's own order is kept on equal dates.
pub(crate) fn merge_hits(mut hits: Vec<Row>, limit: u32) -> Vec<Row> {
    hits.sort_by_key(|row| std::cmp::Reverse(row.date));
    hits.truncate(limit as usize);
    hits
}
```

In `show`, the search field's hint:

```rust
                    .hint_text(if app.view == View::AllInboxes { "Search every account" } else { "Search this account" })
```

`app.rs`: in `reload_view`'s search branch, `self.list.hits = list::merge_hits(hits, list::SEARCH_LIMIT);`. `UiAction::StartSearch` checks `if self.view.lists_mail() {` instead of `self.view_account().is_some()`.

`status.rs` `KEYS`: `("/", "search this account; in All inboxes, every account"),`.

If redesign Task 8 landed: the toolbar search button is enabled on `app.view.lists_mail()`.

- [ ] **Step 4: Run tests**

Run: `cargo test --lib gui:: && cargo test --test architecture`
Expected: PASS, including `slash_searches_the_account_and_escape_returns_to_the_folder`.

- [ ] **Step 5: Docs, snapshots, commit**

`docs/src/gui.md` keys table: `/` reads "search this account; in All inboxes, every account". The `keys_help` snapshot changes with the `KEYS` line; regenerate on Linux and look at it.

```bash
git add src/gui/ docs/src/gui.md tests/snapshots/
git commit -m "feat(gui): search every account from All inboxes"
```

---

## Finish

- [ ] Full gate: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && cargo machete && cargo audit`
- [ ] Snapshots regenerated on Linux with lavapipe after Tasks 2, 3 and 5, and each changed PNG looked at.
- [ ] Report which platforms the PR was compiled on and ran on.
