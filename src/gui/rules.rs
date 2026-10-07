//! The Rules, Activity and Trash views: what replaces the list and body columns.
use std::io::Read;
use std::path::Path;

use eframe::egui;

use crate::message::{clean, parse_headers};
use crate::paths::Paths;
use crate::rules::{self, Rule, RuleFile};
use crate::store::LogEntry;
use crate::time::local_time;
use crate::trash::{Trash, TrashEntry};

use super::app::{Account, App, UiAction};

/// Entries shown in the Activity view.
const LOG_LIMIT: u32 = 500;

/// How much of a backup is read to find its sender and subject.
const HEADER_BYTES: u64 = 64 * 1024;

pub(crate) struct RulesState {
    pub error: Option<String>,
    pub rules: Vec<Rule>,
}

impl RulesState {
    /// The file's rules, or the previous ones when it does not parse. A rule that does not compile is reported but
    /// still listed, so it can be switched off.
    pub fn load(path: &Path, previous: Vec<Rule>) -> RulesState {
        match rules::load(path) {
            Ok(file) => RulesState {
                error: rules::compile(&file).err().map(|e| e.to_string()),
                rules: file.rules,
            },
            Err(e) => RulesState {
                error: Some(e.to_string()),
                rules: previous,
            },
        }
    }
}

fn is_pending(rule: &Rule) -> bool {
    !rule.enabled && rule.proposed_by.is_some()
}

fn toml_source(rule: &Rule) -> String {
    toml::to_string(&RuleFile {
        rules: vec![rule.clone()],
    })
    .unwrap_or_else(|e| e.to_string())
}

pub(crate) fn show_rules(app: &App, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    if let Some(error) = &app.rules.error {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!(
                "{} — sync keeps the previous rules until this is fixed",
                clean(error, false)
            ),
        );
    }
    if ui.button("Open rules.toml").clicked() {
        actions.push(UiAction::OpenRulesFile);
    }
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            let pending: Vec<&Rule> = app
                .rules
                .rules
                .iter()
                .filter(|rule| is_pending(rule))
                .collect();
            if !pending.is_empty() {
                ui.heading("Proposals");
            }
            for rule in pending {
                ui.group(|ui| {
                    let by = rule.proposed_by.as_deref().unwrap_or_default();
                    ui.strong(format!(
                        "{} · proposed by {}",
                        clean(&rule.name, false),
                        clean(by, false)
                    ));
                    ui.label(egui::RichText::new(clean(&toml_source(rule), true)).monospace());
                    ui.horizontal(|ui| {
                        if ui.button("Approve").clicked() {
                            actions.push(UiAction::ApproveRule(rule.name.clone()));
                        }
                        if ui.button("Reject").clicked() {
                            actions.push(UiAction::RejectRule(rule.name.clone()));
                        }
                    });
                });
            }
            ui.heading("Rules");
            if app.rules.rules.is_empty() {
                ui.weak("rules.toml has no rules yet.");
            }
            egui::Grid::new("rules")
                .striped(true)
                .num_columns(3)
                .show(ui, |ui| {
                    for rule in &app.rules.rules {
                        let mut enabled = rule.enabled;
                        if ui
                            .checkbox(&mut enabled, clean(&rule.name, false))
                            .changed()
                        {
                            actions.push(UiAction::SetRuleEnabled(rule.name.clone(), enabled));
                        }
                        let account = rule
                            .account
                            .as_deref()
                            .map_or("every account".into(), |a| clean(a, false));
                        let folder = rule
                            .folder
                            .as_deref()
                            .map_or("INBOX".into(), |f| clean(f, false));
                        ui.label(format!("{account} · {folder}"));
                        ui.label(
                            rule.proposed_by
                                .as_deref()
                                .map(|by| format!("proposed by {}", clean(by, false)))
                                .unwrap_or_default(),
                        );
                        ui.end_row();
                    }
                });
        });
    actions
}

pub(crate) struct TrashRow {
    pub account: usize,
    pub entry: TrashEntry,
    pub from: String,
    pub subject: String,
}

/// Every account's rule and action log, newest first.
pub(crate) fn activity_log(accounts: &[Account]) -> Vec<(String, LogEntry)> {
    let mut entries = Vec::new();
    for account in accounts {
        let Ok(store) = &account.store else { continue };
        match store.log(LOG_LIMIT) {
            Ok(found) => {
                entries.extend(found.into_iter().map(|entry| (account.name.clone(), entry)));
            }
            Err(e) => log::warn!("[{}] could not read the log: {e}", account.name),
        }
    }
    entries.sort_by_key(|(_, entry)| std::cmp::Reverse(entry.at));
    entries.truncate(LOG_LIMIT as usize);
    entries
}

/// Every account's `.eml` backups, newest first, with sender and subject from their headers.
pub(crate) fn trash_rows(accounts: &[Account], paths: &Paths) -> Vec<TrashRow> {
    let mut rows = Vec::new();
    for (index, account) in accounts.iter().enumerate() {
        match Trash::new(paths.trash_dir(&account.name)).list() {
            Ok(entries) => rows.extend(entries.into_iter().map(|entry| {
                let (from, subject) = eml_summary(&entry.path);
                TrashRow {
                    account: index,
                    entry,
                    from,
                    subject,
                }
            })),
            Err(e) => log::warn!("[{}] could not list the trash: {e}", account.name),
        }
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.entry.saved_at));
    rows
}

