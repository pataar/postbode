# Reader Save .eml, View Source and Text | HTML Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A small reader toolbar above the message header with icon buttons "Save .eml" and "View source", and a Text | HTML switch for mail with an HTML part.

**Architecture:** File writing lives in `message.rs` beside `save_attachment` (pure, unit tested); the reader view (`body.rs`) draws the toolbar and the source window and only returns `UiAction`s; `app.rs` reads the raw bytes from the store and changes state. The source window keeps the cleaned text once plus the byte ranges of its rows, and draws only the visible rows with `ScrollArea::show_rows`, so a large message costs one conversion on open, not per frame.

**Tech Stack:** Rust 2024 (toolchain 1.99), eframe/egui 0.36, egui_kittest 0.36, egui-phosphor 0.14, mail-parser (already used by `message.rs`). No new dependency.

**Spec:** design canvas https://claude.ai/artifact/B5W17bKLqpKUw9z3bCw65Q (Inbox · Mocha board, reader toolbar) + docs/superpowers/specs/2026-10-06-postbode-core-design.md

**Lands after #65** ("feat(gui): render HTML mail with Blitz", branch `ccr-993a2548-6jd8zc`) and after Tasks 2 (icon font, `src/gui/icons.rs`) and 8 (`toolbar::icon_button`) of `docs/superpowers/plans/2026-10-08-gui-redesign.md`. Every `app.rs` and `body.rs` excerpt below is written against #65's version of those files: `BodyState` already has `html`, `html_view`, `show_text`, `text_note` and `shows_html()`, `UiAction::ToggleHtml` exists and `v` sends it, and `read_stored` returns the `Stored` tuple.

## Scope

| # | Task | Canvas element |
|---|---|---|
| 1 | `message::save_eml` | Save .eml writes a file |
| 2 | Reader toolbar with Save .eml | toolbar row, save button |
| 3 | View source window | source button, window with Copy |
| 4 | Text \| HTML switch | segmented switch |

One branch off `origin/main` once #65 and gui-redesign Tasks 2 and 8 are merged, a commit per task, one PR. Tasks are ordered 1 → 2 → 3 → 4.

Not in this plan: Reply, Reply all, Forward (compose and SMTP are phase 4 in the core spec); a key for Save .eml or View source; keeping the source window open across messages; syntax colouring of the source.

## Global Constraints

- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `cargo machete` and `cargo audit` all pass before the PR.
- No `unwrap`, `expect`, `panic!`, `todo!` or `unimplemented!` outside tests; where a call cannot fail, a narrow `#[allow]` says why.
- No new dependency.
- GUI views draw and return `UiAction`s; only `app.rs` changes state (`tests/architecture.rs` checks this). `ctx.copy_text` and `ctx.open_url` are output commands, not state, and views may call them (`body.rs` already calls `open_url`).
- Server-supplied text is shown through `message::clean`; never log message bodies or raw bytes.
- The saved `.eml` is the stored raw bytes unchanged; only what is shown or copied is cleaned.
- Colours come from `theme::Palette`; icons come from `gui::icons`.
- After GUI changes, regenerate snapshots on Linux with lavapipe: `TZ=UTC UPDATE_SNAPSHOTS=1 cargo test gui::snapshots`, and look at every changed PNG in `tests/snapshots/`.
- Prose changes go in `docs/src/gui.md`; `README.md` is not touched (it only contains `docs/src/index.md`).
- Branch: `git switch -c feat/reader-save-source --no-track origin/main`, first push `git push -u origin HEAD`. Conventional commits.
- Report platform coverage honestly: "compiled on" vs "ran on".

## Review Focus

1. **A subject with path separators, leading dots, control characters, or none at all** ("../../.ssh/authorized_keys", "Re: lunch", an ESC sequence, empty): the file lands inside the downloads folder with a plain visible name, never a hidden file or a path elsewhere. Test in Task 1.
2. **Saving the same message (or two "Re: lunch" messages) twice**: the second save writes "Re- lunch (2).eml" and leaves the first file byte for byte as it was. Test in Task 1.
3. **A message whose body is not downloaded yet, or an account that is offline**: Save .eml and View source are disabled with the reader's own wording on hover, clicking them writes nothing and sends no extra command, and the frame never waits on the network. Test in Task 2, extended to both buttons in Task 3.
4. **Raw source with terminal escapes and a 1 MB single line** (a minified HTML part): the window shows it without control characters, splits the long line into rows of at most 1000 characters so every row lays out quickly, and Copy puts text without ESC on the clipboard. Test in Task 3.
5. **Moving to another message while the source window is open**: the window closes rather than keep showing the previous message's source under the new headers. Test in Task 3.

