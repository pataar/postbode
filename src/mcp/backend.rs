//! The only MCP code that touches the store, rules.toml or the daemon; the daemon owns every IMAP connection.
use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};

use crate::actions;
use crate::config::{AccountConfig, Config};
use crate::daemon::Client;
use crate::daemon::client::LazyClient;
use crate::message;
use crate::output;
use crate::paths::Paths;
use crate::rules::{self, Action, Rule, RuleFile};
use crate::store::{Message, Store};
use crate::sync::{Command, Event};
use crate::time;
use crate::trash::Trash;

pub const MAX_LIMIT: u32 = 500;

pub struct Backend {
    /// Every configured account, visible or not: an approved rule's clock restarts in all of them.
    all_accounts: Vec<String>,
    accounts: Vec<AccountConfig>,
    daemon: LazyClient,
    paths: Paths,
}

impl Backend {
    /// The accounts in `only`, or all of them; an unknown name, or no account at all, is an error.
    pub fn new(config: &Config, paths: &Paths, only: &[String]) -> Result<Backend> {
        Backend::build(config, paths, only, LazyClient::new(paths))
    }

    /// Like `new`, over a daemon connection the caller made.
    pub fn with_client(
        config: &Config,
        paths: &Paths,
        only: &[String],
        client: Client,
    ) -> Result<Backend> {
        Backend::build(config, paths, only, LazyClient::with_client(paths, client))
    }

    fn build(
        config: &Config,
        paths: &Paths,
        only: &[String],
        daemon: LazyClient,
    ) -> Result<Backend> {
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
            daemon,
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
        Ok(Store::open_account(&self.paths, &account.name)?)
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
        let raw = self.daemon.raw_message(&acc.name, &store, &msg)?;
        Ok((message_row(&acc.name, &msg)?, message::body_text(&raw)))
    }

