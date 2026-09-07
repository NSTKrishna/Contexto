-- =============================================================================
-- Migration 003: Task Tracking Table
-- Contexto — Universal Developer Context Manager
-- =============================================================================
-- Tasks are the Layer 2 abstraction: a named, time-bounded unit of developer
-- work that groups related context events. When a task is stopped, it becomes
-- a candidate for LLM summarization (Phase 6+).

CREATE TABLE IF NOT EXISTS tasks (
    id          TEXT PRIMARY KEY NOT NULL,

    -- Human-readable task name (e.g. "Implement ring buffer in ctx-db")
    name        TEXT NOT NULL,

    -- Optional description / initial note
    description TEXT,

    -- ISO-8601 UTC: when the task was started
    started_at  TEXT NOT NULL,

    -- ISO-8601 UTC: when the task was stopped (NULL = still active)
    stopped_at  TEXT,

    -- Current status: active | paused | completed | abandoned
    status      TEXT NOT NULL DEFAULT 'active',

    -- LLM-generated summary (populated in Phase 6+ when task is completed)
    summary     TEXT,

    -- Git repository this task is associated with
    git_repo    TEXT,

    -- Number of context events captured during this task (denormalized for display)
    event_count INTEGER NOT NULL DEFAULT 0
);

-- Index for finding the currently active task quickly
CREATE INDEX IF NOT EXISTS idx_tasks_status
    ON tasks (status);

-- Time-ordered task history
CREATE INDEX IF NOT EXISTS idx_tasks_started_at
    ON tasks (started_at DESC);