---

### Task 1: `message::save_eml`

**Files:**
- Modify: `src/message.rs` (`save_attachment` uses a new `write_new`; new `save_eml`, `eml_stem`, tests)

**Interfaces:**
- Consumes: nothing new.
- Produces: `pub fn save_eml(raw: &[u8], subject: Option<&str>, dir: &Path) -> io::Result<PathBuf>`; private `fn eml_stem(subject: Option<&str>) -> String`, `fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()>`.

Behaviour: the name is the subject with `/`, `\` and `:` turned into `-`, control characters removed, leading dots and whitespace trimmed, cut to at most 120 bytes on a character boundary, and "message" when nothing is left. The file is created with `create_new`, so an existing file is never touched; when the name is taken, " (2)" up to " (100)" is tried before giving up with `AlreadyExists`. A failed write removes the partial file. `:` is replaced because Finder shows it as `/`; the release targets are macOS and Linux only (`dist-workspace.toml`), so Windows-only reserved names are not handled.

- [ ] **Step 1: Write the failing tests** in `message.rs` tests

```rust
    #[test]
    fn eml_names_come_from_the_subject_and_stay_plain_file_names() {
        assert_eq!(eml_stem(Some("Re: lunch")), "Re- lunch");
        assert_eq!(
            eml_stem(Some("../../.ssh/authorized_keys")),
            "-..-.ssh-authorized_keys"
        );
        assert_eq!(eml_stem(Some("\u{1b}]0;pwned\u{7}hi")), "]0;pwnedhi");
        assert_eq!(eml_stem(Some(" ... ")), "message");
        assert_eq!(eml_stem(None), "message");
        assert_eq!(eml_stem(Some(&"é".repeat(100))), "é".repeat(60));
    }

    #[test]
    fn save_eml_never_overwrites_and_numbers_the_copy() {
        let dir = tempfile::tempdir().unwrap();
        let first = save_eml(b"one", Some("Re: lunch"), dir.path()).unwrap();
        let second = save_eml(b"two", Some("Re: lunch"), dir.path()).unwrap();
        assert_eq!(first, dir.path().join("Re- lunch.eml"));
        assert_eq!(second, dir.path().join("Re- lunch (2).eml"));
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
    }

    #[test]
    fn save_eml_reports_a_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let err = save_eml(b"x", None, &dir.path().join("gone")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
```

`tempfile` is already a dev-dependency (`gui/test_support.rs` uses it).

- [ ] **Step 2: Run them**

Run: `cargo test --lib message::tests`
Expected: FAIL to compile (`eml_stem`, `save_eml` not found).

- [ ] **Step 3: Implement** in `message.rs`, below `safe_file_name`

```rust
/// Bytes kept of a subject in a file name; well under the 255-byte limit of APFS and ext4, with room for " (100).eml".
const MAX_STEM: usize = 120;

/// Numbered names tried after the plain one before Save .eml gives up.
const MAX_COPIES: u32 = 100;

/// Writes `raw` unchanged to `dir` as `<subject>.eml`, or `<subject> (2).eml` and on when that name is taken; never
/// overwrites a file.
pub fn save_eml(raw: &[u8], subject: Option<&str>, dir: &Path) -> io::Result<PathBuf> {
    let stem = eml_stem(subject);
    for copy in 1..=MAX_COPIES {
        let name = match copy {
            1 => format!("{stem}.eml"),
            n => format!("{stem} ({n}).eml"),
        };
        let path = dir.join(name);
        match write_new(&path, raw) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("{stem}.eml and {MAX_COPIES} numbered copies exist"),
    ))
}

