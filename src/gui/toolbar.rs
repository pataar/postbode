//! The top bar: the logo, then actions on the cursor row or the selection, then search.
use eframe::egui;
use eframe::egui::containers::panel::PanelState;

use crate::rules::Action;

use super::app::{App, FOLDERS, FOLDERS_WIDTH, UiAction, View};
use super::icons;
use super::theme::{CONTROL, INSET};

pub(crate) fn show(app: &App, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    // As wide as the folder pane (last frame's, so it follows a resize), less the toolbar's margin, so the first
    // action's glyph lines up with the list's content.
    let folders = PanelState::load(ui.ctx(), egui::Id::new(FOLDERS))
        .map_or(FOLDERS_WIDTH, |state| state.outer_rect.width());
    let logo_cell = folders - INSET;
    ui.horizontal(|ui| {
        // As tall as the action buttons, so the row centres it on the same line as them.
        ui.allocate_ui(egui::vec2(logo_cell, CONTROL), |ui| {
            ui.set_min_size(egui::vec2(logo_cell, CONTROL));
            ui.horizontal_centered(|ui| {
                if let Some(logo) = &app.logo {
                    ui.add(egui::Image::new(logo).fit_to_exact_size(egui::vec2(20.0, 20.0)));
                }
                let serif = egui::FontFamily::Name(super::theme::SERIF.into());
                ui.label(
                    egui::RichText::new("Postbode")
                        .family(serif)
                        .size(19.0)
                        .strong(),
                );
            });
        });
        let in_folder = matches!(app.view, View::Folder { .. });
        // Closed picker only: an action would move the cursor, and the picker would then move a different message.
        let has_row = in_folder && !app.list.rows.is_empty() && app.move_picker.is_none();
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
    use eframe::egui;
    use egui_kittest::kittest::{NodeT, Queryable};

    use crate::gui::app::View;
    use crate::gui::test_support::{Fixture, message};
    use crate::rules::Action;
    use crate::sync::Command;

    const ACTIONS: [&str; 5] = [
        "Archive message",
        "Move message",
        "Delete message",
        "Flag message",
        "Mark read or unread",
    ];

    fn disabled(harness: &egui_kittest::Harness<'_, crate::gui::App>, label: &str) -> bool {
        harness.get_by_label(label).accesskit_node().is_disabled()
    }

    #[test]
    fn the_wordmark_lines_up_with_the_action_buttons() {
        let fx = Fixture::new(&["work"]);
        let (harness, _wires) = fx.harness();
        let wordmark = harness.get_by_label("Postbode").rect().center().y;
        let archive = harness.get_by_label(ACTIONS[0]).rect().center().y;
        assert!((wordmark - archive).abs() <= 1.0, "{wordmark} vs {archive}");
    }

    #[test]
    fn the_actions_are_enabled_on_a_row_and_disabled_outside_a_folder() {
        let fx = Fixture::new(&["work"]);
        fx.add("work", message("INBOX", 1, "hello"));
        let (mut harness, _wires) = fx.harness();
        assert!(ACTIONS.iter().all(|label| !disabled(&harness, label)));
        harness.state_mut().select_view(View::Rules);
        harness.run();
        assert!(ACTIONS.iter().all(|label| disabled(&harness, label)));
        assert!(disabled(&harness, "Search"));
    }

    #[test]
    fn the_actions_are_disabled_while_the_move_picker_is_open() {
        let fx = Fixture::new(&["work"]);
        fx.folder("work", "Archive", Some("Archive"));
        fx.add("work", message("INBOX", 1, "hello"));
        let (mut harness, _wires) = fx.harness();
        harness.event(egui::Event::Text("m".into()));
        harness.run();
        assert!(harness.state().move_picker.is_some());
        assert!(ACTIONS.iter().all(|label| disabled(&harness, label)));
    }

    #[test]
    fn the_archive_button_archives_the_cursor_row() {
        let fx = Fixture::new(&["work"]);
        fx.folder("work", "Archive", Some("Archive"));
        fx.add("work", message("INBOX", 1, "hello"));
        let (mut harness, wires) = fx.harness();
        harness.get_by_label("Archive message").click();
        harness.run();
        assert!(matches!(
            &wires.sent()[..],
            [(account, Command::Apply { folder, uids, action: Action::Archive, .. })]
                if account == "work" && folder == "INBOX" && uids == &[1]
        ));
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
