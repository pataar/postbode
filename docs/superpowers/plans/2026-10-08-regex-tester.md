# Regex Tester Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A regex tester dialog in the Rules view: type a pattern, see at once whether it is valid and which cached subjects, senders, recipients, bodies or lines of your own text it matches, with the matched part highlighted, then put it into the rule being edited.

**Architecture:** One new module, `src/gui/regex_tester.rs`, holds the tester's state, pure matching helpers (`match_ranges`, `samples`, `excerpt`, `summary`, `highlighted`) and the `egui::Modal` view, which returns `UiAction`s. `app.rs` owns `App.regex_tester`, loads the texts to test from the local store when the field or folder changes, re-runs the pattern on each edit, and carries "Use in rule" into `App.rule_draft`.

**Tech Stack:** Rust 2024, eframe/egui 0.36.2, egui_kittest 0.36.2 (kittest 0.4.0), regex 1.13 (already a dependency; the same crate and syntax rules use), egui-phosphor 0.14. No new dependencies.

**Spec:** design canvas https://claude.ai/artifact/B5W17bKLqpKUw9z3bCw65Q (Rules · Latte and Regex tester boards) + docs/src/rules.md + docs/superpowers/specs/2026-10-06-postbode-core-design.md.

## Scope

One PR off `origin/main`, after `docs/superpowers/plans/2026-10-08-rules-editor.md` has merged (it provides the Rules view header, `Draft`, `Condition`, `Field`, `Op`, `Fixture::rules_view` and the editor's value fields). One commit per task.

| # | Task | Design item 5 |
|---|---|---|
| 1 | The tester: pattern, validity, Copy, Test on, folder, results, Close | everything but "Use in rule" |
| 2 | "Use in rule" | puts the pattern into the selected condition |

## Global Constraints

- `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`, `cargo machete` and `cargo audit` all pass after every task.
- No `unwrap`, `expect`, `panic!`, `todo!` or `unimplemented!` outside tests; where a call cannot fail, a narrow `#[allow]` says why.
- GUI views draw from `&App` and return `UiAction`s; only `app.rs` changes state (`tests/architecture.rs`).
- The tester reads the local store only: no daemon command, no network, no write. "Use in rule" changes the open draft only; nothing reaches rules.toml until the editor's Save.
- Matching is `regex::Regex` as in `rules::Matcher::Regex`: case-sensitive unless the pattern starts with `(?i)`. A pattern that is empty after trimming is not valid, as `rules::compile` refuses it.
- Body covers only messages whose body is downloaded, and the count line says so.
- Copy, verbatim: "Regex tester", "Pattern", "valid", "invalid", "empty", "Copy", "Rust regex syntax. Start with (?i) to ignore case.", "Test on", "Subject", "From", "To", "Body", "Own text", "Your text", "6 of 214 cached subjects match", "Show non-matching", "Runs on mail already on this device; nothing is sent.", "Close", "Use in rule", "Enter a pattern.".
- Colours from `theme::Palette`: a match is `on_accent` on `accent`; valid `success`, invalid `error`.
- Agent and mail text is passed through `message::clean` before it is drawn; highlight ranges are computed on the raw text and each piece is cleaned separately.
- Prose changes go in `docs/src/gui.md`.
- After GUI changes, regenerate snapshots on Linux with lavapipe: `TZ=UTC UPDATE_SNAPSHOTS=1 cargo test gui::snapshots`, and look at every changed PNG.
- Branch: `git switch -c feat/regex-tester --no-track origin/main`, first push `git push -u origin HEAD`. Conventional commits. Report "compiled on" vs "ran on".

## Verification notes

Checked against source in `~/.cargo/registry/src`: `egui::Modal::new(Id).show(ctx, ..) -> ModalResponse { inner, .. }` and `ModalResponse::should_close` (egui 0.36.2), `Context::copy_text`, `OutputCommand::CopyText`, `TextEdit::font/multiline/desired_rows`, `ScrollArea::max_height/auto_shrink/show_rows`, `Ui::text_style_height/set_width/checkbox`, `text::LayoutJob::append`, `TextFormat { background, .. }`, `Sides`, kittest `get_all_by_role`, `Node::value`, `NodeT::accesskit_node().is_disabled()` (accesskit_consumer), egui-phosphor `COPY`, and `regex::Match::range`. egui's `TextEdit` drops a typed `"\n"` from a text event, so the tests press Enter for new lines. Not run: the GUI code as a whole, in particular that kittest focuses a `TextEdit` inside a `Modal` with a click (the tests rely on it as the rules-editor tests do).

## Review Focus

1. **An empty pattern, or one that matches the empty string** (`^`, `a*`): empty shows "Enter a pattern." and cannot go into a rule; a zero-width match counts as a match and draws no empty highlight. Test in Task 1.
2. **Non-ASCII mail** (accents, emoji) and **a match far into a long body line**: highlights cover whole characters, slicing never panics, and the excerpt starts shortly before the match so it is on screen. Test in Task 1.
3. **An invalid pattern**: the regex error shows in place of the results, the line says "invalid", nothing panics, and "Use in rule" stays off. Tests in Task 1 and Task 2.
4. **Body with mail whose body is not downloaded**: those messages are left out of the count, and the line says "only messages whose body is downloaded". Test in Task 1.
5. **Escape while the tester is open**: it closes the tester only; the open draft, the view and search stay. Test in Task 1.

---

### Task 1: The tester

**Files:**
- Create: `src/gui/regex_tester.rs`
- Modify: `src/gui/mod.rs` (`mod regex_tester;`), `src/gui/icons.rs` (`COPY`), `src/gui/rules.rs` (header button), `src/gui/app.rs` (state, actions, Escape, drawing), `src/gui/snapshots.rs` (`regex_tester`), `docs/src/gui.md`

**Interfaces:**
- Consumes: `Fixture::rules_view`, `App.rule_draft`, `Draft` (rules-editor plan); `theme::{palette, text_field, Palette}`.
- Produces:

```rust
// regex_tester.rs
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Source { Subject, From, To, Body, Own }
impl Source { pub const ALL: [Source; 5]; pub fn label(self) -> &'static str; pub fn noun(self) -> &'static str; }
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TesterInput {
    pub account: usize, pub folder: String, pub own_text: String, pub pattern: String,
    pub show_non_matching: bool, pub source: Source,
}
/// Per sample: the matched byte ranges (empty for a zero-width match), or None when it did not match.
pub(crate) type Found = Vec<Option<Vec<Range<usize>>>>;
pub(crate) struct RegexTester { pub input: TesterInput, pub results: Result<Found, String>, pub samples: Vec<String> }
pub(crate) const EXCERPT_LEAD: usize = 40;
pub(crate) fn samples(messages: &[Message], source: Source) -> Vec<String>;
pub(crate) fn match_ranges(pattern: &str, samples: &[String]) -> Result<Found, String>;
pub(crate) fn excerpt<'a>(text: &'a str, ranges: &[Range<usize>]) -> (&'a str, Vec<Range<usize>>);
pub(crate) fn summary(source: Source, found: &[Option<Vec<Range<usize>>>]) -> String;
pub(crate) fn highlighted(line: &str, ranges: &[Range<usize>], palette: &Palette, font: egui::FontId) -> egui::text::LayoutJob;
pub(crate) fn show(app: &App, ctx: &egui::Context) -> Vec<UiAction>;
// icons.rs
pub(crate) const COPY: &str;                       // ph::COPY
// app.rs
App.regex_tester: Option<RegexTester>;
UiAction::{CloseRegexTester, EditRegexTester(TesterInput), OpenRegexTester};
```

The view edits a clone of `tester.input` and returns `EditRegexTester(changed)`; `app.rs` reloads the samples only when the field, account, folder or own text changed, and re-runs the pattern every time.

- [ ] **Step 1: Write the failing tests** in a new `src/gui/regex_tester.rs`

```rust
#[cfg(test)]
mod tests {
    use eframe::egui;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::gui::app::View;
    use crate::gui::draft::Draft;
    use crate::gui::test_support::{Fixture, Wires, message};
    use crate::gui::theme::MOCHA;

    type Harness = egui_kittest::Harness<'static, crate::gui::App>;

    fn texts(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    fn inbox(fx: &Fixture) {
        for (uid, subject) in [(1, "Your verification code"), (2, "Lunch"), (3, "code 1234")] {
            fx.add("work", message("INBOX", uid, subject));
        }
    }

    pub(crate) fn open_tester(fx: &Fixture) -> (Harness, Wires) {
        let (mut harness, wires) = fx.rules_view("");
        harness.get_by_label("Regex tester").click();
        harness.run();
        (harness, wires)
    }

    pub(crate) fn type_into(harness: &mut Harness, label: &str, text: &str) {
        harness.get_by_label(label).click();
        harness.run();
        harness.get_by_label(label).type_text(text);
        harness.run();
    }

    #[test]
    fn match_ranges_marks_each_match_and_skips_the_rest() {
        let samples = texts(&["Your verification code", "Lunch", "CODE 1234 and code"]);
        assert_eq!(
            match_ranges("(?i)code", &samples).unwrap(),
            [Some(vec![18..22]), None, Some(vec![0..4, 14..18])]
        );
        assert_eq!(match_ranges("code", &samples).unwrap()[2], Some(vec![14..18]), "case-sensitive by default");
    }

    #[test]
    fn empty_and_zero_width_patterns() {
        assert_eq!(match_ranges("", &texts(&["a"])).unwrap_err(), "Enter a pattern.");
        assert_eq!(match_ranges("  ", &texts(&["a"])).unwrap_err(), "Enter a pattern.");
        assert_eq!(match_ranges("^", &texts(&["a", ""])).unwrap(), [Some(vec![]), Some(vec![])]);
        assert_eq!(match_ranges("a*", &texts(&["bab"])).unwrap(), [Some(vec![1..2])]);
    }

    #[test]
    fn an_invalid_pattern_reports_the_regex_error() {
        let error = match_ranges("(", &texts(&["a"])).unwrap_err();
        assert!(error.contains("unclosed group"), "{error}");
    }

    #[test]
    fn non_ascii_text_highlights_whole_characters() {
        let samples = texts(&["Grüße 🎉 code"]);
        let found = match_ranges("ü|🎉", &samples).unwrap();
        assert_eq!(found, [Some(vec![2..4, 8..12])]);
        let (line, ranges) = excerpt(&samples[0], found[0].as_deref().unwrap());
        for range in ranges {
            assert!(line.get(range.clone()).is_some(), "{range:?}");
        }
    }

    #[test]
    fn excerpt_shows_the_line_of_the_first_match_with_its_ranges() {
        let body = "Hello\nYour code is 123456 and 654321\nBye";
        let found = match_ranges(r"\d{6}", &texts(&[body])).unwrap();
        let (line, ranges) = excerpt(body, found[0].as_deref().unwrap());
        assert_eq!(line, "Your code is 123456 and 654321");
        assert_eq!(ranges, [13..19, 24..30]);
        assert_eq!(excerpt("one\ntwo", &[]), ("one", vec![]));
    }

    #[test]
    fn a_match_far_into_a_long_line_starts_the_excerpt_shortly_before_it() {
        let long = format!("{}code", "é".repeat(100));
        let found = match_ranges("code", &texts(&[&long])).unwrap();
        let (line, ranges) = excerpt(&long, found[0].as_deref().unwrap());
        assert_eq!(line.chars().count(), EXCERPT_LEAD + 4);
        assert_eq!(&line[ranges[0].clone()], "code");
    }

    #[test]
    fn samples_take_the_field_and_body_skips_mail_without_a_downloaded_body() {
        let mut first = message("INBOX", 1, "first");
        first.body_text = None;
        first.from_addr = None;
        let messages = [first, message("INBOX", 2, "second")];
        assert_eq!(samples(&messages, Source::Subject), ["first", "second"]);
        assert_eq!(samples(&messages, Source::From), ["", "Sender 2 <sender2@example.com>"]);
        assert_eq!(samples(&messages, Source::Body), ["Body of 2"]);
    }

    #[test]
    fn the_count_line_names_what_was_searched() {
        let found = vec![Some(vec![0..1]), None, Some(vec![])];
        assert_eq!(summary(Source::Subject, &found), "2 of 3 cached subjects match");
        assert_eq!(
            summary(Source::Body, &found),
            "2 of 3 cached bodies match · only messages whose body is downloaded"
        );
        assert_eq!(summary(Source::Own, &found), "2 of 3 lines match");
    }

    #[test]
    fn highlighted_puts_matches_on_the_accent() {
        let job = highlighted("Your code 1234", &[5..9], &MOCHA, egui::FontId::monospace(12.0));
        let parts: Vec<(&str, egui::Color32)> = job
            .sections
            .iter()
            .map(|s| (&job.text[s.byte_range.start.0..s.byte_range.end.0], s.format.background))
            .collect();
        assert_eq!(
            parts,
            [
                ("Your ", egui::Color32::TRANSPARENT),
                ("code", MOCHA.accent),
                (" 1234", egui::Color32::TRANSPARENT)
            ]
        );
    }

    #[test]
    fn a_pattern_counts_and_shows_the_matching_subjects() {
        let fx = Fixture::new(&["work"]);
        inbox(&fx);
        let (mut harness, wires) = open_tester(&fx);
        assert!(harness.query_by_label("Enter a pattern.").is_some());
        type_into(&mut harness, "Pattern", "(?i)code");
        assert!(harness.query_by_label("valid").is_some());
        assert!(harness.query_by_label("2 of 3 cached subjects match").is_some());
        assert!(harness.query_by_label("Your verification code").is_some());
        assert!(harness.query_by_label("Lunch").is_none());
        harness.get_by_label("Show non-matching").click();
        harness.run();
        assert!(harness.query_by_label("Lunch").is_some());
        assert!(wires.sent().is_empty());
    }

    #[test]
    fn an_invalid_pattern_shows_the_error_instead_of_results() {
        let fx = Fixture::new(&["work"]);
        inbox(&fx);
        let (mut harness, _wires) = open_tester(&fx);
        type_into(&mut harness, "Pattern", "(");
        assert!(harness.query_by_label("invalid").is_some());
        assert!(harness.query_by_label_contains("unclosed group").is_some());
        assert!(harness.query_by_label_contains("cached subjects match").is_none());
    }

    #[test]
    fn body_counts_only_mail_whose_body_is_downloaded() {
        let fx = Fixture::new(&["work"]);
        let mut code = message("INBOX", 1, "a");
        code.body_text = Some("Hello\nYour code is 123456\nBye".into());
        let mut missing = message("INBOX", 2, "b");
        missing.body_text = None;
        fx.add("work", code);
        fx.add("work", missing);
        let (mut harness, _wires) = open_tester(&fx);
        harness.get_by_label("Body").click();
        harness.run();
        type_into(&mut harness, "Pattern", r"\d{6}");
        assert!(
            harness
                .query_by_label("1 of 1 cached bodies match · only messages whose body is downloaded")
                .is_some()
        );
        assert!(harness.query_by_label("Your code is 123456").is_some());
    }

    #[test]
    fn own_text_tests_each_line() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = open_tester(&fx);
        harness.get_by_label("Own text").click();
        harness.run();
        type_into(&mut harness, "Your text", "sign in now");
        harness.key_press(egui::Key::Enter);
        harness.run();
        harness.get_by_label("Your text").type_text("lunch");
        harness.run();
        type_into(&mut harness, "Pattern", "sign.?in");
        assert!(harness.query_by_label("1 of 2 lines match").is_some());
    }

    #[test]
    fn copy_puts_the_pattern_on_the_clipboard() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = open_tester(&fx);
        type_into(&mut harness, "Pattern", "(?i)code");
        harness.get_by_label_contains("Copy").click();
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
        assert_eq!(copied, ["(?i)code"]);
    }

    #[test]
    fn escape_closes_only_the_tester() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = open_tester(&fx);
        harness.state_mut().rule_draft = Some(Draft::new());
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(harness.state().regex_tester.is_none());
        assert_eq!(harness.state().view, View::Rules);
        assert!(harness.state().rule_draft.is_some());
        harness.get_by_label("Regex tester").click();
        harness.run();
        harness.get_by_label("Close").click();
        harness.run();
        assert!(harness.state().regex_tester.is_none());
    }
}
```

and in `src/gui/snapshots.rs`:

```rust
#[test]
fn regex_tester() {
    let fx = mailbox("light");
    let (mut harness, _wires) = open(&fx);
    harness.state_mut().select_view(View::Rules);
    harness.run();
    harness.get_by_label("Regex tester").click();
    harness.run();
    harness.get_by_label("Pattern").click();
    harness.run();
    harness.get_by_label("Pattern").type_text("(?i)receipt|code");
    harness.run();
    snapshot(&mut harness, "gui_regex_tester");
}
```

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::regex_tester`
Expected: FAIL to compile (`match_ranges`, `Source`, `regex_tester` field … not found).

- [ ] **Step 3: Implement the module** above its tests

```rust
//! The regex tester: try a pattern on cached subjects, senders, recipients or bodies, or on your own text, before it
//! goes into a rule. It uses the `regex` crate exactly as rule conditions do.
use std::ops::Range;

use eframe::egui;

use crate::message::clean;
use crate::store::Message;

use super::app::{App, UiAction};
use super::icons;
use super::theme::{self, Palette};

/// Characters of a long line shown before its first match.
pub(crate) const EXCERPT_LEAD: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Source {
    Subject,
    From,
    To,
    Body,
    Own,
}

impl Source {
    pub const ALL: [Source; 5] = [Source::Subject, Source::From, Source::To, Source::Body, Source::Own];

    pub fn label(self) -> &'static str {
        match self {
            Source::Body => "Body",
            Source::From => "From",
            Source::Own => "Own text",
            Source::Subject => "Subject",
            Source::To => "To",
        }
    }

    /// What the samples are called in "6 of 214 cached subjects match".
    pub fn noun(self) -> &'static str {
        match self {
            Source::Body => "cached bodies",
            Source::From => "cached senders",
            Source::Own => "lines",
            Source::Subject => "cached subjects",
            Source::To => "cached recipients",
        }
    }
}