/// The sender picks the subject: no path separators or `:`, no control characters, no leading dots, so it is one
/// visible file name.
fn eml_stem(subject: Option<&str>) -> String {
    let plain: String = subject
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if matches!(c, '/' | '\\' | ':') { '-' } else { c })
        .collect();
    let trimmed = plain.trim_start_matches(|c: char| c == '.' || c.is_whitespace());
    let end = trimmed
        .char_indices()
        .map(|(at, c)| at + c.len_utf8())
        .take_while(|&end| end <= MAX_STEM)
        .last()
        .unwrap_or(0);
    match trimmed[..end].trim_end() {
        "" => "message".into(),
        stem => stem.into(),
    }
}

/// Creates `path`, failing when it exists, and writes `bytes`; a failed write leaves no partial file.
fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    if let Err(e) = file.write_all(bytes) {
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok(())
}
```

`str::floor_char_boundary` is not used: it was unstable on the rustc available while writing this plan (1.85), and the toolchain pin (1.99) could not be checked here. The `char_indices` scan does the same.

In `save_attachment`, replace the `OpenOptions … remove_file … Ok(path)` tail with:

```rust
    let path = dir.join(safe_file_name(part.attachment_name(), index));
    write_new(&path, part.contents())?;
    Ok(path)
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib message::tests`
Expected: PASS, including `attachments_are_listed_and_saved_inside_the_directory` (unchanged behaviour of `save_attachment`).

- [ ] **Step 5: Commit**

```bash
git add src/message.rs
git commit -m "feat(message): save a raw message as an .eml file named after its subject"
```

---

### Task 2: Reader toolbar with Save .eml

**Files:**
- Modify: `src/gui/icons.rs` (`SAVE`), `src/gui/body.rs` (`toolbar`, tests), `src/gui/app.rs` (`UiAction::SaveEml`, `shown_raw`, `save_eml`, `saved_note`), `docs/src/gui.md`

**Interfaces:**
- Consumes: `message::save_eml` (Task 1); `toolbar::icon_button(ui: &mut egui::Ui, enabled: bool, icon: &str, name: &str, hint: &str) -> egui::Response` (gui-redesign Task 8); `BodyState` and `UiAction` as of #65.
- Produces: `icons::SAVE: &str`; `UiAction::SaveEml`; `App::shown_raw(&self) -> Option<Vec<u8>>` (private); `fn toolbar(body: &BodyState, account: &Account, ui: &mut egui::Ui) -> Vec<UiAction>` in `body.rs` (private), drawn by `body::show` right after the "no longer in the local store" check, so it shows above the headers in both the text and the HTML view.

"Downloaded" is `message.body_text.is_some()`, the same signal the reader uses for "Loading…": sync stores `raw` and `body_text` together (`Store::set_raw`, `sync::ensure_raw`). The disabled hover text is `missing_text(account)`, the reader's own wording. Nothing in the toolbar fetches: the reader's existing `FetchBody` brings the raw message, and `BodyReady` reloads `body.message`, which enables the buttons.

- [ ] **Step 1: Write the failing tests** in `body.rs` tests (add `use egui_kittest::kittest::NodeT;` to the test imports)

```rust
    #[test]
    fn save_eml_writes_the_raw_message_to_downloads() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 1, "Re: lunch"));
        fx.store("work")
            .set_raw("INBOX", 1, WITH_ATTACHMENT.as_bytes(), "See attached")
            .unwrap();
        let (mut harness, _wires) = fx.harness();
        harness.get_by_label("Save .eml").click();
        harness.run();
        let saved = fx.paths.cache_dir.join("downloads").join("Re- lunch.eml");
        assert_eq!(std::fs::read(&saved).unwrap(), WITH_ATTACHMENT.as_bytes());
        assert!(harness.query_by_label_contains("Saved to").is_some());
    }

    #[test]
    fn reader_actions_wait_for_the_download_and_never_fetch_on_their_own() {
        let fx = Fixture::new(&["work"]);
        let mut m = message("INBOX", 1, "later");
        m.body_text = None;
        fx.add("work", m);
        let (mut harness, wires) = fx.harness();
        assert_eq!(wires.sent(), [fetch(1)]);
        let button = harness.get_by_label("Save .eml");
        assert!(button.accesskit_node().is_disabled());
        button.click();
        harness.run();
        assert!(wires.sent().is_empty());
        let downloads = fx.paths.cache_dir.join("downloads");
        assert_eq!(std::fs::read_dir(downloads).unwrap().count(), 0);
    }
