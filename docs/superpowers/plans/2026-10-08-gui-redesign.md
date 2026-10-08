# GUI Redesign (Phase 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the egui window in line with the approved redesign: an icon toolbar, a sidebar with icons, counts, a folder tree and pinned views, a column-aligned message list, a fuller reader date, and light-theme colours that pass WCAG AA.

**Architecture:** Pure helpers (folder tree, list columns, number grouping, date formats) live beside the views that use them and carry the unit tests; the views stay thin and only return `UiAction`s; `app.rs` remains the only code that changes state. One new state field (`collapsed` folders) and two new `UiAction`s (`ToggleFolder`, none other). Folder hierarchy needs the IMAP delimiter, which is plumbed from `list_folders` through `RemoteFolder` into a new `folders.delimiter` column.

**Tech Stack:** Rust 2024, eframe/egui 0.36, egui_kittest, chrono, rusqlite + rusqlite_migration, `egui-phosphor` 0.14 (new), `sys-locale` 0.3 (new).

**Spec:** the design canvas https://claude.ai/artifact/B5W17bKLqpKUw9z3bCw65Q (boards "Inbox · Mocha", "Rules · Latte", "Regex tester · Latte"; private to the repo owner), read with `docs/superpowers/specs/2026-10-06-postbode-core-design.md` and `AGENTS.md`.

## Scope

In this plan (buildable now, no new product features):

| # | PR | Canvas comment it answers |
|---|---|---|
| 1 | Latte contrast | "contrast is weird", "contrast" |
| 2 | Icon font | folder, toolbar and view icons |
| 3 | Sidebar polish | full-width selection, counts, icons, Junk, separator, views pinned, "Backups" |
| 4 | Folder delimiter in the store | prerequisite for 5 |
| 5 | Folder tree | nested and custom folders |
| 6 | Message list columns and dates | aligned rows, date right, readable dates |
| 7 | Reader date | full date with seconds and offset |
| 8 | Toolbar, sync button, version | toolbar, sync in the status bar, version label |
| 9 | Regional number grouping | "format the numbers using the region settings" |

Each task is one PR off `origin/main`. Tasks 4 → 5 are ordered; 2 must land before 3, 5, 6 and 8; the rest are independent.

Not in this plan, each needs its own spec or plan first:

- **All inboxes** (unified inbox): rows need an account per row and actions must route per row; a separate plan.
- **Rules view redesign, rule editor, regex tester, "Rule" buttons on From and Subject, proposal TOML highlighting**: needs new `rules/edit.rs` functions to add or replace a rule; a separate plan.
- **New, Reply, Reply all, Forward**: compose and SMTP are phase 4 in the core spec.
- **Text / HTML switch**: HTML rendering landed in #65 (Blitz, `v` toggles); the switch is a task in `2026-10-08-reader-save-and-source.md`.
- **"vX available" update check**: a new network call; needs a decision on opt-out and privacy.
- **Save .eml and View source buttons, bold unread rows**: small, but not asked for in this round; bold also needs a second font file.

## Global Constraints

- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `cargo machete` and `cargo audit` all pass before each PR.
- No `unwrap`, `expect`, `panic!`, `todo!` or `unimplemented!` outside tests; where a call cannot fail, a narrow `#[allow]` says why.
- Every new dependency has a one-line reason in `Cargo.toml`.
- GUI views draw and return `UiAction`s; only `app.rs` changes state (`tests/architecture.rs` checks this).
- Colours come from `theme::Palette`; Mocha stays exact Catppuccin; no blue or purple accents.
- One grid everywhere: a 4 px unit; rows 24 px, buttons and fields 28 px, top bar 40 px, status bar 48 px; spacing in steps of 4 and 8; content vertically centred in its row; every pane puts its content 16 px in from its edge, and the toolbar's first cell is as wide as the folder pane so toolbar items line up with the panes. Define these once as constants in `theme.rs` (for example `ROW`, `CONTROL`, `GAP`) and use them instead of literal sizes.
- Keys in hints are spelled out (`Ctrl+R`, `Cmd+R`): the default fonts lack the ⌘ glyph.
- After GUI changes, regenerate snapshots on Linux with lavapipe: `TZ=UTC UPDATE_SNAPSHOTS=1 cargo test gui::snapshots`, and look at every changed PNG in `tests/snapshots/`.
- Prose changes go in `docs/src/gui.md`.
- Branch per task: `git switch -c <name> --no-track origin/main`, first push `git push -u origin HEAD`. Conventional commits.
- Report platform coverage honestly: "compiled on" vs "ran on".

## Review Focus

1. **A sibling that sorts between a parent and its child** ("Work", "Work Archive", "Work/Clients"): the child must still appear under "Work". Test in Task 5.
2. **Servers that put every folder under `INBOX.`** (Dovecot and Courier namespaces): "INBOX.Clients" must show as top-level "Clients", not as a child of INBOX. Test in Task 5.
3. **An INBOX-only sync, which lists no delimiter, must not erase the stored delimiter** and flatten the tree. Test in Task 4.
4. **A narrow list pane or a very long subject**: the date stays fully visible and the subject truncates with "…". Test in Task 6.
5. **`LANG=C`, `POSIX`, an empty locale or an unknown language**: counts show without grouping, never a panic; `nl-NL`, `nl_NL.UTF-8` and `en-US` all parse. Test in Task 9.

---

### Task 1: Latte contrast

**Files:**
- Modify: `src/gui/theme.rs` (`LATTE`, tests)

**Interfaces:**
- Produces: `LATTE.accent = #10757a`, `LATTE.on_accent = #ffffff`, `LATTE.pressed = #bcc0cc`, `LATTE.success = #276b19`. Mocha unchanged.

- [ ] **Step 1: Write the failing test** in `theme.rs` tests

```rust
    #[test]
    fn accent_fills_links_and_success_text_pass_aa_in_both_themes() {
        for (name, p) in [("mocha", &MOCHA), ("latte", &LATTE)] {
            let fill = contrast(p.on_accent, p.accent);
            assert!(fill >= 4.5, "{name}: text on accent {fill}");
            let link = contrast(p.accent, p.background);
            assert!(link >= 4.5, "{name}: accent text {link}");
            let success = contrast(p.success, p.panel);
            assert!(success >= 4.5, "{name}: success text {success}");
        }
    }
```

- [ ] **Step 2: Run it**

