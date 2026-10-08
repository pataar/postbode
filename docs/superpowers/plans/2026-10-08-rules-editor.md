# Rules Editor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the Rules view into a list of proposals and rules on the left and an editor for the selected rule on the right, with plain-language summaries, a highlighted TOML preview of each proposal, a preview on cached mail, and "Rule" buttons in the reader that start a draft.

**Architecture:** `rules::edit` gains `save_rule` (add, or replace by name in place) and `remove_rule`, both keeping the file's comments; it stays the only writer of rules.toml. A new pure module `gui/draft.rs` turns a `Rule` into rows of conditions and back and writes the summary and preview lines; the views (`rules.rs`, a new `rule_editor.rs`, `body.rs`) draw from `&App` and return `UiAction`s, and `app.rs` holds the one draft (`App.rule_draft`) and is the only code that changes it, saves it, or previews it through `actions::planned`.

**Tech Stack:** Rust 2024, eframe/egui 0.36.2, egui_kittest 0.36.2 (kittest 0.4.0), toml 1.1.6, toml_edit 0.25.15, egui-phosphor 0.14 (added by the GUI redesign plan). No new dependencies.

**Spec:** design canvas https://claude.ai/artifact/B5W17bKLqpKUw9z3bCw65Q (Rules · Latte and Regex tester boards) + docs/src/rules.md + docs/superpowers/specs/2026-10-06-postbode-core-design.md. The regex tester is the sibling plan `docs/superpowers/plans/2026-10-08-regex-tester.md`, which builds on this one.

## Scope

One PR off `origin/main`, one commit per task. Task order matters: each task adds code together with its first caller, so every commit passes `clippy -D warnings` (no dead code waiting for a later task).

| # | Task | Design item |
|---|---|---|
| 1 | `rules::edit::save_rule` and `remove_rule` | 4 (saving keeps comments; renaming) |
| 2 | Rules list and proposal cards | 1, 2, 3 |
| 3 | Rule editor | 1 ("New rule"), 4 |
| 4 | "Rule" buttons in the reader | 6 |

Prerequisites on `main`: Task 1 (Latte contrast) and Task 2 (icon font, `src/gui/icons.rs`) of `docs/superpowers/plans/2026-10-08-gui-redesign.md`. This plan's colour tests assume Latte `accent = #10757a`, `on_accent = #ffffff`, `success = #276b19`.

Not in this plan: the regex tester (sibling plan); editing a pending proposal in the editor (proposals are approved, rejected or previewed from their card); warning about unsaved edits when another rule is selected (the draft is replaced).

## Global Constraints

- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `cargo machete` and `cargo audit` all pass after every task.
- No `unwrap`, `expect`, `panic!`, `todo!` or `unimplemented!` outside tests; where a call cannot fail, a narrow `#[allow]` says why.
- Only `src/rules/edit.rs` writes rules.toml. GUI views draw from `&App` and return `UiAction`s; only `app.rs` changes state (`tests/architecture.rs` checks both).
- Nothing is written until Save: a new rule, and a draft from the reader's Rule buttons, live only in `App.rule_draft`.
- Test and "Preview on cached mail" are read-only: `actions::planned` over the local stores, no daemon command, no store write.
- Renaming a rule makes it a new rule and restarts its clock; the editor says so before Save: "Renaming makes this a new rule: it acts only on mail that arrives after you save."
- Rules act only on mail that arrives after they are enabled; nothing in this plan runs `apply-existing`.
- Copy, verbatim: "Rules", "1 rule · 2 proposals", "Open rules.toml", "New rule", "proposed by {who}", "Adds to rules.toml", "Approve", "Reject", "Preview on cached mail", "Name", "Enabled", "Watches", "Every account", "When all of these match", "+ Add condition", "Then", "+ Add action", "keeps a .eml backup", "Test", "Remove rule", "Remove from rules.toml", "Cancel", "Save", "would delete 4 cached messages", "matches no cached message".
- The rule list is 460 px wide.
- Colours come from `theme::Palette`: proposal chip `agent` on `agent_bg` (Latte #a34106 on #fde6d6); TOML block on `added` with section headers `agent`, keys `accent`, strings `success`, booleans `error`, "+" gutter `success`. No blue or purple. Every text colour on its background passes WCAG AA (4.5:1) in both themes.
- egui's proportional font has no "→" (checked: Ubuntu-Light lacks U+2192, Hack has it), so every line holding "→" is drawn monospace.
- Agent text is untrusted: everything from rules.toml is passed through `message::clean` before it is drawn.
- Prose changes go in `docs/src/gui.md`.
- After GUI changes, regenerate snapshots on Linux with lavapipe: `TZ=UTC UPDATE_SNAPSHOTS=1 cargo test gui::snapshots`, and look at every changed PNG in `tests/snapshots/`.
- Branch: `git switch -c feat/rules-editor --no-track origin/main`, first push `git push -u origin HEAD`. Conventional commits. Report platform coverage honestly: "compiled on" vs "ran on".

## Verification notes

Checked against source in `~/.cargo/registry/src`: every egui call below (egui 0.36.2: `Sides`, `Panel::left(..).exact_size(..).show(ui, ..)`, `CentralPanel::no_frame`, `ComboBox::from_id_salt/selected_text/show_ui`, `Ui::selectable_value/selectable_label/menu_button/close/small/allocate_exact_size/is_rect_visible`, `Context::animate_bool_responsive`, `Painter::rect/circle`, `Frame::new/group/fill/stroke/corner_radius/inner_margin`, `Margin::symmetric/same`, `CollapsingHeader::id_salt/default_open`, `text::LayoutJob::append`, `TextFormat { background, .. }`, `LayoutSection::byte_range: Range<ByteIndex>`, `Response::labelled_by/widget_info/gained_focus`, `WidgetInfo::selected/labeled`, `Button::small/fill/stroke/frame`), kittest 0.4.0 (`get_by_role_and_label`, `get_all_by_role`, `Node::value`), egui-phosphor 0.14.0 (`ROBOT`, `X`), and toml_edit 0.25.15. The `rules::edit` merge and append code was run in a scratch crate against toml 1.1.6 and toml_edit 0.25.15; its outputs are the expected strings in Task 1. Not run: the GUI code as a whole, and in particular whether kittest opens the "+ Add action" `menu_button` popup with one `click` + `run` (Task 3, Step 1, last test) and whether `widget_info` after `ui.add(Button)` replaces the button's accessible name (Task 4); if either fails, the step says what to do.

## Review Focus

1. **A hand-written rules.toml** (a comment block above a rule, comments at line ends, `[rules.match]` header tables, a file holding only comments): Save changes only the edited rule's changed values; every other byte stays. Tests in Task 1.
2. **Saving or renaming onto another rule's name**: refused, the file unchanged, the reason shown in the editor. Tests in Task 1 and Task 3.
3. **The open rule vanished from the file** (rejected with the CLI, renamed in a text editor) before Save: Save reports "no such rule" and does not add it back. Tests in Task 1 and Task 3.
4. **A proposal carrying control characters** (an ANSI escape in a condition value or in `proposed_by`): drawn without them in the card's summary, chip and TOML. Test in Task 2.
5. **The list switch flipped while that rule is open in the editor**: a later Save keeps the new on/off state, not the editor's stale one. Test in Task 3.

---

### Task 1: `save_rule` and `remove_rule` in `rules::edit`

**Files:**
- Modify: `src/rules/edit.rs` (new public functions, `reject` and `edit` share helpers, tests)

**Interfaces:**
- Produces:
  - `pub fn save_rule(path: &Path, replacing: Option<&str>, rule: &Rule) -> Result<(), RulesError>`: `None` appends `rule`; `Some(name)` replaces the rule named `name` in place (which may rename it). Errors with `RulesError::Invalid { reason: "no such rule", .. }` when `name` is gone, and with whatever `rules::compile` reports (duplicate name, invalid regex, empty actions) without touching the file.
  - `pub fn remove_rule(path: &Path, name: &str) -> Result<(), RulesError>`: removes any rule by name, keeping the comments above it the way `reject` does.

A new rule is written in the docs' style: `match.subject = { contains = "x" }` dotted keys with inline tables, `actions` last, and no `enabled = true` (the default). A replaced rule keeps its table's comments, each unchanged value as written, and the comment on a changed value's line.

- [ ] **Step 1: Write the failing tests** in the `tests` module of `src/rules/edit.rs`

```rust
    fn rule(name: &str, subject: &str) -> Rule {
        serde_json::from_str(&format!(
            r#"{{"name": "{name}", "match": {{"subject": {{"contains": "{subject}"}}}}, "actions": ["delete"]}}"#
        ))
        .unwrap()
    }

    const TWO: &str = "# my rules\n\n# codes go after an hour\n[[rules]]\nname = \"codes\" # sign-in codes\nmatch.subject = { contains = \"code\" } # any code\nmatch.seen = true\nactions = [\"delete\"]\n\n[[rules]]\nname = \"keep\"   # do not touch\nmatch.seen = true\nactions = [\"flag\"]\n";

    const KEEP: &str = "\n[[rules]]\nname = \"keep\"   # do not touch\nmatch.seen = true\nactions = [\"flag\"]\n";

    #[test]
    fn save_rule_appends_a_new_rule_in_the_docs_style() {
        let (_dir, path) = rules_file(HUMAN);
        save_rule(&path, None, &rule("codes", "code")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!(
                "{HUMAN}\n[[rules]]\nname = \"codes\"\nmatch.subject = {{ contains = \"code\" }}\nactions = [\"delete\"]\n"
            )
        );
    }

    #[test]
    fn save_rule_into_a_missing_or_comment_only_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        save_rule(&path, None, &rule("codes", "code")).unwrap();
        assert_eq!(load(&path).unwrap().rules.len(), 1);

        let (_dir, path) = rules_file("# my rules\n");
        save_rule(&path, None, &rule("codes", "code")).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# my rules\n\n[[rules]]\nname = \"codes\"\n"), "{text}");
    }

    #[test]
    fn save_rule_replaces_in_place_and_keeps_comments_and_unchanged_lines() {
        let (_dir, path) = rules_file(TWO);
        let mut changed = rule("codes", "verification code");
        changed.matches.seen = Some(true);
        save_rule(&path, Some("codes"), &changed).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            text,
            TWO.replace("{ contains = \"code\" }", "{ contains = \"verification code\" }")
        );
        assert!(text.ends_with(KEEP), "{text}");
    }

    #[test]
    fn save_rule_edits_header_style_tables_without_restyling_them() {
        let headers = "[[rules]]\nname = \"codes\"\nactions = [\"delete\"]\n\n[rules.match]\nseen = true # after reading\n\n[rules.match.subject]\ncontains = \"code\"\n";
        let (_dir, path) = rules_file(headers);
        let mut changed = rule("codes", "code");
        changed.matches.seen = Some(false);
        save_rule(&path, Some("codes"), &changed).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            headers.replace("seen = true", "seen = false")
        );
    }

    #[test]
    fn save_rule_renames_in_place() {
        let (_dir, path) = rules_file(TWO);
        let mut renamed = rule("sign-in codes", "code");
        renamed.matches.seen = Some(true);
        save_rule(&path, Some("codes"), &renamed).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("name = \"sign-in codes\" # sign-in codes\n"), "{text}");
        let names: Vec<String> = load(&path).unwrap().rules.into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["sign-in codes", "keep"]);
    }

    #[test]
    fn save_rule_refuses_a_taken_name_a_vanished_rule_and_bad_rules() {
        let (_dir, path) = rules_file(TWO);
        assert!(save_rule(&path, None, &rule("keep", "x")).is_err(), "new rule, taken name");
        assert!(save_rule(&path, Some("codes"), &rule("keep", "x")).is_err(), "rename onto a taken name");
        match save_rule(&path, Some("gone"), &rule("gone", "x")) {
            Err(RulesError::Invalid { reason, .. }) => assert_eq!(reason, "no such rule"),
            other => panic!("expected no such rule, got {other:?}"),
        }
        let mut bad = rule("codes", "x");
        bad.matches.subject = Some(TextMatch {
            regex: Some("(".into()),
            ..Default::default()
        });
        assert!(save_rule(&path, Some("codes"), &bad).is_err(), "invalid regex");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TWO);
    }

    #[test]
    fn save_rule_refuses_an_inline_rules_array() {
        let (_dir, path) = rules_file("rules = []\n");
        assert!(save_rule(&path, None, &rule("codes", "code")).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "rules = []\n");
    }

    #[test]
    fn remove_rule_removes_any_rule_and_keeps_the_comments_above_it() {
        let (_dir, path) = rules_file(TWO);
        remove_rule(&path, "codes").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("# my rules\n\n# codes go after an hour\n{KEEP}")
        );
        assert!(remove_rule(&path, "codes").is_err());
    }
```

- [ ] **Step 2: Run them**

Run: `cargo test --lib rules::edit`
Expected: FAIL to compile (`save_rule`, `remove_rule` not found).

- [ ] **Step 3: Implement** in `src/rules/edit.rs`

Imports become:

```rust
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value};
```

Below `reject`, the new public functions:

```rust
/// Adds `rule`, or replaces the rule named `replacing` with it in place. Keeps the file's comments, the other rules as
/// written, and each unchanged line of the replaced rule.
pub fn save_rule(path: &Path, replacing: Option<&str>, rule: &Rule) -> Result<(), RulesError> {
    let new = rule_table(rule)?;
    let Some(name) = replacing else {
        let _lock = lock(path)?;
        let text = read(path)?;
        let mut doc = parse_doc(&text)?;
        append(&mut doc, new, !text.trim().is_empty())?;
        return save(path, &doc.to_string());
    };
    edit(path, name, |rules, index| {
        let old = rules
            .get_mut(index)
            .ok_or_else(|| invalid(name, "no such rule"))?;
        merge(old, &new);
        Ok(None)
    })
}

/// Removes the rule named `name`, keeping the comments above it.
pub fn remove_rule(path: &Path, name: &str) -> Result<(), RulesError> {
    edit(path, name, |rules, index| Ok(remove_at(rules, index)))
}
```

`reject` keeps its checks and hands the removal to a shared helper:

```rust
pub fn reject(path: &Path, name: &str) -> Result<(), RulesError> {
    edit(path, name, |rules, index| {
        let table = rules
            .get(index)
            .ok_or_else(|| invalid(name, "no such rule"))?;
        if !is_disabled(table) || !table.contains_key("proposed_by") {
            return Err(invalid(
                name,
                "is not a pending proposal; edit rules.toml to remove it",
            ));
        }
        Ok(remove_at(rules, index))
    })
}

/// Removes the table at `index`. Comments above a table belong to its decor, so they move to the next table, or are
/// returned for the document's end when it was the last; the blank line `propose` adds is dropped.
fn remove_at(rules: &mut ArrayOfTables, index: usize) -> Option<String> {
    let prefix = rules
        .get(index)
        .and_then(|table| table.decor().prefix())
        .and_then(|p| p.as_str())
        .unwrap_or("");
    let mut orphaned = prefix
        .strip_suffix('\n')
        .filter(|p| p.ends_with('\n'))
        .unwrap_or(prefix)
        .to_string();
    rules.remove(index);
    match rules.get_mut(index) {
        Some(next) => {
            let next_prefix = next.decor().prefix().and_then(|p| p.as_str()).unwrap_or("");
            orphaned.push_str(next_prefix);
            next.decor_mut().set_prefix(orphaned);
            None
        }
        None => Some(orphaned),
    }
}
```

`edit` parses through the new `parse_doc`:

```rust
    let mut doc = parse_doc(&read(path)?)?;
```

and the helpers, beside `save` and `read`:

```rust
fn parse_doc(text: &str) -> Result<DocumentMut, RulesError> {
    text.parse()
        .map_err(|e: toml_edit::TomlError| RulesError::Parse(e.to_string()))
}

/// `rule` as a `[[rules]]` table in the docs' style: `match.from = { … }` dotted keys with inline tables, `actions`
/// last, and no `enabled = true`.
fn rule_table(rule: &Rule) -> Result<Table, RulesError> {
    let text = toml::to_string(&RuleFile {
        rules: vec![rule.clone()],
    })
    .map_err(|e| RulesError::Parse(e.to_string()))?;
    let mut table = parse_doc(&text)?
        .remove("rules")
        .and_then(|item| item.into_array_of_tables().ok())
        .filter(|rules| !rules.is_empty())
        .map(|mut rules| rules.remove(0))
        .ok_or_else(|| invalid(&rule.name, "could not be written as TOML"))?;
    if let Some(Item::Table(conditions)) = table.get_mut("match") {
        conditions.set_dotted(true);
        for (_, item) in conditions.iter_mut() {
            *item = match std::mem::take(item).into_value() {
                Ok(value) => Item::Value(value),
                Err(other) => other,
            };
        }
        conditions.fmt();
    }
    if let Some(actions) = table.remove("actions") {
        table.insert("actions", actions);
    }
    if rule.enabled {
        table.remove("enabled");
    }
    Ok(table)
}

/// Adds `table` after the last rule and after any comments that end the file, a blank line apart from what is above.
fn append(doc: &mut DocumentMut, mut table: Table, after_text: bool) -> Result<(), RulesError> {
    let trailing = doc.trailing().as_str().unwrap_or("").to_string();
    doc.set_trailing("");
    let gap = if after_text && !trailing.ends_with("\n\n") {
        "\n"
    } else {
        ""
    };
    table.decor_mut().set_prefix(format!("{trailing}{gap}"));
    match doc.get_mut("rules") {
        None => {
            let mut rules = ArrayOfTables::new();
            rules.push(table);
            doc.insert("rules", Item::ArrayOfTables(rules));
        }
        Some(item) => item
            .as_array_of_tables_mut()
            .ok_or_else(|| {
                RulesError::Parse("`rules` is an inline array; add the rule there by hand".into())
            })?
            .push(table),
    }
    Ok(())
}

/// Makes `old` hold `new`'s keys and values: keys `new` lacks go, keys it adds come last, an unchanged value stays as
/// written, and a changed value keeps the old one's comment.
fn merge(old: &mut Table, new: &Table) {
    let gone: Vec<String> = old
        .iter()
        .map(|(key, _)| key.to_string())
        .filter(|key| !new.contains_key(key))
        .collect();
    for key in gone {
        old.remove(&key);
    }
    for (key, item) in new.iter() {
        match (old.get_mut(key), item) {
            (Some(Item::Table(old_table)), Item::Table(new_table)) => merge(old_table, new_table),
            (Some(old_item), new_item) if same_value(old_item, new_item) => {}
            (Some(old_item), new_item) => {
                let decor = old_item.as_value().map(|value| value.decor().clone());
                *old_item = new_item.clone();
                if let (Some(decor), Some(value)) = (decor, old_item.as_value_mut()) {
                    *value.decor_mut() = decor;
                }
            }
            (None, new_item) => {
                old.insert(key, new_item.clone());
            }
        }
    }
}

/// Whether two items hold the same value however each is written: a header table and an inline table compare equal.
fn same_value(a: &Item, b: &Item) -> bool {
    let plain = |item: &Item| {
        let mut value = item.clone().into_value().ok()?;
        match &mut value {
            Value::InlineTable(table) => table.fmt(),
            Value::Array(array) => array.fmt(),
            _ => {}
        }
        value.decor_mut().clear();
        Some(value.to_string())
    };
    plain(a).is_some_and(|text| Some(text) == plain(b))
}
```

Update the module doc: "The only code that writes rules.toml: proposals, approvals, rejections, and rules saved or removed by the window. Each edit is validated as a whole file before it replaces the old one."

- [ ] **Step 4: Run the edit tests and the architecture check**

Run: `cargo test --lib rules::edit && cargo test --test architecture`
Expected: PASS, including the existing `reject_*` tests, which now run through `remove_at`.

- [ ] **Step 5: Commit**

```bash
git add src/rules/edit.rs
git commit -m "feat(rules): save or remove one rule in rules.toml, keeping comments"
```

---

### Task 2: Rules list and proposal cards

**Files:**
- Create: `src/gui/draft.rs` (conditions as rows, summaries, preview line), `src/gui/widgets.rs` (switch, chip)
- Modify: `src/gui/mod.rs` (`mod draft; mod widgets;`), `src/gui/theme.rs` (`added`, `agent`, `agent_bg`), `src/gui/icons.rs` (`PROPOSAL`), `src/gui/rules.rs` (`show_rules`, TOML highlighting, tests), `src/gui/app.rs` (`config`, `previews`, `UiAction::PreviewRule`, `preview`), `src/gui/test_support.rs` (`Fixture::rules_view`), `src/gui/snapshots.rs` (none new; `gui_rules` regenerates), `docs/src/gui.md`

**Interfaces:**
- Consumes: `icons` module (GUI redesign Task 2); Latte colours (GUI redesign Task 1).
- Produces:

```rust
// draft.rs
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Field { From, To, Cc, Subject, Body, Header, OlderThan, Seen, ToMe, Alias }
impl Field { pub fn label(self) -> &'static str; pub fn is_text(self) -> bool; }
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Op { Contains, Equals, Regex }
impl Op { pub fn label(self) -> &'static str; }
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Condition { pub field: Field, pub header: String, pub op: Op, pub value: String }
pub(crate) fn conditions(m: &Match) -> Vec<Condition>;
pub(crate) fn condition_label(condition: &Condition) -> String;
pub(crate) fn action_label(action: &Action) -> String;
pub(crate) fn summary(rule: &Rule) -> String;          // `subject contains "code" → delete`, cleaned
pub(crate) fn preview_text(rule: &Rule, matched: usize) -> String;
// widgets.rs
pub(crate) fn switch(ui: &mut egui::Ui, on: bool, label: &str) -> egui::Response;
pub(crate) fn chip(fill: egui::Color32, stroke: egui::Color32) -> egui::Frame;
// theme.rs: Palette gains `added`, `agent`, `agent_bg`
// icons.rs
pub(crate) const PROPOSAL: &str;                      // ph::ROBOT
// rules.rs
pub(crate) enum Token { Bool, Header, Key, Plain, Text }
pub(crate) fn toml_tokens(text: &str) -> Vec<(Token, &str)>;
pub(crate) fn toml_job(text: &str, palette: &Palette, font: egui::FontId) -> egui::text::LayoutJob;
pub(crate) fn counts(rules: usize, proposals: usize) -> String;
// app.rs
App.config: Config; App.previews: BTreeMap<String, String>;
UiAction::PreviewRule(String);
fn App::preview(&self, rule: Rule) -> String;          // private; Task 3 calls it for Test
// test_support.rs
Fixture::rules_view(&self, rules: &str) -> (Harness<'static, App>, Wires);
```

Layout: a header row ("Rules", "1 rule · 1 proposal" on the left; "Open rules.toml" on the right), the parse-error banner, then a 460 px left panel with proposal cards first and rule rows below, and the rest of the view for the editor (a placeholder line until Task 3).

- [ ] **Step 1: Write the failing tests**

`src/gui/draft.rs` (new file, tests only for now):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::parse;

    const ALL_FIELDS: &str = r#"
[[rules]]
name = "everything"
match.from = { equals = "a@example.com" }
match.to = { contains = "team" }
match.cc = { regex = "(?i)boss" }
match.subject = { contains = "code" }
match.body = { regex = "\\d{6}" }
match.header = { name = "List-Id", contains = "github.com" }
match.older_than = "1h"
match.seen = true
match.to_me = false
match.alias = "*@shop.example.com"
actions = [{ move = "Codes" }, "mark_read", "flag", "silent"]
"#;

    fn rule(text: &str) -> Rule {
        parse(text).unwrap().rules.remove(0)
    }

    #[test]
    fn every_condition_becomes_a_row_in_docs_order() {
        let rows = conditions(&rule(ALL_FIELDS).matches);
        let fields: Vec<Field> = rows.iter().map(|c| c.field).collect();
        assert_eq!(
            fields,
            [
                Field::From,
                Field::To,
                Field::Cc,
                Field::Subject,
                Field::Body,
                Field::Header,
                Field::OlderThan,
                Field::Seen,
                Field::ToMe,
                Field::Alias
            ]
        );
        assert_eq!((rows[0].op, rows[0].value.as_str()), (Op::Equals, "a@example.com"));
        assert_eq!((rows[5].header.as_str(), rows[5].op), ("List-Id", Op::Contains));
    }

    #[test]
    fn the_summary_reads_conditions_then_actions() {
        assert_eq!(
            summary(&rule(
                "[[rules]]\nname = \"x\"\nmatch.subject = { contains = \"verification code\" }\nactions = [\"delete\"]\n"
            )),
            "subject contains \"verification code\" → delete"
        );
        assert_eq!(
            summary(&rule(ALL_FIELDS)),
            "from equals \"a@example.com\", to contains \"team\", cc matches regex \"(?i)boss\", \
             subject contains \"code\", body matches regex \"\\d{6}\", header List-Id contains \"github.com\", \
             older than 1h, read, not to me, alias *@shop.example.com → move to Codes, mark read, flag, silent"
        );
    }

    #[test]
    fn the_summary_drops_control_characters() {
        let sneaky = rule("[[rules]]\nname = \"x\"\nmatch.subject = { contains = \"\\u001b[2Jcode\" }\nactions = [\"delete\"]\n");
        assert_eq!(summary(&sneaky), "subject contains \"[2Jcode\" → delete");
    }

    #[test]
    fn the_preview_line_counts_messages_and_names_the_actions() {
        let delete = rule("[[rules]]\nname = \"x\"\nmatch.seen = true\nactions = [\"delete\"]\n");
        assert_eq!(preview_text(&delete, 4), "would delete 4 cached messages");
        assert_eq!(preview_text(&delete, 1), "would delete 1 cached message");
        assert_eq!(preview_text(&delete, 0), "matches no cached message");
        let filed = rule(
            "[[rules]]\nname = \"x\"\nmatch.seen = true\nactions = [{ move = \"Lists/GitHub\" }, \"mark_read\"]\n",
        );
        assert_eq!(preview_text(&filed, 3), "would move and mark read 3 cached messages to Lists/GitHub");
        let body = rule("[[rules]]\nname = \"x\"\nmatch.body = { contains = \"code\" }\nactions = [\"flag\"]\n");
        assert_eq!(
            preview_text(&body, 2),
            "would flag 2 cached messages; only mail whose body is downloaded is checked"
        );
    }
}
```

`src/gui/widgets.rs` (new file, tests only for now):

```rust
#[cfg(test)]
mod tests {
    use eframe::egui;
    use egui_kittest::kittest::Queryable;

