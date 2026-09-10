# ARCHITECTURE.md — Contexto Universal Developer Context Manager

Technical architecture reference and phase-by-phase implementation roadmap.

---

## System Overview

```
┌────────────────────────────────────────────────────────────────────┐
│                        Developer Workstation                       │
│                                                                    │
│  ┌───────────┐   ┌───────────┐   ┌─────────────────────────────┐   │
│  │ VS Code   │   │ Terminal  │   │     Tauri Desktop App       │   │
│  │ Extension │   │   Hook    │   │   (Linear/Raycast Dark UI)  │   │
│  └─────┬─────┘   └─────┬─────┘   └──────────────┬──────────────┘   │
│        │               │                         │                 │
│        │  HTTP + Bearer Token                    │                 │
│        └───────────────┼─────────────────────────┘                 │
│                        ▼                                           │
│              ┌─────────────────┐                                   │
│              │   ctxd Daemon   │  ← Auth token guard (0600)        │
│              │  ┌───────────┐  │                                   │
│              │  │ REST API  │  │  ← HTTP 127.0.0.1:8942            │
│              │  │ MCP stdio │  │  ← Model Context Protocol         │
│              │  └─────┬─────┘  │                                   │
│              │        │        │                                   │
│              │  ┌─────▼─────┐  │                                   │
│              │  │  mpsc(1K) │  │  ← Ring buffer (burst absorber)   │
│              │  │ Ring Buf  │  │                                   │
│              │  └─────┬─────┘  │                                   │
│              │        │        │                                   │
│              │  ┌─────▼─────┐  │                                   │
│              │  │  ctx-db   │  │  ← SQLite + FTS5                  │
│              │  │ Batch Wtr │  │  ← 500ms / 50 events flush        │
│              │  └───────────┘  │                                   │
│              └─────────────────┘                                   │
│                                                                    │
│              ┌─────────────────┐                                   │
│              │   ctx CLI       │  ← ctx status / context / search  │
│              └─────────────────┘                                   │
└────────────────────────────────────────────────────────────────────┘
```

---

## Crate Structure

```
Contexto/
├── Cargo.toml                    # Workspace root
├── rust-toolchain.toml           # Pinned to 1.82.0
│
├── crates/
│   ├── ctx-core/                 # Shared types & ring buffer
│   │   └── src/lib.rs            # ContextEvent, EventSource, ingestion_buffer()
│   │
│   ├── ctx-db/                   # Database layer
│   │   └── src/lib.rs            # ContextoDb, batch writer, FTS5 search
│   │
│   ├── ctx-cli/                  # CLI binary
│   │   └── src/main.rs           # `ctx` command with clap subcommands
│   │
│   └── ctxd/                     # Daemon binary
│       └── src/main.rs           # REST API, MCP server, auth token
│
├── apps/
│   └── desktop/                  # Tauri desktop app (Phase 7)
│       ├── src/                  # TypeScript/React frontend
│       └── src-tauri/            # Rust Tauri backend
│
├── extensions/
│   └── vscode/                   # VS Code extension (Phase 4)
│
├── docs/
│   ├── DESIGN.md                 # UI design system specification
│   ├── ARCHITECTURE.md           # This file
│   └── SECURITY.md               # Security model & threat vectors
│
└── .githooks/                    # Git hooks (pre-commit, pre-push)
```

---

## Phase Roadmap

### Phase 0 — Pre-Codebase & Living Docs ✅
**Goal:** Safe foundations before any feature code.

| Deliverable | Status |
|---|---|
| `.githooks/pre-commit` (gitleaks, fmt, clippy, TS typecheck, file guard) | ✅ |
| `.githooks/pre-push` (full test suite) | ✅ |
| `.gitignore` (runtime state, secrets, build artifacts) | ✅ |
| `.editorconfig` (cross-language consistency) | ✅ |
| `rust-toolchain.toml` (compiler pinning) | ✅ |
| `Cargo.toml` (workspace with shared deps) | ✅ |
| Crate stubs: `ctx-core`, `ctx-db`, `ctx-cli`, `ctxd` | ✅ |
| `docs/DESIGN.md` (Linear/Raycast design system) | ✅ |
| `docs/ARCHITECTURE.md` | ✅ |
| `docs/SECURITY.md` (auth token architecture) | ✅ |
| `.env.example` | ✅ |

