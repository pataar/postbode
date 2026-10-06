//! The MCP tools: definitions, scope filtering, and results cleaned for an agent.
use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorCode,
    Implementation, JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities,
    ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{Backend, Scope};
use crate::help;
use crate::message::clean;

/// The tool list depends only on the server's arguments, so a host may keep it for the session.
const LIST_TTL_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Clone, Copy)]
#[expect(dead_code, reason = "the changing tools construct these")]
enum Effect {
    /// Changes something but adds or moves rather than destroys; MCP treats an absent hint as destructive.
    Changes,
    Destructive,
    ReadOnly,
}

struct ToolDef {
    name: &'static str,
    scope: Scope,
    effect: Effect,
    description: String,
    input_schema: Arc<JsonObject>,
}

impl ToolDef {
    fn new<A: JsonSchema>(
        name: &'static str,
        scope: Scope,
        effect: Effect,
        description: impl Into<String>,
    ) -> ToolDef {
        ToolDef {
            name,
            scope,
            effect,
            description: description.into(),
            input_schema: schema::<A>(),
        }
    }

    fn into_tool(self) -> Tool {
        let hints = match self.effect {
            Effect::Changes => ToolAnnotations::new().read_only(false).destructive(false),
            Effect::Destructive => ToolAnnotations::new().read_only(false).destructive(true),
            Effect::ReadOnly => ToolAnnotations::new().read_only(true),
        };
        Tool::new(self.name, self.description, self.input_schema).with_annotations(hints)
    }
}

fn schema<A: JsonSchema>() -> Arc<JsonObject> {
    match serde_json::to_value(schemars::schema_for!(A)) {
        Ok(Value::Object(map)) => Arc::new(map),
        _ => unreachable!("a derived schema is a JSON object"),
    }
}

fn args<A: DeserializeOwned>(arguments: Option<JsonObject>) -> Result<A> {
    serde_json::from_value(Value::Object(arguments.unwrap_or_default()))
        .map_err(|e| anyhow!("invalid arguments: {e}"))
}

fn inbox() -> String {
    "INBOX".into()
}

fn fifty() -> u32 {
    50
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AccountArgs {
    /// Only this account; default every visible account
    account: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    /// Only this account; default every visible account
    account: Option<String>,
    #[serde(default = "inbox")]
    folder: String,
    /// At most 500
    #[serde(default = "fifty")]
    limit: u32,
    /// Group by conversation, the most recently active thread first, each row with its `depth`
    #[serde(default)]
    threads: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LogArgs {
    /// Only this account; default every visible account
    account: Option<String>,
    #[serde(default = "fifty")]
    limit: u32,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    query: String,
    /// Only this account; default every visible account
    account: Option<String>,
    /// Only this folder; default every folder
    folder: Option<String>,
    /// At most 500
    #[serde(default = "fifty")]
    limit: u32,
}

/// Every tool, in any order; the scope filter and the alphabetical sort happen when listing. `bodies` is whether
/// `read:bodies` is granted, which changes what `search` covers.
fn catalog(bodies: bool) -> Vec<ToolDef> {
    let search = if bodies {
        help::SEARCH.to_string()
    } else {
        "Search subject and addresses for all the given words, newest first; bodies need the read:bodies scope".to_string()
    };
    vec![
        ToolDef::new::<AccountArgs>("folders", Scope::Read, Effect::ReadOnly, help::FOLDERS),
        ToolDef::new::<ListArgs>("list", Scope::Read, Effect::ReadOnly, help::LIST),
        ToolDef::new::<LogArgs>("log", Scope::Read, Effect::ReadOnly, help::LOG),
        ToolDef::new::<SearchArgs>("search", Scope::Read, Effect::ReadOnly, search),
        ToolDef::new::<AccountArgs>(
            "trash_list",
            Scope::Read,
            Effect::ReadOnly,
            help::TRASH_LIST,
        ),
    ]
}

/// Runs one tool the caller has checked against the scopes; list results come back as `{"rows": [...]}`.
fn dispatch(
    backend: &Backend,
    bodies: bool,
    client: Option<&str>,
    name: &str,
    arguments: Option<JsonObject>,
) -> Result<Value> {
    let _ = client;
    match name {
        "folders" => {
            let a: AccountArgs = args(arguments)?;
            rows(backend.folders(a.account.as_deref())?)
        }
        "list" => {
            let a: ListArgs = args(arguments)?;
            rows(backend.list(a.account.as_deref(), &a.folder, a.limit, a.threads)?)
        }
        "log" => {
            let a: LogArgs = args(arguments)?;
            rows(backend.log(a.account.as_deref(), a.limit)?)
        }
        "search" => {
            let a: SearchArgs = args(arguments)?;
            rows(backend.search(
                &a.query,
                a.account.as_deref(),
                a.folder.as_deref(),
                a.limit,
                bodies,
            )?)
        }
        "trash_list" => {
            let a: AccountArgs = args(arguments)?;
            rows(backend.trash_list(a.account.as_deref())?)
        }
        _ => bail!("unknown tool {name}"),
    }
}

fn rows(rows: Vec<Value>) -> Result<Value> {
    Ok(json!({ "rows": rows }))
}

/// Every string in the result without control characters; mail text is written by strangers.
fn cleaned(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(clean(&s, false)),
        Value::Array(items) => Value::Array(items.into_iter().map(cleaned).collect()),
        Value::Object(map) => {
            Value::Object(map.into_iter().map(|(k, v)| (k, cleaned(v))).collect())
        }
        other => other,
    }
}

fn error_result(text: &str) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(clean(text, false))])
}

pub struct Server {
    backend: Arc<Backend>,
    scopes: BTreeSet<Scope>,
}

impl Server {
    pub fn new(backend: Backend, scopes: BTreeSet<Scope>) -> Server {
        Server {
            backend: Arc::new(backend),
            scopes,
        }
    }

    fn bodies(&self) -> bool {
        self.scopes.contains(&Scope::ReadBodies)
    }
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("postbode", env!("CARGO_PKG_VERSION")))
            .with_instructions(include_str!("../../docs/src/agent-guide.md"))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let mut granted: Vec<ToolDef> = catalog(self.bodies())
            .into_iter()
            .filter(|t| self.scopes.contains(&t.scope))
            .collect();
        granted.sort_by_key(|t| t.name);
        let tools = granted.into_iter().map(ToolDef::into_tool).collect();
        Ok(ListToolsResult::with_all_items(tools)
            .with_ttl_ms(LIST_TTL_MS)
            .with_cache_scope(CacheScope::Private))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = request.name.to_string();
        let Some(tool) = catalog(false).into_iter().find(|t| t.name == name) else {
            return Err(McpError::new(
                ErrorCode::METHOD_NOT_FOUND,
                format!("unknown tool {name}"),
                None,
            ));
        };
        if !self.scopes.contains(&tool.scope) {
            return Ok(error_result("not allowed with these scopes").into());
        }
        let client = context.client_info().map(|info| info.name);
        let (backend, bodies, arguments) = (self.backend.clone(), self.bodies(), request.arguments);
        let outcome = tokio::task::spawn_blocking(move || {
            dispatch(&backend, bodies, client.as_deref(), &name, arguments)
        })
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(match outcome {
            Ok(value) => CallToolResult::structured(cleaned(value)),
            Err(e) => error_result(&format!("{e:#}")),
        }
        .into())
    }
}
