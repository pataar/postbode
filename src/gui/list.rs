//! The middle column: one row per thread, followed by its members when expanded.
use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Local, TimeZone};
use eframe::egui;

use crate::message::clean;
use crate::rules::Action;
use crate::store::{MessageSummary, ThreadSummary};

use super::app::{App, UiAction, View};

/// Threads loaded per folder. ponytail: older mail is reachable through search; page by date if that is not enough.
pub(crate) const THREAD_LIMIT: u32 = 10_000;
const ROW_HEIGHT: f32 = 22.0;

pub(crate) type RowKey = (String, u32);

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row {
    pub count: u32,
    pub date: i64,
    pub flagged: bool,
    pub folder: String,
    pub from: String,
    pub member: bool,
    pub subject: String,
    /// Set on thread rows, so actions and Right/Left reach the whole thread.
    pub thread_id: Option<String>,
    pub to: String,
    pub uid: u32,
    pub unread: bool,
}

impl Row {
    pub fn key(&self) -> RowKey {
        (self.folder.clone(), self.uid)
    }

    fn from_summary(folder: &str, message: &MessageSummary, member: bool) -> Row {
        Row {
            count: 1,
            date: message.date,
            flagged: message.is_flagged(),
            folder: folder.to_string(),
            from: message.from.clone(),
            member,
            subject: message.subject.clone(),
            thread_id: None,
            to: message.to.clone(),
            uid: message.uid,
            unread: !message.is_seen(),
        }
    }
}

#[derive(Default)]
pub(crate) struct ListState {
    pub cursor: usize,
    pub expanded: HashMap<String, Vec<MessageSummary>>,
    /// Set when a key moved the cursor, so this frame scrolls it into view.
    pub follow_cursor: bool,
    pub marked: HashSet<RowKey>,
    pub rows: Vec<Row>,
    pub threads: Vec<ThreadSummary>,
    /// Scroll offset and height of the list from the last frame.
    pub viewport: (f32, f32),
}

/// One row per thread describing its latest message, followed by its members when expanded.
pub(crate) fn build_rows(
    folder: &str,
    threads: &[ThreadSummary],
    expanded: &HashMap<String, Vec<MessageSummary>>,
) -> Vec<Row> {
    let mut rows = Vec::with_capacity(threads.len());
    for thread in threads {
        let mut row = Row::from_summary(folder, &thread.latest, false);
        row.count = thread.count;
        row.flagged = thread.flagged;
        row.thread_id = Some(thread.thread_id.clone());
        row.unread = thread.unread;
        rows.push(row);
        if let Some(members) = expanded.get(&thread.thread_id) {
            rows.extend(members.iter().map(|m| Row::from_summary(folder, m, true)));
        }
    }
    rows
}

/// What a sent action will do to a row, shown until its `ActionDone` arrives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Optimistic {
    Flagged(bool),
    Hidden,
    Seen(bool),
}

impl Optimistic {
    pub fn of(action: &Action) -> Option<Optimistic> {
        match action {
            Action::Archive | Action::Delete | Action::Move(_) | Action::Trash => {
                Some(Optimistic::Hidden)
            }
            Action::Flag => Some(Optimistic::Flagged(true)),
            Action::MarkRead => Some(Optimistic::Seen(true)),
            Action::MarkUnread => Some(Optimistic::Seen(false)),
            Action::Unflag => Some(Optimistic::Flagged(false)),
            Action::Notify | Action::Silent => None,
        }
    }
}

/// Rows of sent actions as they will be: moved rows hidden, read and flag changes shown.
pub(crate) fn apply_pending(rows: &mut Vec<Row>, pending: &HashMap<RowKey, Optimistic>) {
    rows.retain(|row| pending.get(&row.key()) != Some(&Optimistic::Hidden));
    for row in rows {
        match pending.get(&row.key()) {
            Some(Optimistic::Flagged(flagged)) => row.flagged = *flagged,
            Some(Optimistic::Seen(seen)) => row.unread = !seen,
            Some(Optimistic::Hidden) | None => {}
        }
    }
}

/// "Alice <alice@x>, bob@y" is "Alice"; without a name, the first address.
pub(crate) fn display_name(field: &str) -> String {
    let field = field.trim();
    if let Some(rest) = field.strip_prefix('"')
        && let Some((name, _)) = rest.split_once('"')
        && !name.trim().is_empty()
    {
        return name.trim().to_string();
    }
    let first = field.split(',').next().unwrap_or("").trim();
    match first.split_once('<') {
        Some((name, address)) => {
            let name = name.trim().trim_matches('"').trim();
            if name.is_empty() {
                address.trim_end_matches('>').trim().to_string()
            } else {
                name.to_string()
            }
        }
        None => first.to_string(),
    }
}

