//! # ctx-core
//!
//! Core event types, ingestion ring buffer, and shared abstractions for the
//! Contexto Universal Developer Context Manager.
//!
//! ## Architecture
//!
//! This crate defines the canonical data model shared across all Contexto crates:
//! - [`ContextEvent`] — the atomic unit of captured developer context
//! - [`EventSource`] — where an event originated (terminal, IDE, git, etc.)
//! - [`EventFilter`] — query parameters for filtering stored events
//! - [`Task`] — a named, time-bounded unit of developer work (Layer 2)
//! - [`ingestion_buffer`] — a `tokio::sync::mpsc` ring buffer decoupling
//!   high-frequency event ingestion from SQLite batch writes
//!
//! ## Ring Buffer Design
//!
//! During active development, the IDE or terminal can fire 50–200 events/second.
//! Direct SQLite INSERTs at this rate cause `database is locked` / `SQLITE_BUSY`
//! errors. The ingestion buffer absorbs burst traffic; a background `BatchWriter`
//! in `ctx-db` flushes every 500ms or when 50 events accumulate.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// =============================================================================
// Event Source
// =============================================================================

/// Where a context event originated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    /// Terminal / shell command output
    Terminal,
    /// VS Code editor activity (file open, save, edit, selection)
    Editor,
    /// Git operations (commit, diff, branch, merge)
    Git,
    /// File system changes (write, create, delete)
    FileSystem,
    /// Manual user annotation via CLI (`ctx remember`)
    Manual,
    /// MCP tool invocation result
    Mcp,
}

impl std::fmt::Display for EventSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Terminal => write!(f, "TERM"),
            Self::Editor => write!(f, "IDE"),
            Self::Git => write!(f, "GIT"),
            Self::FileSystem => write!(f, "FS"),
            Self::Manual => write!(f, "NOTE"),
            Self::Mcp => write!(f, "MCP"),
        }
    }
}

impl std::str::FromStr for EventSource {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "TERM" | "terminal" => Ok(Self::Terminal),
            "IDE" | "editor" => Ok(Self::Editor),
            "GIT" | "git" => Ok(Self::Git),
            "FS" | "file_system" => Ok(Self::FileSystem),
            "NOTE" | "manual" => Ok(Self::Manual),
            "MCP" | "mcp" => Ok(Self::Mcp),
            other => Err(CoreError::UnknownSource(other.to_string())),
        }
    }
}

// =============================================================================
// Context Event
// =============================================================================

/// The maximum allowed content size for a single event (bytes).
/// Matches the `CTX_MAX_TERMINAL_EVENT_SIZE` env var default.
pub const MAX_CONTENT_SIZE: usize = 8_192;

/// The atomic unit of captured developer context.
///
/// Every piece of information flowing through Contexto is a `ContextEvent`.
/// Events are ingested via the ring buffer, batch-written to SQLite, and
/// indexed by FTS5 for instant BM25-ranked search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEvent {
    /// Unique identifier for this event
    pub id: Uuid,

    /// When this event occurred (UTC)
    pub timestamp: DateTime<Utc>,

    /// Where this event came from
    pub source: EventSource,

    /// Short human-readable label (e.g. "cargo build", "main.rs saved")
    pub label: String,

    /// Raw captured content (command output, diff, file content, etc.)
    /// Truncated to `MAX_CONTENT_SIZE` bytes if needed.
    pub content: String,

    /// Optional structured metadata (JSON blob)
    pub metadata: Option<serde_json::Value>,

    /// True if this content was scrubbed by the redaction engine
    pub was_redacted: bool,

    /// Working directory at the time of the event
    pub cwd: Option<String>,

    /// Git repository root this event belongs to (if any)
    pub git_repo: Option<String>,

    /// Active task ID this event is associated with (if any)
    pub task_id: Option<Uuid>,
}

