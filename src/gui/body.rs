//! The right column: headers, the text body with its links, and attachments.
use eframe::egui;

use crate::engine::StartState;
use crate::message::clean;
use crate::sync::Activity;

use super::app::{Account, App, UiAction};
use super::status::local_time;

#[derive(Debug, PartialEq)]
pub(crate) enum Segment<'a> {
    Link(&'a str),
    Text(&'a str),
}

/// The only schemes a body link may open; anything else stays text.
const SCHEMES: [&str; 3] = ["http://", "https://", "mailto:"];

/// Splits a line into text and links. A link starts at a word start and ends at whitespace, a quote or an angle
/// bracket, minus trailing punctuation.
pub(crate) fn segments(text: &str) -> Vec<Segment<'_>> {
    let mut parts = Vec::new();
    let (mut text_start, mut scan) = (0, 0);
    while let Some(offset) = next_link(&text[scan..]) {
        let start = scan + offset;
        let tail = &text[start..];
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\''))
            .unwrap_or(tail.len());
        let link = tail[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']']);
        scan = start + link.len();
        // Trimming can leave a bare scheme, which is not a link.
        if has_target(link) {
            if start > text_start {
                parts.push(Segment::Text(&text[text_start..start]));
            }
            parts.push(Segment::Link(link));
            text_start = scan;
        }
    }
    if text_start < text.len() {
        parts.push(Segment::Text(&text[text_start..]));
    }
    parts
}

fn has_target(link: &str) -> bool {
    SCHEMES.iter().any(|scheme| {
        link.strip_prefix(scheme)
            .is_some_and(|target| !target.is_empty())
    })
}

fn next_link(text: &str) -> Option<usize> {
    SCHEMES
        .iter()
        .filter_map(|scheme| {
            text.match_indices(scheme).map(|(at, _)| at).find(|&at| {
                let word_start = text[..at]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !c.is_alphanumeric());
                let more = text[at + scheme.len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| !c.is_whitespace());
                word_start && more
            })
        })
        .min()
}

pub(crate) fn size(bytes: usize) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{} KB", b / 1024),
        b => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
    }
}

pub(crate) fn show(app: &App, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let Some(body) = &app.body else {
        ui.weak("No message selected.");
        return actions;
    };
    let Some(message) = &body.message else {
        ui.weak("This message is no longer in the local store.");
        return actions;
    };
    let date = local_time(
        message.date.unwrap_or(message.internaldate),
        "%a %Y-%m-%d %H:%M",
    );
    egui::Grid::new("headers").num_columns(2).show(ui, |ui| {
        for (name, value) in [
            ("From", message.from_addr.as_deref()),
            ("To", message.to_addr.as_deref()),
            ("Cc", message.cc_addr.as_deref()),
            ("Date", Some(date.as_str())),
            ("Subject", message.subject.as_deref()),
        ] {
            if let Some(value) = value {
                ui.strong(name);
                ui.label(clean(value, false));
                ui.end_row();
            }
        }
    });
    for attachment in &body.attachments {
        ui.horizontal(|ui| {
            let name = attachment
                .name
                .as_deref()
                .map(|name| clean(name, false))
                .unwrap_or_else(|| format!("attachment {}", attachment.index));
            ui.label(format!("{name} ({})", size(attachment.size)));
            if ui.button("Save").clicked() {
                actions.push(UiAction::SaveAttachment(attachment.index));
            }
        });
    }
    if let Some(saved) = &body.saved {
        ui.label(clean(saved, false));
    }
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| match &body.text {
            Some(text) => show_text(ui, text),
            None => {
                ui.weak(missing_text(&app.accounts[body.account]));
            }
        });
    actions
}

fn missing_text(account: &Account) -> String {
    if account.state != StartState::Running {
        format!(
            "Not downloaded; another Postbode process syncs {}.",
            account.name
        )
    } else if matches!(account.activity, Some(Activity::Offline { .. })) {
        format!("Not downloaded; loads when {} reconnects.", account.name)
    } else {
        "Loading…".into()
    }
}