Run: `cargo test --lib gui::theme`
Expected: FAIL, `latte: text on accent 5.0…` passes but `latte: accent text 3.…` fails (#179299 on #eff1f5).

- [ ] **Step 3: Change `LATTE`**

```rust
/// Catppuccin Latte, except the teal and green: Latte's own (#179299, #40a02b) fail WCAG AA as text and under white
/// text, so both are darkened.
pub(crate) const LATTE: Palette = Palette {
    accent: hex(0x10757a),
    background: hex(0xeff1f5),
    border: hex(0xbcc0cc),
    error: hex(0xd20f39),
    faint: hex(0xdce0e8),
    highlight: hex(0xfe640b),
    hover: hex(0xccd0da),
    muted: hex(0x8c8fa1),
    on_accent: hex(0xffffff),
    panel: hex(0xe6e9ef),
    pressed: hex(0xbcc0cc),
    secondary: hex(0x6c6f85),
    strong: hex(0x11111b),
    success: hex(0x276b19),
    text: hex(0x4c4f69),
    warning: hex(0xdf8e1d),
};
```

`pressed` moves off the accent because `strong` (near black) is drawn on it and only reaches 3.8:1 on #10757a.

- [ ] **Step 4: Update the two tests that pinned the old values**

In `the_palettes_are_catppuccin`, replace the two Latte lines:

```rust
        assert_eq!(LATTE.accent, Color32::from_rgb(0x10, 0x75, 0x7a));
        assert_eq!(LATTE.on_accent, Color32::from_rgb(0xff, 0xff, 0xff));
```

Replace `light_strong_text_and_pressed_widgets_keep_their_colours` with:

```rust
    #[test]
    fn light_strong_text_and_pressed_widgets_keep_their_colours() {
        let light = LATTE.visuals(false);
        assert_eq!(light.strong_text_color(), LATTE.strong);
        assert_eq!(light.widgets.active.bg_fill, LATTE.pressed);
    }
```

- [ ] **Step 5: Run the theme tests and the whole GUI suite**

Run: `cargo test --lib gui::`
Expected: PASS (`strong_text_stands_out_from_normal_text_in_both_themes` still passes: #11111b on #bcc0cc is 10.3:1).

- [ ] **Step 6: Snapshots on Linux**, then commit

```bash
git add src/gui/theme.rs tests/snapshots/
git commit -m "fix(gui): darken the light theme's teal and green to pass WCAG AA"
```

---

### Task 2: Icon font

**Files:**
- Modify: `Cargo.toml` (dependency, `gui` feature)
- Create: `src/gui/icons.rs`
- Modify: `src/gui/mod.rs` (`mod icons;`), `src/gui/theme.rs` (`install` sets fonts)

**Interfaces:**
- Produces: `gui::icons::{ACTIVITY, ARCHIVE, BACKUPS, CARET_DOWN, CARET_RIGHT, CHECK, DRAFTS, FLAG, FOLDER, INBOX, JUNK, MARK_UNREAD, MOVE, RULES, SEARCH, SENT, SYNC, TRASH}: &str`, `icons::for_special_use(special_use: Option<&str>, name: &str) -> &'static str`, `theme::fonts() -> egui::FontDefinitions`.

- [ ] **Step 1: Add the dependency**

In `Cargo.toml` `[dependencies]`:

```toml
egui-phosphor = { version = "0.14", optional = true }  # icon font for the GUI (folders, toolbar, views); targets egui 0.36
```

and `gui = ["dep:eframe", "dep:egui-phosphor"]`.

- [ ] **Step 2: Write the failing tests** in a new `src/gui/icons.rs`

```rust
//! The Phosphor icons the window uses, by meaning rather than by glyph name.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_folders_get_their_icon_and_others_a_folder() {
        assert_eq!(for_special_use(None, "INBOX"), INBOX);
        assert_eq!(for_special_use(None, "inbox"), INBOX);
        assert_eq!(for_special_use(Some("Archive"), "All Mail"), ARCHIVE);
        assert_eq!(for_special_use(Some("Drafts"), "Drafts"), DRAFTS);
        assert_eq!(for_special_use(Some("Junk"), "Spam"), JUNK);
        assert_eq!(for_special_use(Some("Sent"), "Sent Items"), SENT);
        assert_eq!(for_special_use(Some("Trash"), "Deleted"), TRASH);
        assert_eq!(for_special_use(None, "Receipts"), FOLDER);
    }
}
```

and in `theme.rs` tests:

```rust
    #[test]
    fn the_icon_font_falls_back_behind_the_text_font() {
        let fonts = fonts();
        assert!(fonts.font_data.contains_key("phosphor"));
        let proportional = &fonts.families[&egui::FontFamily::Proportional];
        assert_eq!(proportional.get(1).map(String::as_str), Some("phosphor"));
    }
```

- [ ] **Step 3: Run them**

Run: `cargo test --lib gui::icons gui::theme`
Expected: FAIL to compile (`for_special_use`, `fonts` not found).

- [ ] **Step 4: Implement**

`src/gui/icons.rs`, above the tests:

```rust
use egui_phosphor::regular as ph;

pub(crate) const ACTIVITY: &str = ph::PULSE;
pub(crate) const ARCHIVE: &str = ph::ARCHIVE;
pub(crate) const BACKUPS: &str = ph::CLOCK_COUNTER_CLOCKWISE;
pub(crate) const CARET_DOWN: &str = ph::CARET_DOWN;
pub(crate) const CARET_RIGHT: &str = ph::CARET_RIGHT;
pub(crate) const CHECK: &str = ph::CHECK;
pub(crate) const DRAFTS: &str = ph::FILE_DASHED;
pub(crate) const FLAG: &str = ph::FLAG;
pub(crate) const FOLDER: &str = ph::FOLDER;
pub(crate) const INBOX: &str = ph::TRAY;
pub(crate) const JUNK: &str = ph::WARNING_OCTAGON;
pub(crate) const MARK_UNREAD: &str = ph::ENVELOPE_SIMPLE;
pub(crate) const MOVE: &str = ph::FOLDER_SIMPLE_DASHED;
pub(crate) const RULES: &str = ph::FUNNEL;
pub(crate) const SEARCH: &str = ph::MAGNIFYING_GLASS;
pub(crate) const SENT: &str = ph::PAPER_PLANE_TILT;
pub(crate) const SYNC: &str = ph::ARROWS_CLOCKWISE;
pub(crate) const TRASH: &str = ph::TRASH;

/// The icon for a folder: its special use, INBOX by name, else a plain folder.
pub(crate) fn for_special_use(special_use: Option<&str>, name: &str) -> &'static str {
    if name.eq_ignore_ascii_case("INBOX") {
        return INBOX;
    }
    match special_use {
        Some("Archive") => ARCHIVE,
        Some("Drafts") => DRAFTS,
        Some("Junk") => JUNK,
        Some("Sent") => SENT,
        Some("Trash") => TRASH,
        _ => FOLDER,
    }
}
```

If `cargo check` reports `CHECK` or `FOLDER_SIMPLE_DASHED` missing, pick the nearest name from `egui_phosphor::variants::codepoints` (the others were verified against 0.14.0).

`theme.rs`:

```rust
/// egui's fonts with the Phosphor icons as the first fallback, so icons mix into ordinary labels.
pub(crate) fn fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    fonts
}

pub(crate) fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    ctx.set_visuals_of(egui::Theme::Dark, MOCHA.visuals(true));
    ctx.set_visuals_of(egui::Theme::Light, LATTE.visuals(false));
}
```

`src/gui/mod.rs`: add `mod icons;` beside the other modules.

- [ ] **Step 5: Run tests and `cargo machete`**

Run: `cargo test --lib gui:: && cargo machete`
Expected: PASS; machete reports nothing (the crate is used in `icons.rs` and `theme.rs`).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/gui/icons.rs src/gui/mod.rs src/gui/theme.rs
git commit -m "feat(gui): add the Phosphor icon font"
```

---

### Task 3: Sidebar polish

**Files:**
- Modify: `src/gui/folders.rs`, `docs/src/gui.md`
- Test: `src/gui/folders.rs` tests

**Interfaces:**
- Consumes: `icons::for_special_use`, `icons::{RULES, ACTIVITY, BACKUPS}` (Task 2).
- Produces: `folders::is_special(name: &str, special_use: Option<&str>) -> bool` (rank below 6; `rank` takes the same two arguments so it serves both `store::Folder` in `sort` and `app::FolderRow` in drawing), `folders::folder_row(ui, selected: bool, icon: &str, label: &str, indent: f32, count: Option<String>, accessible: &str) -> egui::Response`. Task 5 calls both.

Behaviour: each row is full width; icon, name, and the unread count right-aligned in `muted` (in `on_accent` when selected); a thin separator between special and custom folders when both exist; Rules, Activity and Backups (the `View::Trash` label) pinned to the bottom of the pane with icons. The accessible name stays `"INBOX (1)"` so screen readers and tests read the count.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn rows_are_full_width_and_keep_the_count_in_their_name() {
        let fx = Fixture::new(&["work"]);
        let mut unread = message("INBOX", 1, "hello");
        unread.flags = String::new();
        fx.add("work", unread);
        let (harness, _wires) = fx.harness();
        let inbox = harness.get_by_label("INBOX (1)");
        let pane = harness.get_by_label("Rules").rect();
        assert!(inbox.rect().width() >= pane.width() - 1.0);
    }

    #[test]
    fn a_separator_divides_special_from_custom_folders() {
        let fx = Fixture::new(&["work"]);
        fx.folder("work", "Junk", Some("Junk"));
        fx.folder("work", "Receipts", None);
        let (harness, _wires) = fx.harness();
        let junk = harness.get_by_label("Junk").rect();
        let receipts = harness.get_by_label("Receipts").rect();
        assert!(receipts.top() - junk.bottom() > harness.ctx.style().spacing.item_spacing.y + 2.0);
    }

    #[test]
    fn the_backup_view_is_labelled_backups_and_sits_at_the_bottom() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        let backups = harness.get_by_label("Backups").rect();
        assert!(backups.bottom() > 700.0, "pinned to the bottom of an 800 px window");
        harness.get_by_label("Backups").click();
        harness.run();
        assert_eq!(harness.state().view, View::Trash);
    }
```

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::folders`
Expected: FAIL (rows are text-wide; no separator; label is "Trash" and near the top).

- [ ] **Step 3: Implement** in `folders.rs`

```rust
pub(crate) fn is_special(name: &str, special_use: Option<&str>) -> bool {
    rank(name, special_use) < 6
}

// `rank` becomes `fn rank(name: &str, special_use: Option<&str>) -> usize`, and `sort` calls
// `rank(&f.name, f.special_use.as_deref())`.

/// A full-width selectable row: indent, icon, label, and the count right-aligned. `accessible` names it for screen
/// readers and tests, since the count is painted apart from the label.
pub(crate) fn folder_row(
    ui: &mut egui::Ui,
    selected: bool,
    icon: &str,
    label: &str,
    indent: f32,
    count: Option<String>,
    accessible: &str,
) -> egui::Response {
    let text = format!("{icon}  {label}");
    let response = ui.add(
        egui::Button::selectable(selected, text)
            .truncate()
            .min_size(egui::vec2(ui.available_width() - indent, 0.0)),
    );
    if let Some(count) = count {
        let palette = super::theme::palette(ui);
        let color = if selected { palette.on_accent } else { palette.muted };
        ui.painter().text(
            response.rect.right_center() - egui::vec2(6.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            count,
            egui::TextStyle::Button.resolve(ui.style()),
            color,
        );
    }
    let name = accessible.to_string();
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &name));
    response
}
```

Indentation is applied by the caller with `ui.add_space(indent)` inside `ui.horizontal`. In `show`, draw the views first in a bottom panel so they stay pinned, then the scrolling tree:

```rust
pub(crate) fn show(app: &App, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    egui::Panel::bottom("views").show(ui, |ui| {
        for (view, icon, label) in [
            (View::Rules, icons::RULES, "Rules"),
            (View::Activity, icons::ACTIVITY, "Activity"),
            (View::Trash, icons::BACKUPS, "Backups"),
        ] {
            if folder_row(ui, app.view == view, icon, label, 0.0, None, label).clicked() {
                actions.push(UiAction::SelectView(view));
            }
        }
    });
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (index, account) in app.accounts.iter().enumerate() {
            // account heading and store error unchanged
            let mut previous_special = false;
            for folder in &account.folders {
                let special = is_special(&folder.name, folder.special_use.as_deref());
                if previous_special && !special {
                    ui.separator();
                }
                previous_special = special;
                let view = View::Folder { account: index, folder: folder.name.clone() };
                let name = clean(&folder.name, false);
                let (count, accessible) = match folder.unread {
                    0 => (None, name.clone()),
                    unread => (Some(unread.to_string()), format!("{name} ({unread})")),
                };
                let icon = icons::for_special_use(folder.special_use.as_deref(), &folder.name);
                if folder_row(ui, app.view == view, icon, &name, 0.0, count, &accessible).clicked() {
                    actions.push(UiAction::SelectView(view));
                }
            }
            ui.add_space(8.0);
        }
    });
    actions
}
```

`egui::Panel::bottom(...).show(ui, …)` is the same panel API `App::show` uses. Update the module doc to "then the Rules, Activity and Backups entries, pinned to the bottom".

- [ ] **Step 4: Run the folder tests and the existing ones**

Run: `cargo test --lib gui::`
Expected: PASS, including `tree_shows_folders_with_unread_counts_and_selects_on_click` (it reads "INBOX (1)" and clicks "Rules").

- [ ] **Step 5: Docs** — in `docs/src/gui.md` rename the heading to "Rules, Activity and Backups", say **Backups** lists the `.eml` backups (the view formerly called Trash), and note the views sit at the bottom of the folder pane.

- [ ] **Step 6: Snapshots on Linux**, then commit

```bash
git add src/gui/folders.rs docs/src/gui.md tests/snapshots/
git commit -m "feat(gui): full-width folder rows with icons and counts, views pinned below"
```

---

### Task 4: Folder delimiter in the store

**Files:**
- Modify: `src/mail_ops.rs` (`RemoteFolder`, `RecordingOps`), `src/mail_ops/imap.rs` (`list_folders`), `src/store.rs` (`Folder`, `folders`, `folder`, `upsert_folder`), `src/sync.rs` (folder row, `sync_inbox_with`), `src/gui/app.rs` (`FolderRow.delimiter`, filled where `FolderRow` is built from the store, around line 120)
- Create: `migrations/005-folder-delimiter/up.sql`
- Modify: every `Folder { … }` and `RemoteFolder { … }` literal (about 37 and 10; the compiler lists them)

**Interfaces:**
- Produces: `RemoteFolder.delimiter: Option<String>`, `Folder.delimiter: Option<String>` (`#[serde(default)]`), `app::FolderRow.delimiter: Option<String>`. `upsert_folder` keeps a stored delimiter when the new one is `None`.

