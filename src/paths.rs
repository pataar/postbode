use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    /// The root given to `under` (`POSTBODE_HOME`); None for the platform directories.
    pub home: Option<PathBuf>,
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub cache_dir: PathBuf,
}

impl Paths {
    pub fn discover() -> anyhow::Result<Paths> {
        let dirs = directories::ProjectDirs::from("", "", "postbode")
            .ok_or_else(|| anyhow::anyhow!("no home directory found"))?;
        let state_dir = dirs
            .state_dir()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| dirs.data_local_dir().to_path_buf());
        Ok(Paths {
            home: None,
            config_dir: dirs.config_dir().to_path_buf(),
            state_dir,
            cache_dir: dirs.cache_dir().to_path_buf(),
        })
    }

    pub fn under(root: &Path) -> Paths {
        Paths {
            home: Some(root.to_path_buf()),
            config_dir: root.join("config"),
            state_dir: root.join("state"),
            cache_dir: root.join("cache"),
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn rules_file(&self) -> PathBuf {
        self.config_dir.join("rules.toml")
    }

    pub fn daemon_socket(&self) -> PathBuf {
        self.state_dir.join("daemon.sock")
    }

    pub fn daemon_lock(&self) -> PathBuf {
        self.state_dir.join("daemon.lock")
    }

    pub fn daemon_log(&self) -> PathBuf {
        self.state_dir.join("daemon.log")
    }

    pub fn account_dir(&self, name: &str) -> PathBuf {
        self.state_dir.join("accounts").join(name)
    }

    pub fn mail_db(&self, name: &str) -> PathBuf {
        self.account_dir(name).join("mail.db")
    }

    pub fn trash_dir(&self, name: &str) -> PathBuf {
        self.account_dir(name).join("trash")
    }

    pub fn ensure_account(&self, name: &str) -> io::Result<()> {
        create_private_dir(&self.config_dir)?;
        create_private_dir(&self.cache_dir)?;
        create_private_dir(&self.account_dir(name))?;
        create_private_dir(&self.trash_dir(name))
    }
}

pub(crate) fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::fs::DirBuilder;
        use std::os::unix::fs::DirBuilderExt;
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)?;
    }
    Ok(())
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_in(path, bytes, true)
}

/// For a folder another app owns: an existing parent keeps its mode, a missing one is created private.
pub fn write_atomic_keeping_dir_mode(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_in(path, bytes, false)
}

fn write_atomic_in(path: &Path, bytes: &[u8], private_dir: bool) -> io::Result<()> {
    // A symlink (say into a dotfiles repo) is written through, not replaced; a dangling one fails in canonicalize.
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return write_file_atomic(&fs::canonicalize(path)?, bytes);
    }
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent"))?;
    if private_dir || !dir.exists() {
        create_private_dir(dir)?;
    }
    write_file_atomic(path, bytes)
}

fn write_file_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;

    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent"))?;
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    let mut file = fs::File::create(&tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&tmp, path)?;
    // The rename lives in the directory entry; without this a crash can bring back the old file or none.
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_root_lays_out_three_dirs() {
        let root = tempfile::tempdir().unwrap();
        let p = Paths::under(root.path());
        assert_eq!(p.config_file(), root.path().join("config/config.toml"));
        assert_eq!(p.rules_file(), root.path().join("config/rules.toml"));
        assert_eq!(
            p.mail_db("work"),
            root.path().join("state/accounts/work/mail.db")
        );
        assert_eq!(
            p.trash_dir("work"),
            root.path().join("state/accounts/work/trash")
        );
    }

    #[test]
    fn ensure_account_creates_dirs() {
        let root = tempfile::tempdir().unwrap();
        let p = Paths::under(root.path());
        p.ensure_account("work").unwrap();
        assert!(p.trash_dir("work").is_dir());
        assert!(p.config_dir.is_dir());
    }

    #[test]
    fn write_atomic_replaces_content_and_leaves_no_tmp() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("a.txt");
        write_atomic(&file, b"one").unwrap();
        write_atomic(&file, b"two").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"two");
        let leftovers: Vec<_> = fs::read_dir(root.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_creates_private_dir_and_file() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config").join("config.toml");
        write_atomic(&path, b"x").unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    #[cfg(unix)]
    fn ensure_account_dirs_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let p = Paths::under(root.path());
        p.ensure_account("work").unwrap();

        let check_mode = |dir: &Path| {
            let mode = fs::metadata(dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "directory {:?} is not 0700", dir);
        };

        check_mode(&p.state_dir);
        check_mode(&p.state_dir.join("accounts"));
        check_mode(&p.account_dir("work"));
        check_mode(&p.trash_dir("work"));
    }

    #[test]
    fn ensure_account_is_idempotent_across_accounts() {
        let root = tempfile::tempdir().unwrap();
        let p = Paths::under(root.path());
        p.ensure_account("work").unwrap();
        p.ensure_account("work").unwrap();
        p.ensure_account("personal").unwrap();
        assert!(p.trash_dir("work").is_dir());
        assert!(p.trash_dir("personal").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_writes_through_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("dotfiles/rules.toml");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "old").unwrap();
        let link = root.path().join("config/rules.toml");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, b"new").unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_refuses_a_dangling_symlink() {
        let root = tempfile::tempdir().unwrap();
        let link = root.path().join("rules.toml");
        std::os::unix::fs::symlink(root.path().join("missing/rules.toml"), &link).unwrap();
        assert!(write_atomic(&link, b"new").is_err());
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
