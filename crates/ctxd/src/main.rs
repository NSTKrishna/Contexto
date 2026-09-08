//! # ctxd — Contexto Daemon
//!
//! The central daemon for the Contexto Universal Developer Context Manager.
//!
//! ## Startup Sequence
//!
//! 1. Initialize structured tracing (JSON logs in production, pretty in dev)
//! 2. Load `.env` if present
//! 3. Generate / refresh auth token → `~/.ctx/auth_token` (mode 0600)
//! 4. Open SQLite database → `~/.ctx/ctx.db` (WAL mode, FTS5)
//! 5. Create ingestion ring buffer (1,000 event capacity)
//! 6. Spawn `BatchWriter` (drains ring buffer → SQLite every 500ms or 50 events)
//! 7. Spawn MCP stdio server (JSON-RPC 2.0 over stdin/stdout)
//! 8. Bind axum REST API on `127.0.0.1:8942`
//! 9. Await Ctrl+C → graceful shutdown
//!
//! ## Security
//!
//! All REST endpoints require `Authorization: Bearer <token>`.
//! The token is generated fresh on every daemon start and stored at
//! `~/.ctx/auth_token` (0600). Clients read this file to authenticate.
//!
//! ## MCP Dogfood Loop
//!
//! With the MCP server active from Phase 2, the AI agent building Contexto
//! can call `get_context` / `search_context` to track its own progress — a
//! positive feedback loop that accelerates all subsequent phases.

use std::sync::Arc;

use anyhow::Result;
use ctx_core::ingestion_buffer;
use ctx_db::{BatchWriter, ContextoDb};
use tokio::sync::broadcast;

mod api;
mod auth;
mod mcp;

// =============================================================================
// Entry Point
// =============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    // ── 1. Tracing ────────────────────────────────────────────────────────────
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ctxd=info,ctx_core=info,ctx_db=info".into()),
        )
        .with_target(true)
        .init();

    // ── 2. .env ───────────────────────────────────────────────────────────────
    let _ = dotenvy::dotenv();

    tracing::info!("==============================================");
    tracing::info!("  ctxd — Contexto Daemon v{}", env!("CARGO_PKG_VERSION"));
    tracing::info!("==============================================");

    // ── 3. Auth token ─────────────────────────────────────────────────────────
    let token_str = auth::init_auth_token()?;
    let token = Arc::new(token_str);
    tracing::info!(
        "✅ Auth token ready  ({})",
        auth::auth_token_path().display()
    );

    // ── 4. Database ───────────────────────────────────────────────────────────
    let db_path = auth::db_path();
    let db_path_str = db_path.to_string_lossy().to_string();
    let db = Arc::new(ContextoDb::open(&db_path_str).await?);
    tracing::info!("✅ Database ready    ({db_path_str})");

    // ── 5. Ring buffer ────────────────────────────────────────────────────────
    let (tx, rx) = ingestion_buffer();
    let tx = Arc::new(tx);
    tracing::info!(
        "✅ Ring buffer ready (capacity={})",
        ctx_core::RING_BUFFER_CAPACITY
    );

    // ── 6. BatchWriter ────────────────────────────────────────────────────────
    let writer = BatchWriter::new(rx, (*db).clone());
    tokio::spawn(writer.run());
    tracing::info!("✅ BatchWriter spawned (flush_interval=500ms, max_batch=50)");

    // ── 7. MCP stdio server ───────────────────────────────────────────────────
    let mcp_db = db.clone();
    tokio::spawn(mcp::run_stdio_server(mcp_db));
    tracing::info!("✅ MCP stdio server spawned (JSON-RPC 2.0 on stdin/stdout)");

    // ── 8. SSE broadcast channel ──────────────────────────────────────────────
    // Capacity 256: at 50 events/sec, this gives ~5s of lag tolerance.
    let (sse_tx, _) = broadcast::channel::<ctx_core::ContextEvent>(256);

    // ── 9. Axum REST API ──────────────────────────────────────────────────────
    let state = api::AppState {
        db,
        tx,
        token: token.clone(),
        started_at: chrono::Utc::now(),
        sse_tx,
    };

    let router = api::build_router(state);
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 8942));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!("✅ REST API listening on http://{addr}");
    tracing::info!("");
    tracing::info!("  TOKEN:  cat ~/.ctx/auth_token");
    tracing::info!("  HEALTH: curl -H 'Authorization: Bearer ...' http://127.0.0.1:8942/status");
    tracing::info!("");

    // Serve until Ctrl+C
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("ctxd shut down gracefully. Goodbye.");
    Ok(())
}

// =============================================================================
// Shutdown Signal
// =============================================================================

/// Await either Ctrl+C (Unix: also SIGTERM) for graceful shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {
            tracing::info!("Received Ctrl+C — shutting down...");
        }
        () = terminate => {
            tracing::info!("Received SIGTERM — shutting down...");
        }
    }
}
