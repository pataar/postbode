//! The one process that owns the engine, serving it to the GUI, CLI and MCP over a private Unix socket.
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, anyhow, bail};

use crate::config::Config;
use crate::engine::{self, Connector, Engine, StartState};
use crate::message::clean;
use crate::paths::{self, Paths};
use crate::store::Store;
use crate::sync::{self, Activity, Command, Event, RequestId};
use wire::{AccountStatus, ClientMessage, DaemonMessage, Outcome, Payload, Status};

pub mod client;
pub mod service;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;
pub mod wire;

pub use client::Client;

const CONFIG_POLL: Duration = Duration::from_secs(2);
/// How long exit waits for clients to take their last replies before closing their connections.
const FLUSH_GRACE: Duration = Duration::from_secs(1);
const LOG_LIMIT: u64 = 1_048_576;
const POLL: Duration = Duration::from_millis(200);

pub struct Options {
    /// Exit after this long without a connected client; None never idles out.
    pub idle_exit: Option<Duration>,
    pub connect: Connector,
    /// Prints `[account] …` report lines to stdout, for the foreground `postbode run`.
    pub report: bool,
    /// Sends desktop notifications; tests turn it off.
    pub notify: bool,
}

impl Options {
    pub fn foreground() -> Options {
        Options {
            idle_exit: None,
            connect: engine::imap_connector(),
            report: true,
            notify: true,
        }
    }

    pub fn auto_started(idle: Duration) -> Options {
        Options {
            idle_exit: Some(idle),
            ..Options::foreground()
        }
    }
}

/// Serves until `shutdown`, idle exit, or a fatal error; returns after the engine stopped and the socket file is gone.
pub fn run(paths: &Paths, options: Options) -> anyhow::Result<()> {
    // The private state directory guards the socket between `bind` and its chmod.
    paths::create_private_dir(&paths.state_dir)?;
    let _lock = take_daemon_lock(&paths.daemon_lock())?;
    truncate_large_log(&paths.daemon_log())?;
    let socket = paths.daemon_socket();
    let listener = bind_private(&socket)?;
    let served = serve(paths, options, listener);
    let _ = fs::remove_file(&socket);
    served
}

/// The `[account] …` line for an event, as `postbode run` and `sync` print it.
pub fn report(event: &Event) {
    match event {
        Event::NewMail {
            account,
            from,
            subject,
            ..
        } => println!(
            "[{account}] new mail from {}: {}",
            clean(from, false),
            clean(subject, false)
        ),
        Event::Synced {
            account,
            new_messages,
            actions,
            ..
        } => println!("[{account}] synced: {new_messages} new, {actions} rule actions"),
        Event::CommandFailed {
            account, message, ..
        }
        | Event::Error { account, message } => {
            eprintln!("[{account}] error: {}", clean(message, false))
        }
        Event::Activity { account, activity } => log::debug!("[{account}] {activity:?}"),
        Event::ActionDone { .. }
        | Event::BodiesFetched { .. }
        | Event::BodyReady { .. }
        | Event::Restored { .. }
        | Event::RuleApplied { .. } => {}
    }
}

fn truncate_large_log(log: &Path) -> io::Result<()> {
    match fs::metadata(log) {
        Ok(meta) if meta.len() > LOG_LIMIT => File::options().write(true).open(log)?.set_len(0),
        _ => Ok(()),
    }
}

fn take_daemon_lock(path: &Path) -> anyhow::Result<File> {
    match engine::lock_pid_file(path)? {
        Ok(file) => Ok(file),
        Err(pid) => bail!(
            "already running (pid {})",
            pid.map_or_else(|| "unknown".to_string(), |pid| pid.to_string())
        ),
    }
}

/// Replaces a leftover socket file and binds a fresh one only we can open.
fn bind_private(socket: &Path) -> anyhow::Result<UnixListener> {
    match fs::remove_file(socket) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            return Err(e).with_context(|| format!("removing {}", socket.display()));
        }
        _ => {}
    }
    let listener = UnixListener::bind(socket).map_err(|e| match e.kind() {
        io::ErrorKind::InvalidInput => anyhow!("socket path too long: {}", socket.display()),
        _ => anyhow::Error::new(e).context(format!("binding {}", socket.display())),
    })?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

type ClientId = u64;

struct ClientConn {
    out: Sender<String>,
    subscribed: bool,
    writer: JoinHandle<()>,
}

struct Hub {
    activity: HashMap<String, Activity>,
    clients: HashMap<ClientId, ClientConn>,
    last_client_left: Instant,
    next_request: RequestId,
    /// Accounts whose new mail gets a desktop notification.
    notify: HashSet<String>,
    /// The daemon's request id to the client that sent it and that client's own id.
    pending: HashMap<RequestId, (ClientId, u64)>,
    shutdown: bool,
    started: Instant,
}

