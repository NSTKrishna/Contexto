//! # ctxd — Contexto Daemon
//!
//! The central daemon for the Contexto Universal Developer Context Manager.
//!
//! ## Responsibilities
//! - Serves a REST API for the CLI, VS Code extension, and Tauri desktop app
//! - Serves an MCP (Model Context Protocol) stdio server for AI agent integration
//! - Manages the ingestion ring buffer → SQLite batch writer pipeline
//! - Generates and guards the auth token (`~/.ctx/auth_token`, mode 0600)
//!
//! ## Security: DNS Rebinding / CSRF Guard
//!
//! On startup, ctxd generates a cryptographically secure random token and
//! writes it to `~/.ctx/auth_token` with permissions 0600 (user-read-only).
//!
//! Every inbound HTTP request MUST include:
//! ```text
//! Authorization: Bearer <token>
//! ```
//! Requests without this header are rejected with HTTP 401.
//!
//! This prevents malicious webpages from calling the daemon via `fetch()` on
//! the loopback address (DNS rebinding / direct loopback attack vector).
//!
//! ## MCP Integration (Phase 2)
//! Moving MCP to Phase 2 allows AI coding agents (Claude, Cursor, Antigravity)
//! to query their own project context while building the rest of Contexto.
//! This "dogfooding" loop dramatically accelerates development velocity.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use anyhow::Result;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;

// =============================================================================
// Auth Token
// =============================================================================

/// Generate a cryptographically secure 32-byte random auth token,
/// write it to `~/.ctx/auth_token` with mode 0600, and return the token string.
///
/// # Errors
/// Returns an error if the token cannot be written to disk.
pub fn init_auth_token() -> Result<String> {
    let ctx_dir = auth_token_path()
        .parent()
        .expect("auth_token_path must have a parent")
        .to_path_buf();

    // Create ~/.ctx/ if it doesn't exist
    fs::create_dir_all(&ctx_dir)?;

    // Generate 32 bytes of cryptographically secure randomness
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token = URL_SAFE_NO_PAD.encode(bytes);

    let token_path = auth_token_path();
    fs::write(&token_path, &token)?;

    // Set permissions to 0600 (owner read/write only)
    let mut perms = fs::metadata(&token_path)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(&token_path, perms)?;

    tracing::info!(
        "Auth token written to {} (mode 0600)",
        token_path.display()
    );

    Ok(token)
}

/// Returns the canonical path for the auth token file.
fn auth_token_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".ctx").join("auth_token")
}

// =============================================================================
// Entry Point
// =============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize structured logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ctxd=info,ctx_core=info,ctx_db=info".into()),
        )
        .with_target(true)
        .init();

    // Load .env if present
    let _ = dotenvy::dotenv();

    tracing::info!("==============================================");
    tracing::info!("  ctxd — Contexto Daemon v{}", env!("CARGO_PKG_VERSION"));
    tracing::info!("==============================================");

    // --- Security: Initialize auth token ---
    let _auth_token = init_auth_token()?;
    tracing::info!("✅ Auth token initialized (all requests require Bearer token)");

    // --- Phase 1: Initialize database ---
    tracing::info!("⏳ ctx-db: Phase 1 — database not yet implemented");

    // --- Phase 2: Start REST API + MCP server ---
    tracing::info!("⏳ REST API + MCP server: Phase 2 — not yet implemented");

    tracing::info!("");
    tracing::info!("ctxd Phase 0 stub started successfully.");
    tracing::info!("Next: Implement ctx-db (Phase 1), then ctxd REST + MCP (Phase 2).");
    tracing::info!("");

    // Block indefinitely (Phase 2 will replace this with axum server loop)
    tokio::signal::ctrl_c().await?;
    tracing::info!("Received Ctrl+C — ctxd shutting down gracefully.");

    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_token_path() {
        let path = auth_token_path();
        assert!(path.to_string_lossy().contains(".ctx/auth_token"));
    }

    #[tokio::test]
    async fn test_auth_token_generation() {
        // Use a temp dir to avoid stomping on real auth token during tests
        let tmp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("HOME", tmp.path().to_str().unwrap());

        let result = init_auth_token();
        assert!(result.is_ok(), "auth token init should succeed: {:?}", result);

        let token = result.unwrap();
        assert!(!token.is_empty(), "token must not be empty");
        assert!(token.len() >= 40, "token must be at least 40 chars (32 bytes base64url)");

        // Verify file permissions
        let path = auth_token_path();
        let meta = std::fs::metadata(&path).expect("auth_token file must exist");
        let mode = meta.permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "auth_token must be mode 0600");
    }
}
