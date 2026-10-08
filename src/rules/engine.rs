use crate::config::Identity;
use crate::message::{bare_addresses, header_value};
use crate::rules::{Action, CompiledRule, Conditions};
use crate::store::Message;

/// Set by `trash restore`; rules never act on, or notify about, mail carrying it.
pub const RESTORED_KEYWORD: &str = "$PostbodeRestored";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    ApplyExisting,
}

pub struct Context<'a> {
    pub account: &'a str,
    pub identity: &'a Identity,
    pub now: i64,
    pub mode: Mode,
    pub notify_default: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlannedAction {
    pub rule: String,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    pub actions: Vec<PlannedAction>,
    pub notify: bool,
}

impl Plan {
    pub fn moves_or_deletes(&self) -> bool {
        self.actions
            .iter()
            .any(|a| matches!(a.action, Action::Delete | Action::Move(_) | Action::Archive))
    }
}

pub fn folder_needs_body(rules: &[CompiledRule], account: &str, folder: &str) -> bool {
    rules.iter().any(|r| {
        r.rule.enabled && r.applies_to_account(account) && r.folder() == folder && r.needs_body()
    })
}

pub fn evaluate(rules: &[CompiledRule], msg: &Message, ctx: &Context) -> Plan {
    if msg
        .flags
        .split(' ')
        .any(|flag| flag.eq_ignore_ascii_case(RESTORED_KEYWORD))
    {
        return Plan::default();
    }
    let mut plan = Plan::default();
    let mut explicit_notify: Option<bool> = None;
    for rule in rules {
        if !rule.rule.enabled
            || !rule.applies_to_account(ctx.account)
            || rule.folder() != msg.folder
        {
            continue;
        }
        if ctx.mode == Mode::Normal && msg.internaldate < rule.first_seen_at {
            continue;
        }
        // An unknown body is neither a match nor a mismatch, so `none` cannot turn a failed fetch into a hit.
        if rule.needs_body() && msg.body_text.is_none() {
            continue;
        }
        if !matches(&rule.conditions, msg, ctx) {
            continue;
        }
        let mut deleted = false;
        for action in &rule.rule.actions {
            match action {
                Action::Notify => explicit_notify = Some(true),
                Action::Silent => explicit_notify = Some(false),
                _ => plan.actions.push(PlannedAction {
                    rule: rule.rule.name.clone(),
                    action: action.clone(),
                }),
            }
            if *action == Action::Delete {
                deleted = true;
            }
        }
        if deleted {
            break;
        }
    }
    let deleted = plan.actions.iter().any(|a| a.action == Action::Delete);
    plan.notify = !deleted
        && match explicit_notify {
            Some(explicit) => explicit,
            None => ctx.notify_default && !plan.moves_or_deletes(),
        };
    plan
}

fn field(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("")
}

