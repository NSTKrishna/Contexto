# Contexto — Project Implementation Progress Report

> **Living Document**  
> **Last Updated:** September 12, 2026  
> **Current Version:** `0.1.0`  
> **Repository:** [`NSTKrishna/Contexto`](https://github.com/NSTKrishna/Contexto)  
> **Status:** Phase 0–4 Completed & Verified Live in Production

---

## Executive Summary

Contexto is the invisible flight recorder for software engineering workflows. It silently captures, indexes, and surfaces developer context across terminals, editors, Git, and AI assistants with zero telemetry and local-first persistence.

As of this report, **Phases 0 through 4** and the **Automated OMNI PR Code Review Bot** are fully implemented, battle-tested, merged into `main`, and validated end-to-end in real-world VS Code sessions.

```
┌────────────────────────────────────────────────────────────────────────────┐
│                              Contexto Stack                                │
│                                                                            │
│  ┌──────────────────────┐  ┌───────────────────┐  ┌──────────────────────┐ │
│  │   VS Code Extension  │  │   Terminal CLI    │  │   AI Coding Agents   │ │
│  │  (Phase 4 — Merged)  │  │ (Phase 3 — Merged)│  │  (Claude/Cursor/AGY) │ │
│  └──────────┬───────────┘  └─────────┬─────────┘  └──────────┬───────────┘ │
│             │                        │                       │             │
│             │ HTTP 202 Ingest        │ HTTP + Bearer Token   │ MCP (stdio) │
│             └────────────────────────┼───────────────────────┘             │
│                                      ▼                                     │
│                        ┌───────────────────────────┐                       │
│                        │   ctxd Daemon (Phase 2)   │                       │
│                        │    127.0.0.1:8942 / SSE   │                       │
│                        └─────────────┬─────────────┘                       │
│                                      │                                     │
│                                ┌─────▼─────┐                               │
│                                │Ring Buffer│ (tokio::sync::mpsc 1K)        │
│                                └─────┬─────┘                               │
│                                      │                                     │
│                        ┌─────────────▼─────────────┐                       │
│                        │  BatchWriter (Phase 1)    │                       │
│                        │   500ms / 50 events flush │                       │
│                        └─────────────┬─────────────┘                       │
│                                      │                                     │
│                        ┌─────────────▼─────────────┐                       │
│                        │  SQLite + FTS5 (Phase 1)  │                       │
│                        │    WAL Mode (~/.ctx/)     │                       │
│                        └───────────────────────────┘                       │
└────────────────────────────────────────────────────────────────────────────┘
```

---

## Phase Matrix & Status

| Phase | Component | Description | Status | Verification |
|:---:|---|---|:---:|:---:|
| **0** | **Foundations & Specs** | Git hooks, Rust 2024 workspace, `.editorconfig`, design specs | ✅ Completed | Pre-commit & pre-push hooks active |
| **1** | **`ctx-core` + `ctx-db`** | SQLite + WAL mode, FTS5 BM25 search, ring buffer, BatchWriter | ✅ Completed | 20/20 unit/integration tests pass |
| **2** | **`ctxd` Daemon + MCP** | Axum REST server (127.0.0.1:8942), Bearer auth, MCP stdio | ✅ Completed | 20/20 daemon/MCP tests pass |
| **3** | **`ctx-cli`** | CLI binary (`status`, `context`, `remember`, `search`, `task`) | ✅ Completed | Manual & automated CLI workflows |
| **4** | **VS Code Extension** | Editor event hooks, status bar, Raycast/Linear dark sidebar | ✅ Completed | PR #8 merged, 6/6 TS tests pass |
| **—** | **OMNI Review Bot** | GitHub Actions automated code review on PRs | ✅ Completed | PR #7 merged, live on PR #8 |
| **5** | **Terminal Hook + Redaction** | Shell hook (`.zshrc`/`.bashrc`) capturing commands with scrubbing | 🔜 Planned | Next phase |
| **6** | **Local Vector Search** | Embedded LanceDB + FastEmbed for semantic search | 🔜 Planned | Roadmap |
| **7** | **Tauri Desktop App** | High-density Linear/Raycast dark UI window with hotkey | 🔜 Planned | Roadmap |
| **8** | **Hardening & Packaging** | 200 events/sec stress tests, signed macOS binaries, Homebrew | 🔜 Planned | Roadmap |

---

## Detailed Phase Breakdown

### Phase 0 — Pre-Codebase Setup & Tooling ✅
* **Git Hooks Architecture**:
  * `.githooks/pre-commit`: Runs secret detection, `cargo fmt --check`, `cargo clippy -D warnings`, TypeScript typechecking, and blocks committing runtime database files (`*.db`, `*.sqlite`, `*.sock`, `*.pid`).
  * `.githooks/pre-push`: Executes the full workspace test suite before permitting remote pushes.
* **Workspace Configuration**:
  * Root `Cargo.toml` orchestrating virtual workspace crates (`ctx-core`, `ctx-db`, `ctxd`, `ctx-cli`).
  * `rust-toolchain.toml` pinned to stable `1.85+` supporting Edition 2024.
  * Comprehensive `.gitignore` protecting secrets, SQLite runtime files, tokens, and build artifacts.
* **Living Documentation**:
  * `docs/DESIGN.md`: Complete design token system based on Linear and Raycast Developer Dark aesthetics (4px grid, obsidian canvas, semantic badges).
  * `docs/ARCHITECTURE.md`: Deep technical blueprint and phase roadmap.
  * `docs/SECURITY.md`: Security threat models, loopback guarantees, and token storage specifications.

### Phase 1 — `ctx-core` & `ctx-db` ✅
* **Domain Models (`crates/ctx-core`)**:
  * `ContextEvent`: Unified flight-recorder event structure with `id`, `timestamp`, `source`, `label`, `content`, `metadata`, `was_redacted`, `cwd`, `git_repo`, and `task_id`.
  * `EventSource`: Strongly-typed enum supporting `Terminal`, `Editor`, `Git`, `Manual`, and `Mcp`.
  * `Task`: Living work session model tracking active session names, durations, and event correlations.
* **Persistence Layer (`crates/ctx-db`)**:
  * SQLite configured in **WAL (Write-Ahead Logging)** mode for high concurrency.
  * Embedded migrations via `sqlx::migrate!()`.
  * **FTS5 Virtual Table**: Full-text inverted index providing sub-2ms BM25 ranking across all event contents and labels.
  * **Asynchronous `BatchWriter`**: Ingests through an in-memory ring buffer (`tokio::sync::mpsc::channel(1000)`). Returns HTTP 202 in `< 1ms` and flushes batched SQLite writes every 500ms or 50 events to completely prevent `SQLITE_BUSY` lock contention.

### Phase 2 — `ctxd` Daemon + MCP Server ✅
* **Axum REST API (`http://127.0.0.1:8942`)**:
  * `POST /events`: High-throughput ingestion endpoint.
  * `GET /context`: Retrieve recent timeline with source/limit/repo filters.
  * `GET /search?q=...`: Sub-millisecond FTS5 search.
  * `POST /remember`: Permanent note recording.
  * `GET /status`: Daemon uptime, event count, and active task tracking.
  * `POST /tasks/start`, `POST /tasks/stop`, `POST /tasks/abandon`, `GET /tasks`: Session management.
  * `GET /events/stream`: Server-Sent Events (SSE) stream for live reactive updates.
* **Security & Auth Guard**:
  * Binds exclusively to localhost (`127.0.0.1`).
  * Authenticates all REST endpoints with a cryptographically secure 256-bit token stored in `~/.ctx/auth_token` (`0600` permissions).
* **Model Context Protocol (MCP) Server**:
  * Native stdio JSON-RPC 2.0 protocol implemented directly via `ctxd --mcp`.
  * Tools exposed: `get_context`, `search_context`, `remember`, `get_task`.
  * Supports Claude Desktop, Cursor, and Antigravity.

### Phase 3 — `ctx-cli` Terminal Flight Recorder ✅
* **Ergonomic CLI Client (`ctx`)**:
  * Built with `clap` (derive API) and `colored`/`comfy-table` formatting.
  * Reads credentials automatically from `~/.ctx/auth_token`.
  * Supports both human-readable terminal rendering and machine-readable `--format json`.
* **Commands**:
  * `ctx status`: Displays daemon health, uptime, active task, and database path.
  * `ctx context`: Displays recent event stream with color-coded source tags.
  * `ctx search <query>`: Instant BM25 full-text search across history.
  * `ctx remember <note>`: Manually annotates the context flight recorder.
  * `ctx task start|stop|list|abandon`: Manages live coding sessions.

### Phase 4 — Contexto VS Code Extension ✅
* **Core Architecture (`extensions/vscode`)**:
  * **Zero runtime dependencies**: Uses Node.js built-in `http` module.
  * **Security Enforcement**: `isLoopbackUrl` guard restricts connections strictly to `127.0.0.1`, `localhost`, and `::1`. Configuration settings (`contexto.daemonUrl`, `contexto.authTokenPath`) use `"scope": "application"` to prevent credential exfiltration from untrusted workspace `.vscode/settings.json`.
  * **401 Auto-Retry**: Automatically re-reads the token file and retries failed requests once before rejecting.
  * **Self-Healing SSE Stream**: Connects to `GET /events/stream` with resilient backoff reconnection and auto-recovery on daemon restarts.
* **Event Listeners**:
  * `onDidOpenTextDocument`: Captures opened documents and line counts.
  * `onDidSaveTextDocument`: Captures saved files with Git repo awareness.
  * `onDidChangeTextEditorSelection`: Debounced (default 2000ms) cursor and selection tracking.
  * `onDidChangeActiveTextEditor`: Captures active buffer switches.
* **Linear / Raycast Dark Sidebar Webview**:
  * Pure CSS implementation of `docs/DESIGN.md` tokens (obsidian canvas, semantic badge colors, 4px grid spacing, $\le$120ms transitions).
  * Real-time event stream with auto-scrolling and expandable card details.
  * Live active task indicator with elapsed time counter.
  * Keyboard navigation (`j`/`k` to navigate, `Enter` to expand, `/` or `⌘K` to search, `Esc` to clear).
  * Strict Content Security Policy with unique nonces.
* **Status Bar Item**:
  * Bottom-left status widget displaying live connection state (`Connected`, `Connecting...`, `Offline`), event counters, and active task name.
  * Click to view daemon status notification or trigger commands.

### OMNI / DeepWiki Automated PR Review Bot ✅
* Integrated via GitHub Actions (`.github/workflows/omni-pr-review.yml`).
* Evaluates incoming PR diffs against security, correctness, and architecture rules.
* Deduplicates comments using `<!-- omni-pr-review-bot -->` markers.
* Successfully reviewed and verified PR #7 and PR #8.

---

## Test & Quality Metrics

```
Total Test Suite: 46 / 46 Passing (100%)
Clippy Warnings:  0 (-D warnings enforced)
Format Status:    Clean (cargo fmt, prettier)
Security Audits:  Clean (GitGuardian, cargo-audit)
```

### Breakdown:
1. **`ctx-core` Unit Tests (10/10 Passing)**:
   - Event source serialization/deserialization.
   - Event construction and builder patterns.
   - Content truncation rules.
   - Ring buffer multi-producer single-consumer concurrency.
   - Task creation and duration calculation.
2. **`ctx-db` Integration Tests (10/10 Passing)**:
   - In-memory SQLite initialization in WAL mode.
   - Idempotent event insertion.
   - FTS5 BM25 search matching and empty-query safety.
   - BatchWriter buffer draining and 50-event batch transactions.
   - Task persistence and state transitions.
3. **`ctxd` Daemon & MCP Tests (20/20 Passing)**:
   - Bearer token authentication and rejection of invalid/missing tokens.
   - Event ingestion returning HTTP 202 Accepted.
   - Input validation (source enum checks, search parameter validation).
   - MCP stdio handshake, tools listing, and tool executions (`get_context`, `search_context`, `remember`, `get_task`).
4. **VS Code Extension Tests (6/6 Passing)**:
   - Loopback URL security validation (accepts `127.0.0.1`, `localhost`, `::1`; rejects remote hosts).
   - Constructor security guard blocking malicious hosts.
   - Automatic 401 token refresh and request retry.
   - Rejection when 401 persists.
   - SSE stream connection and message parsing.

---

## Live Production Validation

Real-world verification was performed with `ctxd` running on macOS:
* **Uptime Verified**: Daemon running stably for > 2 hours with steady-state RSS memory `< 25MB`.
* **Live Extension Host**: Extension installed and running inside VS Code (`nstkrishna.contexto v0.1.0`).
* **Active Event Stream**: Automatically recorded open/save events from active workspaces (e.g. `package.json` in `notion-todo-app`), manual notes (`ctx remember`), and task durations (`Testing VS Code Extension 2h 45m`).
* **FTS5 Search**: Sub-millisecond search response times directly from the sidebar search box.

---

## Upcoming Roadmap

### Phase 5 — Terminal Hook + Secret Redaction Engine *(Next Up)*
* **Shell Integration Script**:
  * Native Zsh / Bash hooks (`precmd` / `preexec`) installed via `ctx init-shell`.
  * Captures command strings, working directories, execution timestamps, and exit codes.
* **Pre-Ingestion Secret Redaction Engine**:
  * High-speed regex scanner scrubbing API keys, tokens, and credentials (`ghp_`, `AWS_SECRET_ACCESS_KEY`, `.env` key-value pairs, private keys) *before* events reach the SQLite ring buffer.
  * Zero secret leakage guarantee.

### Phase 6 — Local Vector Search & Embeddings
* Embedded **LanceDB** with **FastEmbed-rs** (pure Rust ONNX runtime, zero Python or C++ dependencies).
* Dual-tier summarization: `tree-sitter` AST outline for files < 50KB; semantic embeddings for larger files and task summaries.

### Phase 7 — Tauri Desktop Application
* Native macOS/Linux desktop app with global hotkey (`⌘ + Shift + Space`).
* Full Linear/Raycast Dark cockpit interface with dual-pane layout, real-time SSE event graph, and interactive context timeline.

### Phase 8 — Hardening & Distribution
* Stress testing at 200 events/sec for 60 seconds with 0 dropped events.
* Homebrew tap (`brew install contexto`) and signed macOS universal binaries.
