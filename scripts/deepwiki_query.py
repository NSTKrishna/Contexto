#!/usr/bin/env python3
"""
DeepWiki OMNI Agent API Client.

A zero-external-dependency Python client for Cognition's api.devin.ai OMNI agent.
Provides automated, full-codebase semantic understanding and diff reviews.

Key Protocol Details:
- Endpoint: POST https://api.devin.ai/ada/query
- Polling:  GET https://api.devin.ai/ada/query/<query_id>
- Headers:  Origin: https://deepwiki.com, Referer: https://deepwiki.com/
- Query ID: Full UUID with underscore separator (prefix_<full_uuid>)
- Payload:  Requires `additional_context` field
- OPSEC:    Sanitizes all backend references from final responses
"""

import json
import os
import re
import sys
import time
import urllib.error
import urllib.request
import uuid
from typing import Any, Dict, List, Optional, Tuple

API_BASE = os.getenv("OMNI_BASE_URL", "https://api.devin.ai/ada/query")
DEFAULT_TIMEOUT = int(os.getenv("REVIEW_TIMEOUT", "180"))

# Regex patterns to strip backend/vendor references for OPSEC compliance
_BACKEND_STRIP_PATTERNS = [
    re.compile(r"^Wiki pages? you might want to explore:[^\n]*(?:\n[ \t]*[-*]\s*[^\n]+)*", re.MULTILINE | re.IGNORECASE),
    re.compile(r"View this search on DeepWiki:?[^\n]*", re.IGNORECASE),
    re.compile(r"\[DeepWiki\]\(.*?\)", re.IGNORECASE),
    re.compile(r"https?://(?:www\.)?deepwiki\.com\S*", re.IGNORECASE),
    re.compile(r"https?://(?:www\.)?devin\.ai\S*", re.IGNORECASE),
    re.compile(r"^(?:View|Check|Open|See)\s+(?:this|the|our)\s+(?:search|result|wiki|page)[^\n]*", re.MULTILINE | re.IGNORECASE),
    re.compile(r"Powered by[^\n]*", re.IGNORECASE),
]


def strip_backend_refs(text: str) -> str:
    """Strip all DeepWiki, Devin, and vendor references from agent output."""
    sanitized = text
    for pattern in _BACKEND_STRIP_PATTERNS:
        sanitized = pattern.sub("", sanitized)
    return sanitized.strip()


def slugify(text: str, max_len: int = 40) -> str:
    """Normalize text into an alphanumeric slug for query_id prefix."""
    text = text.lower().strip()
    text = re.sub(r"[^\w\s-]", "", text)
    text = re.sub(r"[\s_-]+", "-", text)
    slug = text[:max_len].strip("-")
    return slug or "review"


def get_headers(api_key: Optional[str] = None) -> Dict[str, str]:
    """Construct mandatory headers for OMNI API requests."""
    headers = {
        "Content-Type": "application/json",
        "Origin": "https://deepwiki.com",
        "Referer": "https://deepwiki.com/",
        "User-Agent": "Contexto-PR-Reviewer/1.0",
    }
    key = api_key or os.getenv("OMNI_API_KEY", "").strip()
    if key:
        headers["Authorization"] = f"Bearer {key}"
    return headers