/// What the person set in the dialog; the view edits a copy and returns it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TesterInput {
    pub account: usize,
    pub folder: String,
    pub own_text: String,
    pub pattern: String,
    pub show_non_matching: bool,
    pub source: Source,
}

/// Per sample: the matched byte ranges (empty for a zero-width match), or None when it did not match.
pub(crate) type Found = Vec<Option<Vec<Range<usize>>>>;

pub(crate) struct RegexTester {
    pub input: TesterInput,
    /// The pattern run on `samples`; Err when it is empty or does not compile.
    pub results: Result<Found, String>,
    pub samples: Vec<String>,
}

/// The texts `source` tests, one per message; Body skips messages whose body is not downloaded.
pub(crate) fn samples(messages: &[Message], source: Source) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| match source {
            Source::Body => m.body_text.clone(),
            Source::From => Some(m.from_addr.clone().unwrap_or_default()),
            Source::Own => None,
            Source::Subject => Some(m.subject.clone().unwrap_or_default()),
            Source::To => Some(m.to_addr.clone().unwrap_or_default()),
        })
        .collect()
}

/// Runs `pattern` on every sample. A blank pattern is refused, as `rules::compile` refuses it.
pub(crate) fn match_ranges(pattern: &str, samples: &[String]) -> Result<Found, String> {
    if pattern.trim().is_empty() {
        return Err("Enter a pattern.".into());
    }
    let regex = regex::Regex::new(pattern).map_err(|e| e.to_string())?;
    Ok(samples
        .iter()
        .map(|text| {
            regex.is_match(text).then(|| {
                regex
                    .find_iter(text)
                    .map(|m| m.range())
                    .filter(|range| !range.is_empty())
                    .collect()
            })
        })
        .collect())
}