---

### Phase 1 — `ctx-db` + `ctx-core`
**Goal:** Battle-tested data pipeline with SQLite + FTS5.

**`ctx-core` additions:**
- Event validation (content length guards, source enum exhaustiveness)
- `EventFilter` struct for query parameters

**`ctx-db` implementation:**
- `sqlx::SqlitePool` with WAL mode + `PRAGMA journal_mode=WAL`
- Schema migrations via `sqlx::migrate!()`
- FTS5 virtual table for BM25 full-text search across `label` + `content`
- `BatchWriter` task: drains ring buffer → batched `INSERT` transactions

```sql
-- Core events table
CREATE TABLE IF NOT EXISTS context_events (
    id          TEXT PRIMARY KEY,
    timestamp   TEXT NOT NULL,
    source      TEXT NOT NULL,
    label       TEXT NOT NULL,
    content     TEXT NOT NULL,
    metadata    TEXT,
    was_redacted INTEGER NOT NULL DEFAULT 0,
    cwd         TEXT,
    git_repo    TEXT,
    task_id     TEXT
);

-- FTS5 virtual table (BM25 search, zero extra dependencies)
CREATE VIRTUAL TABLE IF NOT EXISTS events_fts USING fts5(
    label, content,
    content=context_events,
    content_rowid=rowid
);
```

**Batch write pattern (prevents SQLite BUSY errors):**
```rust
// Drain ring buffer on 500ms timer OR 50-event threshold
loop {
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_millis(500)) => flush(&pool, &mut batch).await,
        Some(event) = rx.recv() => {
            batch.push(event);
            if batch.len() >= 50 { flush(&pool, &mut batch).await; }
        }
    }
}
```

---

### Phase 2 — `ctxd` Daemon + MCP Server ✅
**Goal:** REST API live + MCP dogfooding begins immediately.

> **Why MCP in Phase 2?** Once `ctxd` has a basic MCP interface, your AI coding
> agent (Claude, Cursor, Antigravity) can connect to it **while you write the
> rest of Contexto**. The AI assistant dogfoods the Context Manager to understand
> the repository it is building — a powerful velocity multiplier.

**REST API endpoints:**
```
POST /events           # Ingest a ContextEvent (returns 202 Accepted)
GET  /context          # Retrieve recent events (with filters)
GET  /search?q=...     # FTS5 search
POST /remember         # Manual annotation
GET  /status           # Daemon health, token budget, event count
GET  /tasks            # List tasks
POST /tasks            # Create task
PATCH /tasks/:id       # Update/stop task
```

**Auth middleware (axum):**
```rust
async fn auth_middleware(
    State(token): State<Arc<String>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let header = req.headers().get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match header {
        Some(t) if t == token.as_str() => Ok(next.run(req).await),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}
```

**MCP tools (stdio transport):**
- `get_context` — Return recent events as formatted context
- `search_context` — FTS5 search with optional filters
- `remember` — Store a manual annotation
- `get_task` — Return current task summary

---

### Phase 3 — `ctx-cli` ✅
**Goal:** Full terminal workflow without the desktop app.

Commands: `ctx status`, `ctx context`, `ctx remember <note>`, `ctx search <query>`, `ctx task start/stop/list`

All commands send HTTP requests to `ctxd` with the auth token from `~/.ctx/auth_token`.

---

### Phase 4 — VS Code Extension ✅
**Goal:** Capture editor events automatically.

