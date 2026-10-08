//! The left column: special folders, then custom folders as a tree, per account, then the Rules, Activity and Backups entries, pinned to the bottom.
use eframe::egui;

use crate::message::clean;
use crate::store::Folder;

use super::app::{Account, App, FolderRow, UiAction, View};
use super::{icons, numbers};

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
            actions.extend(account_folders(app, index, account, ui));
            ui.add_space(8.0);
        }
    });
    actions
}

/// The special folders flat, a separator, then the custom folders as a tree.
fn account_folders(app: &App, index: usize, account: &Account, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let open = |name: &str| View::Folder {
        account: index,
        folder: name.to_string(),
    };
    let (special, custom): (Vec<&FolderRow>, Vec<&FolderRow>) = account
        .folders
        .iter()
        .partition(|f| is_special(&f.name, f.special_use.as_deref()));
    for folder in &special {
        let name = clean(&folder.name, false);
        let (count, accessible) = count_and_name(&name, folder.unread);
        let icon = icons::for_special_use(folder.special_use.as_deref(), &folder.name);
        let view = open(&folder.name);
        if folder_row(ui, app.view == view, icon, &name, 0.0, count, &accessible).clicked() {
            actions.push(UiAction::SelectView(view));
        }
    }
    if !special.is_empty() && !custom.is_empty() {
        ui.separator();
    }
    let rows = tree(&account.folders);
    let is_collapsed = |path: &str| app.collapsed.contains(&(index, path.to_string()));
    for row in visible(&rows, is_collapsed) {
        let label = clean(&row.label, false);
        let unread = row
            .folder
            .as_ref()
            .and_then(|name| account.folders.iter().find(|f| &f.name == name))
            .map_or(0, |f| f.unread);
        let (count, accessible) = count_and_name(&label, unread);
        let expanded = !is_collapsed(&row.path);
        let icon = match (row.has_children, expanded) {
            (false, _) => icons::FOLDER,
            (true, true) => icons::CARET_DOWN,
            (true, false) => icons::CARET_RIGHT,
        };
        let selected = row
            .folder
            .as_deref()
            .is_some_and(|name| app.view == open(name));
        let indent = INDENT * row.depth as f32;
        let response = folder_row(ui, selected, icon, &label, indent, count, &accessible);
        if row.has_children && caret(ui, &response, expanded, &label, (index, &row.path)).clicked()
        {
            actions.push(UiAction::ToggleFolder(index, row.path.clone()));
        } else if response.clicked()
            && let Some(name) = &row.folder
        {
            actions.push(UiAction::SelectView(open(name)));
        }
    }
    actions
}

/// How far each tree level is indented.
const INDENT: f32 = 16.0;

/// The click target over a parent row's icon, which shows the caret: it folds the branch instead of opening the row.
fn caret(
    ui: &mut egui::Ui,
    row: &egui::Response,
    expanded: bool,
    label: &str,
    id: (usize, &String),
) -> egui::Response {
    let padding = ui.spacing().button_padding.x;
    let rect = egui::Rect::from_min_size(
        row.rect.min,
        egui::vec2(padding * 2.0 + INDENT, row.rect.height()),
    );
    let response = ui.interact(rect, ui.id().with(("caret", id)), egui::Sense::click());
    let name = format!("{} {label}", if expanded { "Collapse" } else { "Expand" });
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
    response
}

fn count_and_name(name: &str, unread: u32) -> (Option<String>, String) {
    match unread {
        0 => (None, name.to_string()),
        unread => {
            let unread = numbers::count(u64::from(unread));
            (Some(unread.clone()), format!("{name} ({unread})"))
        }
    }
}

/// One row of the custom folder tree.
#[derive(Debug, PartialEq)]
pub(crate) struct TreeRow {
    /// The folder a click opens; None for a parent the server does not list (not selectable).
    pub folder: Option<String>,
    pub label: String,
    pub depth: usize,
    /// Segments joined by '/', the key for collapse state.
    pub path: String,
    pub has_children: bool,
}

