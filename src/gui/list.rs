//! The middle column: one row per thread, followed by its members when expanded.
use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Datelike, Local, TimeZone};
use eframe::egui;

use crate::message::clean;
use crate::rules::Action;
use crate::store::{Message, MessageSummary, ThreadSummary};

use super::app::{App, UiAction, View};
use super::theme::{self, Palette};
use super::{icons, numbers};

/// Threads loaded per folder. ponytail: older mail is reachable through search; page by date if that is not enough.
pub(crate) const THREAD_LIMIT: u32 = 10_000;
/// Results shown for one search.
pub(crate) const SEARCH_LIMIT: u32 = 500;
const ROW_HEIGHT: f32 = 22.0;

pub(crate) type RowKey = (String, u32);

pub(crate) fn search_id() -> egui::Id {
    egui::Id::new("search")
}

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

    pub fn from_message(message: &Message) -> Row {
        Row {
            count: 1,
            date: message.internaldate,
            flagged: message.is_flagged(),
            folder: message.folder.clone(),
            from: message.from_addr.clone().unwrap_or_default(),
            member: false,
            subject: message.subject.clone().unwrap_or_default(),
            thread_id: None,
            to: message.to_addr.clone().unwrap_or_default(),
            uid: message.uid,
            unread: !message.is_seen(),
        }
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
    pub hits: Vec<Row>,
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
            Action::Notify | Action::Silent | Action::Tag(_) => None,
        }
    }
}

