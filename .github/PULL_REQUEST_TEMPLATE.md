## Description
<!-- What does this PR do? Why? -->


## Type of Change
- [ ] `feat` — New feature
- [ ] `fix` — Bug fix
- [ ] `refactor` — Code refactor (no behavior change)
- [ ] `test` — Test addition or improvement
- [ ] `docs` — Documentation only
- [ ] `chore` — Tooling, CI, dependency update
- [ ] `perf` — Performance improvement
- [ ] `security` — Security fix

## Phase
<!-- Which Contexto phase does this belong to? -->
- [ ] Phase 0 — Pre-codebase setup
- [ ] Phase 1 — ctx-db + ctx-core
- [ ] Phase 2 — ctxd daemon + MCP
- [ ] Phase 3 — ctx-cli
- [ ] Phase 4 — VS Code extension
- [ ] Phase 5 — Terminal hook + redaction
- [ ] Phase 6 — Vector search
- [ ] Phase 7 — Tauri desktop app
- [ ] Phase 8 — Hardening
- [ ] Cross-cutting (CI, docs, security)

## Testing
<!-- How was this tested? -->
- [ ] `cargo test --workspace` passes locally
- [ ] New tests added for new functionality
- [ ] Manually verified behavior

## Security Checklist
<!-- Required for any PR touching ctxd, auth, or data ingestion -->
- [ ] No new endpoints bypass `Authorization: Bearer` middleware
- [ ] `ctxd` still binds only to `127.0.0.1` (not `0.0.0.0`)
- [ ] No new `unsafe {}` blocks
- [ ] Terminal content passes through redaction engine before storage
- [ ] No secrets, tokens, or credentials in diff

## Design Checklist
<!-- Required for any PR touching Tauri UI or VS Code webview -->
- [ ] Uses semantic CSS variables from `docs/DESIGN.md`
- [ ] Transitions are `≤ 120ms ease-out`
- [ ] Spacing is a multiple of 4px
- [ ] Keyboard navigation works (↑↓ / j/k / Enter / Esc)
- [ ] No hardcoded hex colors

## Database Checklist
<!-- Required for any PR touching ctx-db -->
- [ ] New schema changes are in a numbered migration file (`crates/ctx-db/migrations/`)
- [ ] Hot ingestion paths use `BatchWriter`, not per-event INSERT
- [ ] No `sqlite-vss` or FAISS C++ dependencies added

## Related Issues
<!-- Closes #XX or References #XX -->

---
<!-- 
Commit message format: type(scope): description
Scopes: ctx-core | ctx-db | ctxd | ctx-cli | desktop | vscode | ci | docs | security
-->
