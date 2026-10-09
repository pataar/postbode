//! App state, the frame loop, and the only code that changes state: `handle` for events, `apply` for UI actions.
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, SystemTime};

use eframe::egui;

use crate::config::{self, Config, Theme};
use crate::daemon::Client;
use crate::daemon::client::NewerDaemon;
use crate::daemon::wire::AccountStatus;
use crate::message::{self, Attachment, HtmlBody, clean};
use crate::paths::Paths;
use crate::rules::{self as rule_file, Action, RulesError};
use crate::store::{LogEntry, Message, Store, StoreError};
use crate::sync::{Activity, Command, Event};
use crate::update;

use super::body;
use super::folders;
use super::html::{self, HtmlState, Renderer, Reply};
use super::list::{self, ListState, Optimistic, Row, RowKey, THREAD_LIMIT};
use super::rules::{self, RulesState, TrashRow};
use super::status;
use super::theme;
use super::toolbar;

/// The status line while the daemon is gone.
pub(crate) const DAEMON_LOST: &str = "background sync stopped — reconnecting";

/// How `NewerDaemon` reads on the status line, which a reconnect clears.
const NEWER_DAEMON: &str = "the daemon is version ";

/// Lines kept for the history window.
const HISTORY: usize = 50;

/// Seconds between checks of rules.toml and config.toml for outside edits.
const POLL: f64 = 2.0;

/// How long a message stays on screen before it is marked read.
const READ_DELAY: f64 = 1.0;

/// Seconds between attempts to reach the daemon again.
const RECONNECT: f64 = 5.0;

/// A daemon client, its event subscription, and each account's state and activity when it connected.
pub(crate) type Connection = (Client, Receiver<Event>, Vec<AccountStatus>);

/// Called after each daemon event and once the daemon hung up, so an idle window still shows them.
pub(crate) type Waker = Box<dyn Fn() + Send>;

/// Connects to the daemon, starting it when none answers.
pub(crate) fn connect(paths: &Paths, wake: Waker) -> anyhow::Result<Connection> {
    session(Client::connect_or_start(paths)?, wake)
}

/// Subscribes before asking for the states, so no event between the two is lost.
pub(crate) fn session(client: Client, wake: Waker) -> anyhow::Result<Connection> {
    let events = client.subscribe_waking(wake)?;
    let states = client.status()?.accounts;
    Ok((client, events, states))
}

pub(crate) struct BodyState {
    pub account: usize,
    /// Set when the user chose this message, not when a view change or reload put it on screen; only then does it
    /// become read.
    pub armed: bool,
    pub attachments: Vec<Attachment>,
    /// The message's HTML, when it has some and this build renders it.
    pub html: Option<HtmlBody>,
    /// The HTML view, once the panel asked for a layout; `None` while the text view shows.
    pub html_view: Option<HtmlState>,
    pub key: RowKey,
    pub message: Option<Message>,
    /// Set once mark-read was sent, or the user marked read or unread by hand, so the delay does not act again.
    pub read_sent: bool,
    pub saved: Option<String>,
    pub shown_at: f64,
    /// Set by `v`: the text view for this message even though it has HTML.
    pub show_text: bool,
    /// The raw message, while the source window shows it.
    pub source: Option<body::Source>,
    /// Why the HTML could not be shown; the text view shows instead, under this note.
    pub text_note: Option<&'static str>,
    /// The body text cleaned once on load, since the panel draws it every frame.
    pub text: Option<String>,
}

impl BodyState {
    /// True when the panel shows the HTML view rather than the text.
    pub fn shows_html(&self) -> bool {
        self.html.is_some() && !self.show_text && self.text_note.is_none()
    }
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
    /// Edits of sent commands per row, oldest first; each `ActionDone` removes the oldest of its uids.
    pub pending: HashMap<RowKey, Vec<Optimistic>>,
    pub queued: usize,
    pub store: Result<Store, String>,
}