/// Rows of sent actions as they will be: moved rows hidden, read and flag changes shown, a later edit over an earlier.
pub(crate) fn apply_pending(rows: &mut Vec<Row>, pending: &HashMap<RowKey, Vec<Optimistic>>) {
    if pending.is_empty() {
        return;
    }
    rows.retain(|row| {
        !pending
            .get(&row.key())
            .is_some_and(|edits| edits.contains(&Optimistic::Hidden))
    });
    for row in rows {
        for edit in pending.get(&row.key()).into_iter().flatten() {
            match edit {
                Optimistic::Flagged(flagged) => row.flagged = *flagged,
                Optimistic::Seen(seen) => row.unread = !seen,
                Optimistic::Hidden => {}
            }
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

/// The time today, weekday and time within the past week, day and month this year, else day, month and year.
pub(crate) fn list_date<Tz: TimeZone>(ts: i64, now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let Some(at) = now.timezone().timestamp_opt(ts, 0).single() else {
        return String::new();
    };
    let days = now
        .date_naive()
        .signed_duration_since(at.date_naive())
        .num_days();
    let format = match days {
        0 => "%H:%M",
        1..=6 => "%a %H:%M",
        _ if at.year() == now.year() => "%-d %b",
        _ => "%-d %b %Y",
    };
    at.format(format).to_string()
}

/// One list row split into what each column draws, cleaned of control characters.
pub(crate) struct Columns {
    pub unread: bool,
    pub flagged: bool,
    pub marked: bool,
    pub who: String,
    pub subject: String,
    pub date: String,
    pub member: bool,
}

/// The unread dot and the flag or mark, before the sender.
const MARKERS: f32 = 24.0;
const SENDER: f32 = 130.0;
const GAP: f32 = 12.0;
/// Narrower than this, a column is left out: truncating would still draw its "…", over the date.
const MIN_TEXT: f32 = 16.0;
/// The list pane's narrowest width, so the unread dot and the flag never reach the date.
pub(crate) const MIN_WIDTH: f32 = 220.0;
/// How much further a thread member's sender is indented than its thread row's.
const MEMBER_INDENT: f32 = 12.0;

/// (sender width, subject width) for a row of `total` px whose date needs `date` px.
pub(crate) fn column_widths(total: f32, date: f32) -> (f32, f32) {
    let rest = (total - MARKERS - date - 2.0 * GAP).max(0.0);
    let sender = SENDER.min(rest / 2.0);
    (sender, (rest - sender).max(0.0))
}

pub(crate) fn columns<Tz: TimeZone>(
    row: &Row,
    marked: bool,
    recipient: bool,
    in_search: bool,
    now: &DateTime<Tz>,
) -> Columns
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

/// Unread dot, flag, sender (recipient in Sent and Drafts), subject, thread count and date, on one line.
pub(crate) fn row_text<Tz: TimeZone>(
    row: &Row,
    recipient: bool,
    in_search: bool,
    now: &DateTime<Tz>,
) -> String
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
    if in_search {
        text.push_str(&format!("[{}] ", row.folder));
    }
    text.push_str(&format!("{who} — {}", row.subject));
    if row.count > 1 {
        text.push_str(&format!(" ({})", row.count));
    }
    text.push_str(&format!("  ·  {}", list_date(row.date, now)));
    clean(&text, false)
}

/// Unread dot, flag or mark, sender, subject truncated with "…", and the date right-aligned; everything on-accent when
/// the row is selected.
fn draw_row(
    ui: &egui::Ui,
    rect: egui::Rect,
    c: &Columns,
    selected: bool,
    hovered: bool,
    palette: &Palette,
    font: &egui::FontId,
) {
    let painter = ui.painter_at(rect);
    if selected {
        painter.rect_filled(rect, 2.0, palette.accent);
    } else if hovered {
        painter.rect_filled(rect, 2.0, palette.hover);
    }
    let ink = |color| if selected { palette.on_accent } else { color };
    let middle = rect.center().y;
    let mut x = rect.left() + 6.0;
    if c.unread {
        painter.circle_filled(egui::pos2(x + 2.0, middle), 3.0, ink(palette.accent));
    }
    x += 10.0;
    let marker = if c.marked {
        Some(icons::CHECK)
    } else if c.flagged {
        Some(icons::FLAG)
    } else {
        None
    };
    if let Some(marker) = marker {
        let color = ink(palette.highlight);
        painter.text(
            egui::pos2(x + 6.0, middle),
            egui::Align2::CENTER_CENTER,
            marker,
            font.clone(),
            color,
        );
    }
    x += 14.0;
    let date_color = ink(palette.muted);
    let date = painter.layout_no_wrap(c.date.clone(), font.clone(), date_color);
    let date_at = egui::pos2(
        rect.right() - 6.0 - date.size().x,
        middle - date.size().y / 2.0,
    );
    let (sender_width, subject_width) = column_widths(rect.width() - 12.0, date.size().x);
    painter.galley(date_at, date, date_color);
    let text_color = ink(if c.unread {
        palette.text
    } else {
        palette.secondary
    });
    let truncated = |text: &str, width: f32| {
        let mut job =
            egui::text::LayoutJob::simple_singleline(text.to_string(), font.clone(), text_color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
        painter.layout_job(job)
    };
    let indent = if c.member { MEMBER_INDENT } else { 0.0 };
    if sender_width - indent >= MIN_TEXT {
        let sender = truncated(&c.who, sender_width - indent);
        let sender_at = egui::pos2(x + indent, middle - sender.size().y / 2.0);
        painter.galley(sender_at, sender, text_color);
    }
    if subject_width >= MIN_TEXT {
        let subject = truncated(&c.subject, subject_width);
        let subject_at = egui::pos2(x + sender_width + GAP, middle - subject.size().y / 2.0);
        painter.galley(subject_at, subject, text_color);
    }
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
    if let Some(query) = &app.search {
        let mut text = query.clone();
        let response = theme::text_field(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut text)
                    .id(search_id())
                    .hint_text("Search this account")
                    .desired_width(f32::INFINITY),
            )
        });
        if app.focus_search {
            response.request_focus();
            actions.push(UiAction::SearchFocused);
        }
        if response.changed() {
            actions.push(UiAction::SearchFor(text));
        }
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            actions.push(UiAction::SelectRow(0));
        }
        ui.weak(format!(
            "{} results · mail whose body is not downloaded matches on sender, recipients and subject only",
            numbers::count(list.rows.len() as u64)
        ));
    }
    let mut area = egui::ScrollArea::vertical().auto_shrink(false);
    if list.follow_cursor {
        let row_height = ROW_HEIGHT + ui.spacing().item_spacing.y;
        let (offset, height) = list.viewport;
        area = area.vertical_scroll_offset(offset_showing(list.cursor, row_height, offset, height));
    }
    let palette = theme::palette(ui);
    let font = egui::TextStyle::Button.resolve(ui.style());
    let output = area.show_rows(ui, ROW_HEIGHT, list.rows.len(), |ui, range| {
        for index in range {
            let row = &list.rows[index];
            let marked = list.marked.contains(&row.key());
            let selected = index == list.cursor || marked;
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), ROW_HEIGHT),
                egui::Sense::click(),
            );
            let c = columns(row, marked, recipient, app.search.is_some(), &now);
            draw_row(ui, rect, &c, selected, response.hovered(), palette, &font);
            let mut name = row_text(row, recipient, app.search.is_some(), &now);
            if marked {
                name = format!("✔ {name}");
            }
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &name)
            });
            if response.clicked() {
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

/// The `m` popup: type to filter the account's folders, Enter or a click moves. It leaves out the folder the cursor
/// message is in: the view's, or in search the hit's own.
pub(crate) fn show_move_picker(app: &App, ctx: &egui::Context) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let (Some(filter), View::Folder { account, folder }) = (&app.move_picker, &app.view) else {
        return actions;
    };
    let folder = app
        .list
        .rows
        .get(app.list.cursor)
        .map_or(folder, |row| &row.folder);
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
            let response = theme::text_field(ui, |ui| {
                ui.add(egui::TextEdit::singleline(&mut text).hint_text("Folder"))
            });
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
    fn list_date_is_time_today_weekday_and_time_this_week_day_month_this_year_else_with_year() {
        let now = Utc.with_ymd_and_hms(2026, 10, 6, 12, 0, 0).unwrap();
        let at = |y, mo, d, h, mi| {
            Utc.with_ymd_and_hms(y, mo, d, h, mi, 0)
                .unwrap()
                .timestamp()
        };
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
        row.date = Utc
            .with_ymd_and_hms(2026, 10, 6, 9, 13, 0)
            .unwrap()
            .timestamp();
        let c = columns(&row, false, false, true, &now);
        assert!(c.unread && c.flagged && !c.marked);
        assert_eq!(c.who, "Linus Example");
        assert_eq!(c.subject, "[INBOX] Lunch? (3)");
        assert_eq!(c.date, "09:13");
    }

    #[test]
    fn column_widths_keep_the_date_and_never_go_negative() {
        assert_eq!(
            column_widths(600.0, 80.0),
            (130.0, 600.0 - 24.0 - 130.0 - 12.0 - 80.0 - 12.0)
        );
        let (sender, subject) = column_widths(150.0, 80.0);
        assert!(sender >= 0.0 && subject >= 0.0);
        assert!(sender + subject <= 150.0 - 24.0 - 80.0 - 24.0);
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
        let text = row_text(&row, false, false, &now);
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
    fn a_marked_row_says_so_in_its_name() {
        let fx = Fixture::new(&["work"]);
        for uid in 1..=2 {
            fx.add("work", message("INBOX", uid, "hi"));
        }
        let (mut harness, _wires) = fx.harness();
        harness.event(egui::Event::Text("x".into()));
        harness.run();
        assert_eq!(
            harness
                .get_all_by(|node| node.label().is_some_and(|l| l.starts_with("✔ ")))
                .count(),
            1
        );
    }

    #[test]
    fn in_a_narrow_row_nothing_is_drawn_over_the_date() {
        for width in [150.0, 130.0] {
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(width + 16.0, 40.0))
                .build_ui(move |ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::hover());
                    let c = Columns {
                        unread: true,
                        flagged: true,
                        marked: false,
                        who: "A very long sender name indeed".into(),
                        subject: "A subject long enough to need truncating twice over".into(),
                        date: "10 Dec 2025".into(),
                        member: true,
                    };
                    let font = egui::TextStyle::Button.resolve(ui.style());
                    draw_row(ui, rect, &c, false, false, &crate::gui::theme::MOCHA, &font);
                });
            harness.run();
            let texts: Vec<(String, egui::Rect)> = harness
                .output()
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => Some((
                        text.galley.text().to_string(),
                        text.galley.rect.translate(text.pos.to_vec2()),
                    )),
                    _ => None,
                })
                .collect();
            let date = texts.iter().find(|(t, _)| t == "10 Dec 2025").unwrap().1;
            for (text, rect) in texts.iter().filter(|(t, _)| t != "10 Dec 2025") {
                assert!(
                    rect.right() <= date.left(),
                    "{width}: {text:?} {rect:?} vs date {date:?}"
                );
            }
        }
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
            (("INBOX".to_string(), 1), vec![Optimistic::Hidden]),
            (("INBOX".to_string(), 2), vec![Optimistic::Seen(true)]),
            (
                ("INBOX".to_string(), 3),
                vec![
                    Optimistic::Flagged(true),
                    Optimistic::Flagged(false),
                    Optimistic::Flagged(true),
                ],
            ),
        ]);
        apply_pending(&mut rows, &pending);
        let shape: Vec<(u32, bool, bool)> =
            rows.iter().map(|r| (r.uid, r.unread, r.flagged)).collect();
        assert_eq!(shape, [(2, false, false), (3, true, true)]);
    }

    #[test]
    fn slash_searches_the_account_and_escape_returns_to_the_folder() {
        let fx = Fixture::new(&["work"]);
        fx.folder("work", "Archive", Some("Archive"));
        fx.add("work", message("INBOX", 1, "invoice march"));
        fx.add("work", message("INBOX", 2, "lunch"));
        fx.add("work", message("Archive", 3, "invoice april"));
        let (mut harness, wires) = fx.harness();
        harness.event(egui::Event::Text("/".into()));
        harness.run();
        harness.event(egui::Event::Text("invoice".into()));
        harness.run();
        let found: Vec<(String, u32)> = harness.state().list.rows.iter().map(Row::key).collect();
        assert_eq!(
            found,
            [("Archive".to_string(), 3), ("INBOX".to_string(), 1)]
        );
        assert!(
            harness
                .query_by_label_contains("[Archive] Sender 3")
                .is_some()
        );
        assert!(harness.query_by_label_contains("2 results").is_some());
        harness.key_press(egui::Key::Enter);
        harness.run();
        harness.event(egui::Event::Text("e".into()));
        harness.run();
        let archive = crate::sync::Command::Apply {
            folder: "Archive".into(),
            uids: vec![3],
            action: crate::rules::Action::Archive,
            by: "gui".into(),
        };
        assert_eq!(wires.sent(), [("work".to_string(), archive)]);
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(harness.state().search.is_none());
        let uids: Vec<u32> = harness.state().list.rows.iter().map(|r| r.uid).collect();
        assert_eq!(uids, [2, 1]);
        assert_eq!(harness.ctx.memory(|m| m.focused()), None);
        harness.event(egui::Event::Text("j".into()));
        harness.run();
        assert_eq!(harness.state().list.cursor, 1);
    }

    #[test]
    fn shortcut_letters_typed_into_search_stay_text() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 1, "hello"));
        let (mut harness, wires) = fx.harness();
        harness.event(egui::Event::Text("/".into()));
        harness.run();
        harness.event(egui::Event::Text("e#us".into()));
        harness.run();
        assert!(wires.sent().is_empty());
        assert_eq!(harness.state().search.as_deref(), Some("e#us"));
    }

    #[test]
    fn the_move_picker_in_search_offers_the_inbox_for_an_archived_hit() {
        let fx = Fixture::new(&["work"]);
        fx.folder("work", "Archive", Some("Archive"));
        fx.add("work", message("Archive", 3, "invoice april"));
        let (mut harness, wires) = fx.harness();
        harness.event(egui::Event::Text("/".into()));
        harness.run();
        harness.event(egui::Event::Text("invoice".into()));
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();
        harness.event(egui::Event::Text("m".into()));
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();
        let back = crate::sync::Command::Apply {
            folder: "Archive".into(),
            uids: vec![3],
            action: crate::rules::Action::Move("INBOX".into()),
            by: "gui".into(),
        };
        assert_eq!(wires.sent(), [("work".to_string(), back)]);
    }
}