- [ ] **Step 1: Write the failing store tests** in `store.rs` tests

```rust
    #[test]
    fn a_folder_keeps_its_delimiter_through_an_upsert_without_one() {
        let store = Store::open_in_memory().unwrap();
        let mut folder = Folder {
            name: "Projects/Postbode".into(),
            uidvalidity: 1,
            last_uid: 0,
            special_use: None,
            delimiter: Some("/".into()),
        };
        store.upsert_folder(&folder).unwrap();
        folder.delimiter = None;
        folder.last_uid = 5;
        store.upsert_folder(&folder).unwrap();
        let stored = store.folder("Projects/Postbode").unwrap().unwrap();
        assert_eq!(stored.delimiter.as_deref(), Some("/"));
        assert_eq!(stored.last_uid, 5);
        assert_eq!(store.folders().unwrap()[0].delimiter.as_deref(), Some("/"));
    }
```

If `Store::open_in_memory` does not exist, use the constructor the neighbouring store tests use.

- [ ] **Step 2: Run it**

Run: `cargo test --lib store::tests::a_folder_keeps_its_delimiter`
Expected: FAIL to compile (`delimiter` unknown).

- [ ] **Step 3: Implement**

`migrations/005-folder-delimiter/up.sql`:

```sql
-- The IMAP hierarchy delimiter, so the window can show folders as a tree. NULL until the next full sync lists it.
ALTER TABLE folders ADD COLUMN delimiter TEXT;
```

