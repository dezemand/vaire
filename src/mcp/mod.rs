//! The STDIO MCP server — `vaire mcp` (cli.md §5, design.md §9).
//!
//! Speaks JSON-RPC 2.0 over newline-delimited stdio and exposes the five **read**
//! commands as MCP tools, one-to-one. The tools *are* the CLI commands: each tool
//! re-dispatches into `commands::*::run` and returns that command's `--json` shape
//! verbatim as the tool result text — so there is no second serialization to drift.
//!
//! Maintenance commands are **not** exposed: the agent-facing surface is bounded to
//! reads. The server operates against the already-built index and never builds or
//! writes it; a missing index surfaces as a tool error pointing at `vaire index`
//! (mirroring exit `4`), not a build.

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

use crate::commands::{self, Ctx};
use crate::error::Result;
use crate::output::Output;

/// The MCP protocol revision this server advertises.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// The read tools, mapped 1:1 to CLI commands (cli.md §5 table).
pub const READ_TOOLS: [&str; 7] = [
    "resolve",
    "render",
    "backlinks",
    "refs",
    "search",
    "suggest",
    "unresolved",
];

/// Run the STDIO MCP server until the client disconnects (EOF on stdin).
pub fn serve(ctx: Ctx) -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_line(&ctx, &line) {
            writeln!(out, "{response}")?;
            out.flush()?;
        }
    }
    Ok(())
}

/// Handle one JSON-RPC line. Returns `Some(response_json)` for requests (those with an
/// `id`) and `None` for notifications (e.g. `notifications/initialized`).
fn handle_line(ctx: &Ctx, line: &str) -> Option<String> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Some(error_response(&Value::Null, -32700, "parse error")),
    };
    let id = msg.get("id").cloned()?; // no id ⇒ notification ⇒ no response
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    Some(match dispatch(ctx, method, &params) {
        Ok(result) => success_response(&id, result),
        Err((code, message)) => error_response(&id, code, &message),
    })
}

/// Route a JSON-RPC method to its result, or a `(code, message)` JSON-RPC error.
fn dispatch(ctx: &Ctx, method: &str, params: &Value) -> std::result::Result<Value, (i64, String)> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "vaire", "version": env!("CARGO_PKG_VERSION") },
        })),
        "tools/list" => Ok(json!({ "tools": tools_list() })),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or((-32602, "missing tool name".to_string()))?;
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            // Protocol errors (unknown tool / bad args) → JSON-RPC error; command errors
            // come back inside the tool result as `isError: true`.
            call_tool(ctx, name, &args).map_err(|m| (-32602, m))
        }
        "ping" => Ok(json!({})),
        "" => Err((-32600, "invalid request".to_string())),
        other => Err((-32601, format!("method not found: {other}"))),
    }
}

/// Invoke a read tool by name. `Ok` is the MCP tool result (`content` + `isError`);
/// `Err` is a protocol-level problem (unknown tool, missing required argument).
pub fn call_tool(ctx: &Ctx, name: &str, args: &Value) -> std::result::Result<Value, String> {
    let outcome: Result<Value> = match name {
        "resolve" => commands::resolve::run(ctx, &required_str(args, "id")?).map(|o| o.to_json()),
        "render" => commands::render::run(ctx, &required_str(args, "id")?).map(|o| o.to_json()),
        "backlinks" => commands::backlinks::run(
            ctx,
            &required_str(args, "id")?,
            opt_str(args, "type").as_deref(),
            opt_usize(args, "limit"),
        )
        .map(|o| o.to_json()),
        "refs" => commands::refs::run(
            ctx,
            &required_str(args, "id")?,
            opt_u64(args, "depth").unwrap_or(1) as u32,
            opt_str(args, "type").as_deref(),
        )
        .map(|o| o.to_json()),
        "search" => commands::search::run(
            ctx,
            &required_str(args, "query")?,
            opt_str(args, "type").as_deref(),
            opt_str(args, "scope").as_deref(),
            opt_usize(args, "limit"),
        )
        .map(|o| o.to_json()),
        "suggest" => commands::suggest::run(
            ctx,
            &required_str(args, "descriptor")?,
            opt_str(args, "type").as_deref(),
            opt_usize(args, "limit"),
        )
        .map(|o| o.to_json()),
        "unresolved" => commands::unresolved::run(
            ctx,
            opt_str(args, "type").as_deref(),
            opt_str(args, "scope").as_deref(),
        )
        .map(|o| o.to_json()),
        other => return Err(format!("unknown tool: {other}")),
    };

    Ok(match outcome {
        Ok(value) => tool_result(value, false),
        // Command errors (id not found, index missing, …) become a tool error the agent
        // sees, carrying the same `{"error": {...}}` JSON shape as the CLI (cli.md §7).
        Err(e) => tool_result(e.to_json(), true),
    })
}

/// The tool definitions advertised by `tools/list`. Input schemas mirror each command's
/// args/flags (cli.md §5).
pub fn tools_list() -> Value {
    json!([
        {
            "name": "resolve",
            "description": "Resolve a node ID (type:id) to its location and frontmatter; follows superseded_by redirects.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string", "description": "A composed node ID, e.g. person:jane-doe" } },
                "required": ["id"]
            }
        },
        {
            "name": "render",
            "description": "Render a node as portable Markdown: frontmatter kept, wikilinks resolved to [name](relative-path).",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }
        },
        {
            "name": "backlinks",
            "description": "Nodes that reference <id> (inbound edges).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "type": { "type": "string", "description": "Restrict to referencing nodes of this type" },
                    "limit": { "type": "integer", "description": "Cap results" }
                },
                "required": ["id"]
            }
        },
        {
            "name": "refs",
            "description": "Nodes that <id> references (outbound edges); unresolved [[?...]] are excluded.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "depth": { "type": "integer", "description": "Traverse N hops (default 1)" },
                    "type": { "type": "string" }
                },
                "required": ["id"]
            }
        },
        {
            "name": "search",
            "description": "Hybrid full-text + vector search; returns files with matching section anchors.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "type": { "type": "string" },
                    "scope": { "type": "string", "description": "Restrict to records in a project (project:...)" },
                    "limit": { "type": "integer", "description": "Max results (default 10)" }
                },
                "required": ["query"]
            }
        },
        {
            "name": "suggest",
            "description": "Suggest existing node IDs a descriptor might refer to (lookup-before-reference); ranked by name/alias then prose.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "descriptor": { "type": "string" },
                    "type": { "type": "string", "description": "Restrict to a node type" },
                    "limit": { "type": "integer", "description": "Max suggestions (default 5)" }
                },
                "required": ["descriptor"]
            }
        },
        {
            "name": "unresolved",
            "description": "Every unresolved reference ([[?...]]) currently in the corpus.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "type": { "type": "string", "description": "Restrict to a ?type hint" },
                    "scope": { "type": "string" }
                }
            }
        }
    ])
}

// ---- JSON-RPC + argument helpers -------------------------------------------

fn tool_result(value: Value, is_error: bool) -> Value {
    json!({
        "content": [ { "type": "text", "text": value.to_string() } ],
        "isError": is_error,
    })
}

fn success_response(id: &Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error_response(id: &Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

fn required_str(args: &Value, key: &str) -> std::result::Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing required argument '{key}'"))
}

fn opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}

fn opt_usize(args: &Value, key: &str) -> Option<usize> {
    args.get(key).and_then(Value::as_u64).map(|n| n as usize)
}

fn opt_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(Value::as_u64)
}
