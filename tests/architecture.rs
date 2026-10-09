//! The module ownership rules from AGENTS.md's module map, checked on the source text.
//!
//! Comments, string contents and `#[cfg(test)]` items are ignored: the rules are about shipped code. Each check is
//! a plain text scan, so it only enforces what can be checked without false positives; allowed exceptions are listed
//! below with their reason.

// Test code: unwrap, expect and panic are how a test fails.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

/// Paths into `rules` that `src/mcp/` outside `backend.rs` may name: the data types its tool arguments deserialize
/// into and describe in their schemas. Loading, evaluating or editing rules stays in `backend.rs`.
const MCP_RULES_TYPES: [&str; 2] = ["Rule", "Action"];

/// Functions outside `src/rules/edit.rs` that may write next to the rules path, as `(file, fn, reason)`.
const RULES_WRITE_EXCEPTIONS: [(&str, &str, &str); 1] = [(
    "src/gui/app.rs",
    "open_rules_file",
    "creates an empty rules.toml when there is none, so the editor has a file to open; never changes an existing one",
)];

/// A source file with comments and `#[cfg(test)]` items blanked; `code` also blanks string contents.
/// Both keep every character's line, so positions map back to the file.
struct Source {
    path: String,
    text: Vec<char>,
    code: Vec<char>,
}

impl Source {
    fn parse(path: &str, raw: &str) -> Source {
        let (mut text, mut code) = strip(raw);
        blank_cfg_test(&mut text, &mut code);
        Source {
            path: path.to_string(),
            text,
            code,
        }
    }

    fn line(&self, pos: usize) -> usize {
        1 + self.code[..pos].iter().filter(|&&c| c == '\n').count()
    }

    fn violation(&self, pos: usize, rule: &str, detail: &str) -> String {
        format!(
            "{}:{}: {detail}\n  rule: {rule} (AGENTS.md, module map)",
            self.path,
            self.line(pos)
        )
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Blanks comments (both copies) and string and char literal contents (the `code` copy only), keeping newlines.
fn strip(raw: &str) -> (Vec<char>, Vec<char>) {
    let src: Vec<char> = raw.chars().collect();
    let mut text = src.clone();
    let mut code = src.clone();
    let blank = |v: &mut Vec<char>, from: usize, to: usize| {
        for c in &mut v[from..to] {
            if *c != '\n' {
                *c = ' ';
            }
        }
    };
    let n = src.len();
    let mut i = 0;
    while i < n {
        let c = src[i];
        let next = src.get(i + 1).copied();
        let prev_ident = i > 0 && is_ident(src[i - 1]);
        if c == '/' && next == Some('/') {
            let end = src[i..]
                .iter()
                .position(|&c| c == '\n')
                .map_or(n, |p| i + p);
            blank(&mut text, i, end);
            blank(&mut code, i, end);
            i = end;
        } else if c == '/' && next == Some('*') {
            let (mut depth, mut j) = (1, i + 2);
            while j < n && depth > 0 {
                if src[j] == '/' && src.get(j + 1) == Some(&'*') {
                    depth += 1;
                    j += 2;
                } else if src[j] == '*' && src.get(j + 1) == Some(&'/') {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            blank(&mut text, i, j);
            blank(&mut code, i, j);
            i = j;
        } else if c == 'r'
            && (!prev_ident || (src[i - 1] == 'b' && (i < 2 || !is_ident(src[i - 2]))))
            && matches!(next, Some('"' | '#'))
        {
            let hashes = src[i + 1..].iter().take_while(|&&c| c == '#').count();
            let open = i + 1 + hashes;
            if src.get(open) != Some(&'"') {
                i += 1;
                continue;
            }
            let mut j = open + 1;
            while j < n
                && !(src[j] == '"'
                    && src[j + 1..]
                        .iter()
                        .take(hashes)
                        .filter(|&&c| c == '#')
                        .count()
                        == hashes)
            {
                j += 1;
            }
            blank(&mut code, open + 1, j.min(n));
            i = j + 1 + hashes;
        } else if c == '"' {
            let mut j = i + 1;
            while j < n && src[j] != '"' {
                j += if src[j] == '\\' { 2 } else { 1 };
            }
            blank(&mut code, i + 1, j.min(n));
            i = j + 1;
        } else if c == '\'' && next == Some('\\') {
            let end = src[i + 2..]
                .iter()
                .position(|&c| c == '\'')
                .map_or(n, |p| i + 2 + p);
            blank(&mut code, i + 1, end);
            i = end + 1;
        } else if c == '\'' && src.get(i + 2) == Some(&'\'') {
            blank(&mut code, i + 1, i + 2);
            i += 3;
        } else {
            i += 1;
        }
    }
    (text, code)
}

/// The position just past the `}` matching the `{` at `open`.
fn matching_brace(code: &[char], open: usize) -> usize {
    let mut depth = 0;
    for (j, &c) in code.iter().enumerate().skip(open) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return j + 1;
                }
            }
            _ => {}
        }
    }
    code.len()
}

