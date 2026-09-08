//! # ctx — Contexto CLI
//!
//! Command-line interface for the Contexto Universal Developer Context Manager.
//!
//! ## Commands
//! - `ctx status` — Show daemon status, active task, uptime
//! - `ctx context [--limit N]` — Print recent context events
//! - `ctx remember <note>` — Manually annotate context with a permanent note
//! - `ctx search <query> [--limit N]` — FTS5 search across all captured context
//! - `ctx task start <name>` — Start a new tracked task
//! - `ctx task stop` — Stop the current active task
//! - `ctx task abandon` — Abandon the current task without summarization
//! - `ctx task list` — List all tasks
//!
//! ## Usage
//! ```sh
//! ctx status
//! ctx remember "Decided to use FTS5 instead of sqlite-vss for now"
//! ctx search "sqlx migration"
//! ctx context --limit 50 --format json
//! ```
//!
//! ## Auth
//! The CLI reads the Bearer token from `~/.ctx/auth_token` (written by ctxd on
//! startup). Make sure ctxd is running before using these commands.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

// =============================================================================
// CLI Definition
// =============================================================================

#[derive(Parser)]
#[command(
    name = "ctx",
    version = env!("CARGO_PKG_VERSION"),
    about = "Contexto — Universal Developer Context Manager",
    long_about = "Query, annotate, and manage your developer context captured by ctxd.\n\nMake sure ctxd is running before using these commands.",
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// ctxd REST API URL (default: http://127.0.0.1:8942)
    #[arg(
        long,
        env = "CTX_REST_URL",
        default_value = "http://127.0.0.1:8942",
        global = true
    )]
    url: String,

    /// Output format: pretty | json
    #[arg(long, default_value = "pretty", global = true)]
    format: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Show the current daemon status, active task, and event count
    Status,

    /// Print the current context window (recent events)
    Context {
        /// Number of recent events to include (max 200)
        #[arg(short, long, default_value = "20")]
        limit: u32,

        /// Filter by source (terminal, editor, git, file_system, manual, mcp)
        #[arg(short, long)]
        source: Option<String>,
    },

    /// Manually annotate context with a permanent note
    Remember {
        /// The note to record (wrap in quotes for multi-word notes)
        note: String,

        /// Short label (default: "note")
        #[arg(short, long)]
        label: Option<String>,
    },

    /// Full-text search across all captured context (FTS5 BM25)
    Search {
        /// Search query — supports FTS5 syntax: "exact phrase", term1 AND term2, term*
        query: String,

        /// Maximum number of results to return
        #[arg(short, long, default_value = "10")]
        limit: u32,
    },

    /// Manage tracked tasks
    Task {
        #[command(subcommand)]
        action: TaskAction,
    },
}

#[derive(Subcommand)]
enum TaskAction {
    /// Start a new task and set it as active
    Start {
        /// Task name / description
        name: String,
    },
    /// Stop the current active task (marks as completed)
    Stop,
    /// Abandon the current active task (no summarization)
    Abandon,
    /// List all tasks (newest first)
    List,
}

// =============================================================================
// Auth helpers
// =============================================================================

fn token_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".ctx").join("auth_token")
}

fn read_token() -> anyhow::Result<String> {
    let path = token_path();
    let token = std::fs::read_to_string(&path).map_err(|e| {
        anyhow::anyhow!(
            "Cannot read auth token from {}:\n  {}\n\nIs ctxd running? Start it with:\n  cargo run -p ctxd --bin ctxd",
            path.display(),
            e
        )
    })?;
    Ok(token.trim().to_string())
}

fn build_client(token: &str) -> anyhow::Result<reqwest::Client> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))?,
    );
    Ok(reqwest::Client::builder()
        .default_headers(headers)
        .build()?)
}

// =============================================================================
// Output helpers
// =============================================================================