impl Account {
    /// True while the sync thread works, for the spinner beside the account.
    pub fn busy(&self) -> bool {
        !matches!(
            self.activity,
            None | Some(
                Activity::Idle { .. } | Activity::NotRunning { .. } | Activity::Offline { .. }
            )
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
                delimiter: f.delimiter,
                special_use: f.special_use,
                unread: store.unread_count(&f.name)?,
                name: f.name,
            })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FolderRow {
    pub delimiter: Option<String>,
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
    CloseSource,
    Escape,
    Expand,
    /// The pointer over the HTML page, in page points, or `None` once it left.
    HtmlHover(Option<egui::Pos2>),
    /// The part of the HTML page on screen, in page points.
    HtmlVisible {
        top: f32,
        bottom: f32,
    },
    /// The width (points) and scale the HTML view would lay out at now.
    HtmlWidth {
        scale: f32,
        width: f32,
    },
    ListViewport {
        offset: f32,
        height: f32,
    },
    MoveCursor(isize),
    MoveFilter(String),
    MoveTo(String),
    NextFocus,
    OpenMovePicker,
    OpenRulesFile,
    OpenSource,
    RejectRule(String),
    Restore(usize, PathBuf),
    SaveAttachment(usize),
    SaveEml,
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
    ToggleFolder(usize, String),
    ToggleHelp,
    ToggleHistory,
    ToggleHtml,
    ToggleMark,
    ToggleRead,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Focus {
    Body,
    Folders,
    List,
}

/// The release check: off in config, waiting for the first frame, running on its thread, or done with the version to
/// announce.
pub(crate) enum UpdateCheck {
    Off,
    Waiting,
    Running(Receiver<Option<String>>),
    Done(Option<String>),
}

pub struct App {
    pub(crate) accounts: Vec<Account>,
    pub(crate) activity_log: Vec<(String, LogEntry)>,
    pub(crate) body: Option<BodyState>,
    pub(crate) client: Client,
    /// Folder tree branches folded shut, by account index and path; kept for the window's lifetime.
    pub(crate) collapsed: HashSet<(usize, String)>,
    pub(crate) config_changed: bool,
    pub(crate) config_mtime: Option<SystemTime>,
    /// Set when the event stream ended; cleared once a reconnect succeeds.
    pub(crate) daemon_lost: bool,
    pub(crate) downloads: PathBuf,
    pub(crate) error: Option<String>,
    pub(crate) events: Receiver<Event>,
    pub(crate) fetch_release: fn() -> anyhow::Result<String>,
    pub(crate) focus: Focus,
    pub(crate) focus_search: bool,
    pub(crate) help_open: bool,
    /// The app icon drawn in the toolbar, loaded on the first frame.
    pub(crate) logo: Option<egui::TextureHandle>,
    /// Numbers HTML layouts, so a reply for an older one is dropped.
    pub(crate) html_generation: u64,
    pub(crate) history: VecDeque<HistoryLine>,
    pub(crate) history_open: bool,
    pub(crate) last_poll: f64,
    pub(crate) last_reconnect: f64,
    pub(crate) list: ListState,
    pub(crate) move_picker: Option<String>,
    pub(crate) paths: Paths,
    /// Set by the user's own navigation; the next body `sync_body` loads is armed.
    pub(crate) pending_arm: bool,
    /// The HTML render thread, started for the first HTML message.
    pub(crate) renderer: Option<Renderer>,
    pub(crate) reconnect: fn(&Paths, Waker) -> anyhow::Result<Connection>,
    /// The result of the reconnect attempt in flight; at most one runs.
    pub(crate) reconnecting: Option<Receiver<anyhow::Result<Connection>>>,
    pub(crate) requested: HashSet<(usize, RowKey)>,
    pub(crate) rules: RulesState,
    pub(crate) rules_mtime: Option<SystemTime>,
    pub(crate) search: Option<String>,
    pub(crate) theme: egui::ThemePreference,
    pub(crate) theme_applied: bool,
    pub(crate) trash: Vec<TrashRow>,
    pub(crate) update: UpdateCheck,
    pub(crate) view: View,
    pub(crate) view_dirty: bool,
}

impl App {
    pub fn new(
        config: &Config,
        paths: Paths,
        client: Client,
        events: Receiver<Event>,
        states: Vec<AccountStatus>,
    ) -> App {
        let accounts = states
            .into_iter()
            .map(|AccountStatus { activity, name }| {
                let store = Store::open_account(&paths, &name)
                    .map_err(|e| format!("could not open the store: {e}"));
                let mut account = Account {
                    activity,
                    error: None,
                    folders: Vec::new(),
                    name,
                    pending: HashMap::new(),
                    queued: 0,
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
            activity_log: Vec::new(),
            body: None,
            client,
            collapsed: HashSet::new(),
            config_changed: false,
            config_mtime,
            daemon_lost: false,
            downloads: downloads_dir(),
            error: None,
            events,
            fetch_release: update::fetch,
            focus: Focus::List,
            focus_search: false,
            help_open: false,
            logo: None,
            html_generation: 0,
            history: VecDeque::new(),
            history_open: false,
            last_poll: 0.0,
            last_reconnect: 0.0,
            list: ListState::default(),
            move_picker: None,
            paths,
            pending_arm: false,
            reconnect: connect,
            renderer: None,
            reconnecting: None,
            requested: HashSet::new(),
            rules,
            rules_mtime,
            search: None,
            theme: preference(config.ui.theme),
            theme_applied: false,
            trash: Vec::new(),
            update: if config.ui.check_updates {
                UpdateCheck::Waiting
            } else {
                UpdateCheck::Off
            },
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
            theme::install(&ctx);
            ctx.set_theme(self.theme);
            self.theme_applied = true;
            // Fonts load at the start of the next pass, and drawing in the serif family before then panics.
            ctx.request_repaint();
            return;
        }
        let now = ui.input(|input| input.time);
        self.receive(now);
        self.receive_html(&ctx);
        self.keep_daemon(&ctx, now);
        self.check_release(&ctx);
        if std::mem::take(&mut self.view_dirty) {
            self.reload_view();
        }
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
        let typing_search =
            self.focus_search || ctx.memory(|memory| memory.has_focus(list::search_id()));
        self.sync_body(now, typing_search);
        self.mark_read_after_delay(now, &ctx);
        let mut actions = Vec::new();
        if self.logo.is_none()
            && let Ok(icon) = eframe::icon_data::from_png_bytes(super::ICON)
        {
            let size = [icon.width as usize, icon.height as usize];
            let image = egui::ColorImage::from_rgba_unmultiplied(size, &icon.rgba);
            self.logo = Some(ctx.load_texture("logo", image, egui::TextureOptions::LINEAR));
        }
        let bar = theme::chrome_frame(ui);
        // The toolbar's and status bar's first and last items are icon buttons on one side, whose glyphs sit a
        // padding inside them, so that side gets one padding less to line the glyphs up with the panes' content.
        egui::Panel::top("toolbar")
            .frame(bar.inner_margin(margin(theme::INSET, theme::PAD, 2.0)))
            .exact_size(theme::TOP_BAR)
            .show(ui, |ui| {
                theme::chrome(ui);
                actions.extend(toolbar::show(self, ui));
            });
        if self.config_changed {
            egui::Panel::top("banner").show(ui, |ui| {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "config.toml changed — reopen the window to show it",
                );
            });
        }
        egui::Panel::bottom("status")
            .frame(bar.inner_margin(margin(theme::PAD, theme::INSET, theme::PAD)))
            .show(ui, |ui| {
                theme::chrome(ui);
                actions.extend(status::show(self, ui));
            });
        let chrome = theme::chrome_frame(ui);
        egui::Panel::left(FOLDERS)
            .frame(theme::pane(
                chrome.inner_margin(theme::PAD),
                ui,
                self.focus == Focus::Folders,
            ))
            .resizable(true)
            .default_size(FOLDERS_WIDTH)
            .show(ui, |ui| {
                theme::chrome(ui);
                actions.extend(folders::show(self, ui));
            });
        // The panes sit in the frame like mail in a letterbox: rounded where they meet it.
        let frame_around_panes = chrome.inner_margin(egui::Margin {
            right: theme::PAD as i8,
            ..egui::Margin::ZERO
        });
        egui::CentralPanel::default()
            .frame(frame_around_panes)
            .show(ui, |ui| self.show_panes(ui, &mut actions));
        actions.extend(list::show_move_picker(self, &ctx));
        actions.extend(body::show_source(self, &ctx));
        actions.extend(status::show_help(self, &ctx));
        for action in actions {
            self.apply(&ctx, action);
        }
        ctx.request_repaint_after(Duration::from_secs_f64(POLL));
    }

    fn receive(&mut self, now: f64) {
        while !self.daemon_lost {
            match self.events.try_recv() {
                Ok(event) => self.handle(event, now),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.daemon_lost = true;
                    self.last_reconnect = now;
                    self.note_error(None, DAEMON_LOST.into());
                }
            }
        }
    }

    /// While the daemon is gone: takes the result of the attempt in flight, or starts one every 5 s.
    fn keep_daemon(&mut self, ctx: &egui::Context, now: f64) {
        if !self.daemon_lost {
            return;
        }
        let Some(attempt) = &self.reconnecting else {
            if now - self.last_reconnect >= RECONNECT {
                self.last_reconnect = now;
                self.reconnecting = Some(self.start_reconnect(ctx));
            }
            return;
        };
        match attempt.try_recv() {
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.reconnecting = None,
            Ok(Err(e)) => {
                log::warn!("could not reach the daemon: {e:#}");
                self.reconnecting = None;
                if let Some(newer) = e.downcast_ref::<NewerDaemon>() {
                    let text = newer.to_string();
                    if self.error.as_deref() != Some(text.as_str()) {
                        self.note_error(None, text);
                    }
                }
            }
            Ok(Ok(connection)) => {
                self.reconnecting = None;
                self.reconnected(connection);
            }
        }
    }

    /// Connects on a background thread so the frame never waits; a repaint follows the result.
    fn start_reconnect(&self, ctx: &egui::Context) -> Receiver<anyhow::Result<Connection>> {
        let (done, attempt) = mpsc::channel();
        let (reconnect, paths, ctx) = (self.reconnect, self.paths.clone(), ctx.clone());
        std::thread::spawn(move || {
            let waker = ctx.clone();
            let connection = reconnect(&paths, Box::new(move || waker.request_repaint()));
            let _ = done.send(connection);
            ctx.request_repaint();
        });
        attempt
    }

    /// Starts the release check at the first frame, after the test fixture has swapped `fetch_release`, then takes its
    /// answer.
    fn check_release(&mut self, ctx: &egui::Context) {
        let next = match &self.update {
            UpdateCheck::Waiting => {
                let (done, answer) = mpsc::channel();
                let (cache, fetch, ctx) =
                    (self.paths.update_check(), self.fetch_release, ctx.clone());
                std::thread::spawn(move || {
                    let _ = done.send(update::check(
                        &cache,
                        crate::time::now(),
                        env!("CARGO_PKG_VERSION"),
                        fetch,
                    ));
                    ctx.request_repaint();
                });
                Some(UpdateCheck::Running(answer))
            }
            UpdateCheck::Running(answer) => match answer.try_recv() {
                Ok(newer) => Some(UpdateCheck::Done(newer)),
                Err(TryRecvError::Disconnected) => Some(UpdateCheck::Done(None)),
                Err(TryRecvError::Empty) => None,
            },
            UpdateCheck::Off | UpdateCheck::Done(_) => None,
        };
        if let Some(next) = next {
            self.update = next;
        }
    }

    /// The newer release to announce, once the check has found one.
    pub(crate) fn newer_release(&self) -> Option<&str> {
        match &self.update {
            UpdateCheck::Done(newer) => newer.as_deref(),
            _ => None,
        }
    }

    /// Commands sent before the loss may never be answered, so their edits go and the view reloads from the store, which
    /// the daemon may have written meanwhile.
    fn reconnected(&mut self, (client, events, states): Connection) {
        self.client = client;
        self.events = events;
        self.daemon_lost = false;
        if self
            .error
            .as_deref()
            .is_some_and(|error| error == DAEMON_LOST || error.starts_with(NEWER_DAEMON))
        {
            self.error = None;
        }
        self.requested.clear();
        for account in &mut self.accounts {
            if let Some(status) = states.iter().find(|status| status.name == account.name) {
                account.activity = status.activity.clone();
            }
            if account.error.as_deref() == Some(DAEMON_LOST) {
                account.error = None;
            }
            account.pending.clear();
            account.queued = 0;
            account.reload_folders();
        }
        self.push_history("background sync reconnected".into());
        self.view_dirty = true;
    }

    fn handle(&mut self, event: Event, now: f64) {
        let name = match &event {
            Event::Activity { account, .. }
            | Event::ActionDone { account, .. }
            | Event::BodiesFetched { account, .. }
            | Event::BodyReady { account, .. }
            | Event::CommandFailed { account, .. }
            | Event::Error { account, .. }
            | Event::NewMail { account, .. }
            | Event::Restored { account, .. }
            | Event::RuleApplied { account, .. }
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
                    let line = format!("{}: {activity}", self.accounts[index].name);
                    self.push_history(line);
                }
                let account = &mut self.accounts[index];
                // Offline follows the Error that caused it and must not wipe it.
                if !matches!(activity, Activity::Offline { .. }) {
                    account.error = None;
                }
                account.activity = Some(activity);
            }
            Event::NewMail { .. } => {}
            // Request 0 is this window's own outcome. A daemon id is another client's, or the broadcast copy of our own
            // failure; the client drops the copy of our own `ActionDone`.
            Event::CommandFailed { request, .. } if request != 0 => {}
            Event::ActionDone { request, .. } if request != 0 => self.refresh(index),
            Event::CommandFailed { message, .. } => {
                // A refusal carries no uids, so every edit of the account goes and the store shows what is true.
                let account = &mut self.accounts[index];
                account.pending.clear();
                account.queued = 0;
                self.note_error(Some(index), message);
                self.refresh(index);
            }
            Event::Error { message, .. } => self.note_error(Some(index), message),
            Event::BodiesFetched { .. } | Event::RuleApplied { .. } | Event::Synced { .. } => {
                self.refresh(index)
            }
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
            Event::BodyReady { folder, uid, .. } => self.reload_body(index, (folder, uid), now),
            Event::Restored { folder, .. } => {
                let line = format!(
                    "{}: restored to {}",
                    self.accounts[index].name,
                    clean(&folder, false)
                );
                self.push_history(line);
                self.refresh(index);
            }
        }
    }