impl Hub {
    fn send(&self, client: ClientId, message: &DaemonMessage) {
        let Some(conn) = self.clients.get(&client) else {
            return;
        };
        match wire::line(message) {
            Ok(line) => {
                let _ = conn.out.send(line);
            }
            Err(e) => log::error!("could not encode a message: {e}"),
        }
    }

    fn reply(&self, client: ClientId, id: u64, outcome: Outcome) {
        self.send(client, &DaemonMessage::Reply { id, outcome });
    }

    fn broadcast(&self, event: &Event) {
        let line = match wire::line(&DaemonMessage::Event(event.clone())) {
            Ok(line) => line,
            Err(e) => return log::error!("could not encode an event: {e}"),
        };
        for conn in self.clients.values().filter(|conn| conn.subscribed) {
            let _ = conn.out.send(line.clone());
        }
    }
}

/// Readers reach the engine through its own mutex, so `apply_config` joining stopped threads never stalls the hub.
struct Shared {
    engine: Mutex<Option<Engine>>,
    hub: Mutex<Hub>,
}

/// A panic elsewhere must not take the daemon down with a poisoned lock.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn notifying(config: &Config) -> HashSet<String> {
    config
        .accounts
        .iter()
        .filter(|account| account.notify)
        .map(|account| account.name.clone())
        .collect()
}

fn serve(paths: &Paths, options: Options, listener: UnixListener) -> anyhow::Result<()> {
    let mut watch = ConfigWatch::new(paths.config_file());
    let config = Config::load(&paths.config_file())?;
    migrate_stores(&config, paths);
    let (engine, events) = Engine::start_with(&config, paths, options.connect.clone());
    let started = Instant::now();
    let shared = Arc::new(Shared {
        engine: Mutex::new(Some(engine)),
        hub: Mutex::new(Hub {
            activity: HashMap::new(),
            clients: HashMap::new(),
            last_client_left: started,
            next_request: 0,
            notify: notifying(&config),
            pending: HashMap::new(),
            shutdown: false,
            started,
        }),
    });
    let router = std::thread::Builder::new()
        .name("daemon-events".into())
        .spawn({
            let shared = shared.clone();
            move || route_events(&shared, events, options.report, options.notify)
        })?;
    let connections = accept_until_done(&shared, &listener, &mut watch, options.idle_exit);
    stop(&shared, router, connections);
    Ok(())
}

/// Clients open the stores as soon as the daemon answers, so every migration has to be done before it does; a store
/// that fails to open fails again in its account's thread, which reports it.
fn migrate_stores(config: &Config, paths: &Paths) {
    for account in &config.accounts {
        if let Err(e) = Store::open(&paths.mail_db(&account.name)) {
            log::warn!("[{}] could not open the store: {e}", account.name);
        }
    }
}

struct ConfigWatch {
    path: PathBuf,
    modified: Option<SystemTime>,
    checked: Instant,
}

impl ConfigWatch {
    fn new(path: PathBuf) -> ConfigWatch {
        ConfigWatch {
            modified: modified_time(&path),
            path,
            checked: Instant::now(),
        }
    }