```

`fetch(uid)` is the existing helper in `body.rs` tests.

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::body`
Expected: FAIL, `get_by_label("Save .eml")` finds no node.

- [ ] **Step 3: Implement**

`icons.rs`, in alphabetical place among the constants (`DOWNLOAD_SIMPLE` verified in egui-phosphor 0.14.0 `variants/codepoints.rs`):

```rust
pub(crate) const SAVE: &str = ph::DOWNLOAD_SIMPLE;
```

`app.rs`: add `SaveEml` to `UiAction` between `SaveAttachment(usize)` and `SearchFocused`, and in `apply` beside `SaveAttachment`:

```rust
            UiAction::SaveEml => self.save_eml(),
```

Replace `save_attachment` and add the helpers beside it:

```rust
    /// The shown message's raw bytes from the store, or `None` while it is not downloaded.
    fn shown_raw(&self) -> Option<Vec<u8>> {
        let body = self.body.as_ref()?;
        let store = self.accounts[body.account].store.as_ref().ok()?;
        store.raw(&body.key.0, body.key.1).ok().flatten()
    }

    fn save_attachment(&mut self, index: usize) {
        let raw = self.shown_raw();
        let note = saved_note(raw.map(|raw| message::save_attachment(&raw, index, &self.downloads)));
        if let Some(body) = &mut self.body {
            body.saved = Some(note);
        }
    }

    fn save_eml(&mut self) {
        let raw = self.shown_raw();
        let subject = self
            .body
            .as_ref()
            .and_then(|b| b.message.as_ref()?.subject.clone());
        let note = saved_note(raw.map(|raw| message::save_eml(&raw, subject.as_deref(), &self.downloads)));
        if let Some(body) = &mut self.body {
            body.saved = Some(note);
        }
    }
```

and, as a free function near `body_text`:

```rust
/// The line under the attachments after a save: where the file went, or why not.
fn saved_note(result: Option<std::io::Result<PathBuf>>) -> String {
    match result {
        Some(Ok(path)) => format!("Saved to {}", path.display()),
        Some(Err(e)) => format!("Could not save: {e}"),
        None => "Could not save: the message is not downloaded".into(),
    }
}
```

`body.rs`: import `use super::app::{Account, App, BodyState, UiAction};`, `use super::icons;` and `use super::toolbar::icon_button;`. In `show`, right after `let Some(message) = &body.message else { … };`:

```rust
    actions.extend(toolbar(body, &app.accounts[body.account], ui));
```

and below `show`:

```rust
/// Actions on the shown message, on the right; disabled until its raw message is downloaded.
fn toolbar(body: &BodyState, account: &Account, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let downloaded = body.message.as_ref().is_some_and(|m| m.body_text.is_some());
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut save = icon_button(ui, downloaded, icons::SAVE, "Save .eml", "Save .eml");
            if !downloaded {
                save = save.on_disabled_hover_text(missing_text(account));
            }
            if save.clicked() {
                actions.push(UiAction::SaveEml);
            }
        });
    });
    actions
}
```

The `ui.horizontal` wrapper matters: `with_layout(right_to_left(Center))` straight in the vertical body pane would centre the buttons in the pane's whole remaining height. `on_hover_text` (inside `icon_button`) shows only while enabled and `on_disabled_hover_text` only while disabled (both verified in egui 0.36.2 `response.rs`), so each state has one hint.

`docs/src/gui.md`, a new section after "HTML mail":

```markdown
## Reader toolbar

Above the headers, **Save .eml** writes the message exactly as the server sent it to your Downloads folder, named after its subject; when that name is taken it saves beside it as "… (2).eml" and never replaces a file. The line under the attachments says where it went. Until the message is downloaded the button is greyed out and says why on hover.
```

- [ ] **Step 4: Run tests and the architecture check**

Run: `cargo test --lib gui:: && cargo test --test architecture`
Expected: PASS. `body.rs` stays a view: it takes `&App` and returns `UiAction`s.

- [ ] **Step 5: Snapshots on Linux** (every reader snapshot gains the toolbar row), look at each changed PNG, then commit

```bash
git add src/gui/ docs/src/gui.md tests/snapshots/
git commit -m "feat(gui): add a reader toolbar with Save .eml"
```

---

### Task 3: View source window

