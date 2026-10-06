//! The left column: one tree per account, then the Rules, Activity and Trash entries.
use eframe::egui;

use crate::engine::StartState;
use crate::message::clean;
use crate::store::Folder;

use super::app::{App, UiAction, View};

pub(crate) fn show(app: &App, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (index, account) in app.accounts.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.strong(&account.name);
                if account.busy() {
                    ui.spinner();
                }
            });
            match &account.state {
                StartState::Locked { pid } => {
                    ui.weak(locked_text(*pid));
                }
                StartState::Failed(e) => {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("could not start: {}", clean(e, false)),
                    );
                }
                StartState::Running => {}
            }
            if let Err(e) = &account.store {
                ui.colored_label(ui.visuals().error_fg_color, clean(e, false));
            }
            for folder in &account.folders {
                let view = View::Folder {
                    account: index,
                    folder: folder.name.clone(),
                };
                let name = clean(&folder.name, false);
                let label = match folder.unread {
                    0 => name,
                    unread => format!("{name} ({unread})"),
                };
                if ui.selectable_label(app.view == view, label).clicked() {
                    actions.push(UiAction::SelectView(view));
                }
            }
            ui.add_space(8.0);
        }
        ui.separator();
        for (view, label) in [
            (View::Rules, "Rules"),
            (View::Activity, "Activity"),
            (View::Trash, "Trash"),
        ] {
            if ui.selectable_label(app.view == view, label).clicked() {
                actions.push(UiAction::SelectView(view));
            }
        }
    });
    actions
}

pub(crate) fn locked_text(pid: Option<u32>) -> String {
    match pid {
        Some(pid) => format!("synced by another Postbode process (pid {pid})"),
        None => "synced by another Postbode process".into(),
    }
}

/// INBOX first, then the special-use folders in a fixed order, then the rest alphabetically.
pub(crate) fn sort(folders: &mut [Folder]) {
    folders.sort_by_key(|f| (rank(f), f.name.to_lowercase()));
}

fn rank(folder: &Folder) -> usize {
    if folder.name.eq_ignore_ascii_case("INBOX") {
        return 0;
    }
    match folder.special_use.as_deref() {
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
    use crate::engine::StartState;
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
    fn locked_account_says_who_syncs_it() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        harness.state_mut().accounts[0].state = StartState::Locked { pid: Some(42) };
        harness.run();
        assert!(
            harness
                .query_by_label("synced by another Postbode process (pid 42)")
                .is_some()
        );
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
