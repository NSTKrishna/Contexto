//! # auth
//!
//! Auth token management for ctxd.
//!
//! ## Security Model
//!
//! On startup, ctxd generates a 32-byte cryptographically secure random token,
//! writes it to `~/.ctx/auth_token` with permissions 0600 (owner read-only),
//! and keeps it in memory as an `Arc<String>`.
//!
//! Every inbound HTTP request MUST include:
//! ```text
//! Authorization: Bearer <token>
//! ```
//!
//! This prevents DNS-rebinding attacks: a malicious web page cannot call the
//! daemon via `fetch()` on the loopback interface without knowing the token,
//! which is only readable by the owning OS user.
//!
//! The token file is regenerated on every daemon start. Clients (CLI, VS Code
//! extension) read `~/.ctx/auth_token` to obtain the current token.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

// =============================================================================
// Token Paths
// =============================================================================

/// Returns the canonical path for the auth token file: `~/.ctx/auth_token`.
pub fn auth_token_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".ctx").join("auth_token")
}

/// Returns the canonical path for the database file: `~/.ctx/ctx.db`.
pub fn db_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".ctx").join("ctx.db")
}

// =============================================================================
// Token Initialization
// =============================================================================

/// Generate a cryptographically secure 32-byte random auth token,
/// write it to `~/.ctx/auth_token` with mode 0600, and return the token string.
///
/// The token is URL-safe base64-encoded (no padding), giving ~43 printable chars.
///
/// # Errors
/// Returns an error if the file cannot be written or permissions cannot be set.
pub fn init_auth_token() -> Result<String> {
    let token_path = auth_token_path();
    let ctx_dir = token_path
        .parent()
        .expect("auth_token_path must have a parent directory")
        .to_path_buf();

    // Create ~/.ctx/ if it doesn't exist yet
    fs::create_dir_all(&ctx_dir)?;

    // Generate 32 bytes of cryptographically secure randomness
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let token = URL_SAFE_NO_PAD.encode(bytes);

    // Write token to disk
    fs::write(&token_path, &token)?;

    // Lock down to owner-only read/write (0600) — the critical security property.
    // Non-Unix targets (Windows dev) skip this silently.
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&token_path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&token_path, perms)?;
    }

    tracing::info!(
        path = %token_path.display(),
        "Auth token written (mode 0600)"
    );

    Ok(token)
}

// =============================================================================
// Axum Auth Middleware
// =============================================================================

use axum::extract::Request;
use axum::{body::Body, extract::State, http::StatusCode, middleware::Next, response::Response};

/// Shared daemon state injected into every axum handler.
///
/// This is a subset of `AppState` — just enough for the auth layer.
/// The full [`crate::api::AppState`] embeds `Arc<String>` as the token.
#[derive(Clone)]
pub struct AuthToken(pub Arc<String>);

/// Axum middleware that validates `Authorization: Bearer <token>` on every request.
///
/// Returns `401 Unauthorized` for:
/// - Missing `Authorization` header
/// - Wrong scheme (not `Bearer`)
/// - Token mismatch
///
/// The comparison is done with a simple `==` — constant-time comparison is not
/// required here because the token is 256 bits of entropy (brute-force infeasible).
pub async fn require_auth(
    State(AuthToken(token)): State<AuthToken>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let provided = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match provided {
        Some(t) if t == token.as_str() => Ok(next.run(req).await),
        _ => {
            tracing::warn!(
                method = %req.method(),
                uri = %req.uri(),
                "Rejected unauthorized request"
            );
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_token_path_contains_ctx() {
        let path = auth_token_path();
        assert!(path.to_string_lossy().contains(".ctx/auth_token"));
    }

    #[test]
    fn test_db_path_contains_ctx() {
        let path = db_path();
        assert!(path.to_string_lossy().contains(".ctx/ctx.db"));
    }

    #[test]
    fn test_init_auth_token_in_tmp() {
        let tmp = tempfile::tempdir().expect("temp dir");
        std::env::set_var("HOME", tmp.path().to_str().unwrap());

        let result = init_auth_token();
        assert!(result.is_ok(), "should succeed: {result:?}");

        let token = result.unwrap();
        assert!(!token.is_empty());
        // 32 bytes → 43 base64url chars
        assert!(token.len() >= 42, "token too short: {} chars", token.len());

        // File must exist and be readable
        let path = auth_token_path();
        assert!(path.exists(), "auth_token file should exist");

        #[cfg(unix)]
        {
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "must be 0600");
        }
    }
}