fn find_all(hay: &[char], needle: &str) -> Vec<usize> {
    let needle: Vec<char> = needle.chars().collect();
    (0..hay.len().saturating_sub(needle.len() - 1))
        .filter(|&i| hay[i..i + needle.len()] == needle[..])
        .collect()
}

/// Like `find_all`, but only where `needle` is not glued to a longer identifier on either side.
fn find_tokens(hay: &[char], needle: &str) -> Vec<usize> {
    let len = needle.chars().count();
    let first_ident = needle.chars().next().is_some_and(is_ident);
    let last_ident = needle.chars().last().is_some_and(is_ident);
    find_all(hay, needle)
        .into_iter()
        .filter(|&i| !(first_ident && i > 0 && is_ident(hay[i - 1])))
        .filter(|&i| !(last_ident && hay.get(i + len).is_some_and(|&c| is_ident(c))))
        .collect()
}

/// Blanks every item marked `#[cfg(test)]`: a `mod tests { … }`, a `fn`, a `use …;`, or a `mod x;` declaration.
fn blank_cfg_test(text: &mut [char], code: &mut [char]) {
    for at in find_all(code, "#[cfg(test)]") {
        let start = at + "#[cfg(test)]".len();
        let end = code[start..]
            .iter()
            .position(|&c| c == ';' || c == '{')
            .map_or(code.len(), |p| start + p);
        let end = if code.get(end) == Some(&'{') {
            matching_brace(code, end)
        } else {
            end + 1
        };
        let end = end.min(code.len());
        for v in [&mut *text, &mut *code] {
            for c in &mut v[at..end] {
                if *c != '\n' {
                    *c = ' ';
                }
            }
        }
    }
}