/// Sender and subject of a backup; reads only the start of the file.
fn eml_summary(path: &Path) -> (String, String) {
    let mut head = Vec::new();
    if let Ok(file) = std::fs::File::open(path) {
        let _ = file.take(HEADER_BYTES).read_to_end(&mut head);
    }
    let parsed = parse_headers(&head);
    (
        parsed.from.unwrap_or_default(),
        parsed.subject.unwrap_or_default(),
    )
}

pub(crate) fn show_activity(app: &App, ui: &mut egui::Ui) -> Vec<UiAction> {
    if app.activity_log.is_empty() {
        ui.weak("No rule or action has run yet.");
        return Vec::new();
    }
    egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
        egui::Grid::new("log")
            .striped(true)
            .num_columns(6)
            .show(ui, |ui| {
                for heading in ["Time", "Account", "Rule", "Action", "Folder", "Subject"] {
                    ui.strong(heading);
                }
                ui.end_row();
                for (account, entry) in &app.activity_log {
                    ui.label(local_time(entry.at, "%Y-%m-%d %H:%M"));
                    ui.label(account);
                    ui.label(clean(&entry.rule_name, false));
                    ui.label(clean(&entry.action, false));
                    ui.label(clean(&entry.folder, false));
                    ui.label(
                        entry
                            .subject
                            .as_deref()
                            .map(|subject| clean(subject, false))
                            .unwrap_or_default(),
                    );
                    ui.end_row();
                }
            });
    });
    Vec::new()
}

