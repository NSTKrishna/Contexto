//! # ctx — Contexto CLI
//!
//! Command-line interface for the Contexto Universal Developer Context Manager.
//!
//! ## Commands (Phase 3 Targets)
//! - `ctx status` — Show daemon status, active task, token budget
//! - `ctx context` — Print the current context window (JSON or pretty)
//! - `ctx remember <note>` — Manually annotate context with a note
//! - `ctx search <query>` — FTS5 search across all captured context
//! - `ctx task start <name>` — Start a new tracked task
//! - `ctx task stop` — Stop the current task and trigger LLM summarization
//!
//! ## Usage
//! ```sh
//! ctx status
//! ctx remember "Decided to use FTS5 instead of sqlite-vss for now"
//! ctx search "sqlx migration"
//! ```

use clap::{Parser, Subcommand};

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
    #[arg(long, env = "CTX_REST_URL", default_value = "http://127.0.0.1:8942")]
    url: String,

    /// Output format: pretty | json
    #[arg(long, default_value = "pretty", global = true)]
    format: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Show the current daemon status, active task, and token budget
    Status,

    /// Print the current context window
    Context {
        /// Number of recent events to include
        #[arg(short, long, default_value = "20")]
        limit: u32,
    },

    /// Manually annotate context with a permanent note
    Remember {
        /// The note to record (wrap in quotes for multi-word notes)
        note: String,
    },

    /// Full-text search across all captured context
    Search {
        /// Search query (supports FTS5 syntax)
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
    /// Start a new task
    Start {
        /// Task name / description
        name: String,
    },
    /// Stop the current task (triggers LLM summarization in Phase 6+)
    Stop,
    /// List all tasks
    List,
}

// =============================================================================
// Entry Point
// =============================================================================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize structured logging (respects RUST_LOG env var)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ctx=info".into()),
        )
        .with_target(false)
        .init();

    // Load .env if present (development convenience)
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();

    match cli.command {
        Commands::Status => {
            println!("ctx-cli Phase 0 stub — ctxd not yet implemented.");
            println!("Daemon URL: {}", cli.url);
            println!("Run 'cargo build' to verify workspace compiles.");
        }
        Commands::Context { limit } => {
            println!("ctx context --limit {limit}: Phase 3 implementation pending.");
        }
        Commands::Remember { note } => {
            println!("ctx remember: Phase 3 implementation pending.");
            println!("Note staged: {note:?}");
        }
        Commands::Search { query, limit } => {
            println!("ctx search '{query}' --limit {limit}: Phase 3 implementation pending.");
        }
        Commands::Task { action } => match action {
            TaskAction::Start { name } => {
                println!("ctx task start '{name}': Phase 3 implementation pending.");
            }
            TaskAction::Stop => {
                println!("ctx task stop: Phase 3 implementation pending.");
            }
            TaskAction::List => {
                println!("ctx task list: Phase 3 implementation pending.");
            }
        },
    }

    Ok(())
}
