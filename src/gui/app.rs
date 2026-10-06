//! App state, the frame loop, and the only code that changes state: `handle` for events, `apply` for UI actions.
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime};

use eframe::egui;

use crate::config::{self, Config, Theme};
use crate::engine::{Engine, StartState};
use crate::message::{self, Attachment, clean};
use crate::paths::Paths;
use crate::rules::{self as rule_file, Action, RulesError};
use crate::store::{Message, Store, StoreError};
use crate::sync::{self, Activity, Command, Event};

use super::body;
use super::folders;
use super::list::{self, ListState, Optimistic, Row, RowKey, THREAD_LIMIT};
use super::rules::{self, RulesState};
use super::status;

/// Lines kept for the history window.
const HISTORY: usize = 50;

/// Seconds between checks of rules.toml and config.toml for outside edits.
const POLL: f64 = 2.0;

/// How long a message stays on screen before it is marked read.
const READ_DELAY: f64 = 1.0;

pub(crate) struct BodyState {
    pub account: usize,
    pub attachments: Vec<Attachment>,
    pub key: RowKey,
    pub message: Option<Message>,
    /// Set once mark-read was sent, or the user marked read or unread by hand, so the delay does not act again.
    pub read_sent: bool,
    pub saved: Option<String>,
    pub shown_at: f64,
}

pub(crate) struct HistoryLine {
    pub at: i64,
    pub text: String,
}

