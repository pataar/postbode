pub mod imap;

use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteFolder {
    pub name: String,
    pub special_use: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SelectInfo {
    pub uidvalidity: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Envelope {
    pub uid: u32,
    pub flags: Vec<String>,
    pub internaldate: i64,
    pub size: Option<u32>,
    pub headers: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FlagUpdate {
    pub uid: u32,
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleOutcome {
    NewMail,
    Timeout,
    Interrupted,
}

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("could not connect: {0}")]
    Connect(String),
    #[error("login failed: {0}")]
    Auth(String),
    #[error("server error: {0}")]
    Protocol(String),
    #[error("connection lost: {0}")]
    Io(String),
}

pub type MailResult<T> = Result<T, MailError>;

pub trait MailOps {
    fn list_folders(&mut self) -> MailResult<Vec<RemoteFolder>>;
    fn select(&mut self, folder: &str) -> MailResult<SelectInfo>;
    /// Envelopes with uid >= `from_uid` in the selected folder.
    fn fetch_new(&mut self, from_uid: u32) -> MailResult<Vec<Envelope>>;
    /// Current flags for every uid <= `upto_uid` in the selected folder.
    fn fetch_flags(&mut self, upto_uid: u32) -> MailResult<Vec<FlagUpdate>>;
    fn fetch_raw(&mut self, uid: u32) -> MailResult<Option<Vec<u8>>>;
    fn add_flags(&mut self, uid: u32, flags: &[&str]) -> MailResult<()>;
    fn remove_flags(&mut self, uid: u32, flags: &[&str]) -> MailResult<()>;
    fn expunge(&mut self, uid: u32) -> MailResult<()>;
    /// Moves the message; returns the new uid when the server reports it.
    fn move_message(&mut self, uid: u32, to: &str) -> MailResult<Option<u32>>;
    fn create_folder(&mut self, name: &str) -> MailResult<()>;
    fn append(&mut self, folder: &str, raw: &[u8]) -> MailResult<()>;
    /// Waits for new mail in the selected folder, the timeout, or `interrupt` becoming true.
    fn idle(&mut self, timeout: Duration, interrupt: &AtomicBool) -> MailResult<IdleOutcome>;
}

#[cfg(any(test, feature = "testing"))]
pub use recording::RecordingOps;

#[cfg(any(test, feature = "testing"))]
mod recording {
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::*;

    #[derive(Debug, Default)]
    pub struct RecordingOps {
        pub folders: Vec<RemoteFolder>,
        pub uidvalidity: HashMap<String, u32>,
        pub mail: HashMap<String, Vec<Envelope>>,
        pub raw: HashMap<(String, u32), Vec<u8>>,
        pub supports_move: bool,
        pub calls: Vec<String>,
        pub idle_outcomes: VecDeque<IdleOutcome>,
        selected: String,
        next_uid: HashMap<String, u32>,
    }

    impl RecordingOps {
        pub fn new() -> RecordingOps {
            RecordingOps {
                supports_move: true,
                ..Default::default()
            }
        }

        pub fn with_folder(mut self, name: &str, special_use: Option<&str>) -> RecordingOps {
            self.folders.push(RemoteFolder {
                name: name.into(),
                special_use: special_use.map(str::to_string),
            });
            self.uidvalidity.insert(name.into(), 1);
            self.mail.entry(name.into()).or_default();
            self
        }

        pub fn add_mail(
            &mut self,
            folder: &str,
            uid: u32,
            internaldate: i64,
            headers: &str,
            raw: Option<&str>,
        ) {
            self.mail.entry(folder.into()).or_default().push(Envelope {
                uid,
                flags: vec![],
                internaldate,
                size: raw.map(|r| r.len() as u32),
                headers: headers.as_bytes().to_vec(),
            });
            if let Some(raw) = raw {
                self.raw
                    .insert((folder.into(), uid), raw.as_bytes().to_vec());
            }
            let next = self.next_uid.entry(folder.into()).or_insert(1);
            *next = (*next).max(uid + 1);
        }

        fn envelope_mut(&mut self, uid: u32) -> MailResult<&mut Envelope> {
            let folder = self.selected.clone();
            self.mail
                .get_mut(&folder)
                .and_then(|list| list.iter_mut().find(|e| e.uid == uid))
                .ok_or_else(|| MailError::Protocol(format!("no uid {uid} in {folder}")))
        }
    }

    impl MailOps for RecordingOps {
        fn list_folders(&mut self) -> MailResult<Vec<RemoteFolder>> {
            self.calls.push("list_folders".into());
            Ok(self.folders.clone())
        }

        fn select(&mut self, folder: &str) -> MailResult<SelectInfo> {
            self.calls.push(format!("select {folder}"));
            let uidvalidity = *self
                .uidvalidity
                .get(folder)
                .ok_or_else(|| MailError::Protocol(format!("no folder {folder}")))?;
            self.selected = folder.to_string();
            Ok(SelectInfo { uidvalidity })
        }

        fn fetch_new(&mut self, from_uid: u32) -> MailResult<Vec<Envelope>> {
            self.calls
                .push(format!("fetch_new {} {from_uid}", self.selected));
            let list = self.mail.get(&self.selected).cloned().unwrap_or_default();
            let max = list.iter().map(|e| e.uid).max().unwrap_or(0);
            // Real servers answer `N:*` with the highest message when N exceeds it.
            let out: Vec<Envelope> = if from_uid > max {
                list.into_iter().filter(|e| e.uid == max).collect()
            } else {
                list.into_iter().filter(|e| e.uid >= from_uid).collect()
            };
            Ok(out)
        }

        fn fetch_flags(&mut self, upto_uid: u32) -> MailResult<Vec<FlagUpdate>> {
            self.calls
                .push(format!("fetch_flags {} {upto_uid}", self.selected));
            Ok(self
                .mail
                .get(&self.selected)
                .map(|l| {
                    l.iter()
                        .filter(|e| e.uid <= upto_uid)
                        .map(|e| FlagUpdate {
                            uid: e.uid,
                            flags: e.flags.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default())
        }

        fn fetch_raw(&mut self, uid: u32) -> MailResult<Option<Vec<u8>>> {
            self.calls
                .push(format!("fetch_raw {} {uid}", self.selected));
            Ok(self.raw.get(&(self.selected.clone(), uid)).cloned())
        }

        fn add_flags(&mut self, uid: u32, flags: &[&str]) -> MailResult<()> {
            self.calls.push(format!(
                "add_flags {} {uid} {}",
                self.selected,
                flags.join(" ")
            ));
            let env = self.envelope_mut(uid)?;
            for f in flags {
                if !env.flags.iter().any(|x| x == f) {
                    env.flags.push(f.to_string());
                }
            }
            Ok(())
        }

        fn remove_flags(&mut self, uid: u32, flags: &[&str]) -> MailResult<()> {
            self.calls.push(format!(
                "remove_flags {} {uid} {}",
                self.selected,
                flags.join(" ")
            ));
            let env = self.envelope_mut(uid)?;
            env.flags.retain(|f| !flags.contains(&f.as_str()));
            Ok(())
        }

        fn expunge(&mut self, uid: u32) -> MailResult<()> {
            self.calls.push(format!("expunge {} {uid}", self.selected));
            let folder = self.selected.clone();
            if let Some(list) = self.mail.get_mut(&folder) {
                list.retain(|e| e.uid != uid);
            }
            self.raw.remove(&(folder, uid));
            Ok(())
        }

        fn move_message(&mut self, uid: u32, to: &str) -> MailResult<Option<u32>> {
            self.calls
                .push(format!("move {} {uid} -> {to}", self.selected));
            if !self.mail.contains_key(to) {
                return Err(MailError::Protocol(format!("no folder {to}")));
            }
            let folder = self.selected.clone();
            let mut env = self.envelope_mut(uid)?.clone();
            self.mail.get_mut(&folder).unwrap().retain(|e| e.uid != uid);
            let raw = self.raw.remove(&(folder, uid));
            let next = self.next_uid.entry(to.into()).or_insert(1);
            let new_uid = *next;
            *next += 1;
            env.uid = new_uid;
            self.mail.get_mut(to).unwrap().push(env);
            if let Some(raw) = raw {
                self.raw.insert((to.into(), new_uid), raw);
            }
            Ok(if self.supports_move {
                Some(new_uid)
            } else {
                None
            })
        }

        fn create_folder(&mut self, name: &str) -> MailResult<()> {
            self.calls.push(format!("create_folder {name}"));
            self.folders.push(RemoteFolder {
                name: name.into(),
                special_use: None,
            });
            self.uidvalidity.insert(name.into(), 1);
            self.mail.entry(name.into()).or_default();
            Ok(())
        }

        fn append(&mut self, folder: &str, raw: &[u8]) -> MailResult<()> {
            self.calls
                .push(format!("append {folder} {} bytes", raw.len()));
            let uid = *self.next_uid.get(folder).unwrap_or(&1);
            let end = raw
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|i| i + 4)
                .unwrap_or(raw.len());
            self.add_mail(
                folder,
                uid,
                0,
                &String::from_utf8_lossy(&raw[..end]),
                Some(&String::from_utf8_lossy(raw)),
            );
            Ok(())
        }

        fn idle(&mut self, _timeout: Duration, interrupt: &AtomicBool) -> MailResult<IdleOutcome> {
            self.calls.push(format!("idle {}", self.selected));
            if interrupt.load(Ordering::Relaxed) {
                return Ok(IdleOutcome::Interrupted);
            }
            Ok(self
                .idle_outcomes
                .pop_front()
                .unwrap_or(IdleOutcome::Interrupted))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_fetch_new_mimics_star_semantics() {
        let mut ops = RecordingOps::new().with_folder("INBOX", None);
        ops.add_mail("INBOX", 1, 10, "Subject: a\r\n\r\n", None);
        ops.add_mail("INBOX", 2, 20, "Subject: b\r\n\r\n", None);
        ops.select("INBOX").unwrap();
        assert_eq!(ops.fetch_new(2).unwrap().len(), 1);
        assert_eq!(ops.fetch_new(5).unwrap()[0].uid, 2);
    }

    #[test]
    fn fake_move_assigns_new_uid_and_records_call() {
        let mut ops = RecordingOps::new()
            .with_folder("INBOX", None)
            .with_folder("Archive", Some("Archive"));
        ops.add_mail("INBOX", 1, 10, "Subject: a\r\n\r\n", Some("raw"));
        ops.select("INBOX").unwrap();
        assert_eq!(ops.move_message(1, "Archive").unwrap(), Some(1));
        assert!(ops.mail["INBOX"].is_empty());
        assert_eq!(ops.raw.get(&("Archive".into(), 1)).unwrap(), b"raw");
        assert_eq!(ops.calls.last().unwrap(), "move INBOX 1 -> Archive");
    }
}
