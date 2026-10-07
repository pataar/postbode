//! An in-process daemon on a temp home and a raw line client, shared by the daemon, client and MCP tests.
use std::io::{BufRead, BufReader, Lines};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tempfile::TempDir;

use super::wire::{self, ClientMessage, DaemonMessage, Outcome};
use super::{Options, run};
use crate::engine::Connector;
use crate::mail_ops::{MailOps, RecordingOps};
use crate::paths::Paths;
use crate::sync::{Event, SyncError};

pub(crate) const WAIT: Duration = Duration::from_secs(5);

pub(crate) const CONFIG: &str = r#"
[[accounts]]
name = "work"
host = "127.0.0.1"
username = "me@example.com"
password = { command = "printf x" }
notify = false

[[accounts]]
name = "play"
host = "127.0.0.1"
username = "me@example.org"
password = { command = "printf x" }
notify = false
"#;

/// A fake server per connection that idles until woken.
pub(crate) fn recording_connector() -> Connector {
    Arc::new(|_| Ok(Box::new(RecordingOps::new().with_folder("INBOX", None)) as Box<dyn MailOps>))
}

/// Every connection attempt fails, so each account goes offline.
pub(crate) fn offline_connector() -> Connector {
    Arc::new(|_| Err(SyncError::Io(std::io::Error::other("no route to host"))))
}

pub(crate) fn options(connect: Connector, idle_exit: Option<Duration>) -> Options {
    Options {
        idle_exit,
        connect,
        notify: false,
    }
}

pub(crate) fn write_config(paths: &Paths) {
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::write(paths.config_file(), CONFIG).unwrap();
}

/// A daemon serving a temp home; dropping it shuts the daemon down and waits for `run` to return.
pub(crate) struct TestDaemon {
    pub paths: Paths,
    finished: Receiver<anyhow::Result<()>>,
    handle: Option<JoinHandle<()>>,
    _home: TempDir,
}

impl TestDaemon {
    pub fn start() -> TestDaemon {
        TestDaemon::start_with(options(recording_connector(), None))
    }

    pub fn start_with(options: Options) -> TestDaemon {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::under(home.path());
        write_config(&paths);
        TestDaemon::serve(home, paths, options)
    }

    /// Serves `paths` under `home`, which the caller prepared.
    pub fn serve(home: TempDir, paths: Paths, options: Options) -> TestDaemon {
        let (done, finished) = mpsc::channel();
        let handle = std::thread::spawn({
            let paths = paths.clone();
            move || {
                let _ = done.send(run(&paths, options));
            }
        });
        let daemon = TestDaemon {
            paths,
            finished,
            handle: Some(handle),
            _home: home,
        };
        daemon.wait_for_socket();
        daemon
    }

    fn wait_for_socket(&self) {
        let deadline = Instant::now() + WAIT;
        while UnixStream::connect(self.paths.daemon_socket()).is_err() {
            if let Ok(result) = self.finished.try_recv() {
                panic!("the daemon ended before serving: {result:?}");
            }
            assert!(Instant::now() < deadline, "no daemon socket within 5 s");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A client that has exchanged `hello`.
    pub fn client(&self) -> TestClient {
        TestClient::hello(&self.paths.daemon_socket())
    }

    /// What `run` returned, waiting up to 5 s for it.
    pub fn finished(&mut self) -> anyhow::Result<()> {
        let result = self
            .finished
            .recv_timeout(WAIT)
            .expect("the daemon did not stop within 5 s");
        self.handle.take().unwrap().join().unwrap();
        result
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        if self.handle.is_none() {
            return;
        }
        if let Ok(stream) = UnixStream::connect(self.paths.daemon_socket()) {
            let mut client = TestClient::over(stream);
            client.send(&ClientMessage::Hello {
                protocol: wire::PROTOCOL,
                version: wire::VERSION.into(),
            });
            client.send(&ClientMessage::Shutdown { id: u64::MAX });
        }
        if !std::thread::panicking() {
            self.finished().unwrap();
        }
    }
}

/// Writes `ClientMessage` lines and reads `DaemonMessage` lines, failing a read after 5 s.
pub(crate) struct TestClient {
    stream: UnixStream,
    lines: Lines<BufReader<UnixStream>>,
}

impl TestClient {
    pub fn connect(socket: &Path) -> TestClient {
        TestClient::over(UnixStream::connect(socket).unwrap())
    }

    pub fn hello(socket: &Path) -> TestClient {
        let mut client = TestClient::connect(socket);
        client.send(&ClientMessage::Hello {
            protocol: wire::PROTOCOL,
            version: wire::VERSION.into(),
        });
        match client.recv() {
            Some(DaemonMessage::Hello { .. }) => client,
            other => panic!("expected hello, got {other:?}"),
        }
    }

    fn over(stream: UnixStream) -> TestClient {
        stream.set_read_timeout(Some(WAIT)).unwrap();
        let lines = BufReader::new(stream.try_clone().unwrap()).lines();
        TestClient { stream, lines }
    }

    pub fn send(&mut self, message: &ClientMessage) {
        wire::write_line(&mut self.stream, message).unwrap();
    }

    pub fn send_raw(&mut self, line: &str) {
        use std::io::Write;
        writeln!(self.stream, "{line}").unwrap();
    }

    /// The next message, or None once the daemon closed the connection.
    pub fn recv(&mut self) -> Option<DaemonMessage> {
        let line = self
            .lines
            .next()?
            .expect("no message from the daemon within 5 s");
        Some(serde_json::from_str(&line).unwrap())
    }

    /// The reply to `id`, skipping events.
    pub fn reply(&mut self, id: u64) -> Outcome {
        loop {
            match self.recv() {
                Some(DaemonMessage::Reply { id: got, outcome }) if got == id => return outcome,
                Some(DaemonMessage::Reply { id: got, .. }) => {
                    panic!("reply to {got}, expected {id}")
                }
                Some(_) => {}
                None => panic!("the daemon closed the connection before replying to {id}"),
            }
        }
    }

    /// Sends the request and waits for its reply.
    pub fn request(&mut self, message: ClientMessage) -> Outcome {
        let id = match &message {
            ClientMessage::Command { id, .. }
            | ClientMessage::Shutdown { id }
            | ClientMessage::Status { id }
            | ClientMessage::Subscribe { id } => *id,
            ClientMessage::Hello { .. } => panic!("hello has no reply"),
        };
        self.send(&message);
        self.reply(id)
    }

    /// The first broadcast event matching `wanted`.
    pub fn event_where(&mut self, wanted: impl Fn(&Event) -> bool) -> Event {
        loop {
            match self.recv() {
                Some(DaemonMessage::Event(event)) if wanted(&event) => return event,
                Some(_) => {}
                None => panic!("the daemon closed the connection"),
            }
        }
    }
}

/// Keeps `client` subscribed and returns it.
pub(crate) fn subscribed(mut client: TestClient) -> TestClient {
    assert_eq!(
        client.request(ClientMessage::Subscribe { id: 0 }),
        Outcome::Ok(wire::Payload::Done)
    );
    client
}
