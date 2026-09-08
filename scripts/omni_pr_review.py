#!/usr/bin/env python3
"""
Automated GitHub Pull Request Code Review Bot Engine.

Orchestrates PR diff extraction, intelligent filtering, prompt assembly,
OMNI API code analysis, response parsing, and idempotent PR comment posting.
"""

import argparse
import datetime
import json
import os
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

# Ensure local script directory is on sys.path to import deepwiki_query
SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))

try:
    from deepwiki_query import ask_deepwiki, strip_backend_refs
except ImportError:
    # Inline fallback if deepwiki_query is moved
    from scripts.deepwiki_query import ask_deepwiki, strip_backend_refs  # type: ignore

BOT_COMMENT_MARKER = "<!-- omni-pr-review-bot -->"

# Environment Configurations with production-grade defaults
OMNI_MODEL = os.getenv("OMNI_MODEL", "deep").strip().lower()
if OMNI_MODEL not in ("fast", "deep"):
    OMNI_MODEL = "deep"

OMNI_API_KEY = os.getenv("OMNI_API_KEY", "").strip() or None
MAX_DIFF_SIZE = int(os.getenv("MAX_DIFF_SIZE", "65536"))  # 64 KB
MAX_FILE_SIZE = int(os.getenv("MAX_FILE_SIZE", "16384"))  # 16 KB
MAX_FILES = int(os.getenv("MAX_FILES", "30"))
REVIEW_TIMEOUT = int(os.getenv("REVIEW_TIMEOUT", "180"))  # 3 minutes default

IGNORED_FILE_EXTENSIONS = (
    ".lock",
    ".png",
    ".jpg",
    ".jpeg",
    ".gif",
    ".ico",
    ".svg",
    ".woff",
    ".woff2",
    ".ttf",
    ".eot",
    ".pdf",
    ".zip",
    ".tar",
    ".gz",
)

IGNORED_FILE_NAMES = (
    "cargo.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "poetry.lock",
    "gemfile.lock",
)


# =============================================================================
# Diff Extraction & Filtering
# =============================================================================

def is_ignored_file(file_path: str) -> bool:
    """Check if file should be excluded from review (lockfiles, images, etc.)."""
    lower = file_path.lower()
    base = os.path.basename(lower)
    if base in IGNORED_FILE_NAMES:
        return True
    return any(lower.endswith(ext) for ext in IGNORED_FILE_EXTENSIONS)


def parse_and_filter_diff(
    raw_diff: str,
    max_total_bytes: int = MAX_DIFF_SIZE,
    max_file_bytes: int = MAX_FILE_SIZE,
    max_files: int = MAX_FILES,
) -> Tuple[str, List[str], bool, Optional[str]]:
    """
    Filter out ignored, binary, or deleted files and enforce size limits.

    Returns:
        (filtered_diff, list_of_kept_files, was_truncated, truncation_warning)
    """
    if not raw_diff or not raw_diff.strip():
        return "", [], False, None

    # Split into per-file diff blocks
    # Standard git diff format starts each file with "diff --git a/..."
    raw_blocks = re.split(r"(?=^diff --git )", raw_diff, flags=re.MULTILINE)
    
    kept_blocks: List[str] = []
    kept_files: List[str] = []
    was_truncated = False
    warning_reasons: List[str] = []

    files_processed = 0

    for block in raw_blocks:
        block = block.strip()
        if not block:
            continue

        # Extract file path
        # Header example: diff --git a/path/to/file b/path/to/file
        match = re.search(r"^diff --git a/(.*?) b/(.*?)$", block, re.MULTILINE)
        if not match:
            continue

        file_path = match.group(2)

        # Skip deleted files
        if "deleted file mode" in block:
            continue

        # Skip binary files
        if "Binary files " in block and "differ" in block:
            continue

        # Skip ignored lockfiles and media
        if is_ignored_file(file_path):
            continue

        files_processed += 1
        if files_processed > max_files:
            was_truncated = True
            warning_reasons.append(f"Reached maximum file limit ({max_files} files). Remaining files omitted.")
            break

        # Check individual file size limit
        block_bytes = len(block.encode("utf-8"))
        if block_bytes > max_file_bytes:
            was_truncated = True
            # Keep header and first max_file_bytes
            truncated_block = block[:max_file_bytes] + f"\n\n[... File diff truncated at {max_file_bytes} bytes ...]\n"
            kept_blocks.append(truncated_block)
            kept_files.append(file_path)
            warning_reasons.append(f"File `{file_path}` was truncated exceeding {max_file_bytes} bytes.")
        else:
            kept_blocks.append(block)
            kept_files.append(file_path)

    combined_diff = "\n\n".join(kept_blocks)
    total_bytes = len(combined_diff.encode("utf-8"))

    if total_bytes > max_total_bytes:
        was_truncated = True
        combined_diff = combined_diff[:max_total_bytes] + f"\n\n[... Unified diff truncated at {max_total_bytes} bytes ...]\n"
        warning_reasons.append(f"Total diff exceeded {max_total_bytes} bytes limit.")

    truncation_warning = None
    if was_truncated and warning_reasons:
        truncation_warning = (
            "> ⚠️ **Review Scope Notice:** " + " ".join(warning_reasons)
        )

    return combined_diff, kept_files, was_truncated, truncation_warning