impl ContextEvent {
    /// Create a new event with the current timestamp and a fresh UUID.
    ///
    /// Content is automatically truncated to [`MAX_CONTENT_SIZE`] bytes
    /// to prevent runaway `npm install` output from flooding the database.
    pub fn new(source: EventSource, label: impl Into<String>, content: impl Into<String>) -> Self {
        let raw_content: String = content.into();
        let content = if raw_content.len() > MAX_CONTENT_SIZE {
            tracing::debug!(
                "Content truncated from {} to {} bytes",
                raw_content.len(),
                MAX_CONTENT_SIZE
            );
            raw_content[..MAX_CONTENT_SIZE].to_string()
        } else {
            raw_content
        };

        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            source,
            label: label.into(),
            content,
            metadata: None,
            was_redacted: false,
            cwd: None,
            git_repo: None,
            task_id: None,
        }
    }

    /// Builder: attach structured metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Builder: mark that this event's content was modified by the redaction engine.
    #[must_use]
    pub fn with_redaction(mut self) -> Self {
        self.was_redacted = true;
        self
    }

    /// Builder: set the working directory.
    #[must_use]
    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Builder: associate with a git repository root.
    #[must_use]
    pub fn with_git_repo(mut self, repo: impl Into<String>) -> Self {
        self.git_repo = Some(repo.into());
        self
    }

    /// Builder: associate with an active task.
    #[must_use]
    pub fn with_task(mut self, task_id: Uuid) -> Self {
        self.task_id = Some(task_id);
        self
    }
}

// =============================================================================
// Event Filter
// =============================================================================

/// Query parameters for filtering stored context events.
///
/// Used by `ctx-db`'s `get_recent()` and `search()` methods, and forwarded
/// from the REST API's query parameters.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct EventFilter {
    /// Filter to a specific event source (e.g. only `Git` events)
    pub source: Option<EventSource>,

    /// Filter to events from a specific git repository
    pub git_repo: Option<String>,

    /// Filter to events associated with a specific task
    pub task_id: Option<Uuid>,

    /// Only return events after this timestamp (UTC)
    pub since: Option<DateTime<Utc>>,

    /// Only return events before this timestamp (UTC)
    pub until: Option<DateTime<Utc>>,

    /// Maximum number of events to return (default: 20, max: 200)
    pub limit: Option<u32>,

    /// Offset for pagination (prefer cursor-based pagination in Phase 2+)
    pub offset: Option<u32>,
}

impl EventFilter {
    /// Create a new empty filter (returns everything up to the default limit).
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolved limit, clamped to sane bounds (1–200).
    pub fn resolved_limit(&self) -> u32 {
        self.limit.unwrap_or(20).clamp(1, 200)
    }

    /// Resolved offset (default: 0).
    pub fn resolved_offset(&self) -> u32 {
        self.offset.unwrap_or(0)
    }
}

// =============================================================================
// Task
// =============================================================================

/// Status of a tracked developer task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Currently capturing events
    Active,
    /// Temporarily paused
    Paused,
    /// Finished — eligible for LLM summarization
    Completed,
    /// Discarded without summarization
    Abandoned,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Paused => write!(f, "paused"),
            Self::Completed => write!(f, "completed"),
            Self::Abandoned => write!(f, "abandoned"),
        }
    }
}

/// A named, time-bounded unit of developer work (Layer 2 abstraction).
///
/// Tasks group related `ContextEvent`s together. When stopped, a task becomes
/// a candidate for LLM summarization (Phase 6+).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub started_at: DateTime<Utc>,
    pub stopped_at: Option<DateTime<Utc>>,
    pub status: TaskStatus,
    pub summary: Option<String>,
    pub git_repo: Option<String>,
    pub event_count: u32,
}

impl Task {
    /// Create a new active task.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            description: None,
            started_at: Utc::now(),
            stopped_at: None,
            status: TaskStatus::Active,
            summary: None,
            git_repo: None,
            event_count: 0,
        }
    }

    /// Duration of the task so far.
    pub fn duration(&self) -> chrono::Duration {
        let end = self.stopped_at.unwrap_or_else(Utc::now);
        end - self.started_at
    }
}

// =============================================================================
// Ingestion Ring Buffer
// =============================================================================

/// Capacity of the ingestion ring buffer.
/// At 200 events/sec burst, this provides ~5 seconds of buffer headroom.
pub const RING_BUFFER_CAPACITY: usize = 1_000;

/// The sender half of the ingestion ring buffer.
pub type IngestionSender = tokio::sync::mpsc::Sender<ContextEvent>;

/// The receiver half of the ingestion ring buffer.
pub type IngestionReceiver = tokio::sync::mpsc::Receiver<ContextEvent>;

