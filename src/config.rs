use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::write_atomic;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config.toml: {0}")]
    Parse(String),
    #[error("config.toml: {0}")]
    Invalid(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub accounts: Vec<AccountConfig>,
    #[serde(default, skip_serializing_if = "UiConfig::is_default")]
    pub ui: UiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountConfig {
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub username: String,
    pub password: PasswordSource,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default = "default_sync_interval")]
    pub sync_interval_secs: u64,
    #[serde(default = "default_retention")]
    pub trash_retention_days: u64,
    #[serde(default = "default_true")]
    pub notify: bool,
    /// Extra trusted root certificate (PEM), for servers with a private CA.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PasswordSource {
    Keyring { keyring: bool },
    Command { command: String },
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiConfig {
    #[serde(default)]
    pub theme: Theme,
}

impl UiConfig {
    fn is_default(&self) -> bool {
        *self == UiConfig::default()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Dark,
    Light,
    #[default]
    System,
}

impl Theme {
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
            Theme::System => "system",
        }
    }
}

fn default_port() -> u16 {
    993
}
fn default_sync_interval() -> u64 {
    120
}
fn default_retention() -> u64 {
    30
}
fn default_true() -> bool {
    true
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let cfg: Config = toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let text = toml::to_string_pretty(self).map_err(|e| ConfigError::Invalid(e.to_string()))?;
        write_atomic(path, text.as_bytes())?;
        Ok(())
    }

    pub fn account(&self, name: &str) -> Option<&AccountConfig> {
        self.accounts.iter().find(|a| a.name == name)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut seen = std::collections::HashSet::new();
        for account in &self.accounts {
            let name = &account.name;
            let safe = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if !safe {
                return Err(ConfigError::Invalid(format!(
                    "account name '{name}' may only contain letters, digits, '-' and '_'"
                )));
            }
            if !seen.insert(name) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate account name '{name}'"
                )));
            }
            if account.address.is_none() && !account.username.contains('@') {
                return Err(ConfigError::Invalid(format!(
                    "account '{name}': set `address` because the username is not an email address"
                )));
            }
            if let Some(ca_file) = &account.ca_file
                && !ca_file.is_absolute()
            {
                return Err(ConfigError::Invalid(format!(
                    "account '{name}': ca_file must be an absolute path"
                )));
            }
            account.identity()?;
        }
        Ok(())
    }
}

/// Sets `[ui] theme`, keeping the rest of the file's text and comments.
pub fn save_theme(path: &Path, theme: Theme) -> Result<(), ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| ConfigError::Parse(e.to_string()))?;
    let ui = doc
        .entry("ui")
        .or_insert_with(toml_edit::table)
        .as_table_like_mut()
        .ok_or_else(|| ConfigError::Invalid("`ui` must be a table".into()))?;
    ui.insert("theme", toml_edit::value(theme.as_str()));
    write_atomic(path, doc.to_string().as_bytes())?;
    Ok(())
}

impl AccountConfig {
    pub fn address(&self) -> &str {
        self.address.as_deref().unwrap_or(&self.username)
    }