`store.rs`: add `#[serde(default)] pub delimiter: Option<String>,` to `Folder`; add `delimiter` to both `SELECT`s; and

```rust
    pub fn upsert_folder(&self, folder: &Folder) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO folders (name, uidvalidity, last_uid, special_use, delimiter) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(name) DO UPDATE SET uidvalidity = excluded.uidvalidity, last_uid = excluded.last_uid,
             special_use = excluded.special_use, delimiter = COALESCE(excluded.delimiter, folders.delimiter)",
            params![folder.name, folder.uidvalidity, folder.last_uid, folder.special_use, folder.delimiter],
        )?;
        Ok(())
    }
```

`mail_ops.rs`: add `pub delimiter: Option<String>,` to `RemoteFolder`. `imap.rs` in `list_folders`:

```rust
                .map(|n| RemoteFolder {
                    name: n.name().to_string(),
                    special_use: special_use(n.attributes()),
                    delimiter: n.delimiter().map(str::to_string),
                })
```

`sync.rs`: the folder `row` closure gets `delimiter: folder.delimiter.clone(),`; `sync_inbox_with`'s `RemoteFolder` gets `delimiter: None` (the COALESCE keeps the stored one).

Then fix every other literal:

Run: `cargo check --all-targets --all-features 2>&1 | ggrep -B2 -A6 'missing field .delimiter.'`
and add `delimiter: None,` to each (`RecordingOps` in `mail_ops.rs` may use `Some("/".into())` where a test needs nesting).

- [ ] **Step 4: Run all tests**

Run: `cargo test --all-features && cargo test --lib store::tests::migrations_are_valid`
Expected: PASS.

- [ ] **Step 5: Live check (optional, if Docker is up)**

Run: `POSTBODE_TEST_IMAP_HOST=localhost cargo test --features testing --test imap_live`
Expected: PASS. Dovecot lists a delimiter, so a stored folder now has one.

- [ ] **Step 6: Commit**

```bash
git add migrations/005-folder-delimiter src/
git commit -m "feat(store): keep each folder's IMAP hierarchy delimiter"
```

---

### Task 5: Folder tree

**Files:**
- Modify: `src/gui/folders.rs` (tree builder, drawing), `src/gui/app.rs` (`collapsed`, `UiAction::ToggleFolder`), `docs/src/gui.md`

**Interfaces:**
- Consumes: `FolderRow.delimiter` (Task 4), `folder_row`, `is_special` (Task 3), `icons::{FOLDER, CARET_DOWN, CARET_RIGHT}` (Task 2).
- Produces:

```rust
#[derive(Debug, PartialEq)]
pub(crate) struct TreeRow {
    /// The folder a click opens; None for a parent the server does not list (not selectable).
    pub folder: Option<String>,
    pub label: String,
    pub depth: usize,
    /// Segments joined by '/', the key for collapse state.
    pub path: String,
    pub has_children: bool,
}
pub(crate) fn tree(folders: &[FolderRow]) -> Vec<TreeRow>;
pub(crate) fn visible<'a>(rows: &'a [TreeRow], collapsed: impl Fn(&str) -> bool) -> Vec<&'a TreeRow>;
```

`App` gains `pub collapsed: HashSet<(usize, String)>` and `UiAction::ToggleFolder(usize, String)`. Collapse state lasts for the window's lifetime; persisting it is not in scope.

- [ ] **Step 1: Write the failing tests**