/// The files that `#[cfg(test)] mod x;` declarations in `source` (at `path`) pull in.
fn test_only_modules(path: &str, raw: &str) -> Vec<String> {
    let (_, code) = strip(raw);
    let code: String = code.into_iter().collect();
    let dir = match path.strip_suffix(".rs") {
        Some(p) if p.ends_with("/mod") || p.ends_with("/lib") || p.ends_with("/main") => {
            p.rsplit_once('/').unwrap().0.to_string()
        }
        Some(p) => p.to_string(),
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut rest = code.as_str();
    while let Some(at) = rest.find("#[cfg(test)]") {
        rest = &rest[at + "#[cfg(test)]".len()..];
        let item = rest.split([';', '{']).next().unwrap_or("");
        let words: Vec<&str> = item.split_whitespace().collect();
        if let [.., "mod", name] = words.as_slice()
            && rest[item.len()..].starts_with(';')
        {
            out.push(format!("{dir}/{name}.rs"));
            out.push(format!("{dir}/{name}/mod.rs"));
        }
    }
    out
}

/// Every crate path named in `code` after one of `roots`, relative to the crate root, with `use` groups expanded:
/// `crate::rules::{self, Rule}` gives `rules` and `rules::Rule`.
fn crate_paths(code: &[char], roots: &[&str]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for root in roots {
        for at in find_tokens(code, root) {
            let mut paths = Vec::new();
            use_tree(code, at + root.chars().count(), String::new(), &mut paths);
            out.extend(paths.into_iter().map(|p| (at, p)));
        }
    }
    out
}

fn skip_ws(code: &[char], mut i: usize) -> usize {
    while code.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    i
}

/// Parses one use tree at `i` under `prefix`, pushing each full path; returns where it ended.
fn use_tree(code: &[char], i: usize, prefix: String, out: &mut Vec<String>) -> usize {
    let mut i = skip_ws(code, i);
    if code.get(i) == Some(&'{') {
        i += 1;
        loop {
            i = skip_ws(code, i);
            match code.get(i) {
                Some('}') | None => return i + 1,
                Some(',') => i += 1,
                _ => i = use_tree(code, i, prefix.clone(), out),
            }
        }
    }
    if code.get(i) == Some(&'*') {
        out.push(format!("{prefix}*"));
        return i + 1;
    }
    let start = i;
    while code.get(i).is_some_and(|&c| is_ident(c)) {
        i += 1;
    }
    let segment: String = code[start..i].iter().collect();
    if segment.is_empty() {
        if !prefix.is_empty() {
            out.push(prefix.trim_end_matches("::").to_string());
        }
        return i;
    }
    let path = if segment == "self" {
        prefix.trim_end_matches("::").to_string()
    } else {
        format!("{prefix}{segment}")
    };
    let after = skip_ws(code, i);
    if code.get(after) == Some(&':') && code.get(after + 1) == Some(&':') {
        return use_tree(code, after + 2, format!("{path}::"), out);
    }
    out.push(path);
    let alias = skip_ws(code, i);
    if code[alias..].starts_with(&['a', 's'])
        && code.get(alias + 2).is_some_and(|c| c.is_whitespace())
    {
        i = skip_ws(code, alias + 2);
        while code.get(i).is_some_and(|&c| is_ident(c)) {
            i += 1;
        }
    }
    i
}

/// Rule 1: in `src/mcp/`, only `backend.rs` touches the store, rules or the daemon.
fn check_mcp(source: &Source) -> Vec<String> {
    const RULE: &str = "in src/mcp/, only backend.rs touches the store, rules or the daemon";
    let in_mcp = source.path.starts_with("src/mcp/");
    if !in_mcp || source.path == "src/mcp/backend.rs" {
        return Vec::new();
    }
    let roots: &[&str] = if source.path == "src/mcp/mod.rs" {
        &["crate::", "postvak::", "super::"]
    } else {
        &["crate::", "postvak::", "super::super::"]
    };
    let mut out = Vec::new();
    for (at, path) in crate_paths(&source.code, roots) {
        let segments: Vec<&str> = path.split("::").collect();
        let allowed = match segments.as_slice() {
            ["store" | "daemon", ..] => false,
            ["rules", item, ..] => MCP_RULES_TYPES.contains(item),
            ["rules"] => false,
            _ => true,
        };
        if !allowed {
            out.push(source.violation(at, RULE, &format!("names `{path}`; go through `Backend`")));
        }
    }
    for at in find_tokens(&source.code, "rusqlite") {
        out.push(source.violation(at, RULE, "uses rusqlite; go through `Backend`"));
    }
    out
}

/// Every `fn` in `source` with its name, where the keyword starts, and its body's range.
fn functions(code: &[char]) -> Vec<(String, usize, usize, usize)> {
    let mut out = Vec::new();
    for at in find_tokens(code, "fn") {
        let name_start = skip_ws(code, at + 2);
        let name_end = (name_start..code.len())
            .find(|&j| !is_ident(code[j]))
            .unwrap_or(code.len());
        if name_end == name_start {
            continue;
        }
        let Some(open) = code[name_end..].iter().position(|&c| c == '{' || c == ';') else {
            continue;
        };
        let open = name_end + open;
        if code[open] == '{' {
            let name = code[name_start..name_end].iter().collect();
            out.push((name, at, open, matching_brace(code, open)));
        }
    }
    out
}

/// Rule 2: only `src/rules/edit.rs` writes rules.toml. Flags any function outside it that both names the rules path
/// and calls a file write.
fn check_rules_writes(source: &Source) -> Vec<String> {
    const RULE: &str = "only src/rules/edit.rs writes rules.toml";
    const PATH_MARKERS: [&str; 3] = ["rules_file", "rules_path", "\"rules.toml\""];
    const WRITES: [&str; 7] = [
        "write_atomic",
        "write_atomic_keeping_dir_mode",
        "fs::write",
        "File::create",
        "OpenOptions",
        "fs::rename",
        "fs::remove_file",
    ];
    if source.path == "src/rules/edit.rs" {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (name, at, open, close) in functions(&source.code) {
        let excepted = RULES_WRITE_EXCEPTIONS
            .iter()
            .any(|(file, f, _)| *file == source.path && *f == name);
        let names_path = PATH_MARKERS
            .iter()
            .any(|m| !find_tokens(&source.text[open..close], m).is_empty());
        let write = WRITES.iter().find_map(|w| {
            find_tokens(&source.code[open..close], w)
                .first()
                .map(|p| (w, open + p))
        });
        if let (false, true, Some((w, pos))) = (excepted, names_path, write) {
            out.push(source.violation(
                pos,
                RULE,
                &format!(
                    "`fn {name}` (line {}) names the rules path and calls `{w}`; use `rules::edit`",
                    source.line(at)
                ),
            ));
        }
    }
    out
}

/// Rule 3: in `src/gui/`, `app.rs` is the only code that changes state; the view modules draw from `&App` and
/// return `UiAction`s. So a view never takes `&mut App`, reaches the daemon, or edits rules.
fn check_gui_views(source: &Source) -> Vec<String> {
    const RULE: &str =
        "in src/gui/, app.rs is the only code that changes state; views draw and return UiActions";
    let Some(file) = source.path.strip_prefix("src/gui/") else {
        return Vec::new();
    };
    if matches!(file, "app.rs" | "mod.rs" | "test_support.rs") {
        return Vec::new();
    }
    let mut out = Vec::new();
    for at in find_tokens(&source.code, "&mut") {
        let next = skip_ws(&source.code, at + 4);
        if find_tokens(&source.code[next..(next + 3).min(source.code.len())], "App").first()
            == Some(&0)
        {
            out.push(source.violation(at, RULE, "takes `&mut App`; return a `UiAction` instead"));
        }
    }
    for at in find_tokens(&source.code, ".client") {
        out.push(source.violation(
            at,
            RULE,
            "uses the daemon client; return a `UiAction` instead",
        ));
    }
    for (at, path) in crate_paths(&source.code, &["crate::", "postvak::", "super::super::"]) {
        if path.starts_with("daemon") || path.starts_with("rules::edit") {
            out.push(source.violation(
                at,
                RULE,
                &format!("names `{path}`; return a `UiAction` instead"),
            ));
        }
    }
    for at in find_tokens(&source.code, "rules::edit") {
        out.push(source.violation(at, RULE, "edits rules; return a `UiAction` instead"));
    }
    // `crate::rules::edit` matches both scans; one report per line is enough.
    out.sort();
    out.dedup_by(|a, b| a.split(": ").next() == b.split(": ").next());
    out
}

/// Rule 4, the checkable part of "main + cli/: clap only": they never open SQLite themselves.
fn check_cli(source: &Source) -> Vec<String> {
    const RULE: &str = "main.rs and src/cli/ hold no logic; the store owns SQLite";
    if source.path != "src/main.rs" && !source.path.starts_with("src/cli/") {
        return Vec::new();
    }
    find_tokens(&source.code, "rusqlite")
        .into_iter()
        .map(|at| source.violation(at, RULE, "uses rusqlite; go through `store`"))
        .collect()
}

fn check(source: &Source) -> Vec<String> {
    let mut out = check_mcp(source);
    out.extend(check_rules_writes(source));
    out.extend(check_gui_views(source));
    out.extend(check_cli(source));
    out
}

fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn module_ownership_rules_from_agents_md_hold() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    files.sort();
    let sources: Vec<(String, String)> = files
        .iter()
        .map(|f| {
            let rel = f
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            (rel, std::fs::read_to_string(f).unwrap())
        })
        .collect();
    let test_only: Vec<String> = sources
        .iter()
        .flat_map(|(p, raw)| test_only_modules(p, raw))
        .collect();
    assert!(test_only.contains(&"src/mcp/tests.rs".to_string()));
    let mut violations = Vec::new();
    for (path, raw) in &sources {
        if !test_only.contains(path) {
            violations.extend(check(&Source::parse(path, raw)));
        }
    }
    assert!(
        violations.is_empty(),
        "module ownership rules broken:\n{}",
        violations.join("\n")
    );
}

#[test]
fn every_rules_write_exception_still_exists() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for (file, name, _reason) in RULES_WRITE_EXCEPTIONS {
        let source = Source::parse(file, &std::fs::read_to_string(root.join(file)).unwrap());
        assert!(
            functions(&source.code).iter().any(|(n, ..)| n == name),
            "{file} has no `fn {name}`; drop it from RULES_WRITE_EXCEPTIONS"
        );
    }
}

