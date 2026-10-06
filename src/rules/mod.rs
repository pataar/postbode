pub mod apply;
pub mod engine;

use std::io;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum RulesError {
    #[error("rules.toml: {0}")]
    Parse(String),
    #[error("rule '{rule}': {reason}")]
    Invalid { rule: String, reason: String },
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleFile {
    #[serde(default)]
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_by: Option<String>,
    #[serde(rename = "match")]
    pub matches: Match,
    pub actions: Vec<Action>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Match {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<TextMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<TextMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<TextMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<TextMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<TextMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<HeaderMatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub older_than: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_me: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextMatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderMatch {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Delete,
    MarkRead,
    Flag,
    Archive,
    Notify,
    Silent,
    #[serde(rename = "move")]
    Move(String),
}

impl Action {
    pub fn label(&self) -> String {
        match self {
            Action::Delete => "delete".into(),
            Action::MarkRead => "mark_read".into(),
            Action::Flag => "flag".into(),
            Action::Archive => "archive".into(),
            Action::Notify => "notify".into(),
            Action::Silent => "silent".into(),
            Action::Move(folder) => format!("move:{folder}"),
        }
    }
}

fn default_true() -> bool {
    true
}

pub fn parse(text: &str) -> Result<RuleFile, RulesError> {
    toml::from_str(text).map_err(|e| RulesError::Parse(e.to_string()))
}

pub fn load(path: &Path) -> Result<RuleFile, RulesError> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(RuleFile::default()),
        Err(e) => Err(e.into()),
    }
}

#[derive(Debug, Clone)]
// used by rules::engine (next task)
#[allow(dead_code)]
pub(crate) enum Matcher {
    Contains(String),
    Equals(String),
    Regex(regex::Regex),
}

impl Matcher {
    // used by rules::engine (next task)
    #[allow(dead_code)]
    pub(crate) fn is_match(&self, text: &str) -> bool {
        match self {
            Matcher::Contains(needle) => text.to_lowercase().contains(needle),
            Matcher::Equals(wanted) => text.trim().eq_ignore_ascii_case(wanted),
            Matcher::Regex(re) => re.is_match(text),
        }
    }

    fn build(
        rule: &str,
        field: &str,
        contains: &Option<String>,
        equals: &Option<String>,
        regex: &Option<String>,
    ) -> Result<Matcher, RulesError> {
        let invalid = |reason: String| RulesError::Invalid {
            rule: rule.to_string(),
            reason,
        };
        if [contains, equals, regex]
            .into_iter()
            .flatten()
            .any(|value| value.trim().is_empty())
        {
            return Err(invalid(format!("match.{field} must not be empty")));
        }
        match (contains, equals, regex) {
            (Some(c), None, None) => Ok(Matcher::Contains(c.to_lowercase())),
            (None, Some(e), None) => Ok(Matcher::Equals(e.clone())),
            (None, None, Some(r)) => regex::Regex::new(r)
                .map(Matcher::Regex)
                .map_err(|e| invalid(format!("match.{field}: invalid regex: {e}"))),
            _ => Err(invalid(format!(
                "match.{field} needs exactly one of contains, equals, regex"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
// used by rules::engine (next task)
#[allow(dead_code)]
pub struct CompiledRule {
    pub rule: Rule,
    pub first_seen_at: i64,
    pub(crate) from: Option<Matcher>,
    pub(crate) to: Option<Matcher>,
    pub(crate) cc: Option<Matcher>,
    pub(crate) subject: Option<Matcher>,
    pub(crate) body: Option<Matcher>,
    pub(crate) header: Option<(String, Matcher)>,
    pub older_than: Option<Duration>,
    pub(crate) alias: Option<globset::GlobMatcher>,
}

impl CompiledRule {
    pub fn folder(&self) -> &str {
        self.rule.folder.as_deref().unwrap_or("INBOX")
    }

    pub fn applies_to_account(&self, account: &str) -> bool {
        self.rule.account.as_deref().is_none_or(|a| a == account)
    }

    pub fn needs_body(&self) -> bool {
        self.body.is_some()
    }
}

pub fn compile(file: &RuleFile) -> Result<Vec<CompiledRule>, RulesError> {
    let mut names = std::collections::HashSet::new();
    file.rules
        .iter()
        .map(|rule| compile_rule(rule, &mut names))
        .collect()
}

fn compile_rule(
    rule: &Rule,
    names: &mut std::collections::HashSet<String>,
) -> Result<CompiledRule, RulesError> {
    let invalid = |reason: &str| RulesError::Invalid {
        rule: rule.name.clone(),
        reason: reason.to_string(),
    };
    if rule.name.trim().is_empty() {
        return Err(invalid("name must not be empty"));
    }
    if !names.insert(rule.name.clone()) {
        return Err(invalid("duplicate rule name"));
    }
    if rule.actions.is_empty() {
        return Err(invalid("actions must not be empty"));
    }
    let m = &rule.matches;
    let has_condition = m.from.is_some()
        || m.to.is_some()
        || m.cc.is_some()
        || m.subject.is_some()
        || m.body.is_some()
        || m.header.is_some()
        || m.older_than.is_some()
        || m.seen.is_some()
        || m.to_me.is_some()
        || m.alias.is_some();
    if !has_condition {
        return Err(invalid("match needs at least one condition"));
    }
    if m.header.as_ref().is_some_and(|h| h.name.trim().is_empty()) {
        return Err(invalid("match.header.name must not be empty"));
    }
    if m.alias
        .as_ref()
        .is_some_and(|alias| alias.trim().is_empty())
    {
        return Err(invalid("match.alias must not be empty"));
    }
    if rule
        .actions
        .iter()
        .any(|action| matches!(action, Action::Move(folder) if folder.trim().is_empty()))
    {
        return Err(invalid("move folder must not be empty"));
    }
    let text = |field: &str, t: &Option<TextMatch>| -> Result<Option<Matcher>, RulesError> {
        t.as_ref()
            .map(|t| Matcher::build(&rule.name, field, &t.contains, &t.equals, &t.regex))
            .transpose()
    };
    let header = m
        .header
        .as_ref()
        .map(|h| {
            Matcher::build(&rule.name, "header", &h.contains, &h.equals, &h.regex)
                .map(|mm| (h.name.clone(), mm))
        })
        .transpose()?;
    let older_than = m
        .older_than
        .as_ref()
        .map(|s| {
            humantime::parse_duration(s).map_err(|e| invalid(&format!("match.older_than: {e}")))
        })
        .transpose()?;
    let alias = m
        .alias
        .as_ref()
        .map(|pattern| {
            globset::GlobBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .map(|g| g.compile_matcher())
                .map_err(|e| invalid(&format!("match.alias: {e}")))
        })
        .transpose()?;
    Ok(CompiledRule {
        rule: rule.clone(),
        first_seen_at: 0,
        from: text("from", &m.from)?,
        to: text("to", &m.to)?,
        cc: text("cc", &m.cc)?,
        subject: text("subject", &m.subject)?,
        body: text("body", &m.body)?,
        header,
        older_than,
        alias,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const SAMPLE: &str = r#"
[[rules]]
name = "purge sign-in codes"
account = "work"
match.from = { regex = "no-?reply@" }
match.subject = { regex = "(?i)sign.?in|verification code|magic link" }
match.older_than = "1h"
match.seen = true
actions = ["delete"]

[[rules]]
name = "github to folder"
match.header = { name = "List-Id", contains = "github.com" }
actions = [{ move = "Lists/GitHub" }, "mark_read"]

[[rules]]
name = "shop alias"
enabled = false
proposed_by = "cli:test"
match.alias = "*@shop.example.com"
actions = [{ move = "Shopping" }, "notify"]
"#;

    #[test]
    fn parses_sample() {
        let file = parse(SAMPLE).unwrap();
        assert_eq!(file.rules.len(), 3);
        let github = &file.rules[1];
        assert_eq!(
            github.actions,
            vec![Action::Move("Lists/GitHub".into()), Action::MarkRead]
        );
        assert_eq!(github.matches.header.as_ref().unwrap().name, "List-Id");
        assert!(!file.rules[2].enabled);
        assert_eq!(file.rules[2].proposed_by.as_deref(), Some("cli:test"));
    }

    #[test]
    fn compile_sets_defaults() {
        let compiled = compile(&parse(SAMPLE).unwrap()).unwrap();
        assert_eq!(compiled[0].folder(), "INBOX");
        assert!(compiled[0].applies_to_account("work"));
        assert!(!compiled[0].applies_to_account("home"));
        assert!(compiled[1].applies_to_account("anything"));
        assert!(!compiled[0].needs_body());
        assert_eq!(compiled[0].older_than, Some(Duration::from_secs(3600)));
    }

    #[test]
    fn rejects_bad_regex_with_rule_name() {
        let bad = SAMPLE.replace("no-?reply@", "(unclosed");
        match parse(&bad).and_then(|f| compile(&f)) {
            Err(RulesError::Invalid { rule, reason }) => {
                assert_eq!(rule, "purge sign-in codes");
                assert!(reason.contains("regex"), "{reason}");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn rejects_text_match_without_exactly_one_operator() {
        let two = SAMPLE.replace(
            r#"{ regex = "no-?reply@" }"#,
            r#"{ regex = "a", contains = "b" }"#,
        );
        assert!(matches!(
            parse(&two).and_then(|f| compile(&f)),
            Err(RulesError::Invalid { .. })
        ));
        let none = SAMPLE.replace(r#"{ regex = "no-?reply@" }"#, "{ }");
        assert!(matches!(
            parse(&none).and_then(|f| compile(&f)),
            Err(RulesError::Invalid { .. })
        ));
    }

    #[test]
    fn rejects_rule_without_conditions_or_actions_or_unknown_action() {
        let empty_match = "[[rules]]\nname = \"x\"\nmatch = {}\nactions = [\"delete\"]\n";
        assert!(matches!(
            parse(empty_match).and_then(|f| compile(&f)),
            Err(RulesError::Invalid { .. })
        ));
        let no_actions = "[[rules]]\nname = \"x\"\nmatch.seen = true\nactions = []\n";
        assert!(matches!(
            parse(no_actions).and_then(|f| compile(&f)),
            Err(RulesError::Invalid { .. })
        ));
        let unknown = "[[rules]]\nname = \"x\"\nmatch.seen = true\nactions = [\"explode\"]\n";
        assert!(matches!(parse(unknown), Err(RulesError::Parse(_))));
    }

    #[test]
    fn rejects_bad_duration_and_duplicate_names() {
        let bad = SAMPLE.replace("\"1h\"", "\"soon\"");
        assert!(matches!(
            parse(&bad).and_then(|f| compile(&f)),
            Err(RulesError::Invalid { .. })
        ));
        let dup = SAMPLE.replace(
            "name = \"github to folder\"",
            "name = \"purge sign-in codes\"",
        );
        assert!(matches!(
            parse(&dup).and_then(|f| compile(&f)),
            Err(RulesError::Invalid { .. })
        ));
    }

    #[test]
    fn load_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            load(&dir.path().join("rules.toml"))
                .unwrap()
                .rules
                .is_empty()
        );
    }

    #[test]
    fn matcher_semantics() {
        assert!(Matcher::Contains("github".into()).is_match("Lists GitHub Dev"));
        assert!(Matcher::Equals("a@b.c".into()).is_match("A@B.C"));
        assert!(!Matcher::Equals("a@b.c".into()).is_match("xa@b.c"));
        assert!(Matcher::Regex(regex::Regex::new("^no-?reply").unwrap()).is_match("noreply@x"));
    }

    #[test]
    fn rejects_unknown_top_level_key() {
        let typo = "[[rule]]\nname = \"x\"\nmatch.seen = true\nactions = [\"flag\"]\n";
        assert!(matches!(parse(typo), Err(RulesError::Parse(_))));
    }

    #[test]
    fn rejects_empty_values() {
        let rule = |matcher: &str, action: &str| {
            format!("[[rules]]\nname = \"x\"\n{matcher}\nactions = [{action}]\n")
        };
        let cases = [
            rule("match.subject = { contains = \"\" }", "\"delete\""),
            rule("match.subject = { equals = \"  \" }", "\"delete\""),
            rule("match.subject = { regex = \"\" }", "\"delete\""),
            rule(
                "match.header = { name = \"\", contains = \"a\" }",
                "\"delete\"",
            ),
            rule("match.seen = true", "{ move = \"\" }"),
            rule("match.alias = \"\"", "\"delete\""),
        ];
        for text in cases {
            assert!(
                matches!(
                    parse(&text).and_then(|f| compile(&f)),
                    Err(RulesError::Invalid { .. })
                ),
                "{text}"
            );
        }
    }
}