```rust
    fn custom(name: &str, delimiter: &str) -> FolderRow {
        FolderRow {
            name: name.into(),
            special_use: None,
            unread: 0,
            delimiter: Some(delimiter.into()),
        }
    }

    fn stored(name: &str) -> Folder {
        Folder {
            name: name.into(),
            uidvalidity: 1,
            last_uid: 0,
            special_use: None,
            delimiter: Some("/".into()),
        }
    }

    fn shape(rows: &[TreeRow]) -> Vec<(usize, &str, bool)> {
        rows.iter().map(|r| (r.depth, r.label.as_str(), r.folder.is_some())).collect()
    }

    #[test]
    fn tree_nests_by_delimiter_and_adds_unlisted_parents() {
        let rows = tree(&[custom("Projects/Postbode/GitHub", "/"), custom("Projects/Website", "/")]);
        assert_eq!(
            shape(&rows),
            [(0, "Projects", false), (1, "Postbode", false), (2, "GitHub", true), (1, "Website", true)]
        );
        assert!(rows[0].has_children && rows[1].has_children && !rows[2].has_children);
    }

    #[test]
    fn a_sibling_sorting_between_parent_and_child_does_not_steal_the_child() {
        let rows = tree(&[custom("Work", "/"), custom("Work Archive", "/"), custom("Work/Clients", "/")]);
        assert_eq!(
            shape(&rows),
            [(0, "Work", true), (1, "Clients", true), (0, "Work Archive", true)]
        );
    }

    #[test]
    fn an_inbox_namespace_prefix_is_not_a_parent() {
        let rows = tree(&[custom("INBOX.Clients", "."), custom("INBOX.Clients.Acme", ".")]);
        assert_eq!(shape(&rows), [(0, "Clients", true), (1, "Acme", true)]);
    }

    #[test]
    fn a_folder_without_a_delimiter_stays_flat() {
        let mut flat = custom("a/b", "/");
        flat.delimiter = None;
        assert_eq!(shape(&tree(&[flat])), [(0, "a/b", true)]);
    }

    #[test]
    fn collapsing_hides_descendants_only() {
        let rows = tree(&[custom("A/B/C", "/"), custom("A/D", "/"), custom("E", "/")]);
        let shown: Vec<&str> = visible(&rows, |path| path == "A/B").iter().map(|r| r.label.as_str()).collect();
        assert_eq!(shown, ["A", "B", "D", "E"]);
    }

    #[test]
    fn clicking_a_caret_collapses_its_folder() {
        let fx = Fixture::new(&["work"]);
        let store = fx.store("work");
        for name in ["Projects/Postbode", "Projects/Website"] {
            store.upsert_folder(&stored(name)).unwrap();
        }
        let (mut harness, _wires) = fx.harness();
        assert!(harness.query_by_label("Website").is_some());
        harness.get_by_label("Collapse Projects").click();
        harness.run();
        assert!(harness.query_by_label("Website").is_none());
        assert!(harness.state().collapsed.contains(&(0, "Projects".to_string())));
    }
```

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::folders`
Expected: FAIL to compile (`tree`, `visible`, `TreeRow`, `collapsed` missing).

- [ ] **Step 3: Implement the pure part** in `folders.rs`

```rust
/// The custom folders as a tree. Sorted by path segments, not by full name, so "Work Archive" cannot land between
/// "Work" and "Work/Clients". An `INBOX` + delimiter prefix (Dovecot and Courier namespaces) is dropped.
pub(crate) fn tree(folders: &[FolderRow]) -> Vec<TreeRow> {
    let mut paths: Vec<(Vec<String>, &FolderRow)> = folders
        .iter()
        .filter(|f| !is_special(&f.name, f.special_use.as_deref()))
        .map(|f| (segments(f), f))
        .collect();
    paths.sort_by_key(|(segments, _)| segments.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>());
    let mut rows: Vec<TreeRow> = Vec::new();
    for (segments, folder) in paths {
        for depth in 0..segments.len() {
            let path = segments[..=depth].join("/");
            let leaf = depth + 1 == segments.len();
            match rows.iter_mut().find(|row| row.path == path) {
                Some(row) if leaf => row.folder = Some(folder.name.clone()),
                Some(_) => {}
                None => rows.push(TreeRow {
                    folder: leaf.then(|| folder.name.clone()),
                    label: segments[depth].clone(),
                    depth,
                    path,
                    has_children: false,
                }),
            }
        }
    }
    for index in 1..rows.len() {
        if rows[index].depth > rows[index - 1].depth {
            rows[index - 1].has_children = true;
        }
    }
    rows
}

fn segments(folder: &FolderRow) -> Vec<String> {
    let Some(delimiter) = folder.delimiter.as_deref().filter(|d| !d.is_empty()) else {
        return vec![folder.name.clone()];
    };
    let prefix = format!("INBOX{delimiter}");
    let name = match folder.name.get(..prefix.len()) {
        Some(head) if head.eq_ignore_ascii_case(&prefix) => &folder.name[prefix.len()..],
        _ => folder.name.as_str(),
    };
    name.split(delimiter).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// The rows not under a collapsed ancestor.
pub(crate) fn visible<'a>(rows: &'a [TreeRow], collapsed: impl Fn(&str) -> bool) -> Vec<&'a TreeRow> {
    let mut hidden_below: Option<usize> = None;
    let mut shown = Vec::new();
    for row in rows {
        if hidden_below.is_some_and(|depth| row.depth > depth) {
            continue;
        }
        hidden_below = collapsed(&row.path).then_some(row.depth);
        shown.push(row);
    }
    shown
}
```

`rows.iter_mut().find` is linear per segment; folder counts are in the hundreds at most. Mark it: `// ponytail: linear parent lookup; a HashMap from path to index if accounts with thousands of folders show up`.

- [ ] **Step 4: Draw it and wire the action**

In `show`, after the special folders and the separator from Task 3:

```rust
            let rows = tree(&account.folders);
            for row in visible(&rows, |path| app.collapsed.contains(&(index, path.to_string()))) {
                let indent = 16.0 * row.depth as f32;
                ui.horizontal(|ui| {
                    ui.add_space(indent);
                    let selected = row.folder.as_ref().is_some_and(|name| {
                        app.view == View::Folder { account: index, folder: name.clone() }
                    });
                    let unread = row.folder.as_ref().and_then(|name| account.folders.iter().find(|f| &f.name == name)).map_or(0, |f| f.unread);
                    let label = clean(&row.label, false);
                    let (count, accessible) = match unread {
                        0 => (None, label.clone()),
                        n => (Some(n.to_string()), format!("{label} ({n})")),
                    };
                    let response = folder_row(ui, selected, icons::FOLDER, &label, indent, count, &accessible);
                    if response.clicked() && let Some(name) = &row.folder {
                        actions.push(UiAction::SelectView(View::Folder { account: index, folder: name.clone() }));
                    }
                    if row.has_children {
                        let open = !app.collapsed.contains(&(index, row.path.clone()));
                        let caret = if open { icons::CARET_DOWN } else { icons::CARET_RIGHT };
                        let verb = if open { "Collapse" } else { "Expand" };
                        let rect = egui::Rect::from_center_size(
                            response.rect.right_center() - egui::vec2(28.0, 0.0),
                            egui::vec2(16.0, response.rect.height()),
                        );
                        let caret_response = ui.interact(rect, ui.id().with(("caret", index, &row.path)), egui::Sense::click());
                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, caret,
                            egui::TextStyle::Button.resolve(ui.style()), theme::palette(ui).muted);
                        let name = format!("{verb} {label}");
                        caret_response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
                        if caret_response.clicked() {
                            actions.push(UiAction::ToggleFolder(index, row.path.clone()));
                        }
                    }
                });
            }
```

The custom folders must leave the flat loop: that loop now iterates only the special folders, `account.folders.iter().filter(|f| is_special(&f.name, f.special_use.as_deref()))`.

`app.rs`: add `ToggleFolder(usize, String),` to `UiAction` (alphabetical, after `ToggleFlag`), the field `pub collapsed: HashSet<(usize, String)>` (initialised empty in `App::new`), and in `apply`:

```rust
            UiAction::ToggleFolder(account, path) => {
                if !self.collapsed.remove(&(account, path.clone())) {
                    self.collapsed.insert((account, path));
                }
            }
```

A collapsed parent that holds the open folder stays collapsed; the list keeps showing that folder.

- [ ] **Step 5: Run tests**