pub(crate) struct Account {
    pub activity: Option<Activity>,
    pub error: Option<String>,
    pub folders: Vec<FolderRow>,
    pub name: String,
    pub notify: bool,
    /// Edits of sent commands per row, oldest first; each `ActionDone` removes the oldest of its uids.
    pub pending: HashMap<RowKey, Vec<Optimistic>>,
    pub queued: usize,
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
    Act(Action),
    ApproveRule(String),
    Collapse,
    Escape,
    Expand,
    ListViewport { offset: f32, height: f32 },
    MoveCursor(isize),
    MoveFilter(String),
    MoveTo(String),
    NextFocus,
    OpenMovePicker,
    OpenRulesFile,
    RejectRule(String),
    SaveAttachment(usize),
    SearchFocused,
    SearchFor(String),
    SelectRow(usize),
    SelectView(View),
    SetRuleEnabled(String, bool),
    SetTheme(egui::ThemePreference),
    StartSearch,
    StepFolder(isize),
    SyncNow,
    ToggleFlag,
    ToggleHistory,
    ToggleMark,
    ToggleRead,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Focus {
    Body,
    Folders,
    List,
}

pub struct App {
    pub(crate) accounts: Vec<Account>,
    pub(crate) body: Option<BodyState>,
    pub(crate) config_changed: bool,
    pub(crate) config_mtime: Option<SystemTime>,
    pub(crate) downloads: PathBuf,
    pub(crate) engine: Option<Engine>,
    pub(crate) error: Option<String>,
    pub(crate) events: Receiver<Event>,
    pub(crate) focus: Focus,
    pub(crate) focus_search: bool,
    pub(crate) history: VecDeque<HistoryLine>,
    pub(crate) history_open: bool,
    pub(crate) last_poll: f64,
    pub(crate) list: ListState,
    pub(crate) move_picker: Option<String>,
    pub(crate) notifier: fn(&str, &str),
    pub(crate) paths: Paths,
    pub(crate) requested: HashSet<(usize, RowKey)>,
    pub(crate) rules: RulesState,
    pub(crate) rules_mtime: Option<SystemTime>,
    pub(crate) search: Option<String>,
    pub(crate) theme: egui::ThemePreference,
    pub(crate) theme_applied: bool,
    pub(crate) view: View,
    pub(crate) view_dirty: bool,
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
                    error: None,
                    folders: Vec::new(),
                    name: name.clone(),
                    notify: config.account(name).is_none_or(|a| a.notify),
                    pending: HashMap::new(),
                    queued: 0,
                    state: state.clone(),
                    store,
                };
                account.reload_folders();
                account
            })
            .collect();
        let rules_path = paths.rules_file();
        let rules = RulesState::load(&rules_path, Vec::new());
        let (config_mtime, rules_mtime) = (mtime(&paths.config_file()), mtime(&rules_path));
        let mut app = App {
            accounts,
            body: None,
            config_changed: false,
            config_mtime,
            downloads: downloads_dir(),
            engine: Some(engine),
            error: None,
            events,
            focus: Focus::List,
            focus_search: false,
            history: VecDeque::new(),
            history_open: false,
            last_poll: 0.0,
            list: ListState::default(),
            move_picker: None,
            notifier: crate::notify::new_mail,
            paths,
            requested: HashSet::new(),
            rules,
            rules_mtime,
            search: None,
            theme: preference(config.ui.theme),
            theme_applied: false,
            view: View::Folder {
                account: 0,
                folder: "INBOX".into(),
            },
            view_dirty: false,
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
        if std::mem::take(&mut self.view_dirty) {
            self.reload_view();
        }
        let now = ui.input(|input| input.time);
        self.poll_files(now);
        for action in self.keys(&ctx) {
            self.apply(&ctx, action);
        }
        // egui moves widget focus on Tab and arrows itself, and Space or Enter would then click the focused row.
        if !ctx.text_edit_focused() {
            ctx.memory_mut(|memory| {
                if let Some(id) = memory.focused() {
                    memory.surrender_focus(id);
                }
            });
        }
        self.sync_body(now);
        self.mark_read_after_delay(now, &ctx);
        let mut actions = Vec::new();
        if self.config_changed {
            egui::Panel::top("banner").show(ui, |ui| {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "config.toml changed — restart Postbode to apply",
                );
            });
        }
        egui::Panel::bottom("status").show(ui, |ui| actions.extend(status::show(self, ui)));
        egui::Panel::left("folders")
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| actions.extend(folders::show(self, ui)));
        match &self.view {
            View::Folder { .. } => {
                egui::Panel::left("list")
                    .resizable(true)
                    .default_size(480.0)
                    .show(ui, |ui| actions.extend(list::show(self, ui)));
                egui::CentralPanel::default().show(ui, |ui| actions.extend(body::show(self, ui)));
            }
            View::Rules => {
                egui::CentralPanel::default()
                    .show(ui, |ui| actions.extend(rules::show_rules(self, ui)));
            }
            View::Activity | View::Trash => {
                egui::CentralPanel::default().show(ui, |_ui| {});
            }
        }
        actions.extend(list::show_move_picker(self, &ctx));
        for action in actions {
            self.apply(&ctx, action);
        }
        ctx.request_repaint_after(Duration::from_secs_f64(POLL));
    }

    fn handle(&mut self, event: Event) {
        let name = match &event {
            Event::Activity { account, .. }
            | Event::ActionDone { account, .. }
            | Event::BodyReady { account, .. }
            | Event::Error { account, .. }
            | Event::NewMail { account, .. }
            | Event::Restored { account, .. }
            | Event::Synced { account, .. } => account,
        };
        let Some(index) = self.accounts.iter().position(|a| &a.name == name) else {
            return;
        };
        match event {
            Event::Activity { activity, .. } => {
                // Progress lives on the status line; ticks would evict errors from the history.
                if !matches!(
                    activity,
                    Activity::FetchingBodies { .. } | Activity::FetchingHeaders { .. }
                ) {
                    let line = format!(
                        "{}: {}",
                        self.accounts[index].name,
                        status::activity_text(&activity)
                    );
                    self.push_history(line);
                }
                let account = &mut self.accounts[index];
                // Offline follows the Error that caused it and must not wipe it.
                if !matches!(activity, Activity::Offline { .. }) {
                    account.error = None;
                }
                account.activity = Some(activity);
            }
            Event::Error { message, .. } => self.note_error(Some(index), message),
            Event::NewMail { from, subject, .. } => {
                if self.accounts[index].notify {
                    (self.notifier)(&from, &subject);
                }
            }
            Event::Synced { .. } => self.refresh(index),
            Event::ActionDone {
                folder, results, ..
            } => {
                let account = &mut self.accounts[index];
                account.queued = account.queued.saturating_sub(1);
                for (uid, _) in &results {
                    let key = (folder.clone(), *uid);
                    if let Some(edits) = account.pending.get_mut(&key) {
                        edits.remove(0);
                        if edits.is_empty() {
                            account.pending.remove(&key);
                        }
                    }
                }
                let failed: Vec<&String> = results
                    .iter()
                    .filter_map(|(_, result)| result.as_ref().err())
                    .collect();
                if let Some(first) = failed.first() {
                    let line = format!(
                        "{} of {} messages failed: {first}",
                        failed.len(),
                        results.len()
                    );
                    self.note_error(Some(index), line);
                }
                self.refresh(index);
            }
            Event::BodyReady { folder, uid, .. } => self.reload_body(index, (folder, uid)),
            Event::Restored { .. } => {}
        }
    }

    /// The account's mail changed: reload its unread counts, and the shown rows at the start of the next frame.
    pub(crate) fn refresh(&mut self, index: usize) {
        self.accounts[index].reload_folders();
        if self.view_account() == Some(index) {
            self.view_dirty = true;
        }
    }

    /// Records an error in the history and on the account's status line, or on the app's own line.
    pub(crate) fn note_error(&mut self, account: Option<usize>, message: String) {
        let message = clean(&message, false);
        let line = match account {
            Some(index) => {
                // A body fetch that failed is retried the next time its message is shown.
                self.requested.retain(|(account, _)| *account != index);
                self.accounts[index].error = Some(message.clone());
                format!("{}: {message}", self.accounts[index].name)
            }
            None => {
                self.error = Some(message.clone());
                message
            }
        };
        self.push_history(line);
    }

    fn push_history(&mut self, text: String) {
        if self.history.back().is_some_and(|last| last.text == text) {
            return;
        }
        self.history.push_back(HistoryLine {
            at: sync::now(),
            text,
        });
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
    }

    fn apply(&mut self, ctx: &egui::Context, action: UiAction) {
        match action {
            UiAction::Act(action) => self.act(action),
            UiAction::ApproveRule(name) => {
                self.edit_rules(|path| rule_file::edit::approve(path, &name));
            }
            UiAction::Collapse => self.collapse(),
            UiAction::Escape => {
                if self.move_picker.is_some() {
                    self.move_picker = None;
                } else if self.history_open {
                    self.history_open = false;
                } else if self.search.is_some() {
                    self.search = None;
                    self.list = ListState::default();
                    self.reload_view();
                } else {
                    self.list.marked.clear();
                }
            }
            UiAction::Expand => self.expand(),
            UiAction::ListViewport { offset, height } => {
                self.list.viewport = (offset, height);
                self.list.follow_cursor = false;
            }
            UiAction::MoveCursor(delta) => {
                self.list.cursor = step(self.list.cursor, delta, self.list.rows.len());
                self.list.follow_cursor = true;
            }
            UiAction::MoveFilter(filter) => self.move_picker = Some(filter),
            UiAction::MoveTo(folder) => {
                self.move_picker = None;
                self.act(Action::Move(folder));
            }
            UiAction::NextFocus => {
                self.focus = match self.focus {
                    Focus::Body => Focus::Folders,
                    Focus::Folders => Focus::List,
                    Focus::List => Focus::Body,
                }
            }
            UiAction::OpenMovePicker => {
                if let Some(account) = self.view_account()
                    && !self.list.rows.is_empty()
                    && self.ensure_can_act(account)
                {
                    self.move_picker = Some(String::new());
                }
            }
            UiAction::OpenRulesFile => self.open_rules_file(),
            UiAction::RejectRule(name) => {
                self.edit_rules(|path| rule_file::edit::reject(path, &name));
            }
            UiAction::SaveAttachment(index) => self.save_attachment(index),
            UiAction::SearchFocused => self.focus_search = false,
            UiAction::SearchFor(query) => {
                self.search = Some(query);
                self.list.cursor = 0;
                self.reload_view();
            }
            UiAction::SelectRow(index) => {
                self.list.cursor = index;
                self.focus = Focus::List;
            }
            UiAction::SelectView(view) => self.select_view(view),
            UiAction::SetRuleEnabled(name, enabled) => {
                self.edit_rules(|path| rule_file::edit::set_enabled(path, &name, enabled));
            }
            UiAction::SetTheme(preference) => {
                ctx.set_theme(preference);
                self.theme = preference;
                let path = self.paths.config_file();
                match config::save_theme(&path, theme_of(preference)) {
                    Ok(()) => self.config_mtime = mtime(&path),
                    Err(e) => self.note_error(None, format!("could not save the theme: {e}")),
                }
            }
            UiAction::StartSearch => {
                if self.view_account().is_some() {
                    self.search = Some(String::new());
                    self.focus_search = true;
                    self.list = ListState::default();
                    self.reload_view();
                }
            }
            UiAction::StepFolder(delta) => {
                let entries = self.tree_entries();
                let at = entries.iter().position(|v| *v == self.view).unwrap_or(0);
                let view = entries[step(at, delta, entries.len())].clone();
                if view != self.view {
                    self.select_view(view);
                }
            }
            UiAction::SyncNow => self.sync_all(),
            UiAction::ToggleFlag => {
                if let Some(flagged) = self.first_target().map(|row| row.flagged) {
                    self.act(if flagged {
                        Action::Unflag
                    } else {
                        Action::Flag
                    });
                }
            }
            UiAction::ToggleHistory => {
                self.history_open = !self.history_open;
                self.error = None;
            }
            UiAction::ToggleMark => {
                if let Some(key) = self.list.rows.get(self.list.cursor).map(list::Row::key)
                    && !self.list.marked.remove(&key)
                {
                    self.list.marked.insert(key);
                }
            }
            UiAction::ToggleRead => {
                if let Some(unread) = self.first_target().map(|row| row.unread) {
                    self.act(if unread {
                        Action::MarkRead
                    } else {
                        Action::MarkUnread
                    });
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
            if input.consume_key(egui::Modifiers::COMMAND, egui::Key::R) {
                actions.push(UiAction::SyncNow);
            }
            if typing {
                return;
            }
            if input.consume_key(none, egui::Key::Tab) {
                actions.push(UiAction::NextFocus);
            }
            if typed(input, "/") {
                actions.push(UiAction::StartSearch);
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
                    if typed(input, "e") {
                        actions.push(UiAction::Act(Action::Archive));
                    }
                    if typed(input, "#") || input.consume_key(none, egui::Key::Delete) {
                        actions.push(UiAction::Act(Action::Trash));
                    }
                    if typed(input, "m") {
                        actions.push(UiAction::OpenMovePicker);
                    }
                    if typed(input, "s") {
                        actions.push(UiAction::ToggleFlag);
                    }
                    if typed(input, "u") {
                        actions.push(UiAction::ToggleRead);
                    }
                }
            }
        });
        actions
    }

    pub(crate) fn select_view(&mut self, view: View) {
        self.view = view;
        self.move_picker = None;
        self.search = None;
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
            self.list.hits.clear();
            self.rebuild_rows();
            return;
        };
        if let Some(query) = &self.search {
            self.list.hits = match store.search(query, None, list::SEARCH_LIMIT) {
                Ok(found) => found.iter().map(Row::from_message).collect(),
                Err(e) => {
                    log::warn!("[{}] search failed: {e}", self.accounts[account].name);
                    Vec::new()
                }
            };
            self.rebuild_rows();
            return;
        }
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

    /// Loads the cursor's message when the cursor moved to another one.
    fn sync_body(&mut self, now: f64) {
        let current = self
            .view_account()
            .zip(self.list.rows.get(self.list.cursor).map(Row::key));
        if current == self.body.as_ref().map(|b| (b.account, b.key.clone())) {
            return;
        }
        self.body = current.map(|(account, key)| self.load_body(account, key, now));
    }

    /// The message and attachments as stored now.
    fn read_stored(&self, account: usize, key: &RowKey) -> (Option<Message>, Vec<Attachment>) {
        match &self.accounts[account].store {
            Ok(store) => (
                store.message(&key.0, key.1).ok().flatten(),
                store
                    .raw(&key.0, key.1)
                    .ok()
                    .flatten()
                    .map(|raw| message::attachments(&raw))
                    .unwrap_or_default(),
            ),
            Err(_) => (None, Vec::new()),
        }
    }

    /// Refreshes the shown message after its body arrived, keeping the read timer and the saved line.
    fn reload_body(&mut self, account: usize, key: RowKey) {
        if self
            .body
            .as_ref()
            .is_none_or(|b| b.account != account || b.key != key)
        {
            return;
        }
        let (message, attachments) = self.read_stored(account, &key);
        if let Some(body) = &mut self.body {
            body.message = message;
            body.attachments = attachments;
        }
    }

    /// The stored message and its attachments; asks the sync thread for a missing body once per message.
    fn load_body(&mut self, account: usize, key: RowKey, now: f64) -> BodyState {
        let (stored, attachments) = self.read_stored(account, &key);
        let missing = stored.as_ref().is_some_and(|m| m.body_text.is_none());
        if missing
            && self.accounts[account].state == StartState::Running
            && self.requested.insert((account, key.clone()))
        {
            self.send(
                account,
                Command::FetchBody {
                    folder: key.0.clone(),
                    uid: key.1,
                },
            );
        }
        BodyState {
            account,
            attachments,
            key,
            message: stored,
            read_sent: false,
            saved: None,
            shown_at: now,
        }
    }

    /// Marks the shown message read once it has been on screen for `READ_DELAY`.
    fn mark_read_after_delay(&mut self, now: f64, ctx: &egui::Context) {
        let Some(body) = &self.body else { return };
        let account = &self.accounts[body.account];
        let last_edit = account.pending.get(&body.key).and_then(|edits| {
            edits.iter().rev().find_map(|edit| match edit {
                Optimistic::Seen(seen) => Some(*seen),
                _ => None,
            })
        });
        let seen = last_edit.unwrap_or_else(|| body.message.as_ref().is_none_or(Message::is_seen));
        if seen || body.read_sent || account.state != StartState::Running {
            return;
        }
        let waited = now - body.shown_at;
        if waited < READ_DELAY {
            ctx.request_repaint_after(Duration::from_secs_f64(READ_DELAY - waited));
            return;
        }
        let (index, (folder, uid)) = (body.account, body.key.clone());
        if let Some(body) = &mut self.body {
            body.read_sent = true;
        }
        self.send_apply(
            index,
            folder,
            vec![uid],
            Action::MarkRead,
            Optimistic::Seen(true),
        );
        self.rebuild_rows();
    }

    fn save_attachment(&mut self, index: usize) {
        let Some(body) = &self.body else { return };
        let raw = match &self.accounts[body.account].store {
            Ok(store) => store.raw(&body.key.0, body.key.1).ok().flatten(),
            Err(_) => None,
        };
        let saved = match raw.map(|raw| message::save_attachment(&raw, index, &self.downloads)) {
            Some(Ok(path)) => format!("Saved to {}", path.display()),
            Some(Err(e)) => format!("Could not save: {e}"),
            None => "Could not save: the message is not downloaded".into(),
        };
        if let Some(body) = &mut self.body {
            body.saved = Some(saved);
        }
    }

    /// Rows from the cached threads, without a store query; keeps the cursor in range.
    pub(crate) fn rebuild_rows(&mut self) {
        let View::Folder { account, folder } = &self.view else {
            return;
        };
        let mut rows = if self.search.is_some() {
            self.list.hits.clone()
        } else {
            list::build_rows(folder, &self.list.threads, &self.list.expanded)
        };
        list::apply_pending(&mut rows, &self.accounts[*account].pending);
        self.list.rows = rows;
        self.list.cursor = self.list.cursor.min(self.list.rows.len().saturating_sub(1));
    }

    /// Sends `action` for the marked rows, or the cursor row, and shows its effect before the server confirms it.
    fn act(&mut self, action: Action) {
        let Some(account) = self.view_account() else {
            return;
        };
        let Some(optimistic) = Optimistic::of(&action) else {
            return;
        };
        let targets = self.targets(account);
        if targets.is_empty() || !self.ensure_can_act(account) {
            return;
        }
        if matches!(action, Action::MarkRead | Action::MarkUnread)
            && let Some(body) = &mut self.body
            && body.account == account
            && targets
                .iter()
                .any(|(folder, uids)| *folder == body.key.0 && uids.contains(&body.key.1))
        {
            body.read_sent = true;
        }
        for (folder, uids) in targets {
            self.send_apply(account, folder, uids, action.clone(), optimistic);
        }
        self.list.marked.clear();
        self.rebuild_rows();
    }

    /// Sends one `Apply` and records its optimistic edit, which the matching `ActionDone` clears.
    pub(crate) fn send_apply(
        &mut self,
        account: usize,
        folder: String,
        uids: Vec<u32>,
        action: Action,
        optimistic: Optimistic,
    ) {
        let keys: Vec<RowKey> = uids.iter().map(|&uid| (folder.clone(), uid)).collect();
        if self.send(
            account,
            Command::Apply {
                folder,
                uids,
                action,
            },
        ) {
            let state = &mut self.accounts[account];
            state.queued += 1;
            for key in keys {
                state.pending.entry(key).or_default().push(optimistic);
            }
        } else {
            self.note_error(
                Some(account),
                "the sync thread has stopped; restart Postbode".into(),
            );
        }
    }

    pub(crate) fn send(&self, account: usize, command: Command) -> bool {
        self.engine
            .as_ref()
            .is_some_and(|engine| engine.send(&self.accounts[account].name, command))
    }

    /// False, with the reason on the status line, when another process owns the account's connection.
    pub(crate) fn ensure_can_act(&mut self, account: usize) -> bool {
        let reason = match &self.accounts[account].state {
            StartState::Running => return true,
            StartState::Failed(reason) => {
                format!("could not start: {reason}; actions are off here")
            }
            StartState::Locked { .. } => {
                "another Postbode process syncs this account; actions are off here".into()
            }
        };
        self.note_error(Some(account), reason);
        false
    }

    /// Notices edits to rules.toml and config.toml made outside the app.
    fn poll_files(&mut self, now: f64) {
        if now - self.last_poll < POLL {
            return;
        }
        self.last_poll = now;
        if mtime(&self.paths.rules_file()) != self.rules_mtime {
            self.rules_changed();
        }
        if mtime(&self.paths.config_file()) != self.config_mtime {
            self.config_changed = true;
        }
    }

    /// Rules act on mail synced after they change, so every running account syncs at once.
    fn rules_changed(&mut self) {
        let path = self.paths.rules_file();
        self.rules_mtime = mtime(&path);
        self.rules = RulesState::load(&path, std::mem::take(&mut self.rules.rules));
        self.sync_all();
    }

    fn edit_rules(&mut self, edit: impl FnOnce(&Path) -> Result<(), RulesError>) {
        match edit(&self.paths.rules_file()) {
            Ok(()) => self.rules_changed(),
            Err(e) => self.note_error(None, e.to_string()),
        }
    }

    fn open_rules_file(&mut self) {
        let path = self.paths.rules_file();
        let opened = (|| -> std::io::Result<()> {
            if !path.exists() {
                crate::paths::write_atomic(&path, b"")?;
            }
            let opener = if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            let mut child = std::process::Command::new(opener).arg(&path).spawn()?;
            std::thread::spawn(move || child.wait());
            Ok(())
        })();
        if let Err(e) = opened {
            self.note_error(None, format!("could not open rules.toml: {e}"));
        }
    }

    pub(crate) fn sync_all(&mut self) {
        for index in 0..self.accounts.len() {
            if self.accounts[index].state == StartState::Running {
                self.send(index, Command::SyncNow);
            }
        }
    }

    fn first_target(&self) -> Option<&Row> {
        if self.list.marked.is_empty() {
            self.list.rows.get(self.list.cursor)
        } else {
            self.list
                .rows
                .iter()
                .find(|row| self.list.marked.contains(&row.key()))
        }
    }

    /// The uids each action covers, per folder: the marked rows or the cursor row, a thread row standing for every
    /// message of its thread in that folder.
    fn targets(&self, account: usize) -> Vec<(String, Vec<u32>)> {
        let rows: Vec<&Row> = if self.list.marked.is_empty() {
            self.list.rows.get(self.list.cursor).into_iter().collect()
        } else {
            self.list
                .rows
                .iter()
                .filter(|row| self.list.marked.contains(&row.key()))
                .collect()
        };
        let mut by_folder: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
        for row in rows {
            let uids = by_folder.entry(row.folder.clone()).or_default();
            let members = match (&row.thread_id, &self.accounts[account].store) {
                (Some(thread), Ok(store)) if row.count > 1 => {
                    store.thread_members(&row.folder, thread).ok()
                }
                _ => None,
            };
            match members {
                Some(members) => uids.extend(members.iter().map(|member| member.uid)),
                None => {
                    uids.insert(row.uid);
                }
            }
        }
        by_folder
            .into_iter()
            .map(|(folder, uids)| (folder, uids.into_iter().collect()))
            .collect()
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

/// Where Save puts attachments: the Downloads folder, else home.
fn downloads_dir() -> PathBuf {
    directories::UserDirs::new()
        .map(|dirs| dirs.download_dir().unwrap_or(dirs.home_dir()).to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
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

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

pub(crate) fn preference(theme: Theme) -> egui::ThemePreference {
    match theme {
        Theme::Dark => egui::ThemePreference::Dark,
        Theme::Light => egui::ThemePreference::Light,
        Theme::System => egui::ThemePreference::System,
    }
}

pub(crate) fn theme_of(preference: egui::ThemePreference) -> Theme {
    match preference {
        egui::ThemePreference::Dark => Theme::Dark,
        egui::ThemePreference::Light => Theme::Light,
        egui::ThemePreference::System => Theme::System,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::engine::StartState;
    use crate::gui::test_support::{Fixture, message};
    use crate::rules::Action;
    use crate::sync::{Command, Event};

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

    fn inbox(fx: &Fixture, uids: &[u32]) {
        for &uid in uids {
            fx.add("work", message("INBOX", uid, &format!("subject {uid}")));
        }
    }

    fn press(harness: &mut egui_kittest::Harness<'_, App>, text: &str) {
        harness.event(egui::Event::Text(text.into()));
        harness.run();
    }

    fn apply(uids: &[u32], action: Action) -> (String, Command) {
        let command = Command::Apply {
            folder: "INBOX".into(),
            uids: uids.to_vec(),
            action,
        };
        ("work".into(), command)
    }

    fn uids(harness: &egui_kittest::Harness<'_, App>) -> Vec<u32> {
        harness.state().list.rows.iter().map(|r| r.uid).collect()
    }

    #[test]
    fn archive_sends_the_cursor_row_hides_it_and_selects_the_next() {
        let fx = Fixture::new(&["work"]);
        inbox(&fx, &[1, 2, 3]);
        let (mut harness, wires) = fx.harness();
        press(&mut harness, "e");
        assert_eq!(wires.sent(), [apply(&[3], Action::Archive)]);
        assert_eq!(uids(&harness), [2, 1]);
        assert_eq!(
            harness.state().list.rows[harness.state().list.cursor].uid,
            2
        );
        assert!(harness.query_by_label_contains("· 1 queued").is_some());
    }

    #[test]
    fn archive_on_a_thread_row_sends_every_member() {
        let fx = Fixture::new(&["work"]);
        for uid in [4, 5] {
            let mut m = message("INBOX", uid, "thread");
            m.thread_id = "<t@example.com>".into();
            fx.add("work", m);
        }
        let (mut harness, wires) = fx.harness();
        press(&mut harness, "e");
        assert_eq!(wires.sent(), [apply(&[4, 5], Action::Archive)]);
    }

    #[test]
    fn marked_rows_are_deleted_together() {
        let fx = Fixture::new(&["work"]);
        inbox(&fx, &[1, 2, 3]);
        let (mut harness, wires) = fx.harness();
        for key in ["x", "j", "x", "#"] {
            press(&mut harness, key);
        }
        assert_eq!(wires.sent(), [apply(&[2, 3], Action::Trash)]);
        assert_eq!(uids(&harness), [1]);
        assert!(harness.state().list.marked.is_empty());
    }

    #[test]
    fn a_failed_action_puts_the_row_back_and_says_why() {
        let fx = Fixture::new(&["work"]);
        inbox(&fx, &[1, 2, 3]);
        let (mut harness, wires) = fx.harness();
        press(&mut harness, "e");
        let results = vec![(3, Err("NO [SERVERBUG] try later".to_string()))];
        wires
            .events
            .send(Event::ActionDone {
                account: "work".into(),
                folder: "INBOX".into(),
                results,
            })
            .unwrap();
        harness.run();
        assert_eq!(uids(&harness), [3, 2, 1]);
        assert!(
            harness
                .query_by_label("work: 1 of 1 messages failed: NO [SERVERBUG] try later")
                .is_some()
        );
        assert_eq!(harness.state().accounts[0].queued, 0);
    }

    #[test]
    fn a_sync_before_the_action_finishes_keeps_the_row_hidden() {
        let fx = Fixture::new(&["work"]);
        inbox(&fx, &[1, 2, 3]);
        let (mut harness, wires) = fx.harness();
        press(&mut harness, "e");
        wires
            .events
            .send(Event::Synced {
                account: "work".into(),
                new_messages: 0,
                actions: 0,
            })
            .unwrap();
        harness.run();
        assert_eq!(uids(&harness), [2, 1]);
        fx.store("work").remove_message("INBOX", 3).unwrap();
        let results = vec![(3, Ok(1))];
        wires
            .events
            .send(Event::ActionDone {
                account: "work".into(),
                folder: "INBOX".into(),
                results,
            })
            .unwrap();
        harness.run();
        assert_eq!(uids(&harness), [2, 1]);
    }

    #[test]
    fn acting_on_an_empty_folder_sends_nothing() {
        let fx = Fixture::new(&["work"]);
        inbox(&fx, &[1]);
        let (mut harness, wires) = fx.harness();
        press(&mut harness, "e");
        assert_eq!(wires.sent().len(), 1);
        for key in ["e", "#", "u", "s", "x", "j"] {
            press(&mut harness, key);
        }
        assert!(wires.sent().is_empty());
        assert_eq!(harness.state().list.cursor, 0);
    }

    #[test]
    fn a_locked_account_ignores_action_keys_and_says_why() {
        let fx = Fixture::new(&["work"]);
        inbox(&fx, &[1]);
        let (mut harness, wires) = fx.harness();
        harness.state_mut().accounts[0].state = StartState::Locked { pid: Some(42) };
        press(&mut harness, "e");
        assert!(wires.sent().is_empty());
        assert_eq!(uids(&harness), [1]);
        assert!(harness.query_by_label_contains("actions are off").is_some());
    }

    #[test]
    fn a_second_edit_on_a_row_stacks_and_the_first_done_keeps_it() {
        let fx = Fixture::new(&["work"]);
        let mut unread = message("INBOX", 1, "hello");
        unread.flags = String::new();
        fx.add("work", unread);
        let (mut harness, wires) = fx.harness();
        press(&mut harness, "u");
        press(&mut harness, "s");
        let row = &harness.state().list.rows[0];
        assert!(!row.unread && row.flagged);
        wires
            .events
            .send(Event::ActionDone {
                account: "work".into(),
                folder: "INBOX".into(),
                results: vec![(1, Ok(1))],
            })
            .unwrap();
        harness.run();
        assert!(harness.state().list.rows[0].flagged);
        assert_eq!(harness.state().accounts[0].queued, 1);
    }

    #[test]
    fn a_failed_account_says_why_actions_are_off() {
        let fx = Fixture::new(&["work"]);
        inbox(&fx, &[1]);
        let (mut harness, wires) = fx.harness();
        harness.state_mut().accounts[0].state = StartState::Failed("no keyring".into());
        press(&mut harness, "e");
        assert!(wires.sent().is_empty());
        assert!(
            harness
                .query_by_label_contains("could not start: no keyring; actions are off here")
                .is_some()
        );
    }

    #[test]
    fn switching_views_closes_the_move_picker() {
        let fx = Fixture::new(&["work"]);
        fx.folder("work", "Archive", Some("Archive"));
        inbox(&fx, &[1]);
        let (mut harness, _wires) = fx.harness();
        press(&mut harness, "m");
        assert!(harness.state().move_picker.is_some());
        harness.state_mut().select_view(View::Rules);
        assert!(harness.state().move_picker.is_none());
    }

    #[test]
    fn u_marks_read_and_s_flags() {
        let fx = Fixture::new(&["work"]);
        let mut unread = message("INBOX", 1, "hello");
        unread.flags = String::new();
        fx.add("work", unread);
        let (mut harness, wires) = fx.harness();
        press(&mut harness, "u");
        assert!(!harness.state().list.rows[0].unread);
        press(&mut harness, "s");
        assert_eq!(
            wires.sent(),
            [apply(&[1], Action::MarkRead), apply(&[1], Action::Flag)]
        );
        assert!(harness.state().list.rows[0].flagged);
    }

    #[test]
    fn m_filters_the_folders_and_enter_moves() {
        let fx = Fixture::new(&["work"]);
        fx.folder("work", "Archive", Some("Archive"));
        fx.folder("work", "Receipts", None);
        inbox(&fx, &[1]);
        let (mut harness, wires) = fx.harness();
        press(&mut harness, "m");
        assert!(harness.state().move_picker.is_some());
        press(&mut harness, "rec");
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert_eq!(wires.sent(), [apply(&[1], Action::Move("Receipts".into()))]);
        assert!(harness.state().move_picker.is_none());
    }

    #[test]
    fn command_r_syncs_every_running_account() {
        let fx = Fixture::new(&["home", "work"]);
        let (mut harness, wires) = fx.harness();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::R);
        harness.run();
        let sent = wires.sent();
        assert_eq!(
            sent,
            [
                ("home".to_string(), Command::SyncNow),
                ("work".to_string(), Command::SyncNow)
            ]
        );
    }
}
