//! # mcp
//!
//! Model Context Protocol (MCP) stdio transport server for `ctxd`.
//!
//! ## What is MCP?
//!
//! MCP is an open standard by Anthropic that lets AI agents (Claude Desktop,
//! Cursor, Antigravity) call external "tools" over JSON-RPC 2.0 via stdin/stdout.
//! Once `ctxd` is registered as an MCP server, AI agents can:
//! - Call `get_context` to inject recent developer activity into their context window
//! - Call `search_context` to retrieve semantically relevant past events via BM25
//! - Call `remember` to store decisions or notes from within the AI conversation
//! - Call `get_task` to know what the developer is currently working on
//!
//! ## Protocol Shape (MCP 0.1 — Claude Desktop compatible)
//!
//! ```text
//! stdin  (from AI agent) → { "jsonrpc": "2.0", "method": "...", "params": {...}, "id": N }
//! stdout (to AI agent)   → { "jsonrpc": "2.0", "result": { "content": [...] }, "id": N }
//! ```
//!
//! ## Dogfood Loop (Phase 2 design rationale)
//!
//! By shipping MCP in Phase 2 rather than Phase 6, the AI agent building Contexto
//! can immediately use Contexto to track its own progress. This "dogfood loop"
//! creates a positive feedback cycle that accelerates all subsequent phases.
//!
//! ## Registering with Claude Desktop
//!
//! Add to `~/Library/Application Support/Claude/claude_desktop_config.json`:
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "contexto": {
//!       "command": "/path/to/ctxd",
//!       "args": ["--mcp"]
//!     }
//!   }
//! }
//! ```

use std::sync::Arc;

use ctx_core::{ContextEvent, EventFilter, EventSource};
use ctx_db::ContextoDb;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

// =============================================================================
// JSON-RPC 2.0 Types
// =============================================================================

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: Option<Value>,
    pub id: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    pub id: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

impl JsonRpcResponse {
    fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            result: Some(result),
            error: None,
            id,
        }
    }

    fn err(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
            }),
            id,
        }
    }
}

// =============================================================================
// MCP Tool Descriptors
// =============================================================================

/// Returns the `tools/list` result — the list of tools ctxd exposes to AI agents.
fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "get_context",
                "description": "Retrieve recent developer context events from Contexto. Use this to understand what the developer has been working on recently — file changes, terminal commands, git commits, and manual notes.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of events to return (1–50, default 10)",
                            "default": 10
                        },
                        "source": {
                            "type": "string",
                            "description": "Filter by event source: terminal, editor, git, file_system, manual, mcp",
                            "enum": ["terminal", "editor", "git", "file_system", "manual", "mcp"]
                        },
                        "git_repo": {
                            "type": "string",
                            "description": "Filter to a specific git repository root path"
                        }
                    }
                }
            },
            {
                "name": "search_context",
                "description": "Full-text search through all developer context events using BM25 ranking. Use this to find specific past decisions, errors, commands, or file changes by keyword.",
                "inputSchema": {
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query. Supports FTS5 syntax: 'exact phrase', term1 AND term2, prefix*"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum results (1–20, default 5)",
                            "default": 5
                        }
                    }
                }
            },
            {
                "name": "remember",
                "description": "Store a note or decision in Contexto. Use this to record architectural decisions, TODOs, or important context from the current conversation that should be persisted.",
                "inputSchema": {
                    "type": "object",
                    "required": ["note"],
                    "properties": {
                        "note": {
                            "type": "string",
                            "description": "The content to remember"
                        },
                        "label": {
                            "type": "string",
                            "description": "Short label for the note (default: 'AI note')"
                        }
                    }
                }
            },
            {
                "name": "get_task",
                "description": "Get the currently active developer task from Contexto, including its name, start time, and event count. Returns null if no task is active.",
                "inputSchema": {
                    "type": "object",
                    "properties": {}
                }
            }
        ]
    })
}

// =============================================================================
// MCP Server Entry Point
// =============================================================================

/// Run the MCP JSON-RPC 2.0 stdio server.
///
/// Reads newline-delimited JSON from `stdin`, processes each request,
/// and writes responses to `stdout`. Runs until EOF on stdin.
///
/// This function is intended to be spawned as a background tokio task:
/// ```rust
/// tokio::spawn(mcp::run_stdio_server(db.clone()));
/// ```
pub async fn run_stdio_server(db: Arc<ContextoDb>) {
    tracing::info!("MCP stdio server started — listening on stdin");

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();
    let mut writer = tokio::io::BufWriter::new(stdout);

    while let Ok(Some(line)) = reader.next_line().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let response = handle_request(&db, &line).await;

        match serde_json::to_string(&response) {
            Ok(json) => {
                if let Err(e) = writer.write_all(json.as_bytes()).await {
                    tracing::error!("MCP: failed to write response: {e}");
                    break;
                }
                if let Err(e) = writer.write_all(b"\n").await {
                    tracing::error!("MCP: failed to write newline: {e}");
                    break;
                }
                if let Err(e) = writer.flush().await {
                    tracing::error!("MCP: failed to flush: {e}");
                    break;
                }
            }
            Err(e) => {
                tracing::error!("MCP: failed to serialize response: {e}");
            }
        }
    }

    tracing::info!("MCP stdio server stopped (stdin closed)");
}

