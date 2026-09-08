# Contexto — GitHub Copilot Agent Instructions
#
# This file is read by GitHub Copilot Coding Agent, GitHub Copilot Chat,
# and the GitHub Copilot workspace feature to understand this repository.
# Reference: https://docs.github.com/en/copilot/customizing-copilot/adding-repository-instructions-for-github-copilot

---

## Project Identity

**Contexto** is a Universal Developer Context Manager — a locally-running daemon (`ctxd`) that silently captures terminal output, git operations, file saves, and editor activity into a SQLite database, then exposes it via REST API and MCP (Model Context Protocol) so AI coding agents can query their own project context.

**Repository:** `NSTKrishna/Contexto`  
**Language split:** ~80% Rust, ~20% TypeScript (VS Code extension, Phase 4+)  
**Design system:** `docs/DESIGN.md` — "Linear/Raycast Developer Dark"

---

## Architecture at a Glance

```
Event Sources (IDE, Terminal, Git)
        │  HTTP POST /events  +  Bearer token
        ▼
   ctxd daemon  (REST API + MCP stdio server)
        │
        ▼  tokio::mpsc ring buffer (1000 capacity)
   BatchWriter  →  SQLite WAL + FTS5  →  ContextoDb
        │
        ├── ctx-cli  (ctx status / search / remember / task)
        ├── VS Code extension  (Phase 4)
        └── Tauri desktop app  (Phase 7, DESIGN.md)
```

**Auth:** Every request to `ctxd` must include `Authorization: Bearer <token>` read from `~/.ctx/auth_token` (mode 0600, generated on daemon startup). This prevents DNS rebinding attacks. See `docs/SECURITY.md`.

---

## Crate Map

| Crate | Path | Purpose |
|-------|------|---------|
| `ctx-core` | `crates/ctx-core/` | `ContextEvent`, `EventSource`, `EventFilter`, `Task`, ring buffer |
| `ctx-db` | `crates/ctx-db/` | SQLite pool, 3 migrations, `BatchWriter`, FTS5 search |
| `ctx-cli` | `crates/ctx-cli/` | `ctx` binary — clap subcommands |
| `ctxd` | `crates/ctxd/` | Daemon — auth token, REST API (Phase 2), MCP (Phase 2) |

---

## Coding Rules for Copilot

### Rust
- **Edition:** 2021. Workspace resolver: `"2"`.
- **Toolchain:** Pinned to `1.82.0` via `rust-toolchain.toml`. Never suggest `nightly` features.
- **`unsafe_code = "forbid"`** — workspace lint. Never write unsafe blocks.
- **`clippy::all + pedantic`** — all clippy lints enabled as warnings. Code must be clippy-clean.
- Use `anyhow::Result` for error propagation in binary crates (`ctxd`, `ctx-cli`).
- Use `thiserror::Error` for typed errors in library crates (`ctx-core`, `ctx-db`).
- Prefer `tokio::sync::mpsc` over `std::sync` channels — we are async-first.
- Use `tracing::info!` / `debug!` / `warn!` / `error!` for all logging. No `println!` in library code.
- All public API items must have doc comments (`///`).

### Database (ctx-db)
- **Never** write a raw `INSERT` per event in hot paths — always use `BatchWriter`.
- **Never** use `sqlite-vss` — it requires FAISS C++ bindings that break Tauri cross-compilation. Use FTS5 for Phase 1–5; LanceDB for Phase 6+.
- Use `sqlx::query()` (not `query!()`) until `cargo sqlx prepare` is set up in CI.
- All schema changes go in `crates/ctx-db/migrations/` as numbered SQL files.
- Always enable WAL mode and set `PRAGMA foreign_keys=ON`.

### Security
- **Always** check the `Authorization: Bearer <token>` header on every `ctxd` HTTP endpoint.
- **Never** bind `ctxd` to `0.0.0.0` — only `127.0.0.1`.
- **Never** log or print the auth token value.
- Use constant-time byte comparison (not `==`) for token validation to prevent timing attacks.
- Run the redaction engine **before** storing any terminal event content in SQLite.

### Design (UI components for Tauri/VS Code)
- Follow `docs/DESIGN.md` exactly. Use only semantic CSS variables, never hardcoded hex.
- Primary font: `Inter` with `font-feature-settings: 'ss03'`. Mono: `JetBrains Mono`.
- Transitions: `≤ 120ms ease-out`. No slow animations.
- Spacing: multiples of 4px only.
- When building a new UI component, include this in your prompt:
  > "Use the 'Linear/Raycast Developer Dark' design system from docs/DESIGN.md.
  > Semantic variables: --bg-surface, --text-primary, --border-subtle, --status-accent."

---

## Phase Status

| Phase | Status | Branch |
|-------|--------|--------|
| 0 — Pre-codebase setup | ✅ Merged | `main` |
| 1 — ctx-db + ctx-core | ✅ Merged | `main` |
| 2 — ctxd REST + MCP | ✅ Merged | `main` |
| 3 — ctx-cli | ✅ Merged | `main` |
| 4 — VS Code extension | ⏳ Next | `phase/4-vscode-extension` |
| 5 — Terminal hook + redaction | 🔜 | — |
| 6 — Vector search | 🔜 | — |
| 7 — Tauri desktop | 🔜 | — |
| 8 — Hardening | 🔜 | — |

---

## What Copilot Should NOT Do

- ❌ Do not suggest `sqlite-vss`, `hnswlib`, or any C++ vector library
- ❌ Do not bind the HTTP server to `0.0.0.0`
- ❌ Do not write per-event SQLite INSERTs in hot ingestion paths
- ❌ Do not use `unsafe {}` blocks
- ❌ Do not use `println!` in library crates — use `tracing`
- ❌ Do not hardcode hex colors in UI — use CSS variables from `docs/DESIGN.md`
- ❌ Do not suggest `nightly` Rust features
- ❌ Do not suggest adding new workspace dependencies without updating `Cargo.toml [workspace.dependencies]`

---

## Commit Message Format

```
type(scope): short description

Body with details (if needed).
```

Types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `perf`  
Scopes: `ctx-core`, `ctx-db`, `ctxd`, `ctx-cli`, `desktop`, `vscode`, `ci`, `docs`, `security`

Examples:
- `feat(ctx-db): add FTS5 porter stemmer tokenizer`
- `fix(ctxd): use constant-time token comparison`
- `test(ctx-db): add batch writer integration test`

---

## Running the Project

```bash
# Configure git hooks (required once)
git config core.hooksPath .githooks

# Check workspace compiles
cargo check --workspace

# Run all tests (excluding Tauri desktop)
cargo test --workspace --exclude ctx-desktop

# Run a specific crate's tests
cargo test -p ctx-db

# Start the daemon (Phase 2+)
cargo run -p ctxd

# Use the CLI (Phase 3+)
cargo run -p ctx-cli -- status
```

---

## Key Files to Read Before Making Changes

| File | Why |
|------|-----|
| `docs/ARCHITECTURE.md` | Phase roadmap, system diagram, key decisions |
| `docs/SECURITY.md` | Auth model, threat vectors, redaction design |
| `docs/DESIGN.md` | UI design system — read before any frontend work |
| `crates/ctx-core/src/lib.rs` | Canonical data model — change carefully |
| `crates/ctx-db/src/lib.rs` | Database API — batch writer contract |
| `Cargo.toml` | Workspace deps — add deps here, not in crate manifests |
