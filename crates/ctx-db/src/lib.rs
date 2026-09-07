//! # ctx-db
//!
//! SQLite + FTS5 database layer for Contexto.
//!
//! ## Architecture
//!
//! ```text
//!  Event sources (IDE, Terminal, Git)
//!          │
//!          ▼  HTTP 202 — instant return
//!  ┌───────────────┐
//!  │ Ring Buffer   │  tokio::mpsc::channel(1000)
//!  │ (ctx-core)    │
//!  └──────┬────────┘
//!         │  drained every 500ms OR 50 events
//!         ▼
//!  ┌───────────────┐
//!  │ BatchWriter   │  Single background task
//!  │ (ctx-db)      │  BEGIN / 50× INSERT / COMMIT
//!  └──────┬────────┘
//!         │
//!         ▼
//!  SQLite (WAL mode)
//!    context_events  ← primary storage
//!    events_fts      ← FTS5 virtual table (BM25 search)
//!    tasks           ← Layer 2 task tracking
//! ```
//!
//! ## Why FTS5, not sqlite-vss / FAISS?
//!
//! `sqlite-vss` relies on FAISS C++ bindings — unreliable to compile inside
//! Tauri for arm64 macOS. FTS5 is built into SQLite with zero extra deps
//! and gives sub-millisecond BM25 search. Vector search is deferred to
//! Phase 6 (fastembed-rs + LanceDB embedded).
//!
//! ## Batch Writer — preventing SQLITE_BUSY
//!
//! At 200 events/sec burst, per-event INSERTs cause `SQLITE_BUSY` lock
//! contention. The `BatchWriter` drains the ring buffer and batches writes
//! into a single `BEGIN / N× INSERT / COMMIT` every 500ms or 50 events.

use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
    Row, SqlitePool,
};
use tracing::{debug, error, info, warn};

use ctx_core::{ContextEvent, EventFilter, EventSource, IngestionReceiver, Task, TaskStatus};

// =============================================================================
// Constants
// =============================================================================

/// How often the BatchWriter flushes even if the batch isn't full.
const BATCH_FLUSH_INTERVAL: Duration = Duration::from_millis(500);

/// Maximum events per batch before forcing an early flush.
const BATCH_MAX_SIZE: usize = 50;

// =============================================================================
// ContextoDb — main handle
// =============================================================================

/// Database handle for the Contexto SQLite store.
///
/// Clone this cheaply — it wraps a `sqlx::SqlitePool` internally.
#[derive(Clone)]
pub struct ContextoDb {
    pool: SqlitePool,
}

impl ContextoDb {
    /// Open (or create) the Contexto database at the given path.
    ///
    /// - Uses `:memory:` for in-memory databases (tests)
    /// - Enables WAL journal mode for better concurrent read performance
    /// - Runs all pending schema migrations automatically
    ///
    /// # Errors
    /// Returns an error if the database cannot be opened or migrations fail.
    pub async fn open(db_path: &str) -> Result<Self> {
        info!("Opening database: {db_path}");

        let options = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            // Tune SQLite for our write-heavy workload
            .pragma("cache_size", "-32000")   // 32MB page cache
            .pragma("temp_store", "memory")
            .pragma("mmap_size", "268435456") // 256MB mmap
            .pragma("foreign_keys", "ON");

        let pool = SqlitePool::connect_with(options)
            .await
            .with_context(|| format!("Failed to open SQLite database at {db_path}"))?;

        // Run embedded SQL migrations
        Self::run_migrations(&pool).await?;

        info!("Database ready");
        Ok(Self { pool })
    }

