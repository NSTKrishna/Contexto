-- =============================================================================
-- Migration 002: FTS5 Full-Text Search Index
-- Contexto — Universal Developer Context Manager
-- =============================================================================
-- Creates a SQLite FTS5 virtual table that indexes the `label` and `content`
-- columns of `context_events` for instant BM25-ranked full-text search.
--
-- Why FTS5 over sqlite-vss / FAISS:
--   sqlite-vss relies on C++ FAISS bindings — painful to compile cross-platform
--   inside a Tauri app (especially arm64 macOS). FTS5 is built into SQLite,
--   has zero extra dependencies, and provides sub-millisecond BM25 search.
--   Vector/semantic search is deferred to Phase 6 (fastembed-rs + LanceDB).
--
-- FTS5 content mode: we use `content=context_events` so the FTS table stores
-- only the index data, not a copy of the text. Triggers keep it in sync.

CREATE VIRTUAL TABLE IF NOT EXISTS events_fts USING fts5(
    label,
    content,
    -- Link to the source table (content mode — no data duplication)
    content     = context_events,
    content_rowid = rowid,
    -- BM25 tokenizer with porter stemmer for better recall
    tokenize    = 'porter ascii'
);

-- ---------------------------------------------------------------------------
-- Sync Triggers: keep events_fts in sync with context_events automatically
-- ---------------------------------------------------------------------------

-- After INSERT: add new row to FTS index
CREATE TRIGGER IF NOT EXISTS events_fts_ai
AFTER INSERT ON context_events BEGIN
    INSERT INTO events_fts (rowid, label, content)
    VALUES (new.rowid, new.label, new.content);
END;

-- After DELETE: remove row from FTS index
CREATE TRIGGER IF NOT EXISTS events_fts_ad
AFTER DELETE ON context_events BEGIN
    INSERT INTO events_fts (events_fts, rowid, label, content)
    VALUES ('delete', old.rowid, old.label, old.content);
END;

-- After UPDATE: replace row in FTS index
CREATE TRIGGER IF NOT EXISTS events_fts_au
AFTER UPDATE ON context_events BEGIN
    INSERT INTO events_fts (events_fts, rowid, label, content)
    VALUES ('delete', old.rowid, old.label, old.content);
    INSERT INTO events_fts (rowid, label, content)
    VALUES (new.rowid, new.label, new.content);
END;
