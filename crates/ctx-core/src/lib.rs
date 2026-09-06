//! # ctx-core
//!
//! Core event types, ingestion ring buffer, and shared abstractions for the
//! Contexto Universal Developer Context Manager.
//!
//! ## Architecture
//!
//! This crate defines the canonical data model shared across all Contexto crates:
//! - `ContextEvent` — the atomic unit of captured developer context
//! - `EventSource` — where an event originated (terminal, IDE, git, etc.)
//! - `IngestionBuffer` — a `tokio::sync::mpsc` ring buffer that decouples
//!   high-frequency event ingestion from SQLite batch writes (prevents lock contention)
//!
//! ## Ring Buffer Design
//!
//! During active development, the IDE or terminal can fire 50–200 events/second.
//! Direct SQLite INSERTs at this rate cause `database is locked` / `SQLITE_BUSY`
//! errors. The ingestion buffer absorbs burst traffic and a background task
//! flushes to `ctx-db` every 500ms or when 50 events accumulate.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// =============================================================================
// Event Source
// =============================================================================

/// Where a context event originated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

// =============================================================================
// Context Event
// =============================================================================

/// The atomic unit of captured developer context.
///
/// Every piece of information flowing through Contexto is modeled as a
/// `ContextEvent`. Events are ingested via the ring buffer, batch-written to
/// SQLite, and indexed by FTS5 for instant search.
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
    /// May be truncated to `CTX_MAX_TERMINAL_EVENT_SIZE`
    pub content: String,

    /// Optional structured metadata (JSON blob)
    pub metadata: Option<serde_json::Value>,

    /// True if this content was scrubbed by the redaction engine
    pub was_redacted: bool,

    /// Working directory at the time of the event
    pub cwd: Option<String>,

    /// Git repository this event belongs to (if any)
    pub git_repo: Option<String>,

    /// Active task ID this event is associated with (if any)
    pub task_id: Option<Uuid>,
}

impl ContextEvent {
    /// Create a new event with the current timestamp and a fresh UUID.
    pub fn new(source: EventSource, label: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            source,
            label: label.into(),
            content: content.into(),
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

    /// Builder: mark that this event's content was redacted.
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

    /// Builder: associate with a git repository.
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
// Ingestion Ring Buffer
// =============================================================================

/// Capacity of the ingestion ring buffer.
/// At 200 events/sec burst, this provides 5 seconds of buffer headroom.
pub const RING_BUFFER_CAPACITY: usize = 1_000;

/// The sender half of the ingestion ring buffer.
/// Callers use this to submit events without blocking on SQLite.
pub type IngestionSender = tokio::sync::mpsc::Sender<ContextEvent>;

/// The receiver half of the ingestion ring buffer.
/// The background batch-writer task holds this and drains it to SQLite.
pub type IngestionReceiver = tokio::sync::mpsc::Receiver<ContextEvent>;

/// Create a new ingestion ring buffer with the standard capacity.
///
/// Returns `(sender, receiver)`. The sender is cloned and distributed to all
/// event sources. The receiver is held by the single background batch-writer.
pub fn ingestion_buffer() -> (IngestionSender, IngestionReceiver) {
    tokio::sync::mpsc::channel(RING_BUFFER_CAPACITY)
}

// =============================================================================
// Error Type
// =============================================================================

/// Errors that can occur in ctx-core.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("Ring buffer is full — ingestion dropped (capacity: {capacity})")]
    BufferFull { capacity: usize },

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
    fn test_event_construction() {
        let event = ContextEvent::new(EventSource::Terminal, "cargo build", "Finished in 2.3s")
            .with_cwd("/home/user/project")
            .with_redaction();

        assert_eq!(event.source, EventSource::Terminal);
        assert_eq!(event.label, "cargo build");
        assert!(event.was_redacted);
        assert_eq!(event.cwd.as_deref(), Some("/home/user/project"));
    }

    #[test]
    fn test_event_source_display() {
        assert_eq!(EventSource::Terminal.to_string(), "TERM");
        assert_eq!(EventSource::Git.to_string(), "GIT");
        assert_eq!(EventSource::Editor.to_string(), "IDE");
    }

    #[tokio::test]
    async fn test_ring_buffer_send_receive() {
        let (tx, mut rx) = ingestion_buffer();
        let event = ContextEvent::new(EventSource::Manual, "test", "hello world");
        let event_id = event.id;

        tx.send(event).await.expect("buffer should not be full");
        let received = rx.recv().await.expect("should receive event");
        assert_eq!(received.id, event_id);
    }
}