fn matches(c: &Conditions, msg: &Message, ctx: &Context) -> bool {
    if let Some(m) = &c.from
        && !m.is_address_match(field(&msg.from_addr))
    {
        return false;
    }
    if let Some(m) = &c.to
        && !m.is_address_match(field(&msg.to_addr))
    {
        return false;
    }
    if let Some(m) = &c.cc
        && !m.is_address_match(field(&msg.cc_addr))
    {
        return false;
    }
    if let Some(m) = &c.subject
        && !m.is_match(field(&msg.subject))
    {
        return false;
    }
    if let Some(m) = &c.body
        && !m.is_match(field(&msg.body_text))
    {
        return false;
    }
    for (name, m) in &c.headers {
        match header_value(&msg.headers, name) {
            Some(value) if m.is_match(&value) => {}
            _ => return false,
        }
    }
    if let Some(min_age) = c.older_than
        && ctx.now - msg.internaldate < i64::try_from(min_age.as_secs()).unwrap_or(i64::MAX)
    {
        return false;
    }
    if let Some(seen) = c.seen
        && msg.is_seen() != seen
    {
        return false;
    }
    if let Some(tag) = &c.tag
        && !msg
            .flags
            .split_whitespace()
            .any(|flag| flag.eq_ignore_ascii_case(tag))
    {
        return false;
    }
    if c.to_me.is_some() || c.alias.is_some() {
        let recipients: Vec<String> = [&msg.to_addr, &msg.cc_addr, &msg.delivered_to]
            .into_iter()
            .flat_map(|f| bare_addresses(field(f)))
            .collect();
        if let Some(to_me) = c.to_me
            && recipients.iter().any(|r| ctx.identity.is_me(r)) != to_me
        {
            return false;
        }
        if let Some(glob) = &c.alias
            && !recipients.iter().any(|r| glob.is_match(r))
        {
            return false;
        }
    }
    if c.none.iter().any(|entry| matches(entry, msg, ctx)) {
        return false;
    }
    c.any.is_empty() || c.any.iter().any(|branch| matches(branch, msg, ctx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AccountConfig, PasswordSource};
    use crate::rules::{compile, parse};

    const HOUR: i64 = 3600;

    fn identity() -> Identity {
        let password = PasswordSource::Keyring { keyring: true };
        AccountConfig {
            aliases: vec!["*@shop.example.com".into()],
            ..AccountConfig::new("work", "h", "pieter@example.com", password)
        }
        .identity()
        .unwrap()
    }

    fn rules(toml: &str, first_seen_at: i64) -> Vec<CompiledRule> {
        let mut compiled = compile(&parse(toml).unwrap()).unwrap();
        for r in &mut compiled {
            r.first_seen_at = first_seen_at;
        }
        compiled
    }

    fn msg(from: &str, to: &str, subject: &str, internaldate: i64, seen: bool) -> Message {
        let list_id = if from.contains("github") {
            "List-Id: dev <dev.github.com>\r\n"
        } else {
            ""
        };
        let headers = format!("From: {from}\r\nTo: {to}\r\nSubject: {subject}\r\n{list_id}\r\n");
        Message {
            folder: "INBOX".into(),
            uid: 1,
            message_id: Some("m1@x".into()),
            from_addr: Some(from.into()),
            to_addr: Some(to.into()),
            thread_id: "m1@x".into(),
            subject: Some(subject.into()),
            date: Some(internaldate),
            internaldate,
            flags: if seen { "\\Seen".into() } else { String::new() },
            headers: headers.into_bytes(),
            ..Default::default()
        }
    }

    fn ctx<'a>(id: &'a Identity, now: i64) -> Context<'a> {
        Context {
            account: "work",
            identity: id,
            now,
            mode: Mode::Normal,
            notify_default: true,
        }
    }

    const PURGE: &str = r#"