/// The time if `ts` is today, the weekday within the past week, else the date.
pub(crate) fn list_date<Tz: TimeZone>(ts: i64, now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let Some(at) = now.timezone().timestamp_opt(ts, 0).single() else {
        return String::new();
    };
    match now
        .date_naive()
        .signed_duration_since(at.date_naive())
        .num_days()
    {
        0 => at.format("%H:%M").to_string(),
        1..=6 => at.format("%a").to_string(),
        _ => at.format("%Y-%m-%d").to_string(),
    }
}

/// Unread dot, flag, sender (recipient in Sent and Drafts), subject, thread count and date, on one line.
pub(crate) fn row_text<Tz: TimeZone>(row: &Row, recipient: bool, now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let who = display_name(if recipient { &row.to } else { &row.from });
    let mut text = String::from(if row.unread { "• " } else { "   " });
    if row.flagged {
        text.push_str("⚑ ");
    }
    if row.member {
        text.push_str("      ");
    }
    text.push_str(&format!("{who} — {}", row.subject));
    if row.count > 1 {
        text.push_str(&format!(" ({})", row.count));
    }
    text.push_str(&format!("  ·  {}", list_date(row.date, now)));
    clean(&text, false)
}

/// The scroll offset that shows the cursor row, moving the view as little as possible.
pub(crate) fn offset_showing(cursor: usize, row_height: f32, offset: f32, height: f32) -> f32 {
    let top = cursor as f32 * row_height;
    let bottom = top + row_height;
    if top < offset {
        top
    } else if bottom > offset + height {
        bottom - height
    } else {
        offset
    }
}

pub(crate) fn show(app: &App, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let list = &app.list;
    let recipient = app.shows_recipient();
    let now = Local::now();
    let mut area = egui::ScrollArea::vertical().auto_shrink(false);
    if list.follow_cursor {
        let row_height = ROW_HEIGHT + ui.spacing().item_spacing.y;
        let (offset, height) = list.viewport;
        area = area.vertical_scroll_offset(offset_showing(list.cursor, row_height, offset, height));
    }
    let output = area.show_rows(ui, ROW_HEIGHT, list.rows.len(), |ui, range| {
        for index in range {
            let row = &list.rows[index];
            let marked = list.marked.contains(&row.key());
            let mut text = row_text(row, recipient, &now);
            if marked {
                text = format!("✔ {text}");
            }
            let button = egui::Button::selectable(index == list.cursor || marked, text)
                .truncate()
                .min_size(egui::vec2(ui.available_width(), ROW_HEIGHT));
            if ui.add(button).clicked() {
                actions.push(UiAction::SelectRow(index));
            }
        }
    });
    actions.push(UiAction::ListViewport {
        offset: output.state.offset.y,
        height: output.inner_rect.height(),
    });
    actions
}

