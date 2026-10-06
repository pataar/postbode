use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};

use postbode::config::{AccountConfig, Config, Identity, PasswordSource};
use postbode::credentials::{self, Secret};
use postbode::mail_ops::MailOps;
use postbode::paths::Paths;
use postbode::rules::engine::{Context, Mode, evaluate};
use postbode::rules::{Action, CompiledRule};
use postbode::store::{Message, Store};
use postbode::sync::{self, Event};
use postbode::trash::Trash;

#[derive(Parser)]
#[command(
    name = "postbode",
    version,
    about = "A fast, simple mail client with automatic mailbox rules"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Args)]
struct Selection {
    /// Message uids in --folder, as `list` prints them
    #[arg(required = true)]
    uids: Vec<u32>,
    #[arg(long)]
    account: Option<String>,
    #[arg(long, default_value = "INBOX")]
    folder: String,
    /// Print what would happen without touching the server
    #[arg(long)]
    dry_run: bool,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Mark {
    Flag,
    Read,
    Unflag,
    Unread,
}

#[derive(Subcommand)]
enum Command {
    /// Sync all accounts continuously and apply rules; Ctrl-C stops
    Run,
    /// Sync once, apply rules, exit
    Sync {
        #[arg(long)]
        account: Option<String>,
    },
    /// Inspect and test rules.toml
    Rules {
        #[command(subcommand)]
        command: RulesCommand,
    },
    /// List folders with message and unread counts
    Folders {
        #[arg(long)]
        account: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List recent messages, newest first
    List {
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value = "INBOX")]
        folder: String,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Full-text search (FTS5 syntax) over subject, addresses and fetched bodies, newest first
    Search {
        query: String,
        #[arg(long)]
        account: Option<String>,
        #[arg(long)]
        folder: Option<String>,
        /// Fetch and index missing bodies first; slow on a large folder
        #[arg(long)]
        bodies: bool,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Show one message
    Show {
        uid: u32,
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value = "INBOX")]
        folder: String,
        /// Print the raw RFC 5322 message instead of the text body
        #[arg(long)]
        raw: bool,
        #[arg(long)]
        json: bool,
    },
    /// Mark messages read or unread, flagged or unflagged
    Mark {
        #[arg(value_enum)]
        how: Mark,
        #[command(flatten)]
        selection: Selection,
    },
    /// Move messages to another folder, creating it if needed
    Move {
        #[arg(long)]
        to: String,
        #[command(flatten)]
        selection: Selection,
    },
    /// Move messages to the Archive folder
    Archive {
        #[command(flatten)]
        selection: Selection,
    },
    /// Move messages to Trash; inside Trash, or without one, delete them keeping a local .eml backup
    Delete {
        #[command(flatten)]
        selection: Selection,
    },
    /// Show what rules did, newest first
    Log {
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// Deleted mail kept for the retention period
    Trash {
        #[command(subcommand)]
        command: TrashCommand,
    },
    /// Manage accounts
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
}

#[derive(Subcommand)]
enum RulesCommand {
    /// Validate rules.toml
    Check,
    /// Dry run: print what each rule would do to the cached messages
    Test {
        name: Option<String>,
        #[arg(long)]
        account: Option<String>,
    },
    /// Names, enabled state and who proposed them
    List {
        #[arg(long)]
        json: bool,
    },
    /// Run one rule against mail that predates it
    ApplyExisting {
        name: String,
        #[arg(long)]
        account: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum TrashCommand {
    List {
        #[arg(long)]
        account: Option<String>,
    },
    /// Append a trashed .eml back into its original folder
    Restore {
        file: String,
        #[arg(long)]
        account: Option<String>,
    },
    /// Remove trash files older than the retention period
    Purge {
        #[arg(long)]
        account: Option<String>,
    },
}

#[derive(Subcommand)]
enum AccountCommand {
    /// Interactively add an IMAP account and test the login
    Add,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let paths = match std::env::var_os("POSTBODE_HOME") {
        Some(home) => Paths::under(Path::new(&home)),
        None => Paths::discover()?,
    };
    let config = Config::load(&paths.config_file())?;
    match cli.command {
        Command::Run => cmd_run(&config, &paths),
        Command::Sync { account } => {
            let (tx, rx) = mpsc::channel();
            let mut failed = false;
            for acc in select_accounts(&config, account.as_deref())? {
                if let Err(e) = sync::run_once(acc, &paths, &tx) {
                    eprintln!("[{}] error: {}", acc.name, clean(&format!("{e:#}"), false));
                    failed = true;
                }
            }
            drop(tx);
            for event in rx {
                failed |= matches!(event, Event::Error { .. });
                print_event(&event);
            }
            if failed {
                bail!("sync failed for at least one account or folder");
            }
            Ok(())
        }
        Command::Rules { command } => cmd_rules(command, &config, &paths),
        Command::Folders { account, json } => {
            for acc in select_accounts(&config, account.as_deref())? {
                let store = open_store(&paths, &acc.name)?;
                for f in store.folders()? {
                    let total = store.message_count(&f.name)?;
                    let unread = store.unread_count(&f.name)?;
                    if json {
                        println!(
                            "{}",
                            serde_json::json!({ "account": acc.name, "folder": f.name, "total": total, "unread": unread, "special_use": f.special_use })
                        );
                    } else {
                        println!("{}\t{}\t{total}\t{unread}", acc.name, clean(&f.name, false));
                    }
                }
            }
            Ok(())
        }
        Command::List {
            account,
            folder,
            limit,
            json,
        } => {
            for acc in select_accounts(&config, account.as_deref())? {
                let store = open_store(&paths, &acc.name)?;
                for m in store.messages(&folder, limit)? {
                    if json {
                        println!("{}", json_line(&acc.name, &m)?);
                    } else {
                        println!("{}", message_line(&acc.name, &m, 0));
                    }
                }
            }
            Ok(())
        }
        Command::Search {
            query,
            account,
            folder,
            bodies,
            limit,
            json,
        } => {
            for acc in select_accounts(&config, account.as_deref())? {
                let store = open_store(&paths, &acc.name)?;
                if bodies {
                    fetch_missing_bodies(acc, &store, folder.as_deref())?;
                }
                for m in store.search(&query, folder.as_deref(), limit)? {
                    if json {
                        println!("{}", json_line(&acc.name, &m)?);
                    } else {
                        println!("{}", message_line(&acc.name, &m, 0));
                    }
                }
            }
            Ok(())
        }
        Command::Show {
            uid,
            account,
            folder,
            raw,
            json,
        } => {
            let acc = single_account(&config, account.as_deref())?;
            let store = open_store(&paths, &acc.name)?;
            let msg = store
                .message(&folder, uid)?
                .with_context(|| format!("no message {folder}/{uid}"))?;
            let body = message_raw(acc, &store, &msg)?;
            if raw {
                io::stdout().write_all(&body)?;
            } else if json {
                println!(
                    "{}",
                    serde_json::json!({ "account": acc.name, "message": msg, "body_text": postbode::message::body_text(&body) })
                );
            } else {
                println!(
                    "From: {}\nTo: {}\nSubject: {}\n",
                    clean(msg.from_addr.as_deref().unwrap_or(""), false),
                    clean(msg.to_addr.as_deref().unwrap_or(""), false),
                    clean(msg.subject.as_deref().unwrap_or(""), false)
                );
                println!("{}", clean(&postbode::message::body_text(&body), true));
            }
            Ok(())
        }
        Command::Mark { how, selection } => {
            let action = match how {
                Mark::Flag => Action::Flag,
                Mark::Read => Action::MarkRead,
                Mark::Unflag => Action::Unflag,
                Mark::Unread => Action::MarkUnread,
            };
            cmd_act(&config, &paths, selection, action)
        }
        Command::Move { to, selection } => {
            if to.trim().is_empty() {
                bail!("--to must name a folder");
            }
            cmd_act(&config, &paths, selection, Action::Move(to))
        }
        Command::Archive { selection } => cmd_act(&config, &paths, selection, Action::Archive),
        Command::Delete { selection } => cmd_act(&config, &paths, selection, Action::Trash),
        Command::Log {
            account,
            limit,
            json,
        } => {
            for acc in select_accounts(&config, account.as_deref())? {
                let store = open_store(&paths, &acc.name)?;
                for e in store.log(limit)? {
                    if json {
                        println!("{}", json_line(&acc.name, &e)?);
                    } else {
                        println!(
                            "{}  {}  {:<20} {:<12} {}/{}  {}",
                            acc.name,
                            format_time(e.at),
                            clean(&e.rule_name, false),
                            clean(&e.action, false),
                            clean(&e.folder, false),
                            e.uid,
                            clean(e.subject.as_deref().unwrap_or(""), false)
                        );
                    }
                }
            }
            Ok(())
        }
        Command::Trash { command } => cmd_trash(command, &config, &paths),
        Command::Account { command } => match command {
            AccountCommand::Add => cmd_account_add(config, &paths),
        },
    }
}

/// Runs a direct action on the selected uids; `--dry-run` only reads the local store.
fn cmd_act(config: &Config, paths: &Paths, selection: Selection, action: Action) -> Result<()> {
    let acc = single_account(config, selection.account.as_deref())?;
    let store = open_store(paths, &acc.name)?;
    let folder = clean(&selection.folder, false);
    if selection.dry_run {
        let mut missing = 0;
        for &uid in &selection.uids {
            match store.message(&selection.folder, uid)? {
                Some(m) => println!(
                    "would {}  {folder}/{uid}  {}",
                    clean(&action.label(), false),
                    clean(m.subject.as_deref().unwrap_or(""), false)
                ),
                None => {
                    eprintln!("{folder}/{uid}: not in the local store");
                    missing += 1;
                }
            }
        }
        if missing > 0 {
            bail!("{missing} messages are not in the local store; run `postbode sync` first");
        }
        return Ok(());
    }
    let mut ops = sync::connect(acc)?;
    let trash = Trash::new(paths.trash_dir(&acc.name));
    let results = postbode::actions::run(
        &mut ops,
        &store,
        &trash,
        &selection.folder,
        &selection.uids,
        &action,
        sync::now(),
    )?;
    let mut failed = 0;
    for (uid, result) in &results {
        if let Err(e) = result {
            eprintln!("{folder}/{uid}: {}", clean(&e.to_string(), false));
            failed += 1;
        }
    }
    println!(
        "{}: {} of {} messages",
        clean(&action.label(), false),
        results.len() - failed,
        results.len()
    );
    if failed > 0 {
        bail!("{failed} of {} messages failed", results.len());
    }
    Ok(())
}

/// The full message, from the store or fetched once from the server.
/// Fetches the bodies `search --bodies` needs; a folder that cannot be fetched is reported and skipped.
fn fetch_missing_bodies(
    account: &AccountConfig,
    store: &Store,
    folder: Option<&str>,
) -> Result<()> {
    let folders = match folder {
        Some(folder) => vec![folder.to_string()],
        None => store.folders()?.into_iter().map(|f| f.name).collect(),
    };
    let mut ops = sync::connect(account)?;
    for folder in folders {
        let name = clean(&folder, false);
        let announce = |count| {
            eprintln!(
                "{}: fetching {count} bodies in {name}; this can take a while on a large folder",
                account.name
            )
        };
        if let Err(e) = postbode::actions::fetch_bodies(&mut ops, store, &folder, announce) {
            eprintln!("{}: {name}: {}", account.name, clean(&e.to_string(), false));
        }
    }
    Ok(())
}

fn message_raw(account: &AccountConfig, store: &Store, msg: &Message) -> Result<Vec<u8>> {
    if let Some(raw) = store.raw(&msg.folder, msg.uid)? {
        return Ok(raw);
    }
    let mut ops = sync::connect(account)?;
    postbode::actions::select_synced(&mut ops, store, &msg.folder)?;
    Ok(postbode::rules::apply::ensure_raw(msg, &mut ops, store)?)
}

fn select_accounts<'a>(config: &'a Config, name: Option<&str>) -> Result<Vec<&'a AccountConfig>> {
    match name {
        Some(n) => Ok(vec![
            config
                .account(n)
                .with_context(|| format!("no account named '{n}'"))?,
        ]),
        None => Ok(config.accounts.iter().collect()),
    }
}

fn single_account<'a>(config: &'a Config, name: Option<&str>) -> Result<&'a AccountConfig> {
    match (name, config.accounts.len()) {
        (Some(n), _) => config
            .account(n)
            .with_context(|| format!("no account named '{n}'")),
        (None, 1) => Ok(&config.accounts[0]),
        (None, 0) => bail!("no accounts configured; run `postbode account add`"),
        (None, _) => bail!("several accounts configured; pass --account"),
    }
}

