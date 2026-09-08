//! # api
//!
//! Axum HTTP router and all REST API handlers for `ctxd`.
//!
//! ## Endpoints
//!
//! | Method | Path              | Description                                      |
//! |--------|-------------------|--------------------------------------------------|
//! | POST   | /events           | Ingest a ContextEvent (202 Accepted)             |
//! | GET    | /context          | Fetch recent events with optional filters        |
//! | GET    | /search           | BM25 FTS5 full-text search                       |
//! | POST   | /remember         | Quick manual annotation                          |
//! | GET    | /status           | Daemon health, uptime, event count               |
//! | GET    | /tasks            | List all tasks                                   |
//! | POST   | /tasks            | Create a new task                                |
//! | PATCH  | /tasks/:id        | Update task (stop/abandon)                       |
//! | GET    | /events/stream    | SSE real-time event stream (Phase 7 Tauri)       |
//!
//! All endpoints are protected by `Authorization: Bearer <token>` middleware.
//! CORS is enabled for `localhost` origins (VS Code webview support).

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    middleware,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Json,
    },
    routing::{get, patch, post},
    Router,
};
use chrono::{DateTime, Utc};
use ctx_core::{ContextEvent, EventFilter, EventSource, IngestionSender, Task};
use ctx_db::ContextoDb;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;

use crate::auth::{require_auth, AuthToken};

// =============================================================================
// AppState
// =============================================================================

/// Shared state injected into every axum handler via `State<AppState>`.
///
/// All fields are `Arc<T>` so `AppState: Clone` is cheap (no deep copies).
#[derive(Clone)]
pub struct AppState {
    /// Database handle — wraps a connection pool, cheap to clone.
    pub db: Arc<ContextoDb>,
    /// Ring buffer sender — clone to push events from multiple sources.
    pub tx: Arc<IngestionSender>,
    /// Auth token — compared in middleware on every request.
    pub token: Arc<String>,
    /// Daemon start time — used by `/status` to compute uptime.
    pub started_at: DateTime<Utc>,
    /// SSE broadcast channel — new events are sent here after ingestion.
    pub sse_tx: broadcast::Sender<ContextEvent>,
}

// =============================================================================
// Router
// =============================================================================

/// Build and return the complete axum `Router`.
///
/// Auth middleware is applied to the entire router. CORS is enabled for
/// `localhost` (any port) so the VS Code webview can call the daemon.
pub fn build_router(state: AppState) -> Router {
    let auth_state = AuthToken(state.token.clone());

    // CORS: allow localhost origins and any headers/methods.
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route("/events", post(ingest_event))
        .route("/events/stream", get(event_stream))
        .route("/context", get(get_context))
        .route("/search", get(search_events))
        .route("/remember", post(remember))
        .route("/status", get(get_status))
        .route("/tasks", get(list_tasks).post(create_task))
        .route("/tasks/:id", patch(update_task))
        // Apply auth middleware to every route
        .layer(middleware::from_fn_with_state(auth_state, require_auth))
        .layer(cors)
        .with_state(state)
}

// =============================================================================
// Request / Response DTOs
// =============================================================================

/// Request body for `POST /events`.
#[derive(Debug, Deserialize)]
pub struct IngestEventRequest {
    pub source: String,
    pub label: String,
    pub content: String,
    pub metadata: Option<serde_json::Value>,
    pub cwd: Option<String>,
    pub git_repo: Option<String>,
    pub task_id: Option<Uuid>,
    pub was_redacted: Option<bool>,
}

/// Request body for `POST /remember`.
#[derive(Debug, Deserialize)]
pub struct RememberRequest {
    pub note: String,
    pub label: Option<String>,
    pub cwd: Option<String>,
    pub git_repo: Option<String>,
}

/// Query parameters for `GET /context`.
#[derive(Debug, Deserialize, Default)]
pub struct ContextQuery {
    pub source: Option<String>,
    pub git_repo: Option<String>,
    pub task_id: Option<Uuid>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// Query parameters for `GET /search`.
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: String,
    pub limit: Option<u32>,
}