# =============================================================================
# GitHub API Operations (Zero-Dependency)
# =============================================================================

def _github_api_request(
    url: str,
    method: str = "GET",
    data: Optional[Dict[str, Any]] = None,
    token: Optional[str] = None,
    custom_headers: Optional[Dict[str, str]] = None,
) -> Tuple[int, Any]:
    """Execute authenticated GitHub REST API request via urllib."""
    headers = {
        "Accept": "application/vnd.github+json",
        "User-Agent": "Contexto-PR-Review-Bot/1.0",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if custom_headers:
        headers.update(custom_headers)

    body = None
    if data is not None:
        headers["Content-Type"] = "application/json"
        body = json.dumps(data).encode("utf-8")

    req = urllib.request.Request(url, data=body, headers=headers, method=method)

    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            content_type = resp.headers.get("Content-Type", "")
            raw = resp.read()
            if "application/json" in content_type:
                return resp.status, json.loads(raw.decode("utf-8"))
            return resp.status, raw.decode("utf-8", errors="replace")
    except urllib.error.HTTPError as e:
        err_body = e.read().decode("utf-8", errors="replace")
        return e.code, err_body
    except urllib.error.URLError as e:
        raise RuntimeError(f"GitHub API connection failure: {e.reason}") from e


def fetch_pr_diff_from_github(repo: str, pr_number: int, token: str) -> str:
    """Fetch raw unified diff for a PR using GitHub API Accept diff header."""
    url = f"https://api.github.com/repos/{repo}/pulls/{pr_number}"
    status, result = _github_api_request(
        url,
        method="GET",
        token=token,
        custom_headers={"Accept": "application/vnd.github.v3.diff"},
    )
    if status != 200:
        raise RuntimeError(f"Failed to fetch PR diff ({status}): {result}")
    return str(result)


def fetch_pr_metadata_from_github(repo: str, pr_number: int, token: str) -> Dict[str, Any]:
    """Fetch PR details: title, description, base, head, author."""
    url = f"https://api.github.com/repos/{repo}/pulls/{pr_number}"
    status, result = _github_api_request(url, method="GET", token=token)
    if status != 200 or not isinstance(result, dict):
        raise RuntimeError(f"Failed to fetch PR #{pr_number} metadata ({status}): {result}")
    return {
        "title": result.get("title", ""),
        "body": result.get("body", "") or "",
        "base_branch": result.get("base", {}).get("ref", "main"),
        "head_branch": result.get("head", {}).get("ref", ""),
        "author": result.get("user", {}).get("login", ""),
    }


def find_existing_bot_comment(repo: str, pr_number: int, token: str) -> Optional[int]:
    """Find existing bot comment containing the hidden BOT_COMMENT_MARKER."""
    url = f"https://api.github.com/repos/{repo}/issues/{pr_number}/comments?per_page=50"
    status, comments = _github_api_request(url, method="GET", token=token)
    if status != 200 or not isinstance(comments, list):
        return None

    for comment in comments:
        body = comment.get("body", "")
        if BOT_COMMENT_MARKER in body:
            return comment.get("id")
    return None


def post_or_update_pr_comment(
    repo: str, pr_number: int, comment_body: str, token: str
) -> Tuple[str, int]:
    """Post new review comment or update existing one to eliminate comment spam."""
    existing_id = find_existing_bot_comment(repo, pr_number, token)

    if existing_id:
        update_url = f"https://api.github.com/repos/{repo}/issues/comments/{existing_id}"
        status, res = _github_api_request(
            update_url, method="PATCH", data={"body": comment_body}, token=token
        )
        if status not in (200, 201):
            raise RuntimeError(f"Failed to update existing comment #{existing_id} ({status}): {res}")
        return "updated", existing_id
    else:
        post_url = f"https://api.github.com/repos/{repo}/issues/{pr_number}/comments"
        status, res = _github_api_request(
            post_url, method="POST", data={"body": comment_body}, token=token
        )
        if status not in (200, 201) or not isinstance(res, dict):
            raise RuntimeError(f"Failed to post comment to PR #{pr_number} ({status}): {res}")
        return "created", res.get("id", 0)


# =============================================================================
# Prompt Assembly & Response Parsing
# =============================================================================

def load_prompt_template() -> str:
    """Load the review prompt template from prompts/pr_review.md."""
    template_path = Path(__file__).resolve().parent.parent / "prompts" / "pr_review.md"
    if not template_path.is_file():
        # Fallback inline template
        return (
            "You are a Principal Software Engineer reviewing this PR diff.\n"
            "Review Priorities: Correctness, Security, Data-loss, Breaking changes, Concurrency, Performance.\n"
            "Do NOT report formatting/style.\n"
            "Structure output into Summary, Findings (CRITICAL, HIGH, MEDIUM, LOW), and Recommendation (PASS|WARN|FAIL).\n\n"
            "Repo: {repository} PR #{pr_number}\n\n{diff_truncation_warning}\n\n"
            "<<<UNTRUSTED_PR_DIFF>>>\n{unified_diff}\n<<<END_UNTRUSTED_PR_DIFF>>>"
        )
    return template_path.read_text(encoding="utf-8")


def assemble_prompt(
    repo: str,
    pr_number: int,
    pr_title: str,
    pr_description: str,
    base_branch: str,
    head_branch: str,
    unified_diff: str,
    truncation_warning: Optional[str],
) -> str:
    """Build the final review prompt with quarantined untrusted PR content."""
    template = load_prompt_template()

    # Sanitize PR description against markdown breakages
    safe_description = (pr_description or "No description provided.").strip()

    warning_text = truncation_warning if truncation_warning else ""

    prompt = template.replace("{repository}", repo)
    prompt = prompt.replace("{pr_number}", str(pr_number))
    prompt = prompt.replace("{pr_title}", pr_title)
    prompt = prompt.replace("{base_branch}", base_branch)
    prompt = prompt.replace("{head_branch}", head_branch)
    prompt = prompt.replace("{pr_description}", safe_description)
    prompt = prompt.replace("{diff_truncation_warning}", warning_text)
    prompt = prompt.replace("{unified_diff}", unified_diff)

    return prompt


def parse_review_findings(raw_review: str) -> Tuple[Dict[str, int], str, str]:
    """
    Parse OMNI output into severity counts, overall recommendation, and clean text.

    Returns:
        (severity_counts, recommendation_status, sanitized_review)
    """
    clean_text = strip_backend_refs(raw_review)

    counts = {
        "CRITICAL": 0,
        "HIGH": 0,
        "MEDIUM": 0,
        "LOW": 0,
    }

    # Count structured severity occurrences
    for sev in ("CRITICAL", "HIGH", "MEDIUM", "LOW"):
        # Match headers or bullet points like "#### 🔴 CRITICAL" or "**Severity**: CRITICAL"
        pattern = rf"(?:####\s*.*?\b{sev}\b|\*\*Severity\*\*:\s*{sev})"
        matches = re.findall(pattern, clean_text, flags=re.IGNORECASE)
        # Avoid counting the template legend section if present
        counts[sev] = len(matches)

    # Determine recommendation
    # Priority: explicit recommendation in text, or computed from findings
    rec_match = re.search(r"###\s*Recommendation\s*\n+.*?\b(PASS|WARN|FAIL)\b", clean_text, flags=re.IGNORECASE)
    if rec_match:
        declared_status = rec_match.group(1).upper()
    else:
        declared_status = None

    # Compute grounded status based on findings
    if counts["CRITICAL"] > 0 or counts["HIGH"] > 0:
        computed_status = "FAIL"
    elif counts["MEDIUM"] > 0 or counts["LOW"] > 0:
        computed_status = "WARN"
    else:
        computed_status = "PASS"

    status = declared_status or computed_status

    # Security check: If critical/high findings exist, status CANNOT be PASS
    if (counts["CRITICAL"] > 0 or counts["HIGH"] > 0) and status == "PASS":
        status = "FAIL"

    return counts, status, clean_text


def format_pr_comment(
    review_content: str,
    status: str,
    counts: Dict[str, int],
    truncation_warning: Optional[str] = None,
) -> str:
    """Format final Markdown comment with header badge, findings, and metadata."""
    now_utc = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")

    status_badge = {
        "PASS": "🟢 **PASS** — No critical issues found",
        "WARN": "🟡 **WARN** — Actionable observations detected",
        "FAIL": "🔴 **FAIL** — Critical or high-risk issues require resolution",
    }.get(status, f"⚪ **{status}**")

    parts = [
        BOT_COMMENT_MARKER,
        "## 🤖 Automated Code Review",
        "",
        f"**Review Outcome:** {status_badge}",
        f"**Findings Summary:** {counts['CRITICAL']} Critical • {counts['HIGH']} High • {counts['MEDIUM']} Medium • {counts['LOW']} Low",
        "",
    ]

    if truncation_warning:
        parts.append(f"{truncation_warning}\n")

    # If the review content already starts with markdown sections, append cleanly
    parts.append(review_content)
    parts.append("")
    parts.append("---")
    parts.append(f"*Evaluated automatically against repository safety rules • {now_utc}*")

    return "\n".join(parts)


def format_error_comment(reason: str) -> str:
    """Format graceful diagnostic comment when review fails."""
    now_utc = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")
    return (
        f"{BOT_COMMENT_MARKER}\n"
        "## 🤖 Automated Code Review\n\n"
        "⚠️ **Review Notice:** The automated review could not be completed for this change.\n\n"
        f"**Reason:** {reason}\n\n"
        "The review will re-trigger automatically on the next pull request update.\n\n"
        "---\n"
        f"*Evaluated automatically • {now_utc}*\n"
    )


# =============================================================================
# Main Orchestration Routine
# =============================================================================

def run_review(
    repo: str,
    pr_number: int,
    raw_diff: Optional[str] = None,
    pr_meta: Optional[Dict[str, Any]] = None,
    token: Optional[str] = None,
    dry_run: bool = False,
    mode: str = OMNI_MODEL,
    timeout: int = REVIEW_TIMEOUT,
) -> Tuple[str, str]:
    """
    Execute full PR review pipeline.

    Returns:
        (status: PASS|WARN|FAIL, comment_body: str)
    """
    token = token or os.getenv("GITHUB_TOKEN", "")

    # 1. Fetch metadata if not provided
    if not pr_meta:
        if token:
            try:
                pr_meta = fetch_pr_metadata_from_github(repo, pr_number, token)
            except Exception as e:
                pr_meta = {
                    "title": f"PR #{pr_number}",
                    "body": "",
                    "base_branch": "main",
                    "head_branch": "feature",
                    "author": "unknown",
                }
        else:
            pr_meta = {
                "title": f"PR #{pr_number}",
                "body": "Local execution",
                "base_branch": "main",
                "head_branch": "feature",
                "author": "local",
            }

    # 2. Fetch diff if not provided
    if raw_diff is None:
        if not token:
            raise ValueError("Cannot fetch PR diff from GitHub without GITHUB_TOKEN.")
        raw_diff = fetch_pr_diff_from_github(repo, pr_number, token)

    # 3. Filter and bound diff
    filtered_diff, files, was_truncated, truncation_warning = parse_and_filter_diff(raw_diff)

    if not filtered_diff.strip():
        # Clean diff or non-code files only
        summary = "No reviewable code changes detected (only ignored files, lockfiles, or deletions were modified)."
        comment_body = format_pr_comment(
            f"### Summary\n{summary}\n\n### Recommendation\nPASS",
            status="PASS",
            counts={"CRITICAL": 0, "HIGH": 0, "MEDIUM": 0, "LOW": 0},
            truncation_warning=truncation_warning,
        )
        if not dry_run and token:
            post_or_update_pr_comment(repo, pr_number, comment_body, token)
        return "PASS", comment_body

    # 4. Assemble prompt
    prompt = assemble_prompt(
        repo=repo,
        pr_number=pr_number,
        pr_title=pr_meta.get("title", ""),
        pr_description=pr_meta.get("body", ""),
        base_branch=pr_meta.get("base_branch", "main"),
        head_branch=pr_meta.get("head_branch", ""),
        unified_diff=filtered_diff,
        truncation_warning=truncation_warning,
    )

    # 5. Call OMNI
    try:
        raw_answer, _, _ = ask_deepwiki(
            repo=repo,
            question=prompt,
            mode=mode,
            timeout=timeout,
            api_key=OMNI_API_KEY,
            verbose=True,
        )
    except Exception as exc:
        clean_err = re.sub(r"(?:Bearer\s+|token=)[a-zA-Z0-9_\-]+", "[REDACTED]", str(exc))
        error_comment = format_error_comment(f"Code analysis engine unavailable: {clean_err}")
        if not dry_run and token:
            try:
                post_or_update_pr_comment(repo, pr_number, error_comment, token)
            except Exception as post_err:
                sys.stderr.write(f"Failed to post error comment: {post_err}\n")
        return "FAIL", error_comment

    # 6. Parse findings & recommendation
    counts, status, clean_review = parse_review_findings(raw_answer)

    # 7. Format comment body
    comment_body = format_pr_comment(
        clean_review, status=status, counts=counts, truncation_warning=truncation_warning
    )

    # 8. Post to GitHub
    if not dry_run and token:
        action, c_id = post_or_update_pr_comment(repo, pr_number, comment_body, token)
        sys.stderr.write(f"Successfully {action} PR comment #{c_id} for {repo}#{pr_number}\n")

    return status, comment_body


# =============================================================================
# CLI Entry Point & Local Testing Support
# =============================================================================

SAMPLE_DIFF = """diff --git a/crates/ctxd/src/auth.rs b/crates/ctxd/src/auth.rs
index 10a1234..89b5678 100644
--- a/crates/ctxd/src/auth.rs
+++ b/crates/ctxd/src/auth.rs
@@ -115,10 +115,10 @@ pub async fn require_auth(
     req: Request<Body>,
     next: Next,
 ) -> Result<Response, StatusCode> {
-    let provided = req
+    let token_param = req
         .headers()
-        .get("Authorization")
-        .and_then(|v| v.to_str().ok());
+        .get("X-Custom-Auth")
+        .and_then(|v| v.to_str().ok());
     
+    // Potential vulnerability: Bypasses Bearer scheme checks
+    Ok(next.run(req).await)
 }
"""


def main():
    parser = argparse.ArgumentParser(description="Automated PR Code Reviewer")
    parser.add_argument("--repo", help="GitHub repo (owner/name)", default=os.getenv("GITHUB_REPOSITORY"))
    parser.add_argument("--pr", type=int, help="PR number", default=None)
    parser.add_argument("--diff", help="Path to local diff file")
    parser.add_argument("--sample-diff", action="store_true", help="Run against built-in sample diff")
    parser.add_argument("--dry-run", action="store_true", help="Do not post comment to GitHub")
    parser.add_argument("--mode", choices=["fast", "deep"], default=OMNI_MODEL, help="OMNI model mode")
    parser.add_argument("--timeout", type=int, default=REVIEW_TIMEOUT, help="Review timeout in seconds")
    parser.add_argument("--out", help="Write final markdown review to file")

    args = parser.parse_args()

    # Discover PR and Repo from GitHub Actions environment if not explicitly provided
    repo = args.repo
    pr_number = args.pr
    pr_meta = None

    event_path = os.getenv("GITHUB_EVENT_PATH")
    if event_path and os.path.isfile(event_path) and (not repo or not pr_number):
        try:
            with open(event_path, "r", encoding="utf-8") as f:
                event_data = json.load(f)
            if "pull_request" in event_data:
                pr_data = event_data["pull_request"]
                pr_number = pr_number or pr_data.get("number")
                if "repository" in event_data:
                    repo = repo or event_data["repository"].get("full_name")
                pr_meta = {
                    "title": pr_data.get("title", ""),
                    "body": pr_data.get("body", ""),
                    "base_branch": pr_data.get("base", {}).get("ref", "main"),
                    "head_branch": pr_data.get("head", {}).get("ref", ""),
                    "author": pr_data.get("user", {}).get("login", ""),
                }
        except Exception as e:
            sys.stderr.write(f"Warning: Could not parse GITHUB_EVENT_PATH: {e}\n")

    diff_content: Optional[str] = None
    if args.sample_diff:
        diff_content = SAMPLE_DIFF
        repo = repo or "NSTKrishna/Contexto"
        pr_number = pr_number or 999
        pr_meta = {
            "title": "Sample: Refactor auth header checking",
            "body": "Replaces standard Bearer header with X-Custom-Auth",
            "base_branch": "main",
            "head_branch": "feat/custom-auth",
            "author": "sample-contributor",
        }
    elif args.diff:
        diff_path = Path(args.diff)
        if not diff_path.is_file():
            sys.stderr.write(f"Error: diff file '{args.diff}' not found.\n")
            sys.exit(1)
        diff_content = diff_path.read_text(encoding="utf-8")
        repo = repo or "NSTKrishna/Contexto"
        pr_number = pr_number or 1

    if not repo or not pr_number:
        sys.stderr.write("Error: Both --repo and --pr (or valid GITHUB_EVENT_PATH) are required.\n")
        parser.print_help()
        sys.exit(1)

    status, comment = run_review(
        repo=repo,
        pr_number=pr_number,
        raw_diff=diff_content,
        pr_meta=pr_meta,
        dry_run=args.dry_run or (diff_content is not None and not os.getenv("GITHUB_TOKEN")),
        mode=args.mode,
        timeout=args.timeout,
    )

    print(f"\nReview Status: {status}")
    print("=" * 60)
    print(comment)

    if args.out:
        Path(args.out).write_text(comment, encoding="utf-8")
        print(f"\nReview written to {args.out}")


if __name__ == "__main__":
    main()
