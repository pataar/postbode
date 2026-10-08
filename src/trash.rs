use std::io;
use std::path::{Path, PathBuf};

use crate::mail_ops::{MailError, MailOps};
use crate::paths::write_atomic;
use crate::rules::engine::RESTORED_KEYWORD;

pub struct Trash {
    dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("{0} is not a backup in this account's trash")]
    NotInTrash(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Mail(#[from] MailError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrashEntry {
    pub path: PathBuf,
    pub folder: String,
    pub uid: u32,
    pub saved_at: i64,
}

impl Trash {
    pub fn new(dir: PathBuf) -> Trash {
        Trash { dir }
    }

    pub fn save(&self, folder: &str, uid: u32, raw: &[u8], now: i64) -> io::Result<PathBuf> {
        let encoded = folder.replace('%', "%25").replace('/', "%2F");
        let name = |at: i64| self.dir.join(format!("{at}-{encoded}-{uid}.eml"));
        // After a UIDVALIDITY change the same folder and uid name another message, which can be deleted within the same
        // second: never overwrite a backup, take the next free second instead.
        let mut at = now;
        while name(at).symlink_metadata().is_ok() {
            at += 1;
        }
        let path = name(at);
        write_atomic(&path, raw)?;
        Ok(path)
    }

    pub fn list(&self) -> io::Result<Vec<TrashEntry>> {
        let mut entries = Vec::new();
        let dir = match std::fs::read_dir(&self.dir) {
            Ok(d) => d,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(entries),
            Err(e) => return Err(e),
        };
        for entry in dir {
            let entry = entry?;
            let name = entry.file_name();
            let Some((saved_at, folder, uid)) = name.to_str().and_then(Trash::parse_name) else {
                continue;
            };
            entries.push(TrashEntry {
                path: entry.path(),
                folder,
                uid,
                saved_at,
            });
        }
        entries.sort_by(|a, b| b.saved_at.cmp(&a.saved_at).then(b.uid.cmp(&a.uid)));
        Ok(entries)
    }

    pub fn purge(&self, retention_secs: i64, now: i64) -> io::Result<usize> {
        let mut removed = 0;
        for entry in self.list()? {
            if now - entry.saved_at <= retention_secs {
                continue;
            }
            match std::fs::remove_file(&entry.path) {
                Ok(()) => removed += 1,
                // A concurrent `trash restore` may have taken it already.
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => log::warn!("{}: could not purge: {e}", entry.path.display()),
            }
        }
        Ok(removed)
    }

    /// Appends the backup to its original folder with the restored keyword, then removes the file. Returns the folder.
    pub fn restore(&self, ops: &mut dyn MailOps, file: &Path) -> Result<String, RestoreError> {
        let not_in_trash = || RestoreError::NotInTrash(file.display().to_string());
        let file = file.canonicalize().map_err(|_| not_in_trash())?;
        let in_trash = self.dir.canonicalize().ok().as_deref() == file.parent();
        let parsed = file
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(Trash::parse_name);
        let (Some((_, folder, _)), true) = (parsed, in_trash) else {
            return Err(not_in_trash());
        };
        let raw = std::fs::read(&file)?;
        ops.append(&folder, &raw, &[RESTORED_KEYWORD])?;
        std::fs::remove_file(&file)?;
        Ok(folder)
    }

    pub fn parse_name(name: &str) -> Option<(i64, String, u32)> {
        let stem = name.strip_suffix(".eml")?;
        let (ts, rest) = stem.split_once('-')?;
        let (encoded, uid) = rest.rsplit_once('-')?;
        let folder = encoded.replace("%2F", "/").replace("%25", "%");
        Some((ts.parse().ok()?, folder, uid.parse().ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_list_and_parse_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let trash = Trash::new(dir.path().join("trash"));
        let path = trash.save("Lists/GitHub", 7, b"raw", 1000).unwrap();
        assert_eq!(path.file_name().unwrap(), "1000-Lists%2FGitHub-7.eml");
        assert_eq!(std::fs::read(&path).unwrap(), b"raw");
        trash.save("INBOX", 3, b"raw2", 2000).unwrap();
        let list = trash.list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(
            (list[0].saved_at, list[0].folder.as_str(), list[0].uid),
            (2000, "INBOX", 3)
        );
        assert_eq!(list[1].folder, "Lists/GitHub");
    }

    /// Found by tests/sync_model.rs: a delete after a UIDVALIDITY reset overwrote the backup of another message.
    #[test]
    fn save_never_overwrites_a_backup_with_the_same_name() {
        let dir = tempfile::tempdir().unwrap();
        let trash = Trash::new(dir.path().to_path_buf());
        let first = trash.save("INBOX", 2, b"first message", 1000).unwrap();
        let second = trash.save("INBOX", 2, b"other message", 1000).unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read(&first).unwrap(), b"first message");
        assert_eq!(std::fs::read(&second).unwrap(), b"other message");
        assert_eq!(second.file_name().unwrap(), "1001-INBOX-2.eml");
    }

    #[test]
    fn purge_removes_only_old_files() {
        let dir = tempfile::tempdir().unwrap();
        let trash = Trash::new(dir.path().to_path_buf());
        trash.save("INBOX", 1, b"a", 1000).unwrap();
        trash.save("INBOX", 2, b"b", 5000).unwrap();
        assert_eq!(trash.purge(3000, 6000).unwrap(), 1);
        assert_eq!(trash.list().unwrap().len(), 1);
    }

    #[test]
    fn parse_name_rejects_garbage() {
        assert_eq!(Trash::parse_name("notes.txt"), None);
        assert_eq!(Trash::parse_name("12-INBOX-x.eml"), None);
        assert_eq!(
            Trash::parse_name("12-A%25B-4.eml"),
            Some((12, "A%B".into(), 4))
        );
    }

    #[test]
    fn purge_skips_what_it_cannot_remove_and_continues() {
        let dir = tempfile::tempdir().unwrap();
        let trash = Trash::new(dir.path().to_path_buf());
        // A directory with a backup's name cannot be removed with remove_file.
        std::fs::create_dir(dir.path().join("1000-INBOX-9.eml")).unwrap();
        trash.save("INBOX", 1, b"a", 1000).unwrap();
        assert_eq!(trash.purge(3000, 6000).unwrap(), 1);
        assert!(!dir.path().join("1000-INBOX-1.eml").exists());
    }
}