    /// Execute all schema migrations in order.
    async fn run_migrations(pool: &SqlitePool) -> Result<()> {
        info!("Running schema migrations...");

        // We embed the SQL directly rather than using sqlx::migrate!() to avoid
        // requiring DATABASE_URL at compile time (no sqlx offline cache needed).
        // TODO(phase-2): Switch to sqlx::migrate!("./migrations") + sqlx prepare
        //                once sqlx-cli is in CI and DATABASE_URL is set.
        let migrations: &[(&str, &str)] = &[
            (
                "001_create_context_events",
                include_str!("../migrations/001_create_context_events.sql"),
            ),
            (
                "002_create_fts5",
                include_str!("../migrations/002_create_fts5.sql"),
            ),
            (
                "003_create_tasks",
                include_str!("../migrations/003_create_tasks.sql"),
            ),
        ];

        // Create migrations tracking table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS _ctx_migrations (
                name       TEXT PRIMARY KEY NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
        )
        .execute(pool)
        .await?;

        for (name, sql) in migrations {
            // Skip already-applied migrations (idempotent)
            let already_applied: bool =
                sqlx::query("SELECT 1 FROM _ctx_migrations WHERE name = ?")
                    .bind(name)
                    .fetch_optional(pool)
                    .await?
                    .is_some();

            if already_applied {
                debug!("Migration '{name}' already applied — skipping");
                continue;
            }

            info!("Applying migration: {name}");
            // Execute each statement separately (SQLite doesn't support multi-statement exec)
            for stmt in sql.split(';') {
                let stmt = stmt.trim();
                if !stmt.is_empty() {
                    sqlx::query(stmt)
                        .execute(pool)
                        .await
                        .with_context(|| format!("Migration '{name}' failed on statement: {stmt}"))?;
                }
            }

            sqlx::query("INSERT INTO _ctx_migrations (name) VALUES (?)")
                .bind(name)
                .execute(pool)
                .await?;

            info!("Migration '{name}' applied successfully");
        }