/// The line holding the first range, starting at most `EXCERPT_LEAD` characters before it, with the ranges inside it
/// moved to match; the first line when there is no range.
pub(crate) fn excerpt<'a>(text: &'a str, ranges: &[Range<usize>]) -> (&'a str, Vec<Range<usize>>) {
    let Some(first) = ranges.first() else {
        return (text.lines().next().unwrap_or(""), Vec::new());
    };
    let line_start = text[..first.start].rfind('\n').map_or(0, |at| at + 1);
    let start = text[line_start..first.start]
        .char_indices()
        .rev()
        .nth(EXCERPT_LEAD - 1)
        .map_or(line_start, |(at, _)| line_start + at);
    let end = text[first.end..]
        .find('\n')
        .map_or(text.len(), |at| first.end + at);
    let moved = ranges
        .iter()
        .filter(|range| range.start >= start && range.end <= end)
        .map(|range| range.start - start..range.end - start)
        .collect();
    (&text[start..end], moved)
}

/// "6 of 214 cached subjects match".
pub(crate) fn summary(source: Source, found: &[Option<Vec<Range<usize>>>]) -> String {
    let matched = found.iter().filter(|ranges| ranges.is_some()).count();
    let note = if source == Source::Body {
        " · only messages whose body is downloaded"
    } else {
        ""
    };
    format!("{matched} of {} {} match{note}", found.len(), source.noun())
}