fn open_store(paths: &Paths, account: &str) -> Result<Store> {
    paths.ensure_account(account)?;
    Ok(Store::open(&paths.mail_db(account))?)
}

fn format_time(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

/// `account  * INBOX/42  date  from  subject`; `*` marks unread, `depth` indents the subject in thread views.
fn message_line(account: &str, m: &Message, depth: usize) -> String {
    format!(
        "{account}  {} {:<14}  {}  {:<30}  {}{}",
        if m.is_seen() { " " } else { "*" },
        clean(&format!("{}/{}", m.folder, m.uid), false),
        format_time(m.internaldate),
        truncate(&clean(m.from_addr.as_deref().unwrap_or(""), false), 30),
        "  ".repeat(depth),
        clean(m.subject.as_deref().unwrap_or(""), false)
    )
}

/// The row as one JSON line carrying its account, the shape the MCP tools will return.
fn json_line(account: &str, row: &impl serde::Serialize) -> Result<String> {
    let mut value = serde_json::to_value(row)?;
    if let Some(object) = value.as_object_mut() {
        object.insert("account".into(), account.into());
    }
    Ok(value.to_string())
}

fn truncate(s: &str, width: usize) -> String {
    let mut out: String = s.chars().take(width).collect();
    if s.chars().count() > width {
        out.pop();
        out.push('…');
    }
    out
}

/// Server-supplied text with control characters removed, so a header cannot drive the terminal. `keep_layout` keeps
/// newlines and tabs, for message bodies.
pub(crate) fn clean(text: &str, keep_layout: bool) -> String {
    text.chars()
        .filter(|c| !c.is_control() || (keep_layout && matches!(c, '\n' | '\t')))
        .collect()
}

/// Linux notification servers render a subset of HTML in the summary and body.
fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn notification_text(text: &str) -> String {
    let text = clean(text, false);
    if cfg!(target_os = "linux") {
        escape_markup(&text)
    } else {
        text
    }
}

fn print_event(event: &Event) {
    match event {
        Event::NewMail {
            account,
            from,
            subject,
            ..
        } => println!(
            "[{account}] new mail from {}: {}",
            clean(from, false),
            clean(subject, false)
        ),
        Event::Synced {
            account,
            new_messages,
            actions,
        } => println!("[{account}] synced: {new_messages} new, {actions} rule actions"),
        Event::Error { account, message } => {
            eprintln!("[{account}] error: {}", clean(message, false))
        }
    }
}

fn cmd_run(config: &Config, paths: &Paths) -> Result<()> {
    if config.accounts.is_empty() {
        bail!("no accounts configured; run `postbode account add`");
    }
    compiled_rules(paths, None)?;
    // Ctrl-C ends the process through the default SIGINT handler; WAL and trash-before-delete leave nothing half done.
    let shutdown = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let mut handles = Vec::new();
    for account in config.accounts.clone() {
        let (paths, tx, shutdown) = (paths.clone(), tx.clone(), shutdown.clone());
        handles.push(
            std::thread::Builder::new()
                .name(format!("sync-{}", account.name))
                .spawn(move || sync::run_loop(account, paths, tx, shutdown))?,
        );
    }
    drop(tx);
    for event in rx {
        print_event(&event);
        if let Event::NewMail { from, subject, .. } = &event {
            let _ = notify_rust::Notification::new()
                .summary(&notification_text(from))
                .body(&notification_text(subject))
                .appname("Postbode")
                .show();
        }
    }
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}

fn cmd_rules(command: RulesCommand, config: &Config, paths: &Paths) -> Result<()> {
    match command {
        RulesCommand::Check => {
            println!("{} rules ok", compiled_rules(paths, None)?.len());
            Ok(())
        }
        RulesCommand::List { json } => {
            let file = postbode::rules::load(&paths.rules_file())?;
            for r in &file.rules {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({ "name": r.name, "enabled": r.enabled, "proposed_by": r.proposed_by, "account": r.account, "folder": r.folder })
                    );
                } else {
                    println!(
                        "{}\t{}\t{}",
                        if r.enabled { "on " } else { "off" },
                        clean(&r.name, false),
                        clean(r.proposed_by.as_deref().unwrap_or(""), false)
                    );
                }
            }
            Ok(())
        }
        RulesCommand::ApplyExisting {
            name,
            account,
            dry_run,
        } => {
            let rules = compiled_rules(paths, Some(&name))?;
            let mut failed = false;
            for acc in select_accounts(config, account.as_deref())? {
                if !rules.iter().any(|r| r.applies_to_account(&acc.name)) {
                    continue;
                }
                let store = open_store(paths, &acc.name)?;
                let identity = acc.identity()?;
                if dry_run {
                    print_planned_actions(&rules, &store, acc, &identity)?;
                    continue;
                }
                let mut ops = sync::connect(acc)?;
                let trash = Trash::new(paths.trash_dir(&acc.name));
                let run = sync::run_rules(
                    &mut ops,
                    &store,
                    &trash,
                    &rules,
                    acc,
                    &identity,
                    &[],
                    Mode::ApplyExisting,
                    sync::now(),
                )?;
                for event in &run.events {
                    failed |= matches!(event, Event::Error { .. });
                    print_event(event);
                }
                println!(
                    "{}: {} actions on {} messages",
                    acc.name, run.actions, run.evaluated
                );
            }
            if failed {
                bail!("some actions failed; see the errors above");
            }
            Ok(())
        }
        RulesCommand::Test { name, account } => {
            let rules = compiled_rules(paths, name.as_deref())?;
            for acc in select_accounts(config, account.as_deref())? {
                let store = open_store(paths, &acc.name)?;
                print_planned_actions(&rules, &store, acc, &acc.identity()?)?;
            }
            Ok(())
        }
    }
}

