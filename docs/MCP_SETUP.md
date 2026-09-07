# Connecting Contexto to AI Agents via MCP

`ctxd` speaks the **Model Context Protocol (MCP)** over stdin/stdout, which means
any MCP-compatible AI agent can call Contexto tools while it works with you.

---

## Available MCP Tools

| Tool | Description |
|---|---|
| `get_context` | Retrieve recent developer events (terminal, git, editor, files) |
| `search_context` | BM25 full-text search through all captured events |
| `remember` | Store a note or decision persistently in Contexto |
| `get_task` | Get the currently active developer task |

---

## Claude Desktop Setup

1. Make sure `ctxd` is built and accessible: `cargo build --release`
2. Edit `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "contexto": {
      "command": "/Users/<you>/Desktop/Contexto/target/release/ctxd",
      "args": ["--mcp"]
    }
  }
}
```

3. Restart Claude Desktop — you'll see Contexto tools appear in the tool picker.

---

## Cursor Setup

Add to `.cursor/mcp.json` in your project root or `~/.cursor/mcp.json` globally:

```json
{
  "servers": {
    "contexto": {
      "command": "/path/to/ctxd",
      "args": ["--mcp"],
      "transport": "stdio"
    }
  }
}
```

---

## Antigravity / AGY Setup

Add to `.agents/mcp_config.json` in your workspace:

```json
{
  "servers": {
    "contexto": {
      "command": "/path/to/ctxd",
      "args": ["--mcp"],
      "transport": "stdio"
    }
  }
}
```

---

## REST API Quickstart

```bash
# Start the daemon
./target/release/ctxd

# Grab the auth token
TOKEN=$(cat ~/.ctx/auth_token)

# Ingest an event
curl -X POST http://127.0.0.1:8942/events \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"source":"terminal","label":"cargo build","content":"Compiling ctxd v0.2.0"}'

# Query recent context
curl "http://127.0.0.1:8942/context?limit=10" \
  -H "Authorization: Bearer $TOKEN" | jq .

# Full-text search
curl "http://127.0.0.1:8942/search?q=cargo+build" \
  -H "Authorization: Bearer $TOKEN" | jq .

# Store a manual note
curl -X POST http://127.0.0.1:8942/remember \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"note":"Decided to use LanceDB for Phase 6 vector search"}'

# Daemon health
curl "http://127.0.0.1:8942/status" \
  -H "Authorization: Bearer $TOKEN" | jq .

# Real-time SSE stream (open in a second terminal)
curl -N "http://127.0.0.1:8942/events/stream" \
  -H "Authorization: Bearer $TOKEN"
```