fn print_json(value: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

fn print_events_pretty(events: &serde_json::Value) {
    let Some(arr) = events.as_array() else {
        println!("{events}");
        return;
    };
    if arr.is_empty() {
        println!("  (no events)");
        return;
    }
    for event in arr {
        let source = event["source"].as_str().unwrap_or("?");
        let label = event["label"].as_str().unwrap_or("(no label)");
        let ts = event["timestamp"].as_str().unwrap_or("");
        let content = event["content"].as_str().unwrap_or("");
        let ts_short = ts.get(..19).unwrap_or(ts);
        println!("─────────────────────────────────────────");
        println!("  [{source:^4}] {label}");
        println!("  {ts_short}");
        if !content.is_empty() {
            let preview = if content.len() > 200 {
                &content[..200]
            } else {
                content
            };
            println!("  {preview}");
        }
    }
    println!("─────────────────────────────────────────");
}

fn print_tasks_pretty(tasks: &serde_json::Value) {
    let Some(arr) = tasks.as_array() else {
        println!("{tasks}");
        return;
    };
    if arr.is_empty() {
        println!("  (no tasks)");
        return;
    }
    for task in arr {
        let name = task["name"].as_str().unwrap_or("?");
        let status = task["status"].as_str().unwrap_or("?");
        let id = task["id"].as_str().unwrap_or("?");
        let count = task["event_count"].as_u64().unwrap_or(0);
        let ts = task["started_at"].as_str().unwrap_or("");
        let ts_short = ts.get(..19).unwrap_or(ts);
        let icon = match status {
            "active" => "🟢",
            "completed" => "✅",
            "abandoned" => "❌",
            _ => "⏸",
        };
        println!("{icon} {name}");
        println!("   id:      {id}");
        println!("   status:  {status}  |  events: {count}  |  started: {ts_short}");
        println!();
    }
}

// =============================================================================
// Command implementations
// =============================================================================

async fn cmd_status(client: &reqwest::Client, base: &str, format: &str) -> anyhow::Result<()> {
    let resp: serde_json::Value = client
        .get(format!("{base}/status"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if format == "json" {
        print_json(&resp);
        return Ok(());
    }

    let version = resp["version"].as_str().unwrap_or("?");
    let uptime = resp["uptime_seconds"].as_i64().unwrap_or(0);
    let event_count = resp["event_count"].as_u64().unwrap_or(0);
    let db_path = resp["db_path"].as_str().unwrap_or("?");
    let listen_addr = resp["listen_addr"].as_str().unwrap_or("?");

    let uptime_str = if uptime < 60 {
        format!("{uptime}s")
    } else if uptime < 3600 {
        format!("{}m {}s", uptime / 60, uptime % 60)
    } else {
        format!("{}h {}m", uptime / 3600, (uptime % 3600) / 60)
    };

    println!("╭──────────────────────────────────────╮");
    println!("│   ctxd  v{version:<28}│");
    println!("╰──────────────────────────────────────╯");
    println!("  ✅ Running on  {listen_addr}");
    println!("  ⏱  Uptime:     {uptime_str}");
    println!("  📦 Events:     {event_count}");
    println!("  🗄️  DB:         {db_path}");

    if let Some(task) = resp["active_task"].as_object() {
        let tname = task["name"].as_str().unwrap_or("?");
        let tevents = task
            .get("event_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        println!("  🟢 Task:       {tname} ({tevents} events)");
    } else {
        println!("  ⏸  Task:       (none active)");
    }
    Ok(())
}

async fn cmd_context(
    client: &reqwest::Client,
    base: &str,
    format: &str,
    limit: u32,
    source: Option<String>,
) -> anyhow::Result<()> {
    let mut url = reqwest::Url::parse(&format!("{base}/context"))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("limit", &limit.to_string());
        if let Some(s) = &source {
            q.append_pair("source", s);
        }
    }

    let events: serde_json::Value = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if format == "json" {
        print_json(&events);
    } else {
        let count = events.as_array().map_or(0, Vec::len);
        println!("Recent context ({count} events):\n");
        print_events_pretty(&events);
    }
    Ok(())
}

async fn cmd_remember(
    client: &reqwest::Client,
    base: &str,
    note: String,
    label: Option<String>,
) -> anyhow::Result<()> {
    let mut body = serde_json::json!({ "note": note });
    if let Some(l) = label {
        body["label"] = serde_json::Value::String(l);
    }
    client
        .post(format!("{base}/remember"))
        .json(&body)
        .send()
        .await?
        .error_for_status()?;
    println!("✅ Note saved.");
    Ok(())
}

async fn cmd_search(
    client: &reqwest::Client,
    base: &str,
    format: &str,
    query: String,
    limit: u32,
) -> anyhow::Result<()> {
    let mut url = reqwest::Url::parse(&format!("{base}/search"))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("q", &query)
            .append_pair("limit", &limit.to_string());
    }

    let events: serde_json::Value = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if format == "json" {
        print_json(&events);
    } else {
        let count = events.as_array().map_or(0, Vec::len);
        if count == 0 {
            println!("No results for \"{query}\".");
        } else {
            println!("Search results for \"{query}\" ({count}):\n");
            print_events_pretty(&events);
        }
    }
    Ok(())
}

async fn cmd_task_start(client: &reqwest::Client, base: &str, name: String) -> anyhow::Result<()> {
    let task: serde_json::Value = client
        .post(format!("{base}/tasks"))
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let id = task["id"].as_str().unwrap_or("?");
    println!("🟢 Task started: {name}");
    println!("   id: {id}");
    Ok(())
}

async fn cmd_task_action(client: &reqwest::Client, base: &str, action: &str) -> anyhow::Result<()> {
    // Fetch the currently active task from /status
    let status: serde_json::Value = client
        .get(format!("{base}/status"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let task_id = status["active_task"]["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No active task to {action}."))?;
    let task_name = status["active_task"]["name"]
        .as_str()
        .unwrap_or("(unknown)");

    client
        .patch(format!("{base}/tasks/{task_id}"))
        .json(&serde_json::json!({ "action": action }))
        .send()
        .await?
        .error_for_status()?;

    match action {
        "stop" => println!("✅ Task stopped:   {task_name}"),
        "abandon" => println!("❌ Task abandoned: {task_name}"),
        _ => println!("Done: {task_name}"),
    }
    Ok(())
}

async fn cmd_task_list(client: &reqwest::Client, base: &str, format: &str) -> anyhow::Result<()> {
    let tasks: serde_json::Value = client
        .get(format!("{base}/tasks"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if format == "json" {
        print_json(&tasks);
    } else {
        let count = tasks.as_array().map_or(0, Vec::len);
        println!("All tasks ({count}):\n");
        print_tasks_pretty(&tasks);
    }
    Ok(())
}

// =============================================================================
// Entry point
// =============================================================================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "off".into()),
        )
        .with_target(false)
        .init();

    let _ = dotenvy::dotenv();

    let cli = Cli::parse();
    let format = cli.format.as_str();

    let token = read_token()?;
    let client = build_client(&token)?;
    let base = &cli.url;

    let result = match cli.command {
        Commands::Status => cmd_status(&client, base, format).await,

        Commands::Context { limit, source } => {
            cmd_context(&client, base, format, limit, source).await
        }

        Commands::Remember { note, label } => cmd_remember(&client, base, note, label).await,

        Commands::Search { query, limit } => cmd_search(&client, base, format, query, limit).await,

        Commands::Task { action } => match action {
            TaskAction::Start { name } => cmd_task_start(&client, base, name).await,
            TaskAction::Stop => cmd_task_action(&client, base, "stop").await,
            TaskAction::Abandon => cmd_task_action(&client, base, "abandon").await,
            TaskAction::List => cmd_task_list(&client, base, format).await,
        },
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }

    Ok(())
}
