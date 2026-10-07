//! The only code that writes rules.toml. Each edit is validated as a whole file before it replaces the old one.
use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

use toml_edit::{ArrayOfTables, DocumentMut, Item, Value};

use crate::paths::{Paths, create_private_dir, write_atomic};
use crate::rules::{Rule, RuleFile, RulesError, compile, parse};
use crate::store::Store;

/// Appends `rule` disabled and attributed to `by`, so it only acts once a human approves it.
pub fn propose(path: &Path, mut rule: Rule, by: &str) -> Result<(), RulesError> {
    rule.enabled = false;
    rule.proposed_by = Some(by.to_string());
    let snippet = toml::to_string(&RuleFile { rules: vec![rule] })
        .map_err(|e| RulesError::Parse(e.to_string()))?;
    let _lock = lock(path)?;
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

/// Turns a rule on or off, keeping the file's comments and layout.
pub fn set_enabled(path: &Path, name: &str, enabled: bool) -> Result<(), RulesError> {
    edit(path, name, |rules, index| {
        let table = rules
            .get_mut(index)
            .ok_or_else(|| invalid(name, "no such rule"))?;
        write_enabled(table, enabled);
        Ok(None)
    })
}

pub fn approve(path: &Path, name: &str) -> Result<(), RulesError> {
    edit(path, name, |rules, index| {
        let table = rules
            .get_mut(index)
            .ok_or_else(|| invalid(name, "no such rule"))?;
        if !is_disabled(table) {
            return Err(invalid(name, "is already enabled"));
        }
        write_enabled(table, true);
        Ok(None)
    })
}

/// Approves `name` and restarts its clock in each account's store, so it acts only on mail that arrives from now on,
/// even where an older clock of the rule survived.
pub fn approve_from_now(
    paths: &Paths,
    accounts: &[String],
    name: &str,
    now: i64,
) -> Result<(), RulesError> {
    approve(&paths.rules_file(), name)?;
    for account in accounts {
        Store::open_account(paths, account)?.restart_rule_clock(name, now)?;
    }
    Ok(())
}

fn write_enabled(table: &mut toml_edit::Table, enabled: bool) {
    let mut value = Value::from(enabled);
    if let Some(old) = table.get("enabled").and_then(|item| item.as_value()) {
        *value.decor_mut() = old.decor().clone();
    }
    table["enabled"] = Item::Value(value);
}

pub fn reject(path: &Path, name: &str) -> Result<(), RulesError> {
    edit(path, name, |rules, index| {
        let table = rules
            .get(index)
            .ok_or_else(|| invalid(name, "no such rule"))?;
        if !is_disabled(table) || !table.contains_key("proposed_by") {
            return Err(invalid(
                name,
                "is not a pending proposal; edit rules.toml to remove it",
            ));
        }
        // Comments above a table belong to its decor; keep the human's, minus the blank line `propose` added.
        let prefix = table
            .decor()
            .prefix()
            .and_then(|p| p.as_str())
            .unwrap_or("");
        let mut orphaned = prefix
            .strip_suffix('\n')
            .filter(|p| p.ends_with('\n'))
            .unwrap_or(prefix)
            .to_string();
        rules.remove(index);
        match rules.get_mut(index) {
            Some(next) => {
                let next_prefix = next.decor().prefix().and_then(|p| p.as_str()).unwrap_or("");
                orphaned.push_str(next_prefix);
                next.decor_mut().set_prefix(orphaned);
                Ok(None)
            }
            None => Ok(Some(orphaned)),
        }
    })
}

fn is_disabled(table: &toml_edit::Table) -> bool {
    table.get("enabled").and_then(|v| v.as_bool()) == Some(false)
}

fn edit(
    path: &Path,
    name: &str,
    change: impl FnOnce(&mut ArrayOfTables, usize) -> Result<Option<String>, RulesError>,
) -> Result<(), RulesError> {
    let _lock = lock(path)?;
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
    if let Some(orphaned) = change(rules, index)? {
        let trailing = format!("{orphaned}{}", doc.trailing().as_str().unwrap_or(""));
        doc.set_trailing(trailing);
    }
    save(path, &doc.to_string())
}

/// An exclusive lock on `.rules.lock` beside rules.toml, held until dropped, so concurrent edits cannot lose each other's.
fn lock(path: &Path) -> Result<File, RulesError> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent"))?;
    create_private_dir(dir)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".rules.lock"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.lock()?;
    Ok(file)
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

    #[test]
    fn reject_keeps_comments_above_the_proposal() {
        let human = format!("{HUMAN}\n# TODO newsletters rule\n# [[rules]]\n# name = \"draft\"\n");
        let (_dir, path) = rules_file(&human);
        propose(&path, proposal("codes"), "cli").unwrap();
        reject(&path, "codes").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), human);

        propose(&path, proposal("codes"), "cli").unwrap();
        propose(&path, proposal("other"), "cli").unwrap();
        reject(&path, "codes").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("# TODO newsletters rule\n# [[rules]]\n# name = \"draft\""),
            "{text}"
        );
        assert_eq!(parse(&text).unwrap().rules.len(), 2);
    }

    #[test]
    fn reject_of_the_last_table_keeps_comment_order() {
        let (_dir, path) = rules_file(
            "# head\n[[rules]]\nname = \"keep\"\nmatch.seen = true\nactions = [\"flag\"]\n\n# above\n",
        );
        propose(&path, proposal("codes"), "cli").unwrap();
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("# trailing\n");
        std::fs::write(&path, text).unwrap();
        reject(&path, "codes").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let at = |needle: &str| text.find(needle).unwrap();
        assert!(
            at("# head") < at("# above") && at("# above") < at("# trailing"),
            "{text}"
        );
        assert_eq!(parse(&text).unwrap().rules.len(), 1);
    }

    #[test]
    fn concurrent_proposals_are_all_kept() {
        let (_dir, path) = rules_file(HUMAN);
        std::thread::scope(|scope| {
            for i in 0..8 {
                let path = &path;
                scope.spawn(move || propose(path, proposal(&format!("p{i}")), "cli").unwrap());
            }
        });
        let names: std::collections::HashSet<String> = load(&path)
            .unwrap()
            .rules
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(names.len(), 9, "{names:?}");
    }

    #[test]
    fn approve_keeps_a_comment_on_the_enabled_line() {
        let (_dir, path) = rules_file(
            "[[rules]]\nname = \"x\"\nenabled = false # waiting for review\nproposed_by = \"cli\"\nmatch.seen = true\nactions = [\"flag\"]\n",
        );
        approve(&path, "x").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("enabled = true # waiting for review\n"),
            "{text}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn lock_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, path) = rules_file("");
        let lock_path = path.parent().unwrap().join(".rules.lock");
        std::fs::write(&lock_path, "").unwrap();
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        drop(lock(&path).unwrap());
        let mode = std::fs::metadata(&lock_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn set_enabled_toggles_one_rule_and_keeps_the_rest() {
        const TWO_RULES: &str = "# keep me\n[[rules]]\nname = \"a\"   # inline\nmatch.seen = true\nactions = [\"flag\"]\n\n[[rules]]\nname = \"b\"\nenabled = false # off for now\nmatch.seen = true\nactions = [\"flag\"]\n";

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rules.toml");
        std::fs::write(&path, TWO_RULES).unwrap();
        let b_section =
            |text: &str| text[text.find("[[rules]]\nname = \"b\"").unwrap()..].to_string();

        set_enabled(&path, "a", false).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("# keep me\n[[rules]]\nname = \"a\"   # inline\n"),
            "{text}"
        );
        assert!(text.contains("enabled = false"));
        assert_eq!(b_section(&text), b_section(TWO_RULES));

        set_enabled(&path, "b", true).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("enabled = true # off for now"), "{text}");
        assert!(set_enabled(&path, "missing", true).is_err());
    }
}