    use super::*;

    #[test]
    fn the_switch_is_a_named_checkbox_that_reports_clicks() {
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, on: &mut bool| {
                if switch(ui, *on, "Enable codes").clicked() {
                    *on = !*on;
                }
            },
            false,
        );
        harness.run();
        harness
            .get_by_role_and_label(egui::accesskit::Role::CheckBox, "Enable codes")
            .click();
        harness.run();
        assert!(*harness.state());
    }
}
```

`src/gui/theme.rs` tests:

```rust
    #[test]
    fn proposal_chip_toml_block_and_delete_chip_pass_aa_in_both_themes() {
        for (name, p) in [("mocha", &MOCHA), ("latte", &LATTE)] {
            let chip = contrast(p.agent, p.agent_bg);
            assert!(chip >= 4.5, "{name}: proposed-by chip {chip}");
            for (what, colour) in [
                ("headers", p.agent),
                ("keys", p.accent),
                ("strings and gutter", p.success),
                ("booleans", p.error),
                ("text", p.text),
            ] {
                let ratio = contrast(colour, p.added);
                assert!(ratio >= 4.5, "{name}: TOML {what} {ratio}");
            }
            let delete = contrast(p.error, p.background);
            assert!(delete >= 4.5, "{name}: delete chip {delete}");
        }
    }
```

`src/gui/rules.rs` tests: replace the private `rules_view` helper with `fx.rules_view(RULES)` everywhere. The test module's imports gain `use super::*;` and `use crate::gui::theme::MOCHA;`, and `use crate::gui::test_support::Fixture;` becomes `use crate::gui::test_support::{Fixture, message};`. Then change the first two tests and add the rest:

```rust
    #[test]
    fn a_proposal_shows_its_summary_and_toml_and_approve_enables_it() {
        let fx = Fixture::new(&["work"]);
        // A clock left from when the rule ran before must not let it act on older mail.
        fx.store("work").restart_rule_clock("codes", 1).unwrap();
        let (mut harness, wires) = fx.rules_view(RULES);
        assert!(harness.query_by_label("proposed by agent").is_some());
        assert!(harness.query_by_label("subject contains \"code\" → delete").is_some());
        assert!(harness.query_by_label_contains("+ name = \"codes\"").is_some());
        harness.get_by_label("Approve").click();
        harness.run();
        assert!(enabled(&fx, "codes"));
        assert!(fx.store("work").rule_first_seen("codes", 0).unwrap() > 1);
        assert!(std::fs::read_to_string(fx.paths.rules_file()).unwrap().starts_with("# keep me\n"));
        assert!(wires.sent().is_empty());
        assert!(harness.query_by_label("Approve").is_none());
    }

    #[test]
    fn the_switch_turns_a_rule_off_and_keeps_comments() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, wires) = fx.rules_view(RULES);
        harness.get_by_label("Enable newsletters").click();
        harness.run();
        assert!(!enabled(&fx, "newsletters"));
        assert!(std::fs::read_to_string(fx.paths.rules_file()).unwrap().starts_with("# keep me\n"));
        assert!(wires.sent().is_empty());
    }

    #[test]
    fn the_header_counts_rules_and_proposals_and_rows_say_what_they_do() {
        assert_eq!(counts(1, 2), "1 rule · 2 proposals");
        assert_eq!(counts(0, 1), "0 rules · 1 proposal");
        let fx = Fixture::new(&["work"]);
        let (harness, _wires) = fx.rules_view(RULES);
        assert!(harness.query_by_label("1 rule · 1 proposal").is_some());
        assert!(harness.query_by_label("every account · INBOX").is_some());
        assert!(harness.query_by_label("from contains \"news@\" → archive").is_some());
    }

    #[test]
    fn preview_counts_what_a_proposal_would_do_to_cached_mail() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 1, "your code is 1234"));
        fx.add("work", message("INBOX", 2, "lunch"));
        let (mut harness, wires) = fx.rules_view(RULES);
        harness.get_by_label("Preview on cached mail").click();
        harness.run();
        assert!(harness.query_by_label("would delete 1 cached message").is_some());
        assert!(wires.sent().is_empty());
        assert!(!enabled(&fx, "codes"), "a preview changes nothing");
    }

    #[test]
    fn agent_text_is_drawn_without_control_characters() {
        let sneaky = "[[rules]]\nname = \"codes\"\nenabled = false\nproposed_by = \"age\\u001bnt\"\nmatch.subject = { contains = \"\\u001b[2Jcode\" }\nactions = [\"delete\"]\n";
        let fx = Fixture::new(&["work"]);
        let (harness, _wires) = fx.rules_view(sneaky);
        assert!(harness.query_by_label("proposed by agent").is_some());
        assert!(harness.query_by_label("subject contains \"[2Jcode\" → delete").is_some());
        assert!(harness.query_by_label_contains("\u{1b}").is_none());
    }

    #[test]
    fn toml_tokens_mark_headers_keys_strings_and_booleans() {
        let text = "[[rules]]\nname = \"codes\"\nenabled = false\nactions = [{ move = 'Lists' }, \"delete\"]\n\n[rules.match.body]\nregex = '''a'b'''\n";
        let coloured: Vec<(Token, &str)> = toml_tokens(text)
            .into_iter()
            .filter(|(token, _)| *token != Token::Plain)
            .collect();
        assert_eq!(
            coloured,
            [
                (Token::Header, "[[rules]]"),
                (Token::Key, "name"),
                (Token::Text, "\"codes\""),
                (Token::Key, "enabled"),
                (Token::Bool, "false"),
                (Token::Key, "actions"),
                (Token::Key, "move"),
                (Token::Text, "'Lists'"),
                (Token::Text, "\"delete\""),
                (Token::Header, "[rules.match.body]"),
                (Token::Key, "regex"),
                (Token::Text, "'''a'b'''"),
            ]
        );
        assert_eq!(
            toml_tokens("a = \"x\\\"y\" true"),
            [
                (Token::Key, "a"),
                (Token::Plain, " "),
                (Token::Plain, "="),
                (Token::Plain, " "),
                (Token::Text, "\"x\\\"y\""),
                (Token::Plain, " "),
                (Token::Bool, "true"),
            ]
        );
    }

    #[test]
    fn every_line_of_the_added_block_has_a_plus_gutter_and_toml_colours() {
        let job = toml_job(
            "[[rules]]\nname = \"codes\"\n\n[rules.match]\nseen = true\n",
            &MOCHA,
            egui::FontId::monospace(12.0),
        );
        assert!(job.text.lines().all(|line| line.starts_with("+ ")), "{}", job.text);
        let colour_of = |needle: &str| {
            job.sections
                .iter()
                .find(|s| &job.text[s.byte_range.start.0..s.byte_range.end.0] == needle)
                .map(|s| s.format.color)
        };
        assert_eq!(colour_of("[[rules]]"), Some(MOCHA.agent));
        assert_eq!(colour_of("name"), Some(MOCHA.accent));
        assert_eq!(colour_of("\"codes\""), Some(MOCHA.success));
        assert_eq!(colour_of("true"), Some(MOCHA.error));
    }
```

`src/gui/test_support.rs`:

```rust
    /// The app over this home with `rules` as rules.toml, showing the Rules view.
    pub fn rules_view(&self, rules: &str) -> (Harness<'static, App>, Wires) {
        std::fs::write(self.paths.rules_file(), rules).unwrap();
        let (mut harness, wires) = self.harness();
        harness.state_mut().select_view(View::Rules);
        harness.run();
        (harness, wires)
    }
```

with `use super::app::{View, session};`.

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::`
Expected: FAIL to compile (`conditions`, `summary`, `switch`, `agent`, `toml_tokens`, `counts`, … not found).

- [ ] **Step 3: Implement `draft.rs`** above its tests

