use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};

use postbode::config::{AccountConfig, Config, PasswordSource};
use postbode::credentials::{self, Secret};
use postbode::mail_ops::MailOps;
use postbode::paths::Paths;
use postbode::rules::engine::{Context, Mode, evaluate};
use postbode::store::Store;
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
    /// Names, enabled state and first-seen time
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
            for acc in select_accounts(&config, account.as_deref())? {
                sync::run_once(acc, &paths, &tx)?;
            }
            drop(tx);
            for event in rx {
                print_event(&event);
            }
            Ok(())
        }
        Command::Rules { command } => cmd_rules(command, &config, &paths),
        Command::Folders { account, json } => {
            for acc in select_accounts(&config, account.as_deref())? {
                let store = open_store(&paths, &acc.name)?;
                for f in store.folders()? {
                    let total = store.messages_in_folder(&f.name)?.len();
                    let unread = store.unread_count(&f.name)?;
                    if json {
                        println!(
                            "{}",
                            serde_json::json!({ "account": acc.name, "folder": f.name, "total": total, "unread": unread, "special_use": f.special_use })
                        );
                    } else {
                        println!("{}\t{}\t{total}\t{unread}", acc.name, f.name);
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
                        println!("{}", serde_json::to_string(&m)?);
                    } else {
                        let date = chrono::DateTime::from_timestamp(m.internaldate, 0)
                            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_default();
                        let flag = if m.is_seen() { " " } else { "*" };
                        println!(
                            "{flag} {:>6}  {date}  {:<30}  {}",
                            m.uid,
                            truncate(m.from_addr.as_deref().unwrap_or(""), 30),
                            m.subject.as_deref().unwrap_or("")
                        );
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
            let body = match store.raw(&folder, uid)? {
                Some(raw) => raw,
                None => {
                    let secret = credentials::resolve(acc)?;
                    let mut ops = postbode::mail_ops::imap::ImapOps::connect(acc, &secret)?;
                    ops.select(&folder)?;
                    postbode::rules::apply::ensure_raw(&msg, &mut ops, &store)?
                }
            };
            if raw {
                io::stdout().write_all(&body)?;
            } else if json {
                println!(
                    "{}",
                    serde_json::json!({ "message": msg, "body_text": postbode::message::body_text(&body) })
                );
            } else {
                println!(
                    "From: {}\nTo: {}\nSubject: {}\n",
                    msg.from_addr.as_deref().unwrap_or(""),
                    msg.to_addr.as_deref().unwrap_or(""),
                    msg.subject.as_deref().unwrap_or("")
                );
                println!("{}", postbode::message::body_text(&body));
            }
            Ok(())
        }
        Command::Log {
            account,
            limit,
            json,
        } => {
            for acc in select_accounts(&config, account.as_deref())? {
                let store = open_store(&paths, &acc.name)?;
                for e in store.log(limit)? {
                    if json {
                        println!("{}", serde_json::to_string(&e)?);
                    } else {
                        let at = chrono::DateTime::from_timestamp(e.at, 0)
                            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_default();
                        println!(
                            "{at}  {:<20} {:<12} {}/{}  {}",
                            e.rule_name,
                            e.action,
                            e.folder,
                            e.uid,
                            e.subject.as_deref().unwrap_or("")
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

fn truncate(s: &str, width: usize) -> String {
    let mut out: String = s.chars().take(width).collect();
    if s.chars().count() > width {
        out.pop();
        out.push('…');
    }
    out
}

fn print_event(event: &Event) {
    match event {
        Event::NewMail {
            account,
            from,
            subject,
            ..
        } => println!("[{account}] new mail from {from}: {subject}"),
        Event::Synced {
            account,
            new_messages,
            actions,
        } => println!("[{account}] synced: {new_messages} new, {actions} rule actions"),
        Event::Error { account, message } => eprintln!("[{account}] error: {message}"),
    }
}

fn cmd_run(config: &Config, paths: &Paths) -> Result<()> {
    if config.accounts.is_empty() {
        bail!("no accounts configured; run `postbode account add`");
    }
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
                .summary(from)
                .body(subject)
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
            let file = postbode::rules::load(&paths.rules_file())?;
            let compiled = postbode::rules::compile(&file)?;
            println!("{} rules ok", compiled.len());
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
                        r.name,
                        r.proposed_by.as_deref().unwrap_or("")
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
            for acc in select_accounts(config, account.as_deref())? {
                let store = open_store(paths, &acc.name)?;
                let rules: Vec<_> = sync::load_rules_for(&store, &paths.rules_file(), sync::now())?
                    .into_iter()
                    .filter(|r| r.rule.name == name)
                    .collect();
                if rules.is_empty() {
                    bail!("no rule named '{name}'");
                }
                let identity = acc.identity()?;
                if dry_run {
                    let ctx = Context {
                        account: &acc.name,
                        identity: &identity,
                        now: sync::now(),
                        mode: Mode::ApplyExisting,
                        notify_default: false,
                    };
                    for msg in store.messages_in_folder(rules[0].folder())? {
                        for a in evaluate(&rules, &msg, &ctx).actions {
                            println!(
                                "{}\t{}/{}\t{}\t{}",
                                a.rule,
                                msg.folder,
                                msg.uid,
                                a.action.label(),
                                msg.subject.as_deref().unwrap_or("")
                            );
                        }
                    }
                    continue;
                }
                let secret = credentials::resolve(acc)?;
                let mut ops = postbode::mail_ops::imap::ImapOps::connect(acc, &secret)?;
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
                println!(
                    "{}: {} actions on {} messages",
                    acc.name, run.actions, run.evaluated
                );
            }
            Ok(())
        }
        RulesCommand::Test { name, account } => {
            for acc in select_accounts(config, account.as_deref())? {
                let store = open_store(paths, &acc.name)?;
                let rules = sync::load_rules_for(&store, &paths.rules_file(), sync::now())?;
                let rules: Vec<_> = rules
                    .into_iter()
                    .filter(|r| name.as_deref().is_none_or(|n| n == r.rule.name))
                    .collect();
                let identity = acc.identity()?;
                let ctx = Context {
                    account: &acc.name,
                    identity: &identity,
                    now: sync::now(),
                    mode: Mode::Normal,
                    notify_default: acc.notify,
                };
                for folder in store.folders()? {
                    for msg in store.messages_in_folder(&folder.name)? {
                        let plan = evaluate(&rules, &msg, &ctx);
                        for a in plan.actions {
                            println!(
                                "{}\t{}/{}\t{}\t{}",
                                a.rule,
                                msg.folder,
                                msg.uid,
                                a.action.label(),
                                msg.subject.as_deref().unwrap_or("")
                            );
                        }
                    }
                }
            }
            Ok(())
        }
    }
}

fn cmd_trash(command: TrashCommand, config: &Config, paths: &Paths) -> Result<()> {
    match command {
        TrashCommand::List { account } => {
            for acc in select_accounts(config, account.as_deref())? {
                for e in Trash::new(paths.trash_dir(&acc.name)).list()? {
                    let at = chrono::DateTime::from_timestamp(e.saved_at, 0)
                        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_default();
                    println!("{at}  {}/{}  {}", e.folder, e.uid, e.path.display());
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
            let secret = credentials::resolve(acc)?;
            let mut ops = postbode::mail_ops::imap::ImapOps::connect(acc, &secret)?;
            ops.append(&folder, &raw)?;
            std::fs::remove_file(path)?;
            println!("restored to {folder}; run `postbode sync` to see it");
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
    let (password, secret) = if storage.starts_with('c') {
        let command = prompt("Password command")?;
        let src = PasswordSource::Command { command };
        let tmp = AccountConfig {
            name: name.clone(),
            host: host.clone(),
            port,
            username: username.clone(),
            password: src.clone(),
            address: address.clone(),
            aliases: vec![],
            sync_interval_secs: 120,
            trash_retention_days: 30,
            notify: true,
        };
        let secret = credentials::resolve(&tmp)?;
        (src, secret)
    } else {
        let pw = rpassword::prompt_password("Password: ")?;
        (PasswordSource::Keyring { keyring: true }, Secret::new(pw))
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
    print!("Testing login... ");
    io::stdout().flush()?;
    let mut ops = postbode::mail_ops::imap::ImapOps::connect(&account, &secret)?;
    let folders = ops.list_folders()?;
    println!("ok, {} folders", folders.len());
    if matches!(account.password, PasswordSource::Keyring { .. }) {
        credentials::store(&account.name, &secret)?;
    }
    config.accounts.retain(|a| a.name != account.name);
    config.accounts.push(account);
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