[[rules]]
name = "purge"
match.from = { regex = "no-?reply@" }
match.subject = { regex = "(?i)sign.?in|verification code" }
match.older_than = "1h"
match.seen = true
actions = ["delete"]
"#;

    #[test]
    fn restored_mail_is_left_alone() {
        let id = identity();
        let rules = rules(
            "[[rules]]\nname = \"codes\"\nmatch.subject = { contains = \"code\" }\nactions = [\"delete\"]\n",
            0,
        );
        let mut m = msg("noreply@x.com", "me@example.com", "your code", 100, true);
        assert!(!evaluate(&rules, &m, &ctx(&id, 200)).actions.is_empty());
        m.flags = format!("\\Seen {RESTORED_KEYWORD}");
        assert_eq!(evaluate(&rules, &m, &ctx(&id, 200)), Plan::default());
    }

    #[test]
    fn sign_in_code_deleted_only_when_old_and_seen() {
        let id = identity();
        let rules = rules(PURGE, 0);
        let now = 10 * HOUR;
        let fresh = msg(
            "noreply@login.example",
            "pieter@example.com",
            "Your sign-in code",
            now - 600,
            true,
        );
        assert!(evaluate(&rules, &fresh, &ctx(&id, now)).actions.is_empty());
        let old_unseen = msg(
            "noreply@login.example",
            "pieter@example.com",
            "Your sign-in code",
            now - 2 * HOUR,
            false,
        );
        assert!(
            evaluate(&rules, &old_unseen, &ctx(&id, now))
                .actions
                .is_empty()
        );
        let old_seen = msg(
            "noreply@login.example",
            "pieter@example.com",
            "Your sign-in code",
            now - 2 * HOUR,
            true,
        );
        let plan = evaluate(&rules, &old_seen, &ctx(&id, now));
        assert_eq!(
            plan.actions,
            vec![PlannedAction {
                rule: "purge".into(),
                action: Action::Delete
            }]
        );
        assert!(!plan.notify, "deleted mail is silent");
    }

    #[test]
    fn rule_ignores_mail_older_than_its_first_seen_unless_apply_existing() {
        let id = identity();
        let rules = rules(PURGE, 5 * HOUR);
        let now = 10 * HOUR;
        let before_rule = msg("noreply@x", "pieter@example.com", "sign in", 4 * HOUR, true);
        assert!(
            evaluate(&rules, &before_rule, &ctx(&id, now))
                .actions
                .is_empty()
        );
        let mut c = ctx(&id, now);
        c.mode = Mode::ApplyExisting;
        assert_eq!(evaluate(&rules, &before_rule, &c).actions.len(), 1);
    }

    #[test]
    fn equals_on_address_fields_compares_bare_addresses() {
        let id = identity();
        let from_rule = rules(
            "[[rules]]\nname = \"gh\"\nmatch.from = { equals = \" NoReply@GitHub.com \" }\nactions = [\"flag\"]\n",
            0,
        );
        let github = msg(
            "GitHub <noreply@github.com>",
            "pieter@example.com",
            "PR",
            100,
            false,
        );
        assert_eq!(
            evaluate(&from_rule, &github, &ctx(&id, 200)).actions.len(),
            1
        );
        let to_rule = rules(
            "[[rules]]\nname = \"second\"\nmatch.to = { equals = \"b@x\" }\nactions = [\"flag\"]\n",
            0,
        );
        let two = msg("a@y", "A <a@x>, B <b@x>", "hi", 100, false);
        assert_eq!(evaluate(&to_rule, &two, &ctx(&id, 200)).actions.len(), 1);
        let subject_rule = rules(
            "[[rules]]\nname = \"s\"\nmatch.subject = { equals = \"pr\" }\nactions = [\"flag\"]\n",
            0,
        );
        assert_eq!(
            evaluate(&subject_rule, &github, &ctx(&id, 200))
                .actions
                .len(),
            1
        );
        let other = msg(
            "noreply@github.com.evil",
            "pieter@example.com",
            "PR",
            100,
            false,
        );
        assert!(
            evaluate(&from_rule, &other, &ctx(&id, 200))
                .actions
                .is_empty()
        );
    }

    #[test]
    fn move_and_mark_read_chain_and_header_match() {
        let id = identity();
        let toml = r#"
[[rules]]
name = "github"
match.header = { name = "List-Id", contains = "github.com" }
actions = [{ move = "Lists/GitHub" }, "mark_read"]
"#;
        let plan = evaluate(
            &rules(toml, 0),
            &msg("bot@github.com", "pieter@example.com", "PR", 100, false),
            &ctx(&id, 200),
        );
        assert_eq!(
            plan.actions
                .iter()
                .map(|a| a.action.clone())
                .collect::<Vec<_>>(),
            vec![Action::Move("Lists/GitHub".into()), Action::MarkRead]
        );
        assert!(!plan.notify, "moved mail is silent by default");
    }

    #[test]
    fn delete_stops_the_chain() {
        let id = identity();
        let toml = r#"
[[rules]]
name = "first"
match.subject = { contains = "spam" }
actions = ["delete"]

[[rules]]
name = "second"
match.subject = { contains = "spam" }
actions = ["flag"]
"#;
        let plan = evaluate(
            &rules(toml, 0),
            &msg("a@x", "pieter@example.com", "SPAM offer", 100, false),
            &ctx(&id, 200),
        );
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.actions[0].rule, "first");
    }

    #[test]
    fn account_and_folder_scoping() {
        let id = identity();
        let toml = r#"
[[rules]]
name = "other account"
account = "home"
match.seen = false
actions = ["flag"]

[[rules]]
name = "other folder"
folder = "Archive"
match.seen = false
actions = ["flag"]
"#;
        let plan = evaluate(
            &rules(toml, 0),
            &msg("a@x", "pieter@example.com", "s", 100, false),
            &ctx(&id, 200),
        );
        assert!(plan.actions.is_empty());
    }

    #[test]
    fn to_me_and_alias() {
        let id = identity();
        let toml = r#"
[[rules]]
name = "lists"
match.to_me = false
actions = [{ move = "Lists" }]

[[rules]]
name = "shop"
match.alias = "*@shop.example.com"
actions = [{ move = "Shopping" }]
"#;
        let r = rules(toml, 0);
        let direct = msg("a@x", "Pieter <pieter@example.com>", "s", 100, false);
        assert!(evaluate(&r, &direct, &ctx(&id, 200)).actions.is_empty());
        let list = msg("a@x", "dev@lists.example", "s", 100, false);
        assert_eq!(evaluate(&r, &list, &ctx(&id, 200)).actions[0].rule, "lists");
        let shop = msg("a@x", "orders@shop.example.com", "s", 100, false);
        let plan = evaluate(&r, &shop, &ctx(&id, 200));
        assert_eq!(
            plan.actions
                .iter()
                .map(|a| a.rule.as_str())
                .collect::<Vec<_>>(),
            vec!["shop"]
        );
    }

    #[test]
    fn body_rule_needs_body_and_matches_when_present() {
        let id = identity();
        let toml = r#"
[[rules]]
name = "unsubscribe"
match.body = { contains = "unsubscribe" }
actions = ["flag"]
"#;
        let r = rules(toml, 0);
        assert!(folder_needs_body(&r, "work", "INBOX"));
        assert!(!folder_needs_body(&r, "work", "Sent"));
        let mut m = msg("a@x", "pieter@example.com", "s", 100, false);
        assert!(evaluate(&r, &m, &ctx(&id, 200)).actions.is_empty());
        m.body_text = Some("Click here to UNSUBSCRIBE".into());
        assert_eq!(evaluate(&r, &m, &ctx(&id, 200)).actions.len(), 1);
    }

    #[test]
    fn notification_policy() {
        let id = identity();
        let toml = r#"
[[rules]]
name = "github"
match.header = { name = "List-Id", contains = "github.com" }
actions = [{ move = "Lists/GitHub" }, "notify"]

[[rules]]
name = "quiet"
match.subject = { contains = "newsletter" }
actions = ["silent"]
"#;
        let r = rules(toml, 0);
        let untouched = msg("a@x", "pieter@example.com", "hello", 100, false);
        assert!(evaluate(&r, &untouched, &ctx(&id, 200)).notify);
        let mut off = ctx(&id, 200);
        off.notify_default = false;
        assert!(!evaluate(&r, &untouched, &off).notify);
        let moved_but_notify = msg("bot@github.com", "pieter@example.com", "PR", 100, false);
        assert!(evaluate(&r, &moved_but_notify, &ctx(&id, 200)).notify);
        let quiet = msg("a@x", "pieter@example.com", "Weekly newsletter", 100, false);
        assert!(!evaluate(&r, &quiet, &ctx(&id, 200)).notify);
    }

    #[test]
    fn deleted_mail_never_notifies() {
        let id = identity();
        let toml = r#"
[[rules]]
name = "loud"
match.subject = { contains = "x" }
actions = ["notify"]

[[rules]]
name = "purge"
match.subject = { contains = "x" }
actions = ["delete"]
"#;
        let plan = evaluate(
            &rules(toml, 0),
            &msg("a@x", "pieter@example.com", "x", 100, false),
            &ctx(&id, 200),
        );
        assert!(!plan.notify);
        assert_eq!(plan.actions.len(), 1);
        assert_eq!(plan.actions[0].action, Action::Delete);
    }

    #[test]
    fn huge_older_than_never_matches() {
        let id = identity();
        let toml = "[[rules]]\nname = \"never\"\nmatch.older_than = \"300000000000y\"\nactions = [\"delete\"]\n";
        let m = msg("a@x", "pieter@example.com", "s", 0, true);
        assert!(
            evaluate(&rules(toml, 0), &m, &ctx(&id, 10 * HOUR))
                .actions
                .is_empty()
        );
    }

    #[test]
    fn to_me_via_delivered_to_and_cc() {
        let id = identity();
        let toml = r#"
[[rules]]
name = "lists"
match.to_me = false
actions = [{ move = "Lists" }]

[[rules]]
name = "shop"
match.alias = "orders@shop.example.com"
actions = [{ move = "Shopping" }]
"#;
        let r = rules(toml, 0);
        let mut delivered = msg("a@x", "dev@lists.example", "s", 100, false);
        delivered.delivered_to = Some("pieter@example.com".into());
        assert!(evaluate(&r, &delivered, &ctx(&id, 200)).actions.is_empty());
        let mut cc = msg("a@x", "pieter@example.com", "s", 100, false);
        cc.cc_addr = Some("Shop <orders@shop.example.com>".into());
        let plan = evaluate(&r, &cc, &ctx(&id, 200));
        assert_eq!(
            plan.actions
                .iter()
                .map(|a| a.rule.as_str())
                .collect::<Vec<_>>(),
            vec!["shop"]
        );
    }

    #[test]
    fn older_than_boundary_is_inclusive() {
        let id = identity();
        let toml = "[[rules]]\nname = \"age\"\nmatch.older_than = \"1h\"\nactions = [\"flag\"]\n";
        let r = rules(toml, 0);
        let exact = msg("a@x", "pieter@example.com", "s", 1000, false);
        assert_eq!(
            evaluate(&r, &exact, &ctx(&id, 1000 + HOUR)).actions.len(),
            1
        );
        assert!(
            evaluate(&r, &exact, &ctx(&id, 1000 + HOUR - 1))
                .actions
                .is_empty()
        );
    }

    fn fires(toml: &str, m: &Message) -> bool {
        let id = identity();
        !evaluate(&rules(toml, 0), m, &ctx(&id, 200))
            .actions
            .is_empty()
    }

    #[test]
    fn none_excludes_mail_any_entry_matches() {
        let toml = r#"
[[rules]]
name = "shop but no receipts"
match.from = { contains = "shop" }
match.none = [{ subject = { contains = "receipt" } }, { from = { contains = "billing" } }]
actions = ["flag"]
"#;
        assert!(fires(toml, &msg("shop@x", "p@x", "Sale", 100, false)));
        assert!(!fires(
            toml,
            &msg("shop@x", "p@x", "Your receipt", 100, false)
        ));
        assert!(!fires(
            toml,
            &msg("billing@shop.x", "p@x", "Sale", 100, false)
        ));
        assert!(!fires(toml, &msg("bank@x", "p@x", "Sale", 100, false)));
    }

    #[test]
    fn a_none_entry_excludes_only_when_all_its_conditions_hold() {
        let toml = r#"
[[rules]]
name = "x"
match.seen = false
match.none = [{ from = { contains = "alice" }, subject = { contains = "lunch" } }]
actions = ["flag"]
"#;
        assert!(!fires(toml, &msg("alice@x", "p@x", "lunch?", 100, false)));
        assert!(fires(toml, &msg("alice@x", "p@x", "report", 100, false)));
        assert!(fires(toml, &msg("bob@x", "p@x", "lunch?", 100, false)));
    }

    #[test]
    fn a_list_of_values_matches_any_of_them() {
        let toml = r#"
[[rules]]
name = "x"
match.subject = { contains = ["receipt", "INVOICE"] }
match.from = { equals = ["a@x", "b@x"] }
actions = ["flag"]
"#;
        assert!(fires(
            toml,
            &msg("A <a@x>", "p@x", "Your invoice", 100, false)
        ));
        assert!(fires(toml, &msg("b@x", "p@x", "receipt 12", 100, false)));
        assert!(!fires(toml, &msg("c@x", "p@x", "receipt 12", 100, false)));
        assert!(!fires(toml, &msg("a@x", "p@x", "hello", 100, false)));
    }

    #[test]
    fn any_needs_one_branch_and_the_rest_still_holds() {
        let toml = r#"
[[rules]]
name = "either"
match.seen = false
match.any = [{ from = { contains = "alice" } }, { subject = { contains = "urgent" } }]
actions = ["flag"]
"#;
        assert!(fires(toml, &msg("alice@x", "p@x", "hi", 100, false)));
        assert!(fires(toml, &msg("bob@x", "p@x", "URGENT", 100, false)));
        assert!(!fires(toml, &msg("bob@x", "p@x", "hi", 100, false)));
        assert!(!fires(toml, &msg("alice@x", "p@x", "hi", 100, true)));
    }

    #[test]
    fn every_listed_header_must_match() {
        let toml = r#"
[[rules]]
name = "both"
match.header = [{ name = "List-Id", contains = "github.com" }, { name = "Subject", contains = "PR" }]
actions = ["flag"]
"#;
        assert!(fires(
            toml,
            &msg("bot@github.com", "p@x", "PR 1", 100, false)
        ));
        assert!(!fires(
            toml,
            &msg("bot@github.com", "p@x", "Issue", 100, false)
        ));
        assert!(!fires(toml, &msg("a@x", "p@x", "PR 1", 100, false)));
    }

    #[test]
    fn tag_matches_keywords_ignoring_case() {
        let toml = "[[rules]]\nname = \"t\"\nmatch.tag = \"$label1\"\nactions = [\"flag\"]\n";
        let mut m = msg("a@x", "p@x", "s", 100, false);
        assert!(!fires(toml, &m));
        m.flags = "\\Seen $Label1".into();
        assert!(fires(toml, &m));
        m.flags = "$label10".into();
        assert!(!fires(toml, &m));
    }

    #[test]
    fn rule_needing_a_missing_body_never_fires_even_negated() {
        let toml = "[[rules]]\nname = \"b\"\nmatch.none = [{ body = { contains = \"keep\" } }]\nactions = [\"delete\"]\n";
        let r = rules(toml, 0);
        assert!(folder_needs_body(&r, "work", "INBOX"));
        let mut m = msg("a@x", "p@x", "s", 100, false);
        assert!(!fires(toml, &m));
        m.body_text = Some("please keep this".into());
        assert!(!fires(toml, &m));
        m.body_text = Some("junk".into());
        assert!(fires(toml, &m));
    }

    #[test]
    fn disabled_rules_are_skipped() {
        let id = identity();
        let toml = "[[rules]]\nname = \"off\"\nenabled = false\nmatch.seen = false\nactions = [\"flag\"]\n";
        assert!(
            evaluate(
                &rules(toml, 0),
                &msg("a@x", "p@x", "s", 100, false),
                &ctx(&id, 200)
            )
            .actions
            .is_empty()
        );
    }
}