    pub fn identity(&self) -> Result<Identity, ConfigError> {
        let mut builder = globset::GlobSetBuilder::new();
        for pattern in &self.aliases {
            let glob = globset::GlobBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .map_err(|e| {
                    ConfigError::Invalid(format!("account '{}': alias '{pattern}': {e}", self.name))
                })?;
            builder.add(glob);
        }
        let aliases = builder
            .build()
            .map_err(|e| ConfigError::Invalid(format!("account '{}': aliases: {e}", self.name)))?;
        Ok(Identity {
            address: self.address().to_ascii_lowercase(),
            aliases,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Identity {
    address: String,
    aliases: globset::GlobSet,
}

impl Identity {
    pub fn is_me(&self, addr: &str) -> bool {
        let addr = addr.trim().to_ascii_lowercase();
        addr == self.address || self.aliases.is_match(&addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[[accounts]]
name = "work"
host = "imap.example.com"
username = "pieter@example.com"
password = { keyring = true }
aliases = ["p@example.org", "*@shop.example.com"]

[[accounts]]
name = "home"
host = "mail.home.test"
port = 143
username = "pieter"
address = "pieter@home.test"
password = { command = "pass show mail/home" }
notify = false
"#;

    #[test]
    fn parses_accounts_with_defaults() {
        let cfg = Config::parse(SAMPLE).unwrap();
        let work = cfg.account("work").unwrap();
        assert_eq!(work.port, 993);
        assert_eq!(work.sync_interval_secs, 120);
        assert_eq!(work.trash_retention_days, 30);
        assert!(work.notify);
        assert!(matches!(
            work.password,
            PasswordSource::Keyring { keyring: true }
        ));
        let home = cfg.account("home").unwrap();
        assert_eq!(home.port, 143);
        assert!(!home.notify);
        assert!(
            matches!(&home.password, PasswordSource::Command { command } if command == "pass show mail/home")
        );
    }

    #[test]
    fn address_defaults_to_username_when_it_is_an_email() {
        let cfg = Config::parse(SAMPLE).unwrap();
        assert_eq!(cfg.account("work").unwrap().address(), "pieter@example.com");
        assert_eq!(cfg.account("home").unwrap().address(), "pieter@home.test");
    }

    #[test]
    fn identity_matches_address_and_aliases_case_insensitively() {
        let cfg = Config::parse(SAMPLE).unwrap();
        let id = cfg.account("work").unwrap().identity().unwrap();
        assert!(id.is_me("Pieter@Example.com"));
        assert!(id.is_me("p@example.org"));
        assert!(id.is_me("orders@shop.example.com"));
        assert!(!id.is_me("someone@example.com"));
    }

    #[test]
    fn rejects_duplicate_and_unsafe_names() {
        let dup = SAMPLE.replace("name = \"home\"", "name = \"work\"");
        assert!(matches!(Config::parse(&dup), Err(ConfigError::Invalid(_))));
        let bad = SAMPLE.replace("name = \"home\"", "name = \"ho/me\"");
        assert!(matches!(Config::parse(&bad), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn rejects_unknown_account_keys() {
        let typo = SAMPLE.replace("aliases = ", "alias = ");
        assert!(
            matches!(Config::parse(&typo), Err(ConfigError::Parse(e)) if e.contains("alias")),
            "a misspelled key must not be ignored"
        );
    }

    #[test]
    fn rejects_username_without_at_and_no_address() {
        let bad = SAMPLE.replace("address = \"pieter@home.test\"\n", "");
        assert!(matches!(Config::parse(&bad), Err(ConfigError::Invalid(_))));
    }

    #[test]
    fn relative_ca_file_is_rejected() {
        let text = "[[accounts]]\nname = \"self\"\nhost = \"localhost\"\nusername = \"me@example.com\"\npassword = { keyring = true }\nca_file = \"certs/ca.pem\"\n";
        let err = Config::parse(text).unwrap_err().to_string();
        assert!(err.contains("ca_file must be an absolute path"), "{err}");
        let absolute = text.replace("certs/ca.pem", "/etc/ssl/self/ca.pem");
        assert!(
            Config::parse(&absolute).unwrap().accounts[0]
                .ca_file
                .is_some()
        );
    }

    #[test]
    fn load_missing_file_is_empty_and_save_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let cfg = Config::load(&path).unwrap();
        assert!(cfg.accounts.is_empty());
        let cfg = Config::parse(SAMPLE).unwrap();
        cfg.save(&path).unwrap();
        let again = Config::load(&path).unwrap();
        assert_eq!(again.accounts.len(), 2);
    }

    #[test]
    fn ui_theme_defaults_to_system() {
        assert_eq!(Config::parse(SAMPLE).unwrap().ui.theme, Theme::System);
    }

    #[test]
    fn ui_theme_parses_and_rejects_unknown_values() {
        let dark = format!("{SAMPLE}\n[ui]\ntheme = \"dark\"\n");
        assert_eq!(Config::parse(&dark).unwrap().ui.theme, Theme::Dark);
        let blue = format!("{SAMPLE}\n[ui]\ntheme = \"blue\"\n");
        assert!(matches!(Config::parse(&blue), Err(ConfigError::Parse(_))));
        let typo = format!("{SAMPLE}\n[ui]\ntheme_ = \"dark\"\n");
        assert!(matches!(Config::parse(&typo), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn save_theme_keeps_the_rest_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = format!("# my accounts\n{SAMPLE}");
        std::fs::write(&path, &text).unwrap();
        save_theme(&path, Theme::Dark).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.starts_with(&text), "{saved}");
        assert!(saved.contains("[ui]\ntheme = \"dark\""), "{saved}");
        save_theme(&path, Theme::Light).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(
            saved.contains("theme = \"light\"") && !saved.contains("dark"),
            "{saved}"
        );
        let cfg = Config::load(&path).unwrap();
        assert_eq!((cfg.ui.theme, cfg.accounts.len()), (Theme::Light, 2));
    }

    #[test]
    fn save_round_trip_leaves_out_a_default_ui_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        Config::parse(SAMPLE).unwrap().save(&path).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("[ui]"));
    }
}
