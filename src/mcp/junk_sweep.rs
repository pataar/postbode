//! Calls every listed tool with junk arguments: each call must come back as a result or an error, never a panic or a
//! hang, an error must leave the store, rules.toml and the trash as they were, and the server must keep serving.
use std::collections::BTreeSet;
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, JsonObject, Tool};
use rmcp::service::{RunningService, ServiceError};
use rmcp::{RoleClient, model::ClientConfig};
use serde_json::{Map, Value, json};

use super::Scope;
use super::tests::{Fixture, connect, fixture, fixture_message, rows};

const PER_CALL: Duration = Duration::from_secs(10);

/// Values of every JSON type, at and past the edges a tool might assume.
fn junk_values() -> Vec<Value> {
    let mut deep_array = json!([]);
    let mut deep_object = json!({});
    for _ in 0..100 {
        deep_array = json!([deep_array]);
        deep_object = json!({ "a": deep_object });
    }
    vec![
        Value::Null,
        json!(true),
        json!(false),
        json!(0),
        json!(-1),
        json!(i64::MIN),
        json!(u64::MAX),
        json!(u64::from(u32::MAX) + 1),
        json!(1.5),
        json!(-0.5),
        json!(1e308),
        json!(""),
        json!("   "),
        json!("x".repeat(100_000)),
        json!("\u{0}\u{7}\u{1b}]0;title\u{7}\u{202e}\u{feff}é😀\r\n\t"),
        json!("../../config/config.toml"),
        json!("INBOX"),
        json!([]),
        json!([null]),
        json!([-1, 1.5, "x", {}, []]),
        Value::Array((0..10_000).map(Value::from).collect()),
        deep_array,
        json!({}),
        json!({ "a": null }),
        deep_object,
    ]
}

/// The tool's own schema with `$ref`s resolved against its root.
fn resolve<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    match schema["$ref"].as_str() {
        Some(r) => r
            .strip_prefix("#/")
            .map(|path| path.split('/').fold(root, |v, key| &v[key]))
            .map_or(schema, |target| resolve(root, target)),
        None => schema,
    }
}

/// A plausible value for `schema`: a string, uid 1, the first enum variant, an object with its required keys.
fn sample(root: &Value, schema: &Value, depth: usize) -> Value {
    let schema = resolve(root, schema);
    if depth > 8 {
        return Value::Null;
    }
    if let Some(first) = schema["enum"].as_array().and_then(|e| e.first()) {
        return first.clone();
    }
    if let Some(value) = schema.get("const") {
        return value.clone();
    }
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(options) = schema[key].as_array() {
            let option = options
                .iter()
                .find(|o| {
                    let o = resolve(root, o);
                    o["type"] != "null" && o["enum"] != json!([])
                })
                .or(options.first());
            if let Some(option) = option {
                return sample(root, option, depth + 1);
            }
        }
    }
    let kind = match &schema["type"] {
        Value::String(kind) => kind.as_str(),
        Value::Array(kinds) => kinds
            .iter()
            .filter_map(Value::as_str)
            .find(|k| *k != "null")
            .unwrap_or("null"),
        _ => "object",
    };
    match kind {
        "string" => json!("x"),
        "integer" | "number" => json!(1),
        "boolean" => json!(false),
        "array" => json!([sample(root, &schema["items"], depth + 1)]),
        "null" => Value::Null,
        _ => {
            let mut object = Map::new();
            let mut keys = required(schema);
            // A nested object such as a rule's `match` needs at least one key to mean anything.
            if keys.is_empty() && depth > 0 {
                keys.extend(
                    schema["properties"]
                        .as_object()
                        .and_then(|p| p.keys().next().cloned()),
                );
            }
            for key in keys {
                object.insert(
                    key.clone(),
                    sample(root, &schema["properties"][&key], depth + 1),
                );
            }
            Value::Object(object)
        }
    }
}

fn required(schema: &Value) -> Vec<String> {
    schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|k| k.as_str().map(String::from))
        .collect()
}

