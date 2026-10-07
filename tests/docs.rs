// Test code: unwrap, expect and panic are how a test fails.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use postbode::config::Config;
use postbode::rules::{Rule, RuleFile, compile, parse};

fn read(path: &str) -> String {
    std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// The bodies of the ```lang fenced blocks in a markdown page.
fn fenced(text: &str, lang: &str) -> Vec<String> {
    let opening = format!("```{lang}");
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        match current.as_mut() {
            None if line.trim_end() == opening => current = Some(String::new()),
            Some(_) if line.trim_end() == "```" => blocks.push(current.take().unwrap()),
            Some(block) => {
                block.push_str(line);
                block.push('\n');
            }
            None => {}
        }
    }
    blocks
}

#[test]
fn rule_examples_parse_and_validate() {
    for page in ["docs/src/index.md", "docs/src/rules.md"] {
        let blocks = fenced(&read(page), "toml");
        assert!(!blocks.is_empty(), "{page} has no toml examples");
        for block in blocks {
            let file = parse(&block).unwrap_or_else(|e| panic!("{page}: {e}\n{block}"));
            compile(&file).unwrap_or_else(|e| panic!("{page}: {e}\n{block}"));
        }
    }
}

#[test]
fn proposal_examples_parse_and_validate() {
    let blocks = fenced(&read("docs/src/agent-guide.md"), "json");
    assert!(!blocks.is_empty());
    for block in blocks {
        let rule: Rule = serde_json::from_str(&block).unwrap_or_else(|e| panic!("{e}\n{block}"));
        compile(&RuleFile { rules: vec![rule] }).unwrap_or_else(|e| panic!("{e}\n{block}"));
    }
}

#[test]
fn account_examples_parse() {
    let blocks = fenced(&read("docs/src/accounts.md"), "toml");
    assert!(!blocks.is_empty());
    for block in blocks {
        Config::parse(&block).unwrap_or_else(|e| panic!("{e}\n{block}"));
    }
}

#[test]
fn readme_contains_the_book_index() {
    assert!(
        read("README.md").contains(&read("docs/src/index.md")),
        "README.md must contain docs/src/index.md verbatim"
    );
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_page_snippet_matches_install_json() {
    use postbode::mcp::install::{Entry, json_snippet};
    let entry = Entry::new(
        std::path::Path::new("/opt/homebrew/bin/postbode"),
        "read,rules:propose",
        &[],
    );
    let page = read("docs/src/mcp.md");
    let snippet = fenced(&page, "json")
        .into_iter()
        .find(|block| block.contains("mcpServers"))
        .expect("a snippet on the MCP page");
    assert_eq!(snippet, json_snippet(&entry));
}

#[cfg(feature = "mcp")]
#[test]
fn mcp_page_scope_table_lists_every_scope() {
    let page = read("docs/src/mcp.md");
    for scope in postbode::mcp::Scope::ALL {
        assert!(
            page.contains(&format!("| `{}` |", scope.as_str())),
            "scope {} missing from the table",
            scope.as_str()
        );
    }
}

#[test]
fn the_daemon_page_names_every_daemon_command() {
    let page = read("docs/src/daemon.md");
    for command in [
        "postbode run",
        "postbode daemon status",
        "postbode daemon stop",
        "postbode service install",
        "postbode service remove",
        "daemon.log",
    ] {
        assert!(
            page.contains(command),
            "docs/src/daemon.md does not mention {command}"
        );
    }
}