**Files:**
- Modify: `src/gui/icons.rs` (`SOURCE`), `src/gui/body.rs` (`Source`, `source_rows`, `show_source`, toolbar loop, tests), `src/gui/app.rs` (`BodyState.source`, `UiAction::{CloseSource, OpenSource}`, `open_source`, Escape, `show`), `docs/src/gui.md`

**Interfaces:**
- Consumes: `App::shown_raw`, `fn toolbar` (Task 2); `message::clean`.
- Produces (also): `icons::SOURCE: &str`.
- Produces: `pub(crate) struct Source { pub rows: Vec<Range<usize>>, pub text: String }` with `Source::new(raw: &[u8]) -> Source`; `pub(crate) const SOURCE_ROW: usize = 1000`; `pub(crate) fn source_rows(text: &str) -> Vec<Range<usize>>`; `pub(crate) fn show_source(app: &App, ctx: &egui::Context) -> Vec<UiAction>`; `BodyState.source: Option<body::Source>`; `UiAction::CloseSource`, `UiAction::OpenSource`.

The window is titled "Message source", with a Copy button above a `ScrollArea::both()` of monospace rows. `Source::new` decodes the raw bytes lossily (8-bit parts show U+FFFD; Save .eml keeps the exact bytes), runs `clean(…, true)` (drops `\r`, ESC and every other control character except `\n` and `\t`), and records each line as a row, split every 1000 characters (RFC 5322's 998-character limit, so only malformed mail splits). Rows are drawn with `show_rows`, which lays out only the visible ones, and `Label::extend()` so nothing wraps. Copy copies the cleaned `text`, not the raw bytes, so pasting into a terminal cannot replay escape sequences. The source lives in `BodyState`, which is rebuilt when the cursor moves, so the window closes with its message; Escape closes it first, before help and history.

- [ ] **Step 1: Write the failing tests** in `body.rs` tests

```rust
    const HOSTILE: &[u8] = b"From: a@example.com\r\nSubject: \x1b]0;pwned\x07report\r\n\r\nline one\r\n";

    #[test]
    fn source_rows_split_lines_and_long_lines_on_char_boundaries() {
        assert_eq!(source_rows("ab\ncd"), [0..2, 3..5]);
        assert_eq!(source_rows(""), [0..0]);
        let long = "é".repeat(2 * SOURCE_ROW + 500);
        let rows = source_rows(&long);
        let lengths: Vec<usize> = rows.iter().map(|r| long[r.clone()].chars().count()).collect();
        assert_eq!(lengths, [SOURCE_ROW, SOURCE_ROW, 500]);
    }

    #[test]
    fn the_source_window_shows_the_raw_message_cleaned_and_copies_it() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 1, "report"));
        fx.store("work").set_raw("INBOX", 1, HOSTILE, "line one").unwrap();
        let (mut harness, _wires) = fx.harness();
        harness.get_by_label("View source").click();
        harness.run();
        assert!(harness.query_by_label("Subject: ]0;pwnedreport").is_some());
        harness.get_by_label("Copy").click();
        harness.step();
        let copied: Vec<&str> = harness
            .output()
            .platform_output
            .commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::CopyText(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            copied,
            ["From: a@example.com\nSubject: ]0;pwnedreport\n\nline one\n"]
        );
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(harness.query_by_label("Subject: ]0;pwnedreport").is_none());
    }

    #[test]
    fn a_megabyte_line_opens_as_short_rows() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 1, "minified"));
        let raw = format!("Subject: minified\r\n\r\n{}\r\n", "x".repeat(1 << 20));
        fx.store("work").set_raw("INBOX", 1, raw.as_bytes(), "x").unwrap();
        let (mut harness, _wires) = fx.harness();
        harness.get_by_label("View source").click();
        harness.run();
        let body = harness.state().body.as_ref().unwrap();
        let source = body.source.as_ref().unwrap();
        assert!(source.rows.len() > (1 << 20) / SOURCE_ROW);
        assert!(source.rows.iter().all(|r| source.text[r.clone()].chars().count() <= SOURCE_ROW));
        assert!(harness.query_by_label("Subject: minified").is_some());
    }

    #[test]
    fn moving_to_another_message_closes_the_source() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 1, "older"));
        fx.add("work", message("INBOX", 2, "report"));
        fx.store("work").set_raw("INBOX", 2, HOSTILE, "line one").unwrap();
        let (mut harness, _wires) = fx.harness();
        harness.get_by_label("View source").click();
        harness.run();
        assert!(harness.query_by_label("Subject: ]0;pwnedreport").is_some());
        harness.event(egui::Event::Text("j".into()));
        harness.run();
        assert!(harness.state().body.as_ref().is_some_and(|b| b.source.is_none()));
        assert!(harness.query_by_label("Subject: ]0;pwnedreport").is_none());
    }
```

Replace Task 2's `reader_actions_wait_for_the_download_and_never_fetch_on_their_own` with the version that covers both buttons:

```rust
    #[test]
    fn reader_actions_wait_for_the_download_and_never_fetch_on_their_own() {
        let fx = Fixture::new(&["work"]);
        let mut m = message("INBOX", 1, "later");
        m.body_text = None;
        fx.add("work", m);
        let (mut harness, wires) = fx.harness();
        assert_eq!(wires.sent(), [fetch(1)]);
        for name in ["Save .eml", "View source"] {
            let button = harness.get_by_label(name);
            assert!(button.accesskit_node().is_disabled(), "{name}");
            button.click();
            harness.run();
        }
        assert!(wires.sent().is_empty());
        let downloads = fx.paths.cache_dir.join("downloads");
        assert_eq!(std::fs::read_dir(downloads).unwrap().count(), 0);
        assert!(harness.state().body.as_ref().is_some_and(|b| b.source.is_none()));
    }
```

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::body`
Expected: FAIL to compile (`source_rows`, `SOURCE_ROW`, `BodyState.source` not found).

- [ ] **Step 3: Implement**

`body.rs`, add `use std::ops::Range;` and, below `show_text`:

```rust
/// Characters per row of the source window; a longer line is split so every row lays out quickly.
pub(crate) const SOURCE_ROW: usize = 1000;

/// A raw message as the source window shows it: the cleaned text once, and the byte range of each row in it.
pub(crate) struct Source {
    pub rows: Vec<Range<usize>>,
    pub text: String,
}

impl Source {
    /// ponytail: converts the whole message on the UI thread when the window opens; a message of tens of MB stalls one
    /// frame. Build it on a thread if that is ever felt.
    pub fn new(raw: &[u8]) -> Source {
        let text = clean(&String::from_utf8_lossy(raw), true);
        let rows = source_rows(&text);
        Source { rows, text }
    }
}

/// One range per line of `text`, and one per `SOURCE_ROW` characters of a longer line.
pub(crate) fn source_rows(text: &str) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut line_start = 0;
    for line in text.split('\n') {
        let mut row_start = line_start;
        for (count, (at, _)) in line.char_indices().enumerate() {
            if count > 0 && count % SOURCE_ROW == 0 {
                rows.push(row_start..line_start + at);
                row_start = line_start + at;
            }
        }
        rows.push(row_start..line_start + line.len());
        line_start += line.len() + 1;
    }
    rows
}