def ask_deepwiki(
    repo: str,
    question: str,
    mode: str = "deep",
    timeout: int = DEFAULT_TIMEOUT,
    wiki_page: str = "Overview",
    verbose: bool = False,
    api_key: Optional[str] = None,
    base_url: str = API_BASE,
) -> Tuple[str, Dict[str, str], List[Dict[str, Any]]]:
    """
    Query the DeepWiki OMNI agent and poll until completion.

    Args:
        repo: Exact case-sensitive GitHub repository (e.g. 'NSTKrishna/Contexto').
        question: User query or prompt with embedded diff.
        mode: 'fast' (~7-15s for diffs) or 'deep' (~60-140s for deep audits).
        timeout: Maximum polling duration in seconds.
        wiki_page: Context label.
        verbose: Print progress traces to stderr.
        api_key: Optional authorization token if configured.
        base_url: OMNI endpoint URL.

    Returns:
        (answer_text, files_dict, references_list)
    """
    if mode not in ("fast", "deep", "codemap"):
        mode = "deep"

    headers = get_headers(api_key)
    query_id = f"{slugify(question)}_{uuid.uuid4()}"

    payload = {
        "query_id": query_id,
        "mode": mode,
        "repo_names": [repo],
        "source": "ada.deepwiki_public",
        "user_query": question,
        "additional_context": (
            f"<relevant_context>\n"
            f"This query was sent from the wiki page: {wiki_page}.\n"
            f"Verify against the actual PR-branch code embedded in user_query.\n"
            f"Do NOT rely on the repository's indexed snapshot.\n"
            f"</relevant_context>"
        ),
        "generate_summary": False,
        "keywords": [],
    }

    if verbose:
        sys.stderr.write(f"[OMNI] Submitting query_id={query_id} mode={mode} repo={repo}\n")

    # Step 1: Submit query
    req = urllib.request.Request(
        base_url,
        data=json.dumps(payload).encode("utf-8"),
        headers=headers,
        method="POST",
    )

    fallback_repo = os.getenv("OMNI_FALLBACK_REPO", "NSTKrishna/Contexto")
    backup_carrier = os.getenv("OMNI_BACKUP_CARRIER", "bitflicker64/Termstory")

    def _try_submit(target_repo: str) -> bool:
        payload["repo_names"] = [target_repo]
        s_req = urllib.request.Request(
            base_url,
            data=json.dumps(payload).encode("utf-8"),
            headers=headers,
            method="POST",
        )
        try:
            with urllib.request.urlopen(s_req, timeout=30) as resp:
                data = json.loads(resp.read().decode("utf-8"))
                return data.get("status") == "success"
        except urllib.error.HTTPError as err:
            err_body = err.read().decode("utf-8", errors="replace")
            if err.code == 400 and "Repos not found" in err_body:
                return False
            raise RuntimeError(f"OMNI submit HTTP {err.code}: {err_body}") from err
        except urllib.error.URLError as err:
            raise RuntimeError(f"OMNI submit network error: {err.reason}") from err

    # Step 1: Submit query (with automatic routing to NSTKrishna/Contexto if unindexed)
    submitted = _try_submit(repo)
    if not submitted:
        # Route to NSTKrishna/Contexto if target repo is not yet indexed
        candidates = []
        if repo != fallback_repo:
            candidates.append(fallback_repo)
        if backup_carrier not in candidates and repo != backup_carrier:
            candidates.append(backup_carrier)

        for carrier in candidates:
            if verbose:
                sys.stderr.write(f"[OMNI] Repo '{repo}' not in public index. Automatically routing to '{carrier}'...\n")
            if _try_submit(carrier):
                submitted = True
                break

        if not submitted:
            raise RuntimeError(f"OMNI submit failed: Neither '{repo}' nor fallback carrier repos could be accessed.")

    # Step 2: Poll query status until 'done' or timeout
    poll_url = f"{base_url}/{query_id}"
    start_time = time.time()
    get_retries = 0

    while time.time() - start_time < timeout:
        time.sleep(3)
        poll_req = urllib.request.Request(poll_url, headers=headers, method="GET")

        try:
            with urllib.request.urlopen(poll_req, timeout=45) as resp:
                data = json.loads(resp.read().decode("utf-8"))
                get_retries = 0  # Reset retry count on success
        except (urllib.error.URLError, TimeoutError) as e:
            get_retries += 1
            if verbose:
                sys.stderr.write(f"[OMNI] Transient poll error ({e}), retry #{get_retries}\n")
            if get_retries >= 5:
                raise RuntimeError("Too many consecutive poll errors connecting to OMNI API") from e
            continue
        except (ValueError, json.JSONDecodeError) as e:
            get_retries += 1
            if get_retries >= 5:
                raise RuntimeError(f"Repeated JSON decoding errors from OMNI: {e}") from e
            continue

        queries = data.get("queries", [])
        if not queries:
            continue

        latest = queries[-1]
        state = latest.get("state", "pending")
        elapsed = int(time.time() - start_time)

        if verbose:
            items = latest.get("response", [])
            tools = sum(1 for i in items if "tool_call" in i.get("type", ""))
            chunks = sum(1 for i in items if i.get("type") == "chunk")
            sys.stderr.write(f"[OMNI] state={state} tools={tools} chunks={chunks} elapsed={elapsed}s\n")

        if state == "done":
            items = latest.get("response", [])
            chunks_text = []
            files: Dict[str, str] = {}
            refs: List[Dict[str, Any]] = []

            for item in items:
                t = item.get("type")
                if t == "chunk":
                    chunks_text.append(item.get("data", ""))
                elif t == "file_contents":
                    f_data = item.get("data", [])
                    if isinstance(f_data, list) and len(f_data) >= 3:
                        files[str(f_data[1])] = str(f_data[2])
                elif t == "reference":
                    refs.append(item.get("data", {}))

            raw_answer = "".join(chunks_text).strip()
            clean_answer = strip_backend_refs(raw_answer)
            return clean_answer, files, refs

        elif state == "error":
            err_msg = latest.get("error", "Unknown OMNI query failure")
            raise RuntimeError(f"OMNI query failed: {err_msg}")

    raise TimeoutError(f"OMNI query did not complete within {timeout} seconds")


if __name__ == "__main__":
    if len(sys.argv) < 3:
        print("Usage: python deepwiki_query.py <repo> <question> [mode]", file=sys.stderr)
        sys.exit(1)

    repo_arg = sys.argv[1]
    question_arg = sys.argv[2]
    mode_arg = sys.argv[3] if len(sys.argv) > 3 else "deep"

    try:
        ans, f_dict, r_list = ask_deepwiki(repo_arg, question_arg, mode=mode_arg, verbose=True)
        print("=" * 60)
        print("ANSWER:")
        print(ans)
        print("=" * 60)
        print(f"Files referenced: {len(f_dict)}, Refs: {len(r_list)}")
    except Exception as exc:
        print(f"Error: {exc}", file=sys.stderr)
        sys.exit(1)
