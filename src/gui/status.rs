//! The bottom bar: a line per account from its latest activity, the history window, and the theme switch.
use chrono::{Local, TimeZone};
use eframe::egui;

use crate::engine::StartState;
use crate::message::clean;
use crate::sync::Activity;

use super::app::{Account, App, UiAction};
use super::theme;

pub(crate) fn activity_text(activity: &Activity) -> String {
    match activity {
        Activity::Connecting => "connecting…".into(),
        Activity::ListingFolders => "listing folders".into(),
        Activity::SyncingFolder { folder, index, of } => {
            format!("{} ({index}/{of})", clean(folder, false))
        }
        Activity::FetchingHeaders {
            folder,
            done,
            total,
        } => fetching(folder, "headers", *done, *total),
        Activity::FetchingBodies {
            folder,
            done,
            total,
        } => fetching(folder, "bodies", *done, *total),
        Activity::RunningRules { folder } => format!("running rules on {}", clean(folder, false)),
        Activity::RunningCommand { what } => clean(what, false),
        Activity::Idle { since } => format!("up to date · {}", clock(*since)),
        Activity::Offline { reason, retry_at } => {
            format!(
                "offline ({}) · retry {}",
                clean(reason, false),
                clock(*retry_at)
            )
        }
    }
}

/// The account's status line: its error, why it is not running, or its latest activity; plus queued commands.
pub(crate) fn account_line(account: &Account) -> String {
    let state = match (&account.error, &account.state, &account.activity) {
        (Some(error), _, _) => error.clone(),
        (None, StartState::Failed(e), _) => format!("could not start: {}", clean(e, false)),
        (None, StartState::Running, Some(activity)) => activity_text(activity),
        (None, StartState::Running, None) => "starting…".into(),
    };
    let mut line = format!("{}: {state}", account.name);
    if account.queued > 0 {
        line.push_str(&format!(" · {} queued", account.queued));
    }
    line
}

pub(crate) fn local_time(ts: i64, format: &str) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|at| at.format(format).to_string())
        .unwrap_or_default()
}

/// "INBOX headers 1,200 / 5,000".
fn fetching(folder: &str, what: &str, done: usize, total: usize) -> String {
    format!(
        "{} {what} {} / {}",
        clean(folder, false),
        thousands(done),
        thousands(total)
    )
}

pub(crate) fn clock(ts: i64) -> String {
    local_time(ts, "%H:%M")
}

pub(crate) fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

fn progress(activity: &Activity) -> Option<f32> {
    match activity {
        Activity::FetchingBodies { done, total, .. }
        | Activity::FetchingHeaders { done, total, .. }
            if *total > 0 =>
        {
            Some(*done as f32 / *total as f32)
        }
        _ => None,
    }
}

/// Red for an error, green once the account is up to date, otherwise the default text colour.
fn line_color(
    account: &Account,
    error_color: egui::Color32,
    palette: &theme::Palette,
) -> Option<egui::Color32> {
    match (&account.error, &account.state, &account.activity) {
        (Some(_), _, _) => Some(error_color),
        (None, StartState::Running, Some(Activity::Idle { .. })) => Some(palette.success),
        _ => None,
    }
}

pub(crate) fn show(app: &App, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let error_color = ui.visuals().error_fg_color;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            if let Some(error) = &app.error
                && ui
                    .add(
                        egui::Label::new(egui::RichText::new(error).color(error_color))
                            .sense(egui::Sense::click()),
                    )
                    .clicked()
            {
                actions.push(UiAction::ToggleHistory);
            }
            for account in &app.accounts {
                ui.horizontal(|ui| {
                    let mut text = egui::RichText::new(account_line(account));
                    if let Some(color) = line_color(account, error_color, theme::palette(ui)) {
                        text = text.color(color);
                    }
                    if ui
                        .add(egui::Label::new(text).sense(egui::Sense::click()))
                        .clicked()
                    {
                        actions.push(UiAction::ToggleHistory);
                    }
                    if let Some(fraction) = account.activity.as_ref().and_then(progress) {
                        ui.add(egui::ProgressBar::new(fraction).desired_width(160.0));
                    }
                });
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            for (preference, label) in [
                (egui::ThemePreference::Dark, "Dark"),
                (egui::ThemePreference::Light, "Light"),
                (egui::ThemePreference::System, "System"),
            ] {
                if ui
                    .selectable_label(app.theme == preference, label)
                    .clicked()
                {
                    actions.push(UiAction::SetTheme(preference));
                }
            }
        });
    });
    if app.history_open {
        let mut open = true;
        egui::Window::new("History")
            .open(&mut open)
            .default_width(520.0)
            .show(ui.ctx(), |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if app.history.is_empty() {
                        ui.weak("Nothing has happened yet.");
                    }
                    for line in app.history.iter().rev() {
                        ui.label(format!("{}  {}", clock(line.at), line.text));
                    }
                });
            });
        if !open {
            actions.push(UiAction::ToggleHistory);
        }
    }
    actions
}