/// Compiles rules with `first_seen_at` left at 0 and without touching any store, so previews are
/// side-effect free and cover mail that predates the rule. A `name` that matches no rule is an error.
fn compiled_rules(paths: &Paths, name: Option<&str>) -> Result<Vec<CompiledRule>> {
    let file = postbode::rules::load(&paths.rules_file())?;
    let compiled: Vec<CompiledRule> = postbode::rules::compile(&file)?
        .into_iter()
        .filter(|r| name.is_none_or(|n| n == r.rule.name))
        .collect();
    if let Some(name) = name
        && compiled.is_empty()
    {
        bail!("no rule named '{name}'");
    }
    Ok(compiled)
}

fn print_planned_actions(
    rules: &[CompiledRule],
    store: &Store,
    account: &AccountConfig,
    identity: &Identity,
) -> Result<()> {
    let ctx = Context {
        account: &account.name,
        identity,
        now: sync::now(),
        mode: Mode::ApplyExisting,
        notify_default: false,
    };
    for folder in store.folders()? {
        for msg in store.messages_in_folder(&folder.name)? {
            for a in evaluate(rules, &msg, &ctx).actions {
                println!(
                    "{}\t{}/{}\t{}\t{}",
                    clean(&a.rule, false),
                    clean(&msg.folder, false),
                    msg.uid,
                    a.action.label(),
                    clean(msg.subject.as_deref().unwrap_or(""), false)
                );
            }
        }
    }
    Ok(())
}