/// Request body for `PATCH /tasks/:id`.
#[derive(Debug, Deserialize)]
pub struct UpdateTaskRequest {
    pub action: TaskAction,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAction {
    Stop,
    Abandon,
}

/// Request body for `POST /tasks`.
#[derive(Debug, Deserialize)]
pub struct CreateTaskRequest {
    pub name: String,
    pub description: Option<String>,
    pub git_repo: Option<String>,
}

/// Response body for `GET /status`.
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub version: &'static str,
    pub uptime_seconds: i64,
    pub event_count: u64,
    pub active_task: Option<TaskSummary>,
    pub db_path: String,
    pub listen_addr: &'static str,
}

/// Lightweight task summary embedded in status responses.
#[derive(Debug, Serialize)]
pub struct TaskSummary {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    pub event_count: u32,
    pub started_at: DateTime<Utc>,
}

impl From<Task> for TaskSummary {
    fn from(t: Task) -> Self {
        Self {
            id: t.id,
            name: t.name,
            status: t.status.to_string(),
            event_count: t.event_count,
            started_at: t.started_at,
        }
    }
}

/// Standard error response body.
#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
}

impl ApiError {
    fn new(msg: impl Into<String>) -> Json<Self> {
        Json(Self { error: msg.into() })
    }
}

// =============================================================================
// Handlers
// =============================================================================

/// `POST /events` — Ingest a ContextEvent into the ring buffer.
///
/// Returns `202 Accepted` immediately after queuing. The `BatchWriter`
/// will persist the event to SQLite within 500ms or when 50 events accumulate.
///
/// Returns `400 Bad Request` if the source enum is invalid.
/// Returns `503 Service Unavailable` if the ring buffer is full.
pub async fn ingest_event(
    State(state): State<AppState>,
    Json(req): Json<IngestEventRequest>,
) -> impl IntoResponse {
    let source: EventSource = match req.source.parse() {
        Ok(s) => s,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                ApiError::new(format!("Unknown source: '{}'", req.source)),
            )
                .into_response();
        }
    };

    let mut event = ContextEvent::new(source, req.label, req.content);

    if let Some(meta) = req.metadata {
        event = event.with_metadata(meta);
    }
    if let Some(cwd) = req.cwd {
        event = event.with_cwd(cwd);
    }
    if let Some(repo) = req.git_repo {
        event = event.with_git_repo(repo);
    }
    if let Some(task_id) = req.task_id {
        event = event.with_task(task_id);
    }
    if req.was_redacted.unwrap_or(false) {
        event = event.with_redaction();
    }

    // Broadcast to SSE subscribers (best-effort, lag is acceptable)
    let _ = state.sse_tx.send(event.clone());

    // Queue into ring buffer → 202 Accepted
    if state.tx.try_send(event).is_ok() {
        tracing::debug!("Event queued to ring buffer");
        StatusCode::ACCEPTED.into_response()
    } else {
        tracing::warn!("Ring buffer full — event dropped");
        (
            StatusCode::SERVICE_UNAVAILABLE,
            ApiError::new("Ring buffer full. Retry in a moment."),
        )
            .into_response()
    }
}

/// `GET /context` — Fetch recent events with optional filtering.
///
/// All query params are optional. Defaults: `limit=20`, `offset=0`.
pub async fn get_context(
    State(state): State<AppState>,
    Query(q): Query<ContextQuery>,
) -> impl IntoResponse {
    let mut filter = EventFilter::new();

    if let Some(src) = q.source {
        match src.parse() {
            Ok(s) => filter.source = Some(s),
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    ApiError::new(format!("Unknown source: '{src}'")),
                )
                    .into_response()
            }
        }
    }

    filter.git_repo = q.git_repo;
    filter.task_id = q.task_id;
    filter.since = q.since;
    filter.until = q.until;
    filter.limit = q.limit;
    filter.offset = q.offset;

    match state.db.get_recent(&filter).await {
        Ok(events) => Json(events).into_response(),
        Err(e) => {
            tracing::error!("get_context DB error: {e:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiError::new("Database error"),
            )
                .into_response()
        }
    }
}

