# SECURITY.md — Contexto Daemon Security Model

This document describes the security architecture of the `ctxd` daemon, the primary
threat vectors for a locally-running developer context manager, and the mitigations
implemented.

> [!CAUTION]
> Contexto captures terminal output, file contents, git diffs, and editor activity.
> This data is extremely sensitive — it can contain API keys, database credentials,
> private code, and personal information. Security is not optional.

---

## Threat Model

### Threat 1: DNS Rebinding / Direct Loopback Attack

**Severity: Critical**

If `ctxd` binds an HTTP server on `127.0.0.1` without authentication, **any malicious
webpage** open in your browser can access it:

```javascript
// Attacker's webpage — steals your entire developer context
fetch('http://127.0.0.1:8942/context')
  .then(r => r.json())
  .then(data => exfiltrate(data)); // All your terminal output, file contents, git diffs
```

This works via:
1. **Direct loopback access**: The browser can fetch `127.0.0.1` directly
2. **DNS rebinding**: Attacker's DNS resolves their domain to `127.0.0.1` after the page loads

**Mitigation: Bearer Token Authentication (implemented in Phase 0)**

---

## Auth Token Architecture

### Token Generation (on daemon startup)

```rust
// ctxd generates 32 bytes of cryptographically secure randomness
let mut bytes = [0u8; 32];
rand::thread_rng().fill_bytes(&mut bytes);
let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
```

### Token Storage

The token is written to `~/.ctx/auth_token` with **mode 0600** (owner read/write only):

```rust
fs::write(&token_path, &token)?;
let mut perms = fs::metadata(&token_path)?.permissions();
perms.set_mode(0o600);
fs::set_permissions(&token_path, perms)?;
```

**Result:**
- Only the owning user can read the token
- Other processes, web browsers, and other users cannot access it
- A new token is generated on each daemon startup (rotation on restart)

### Token Usage by Clients

All clients (CLI, VS Code extension, Tauri app) must:

1. Read the token from `~/.ctx/auth_token`
2. Include it in every request:
   ```
   Authorization: Bearer <token>
   ```
3. Handle HTTP 401 gracefully (daemon restarted, token rotated)

### Server-Side Enforcement

The `ctxd` axum HTTP server uses a middleware layer:

```rust
async fn auth_middleware(
    State(token): State<Arc<String>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let provided = req.headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match provided {
        // Constant-time comparison prevents timing attacks
        Some(t) if constant_time_eq(t.as_bytes(), token.as_bytes()) => {
            Ok(next.run(req).await)
        }
        _ => {
            tracing::warn!("Rejected unauthenticated request from {:?}",
                req.headers().get("host"));
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}
```

> [!IMPORTANT]
> Use constant-time byte comparison (`subtle::ConstantTimeEq` or equivalent)
> to prevent timing side-channel attacks on the token comparison.

---

### Threat 2: Secret Leakage in Captured Content

**Severity: High**

When `ctxd` captures terminal output, it may capture:
- `echo $AWS_SECRET_ACCESS_KEY`
- `curl -H "Authorization: Bearer ghp_xxxx" https://api.github.com`
- Paste from a `.env` file

**Mitigation: Redaction Engine (Phase 5)**

Before storing any terminal event content, the redaction engine scrubs:

```rust
// Pattern library (Phase 5 implementation)
const REDACT_PATTERNS: &[(&str, &str)] = &[
    (r"ghp_[A-Za-z0-9]{36}", "[REDACTED:GITHUB_TOKEN]"),
    (r"AKIA[0-9A-Z]{16}", "[REDACTED:AWS_ACCESS_KEY]"),
    (r"sk-[A-Za-z0-9]{48}", "[REDACTED:OPENAI_KEY]"),
    (r"sk-ant-[A-Za-z0-9\-]{40,}", "[REDACTED:ANTHROPIC_KEY]"),
    (r"-----BEGIN [A-Z]+ PRIVATE KEY-----", "[REDACTED:PRIVATE_KEY]"),
];
```

Events where content was modified set `was_redacted: true`.

The UI displays a `[REDACTED KEY]` badge on these events (see `docs/DESIGN.md §4.1`).

---

### Threat 3: Committed Runtime State Files

**Severity: High**

Accidentally committing `~/.ctx/contexto.db` or `.ctx/auth_token` would leak:
- All captured developer context (potentially months of terminal history)
- The daemon auth token (active until daemon restart)

**Mitigation: Pre-commit hook** (`.githooks/pre-commit`)

The pre-commit hook hard-blocks any commit containing `.db`, `.sqlite`, `.sock`, `.pid`, or `.log` files.

**Mitigation: `.gitignore`**

Comprehensive ignore rules cover all runtime state directories and file extensions.

---

### Threat 4: Over-Privileged Daemon Process

**Mitigation:**
- `ctxd` runs as the current user — no `sudo`, no elevated privileges
- Binds only to `127.0.0.1` (loopback) — never `0.0.0.0`
- Workspace-level Rust lint: `unsafe_code = "forbid"` in `Cargo.toml`

---

## Security Checklist

Before shipping any phase, verify:

- [ ] `ctxd` listens only on `127.0.0.1`, not `0.0.0.0`
- [ ] Every HTTP endpoint is behind the auth middleware
- [ ] `~/.ctx/auth_token` is created with mode `0600`
- [ ] Token comparison uses constant-time equality
- [ ] Pre-commit hook is installed (`git config core.hooksPath .githooks`)
- [ ] `.gitignore` covers all runtime state patterns
- [ ] `CTX_DISABLE_AUTH=true` is never set in non-development environments
- [ ] Redaction engine is enabled before Phase 5 (terminal hook)
- [ ] `gitleaks` is installed and pre-commit secret scan is active

---

## Development Override

The `.env.example` includes `CTX_DISABLE_AUTH=false`. Setting this to `true` bypasses
the auth check for local development convenience.

> [!CAUTION]
> **NEVER set `CTX_DISABLE_AUTH=true` in any environment where the machine is
> accessible via network or shares a browser profile with untrusted websites.**