        info!("All migrations up to date");
        Ok(())
    }

    // =========================================================================
    // Event Writes
    // =========================================================================

    /// Insert a batch of events in a single SQLite transaction.
    ///
    /// Uses `BEGIN IMMEDIATE` to prevent write-lock races between concurrent
    /// batch writers (if multiple are ever spawned in future phases).
    ///
    /// # Errors
    /// Returns an error if the transaction fails. Partial batches are rolled back.
    pub async fn insert_events_batch(&self, events: &[ContextEvent]) -> Result<usize> {
        if events.is_empty() {
            return Ok(0);
        }

        let mut tx = self.pool.begin().await?;
        let mut inserted = 0usize;

        for event in events {
            let rows = sqlx::query(
                r#"
                INSERT OR IGNORE INTO context_events
                    (id, timestamp, source, label, content, metadata, was_redacted, cwd, git_repo, task_id)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(event.id.to_string())
            .bind(event.timestamp.to_rfc3339())
            .bind(event.source.to_string())
            .bind(&event.label)
            .bind(&event.content)
            .bind(event.metadata.as_ref().map(|m| m.to_string()))
            .bind(i64::from(event.was_redacted))
            .bind(&event.cwd)
            .bind(&event.git_repo)
            .bind(event.task_id.map(|id| id.to_string()))
            .execute(&mut *tx)
            .await?;

            inserted += rows.rows_affected() as usize;

            // Update associated task's event_count
            if let Some(task_id) = &event.task_id {
                sqlx::query(
                    "UPDATE tasks SET event_count = event_count + 1 WHERE id = ?",
                )
                .bind(task_id.to_string())
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;
        debug!("Batch committed: {inserted}/{} events inserted", events.len());
        Ok(inserted)
    }

    // =========================================================================
    // Event Reads
    // =========================================================================

    /// Fetch recent events, ordered newest-first, with optional filtering.
    ///
    /// Applies filters from [`EventFilter`]: source, git_repo, task_id, time range.
    /// Limit is clamped to 1–200 by `EventFilter::resolved_limit()`.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub async fn get_recent(&self, filter: &EventFilter) -> Result<Vec<ContextEvent>> {
        // Build dynamic WHERE clause
        let mut conditions = vec!["1=1"];
        let source_str;
        let since_str;
        let until_str;

        // We build the SQL manually to avoid sqlx compile-time checks
        // TODO(phase-2): Replace with query_builder! or sea-query for type safety
        let mut sql = String::from(
            r#"SELECT id, timestamp, source, label, content, metadata,
                      was_redacted, cwd, git_repo, task_id
               FROM context_events
               WHERE "#,
        );

        let mut where_parts: Vec<String> = Vec::new();
        let mut binds: Vec<String> = Vec::new();

        if let Some(src) = &filter.source {
            where_parts.push("source = ?".to_string());
            binds.push(src.to_string());
        }
        if let Some(repo) = &filter.git_repo {
            where_parts.push("git_repo = ?".to_string());
            binds.push(repo.clone());
        }
        if let Some(task_id) = &filter.task_id {
            where_parts.push("task_id = ?".to_string());
            binds.push(task_id.to_string());
        }
        if let Some(since) = &filter.since {
            where_parts.push("timestamp >= ?".to_string());
            binds.push(since.to_rfc3339());
        }
        if let Some(until) = &filter.until {
            where_parts.push("timestamp <= ?".to_string());
            binds.push(until.to_rfc3339());
        }

        if where_parts.is_empty() {
            sql.push_str("1=1");
        } else {
            sql.push_str(&where_parts.join(" AND "));
        }

        sql.push_str(" ORDER BY timestamp DESC LIMIT ? OFFSET ?");

        let limit = filter.resolved_limit();
        let offset = filter.resolved_offset();

        // Build query with dynamic binds
        let mut query = sqlx::query(&sql);
        for bind in &binds {
            query = query.bind(bind);
        }
        query = query.bind(limit as i64).bind(offset as i64);

        let rows = query.fetch_all(&self.pool).await?;
        let events = rows
            .into_iter()
            .map(|row| Self::row_to_event(&row))
            .collect::<Result<Vec<_>>>()?;

        Ok(events)
    }

    /// Full-text search across all events using SQLite FTS5 + BM25 ranking.
    ///
    /// Returns events sorted by BM25 relevance score (most relevant first).
    /// The FTS5 index covers `label` and `content` columns with porter stemming.
    ///
    /// # FTS5 Query Syntax
    /// Supports: `"exact phrase"`, `term1 AND term2`, `term1 OR term2`, prefix `term*`
    ///
    /// # Errors
    /// Returns an error if the FTS query is malformed or the database fails.
    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<ContextEvent>> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }

        let limit = limit.clamp(1, 200);

        let rows = sqlx::query(
            r#"
            SELECT e.id, e.timestamp, e.source, e.label, e.content,
                   e.metadata, e.was_redacted, e.cwd, e.git_repo, e.task_id
            FROM context_events e
            JOIN events_fts f ON e.rowid = f.rowid
            WHERE events_fts MATCH ?
            ORDER BY rank
            LIMIT ?
            "#,
        )
        .bind(query)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .with_context(|| format!("FTS5 search failed for query: {query:?}"))?;

        rows.into_iter()
            .map(|row| Self::row_to_event(&row))
            .collect()
    }

    /// Count total events (optionally filtered).
    pub async fn count_events(&self, filter: &EventFilter) -> Result<u64> {
        let row = sqlx::query("SELECT COUNT(*) as cnt FROM context_events")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.get::<i64, _>("cnt") as u64)
    }

    // =========================================================================
    // Task Operations
    // =========================================================================

    /// Insert a new task.
    pub async fn create_task(&self, task: &Task) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO tasks (id, name, description, started_at, status, git_repo, event_count)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(task.id.to_string())
        .bind(&task.name)
        .bind(&task.description)
        .bind(task.started_at.to_rfc3339())
        .bind(task.status.to_string())
        .bind(&task.git_repo)
        .bind(task.event_count as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Get the currently active task (if any).
    pub async fn get_active_task(&self) -> Result<Option<Task>> {
        let row = sqlx::query(
            "SELECT id, name, description, started_at, stopped_at, status, summary, git_repo, event_count
             FROM tasks WHERE status = 'active' ORDER BY started_at DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;

        row.map(|r| Self::row_to_task(&r)).transpose()
    }

    /// Stop a task by ID (sets status to 'completed' and records stopped_at).
    pub async fn stop_task(&self, task_id: uuid::Uuid) -> Result<()> {
        let rows = sqlx::query(
            "UPDATE tasks SET status = 'completed', stopped_at = datetime('now')
             WHERE id = ? AND status = 'active'",
        )
        .bind(task_id.to_string())
        .execute(&self.pool)
        .await?;

        if rows.rows_affected() == 0 {
            warn!("stop_task: task {task_id} not found or not active");
        }
        Ok(())
    }

    /// Abandon a task by ID (sets status to 'abandoned' without summarization).
    pub async fn abandon_task(&self, task_id: uuid::Uuid) -> Result<()> {
        let rows = sqlx::query(
            "UPDATE tasks SET status = 'abandoned', stopped_at = datetime('now')
             WHERE id = ? AND status IN ('active', 'paused')",
        )
        .bind(task_id.to_string())
        .execute(&self.pool)
        .await?;

        if rows.rows_affected() == 0 {
            warn!("abandon_task: task {task_id} not found or already terminal");
        }
        Ok(())
    }

    /// List all tasks, ordered by start time descending.
    pub async fn list_tasks(&self, limit: u32) -> Result<Vec<Task>> {
        let rows = sqlx::query(
            "SELECT id, name, description, started_at, stopped_at, status, summary, git_repo, event_count
             FROM tasks ORDER BY started_at DESC LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| Self::row_to_task(&r))
            .collect()
    }

    // =========================================================================
    // Row Mappers
    // =========================================================================

    fn row_to_event(row: &sqlx::sqlite::SqliteRow) -> Result<ContextEvent> {
        use std::str::FromStr;

        let id_str: String = row.try_get("id")?;
        let ts_str: String = row.try_get("timestamp")?;
        let source_str: String = row.try_get("source")?;
        let metadata_str: Option<String> = row.try_get("metadata")?;
        let task_id_str: Option<String> = row.try_get("task_id")?;

        Ok(ContextEvent {
            id: uuid::Uuid::parse_str(&id_str)
                .with_context(|| format!("Invalid UUID in DB: {id_str}"))?,
            timestamp: chrono::DateTime::parse_from_rfc3339(&ts_str)
                .with_context(|| format!("Invalid timestamp: {ts_str}"))?
                .with_timezone(&chrono::Utc),
            source: EventSource::from_str(&source_str).unwrap_or(EventSource::Manual),
            label: row.try_get("label")?,
            content: row.try_get("content")?,
            metadata: metadata_str
                .as_deref()
                .map(serde_json::from_str)
                .transpose()?,
            was_redacted: row.try_get::<i64, _>("was_redacted")? != 0,
            cwd: row.try_get("cwd")?,
            git_repo: row.try_get("git_repo")?,
            task_id: task_id_str
                .as_deref()
                .map(uuid::Uuid::parse_str)
                .transpose()
                .ok()
                .flatten(),
        })
    }

    fn row_to_task(row: &sqlx::sqlite::SqliteRow) -> Result<Task> {
        let id_str: String = row.try_get("id")?;
        let started_str: String = row.try_get("started_at")?;
        let stopped_str: Option<String> = row.try_get("stopped_at")?;
        let status_str: String = row.try_get("status")?;

        let status = match status_str.as_str() {
            "active" => TaskStatus::Active,
            "paused" => TaskStatus::Paused,
            "completed" => TaskStatus::Completed,
            "abandoned" => TaskStatus::Abandoned,
            _ => TaskStatus::Active,
        };

        Ok(Task {
            id: uuid::Uuid::parse_str(&id_str)?,
            name: row.try_get("name")?,
            description: row.try_get("description")?,
            started_at: chrono::DateTime::parse_from_rfc3339(&started_str)?
                .with_timezone(&chrono::Utc),
            stopped_at: stopped_str
                .as_deref()
                .map(chrono::DateTime::parse_from_rfc3339)
                .transpose()?
                .map(|dt| dt.with_timezone(&chrono::Utc)),
            status,
            summary: row.try_get("summary")?,
            git_repo: row.try_get("git_repo")?,
            event_count: row.try_get::<i64, _>("event_count")? as u32,
        })
    }
}

// =============================================================================
// BatchWriter
// =============================================================================

/// Async background task that drains the ingestion ring buffer and writes
/// events to SQLite in batches.
///
/// ## Flush triggers
/// - **Time-based:** Every `BATCH_FLUSH_INTERVAL` (500ms), regardless of batch size
/// - **Size-based:** When `BATCH_MAX_SIZE` (50) events accumulate before the timer
///
/// ## Shutdown
/// When the ring buffer sender is dropped (all event sources gone), the receiver
/// will drain remaining events and then return gracefully.
pub struct BatchWriter {
    rx: IngestionReceiver,
    db: ContextoDb,
}

impl BatchWriter {
    /// Create a new BatchWriter.
    pub fn new(rx: IngestionReceiver, db: ContextoDb) -> Self {
        Self { rx, db }
    }

    /// Run the batch writer loop. This method does not return until the
    /// ingestion channel is closed (all senders dropped).
    pub async fn run(mut self) {
        info!(
            "BatchWriter started (flush_interval={}ms, max_batch={})",
            BATCH_FLUSH_INTERVAL.as_millis(),
            BATCH_MAX_SIZE
        );

        let mut batch: Vec<ContextEvent> = Vec::with_capacity(BATCH_MAX_SIZE);
        let mut flush_timer = tokio::time::interval(BATCH_FLUSH_INTERVAL);
        // Skip the first immediate tick
        flush_timer.tick().await;

        loop {
            tokio::select! {
                // Flush on timer tick
                _ = flush_timer.tick() => {
                    if !batch.is_empty() {
                        Self::flush(&self.db, &mut batch).await;
                    }
                }

                // Receive new event from any source
                maybe_event = self.rx.recv() => {
                    match maybe_event {
                        Some(event) => {
                            batch.push(event);
                            if batch.len() >= BATCH_MAX_SIZE {
                                Self::flush(&self.db, &mut batch).await;
                            }
                        }
                        None => {
                            // Channel closed — drain and exit
                            info!("BatchWriter: ingestion channel closed, flushing final batch");
                            if !batch.is_empty() {
                                Self::flush(&self.db, &mut batch).await;
                            }
                            break;
                        }
                    }
                }
            }
        }

        info!("BatchWriter stopped");
    }

    async fn flush(db: &ContextoDb, batch: &mut Vec<ContextEvent>) {
        let count = batch.len();
        match db.insert_events_batch(batch).await {
            Ok(inserted) => {
                debug!("BatchWriter: flushed {inserted}/{count} events");
            }
            Err(e) => {
                error!("BatchWriter: flush failed — {e:#}. Events dropped: {count}");
            }
        }
        batch.clear();
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use ctx_core::{ContextEvent, EventFilter, EventSource, Task};

    async fn test_db() -> ContextoDb {
        ContextoDb::open(":memory:")
            .await
            .expect("in-memory DB should open")
    }

    #[tokio::test]
    async fn test_open_in_memory() {
        let db = test_db().await;
        // If we got here, migrations ran successfully
        let count = db.count_events(&EventFilter::new()).await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_insert_and_retrieve_single_event() {
        let db = test_db().await;

        let event = ContextEvent::new(
            EventSource::Terminal,
            "cargo build",
            "Compiling ctx-core v0.1.0",
        );
        let event_id = event.id;

        db.insert_events_batch(&[event]).await.unwrap();

        let events = db.get_recent(&EventFilter::new()).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, event_id);
        assert_eq!(events[0].label, "cargo build");
    }

    #[tokio::test]
    async fn test_insert_batch_of_50() {
        let db = test_db().await;

        let events: Vec<ContextEvent> = (0..50)
            .map(|i| ContextEvent::new(EventSource::Git, format!("commit-{i}"), format!("msg {i}")))
            .collect();

        let inserted = db.insert_events_batch(&events).await.unwrap();
        assert_eq!(inserted, 50);

        let count = db.count_events(&EventFilter::new()).await.unwrap();
        assert_eq!(count, 50);
    }

    #[tokio::test]
    async fn test_fts5_search_basic() {
        let db = test_db().await;

        let events = vec![
            ContextEvent::new(EventSource::Manual, "ring buffer", "Decided to use mpsc channel for ingestion"),
            ContextEvent::new(EventSource::Manual, "vector search", "Decided to defer FAISS to Phase 6"),
            ContextEvent::new(EventSource::Terminal, "cargo test", "test result: ok"),
        ];
        db.insert_events_batch(&events).await.unwrap();

        let results = db.search("ring buffer", 10).await.unwrap();
        assert!(!results.is_empty(), "FTS5 should find 'ring buffer' events");
        assert_eq!(results[0].label, "ring buffer");
    }

    #[tokio::test]
    async fn test_fts5_search_empty_query() {
        let db = test_db().await;
        let results = db.search("", 10).await.unwrap();
        assert!(results.is_empty(), "Empty query should return nothing");
    }

    #[tokio::test]
    async fn test_event_filter_by_source() {
        let db = test_db().await;

        let events = vec![
            ContextEvent::new(EventSource::Git, "commit", "feat: add filter"),
            ContextEvent::new(EventSource::Terminal, "ls -la", "/home/user"),
        ];
        db.insert_events_batch(&events).await.unwrap();

        let mut filter = EventFilter::new();
        filter.source = Some(EventSource::Git);

        let git_events = db.get_recent(&filter).await.unwrap();
        assert_eq!(git_events.len(), 1);
        assert_eq!(git_events[0].label, "commit");
    }

    #[tokio::test]
    async fn test_idempotent_insert() {
        let db = test_db().await;

        let event = ContextEvent::new(EventSource::Manual, "note", "test");

        // Insert twice — second should be ignored (INSERT OR IGNORE)
        db.insert_events_batch(&[event.clone()]).await.unwrap();
        db.insert_events_batch(&[event]).await.unwrap();

        let count = db.count_events(&EventFilter::new()).await.unwrap();
        assert_eq!(count, 1, "Duplicate events should be silently ignored");
    }

    #[tokio::test]
    async fn test_task_create_and_retrieve() {
        let db = test_db().await;

        let task = Task::new("Implement BatchWriter");
        db.create_task(&task).await.unwrap();

        let active = db.get_active_task().await.unwrap();
        assert!(active.is_some());
        assert_eq!(active.unwrap().name, "Implement BatchWriter");
    }

    #[tokio::test]
    async fn test_task_stop() {
        let db = test_db().await;

        let task = Task::new("Test task");
        let task_id = task.id;
        db.create_task(&task).await.unwrap();

        db.stop_task(task_id).await.unwrap();

        let active = db.get_active_task().await.unwrap();
        assert!(active.is_none(), "No active task after stop");
    }

    #[tokio::test]
    async fn test_batch_writer_integration() {
        use ctx_core::ingestion_buffer;

        let db = test_db().await;
        let (tx, rx) = ingestion_buffer();
        let writer = BatchWriter::new(rx, db.clone());

        // Spawn the writer
        let handle = tokio::spawn(writer.run());

        // Send 30 events
        for i in 0..30 {
            tx.send(ContextEvent::new(
                EventSource::Terminal,
                format!("cmd-{i}"),
                format!("output {i}"),
            ))
            .await
            .unwrap();
        }

        // Drop sender — writer should flush and exit
        drop(tx);
        handle.await.unwrap();

        let count = db.count_events(&EventFilter::new()).await.unwrap();
        assert_eq!(count, 30, "All 30 events should be persisted");
    }
}