```rust
//! Rules as rows of conditions, and the plain-language lines the Rules view shows for them.
use crate::message::clean;
use crate::rules::{Action, Match, Rule};

/// A condition's key in rules.toml.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Field {
    From,
    To,
    Cc,
    Subject,
    Body,
    Header,
    OlderThan,
    Seen,
    ToMe,
    Alias,
}

impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Field::Alias => "alias",
            Field::Body => "body",
            Field::Cc => "cc",
            Field::From => "from",
            Field::Header => "header",
            Field::OlderThan => "older than",
            Field::Seen => "seen",
            Field::Subject => "subject",
            Field::To => "to",
            Field::ToMe => "to me",
        }
    }

    /// Fields that take contains, equals or regex.
    pub fn is_text(self) -> bool {
        matches!(
            self,
            Field::Body | Field::Cc | Field::From | Field::Header | Field::Subject | Field::To
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Op {
    Contains,
    Equals,
    Regex,
}

impl Op {
    pub fn label(self) -> &'static str {
        match self {
            Op::Contains => "contains",
            Op::Equals => "equals",
            Op::Regex => "matches regex",
        }
    }
}

/// One row of a rule's conditions.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Condition {
    pub field: Field,
    /// The header's name; used by `Field::Header` only.
    pub header: String,
    /// Used by text fields only.
    pub op: Op,
    /// The text, duration or alias; "true" or "false" for seen and to me.
    pub value: String,
}

/// A rule's conditions, in the order of the rules docs. A text condition with more than one operator (which does not
/// compile) shows the first.
pub(crate) fn conditions(m: &Match) -> Vec<Condition> {
    let text = |field, header: &str, contains: &Option<String>, equals: &Option<String>, regex: &Option<String>| {
        let (op, value) = match (contains, equals, regex) {
            (Some(value), _, _) => (Op::Contains, value.clone()),
            (None, Some(value), _) => (Op::Equals, value.clone()),
            (None, None, value) => (Op::Regex, value.clone().unwrap_or_default()),
        };
        Condition {
            field,
            header: header.to_string(),
            op,
            value,
        }
    };
    let plain = |field, value: String| Condition {
        field,
        header: String::new(),
        op: Op::Contains,
        value,
    };
    let mut rows = Vec::new();
    for (field, condition) in [
        (Field::From, &m.from),
        (Field::To, &m.to),
        (Field::Cc, &m.cc),
        (Field::Subject, &m.subject),
        (Field::Body, &m.body),
    ] {
        if let Some(c) = condition {
            rows.push(text(field, "", &c.contains, &c.equals, &c.regex));
        }
    }
    if let Some(h) = &m.header {
        rows.push(text(Field::Header, &h.name, &h.contains, &h.equals, &h.regex));
    }
    rows.extend(m.older_than.clone().map(|v| plain(Field::OlderThan, v)));
    rows.extend(m.seen.map(|v| plain(Field::Seen, v.to_string())));
    rows.extend(m.to_me.map(|v| plain(Field::ToMe, v.to_string())));
    rows.extend(m.alias.clone().map(|v| plain(Field::Alias, v)));
    rows
}

pub(crate) fn condition_label(c: &Condition) -> String {
    let no = c.value == "false";
    match c.field {
        Field::Alias => format!("alias {}", c.value),
        Field::Header => format!("header {} {} \"{}\"", c.header, c.op.label(), c.value),
        Field::OlderThan => format!("older than {}", c.value),
        Field::Seen => (if no { "unread" } else { "read" }).into(),
        Field::ToMe => (if no { "not to me" } else { "to me" }).into(),
        Field::Body | Field::Cc | Field::From | Field::Subject | Field::To => {
            format!("{} {} \"{}\"", c.field.label(), c.op.label(), c.value)
        }
    }
}

pub(crate) fn action_label(action: &Action) -> String {
    match action {
        Action::Archive => "archive".into(),
        Action::Delete => "delete".into(),
        Action::Flag => "flag".into(),
        Action::MarkRead => "mark read".into(),
        Action::Move(folder) => format!("move to {folder}"),
        Action::Notify => "notify".into(),
        Action::Silent => "silent".into(),
        // The CLI-only actions never parse from rules.toml.
        other => other.label(),
    }
}

/// `subject contains "code" → delete`: what a rule matches and what it does, one line, without control characters.
/// Draw it monospace: the proportional font has no "→".
pub(crate) fn summary(rule: &Rule) -> String {
    let conditions: Vec<String> = conditions(&rule.matches).iter().map(condition_label).collect();
    let actions: Vec<String> = rule.actions.iter().map(action_label).collect();
    clean(&format!("{} → {}", conditions.join(", "), actions.join(", ")), false)
}

/// "would delete 4 cached messages": what a preview on cached mail found.
pub(crate) fn preview_text(rule: &Rule, matched: usize) -> String {
    if matched == 0 {
        return "matches no cached message".into();
    }
    let verbs: Vec<String> = rule
        .actions
        .iter()
        .map(|action| match action {
            Action::Move(_) => "move".into(),
            Action::Notify => "notify about".into(),
            Action::Silent => "silence".into(),
            other => action_label(other),
        })
        .collect();
    let target = rule
        .actions
        .iter()
        .find_map(|action| match action {
            Action::Move(folder) => Some(format!(" to {folder}")),
            _ => None,
        })
        .unwrap_or_default();
    let noun = if matched == 1 { "message" } else { "messages" };
    let body = if rule.matches.body.is_some() {
        "; only mail whose body is downloaded is checked"
    } else {
        ""
    };
    clean(
        &format!("would {} {matched} cached {noun}{target}{body}", verbs.join(" and ")),
        false,
    )
}
```

- [ ] **Step 4: Implement `widgets.rs`, the palette and the icon**

`src/gui/widgets.rs` above its tests:

```rust
//! Small widgets the views share.
use eframe::egui::{self, Color32};

use super::theme;

/// An on/off switch; `label` names it for screen readers and tests. Returns the response; the caller acts on a click.
pub(crate) fn switch(ui: &mut egui::Ui, on: bool, label: &str) -> egui::Response {
    let size = ui.spacing().interact_size.y * egui::vec2(1.8, 0.9);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), on, label)
    });
    if ui.is_rect_visible(rect) {
        let palette = theme::palette(ui);
        let how_on = ui.ctx().animate_bool_responsive(response.id, on);
        let radius = 0.5 * rect.height();
        let track = if on { palette.accent } else { palette.border };
        ui.painter()
            .rect(rect, radius, track, egui::Stroke::NONE, egui::StrokeKind::Inside);
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
        let knob = if on { palette.on_accent } else { palette.background };
        ui.painter()
            .circle(egui::pos2(x, rect.center().y), 0.75 * radius, knob, egui::Stroke::NONE);
    }
    response
}

/// The rounded frame of a chip, such as "every account · INBOX" or "proposed by agent".
pub(crate) fn chip(fill: Color32, stroke: Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, stroke))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(6, 1))
}
```

`src/gui/theme.rs`: add to `Palette` (keeping the fields alphabetical)

```rust
    /// Behind a TOML block that would be added to rules.toml.
    pub added: Color32,
    /// Text of the "proposed by" chip and TOML section headers: a dark peach on light, peach on dark.
    pub agent: Color32,
    pub agent_bg: Color32,
```

with `added: hex(0x1f3324), agent: hex(0xfab387), agent_bg: hex(0x3e2e2c)` in `MOCHA` and `added: hex(0xeaf6e6), agent: hex(0xa34106), agent_bg: hex(0xfde6d6)` in `LATTE`.

`src/gui/icons.rs`: `pub(crate) const PROPOSAL: &str = ph::ROBOT;` (alphabetical among the constants).

`src/gui/mod.rs`: `mod draft;` and `mod widgets;` in the alphabetical module list.

- [ ] **Step 5: Implement the view** in `src/gui/rules.rs`

Imports:

```rust
use super::app::{Account, App, UiAction};
use super::draft;
use super::theme::{self, Palette};
use super::{icons, widgets};

/// Width of the rule list; the editor takes the rest.
const LIST_WIDTH: f32 = 460.0;
```

Replace `show_rules` (keep `RulesState`, `is_pending`, `toml_source` and everything below `show_rules` as is):

```rust
pub(crate) fn show_rules(app: &App, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let (pending, rules): (Vec<&Rule>, Vec<&Rule>) =
        app.rules.rules.iter().partition(|rule| is_pending(rule));
    let (_, header) = egui::Sides::new().show(
        ui,
        |ui| {
            ui.heading("Rules");
            ui.weak(counts(rules.len(), pending.len()));
        },
        |ui| {
            let mut clicked = Vec::new();
            if ui.button("Open rules.toml").clicked() {
                clicked.push(UiAction::OpenRulesFile);
            }
            clicked
        },
    );
    actions.extend(header);
    if let Some(error) = &app.rules.error {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!(
                "{} — sync keeps the previous rules until this is fixed",
                clean(error, false)
            ),
        );
    }
    egui::Panel::left("rule_list")
        .resizable(false)
        .exact_size(LIST_WIDTH)
        .show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                for rule in &pending {
                    actions.extend(proposal_card(app, rule, ui));
                }
                if app.rules.rules.is_empty() {
                    ui.weak("rules.toml has no rules yet.");
                }
                for rule in &rules {
                    actions.extend(rule_row(rule, ui));
                }
            });
        });
    egui::CentralPanel::no_frame().show(ui, |ui| {
        ui.weak("Select a rule to edit it.");
    });
    actions
}

/// "1 rule · 2 proposals".
pub(crate) fn counts(rules: usize, proposals: usize) -> String {
    let count = |n: usize, one: &str| match n {
        1 => format!("1 {one}"),
        n => format!("{n} {one}s"),
    };
    format!("{} · {}", count(rules, "rule"), count(proposals, "proposal"))
}

/// "every account · INBOX": what a rule watches.
fn watches(rule: &Rule) -> String {
    let account = rule
        .account
        .as_deref()
        .map_or("every account".into(), |a| clean(a, false));
    let folder = rule
        .folder
        .as_deref()
        .map_or("INBOX".into(), |f| clean(f, false));
    format!("{account} · {folder}")
}

/// A rule: its switch, name, what it watches, and what it does.
fn rule_row(rule: &Rule, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let palette = theme::palette(ui);
    let name = clean(&rule.name, false);
    ui.horizontal(|ui| {
        if widgets::switch(ui, rule.enabled, &format!("Enable {name}")).clicked() {
            actions.push(UiAction::SetRuleEnabled(rule.name.clone(), !rule.enabled));
        }
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.strong(&name);
                widgets::chip(palette.background, palette.border)
                    .show(ui, |ui| ui.small(watches(rule)));
            });
            ui.add(egui::Label::new(egui::RichText::new(draft::summary(rule)).monospace()).truncate());
        });
    });
    ui.separator();
    actions
}

/// A pending proposal: who proposed it, what it does, the TOML it added, and Approve, Reject and Preview.
fn proposal_card(app: &App, rule: &Rule, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let palette = theme::palette(ui);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(icons::PROPOSAL).color(palette.agent));
            ui.strong(clean(&rule.name, false));
            let by = clean(rule.proposed_by.as_deref().unwrap_or_default(), false);
            widgets::chip(palette.agent_bg, palette.agent_bg).show(ui, |ui| {
                ui.label(egui::RichText::new(format!("proposed by {by}")).small().color(palette.agent))
            });
        });
        ui.label(egui::RichText::new(draft::summary(rule)).monospace());
        egui::CollapsingHeader::new("Adds to rules.toml")
            .id_salt(("proposal", &rule.name))
            .default_open(true)
            .show(ui, |ui| {
                let font = egui::TextStyle::Monospace.resolve(ui.style());
                egui::Frame::new()
                    .fill(palette.added)
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| ui.label(toml_job(&clean(&toml_source(rule), true), palette, font)));
            });
        ui.horizontal(|ui| {
            if ui.button("Approve").clicked() {
                actions.push(UiAction::ApproveRule(rule.name.clone()));
            }
            if ui.button("Reject").clicked() {
                actions.push(UiAction::RejectRule(rule.name.clone()));
            }
            if ui.button("Preview on cached mail").clicked() {
                actions.push(UiAction::PreviewRule(rule.name.clone()));
            }
        });
        if let Some(preview) = app.previews.get(&rule.name) {
            ui.label(clean(preview, false));
        }
    });
    ui.add_space(6.0);
    actions
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Token {
    Bool,
    Header,
    Key,
    Plain,
    Text,
}

/// The TOML `rules propose` writes, split for colouring: table headers, keys, strings and booleans. A scanner for that
/// output, not a TOML parser.
pub(crate) fn toml_tokens(text: &str) -> Vec<(Token, &str)> {
    let mut tokens = Vec::new();
    let mut at = 0;
    while let Some(c) = text[at..].chars().next() {
        let rest = &text[at..];
        let before = text[..at].trim_end_matches([' ', '\t']);
        let line_start = before.is_empty() || before.ends_with('\n');
        let word = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.');
        let (token, len) = if c == '[' && line_start {
            (Token::Header, rest.find('\n').unwrap_or(rest.len()))
        } else if c == '"' || c == '\'' {
            (Token::Text, string_len(rest, c))
        } else if word(c) {
            let len = rest.find(|c: char| !word(c)).unwrap_or(rest.len());
            let token = if rest[len..].trim_start_matches([' ', '\t']).starts_with('=') {
                Token::Key
            } else if matches!(&rest[..len], "true" | "false") {
                Token::Bool
            } else {
                Token::Plain
            };
            (token, len)
        } else {
            (Token::Plain, c.len_utf8())
        };
        tokens.push((token, &rest[..len]));
        at += len;
    }
    tokens
}

/// Bytes up to and including the closing quote of the string `text` starts with; all of `text` when it is not closed.
fn string_len(text: &str, quote: char) -> usize {
    let triple = if quote == '"' { "\"\"\"" } else { "'''" };
    if let Some(inner) = text.strip_prefix(triple) {
        return inner.find(triple).map_or(text.len(), |end| end + 6);
    }
    let mut escaped = false;
    for (at, c) in text.char_indices().skip(1) {
        if c == quote && !escaped {
            return at + 1;
        }
        escaped = quote == '"' && c == '\\' && !escaped;
    }
    text.len()
}

/// `text` as the added side of a diff: a "+" gutter on every line, and TOML colours.
pub(crate) fn toml_job(text: &str, palette: &Palette, font: egui::FontId) -> egui::text::LayoutJob {
    let format = |color| egui::TextFormat {
        font_id: font.clone(),
        color,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    job.append("+ ", 0.0, format(palette.success));
    for (token, part) in toml_tokens(text.trim_end()) {
        let color = match token {
            Token::Bool => palette.error,
            Token::Header => palette.agent,
            Token::Key => palette.accent,
            Token::Plain => palette.text,
            Token::Text => palette.success,
        };
        for (index, piece) in part.split('\n').enumerate() {
            if index > 0 {
                job.append("\n", 0.0, format(palette.text));
                job.append("+ ", 0.0, format(palette.success));
            }
            if !piece.is_empty() {
                job.append(piece, 0.0, format(color));
            }
        }
    }
    job
}
```

