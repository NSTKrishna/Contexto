-- =============================================================================
-- Migration 001: Core Events Table
-- Contexto — Universal Developer Context Manager
-- =============================================================================
-- This is the primary storage table for all captured developer context events.
-- Indexed for time-ordered reads (the most common query pattern).
-- FTS5 virtual table is created in migration 002.

CREATE TABLE IF NOT EXISTS context_events (
    -- Canonical UUID for deduplication (from ctx-core::ContextEvent::id)
    id          TEXT PRIMARY KEY NOT NULL,

    -- ISO-8601 UTC timestamp
    timestamp   TEXT NOT NULL,

    -- EventSource enum value: TERM | IDE | GIT | FS | NOTE | MCP
    source      TEXT NOT NULL,

    -- Short human-readable label (e.g. "cargo build", "main.rs saved")
    label       TEXT NOT NULL,

    -- Raw captured content (may be truncated at CTX_MAX_TERMINAL_EVENT_SIZE)
    content     TEXT NOT NULL DEFAULT '',

    -- JSON blob for structured metadata (nullable)
    metadata    TEXT,

    -- 1 if content was scrubbed by the redaction engine, 0 otherwise
    was_redacted INTEGER NOT NULL DEFAULT 0,

    -- Working directory at time of event
    cwd         TEXT,

    -- Git repository root (if applicable)
    git_repo    TEXT,

    -- Associated task UUID (if applicable)
    task_id     TEXT REFERENCES tasks(id) ON DELETE SET NULL
);

-- Time-ordered index (primary query pattern: recent events first)
CREATE INDEX IF NOT EXISTS idx_events_timestamp
    ON context_events (timestamp DESC);

-- Source filtering index (e.g. "show only GIT events")
CREATE INDEX IF NOT EXISTS idx_events_source
    ON context_events (source);

-- Task grouping index (e.g. "all events for task X")
CREATE INDEX IF NOT EXISTS idx_events_task_id
    ON context_events (task_id)
    WHERE task_id IS NOT NULL;

-- Repo filtering index (e.g. "events from Contexto repo only")
CREATE INDEX IF NOT EXISTS idx_events_git_repo
    ON context_events (git_repo)
    WHERE git_repo IS NOT NULL;