fn cmd_trash(command: TrashCommand, config: &Config, paths: &Paths) -> Result<()> {
    match command {
        TrashCommand::List { account } => {
            for acc in select_accounts(config, account.as_deref())? {
                for e in Trash::new(paths.trash_dir(&acc.name)).list()? {
                    let subject = std::fs::read(&e.path)
                        .ok()
                        .and_then(|raw| postbode::message::parse_headers(&raw).subject)
                        .unwrap_or_default();
                    println!(
                        "{}  {}  {}/{}  {}  {}",
                        acc.name,
                        format_time(e.saved_at),
                        clean(&e.folder, false),
                        e.uid,
                        clean(&subject, false),
                        clean(&e.path.display().to_string(), false)
                    );
                }
            }
            Ok(())
        }
        TrashCommand::Restore { file, account } => {
            let acc = single_account(config, account.as_deref())?;
            let path = Path::new(&file);
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .context("bad file name")?;
            let (_, folder, _) = Trash::parse_name(name).context("not a postbode trash file")?;
            let raw = std::fs::read(path)?;
            let mut ops = sync::connect(acc)?;
            ops.append(&folder, &raw, &[postbode::rules::engine::RESTORED_KEYWORD])?;
            std::fs::remove_file(path)?;
            println!(
                "restored to {}; rules leave restored mail alone. Run `postbode sync` to see it",
                clean(&folder, false)
            );
            Ok(())
        }
        TrashCommand::Purge { account } => {
            for acc in select_accounts(config, account.as_deref())? {
                let removed = Trash::new(paths.trash_dir(&acc.name))
                    .purge(acc.trash_retention_days as i64 * 86_400, sync::now())?;
                println!("{}: removed {removed}", acc.name);
            }
            Ok(())
        }
    }
}