/// `line` with `ranges` drawn on the accent. Ranges come from the raw text, so each piece is cleaned on its own.
pub(crate) fn highlighted(
    line: &str,
    ranges: &[Range<usize>],
    palette: &Palette,
    font: egui::FontId,
) -> egui::text::LayoutJob {
    let plain = egui::TextFormat {
        font_id: font.clone(),
        color: palette.text,
        ..Default::default()
    };
    let hit = egui::TextFormat {
        font_id: font,
        color: palette.on_accent,
        background: palette.accent,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    let mut at = 0;
    for range in ranges {
        job.append(&clean(&line[at..range.start], false), 0.0, plain.clone());
        job.append(&clean(&line[range.clone()], false), 0.0, hit.clone());
        at = range.end;
    }
    job.append(&clean(&line[at..], false), 0.0, plain);
    job
}

pub(crate) fn show(app: &App, ctx: &egui::Context) -> Vec<UiAction> {
    let Some(tester) = &app.regex_tester else {
        return Vec::new();
    };
    let mut input = tester.input.clone();
    let modal = egui::Modal::new(egui::Id::new("regex_tester")).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.heading("Regex tester");
        pattern_row(ui, tester, &mut input);
        ui.weak("Rust regex syntax. Start with (?i) to ignore case.");
        source_row(app, ui, &mut input);
        if input.source == Source::Own {
            let label = ui.label("Your text");
            theme::text_field(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut input.own_text)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                )
            })
            .labelled_by(label.id);
        }
        results(ui, tester, &mut input.show_non_matching);
        ui.separator();
        ui.weak("Runs on mail already on this device; nothing is sent.");
        let (_, clicked) = egui::Sides::new().show(
            ui,
            |_| {},
            |ui| {
                let mut clicked = Vec::new();
                if ui.button("Close").clicked() {
                    clicked.push(UiAction::CloseRegexTester);
                }
                clicked
            },
        );
        clicked
    });
    let mut actions = Vec::new();
    if input != tester.input {
        actions.push(UiAction::EditRegexTester(input));
    }
    actions.extend(modal.inner);
    if modal.should_close() {
        actions.push(UiAction::CloseRegexTester);
    }
    actions
}

