//! The Rules view, and in Task 9 the Activity and Trash views: what replaces the list and body columns.
use std::path::Path;

use eframe::egui;

use crate::message::clean;
use crate::rules::{self, Rule, RuleFile};

use super::app::{App, UiAction};

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

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    use egui_kittest::kittest::Queryable;

    use crate::gui::app::View;
    use crate::gui::test_support::Fixture;
    use crate::sync::Command;

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
        assert!(
            std::fs::read_to_string(fx.paths.rules_file())
                .unwrap()
                .starts_with("# keep me\n")
        );
        assert_eq!(wires.sent(), [("work".to_string(), Command::SyncNow)]);
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
        assert_eq!(wires.sent(), [("work".to_string(), Command::SyncNow)]);
    }

    #[test]
    fn an_edit_from_outside_reloads_the_view_and_syncs() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, wires) = rules_view(&fx);
        let more = format!(
            "{RULES}\n[[rules]]\nname = \"receipts\"\nmatch.subject = {{ contains = \"receipt\" }}\nactions = [{{ move = \"Receipts\" }}]\n"
        );
        std::fs::write(fx.paths.rules_file(), more).unwrap();
        touch(&fx.paths.rules_file());
        poll(&mut harness);
        assert!(harness.query_by_label("receipts").is_some());
        assert_eq!(wires.sent(), [("work".to_string(), Command::SyncNow)]);
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
    fn a_config_change_asks_for_a_restart() {
        let fx = Fixture::new(&["work"]);
        let (mut harness, _wires) = fx.harness();
        touch(&fx.paths.config_file());
        poll(&mut harness);
        assert!(
            harness
                .query_by_label("config.toml changed — restart Postbode to apply")
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
                .query_by_label_contains("restart Postbode")
                .is_none()
        );
    }
}
