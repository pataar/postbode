//! The top bar: the logo, then actions on the cursor row or the selection, then search.
use eframe::egui;

use crate::rules::Action;

use super::app::{App, UiAction, View};
use super::icons;
use super::theme::CONTROL;

/// As wide as the folder pane's default, so the action buttons line up with the list.
const LOGO_CELL: f32 = 212.0;

pub(crate) fn show(app: &App, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        ui.allocate_ui(egui::vec2(LOGO_CELL, ui.available_height()), |ui| {
            ui.set_min_width(LOGO_CELL);
            ui.horizontal(|ui| {
                if let Some(logo) = &app.logo {
                    ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(20.0, 20.0)));
                }
                ui.label(egui::RichText::new("postbode").monospace().strong());
            });
        });
        let in_folder = matches!(app.view, View::Folder { .. });
        let has_row = in_folder && !app.list.rows.is_empty();
        for (icon, name, hint, action) in [
            (
                icons::ARCHIVE,
                "Archive message",
                "Archive · e",
                UiAction::Act(Action::Archive),
            ),
            (
                icons::MOVE,
                "Move message",
                "Move · m",
                UiAction::OpenMovePicker,
            ),
            (
                icons::TRASH,
                "Delete message",
                "Delete · #",
                UiAction::Act(Action::Trash),
            ),
            (
                icons::FLAG,
                "Flag message",
                "Flag · s",
                UiAction::ToggleFlag,
            ),
            (
                icons::MARK_UNREAD,
                "Mark read or unread",
                "Read or unread · u",
                UiAction::ToggleRead,
            ),
        ] {
            if icon_button(ui, has_row, icon, name, hint).clicked() {
                actions.push(action);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if icon_button(ui, in_folder, icons::SEARCH, "Search", "Search · /").clicked() {
                actions.push(UiAction::StartSearch);
            }
        });
    });
    actions
}

/// An icon button named `name` for screen readers and tests, with `hint` on hover.
pub(crate) fn icon_button(
    ui: &mut egui::Ui,
    enabled: bool,
    icon: &str,
    name: &str,
    hint: &str,
) -> egui::Response {
    let response = ui
        .add_enabled(
            enabled,
            egui::Button::new(icon)
                .frame(false)
                .min_size(egui::vec2(CONTROL, CONTROL)),
        )
        .on_hover_text(hint);
    let name = name.to_string();
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &name));
    response
}

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
            [
                ("home".to_string(), Command::SyncNow),
                ("work".to_string(), Command::SyncNow)
            ]
        );
    }

    #[test]
    fn the_status_bar_shows_the_version() {
        let fx = Fixture::new(&["work"]);
        let (harness, _wires) = fx.harness();
        assert!(
            harness
                .query_by_label(&format!("v{}", env!("CARGO_PKG_VERSION")))
                .is_some()
        );
    }
}
