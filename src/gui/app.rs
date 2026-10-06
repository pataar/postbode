//! App state, the frame loop, and the only code that changes state: `handle` for events, `apply` for UI actions.
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use eframe::egui;

use crate::config::{Config, Theme};
use crate::engine::{Engine, StartState};
use crate::paths::Paths;
use crate::store::{Store, StoreError};
use crate::sync::{Activity, Event};

use super::folders;
use super::list::{self, ListState, THREAD_LIMIT};

pub(crate) struct Account {
    pub activity: Option<Activity>,
    pub folders: Vec<FolderRow>,
    pub name: String,
    pub state: StartState,
    pub store: Result<Store, String>,
}

impl Account {
    /// True while the sync thread works, for the spinner beside the account.
    pub fn busy(&self) -> bool {
        self.state == StartState::Running
            && !matches!(
                self.activity,
                None | Some(Activity::Idle { .. } | Activity::Offline { .. })
            )
    }

    pub fn reload_folders(&mut self) {
        let Ok(store) = &self.store else { return };
        match folder_rows(store) {
            Ok(rows) => self.folders = rows,
            Err(e) => log::warn!("[{}] could not list folders: {e}", self.name),
        }
    }
}

fn folder_rows(store: &Store) -> Result<Vec<FolderRow>, StoreError> {
    let mut folders = store.folders()?;
    folders::sort(&mut folders);
    folders
        .into_iter()
        .map(|f| {
            Ok(FolderRow {
                special_use: f.special_use,
                unread: store.unread_count(&f.name)?,
                name: f.name,
            })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FolderRow {
    pub name: String,
    pub special_use: Option<String>,
    pub unread: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum View {
    Activity,
    Folder { account: usize, folder: String },
    Rules,
    Trash,
}

/// What a view asks for; `App::apply` carries it out after the frame.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UiAction {
    Collapse,
    Escape,
    Expand,
    ListViewport { offset: f32, height: f32 },
    MoveCursor(isize),
    NextFocus,
    SelectRow(usize),
    SelectView(View),
    StepFolder(isize),
    ToggleMark,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Focus {
    Body,
    Folders,
    List,
}

pub struct App {
    pub(crate) accounts: Vec<Account>,
    pub(crate) engine: Option<Engine>,
    pub(crate) events: Receiver<Event>,
    pub(crate) focus: Focus,
    pub(crate) list: ListState,
    pub(crate) theme: egui::ThemePreference,
    pub(crate) theme_applied: bool,
    pub(crate) view: View,
}

impl App {
    pub fn new(config: &Config, paths: Paths, engine: Engine, events: Receiver<Event>) -> App {
        let accounts = engine
            .accounts()
            .iter()
            .map(|(name, state)| {
                let store = Store::open(&paths.mail_db(name))
                    .map_err(|e| format!("could not open the store: {e}"));
                let mut account = Account {
                    activity: None,
                    folders: Vec::new(),
                    name: name.clone(),
                    state: state.clone(),
                    store,
                };
                account.reload_folders();
                account
            })
            .collect();
        let mut app = App {
            accounts,
            engine: Some(engine),
            events,
            focus: Focus::List,
            list: ListState::default(),
            theme: preference(config.ui.theme),
            theme_applied: false,
            view: View::Folder {
                account: 0,
                folder: "INBOX".into(),
            },
        };
        app.reload_view();
        app
    }

    /// One frame: events in, keys applied, panels drawn, then what the panels asked for.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.theme_applied {
            ctx.set_theme(self.theme);
            self.theme_applied = true;
        }
        while let Ok(event) = self.events.try_recv() {
            self.handle(event);
        }
        for action in self.keys(&ctx) {
            self.apply(action);
        }
        // egui moves widget focus on Tab and arrows itself, and Space or Enter would then click the focused row.
        if !ctx.text_edit_focused() {
            ctx.memory_mut(|memory| {
                if let Some(id) = memory.focused() {
                    memory.surrender_focus(id);
                }
            });
        }
        let mut actions = Vec::new();
        egui::Panel::left("folders")
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| actions.extend(folders::show(self, ui)));
        if let View::Folder { .. } = self.view {
            egui::Panel::left("list")
                .resizable(true)
                .default_size(480.0)
                .show(ui, |ui| actions.extend(list::show(self, ui)));
        }
        egui::CentralPanel::default().show(ui, |_ui| {});
        for action in actions {
            self.apply(action);
        }
    }

    fn handle(&mut self, event: Event) {
        if let Event::Activity { account, activity } = event
            && let Some(account) = self.accounts.iter_mut().find(|a| a.name == account)
        {
            account.activity = Some(activity);
        }
    }

    fn apply(&mut self, action: UiAction) {
        match action {
            UiAction::Collapse => self.collapse(),
            UiAction::Escape => self.list.marked.clear(),
            UiAction::Expand => self.expand(),
            UiAction::ListViewport { offset, height } => {
                self.list.viewport = (offset, height);
                self.list.follow_cursor = false;
            }
            UiAction::MoveCursor(delta) => {
                self.list.cursor = step(self.list.cursor, delta, self.list.rows.len());
                self.list.follow_cursor = true;
            }
            UiAction::NextFocus => {
                self.focus = match self.focus {
                    Focus::Body => Focus::Folders,
                    Focus::Folders => Focus::List,
                    Focus::List => Focus::Body,
                }
            }
            UiAction::SelectRow(index) => {
                self.list.cursor = index;
                self.focus = Focus::List;
            }
            UiAction::SelectView(view) => self.select_view(view),
            UiAction::StepFolder(delta) => {
                let entries = self.tree_entries();
                let at = entries.iter().position(|v| *v == self.view).unwrap_or(0);
                let view = entries[step(at, delta, entries.len())].clone();
                if view != self.view {
                    self.select_view(view);
                }
            }
            UiAction::ToggleMark => {
                if let Some(key) = self.list.rows.get(self.list.cursor).map(list::Row::key)
                    && !self.list.marked.remove(&key)
                {
                    self.list.marked.insert(key);
                }
            }
        }
    }

    /// Shortcut keys for the focused pane. While a text field has focus only Esc counts, so typed letters stay text.
    fn keys(&self, ctx: &egui::Context) -> Vec<UiAction> {
        let typing = ctx.text_edit_focused();
        let mut actions = Vec::new();
        ctx.input_mut(|input| {
            let none = egui::Modifiers::NONE;
            if input.consume_key(none, egui::Key::Escape) {
                actions.push(UiAction::Escape);
            }
            if typing {
                return;
            }
            if input.consume_key(none, egui::Key::Tab) {
                actions.push(UiAction::NextFocus);
            }
            let down = typed(input, "j") || input.consume_key(none, egui::Key::ArrowDown);
            let up = typed(input, "k") || input.consume_key(none, egui::Key::ArrowUp);
            match self.focus {
                Focus::Folders => {
                    if down {
                        actions.push(UiAction::StepFolder(1));
                    }
                    if up {
                        actions.push(UiAction::StepFolder(-1));
                    }
                }
                Focus::Body | Focus::List => {
                    if down {
                        actions.push(UiAction::MoveCursor(1));
                    }
                    if up {
                        actions.push(UiAction::MoveCursor(-1));
                    }
                    if input.consume_key(none, egui::Key::ArrowRight) {
                        actions.push(UiAction::Expand);
                    }
                    if input.consume_key(none, egui::Key::ArrowLeft) {
                        actions.push(UiAction::Collapse);
                    }
                    if typed(input, "x") {
                        actions.push(UiAction::ToggleMark);
                    }
                }
            }
        });
        actions
    }

    pub(crate) fn select_view(&mut self, view: View) {
        self.view = view;
        self.list = ListState::default();
        self.reload_view();
    }

    pub(crate) fn view_account(&self) -> Option<usize> {
        match self.view {
            View::Folder { account, .. } => Some(account),
            _ => None,
        }
    }

    /// Sent and Drafts list the recipient instead of the sender.
    pub(crate) fn shows_recipient(&self) -> bool {
        let View::Folder { account, folder } = &self.view else {
            return false;
        };
        self.accounts[*account].folders.iter().any(|f| {
            &f.name == folder && matches!(f.special_use.as_deref(), Some("Drafts" | "Sent"))
        })
    }

    /// Loads the shown view's rows from the store. Runs on view changes and when events mark the view dirty, never
    /// per frame: `thread_summaries` scans the folder.
    pub(crate) fn reload_view(&mut self) {
        let View::Folder { account, folder } = &self.view else {
            return;
        };
        let (account, folder) = (*account, folder.clone());
        let Ok(store) = &self.accounts[account].store else {
            self.list.threads.clear();
            self.list.expanded.clear();
            self.rebuild_rows();
            return;
        };
        match store.thread_summaries(&folder, THREAD_LIMIT) {
            Ok(threads) => self.list.threads = threads,
            Err(e) => log::warn!(
                "[{}] {folder}: could not load threads: {e}",
                self.accounts[account].name
            ),
        }
        let expanded: Vec<String> = self.list.expanded.keys().cloned().collect();
        for thread in expanded {
            match store.thread_members(&folder, &thread) {
                Ok(members) if members.len() > 1 => {
                    self.list.expanded.insert(thread, members);
                }
                _ => {
                    self.list.expanded.remove(&thread);
                }
            }
        }
        self.rebuild_rows();
    }

    /// Rows from the cached threads, without a store query; keeps the cursor in range.
    pub(crate) fn rebuild_rows(&mut self) {
        let View::Folder { folder, .. } = &self.view else {
            return;
        };
        self.list.rows = list::build_rows(folder, &self.list.threads, &self.list.expanded);
        self.list.cursor = self.list.cursor.min(self.list.rows.len().saturating_sub(1));
    }

    fn expand(&mut self) {
        let Some(account) = self.view_account() else {
            return;
        };
        let Some(row) = self.list.rows.get(self.list.cursor) else {
            return;
        };
        let (Some(thread), folder) = (row.thread_id.clone(), row.folder.clone()) else {
            return;
        };
        if row.count < 2 || self.list.expanded.contains_key(&thread) {
            return;
        }
        if let Ok(store) = &self.accounts[account].store
            && let Ok(members) = store.thread_members(&folder, &thread)
        {
            self.list.expanded.insert(thread, members);
            self.rebuild_rows();
        }
    }

    /// Collapses the thread the cursor is in and puts the cursor on its thread row.
    fn collapse(&mut self) {
        let rows = &self.list.rows;
        if rows.is_empty() {
            return;
        }
        let from = self.list.cursor.min(rows.len() - 1);
        let Some(parent) = (0..=from).rev().find(|&i| rows[i].thread_id.is_some()) else {
            return;
        };
        let Some(thread) = rows[parent].thread_id.clone() else {
            return;
        };
        if self.list.expanded.remove(&thread).is_some() {
            self.list.cursor = parent;
            self.rebuild_rows();
        }
    }

    /// Every folder of every account, then Rules, Activity and Trash, in tree order.
    fn tree_entries(&self) -> Vec<View> {
        let mut entries: Vec<View> = self
            .accounts
            .iter()
            .enumerate()
            .flat_map(|(account, a)| {
                a.folders.iter().map(move |f| View::Folder {
                    account,
                    folder: f.name.clone(),
                })
            })
            .collect();
        entries.extend([View::Rules, View::Activity, View::Trash]);
        entries
    }
}

/// A printable shortcut, matched on the typed text so it follows the keyboard layout.
pub(crate) fn typed(input: &egui::InputState, text: &str) -> bool {
    input
        .events
        .iter()
        .any(|event| matches!(event, egui::Event::Text(t) if t == text))
}

fn step(index: usize, delta: isize, len: usize) -> usize {
    index
        .saturating_add_signed(delta)
        .min(len.saturating_sub(1))
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(engine) = self.engine.take() {
            stop_within(move || engine.stop(), Duration::from_secs(2));
        }
    }
}

