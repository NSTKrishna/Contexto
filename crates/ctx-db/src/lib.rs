//! # ctx-db
//!
//! SQLite + FTS5 database layer for Contexto.
//!
//! ## Design Decisions
//!
//! ### Storage: SQLite with FTS5
//! We use SQLite's built-in FTS5 extension for full-text search in Phases 1–4.
//! This gives instant BM25-ranked search with zero extra dependencies.
//! Vector/semantic search (fastembed-rs + HNSW) is added in Phase 6.
//!
//! ### Avoiding `sqlite-vss`: The C++ Compilation Problem
//! `sqlite-vss` relies on FAISS C++ bindings which are notoriously painful to
//! compile cross-platform inside Tauri apps (especially arm64 macOS).
//! We defer vector search to Phase 6 where we use LanceDB embedded or
//! `fastembed-rs`, both of which are pure Rust / pre-built friendly.
//!
//! ### Batch Writer
//! Never write single events to SQLite. A background task drains the ring
//! buffer from `ctx-core` and writes in transactions of up to 50 events,
//! flushing at most every 500ms. This prevents SQLITE_BUSY under burst load.

use anyhow::Result;
use ctx_core::ContextEvent;

/// Database handle for Contexto's SQLite store.
///
/// This is a placeholder for Phase 1 implementation. The full implementation
/// will include:
/// - `sqlx::SqlitePool` connection pool
/// - `migrate!()` macro for schema migrations
/// - FTS5 virtual table for full-text search
/// - Batch writer that drains the `ctx-core` ingestion ring buffer
pub struct ContextoDb {
    // Phase 1: db_path: std::path::PathBuf,
    // Phase 1: pool: sqlx::SqlitePool,
}

impl ContextoDb {
    /// Open (or create) the Contexto database at the given path.
    ///
    /// Runs all pending schema migrations on startup.
    /// # Errors
    /// Returns an error if the database cannot be opened or migrations fail.
    pub async fn open(_db_path: &str) -> Result<Self> {
        tracing::info!("ctx-db: Phase 1 stub — database layer not yet implemented");
        Ok(Self {})
    }

    /// Insert a batch of events in a single SQLite transaction.
    ///
    /// Uses `BEGIN TRANSACTION` / `COMMIT` to atomically write all events,
    /// preventing partial writes and minimizing lock duration.
    /// # Errors
    /// Returns an error if the transaction fails.
    pub async fn insert_events_batch(&self, events: &[ContextEvent]) -> Result<()> {
        tracing::debug!("ctx-db: would insert {} events (Phase 1 stub)", events.len());
        Ok(())
    }

    /// Full-text search across all stored events using SQLite FTS5 + BM25 ranking.
    ///
    /// Returns events sorted by relevance score (highest first).
    /// # Errors
    /// Returns an error if the query fails.
    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<ContextEvent>> {
        tracing::debug!("ctx-db: would search '{}' limit {} (Phase 1 stub)", query, limit);
        Ok(vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_db_stub_open() {
        let db = ContextoDb::open(":memory:").await;
        assert!(db.is_ok(), "stub open should succeed");
    }

    #[tokio::test]
    async fn test_db_stub_insert_batch() {
        let db = ContextoDb::open(":memory:").await.unwrap();
        let result = db.insert_events_batch(&[]).await;
        assert!(result.is_ok(), "stub batch insert should succeed");
    }
}
