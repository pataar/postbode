//! The messages clients and the daemon exchange, one JSON object per line; these types are the protocol.
use serde::{Deserialize, Serialize};

use crate::engine::StartState;
use crate::sync::{Activity, Command, Event};

pub const PROTOCOL: u32 = 1;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        protocol: u32,
        version: String,
    },
    Subscribe {
        id: u64,
    },
    Status {
        id: u64,
    },
    Shutdown {
        id: u64,
    },
    Command {
        id: u64,
        account: String,
        command: Command,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonMessage {
    Hello {
        protocol: u32,
        version: String,
        pid: u32,
    },
    Reply {
        id: u64,
        outcome: Outcome,
    },
    Event(Event),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Ok(Payload),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Payload {
    Done,
    Event(Event),
    Status(Status),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub pid: u32,
    pub version: String,
    pub uptime_secs: u64,
    pub clients: usize,
    pub accounts: Vec<AccountStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountStatus {
    pub name: String,
    pub state: StartState,
    pub activity: Option<Activity>,
}

/// One message per line.
pub fn write_line(out: &mut impl std::io::Write, message: &impl Serialize) -> std::io::Result<()> {
    out.write_all(line(message)?.as_bytes())
}

/// The message as one JSON line, newline included; serde_json escapes any newline inside it.
pub(crate) fn line(message: &impl Serialize) -> std::io::Result<String> {
    let mut text = serde_json::to_string(message)?;
    text.push('\n');
    Ok(text)
}