Run: `cargo test --lib gui:: && cargo test --test architecture`
Expected: PASS.

- [ ] **Step 6: Docs, snapshots, commit**

Add to `docs/src/gui.md`: custom folders show as a tree by the server's hierarchy; the caret folds a branch; a branch the server does not list as a folder is shown but cannot be opened. Add one nested folder to the snapshot fixture in `snapshots.rs` so the tree is in the reference image, regenerate on Linux.

```bash
git add src/gui/ docs/src/gui.md tests/snapshots/
git commit -m "feat(gui): show custom folders as a collapsible tree"
```

---

### Task 6: Message list columns and dates

**Files:**
- Modify: `src/gui/list.rs` (`list_date`, new `Columns`, `column_widths`, row drawing; `row_job` removed), `docs/src/gui.md`

**Interfaces:**
- Consumes: `icons::{FLAG, CHECK}` (Task 2).
- Produces:

```rust
pub(crate) struct Columns { pub unread: bool, pub flagged: bool, pub marked: bool, pub who: String, pub subject: String, pub date: String, pub member: bool }
pub(crate) fn columns<Tz: TimeZone>(row: &Row, marked: bool, recipient: bool, in_search: bool, now: &DateTime<Tz>) -> Columns where Tz::Offset: Display;
/// (sender width, subject width) for a row of `total` px whose date needs `date` px.
pub(crate) fn column_widths(total: f32, date: f32) -> (f32, f32);
```

`row_text` stays: it is each row's accessible name, which the tests and screen readers read.

Layout per row, left to right: 10 px unread dot (accent circle), 14 px flag or check, 130 px sender (12 px more indent for thread members), the subject filling the rest and truncating with "…", 12 px gap, the date right-aligned in `muted`. Selected rows paint the accent fill and every cell in `on_accent`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn list_date_is_time_today_weekday_and_time_this_week_day_month_this_year_else_with_year() {
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let at = |y, mo, d, h, mi| Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap().timestamp();
        assert_eq!(list_date(at(2026, 10, 6, 9, 30), &now), "09:30");
        assert_eq!(list_date(at(2026, 10, 4, 18, 0), &now), "Sun 18:00");
        assert_eq!(list_date(at(2026, 9, 3, 8, 0), &now), "3 Sep");
        assert_eq!(list_date(at(2025, 12, 10, 8, 0), &now), "10 Dec 2025");
    }

    #[test]
    fn columns_split_markers_sender_subject_and_date() {
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let mut row = Row::from_message(&message("INBOX", 1, "Lunch?"));
        row.unread = true;
        row.flagged = true;
        row.count = 3;
        row.from = "Linus Example <linus@example.com>".into();
        row.date = Utc.with_ymd_and_hms(2026, 10, 6, 9, 13, 0).unwrap().timestamp();
        let c = columns(&row, false, false, true, &now);
        assert!(c.unread && c.flagged && !c.marked);
        assert_eq!(c.who, "Linus Example");
        assert_eq!(c.subject, "[INBOX] Lunch? (3)");
        assert_eq!(c.date, "09:13");
    }

    #[test]
    fn column_widths_keep_the_date_and_never_go_negative() {
        assert_eq!(column_widths(600.0, 80.0), (130.0, 600.0 - 24.0 - 130.0 - 12.0 - 80.0 - 12.0));
        let (sender, subject) = column_widths(150.0, 80.0);
        assert!(sender >= 0.0 && subject >= 0.0);
        assert!(sender + subject <= 150.0 - 24.0 - 80.0 - 24.0);
    }
```

The old `list_date_is_time_today_weekday_this_week_else_the_date` and `row_job_colours_markers_and_mutes_read_rows` tests are replaced by these.

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::list`
Expected: FAIL (`"Sun"` not `"Sun 18:00"`; `columns`, `column_widths` missing).

- [ ] **Step 3: Implement the pure parts**

```rust
/// The time today, weekday and time within the past week, day and month this year, else day, month and year.
pub(crate) fn list_date<Tz: TimeZone>(ts: i64, now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let Some(at) = now.timezone().timestamp_opt(ts, 0).single() else {
        return String::new();
    };
    let days = now.date_naive().signed_duration_since(at.date_naive()).num_days();
    let format = match days {
        0 => "%H:%M",
        1..=6 => "%a %H:%M",
        _ if at.year() == now.year() => "%-d %b",
        _ => "%-d %b %Y",
    };
    at.format(format).to_string()
}

const MARKERS: f32 = 24.0;
const SENDER: f32 = 130.0;
const GAP: f32 = 12.0;

pub(crate) fn column_widths(total: f32, date: f32) -> (f32, f32) {
    let rest = (total - MARKERS - date - 2.0 * GAP).max(0.0);
    let sender = SENDER.min(rest / 2.0);
    (sender, (rest - sender).max(0.0))
}

pub(crate) fn columns<Tz: TimeZone>(row: &Row, marked: bool, recipient: bool, in_search: bool, now: &DateTime<Tz>) -> Columns
where
    Tz::Offset: std::fmt::Display,
{
    let who = display_name(if recipient { &row.to } else { &row.from });
    let mut subject = String::new();
    if in_search {
        subject.push_str(&format!("[{}] ", row.folder));
    }
    subject.push_str(&row.subject);
    if row.count > 1 {
        subject.push_str(&format!(" ({})", row.count));
    }
    Columns {
        unread: row.unread,
        flagged: row.flagged,
        marked,
        who: clean(&who, false),
        subject: clean(&subject, false),
        date: list_date(row.date, now),
        member: row.member,
    }
}
```

(`chrono::Datelike` must be imported for `year()`.)

- [ ] **Step 4: Draw the rows** — in `show`, replace the `row_job` button with an allocated row:

```rust
            let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), egui::Sense::click());
            let c = columns(row, marked, recipient, app.search.is_some(), &now);
            let selected = index == list.cursor || marked;
            let painter = ui.painter_at(rect);
            if selected {
                painter.rect_filled(rect, 2.0, palette.accent);
            } else if response.hovered() {
                painter.rect_filled(rect, 2.0, palette.hover);
            }
            let ink = |color| if selected { palette.on_accent } else { color };
            let mut x = rect.left() + 6.0;
            if c.unread {
                painter.circle_filled(egui::pos2(x + 2.0, rect.center().y), 3.0, ink(palette.accent));
            }
            x += 10.0;
            let marker = if c.marked { Some(icons::CHECK) } else if c.flagged { Some(icons::FLAG) } else { None };
            if let Some(marker) = marker {
                painter.text(egui::pos2(x + 6.0, rect.center().y), egui::Align2::CENTER_CENTER, marker, font.clone(), ink(palette.highlight));
            }
            x += 14.0;
            let date = painter.layout_no_wrap(c.date.clone(), font.clone(), ink(palette.muted));
            painter.galley(egui::pos2(rect.right() - 6.0 - date.size().x, rect.center().y - date.size().y / 2.0), date.clone(), ink(palette.muted));
            let (sender_w, subject_w) = column_widths(rect.width() - 12.0, date.size().x);
            let text_color = ink(if c.unread { palette.text } else { palette.secondary });
            let indent = if c.member { 12.0 } else { 0.0 };
            let truncated = |text: &str, width: f32| {
                let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font.clone(), text_color);
                job.wrap = egui::text::TextWrapping::truncate_at_width(width);
                painter.layout_job(job)
            };
            let sender = truncated(&c.who, sender_w - indent);
            painter.galley(egui::pos2(x + indent, rect.center().y - sender.size().y / 2.0), sender, text_color);
            let subject = truncated(&c.subject, subject_w);
            painter.galley(egui::pos2(x + sender_w + GAP, rect.center().y - subject.size().y / 2.0), subject, text_color);
            let name = row_text(row, recipient, app.search.is_some(), &now);
            response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &name));
            if response.clicked() {
                actions.push(UiAction::SelectRow(index));
            }
```

