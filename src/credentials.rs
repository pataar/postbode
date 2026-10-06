use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use zeroize::Zeroizing;

use crate::config::{AccountConfig, PasswordSource};

const SERVICE: &str = "postbode";

pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: String) -> Secret {
        Secret(Zeroizing::new(value))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("no password stored for account '{0}'; run `postbode account add`")]
    NotFound(String),
    #[error("the keyring is locked or denied access")]
    Locked,
    #[error("no keyring is available on this system; use `password = {{ command = \"...\" }}`")]
    Unavailable,
    #[error("password command for account '{account}' exited with status {status}")]
    CommandFailed { account: String, status: i32 },
    #[error("password command for account '{0}' printed nothing")]
    CommandEmpty(String),
    #[error("password command could not be started: {0}")]
    CommandSpawn(String),
}

pub fn resolve(account: &AccountConfig) -> Result<Secret, CredentialError> {
    match &account.password {
        PasswordSource::Command { command } => run_command(&account.name, command),
        PasswordSource::Keyring { .. } => {
            let entry = keyring_entry(&account.name)?;
            let name = account.name.clone();
            // macOS asks again for Keychain access after each new unsigned binary, and the prompt may sit behind other windows.
            let password = hint_if_slow(
                Duration::from_secs(2),
                move || {
                    log::info!(
                        "{name}: waiting for the OS keyring; answer its password prompt if one is showing"
                    )
                },
                || entry.get_password(),
            );
            match password {
                Ok(password) => Ok(Secret::new(password)),
                Err(e) => Err(map_keyring_error(e, &account.name)),
            }
        }
    }
}

/// Runs `work`, calling `hint` once it has taken longer than `delay`.
fn hint_if_slow<T>(
    delay: Duration,
    hint: impl FnOnce() + Send + 'static,
    work: impl FnOnce() -> T,
) -> T {
    let (done, waiting) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if waiting.recv_timeout(delay) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
            hint();
        }
    });
    let result = work();
    drop(done);
    result
}

pub fn store(account_name: &str, secret: &Secret) -> Result<(), CredentialError> {
    let entry = keyring_entry(account_name)?;
    entry
        .set_password(secret.expose())
        .map_err(|e| map_keyring_error(e, account_name))
}

fn run_command(account_name: &str, command: &str) -> Result<Secret, CredentialError> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|e| CredentialError::CommandSpawn(e.to_string()))?;
    if !output.status.success() {
        return Err(CredentialError::CommandFailed {
            account: account_name.to_string(),
            status: output.status.code().unwrap_or(-1),
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let trimmed = text.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return Err(CredentialError::CommandEmpty(account_name.to_string()));
    }
    Ok(Secret::new(trimmed.to_string()))
}

fn keyring_entry(account_name: &str) -> Result<keyring_core::Entry, CredentialError> {
    static STORE: OnceLock<Result<(), String>> = OnceLock::new();
    let init = STORE.get_or_init(|| platform_store().map(keyring_core::set_default_store));
    if let Err(detail) = init {
        log::debug!("keyring store init failed: {detail}");
        return Err(CredentialError::Unavailable);
    }
    keyring_core::Entry::new(SERVICE, account_name).map_err(|e| map_keyring_error(e, account_name))
}

#[cfg(target_os = "macos")]
fn platform_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>, String> {
    apple_native_keyring_store::keychain::Store::new()
        .map(|s| s as std::sync::Arc<keyring_core::CredentialStore>)
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "linux")]
fn platform_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>, String> {
    zbus_secret_service_keyring_store::Store::new()
        .map(|s| s as std::sync::Arc<keyring_core::CredentialStore>)
        .map_err(|e| e.to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_store() -> Result<std::sync::Arc<keyring_core::CredentialStore>, String> {
    Err("no keyring store for this platform".to_string())
}

fn map_keyring_error(e: keyring_core::Error, account_name: &str) -> CredentialError {
    log::debug!("keyring error for '{account_name}': {e:?}");
    match e {
        keyring_core::Error::NoEntry => CredentialError::NotFound(account_name.to_string()),
        keyring_core::Error::NoStorageAccess(_) => CredentialError::Locked,
        _ => CredentialError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(command: &str) -> AccountConfig {
        AccountConfig {
            name: "t".into(),
            host: "h".into(),
            port: 993,
            username: "u@example.com".into(),
            password: PasswordSource::Command {
                command: command.into(),
            },
            address: None,
            aliases: vec![],
            sync_interval_secs: 120,
            trash_retention_days: 30,
            notify: true,
            ca_file: None,
        }
    }

    #[test]
    fn hint_if_slow_fires_only_for_slow_work() {
        use std::sync::mpsc;
        use std::time::Duration;

        let (tx, rx) = mpsc::channel();
        let slow = hint_if_slow(
            Duration::from_millis(20),
            move || tx.send("slow").unwrap(),
            || {
                std::thread::sleep(Duration::from_millis(200));
                1
            },
        );
        assert_eq!(slow, 1);
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)), Ok("slow"));

        let (tx, rx) = mpsc::channel();
        assert_eq!(
            hint_if_slow(
                Duration::from_millis(500),
                move || tx.send("fast").unwrap(),
                || 2
            ),
            2
        );
        assert!(rx.recv_timeout(Duration::from_millis(800)).is_err());
    }

    #[test]
    fn command_output_is_trimmed() {
        let secret = resolve(&account("printf 'hunter2\\n'")).unwrap();
        assert_eq!(secret.expose(), "hunter2");
    }

    #[test]
    fn empty_command_output_is_error() {
        assert!(matches!(
            resolve(&account("true")),
            Err(CredentialError::CommandEmpty(_))
        ));
    }

    #[test]
    fn failing_command_reports_status() {
        assert!(matches!(
            resolve(&account("exit 3")),
            Err(CredentialError::CommandFailed { status: 3, .. })
        ));
    }
}