/// Runs `stop` but returns after `limit`, so a server stalled mid-command cannot hold the closed window; the process
/// exit then ends the threads, and SQLite's WAL keeps the store consistent.
pub(crate) fn stop_within(stop: impl FnOnce() + Send + 'static, limit: Duration) {
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        stop();
        let _ = done.send(());
    });
    let _ = finished.recv_timeout(limit);
}

pub(crate) fn preference(theme: Theme) -> egui::ThemePreference {
    match theme {
        Theme::Dark => egui::ThemePreference::Dark,
        Theme::Light => egui::ThemePreference::Light,
        Theme::System => egui::ThemePreference::System,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::gui::test_support::Fixture;

    #[test]
    fn theme_starts_from_config() {
        let fx = Fixture::new(&["work"]);
        let (harness, _wires) = fx.harness();
        assert_eq!(
            harness.ctx.options(|o| o.theme_preference),
            egui::ThemePreference::System
        );
        let fx = Fixture::new(&["work"]);
        fx.append_config("[ui]\ntheme = \"dark\"\n");
        let (harness, _wires) = fx.harness();
        assert_eq!(
            harness.ctx.options(|o| o.theme_preference),
            egui::ThemePreference::Dark
        );
    }

    #[test]
    fn stop_within_gives_up_on_a_stuck_stop() {
        let started = Instant::now();
        stop_within(
            || std::thread::sleep(Duration::from_secs(10)),
            Duration::from_millis(100),
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
