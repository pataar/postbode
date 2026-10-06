//! The only code that writes rules.toml. Each edit is validated as a whole file before it replaces the old one.
use std::io;
use std::path::Path;

use toml_edit::{ArrayOfTables, DocumentMut, value};

use crate::paths::write_atomic;
use crate::rules::{Rule, RuleFile, RulesError, compile, parse};

/// Appends `rule` disabled and attributed to `by`, so it only acts once a human approves it.
pub fn propose(path: &Path, mut rule: Rule, by: &str) -> Result<(), RulesError> {
    rule.enabled = false;
    rule.proposed_by = Some(by.to_string());
    let snippet = toml::to_string(&RuleFile { rules: vec![rule] })
        .map_err(|e| RulesError::Parse(e.to_string()))?;
    let mut text = read(path)?;
    if !text.is_empty() {
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
    }
    text.push_str(&snippet);
    save(path, &text)
}

pub fn approve(path: &Path, name: &str) -> Result<(), RulesError> {
    edit(path, name, |rules, index| {
        let table = rules.get_mut(index).expect("index comes from position()");
        if !is_disabled(table) {
            return Err(invalid(name, "is already enabled"));
        }
        table["enabled"] = value(true);
        Ok(())
    })
}

pub fn reject(path: &Path, name: &str) -> Result<(), RulesError> {
    edit(path, name, |rules, index| {
        let table = rules.get(index).expect("index comes from position()");
        if !is_disabled(table) || !table.contains_key("proposed_by") {
            return Err(invalid(
                name,
                "is not a pending proposal; edit rules.toml to remove it",
            ));
        }
        rules.remove(index);
        Ok(())
    })
}

fn is_disabled(table: &toml_edit::Table) -> bool {
    table.get("enabled").and_then(|v| v.as_bool()) == Some(false)
}

fn edit(
    path: &Path,
    name: &str,
    change: impl FnOnce(&mut ArrayOfTables, usize) -> Result<(), RulesError>,
) -> Result<(), RulesError> {
    let mut doc: DocumentMut = read(path)?
        .parse()
        .map_err(|e: toml_edit::TomlError| RulesError::Parse(e.to_string()))?;
    let rules = doc
        .get_mut("rules")
        .and_then(|item| item.as_array_of_tables_mut())
        .ok_or_else(|| invalid(name, "no such rule"))?;
    let index = rules
        .iter()
        .position(|t| t.get("name").and_then(|v| v.as_str()) == Some(name))
        .ok_or_else(|| invalid(name, "no such rule"))?;
    change(rules, index)?;
    save(path, &doc.to_string())
}

fn save(path: &Path, text: &str) -> Result<(), RulesError> {
    compile(&parse(text)?)?;
    write_atomic(path, text.as_bytes())?;
    Ok(())
}

fn read(path: &Path) -> Result<String, RulesError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.into()),
    }
}

fn invalid(name: &str, reason: &str) -> RulesError {
    RulesError::Invalid {
        rule: name.to_string(),
        reason: reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{Action, TextMatch, load};

    const HUMAN: &str = "# my rules\n[[rules]]\nname = \"keep\" # do not touch\nmatch.seen = true\nactions = [\"flag\"]\n";

    fn proposal(name: &str) -> Rule {
        serde_json::from_str(&format!(
            r#"{{"name": "{name}", "match": {{"subject": {{"contains": "code"}}}}, "actions": [{{"move": "Codes"}}, "mark_read"]}}"#
        ))
        .unwrap()
    }

    fn rules_file(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        std::fs::write(&path, text).unwrap();
        (dir, path)
    }

    #[test]
    fn propose_appends_disabled_and_keeps_the_rest_of_the_file() {
        let (_dir, path) = rules_file(HUMAN);
        propose(&path, proposal("codes"), "cli:test").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with(HUMAN), "{text}");
        let added = &parse(&text).unwrap().rules[1];
        assert_eq!(
            (added.enabled, added.proposed_by.as_deref()),
            (false, Some("cli:test"))
        );
        assert_eq!(
            added.actions,
            [Action::Move("Codes".into()), Action::MarkRead]
        );
    }

    #[test]
    fn propose_into_a_missing_file_creates_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        propose(&path, proposal("codes"), "cli").unwrap();
        assert_eq!(load(&path).unwrap().rules.len(), 1);
    }

    #[test]
    fn invalid_proposal_leaves_the_file_untouched() {
        let (_dir, path) = rules_file(HUMAN);
        assert!(
            propose(&path, proposal("keep"), "cli").is_err(),
            "duplicate name"
        );
        let mut bad = proposal("bad");
        bad.matches.subject = Some(TextMatch {
            regex: Some("(".into()),
            ..Default::default()
        });
        assert!(propose(&path, bad, "cli").is_err(), "invalid regex");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), HUMAN);
    }

    #[test]
    fn approve_enables_in_place_and_keeps_comments() {
        let (_dir, path) = rules_file(HUMAN);
        propose(&path, proposal("codes"), "cli").unwrap();
        approve(&path, "codes").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("# my rules") && text.contains("# do not touch"),
            "{text}"
        );
        assert!(parse(&text).unwrap().rules.iter().all(|r| r.enabled));
        assert!(approve(&path, "codes").is_err(), "already enabled");
        assert!(approve(&path, "missing").is_err());
    }

    #[test]
    fn reject_removes_only_pending_proposals() {
        let (_dir, path) = rules_file(HUMAN);
        propose(&path, proposal("codes"), "cli").unwrap();
        assert!(
            reject(&path, "keep").is_err(),
            "a human rule is not a proposal"
        );
        reject(&path, "codes").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(parse(&text).unwrap().rules.len(), 1);
        assert!(text.contains("# do not touch"), "{text}");
    }
}