fn cmd_account_add(mut config: Config, paths: &Paths) -> Result<()> {
    let name = prompt("Account name (letters, digits, - _)")?;
    let host = prompt("IMAP host")?;
    let port: u16 = prompt("Port [993]")?.parse().unwrap_or(993);
    let username = prompt("Username")?;
    let address = {
        let a = prompt(&format!("Email address [{username}]"))?;
        if a.is_empty() { None } else { Some(a) }
    };
    let storage = prompt("Password storage: (k)eyring or (c)ommand [k]")?;
    let password = if storage.starts_with('c') {
        PasswordSource::Command {
            command: prompt("Password command")?,
        }
    } else {
        PasswordSource::Keyring { keyring: true }
    };
    let account = AccountConfig {
        name,
        host,
        port,
        username,
        password,
        address,
        aliases: vec![],
        sync_interval_secs: 120,
        trash_retention_days: 30,
        notify: true,
    };
    config.accounts.retain(|a| a.name != account.name);
    config.accounts.push(account);
    config.validate()?;
    let account = config.accounts.last().expect("account was just pushed");
    let secret = match &account.password {
        PasswordSource::Command { .. } => credentials::resolve(account)?,
        PasswordSource::Keyring { .. } => Secret::new(rpassword::prompt_password("Password: ")?),
    };
    print!("Testing login... ");
    io::stdout().flush()?;
    let mut ops = postbode::mail_ops::imap::ImapOps::connect(account, &secret)?;
    let folders = ops.list_folders()?;
    println!("ok, {} folders", folders.len());
    if matches!(account.password, PasswordSource::Keyring { .. }) {
        credentials::store(&account.name, &secret)?;
    }
    config.save(&paths.config_file())?;
    println!("saved to {}", paths.config_file().display());
    Ok(())
}

fn prompt(label: &str) -> Result<String> {
    print!("{label}: ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_strips_control_characters() {
        let hostile = "Re: \u{1b}]0;pwned\u{7}hi\u{9b}2J\r\n\tthere\u{7f}";
        assert_eq!(clean(hostile, false), "Re: ]0;pwnedhi2Jthere");
        assert_eq!(clean(hostile, true), "Re: ]0;pwnedhi2J\n\tthere");
    }

    #[test]
    fn escape_markup_escapes_tags_and_entities() {
        assert_eq!(
            escape_markup("<b>Tom & Jerry</b>"),
            "&lt;b&gt;Tom &amp; Jerry&lt;/b&gt;"
        );
    }
}