/// The `m` popup: type to filter the account's folders, Enter or a click moves.
pub(crate) fn show_move_picker(app: &App, ctx: &egui::Context) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let (Some(filter), View::Folder { account, folder }) = (&app.move_picker, &app.view) else {
        return actions;
    };
    let needle = filter.to_lowercase();
    let names: Vec<&str> = app.accounts[*account]
        .folders
        .iter()
        .map(|f| f.name.as_str())
        .filter(|name| name != folder && name.to_lowercase().contains(&needle))
        .collect();
    egui::Window::new("Move to")
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            let mut text = filter.clone();
            let response = ui.add(egui::TextEdit::singleline(&mut text).hint_text("Folder"));
            response.request_focus();
            if response.changed() {
                actions.push(UiAction::MoveFilter(text));
            }
            if ui.input(|i| i.key_pressed(egui::Key::Enter))
                && let Some(first) = names.first()
            {
                actions.push(UiAction::MoveTo(first.to_string()));
            }
            for name in &names {
                if ui.selectable_label(false, clean(name, false)).clicked() {
                    actions.push(UiAction::MoveTo(name.to_string()));
                }
            }
        });
    actions
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use eframe::egui;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::gui::app::{Focus, View};
    use crate::gui::test_support::{Fixture, message};
    use crate::store::{MessageSummary, ThreadSummary};

    fn summary(uid: u32, flags: &str) -> MessageSummary {
        MessageSummary {
            uid,
            from: format!("Sender {uid} <s{uid}@example.com>"),
            to: "me@example.com".into(),
            subject: format!("subject {uid}"),
            date: 1_790_000_000 + i64::from(uid),
            flags: flags.into(),
        }
    }

    fn has_row(harness: &egui_kittest::Harness<'_, crate::gui::App>, uid: u32) -> bool {
        harness.state().list.rows.iter().any(|row| row.uid == uid)
    }

    #[test]
    fn build_rows_puts_members_under_an_expanded_thread() {
        let threads = vec![
            ThreadSummary {
                thread_id: "t".into(),
                latest: summary(5, ""),
                count: 2,
                unread: true,
                flagged: false,
            },
            ThreadSummary {
                thread_id: "u".into(),
                latest: summary(3, "\\Seen \\Flagged"),
                count: 1,
                unread: false,
                flagged: true,
            },
        ];
        let mut expanded = HashMap::new();
        assert_eq!(build_rows("INBOX", &threads, &expanded).len(), 2);
        expanded.insert("t".to_string(), vec![summary(4, "\\Seen"), summary(5, "")]);
        let rows = build_rows("INBOX", &threads, &expanded);
        let shape: Vec<(u32, bool, bool, u32)> = rows
            .iter()
            .map(|r| (r.uid, r.member, r.unread, r.count))
            .collect();
        assert_eq!(
            shape,
            [
                (5, false, true, 2),
                (4, true, false, 1),
                (5, true, true, 1),
                (3, false, false, 1)
            ]
        );
        assert_eq!(rows[0].thread_id.as_deref(), Some("t"));
        assert!(rows[3].flagged && rows[1].thread_id.is_none());
    }

    #[test]
    fn list_date_is_time_today_weekday_this_week_else_the_date() {
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let at = |d, h, m| {
            Utc.with_ymd_and_hms(2026, 10, d, h, m, 0)
                .unwrap()
                .timestamp()
        };
        assert_eq!(list_date(at(6, 9, 30), &now), "09:30");
        assert_eq!(list_date(at(4, 18, 0), &now), "Sun");
        assert_eq!(
            list_date(
                Utc.with_ymd_and_hms(2026, 9, 26, 8, 0, 0)
                    .unwrap()
                    .timestamp(),
                &now
            ),
            "2026-09-26"
        );
    }

    #[test]
    fn display_name_prefers_the_name_then_the_address() {
        assert_eq!(display_name("Alice <alice@example.com>"), "Alice");
        assert_eq!(
            display_name("\"Doe, John\" <john@example.com>, bob@example.com"),
            "Doe, John"
        );
        assert_eq!(display_name("<bare@example.com>"), "bare@example.com");
        assert_eq!(
            display_name("plain@example.com, other@example.com"),
            "plain@example.com"
        );
        assert_eq!(display_name(""), "");
    }

    #[test]
    fn offset_showing_scrolls_only_as_far_as_needed() {
        assert_eq!(offset_showing(0, 20.0, 0.0, 100.0), 0.0);
        assert_eq!(offset_showing(4, 20.0, 0.0, 100.0), 0.0);
        assert_eq!(offset_showing(5, 20.0, 0.0, 100.0), 20.0);
        assert_eq!(offset_showing(1, 20.0, 60.0, 100.0), 20.0);
    }

    #[test]
    fn row_text_strips_control_characters_from_headers() {
        let row = Row {
            count: 1,
            date: 0,
            flagged: true,
            folder: "INBOX".into(),
            from: "Evil\u{1b}[2J <e@example.com>".into(),
            member: false,
            subject: "\u{1b}[31mred\u{7} alert".into(),
            thread_id: None,
            to: String::new(),
            uid: 1,
            unread: true,
        };
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let text = row_text(&row, false, &now);
        assert!(!text.chars().any(char::is_control), "{text:?}");
        assert!(
            text.contains("[31mred alert") && text.starts_with("• ⚑ "),
            "{text}"
        );
    }

    #[test]
    fn j_and_k_move_through_rows_newest_first() {
        let fx = Fixture::new(&["work"]);
        for uid in 1..=3 {
            fx.add("work", message("INBOX", uid, &format!("subject {uid}")));
        }
        let (mut harness, _wires) = fx.harness();
        assert!(
            harness
                .query_by_label_contains("Sender 3 — subject 3")
                .is_some()
        );
        let uids: Vec<u32> = harness.state().list.rows.iter().map(|r| r.uid).collect();
        assert_eq!(uids, [3, 2, 1]);
        harness.event(egui::Event::Text("j".into()));
        harness.run();
        harness.key_press(egui::Key::ArrowDown);
        harness.run();
        assert_eq!(harness.state().list.cursor, 2);
        harness.event(egui::Event::Text("j".into()));
        harness.run();
        assert_eq!(
            harness.state().list.cursor,
            2,
            "the cursor stops at the last row"
        );
        harness.event(egui::Event::Text("k".into()));
        harness.run();
        assert_eq!(harness.state().list.cursor, 1);
    }

    #[test]
    fn right_expands_a_thread_and_left_collapses_it_from_a_member() {
        let fx = Fixture::new(&["work"]);
        for uid in [4, 5] {
            let mut m = message("INBOX", uid, "thread");
            m.thread_id = "<t@example.com>".into();
            fx.add("work", m);
        }
        fx.add("work", message("INBOX", 1, "alone"));
        let (mut harness, _wires) = fx.harness();
        assert_eq!(harness.state().list.rows.len(), 2);
        assert!(harness.query_by_label_contains("thread (2)").is_some());
        harness.key_press(egui::Key::ArrowRight);
        harness.run();
        assert_eq!(harness.state().list.rows.len(), 4);
        harness.key_press(egui::Key::ArrowDown);
        harness.run();
        assert!(harness.state().list.rows[1].member);
        harness.key_press(egui::Key::ArrowLeft);
        harness.run();
        assert_eq!(
            (harness.state().list.rows.len(), harness.state().list.cursor),
            (2, 0)
        );
        assert!(has_row(&harness, 1));
    }

    #[test]
    fn x_marks_rows_and_escape_clears_them() {
        let fx = Fixture::new(&["work"]);
        for uid in 1..=2 {
            fx.add("work", message("INBOX", uid, "hi"));
        }
        let (mut harness, _wires) = fx.harness();
        harness.event(egui::Event::Text("x".into()));
        harness.event(egui::Event::Text("j".into()));
        harness.event(egui::Event::Text("x".into()));
        harness.run();
        assert_eq!(harness.state().list.marked.len(), 2);
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(harness.state().list.marked.is_empty());
    }

    #[test]
    fn tab_cycles_focus_and_j_in_the_folder_pane_opens_the_next_folder() {
        let fx = Fixture::new(&["work"]);
        fx.folder("work", "Archive", Some("Archive"));
        fx.add("work", message("Archive", 7, "archived"));
        let (mut harness, _wires) = fx.harness();
        harness.key_press(egui::Key::Tab);
        harness.run();
        assert_eq!(harness.state().focus, Focus::Body);
        harness.key_press(egui::Key::Tab);
        harness.run();
        assert_eq!(harness.state().focus, Focus::Folders);
        harness.event(egui::Event::Text("j".into()));
        harness.run();
        assert_eq!(
            harness.state().view,
            View::Folder {
                account: 0,
                folder: "Archive".into()
            }
        );
        assert!(has_row(&harness, 7));
    }

    #[test]
    fn keys_on_an_account_without_a_store_do_nothing() {
        let fx = Fixture::new(&["work"]);
        let db = fx.paths.mail_db("work");
        std::fs::remove_file(&db).unwrap();
        std::fs::create_dir_all(&db).unwrap();
        let (mut harness, _wires) = fx.harness();
        for key in ["j", "k", "x"] {
            harness.event(egui::Event::Text(key.into()));
        }
        harness.key_press(egui::Key::ArrowRight);
        harness.key_press(egui::Key::ArrowLeft);
        harness.run();
        assert!(harness.state().list.rows.is_empty());
        assert!(harness.state().list.marked.is_empty());
        assert_eq!(harness.state().list.cursor, 0);
    }

    #[test]
    fn tab_and_arrows_leave_no_egui_widget_focus_for_space_or_enter_to_click() {
        let fx = Fixture::new(&["work"]);
        fx.folder("work", "Archive", Some("Archive"));
        for uid in 1..=3 {
            fx.add("work", message("INBOX", uid, "hi"));
        }
        let (mut harness, _wires) = fx.harness();
        let view = harness.state().view.clone();
        harness.key_press(egui::Key::Tab);
        harness.run();
        harness.key_press(egui::Key::Space);
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert_eq!(harness.state().view, view);
        assert_eq!(harness.state().list.cursor, 0);
        assert_eq!(harness.ctx.memory(|m| m.focused()), None);
        harness.key_press(egui::Key::ArrowDown);
        harness.run();
        harness.key_press(egui::Key::Space);
        harness.run();
        assert_eq!(harness.state().list.cursor, 1);
        assert_eq!(harness.ctx.memory(|m| m.focused()), None);
    }

    #[test]
    fn apply_pending_hides_and_overrides_rows() {
        let thread = |id: &str, uid| ThreadSummary {
            thread_id: id.into(),
            latest: summary(uid, ""),
            count: 1,
            unread: true,
            flagged: false,
        };
        let threads = vec![thread("a", 1), thread("b", 2), thread("c", 3)];
        let mut rows = build_rows("INBOX", &threads, &HashMap::new());
        let pending = HashMap::from([
            (("INBOX".to_string(), 1), Optimistic::Hidden),
            (("INBOX".to_string(), 2), Optimistic::Seen(true)),
            (("INBOX".to_string(), 3), Optimistic::Flagged(true)),
        ]);
        apply_pending(&mut rows, &pending);
        let shape: Vec<(u32, bool, bool)> =
            rows.iter().map(|r| (r.uid, r.unread, r.flagged)).collect();
        assert_eq!(shape, [(2, false, false), (3, true, true)]);
    }
}
