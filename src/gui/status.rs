//! The bottom bar: a line per account from its latest activity, the history window, the theme switch, the version and a
//! newer release.
use eframe::egui;

use crate::sync::Activity;
use crate::time::clock;

use super::app::{Account, App, UiAction};
use super::{icons, theme, toolbar};

// Fixed in tests so a release bump does not change every GUI snapshot.
pub(crate) const VERSION: &str = if cfg!(test) {
    "0.0.0"
} else {
    env!("CARGO_PKG_VERSION")
};

/// The account's status line: its error or its latest activity; plus queued commands.
pub(crate) fn account_line(account: &Account) -> String {
    let state = match (&account.error, &account.activity) {
        (Some(error), _) => error.clone(),
        (None, Some(activity)) => activity.to_string(),
        (None, None) => "starting…".into(),
    };
    let mut line = format!("{}: {state}", account.name);
    if account.queued > 0 {
        line.push_str(&format!(" · {} queued", account.queued));
    }
    line
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

fn failed(account: &Account) -> bool {
    account.error.is_some() || matches!(account.activity, Some(Activity::NotRunning { .. }))
}

/// The error colour for an error, otherwise the default text colour.
fn line_color(account: &Account, error_color: egui::Color32) -> Option<egui::Color32> {
    failed(account).then_some(error_color)
}

/// The mark before an account's line.
#[derive(Debug, PartialEq)]
struct Dot {
    color: egui::Color32,
    filled: bool,
}

/// A ring in the error colour for an error, a brass ring while offline, a brass dot once up to date, none while it
/// syncs.
fn dot(account: &Account, error_color: egui::Color32, palette: &theme::Palette) -> Option<Dot> {
    if failed(account) {
        Some(Dot {
            color: error_color,
            filled: false,
        })
    } else {
        match account.activity {
            Some(Activity::Idle { .. }) => Some(Dot {
                color: palette.highlight,
                filled: true,
            }),
            Some(Activity::Offline { .. }) => Some(Dot {
                color: palette.highlight,
                filled: false,
            }),
            _ => None,
        }
    }
}

/// `dot` in a cell as wide as it is tall, so the lines' text lines up whether or not they have one.
fn paint_dot(ui: &mut egui::Ui, dot: Option<Dot>) {
    let size = theme::PAD;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let radius = size / 2.0 - 1.0;
    let Some(Dot { color, filled }) = dot else {
        return;
    };
    if filled {
        ui.painter().circle_filled(rect.center(), radius, color);
    } else {
        let ring = egui::Stroke::new(1.5, color);
        ui.painter().circle_stroke(rect.center(), radius, ring);
    }
}

/// The sync button's hover text, with the platform's shortcut.
pub(crate) fn sync_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "Sync now · Cmd+R"
    } else {
        "Sync now · Ctrl+R"
    }
}

pub(crate) fn show(app: &App, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let error_color = ui.visuals().error_fg_color;
    ui.horizontal(|ui| {
        if toolbar::icon_button(ui, true, icons::SYNC, "Sync now", sync_key()).clicked() {
            actions.push(UiAction::SyncNow);
        }
        ui.vertical(|ui| {
            // Text lines, not rows of controls, though each line can be clicked.
            ui.spacing_mut().interact_size.y = 0.0;
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
                    paint_dot(ui, dot(account, error_color, theme::palette(ui)));
                    let mut text = egui::RichText::new(account_line(account));
                    if let Some(color) = line_color(account, error_color) {
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
            if let Some(version) = app.newer_release() {
                update_link(ui, version);
            }
            ui.label(
                egui::RichText::new(format!("v{VERSION}"))
                    .monospace()
                    .weak(),
            );
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

/// Monospace because the default proportional font lacks the arrow.
fn update_link(ui: &mut egui::Ui, version: &str) {
    let highlight = theme::palette(ui).highlight;
    ui.scope(|ui| {
        ui.visuals_mut().hyperlink_color = highlight;
        ui.hyperlink_to(
            egui::RichText::new(format!("↑ v{version} available")).monospace(),
            crate::update::release_url(version),
        );
    });
}

/// The `?` window. Keys are spelled out, since the default fonts lack some arrow and modifier glyphs.
pub(crate) const KEYS: [(&str, &str); 14] = [
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
    ("v", "show HTML mail as text, or as HTML again"),
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
            theme::grid("keys", ui).num_columns(2).show(ui, |ui| {
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
    use crate::gui::app::UpdateCheck;
    use crate::gui::test_support::{Fixture, message};
    use crate::sync::Event;

    #[test]
    fn an_up_to_date_account_gets_a_brass_dot_and_an_error_wins() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        let account = &mut harness.state_mut().accounts[0];
        let (error_color, palette) = (egui::Color32::RED, &theme::FRAME_NIGHT);
        assert_eq!(line_color(account, error_color), None);
        assert_eq!(dot(account, error_color, palette), None);
        account.activity = Some(Activity::Idle {
            since: 1_790_000_000,
        });
        assert_eq!(line_color(account, error_color), None);
        assert_eq!(
            dot(account, error_color, palette),
            Some(Dot {
                color: palette.highlight,
                filled: true
            })
        );
        account.error = Some("boom".into());
        assert_eq!(line_color(account, error_color), Some(error_color));
        assert_eq!(
            dot(account, error_color, palette),
            Some(Dot {
                color: error_color,
                filled: false
            })
        );
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
    fn an_account_that_is_not_running_says_why_in_red_without_a_spinner() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, wires) = fx.harness();
        let activity = Activity::NotRunning {
            reason: "could not start its sync thread".into(),
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
                .query_by_label("work: not running (could not start its sync thread)")
                .is_some()
        );
        let account = &harness.state().accounts[0];
        assert!(!account.busy());
        let (error_color, palette) = (egui::Color32::RED, &theme::FRAME_NIGHT);
        assert_eq!(line_color(account, error_color), Some(error_color));
        assert_eq!(
            dot(account, error_color, palette),
            Some(Dot {
                color: error_color,
                filled: false
            })
        );
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
                errors: vec![],
            })
            .unwrap();
        harness.run();
        assert_eq!(harness.state().list.rows.len(), 1);
        assert!(harness.query_by_label("Inbox (1)").is_some());
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

    #[test]
    fn a_newer_release_links_to_its_notes() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        harness.state_mut().update = UpdateCheck::Done(Some("9.9.9".into()));
        harness.run();
        harness.get_by_label("↑ v9.9.9 available").click();
        harness.step();
        let opened: Vec<_> = harness
            .output()
            .platform_output
            .commands
            .iter()
            .filter_map(|command| match command {
                egui::OutputCommand::OpenUrl(open) => Some(open.url.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            opened,
            ["https://github.com/postvak-app/postvak/releases/tag/v9.9.9"]
        );
    }

    #[test]
    fn no_newer_release_shows_only_the_version() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        harness.state_mut().update = UpdateCheck::Done(None);
        harness.run();
        assert!(harness.query_by_label_contains("available").is_none());
        assert!(harness.query_by_label(&format!("v{VERSION}")).is_some());
    }
}
