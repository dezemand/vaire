//! Spec tests for the MCP server (cli.md §5).
//!
//! In-process tests exercise tool dispatch directly (fast, deterministic); one
//! process-level test drives the real `vaire mcp` over stdio to validate the wire
//! handshake and framing.

mod common;

use std::io::Write;
use std::process::{Command, Stdio};

use common::Corpus;
use serde_json::{Value, json};
use vaire::mcp;

// ---- in-process: the tools surface ----------------------------------------

#[test]
fn tools_list_exposes_exactly_the_read_commands() {
    let tools = mcp::tools_list();
    let names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vaire::mcp::READ_TOOLS);
    // Every tool advertises an object input schema.
    for t in tools.as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object");
    }
}

#[test]
fn call_tool_returns_command_json_verbatim() {
    let c = Corpus::fixture();
    let result = mcp::call_tool(&c.ctx(), "resolve", &json!({ "id": "person:jane-doe" })).unwrap();

    assert_eq!(result["isError"], false);
    // The tool result text is the command's `--json` shape, verbatim.
    let text = result["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["id"], "person:jane-doe");
    assert_eq!(parsed["type"], "person");
}

#[test]
fn call_tool_command_error_is_a_tool_error_not_a_panic() {
    let c = Corpus::fixture();
    let result = mcp::call_tool(&c.ctx(), "resolve", &json!({ "id": "person:nobody" })).unwrap();
    assert_eq!(result["isError"], true);
    let text = result["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["error"]["code"], 5); // id_not_found mirrors exit 5
}

#[test]
fn unknown_tool_is_a_protocol_error() {
    let c = Corpus::fixture();
    let err = mcp::call_tool(&c.ctx(), "index", &json!({})).unwrap_err();
    assert!(err.contains("unknown tool"));
}

#[test]
fn missing_required_argument_is_a_protocol_error() {
    let c = Corpus::fixture();
    let err = mcp::call_tool(&c.ctx(), "resolve", &json!({})).unwrap_err();
    assert!(err.contains("id"));
}

// ---- process-level: the stdio wire protocol --------------------------------

#[test]
fn stdio_handshake_and_tool_call() {
    let c = Corpus::fixture();

    let mut child = Command::new(env!("CARGO_BIN_EXE_vaire"))
        .arg("--repo")
        .arg(c.root())
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn vaire mcp");

    {
        let stdin = child.stdin.as_mut().unwrap();
        let msgs = [
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "resolve", "arguments": { "id": "department:logistics" } }
            }),
        ];
        for m in msgs {
            writeln!(stdin, "{m}").unwrap();
        }
        // stdin dropped here → EOF → server exits after draining.
    }

    let output = child.wait_with_output().expect("server exits");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let responses: Vec<Value> = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("each line is JSON-RPC"))
        .collect();

    // One response per request (the notification produced none).
    assert_eq!(responses.len(), 3);
    let by_id = |id: i64| responses.iter().find(|r| r["id"] == id).unwrap();

    assert_eq!(by_id(1)["result"]["serverInfo"]["name"], "vaire");
    assert_eq!(by_id(2)["result"]["tools"].as_array().unwrap().len(), 7);

    let call = by_id(3);
    assert_eq!(call["result"]["isError"], false);
    let text = call["result"]["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["id"], "department:logistics");
}
