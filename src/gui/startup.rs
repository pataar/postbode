//! The small window shown in place of the mail window when that cannot start: a launch from a desktop entry or
//! Finder has no terminal to show stderr in.
use std::fmt;
use std::path::Path;

use eframe::egui;

use crate::message::clean;

use super::app::central_panel;
use super::theme;

/// Where the docs explain `postbode account add` and config.toml.
pub(crate) const ACCOUNTS_DOCS: &str = "https://postbode.pataar.nl/accounts.html";

/// config.toml names no account, so the window has nothing to show.
#[derive(Debug)]
pub(crate) struct NoAccounts;

impl fmt::Display for NoAccounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("no accounts configured; run `postbode account add`")
    }
}

impl std::error::Error for NoAccounts {}

/// What went wrong and what to do about it, as the window states it.
#[derive(Debug, PartialEq)]
pub(crate) struct Problem {
    pub summary: String,
    pub fix: String,
    /// The error itself, when the summary does not say it all.
    pub detail: Option<String>,
}

impl Problem {
    /// `log` is the daemon log, when the paths were found.
    pub fn of(error: &anyhow::Error, log: Option<&Path>) -> Problem {
        if error.is::<NoAccounts>() {
            return Problem {
                summary: "Postbode has no mail account set up yet.".into(),
                fix: format!(
                    "Run `postbode account add` in a terminal, then open Postbode again. See {ACCOUNTS_DOCS}"
                ),
                detail: None,
            };
        }
        let fix = match log {
            Some(log) => format!("The background sync logs to {}", log.display()),
            None => "Run `postbode gui` in a terminal to see more.".into(),
        };
        Problem {
            summary: "Postbode could not start.".into(),
            fix,
            detail: Some(clean(&format!("{error:#}"), false)),
        }
    }
}

/// Draws the problem; true when the user asked to close the window.
pub(crate) fn show(problem: &Problem, ui: &mut egui::Ui) -> bool {
    let mut close = false;
    central_panel(ui, false).show(ui, |ui| {
        ui.heading(&problem.summary);
        ui.add_space(8.0);
        ui.label(&problem.fix);
        if let Some(detail) = &problem.detail {
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .max_height(ui.available_height() - 40.0)
                .show(ui, |ui| {
                    ui.colored_label(theme::palette(ui).error, detail);
                });
        }
        ui.add_space(12.0);
        close = ui.button("Close").clicked();
    });
    close
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use eframe::egui;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::gui::Explain;

    fn harness(problem: Problem) -> Harness<'static, Explain> {
        Harness::builder()
            .with_size(egui::vec2(520.0, 260.0))
            .build_ui_state(
                |ui, explain: &mut Explain| explain.show(ui),
                Explain(problem),
            )
    }

    #[test]
    fn no_accounts_points_to_account_add_and_the_docs() {
        let problem = Problem::of(&anyhow::Error::new(NoAccounts), Some(Path::new("/l")));
        assert_eq!(problem.summary, "Postbode has no mail account set up yet.");
        assert!(problem.fix.contains("`postbode account add`"));
        assert!(problem.fix.contains(ACCOUNTS_DOCS));
        assert_eq!(problem.detail, None);
    }

    #[test]
    fn no_accounts_is_found_under_context() {
        let error = anyhow::Error::new(NoAccounts).context("opening the window");
        let problem = Problem::of(&error, None);
        assert_eq!(problem.summary, "Postbode has no mail account set up yet.");
    }

    #[test]
    fn other_errors_show_their_text_and_the_daemon_log() {
        let error = anyhow::anyhow!("socket refused").context("could not start the daemon");
        let problem = Problem::of(&error, Some(Path::new("/state/daemon.log")));
        assert_eq!(problem.summary, "Postbode could not start.");
        assert_eq!(problem.fix, "The background sync logs to /state/daemon.log");
        assert_eq!(
            problem.detail.as_deref(),
            Some("could not start the daemon: socket refused")
        );
    }

    #[test]
    fn without_paths_the_fix_is_to_run_it_in_a_terminal() {
        let problem = Problem::of(&anyhow::anyhow!("no home directory found"), None);
        assert_eq!(problem.fix, "Run `postbode gui` in a terminal to see more.");
        assert_eq!(problem.detail.as_deref(), Some("no home directory found"));
    }

    #[test]
    fn the_window_states_the_missing_account() {
        let mut harness = harness(Problem::of(&anyhow::Error::new(NoAccounts), None));
        harness.run();
        harness.get_by_label("Postbode has no mail account set up yet.");
        harness.get_by_label_contains("Run `postbode account add` in a terminal");
        harness.get_by_label("Close");
    }

    #[test]
    fn the_window_shows_the_error_and_the_daemon_log() {
        let error = anyhow::anyhow!("could not start the daemon");
        let mut harness = harness(Problem::of(&error, Some(Path::new("/state/daemon.log"))));
        harness.run();
        harness.get_by_label("Postbode could not start.");
        harness.get_by_label("could not start the daemon");
        harness.get_by_label("The background sync logs to /state/daemon.log");
    }

    #[test]
    fn close_asks_the_window_to_close() {
        let mut harness = harness(Problem::of(&anyhow::Error::new(NoAccounts), None));
        harness.run();
        let closes = |harness: &Harness<'_, Explain>| {
            harness.output().viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&egui::ViewportCommand::Close)
        };
        assert!(!closes(&harness));
        harness.get_by_label("Close").click();
        harness.step();
        assert!(closes(&harness));
    }
}
