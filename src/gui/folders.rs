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

/// A full-width selectable row: indent, icon, label, and the count right-aligned. `accessible` names it for screen
/// readers and tests, so they read the count but not the icon glyph.
pub(crate) fn folder_row(
    ui: &mut egui::Ui,
    selected: bool,
    icon: &str,
    label: &str,
    indent: f32,
    count: Option<String>,
    accessible: &str,
) -> egui::Response {
    ui.horizontal(|ui| {
        if indent > 0.0 {
            ui.add_space(indent);
        }
        let palette = super::theme::palette(ui);
        let color = if selected {
            palette.on_accent
        } else {
            palette.muted
        };
        // Always a right text, even empty: its grow atom is what keeps the label on the left.
        let button = egui::Button::selectable(selected, format!("{icon}  {label}"))
            .truncate()
            .min_size(egui::vec2(ui.available_width(), 0.0))
            .right_text(egui::RichText::new(count.unwrap_or_default()).color(color));
        let response = ui.add(button);
        let name = accessible.to_string();
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &name)
        });
        response
    })
    .inner
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
            delimiter: None,
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

    fn painted_text(harness: &egui_kittest::Harness<'_>) -> Vec<(String, egui::Rect)> {
        harness
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
            .collect()
    }

    #[test]
    fn a_long_name_truncates_before_its_count() {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(160.0, 40.0))
            .build_ui(|ui| {
                let name = "Receipts from suppliers 2024";
                folder_row(
                    ui,
                    false,
                    icons::FOLDER,
                    name,
                    0.0,
                    Some("123".into()),
                    name,
                );
            });
        harness.run();
        let painted = painted_text(&harness);
        let find = |wanted: &dyn Fn(&str) -> bool| {
            painted
                .iter()
                .find(|(text, _)| wanted(text))
                .map(|(_, rect)| *rect)
        };
        let name = find(&|text| text.contains("Receipts")).unwrap();
        let count = find(&|text| text == "123").unwrap();
        assert!(name.right() <= count.left(), "{painted:?}");
    }

    #[test]
    fn a_row_without_a_count_keeps_its_label_on_the_left() {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(200.0, 40.0))
            .build_ui(|ui| {
                folder_row(ui, false, icons::FOLDER, "Work", 0.0, None, "Work");
            });
        harness.run();
        let painted = painted_text(&harness);
        let (_, label) = painted
            .iter()
            .find(|(text, _)| text.contains("Work"))
            .unwrap();
        assert!(label.left() < 30.0, "{painted:?}");
    }

    #[test]
    fn an_indented_row_starts_later_and_ends_at_the_same_edge() {
        let rects = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = rects.clone();
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(200.0, 80.0))
            .build_ui(move |ui| {
                let flat = folder_row(ui, false, icons::FOLDER, "Work", 0.0, None, "Work");
                let nested = folder_row(ui, false, icons::FOLDER, "Clients", 16.0, None, "Clients");
                *seen.borrow_mut() = vec![flat.rect, nested.rect];
            });
        harness.run();
        let rects = rects.borrow();
        let (flat, nested) = (rects[0], rects[1]);
        assert!(nested.left() >= flat.left() + 16.0, "{flat:?} {nested:?}");
        assert!(
            (nested.right() - flat.right()).abs() < 1.0,
            "{flat:?} {nested:?}"
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