fn pattern_row(ui: &mut egui::Ui, tester: &RegexTester, input: &mut TesterInput) {
    let palette = theme::palette(ui);
    ui.horizontal(|ui| {
        let label = ui.label("Pattern");
        theme::text_field(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut input.pattern)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(340.0),
            )
        })
        .labelled_by(label.id);
        match &tester.results {
            Ok(_) => ui.colored_label(palette.success, "valid"),
            Err(_) if tester.input.pattern.trim().is_empty() => ui.weak("empty"),
            Err(_) => ui.colored_label(palette.error, "invalid"),
        };
        if ui.button(format!("{} Copy", icons::COPY)).clicked() {
            ui.ctx().copy_text(input.pattern.clone());
        }
    });
}

fn source_row(app: &App, ui: &mut egui::Ui, input: &mut TesterInput) {
    ui.horizontal(|ui| {
        ui.label("Test on");
        for source in Source::ALL {
            ui.selectable_value(&mut input.source, source, source.label());
        }
        if input.source == Source::Own {
            return;
        }
        egui::ComboBox::from_id_salt("regex_tester_folder")
            .selected_text(folder_label(app, input.account, &input.folder))
            .show_ui(ui, |ui| {
                for (index, account) in app.accounts.iter().enumerate() {
                    for folder in &account.folders {
                        let chosen = input.account == index && input.folder == folder.name;
                        if ui
                            .selectable_label(chosen, folder_label(app, index, &folder.name))
                            .clicked()
                        {
                            input.account = index;
                            input.folder = folder.name.clone();
                        }
                    }
                }
            });
    });
}

