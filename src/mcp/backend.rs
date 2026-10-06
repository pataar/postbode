//! The only MCP code that touches the store, rules.toml or IMAP; its internals switch to the daemon socket later.
use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::config::{AccountConfig, Config};
use crate::message;
use crate::output;
use crate::paths::Paths;
use crate::store::{Message, Store};
use crate::trash::Trash;

pub const MAX_LIMIT: u32 = 500;

pub struct Backend {
    accounts: Vec<AccountConfig>,
    paths: Paths,
}

impl Backend {
    /// The accounts in `only`, or all of them; an unknown name, or no account at all, is an error.
    pub fn new(config: &Config, paths: &Paths, only: &[String]) -> Result<Backend> {
        if config.accounts.is_empty() {
            bail!("no accounts configured; run `postbode account add`");
        }
        if let Some(name) = only.iter().find(|n| config.account(n).is_none()) {
            bail!("no account named '{name}'");
        }
        Ok(Backend {
            accounts: config
                .accounts
                .iter()
                .filter(|a| only.is_empty() || only.contains(&a.name))
                .cloned()
                .collect(),
            paths: paths.clone(),
        })
    }

    /// The named visible account, or every visible one.
    fn select(&self, name: Option<&str>) -> Result<Vec<&AccountConfig>> {
        match name {
            Some(name) => Ok(vec![
                self.accounts
                    .iter()
                    .find(|a| a.name == name)
                    .with_context(|| format!("no account named '{name}'"))?,
            ]),
            None => Ok(self.accounts.iter().collect()),
        }
    }

    /// One account: the named one, or the only visible one.
    #[expect(dead_code, reason = "the single-message tools use it")]
    fn single(&self, name: Option<&str>) -> Result<&AccountConfig> {
        match (name, self.accounts.as_slice()) {
            (Some(_), _) => Ok(self.select(name)?[0]),
            (None, [only]) => Ok(only),
            (None, _) => bail!("several accounts are visible; pass account"),
        }
    }

    fn store(&self, account: &AccountConfig) -> Result<Store> {
        self.paths.ensure_account(&account.name)?;
        Ok(Store::open(&self.paths.mail_db(&account.name))?)
    }

    pub fn folders(&self, account: Option<&str>) -> Result<Vec<Value>> {
        let mut rows = Vec::new();
        for acc in self.select(account)? {
            let store = self.store(acc)?;
            for f in store.folders()? {
                rows.push(output::folder(
                    &acc.name,
                    &f,
                    store.message_count(&f.name)?,
                    store.unread_count(&f.name)?,
                ));
            }
        }
        Ok(rows)
    }

    pub fn list(
        &self,
        account: Option<&str>,
        folder: &str,
        limit: u32,
        threads: bool,
    ) -> Result<Vec<Value>> {
        let limit = limit.min(MAX_LIMIT);
        let mut rows = Vec::new();
        for acc in self.select(account)? {
            let store = self.store(acc)?;
            if threads {
                for thread in store.threads(folder, limit)? {
                    for (m, depth) in thread.iter().zip(output::depths(&thread)) {
                        let mut row = message_row(&acc.name, m)?;
                        row["depth"] = depth.into();
                        rows.push(row);
                    }
                }
            } else {
                for m in store.messages(folder, limit)? {
                    rows.push(message_row(&acc.name, &m)?);
                }
            }
        }
        Ok(rows)
    }

    /// `bodies` searches stored body text too; without it the query matches subject and addresses as plain words.
    pub fn search(
        &self,
        query: &str,
        account: Option<&str>,
        folder: Option<&str>,
        limit: u32,
        bodies: bool,
    ) -> Result<Vec<Value>> {
        let limit = limit.min(MAX_LIMIT);
        let mut rows = Vec::new();
        for acc in self.select(account)? {
            let store = self.store(acc)?;
            let found = if bodies {
                store.search(query, folder, limit)?
            } else {
                store.search_headers(query, folder, limit)?
            };
            for m in &found {
                rows.push(message_row(&acc.name, m)?);
            }
        }
        Ok(rows)
    }

    pub fn log(&self, account: Option<&str>, limit: u32) -> Result<Vec<Value>> {
        let mut rows = Vec::new();
        for acc in self.select(account)? {
            for entry in self.store(acc)?.log(limit.min(MAX_LIMIT))? {
                rows.push(output::with_account(&acc.name, &entry)?);
            }
        }
        Ok(rows)
    }

    /// Each backup's `file` is its bare name, which `trash_restore` takes.
    pub fn trash_list(&self, account: Option<&str>) -> Result<Vec<Value>> {
        let mut rows = Vec::new();
        for acc in self.select(account)? {
            for e in Trash::new(self.paths.trash_dir(&acc.name)).list()? {
                let subject = std::fs::read(&e.path)
                    .ok()
                    .and_then(|raw| message::parse_headers(&raw).subject);
                let file = e.path.file_name().map(|n| n.to_string_lossy().into_owned());
                rows.push(json!({ "account": acc.name, "saved_at": e.saved_at, "folder": e.folder, "uid": e.uid, "subject": subject, "file": file }));
            }
        }
        Ok(rows)
    }
}

/// A message's listing row: the CLI's `--json` shape with its account, without the body.
fn message_row(account: &str, m: &Message) -> Result<Value> {
    let mut row = output::with_account(account, m)?;
    if let Some(object) = row.as_object_mut() {
        object.remove("body_text");
    }
    Ok(row)
}