Delete `row_job` and its imports. `ROW_HEIGHT` and the `show_rows` scrolling stay as they are.

- [ ] **Step 5: Run the list and body suites**

Run: `cargo test --lib gui::`
Expected: PASS; the keyboard and click tests find rows by `row_text`, which is still each row's name.

- [ ] **Step 6: Docs, snapshots, commit** — `docs/src/gui.md`: the list shows an unread dot, a flag, sender, subject and date in columns; dates read "09:30", "Sun 18:00", "3 Sep" or "10 Dec 2025". Regenerate on Linux.

```bash
git add src/gui/list.rs docs/src/gui.md tests/snapshots/
git commit -m "feat(gui): align message rows in columns with the date on the right"
```

---

### Task 7: Reader date

**Files:**
- Modify: `src/gui/body.rs`

**Interfaces:**
- Produces: `body::HEADER_DATE: &str`.

#65 (merged) rewrote much of `body.rs`; apply this to the header code it left (the format constant is the whole change).

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn the_header_date_is_full_with_seconds_and_offset() {
        let at = chrono::Utc.timestamp_opt(1_790_000_000, 0).unwrap();
        assert_eq!(at.format(HEADER_DATE).to_string(), "Monday 21 September 2026, 14:13:20 (UTC+00:00)");
    }
```

- [ ] **Step 2: Run it**

Run: `cargo test --lib gui::body::tests::the_header_date`
Expected: FAIL to compile (`HEADER_DATE` missing).

- [ ] **Step 3: Implement**

```rust
/// The reader's Date: weekday, day, month, year, time with seconds, and the UTC offset. chrono has no portable zone
/// abbreviation ("CEST"), so the offset stands in for it.
pub(crate) const HEADER_DATE: &str = "%A %-d %B %Y, %H:%M:%S (UTC%:z)";
```

and in `show`: `let date = local_time(message.date.unwrap_or(message.internaldate), HEADER_DATE);`. Update `the_cursor_message_shows_headers_and_text` if it asserts the old format.

- [ ] **Step 4: Run, snapshots on Linux, commit**

Run: `cargo test --lib gui::`
Expected: PASS.

```bash
git add src/gui/body.rs tests/snapshots/
git commit -m "feat(gui): show the full date with seconds and offset in the reader"
```

---

### Task 8: Toolbar, sync button and version

**Files:**
- Create: `src/gui/toolbar.rs`
- Modify: `src/gui/mod.rs` (`mod toolbar;`, `ICON` visible to the crate), `src/gui/app.rs` (panel, `logo` texture), `src/gui/status.rs` (sync button, version), `docs/src/gui.md`

**Interfaces:**
- Consumes: `icons::{ARCHIVE, MOVE, TRASH, FLAG, MARK_UNREAD, SEARCH, SYNC}` (Task 2).
- Produces: `toolbar::show(app: &App, ui: &mut egui::Ui) -> Vec<UiAction>`; `App.logo: Option<egui::TextureHandle>`; `pub(crate) fn sync_key() -> &'static str` in `status.rs`.

The toolbar holds: the logo and "postbode" (as wide as the folder pane's default, 212 px), then icon-only buttons Archive, Move, Trash, Flag, Mark unread, then a search button. Each button's hover text is "Name · key"; each is disabled outside a folder view or with no row. Search stays in the list pane: the button starts it like `/`. New, Reply and Forward are left out (phase 4).

- [ ] **Step 1: Write the failing tests** in `toolbar.rs`

```rust
#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;

    use crate::gui::test_support::{Fixture, message};
    use crate::sync::Command;

    #[test]
    fn the_archive_button_archives_the_cursor_row() {
        let fx = Fixture::new(&["work"]);
        fx.folder("work", "Archive", Some("Archive"));
        fx.add("work", message("INBOX", 1, "hello"));
        let (mut harness, wires) = fx.harness();
        harness.get_by_label("Archive message").click();
        harness.run();
        assert!(!wires.sent().is_empty());
        assert!(harness.state().list.rows.is_empty());
    }

    #[test]
    fn the_search_button_opens_search() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        harness.get_by_label("Search").click();
        harness.run();
        assert!(harness.state().search.is_some());
    }

    #[test]
    fn the_status_bar_sync_button_syncs_every_account() {
        let fx = Fixture::new(&["home", "work"]);
        let (mut harness, wires) = fx.harness();
        harness.get_by_label("Sync now").click();
        harness.run();
        assert_eq!(
            wires.sent(),
            [("home".to_string(), Command::SyncNow), ("work".to_string(), Command::SyncNow)]
        );
    }

    #[test]
    fn the_status_bar_shows_the_version() {
        let fx = Fixture::new(&["work"]);
        let (harness, _wires) = fx.harness();
        assert!(harness.query_by_label(&format!("v{}", env!("CARGO_PKG_VERSION"))).is_some());
    }
}
```

If `harness.state().search` is private to `app.rs`, assert on `harness.state().focus_search` or whatever field `StartSearch` sets.

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::toolbar`
Expected: FAIL to compile (`toolbar` module empty).

- [ ] **Step 3: Implement `toolbar.rs`**

```rust
//! The top bar: the logo, then actions on the cursor row or the selection, then search.
use eframe::egui;

use crate::rules::Action;

use super::app::{App, UiAction, View};
use super::icons;

pub(crate) fn show(app: &App, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        ui.allocate_ui(egui::vec2(212.0, ui.available_height()), |ui| {
            ui.horizontal(|ui| {
                if let Some(logo) = &app.logo {
                    ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(20.0, 20.0)));
                }
                ui.label(egui::RichText::new("postbode").monospace().strong());
            });
        });
        let has_row = matches!(app.view, View::Folder { .. }) && !app.list.rows.is_empty();
        for (icon, name, key, action) in [
            (icons::ARCHIVE, "Archive message", "Archive · e", UiAction::Act(Action::Archive)),
            (icons::MOVE, "Move message", "Move · m", UiAction::OpenMovePicker),
            (icons::TRASH, "Delete message", "Delete · #", UiAction::Act(Action::Trash)),
            (icons::FLAG, "Flag message", "Flag · s", UiAction::ToggleFlag),
            (icons::MARK_UNREAD, "Mark read or unread", "Read or unread · u", UiAction::ToggleRead),
        ] {
            if icon_button(ui, has_row, icon, name, key).clicked() {
                actions.push(action);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_button(ui, matches!(app.view, View::Folder { .. }), icons::SEARCH, "Search", "Search · /").clicked() {
                actions.push(UiAction::StartSearch);
            }
        });
    });
    actions
}