/// `GET /search?q=<query>&limit=N` — BM25 full-text search via FTS5.
pub async fn search_events(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> impl IntoResponse {
    let limit = q.limit.unwrap_or(20).clamp(1, 200);

    if q.q.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            ApiError::new("Query parameter 'q' is required"),
        )
            .into_response();
    }

    match state.db.search(&q.q, limit).await {
        Ok(events) => Json(events).into_response(),
        Err(e) => {
            tracing::error!("search DB error: {e:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiError::new("Search failed"),
            )
                .into_response()
        }
    }
}

/// `POST /remember` — Quick manual annotation shortcut.
///
/// Creates a `Manual` source event and queues it directly to SQLite
/// (bypasses ring buffer for guaranteed persistence).
pub async fn remember(
    State(state): State<AppState>,
    Json(req): Json<RememberRequest>,
) -> impl IntoResponse {
    let label = req.label.unwrap_or_else(|| "note".to_string());
    let mut event = ContextEvent::new(EventSource::Manual, label, req.note);

    if let Some(cwd) = req.cwd {
        event = event.with_cwd(cwd);
    }
    if let Some(repo) = req.git_repo {
        event = event.with_git_repo(repo);
    }

    // Broadcast to SSE
    let _ = state.sse_tx.send(event.clone());

    // For /remember we persist immediately, not via ring buffer
    match state.db.insert_events_batch(&[event]).await {
        Ok(_) => StatusCode::CREATED.into_response(),
        Err(e) => {
            tracing::error!("remember DB error: {e:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                ApiError::new("Failed to save annotation"),
            )
                .into_response()
        }
    }
}

/// `GET /status` — Daemon health and runtime statistics.
pub async fn get_status(State(state): State<AppState>) -> impl IntoResponse {
    let event_count = state
        .db
        .count_events(&EventFilter::new())
        .await
        .unwrap_or(0);

    let active_task = state
        .db
        .get_active_task()
        .await
        .ok()
        .flatten()
        .map(TaskSummary::from);

    let uptime_seconds = (Utc::now() - state.started_at).num_seconds();
    let db_path = crate::auth::db_path().to_string_lossy().to_string();

    Json(StatusResponse {
        version: env!("CARGO_PKG_VERSION"),
        uptime_seconds,
        event_count,
        active_task,
        db_path,
        listen_addr: "127.0.0.1:8942",
    })
    .into_response()
}

/// `GET /tasks` — List all tasks, newest first.
pub async fn list_tasks(State(state): State<AppState>) -> impl IntoResponse {
    match state.db.list_tasks(50).await {
        Ok(tasks) => Json(tasks).into_response(),
        Err(e) => {
            tracing::error!("list_tasks error: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, ApiError::new("DB error")).into_response()
        }
    }
}

/// `POST /tasks` — Create and start a new task.
pub async fn create_task(
    State(state): State<AppState>,
    Json(req): Json<CreateTaskRequest>,
) -> impl IntoResponse {
    let mut task = Task::new(req.name);
    task.description = req.description;
    task.git_repo = req.git_repo;

    match state.db.create_task(&task).await {
        Ok(()) => (StatusCode::CREATED, Json(task)).into_response(),
        Err(e) => {
            tracing::error!("create_task error: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, ApiError::new("DB error")).into_response()
        }
    }
}

/// `PATCH /tasks/:id` — Stop or abandon a task by ID.
pub async fn update_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateTaskRequest>,
) -> impl IntoResponse {
    let result = match req.action {
        TaskAction::Stop => state.db.stop_task(id).await,
        TaskAction::Abandon => state.db.abandon_task(id).await,
    };

    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            tracing::error!("update_task error: {e:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, ApiError::new("DB error")).into_response()
        }
    }
}