/// The source window of the shown message, while it is open.
pub(crate) fn show_source(app: &App, ctx: &egui::Context) -> Vec<UiAction> {
    let Some(source) = app.body.as_ref().and_then(|b| b.source.as_ref()) else {
        return Vec::new();
    };
    let mut open = true;
    egui::Window::new("Message source")
        .open(&mut open)
        .collapsible(false)
        .default_size([720.0, 480.0])
        .show(ctx, |ui| {
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(source.text.clone());
            }
            let height = ui.text_style_height(&egui::TextStyle::Monospace);
            egui::ScrollArea::both().auto_shrink(false).show_rows(
                ui,
                height,
                source.rows.len(),
                |ui, visible| {
                    for row in source.rows.get(visible).unwrap_or_default() {
                        let line = source.text.get(row.clone()).unwrap_or_default();
                        ui.add(egui::Label::new(egui::RichText::new(line).monospace()).extend());
                    }
                },
            );
        });
    if open {
        Vec::new()
    } else {
        vec![UiAction::CloseSource]
    }
}
```

`icons.rs` (`CODE` verified in egui-phosphor 0.14.0 `variants/codepoints.rs`):

```rust
pub(crate) const SOURCE: &str = ph::CODE;
```

In `toolbar`, the Save block becomes a loop over both buttons (right to left, so View source is rightmost):

```rust
            for (icon, name, action) in [
                (icons::SOURCE, "View source", UiAction::OpenSource),
                (icons::SAVE, "Save .eml", UiAction::SaveEml),
            ] {
                let mut button = icon_button(ui, downloaded, icon, name, name);
                if !downloaded {
                    button = button.on_disabled_hover_text(missing_text(account));
                }
                if button.clicked() {
                    actions.push(action);
                }
            }