/// "work · INBOX" when there is more than one account, else the folder alone.
fn folder_label(app: &App, account: usize, folder: &str) -> String {
    let folder = clean(folder, false);
    match app.accounts.get(account) {
        Some(a) if app.accounts.len() > 1 => format!("{} · {folder}", clean(&a.name, false)),
        _ => folder,
    }
}

fn results(ui: &mut egui::Ui, tester: &RegexTester, show_non_matching: &mut bool) {
    let palette = theme::palette(ui);
    let found = match &tester.results {
        Ok(found) => found,
        Err(error) => {
            ui.label(egui::RichText::new(clean(error, true)).monospace().color(palette.error));
            return;
        }
    };
    ui.horizontal(|ui| {
        ui.label(summary(tester.input.source, found));
        ui.checkbox(show_non_matching, "Show non-matching");
    });
    let rows: Vec<usize> = (0..found.len())
        .filter(|&index| *show_non_matching || found[index].is_some())
        .collect();
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let height = ui.text_style_height(&egui::TextStyle::Monospace);
    egui::ScrollArea::vertical()
        .max_height(280.0)
        .auto_shrink([false, true])
        .show_rows(ui, height, rows.len(), |ui, range| {
            for &index in &rows[range] {
                let text = tester.samples.get(index).map_or("", String::as_str);
                let ranges = found[index].as_deref().unwrap_or(&[]);
                let (line, ranges) = excerpt(text, ranges);
                ui.add(egui::Label::new(highlighted(line, &ranges, palette, font.clone())).truncate());
            }
        });
}
```

`src/gui/icons.rs`: `pub(crate) const COPY: &str = ph::COPY;`. `src/gui/mod.rs`: `mod regex_tester;`.

- [ ] **Step 4: The header button** in `src/gui/rules.rs`: in `show_rules`, the header's right side adds a third button after "Open rules.toml", so it sits leftmost:

```rust
            if ui.button("Regex tester").clicked() {
                clicked.push(UiAction::OpenRegexTester);
            }
```

- [ ] **Step 5: Wire it** in `src/gui/app.rs`

`use super::regex_tester::{self, RegexTester, Source, TesterInput, match_ranges, samples};`. Field `pub(crate) regex_tester: Option<RegexTester>,` (`None` in `App::new`). `UiAction` gains `CloseRegexTester`, `EditRegexTester(TesterInput)`, `OpenRegexTester`. In `App::show`, after `show_move_picker`:

```rust
        actions.extend(regex_tester::show(self, &ctx));
