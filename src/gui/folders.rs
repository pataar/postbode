//! The left column: one tree per account, then the Rules, Activity and Backups entries, pinned to the bottom.
use eframe::egui;

use crate::message::clean;
use crate::store::Folder;

use super::app::{App, UiAction, View};
use super::icons;

pub(crate) fn show(app: &App, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    egui::Panel::bottom("views")
        .frame(egui::Frame::NONE)
        .show(ui, |ui| {
            for (view, icon, label) in [
                (View::Rules, icons::RULES, "Rules"),
                (View::Activity, icons::ACTIVITY, "Activity"),
                (View::Trash, icons::BACKUPS, "Backups"),
            ] {
                if folder_row(ui, app.view == view, icon, label, 0.0, None, label).clicked() {
                    actions.push(UiAction::SelectView(view));
                }
            }
        });
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (index, account) in app.accounts.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.strong(&account.name);
                if account.busy() {
                    ui.spinner();
                }
            });
            if let Err(e) = &account.store {
                ui.colored_label(ui.visuals().error_fg_color, clean(e, false));
            }
            let mut previous_special = false;
            for folder in &account.folders {
                let special = is_special(&folder.name, folder.special_use.as_deref());
                if previous_special && !special {
                    ui.separator();
                }
                previous_special = special;
                let view = View::Folder {
                    account: index,
                    folder: folder.name.clone(),
                };
                let name = clean(&folder.name, false);
                let (count, accessible) = match folder.unread {
                    0 => (None, name.clone()),
                    unread => (Some(unread.to_string()), format!("{name} ({unread})")),
                };
                let icon = icons::for_special_use(folder.special_use.as_deref(), &folder.name);
                if folder_row(ui, app.view == view, icon, &name, 0.0, count, &accessible).clicked()
                {
                    actions.push(UiAction::SelectView(view));
                }
            }
            ui.add_space(8.0);
        }
    });
    actions
}

/// A full-width selectable row: icon, label, and the count right-aligned. `accessible` names it for screen readers
/// and tests, since the count is painted apart from the label.
pub(crate) fn folder_row(
    ui: &mut egui::Ui,
    selected: bool,
    icon: &str,
    label: &str,
    indent: f32,
    count: Option<String>,
    accessible: &str,
) -> egui::Response {
    let text = format!("{icon}  {label}");
    let response = ui.add(
        egui::Button::selectable(selected, text)
            .truncate()
            .min_size(egui::vec2(ui.available_width() - indent, 0.0)),
    );
    if let Some(count) = count {
        let palette = super::theme::palette(ui);
        let color = if selected {
            palette.on_accent
        } else {
            palette.muted
        };
        ui.painter().text(
            response.rect.right_center() - egui::vec2(6.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            count,
            egui::TextStyle::Button.resolve(ui.style()),
            color,
        );
    }
    let name = accessible.to_string();
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &name)
    });
    response
}

/// INBOX and the special-use folders, which sort above the rest.
pub(crate) fn is_special(name: &str, special_use: Option<&str>) -> bool {
    rank(name, special_use) < 6
}

/// INBOX first, then the special-use folders in a fixed order, then the rest alphabetically.
pub(crate) fn sort(folders: &mut [Folder]) {
    folders.sort_by_key(|f| {
        (
            rank(&f.name, f.special_use.as_deref()),
            f.name.to_lowercase(),
        )
    });
}

fn rank(name: &str, special_use: Option<&str>) -> usize {
    if name.eq_ignore_ascii_case("INBOX") {
        return 0;
    }
    match special_use {
        Some("Archive") => 1,
        Some("Drafts") => 2,
        Some("Sent") => 3,
        Some("Junk") => 4,
        Some("Trash") => 5,
        _ => 6,
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::gui::app::View;
    use crate::gui::test_support::{Fixture, message};
    use crate::store::Folder;

    fn folder(name: &str, special_use: Option<&str>) -> Folder {
        Folder {
            name: name.into(),
            uidvalidity: 1,
            last_uid: 0,
            special_use: special_use.map(str::to_string),
        }
    }

    #[test]
    fn sort_puts_inbox_then_special_use_then_the_rest_alphabetically() {
        let mut folders = vec![
            folder("zeta", None),
            folder("Trash", Some("Trash")),
            folder("alpha", None),
            folder("Junk", Some("Junk")),
            folder("Sent Items", Some("Sent")),
            folder("Drafts", Some("Drafts")),
            folder("All Mail", Some("Archive")),
            folder("INBOX", None),
        ];
        sort(&mut folders);
        let names: Vec<&str> = folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "INBOX",
                "All Mail",
                "Drafts",
                "Sent Items",
                "Junk",
                "Trash",
                "alpha",
                "zeta"
            ]
        );
    }

    #[test]
    fn tree_shows_folders_with_unread_counts_and_selects_on_click() {
        let fx = Fixture::new(&["work"]);
        fx.folder("work", "Archive", Some("Archive"));
        let mut unread = message("INBOX", 1, "hello");
        unread.flags = String::new();
        fx.add("work", unread);
        fx.add("work", message("INBOX", 2, "read already"));
        let (mut harness, _wires) = fx.harness();
        assert!(harness.query_by_label("INBOX (1)").is_some());
        harness.get_by_label("Archive").click();
        harness.run();
        assert_eq!(
            harness.state().view,
            View::Folder {
                account: 0,
                folder: "Archive".into()
            }
        );
        harness.get_by_label("Rules").click();
        harness.run();
        assert_eq!(harness.state().view, View::Rules);
    }

    #[test]
    fn rows_are_full_width_and_keep_the_count_in_their_name() {
        let fx = Fixture::new(&["work"]);
        let mut unread = message("INBOX", 1, "hello");
        unread.flags = String::new();
        fx.add("work", unread);
        let (harness, _wires) = fx.harness();
        let inbox = harness.get_by_label("INBOX (1)").rect();
        let rules = harness.get_by_label("Rules").rect();
        assert!(
            (inbox.width() - rules.width()).abs() < 1.0,
            "{inbox:?} vs {rules:?}"
        );
    }

    #[test]
    fn a_separator_divides_special_from_custom_folders() {
        let fx = Fixture::new(&["work"]);
        fx.folder("work", "Junk", Some("Junk"));
        fx.folder("work", "Receipts", None);
        let (harness, _wires) = fx.harness();
        let junk = harness.get_by_label("Junk").rect();
        let receipts = harness.get_by_label("Receipts").rect();
        assert!(
            receipts.top() - junk.bottom()
                > harness.ctx.global_style().spacing.item_spacing.y + 2.0
        );
    }

    #[test]
    fn the_backup_view_is_labelled_backups_and_sits_at_the_bottom() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        let backups = harness.get_by_label("Backups").rect();
        assert!(
            backups.bottom() > 700.0,
            "pinned to the bottom of an 800 px window"
        );
        harness.get_by_label("Backups").click();
        harness.run();
        assert_eq!(harness.state().view, View::Trash);
    }

    #[test]
    fn a_store_that_fails_to_open_shows_its_error_and_other_accounts_still_work() {
        let broken = Fixture::new(&["broken", "work"]);
        let db = broken.paths.mail_db("broken");
        std::fs::remove_file(&db).unwrap();
        std::fs::create_dir_all(&db).unwrap();
        let (harness, _wires) = broken.harness();
        assert!(
            harness
                .query_by_label_contains("could not open the store")
                .is_some()
        );
        assert!(harness.query_by_label("INBOX").is_some());
    }
}
