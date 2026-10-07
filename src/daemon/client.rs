//! A connection to the daemon: requests with replies, fire-and-forget sends, and the event subscription.
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};

use super::lock;
use super::wire::{
    self, ClientMessage, DaemonMessage, Outcome, PROTOCOL, Payload, Status, VERSION,
};
use crate::paths::{self, Paths};
use crate::store::{Message, Store};
use crate::sync::{Command, Event, RequestId};

pub const NO_REPLY: &str =
    "no reply from the daemon within 120 s; the command may still run, see `postbode log`";
const LOG_TAIL_LINES: usize = 20;
const REPLY_TIMEOUT: Duration = Duration::from_secs(120);
const RETRY: Duration = Duration::from_millis(100);
const START_WAIT: Duration = Duration::from_secs(5);
const STOP_WAIT: Duration = Duration::from_secs(5);

/// The error of a request that lost its daemon; a caller holding the client should reconnect.
#[derive(Debug)]
pub struct Stopped(String);

impl fmt::Display for Stopped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Stopped {}

/// A daemon of a newer postbode than this one: this program has to restart, and the daemon keeps running.
#[derive(Debug)]
pub struct NewerDaemon {
    pub theirs: String,
    pub ours: String,
}

impl fmt::Display for NewerDaemon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the daemon is version {}, newer than this postbode ({}); restart this program",
            self.theirs, self.ours
        )
    }
}

impl std::error::Error for NewerDaemon {}

pub struct Client {
    transport: Transport,
    inbox: Arc<Mutex<Inbox>>,
    next_id: AtomicU64,
    log: PathBuf,
}

enum Transport {
    Socket(Mutex<UnixStream>),
    #[cfg(test)]
    Memory {
        accounts: Vec<String>,
        commands: Sender<(String, Command)>,
        events: Mutex<Option<Receiver<Event>>>,
    },
}

impl Transport {
    /// The daemon connection; None for a test's in-memory client.
    fn socket(&self) -> Option<&Mutex<UnixStream>> {
        match self {
            Transport::Socket(stream) => Some(stream),
            #[cfg(test)]
            Transport::Memory { .. } => None,
        }
    }
}

/// What the reader thread delivers to; `closed` once the daemon hung up.
#[derive(Default)]
struct Inbox {
    closed: bool,
    /// The daemon's ids of our own `ActionDone`s already published, whose broadcast copies are dropped.
    own: HashSet<RequestId>,
    /// The account of each `send` still unanswered, so a refusal can name it.
    sent: HashMap<u64, String>,
    subscriber: Option<Sender<Event>>,
    /// Called after each event the subscriber gets, and once the daemon hung up.
    wake: Option<Box<dyn Fn() + Send>>,
    waiters: HashMap<u64, Sender<Outcome>>,
}

impl Inbox {
    /// A reply to one of our `send`s goes to the subscriber with request 0, which marks it as this client's own. The
    /// daemon broadcasts the same outcome right after; `receive` drops that copy of an `ActionDone`, and a failure's copy
    /// carries the daemon's id.
    fn deliver(&mut self, id: u64, outcome: Outcome) {
        let account = self.sent.remove(&id);
        if let Some(waiter) = self.waiters.remove(&id) {
            let _ = waiter.send(outcome);
            return;
        }
        let Some(account) = account else { return };
        match outcome {
            Outcome::Error(message) => self.publish(Event::CommandFailed {
                account,
                request: 0,
                message,
            }),
            Outcome::Ok(Payload::Event(Event::ActionDone {
                account,
                folder,
                results,
                request,
            })) => {
                self.own.insert(request);
                self.publish(Event::ActionDone {
                    account,
                    folder,
                    results,
                    request: 0,
                });
            }
            Outcome::Ok(_) => {}
        }
    }

    /// A broadcast event, unless it is the copy of our own `ActionDone` that `deliver` already published.
    fn receive(&mut self, event: Event) {
        if let Event::ActionDone { request, .. } = &event
            && self.own.remove(request)
        {
            return;
        }
        self.publish(event);
    }

