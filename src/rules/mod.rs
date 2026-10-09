pub mod apply;
pub mod edit;
pub mod engine;

use std::io;
use std::path::Path;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum RulesError {
    #[error("rules.toml: {0}")]
    Parse(String),
    #[error("rule '{rule}': {reason}")]
    Invalid { rule: String, reason: String },
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Store(#[from] crate::store::StoreError),
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleFile {
    #[serde(default)]
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Unique name; renaming a rule restarts its clock
    pub name: String,
    /// Only for this account; default all accounts
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// The folder the rule watches; default INBOX
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Disabled rules are skipped; proposals start disabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Who proposed the rule; set by `rules propose`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposed_by: Option<String>,
    /// Conditions that must all hold; at least one
    #[serde(rename = "match")]
    pub matches: Match,
    /// What to do; at least one
    pub actions: Vec<Action>,
}

impl Rule {
    /// What the rule does, as JSON: editing any of it restarts the rule's clock, so a widened rule never acts on older mail.
    pub fn definition(&self) -> Result<String, RulesError> {
        serde_json::to_string(&(&self.account, &self.folder, &self.matches, &self.actions)).map_err(
            |e| RulesError::Invalid {
                rule: self.name.clone(),
                reason: e.to_string(),
            },
        )
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Match {
    /// The From header
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<TextMatch>,
    /// The To header
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<TextMatch>,
    /// The Cc header
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cc: Option<TextMatch>,
    /// The subject
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<TextMatch>,
    /// The plain-text body; HTML mail is converted
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<TextMatch>,
    /// Any header, by name; a list must all match
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<OneOrMany<HeaderMatch>>,
    /// Arrived at least this long ago, e.g. 30m, 1h, 2days
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub older_than: Option<String>,
    /// Read (true) or unread (false)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<bool>,
    /// To, Cc or Delivered-To holds your address or an alias
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_me: Option<bool>,
    /// Sent to this alias; * is a wildcard
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// Carries this IMAP keyword, which Thunderbird shows as a tag (e.g. $label1); case-insensitive
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Holds when no entry holds; an entry holds when all its conditions do
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub none: Option<Vec<Match>>,
    /// Holds when at least one of these holds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub any: Option<Vec<Match>>,
}

/// One value, or a list of them
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged, expecting = "one value or a list of them")]
pub enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    pub fn as_slice(&self) -> &[T] {
        match self {
            OneOrMany::One(one) => std::slice::from_ref(one),
            OneOrMany::Many(many) => many,
        }
    }
}

/// Exactly one of contains, equals, regex
#[derive(Debug, Default, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextMatch {
    /// Case-insensitive substring; a list matches any of them
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<OneOrMany<String>>,
    /// The whole value, case-insensitive; on from, to and cc also any single address; a list matches any of them
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<OneOrMany<String>>,
    /// Rust regex syntax; (?i) makes it case-insensitive
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HeaderMatch {
    /// Header name, e.g. List-Id
    pub name: String,
    /// Case-insensitive substring; a list matches any of them
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<OneOrMany<String>>,
    /// The whole value, case-insensitive; on from, to and cc also any single address; a list matches any of them
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<OneOrMany<String>>,
    /// Rust regex syntax; (?i) makes it case-insensitive
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Back up as .eml locally, then remove from the server; stops later rules
    Delete,
    /// Set \Seen
    MarkRead,
    /// Set \Flagged
    Flag,
    /// Move to the server's Archive folder
    Archive,
    /// Notify even when the message was moved
    Notify,
    /// Never notify
    Silent,
    #[serde(skip)]
    MarkUnread,
    #[serde(skip)]
    Unflag,
    /// A user delete: to the Trash folder, or deleted with a backup when there is none or the message is already in it.
    #[serde(skip)]
    Trash,
    /// Move to this folder, creating it if needed
    #[serde(rename = "move")]
    Move(String),
    /// Add this IMAP keyword, which Thunderbird shows as a tag (e.g. $label1)
    #[serde(rename = "tag")]
    Tag(String),
}

impl Action {
    /// The label of the command a user gave for this action: `delete` sends `Trash` outside the Trash folder.
    pub fn command_label(&self) -> String {
        match self {
            Action::Trash => Action::Delete.label(),
            _ => self.label(),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Action::Delete => "delete".into(),
            Action::MarkRead => "mark_read".into(),
            Action::Flag => "flag".into(),
            Action::Archive => "archive".into(),
            Action::Notify => "notify".into(),
            Action::Silent => "silent".into(),
            Action::MarkUnread => "mark_unread".into(),
            Action::Unflag => "unflag".into(),
            Action::Trash => "trash".into(),
            Action::Move(folder) => format!("move:{folder}"),
            Action::Tag(keyword) => format!("tag:{keyword}"),
        }
    }
}

fn default_true() -> bool {
    true
}

/// JSON Schema of rules.toml; a proposal for `rules propose` is one entry of `rules`.
pub fn schema() -> String {
    let schema = schemars::schema_for!(RuleFile);
    // Schema's own Serialize puts `$schema` and `title` first, unlike `{:#}` on its value. Serializing an
    // in-memory schema to a String can't fail: no I/O, and every map key is a string.
    #[allow(clippy::expect_used)]
    let text = serde_json::to_string_pretty(&schema).expect("a schema serializes");
    text + "\n"
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
pub(crate) enum Matcher {
    Contains(Vec<String>),
    Equals(Vec<String>),
    Regex(regex::Regex),
}

impl Matcher {
    pub(crate) fn is_match(&self, text: &str) -> bool {
        match self {
            Matcher::Contains(needles) => {
                let text = text.to_lowercase();
                needles.iter().any(|needle| text.contains(needle))
            }
            Matcher::Equals(wanted) => wanted.iter().any(|w| text.trim().eq_ignore_ascii_case(w)),
            Matcher::Regex(re) => re.is_match(text),
        }
    }

    /// For from, to and cc: `equals` also accepts any single bare address, so a display name or a second recipient
    /// does not stop the match.
    pub(crate) fn is_address_match(&self, field: &str) -> bool {
        match self {
            Matcher::Equals(wanted) => {
                self.is_match(field)
                    || crate::message::bare_addresses(field)
                        .iter()
                        .any(|address| wanted.iter().any(|w| address.eq_ignore_ascii_case(w)))
            }
            _ => self.is_match(field),
        }
    }

    fn build(
        rule: &str,
        path: &str,
        contains: &Option<OneOrMany<String>>,
        equals: &Option<OneOrMany<String>>,
        regex: &Option<String>,
    ) -> Result<Matcher, RulesError> {
        let invalid = |reason: String| RulesError::Invalid {
            rule: rule.to_string(),
            reason,
        };
        let values =
            |list: &Option<OneOrMany<String>>| list.as_ref().map(|l| l.as_slice().to_vec());
        let (contains, equals) = (values(contains), values(equals));
        let empty_list = [&contains, &equals]
            .into_iter()
            .flatten()
            .any(Vec::is_empty);
        if empty_list
            || [&contains, &equals]
                .into_iter()
                .flatten()
                .flatten()
                .chain(regex)
                .any(|value| value.trim().is_empty())
        {
            return Err(invalid(format!("{path} must not be empty")));
        }
        match (contains, equals, regex) {
            (Some(c), None, None) => Ok(Matcher::Contains(
                c.iter().map(|value| value.to_lowercase()).collect(),
            )),
            (None, Some(e), None) => Ok(Matcher::Equals(
                e.iter().map(|value| value.trim().to_string()).collect(),
            )),
            (None, None, Some(r)) => regex::Regex::new(r)
                .map(Matcher::Regex)
                .map_err(|e| invalid(format!("{path}: invalid regex: {e}"))),
            _ => Err(invalid(format!(
                "{path} needs exactly one of contains, equals, regex"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompiledRule {
    pub rule: Rule,
    pub first_seen_at: i64,
    pub(crate) conditions: Conditions,
}

/// A compiled `Match`: every set condition must hold.
#[derive(Debug, Clone, Default)]
pub(crate) struct Conditions {
    pub(crate) from: Option<Matcher>,
    pub(crate) to: Option<Matcher>,
    pub(crate) cc: Option<Matcher>,
    pub(crate) subject: Option<Matcher>,
    pub(crate) body: Option<Matcher>,
    pub(crate) headers: Vec<(String, Matcher)>,
    pub(crate) older_than: Option<Duration>,
    pub(crate) seen: Option<bool>,
    pub(crate) to_me: Option<bool>,
    pub(crate) alias: Option<globset::GlobMatcher>,
    pub(crate) tag: Option<String>,
    pub(crate) none: Vec<Conditions>,
    pub(crate) any: Vec<Conditions>,
}

impl Conditions {
    fn needs_body(&self) -> bool {
        self.body.is_some()
            || self.none.iter().any(Conditions::needs_body)
            || self.any.iter().any(Conditions::needs_body)
    }
}

impl CompiledRule {
    pub fn folder(&self) -> &str {
        self.rule.folder.as_deref().unwrap_or("INBOX")
    }

    pub fn applies_to_account(&self, account: &str) -> bool {
        self.rule.account.as_deref().is_none_or(|a| a == account)
    }

    pub fn needs_body(&self) -> bool {
        self.conditions.needs_body()
    }
}

/// Longest rule name, account or folder; agents propose rules, so no field may grow without bound.
const MAX_NAME_CHARS: usize = 255;

/// Deepest nesting of `not` and `any`; Thunderbird's filters need two levels at most.
const MAX_DEPTH: usize = 8;

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
    if rule
        .actions
        .iter()
        .any(|action| matches!(action, Action::Move(folder) if folder.trim().is_empty()))
    {
        return Err(invalid("move folder must not be empty"));
    }
    let strings = [
        ("name", Some(&rule.name)),
        ("account", rule.account.as_ref()),
        ("folder", rule.folder.as_ref()),
    ]
    .into_iter()
    .filter_map(|(field, value)| Some((field, value?)))
    .chain(rule.actions.iter().filter_map(|action| match action {
        Action::Move(folder) => Some(("move folder", folder)),
        _ => None,
    }));
    for (field, value) in strings {
        if value.chars().any(char::is_control) {
            return Err(invalid(&format!(
                "{field} must not contain control characters"
            )));
        }
        if value.trim().is_empty() {
            return Err(invalid(&format!("{field} must not be empty")));
        }
        if value.chars().count() > MAX_NAME_CHARS {
            return Err(invalid(&format!(
                "{field} must be at most {MAX_NAME_CHARS} characters"
            )));
        }
    }
    for action in &rule.actions {
        if let Action::Tag(keyword) = action {
            check_keyword(keyword).map_err(|reason| invalid(&format!("tag {reason}")))?;
        }
    }
    Ok(CompiledRule {
        rule: rule.clone(),
        first_seen_at: 0,
        conditions: compile_match(&rule.name, &rule.matches, "match", 0)?,
    })
}

fn compile_match(
    rule: &str,
    m: &Match,
    path: &str,
    depth: usize,
) -> Result<Conditions, RulesError> {
    let invalid = |reason: String| RulesError::Invalid {
        rule: rule.to_string(),
        reason,
    };
    if depth > MAX_DEPTH {
        return Err(invalid(format!("{path} is nested deeper than {MAX_DEPTH}")));
    }
    let headers = m
        .header
        .as_ref()
        .map(OneOrMany::as_slice)
        .unwrap_or_default();
    let has_condition = m.from.is_some()
        || m.to.is_some()
        || m.cc.is_some()
        || m.subject.is_some()
        || m.body.is_some()
        || !headers.is_empty()
        || m.older_than.is_some()
        || m.seen.is_some()
        || m.to_me.is_some()
        || m.alias.is_some()
        || m.tag.is_some()
        || m.none.is_some()
        || m.any.is_some();
    if !has_condition {
        let reason = match &m.header {
            Some(_) => format!("{path}.header must not be empty"),
            None => format!("{path} needs at least one condition"),
        };
        return Err(invalid(reason));
    }
    if m.alias
        .as_ref()
        .is_some_and(|alias| alias.trim().is_empty())
    {
        return Err(invalid(format!("{path}.alias must not be empty")));
    }
    if let Some(tag) = &m.tag {
        check_keyword(tag).map_err(|reason| invalid(format!("{path}.tag {reason}")))?;
    }
    let text = |field: &str, t: &Option<TextMatch>| -> Result<Option<Matcher>, RulesError> {
        t.as_ref()
            .map(|t| {
                Matcher::build(
                    rule,
                    &format!("{path}.{field}"),
                    &t.contains,
                    &t.equals,
                    &t.regex,
                )
            })
            .transpose()
    };
    let headers = headers
        .iter()
        .map(|h| {
            if h.name.trim().is_empty() {
                return Err(invalid(format!("{path}.header.name must not be empty")));
            }
            Matcher::build(
                rule,
                &format!("{path}.header"),
                &h.contains,
                &h.equals,
                &h.regex,
            )
            .map(|matcher| (h.name.clone(), matcher))
        })
        .collect::<Result<_, _>>()?;
    let older_than = m
        .older_than
        .as_ref()
        .map(|s| {
            humantime::parse_duration(s).map_err(|e| invalid(format!("{path}.older_than: {e}")))
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
                .map_err(|e| invalid(format!("{path}.alias: {e}")))
        })
        .transpose()?;
    let branches = |key: &str, list: &Option<Vec<Match>>| -> Result<Vec<Conditions>, RulesError> {
        match list.as_deref() {
            None => Ok(Vec::new()),
            Some([]) => Err(invalid(format!("{path}.{key} must not be empty"))),
            Some(entries) => entries
                .iter()
                .enumerate()
                .map(|(i, entry)| {
                    compile_match(rule, entry, &format!("{path}.{key}[{i}]"), depth + 1)
                })
                .collect(),
        }
    };
    let any = branches("any", &m.any)?;
    let none = branches("none", &m.none)?;
    Ok(Conditions {
        from: text("from", &m.from)?,
        to: text("to", &m.to)?,
        cc: text("cc", &m.cc)?,
        subject: text("subject", &m.subject)?,
        body: text("body", &m.body)?,
        headers,
        older_than,
        seen: m.seen,
        to_me: m.to_me,
        alias,
        tag: m.tag.clone(),
        none,
        any,
    })
}

/// An IMAP keyword is an atom (RFC 3501): printable ASCII without specials. It goes into the STORE command verbatim, so
/// this check is what keeps a proposed tag from injecting IMAP syntax.
pub(crate) fn check_keyword(keyword: &str) -> Result<(), String> {
    const ATOM_SPECIALS: &[char] = &['(', ')', '{', ' ', '%', '*', '"', '\\', ']'];
    if keyword.is_empty() {
        return Err("must not be empty".into());
    }
    if keyword.len() > MAX_NAME_CHARS {
        return Err(format!("must be at most {MAX_NAME_CHARS} characters"));
    }
    if !keyword
        .chars()
        .all(|c| c.is_ascii_graphic() && !ATOM_SPECIALS.contains(&c))
    {
        return Err(
            "must be printable ASCII without spaces, backslashes or ( ) { } % * \" ]".into(),
        );
    }
    if keyword.eq_ignore_ascii_case(engine::RESTORED_KEYWORD) {
        return Err("is reserved for restored mail".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_file_is_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/src/rules.schema.json");
        if std::env::var_os("POSTVAK_BLESS").is_some() {
            std::fs::write(path, schema()).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            schema(),
            "docs/src/rules.schema.json is stale; run POSTVAK_BLESS=1 cargo test"
        );
    }

    #[test]
    fn schema_describes_rules_and_rejects_unknown_keys() {
        let text = schema();
        let schema: serde_json::Value = serde_json::from_str(&text).unwrap();
        let rule = &schema["$defs"]["Rule"];
        assert_eq!(rule["additionalProperties"], false);
        assert!(rule["properties"]["match"].is_object());
        assert!(
            !text.contains("mark_unread"),
            "CLI-only actions stay out of the schema"
        );
    }

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
        assert_eq!(
            github.matches.header.as_ref().unwrap().as_slice()[0].name,
            "List-Id"
        );
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
        assert_eq!(
            compiled[0].conditions.older_than,
            Some(Duration::from_secs(3600))
        );
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
        assert!(Matcher::Contains(vec!["github".into()]).is_match("Lists GitHub Dev"));
        assert!(Matcher::Equals(vec!["a@b.c".into()]).is_match("A@B.C"));
        assert!(!Matcher::Equals(vec!["a@b.c".into()]).is_match("xa@b.c"));
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

    #[test]
    fn rejects_blank_and_overlong_names_and_folders() {
        let long = "a".repeat(MAX_NAME_CHARS + 1);
        let rule = |extra: &str, name: &str, target: &str| {
            format!(
                "[[rules]]\nname = \"{name}\"\n{extra}match.seen = true\nactions = [{{ move = \"{target}\" }}]\n"
            )
        };
        let cases = [
            (
                rule("folder = \"\"\n", "x", "Done"),
                "folder must not be empty",
            ),
            (
                rule("folder = \"   \"\n", "x", "Done"),
                "folder must not be empty",
            ),
            (
                rule("account = \" \"\n", "x", "Done"),
                "account must not be empty",
            ),
            (
                rule(&format!("folder = \"{long}\"\n"), "x", "Done"),
                "folder must be at most 255",
            ),
            (rule("", &long, "Done"), "name must be at most 255"),
            (rule("", "x", &long), "move folder must be at most 255"),
        ];
        for (text, want) in cases {
            match parse(&text).and_then(|f| compile(&f)) {
                Err(RulesError::Invalid { reason, .. }) => {
                    assert!(reason.contains(want), "{reason} should contain {want}")
                }
                other => panic!("expected Invalid ({want}), got {other:?}"),
            }
        }
        // Exactly at the limit is fine.
        let at_limit = "a".repeat(MAX_NAME_CHARS);
        compile(&parse(&rule("", &at_limit, &at_limit)).unwrap()).unwrap();
    }

    #[test]
    fn rejects_control_characters_in_agent_strings() {
        let cases = [
            "[[rules]]\nname = \"x\"\nmatch.seen = true\nactions = [{ move = \"Codes\\u001b[2J\" }]\n",
            "[[rules]]\nname = \"x\\u001b]0;pwned\"\nmatch.seen = true\nactions = [\"flag\"]\n",
        ];
        for text in cases {
            match parse(text).and_then(|f| compile(&f)) {
                Err(RulesError::Invalid { reason, .. }) => {
                    assert!(reason.contains("control characters"), "{reason}")
                }
                other => panic!("expected Invalid for {text}, got {other:?}"),
            }
        }
    }

    const NESTED: &str = r#"
[[rules]]
name = "tagged"
match.from = { contains = "shop" }
match.none = [{ subject = { contains = ["receipt", "invoice"] } }]
match.any = [{ to_me = true }, { tag = "$label1" }]
match.header = [{ name = "List-Id", contains = "shop" }, { name = "X-Mailer", contains = "bulk" }]
actions = [{ tag = "$label4" }, { move = "Shop" }]
"#;

    #[test]
    fn parses_none_any_tag_value_and_header_lists() {
        let file = parse(NESTED).unwrap();
        let rule = &file.rules[0];
        assert_eq!(
            rule.actions,
            vec![Action::Tag("$label4".into()), Action::Move("Shop".into())]
        );
        assert_eq!(rule.matches.any.as_ref().unwrap().len(), 2);
        assert_eq!(rule.matches.none.as_ref().unwrap().len(), 1);
        assert_eq!(rule.matches.header.as_ref().unwrap().as_slice().len(), 2);
        let compiled = compile(&file).unwrap();
        assert_eq!(compiled[0].conditions.headers.len(), 2);
        let again = parse(&toml::to_string(&file).unwrap()).unwrap();
        assert_eq!(again.rules[0].actions, rule.actions);
        assert_eq!(again.rules[0].matches.any.as_ref().unwrap().len(), 2);
        assert_eq!(
            toml::to_string(&again).unwrap(),
            toml::to_string(&file).unwrap()
        );
    }

    #[test]
    fn single_header_table_still_parses() {
        let compiled = compile(&parse(SAMPLE).unwrap()).unwrap();
        assert_eq!(compiled[1].conditions.headers.len(), 1);
    }

    #[test]
    fn nested_matches_are_validated_with_their_path() {
        let rule =
            |matcher: &str| format!("[[rules]]\nname = \"x\"\n{matcher}\nactions = [\"flag\"]\n");
        let deep = format!(
            "match.none = {}{}",
            "[{ none = ".repeat(MAX_DEPTH),
            "[{ seen = true }]".to_string() + &" }]".repeat(MAX_DEPTH)
        );
        let cases = [
            (
                rule("match.none = [{}]"),
                "match.none[0] needs at least one condition",
            ),
            (rule("match.none = []"), "match.none must not be empty"),
            (
                rule("match.subject = { contains = [] }"),
                "match.subject must not be empty",
            ),
            (
                rule("match.subject = { contains = [\"a\", \" \"] }"),
                "match.subject must not be empty",
            ),
            (rule("match.header = []"), "match.header must not be empty"),
            (rule("match.any = []"), "match.any must not be empty"),
            (
                rule("match.any = [{}]"),
                "match.any[0] needs at least one condition",
            ),
            (
                rule("match.none = [{ subject = { regex = \"(\" } }]"),
                "match.none[0].subject: invalid regex",
            ),
            (
                rule("match.any = [{ seen = true }, { from = { contains = \"\" } }]"),
                "match.any[1].from must not be empty",
            ),
            (rule(&deep), "nested deeper than"),
        ];
        for (text, want) in cases {
            match parse(&text).and_then(|f| compile(&f)) {
                Err(RulesError::Invalid { reason, .. }) => {
                    assert!(reason.contains(want), "{reason} should contain {want}")
                }
                other => panic!("expected Invalid ({want}), got {other:?}"),
            }
        }
    }

    #[test]
    fn tags_must_be_imap_keywords() {
        let action = |tag: &str| {
            format!(
                "[[rules]]\nname = \"x\"\nmatch.seen = true\nactions = [{{ tag = \"{tag}\" }}]\n"
            )
        };
        let matcher = |tag: &str| {
            format!("[[rules]]\nname = \"x\"\nmatch.tag = \"{tag}\"\nactions = [\"flag\"]\n")
        };
        for bad in [
            "",
            "two words",
            "\\\\Seen",
            "a)b",
            "x\\r\\nA1 LOGOUT",
            "naïve",
            "$PostvakRestored",
            "$postvakRESTORED",
        ] {
            for text in [action(bad), matcher(bad)] {
                assert!(
                    matches!(
                        parse(&text).and_then(|f| compile(&f)),
                        Err(RulesError::Invalid { .. })
                    ),
                    "{text}"
                );
            }
        }
        for good in ["$label1", "Important", "$Junk", "todo-later"] {
            compile(&parse(&action(good)).unwrap()).unwrap();
            compile(&parse(&matcher(good)).unwrap()).unwrap();
        }
    }

    #[test]
    fn cli_only_actions_are_not_rule_actions() {
        for action in ["mark_unread", "trash", "unflag"] {
            let text =
                format!("[[rules]]\nname = \"x\"\nmatch.seen = true\nactions = [\"{action}\"]\n");
            assert!(parse(&text).is_err(), "{action}");
        }
    }
}