/// Create a new ingestion ring buffer with the standard capacity.
///
/// Returns `(sender, receiver)`. Clone the sender for each event source.
/// The receiver is held by the single background `BatchWriter`.
pub fn ingestion_buffer() -> (IngestionSender, IngestionReceiver) {
    tokio::sync::mpsc::channel(RING_BUFFER_CAPACITY)
}

// =============================================================================
// Error Type
// =============================================================================

/// Errors that can occur in ctx-core.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("Ring buffer is full (capacity: {capacity}) — event dropped")]
    BufferFull { capacity: usize },

    #[error("Content exceeds maximum size: {size} bytes (max: {max})")]
    ContentTooLarge { size: usize, max: usize },

    #[error("Unknown event source: '{0}'")]
    UnknownSource(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_construction_and_builders() {
        let event = ContextEvent::new(EventSource::Terminal, "cargo build", "Finished in 2.3s")
            .with_cwd("/home/user/project")
            .with_git_repo("Contexto")
            .with_redaction();

        assert_eq!(event.source, EventSource::Terminal);
        assert_eq!(event.label, "cargo build");
        assert_eq!(event.content, "Finished in 2.3s");
        assert!(event.was_redacted);
        assert_eq!(event.cwd.as_deref(), Some("/home/user/project"));
        assert_eq!(event.git_repo.as_deref(), Some("Contexto"));
    }

    #[test]
    fn test_content_truncation() {
        let large = "x".repeat(MAX_CONTENT_SIZE + 100);
        let event = ContextEvent::new(EventSource::Terminal, "big", large);
        assert_eq!(event.content.len(), MAX_CONTENT_SIZE);
    }

    #[test]
    fn test_event_source_display() {
        assert_eq!(EventSource::Terminal.to_string(), "TERM");
        assert_eq!(EventSource::Git.to_string(), "GIT");
        assert_eq!(EventSource::Editor.to_string(), "IDE");
        assert_eq!(EventSource::Manual.to_string(), "NOTE");
        assert_eq!(EventSource::Mcp.to_string(), "MCP");
    }

    #[test]
    fn test_event_source_from_str() {
        assert_eq!(
            "TERM".parse::<EventSource>().unwrap(),
            EventSource::Terminal
        );
        assert_eq!("GIT".parse::<EventSource>().unwrap(), EventSource::Git);
        assert_eq!("IDE".parse::<EventSource>().unwrap(), EventSource::Editor);
        assert!("UNKNOWN".parse::<EventSource>().is_err());
    }

    #[test]
    fn test_event_filter_defaults() {
        let filter = EventFilter::new();
        assert_eq!(filter.resolved_limit(), 20);
        assert_eq!(filter.resolved_offset(), 0);
    }

    #[test]
    fn test_event_filter_limit_clamping() {
        let mut filter = EventFilter::new();
        filter.limit = Some(9999);
        assert_eq!(filter.resolved_limit(), 200);
        filter.limit = Some(0);
        assert_eq!(filter.resolved_limit(), 1);
    }

    #[test]
    fn test_task_creation() {
        let task = Task::new("Implement ring buffer");
        assert_eq!(task.name, "Implement ring buffer");
        assert_eq!(task.status, TaskStatus::Active);
        assert!(task.stopped_at.is_none());
        assert_eq!(task.event_count, 0);
    }

    #[test]
    fn test_task_duration() {
        let task = Task::new("test");
        let dur = task.duration();
        assert!(dur.num_milliseconds() >= 0);
    }

    #[tokio::test]
    async fn test_ring_buffer_send_receive() {
        let (tx, mut rx) = ingestion_buffer();
        let event = ContextEvent::new(EventSource::Manual, "test note", "hello world");
        let event_id = event.id;

        tx.send(event).await.expect("buffer should not be full");
        let received = rx.recv().await.expect("should receive event");
        assert_eq!(received.id, event_id);
    }

    #[tokio::test]
    async fn test_ring_buffer_multiple_senders() {
        let (tx, mut rx) = ingestion_buffer();
        let tx2 = tx.clone();

        tx.send(ContextEvent::new(EventSource::Terminal, "cmd1", "out1"))
            .await
            .unwrap();
        tx2.send(ContextEvent::new(EventSource::Git, "commit", "msg"))
            .await
            .unwrap();

        let e1 = rx.recv().await.unwrap();
        let e2 = rx.recv().await.unwrap();
        assert_eq!(e1.source, EventSource::Terminal);
        assert_eq!(e2.source, EventSource::Git);
    }
}