    fn publish(&mut self, event: Event) {
        let Some(subscriber) = &self.subscriber else {
            return;
        };
        if subscriber.send(event).is_err() {
            self.subscriber = None;
            self.wake = None;
        } else if let Some(wake) = &self.wake {
            wake();
        }
    }

    /// Dropping the senders wakes every waiter with "the daemon stopped" and ends the subscription.
    fn close(&mut self) {
        let wake = self.wake.take();
        *self = Inbox {
            closed: true,
            ..Inbox::default()
        };
        if let Some(wake) = wake {
            wake();
        }
    }
}

pub(super) enum ConnectError {
    /// Nothing answered, or the connection closed before the daemon's hello.
    Down(io::Error),
    /// The daemon speaks another protocol or is another version; `connection` is the one that saw its hello.
    Mismatch {
        reason: String,
        /// The daemon's version, when its hello was understood.
        theirs: Option<String>,
        connection: BufReader<UnixStream>,
    },
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectError::Down(e) => write!(f, "no daemon answered: {e}"),
            ConnectError::Mismatch { reason, .. } => f.write_str(reason),
        }
    }
}

impl From<ConnectError> for anyhow::Error {
    fn from(error: ConnectError) -> anyhow::Error {
        anyhow!("{error}")
    }
}

/// The daemon connection a CLI command or the MCP server makes on first use, and again once its daemon hung up.
pub struct LazyClient {
    paths: Paths,
    /// Shared, so a long request does not hold up other callers.
    slot: Mutex<Option<Arc<Client>>>,
}

impl LazyClient {
    pub fn new(paths: &Paths) -> LazyClient {
        LazyClient {
            paths: paths.clone(),
            slot: Mutex::new(None),
        }
    }

    /// Starts out with a connection the caller made.
    pub fn with_client(paths: &Paths, client: Client) -> LazyClient {
        LazyClient {
            paths: paths.clone(),
            slot: Mutex::new(Some(Arc::new(client))),
        }
    }

    /// The connection, starting the daemon when none answers.
    pub fn client(&self) -> anyhow::Result<Arc<Client>> {
        let mut slot = lock(&self.slot);
        if let Some(client) = slot.as_ref().filter(|client| !client.is_closed()) {
            return Ok(client.clone());
        }
        let client = Arc::new(Client::connect_or_start(&self.paths)?);
        *slot = Some(client.clone());
        Ok(client)
    }

    /// The daemon's reply to `command`, as far as `pick` takes it; any other reply is an error.
    pub fn request<T>(
        &self,
        account: &str,
        command: Command,
        pick: impl FnOnce(Event) -> Result<T, Event>,
    ) -> anyhow::Result<T> {
        let client = self.client()?;
        let result = client.request(account, command);
        self.forget_if_stopped(&client, &result);
        pick(result?).map_err(|reply| anyhow!("unexpected reply from the daemon: {reply:?}"))
    }

    /// The message's full raw bytes: from the store, or fetched through the daemon once; a stored body needs no daemon.
    pub fn raw_message(
        &self,
        account: &str,
        store: &Store,
        msg: &Message,
    ) -> anyhow::Result<Vec<u8>> {
        if let Some(raw) = store.raw(&msg.folder, msg.uid)? {
            return Ok(raw);
        }
        let fetch = Command::FetchBody {
            folder: msg.folder.clone(),
            uid: msg.uid,
        };
        self.request(account, fetch, Ok)?;
        store
            .raw(&msg.folder, msg.uid)?
            .ok_or_else(|| anyhow!("no body for {}/{} after fetching", msg.folder, msg.uid))
    }

    /// Drops `client` when its daemon is gone, so the next call reconnects.
    fn forget_if_stopped<T>(&self, client: &Arc<Client>, result: &anyhow::Result<T>) {
        let stopped = result
            .as_ref()
            .is_err_and(|e| e.chain().any(|cause| cause.is::<Stopped>()));
        let mut slot = lock(&self.slot);
        if stopped && slot.as_ref().is_some_and(|held| Arc::ptr_eq(held, client)) {
            *slot = None;
        }
    }
}