#[cfg(test)]
mod checks_fire {
    use super::*;

    fn violations(path: &str, src: &str) -> Vec<String> {
        check(&Source::parse(path, src))
    }

    #[test]
    fn mcp_outside_backend_may_name_rule_types_only() {
        let ok = "use crate::rules::{Action, Rule};\nfn f(r: crate::rules::Rule) { let _ = Action::Archive; }\n\
                  // crate::store::Store in a comment\nconst S: &str = \"crate::daemon::Client\";\n\
                  #[cfg(test)]\nmod tests { use crate::store::Store; }\n";
        assert_eq!(violations("src/mcp/tools.rs", ok), Vec::<String>::new());
        for bad in [
            "use crate::store::Store;",
            "use crate::{config, daemon::Client};",
            "use crate::rules::{self, Rule};",
            "fn f() { crate::rules::load(p); }",
            "use super::super::store;",
            "use crate::rules::edit;",
            "use crate::{rules::{self as r}, config};",
            "fn f(c: rusqlite::Connection) {}",
        ] {
            let found = violations("src/mcp/tools.rs", &format!("\n{bad}\n"));
            assert_eq!(found.len(), 1, "{bad}: {found:?}");
            assert!(found[0].starts_with("src/mcp/tools.rs:2: "), "{found:?}");
            assert!(found[0].contains("AGENTS.md"));
        }
        assert_eq!(
            violations("src/mcp/mod.rs", "use super::store::Store;").len(),
            1
        );
        assert!(violations("src/mcp/backend.rs", "use crate::store::Store;").is_empty());
    }