```

`app.rs`:

- `BodyState` gains, after `show_text`: `/// The raw message, while the source window shows it.` `pub source: Option<body::Source>,`; `load_body` sets `source: None`.
- `UiAction` gains `CloseSource` (after `Collapse`) and `OpenSource` (after `OpenMovePicker`).
- In `apply`:

```rust
            UiAction::CloseSource => {
                if let Some(body) = &mut self.body {
                    body.source = None;
                }
            }
            UiAction::OpenSource => self.open_source(),
```

- In the `UiAction::Escape` arm, right after the `move_picker` branch:

```rust
                } else if let Some(body) = self.body.as_mut().filter(|b| b.source.is_some()) {
                    body.source = None;
```

- Beside `save_eml`:

```rust
    fn open_source(&mut self) {
        let raw = self.shown_raw();
        let Some(body) = &mut self.body else { return };
        match raw {
            Some(raw) => body.source = Some(body::Source::new(&raw)),
            None => body.saved = Some("Could not show the source: the message is not downloaded".into()),
        }
    }
```

- In `show`, after `actions.extend(list::show_move_picker(self, &ctx));`: `actions.extend(body::show_source(self, &ctx));`.

All egui calls here were checked in egui 0.36.2: `Window::open`/`default_size`, `ScrollArea::both`/`show_rows(ui, row_height, total_rows, |ui, Range<usize>|)`, `Ui::text_style_height`, `Label::extend`, `RichText::monospace`, `Context::copy_text(String)` → `OutputCommand::CopyText`.

`docs/src/gui.md`, append to "Reader toolbar":

```markdown
**View source** opens the message as plain text in a window, with control characters left out; Copy puts that text on the clipboard. Esc or moving to another message closes it.
```

- [ ] **Step 4: Run tests and the architecture check**

Run: `cargo test --lib gui:: && cargo test --test architecture`
Expected: PASS.

- [ ] **Step 5: Snapshots on Linux** (the toolbar gains a button), look at each changed PNG, then commit

```bash
git add src/gui/ docs/src/gui.md tests/snapshots/
git commit -m "feat(gui): show a message's source in a window with Copy"
```

---

### Task 4: Text | HTML switch

**Files:**
- Modify: `src/gui/body.rs` (`toolbar`, tests), `docs/src/gui.md`

**Interfaces:**
- Consumes: `fn toolbar` (Tasks 2, 3); from #65: `BodyState.html: Option<HtmlBody>`, `BodyState.text_note: Option<&'static str>`, `BodyState::shows_html()`, `UiAction::ToggleHtml`, and the test helpers `crate::gui::html::view_tests::add_html(fx: &Fixture, uid: u32, html: &str, text: &str)` and `crate::gui::html::MAX_HTML` (both under `cfg(feature = "html")`).
- Produces: nothing new for other tasks.

Two selectable buttons, "Text" and "HTML", left of the icon buttons, shown only when `body.html.is_some()` (never without the `html` feature, since `html_of` returns `None` there). The active one is `shows_html()`. Clicking the inactive one sends `ToggleHtml`, the same action as `v`; clicking the active one does nothing. When `text_note` is set (HTML too large or failed to render), HTML is disabled with the note on hover, and Text shows as active. The source window reads the store's raw bytes, so it shows the whole message whichever view is active.

- [ ] **Step 1: Write the failing tests** in `body.rs` tests

