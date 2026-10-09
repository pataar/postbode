//! A temp Postvak home with stores and an in-memory daemon client, for the GUI tests.
use std::sync::mpsc::{Receiver, Sender};

use eframe::egui;
use egui_kittest::Harness;
use tempfile::TempDir;

use crate::config::Config;
use crate::daemon::Client;
use crate::paths::Paths;
use crate::store::{Folder, Message, Store};
use crate::sync::{Command, Event};

use super::App;
use super::app::session;

pub(crate) struct Fixture {
    pub accounts: Vec<String>,
    pub paths: Paths,
    _dir: TempDir,
}

/// The daemon's ends of the app's client: the commands it sent, and a sender for test events; dropping `events` reads
/// as the daemon going away.
pub(crate) struct Wires {
    pub commands: Receiver<(String, Command)>,
    pub events: Sender<Event>,
}

impl Wires {
    pub fn sent(&self) -> Vec<(String, Command)> {
        self.commands.try_iter().collect()
    }
}

impl Fixture {
    /// A config entry and an empty INBOX per account.
    pub fn new(accounts: &[&str]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::under(dir.path());
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        let mut config = String::from("# test accounts\n");
        for name in accounts {
            config.push_str(&format!(
                "[[accounts]]\nname = \"{name}\"\nhost = \"imap.example.com\"\nusername = \"{name}@example.com\"\npassword = {{ keyring = true }}\n\n"
            ));
        }
        std::fs::write(paths.config_file(), config).unwrap();
        let fixture = Fixture {
            accounts: accounts.iter().map(|name| name.to_string()).collect(),
            paths,
            _dir: dir,
        };
        for name in accounts {
            fixture.folder(name, "INBOX", None);
        }
        fixture
    }

    pub fn append_config(&self, text: &str) {
        let mut config = std::fs::read_to_string(self.paths.config_file()).unwrap();
        config.push_str(text);
        std::fs::write(self.paths.config_file(), config).unwrap();
    }

    pub fn store(&self, account: &str) -> Store {
        Store::open(&self.paths.mail_db(account)).unwrap()
    }

    pub fn folder(&self, account: &str, name: &str, special_use: Option<&str>) {
        let folder = Folder {
            name: name.into(),
            uidvalidity: 1,
            last_uid: 0,
            special_use: special_use.map(str::to_string),
            delimiter: None,
        };
        self.store(account).upsert_folder(&folder).unwrap();
    }

    pub fn add(&self, account: &str, message: Message) {
        self.store(account).insert_message(&message).unwrap();
    }

    /// The app over this home, drawn headless at 1280×800; each frame advances input time by 10 ms.
    pub fn harness(&self) -> (Harness<'static, App>, Wires) {
        let config = Config::load(&self.paths.config_file()).unwrap();
        let names: Vec<&str> = self.accounts.iter().map(String::as_str).collect();
        let (client, commands, events) = Client::in_memory(&names);
        let (client, received, states) = session(client, Box::new(|| {})).unwrap();
        let mut app = App::new(&config, self.paths.clone(), client, received, states);
        app.reconnect = |_, _| Err(anyhow::anyhow!("no daemon in tests"));
        app.fetch_release = || Err(anyhow::anyhow!("no network in tests"));
        app.downloads = self.paths.cache_dir.join("downloads");
        std::fs::create_dir_all(&app.downloads).unwrap();
        let harness = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .with_step_dt(0.01)
            .build_ui_state(|ui, app: &mut App| app.show(ui), app);
        (harness, Wires { commands, events })
    }
}

/// A read message with a stored body, alone in its thread; a higher uid is newer.
pub(crate) fn message(folder: &str, uid: u32, subject: &str) -> Message {
    let at = 1_790_000_000 + i64::from(uid) * 60;
    Message {
        folder: folder.into(),
        uid,
        message_id: Some(format!("<{uid}@example.com>")),
        from_addr: Some(format!("Sender {uid} <sender{uid}@example.com>")),
        to_addr: Some("me@example.com".into()),
        thread_id: format!("<{uid}@example.com>"),
        subject: Some(subject.into()),
        date: Some(at),
        internaldate: at,
        flags: "\\Seen".into(),
        size: Some(100),
        headers: format!("Subject: {subject}\r\n\r\n").into_bytes(),
        body_text: Some(format!("Body of {uid}")),
        ..Default::default()
    }
}