/// Every junk argument set for one tool; `None` is a call without arguments.
fn cases(tool: &Tool) -> Vec<Option<JsonObject>> {
    let root = Value::Object((*tool.input_schema).clone());
    let Value::Object(base) = sample(&root, &root, 0) else {
        panic!("{} takes an object", tool.name);
    };
    let properties: Vec<String> = root["properties"]
        .as_object()
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();
    let with = |key: &str, value: Value| {
        let mut args = base.clone();
        args.insert(key.into(), value);
        Some(args)
    };
    let mut cases = vec![None, Some(JsonObject::new()), Some(base.clone())];
    for unknown in [
        "zzz_unknown".to_string(),
        String::new(),
        "k".repeat(100_000),
    ] {
        cases.push(with(&unknown, json!(1)));
    }
    for key in required(&root) {
        let mut args = base.clone();
        args.remove(&key);
        cases.push(Some(args));
    }
    for value in junk_values() {
        for key in &properties {
            cases.push(with(key, value.clone()));
        }
        let mut all = base.clone();
        for key in &properties {
            all.insert(key.clone(), value.clone());
        }
        cases.push(Some(all));
        if let Value::Object(object) = value {
            cases.push(Some(object));
        }
    }
    cases
}

/// What a junk call may change: every table of the account's store, rules.toml and the trash directory.
fn snapshot(fx: &Fixture) -> (Vec<String>, Option<Vec<u8>>, Vec<String>) {
    let db = rusqlite::Connection::open(fx.paths.mail_db("work")).unwrap();
    let tables: Vec<String> = db
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut rows = Vec::new();
    for table in tables {
        let mut stmt = db.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
        let width = stmt.column_count();
        let mut found = stmt.query([]).unwrap();
        while let Some(row) = found.next().unwrap() {
            let cells: Vec<String> = (0..width)
                .map(|i| format!("{:?}", row.get_ref(i).unwrap()))
                .collect();
            rows.push(format!("{table}: {}", cells.join(" | ")));
        }
    }
    rows.sort();
    let mut trash: Vec<String> = std::fs::read_dir(fx.paths.trash_dir("work"))
        .map(|dir| {
            dir.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    trash.sort();
    (rows, std::fs::read(fx.paths.rules_file()).ok(), trash)
}

const RULES: &str = "[[rules]]\nname = \"on\"\nmatch.subject = { contains = \"code\" }\nactions = [\"flag\"]\n\n\
    [[rules]]\nname = \"news\"\nenabled = false\nproposed_by = \"mcp\"\nmatch.subject = { contains = \"news\" }\nactions = [\"archive\"]\n";

/// One account with two messages, a log entry, a trash backup and two rules, one of them a proposal named `news`.
fn junk_fixture() -> Fixture {
    let fx = fixture(&["work"]);
    fx.add("work", fixture_message("INBOX", 1, "Code 1234", "made up"));
    let mut flagged = fixture_message("INBOX", 2, "News", "made up too");
    flagged.flags = "\\Flagged \\Seen".into();
    fx.add("work", flagged);
    fx.store("work")
        .log_action(&crate::store::LogEntry {
            id: 0,
            at: 1_790_000_000,
            rule_name: "on".into(),
            folder: "INBOX".into(),
            uid: 1,
            message_id: Some("<1@example.com>".into()),
            subject: Some("Code 1234".into()),
            action: "flag".into(),
            trash_file: None,
        })
        .unwrap();
    crate::trash::Trash::new(fx.paths.trash_dir("work"))
        .save("INBOX", 3, b"Subject: Gone\r\n\r\nmade up", 1_790_000_000)
        .unwrap();
    std::fs::write(fx.paths.rules_file(), RULES).unwrap();
    fx
}

/// The tool's result as `Ok(is_error)`, or the JSON-RPC error's message; anything else fails the test.
async fn call_once(
    client: &RunningService<RoleClient, ClientConfig>,
    name: &str,
    arguments: Option<JsonObject>,
) -> Result<bool, String> {
    let mut params = CallToolRequestParams::new(name.to_string());
    params.arguments = arguments;
    let shown = format!("{name} {:.300}", format!("{:?}", params.arguments));
    let outcome = tokio::time::timeout(PER_CALL, client.call_tool(params))
        .await
        .unwrap_or_else(|_| panic!("hung: {shown}"));
    match outcome {
        Ok(result) => {
            let text = serde_json::to_string(&result.content).unwrap();
            assert!(!text.contains("panicked"), "{shown}: {text:.500}");
            Ok(result.is_error == Some(true))
        }
        Err(ServiceError::McpError(e)) => {
            assert!(!e.message.contains("panicked"), "{shown}: {e:?}");
            Err(e.message.to_string())
        }
        Err(e) => panic!("{shown}: {e}"),
    }
}

#[tokio::test]
async fn every_tool_survives_junk_arguments() {
    let fx = junk_fixture();
    let scopes = Scope::ALL.map(Scope::as_str).join(",");
    let client = connect(&fx, &scopes, &[]).await;
    let tools = client.list_all_tools().await.unwrap();
    assert!(tools.len() > 1, "{tools:?}");
    let mut called = BTreeSet::new();
    let mut calls = 0;
    for tool in &tools {
        let name = tool.name.as_ref();
        for arguments in cases(tool) {
            let before = snapshot(&fx);
            let shown = format!("{name} {:.300}", format!("{arguments:?}"));
            let failed = !matches!(call_once(&client, name, arguments).await, Ok(false));
            let after = snapshot(&fx);
            if failed {
                assert!(before == after, "{shown} failed but changed state");
            } else if before.1 != after.1 {
                crate::rules::load(&fx.paths.rules_file())
                    .unwrap_or_else(|e| panic!("{shown} left rules.toml unreadable: {e:#}"));
                // Back to the fixture's rules, so the next proposal is not refused as a duplicate.
                std::fs::write(fx.paths.rules_file(), RULES).unwrap();
            }
            called.insert(name.to_string());
            calls += 1;
        }
    }
    let listed: BTreeSet<String> = tools.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(called, listed);
    assert!(calls > tools.len() * 20, "{calls}");
    let folders = rows(&super::tests::call(&client, "folders", json!({})).await);
    assert_eq!(folders[0]["total"], 2);
}

type RawRead = tokio::io::Lines<tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>>;

/// Sends `line` as is and returns the next line the server writes, or `None` when nothing comes within a second.
async fn exchange(
    write: &mut tokio::io::WriteHalf<tokio::io::DuplexStream>,
    read: &mut RawRead,
    line: &str,
) -> Option<Value> {
    use tokio::io::AsyncWriteExt;
    write.write_all(line.as_bytes()).await.unwrap();
    write.write_all(b"\n").await.unwrap();
    match tokio::time::timeout(Duration::from_secs(1), read.next_line()).await {
        Ok(next) => Some(serde_json::from_str(&next.unwrap().expect("server hung up")).unwrap()),
        Err(_) => None,
    }
}

/// Junk the typed client cannot send: arguments that are no object, params that are null or missing, JSON nested past
/// the parser's limit, and lines that are no JSON at all. Each gets an error or is dropped, and the server keeps serving.
#[tokio::test]
async fn junk_requests_get_errors_and_the_server_keeps_serving() {
    use rmcp::ServiceExt;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    let fx = junk_fixture();
    let (daemon, _sent, _inject) = crate::daemon::Client::in_memory(&["work"]);
    let backend = super::Backend::with_client(&fx.config(), &fx.paths, &[], daemon).unwrap();
    let scopes = super::parse_scopes(&Scope::ALL.map(Scope::as_str).join(",")).unwrap();
    let server = super::Server::new(backend, scopes);
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    let (read, mut write) = tokio::io::split(client_io);
    let mut read = tokio::io::BufReader::new(read).lines();
    let hello = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"raw","version":"1"}}}"#;
    let welcome = exchange(&mut write, &mut read, hello).await.unwrap();
    assert!(welcome["result"].is_object(), "{welcome}");
    write
        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
        .await
        .unwrap();
    let deep = format!("{}{}", "[".repeat(1_000), "]".repeat(1_000));
    let mut junk = Vec::new();
    for arguments in ["null", "[1]", "\"x\"", "7", "true", &deep] {
        junk.push(format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"mark","arguments":{arguments}}}}}"#
        ));
    }
    junk.push(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":null}"#.into());
    junk.push(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":null}}"#.into());
    junk.push(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call"}"#.into());
    junk.push("not json".into());
    junk.push("{".repeat(100_000));
    for (n, line) in junk.iter().enumerate() {
        if let Some(reply) = exchange(&mut write, &mut read, line).await {
            let failed = reply.get("error").is_some() || reply["result"]["isError"] == true;
            assert!(failed, "{line:.200}: {reply}");
            assert!(!reply.to_string().contains("panicked"), "{reply}");
        }
        let alive = format!(
            r#"{{"jsonrpc":"2.0","id":{},"method":"tools/call","params":{{"name":"folders","arguments":{{}}}}}}"#,
            100 + n
        );
        let reply = exchange(&mut write, &mut read, &alive)
            .await
            .unwrap_or_else(|| panic!("no reply after {line:.200}"));
        assert_eq!(reply["id"], 100 + n, "{reply}");
        assert_eq!(reply["result"]["structuredContent"]["rows"][0]["total"], 2);
    }
}