    /// The config, loaded again once its modification time changed; looked at every 2 s.
    fn changed(&mut self) -> Option<Config> {
        if self.checked.elapsed() < CONFIG_POLL {
            return None;
        }
        self.checked = Instant::now();
        let modified = modified_time(&self.path);
        if modified == self.modified {
            return None;
        }
        self.modified = modified;
        Config::load(&self.path)
            .inspect_err(|e| log::error!("{e}; keeping the running accounts"))
            .ok()
    }
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

struct Connection {
    stream: UnixStream,
    reader: JoinHandle<()>,
}

fn accept_until_done(
    shared: &Arc<Shared>,
    listener: &UnixListener,
    watch: &mut ConfigWatch,
    idle_exit: Option<Duration>,
) -> Vec<Connection> {
    let mut connections: Vec<Connection> = Vec::new();
    let mut next_client: ClientId = 0;
    while !done(shared, idle_exit) {
        if let Some(config) = watch.changed() {
            apply_config(shared, &config);
        }
        match listener.accept() {
            Ok((stream, _)) => {
                next_client += 1;
                match open_connection(shared, stream, next_client) {
                    Ok(connection) => connections.push(connection),
                    Err(e) => log::warn!("could not serve a client: {e}"),
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(POLL),
            Err(e) => {
                log::warn!("accept failed: {e}");
                std::thread::sleep(POLL);
            }
        }
        connections.retain(|connection| !connection.reader.is_finished());
    }
    connections
}

fn done(shared: &Shared, idle_exit: Option<Duration>) -> bool {
    let hub = lock(&shared.hub);
    hub.shutdown
        || idle_exit
            .is_some_and(|idle| hub.clients.is_empty() && hub.last_client_left.elapsed() >= idle)
}

fn apply_config(shared: &Shared, config: &Config) {
    if let Some(engine) = lock(&shared.engine).as_mut() {
        engine.apply_config(config);
    }
    lock(&shared.hub).notify = notifying(config);
}

/// Stops the engine, delivers the replies its threads still send, then closes every connection.
fn stop(shared: &Shared, router: JoinHandle<()>, connections: Vec<Connection>) {
    lock(&shared.hub).shutdown = true;
    let engine = lock(&shared.engine).take();
    drop(engine);
    let _ = router.join();
    let clients: Vec<ClientConn> = lock(&shared.hub)
        .clients
        .drain()
        .map(|(_, conn)| conn)
        .collect();
    let writers: Vec<JoinHandle<()>> = clients.into_iter().map(|conn| conn.writer).collect();
    let deadline = Instant::now() + FLUSH_GRACE;
    while writers.iter().any(|writer| !writer.is_finished()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    for connection in &connections {
        let _ = connection.stream.shutdown(Shutdown::Both);
    }
    writers.into_iter().for_each(|writer| drop(writer.join()));
    for connection in connections {
        let _ = connection.reader.join();
    }
}

fn route_events(shared: &Shared, events: Receiver<Event>, report_events: bool, notify: bool) {
    for event in events {
        if report_events {
            report(&event);
        }
        if notify {
            notify_new_mail(shared, &event);
        }
        let mut hub = lock(&shared.hub);
        if let Event::Activity { account, activity } = &event {
            hub.activity.insert(account.clone(), activity.clone());
        }
        for request in event.request_ids() {
            if let Some((client, id)) = hub.pending.remove(&request) {
                hub.reply(client, id, outcome_of(&event));
            }
        }
        hub.broadcast(&event);
    }
}

fn notify_new_mail(shared: &Shared, event: &Event) {
    let Event::NewMail {
        account,
        from,
        subject,
        ..
    } = event
    else {
        return;
    };
    if lock(&shared.hub).notify.contains(account) {
        crate::notify::new_mail(from, subject);
    }
}

fn outcome_of(event: &Event) -> Outcome {
    match event {
        Event::CommandFailed { message, .. } => Outcome::Error(message.clone()),
        _ => Outcome::Ok(Payload::Event(event.clone())),
    }
}

fn open_connection(
    shared: &Arc<Shared>,
    stream: UnixStream,
    client: ClientId,
) -> io::Result<Connection> {
    // macOS hands out accepted sockets with the listener's non-blocking flag.
    stream.set_nonblocking(false)?;
    let kept = stream.try_clone()?;
    let shared = shared.clone();
    let reader = std::thread::Builder::new()
        .name(format!("daemon-client-{client}"))
        .spawn(move || serve_client(&shared, client, stream))?;
    Ok(Connection {
        stream: kept,
        reader,
    })
}

fn serve_client(shared: &Shared, client: ClientId, stream: UnixStream) {
    if let Err(e) = converse(shared, client, &stream) {
        log::debug!("client {client}: {e}");
    }
    if let Some(conn) = unregister(shared, client) {
        drop(conn.out);
        let _ = conn.writer.join();
    }
    let _ = stream.shutdown(Shutdown::Both);
}

fn converse(shared: &Shared, client: ClientId, stream: &UnixStream) -> io::Result<()> {
    let mut lines = BufReader::new(stream).lines();
    let Some(first) = lines.next().transpose()? else {
        return Ok(());
    };
    if !matches!(
        serde_json::from_str(&first),
        Ok(ClientMessage::Hello { .. })
    ) {
        return Ok(());
    }
    if !register(shared, client, stream.try_clone()?)? {
        return Ok(());
    }
    for line in lines {
        if !handle_line(shared, client, &line?) {
            break;
        }
    }
    Ok(())
}

/// Adds the client with its writer thread and greets it; false once the daemon is stopping.
fn register(shared: &Shared, client: ClientId, stream: UnixStream) -> io::Result<bool> {
    let (out, lines) = mpsc::channel();
    let writer = std::thread::Builder::new()
        .name(format!("daemon-writer-{client}"))
        .spawn(move || write_lines(stream, lines))?;
    let mut hub = lock(&shared.hub);
    if hub.shutdown {
        return Ok(false);
    }
    hub.clients.insert(
        client,
        ClientConn {
            out,
            subscribed: false,
            writer,
        },
    );
    let hello = DaemonMessage::Hello {
        protocol: wire::PROTOCOL,
        version: wire::VERSION.into(),
        pid: std::process::id(),
    };
    hub.send(client, &hello);
    Ok(true)
}

fn write_lines(mut stream: UnixStream, lines: Receiver<String>) {
    for line in lines {
        if stream.write_all(line.as_bytes()).is_err() {
            return;
        }
    }
}

fn unregister(shared: &Shared, client: ClientId) -> Option<ClientConn> {
    let mut hub = lock(&shared.hub);
    let conn = hub.clients.remove(&client)?;
    hub.pending.retain(|_, (owner, _)| *owner != client);
    if hub.clients.is_empty() {
        hub.last_client_left = Instant::now();
    }
    Some(conn)
}

/// Answers one line; false when the connection should close.
fn handle_line(shared: &Shared, client: ClientId, line: &str) -> bool {
    let message = match serde_json::from_str::<ClientMessage>(line) {
        Ok(message) => message,
        Err(e) => {
            let Some(id) = id_in(line) else {
                return false;
            };
            lock(&shared.hub).reply(client, id, Outcome::Error(format!("bad message: {e}")));
            return true;
        }
    };
    match message {
        ClientMessage::Hello { .. } => {}
        ClientMessage::Subscribe { id } => {
            let mut hub = lock(&shared.hub);
            if let Some(conn) = hub.clients.get_mut(&client) {
                conn.subscribed = true;
            }
            hub.reply(client, id, Outcome::Ok(Payload::Done));
        }
        ClientMessage::Status { id } => {
            let status = status(shared);
            lock(&shared.hub).reply(client, id, Outcome::Ok(Payload::Status(status)));
        }
        ClientMessage::Shutdown { id } => {
            let mut hub = lock(&shared.hub);
            hub.reply(client, id, Outcome::Ok(Payload::Done));
            hub.shutdown = true;
        }
        ClientMessage::Command {
            id,
            account,
            command,
        } => submit(shared, client, id, &account, command),
    }
    true
}

/// The `id` of a message that did not parse, at its top level or inside its one variant.
fn id_in(line: &str) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let variant = value.as_object()?.values().next();
    value.get("id").or_else(|| variant?.get("id"))?.as_u64()
}

fn status(shared: &Shared) -> Status {
    let accounts = lock(&shared.engine)
        .as_ref()
        .map(Engine::accounts)
        .unwrap_or_default();
    let hub = lock(&shared.hub);
    Status {
        pid: std::process::id(),
        version: wire::VERSION.into(),
        uptime_secs: hub.started.elapsed().as_secs(),
        clients: hub.clients.len(),
        accounts: accounts
            .into_iter()
            .map(|(name, state)| AccountStatus {
                activity: hub.activity.get(&name).cloned(),
                name,
                state,
            })
            .collect(),
    }
}

/// Refuses the command at once when its account cannot run it, and otherwise hands it to the engine.
fn submit(shared: &Shared, client: ClientId, id: u64, account: &str, command: Command) {
    let state = lock(&shared.engine).as_ref().and_then(|engine| {
        engine
            .accounts()
            .into_iter()
            .find_map(|(name, state)| (name == account).then_some(state))
    });
    let request = {
        let mut hub = lock(&shared.hub);
        if let Some(refusal) = refusal(account, state, hub.activity.get(account)) {
            hub.reply(client, id, Outcome::Error(refusal));
            return;
        }
        hub.next_request += 1;
        let request = hub.next_request;
        hub.pending.insert(request, (client, id));
        request
    };
    let sent = lock(&shared.engine)
        .as_ref()
        .is_some_and(|engine| engine.send(account, request, command));
    if !sent {
        let mut hub = lock(&shared.hub);
        if let Some((client, id)) = hub.pending.remove(&request) {
            hub.reply(
                client,
                id,
                Outcome::Error(format!("{account} is not running")),
            );
        }
    }
}

/// Why a command for `account` cannot run now, if it cannot.
fn refusal(
    account: &str,
    state: Option<StartState>,
    activity: Option<&Activity>,
) -> Option<String> {
    match (state, activity) {
        (None, _) => Some(format!("no account named '{account}'")),
        (Some(StartState::Failed(reason)), _) => Some(reason),
        (Some(StartState::Running), Some(Activity::Offline { reason, retry_at })) => {
            Some(sync::offline_message(account, reason, *retry_at))
        }
        (Some(StartState::Running), _) => None,
    }
}