    #[test]
    fn rules_toml_writes_outside_edit_rs_are_flagged() {
        let bad = "fn save(paths: &Paths) {\n    write_atomic(&paths.rules_file(), b\"x\").unwrap();\n}\n";
        let found = violations("src/cli/mod.rs", bad);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].starts_with("src/cli/mod.rs:2: `fn save`"),
            "{found:?}"
        );
        let joined =
            "fn f(dir: &Path) { std::fs::write(dir.join(\"rules.toml\"), \"\").unwrap(); }";
        assert_eq!(violations("src/sync.rs", joined).len(), 1);
        let unrelated = "fn f(p: &Paths) { write_atomic(&p.config_file(), b\"\"); }\n\
                         fn g(p: &Paths) { let _ = rules::load(&p.rules_file()); }";
        assert!(violations("src/sync.rs", unrelated).is_empty());
        assert!(violations("src/rules/edit.rs", bad).is_empty());
        let excepted =
            "fn open_rules_file(&mut self) { write_atomic(&self.paths.rules_file(), b\"\"); }";
        assert!(violations("src/gui/app.rs", excepted).is_empty());
    }

    #[test]
    fn gui_views_never_change_state() {
        let ok = "use super::app::{App, UiAction};\npub(crate) fn show(app: &App, ui: &mut egui::Ui) -> Vec<UiAction> { vec![] }\n\
                  #[cfg(test)]\nmod tests { fn at(h: &mut App) {} }";
        assert_eq!(violations("src/gui/list.rs", ok), Vec::<String>::new());
        for bad in [
            "fn show(app: &mut App) {}",
            "fn show(app: &App) { app.client.send(c); }",
            "use crate::daemon::Client;",
            "fn f(p: &Path) { crate::rules::edit::approve(p, \"x\"); }",
            "fn f(p: &Path) { rules::edit::approve(p, \"x\"); }",
        ] {
            let found = violations("src/gui/rules.rs", bad);
            assert_eq!(found.len(), 1, "{bad}: {found:?}");
        }
        assert!(
            violations(
                "src/gui/app.rs",
                "fn f(app: &mut App) { self.client.send(c); }"
            )
            .is_empty()
        );
    }

    #[test]
    fn cli_never_opens_sqlite() {
        assert_eq!(
            violations("src/cli/mod.rs", "use rusqlite::Connection;").len(),
            1
        );
        assert_eq!(
            violations("src/main.rs", "fn f() { rusqlite::Connection::open(p); }").len(),
            1
        );
        assert!(violations("src/store.rs", "use rusqlite::Connection;").is_empty());
    }

    #[test]
    fn strip_keeps_lines_and_blanks_literals() {
        let src = "let a = r#\"crate::store \"# ; // crate::daemon\nlet c = '\"'; let l: &'a str = \"x\";\n/* a\n b */ crate::rules";
        let s = Source::parse("x.rs", src);
        let code: String = s.code.iter().collect();
        assert!(!code.contains("store") && !code.contains("daemon"));
        assert!(code.contains("&'a str"));
        assert_eq!(s.line(code.find("crate::rules").unwrap()), 4);
    }

    #[test]
    fn cfg_test_module_files_are_found() {
        let found = test_only_modules(
            "src/mcp/mod.rs",
            "mod backend;\n#[cfg(test)]\nmod tests;\nmod tools;",
        );
        assert_eq!(found, ["src/mcp/tests.rs", "src/mcp/tests/mod.rs"]);
        let found = test_only_modules(
            "src/daemon/mod.rs",
            "#[cfg(test)]\npub(crate) mod test_support;",
        );
        assert_eq!(found[0], "src/daemon/test_support.rs");
    }
}