// =============================================================================
// Request Dispatch
// =============================================================================

async fn handle_request(db: &ContextoDb, raw: &str) -> JsonRpcResponse {
    let req: JsonRpcRequest = match serde_json::from_str(raw) {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::err(None, -32700, format!("Parse error: {e}"));
        }
    };

    if req.jsonrpc != "2.0" {
        return JsonRpcResponse::err(req.id, -32600, "Invalid JSON-RPC version");
    }

    tracing::debug!(method = %req.method, "MCP request");

    match req.method.as_str() {
        // MCP handshake
        "initialize" => handle_initialize(req.id),
        "notifications/initialized" => {
            // No response expected for notifications
            JsonRpcResponse::ok(req.id, json!({}))
        }

        // Tool discovery
        "tools/list" => JsonRpcResponse::ok(req.id, tools_list()),

        // Tool invocations
        "tools/call" => {
            let params = req.params.unwrap_or(json!({}));
            let tool_name = params["name"].as_str().unwrap_or("").to_string();
            let args = params["arguments"].clone();
            dispatch_tool(db, req.id, &tool_name, args).await
        }

        unknown => JsonRpcResponse::err(req.id, -32601, format!("Method not found: {unknown}")),
    }
}

fn handle_initialize(id: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::ok(
        id,
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "ctxd",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
    )
}

async fn dispatch_tool(
    db: &ContextoDb,
    id: Option<Value>,
    name: &str,
    args: Value,
) -> JsonRpcResponse {
    match name {
        "get_context" => tool_get_context(db, id, args).await,
        "search_context" => tool_search_context(db, id, args).await,
        "remember" => tool_remember(db, id, args).await,
        "get_task" => tool_get_task(db, id).await,
        unknown => JsonRpcResponse::err(id, -32601, format!("Unknown tool: {unknown}")),
    }
}

// =============================================================================
// Tool Implementations
// =============================================================================

async fn tool_get_context(db: &ContextoDb, id: Option<Value>, args: Value) -> JsonRpcResponse {
    let limit = args["limit"].as_u64().unwrap_or(10).clamp(1, 50) as u32;

    let mut filter = EventFilter::new();
    filter.limit = Some(limit);

    if let Some(src) = args["source"].as_str() {
        match src.parse() {
            Ok(s) => filter.source = Some(s),
            Err(_) => {
                return JsonRpcResponse::err(id, -32602, format!("Invalid source: '{src}'"));
            }
        }
    }

    if let Some(repo) = args["git_repo"].as_str() {
        filter.git_repo = Some(repo.to_string());
    }

    match db.get_recent(&filter).await {
        Ok(events) => {
            let text = format_events_as_markdown(&events);
            JsonRpcResponse::ok(
                id,
                json!({
                    "content": [{ "type": "text", "text": text }]
                }),
            )
        }
        Err(e) => JsonRpcResponse::err(id, -32603, format!("DB error: {e}")),
    }
}

async fn tool_search_context(db: &ContextoDb, id: Option<Value>, args: Value) -> JsonRpcResponse {
    let query = match args["query"].as_str() {
        Some(q) if !q.trim().is_empty() => q.to_string(),
        _ => {
            return JsonRpcResponse::err(id, -32602, "Missing required argument: 'query'");
        }
    };

    let limit = args["limit"].as_u64().unwrap_or(5).clamp(1, 20) as u32;

    match db.search(&query, limit).await {
        Ok(events) => {
            let text = if events.is_empty() {
                format!("No results found for query: '{query}'")
            } else {
                format!(
                    "**Search results for '{query}'** ({} found)\n\n{}",
                    events.len(),
                    format_events_as_markdown(&events)
                )
            };

            JsonRpcResponse::ok(
                id,
                json!({
                    "content": [{ "type": "text", "text": text }]
                }),
            )
        }
        Err(e) => JsonRpcResponse::err(id, -32603, format!("Search error: {e}")),
    }
}

async fn tool_remember(db: &ContextoDb, id: Option<Value>, args: Value) -> JsonRpcResponse {
    let note = match args["note"].as_str() {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => {
            return JsonRpcResponse::err(id, -32602, "Missing required argument: 'note'");
        }
    };

    let label = args["label"].as_str().unwrap_or("AI note").to_string();

    let event = ContextEvent::new(EventSource::Mcp, label, note);

    match db.insert_events_batch(&[event]).await {
        Ok(_) => JsonRpcResponse::ok(
            id,
            json!({
                "content": [{ "type": "text", "text": "Note saved to Contexto." }]
            }),
        ),
        Err(e) => JsonRpcResponse::err(id, -32603, format!("DB error: {e}")),
    }
}