```

The `Escape` arm closes the tester first:

```rust
            UiAction::Escape => {
                if self.regex_tester.is_some() {
                    self.regex_tester = None;
                } else if self.move_picker.is_some() {
```

(the rest of the chain unchanged), and new arms:

```rust
            UiAction::CloseRegexTester => self.regex_tester = None,
            UiAction::EditRegexTester(input) => self.edit_regex_tester(input),
            UiAction::OpenRegexTester => self.open_regex_tester(),
```

with, beside `preview`:

```rust
    /// Opens the regex tester on the subjects of the folder the open rule watches, or of INBOX.
    fn open_regex_tester(&mut self) {
        let draft = self.rule_draft.as_ref();
        let account = draft
            .and_then(|d| d.account.as_deref())
            .and_then(|name| self.accounts.iter().position(|a| a.name == name))
            .unwrap_or(0);
        let folder = draft
            .and_then(|d| d.folder.clone())
            .unwrap_or_else(|| "INBOX".into());
        let input = TesterInput {
            account,
            folder,
            own_text: String::new(),
            pattern: String::new(),
            show_non_matching: false,
            source: Source::Subject,
        };
        let samples = self.tester_samples(&input);
        let results = match_ranges(&input.pattern, &samples);
        self.regex_tester = Some(RegexTester {
            input,
            results,
            samples,
        });
    }

    /// Takes the dialog's new input: texts are reloaded only when what they come from changed.
    fn edit_regex_tester(&mut self, input: TesterInput) {
        let Some(old) = self.regex_tester.take() else { return };
        let same_texts = old.input.source == input.source
            && old.input.account == input.account
            && old.input.folder == input.folder
            && old.input.own_text == input.own_text;
        let samples = if same_texts {
            old.samples
        } else {
            self.tester_samples(&input)
        };
        let results = match_ranges(&input.pattern, &samples);
        self.regex_tester = Some(RegexTester {
            input,
            results,
            samples,
        });
    }

    /// The texts the tester runs on: the lines of its own text, or one field of each message cached in its folder.
    /// ponytail: loads the whole folder, bodies included, when the field or folder changes; page it if a very large
    /// folder stalls the window.
    fn tester_samples(&self, input: &TesterInput) -> Vec<String> {
        if input.source == Source::Own {
            return input.own_text.lines().map(str::to_string).collect();
        }
        let Some(Ok(store)) = self.accounts.get(input.account).map(|a| &a.store) else {
            return Vec::new();
        };
        match store.messages_in_folder(&input.folder) {
            Ok(messages) => samples(&messages, input.source),
            Err(e) => {
                log::warn!("could not read {} for the regex tester: {e}", input.folder);
                Vec::new()
            }
        }
    }
```

- [ ] **Step 6: Run the GUI tests and the architecture check**

Run: `cargo test --lib gui:: && cargo test --test architecture`
Expected: PASS. `regex_tester.rs` is a view: `&App` in, `UiAction`s out; `copy_text` is output, not state.

- [ ] **Step 7: Docs** — in `docs/src/gui.md`, after the Rules paragraph:

```markdown
**Regex tester**, at the top of the Rules view, tries a pattern on the subjects, senders, recipients or downloaded bodies of a folder, or on text you type, and highlights what it matches. It runs on the mail already on this device and sends nothing.
```

- [ ] **Step 8: Snapshots on Linux** (`gui_regex_tester` is new; `gui_rules` gains the button), then commit

```bash
git add src/gui/ docs/src/gui.md tests/snapshots/
git commit -m "feat(gui): add a regex tester to the Rules view"
```

---

### Task 2: "Use in rule"

**Files:**
- Modify: `src/gui/draft.rs` (`Draft.selected`), `src/gui/rule_editor.rs` (track the focused condition), `src/gui/regex_tester.rs` (button, tests), `src/gui/app.rs` (`UiAction::UseRegex`, prefill on open), `docs/src/gui.md`

**Interfaces:**
- Consumes: everything from Task 1; `Draft`, `Condition`, `Field`, `Op` and `rule_editor::value_field` (rules-editor plan).
- Produces: `Draft.selected: Option<usize>` (the condition whose value field was focused last; cleared when a condition is removed), `UiAction::UseRegex`.

"Use in rule" puts the pattern into the selected condition when it is a text condition (operator becomes regex), else into the draft's condition on the tested field, else adds a regex condition on that field (Own text counts as Subject). With no rule open it starts a new draft. The tester then closes. When the tester opens on a selected regex condition, it starts from that pattern and field.

- [ ] **Step 1: Write the failing tests** in the `tests` module of `src/gui/regex_tester.rs`

```rust
    use egui_kittest::kittest::NodeT;

    use crate::gui::draft::{Condition, Field, Op};

    const RULES: &str = "[[rules]]\nname = \"newsletters\"\nmatch.from = { contains = \"news@\" }\nactions = [\"archive\"]\n";

    #[test]
    fn use_in_rule_fills_the_condition_last_focused() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.rules_view(RULES);
        harness.get_by_label("newsletters").click();
        harness.run();
        harness
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .find(|node| node.value().as_deref() == Some("news@"))
            .unwrap()
            .click();
        harness.run();
        assert_eq!(harness.state().rule_draft.as_ref().unwrap().selected, Some(0));
        harness.get_by_label("Regex tester").click();
        harness.run();
        assert_eq!(harness.state().regex_tester.as_ref().unwrap().input.source, Source::From);
        type_into(&mut harness, "Pattern", "^news@");
        harness.get_by_label("Use in rule").click();
        harness.run();
        let draft = harness.state().rule_draft.clone().unwrap();
        assert_eq!(
            (draft.conditions[0].field, draft.conditions[0].op, draft.conditions[0].value.as_str()),
            (Field::From, Op::Regex, "^news@")
        );
        assert!(harness.state().regex_tester.is_none());
        assert_eq!(std::fs::read_to_string(fx.paths.rules_file()).unwrap(), RULES, "nothing saved before Save");

        harness.get_by_label("Regex tester").click();
        harness.run();
        let tester = harness.state().regex_tester.as_ref().unwrap();
        assert_eq!((tester.input.pattern.as_str(), tester.input.source), ("^news@", Source::From));
    }

    #[test]
    fn use_in_rule_without_an_open_rule_starts_one() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = open_tester(&fx);
        type_into(&mut harness, "Pattern", "(?i)code");
        harness.get_by_label("Use in rule").click();
        harness.run();
        let draft = harness.state().rule_draft.clone().unwrap();
        assert_eq!(
            draft.conditions,
            [Condition {
                field: Field::Subject,
                header: String::new(),
                op: Op::Regex,
                value: "(?i)code".into(),
            }]
        );
        assert!(draft.original.is_none());
    }

    #[test]
    fn use_in_rule_is_off_until_the_pattern_is_valid() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = open_tester(&fx);
        assert!(harness.get_by_label("Use in rule").accesskit_node().is_disabled());
        type_into(&mut harness, "Pattern", "(");
        assert!(harness.get_by_label("Use in rule").accesskit_node().is_disabled());
    }
