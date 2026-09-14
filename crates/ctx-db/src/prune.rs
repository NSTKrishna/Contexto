//! # prune
//!
//! TTL-based event pruning and SQLite incremental vacuum for `ctx-db`.
//!
//! ## Tiered Retention Policy
//!
//! | Layer | Retention | Rule |
//! |---|---|---|
//! | **Layer 1** (orphan events) | 14–30 days | Pruned by `prune_expired_events(days)` |
//! | **Layer 1** (task-bound events) | Until task archived | Preserved while `task_id IS NOT NULL` |
//! | **Layer 2** (tasks + session cards) | Indefinite | Never pruned automatically |
//! | **Layer 3** (permanent notes) | Permanent | `EventSource::Manual` / `EventSource::Mcp` never pruned |
//!
//! ## Vacuum Strategy
//!
//! SQLite with `PRAGMA auto_vacuum = INCREMENTAL` (migration 004) accumulates
//! free pages in a free-list. Calling `PRAGMA incremental_vacuum(N)` moves up
//! to N free pages back to the OS. We call this after every pruning run.
//!
//! A page_count of `1000` at the default 4KB page size returns up to 4MB to
//! the filesystem per vacuum pass — a conservative, low-contention approach.
//!
//! ## Scheduling
//!
//! The pruning task is spawned in `ctxd/src/main.rs` as a background
//! `tokio::spawn` loop that fires every 24 hours. It can also be triggered
//! manually via `ctx prune [--days N]`.

use anyhow::{Context, Result};
use tracing::{debug, info, warn};

use crate::ContextoDb;

impl ContextoDb {
    /// Prune expired Layer-1 (orphan) events older than `days` days.
    ///
    /// **Preservation rules:**
    /// - Events with a `task_id` (task-bound) are **never** pruned here.
    ///   They are cleaned up when their parent task is archived.
    /// - `EventSource::Manual` ("NOTE") events are **never** pruned.
    ///   These are permanent Layer-3 annotations.
    /// - `EventSource::Mcp` ("MCP") events are **never** pruned.
    ///   These are AI-authored permanent notes.
    ///
    /// # Returns
    ///
    /// The number of events deleted.
    ///
    /// # Errors
    ///
    /// Returns an error if the DELETE query fails.
    pub async fn prune_expired_events(&self, days: u32) -> Result<u64> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(i64::from(days));
        let cutoff_str = cutoff.to_rfc3339();

        let result = sqlx::query(
            r#"
            DELETE FROM context_events
            WHERE timestamp < ?
              AND task_id IS NULL
              AND source NOT IN ('NOTE', 'MCP')
            "#,
        )
        .bind(&cutoff_str)
        .execute(&self.pool)
        .await
        .with_context(|| {
            format!("Failed to prune events older than {days} days (cutoff: {cutoff_str})")
        })?;

        let deleted = result.rows_affected();

        if deleted > 0 {
            info!("Pruned {deleted} orphan events older than {days} days (cutoff: {cutoff_str})");
        } else {
            debug!("No orphan events to prune (cutoff: {cutoff_str})");
        }

