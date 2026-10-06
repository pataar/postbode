use crate::config::Identity;
use crate::message::{bare_addresses, header_value};
use crate::rules::{Action, CompiledRule};
use crate::store::Message;

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
        if !matches(rule, msg, ctx) {
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

fn matches(rule: &CompiledRule, msg: &Message, ctx: &Context) -> bool {
    if let Some(m) = &rule.from
        && !m.is_match(field(&msg.from_addr))
    {
        return false;
    }
    if let Some(m) = &rule.to
        && !m.is_match(field(&msg.to_addr))
    {
        return false;
    }
    if let Some(m) = &rule.cc
        && !m.is_match(field(&msg.cc_addr))
    {
        return false;
    }
    if let Some(m) = &rule.subject
        && !m.is_match(field(&msg.subject))
    {
        return false;
    }
    if let Some(m) = &rule.body {
        match &msg.body_text {
            Some(body) if m.is_match(body) => {}
            _ => return false,
        }
    }
    if let Some((name, m)) = &rule.header {
        match header_value(&msg.headers, name) {
            Some(value) if m.is_match(&value) => {}
            _ => return false,
        }
    }
    if let Some(min_age) = rule.older_than
        && ctx.now - msg.internaldate < i64::try_from(min_age.as_secs()).unwrap_or(i64::MAX)
    {
        return false;
    }
    if let Some(seen) = rule.rule.matches.seen
        && msg.is_seen() != seen
    {
        return false;
    }
    if rule.rule.matches.to_me.is_some() || rule.alias.is_some() {
        let recipients: Vec<String> = [&msg.to_addr, &msg.cc_addr, &msg.delivered_to]
            .into_iter()
            .flat_map(|f| bare_addresses(field(f)))
            .collect();
        if let Some(to_me) = rule.rule.matches.to_me
            && recipients.iter().any(|r| ctx.identity.is_me(r)) != to_me
        {
            return false;
        }
        if let Some(glob) = &rule.alias
            && !recipients.iter().any(|r| glob.is_match(r))
        {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AccountConfig, PasswordSource};
    use crate::rules::{compile, parse};

    const HOUR: i64 = 3600;

    fn identity() -> Identity {
        AccountConfig {
            name: "work".into(),
            host: "h".into(),
            port: 993,
            username: "pieter@example.com".into(),
            password: PasswordSource::Keyring { keyring: true },
            address: None,
            aliases: vec!["*@shop.example.com".into()],
            sync_interval_secs: 120,
            trash_retention_days: 30,
            notify: true,
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
            cc_addr: None,
            delivered_to: None,
            in_reply_to: None,
            refs: None,
            thread_id: "m1@x".into(),
            subject: Some(subject.into()),
            date: Some(internaldate),
            internaldate,
            flags: if seen { "\\Seen".into() } else { String::new() },
            size: None,
            headers: headers.into_bytes(),
            body_text: None,
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