/// The custom folders as a tree. Sorted by path segments, not by full name, so "Work Archive" cannot land between
/// "Work" and "Work/Clients". An `INBOX` + delimiter prefix (Dovecot and Courier namespaces) is dropped when every
/// custom folder has it; otherwise INBOX's children stay under an INBOX branch, apart from top-level namesakes.
pub(crate) fn tree(folders: &[FolderRow]) -> Vec<TreeRow> {
    let custom: Vec<&FolderRow> = folders
        .iter()
        .filter(|f| !is_special(&f.name, f.special_use.as_deref()))
        .collect();
    let strip = !custom.is_empty() && custom.iter().all(|f| inbox_prefix(f).is_some());
    let mut paths: Vec<(Vec<String>, &FolderRow)> = custom
        .into_iter()
        .map(|f| (segments(f, strip), f))
        .collect();
    paths.sort_by_key(|(segments, _)| {
        segments
            .iter()
            .map(|s| s.to_lowercase())
            .collect::<Vec<_>>()
    });
    let mut rows: Vec<TreeRow> = Vec::new();
    for (segments, folder) in paths {
        for depth in 0..segments.len() {
            let path = segments[..=depth].join("/");
            let leaf = depth + 1 == segments.len();
            // ponytail: linear parent lookup; a HashMap from path to index if accounts with thousands of folders show up
            match rows.iter_mut().find(|row| row.path == path) {
                Some(row) if leaf => row.folder = Some(folder.name.clone()),
                Some(_) => {}
                None => rows.push(TreeRow {
                    folder: leaf.then(|| folder.name.clone()),
                    label: segments[depth].clone(),
                    depth,
                    path,
                    has_children: false,
                }),
            }
        }
    }
    for row in rows.iter_mut().filter(|row| row.folder.is_none()) {
        row.folder = folders
            .iter()
            .find(|f| segments(f, strip).join("/") == row.path)
            .map(|f| f.name.clone());
    }
    for index in 1..rows.len() {
        if rows[index].depth > rows[index - 1].depth {
            rows[index - 1].has_children = true;
        }
    }
    rows
}

/// The name after a leading `INBOX` + delimiter, when it has one.
fn inbox_prefix(folder: &FolderRow) -> Option<&str> {
    let delimiter = folder.delimiter.as_deref().filter(|d| !d.is_empty())?;
    let prefix = format!("INBOX{delimiter}");
    let head = folder.name.get(..prefix.len())?;
    head.eq_ignore_ascii_case(&prefix)
        .then(|| &folder.name[prefix.len()..])
}