```

- [ ] **Step 2: Run them**

Run: `cargo test --lib gui::regex_tester`
Expected: FAIL to compile (`selected`, `UseRegex` not found).

- [ ] **Step 3: Track the focused condition**

`src/gui/draft.rs`, in `Draft` (alphabetical, after `proposed_by`):

```rust
    /// The condition the regex tester's "Use in rule" fills: the one whose value field was focused last.
    pub selected: Option<usize>,
```

`src/gui/rule_editor.rs`, in `conditions`: keep the value field's response and note focus,

```rust
    let mut focused = None;
```

before the loop,

```rust
            if value_field(ui, condition, index).gained_focus() {
                focused = Some(index);
            }
```

in place of the bare `value_field(ui, condition, index);`, and after the loop:

```rust
    if focused.is_some() {
        draft.selected = focused;
    }
    if let Some(index) = remove {
        draft.conditions.remove(index);
        draft.selected = None;
    }
```

(replacing the existing `if let Some(index) = remove` block).

- [ ] **Step 4: The button** in `regex_tester::show`: the footer's right side adds "Use in rule" before "Close", so it sits rightmost:

```rust
            |ui| {
                let mut clicked = Vec::new();
                if ui
                    .add_enabled(tester.results.is_ok(), egui::Button::new("Use in rule"))
                    .clicked()
                {
                    clicked.push(UiAction::UseRegex);
                }
                if ui.button("Close").clicked() {
                    clicked.push(UiAction::CloseRegexTester);
                }
                clicked
            },
```

- [ ] **Step 5: Wire it** in `src/gui/app.rs`

`use super::draft::{self, Condition, Draft, Field, Op, RuleSeed};`. `UiAction::UseRegex`, arm `UiAction::UseRegex => self.use_regex(),`, and:

```rust
    /// Puts the tester's pattern into the condition last focused in the editor, or the draft's condition on the tested
    /// field, or a new regex condition; starts a new rule when none is open. Nothing is saved.
    fn use_regex(&mut self) {
        let Some(tester) = self.regex_tester.take() else { return };
        let field = match tester.input.source {
            Source::Body => Field::Body,
            Source::From => Field::From,
            Source::Own | Source::Subject => Field::Subject,
            Source::To => Field::To,
        };
        let draft = self.rule_draft.get_or_insert_with(Draft::new);
        let target = draft
            .selected
            .filter(|&index| draft.conditions.get(index).is_some_and(|c| c.field.is_text()))
            .or_else(|| draft.conditions.iter().position(|c| c.field == field));
        match target.and_then(|index| draft.conditions.get_mut(index)) {
            Some(condition) => {
                condition.op = Op::Regex;
                condition.value = tester.input.pattern;
            }
            None => {
                draft.conditions.push(Condition {
                    op: Op::Regex,
                    value: tester.input.pattern,
                    ..Condition::new(field)
                });
                draft.selected = Some(draft.conditions.len() - 1);
            }
        }
        draft.tested = None;
    }
```

and in `open_regex_tester`, the `pattern` and `source` come from the selected condition:

```rust
        let condition = draft.and_then(|d| d.selected.and_then(|index| d.conditions.get(index)));
        let pattern = condition
            .filter(|c| c.op == Op::Regex)
            .map(|c| c.value.clone())
            .unwrap_or_default();
        let source = match condition.map(|c| c.field) {
            Some(Field::Body) => Source::Body,
            Some(Field::From) => Source::From,
            Some(Field::To) => Source::To,
            _ => Source::Subject,
        };
```

with `pattern,` and `source,` in the `TesterInput` literal.

- [ ] **Step 6: Run the GUI tests**

Run: `cargo test --lib gui::`
Expected: PASS, including the rules-editor tests (removing a condition clears `selected`).

- [ ] **Step 7: Docs** — extend the Regex tester sentence in `docs/src/gui.md`:

```markdown
Use in rule puts the pattern into the condition you were editing, or adds one; the rule is saved only when you press Save.
```

- [ ] **Step 8: Snapshots on Linux** (`gui_regex_tester` gains the button), then commit

```bash
git add src/gui/ docs/src/gui.md tests/snapshots/
git commit -m "feat(gui): put a tested pattern into the rule being edited"
```

---

## Finish

- [ ] Full gate: `cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && cargo machete && cargo audit`
- [ ] Snapshots regenerated on Linux with lavapipe, and every changed PNG looked at: `gui_rules`, `gui_regex_tester`.
- [ ] Report which platforms the PR was compiled on and ran on.