The test-module `RULES` constant and helpers `enabled`, `poll`, `touch` stay; delete the old `rules_view` function.

- [ ] **Step 6: Wire the preview** in `src/gui/app.rs`

Imports: `use crate::rules::{self as rule_file, Action, Rule, RuleFile, RulesError};` and `use super::draft;`.

Fields (alphabetical): `pub(crate) config: Config,` (in `App::new`: `config: config.clone(),`) and

```rust
    /// "Preview on cached mail" results by proposal name; cleared when rules.toml changes.
    pub(crate) previews: BTreeMap<String, String>,
```

(`previews: BTreeMap::new(),` in `App::new`). `UiAction` gains `PreviewRule(String),` and `apply` gains:

```rust
            UiAction::PreviewRule(name) => {
                if let Some(rule) = self.rules.rules.iter().find(|rule| rule.name == name) {
                    let text = self.preview(rule.clone());
                    self.previews.insert(name, text);
                }
            }
```

`rules_changed` ends with `self.previews.clear();`. Beside `edit_rules`:

```rust
    /// What `rule` would do to every account's cached mail if it were enabled, as `postbode rules test` shows it. Reads
    /// the stores only. ponytail: scans every cached message on the UI thread; move to a thread if large stores stall
    /// the window.
    fn preview(&self, mut rule: Rule) -> String {
        rule.enabled = true;
        let compiled = match rule_file::compile(&RuleFile {
            rules: vec![rule.clone()],
        }) {
            Ok(compiled) => compiled,
            Err(e) => return e.to_string(),
        };
        let mut matched = HashSet::new();
        for account in &self.accounts {
            let Ok(store) = &account.store else { continue };
            let Some(settings) = self.config.accounts.iter().find(|a| a.name == account.name) else {
                continue;
            };
            let planned = settings
                .identity()
                .map_err(|e| e.to_string())
                .and_then(|identity| {
                    crate::actions::planned(&compiled, store, settings, &identity, crate::time::now())
                        .map_err(|e| e.to_string())
                });
            match planned {
                Ok(planned) => matched.extend(
                    planned
                        .into_iter()
                        .map(|p| (account.name.clone(), p.message.folder, p.message.uid)),
                ),
                Err(e) => return format!("{}: {e}", account.name),
            }
        }
        draft::preview_text(&rule, matched.len())
    }
```

- [ ] **Step 7: Run the GUI tests and the architecture check**

Run: `cargo test --lib gui:: && cargo test --test architecture`
Expected: PASS. `an_edit_from_outside_reloads_the_view` and `a_parse_error_shows_a_banner_and_keeps_the_rules` still find their labels ("receipts", "newsletters") as row names.

- [ ] **Step 8: Docs** — in `docs/src/gui.md`, replace the sentence that starts with "**Rules** lists proposals" with:

```markdown
**Rules** lists proposals first, each with who proposed it, what it does in one line, the TOML it adds to `rules.toml`, and Approve, Reject and Preview on cached mail; then every rule with a switch, the account and folder it watches, and what it does. Changes to `rules.toml`, from the window or from your editor, take effect within about two seconds for new mail.
```

- [ ] **Step 9: Snapshots on Linux** (`gui_rules` changes), then commit

```bash
git add src/gui/ docs/src/gui.md tests/snapshots/
git commit -m "feat(gui): rules list with summaries, and proposal cards with their TOML and a preview"
```

---

### Task 3: Rule editor

**Files:**
- Create: `src/gui/rule_editor.rs`
- Modify: `src/gui/draft.rs` (`Draft`, `Field::ALL`, `is_flag`, `Op::ALL`, `Condition::new`, `with_field`, `action_name`), `src/gui/widgets.rs` (`icon_button`), `src/gui/icons.rs` (`REMOVE`), `src/gui/rules.rs` ("New rule", selectable rows, editor in place of the placeholder), `src/gui/app.rs` (`rule_draft`, actions), `src/gui/mod.rs` (`mod rule_editor;`), `src/gui/snapshots.rs` (`rule_editor`), `docs/src/gui.md`

**Interfaces:**
- Consumes: `rules::edit::{save_rule, remove_rule}` (Task 1); `draft::{conditions, Condition, Field, Op, action_label}`, `widgets::{switch, chip}`, `App::preview` (Task 2).
- Produces:

```rust
// draft.rs
impl Field { pub const ALL: [Field; 10]; pub fn is_flag(self) -> bool; }
impl Op { pub const ALL: [Op; 3]; }
impl Condition { pub fn new(field: Field) -> Condition; pub fn with_field(&self, field: Field) -> Condition; }
pub(crate) fn action_name(action: &Action) -> &'static str;   // "Delete", "Mark read", …, "Move to"
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Draft {
    pub account: Option<String>, pub actions: Vec<Action>, pub conditions: Vec<Condition>,
    pub confirm_remove: bool, pub enabled: bool, pub error: Option<String>, pub folder: Option<String>,
    pub name: String, pub original: Option<String>, pub proposed_by: Option<String>, pub tested: Option<String>,
}
impl Draft {
    pub fn new() -> Draft;                         // enabled, nothing else
    pub fn from_rule(rule: &Rule) -> Draft;        // original = Some(rule.name)
    pub fn to_rule(&self) -> Result<Rule, String>;
    pub fn renames(&self) -> bool;
}
// widgets.rs
pub(crate) fn icon_button(ui: &mut egui::Ui, icon: &str, name: &str) -> egui::Response;
// icons.rs
pub(crate) const REMOVE: &str;                     // ph::X
// rule_editor.rs
pub(crate) fn show(app: &App, ui: &mut egui::Ui) -> Vec<UiAction>;
// app.rs
App.rule_draft: Option<Draft>;
UiAction::{EditDraft(Draft), EditRule(String), NewRule, RemoveRule, SaveDraft, TestDraft};
```

The editor edits a clone of `App.rule_draft` and returns `UiAction::EditDraft(changed)` when anything differs; `app.rs` stores it and clears `error` and `tested`. Save sends the draft through `Draft::to_rule` and `rules::edit::save_rule(path, draft.original, rule)`; on success `original` becomes the saved name. "Remove rule" on a saved rule asks first ("Remove from rules.toml" / "Cancel"); on an unsaved draft it just closes it. If GUI redesign Task 8 already added `toolbar::icon_button`, move that function into `widgets.rs` instead of adding a second one.

- [ ] **Step 1: Write the failing tests**

`src/gui/draft.rs` tests:

```rust
    #[test]
    fn a_rule_survives_the_round_trip_through_the_form() {
        let original = rule(&ALL_FIELDS.replace(
            "name = \"everything\"",
            "name = \"everything\"\naccount = \"work\"\nfolder = \"Lists\"\nenabled = false\nproposed_by = \"mcp:agent\"",
        ));
        let back = Draft::from_rule(&original).to_rule().unwrap();
        assert_eq!(toml::to_string(&back).unwrap(), toml::to_string(&original).unwrap());
    }

    #[test]
    fn to_rule_refuses_what_one_rule_cannot_hold() {
        let mut draft = Draft::new();
        draft.conditions = vec![Condition::new(Field::Subject), Condition::new(Field::Subject)];
        assert_eq!(
            draft.to_rule().unwrap_err(),
            "subject appears twice; a rule has one condition per field"
        );
        draft.conditions = vec![Condition {
            value: "maybe".into(),
            ..Condition::new(Field::Seen)
        }];
        assert_eq!(draft.to_rule().unwrap_err(), "seen must be yes or no");
    }

    #[test]
    fn changing_a_conditions_field_keeps_the_value_where_it_fits() {
        let subject = Condition {
            value: "code".into(),
            ..Condition::new(Field::Subject)
        };
        assert_eq!(subject.with_field(Field::From).value, "code");
        assert_eq!(subject.with_field(Field::Seen).value, "true");
        assert_eq!(Condition::new(Field::Seen).with_field(Field::Subject).value, "");
    }

    #[test]
    fn renaming_is_noticed() {
        let mut draft = Draft::from_rule(&rule(ALL_FIELDS));
        assert!(!draft.renames());
        draft.name = "else".into();
        assert!(draft.renames());
        assert!(!Draft::new().renames(), "a new rule has nothing to rename");
    }
```

`src/gui/rule_editor.rs` (new file, tests only for now):

