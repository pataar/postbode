//! `postbode mcp install`: registers the server with Claude Desktop or Claude Code, or prints a snippet for other hosts.
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use super::parse_scopes;
use crate::paths::write_atomic_keeping_dir_mode;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    ClaudeCode,
    ClaudeDesktop,
    Json,
}

/// The command a host runs. The binary path is absolute: hosts started from the Dock do not inherit the shell's PATH.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub command: PathBuf,
    pub args: Vec<String>,
}

impl Entry {
    pub fn new(exe: &Path, scopes: &str, accounts: &[String]) -> Entry {
        let mut args = vec![
            "mcp".to_string(),
            "--scopes".to_string(),
            scopes.to_string(),
        ];
        for account in accounts {
            args.push("--account".to_string());
            args.push(account.clone());
        }
        Entry {
            command: exe.to_path_buf(),
            args,
        }
    }

    fn server(&self) -> Value {
        json!({ "command": self.command, "args": self.args })
    }

    fn argv(&self) -> Vec<String> {
        std::iter::once(self.command.display().to_string())
            .chain(self.args.iter().cloned())
            .collect()
    }

    /// The command quoted for a POSIX shell.
    pub fn shell(&self) -> String {
        shell_words(&self.argv())
    }
}

fn shell_words(words: &[String]) -> String {
    let quote = |w: &String| {
        if !w.is_empty()
            && w.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_./:,=@".contains(c))
        {
            w.clone()
        } else {
            format!("'{}'", w.replace('\'', r"'\''"))
        }
    };
    words.iter().map(quote).collect::<Vec<_>>().join(" ")
}

pub fn json_snippet(entry: &Entry) -> String {
    let snippet = json!({ "mcpServers": { "postbode": entry.server() } });
    serde_json::to_string_pretty(&snippet).expect("JSON values serialize") + "\n"
}

/// Claude Desktop's config: `~/Library/Application Support/Claude/` on macOS, `~/.config/Claude/` on Linux.
pub fn claude_desktop_path() -> Result<PathBuf> {
    let dirs = directories::BaseDirs::new().context("no home directory found")?;
    Ok(dirs
        .config_dir()
        .join("Claude")
        .join("claude_desktop_config.json"))
}

/// Sets `mcpServers.postbode`, or removes it when `entry` is None, keeping every other key.
/// Returns the new file text, or None when there is nothing to change. The old file is copied to `.bak`
/// unless it already is Postbode's own output, so the user's original survives re-runs. A file that is
/// not a JSON object is refused untouched; `dry_run` writes nothing.
pub fn edit_claude_desktop(
    path: &Path,
    entry: Option<&Entry>,
    dry_run: bool,
) -> Result<Option<String>> {
    let old = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    if old.is_none() && entry.is_none() {
        return Ok(None);
    }
    let mut config: Value = match old.as_deref() {
        Some(text) if !text.trim().is_empty() => serde_json::from_str(text).with_context(|| {
            format!(
                "{} is not valid JSON; fix it or move it away first",
                path.display()
            )
        })?,
        _ => json!({}),
    };
    let own_output = serde_json::to_string_pretty(&config)? + "\n";
    let Some(root) = config.as_object_mut() else {
        bail!("{} is not a JSON object", path.display());
    };
    match entry {
        Some(entry) => {
            let Some(servers) = root
                .entry("mcpServers")
                .or_insert_with(|| json!({}))
                .as_object_mut()
            else {
                bail!("mcpServers in {} is not a JSON object", path.display());
            };
            servers.insert("postbode".into(), entry.server());
        }
        None => match root.get_mut("mcpServers") {
            Some(servers) => {
                let Some(servers) = servers.as_object_mut() else {
                    bail!("mcpServers in {} is not a JSON object", path.display());
                };
                if servers.remove("postbode").is_none() {
                    return Ok(None);
                }
            }
            None => return Ok(None),
        },
    }
    let text = serde_json::to_string_pretty(&config)? + "\n";
    if !dry_run {
        if old.as_deref().is_some_and(|old| old != own_output) {
            std::fs::copy(path, path.with_extension("json.bak"))?;
        }
        write_atomic_keeping_dir_mode(path, text.as_bytes())?;
    }
    Ok(Some(text))
}