        Ok(deleted)
    }

    /// Reclaim freed database pages back to the filesystem.
    ///
    /// Requires `PRAGMA auto_vacuum = INCREMENTAL` (migration 004).
    /// Removes up to `page_count` pages from SQLite's free-list and
    /// returns them to the OS, shrinking the `.ctx/ctx.db` file on disk.
    ///
    /// A `page_count` of `1000` at the default 4KB page size reclaims
    /// up to 4MB per call. Call after each pruning run.
    ///
    /// # Errors
    ///
    /// Returns an error if the PRAGMA execution fails.
    pub async fn incremental_vacuum(&self, page_count: u32) -> Result<()> {
        let sql = format!("PRAGMA incremental_vacuum({page_count})");
        sqlx::raw_sql(&sql)
            .execute(&self.pool)
            .await
            .with_context(|| format!("incremental_vacuum({page_count}) failed"))?;
        debug!("Incremental vacuum: up to {page_count} pages reclaimed");
        Ok(())
    }

    /// Combined prune + vacuum convenience method.
    ///
    /// Prunes orphan events older than `days` days, then runs an incremental
    /// vacuum pass if any events were deleted. This is the method called by
    /// the background pruning task in `ctxd`.
    ///
    /// # Errors
    ///
    /// Returns an error if either the prune or vacuum step fails.
    pub async fn prune_and_vacuum(&self, days: u32) -> Result<u64> {
        let deleted = self.prune_expired_events(days).await?;

        if deleted > 0 {
            match self.incremental_vacuum(1000).await {
                Ok(()) => info!("Vacuum completed after pruning {deleted} events"),
                Err(e) => {
                    // Non-fatal: pruning succeeded, vacuum failure is logged but not propagated
                    warn!("Incremental vacuum failed (non-fatal): {e}");
                }
            }
        }

        Ok(deleted)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use ctx_core::{EventFilter, Task};

    use crate::ContextoDb;

    async fn test_db() -> ContextoDb {
        ContextoDb::open(":memory:").await.expect("in-memory DB")
    }

    /// Insert an event with a manually set timestamp (bypasses ContextEvent::new)
    /// by inserting directly via SQL for test control.
    async fn insert_old_event(db: &ContextoDb, source_str: &str, days_ago: i64) {
        let ts = (chrono::Utc::now() - chrono::Duration::days(days_ago)).to_rfc3339();
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO context_events (id, timestamp, source, label, content) VALUES (?, ?, ?, ?, ?)"
        )
        .bind(&id)
        .bind(&ts)
        .bind(source_str)
        .bind("old event")
        .bind("old content")
        .execute(&db.pool)
        .await
        .expect("insert old event");
    }

    async fn count_events(db: &ContextoDb) -> u64 {
        db.count_events(&EventFilter::new()).await.unwrap()
    }

    #[tokio::test]
    async fn test_prune_deletes_old_orphan_events() {
        let db = test_db().await;

        // Insert an event from 45 days ago (orphan, no task_id)
        insert_old_event(&db, "TERM", 45).await;
        assert_eq!(count_events(&db).await, 1);

        // Prune events older than 30 days
        let deleted = db.prune_expired_events(30).await.unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(count_events(&db).await, 0);
    }

    #[tokio::test]
    async fn test_prune_preserves_recent_events() {
        let db = test_db().await;

        // Insert a recent event (5 days ago)
        insert_old_event(&db, "TERM", 5).await;
        assert_eq!(count_events(&db).await, 1);

        let deleted = db.prune_expired_events(30).await.unwrap();
        assert_eq!(deleted, 0);
        assert_eq!(count_events(&db).await, 1);
    }

    #[tokio::test]
    async fn test_prune_preserves_manual_notes() {
        let db = test_db().await;

        // Manual (NOTE) events should never be pruned
        insert_old_event(&db, "NOTE", 60).await;
        assert_eq!(count_events(&db).await, 1);

        let deleted = db.prune_expired_events(1).await.unwrap(); // aggressive: 1 day
        assert_eq!(deleted, 0, "Manual notes must never be pruned");
        assert_eq!(count_events(&db).await, 1);
    }

    #[tokio::test]
    async fn test_prune_preserves_mcp_notes() {
        let db = test_db().await;

        // MCP events (AI-authored notes) should never be pruned
        insert_old_event(&db, "MCP", 60).await;
        assert_eq!(count_events(&db).await, 1);

        let deleted = db.prune_expired_events(1).await.unwrap();
        assert_eq!(deleted, 0, "MCP notes must never be pruned");
    }

    #[tokio::test]
    async fn test_prune_preserves_task_bound_events() {
        let db = test_db().await;

        // Create a task first
        let task = Task::new("my task");
        db.create_task(&task).await.unwrap();

        // Insert an old event linked to that task
        let ts = (chrono::Utc::now() - chrono::Duration::days(45)).to_rfc3339();
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO context_events (id, timestamp, source, label, content, task_id) VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(&id)
        .bind(&ts)
        .bind("TERM")
        .bind("task event")
        .bind("content")
        .bind(task.id.to_string())
        .execute(&db.pool)
        .await
        .unwrap();

        let deleted = db.prune_expired_events(30).await.unwrap();
        assert_eq!(deleted, 0, "Task-bound events must be preserved");
    }

    #[tokio::test]
    async fn test_prune_and_vacuum_combined() {
        let db = test_db().await;

        insert_old_event(&db, "TERM", 45).await;
        let deleted = db.prune_and_vacuum(30).await.unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(count_events(&db).await, 0);
    }

    #[tokio::test]
    async fn test_incremental_vacuum_does_not_panic_on_empty() {
        let db = test_db().await;
        // Should not error even on empty DB
        db.incremental_vacuum(100).await.unwrap();
    }
}