    pub fn attachments(&self, account: Option<&str>, folder: &str, uid: u32) -> Result<Vec<Value>> {
        let (acc, store, msg) = self.message(account, folder, uid)?;
        let raw = self.daemon.raw_message(&acc.name, &store, &msg)?;
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

    /// A direct action on `uids` in `folder`, logged as done `by` the agent; `dry_run` reads the local store only and
    /// opens no connection.
    pub fn act(
        &self,
        account: Option<&str>,
        folder: &str,
        uids: &[u32],
        action: Action,
        dry_run: bool,
        by: &str,
    ) -> Result<Value> {
        if uids.is_empty() {
            bail!("uids must name at least one message");
        }
        let acc = self.single(account)?;
        let store = self.store(acc)?;
        if dry_run {
            let (mut would, mut missing) = (Vec::new(), Vec::new());
            for &uid in uids {
                match store.message(folder, uid)? {
                    Some(m) => would.push(json!({
                        "uid": uid, "effect": actions::planned_effect(&store, &m, &action)?, "subject": m.subject,
                    })),
                    None => missing.push(uid),
                }
            }
            return Ok(
                json!({ "account": acc.name, "folder": folder, "dry_run": true, "would": would, "missing": missing }),
            );
        }
        let command = Command::Apply {
            folder: folder.into(),
            uids: uids.to_vec(),
            action: action.clone(),
            by: by.into(),
        };
        let results = self
            .daemon
            .request(&acc.name, command, |reply| match reply {
                Event::ActionDone { results, .. } => Ok(results),
                other => Err(other),
            })?;
        let failed: Vec<Value> = results
            .iter()
            .filter_map(|(uid, result)| {
                result
                    .as_ref()
                    .err()
                    .map(|message| json!({ "uid": uid, "error": message }))
            })
            .collect();
        let label = match action {
            Action::Trash => Action::Delete.label(),
            _ => action.label(),
        };
        Ok(
            json!({ "account": acc.name, "folder": folder, "action": label, "done": results.len() - failed.len(), "failed": failed }),
        )
    }

    /// Restores a backup named as `trash_list` prints it; paths are refused, so nothing outside the trash directory is read.
    pub fn trash_restore(&self, account: Option<&str>, file: &str, dry_run: bool) -> Result<Value> {
        let bare = std::path::Path::new(file)
            .file_name()
            .is_some_and(|name| name == file);
        let Some((_, folder, _)) = Trash::parse_name(file).filter(|_| bare) else {
            bail!("file must be a name as trash_list returns it");
        };
        let acc = self.single(account)?;
        let path = self.paths.trash_dir(&acc.name).join(file);
        if !path.is_file() {
            bail!("no trash file {file}; see trash_list");
        }
        if dry_run {
            return Ok(
                json!({ "account": acc.name, "file": file, "dry_run": true, "would": format!("restore to {folder}") }),
            );
        }
        let folder =
            self.daemon.request(
                &acc.name,
                Command::Restore { file: path },
                |reply| match reply {
                    Event::Restored { folder, .. } => Ok(folder),
                    other => Err(other),
                },
            )?;
        Ok(json!({ "account": acc.name, "restored_to": folder }))
    }

    /// Asks the daemon for one sync with rules per visible account; a refusal is that account's `error`.
    pub fn sync(&self, account: Option<&str>) -> Result<Vec<Value>> {
        let mut rows = Vec::new();
        for acc in self.select(account)? {
            let synced = self
                .daemon
                .request(&acc.name, Command::SyncNow, |reply| match reply {
                    Event::Synced {
                        new_messages,
                        actions,
                        errors,
                        ..
                    } => Ok((new_messages, actions, errors)),
                    other => Err(other),
                });
            rows.push(match synced {
                Ok((new_messages, actions, errors)) => json!({ "account": acc.name, "new_messages": new_messages, "actions": actions, "errors": errors }),
                Err(e) => json!({ "account": acc.name, "error": format!("{e:#}") }),
            });
        }
        Ok(rows)
    }

    fn is_visible(&self, account: Option<&str>) -> bool {
        account.is_none_or(|a| self.accounts.iter().any(|acc| acc.name == a))
    }

    /// Whether `--account` hides some configured account; a rule without `account` then reaches beyond this server.
    fn narrowed(&self) -> bool {
        self.accounts.len() < self.all_accounts.len()
    }

    /// A rule scoped to a hidden account is as good as absent, and while accounts are hidden a rule for every account
    /// cannot be turned on. An unknown name is left to the edit's own error.
    fn ensure_rule_visible(&self, name: &str, enabling: bool) -> Result<()> {
        let file = rules::load(&self.paths.rules_file())?;
        match file.rules.iter().find(|r| r.name == name) {
            Some(rule) if !self.is_visible(rule.account.as_deref()) => {
                bail!("no rule named '{name}'")
            }
            Some(rule) if enabling && rule.account.is_none() && self.narrowed() => {
                let visible: Vec<&str> = self.accounts.iter().map(|a| a.name.as_str()).collect();
                bail!(
                    "rule '{name}' applies to every account; this server only sees {}",
                    visible.join(", ")
                )
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
    /// Without `bodies` a body condition is refused, since its matches would reveal body text.
    pub fn rules_test(
        &self,
        mut rule: Rule,
        account: Option<&str>,
        bodies: bool,
    ) -> Result<Vec<Value>> {
        rule.enabled = true;
        let compiled = rules::compile(&RuleFile { rules: vec![rule] })?;
        if !bodies && compiled.iter().any(|r| r.needs_body()) {
            bail!("matching on the body needs the read:bodies scope");
        }
        let mut rows = Vec::new();
        for acc in self.select(account)? {
            let store = self.store(acc)?;
            for p in actions::planned(&compiled, &store, acc, &acc.identity()?, time::now())? {
                rows.push(json!({
                    "account": acc.name, "rule": p.rule, "folder": p.message.folder,
                    "uid": p.message.uid, "action": p.action.label(), "subject": p.message.subject,
                }));
            }
        }
        Ok(rows)
    }

    /// While accounts are hidden, a rule without `account` gets the only visible one, or is refused.
    pub fn propose(&self, mut rule: Rule, by: &str) -> Result<Value> {
        match rule.account.as_deref() {
            Some(account) if !self.is_visible(Some(account)) => {
                bail!("no account named '{account}'")
            }
            None if self.narrowed() => rule.account = Some(self.single(None)?.name.clone()),
            _ => {}
        }
        let (name, account) = (rule.name.clone(), rule.account.clone());
        rules::edit::propose(&self.paths.rules_file(), rule, by)?;
        Ok(json!({ "proposed": name, "account": account, "enabled": false, "proposed_by": by }))
    }

    /// Enables the rule and restarts its clock in every account, so it acts only on mail that arrives from now on.
    pub fn approve(&self, name: &str) -> Result<Value> {
        self.ensure_rule_visible(name, true)?;
        rules::edit::approve(&self.paths.rules_file(), name)?;
        for account in &self.all_accounts {
            Store::open_account(&self.paths, account)?.restart_rule_clock(name, time::now())?;
        }
        Ok(json!({ "rule": name, "enabled": true }))
    }

    pub fn reject(&self, name: &str) -> Result<Value> {
        self.ensure_rule_visible(name, false)?;
        rules::edit::reject(&self.paths.rules_file(), name)?;
        Ok(json!({ "rejected": name }))
    }

    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<Value> {
        self.ensure_rule_visible(name, enabled)?;
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