/// An icon button named `name` for screen readers and tests, with `hint` on hover.
pub(crate) fn icon_button(ui: &mut egui::Ui, enabled: bool, icon: &str, name: &str, hint: &str) -> egui::Response {
    let response = ui.add_enabled(enabled, egui::Button::new(icon).frame(false)).on_hover_text(hint);
    let name = name.to_string();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &name));
    response
}
```

`app.rs`, in `App::show` before the status panel:

```rust
        if self.logo.is_none()
            && let Ok(icon) = eframe::icon_data::from_png_bytes(super::ICON)
        {
            let image = egui::ColorImage::from_rgba_unmultiplied([icon.width as usize, icon.height as usize], &icon.rgba);
            self.logo = Some(ctx.load_texture("logo", image, egui::TextureOptions::LINEAR));
        }
        egui::Panel::top("toolbar").show(ui, |ui| actions.extend(toolbar::show(self, ui)));
```

`ICON` in `mod.rs` becomes `pub(crate) const`. Add `pub logo: Option<egui::TextureHandle>` to `App` (`None` in `App::new`).

`status.rs`:

```rust
pub(crate) fn sync_key() -> &'static str {
    if cfg!(target_os = "macos") { "Sync now · Cmd+R" } else { "Sync now · Ctrl+R" }
}
```

At the start of the outer `ui.horizontal` in `show`: `if super::toolbar::icon_button(ui, true, icons::SYNC, "Sync now", sync_key()).clicked() { actions.push(UiAction::SyncNow); }`. In the right-to-left block, after the theme buttons: `ui.weak(format!("v{}", env!("CARGO_PKG_VERSION")));`.

- [ ] **Step 4: Run tests and the architecture check**

Run: `cargo test --lib gui:: && cargo test --test architecture`
Expected: PASS. `toolbar.rs` is a view: it only returns `UiAction`s.

- [ ] **Step 5: Docs, snapshots, commit** — `docs/src/gui.md`: a toolbar with the actions and their keys on hover, a sync button and the version in the status bar.

```bash
git add src/gui/ docs/src/gui.md tests/snapshots/
git commit -m "feat(gui): add a toolbar, a sync button and the version"
```

---

### Task 9: Regional number grouping

**Files:**
- Modify: `Cargo.toml` (`sys-locale`, `gui` feature)
- Create: `src/gui/numbers.rs`
- Modify: `src/gui/mod.rs` (`mod numbers;`), `src/gui/folders.rs` (counts), `src/gui/list.rs` (search result count)

**Interfaces:**
- Produces: `numbers::separator_for(locale: &str) -> Option<char>`, `numbers::group(n: u64, separator: Option<char>) -> String`, `numbers::count(n: u64) -> String` (device locale, cached in a `OnceLock`).

- [ ] **Step 1: Add the dependency**

```toml
sys-locale = { version = "0.3", optional = true }  # the device locale, to group digits in counts the way the user expects
```

and add `"dep:sys-locale"` to `gui`.

- [ ] **Step 2: Write the failing tests** in a new `src/gui/numbers.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales_pick_their_separator_and_unknown_ones_none() {
        assert_eq!(separator_for("nl-NL"), Some('.'));
        assert_eq!(separator_for("nl_NL.UTF-8"), Some('.'));
        assert_eq!(separator_for("de"), Some('.'));
        assert_eq!(separator_for("en-US"), Some(','));
        assert_eq!(separator_for("fr-FR"), Some('\u{202f}'));
        assert_eq!(separator_for("C"), None);
        assert_eq!(separator_for("POSIX"), None);
        assert_eq!(separator_for(""), None);
        assert_eq!(separator_for("xx"), None);
    }

    #[test]
    fn group_inserts_the_separator_every_three_digits() {
        assert_eq!(group(0, Some('.')), "0");
        assert_eq!(group(999, Some('.')), "999");
        assert_eq!(group(1_284, Some('.')), "1.284");
        assert_eq!(group(1_234_567, Some(',')), "1,234,567");
        assert_eq!(group(1_234_567, None), "1234567");
    }
}
```

- [ ] **Step 3: Run them**

Run: `cargo test --lib gui::numbers`
Expected: FAIL to compile.

- [ ] **Step 4: Implement**

```rust
//! Counts grouped the way the device's region writes numbers.
use std::sync::OnceLock;

/// The thousands separator for a BCP 47 or POSIX locale name; None when unknown, which shows plain digits.
pub(crate) fn separator_for(locale: &str) -> Option<char> {
    let language = locale.split(['-', '_', '.']).next().unwrap_or("").to_ascii_lowercase();
    match language.as_str() {
        "da" | "de" | "el" | "es" | "id" | "it" | "nl" | "pt" | "tr" => Some('.'),
        "en" | "he" | "ja" | "ko" | "th" | "zh" => Some(','),
        "cs" | "fi" | "fr" | "nb" | "pl" | "ru" | "sk" | "sv" | "uk" => Some('\u{202f}'),
        _ => None,
    }
}

pub(crate) fn group(n: u64, separator: Option<char>) -> String {
    let digits = n.to_string();
    let Some(separator) = separator else {
        return digits;
    };
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(separator);
        }
        out.push(digit);
    }
    out
}

/// `n` grouped for this device's locale, read once.
pub(crate) fn count(n: u64) -> String {
    static SEPARATOR: OnceLock<Option<char>> = OnceLock::new();
    let separator = *SEPARATOR.get_or_init(|| sys_locale::get_locale().as_deref().and_then(separator_for));
    group(n, separator)
}
```

Swiss German ("de-CH" uses an apostrophe) and Indian grouping are not covered: `// ponytail: language-only table; region overrides such as de-CH when someone asks`.

Use it: in `folders.rs` the count becomes `Some(numbers::count(u64::from(unread)))` (accessible name too); in `list.rs` the search line uses `numbers::count(list.rows.len() as u64)`.

- [ ] **Step 5: Run tests, machete**

Run: `cargo test --lib gui:: && cargo machete`
Expected: PASS. Snapshot fixtures use small counts, so the PNGs do not change.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/gui/
git commit -m "feat(gui): group counts by the device's region"
```

---

## Finish

- [ ] Full gate on the last branch: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && cargo machete && cargo audit`
- [ ] Snapshots regenerated on Linux with lavapipe for every task that changed drawing, and each changed PNG looked at.
- [ ] Report which platforms each PR was compiled on and ran on.