impl Client {
    /// Connects to a running daemon; Err when none answers or it is another version.
    #[cfg(test)]
    pub(crate) fn connect(paths: &Paths) -> anyhow::Result<Client> {
        Ok(connect_as(paths, VERSION, false)?)
    }

    /// Connects to whatever daemon answers, of any version; None when nothing does.
    pub fn connect_any_version(paths: &Paths) -> anyhow::Result<Option<Client>> {
        match connect_as(paths, VERSION, true) {
            Ok(client) => Ok(Some(client)),
            Err(ConnectError::Down(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionRefused
                        | io::ErrorKind::NotFound
                        | io::ErrorKind::UnexpectedEof
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Connects, starting this binary as the daemon when none answers, and replacing a daemon of an older version or
    /// another protocol; a newer daemon is an error, so an old window never kills its upgrade.
    pub fn connect_or_start(paths: &Paths) -> anyhow::Result<Client> {
        let exe = std::env::current_exe().context("finding this postbode")?;
        Client::connect_or_start_with(paths, &exe, VERSION)
    }

    pub fn connect_or_start_with(
        paths: &Paths,
        exe: &Path,
        version: &str,
    ) -> anyhow::Result<Client> {
        match connect_as(paths, version, false) {
            Ok(client) => return Ok(client),
            Err(ConnectError::Mismatch {
                theirs: Some(theirs),
                ..
            }) if is_newer(&theirs, version) => {
                bail!(NewerDaemon {
                    theirs,
                    ours: version.into()
                })
            }
            Err(ConnectError::Mismatch {
                reason, connection, ..
            }) => {
                log::info!("{reason}; restarting it");
                stop_daemon(paths, connection);
            }
            Err(ConnectError::Down(_)) => {}
        }
        start_daemon(paths, exe, version)
    }

    /// True once the daemon hung up; a holder should connect again.
    pub fn is_closed(&self) -> bool {
        lock(&self.inbox).closed
    }

    /// Sends and waits for the reply, up to 120 s unless `reply_timeout` says the command may take longer.
    pub fn request(&self, account: &str, command: Command) -> anyhow::Result<Event> {
        let timeout = reply_timeout(&command);
        self.request_until(account, command, timeout)
    }

    #[cfg(test)]
    pub(crate) fn request_within(
        &self,
        account: &str,
        command: Command,
        timeout: Duration,
    ) -> anyhow::Result<Event> {
        self.request_until(account, command, Some(timeout))
    }

    fn request_until(
        &self,
        account: &str,
        command: Command,
        timeout: Option<Duration>,
    ) -> anyhow::Result<Event> {
        let message = |id| ClientMessage::Command {
            id,
            account: account.into(),
            command,
        };
        match self.call(message, timeout)? {
            Payload::Event(event) => Ok(event),
            other => bail!("unexpected reply from the daemon: {other:?}"),
        }
    }

    /// Sends without waiting; a refusal arrives as `Event::CommandFailed` on the subscription.
    pub fn send(&self, account: &str, command: Command) -> bool {
        #[cfg(test)]
        if let Transport::Memory { commands, .. } = &self.transport {
            return commands.send((account.into(), command)).is_ok();
        }
        let Some(stream) = self.transport.socket() else {
            return false;
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = ClientMessage::Command {
            id,
            account: account.into(),
            command,
        };
        let line = match wire::line(&message) {
            Ok(line) => line,
            Err(e) => {
                lock(&self.inbox).publish(Event::CommandFailed {
                    account: account.into(),
                    request: 0,
                    message: encode_failure(&e),
                });
                return true;
            }
        };
        {
            let mut inbox = lock(&self.inbox);
            if inbox.closed {
                return false;
            }
            inbox.sent.insert(id, account.into());
        }
        let written = lock(stream).write_all(line.as_bytes()).is_ok();
        if !written {
            lock(&self.inbox).sent.remove(&id);
        }
        written
    }

    /// Every broadcast event from now on, plus refusals of `send`. Call once.
    pub fn subscribe(&self) -> anyhow::Result<Receiver<Event>> {
        self.subscribe_waking(|| {})
    }

    /// As `subscribe`, calling `wake` after each event and once the daemon hung up, so a window can repaint.
    pub fn subscribe_waking(
        &self,
        wake: impl Fn() + Send + 'static,
    ) -> anyhow::Result<Receiver<Event>> {
        #[cfg(test)]
        if let Transport::Memory { events, .. } = &self.transport {
            return lock(events)
                .take()
                .ok_or_else(|| anyhow!("already subscribed"));
        }
        let (subscriber, events) = mpsc::channel();
        {
            let mut inbox = lock(&self.inbox);
            if inbox.subscriber.is_some() {
                bail!("already subscribed");
            }
            inbox.subscriber = Some(subscriber);
            inbox.wake = Some(Box::new(wake));
        }
        if let Err(e) = self.call(|id| ClientMessage::Subscribe { id }, Some(REPLY_TIMEOUT)) {
            let mut inbox = lock(&self.inbox);
            inbox.subscriber = None;
            inbox.wake = None;
            return Err(e);
        }
        Ok(events)
    }

    pub fn status(&self) -> anyhow::Result<Status> {
        #[cfg(test)]
        if let Transport::Memory { accounts, .. } = &self.transport {
            return Ok(memory_status(accounts));
        }
        match self.call(|id| ClientMessage::Status { id }, Some(REPLY_TIMEOUT))? {
            Payload::Status(status) => Ok(status),
            other => bail!("unexpected reply from the daemon: {other:?}"),
        }
    }

    pub fn shutdown(&self) -> anyhow::Result<()> {
        self.call(|id| ClientMessage::Shutdown { id }, Some(REPLY_TIMEOUT))?;
        Ok(())
    }

    /// Stops the daemon and waits for its socket to go; returns the pid it had.
    pub fn stop_and_wait(&self, paths: &Paths) -> anyhow::Result<u32> {
        let pid = self.status()?.pid;
        self.shutdown()?;
        let deadline = Instant::now() + STOP_WAIT;
        while paths.daemon_socket().exists() {
            if Instant::now() >= deadline {
                bail!("the daemon (pid {pid}) did not stop within 5 s");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(pid)
    }

    /// A client with no daemon behind it: commands appear on the returned receiver, events are injected by the test.
    #[cfg(test)]
    pub(crate) fn in_memory(
        accounts: &[&str],
    ) -> (Client, Receiver<(String, Command)>, Sender<Event>) {
        let (commands, sent) = mpsc::channel();
        let (inject, events) = mpsc::channel();
        let client = Client {
            transport: Transport::Memory {
                accounts: accounts.iter().map(|name| name.to_string()).collect(),
                commands,
                events: Mutex::new(Some(events)),
            },
            inbox: Arc::default(),
            next_id: AtomicU64::new(1),
            log: PathBuf::new(),
        };
        (client, sent, inject)
    }

    /// Sends the message `with_id` builds and waits for its reply; without a timeout only a lost daemon ends the wait.
    fn call(
        &self,
        with_id: impl FnOnce(u64) -> ClientMessage,
        timeout: Option<Duration>,
    ) -> anyhow::Result<Payload> {
        let Some(stream) = self.transport.socket() else {
            bail!("in-memory client has no daemon");
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let line = wire::line(&with_id(id)).map_err(|e| anyhow!(encode_failure(&e)))?;
        let (waiter, reply) = mpsc::channel();
        {
            let mut inbox = lock(&self.inbox);
            if inbox.closed {
                return Err(self.stopped());
            }
            inbox.waiters.insert(id, waiter);
        }
        if lock(stream).write_all(line.as_bytes()).is_err() {
            lock(&self.inbox).waiters.remove(&id);
            return Err(self.stopped());
        }
        let received = match timeout {
            Some(timeout) => reply.recv_timeout(timeout),
            None => reply.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match received {
            Ok(Outcome::Ok(payload)) => Ok(payload),
            Ok(Outcome::Error(message)) => Err(anyhow!(message)),
            Err(RecvTimeoutError::Timeout) => {
                lock(&self.inbox).waiters.remove(&id);
                Err(anyhow!(NO_REPLY))
            }
            Err(RecvTimeoutError::Disconnected) => Err(self.stopped()),
        }
    }

    fn stopped(&self) -> anyhow::Error {
        anyhow::Error::new(Stopped(format!(
            "the daemon stopped; see {}",
            self.log.display()
        )))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(stream) = self.transport.socket() {
            let _ = lock(stream).shutdown(Shutdown::Both);
        }
    }
}

/// How long `request` waits: a sync pass, a body fetch over a folder or a rule over stored mail can outlast 120 s, and
/// giving up early would drop the client and let an auto-started daemon idle out mid-run.
pub(super) fn reply_timeout(command: &Command) -> Option<Duration> {
    match command {
        Command::ApplyRule { .. } | Command::FetchBodies { .. } | Command::SyncNow => None,
        Command::Apply { .. } | Command::FetchBody { .. } | Command::Restore { .. } => {
            Some(REPLY_TIMEOUT)
        }
    }
}

fn encode_failure(error: &io::Error) -> String {
    format!("could not encode the request: {error}")
}

#[cfg(test)]
fn memory_status(accounts: &[String]) -> Status {
    use super::wire::AccountStatus;
    Status {
        pid: std::process::id(),
        version: VERSION.into(),
        uptime_secs: 0,
        clients: 1,
        accounts: accounts
            .iter()
            .map(|name| AccountStatus {
                name: name.clone(),
                activity: None,
            })
            .collect(),
    }
}

/// Exchanges hello as `version`; with `any_version` only the protocol has to match.
pub(super) fn connect_as(
    paths: &Paths,
    version: &str,
    any_version: bool,
) -> Result<Client, ConnectError> {
    let stream = UnixStream::connect(paths.daemon_socket()).map_err(ConnectError::Down)?;
    let mut reader = handshake(&stream, version, any_version)?;
    let inbox = Arc::new(Mutex::new(Inbox::default()));
    let delivered = inbox.clone();
    std::thread::Builder::new()
        .name("daemon-replies".into())
        .spawn(move || read_messages(&mut reader, &delivered))
        .map_err(ConnectError::Down)?;
    Ok(Client {
        transport: Transport::Socket(Mutex::new(stream)),
        inbox,
        next_id: AtomicU64::new(1),
        log: paths.daemon_log(),
    })
}

/// Sends our hello and checks the daemon's; returns the reader positioned after it.
fn handshake(
    stream: &UnixStream,
    version: &str,
    any_version: bool,
) -> Result<BufReader<UnixStream>, ConnectError> {
    let hello = ClientMessage::Hello {
        protocol: PROTOCOL,
        version: version.into(),
    };
    wire::write_line(&mut &*stream, &hello).map_err(ConnectError::Down)?;
    stream
        .set_read_timeout(Some(START_WAIT))
        .map_err(ConnectError::Down)?;
    let mut reader = BufReader::new(stream.try_clone().map_err(ConnectError::Down)?);
    let mut line = String::new();
    if reader.read_line(&mut line).map_err(ConnectError::Down)? == 0 {
        return Err(ConnectError::Down(io::ErrorKind::UnexpectedEof.into()));
    }
    let (reason, theirs) = match serde_json::from_str(&line) {
        Ok(DaemonMessage::Hello {
            protocol,
            version: theirs,
            ..
        }) if protocol == PROTOCOL && (any_version || theirs == version) => {
            stream.set_read_timeout(None).map_err(ConnectError::Down)?;
            return Ok(reader);
        }
        Ok(DaemonMessage::Hello {
            protocol,
            version: theirs,
            ..
        }) => (
            format!("daemon is version {theirs} (protocol {protocol}); this postbode is {version}"),
            Some(theirs),
        ),
        _ => (
            format!("the daemon's hello was not understood; this postbode is {version}"),
            None,
        ),
    };
    Err(ConnectError::Mismatch {
        reason,
        theirs,
        connection: reader,
    })
}

/// Whether version `theirs` is newer than `ours`, comparing dotted numbers; a pre-release or garbage sorts as older.
pub(super) fn is_newer(theirs: &str, ours: &str) -> bool {
    fn parts(version: &str) -> Option<Vec<u64>> {
        version.split('.').map(|part| part.parse().ok()).collect()
    }
    parts(theirs) > parts(ours)
}

fn read_messages(reader: &mut BufReader<UnixStream>, inbox: &Mutex<Inbox>) {
    for line in reader.lines() {
        let Ok(line) = line else { break };
        match serde_json::from_str(&line) {
            Ok(DaemonMessage::Reply { id, outcome }) => lock(inbox).deliver(id, outcome),
            Ok(DaemonMessage::Event(event)) => lock(inbox).receive(event),
            Ok(DaemonMessage::Hello { .. }) => {}
            Err(e) => log::warn!("unreadable message from the daemon: {e}"),
        }
    }
    lock(inbox).close();
}

/// Shuts down the daemon behind `connection`, waits for it to hang up, then up to 5 s for its socket to go.
fn stop_daemon(paths: &Paths, mut connection: BufReader<UnixStream>) {
    let shutdown = ClientMessage::Shutdown { id: 0 };
    if wire::write_line(&mut connection.get_ref(), &shutdown).is_ok() {
        // The read timeout from the handshake bounds this wait.
        let _ = io::copy(&mut connection, &mut io::sink());
    }
    let deadline = Instant::now() + START_WAIT;
    while paths.daemon_socket().exists() && Instant::now() < deadline {
        std::thread::sleep(RETRY);
    }
}

/// Spawns `exe` as the daemon and connects within 5 s, spawning again when it exited and no socket is left.
fn start_daemon(paths: &Paths, exe: &Path, version: &str) -> anyhow::Result<Client> {
    let deadline = Instant::now() + START_WAIT;
    let mut child = spawn_daemon(paths, exe)?;
    let connected = loop {
        std::thread::sleep(RETRY);
        // We started our own binary; whatever now serves is the daemon to use, so only the protocol must match.
        match connect_as(paths, version, true) {
            Ok(client) => break Ok(client),
            Err(ConnectError::Mismatch { reason, .. }) => break Err(anyhow!(reason)),
            Err(ConnectError::Down(_)) => {}
        }
        if Instant::now() >= deadline {
            let log = paths.daemon_log();
            break Err(anyhow!(
                "could not start the daemon; last lines of {}:\n{}",
                log.display(),
                log_tail(&log)
            ));
        }
        if matches!(child.try_wait(), Ok(Some(_))) && !paths.daemon_socket().exists() {
            child = spawn_daemon(paths, exe)?;
        }
    };
    reap(child);
    connected
}

fn spawn_daemon(paths: &Paths, exe: &Path) -> anyhow::Result<Child> {
    paths::create_private_dir(&paths.state_dir)?;
    let log = File::options()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(paths.daemon_log())
        .with_context(|| format!("opening {}", paths.daemon_log().display()))?;
    let mut command = std::process::Command::new(exe);
    command
        .args(["run", "--idle-exit", "60"])
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0);
    if let Some(home) = &paths.home {
        command.env("POSTBODE_HOME", home);
    }
    command
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))
}

/// Waits on the daemon in the background so it does not linger as a zombie.
fn reap(mut child: Child) {
    std::thread::spawn(move || child.wait());
}

fn log_tail(log: &Path) -> String {
    let text = fs::read(log)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(LOG_TAIL_LINES)..].join("\n")
}
