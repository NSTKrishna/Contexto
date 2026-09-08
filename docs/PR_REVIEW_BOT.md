# Automated PR Code Review Bot

An automated, production-grade GitHub Actions code-review bot powered by the OMNI/DeepWiki semantic code analysis engine.

Whenever a Pull Request is opened or updated, the bot extracts the unified diff, filters out non-code artifacts, sanitizes against prompt injection, runs deep semantic audits for critical defects, and posts or updates a structured, idempotent review directly on the Pull Request.

---

## 🚀 How It Works

```text
GitHub PR Event (opened, synchronize, reopened)
       │
       ▼
GitHub Actions: .github/workflows/omni-pr-review.yml
  [permissions: contents: read, pull-requests: write]
       │
       ▼
scripts/omni_pr_review.py
       │
       ├── 1. Fetch PR Diff & Metadata (GitHub REST API)
       │      • Skips lockfiles (*.lock, package-lock.json, etc.)
       │      • Skips deleted files & binary files
       │      • Bounded by MAX_DIFF_SIZE, MAX_FILES, MAX_FILE_SIZE
       │
       ├── 2. Render Prompt (prompts/pr_review.md)
       │      • Quarantines diff within <<<UNTRUSTED_PR_DIFF>>>
       │      • Injects Denial-of-Index and Prompt Injection guardrails
       │
       ├── 3. Query OMNI (scripts/deepwiki_query.py)
       │      • Direct API submission to api.devin.ai/ada/query
       │      • Automatic fallback carrier repo for unindexed repos
       │      • Resilient polling with exponential backoff on transient errors
       │      • Strips backend names/links per OPSEC rules
       │
       ├── 4. Parse Findings & Severity
       │      • Classifies CRITICAL, HIGH, MEDIUM, LOW
       │      • Computes Status: PASS (clean), WARN (medium/low), FAIL (crit/high)
       │
       └── 5. Post or Update PR Comment
              • Detects previous bot comment via <!-- omni-pr-review-bot -->
              • Updates existing comment via PATCH (zero comment spam)
```

---

## 🎯 Review Priorities

The reviewer acts as a Principal Systems Engineer focusing strictly on high-impact technical defects:

1. **Correctness & Logic Flaws**: Off-by-one errors, inverted conditionals, unhandled enum branches.
2. **Security Vulnerabilities**: Authentication bypasses, injection vectors, secret leakage.
3. **Data Loss & Corruption**: Unsafe SQLite operations, missing rollbacks, race conditions.
4. **Breaking Changes**: Backward-incompatible API contracts, serialization mismatch.
5. **Concurrency & Async Safety**: Deadlocks, channel saturation, unhandled task cancellation.
6. **Performance & Leaks**: Quadratic algorithms, unbuffered I/O, resource leaks.
7. **Error Handling & Panics**: Unchecked `unwrap()` / `expect()` calls in production paths.
8. **Missing Tests**: New logic paths without accompanying test assertions.

> **Style Noise Suppressed**: The bot does **not** comment on formatting, whitespace, personal naming preferences, or harmless refactorings.

---

## ⚙️ Configuration & Environment Variables

All parameters can be configured via GitHub Actions environment variables or repository variables:

| Variable | Default | Description |
|---|---|---|
| `OMNI_MODEL` | `deep` | Analysis mode: `deep` (thorough agentic audit) or `fast` (quick sanity review) |
| `OMNI_FALLBACK_REPO` | `NSTKrishna/Contexto` | Primary carrier repo automatically routed to when target repo is unindexed |
| `OMNI_BACKUP_CARRIER` | `bitflicker64/Termstory` | Secondary public indexed carrier fallback if primary carrier is unindexed |
| `MAX_DIFF_SIZE` | `65536` (64 KB) | Maximum total bytes of unified diff sent to the reviewer |
| `MAX_FILE_SIZE` | `16384` (16 KB) | Maximum bytes reviewed per individual file diff |
| `MAX_FILES` | `30` | Maximum number of modified files reviewed per PR |
| `REVIEW_TIMEOUT` | `180` (seconds) | Maximum duration to wait for OMNI agent polling |
| `GITHUB_TOKEN` | *Actions Token* | Automatically provided by `${{ secrets.GITHUB_TOKEN }}` |

---

## 🔒 Security & Fork PR Handling

The workflow enforces zero-trust security controls:

1. **Least-Privilege Permissions**:
   ```yaml
   permissions:
     contents: read
     pull-requests: write
   ```
   No `write` permissions to repository contents, no access to code push, and no administrative grants.
2. **Untrusted Code Isolation**:
   The workflow analyzes the PR unified diff purely as text data. It **never** compiles, tests, or executes untrusted code submitted in the Pull Request.
3. **Prompt Injection Quarantine**:
   PR titles, descriptions, and diff contents are quarantined inside explicit delimiter boundaries (`<<<UNTRUSTED_PR_DIFF>>>`). The system prompt explicitly instructs the reviewer to ignore any embedded directives attempting to override instructions.
4. **Fork Pull Requests**:
   Standard `pull_request` event runs in the base repository context without giving fork code access to repository secrets or write permissions.
5. **OPSEC & Sanitization**:
   Responses are scrubbed via `strip_backend_refs()` to remove internal API endpoints, vendor URLs, and wiki links before posting publicly.

---

## 💻 Local Testing & Development

You can run the reviewer locally against a sample diff or any local git diff without calling GitHub API:

### 1. Run Built-In Sample Diff (Fast Verification)
```bash
python3 scripts/omni_pr_review.py --sample-diff --dry-run --mode fast
```

### 2. Review a Local Git Diff
```bash
# Generate diff of uncommitted changes
git diff origin/main..HEAD > /tmp/changes.diff

# Run reviewer locally
python3 scripts/omni_pr_review.py --diff /tmp/changes.diff --dry-run
```

### 3. Run the Automated Test Suite
The project includes a full unit test suite covering diff extraction, truncation, response parsing, and error recovery:
```bash
python3 -m unittest discover -s tests -p "test_*.py" -v
```

---

## 🛑 How to Disable the Bot

If you need to pause or disable automated reviews:
* **Option 1 (Workflow level)**: In `.github/workflows/omni-pr-review.yml`, set:
  ```yaml
  if: false
  ```
* **Option 2 (Draft PRs)**: By default, the workflow automatically skips draft PRs (`if: github.event.pull_request.draft == false`).