    /// The account's mail changed: reload its unread counts, and the shown rows at the start of the next frame.
    pub(crate) fn refresh(&mut self, index: usize) {
        self.accounts[index].reload_folders();
        if self.view_account() == Some(index) || matches!(self.view, View::Activity | View::Trash) {
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
            at: crate::time::now(),
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
                let accounts: Vec<String> = self.accounts.iter().map(|a| a.name.clone()).collect();
                let paths = self.paths.clone();
                self.edit_rules(|_| {
                    rule_file::edit::approve_from_now(&paths, &accounts, &name, crate::time::now())
                });
            }
            UiAction::CloseSource => {
                if let Some(body) = &mut self.body {
                    body.source = None;
                }
            }
            UiAction::Collapse => self.collapse(),
            UiAction::HtmlHover(at) => self.html_hover(at),
            UiAction::HtmlVisible { top, bottom } => self.html_visible(top, bottom),
            UiAction::HtmlWidth { scale, width } => {
                let now = ctx.input(|input| input.time);
                self.html_width(ctx, scale, width, now);
            }
            UiAction::Escape => {
                if self.move_picker.is_some() {
                    self.move_picker = None;
                } else if let Some(body) = self.body.as_mut().filter(|b| b.source.is_some()) {
                    body.source = None;
                } else if self.help_open {
                    self.help_open = false;
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
                self.pending_arm = true;
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
                if self.view_account().is_some() && !self.list.rows.is_empty() {
                    self.move_picker = Some(String::new());
                }
            }
            UiAction::OpenSource => self.open_source(),
            UiAction::OpenRulesFile => self.open_rules_file(),
            UiAction::RejectRule(name) => {
                self.edit_rules(|path| rule_file::edit::reject(path, &name));
            }
            UiAction::Restore(account, file) => {
                if !self.send(account, Command::Restore { file }) {
                    self.note_error(Some(account), DAEMON_LOST.into());
                }
            }
            UiAction::SaveAttachment(index) => self.save_attachment(index),
            UiAction::SaveEml => self.save_eml(),
            UiAction::SearchFocused => self.focus_search = false,
            UiAction::SearchFor(query) => {
                self.search = Some(query);
                self.list.cursor = 0;
                self.list.rows.clear();
                self.reload_view();
            }
            UiAction::SelectRow(index) => {
                self.list.cursor = index;
                self.focus = Focus::List;
                self.pending_arm = true;
                self.arm_shown_body(index);
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
                let entries = self.tree_entries(true);
                let at = entries
                    .iter()
                    .position(|v| *v == self.view)
                    .or_else(|| self.nearest_shown_above(&entries))
                    .unwrap_or(0);
                let view = entries[step(at, delta, entries.len())].clone();
                if view != self.view {
                    self.select_view(view);
                }
            }
            UiAction::SyncNow => self.sync_all(),
            UiAction::ToggleFolder(account, path) => {
                if !self.collapsed.remove(&(account, path.clone())) {
                    self.collapsed.insert((account, path));
                }
            }
            UiAction::ToggleFlag => {
                if let Some(flagged) = self.selected_rows().first().map(|row| row.flagged) {
                    self.act(if flagged {
                        Action::Unflag
                    } else {
                        Action::Flag
                    });
                }
            }
            UiAction::ToggleHelp => self.help_open = !self.help_open,
            UiAction::ToggleHtml => {
                if let Some(body) = self.body.as_mut().filter(|b| b.html.is_some()) {
                    body.show_text = !body.show_text;
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
                if let Some(unread) = self.selected_rows().first().map(|row| row.unread) {
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
            if typed(input, "?") {
                actions.push(UiAction::ToggleHelp);
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
                    let delete = typed(input, "#")
                        || input.consume_key(none, egui::Key::Delete)
                        // macOS keyboards label Backspace "delete".
                        || (cfg!(target_os = "macos")
                            && input.consume_key(none, egui::Key::Backspace));
                    if delete {
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
                    if typed(input, "v") {
                        actions.push(UiAction::ToggleHtml);
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
            match self.view {
                View::Activity => self.activity_log = rules::activity_log(&self.accounts),
                View::Trash => self.trash = rules::trash_rows(&self.accounts, &self.paths),
                _ => {}
            }
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

    /// A click on the row already shown opens it; `shown_at` stays, so a message long on screen is read at once.
    fn arm_shown_body(&mut self, index: usize) {
        let Some(key) = self.list.rows.get(index).map(Row::key) else {
            return;
        };
        let account = self.view_account();
        if let Some(body) = &mut self.body
            && Some(body.account) == account
            && body.key == key
        {
            body.armed = true;
        }
    }

    /// Loads the cursor's message when the cursor moved to another one; nothing while a search query is typed.
    fn sync_body(&mut self, now: f64, typing_search: bool) {
        if typing_search {
            self.body = None;
            return;
        }
        let armed = std::mem::take(&mut self.pending_arm);
        let current = self
            .view_account()
            .zip(self.list.rows.get(self.list.cursor).map(Row::key));
        if current == self.body.as_ref().map(|b| (b.account, b.key.clone())) {
            return;
        }
        self.body = current.map(|(account, key)| self.load_body(account, key, now, armed));
    }

    /// The message, its attachments and its HTML as stored now.
    fn read_stored(&self, account: usize, key: &RowKey) -> Stored {
        let Ok(store) = &self.accounts[account].store else {
            return (None, Vec::new(), None);
        };
        let raw = store.raw(&key.0, key.1).ok().flatten();
        let attachments = raw.as_deref().map(message::attachments).unwrap_or_default();
        let html = raw.as_deref().and_then(html::html_of);
        (
            store.message(&key.0, key.1).ok().flatten(),
            attachments,
            html,
        )
    }

    /// Refreshes the shown message after its body arrived; the read delay restarts when the text first appears.
    fn reload_body(&mut self, account: usize, key: RowKey, now: f64) {
        if self
            .body
            .as_ref()
            .is_none_or(|b| b.account != account || b.key != key)
        {
            return;
        }
        let (message, attachments, html) = self.read_stored(account, &key);
        if let Some(body) = &mut self.body {
            let text = body_text(message.as_ref());
            if body.text.is_none() && text.is_some() {
                body.shown_at = now;
            }
            body.text = text;
            body.message = message;
            body.attachments = attachments;
            if body.html != html {
                body.text_note = too_large(html.as_ref());
                body.html = html;
                body.html_view = None;
            }
        }
    }

    /// The stored message and its attachments; asks the sync thread for a missing body once per message.
    fn load_body(&mut self, account: usize, key: RowKey, now: f64, armed: bool) -> BodyState {
        let (stored, attachments, html) = self.read_stored(account, &key);
        let missing = stored.as_ref().is_some_and(|m| m.body_text.is_none());
        if missing && self.requested.insert((account, key.clone())) {
            self.send(
                account,
                Command::FetchBody {
                    folder: key.0.clone(),
                    uid: key.1,
                },
            );
        }
        let text = body_text(stored.as_ref());
        BodyState {
            account,
            armed,
            attachments,
            text_note: too_large(html.as_ref()),
            html,
            html_view: None,
            key,
            message: stored,
            read_sent: false,
            saved: None,
            show_text: false,
            shown_at: now,
            source: None,
            text,
        }
    }

    /// Marks the shown message read once the user opened it and its text has been on screen for `READ_DELAY`.
    fn mark_read_after_delay(&mut self, now: f64, ctx: &egui::Context) {
        let Some(body) = &mut self.body else { return };
        if !body.armed {
            return;
        }
        // The delay counts from when the text appears (`reload_body`), not from "Loading…".
        if body.message.as_ref().is_some_and(|m| m.body_text.is_none()) {
            return;
        }
        let account = &self.accounts[body.account];
        let last_edit = account.pending.get(&body.key).and_then(|edits| {
            edits.iter().rev().find_map(|edit| match edit {
                Optimistic::Seen(seen) => Some(*seen),
                _ => None,
            })
        });
        let seen = last_edit.unwrap_or_else(|| body.message.as_ref().is_none_or(Message::is_seen));
        if seen || body.read_sent {
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

    /// The shown message's raw bytes from the store, or `None` while it is not downloaded.
    fn shown_raw(&self) -> Option<Vec<u8>> {
        let body = self.body.as_ref()?;
        let store = self.accounts[body.account].store.as_ref().ok()?;
        store.raw(&body.key.0, body.key.1).ok().flatten()
    }

    fn save_attachment(&mut self, index: usize) {
        let raw = self.shown_raw();
        let note =
            saved_note(raw.map(|raw| message::save_attachment(&raw, index, &self.downloads)));
        if let Some(body) = &mut self.body {
            body.saved = Some(note);
        }
    }

    fn save_eml(&mut self) {
        let raw = self.shown_raw();
        let Some(body) = &mut self.body else { return };
        let subject = body.message.as_ref().and_then(|m| m.subject.as_deref());
        body.saved =
            Some(saved_note(raw.map(|raw| {
                message::save_eml(&raw, subject, &self.downloads)
            })));
    }

    fn open_source(&mut self) {
        let raw = self.shown_raw();
        let Some(body) = &mut self.body else { return };
        match raw {
            Some(raw) => body.source = Some(body::Source::new(&raw)),
            None => {
                body.saved = Some("Could not show the source: the message is not downloaded".into())
            }
        }
    }

    /// Takes the render thread's replies for the shown layout; replies for an older one are dropped.
    fn receive_html(&mut self, ctx: &egui::Context) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        for reply in renderer.replies() {
            let Some(body) = self.body.as_mut() else {
                continue;
            };
            let Some(view) = body
                .html_view
                .as_mut()
                .filter(|v| v.generation == reply.generation())
            else {
                continue;
            };
            match reply {
                Reply::Failed { .. } => {
                    body.text_note = Some(html::FAILED);
                    body.html_view = None;
                }
                Reply::Laid {
                    height,
                    remote,
                    width,
                    ..
                } => {
                    if height > html::MAX_HEIGHT {
                        body.text_note = Some(html::TOO_LARGE);
                        body.html_view = None;
                        continue;
                    }
                    view.page = Some(egui::vec2(width, height));
                    view.remote = remote;
                    view.strips.clear();
                    view.requested.clear();
                    view.hover = None;
                    view.hovered_at = None;
                }
                Reply::Link { href, .. } => view.hover = href,
                Reply::Strip {
                    generation,
                    image,
                    strip,
                } => {
                    let name = format!("html-{generation}-{strip}");
                    let texture = ctx.load_texture(name, image, egui::TextureOptions::LINEAR);
                    view.strips.insert(strip, texture);
                }
            }
        }
    }

    /// Starts the HTML layout of the shown message, or a new one once the panel width or scale held still for
    /// `html::SETTLE`; until then the old layout stays on screen.
    fn html_width(&mut self, ctx: &egui::Context, scale: f32, width: f32, now: f64) {
        let Some(body) = self.body.as_mut().filter(|b| b.shows_html()) else {
            return;
        };
        let fresh = match &mut body.html_view {
            None => true,
            Some(view) if view.sent == (scale, width) => {
                view.wanted = None;
                false
            }
            Some(view) => match view.wanted {
                Some((wanted, since)) if wanted == (scale, width) => {
                    if now - since < html::SETTLE {
                        ctx.request_repaint_after(Duration::from_secs_f64(html::SETTLE));
                    }
                    now - since >= html::SETTLE
                }
                _ => {
                    view.wanted = Some(((scale, width), now));
                    ctx.request_repaint_after(Duration::from_secs_f64(html::SETTLE));
                    false
                }
            },
        };
        let Some(source) = body.html.as_ref().filter(|_| fresh) else {
            return;
        };
        self.html_generation += 1;
        let generation = self.html_generation;
        let load = html::Load {
            generation,
            html: source.html.clone(),
            inline: source.inline.clone(),
            scale,
            width,
        };
        match &mut body.html_view {
            Some(view) => {
                view.generation = generation;
                view.sent = (scale, width);
                view.wanted = None;
            }
            None => body.html_view = Some(HtmlState::new(generation, scale, width)),
        }
        self.renderer
            .get_or_insert_with(|| Renderer::start(ctx.clone()))
            .send(html::Request::Load(load));
    }

    /// Asks for the strips around what is on screen and lets go of those far from it.
    fn html_visible(&mut self, top: f32, bottom: f32) {
        let (Some(view), Some(renderer)) = (
            self.body.as_mut().and_then(|b| b.html_view.as_mut()),
            self.renderer.as_mut(),
        ) else {
            return;
        };
        let Some(page) = view.page else { return };
        let scale = view.sent.0;
        for strip in html::wanted_strips(top, bottom, page.y, scale) {
            if view.requested.insert(strip) {
                renderer.send(html::Request::Paint {
                    generation: view.generation,
                    strip,
                });
            }
        }
        for strip in html::far_strips(view.strips.keys().copied(), top, bottom, scale) {
            view.strips.remove(&strip);
            view.requested.remove(&strip);
        }
    }

    /// Asks which link is under the pointer; the answer arrives as `Reply::Link`.
    fn html_hover(&mut self, at: Option<egui::Pos2>) {
        let (Some(view), Some(renderer)) = (
            self.body.as_mut().and_then(|b| b.html_view.as_mut()),
            self.renderer.as_mut(),
        ) else {
            return;
        };
        view.hovered_at = at;
        match at {
            Some(at) => renderer.send(html::Request::Hit {
                generation: view.generation,
                x: at.x,
                y: at.y,
            }),
            None => view.hover = None,
        }
    }

    /// Rows from the cached threads, without a store query. The cursor stays on its message; when that row is gone it
    /// keeps its index, so the next row is selected.
    pub(crate) fn rebuild_rows(&mut self) {
        let View::Folder { account, folder } = &self.view else {
            return;
        };
        let current = self.list.rows.get(self.list.cursor).cloned();
        let mut rows = if self.search.is_some() {
            self.list.hits.clone()
        } else {
            list::build_rows(folder, &self.list.threads, &self.list.expanded)
        };
        list::apply_pending(&mut rows, &self.accounts[*account].pending);
        let kept = current.and_then(|cursor| rows.iter().position(|row| same_row(row, &cursor)));
        self.list.rows = rows;
        self.list.cursor =
            kept.unwrap_or_else(|| self.list.cursor.min(self.list.rows.len().saturating_sub(1)));
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
        if targets.is_empty() {
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
        self.pending_arm |= optimistic == Optimistic::Hidden;
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
                by: "gui".into(),
            },
        ) {
            let state = &mut self.accounts[account];
            state.queued += 1;
            for key in keys {
                state.pending.entry(key).or_default().push(optimistic);
            }
        } else {
            self.note_error(Some(account), DAEMON_LOST.into());
        }
    }

    pub(crate) fn send(&self, account: usize, command: Command) -> bool {
        self.client.send(&self.accounts[account].name, command)
    }

    /// Notices edits to rules.toml and config.toml.
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

    /// Reloads the rules view; the daemon notices the change itself and syncs every account.
    fn rules_changed(&mut self) {
        let path = self.paths.rules_file();
        self.rules_mtime = mtime(&path);
        self.rules = RulesState::load(&path, std::mem::take(&mut self.rules.rules));
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
            // `open -t` uses the default text editor, so a missing `.toml` association still opens one.
            let (opener, flags): (&str, &[&str]) = if cfg!(target_os = "macos") {
                ("open", &["-t"])
            } else {
                ("xdg-open", &[])
            };
            let mut child = std::process::Command::new(opener)
                .args(flags)
                .arg(&path)
                .spawn()?;
            std::thread::spawn(move || match child.wait() {
                Ok(status) if !status.success() => log::warn!("{opener} rules.toml: {status}"),
                Ok(_) => {}
                Err(e) => log::warn!("{opener} rules.toml: {e}"),
            });
            Ok(())
        })();
        if let Err(e) = opened {
            self.note_error(None, format!("could not open rules.toml: {e}"));
        }
    }

    pub(crate) fn sync_all(&mut self) {
        for index in 0..self.accounts.len() {
            self.send(index, Command::SyncNow);
        }
    }

    /// The rows an action covers: the marked rows, or else the cursor row.
    fn selected_rows(&self) -> Vec<&Row> {
        if self.list.marked.is_empty() {
            self.list.rows.get(self.list.cursor).into_iter().collect()
        } else {
            self.list
                .rows
                .iter()
                .filter(|row| self.list.marked.contains(&row.key()))
                .collect()
        }
    }

    /// The uids each action covers, per folder: the marked rows or the cursor row, a thread row standing for every
    /// message of its thread in that folder.
    fn targets(&self, account: usize) -> Vec<(String, Vec<u32>)> {
        let mut by_folder: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
        for row in self.selected_rows() {
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
    /// Where a hidden view (in a folded branch) sits in `shown`: at the nearest shown entry above it.
    fn nearest_shown_above(&self, shown: &[View]) -> Option<usize> {
        let all = self.tree_entries(false);
        let at = all.iter().position(|v| *v == self.view)?;
        all[..at]
            .iter()
            .rev()
            .find_map(|view| shown.iter().position(|v| v == view))
    }

    fn tree_entries(&self, skip_collapsed: bool) -> Vec<View> {
        let mut entries: Vec<View> = self
            .accounts
            .iter()
            .enumerate()
            .flat_map(|(account, a)| {
                folders::shown(a, |path| {
                    skip_collapsed && self.collapsed.contains(&(account, path.to_string()))
                })
                .into_iter()
                .map(move |folder| View::Folder { account, folder })
            })
            .collect();
        entries.extend([View::Rules, View::Activity, View::Trash]);
        entries
    }

    /// The list and body, or the one pane of Rules, Activity and Trash, which stands for both.
    fn show_panes(&self, ui: &mut egui::Ui, actions: &mut Vec<UiAction>) {
        let radius = theme::RADIUS;
        let left = egui::CornerRadius {
            nw: radius,
            sw: radius,
            ..egui::CornerRadius::ZERO
        };
        let right = egui::CornerRadius {
            ne: radius,
            se: radius,
            ..egui::CornerRadius::ZERO
        };
        let only = egui::CornerRadius::same(radius);
        let pane = |ui: &egui::Ui, corners, focused| {
            let frame = body_frame(ui).corner_radius(corners);
            egui::CentralPanel::default().frame(theme::pane(frame, ui, focused))
        };
        let others_focused = self.focus != Focus::Folders;
        match &self.view {
            View::Folder { .. } => {
                let side = egui::Frame::side_top_panel(ui.style())
                    .inner_margin(theme::PAD)
                    .corner_radius(left);
                egui::Panel::left("list")
                    .frame(theme::pane(side, ui, self.focus == Focus::List))
                    .resizable(true)
                    .default_size(480.0)
                    .min_size(list::MIN_WIDTH)
                    .show(ui, |ui| actions.extend(list::show(self, ui)));
                pane(ui, right, self.focus == Focus::Body)
                    .show(ui, |ui| actions.extend(body::show(self, ui)));
            }
            View::Rules => {
                pane(ui, only, others_focused)
                    .show(ui, |ui| actions.extend(rules::show_rules(self, ui)));
            }
            View::Activity => {
                pane(ui, only, others_focused)
                    .show(ui, |ui| actions.extend(rules::show_activity(self, ui)));
            }
            View::Trash => {
                pane(ui, only, others_focused)
                    .show(ui, |ui| actions.extend(rules::show_trash(self, ui)));
            }
        }
    }
}

/// The central panel on the base colour; side panels and the status bar keep the darker panel colour.
pub(crate) fn central_panel(ui: &egui::Ui, focused: bool) -> egui::CentralPanel {
    egui::CentralPanel::default().frame(theme::pane(body_frame(ui), ui, focused))
}

fn body_frame(ui: &egui::Ui) -> egui::Frame {
    egui::Frame::central_panel(ui.style())
        .fill(ui.visuals().window_fill)
        .inner_margin(margin(theme::INSET, theme::INSET, theme::PAD))
}

/// The folder pane's id, which the toolbar reads its width from.
pub(crate) const FOLDERS: &str = "folders";
pub(crate) const FOLDERS_WIDTH: f32 = 220.0;

fn margin(left: f32, right: f32, vertical: f32) -> egui::Margin {
    #[allow(clippy::cast_possible_truncation, reason = "margins are a few points")]
    let (left, right, vertical) = (left as i8, right as i8, vertical as i8);
    egui::Margin {
        left,
        right,
        top: vertical,
        bottom: vertical,
    }
}

/// A stored message, its attachments and its HTML.
type Stored = (Option<Message>, Vec<Attachment>, Option<HtmlBody>);

/// The note for HTML too large to lay out, which then shows as text.
fn too_large(html: Option<&HtmlBody>) -> Option<&'static str> {
    html.filter(|h| h.html.len() > html::MAX_HTML)
        .map(|_| html::TOO_LARGE)
}

/// The line under the attachments after a save: where the file went, or why not.
fn saved_note(result: Option<std::io::Result<PathBuf>>) -> String {
    match result {
        Some(Ok(path)) => format!("Saved to {}", path.display()),
        Some(Err(e)) => format!("Could not save: {e}"),
        None => "Could not save: the message is not downloaded".into(),
    }
}

fn body_text(message: Option<&Message>) -> Option<String> {
    message?.body_text.as_deref().map(|text| clean(text, true))
}

/// A thread row's key is its latest uid, which changes with new mail, so it is matched by thread id. A member and its
/// thread row share a key, so `member` tells them apart.
fn same_row(row: &Row, cursor: &Row) -> bool {
    if row.member != cursor.member {
        return false;
    }
    match (&row.thread_id, &cursor.thread_id) {
        (Some(row_thread), Some(cursor_thread)) if !row.member => row_thread == cursor_thread,
        _ => row.key() == cursor.key(),
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
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::daemon::Client;
    use crate::daemon::wire::AccountStatus;
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
            by: "gui".into(),
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
                request: 0,
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
                requests: vec![],
                errors: vec![],
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
                request: 0,
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
                request: 0,
            })
            .unwrap();
        harness.run();
        assert!(harness.state().list.rows[0].flagged);
        assert_eq!(harness.state().accounts[0].queued, 1);
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

    #[test]
    fn backspace_deletes_on_macos_only() {
        let fx = Fixture::new(&["work"]);
        inbox(&fx, &[1]);
        let (mut harness, wires) = fx.harness();
        harness.key_press(egui::Key::Backspace);
        harness.run();
        let expected = if cfg!(target_os = "macos") {
            vec![apply(&[1], Action::Trash)]
        } else {
            Vec::new()
        };
        assert_eq!(wires.sent(), expected);
    }

    #[test]
    fn closing_the_event_stream_shows_the_reconnect_line() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, wires) = fx.harness();
        drop(wires.events);
        harness.run();
        assert!(harness.state().daemon_lost);
        assert!(
            harness
                .query_by_label("background sync stopped — reconnecting")
                .is_some()
        );
    }

    /// The daemon end of the client a reconnect hands over; one per test binary, as `reconnect` is a plain `fn`.
    static RECONNECTED: Mutex<Option<Receiver<(String, Command)>>> = Mutex::new(None);

    fn idle(name: &str) -> AccountStatus {
        AccountStatus {
            name: name.into(),
            activity: Some(Activity::Idle {
                since: 1_790_000_000,
            }),
        }
    }

    /// Steps frames past the 5 s retry until the reconnect landed.
    fn reconnect(harness: &mut egui_kittest::Harness<'_, App>) {
        harness.input_mut().time = Some(10.0);
        let deadline = Instant::now() + Duration::from_secs(5);
        while harness.state().daemon_lost && Instant::now() < deadline {
            harness.step();
            std::thread::sleep(Duration::from_millis(10));
        }
        harness.run();
        assert!(!harness.state().daemon_lost);
    }

    #[test]
    fn a_reconnect_replaces_the_client_and_refreshes_the_accounts() {
        let fx = Fixture::new(&["work"]);
        inbox(&fx, &[1]);
        let (mut harness, wires) = fx.harness();
        harness.state_mut().reconnect = |_, _| {
            let (client, commands, events) = Client::in_memory(&["work"]);
            *RECONNECTED.lock().unwrap() = Some(commands);
            // A dropped sender would read as another loss.
            std::mem::forget(events);
            let (client, events, _) = session(client, Box::new(|| {}))?;
            Ok((client, events, vec![idle("work")]))
        };
        drop(wires);
        harness.run();
        press(&mut harness, "e");
        assert!(
            harness
                .query_by_label(&format!("work: {DAEMON_LOST}"))
                .is_some()
        );
        let mut unread = message("INBOX", 2, "while away");
        unread.flags = String::new();
        fx.add("work", unread);
        reconnect(&mut harness);
        assert!(harness.query_by_label_contains(DAEMON_LOST).is_none());
        let line = format!("work: up to date · {}", crate::time::clock(1_790_000_000));
        assert!(harness.query_by_label(&line).is_some());
        assert!(harness.query_by_label("Inbox (1)").is_some());
        press(&mut harness, "e");
        let sent: Vec<_> = RECONNECTED
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .try_iter()
            .collect();
        assert_eq!(sent, [apply(&[1], Action::Archive)]);
    }

    #[test]
    fn a_newer_daemon_is_named_on_the_status_line_and_retried() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, wires) = fx.harness();
        harness.state_mut().reconnect = |_, _| {
            Err(crate::daemon::client::NewerDaemon {
                theirs: "9.0.0".into(),
                ours: "0.1.0".into(),
            }
            .into())
        };
        drop(wires);
        harness.run();
        let newer =
            "the daemon is version 9.0.0, newer than this postbode (0.1.0); restart this program";
        let deadline = Instant::now() + Duration::from_secs(5);
        harness.input_mut().time = Some(10.0);
        while harness.query_by_label(newer).is_none() && Instant::now() < deadline {
            harness.step();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(harness.query_by_label(newer).is_some());
        assert!(harness.state().daemon_lost);
        harness.input_mut().time = Some(20.0);
        harness.step();
        assert!(
            harness.state().reconnecting.is_some(),
            "it tries again 5 s later"
        );
    }

    #[test]
    fn the_window_opens_with_the_daemons_activity() {
        let fx = Fixture::new(&["work"]);
        let config = Config::load(&fx.paths.config_file()).unwrap();
        let (client, _commands, _events) = Client::in_memory(&["work"]);
        let events = client.subscribe().unwrap();
        let app = App::new(
            &config,
            fx.paths.clone(),
            client,
            events,
            vec![idle("work")],
        );
        assert_eq!(
            app.accounts[0].activity,
            Some(Activity::Idle {
                since: 1_790_000_000
            })
        );
    }

    #[test]
    fn a_refused_command_shows_its_reason() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, wires) = fx.harness();
        let message = "work is offline (x); retrying at 10:00".to_string();
        wires
            .events
            .send(Event::CommandFailed {
                account: "work".into(),
                request: 0,
                message,
            })
            .unwrap();
        harness.run();
        assert!(
            harness
                .query_by_label("work: work is offline (x); retrying at 10:00")
                .is_some()
        );
    }

    #[test]
    fn another_clients_outcomes_leave_this_windows_edits_alone() {
        let fx = Fixture::new(&["work"]);
        inbox(&fx, &[1, 2, 3]);
        let (mut harness, wires) = fx.harness();
        press(&mut harness, "e");
        let foreign = [
            Event::ActionDone {
                account: "work".into(),
                folder: "INBOX".into(),
                results: vec![(3, Ok(1))],
                request: 7,
            },
            Event::CommandFailed {
                account: "work".into(),
                request: 8,
                message: "no rule named 'x'".into(),
            },
        ];
        for event in foreign {
            wires.events.send(event).unwrap();
        }
        harness.run();
        assert_eq!(uids(&harness), [2, 1]);
        assert_eq!(harness.state().accounts[0].queued, 1);
        assert!(harness.query_by_label_contains("no rule named").is_none());
    }

    #[test]
    fn a_refused_action_puts_the_row_back_and_clears_the_queue() {
        let fx = Fixture::new(&["work"]);
        inbox(&fx, &[1, 2, 3]);
        let (mut harness, wires) = fx.harness();
        press(&mut harness, "e");
        assert_eq!(uids(&harness), [2, 1]);
        wires
            .events
            .send(Event::CommandFailed {
                account: "work".into(),
                request: 0,
                message: "work is offline (x); retrying at 10:00".into(),
            })
            .unwrap();
        harness.run();
        assert_eq!(uids(&harness), [3, 2, 1]);
        assert_eq!(harness.state().accounts[0].queued, 0);
        assert!(harness.query_by_label_contains("queued").is_none());
    }

    /// Steps frames until the background release check has answered.
    fn finish_update_check(harness: &mut egui_kittest::Harness<'_, App>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !matches!(harness.state().update, UpdateCheck::Done(_)) && Instant::now() < deadline {
            harness.step();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(matches!(harness.state().update, UpdateCheck::Done(_)));
    }

    #[test]
    fn a_cached_newer_release_reaches_the_window() {
        let fx = Fixture::new(&["work"]);
        fx.append_config("[ui]\ncheck_updates = true\n");
        let cached = update::Cached {
            checked_at: crate::time::now(),
            latest: Some("999.0.0".into()),
        };
        update::save(&fx.paths.update_check(), &cached).unwrap();
        let (mut harness, _wires) = fx.harness();
        finish_update_check(&mut harness);
        assert_eq!(harness.state().newer_release(), Some("999.0.0"));
    }

    #[test]
    fn a_failed_check_shows_nothing_and_is_remembered() {
        let fx = Fixture::new(&["work"]);
        fx.append_config("[ui]\ncheck_updates = true\n");
        let (mut harness, _wires) = fx.harness();
        finish_update_check(&mut harness);
        assert_eq!(harness.state().newer_release(), None);
        assert!(update::load(&fx.paths.update_check()).is_some());
    }

    #[test]
    fn by_default_no_check_starts() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        harness.run();
        assert!(matches!(harness.state().update, UpdateCheck::Off));
        assert!(!fx.paths.update_check().exists());
    }
}