```rust
#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;

    use crate::gui::draft::{Condition, Field};
    use crate::gui::test_support::{Fixture, Wires, message};
    use crate::rules::Action;

    const RULES: &str = "# keep me\n[[rules]]\nname = \"newsletters\"\nmatch.from = { contains = \"news@\" }\nactions = [\"archive\"]\n";

    fn file(fx: &Fixture) -> String {
        std::fs::read_to_string(fx.paths.rules_file()).unwrap()
    }

    fn open_newsletters(fx: &Fixture) -> (egui_kittest::Harness<'static, crate::gui::App>, Wires) {
        let (mut harness, wires) = fx.rules_view(RULES);
        harness.get_by_label("newsletters").click();
        harness.run();
        (harness, wires)
    }

    fn click(harness: &mut egui_kittest::Harness<'static, crate::gui::App>, label: &str) {
        harness.get_by_label(label).click();
        harness.run();
    }

    #[test]
    fn a_new_rule_is_saved_below_the_others_and_keeps_comments() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, wires) = fx.rules_view(RULES);
        click(&mut harness, "New rule");
        let draft = harness.state_mut().rule_draft.as_mut().unwrap();
        draft.name = "receipts".into();
        draft.conditions.push(Condition {
            value: "receipt".into(),
            ..Condition::new(Field::Subject)
        });
        draft.actions.push(Action::Move("Receipts".into()));
        harness.run();
        click(&mut harness, "Save");
        assert_eq!(
            file(&fx),
            format!(
                "{RULES}\n[[rules]]\nname = \"receipts\"\nmatch.subject = {{ contains = \"receipt\" }}\nactions = [{{ move = \"Receipts\" }}]\n"
            )
        );
        let draft = harness.state().rule_draft.clone().unwrap();
        assert_eq!(draft.original.as_deref(), Some("receipts"));
        assert!(wires.sent().is_empty());
    }

    #[test]
    fn editing_a_rule_changes_only_its_value() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = open_newsletters(&fx);
        harness.state_mut().rule_draft.as_mut().unwrap().conditions[0].value = "letters@".into();
        harness.run();
        click(&mut harness, "Save");
        assert_eq!(file(&fx), RULES.replace("news@", "letters@"));
    }

    #[test]
    fn renaming_warns_first_and_saves_in_place() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = open_newsletters(&fx);
        assert!(harness.query_by_label_contains("Renaming makes this a new rule").is_none());
        harness.state_mut().rule_draft.as_mut().unwrap().name = "news".into();
        harness.run();
        assert!(
            harness
                .query_by_label("Renaming makes this a new rule: it acts only on mail that arrives after you save.")
                .is_some()
        );
        click(&mut harness, "Save");
        assert_eq!(file(&fx), RULES.replace("newsletters", "news"));
    }

    #[test]
    fn a_taken_name_is_refused_and_shown() {
        let two = format!("{RULES}\n[[rules]]\nname = \"receipts\"\nmatch.subject = {{ contains = \"receipt\" }}\nactions = [\"flag\"]\n");
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.rules_view(&two);
        click(&mut harness, "newsletters");
        harness.state_mut().rule_draft.as_mut().unwrap().name = "receipts".into();
        harness.run();
        click(&mut harness, "Save");
        assert_eq!(file(&fx), two);
        assert!(harness.query_by_label_contains("duplicate rule name").is_some());
    }

    #[test]
    fn a_rule_removed_elsewhere_is_not_saved_back() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = open_newsletters(&fx);
        std::fs::write(fx.paths.rules_file(), "# emptied\n").unwrap();
        harness.state_mut().rule_draft.as_mut().unwrap().conditions[0].value = "letters@".into();
        harness.run();
        click(&mut harness, "Save");
        assert_eq!(file(&fx), "# emptied\n");
        assert!(harness.query_by_label_contains("no such rule").is_some());
    }

    #[test]
    fn the_list_switch_updates_the_open_rule_so_save_keeps_it() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = open_newsletters(&fx);
        click(&mut harness, "Enable newsletters");
        assert!(!harness.state().rule_draft.as_ref().unwrap().enabled);
        click(&mut harness, "Save");
        assert!(!crate::rules::load(&fx.paths.rules_file()).unwrap().rules[0].enabled);
    }

    #[test]
    fn remove_asks_first_and_keeps_the_comment() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = open_newsletters(&fx);
        click(&mut harness, "Remove rule");
        assert_eq!(file(&fx), RULES, "nothing removed before the confirmation");
        click(&mut harness, "Remove from rules.toml");
        assert_eq!(file(&fx), "# keep me\n");
        assert!(harness.state().rule_draft.is_none());
    }

    #[test]
    fn test_counts_cached_mail_and_changes_nothing() {
        let fx = Fixture::new(&["work"]);
        let mut weekly = message("INBOX", 1, "weekly");
        weekly.from_addr = Some("News <news@example.com>".into());
        fx.add("work", weekly);
        fx.add("work", message("INBOX", 2, "lunch"));
        let (mut harness, wires) = fx.rules_view(RULES);
        click(&mut harness, "newsletters");
        click(&mut harness, "Test");
        assert!(harness.query_by_label("would archive 1 cached message").is_some());
        assert_eq!(file(&fx), RULES);
        assert!(wires.sent().is_empty());
    }

    #[test]
    fn conditions_and_actions_are_added_and_removed() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = open_newsletters(&fx);
        click(&mut harness, "+ Add condition");
        let fields: Vec<Field> = harness.state().rule_draft.as_ref().unwrap().conditions.iter().map(|c| c.field).collect();
        assert_eq!(fields, [Field::From, Field::To]);
        click(&mut harness, "Remove condition 2");
        click(&mut harness, "Remove archive");
        assert!(harness.state().rule_draft.as_ref().unwrap().actions.is_empty());
        // If kittest does not open the menu with one click and run, drop the next three lines and assert on the
        // draft after `harness.state_mut().rule_draft.as_mut().unwrap().actions.push(Action::Delete)`.
        click(&mut harness, "+ Add action");
        click(&mut harness, "Delete");
        assert_eq!(harness.state().rule_draft.as_ref().unwrap().actions, [Action::Delete]);
        assert!(harness.query_by_label("keeps a .eml backup").is_some());
    }
}
```

`src/gui/snapshots.rs`:

```rust
#[test]
fn rule_editor() {
    let fx = mailbox("light");
    std::fs::write(fx.paths.rules_file(), RULES).unwrap();
    let (mut harness, _wires) = open(&fx);
    harness.state_mut().select_view(View::Rules);
    harness.run();
    harness.get_by_label("receipts").click();
    harness.run();
    assert!(harness.query_by_label("Save").is_some());
    snapshot(&mut harness, "gui_rule_editor");
}
```

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::`
Expected: FAIL to compile (`Draft`, `rule_draft`, `Condition::new` … not found).

- [ ] **Step 3: Extend `draft.rs`**

Imports become `use crate::rules::{Action, HeaderMatch, Match, Rule, TextMatch};`. Add:

```rust
impl Field {
    /// In the order the rules docs list them.
    pub const ALL: [Field; 10] = [
        Field::From,
        Field::To,
        Field::Cc,
        Field::Subject,
        Field::Body,
        Field::Header,
        Field::OlderThan,
        Field::Seen,
        Field::ToMe,
        Field::Alias,
    ];

    /// Fields whose value is yes or no.
    pub fn is_flag(self) -> bool {
        matches!(self, Field::Seen | Field::ToMe)
    }
}

impl Op {
    pub const ALL: [Op; 3] = [Op::Contains, Op::Equals, Op::Regex];
}

impl Condition {
    pub fn new(field: Field) -> Condition {
        Condition {
            field,
            header: String::new(),
            op: Op::Contains,
            value: if field.is_flag() { "true".into() } else { String::new() },
        }
    }

    /// This condition on `field`: the value stays unless it switches between yes/no and text.
    pub fn with_field(&self, field: Field) -> Condition {
        if field.is_flag() != self.field.is_flag() {
            return Condition::new(field);
        }
        Condition {
            field,
            ..self.clone()
        }
    }
}

/// An action as a chip or menu entry names it.
pub(crate) fn action_name(action: &Action) -> &'static str {
    match action {
        Action::Archive => "Archive",
        Action::Delete => "Delete",
        Action::Flag => "Flag",
        Action::MarkRead => "Mark read",
        Action::Move(_) => "Move to",
        Action::Notify => "Notify",
        Action::Silent => "Silent",
        _ => "",
    }
}

/// The rule editor's working copy. Lives in `App.rule_draft` until Save writes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Draft {
    pub account: Option<String>,
    pub actions: Vec<Action>,
    pub conditions: Vec<Condition>,
    /// Set by "Remove rule" on a saved rule; the footer then asks to confirm.
    pub confirm_remove: bool,
    pub enabled: bool,
    /// Why the last save or removal failed; cleared by any edit.
    pub error: Option<String>,
    pub folder: Option<String>,
    pub name: String,
    /// The rule's name in rules.toml; None until the draft is first saved.
    pub original: Option<String>,
    pub proposed_by: Option<String>,
    /// The last Test result; cleared by any edit.
    pub tested: Option<String>,
}

impl Draft {
    pub fn new() -> Draft {
        Draft {
            enabled: true,
            ..Draft::default()
        }
    }

    pub fn from_rule(rule: &Rule) -> Draft {
        Draft {
            account: rule.account.clone(),
            actions: rule.actions.clone(),
            conditions: conditions(&rule.matches),
            enabled: rule.enabled,
            folder: rule.folder.clone(),
            name: rule.name.clone(),
            original: Some(rule.name.clone()),
            proposed_by: rule.proposed_by.clone(),
            ..Draft::default()
        }
    }

    /// True when Save would rename a saved rule, which makes it a new rule with a new clock.
    pub fn renames(&self) -> bool {
        self.original.as_deref().is_some_and(|original| original != self.name)
    }

    /// The rule this draft saves as. `rules::compile` checks the values when the file is saved; this only refuses what
    /// one rule cannot hold.
    pub fn to_rule(&self) -> Result<Rule, String> {
        let mut m = Match::default();
        for c in &self.conditions {
            let twice = match c.field {
                Field::Alias => m.alias.replace(c.value.clone()).is_some(),
                Field::Body => m.body.replace(text_match(c)).is_some(),
                Field::Cc => m.cc.replace(text_match(c)).is_some(),
                Field::From => m.from.replace(text_match(c)).is_some(),
                Field::Header => {
                    let t = text_match(c);
                    let header = HeaderMatch {
                        name: c.header.clone(),
                        contains: t.contains,
                        equals: t.equals,
                        regex: t.regex,
                    };
                    m.header.replace(header).is_some()
                }
                Field::OlderThan => m.older_than.replace(c.value.clone()).is_some(),
                Field::Seen => m.seen.replace(flag(c)?).is_some(),
                Field::Subject => m.subject.replace(text_match(c)).is_some(),
                Field::To => m.to.replace(text_match(c)).is_some(),
                Field::ToMe => m.to_me.replace(flag(c)?).is_some(),
            };
            if twice {
                return Err(format!(
                    "{} appears twice; a rule has one condition per field",
                    c.field.label()
                ));
            }
        }
        Ok(Rule {
            name: self.name.clone(),
            account: self.account.clone(),
            folder: self.folder.clone(),
            enabled: self.enabled,
            proposed_by: self.proposed_by.clone(),
            matches: m,
            actions: self.actions.clone(),
        })
    }
}

fn text_match(c: &Condition) -> TextMatch {
    let value = Some(c.value.clone());
    match c.op {
        Op::Contains => TextMatch {
            contains: value,
            ..TextMatch::default()
        },
        Op::Equals => TextMatch {
            equals: value,
            ..TextMatch::default()
        },
        Op::Regex => TextMatch {
            regex: value,
            ..TextMatch::default()
        },
    }
}

fn flag(c: &Condition) -> Result<bool, String> {
    c.value
        .parse()
        .map_err(|_| format!("{} must be yes or no", c.field.label()))
}
```

- [ ] **Step 4: `icon_button` and `REMOVE`**

`src/gui/widgets.rs`:

```rust
/// A frameless icon button; `name` is what screen readers, tests and the hover text call it.
pub(crate) fn icon_button(ui: &mut egui::Ui, icon: &str, name: &str) -> egui::Response {
    let response = ui.add(egui::Button::new(icon).frame(false)).on_hover_text(name);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    response
}
```

`src/gui/icons.rs`: `pub(crate) const REMOVE: &str = ph::X;`.

- [ ] **Step 5: Implement `rule_editor.rs`** above its tests

```rust
//! The right side of the Rules view: the selected rule, or a new one, as a form. Edits stay in `App.rule_draft` until
//! Save hands them to `rules::edit`.
use eframe::egui;