/// `claude` invocations at user scope; an add follows a remove so a re-run replaces the old entry.
pub fn claude_code_commands(entry: Option<&Entry>) -> Vec<Vec<String>> {
    let base = |verb: &str| {
        ["claude", "mcp", verb, "--scope", "user", "postbode"]
            .map(String::from)
            .to_vec()
    };
    let mut commands = vec![base("remove")];
    if let Some(entry) = entry {
        let mut add = base("add");
        add.push("--".into());
        add.extend(entry.argv());
        commands.push(add);
    }
    commands
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// Registers or removes the server for `target`. Returns the text for stdout and, when something was
/// installed, a hint for stderr so that stdout stays machine-readable.
pub fn install(
    target: Target,
    scopes: &str,
    accounts: &[String],
    remove: bool,
    dry_run: bool,
) -> Result<(String, Option<String>)> {
    let granted: Vec<&str> = parse_scopes(scopes)?
        .into_iter()
        .map(|s| s.as_str())
        .collect();
    let exe = std::env::current_exe().context("finding the postbode binary")?;
    let entry = Entry::new(&exe, &granted.join(","), accounts);
    let wanted = (!remove).then_some(&entry);
    let out = match target {
        Target::Json => json_snippet(&entry),
        Target::ClaudeDesktop => install_claude_desktop(wanted, &entry, dry_run)?,
        Target::ClaudeCode => install_claude_code(wanted, remove, dry_run)?,
    };
    let hint = (!remove).then(|| {
        format!(
            "Scopes: {}. For an inbox assistant: --scopes read,read:bodies,rules:propose,mail:modify\n",
            granted.join(", ")
        )
    });
    Ok((out, hint))
}

/// Real runs never echo the config: it may hold other servers' secrets.
fn install_claude_desktop(wanted: Option<&Entry>, entry: &Entry, dry_run: bool) -> Result<String> {
    let path = claude_desktop_path()?;
    let changed = edit_claude_desktop(&path, wanted, dry_run)?.is_some();
    let path = path.display();
    Ok(match (wanted.is_some(), changed, dry_run) {
        (false, false, _) => format!("nothing to remove in {path}\n"),
        (false, true, true) => format!("would remove mcpServers.postbode from {path}\n"),
        (false, true, false) => {
            format!("removed Postbode from {path}\nRestart Claude Desktop to apply.\n")
        }
        (true, _, true) => format!("would write to {path}:\n{}", json_snippet(entry)),
        (true, _, false) => {
            format!("updated {path}\nRestart Claude Desktop to load Postbode.\n")
        }
    })
}

fn install_claude_code(wanted: Option<&Entry>, remove: bool, dry_run: bool) -> Result<String> {
    let commands = claude_code_commands(wanted);
    if dry_run || !on_path("claude") {
        let mut out = String::from(if dry_run {
            "would run:\n"
        } else {
            "claude is not on PATH; run:\n"
        });
        for command in &commands {
            out.push_str(&format!("  {}\n", shell_words(command)));
        }
        return Ok(out);
    }
    for (index, command) in commands.iter().enumerate() {
        let status = std::process::Command::new(&command[0])
            .args(&command[1..])
            .status()
            .with_context(|| format!("running {}", shell_words(command)))?;
        // In an install the leading remove clears an entry that may not exist; every other command must succeed.
        if (remove || index > 0) && !status.success() {
            bail!("`{}` failed", shell_words(command));
        }
    }
    Ok(if remove {
        "removed from Claude Code\n".into()
    } else {
        "registered with Claude Code at user scope\n".into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Entry {
        Entry::new(
            Path::new("/opt/homebrew/bin/postbode"),
            "read,rules:propose",
            &["work".into()],
        )
    }

    #[test]
    fn snippet_names_the_binary_and_its_arguments() {
        let snippet: serde_json::Value = serde_json::from_str(&json_snippet(&entry())).unwrap();
        assert_eq!(
            snippet["mcpServers"]["postbode"]["command"],
            "/opt/homebrew/bin/postbode"
        );
        assert_eq!(
            snippet["mcpServers"]["postbode"]["args"],
            serde_json::json!(["mcp", "--scopes", "read,rules:propose", "--account", "work"])
        );
    }

    #[test]
    fn desktop_config_is_created_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Claude/claude_desktop_config.json");
        edit_claude_desktop(&path, Some(&entry()), false).unwrap();
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(config["mcpServers"]["postbode"]["args"][0], "mcp");
    }

    #[test]
    fn other_servers_and_keys_survive_and_a_backup_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude_desktop_config.json");
        let old = r#"{"theme":"dark","mcpServers":{"other":{"command":"x"}}}"#;
        std::fs::write(&path, old).unwrap();
        edit_claude_desktop(&path, Some(&entry()), false).unwrap();
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(config["theme"], "dark");
        assert_eq!(config["mcpServers"]["other"]["command"], "x");
        assert!(config["mcpServers"]["postbode"].is_object());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("claude_desktop_config.json.bak")).unwrap(),
            old
        );
    }

    #[test]
    fn invalid_json_is_refused_and_nothing_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude_desktop_config.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(edit_claude_desktop(&path, Some(&entry()), false).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        assert!(!dir.path().join("claude_desktop_config.json.bak").exists());
    }

    fn read_json(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn remove_keeps_other_servers_and_never_adds_an_empty_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude_desktop_config.json");
        std::fs::write(
            &path,
            r#"{"mcpServers":{"other":{"command":"x"},"postbode":{"command":"p"}}}"#,
        )
        .unwrap();
        assert!(edit_claude_desktop(&path, None, false).unwrap().is_some());
        let config = read_json(&path);
        assert!(config["mcpServers"].get("postbode").is_none());
        assert_eq!(config["mcpServers"]["other"]["command"], "x");

        let bare = dir.path().join("bare.json");
        std::fs::write(&bare, r#"{"theme":"dark"}"#).unwrap();
        assert!(edit_claude_desktop(&bare, None, false).unwrap().is_none());
        assert_eq!(
            std::fs::read_to_string(&bare).unwrap(),
            r#"{"theme":"dark"}"#
        );

        let missing = dir.path().join("none.json");
        assert!(
            edit_claude_desktop(&missing, None, false)
                .unwrap()
                .is_none()
        );
        assert!(!missing.exists());
    }

    #[test]
    fn dry_run_writes_neither_config_nor_backup() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("none.json");
        let preview = edit_claude_desktop(&missing, Some(&entry()), true)
            .unwrap()
            .unwrap();
        assert!(preview.contains("postbode") && !missing.exists());

        let path = dir.path().join("claude_desktop_config.json");
        let old = r#"{"theme":"dark"}"#;
        std::fs::write(&path, old).unwrap();
        edit_claude_desktop(&path, Some(&entry()), true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), old);
        assert!(!dir.path().join("claude_desktop_config.json.bak").exists());
    }

    #[test]
    fn the_original_backup_survives_a_re_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude_desktop_config.json");
        let bak = dir.path().join("claude_desktop_config.json.bak");
        let original = r#"{"theme":"dark"}"#;
        std::fs::write(&path, original).unwrap();
        edit_claude_desktop(&path, Some(&entry()), false).unwrap();
        let wider = Entry::new(
            Path::new("/opt/homebrew/bin/postbode"),
            "read,read:bodies",
            &[],
        );
        edit_claude_desktop(&path, Some(&wider), false).unwrap();
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), original);
        assert_eq!(
            read_json(&path)["mcpServers"]["postbode"]["args"][2],
            "read,read:bodies"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_backup_keeps_the_config_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude_desktop_config.json");
        std::fs::write(&path, "{}\n ").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        edit_claude_desktop(&path, Some(&entry()), false).unwrap();
        let bak = dir.path().join("claude_desktop_config.json.bak");
        assert_eq!(
            std::fs::metadata(bak).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_config_folder_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Claude");
        std::fs::create_dir(&folder).unwrap();
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        edit_claude_desktop(
            &folder.join("claude_desktop_config.json"),
            Some(&entry()),
            false,
        )
        .unwrap();
        assert_eq!(
            std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn claude_code_replaces_an_existing_entry() {
        let commands = claude_code_commands(Some(&entry()));
        assert_eq!(
            commands[0],
            ["claude", "mcp", "remove", "--scope", "user", "postbode"]
        );
        assert_eq!(
            &commands[1][..7],
            ["claude", "mcp", "add", "--scope", "user", "postbode", "--"]
        );
        assert_eq!(commands[1][7], "/opt/homebrew/bin/postbode");
        assert_eq!(claude_code_commands(None).len(), 1);
    }

    #[test]
    fn shell_quoting_survives_spaces() {
        let e = Entry::new(Path::new("/Users/me/My Tools/postbode"), "read", &[]);
        assert!(e.shell().starts_with("'/Users/me/My Tools/postbode' mcp"));
    }

    #[test]
    fn install_json_validates_scopes_and_lists_them() {
        let (text, hint) = install(Target::Json, "read,rules:propose", &[], false, false).unwrap();
        assert!(text.contains("mcpServers") && serde_json::from_str::<Value>(&text).is_ok());
        assert!(hint.unwrap().contains("read, rules:propose"));
        let (_, removed_hint) = install(Target::Json, "read", &[], true, false).unwrap();
        assert!(removed_hint.is_none());
        assert!(install(Target::Json, "read,everything", &[], false, false).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_config_is_written_through_and_stays_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("dotfiles.json");
        std::fs::write(&target, "{}").unwrap();
        let link = dir.path().join("claude_desktop_config.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        edit_claude_desktop(&link, Some(&entry()), false).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            std::fs::read_to_string(&target)
                .unwrap()
                .contains("postbode")
        );
    }
}
