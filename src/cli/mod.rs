use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};

use postbode::config::{AccountConfig, Config, Identity, PasswordSource};
use postbode::credentials::{self, Secret};
use postbode::daemon::Client;
use postbode::daemon::wire::{AccountStatus, Status};
use postbode::mail_ops::MailOps;
use postbode::message::clean;
use postbode::paths::Paths;
use postbode::rules::{Action, CompiledRule, Rule, RuleFile};
use postbode::store::{Message, Store};
use postbode::sync::{self, Activity, Event};
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
    /// Run the daemon in the foreground: sync all accounts continuously and apply rules; fails when a daemon already runs; Ctrl-C stops
    Run {
        /// Exit after this many seconds with no client connected; what auto-start uses
        #[arg(long, hide = true)]
        idle_exit: Option<u64>,
    },
    /// Inspect or stop the daemon that syncs your accounts; other commands start it when needed
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// Open the mail window; syncs every account like `run`
    Gui,
    /// Start the daemon at login: a launchd agent on macOS, a systemd user unit on Linux
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    /// Sync once, apply rules, exit
    Sync {
        #[arg(long)]
        account: Option<String>,
    },
    /// List or save a message's attachments
    Attachment {
        #[command(subcommand)]
        command: AttachmentCommand,
    },
    /// Inspect and test rules.toml
    Rules {
        #[command(subcommand)]
        command: RulesCommand,
    },
    #[command(about = postbode::help::FOLDERS)]
    Folders {
        #[arg(long)]
        account: Option<String>,
        #[arg(long)]
        json: bool,
    },
    #[command(about = postbode::help::LIST)]
    List {
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value = "INBOX")]
        folder: String,
        #[arg(long, default_value_t = 50)]
        limit: u32,
        #[arg(long)]
        json: bool,
        /// Group by conversation, the most recently active thread first
        #[arg(long)]
        threads: bool,
    },
    #[command(about = postbode::help::SEARCH)]
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
    #[command(about = postbode::help::SHOW)]
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
    #[command(about = postbode::help::MARK)]
    Mark {
        #[arg(value_enum)]
        how: Mark,
        #[command(flatten)]
        selection: Selection,
    },
    #[command(about = postbode::help::MOVE)]
    Move {
        #[arg(long)]
        to: String,
        #[command(flatten)]
        selection: Selection,
    },
    #[command(about = postbode::help::ARCHIVE)]
    Archive {
        #[command(flatten)]
        selection: Selection,
    },
    #[command(about = postbode::help::DELETE)]
    Delete {
        #[command(flatten)]
        selection: Selection,
    },
    #[command(about = postbode::help::LOG)]
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
    /// Print the agent guide: how an LLM should drive Postbode
    Guide,
    /// Serve Postbode to an agent host over MCP on stdio; hosts start this, see `postbode mcp install`
    #[command(args_conflicts_with_subcommands = true)]
    Mcp {
        /// Comma-separated: read, read:bodies, rules:propose, rules:write, mail:modify
        #[arg(long, default_value = postbode::help::MCP_DEFAULT_SCOPES)]
        scopes: String,
        /// Only this account; repeatable; default every account
        #[arg(long)]
        account: Vec<String>,
        #[command(subcommand)]
        command: Option<McpCommand>,
    },
}

#[derive(Subcommand)]
enum DaemonCommand {
    /// Print the daemon's pid, version, uptime and what each account is doing
    Status,
    /// Stop the daemon; it starts again when a command needs it
    Stop,
}

