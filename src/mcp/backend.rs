//! The only MCP code that touches the store, rules.toml or IMAP; its internals switch to the daemon socket later.
use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::actions;
use crate::config::{AccountConfig, Config};
use crate::message;
use crate::output;
use crate::paths::Paths;
use crate::rules::{self, Rule, RuleFile};
use crate::store::{Message, Store};
use crate::sync;
use crate::trash::Trash;

pub const MAX_LIMIT: u32 = 500;

pub struct Backend {
    /// Every configured account, visible or not: an approved rule's clock restarts in all of them.
    all_accounts: Vec<String>,
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
            all_accounts: config.accounts.iter().map(|a| a.name.clone()).collect(),
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

    fn message(
        &self,
        account: Option<&str>,
        folder: &str,
        uid: u32,
    ) -> Result<(&AccountConfig, Store, Message)> {
        let acc = self.single(account)?;
        let store = self.store(acc)?;
        let msg = store
            .message(folder, uid)?
            .with_context(|| format!("no message {folder}/{uid}"))?;
        Ok((acc, store, msg))
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

    /// The message row and its plain body text, fetched once if not stored yet.
    pub fn show(&self, account: Option<&str>, folder: &str, uid: u32) -> Result<(Value, String)> {
        let (acc, store, msg) = self.message(account, folder, uid)?;
        let raw = actions::message_raw(acc, &store, &msg)?;
        Ok((message_row(&acc.name, &msg)?, message::body_text(&raw)))
    }

    pub fn attachments(&self, account: Option<&str>, folder: &str, uid: u32) -> Result<Vec<Value>> {
        let (acc, store, msg) = self.message(account, folder, uid)?;
        let raw = actions::message_raw(acc, &store, &msg)?;
        message::attachments(&raw)
            .iter()
            .map(|a| Ok(output::with_account(&acc.name, a)?))
            .collect()
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

    fn is_visible(&self, account: Option<&str>) -> bool {
        account.is_none_or(|a| self.accounts.iter().any(|acc| acc.name == a))
    }

    /// A rule scoped to a hidden account is as good as absent; an unknown name is left to the edit's own error.
    fn ensure_rule_visible(&self, name: &str) -> Result<()> {
        let file = rules::load(&self.paths.rules_file())?;
        match file.rules.iter().find(|r| r.name == name) {
            Some(rule) if !self.is_visible(rule.account.as_deref()) => {
                bail!("no rule named '{name}'")
            }
            _ => Ok(()),
        }
    }

    /// Rules scoped to a hidden account are left out; rules for every account stay.
    pub fn rules_list(&self) -> Result<Vec<Value>> {
        let file = rules::load(&self.paths.rules_file())?;
        Ok(file
            .rules
            .iter()
            .filter(|r| self.is_visible(r.account.as_deref()))
            .map(output::rule)
            .collect())
    }

    pub fn rules_check(&self) -> Result<Value> {
        let count = rules::compile(&rules::load(&self.paths.rules_file())?)?.len();
        Ok(json!({ "rules": count }))
    }

    pub fn rules_schema(&self) -> Result<Value> {
        Ok(serde_json::from_str(&rules::schema())?)
    }

    /// Previews one rule as if enabled, on the cached mail of the visible accounts; rows never carry mail text.
    pub fn rules_test(&self, mut rule: Rule, account: Option<&str>) -> Result<Vec<Value>> {
        rule.enabled = true;
        let compiled = rules::compile(&RuleFile { rules: vec![rule] })?;
        let mut rows = Vec::new();
        for acc in self.select(account)? {
            let store = self.store(acc)?;
            for p in actions::planned(&compiled, &store, acc, &acc.identity()?, sync::now())? {
                rows.push(json!({
                    "account": acc.name, "rule": p.rule, "folder": p.message.folder,
                    "uid": p.message.uid, "action": p.action.label(), "subject": p.message.subject,
                }));
            }
        }
        Ok(rows)
    }

    pub fn propose(&self, rule: Rule, by: &str) -> Result<Value> {
        if let Some(account) = rule
            .account
            .as_deref()
            .filter(|a| !self.is_visible(Some(a)))
        {
            bail!("no account named '{account}'");
        }
        let name = rule.name.clone();
        rules::edit::propose(&self.paths.rules_file(), rule, by)?;
        Ok(json!({ "proposed": name, "enabled": false, "proposed_by": by }))
    }

    /// Enables the rule and restarts its clock in every account, so it acts only on mail that arrives from now on.
    pub fn approve(&self, name: &str) -> Result<Value> {
        self.ensure_rule_visible(name)?;
        rules::edit::approve(&self.paths.rules_file(), name)?;
        for account in &self.all_accounts {
            self.paths.ensure_account(account)?;
            Store::open(&self.paths.mail_db(account))?.restart_rule_clock(name, sync::now())?;
        }
        Ok(json!({ "rule": name, "enabled": true }))
    }

    pub fn reject(&self, name: &str) -> Result<Value> {
        self.ensure_rule_visible(name)?;
        rules::edit::reject(&self.paths.rules_file(), name)?;
        Ok(json!({ "rejected": name }))
    }

    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<Value> {
        self.ensure_rule_visible(name)?;
        rules::edit::set_enabled(&self.paths.rules_file(), name, enabled)?;
        Ok(json!({ "rule": name, "enabled": enabled }))
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