async fn tool_get_task(db: &ContextoDb, id: Option<Value>) -> JsonRpcResponse {
    match db.get_active_task().await {
        Ok(Some(task)) => {
            let text = format!(
                "**Active Task:** {}\n- **ID:** {}\n- **Status:** {}\n- **Started:** {}\n- **Events captured:** {}{}",
                task.name,
                task.id,
                task.status,
                task.started_at.format("%Y-%m-%d %H:%M UTC"),
                task.event_count,
                task.description.as_deref().map(|d| format!("\n- **Description:** {d}")).unwrap_or_default()
            );
            JsonRpcResponse::ok(
                id,
                json!({
                    "content": [{ "type": "text", "text": text }]
                }),
            )
        }
        Ok(None) => JsonRpcResponse::ok(
            id,
            json!({
                "content": [{ "type": "text", "text": "No active task. Start one with `ctx task start <name>`." }]
            }),
        ),
        Err(e) => JsonRpcResponse::err(id, -32603, format!("DB error: {e}")),
    }
}

// =============================================================================
// Formatting Helpers
// =============================================================================

fn format_events_as_markdown(events: &[ContextEvent]) -> String {
    if events.is_empty() {
        return "No context events found.".to_string();
    }

    events
        .iter()
        .map(|e| {
            let ts = e.timestamp.format("%Y-%m-%d %H:%M UTC");
            let source = &e.source;
            let redacted = if e.was_redacted {
                " ⚠️ redacted"
            } else {
                ""
            };
            format!(
                "**[{source}] {label}** _{ts}{redacted}_\n```\n{content}\n```",
                label = e.label,
                content = truncate_for_context(&e.content, 400),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}

/// Truncate content for MCP context window budget.
fn truncate_for_context(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        s
    } else {
        // Try to cut at a newline boundary
        &s[..s
            .char_indices()
            .nth(max_chars)
            .map_or(max_chars, |(i, _)| i)]
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_db::ContextoDb;

    async fn test_db() -> Arc<ContextoDb> {
        Arc::new(ContextoDb::open(":memory:").await.expect("in-memory DB"))
    }

    fn make_request(method: &str, params: &Value) -> String {
        serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": 1
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn test_initialize_handshake() {
        let db = test_db().await;
        let raw = make_request("initialize", &json!({ "protocolVersion": "2024-11-05" }));
        let resp = handle_request(&db, &raw).await;

        assert!(resp.error.is_none());
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["serverInfo"]["name"].as_str().is_some());
    }

    #[tokio::test]
    async fn test_tools_list_returns_four_tools() {
        let db = test_db().await;
        let raw = make_request("tools/list", &json!({}));
        let resp = handle_request(&db, &raw).await;

        assert!(resp.error.is_none());
        let tools = &resp.result.unwrap()["tools"];
        assert_eq!(tools.as_array().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn test_get_context_tool_empty_db() {
        let db = test_db().await;
        let raw = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": { "name": "get_context", "arguments": { "limit": 5 } },
            "id": 1
        }))
        .unwrap();

        let resp = handle_request(&db, &raw).await;
        assert!(resp.error.is_none());
        let text = resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(text.contains("No context events"), "got: {text}");
    }

    #[tokio::test]
    async fn test_remember_tool_persists_event() {
        let db = test_db().await;
        let raw = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": {
                "name": "remember",
                "arguments": { "note": "Use LanceDB for Phase 6", "label": "design decision" }
            },
            "id": 2
        }))
        .unwrap();

        let resp = handle_request(&db, &raw).await;
        assert!(resp.error.is_none());

        // Verify it was actually persisted
        let events = db.get_recent(&EventFilter::new()).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].label, "design decision");
        assert!(events[0].content.contains("LanceDB"));
    }

    #[tokio::test]
    async fn test_search_context_tool() {
        let db = test_db().await;

        // Seed data
        let event = ContextEvent::new(
            EventSource::Manual,
            "arch note",
            "Use ring buffer mpsc channel for back-pressure",
        );
        db.insert_events_batch(&[event]).await.unwrap();

        let raw = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": {
                "name": "search_context",
                "arguments": { "query": "ring buffer", "limit": 5 }
            },
            "id": 3
        }))
        .unwrap();

        let resp = handle_request(&db, &raw).await;
        assert!(resp.error.is_none());
        let text = resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            text.contains("ring buffer"),
            "search should find seeded event, got: {text}"
        );
    }

    #[tokio::test]
    async fn test_get_task_tool_no_active_task() {
        let db = test_db().await;
        let raw = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": { "name": "get_task", "arguments": {} },
            "id": 4
        }))
        .unwrap();

        let resp = handle_request(&db, &raw).await;
        assert!(resp.error.is_none());
        let text = resp.result.unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(text.contains("No active task"), "got: {text}");
    }

    #[tokio::test]
    async fn test_unknown_method_returns_error() {
        let db = test_db().await;
        let raw = make_request("unknown/method", &json!({}));
        let resp = handle_request(&db, &raw).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[tokio::test]
    async fn test_missing_required_arg_returns_error() {
        let db = test_db().await;
        let raw = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": { "name": "search_context", "arguments": {} },
            "id": 5
        }))
        .unwrap();

        let resp = handle_request(&db, &raw).await;
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }
}