/// The `?` window. Keys are spelled out, since the default fonts lack some arrow and modifier glyphs.
pub(crate) const KEYS: [(&str, &str); 13] = [
    ("j / k, Down / Up", "next or previous row"),
    ("Right / Left", "expand or collapse a thread"),
    ("x", "add the row to the selection, or take it out"),
    ("e", "archive"),
    (
        "#, Delete (Backspace on macOS)",
        "delete (to the Trash folder; from Trash or with no Trash folder, saved as .eml and deleted for good)",
    ),
    ("m", "move to a folder"),
    ("u", "mark read or unread"),
    ("s", "flag or unflag"),
    ("/", "search this account"),
    ("Esc", "close a popup, leave search, clear the selection"),
    ("Tab", "next pane: folders, list, body"),
    ("Ctrl+R (Cmd+R on macOS)", "sync every account now"),
    ("?", "show these keys"),
];

pub(crate) fn show_help(app: &App, ctx: &egui::Context) -> Vec<UiAction> {
    if !app.help_open {
        return Vec::new();
    }
    let mut open = true;
    egui::Window::new("Keys")
        .open(&mut open)
        .collapsible(false)
        .show(ctx, |ui| {
            egui::Grid::new("keys").num_columns(2).show(ui, |ui| {
                for (key, what) in KEYS {
                    ui.strong(key);
                    ui.label(what);
                    ui.end_row();
                }
            });
        });
    if open {
        Vec::new()
    } else {
        vec![UiAction::ToggleHelp]
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::gui::test_support::{Fixture, message};
    use crate::sync::Event;

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(48_213), "48,213");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn activity_text_matches_the_spec_table() {
        let folder = || "INBOX".to_string();
        let cases = [
            (Activity::Connecting, "connecting…".to_string()),
            (Activity::ListingFolders, "listing folders".into()),
            (
                Activity::SyncingFolder {
                    folder: "Archive".into(),
                    index: 4,
                    of: 12,
                },
                "Archive (4/12)".into(),
            ),
            (
                Activity::FetchingHeaders {
                    folder: folder(),
                    done: 12_500,
                    total: 48_213,
                },
                "INBOX headers 12,500 / 48,213".into(),
            ),
            (
                Activity::FetchingBodies {
                    folder: folder(),
                    done: 30,
                    total: 210,
                },
                "INBOX bodies 30 / 210".into(),
            ),
            (
                Activity::RunningRules { folder: folder() },
                "running rules on INBOX".into(),
            ),
            (
                Activity::RunningCommand {
                    what: "archiving 3 messages".into(),
                },
                "archiving 3 messages".into(),
            ),
            (
                Activity::Idle {
                    since: 1_790_000_000,
                },
                format!("up to date · {}", clock(1_790_000_000)),
            ),
            (
                Activity::Offline {
                    reason: "timeout".into(),
                    retry_at: 1_790_000_300,
                },
                format!("offline (timeout) · retry {}", clock(1_790_000_300)),
            ),
        ];
        for (activity, text) in cases {
            assert_eq!(activity_text(&activity), text);
        }
    }

    #[test]
    fn an_up_to_date_account_line_is_green_and_an_error_wins() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        let account = &mut harness.state_mut().accounts[0];
        let (error_color, palette) = (egui::Color32::RED, &theme::MOCHA);
        assert_eq!(line_color(account, error_color, palette), None);
        account.activity = Some(Activity::Idle {
            since: 1_790_000_000,
        });
        assert_eq!(
            line_color(account, error_color, palette),
            Some(palette.success)
        );
        account.error = Some("boom".into());
        assert_eq!(line_color(account, error_color, palette), Some(error_color));
    }

    #[test]
    fn fetching_headers_shows_the_counts() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, wires) = fx.harness();
        let activity = Activity::FetchingHeaders {
            folder: "INBOX".into(),
            done: 12_500,
            total: 48_213,
        };
        wires
            .events
            .send(Event::Activity {
                account: "work".into(),
                activity,
            })
            .unwrap();
        harness.run_ok();
        assert!(
            harness
                .query_by_label("work: INBOX headers 12,500 / 48,213")
                .is_some()
        );
    }

    #[test]
    fn offline_shows_the_retry_time() {
        let fx = Fixture::new(&["work"]);
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
        let line = format!("work: offline (timeout) · retry {}", clock(1_790_000_300));
        assert!(harness.query_by_label(&line).is_some());
    }

    #[test]
    fn an_error_shows_on_the_account_line_and_in_the_history() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, wires) = fx.harness();
        let message = "login failed\u{1b}[2J".to_string();
        wires
            .events
            .send(Event::Error {
                account: "work".into(),
                message,
            })
            .unwrap();
        harness.run();
        harness.get_by_label("work: login failed[2J").click();
        harness.run();
        assert!(harness.state().history_open);
        assert!(
            harness
                .query_by_label_contains("  work: login failed[2J")
                .is_some()
        );
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(!harness.state().history_open);
    }

    #[test]
    fn synced_reloads_the_shown_folder_and_its_unread_count() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, wires) = fx.harness();
        let mut unread = message("INBOX", 9, "late arrival");
        unread.flags = String::new();
        fx.add("work", unread);
        assert!(harness.state().list.rows.is_empty());
        wires
            .events
            .send(Event::Synced {
                account: "work".into(),
                new_messages: 1,
                actions: 0,
                requests: vec![],
            })
            .unwrap();
        harness.run();
        assert_eq!(harness.state().list.rows.len(), 1);
        assert!(harness.query_by_label("INBOX (1)").is_some());
    }

    #[test]
    fn a_rule_applied_or_bodies_fetched_reloads_the_shown_folder() {
        let completions = [
            Event::BodiesFetched {
                account: "work".into(),
                request: 1,
                fetched: 1,
            },
            Event::RuleApplied {
                account: "work".into(),
                request: 2,
                evaluated: 1,
                actions: 1,
                errors: vec![],
            },
        ];
        for (uid, completion) in (1..).zip(completions) {
            let fx = Fixture::new(&["work"]);
            let (mut harness, wires) = fx.harness();
            fx.add("work", message("INBOX", uid, "written by the daemon"));
            assert!(harness.state().list.rows.is_empty());
            wires.events.send(completion).unwrap();
            harness.run();
            assert_eq!(harness.state().list.rows.len(), 1);
        }
    }

    #[test]
    fn choosing_dark_applies_it_and_writes_it_keeping_comments() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        harness.get_by_label("Dark").click();
        harness.run();
        assert_eq!(
            harness.ctx.options(|o| o.theme_preference),
            egui::ThemePreference::Dark
        );
        let config = std::fs::read_to_string(fx.paths.config_file()).unwrap();
        assert!(
            config.starts_with("# test accounts\n") && config.contains("theme = \"dark\""),
            "{config}"
        );
    }

    #[test]
    fn progress_ticks_stay_out_of_the_history() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, wires) = fx.harness();
        let send = |activity| {
            wires
                .events
                .send(Event::Activity {
                    account: "work".into(),
                    activity,
                })
                .unwrap()
        };
        for done in [500, 1_000, 1_500] {
            send(Activity::FetchingHeaders {
                folder: "INBOX".into(),
                done,
                total: 2_000,
            });
        }
        send(Activity::Idle {
            since: 1_790_000_000,
        });
        send(Activity::Idle {
            since: 1_790_000_000,
        });
        wires
            .events
            .send(Event::Error {
                account: "work".into(),
                message: "boom".into(),
            })
            .unwrap();
        harness.run();
        let texts: Vec<&str> = harness
            .state()
            .history
            .iter()
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(
            texts,
            [
                format!("work: up to date · {}", clock(1_790_000_000)),
                "work: boom".to_string()
            ]
        );
    }

    #[test]
    fn offline_keeps_the_error_on_the_line() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, wires) = fx.harness();
        wires
            .events
            .send(Event::Error {
                account: "work".into(),
                message: "login failed".into(),
            })
            .unwrap();
        let activity = Activity::Offline {
            reason: "login failed".into(),
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
        assert!(harness.query_by_label("work: login failed").is_some());
    }

    #[test]
    fn theme_buttons_read_system_light_dark_from_left_to_right() {
        let fx = Fixture::new(&["work"]);
        let (harness, _wires) = fx.harness();
        let left = |label| harness.get_by_label(label).rect().left();
        assert!(left("System") < left("Light") && left("Light") < left("Dark"));
    }
}