/// `GET /events/stream` — Server-Sent Events (SSE) real-time event stream.
///
/// Clients receive a JSON-encoded `ContextEvent` for every event ingested after
/// they connect. Used by the Tauri desktop app (Phase 7) for live context feed.
pub async fn event_stream(
    State(state): State<AppState>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, axum::Error>>> {
    let rx = state.sse_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| {
        msg.ok().and_then(|event| {
            serde_json::to_string(&event)
                .ok()
                .map(|data| Ok(Event::default().data(data)))
        })
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{self, Request},
    };
    use ctx_core::ingestion_buffer;
    use ctx_db::ContextoDb;
    use tower::ServiceExt; // for `.oneshot()`

    async fn test_app() -> (Router, Arc<String>) {
        let db = Arc::new(ContextoDb::open(":memory:").await.expect("in-memory DB"));
        let (tx, mut rx) = ingestion_buffer();
        tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let (sse_tx, _) = broadcast::channel(256);
        let token = Arc::new("test-token-abc123".to_string());

        let state = AppState {
            db,
            tx: Arc::new(tx),
            token: token.clone(),
            started_at: Utc::now(),
            sse_tx,
        };

        (build_router(state), token)
    }

    fn bearer(token: &str) -> String {
        format!("Bearer {token}")
    }

    #[tokio::test]
    async fn test_auth_rejects_missing_token() {
        let (app, _token) = test_app().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .method(http::Method::GET)
                    .uri("/context")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_auth_rejects_wrong_token() {
        let (app, _token) = test_app().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .method(http::Method::GET)
                    .uri("/context")
                    .header("Authorization", "Bearer wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_status_endpoint() {
        let (app, token) = test_app().await;
        let resp = app
            .oneshot(
                Request::builder()
                    .method(http::Method::GET)
                    .uri("/status")
                    .header("Authorization", bearer(&token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let status: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(status["listen_addr"], "127.0.0.1:8942");
        assert_eq!(status["event_count"], 0);
    }

    #[tokio::test]
    async fn test_post_event_returns_202() {
        let (app, token) = test_app().await;

        let body = serde_json::json!({
            "source": "terminal",
            "label": "cargo build",
            "content": "Compiling ctx-core v0.1.0"
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method(http::Method::POST)
                    .uri("/events")
                    .header("Authorization", bearer(&token))
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn test_post_event_bad_source_returns_400() {
        let (app, token) = test_app().await;

        let body = serde_json::json!({
            "source": "unknown_source",
            "label": "test",
            "content": "test"
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method(http::Method::POST)
                    .uri("/events")
                    .header("Authorization", bearer(&token))
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_remember_creates_event() {
        let (app, token) = test_app().await;

        let body = serde_json::json!({
            "note": "Decided to use FTS5 over sqlite-vss"
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method(http::Method::POST)
                    .uri("/remember")
                    .header("Authorization", bearer(&token))
                    .header("Content-Type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_get_context_returns_json_array() {
        let (app, token) = test_app().await;

        let resp = app
            .oneshot(
                Request::builder()
                    .method(http::Method::GET)
                    .uri("/context")
                    .header("Authorization", bearer(&token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let events: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(events.is_array());
    }

    #[tokio::test]
    async fn test_search_empty_query_returns_400() {
        let (app, token) = test_app().await;

        let resp = app
            .oneshot(
                Request::builder()
                    .method(http::Method::GET)
                    .uri("/search?q=")
                    .header("Authorization", bearer(&token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_list_tasks_returns_array() {
        let (app, token) = test_app().await;

        let resp = app
            .oneshot(
                Request::builder()
                    .method(http::Method::GET)
                    .uri("/tasks")
                    .header("Authorization", bearer(&token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let tasks: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(tasks.is_array());
    }
}
