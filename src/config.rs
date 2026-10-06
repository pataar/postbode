use std::io;
use std::path::Path;

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PasswordSource {
    Keyring { keyring: bool },
    Command { command: String },
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
            account.identity()?;
        }
        Ok(())
    }
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
}
