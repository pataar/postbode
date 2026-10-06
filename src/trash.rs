use std::io;
use std::path::{Path, PathBuf};

use crate::paths::write_atomic;

pub struct Trash {
    dir: PathBuf,
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
        let path = self.dir.join(format!("{now}-{encoded}-{uid}.eml"));
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
            if now - entry.saved_at > retention_secs {
                std::fs::remove_file(&entry.path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub fn parse_name(name: &str) -> Option<(i64, String, u32)> {
        let stem = name.strip_suffix(".eml")?;
        let (ts, rest) = stem.split_once('-')?;
        let (encoded, uid) = rest.rsplit_once('-')?;
        let folder = encoded.replace("%2F", "/").replace("%25", "%");
        Some((ts.parse().ok()?, folder, uid.parse().ok()?))
    }
}

pub fn read_eml(path: &Path) -> io::Result<Vec<u8>> {
    std::fs::read(path)
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
}