- TypeScript client wrapping the `ctxd` REST API (`extensions/vscode/src/ctxdClient.ts`) with zero runtime dependencies, token rotation handling, SSE streaming, and health polling
- Event hooks: `onDidOpenTextDocument`, `onDidSaveTextDocument`, `onDidChangeTextEditorSelection` (debounced), and `onDidChangeActiveTextEditor`
- Sidebar webview panel (`extensions/vscode/src/sidebarProvider.ts`) with full `DESIGN.md` tokens (Linear/Raycast dark theme, live event feed, search, relative timestamps)
- Status bar item with live daemon connectivity, event counters, and one-click capture toggle
- Commands: `contexto.toggleCapture`, `contexto.search`, `contexto.remember`, `contexto.showStatus`

---

### Phase 5 — Terminal Hook + Redaction Engine
**Goal:** Capture all shell activity with secret scrubbing.

- Shell wrapper script (`~/.zshrc` / `~/.bashrc` hook) using `precmd` / `preexec`
- Redaction engine: regex patterns for common secret formats (AWS keys, GitHub tokens, `.env` values)
- Pattern: capture → redact → ingest (redacted content never reaches SQLite)

---

### Phase 6 — Local Embeddings / Vector Search
**Goal:** Semantic similarity search without cloud APIs.

**Vector store choice:**
- ❌ `sqlite-vss` — Relies on FAISS C++ bindings; cross-platform compilation in Tauri is unreliable
- ✅ **LanceDB embedded** — Pure Rust, columnar storage, excellent macOS support
- ✅ **`fastembed-rs`** — Pre-built ONNX embeddings, no Python, no C++ compilation

**File content summarization (Dual-Tier rule):**
- Files **< 50KB**: Store AST/symbol outline via `tree-sitter` + git diff chunk. Zero AI calls, instantaneous.
- Files **≥ 50KB** or **task completion**: Run LLM semantic summary (Layer 3 only).

---

### Phase 7 — Tauri Desktop App
**Goal:** Visual cockpit with Linear-grade design quality.

- Full `DESIGN.md` implementation: obsidian canvas, Linear-indigo accent, Inter + JetBrains Mono
- Two-pane layout: Active Tasks + Layer Hierarchy (left) | Live Context Feed + Semantic Query (right)
- Command Palette (`⌘K`) built on top of `ctxd`'s `/search` endpoint
- Real-time event stream via SSE (Server-Sent Events) from `ctxd`

---

### Phase 8 — Hardening & Benchmarking
**Goal:** Production-ready reliability.

- Stress test: simulate 200 events/sec for 60 seconds, verify zero dropped events and zero SQLite BUSY errors
- Memory profiling: `ctxd` should stay under 50MB RSS at steady state
- Response time targets: `/events` ingest < 5ms p99, `/search` < 20ms p99
- Cross-platform CI: GitHub Actions matrix for `x86_64-apple-darwin` and `aarch64-apple-darwin`

---

## Key Architectural Decisions

### 1. Ring Buffer (Prevents SQLite Lock Contention)

An IDE or terminal hook can fire 50–200 events/second. Without buffering, every event triggers an immediate `INSERT INTO context_events`, causing `SQLITE_BUSY` errors and freezing the daemon.

**Solution:** `tokio::sync::mpsc::channel(1000)` sits in front of `ctx-db`. The REST endpoint returns HTTP 202 immediately after queuing the event. A background task batches writes every 500ms or 50 events, using a single SQLite transaction.

### 2. FTS5 Before Vectors

SQLite FTS5 is built into SQLite — zero extra dependencies, sub-millisecond BM25 search. This covers Phases 1–5 completely. Vector/semantic search is a Phase 6 enhancement, not a Phase 1 requirement.

### 3. MCP in Phase 2 (Dogfood Loop)

Moving MCP from Phase 6 to Phase 2 enables AI agents to query their own project context while building Contexto. This "dogfood loop" creates a positive feedback cycle that accelerates all subsequent phases.

### 4. tree-sitter for File Snapshots (Not LLM)

For files under 50KB, tree-sitter AST symbol extraction is instantaneous, deterministic, and costs zero API calls. LLM summarization is reserved for project-level Layer 3 summaries and task completion events.