```rust
    #[cfg(feature = "html")]
    #[test]
    fn the_text_html_switch_toggles_like_v_and_hides_without_html() {
        use crate::gui::html::view_tests::add_html;
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 1, "plain"));
        add_html(&fx, 2, "<p>rich words</p>", "plain words");
        let (mut harness, _wires) = fx.harness();
        let shows_html = |h: &egui_kittest::Harness<'_, crate::gui::App>| {
            h.state().body.as_ref().is_some_and(|b| b.shows_html())
        };
        assert!(shows_html(&harness));
        harness.get_by_label("Text").click();
        harness.run();
        assert!(!shows_html(&harness));
        assert!(harness.query_by_label("plain words").is_some());
        harness.get_by_label("HTML").click();
        harness.run();
        assert!(shows_html(&harness));
        harness.event(egui::Event::Text("j".into()));
        harness.run();
        assert!(harness.query_by_label("HTML").is_none());
        assert!(harness.query_by_label("Text").is_none());
    }

    #[cfg(feature = "html")]
    #[test]
    fn html_too_large_to_render_disables_the_html_side() {
        use crate::gui::html::{MAX_HTML, view_tests::add_html};
        let fx = Fixture::new(&["work"]);
        add_html(&fx, 1, &"x".repeat(MAX_HTML + 1), "plain words");
        let (harness, _wires) = fx.harness();
        assert!(harness.get_by_label("HTML").accesskit_node().is_disabled());
        assert!(!harness.get_by_label("Text").accesskit_node().is_disabled());
    }

    #[cfg(feature = "html")]
    #[test]
    fn the_source_window_shows_the_raw_message_in_either_view() {
        use crate::gui::html::view_tests::add_html;
        let fx = Fixture::new(&["work"]);
        add_html(&fx, 1, "<p>rich words</p>", "plain words");
        let (mut harness, _wires) = fx.harness();
        harness.get_by_label("View source").click();
        harness.run();
        let part = "Content-Type: text/html; charset=utf-8";
        assert!(harness.query_by_label(part).is_some());
        harness.event(egui::Event::Text("v".into()));
        harness.run();
        assert!(harness.state().body.as_ref().is_some_and(|b| !b.shows_html()));
        assert!(harness.query_by_label(part).is_some());
    }
```

The last test toggles with `v` rather than the switch, since the window may sit over the toolbar and a pointer click would land on it. The `j` step relies on uid 2 being the newest row (the list opens on it) and uid 1 being plain.

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::body`
Expected: FAIL, `get_by_label("Text")` finds no node.

- [ ] **Step 3: Implement** in `toolbar`, after the icon-button loop inside the right-to-left layout

```rust
            if body.html.is_some() {
                html_switch(body, ui, &mut actions);
            }
```

and below `toolbar`:

```rust
/// "Text | HTML" for mail with an HTML part; the inactive side sends the same toggle as `v`.
fn html_switch(body: &BodyState, ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
    let html = body.shows_html();
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        // Right to left: HTML is added first so the pair reads "Text | HTML".
        let mut html_side = ui.add_enabled(body.text_note.is_none(), egui::Button::selectable(html, "HTML"));
        if let Some(note) = body.text_note {
            html_side = html_side.on_disabled_hover_text(note);
        }
        let text_side = ui.add(egui::Button::selectable(!html, "Text"));
        if (html_side.clicked() && !html) || (text_side.clicked() && html) {
            actions.push(UiAction::ToggleHtml);
        }
    });
}
```

`Button::selectable(selected: bool, atoms: impl IntoAtoms)` and `Ui::add_enabled` were checked in egui 0.36.2 (`widgets/button.rs`, `ui.rs`). `ui.scope` keeps the parent's right-to-left layout, so the pair sits left of the icon buttons.

`docs/src/gui.md`, append to "Reader toolbar":

```markdown
Mail with an HTML part also has a **Text | HTML** switch there, which does the same as `v`. View source always shows the whole message, whichever is active.
```

- [ ] **Step 4: Run tests, with and without the `html` feature**

Run: `cargo test --lib gui:: && cargo test --test architecture && cargo check --no-default-features --features gui`
Expected: PASS; the `cfg(feature = "html")` tests are skipped in the last build, which must still compile.

- [ ] **Step 5: Snapshots on Linux** (`gui_html_*` gain the switch), look at each changed PNG, then commit

```bash
git add src/gui/body.rs docs/src/gui.md tests/snapshots/
git commit -m "feat(gui): add a Text | HTML switch to the reader toolbar"
```

---

## Finish

- [ ] Full gate: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && cargo machete && cargo audit`
- [ ] Snapshots regenerated on Linux with lavapipe for Tasks 2–4, and each changed PNG looked at.
- [ ] Report which platforms the PR was compiled on and ran on.