use crate::message::clean;
use crate::rules::Action;

use super::app::{App, UiAction};
use super::draft::{Condition, Draft, Field, Op, action_label, action_name};
use super::{icons, theme, widgets};

/// The actions "+ Add action" offers, in the docs' order; a move asks for its folder in its chip.
const ACTIONS: [Action; 7] = [
    Action::Delete,
    Action::MarkRead,
    Action::Flag,
    Action::Archive,
    Action::Move(String::new()),
    Action::Notify,
    Action::Silent,
];

pub(crate) fn show(app: &App, ui: &mut egui::Ui) -> Vec<UiAction> {
    let Some(saved) = &app.rule_draft else {
        ui.weak("Select a rule, or make a new one.");
        return Vec::new();
    };
    let mut draft = saved.clone();
    let mut actions = Vec::new();
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        name_row(ui, &mut draft);
        ui.add_space(8.0);
        watches(app, ui, &mut draft);
        ui.add_space(8.0);
        conditions(ui, &mut draft);
        ui.add_space(8.0);
        then(ui, &mut draft);
        ui.separator();
        actions.extend(footer(ui, &mut draft));
    });
    if draft != *saved {
        actions.insert(0, UiAction::EditDraft(draft));
    }
    actions
}

fn name_row(ui: &mut egui::Ui, draft: &mut Draft) {
    ui.horizontal(|ui| {
        let label = ui.label("Name");
        theme::text_field(ui, |ui| {
            ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(320.0))
        })
        .labelled_by(label.id);
        ui.add_space(12.0);
        let label = ui.label("Enabled");
        if widgets::switch(ui, draft.enabled, "Enabled")
            .labelled_by(label.id)
            .clicked()
        {
            draft.enabled = !draft.enabled;
        }
    });
    if draft.renames() {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "Renaming makes this a new rule: it acts only on mail that arrives after you save.",
        );
    }
}

fn watches(app: &App, ui: &mut egui::Ui, draft: &mut Draft) {
    ui.strong("Watches");
    ui.horizontal(|ui| {
        let account = draft
            .account
            .as_deref()
            .map_or("Every account".into(), |a| clean(a, false));
        egui::ComboBox::from_id_salt("rule_account")
            .selected_text(account)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut draft.account, None, "Every account");
                for account in &app.accounts {
                    ui.selectable_value(
                        &mut draft.account,
                        Some(account.name.clone()),
                        clean(&account.name, false),
                    );
                }
            });
        egui::ComboBox::from_id_salt("rule_folder")
            .selected_text(clean(draft.folder.as_deref().unwrap_or("INBOX"), false))
            .show_ui(ui, |ui| {
                for name in folder_names(app, draft.account.as_deref()) {
                    let value = (!name.eq_ignore_ascii_case("INBOX")).then(|| name.clone());
                    ui.selectable_value(&mut draft.folder, value, clean(&name, false));
                }
            });
    });
}

/// The folders a rule can watch: its account's, or every account's for a rule on all of them.
fn folder_names(app: &App, account: Option<&str>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for a in app.accounts.iter().filter(|a| account.is_none_or(|name| name == a.name)) {
        for folder in &a.folders {
            if !names.contains(&folder.name) {
                names.push(folder.name.clone());
            }
        }
    }
    names
}

fn conditions(ui: &mut egui::Ui, draft: &mut Draft) {
    ui.strong("When all of these match");
    let used: Vec<Field> = draft.conditions.iter().map(|c| c.field).collect();
    let mut remove = None;
    for (index, condition) in draft.conditions.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt(("condition_field", index))
                .selected_text(condition.field.label())
                .show_ui(ui, |ui| {
                    for field in Field::ALL {
                        if (field == condition.field || !used.contains(&field))
                            && ui.selectable_label(field == condition.field, field.label()).clicked()
                        {
                            *condition = condition.with_field(field);
                        }
                    }
                });
            if condition.field == Field::Header {
                theme::text_field(ui, |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut condition.header)
                            .hint_text("List-Id")
                            .desired_width(100.0),
                    )
                });
            }
            if condition.field.is_text() {
                egui::ComboBox::from_id_salt(("condition_op", index))
                    .selected_text(condition.op.label())
                    .show_ui(ui, |ui| {
                        for op in Op::ALL {
                            ui.selectable_value(&mut condition.op, op, op.label());
                        }
                    });
            }
            value_field(ui, condition, index);
            if widgets::icon_button(ui, icons::REMOVE, &format!("Remove condition {}", index + 1)).clicked() {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove {
        draft.conditions.remove(index);
    }
    if let Some(field) = Field::ALL.into_iter().find(|field| !used.contains(field))
        && ui.button("+ Add condition").clicked()
    {
        draft.conditions.push(Condition::new(field));
    }
}

/// A condition's value: yes or no for seen and to me, else text.
fn value_field(ui: &mut egui::Ui, condition: &mut Condition, index: usize) -> egui::Response {
    if condition.field.is_flag() {
        let shown = if condition.value == "false" { "no" } else { "yes" };
        return egui::ComboBox::from_id_salt(("condition_flag", index))
            .selected_text(shown)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut condition.value, "true".to_string(), "yes");
                ui.selectable_value(&mut condition.value, "false".to_string(), "no");
            })
            .response;
    }
    let hint = match condition.field {
        Field::Alias => "*@shop.example.com",
        Field::OlderThan => "1h, 30m, 2days",
        _ => "text",
    };
    theme::text_field(ui, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut condition.value)
                .hint_text(hint)
                .desired_width(220.0),
        )
    })
}

fn then(ui: &mut egui::Ui, draft: &mut Draft) {
    ui.strong("Then");
    let palette = theme::palette(ui);
    let missing: Vec<Action> = ACTIONS
        .iter()
        .filter(|offered| {
            !draft
                .actions
                .iter()
                .any(|have| std::mem::discriminant(have) == std::mem::discriminant(*offered))
        })
        .cloned()
        .collect();
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        for (index, action) in draft.actions.iter_mut().enumerate() {
            let label = action_label(action);
            let delete = *action == Action::Delete;
            let colour = if delete { palette.error } else { palette.text };
            let stroke = if delete { palette.error } else { palette.border };
            widgets::chip(palette.background, stroke).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(action_name(action)).color(colour));
                    if let Action::Move(folder) = action {
                        theme::text_field(ui, |ui| {
                            ui.add(egui::TextEdit::singleline(folder).hint_text("Folder").desired_width(120.0))
                        });
                    }
                    if delete {
                        ui.label(egui::RichText::new("keeps a .eml backup").small().color(colour));
                    }
                    if widgets::icon_button(ui, icons::REMOVE, &format!("Remove {label}")).clicked() {
                        remove = Some(index);
                    }
                });
            });
        }
        ui.menu_button("+ Add action", |ui| {
            for action in missing {
                if ui.button(action_name(&action)).clicked() {
                    draft.actions.push(action);
                    ui.close();
                }
            }
        });
    });
    if let Some(index) = remove {
        draft.actions.remove(index);
    }
}

fn footer(ui: &mut egui::Ui, draft: &mut Draft) -> Vec<UiAction> {
    let palette = theme::palette(ui);
    if let Some(error) = &draft.error {
        ui.colored_label(ui.visuals().error_fg_color, clean(error, false));
    }
    let (test, mut actions) = egui::Sides::new().show(
        ui,
        |ui| {
            let test = ui.button("Test").clicked();
            if let Some(tested) = &draft.tested {
                ui.label(clean(tested, false));
            }
            test
        },
        |ui| {
            let mut actions = Vec::new();
            if ui.button("Save").clicked() {
                actions.push(UiAction::SaveDraft);
            }
            if draft.confirm_remove {
                if ui
                    .button(egui::RichText::new("Remove from rules.toml").color(palette.error))
                    .clicked()
                {
                    actions.push(UiAction::RemoveRule);
                }
                if ui.button("Cancel").clicked() {
                    draft.confirm_remove = false;
                }
            } else if ui.button("Remove rule").clicked() {
                if draft.original.is_some() {
                    draft.confirm_remove = true;
                } else {
                    actions.push(UiAction::RemoveRule);
                }
            }
            actions
        },
    );
    if test {
        actions.push(UiAction::TestDraft);
    }
    ui.weak("Test runs on mail already on this device. A saved rule acts only on mail that arrives after it is enabled.");
    actions
}
```

The two `Sides` closures touch different fields of `draft` (`tested` on the left; `confirm_remove` and `original` on the right), which edition 2024 closures capture separately.

- [ ] **Step 6: Wire the editor** in `src/gui/rules.rs`

In `show_rules`, the right side of the header adds "New rule" first, so it sits rightmost:

```rust
        |ui| {
            let mut clicked = Vec::new();
            if ui.button("New rule").clicked() {
                clicked.push(UiAction::NewRule);
            }
            if ui.button("Open rules.toml").clicked() {
                clicked.push(UiAction::OpenRulesFile);
            }
            clicked
        },
```

the rows learn which rule is open:

```rust
                let open = app.rule_draft.as_ref().and_then(|d| d.original.as_deref());
                for rule in &rules {
                    actions.extend(rule_row(rule, open == Some(rule.name.as_str()), ui));
                }
```

the placeholder becomes `egui::CentralPanel::no_frame().show(ui, |ui| actions.extend(rule_editor::show(app, ui)));` (import `super::rule_editor`), and in `rule_row(rule: &Rule, selected: bool, ui)` the name becomes clickable:

```rust
                if ui.selectable_label(selected, egui::RichText::new(&name).strong()).clicked() {
                    actions.push(UiAction::EditRule(rule.name.clone()));
                }
```

`src/gui/mod.rs`: `mod rule_editor;`.

- [ ] **Step 7: Wire the actions** in `src/gui/app.rs`

`use super::draft::{self, Draft};`. Field `pub(crate) rule_draft: Option<Draft>,` (`None` in `App::new`). `UiAction` gains, alphabetically, `EditDraft(Draft)`, `EditRule(String)`, `NewRule`, `RemoveRule`, `SaveDraft`, `TestDraft`. In `apply` (the `SetRuleEnabled` arm replaces the existing one):

```rust
            UiAction::EditDraft(mut draft) => {
                draft.error = None;
                draft.tested = None;
                self.rule_draft = Some(draft);
            }
            UiAction::EditRule(name) => {
                self.rule_draft = self
                    .rules
                    .rules
                    .iter()
                    .find(|rule| rule.name == name)
                    .map(Draft::from_rule);
            }
            UiAction::NewRule => self.rule_draft = Some(Draft::new()),
            UiAction::RemoveRule => self.remove_draft(),
            UiAction::SaveDraft => self.save_draft(),
            UiAction::SetRuleEnabled(name, enabled) => {
                self.edit_rules(|path| rule_file::edit::set_enabled(path, &name, enabled));
                // The open draft follows the file, so a later Save does not undo the switch.
                let now = self.rules.rules.iter().find(|r| r.name == name).map(|r| r.enabled);
                if let (Some(now), Some(draft)) = (
                    now,
                    self.rule_draft
                        .as_mut()
                        .filter(|d| d.original.as_deref() == Some(name.as_str())),
                ) {
                    draft.enabled = now;
                }
            }
            UiAction::TestDraft => self.test_draft(),
