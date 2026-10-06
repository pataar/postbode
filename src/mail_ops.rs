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
    /// Uids >= `from_uid` in the selected folder, ascending.
    fn search_uids(&mut self, from_uid: u32) -> MailResult<Vec<u32>>;
    /// Envelopes with `first <= uid <= last` in the selected folder.
    fn fetch_envelopes(&mut self, first: u32, last: u32) -> MailResult<Vec<Envelope>>;
    /// Current flags for every uid <= `upto_uid` in the selected folder.
    fn fetch_flags(&mut self, upto_uid: u32) -> MailResult<Vec<FlagUpdate>>;
    fn fetch_raw(&mut self, uid: u32) -> MailResult<Option<Vec<u8>>>;
    fn add_flags(&mut self, uid: u32, flags: &[&str]) -> MailResult<()>;
    fn remove_flags(&mut self, uid: u32, flags: &[&str]) -> MailResult<()>;
    fn expunge(&mut self, uid: u32) -> MailResult<()>;
    /// Moves the message; returns the new uid when the server reports it.
    fn move_message(&mut self, uid: u32, to: &str) -> MailResult<Option<u32>>;
    fn create_folder(&mut self, name: &str) -> MailResult<()>;
    /// Appends a message carrying `flags`, e.g. a keyword.
    fn append(&mut self, folder: &str, raw: &[u8], flags: &[&str]) -> MailResult<()>;
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

    #[derive(Debug)]
    pub struct RecordingOps {
        pub folders: Vec<RemoteFolder>,
        pub uidvalidity: HashMap<String, u32>,
        pub mail: HashMap<String, Vec<Envelope>>,
        pub raw: HashMap<(String, u32), Vec<u8>>,
        pub calls: Vec<String>,
        pub idle_outcomes: VecDeque<IdleOutcome>,
        /// Makes `fetch_envelopes` fail once this many calls succeeded, like a connection dropping mid-sync.
        pub fail_fetch_after: Option<usize>,
        /// Makes the next `add_flags`, `append` or `fetch_raw` fail with this error, once.
        pub fail_next: Option<MailError>,
        envelope_fetches: usize,
        selected: String,
        next_uid: HashMap<String, u32>,
    }

    impl RecordingOps {
        pub fn new() -> RecordingOps {
            RecordingOps {
                folders: Vec::new(),
                uidvalidity: HashMap::new(),
                mail: HashMap::new(),
                raw: HashMap::new(),
                calls: Vec::new(),
                idle_outcomes: VecDeque::new(),
                fail_fetch_after: None,
                fail_next: None,
                envelope_fetches: 0,
                selected: String::new(),
                next_uid: HashMap::new(),
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

    impl Default for RecordingOps {
        fn default() -> Self {
            Self::new()
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

        fn search_uids(&mut self, from_uid: u32) -> MailResult<Vec<u32>> {
            self.calls
                .push(format!("search_uids {} {from_uid}", self.selected));
            let mut uids: Vec<u32> = self
                .mail
                .get(&self.selected)
                .map(|list| list.iter().map(|e| e.uid).collect())
                .unwrap_or_default();
            uids.sort_unstable();
            let max = uids.last().copied().unwrap_or(0);
            // Real servers answer `N:*` with the highest message when N exceeds it.
            if from_uid > max {
                return Ok(uids.into_iter().filter(|&uid| uid == max).collect());
            }
            Ok(uids.into_iter().filter(|&uid| uid >= from_uid).collect())
        }

        fn fetch_envelopes(&mut self, first: u32, last: u32) -> MailResult<Vec<Envelope>> {
            self.calls
                .push(format!("fetch_envelopes {} {first} {last}", self.selected));
            if self
                .fail_fetch_after
                .is_some_and(|n| self.envelope_fetches >= n)
            {
                return Err(MailError::Io("fetch_envelopes failed".into()));
            }
            self.envelope_fetches += 1;
            let mut out: Vec<Envelope> = self
                .mail
                .get(&self.selected)
                .map(|list| {
                    list.iter()
                        .filter(|e| (first..=last).contains(&e.uid))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            out.sort_by_key(|e| e.uid);
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
            self.fail_next.take().map_or(Ok(()), Err)?;
            Ok(self.raw.get(&(self.selected.clone(), uid)).cloned())
        }

        fn add_flags(&mut self, uid: u32, flags: &[&str]) -> MailResult<()> {
            self.calls.push(format!(
                "add_flags {} {uid} {}",
                self.selected,
                flags.join(" ")
            ));
            self.fail_next.take().map_or(Ok(()), Err)?;
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
            let is_deleted = self.mail.get(&folder).is_some_and(|list| {
                list.iter()
                    .any(|e| e.uid == uid && e.flags.iter().any(|f| f == "\\Deleted"))
            });
            if is_deleted {
                if let Some(list) = self.mail.get_mut(&folder) {
                    list.retain(|e| e.uid != uid);
                }
                self.raw.remove(&(folder, uid));
            }
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
            Ok(None)
        }

        fn create_folder(&mut self, name: &str) -> MailResult<()> {
            self.calls.push(format!("create_folder {name}"));
            if self.mail.contains_key(name) {
                return Err(MailError::Protocol(format!("[ALREADYEXISTS] {name}")));
            }
            self.folders.push(RemoteFolder {
                name: name.into(),
                special_use: None,
            });
            self.uidvalidity.insert(name.into(), 1);
            self.mail.entry(name.into()).or_default();
            Ok(())
        }

        fn append(&mut self, folder: &str, raw: &[u8], flags: &[&str]) -> MailResult<()> {
            self.calls
                .push(format!("append {folder} {} bytes", raw.len()));
            self.fail_next.take().map_or(Ok(()), Err)?;
            let Some(list) = self.mail.get_mut(folder) else {
                return Err(MailError::Protocol(format!("no folder {folder}")));
            };
            let uid = *self.next_uid.get(folder).unwrap_or(&1);
            let end = raw
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|i| i + 4)
                .unwrap_or(raw.len());
            list.push(Envelope {
                uid,
                flags: flags.iter().map(|f| f.to_string()).collect(),
                internaldate: 0,
                size: Some(raw.len() as u32),
                headers: raw[..end].to_vec(),
            });
            self.raw.insert((folder.into(), uid), raw.to_vec());
            self.next_uid.insert(folder.into(), uid + 1);
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
    fn fake_search_uids_mimics_star_semantics() {
        let mut ops = RecordingOps::new().with_folder("INBOX", None);
        ops.add_mail("INBOX", 3, 10, "Subject: a\r\n\r\n", None);
        ops.add_mail("INBOX", 9, 20, "Subject: b\r\n\r\n", None);
        ops.select("INBOX").unwrap();
        assert_eq!(ops.search_uids(1).unwrap(), [3, 9]);
        assert_eq!(ops.search_uids(4).unwrap(), [9]);
        // Real servers answer `N:*` with the highest message when N exceeds it.
        assert_eq!(ops.search_uids(50).unwrap(), [9]);
        assert_eq!(ops.calls.last().unwrap(), "search_uids INBOX 50");
    }

    #[test]
    fn fake_fetch_envelopes_returns_the_range_and_can_fail_later() {
        let mut ops = RecordingOps::new().with_folder("INBOX", None);
        for uid in [1, 5, 9] {
            ops.add_mail("INBOX", uid, 10, "Subject: a\r\n\r\n", None);
        }
        ops.select("INBOX").unwrap();
        ops.fail_fetch_after = Some(1);
        let uids: Vec<u32> = ops
            .fetch_envelopes(2, 9)
            .unwrap()
            .iter()
            .map(|e| e.uid)
            .collect();
        assert_eq!(uids, [5, 9]);
        assert_eq!(ops.calls.last().unwrap(), "fetch_envelopes INBOX 2 9");
        assert!(matches!(ops.fetch_envelopes(1, 1), Err(MailError::Io(_))));
    }

    #[test]
    fn fake_move_moves_the_message_and_records_call() {
        let mut ops = RecordingOps::new()
            .with_folder("INBOX", None)
            .with_folder("Archive", Some("Archive"));
        ops.add_mail("INBOX", 1, 10, "Subject: a\r\n\r\n", Some("raw"));
        ops.select("INBOX").unwrap();
        assert_eq!(ops.move_message(1, "Archive").unwrap(), None);
        assert!(ops.mail["INBOX"].is_empty());
        assert_eq!(ops.raw.get(&("Archive".into(), 1)).unwrap(), b"raw");
        assert_eq!(ops.calls.last().unwrap(), "move INBOX 1 -> Archive");
    }

    #[test]
    fn fake_expunge_requires_deleted_flag() {
        let mut ops = RecordingOps::new().with_folder("INBOX", None);
        ops.add_mail("INBOX", 1, 10, "Subject: a\r\n\r\n", Some("raw"));
        ops.select("INBOX").unwrap();
        ops.expunge(1).unwrap();
        assert_eq!(ops.mail["INBOX"].len(), 1);
        assert!(ops.raw.contains_key(&("INBOX".into(), 1)));
        ops.add_flags(1, &["\\Deleted"]).unwrap();
        ops.expunge(1).unwrap();
        assert!(ops.mail["INBOX"].is_empty());
        assert!(ops.raw.is_empty());
        assert_eq!(ops.calls.last().unwrap(), "expunge INBOX 1");
    }

    #[test]
    fn fake_append_keeps_bytes_and_rejects_unknown_folder() {
        let mut ops = RecordingOps::new().with_folder("Backup", None);
        let raw = b"Subject: a\r\n\r\n\xFFbody";
        ops.append("Backup", raw, &["$PostbodeRestored"]).unwrap();
        assert_eq!(ops.raw[&("Backup".into(), 1)], raw);
        assert_eq!(ops.mail["Backup"][0].headers, b"Subject: a\r\n\r\n");
        assert_eq!(ops.mail["Backup"][0].flags, ["$PostbodeRestored"]);
        assert!(matches!(
            ops.append("Nope", raw, &[]),
            Err(MailError::Protocol(_))
        ));
    }
}