pub(crate) fn show_trash(app: &App, ui: &mut egui::Ui) -> Vec<UiAction> {
    let mut actions = Vec::new();
    if app.trash.is_empty() {
        ui.weak(
            "Trash is empty. Deleted mail is kept here as .eml for the account's retention period.",
        );
        return actions;
    }
    egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
        egui::Grid::new("trash")
            .striped(true)
            .num_columns(6)
            .show(ui, |ui| {
                for heading in ["Deleted", "Account", "Folder", "From", "Subject", ""] {
                    ui.strong(heading);
                }
                ui.end_row();
                for row in &app.trash {
                    ui.label(local_time(row.entry.saved_at, "%Y-%m-%d %H:%M"));
                    ui.label(&app.accounts[row.account].name);
                    ui.label(clean(&row.entry.folder, false));
                    ui.label(clean(&row.from, false));
                    ui.label(clean(&row.subject, false));
                    if ui.button("Restore").clicked() {
                        actions.push(UiAction::Restore(row.account, row.entry.path.clone()));
                    }
                    ui.end_row();
                }
            });
    });
    actions
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    use eframe::egui;
    use egui_kittest::kittest::Queryable;

    use crate::gui::app::View;
    use crate::gui::test_support::Fixture;
    use crate::store::LogEntry;
    use crate::sync::{Command, Event};
    use crate::trash::Trash;

    const RULES: &str = "# keep me\n[[rules]]\nname = \"newsletters\"\nmatch.from = { contains = \"news@\" }\nactions = [\"archive\"]\n\n[[rules]]\nname = \"codes\"\nenabled = false\nproposed_by = \"agent\"\nmatch.subject = { contains = \"code\" }\nactions = [\"delete\"]\n";

    fn touch(path: &Path) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
    }

    fn rules_view(
        fx: &Fixture,
    ) -> (
        egui_kittest::Harness<'static, crate::gui::App>,
        crate::gui::test_support::Wires,
    ) {
        std::fs::write(fx.paths.rules_file(), RULES).unwrap();
        let (mut harness, wires) = fx.harness();
        harness.state_mut().select_view(View::Rules);
        harness.run();
        (harness, wires)
    }

    fn poll(harness: &mut egui_kittest::Harness<'static, crate::gui::App>) {
        harness.input_mut().time = Some(5.0);
        harness.step();
        harness.run();
    }

    fn enabled(fx: &Fixture, name: &str) -> bool {
        let file = crate::rules::load(&fx.paths.rules_file()).unwrap();
        file.rules.iter().find(|r| r.name == name).unwrap().enabled
    }

    #[test]
    fn a_proposal_shows_its_toml_and_approve_enables_it() {
        let fx = Fixture::new(&["work"]);
        // A clock left from when the rule ran before must not let it act on older mail.
        fx.store("work").restart_rule_clock("codes", 1).unwrap();
        let (mut harness, wires) = rules_view(&fx);
        assert!(
            harness
                .query_by_label_contains("codes · proposed by agent")
                .is_some()
        );
        assert!(
            harness
                .query_by_label_contains("name = \"codes\"")
                .is_some()
        );
        harness.get_by_label("Approve").click();
        harness.run();
        assert!(enabled(&fx, "codes"));
        assert!(fx.store("work").rule_first_seen("codes", 0).unwrap() > 1);
        assert!(
            std::fs::read_to_string(fx.paths.rules_file())
                .unwrap()
                .starts_with("# keep me\n")
        );
        assert!(wires.sent().is_empty());
        assert!(harness.query_by_label("Approve").is_none());
    }

    #[test]
    fn the_switch_turns_a_rule_off_and_keeps_comments() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, wires) = rules_view(&fx);
        harness.get_by_label("newsletters").click();
        harness.run();
        assert!(!enabled(&fx, "newsletters"));
        assert!(
            std::fs::read_to_string(fx.paths.rules_file())
                .unwrap()
                .starts_with("# keep me\n")
        );
        assert!(wires.sent().is_empty());
    }

    #[test]
    fn an_edit_from_outside_reloads_the_view() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, wires) = rules_view(&fx);
        let more = format!(
            "{RULES}\n[[rules]]\nname = \"receipts\"\nmatch.subject = {{ contains = \"receipt\" }}\nactions = [{{ move = \"Receipts\" }}]\n"
        );
        std::fs::write(fx.paths.rules_file(), more).unwrap();
        touch(&fx.paths.rules_file());
        poll(&mut harness);
        assert!(harness.query_by_label("receipts").is_some());
        assert!(wires.sent().is_empty());
    }

    #[test]
    fn a_parse_error_shows_a_banner_and_keeps_the_rules() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = rules_view(&fx);
        std::fs::write(fx.paths.rules_file(), "[[rules]\nbroken").unwrap();
        touch(&fx.paths.rules_file());
        poll(&mut harness);
        assert!(
            harness
                .query_by_label_contains("sync keeps the previous rules")
                .is_some()
        );
        assert!(harness.query_by_label("newsletters").is_some());
    }

    #[test]
    fn a_config_change_asks_to_reopen_the_window() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        touch(&fx.paths.config_file());
        poll(&mut harness);
        assert!(
            harness
                .query_by_label("config.toml changed — reopen the window to show it")
                .is_some()
        );
    }

    #[test]
    fn the_theme_switch_raises_no_banner() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        harness.get_by_label("Dark").click();
        harness.run();
        poll(&mut harness);
        assert!(
            harness
                .query_by_label_contains("reopen the window")
                .is_none()
        );
    }

    fn logged(fx: &Fixture, account: &str, at: i64, subject: &str) {
        let entry = LogEntry {
            id: 0,
            at,
            rule_name: "newsletters".into(),
            folder: "INBOX".into(),
            uid: 1,
            message_id: None,
            subject: Some(subject.into()),
            action: "archive".into(),
            trash_file: None,
        };
        fx.store(account).log_action(&entry).unwrap();
    }

    fn backup(fx: &Fixture) -> std::path::PathBuf {
        let dir = fx.paths.trash_dir("work");
        std::fs::create_dir_all(&dir).unwrap();
        let raw = b"From: Shop <shop@example.com>\r\nSubject: your receipt\r\n\r\nthanks\r\n";
        Trash::new(dir)
            .save("INBOX", 7, raw, 1_790_000_000)
            .unwrap()
    }

    #[test]
    fn activity_merges_every_account_newest_first() {
        let fx = Fixture::new(&["home", "work"]);
        logged(&fx, "work", 100, "work old");
        logged(&fx, "home", 200, "home new");
        logged(&fx, "work", 300, "work newest");
        let (mut harness, _wires) = fx.harness();
        harness.state_mut().select_view(View::Activity);
        harness.run();
        let order: Vec<(&str, Option<&str>)> = harness
            .state()
            .activity_log
            .iter()
            .map(|(account, e)| (account.as_str(), e.subject.as_deref()))
            .collect();
        assert_eq!(
            order,
            [
                ("work", Some("work newest")),
                ("home", Some("home new")),
                ("work", Some("work old"))
            ]
        );
        assert!(harness.query_by_label("work newest").is_some());
    }

    #[test]
    fn trash_lists_backups_and_restore_sends_the_command() {
        let fx = Fixture::new(&["work"]);
        let file = backup(&fx);
        let (mut harness, wires) = fx.harness();
        harness.state_mut().select_view(View::Trash);
        harness.run();
        assert!(harness.query_by_label("your receipt").is_some());
        assert!(harness.query_by_label("Shop <shop@example.com>").is_some());
        harness.get_by_label("Restore").click();
        harness.run();
        assert_eq!(
            wires.sent(),
            [("work".to_string(), Command::Restore { file: file.clone() })]
        );
        std::fs::remove_file(&file).unwrap();
        wires
            .events
            .send(Event::Restored {
                account: "work".into(),
                folder: "INBOX".into(),
                request: 0,
            })
            .unwrap();
        harness.run();
        assert!(harness.query_by_label("your receipt").is_none());
        assert!(
            harness
                .state()
                .history
                .iter()
                .any(|line| line.text == "work: restored to INBOX")
        );
    }

    #[test]
    fn question_mark_shows_the_keys_and_escape_closes_them() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        harness.event(egui::Event::Text("?".into()));
        harness.run();
        assert!(harness.state().help_open);
        assert!(harness.query_by_label("search this account").is_some());
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(!harness.state().help_open);
    }
}