/// The body line by line with clickable links. ponytail: every line is laid out each frame; draw only the visible
/// lines with `show_rows` if long mail scrolls slowly.
fn show_text(ui: &mut egui::Ui, text: &str) {
    for line in text.lines() {
        if line.trim().is_empty() {
            ui.label(" ");
            continue;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for segment in segments(line) {
                match segment {
                    Segment::Link(url) => {
                        if ui.link(url).clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                        }
                    }
                    Segment::Text(text) => {
                        ui.label(text);
                    }
                }
            }
        });
    }
}
#[cfg(test)]
mod tests {
    use eframe::egui;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::gui::app::View;
    use crate::gui::test_support::{Fixture, message};
    use crate::rules::Action;
    use crate::sync::{Command, Event};

    const WITH_ATTACHMENT: &str = "From: a@example.com\r\nSubject: report\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nSee attached\r\n--b\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=\"report.txt\"\r\n\r\nquarterly numbers\r\n--b--\r\n";

    #[test]
    fn segments_link_only_http_https_and_mailto_at_word_starts() {
        assert_eq!(
            segments("see https://example.com/a?b=1. or mailto:me@example.com, thanks"),
            [
                Segment::Text("see "),
                Segment::Link("https://example.com/a?b=1"),
                Segment::Text(". or "),
                Segment::Link("mailto:me@example.com"),
                Segment::Text(", thanks"),
            ]
        );
        assert_eq!(
            segments("(http://x.test)"),
            [
                Segment::Text("("),
                Segment::Link("http://x.test"),
                Segment::Text(")")
            ]
        );
        for plain in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "xhttps://evil.test",
            "https:// spaced",
            "ftp://x.test",
            "http://!",
            "mailto:)",
            "mailto:.",
        ] {
            assert_eq!(segments(plain), [Segment::Text(plain)], "{plain}");
        }
    }

    #[test]
    fn a_bare_scheme_after_trimming_is_not_a_link() {
        assert!(
            segments("see https://)")
                .iter()
                .all(|segment| matches!(segment, Segment::Text(_)))
        );
    }

    #[test]
    fn size_is_human_readable() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(2_048), "2 KB");
        assert_eq!(size(3_500_000), "3.3 MB");
    }

    #[test]
    fn the_cursor_message_shows_headers_and_text() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 1, "hello there"));
        let (harness, _wires) = fx.harness();
        assert!(harness.query_by_label("Body of 1").is_some());
        assert!(
            harness
                .query_by_label("Sender 1 <sender1@example.com>")
                .is_some()
        );
        assert!(harness.query_by_label("hello there").is_some());
    }

    #[test]
    fn an_https_link_opens_in_the_browser() {
        let fx = Fixture::new(&["work"]);
        let mut m = message("INBOX", 1, "links");
        m.body_text = Some("Visit https://example.com/a. Not javascript:alert(1)".into());
        fx.add("work", m);
        let (mut harness, _wires) = fx.harness();
        harness.get_by_label("https://example.com/a").click();
        harness.step();
        let opened: Vec<&str> = harness
            .output()
            .platform_output
            .commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::OpenUrl(open) => Some(open.url.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(opened, ["https://example.com/a"]);
        assert!(
            harness
                .query_by_label(". Not javascript:alert(1)")
                .is_some()
        );
    }

    #[test]
    fn a_missing_body_is_fetched_once_and_shown_when_ready() {
        let fx = Fixture::new(&["work"]);
        let mut m = message("INBOX", 1, "later");
        m.body_text = None;
        fx.add("work", m);
        let (mut harness, wires) = fx.harness();
        assert_eq!(
            wires.sent(),
            [(
                "work".to_string(),
                Command::FetchBody {
                    folder: "INBOX".into(),
                    uid: 1
                }
            )]
        );
        assert!(harness.query_by_label("Loading…").is_some());
        fx.store("work")
            .set_raw(
                "INBOX",
                1,
                b"Subject: later\r\n\r\nFetched text\r\n",
                "Fetched text",
            )
            .unwrap();
        wires
            .events
            .send(Event::BodyReady {
                account: "work".into(),
                folder: "INBOX".into(),
                uid: 1,
                request: 0,
            })
            .unwrap();
        harness.run();
        assert!(harness.query_by_label("Fetched text").is_some());
        assert!(wires.sent().is_empty());
    }

    #[test]
    fn a_failed_body_fetch_is_retried_when_the_message_is_shown_again() {
        let fx = Fixture::new(&["work"]);
        let mut m = message("INBOX", 2, "later");
        m.body_text = None;
        fx.add("work", m);
        fx.add("work", message("INBOX", 1, "other"));
        let (mut harness, wires) = fx.harness();
        assert_eq!(wires.sent().len(), 1);
        wires
            .events
            .send(Event::Error {
                account: "work".into(),
                message: "boom".into(),
            })
            .unwrap();
        harness.event(egui::Event::Text("j".into()));
        harness.run();
        harness.event(egui::Event::Text("k".into()));
        harness.run();
        assert_eq!(
            wires.sent(),
            [(
                "work".to_string(),
                Command::FetchBody {
                    folder: "INBOX".into(),
                    uid: 2
                }
            )]
        );
    }

    #[test]
    fn a_fetched_body_keeps_the_read_timer_and_the_hand_marks() {
        let fx = Fixture::new(&["work"]);
        let mut m = message("INBOX", 1, "later");
        m.body_text = None;
        m.flags = String::new();
        fx.add("work", m);
        let (mut harness, wires) = fx.harness();
        harness.event(egui::Event::Text("u".into()));
        harness.run();
        harness.event(egui::Event::Text("u".into()));
        harness.run();
        fx.store("work")
            .set_raw("INBOX", 1, b"Subject: later\r\n\r\nFetched\r\n", "Fetched")
            .unwrap();
        wires
            .events
            .send(Event::BodyReady {
                account: "work".into(),
                folder: "INBOX".into(),
                uid: 1,
                request: 0,
            })
            .unwrap();
        harness.run();
        assert!(harness.query_by_label("Fetched").is_some());
        harness.input_mut().time = Some(100.0);
        harness.step();
        let apply = |action| {
            (
                "work".to_string(),
                Command::Apply {
                    folder: "INBOX".into(),
                    uids: vec![1],
                    action,
                    by: "gui".into(),
                },
            )
        };
        assert_eq!(
            wires.sent(),
            [
                (
                    "work".to_string(),
                    Command::FetchBody {
                        folder: "INBOX".into(),
                        uid: 1
                    }
                ),
                apply(Action::MarkRead),
                apply(Action::MarkUnread),
            ]
        );
    }

    #[test]
    fn marking_another_row_does_not_stop_the_shown_message_being_read() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 1, "old"));
        let mut unread = message("INBOX", 2, "new");
        unread.flags = String::new();
        fx.add("work", unread);
        let (mut harness, wires) = fx.harness();
        harness.input_mut().time = Some(10.0);
        for key in ["j", "x", "k", "u"] {
            harness.event(egui::Event::Text(key.into()));
            harness.step();
        }
        harness.input_mut().time = Some(12.0);
        harness.step();
        let apply = |uid, action| {
            (
                "work".to_string(),
                Command::Apply {
                    folder: "INBOX".into(),
                    uids: vec![uid],
                    action,
                    by: "gui".into(),
                },
            )
        };
        assert_eq!(
            wires.sent(),
            [apply(1, Action::MarkUnread), apply(2, Action::MarkRead)]
        );
    }

    #[test]
    fn offline_says_when_the_body_will_load() {
        let fx = Fixture::new(&["work"]);
        let mut m = message("INBOX", 1, "later");
        m.body_text = None;
        fx.add("work", m);
        let (mut harness, wires) = fx.harness();
        let activity = Activity::Offline {
            reason: "timeout".into(),
            retry_at: 1_790_000_300,
        };
        wires
            .events
            .send(Event::Activity {
                account: "work".into(),
                activity,
            })
            .unwrap();
        harness.run();
        assert!(
            harness
                .query_by_label("Not downloaded; loads when work reconnects.")
                .is_some()
        );
    }

    #[test]
    fn a_message_is_marked_read_after_one_second_on_screen() {
        let fx = Fixture::new(&["work"]);
        let mut unread = message("INBOX", 1, "new");
        unread.flags = String::new();
        fx.add("work", unread);
        fx.add("work", message("INBOX", 2, "old"));
        let (mut harness, wires) = fx.harness();
        harness.input_mut().time = Some(10.0);
        harness.event(egui::Event::Text("j".into()));
        harness.step();
        harness.input_mut().time = Some(10.9);
        harness.step();
        assert!(wires.sent().is_empty());
        harness.input_mut().time = Some(11.1);
        harness.step();
        let read = Command::Apply {
            folder: "INBOX".into(),
            uids: vec![1],
            action: Action::MarkRead,
            by: "gui".into(),
        };
        assert_eq!(wires.sent(), [("work".to_string(), read)]);
        harness.input_mut().time = Some(13.0);
        harness.step();
        assert!(wires.sent().is_empty());
    }

    #[test]
    fn marking_unread_by_hand_is_not_undone_by_the_delay() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 1, "read"));
        let (mut harness, wires) = fx.harness();
        harness.event(egui::Event::Text("u".into()));
        harness.run();
        harness.input_mut().time = Some(30.0);
        harness.step();
        let unread = Command::Apply {
            folder: "INBOX".into(),
            uids: vec![1],
            action: Action::MarkUnread,
            by: "gui".into(),
        };
        assert_eq!(wires.sent(), [("work".to_string(), unread)]);
    }

    #[test]
    fn save_writes_the_attachment_to_downloads() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 1, "report"));
        fx.store("work")
            .set_raw("INBOX", 1, WITH_ATTACHMENT.as_bytes(), "See attached")
            .unwrap();
        let (mut harness, _wires) = fx.harness();
        assert!(harness.query_by_label_contains("report.txt (").is_some());
        harness.get_by_label("Save").click();
        harness.run();
        let saved = fx.paths.cache_dir.join("downloads").join("report.txt");
        assert_eq!(
            std::fs::read_to_string(&saved).unwrap().trim_end(),
            "quarterly numbers"
        );
        assert!(harness.query_by_label_contains("Saved to").is_some());
    }

    fn unread(uid: u32, subject: &str) -> crate::store::Message {
        let mut m = message("INBOX", uid, subject);
        m.flags = String::new();
        m
    }

    fn at(harness: &mut egui_kittest::Harness<'_, crate::gui::App>, time: f64) {
        harness.input_mut().time = Some(time);
        harness.step();
    }

    fn read(uid: u32) -> (String, Command) {
        let command = Command::Apply {
            folder: "INBOX".into(),
            uids: vec![uid],
            action: Action::MarkRead,
            by: "gui".into(),
        };
        ("work".to_string(), command)
    }

    fn fetch(uid: u32) -> (String, Command) {
        let command = Command::FetchBody {
            folder: "INBOX".into(),
            uid,
        };
        ("work".to_string(), command)
    }

    #[test]
    fn a_sync_that_adds_newer_mail_keeps_the_cursor_on_its_message() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 1, "old"));
        let (mut harness, wires) = fx.harness();
        fx.add("work", unread(2, "new"));
        wires
            .events
            .send(Event::Synced {
                account: "work".into(),
                new_messages: 1,
                actions: 0,
                requests: vec![],
            })
            .unwrap();
        at(&mut harness, 10.0);
        at(&mut harness, 12.0);
        assert!(wires.sent().is_empty());
        let list = &harness.state().list;
        assert_eq!(list.rows.len(), 2);
        assert_eq!(list.rows[list.cursor].uid, 1);
    }

    #[test]
    fn the_message_shown_at_startup_is_not_marked_read() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", unread(1, "new"));
        let (mut harness, wires) = fx.harness();
        at(&mut harness, 10.0);
        at(&mut harness, 12.0);
        assert!(wires.sent().is_empty());
    }

    #[test]
    fn the_message_shown_after_switching_folders_is_not_marked_read() {
        let fx = Fixture::new(&["work"]);
        fx.folder("work", "Archive", Some("Archive"));
        let mut archived = message("Archive", 7, "archived");
        archived.flags = String::new();
        fx.add("work", archived);
        let (mut harness, wires) = fx.harness();
        harness.state_mut().select_view(View::Folder {
            account: 0,
            folder: "Archive".into(),
        });
        at(&mut harness, 10.0);
        at(&mut harness, 12.0);
        assert!(wires.sent().is_empty());
    }

    #[test]
    fn an_opened_message_is_marked_read_a_second_after_its_body_arrives() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 2, "old"));
        let mut later = unread(1, "later");
        later.body_text = None;
        fx.add("work", later);
        let (mut harness, wires) = fx.harness();
        harness.input_mut().time = Some(10.0);
        harness.event(egui::Event::Text("j".into()));
        harness.step();
        at(&mut harness, 12.0);
        assert_eq!(wires.sent(), [fetch(1)]);
        fx.store("work")
            .set_raw("INBOX", 1, b"Subject: later\r\n\r\nFetched\r\n", "Fetched")
            .unwrap();
        wires
            .events
            .send(Event::BodyReady {
                account: "work".into(),
                folder: "INBOX".into(),
                uid: 1,
                request: 0,
            })
            .unwrap();
        at(&mut harness, 12.5);
        assert!(wires.sent().is_empty());
        at(&mut harness, 13.6);
        assert_eq!(wires.sent(), [read(1)]);
    }

    #[test]
    fn the_row_after_an_archive_is_marked_read_after_a_second() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", unread(1, "next"));
        fx.add("work", message("INBOX", 2, "done"));
        let (mut harness, wires) = fx.harness();
        harness.input_mut().time = Some(10.0);
        harness.event(egui::Event::Text("e".into()));
        harness.step();
        at(&mut harness, 12.0);
        let archive = Command::Apply {
            folder: "INBOX".into(),
            uids: vec![2],
            action: Action::Archive,
            by: "gui".into(),
        };
        assert_eq!(wires.sent(), [("work".to_string(), archive), read(1)]);
    }

    #[test]
    fn a_search_hit_is_not_opened_while_the_query_is_typed() {
        let fx = Fixture::new(&["work"]);
        let mut hit = unread(1, "invoice");
        hit.body_text = None;
        fx.add("work", hit);
        fx.add("work", message("INBOX", 2, "lunch"));
        let (mut harness, wires) = fx.harness();
        harness.input_mut().time = Some(10.0);
        harness.event(egui::Event::Text("/".into()));
        harness.step();
        harness.event(egui::Event::Text("invoice".into()));
        harness.step();
        at(&mut harness, 12.0);
        assert_eq!(harness.state().list.rows.len(), 1);
        assert!(wires.sent().is_empty());
        assert!(harness.state().body.is_none());
    }

    #[test]
    fn clicking_the_message_already_shown_opens_it() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", unread(1, "new"));
        let (mut harness, wires) = fx.harness();
        at(&mut harness, 2.0);
        assert!(wires.sent().is_empty());
        harness.input_mut().time = Some(10.0);
        harness.get_by_label_contains("Sender 1 — new").click();
        harness.step();
        at(&mut harness, 10.5);
        at(&mut harness, 11.1);
        assert_eq!(wires.sent(), [read(1)]);
    }

    #[test]
    fn a_reply_in_the_cursor_thread_keeps_the_cursor_on_that_thread() {
        let fx = Fixture::new(&["work"]);
        for (uid, subject) in [(1, "c"), (2, "b"), (3, "a")] {
            fx.add("work", message("INBOX", uid, subject));
        }
        let (mut harness, wires) = fx.harness();
        harness.event(egui::Event::Text("j".into()));
        harness.step();
        let thread_b = "<2@example.com>";
        let mut reply = message("INBOX", 4, "Re: b");
        reply.thread_id = thread_b.into();
        fx.add("work", reply);
        wires
            .events
            .send(Event::Synced {
                account: "work".into(),
                new_messages: 1,
                actions: 0,
                requests: vec![],
            })
            .unwrap();
        harness.step();
        let list = &harness.state().list;
        assert_eq!(list.rows[list.cursor].thread_id.as_deref(), Some(thread_b));
        harness.event(egui::Event::Text("e".into()));
        harness.step();
        let archive = Command::Apply {
            folder: "INBOX".into(),
            uids: vec![2, 4],
            action: Action::Archive,
            by: "gui".into(),
        };
        assert_eq!(wires.sent(), [("work".to_string(), archive)]);
    }

    #[test]
    fn the_read_delay_starts_when_the_body_text_appears() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 2, "old"));
        let mut later = unread(1, "later");
        later.body_text = None;
        fx.add("work", later);
        let (mut harness, wires) = fx.harness();
        harness.input_mut().time = Some(10.0);
        harness.event(egui::Event::Text("j".into()));
        harness.step();
        assert_eq!(wires.sent(), [fetch(1)]);
        fx.store("work")
            .set_raw("INBOX", 1, b"Subject: later\r\n\r\nFetched\r\n", "Fetched")
            .unwrap();
        wires
            .events
            .send(Event::BodyReady {
                account: "work".into(),
                folder: "INBOX".into(),
                uid: 1,
                request: 0,
            })
            .unwrap();
        at(&mut harness, 20.0);
        at(&mut harness, 20.5);
        assert!(wires.sent().is_empty());
        at(&mut harness, 21.1);
        assert_eq!(wires.sent(), [read(1)]);
    }
}
