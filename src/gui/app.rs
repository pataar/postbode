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
                unread: store.unread_count(&f.name)?,
                name: f.name,
            })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FolderRow {
    pub name: String,
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
    SelectView(View),
}

pub struct App {
    pub(crate) accounts: Vec<Account>,
    pub(crate) engine: Option<Engine>,
    pub(crate) events: Receiver<Event>,
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
        App {
            accounts,
            engine: Some(engine),
            events,
            theme: preference(config.ui.theme),
            theme_applied: false,
            view: View::Folder {
                account: 0,
                folder: "INBOX".into(),
            },
        }
    }

    /// One frame: events in, panels drawn, then what the panels asked for.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if !self.theme_applied {
            ui.ctx().set_theme(self.theme);
            self.theme_applied = true;
        }
        while let Ok(event) = self.events.try_recv() {
            self.handle(event);
        }
        let mut actions = Vec::new();
        egui::Panel::left("folders")
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| actions.extend(folders::show(self, ui)));
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
            UiAction::SelectView(view) => self.view = view,
        }
    }
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