#[derive(Subcommand)]
enum ServiceCommand {
    /// Install and start the service; stops a daemon that is already running so the service's takes over
    Install {
        /// Print the file and the commands and change nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Stop and remove the service
    Remove {
        /// Print the commands and change nothing
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum McpCommand {
    /// Register `postbode mcp` with an agent host; re-run it to change the scopes
    Install {
        #[arg(value_enum)]
        target: InstallTarget,
        /// Comma-separated: read, read:bodies, rules:propose, rules:write, mail:modify
        #[arg(long, default_value = postbode::help::MCP_DEFAULT_SCOPES)]
        scopes: String,
        /// Only this account; repeatable; default every account
        #[arg(long)]
        account: Vec<String>,
        /// Take the entry out again
        #[arg(long)]
        remove: bool,
        /// Print the change and write nothing
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum InstallTarget {
    ClaudeCode,
    ClaudeDesktop,
    Json,
}

#[derive(Subcommand)]
enum AttachmentCommand {
    #[command(about = postbode::help::ATTACHMENT_LIST)]
    List {
        uid: u32,
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value = "INBOX")]
        folder: String,
        #[arg(long)]
        json: bool,
    },
    /// Save attachment N, as numbered by `attachment list`, into --dir
    Save {
        uid: u32,
        n: usize,
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value = "INBOX")]
        folder: String,
    },
}

#[derive(Subcommand)]
enum RulesCommand {
    #[command(about = postbode::help::RULES_CHECK)]
    Check,
    /// Dry run: print what each rule would do to the cached messages; naming a rule previews it even while disabled
    Test {
        #[arg(conflicts_with = "stdin")]
        name: Option<String>,
        #[arg(long)]
        account: Option<String>,
        /// Preview one rule read as JSON from stdin instead of rules.toml
        #[arg(long)]
        stdin: bool,
    },
    #[command(about = postbode::help::RULES_SCHEMA)]
    Schema,
    /// Read one rule as JSON on stdin and add it disabled, for a human to approve
    Propose {
        /// Who proposes it, recorded as proposed_by = "cli:WHO"
        #[arg(long)]
        by: Option<String>,
    },
    #[command(about = postbode::help::RULES_APPROVE)]
    Approve { name: String },
    #[command(about = postbode::help::RULES_REJECT)]
    Reject { name: String },
    #[command(about = postbode::help::RULES_LIST)]
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
    #[command(about = postbode::help::TRASH_RESTORE)]
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
        Command::Run { idle_exit } => cmd_run(&config, &paths, idle_exit),
        Command::Daemon { command } => cmd_daemon(command, &paths),
        Command::Gui => cmd_gui(&config, &paths),
        Command::Service { command } => cmd_service(command, &paths),
        Command::Sync { account } => cmd_sync(&config, &paths, account.as_deref()),
        Command::Attachment { command } => cmd_attachment(command, &config, &paths),
        Command::Rules { command } => cmd_rules(command, &config, &paths),
        Command::Folders { account, json } => {
            for acc in select_accounts(&config, account.as_deref())? {
                let store = open_store(&paths, &acc.name)?;
                for f in store.folders()? {
                    let total = store.message_count(&f.name)?;
                    let unread = store.unread_count(&f.name)?;
                    if json {
                        println!("{}", postbode::output::folder(&acc.name, &f, total, unread));
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
            threads,
        } => {
            for acc in select_accounts(&config, account.as_deref())? {
                let store = open_store(&paths, &acc.name)?;
                if threads {
                    for thread in store.threads(&folder, limit)? {
                        for (m, depth) in thread.iter().zip(postbode::output::depths(&thread)) {
                            if json {
                                let mut value = serde_json::to_value(m)?;
                                value["depth"] = depth.into();
                                println!("{}", postbode::output::with_account(&acc.name, &value)?);
                            } else {
                                println!("{}", message_line(&acc.name, m, depth));
                            }
                        }
                    }
                    continue;
                }
                for m in store.messages(&folder, limit)? {
                    if json {
                        println!("{}", postbode::output::with_account(&acc.name, &m)?);
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
            let mut client = None;
            for acc in select_accounts(&config, account.as_deref())? {
                let store = open_store(&paths, &acc.name)?;
                if bodies && lacks_bodies(&store, folder.as_deref())? {
                    eprintln!(
                        "{}: fetching missing bodies; this can take a while on a large folder",
                        acc.name
                    );
                    if let Err(e) = fetch_bodies(&mut client, &paths, &acc.name, folder.as_deref())
                    {
                        eprintln!(
                            "{}: {}; searching the bodies already stored",
                            acc.name,
                            clean(&format!("{e:#}"), false)
                        );
                    }
                }
                for m in store.search(&query, folder.as_deref(), limit)? {
                    if json {
                        println!("{}", postbode::output::with_account(&acc.name, &m)?);
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
            let body = raw_message(&paths, &acc.name, &store, &msg)?;
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
                        println!("{}", postbode::output::with_account(&acc.name, &e)?);
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
        Command::Guide => {
            print!("{}", include_str!("../../docs/src/agent-guide.md"));
            Ok(())
        }
        Command::Mcp {
            scopes,
            account,
            command,
        } => cmd_mcp(&config, &paths, &scopes, &account, command),
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
                    "{}  {folder}/{uid}  {}",
                    postbode::actions::planned_effect(&store, &m, &action)?,
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
    let command = sync::Command::Apply {
        folder: selection.folder.clone(),
        uids: selection.uids.clone(),
        action: action.clone(),
        by: postbode::actions::RULE_NAME.into(),
    };
    let reply = Client::connect_or_start(paths)?.request(&acc.name, command)?;
    let Event::ActionDone { results, .. } = reply else {
        bail!("unexpected reply from the daemon: {reply:?}");
    };
    let mut failed = 0;
    for (uid, result) in &results {
        if let Err(message) = result {
            eprintln!("{folder}/{uid}: {}", clean(message, false));
            failed += 1;
        }
    }
    let label = match action {
        Action::Trash => Action::Delete.label(),
        _ => action.label(),
    };
    println!(
        "{}: {} of {} messages",
        clean(&label, false),
        results.len() - failed,
        results.len()
    );
    if failed > 0 {
        bail!("{failed} of {} messages failed", results.len());
    }
    Ok(())
}

fn lacks_bodies(store: &Store, folder: Option<&str>) -> Result<bool> {
    let folders = match folder {
        Some(folder) => vec![folder.to_string()],
        None => store.folders()?.into_iter().map(|f| f.name).collect(),
    };
    for folder in folders {
        if store
            .messages_in_folder(&folder)?
            .iter()
            .any(|m| m.body_text.is_none())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Asks the daemon to fetch the missing bodies, connecting on the first call only.
fn fetch_bodies(
    client: &mut Option<Client>,
    paths: &Paths,
    account: &str,
    folder: Option<&str>,
) -> Result<()> {
    let fetch = sync::Command::FetchBodies {
        folder: folder.map(str::to_string),
    };
    daemon_client(client, paths)?.request(account, fetch)?;
    Ok(())
}

/// The daemon connection in `slot`, made on first use.
fn daemon_client<'a>(slot: &'a mut Option<Client>, paths: &Paths) -> Result<&'a Client> {
    match slot {
        Some(client) => Ok(client),
        None => Ok(slot.insert(Client::connect_or_start(paths)?)),
    }
}

/// The message's raw bytes; only a message whose body is not stored yet starts the daemon.
fn raw_message(paths: &Paths, account: &str, store: &Store, msg: &Message) -> Result<Vec<u8>> {
    if let Some(raw) = store.raw(&msg.folder, msg.uid)? {
        return Ok(raw);
    }
    Client::connect_or_start(paths)?.raw_message(account, store, msg)
}

fn cmd_sync(config: &Config, paths: &Paths, account: Option<&str>) -> Result<()> {
    let accounts = select_accounts(config, account)?;
    if accounts.is_empty() {
        return Ok(());
    }
    let names: Vec<&str> = accounts.iter().map(|acc| acc.name.as_str()).collect();
    let client = Client::connect_or_start(paths)?;
    let events = client.subscribe()?;
    std::thread::scope(|scope| {
        let printer = scope.spawn(|| print_sync_events(events, &names));
        let mut failed = false;
        for name in &names {
            failed |= !sync_account(&client, name);
        }
        // Closing the connection ends the event stream once the printer has drained it.
        drop(client);
        failed |= printer.join().unwrap_or(true);
        if failed {
            bail!("sync failed for at least one account or folder");
        }
        Ok(())
    })
}

/// Prints the errors and new mail of `accounts` until the stream ends; true when any error came by.
fn print_sync_events(events: Receiver<Event>, accounts: &[&str]) -> bool {
    let mut failed = false;
    for event in &events {
        let (Event::Error { account, .. } | Event::NewMail { account, .. }) = &event else {
            continue;
        };
        if accounts.contains(&account.as_str()) {
            failed |= matches!(event, Event::Error { .. });
            postbode::daemon::report(&event);
        }
    }
    failed
}

/// Syncs one account through the daemon and reports it; false when it failed.
fn sync_account(client: &Client, name: &str) -> bool {
    match client.request(name, sync::Command::SyncNow) {
        Ok(event) => {
            postbode::daemon::report(&event);
            true
        }
        Err(e) => {
            eprintln!("[{name}] error: {}", clean(&format!("{e:#}"), false));
            false
        }
    }
}

fn cmd_daemon(command: DaemonCommand, paths: &Paths) -> Result<()> {
    let Some(client) = Client::connect_any_version(paths)? else {
        println!("no daemon running");
        return Ok(());
    };
    match command {
        DaemonCommand::Status => {
            print_status(&client.status()?);
            Ok(())
        }
        DaemonCommand::Stop => stop_daemon(&client, paths),
    }
}

fn cmd_service(command: ServiceCommand, paths: &Paths) -> Result<()> {
    let text = match command {
        ServiceCommand::Install { dry_run } => postbode::daemon::service::install(paths, dry_run)?,
        ServiceCommand::Remove { dry_run } => postbode::daemon::service::remove(dry_run)?,
    };
    println!("{text}");
    Ok(())
}

fn stop_daemon(client: &Client, paths: &Paths) -> Result<()> {
    let pid = client.stop_and_wait(paths)?;
    println!("stopped the daemon (pid {pid})");
    Ok(())
}

fn print_status(status: &Status) {
    let minutes = status.uptime_secs / 60;
    println!(
        "pid {}, version {}, up {}h {}m, {} clients",
        status.pid,
        status.version,
        minutes / 60,
        minutes % 60,
        status.clients
    );
    for account in &status.accounts {
        println!("{}", account_line(account));
    }
}

fn account_line(account: &AccountStatus) -> String {
    let name = &account.name;
    match &account.activity {
        Some(activity) => format!("{name}  running  {}", activity_summary(activity)),
        None => format!("{name}  running"),
    }
}

fn activity_summary(activity: &Activity) -> String {
    match activity {
        Activity::Connecting => "connecting".into(),
        Activity::FetchingBodies { folder, .. } => {
            format!("fetching bodies {}", clean(folder, false))
        }
        Activity::FetchingHeaders { folder, .. } => {
            format!("fetching headers {}", clean(folder, false))
        }
        Activity::Idle { .. } => "idle".into(),
        Activity::ListingFolders => "listing folders".into(),
        Activity::Offline { reason, retry_at } => format!(
            "offline: {}, retrying at {}",
            clean(reason, false),
            clock_time(*retry_at)
        ),
        Activity::RunningCommand { what } => format!("running {}", clean(what, false)),
        Activity::RunningRules { folder } => format!("running rules {}", clean(folder, false)),
        Activity::SyncingFolder { folder, .. } => format!("syncing {}", clean(folder, false)),
    }
}

fn clock_time(timestamp: i64) -> String {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

fn cmd_attachment(command: AttachmentCommand, config: &Config, paths: &Paths) -> Result<()> {
    let (uid, account, folder) = match &command {
        AttachmentCommand::List {
            uid,
            account,
            folder,
            ..
        }
        | AttachmentCommand::Save {
            uid,
            account,
            folder,
            ..
        } => (*uid, account.as_deref(), folder.as_str()),
    };
    let acc = single_account(config, account)?;
    let store = open_store(paths, &acc.name)?;
    let msg = store
        .message(folder, uid)?
        .with_context(|| format!("no message {}/{uid}", clean(folder, false)))?;
    let raw = raw_message(paths, &acc.name, &store, &msg)?;
    match command {
        AttachmentCommand::List { json, .. } => {
            for a in postbode::message::attachments(&raw) {
                if json {
                    println!("{}", postbode::output::with_account(&acc.name, &a)?);
                } else {
                    println!(
                        "{}  {}  {}  {}",
                        a.index,
                        clean(&a.content_type, false),
                        a.size,
                        clean(a.name.as_deref().unwrap_or("-"), false)
                    );
                }
            }
        }
        AttachmentCommand::Save { n, dir, .. } => {
            let path = postbode::message::save_attachment(&raw, n, &dir)
                .with_context(|| format!("saving attachment {n}"))?;
            println!("{}", clean(&path.display().to_string(), false));
        }
    }
    Ok(())
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

fn truncate(s: &str, width: usize) -> String {
    let mut out: String = s.chars().take(width).collect();
    if s.chars().count() > width {
        out.pop();
        out.push('…');
    }
    out
}

#[cfg(feature = "gui")]
fn cmd_gui(config: &Config, paths: &Paths) -> Result<()> {
    if config.accounts.is_empty() {
        bail!("no accounts configured; run `postbode account add`");
    }
    postbode::gui::run(config, paths)
}

#[cfg(not(feature = "gui"))]
fn cmd_gui(_config: &Config, _paths: &Paths) -> Result<()> {
    bail!("this postbode was built without the GUI; install it with the default features")
}

#[cfg(feature = "mcp")]
fn cmd_mcp(
    config: &Config,
    paths: &Paths,
    scopes: &str,
    accounts: &[String],
    command: Option<McpCommand>,
) -> Result<()> {
    match command {
        None => postbode::mcp::run(config, paths, scopes, accounts),
        Some(McpCommand::Install {
            target,
            scopes,
            account,
            remove,
            dry_run,
        }) => {
            use postbode::mcp::install::{Target, install};
            if let Some(name) = account.iter().find(|name| config.account(name).is_none()) {
                bail!("no account named '{name}'");
            }
            let target = match target {
                InstallTarget::ClaudeCode => Target::ClaudeCode,
                InstallTarget::ClaudeDesktop => Target::ClaudeDesktop,
                InstallTarget::Json => Target::Json,
            };
            let (stdout, hint) = install(target, &scopes, &account, remove, dry_run)?;
            print!("{stdout}");
            if let Some(hint) = hint {
                eprint!("{hint}");
            }
            Ok(())
        }
    }
}

#[cfg(not(feature = "mcp"))]
fn cmd_mcp(
    _config: &Config,
    _paths: &Paths,
    _scopes: &str,
    _accounts: &[String],
    _command: Option<McpCommand>,
) -> Result<()> {
    bail!("this postbode was built without the MCP server; install it with the default features")
}

fn cmd_run(config: &Config, paths: &Paths, idle_exit: Option<u64>) -> Result<()> {
    if config.accounts.is_empty() {
        bail!("no accounts configured; run `postbode account add`");
    }
    // Test hook: lets tests idle an auto-started daemon out in seconds.
    let override_secs = std::env::var("POSTBODE_IDLE_EXIT_SECS")
        .ok()
        .and_then(|secs| secs.parse().ok());
    let options = match idle_exit.map(|secs| override_secs.unwrap_or(secs)) {
        Some(secs) => postbode::daemon::Options::auto_started(Duration::from_secs(secs)),
        None => postbode::daemon::Options::foreground(),
    };
    postbode::daemon::run(paths, options)
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
                    println!("{}", postbode::output::rule(r));
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
            if rules.iter().any(|r| !r.rule.enabled) {
                bail!("rule '{name}' is disabled; approve or enable it first");
            }
            let mut failed = false;
            let mut client = None;
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
                let command = sync::Command::ApplyRule { name: name.clone() };
                let reply = daemon_client(&mut client, paths)?.request(&acc.name, command)?;
                let Event::RuleApplied {
                    evaluated,
                    actions,
                    errors,
                    ..
                } = reply
                else {
                    bail!("unexpected reply from the daemon: {reply:?}");
                };
                failed |= !errors.is_empty();
                for message in errors {
                    postbode::daemon::report(&Event::Error {
                        account: acc.name.clone(),
                        message,
                    });
                }
                println!("{}: {actions} actions on {evaluated} messages", acc.name);
            }
            if failed {
                bail!("some actions failed; see the errors above");
            }
            Ok(())
        }
        RulesCommand::Test {
            name,
            account,
            stdin,
        } => {
            let rules = if stdin {
                let mut rule = read_rule_json()?;
                rule.enabled = true;
                postbode::rules::compile(&RuleFile { rules: vec![rule] })?
            } else {
                let mut rules = compiled_rules(paths, name.as_deref())?;
                if name.is_some() {
                    rules.iter_mut().for_each(|r| r.rule.enabled = true);
                }
                rules
            };
            for acc in select_accounts(config, account.as_deref())? {
                let store = open_store(paths, &acc.name)?;
                print_planned_actions(&rules, &store, acc, &acc.identity()?)?;
            }
            Ok(())
        }
        RulesCommand::Schema => {
            print!("{}", postbode::rules::schema());
            Ok(())
        }
        RulesCommand::Propose { by } => {
            let rule = read_rule_json()?;
            let name = rule.name.clone();
            let by = by.map_or_else(|| "cli".to_string(), |who| format!("cli:{who}"));
            postbode::rules::edit::propose(&paths.rules_file(), rule, &by)?;
            println!(
                "proposed '{}'; it stays disabled until a human runs `postbode rules approve`",
                clean(&name, false)
            );
            Ok(())
        }
        RulesCommand::Approve { name } => {
            postbode::rules::edit::approve(&paths.rules_file(), &name)?;
            for acc in &config.accounts {
                open_store(paths, &acc.name)?.restart_rule_clock(&name, sync::now())?;
            }
            println!(
                "enabled '{}'; it acts on mail that arrives from now on",
                clean(&name, false)
            );
            Ok(())
        }
        RulesCommand::Reject { name } => {
            postbode::rules::edit::reject(&paths.rules_file(), &name)?;
            println!("removed proposal '{}'", clean(&name, false));
            Ok(())
        }
    }
}

fn read_rule_json() -> Result<Rule> {
    serde_json::from_reader(io::stdin().lock()).context("reading one rule as JSON from stdin")
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
    for p in postbode::actions::planned(rules, store, account, identity, sync::now())? {
        println!(
            "{}\t{}/{}\t{}\t{}",
            clean(&p.rule, false),
            clean(&p.message.folder, false),
            p.message.uid,
            clean(&p.action.label(), false),
            clean(p.message.subject.as_deref().unwrap_or(""), false)
        );
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
            let command = sync::Command::Restore {
                file: std::path::absolute(&file)?,
            };
            let reply = Client::connect_or_start(paths)?.request(&acc.name, command)?;
            let Event::Restored { folder, .. } = reply else {
                bail!("unexpected reply from the daemon: {reply:?}");
            };
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
    let ca_file = {
        let answer = prompt("Extra trusted CA file (PEM, absolute path) [none]")?;
        if answer.is_empty() {
            None
        } else {
            Some(PathBuf::from(answer))
        }
    };
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
        ca_file,
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
    fn cli_reference_is_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/cli.md");
        let generated = clap_markdown::help_markdown::<Cli>();
        if std::env::var_os("POSTBODE_BLESS").is_some() {
            std::fs::write(path, &generated).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            generated,
            "docs/src/cli.md is stale; run POSTBODE_BLESS=1 cargo test"
        );
    }
}