fn segments(folder: &FolderRow, strip_inbox: bool) -> Vec<String> {
    let Some(delimiter) = folder.delimiter.as_deref().filter(|d| !d.is_empty()) else {
        return vec![folder.name.clone()];
    };
    let name = inbox_prefix(folder)
        .filter(|_| strip_inbox)
        .unwrap_or(&folder.name);
    name.split(delimiter)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// The rows not under a collapsed ancestor.
pub(crate) fn visible(rows: &[TreeRow], collapsed: impl Fn(&str) -> bool) -> Vec<&TreeRow> {
    let mut hidden_below: Option<usize> = None;
    let mut shown = Vec::new();
    for row in rows {
        if hidden_below.is_some_and(|depth| row.depth > depth) {
            continue;
        }
        hidden_below = collapsed(&row.path).then_some(row.depth);
        shown.push(row);
    }
    shown
}

/// The folders an account shows, in the order the pane draws them: special folders, then the visible tree.
pub(crate) fn shown(account: &Account, collapsed: impl Fn(&str) -> bool) -> Vec<String> {
    let special = account
        .folders
        .iter()
        .filter(|f| is_special(&f.name, f.special_use.as_deref()))
        .map(|f| f.name.clone());
    let rows = tree(&account.folders);
    let custom = visible(&rows, collapsed)
        .into_iter()
        .filter_map(|row| row.folder.clone());
    special.chain(custom).collect()
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
    use crate::gui::app::{Focus, FolderRow, View};
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

    fn custom(name: &str, delimiter: &str) -> FolderRow {
        FolderRow {
            name: name.into(),
            special_use: None,
            unread: 0,
            delimiter: Some(delimiter.into()),
        }
    }

    fn stored(name: &str) -> Folder {
        Folder {
            name: name.into(),
            uidvalidity: 1,
            last_uid: 0,
            special_use: None,
            delimiter: Some("/".into()),
        }
    }

    fn shape(rows: &[TreeRow]) -> Vec<(usize, &str, bool)> {
        rows.iter()
            .map(|r| (r.depth, r.label.as_str(), r.folder.is_some()))
            .collect()
    }

    #[test]
    fn tree_nests_by_delimiter_and_adds_unlisted_parents() {
        let rows = tree(&[
            custom("Projects/Postbode/GitHub", "/"),
            custom("Projects/Website", "/"),
        ]);
        assert_eq!(
            shape(&rows),
            [
                (0, "Projects", false),
                (1, "Postbode", false),
                (2, "GitHub", true),
                (1, "Website", true)
            ]
        );
        assert!(rows[0].has_children && rows[1].has_children && !rows[2].has_children);
    }

    #[test]
    fn a_sibling_sorting_between_parent_and_child_does_not_steal_the_child() {
        let rows = tree(&[
            custom("Work", "/"),
            custom("Work Archive", "/"),
            custom("Work/Clients", "/"),
        ]);
        assert_eq!(
            shape(&rows),
            [
                (0, "Work", true),
                (1, "Clients", true),
                (0, "Work Archive", true)
            ]
        );
    }

    #[test]
    fn an_inbox_namespace_prefix_is_not_a_parent() {
        let rows = tree(&[
            custom("INBOX.Clients", "."),
            custom("INBOX.Clients.Acme", "."),
        ]);
        assert_eq!(shape(&rows), [(0, "Clients", true), (1, "Acme", true)]);
    }

    #[test]
    fn the_inbox_prefix_is_dropped_only_when_every_custom_folder_has_it() {
        let rows = tree(&[custom("Clients", "."), custom("INBOX.Clients", ".")]);
        let mut opened: Vec<&str> = rows.iter().filter_map(|r| r.folder.as_deref()).collect();
        opened.sort();
        assert_eq!(opened, ["Clients", "INBOX.Clients"]);
    }

    #[test]
    fn a_special_folder_with_children_is_an_openable_parent() {
        let mut archive = custom("Archive", "/");
        archive.special_use = Some("Archive".into());
        let rows = tree(&[archive, custom("Archive/2024", "/")]);
        assert_eq!(rows[0].folder.as_deref(), Some("Archive"));
        assert_eq!(shape(&rows), [(0, "Archive", true), (1, "2024", true)]);
    }

    #[test]
    fn stepping_from_a_folder_in_a_collapsed_branch_stays_in_its_account() {
        let fx = Fixture::new(&["home", "work"]);
        let store = fx.store("work");
        for name in ["Projects/Postbode", "Projects/Website", "Zeta"] {
            store.upsert_folder(&stored(name)).unwrap();
        }
        let (mut harness, _wires) = fx.harness();
        let work = |name: &str| View::Folder {
            account: 1,
            folder: name.into(),
        };
        harness.state_mut().select_view(work("Projects/Postbode"));
        harness.run();
        harness.get_by_label("Collapse Projects").click();
        harness.run();
        harness.state_mut().focus = Focus::Folders;
        harness.key_press(egui::Key::ArrowDown);
        harness.run();
        assert_eq!(harness.state().view, work("Zeta"));
    }

    #[test]
    fn a_folder_without_a_delimiter_stays_flat() {
        let mut flat = custom("a/b", "/");
        flat.delimiter = None;
        assert_eq!(shape(&tree(&[flat])), [(0, "a/b", true)]);
    }

    #[test]
    fn collapsing_hides_descendants_only() {
        let rows = tree(&[custom("A/B/C", "/"), custom("A/D", "/"), custom("E", "/")]);
        let shown: Vec<&str> = visible(&rows, |path| path == "A/B")
            .iter()
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(shown, ["A", "B", "D", "E"]);
    }

    #[test]
    fn clicking_a_caret_collapses_its_folder() {
        let fx = Fixture::new(&["work"]);
        let store = fx.store("work");
        for name in ["Projects/Postbode", "Projects/Website"] {
            store.upsert_folder(&stored(name)).unwrap();
        }
        let (mut harness, _wires) = fx.harness();
        assert!(harness.query_by_label("Website").is_some());
        harness.get_by_label("Collapse Projects").click();
        harness.run();
        assert!(harness.query_by_label("Website").is_none());
        assert!(
            harness
                .state()
                .collapsed
                .contains(&(0, "Projects".to_string()))
        );
    }

    #[test]
    fn arrow_keys_follow_the_tree_and_skip_collapsed_folders() {
        let fx = Fixture::new(&["work"]);
        let store = fx.store("work");
        for name in ["Projects/Postbode", "Projects/Website", "Zeta"] {
            store.upsert_folder(&stored(name)).unwrap();
        }
        let (mut harness, _wires) = fx.harness();
        let folder = |name: &str| View::Folder {
            account: 0,
            folder: name.into(),
        };
        harness.state_mut().select_view(folder("Projects/Postbode"));
        harness.state_mut().focus = Focus::Folders;
        harness.run();
        harness.key_press(egui::Key::ArrowDown);
        harness.run();
        assert_eq!(harness.state().view, folder("Projects/Website"));
        harness.get_by_label("Collapse Projects").click();
        harness.state_mut().select_view(folder("INBOX"));
        harness.state_mut().focus = Focus::Folders;
        harness.run();
        harness.key_press(egui::Key::ArrowDown);
        harness.run();
        assert_eq!(harness.state().view, folder("Zeta"));
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