```

and beside `preview`:

```rust
    fn test_draft(&mut self) {
        let Some(draft) = &self.rule_draft else { return };
        let text = match draft.to_rule() {
            Ok(rule) => self.preview(rule),
            Err(e) => e,
        };
        if let Some(draft) = self.rule_draft.as_mut() {
            draft.tested = Some(text);
        }
    }

    /// Writes the draft to rules.toml: a new rule, or in place of the rule it was opened from.
    fn save_draft(&mut self) {
        let Some(draft) = &self.rule_draft else { return };
        let path = self.paths.rules_file();
        let saved = draft.to_rule().and_then(|rule| {
            rule_file::edit::save_rule(&path, draft.original.as_deref(), &rule)
                .map(|()| rule.name)
                .map_err(|e| e.to_string())
        });
        match saved {
            Ok(name) => {
                if let Some(draft) = self.rule_draft.as_mut() {
                    draft.original = Some(name);
                    draft.error = None;
                }
                self.rules_changed();
            }
            Err(e) => {
                if let Some(draft) = self.rule_draft.as_mut() {
                    draft.error = Some(e);
                }
            }
        }
    }

    /// Removes the draft's rule from rules.toml; an unsaved draft is just closed.
    fn remove_draft(&mut self) {
        let Some(name) = self.rule_draft.as_ref().and_then(|d| d.original.clone()) else {
            self.rule_draft = None;
            return;
        };
        match rule_file::edit::remove_rule(&self.paths.rules_file(), &name) {
            Ok(()) => {
                self.rule_draft = None;
                self.rules_changed();
            }
            Err(e) => {
                if let Some(draft) = self.rule_draft.as_mut() {
                    draft.error = Some(e.to_string());
                    draft.confirm_remove = false;
                }
            }
        }
    }
```

- [ ] **Step 8: Run the GUI tests and the architecture check**

Run: `cargo test --lib gui:: && cargo test --test architecture`
Expected: PASS. `rule_editor.rs` is a view: it takes `&App` and returns `UiAction`s; only `app.rs` names `rules::edit`.

- [ ] **Step 9: Docs** — in `docs/src/gui.md`, after the Rules sentence from Task 2:

```markdown
Select a rule, or press New rule, to edit it on the right: its name, whether it is on, the account and folder it watches, its conditions and its actions. Test shows what it would do to the mail already on this device without doing it. Save writes the rule to `rules.toml` and keeps the file's comments; Remove rule asks before it deletes. Renaming a rule makes it a new rule, which acts only on mail that arrives after you save.
```

- [ ] **Step 10: Snapshots on Linux** (`gui_rules` changes, `gui_rule_editor` is new), then commit

```bash
git add src/gui/ docs/src/gui.md tests/snapshots/
git commit -m "feat(gui): edit, test, save and remove rules in the Rules view"
```

---

### Task 4: "Rule" buttons in the reader

**Files:**
- Modify: `src/gui/body.rs` (header rows, `rule_button`, tests), `src/gui/draft.rs` (`RuleSeed`, `Draft::from_seed`), `src/gui/app.rs` (`UiAction::RuleFrom`), `docs/src/gui.md`

**Interfaces:**
- Consumes: `Draft`, `Condition`, `Field`, `Op` (Tasks 2–3); `icons::RULES` (GUI redesign Task 2).
- Produces:

```rust
// draft.rs
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RuleSeed { From(String), Subject(String) }
impl Draft { pub fn from_seed(seed: &RuleSeed, account: &str, folder: &str) -> Draft; }
// app.rs
UiAction::RuleFrom(RuleSeed);
```

The reader's header rows become `egui::Sides` rows: name and value on the left (the value truncates with "…" and shows in full on hover), and on the From and Subject rows a small outlined "Rule" button with the funnel icon at the right edge. The button opens the Rules view with an unsaved draft: from equals the sender's first bare address, or subject contains the subject; it watches the message's account and folder and has no actions yet, so Save explains what is missing until one is added.

- [ ] **Step 1: Write the failing tests**

`src/gui/draft.rs` tests:

```rust
    #[test]
    fn a_seed_from_a_message_makes_a_short_clean_draft() {
        let from = Draft::from_seed(
            &RuleSeed::From("Alice <Alice@Example.com>, bob@example.com".into()),
            "work",
            "INBOX",
        );
        assert_eq!(
            from.conditions,
            [Condition {
                field: Field::From,
                header: String::new(),
                op: Op::Equals,
                value: "alice@example.com".into(),
            }]
        );
        assert_eq!(from.name, "from alice@example.com");
        assert_eq!((from.account.as_deref(), from.folder.as_deref()), (Some("work"), None));
        assert!(from.original.is_none() && from.actions.is_empty() && from.enabled);

        let long = format!("Your code\u{1b}[2J {}", "x".repeat(300));
        let subject = Draft::from_seed(&RuleSeed::Subject(long), "work", "Lists");
        assert_eq!(subject.folder.as_deref(), Some("Lists"));
        assert!(subject.name.chars().count() <= 60, "{}", subject.name);
        assert!(subject.conditions[0].value.starts_with("Your code[2J x"));
        assert!(!subject.name.contains('\u{1b}') && !subject.conditions[0].value.contains('\u{1b}'));
    }
```

`src/gui/body.rs` tests:

```rust
    use crate::gui::draft::{Condition, Field, Op};

    #[test]
    fn the_rule_button_on_from_opens_an_unsaved_draft_for_that_address() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 1, "hello there"));
        let (mut harness, _wires) = fx.harness();
        let button = harness.get_by_label("New rule for mail from this sender");
        assert!(button.rect().right() > 1200.0, "at the right edge of the 1280 px window");
        button.click();
        harness.run();
        assert_eq!(harness.state().view, View::Rules);
        let draft = harness.state().rule_draft.clone().unwrap();
        assert_eq!(
            draft.conditions,
            [Condition {
                field: Field::From,
                header: String::new(),
                op: Op::Equals,
                value: "sender1@example.com".into(),
            }]
        );
        assert_eq!(draft.original, None);
        assert!(!fx.paths.rules_file().exists(), "nothing is written before Save");
    }

    #[test]
    fn the_rule_button_on_subject_matches_the_subject() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 1, "hello there"));
        let (mut harness, _wires) = fx.harness();
        harness.get_by_label("New rule for mail with this subject").click();
        harness.run();
        let draft = harness.state().rule_draft.clone().unwrap();
        assert_eq!(
            (draft.conditions[0].field, draft.conditions[0].op, draft.conditions[0].value.as_str()),
            (Field::Subject, Op::Contains, "hello there")
        );
        assert_eq!(draft.name, "subject hello there");
    }
```

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::body gui::draft`
Expected: FAIL to compile (`RuleSeed`, `from_seed` not found).

- [ ] **Step 3: Implement the seed** in `draft.rs`

`use crate::message::{bare_addresses, clean};` and:

```rust
/// Longest name a draft made from a message gets; rules.toml allows 255 characters.
const SEED_NAME_CHARS: usize = 60;

/// Where the reader's Rule buttons start a draft from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RuleSeed {
    From(String),
    Subject(String),
}

impl Draft {
    /// A new rule for mail like the message on screen, watching its account and folder.
    pub fn from_seed(seed: &RuleSeed, account: &str, folder: &str) -> Draft {
        let (field, op, value) = match seed {
            RuleSeed::From(from) => {
                let address = bare_addresses(from)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| from.trim().to_string());
                (Field::From, Op::Equals, clean(&address, false))
            }
            RuleSeed::Subject(subject) => (Field::Subject, Op::Contains, clean(subject.trim(), false)),
        };
        let name: String = format!("{} {value}", field.label())
            .chars()
            .take(SEED_NAME_CHARS)
            .collect();
        Draft {
            account: Some(account.to_string()),
            conditions: vec![Condition {
                field,
                header: String::new(),
                op,
                value,
            }],
            folder: (!folder.eq_ignore_ascii_case("INBOX")).then(|| folder.to_string()),
            name: name.trim_end().to_string(),
            ..Draft::new()
        }
    }
}
```

- [ ] **Step 4: Implement the header rows** in `body.rs`

Imports: `use super::draft::RuleSeed;` and `use super::{icons, theme};`. Constant:

```rust
/// Width of the header names, so the values line up.
const HEADER_NAME_WIDTH: f32 = 64.0;
```

Replace the `egui::Grid::new("headers")` block in `show`:

```rust
    let headers = [
        (
            "From",
            message.from_addr.as_deref(),
            message.from_addr.clone().map(RuleSeed::From),
            "New rule for mail from this sender",
        ),
        ("To", message.to_addr.as_deref(), None, ""),
        ("Cc", message.cc_addr.as_deref(), None, ""),
        ("Date", Some(date.as_str()), None, ""),
        (
            "Subject",
            message.subject.as_deref(),
            message.subject.clone().map(RuleSeed::Subject),
            "New rule for mail with this subject",
        ),
    ];
    for (name, value, seed, hint) in headers {
        let Some(value) = value else { continue };
        let (_, clicked) = egui::Sides::new().shrink_left().truncate().show(
            ui,
            |ui| {
                let label = ui.strong(name);
                ui.add_space((HEADER_NAME_WIDTH - label.rect.width()).max(0.0));
                ui.label(clean(value, false));
            },
            |ui| seed.is_some() && rule_button(ui, hint).clicked(),
        );
        if clicked && let Some(seed) = seed {
            actions.push(UiAction::RuleFrom(seed));
        }
    }
```

and below `show`:

```rust
/// A small outlined "Rule" button with the funnel icon; `name` is what screen readers, tests and the hover text call it.
fn rule_button(ui: &mut egui::Ui, name: &str) -> egui::Response {
    let palette = theme::palette(ui);
    let button = egui::Button::new(format!("{} Rule", icons::RULES))
        .small()
        .fill(egui::Color32::TRANSPARENT)
        .stroke(egui::Stroke::new(1.0, palette.border));
    let response = ui.add(button).on_hover_text(name);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, name));
    response
}
```

If kittest still finds the button only by its text (`get_by_label_contains("Rule")` finds two), `widget_info` did not replace the name: give the buttons `labelled_by` a hidden label instead, or match with `get_all_by_label_contains("Rule")` in the tests and take the first (From) and second (Subject).

- [ ] **Step 5: Wire the action** in `app.rs`

`use super::draft::{self, Draft, RuleSeed};`, `UiAction::RuleFrom(RuleSeed)`, and in `apply`:

```rust
            UiAction::RuleFrom(seed) => {
                if let Some(body) = &self.body {
                    let draft = Draft::from_seed(&seed, &self.accounts[body.account].name, &body.key.0);
                    self.select_view(View::Rules);
                    self.rule_draft = Some(draft);
                }
            }
```

- [ ] **Step 6: Run the GUI tests**

Run: `cargo test --lib gui::`
Expected: PASS, including `the_cursor_message_shows_headers_and_text` (the sender is still one label).

- [ ] **Step 7: Docs** — in `docs/src/gui.md`, in the paragraph about the message pane:

```markdown
The Rule buttons beside From and Subject start a new rule for mail like this one: from that address, or with that subject. It opens in the Rules view and is not written to `rules.toml` until you add an action and save it.
```

- [ ] **Step 8: Snapshots on Linux** (the inbox snapshots change), then commit

```bash
git add src/gui/ docs/src/gui.md tests/snapshots/
git commit -m "feat(gui): start a rule from the reader's From and Subject"
```

---

## Finish

- [ ] Full gate: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && cargo machete && cargo audit`
- [ ] `POSTBODE_BLESS=1 cargo test` changes nothing (no CLI flag or rule type changed).
- [ ] Snapshots regenerated on Linux with lavapipe, and every changed PNG looked at: `gui_rules`, `gui_rule_editor`, the inbox snapshots.
- [ ] Report which platforms the PR was compiled on and ran on.
