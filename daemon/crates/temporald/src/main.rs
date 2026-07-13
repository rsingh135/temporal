mod handler;
mod probe;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use temporal_storage::Storage;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "temporald", about = "Temporal Workspace Engine daemon")]
struct Cli {
    /// Unix domain socket path (default: <app dir>/temporald.sock).
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the daemon (default).
    Run {
        /// SQLite database path (default: <app dir>/temporal.db).
        #[arg(long)]
        db: Option<PathBuf>,
    },
    /// Send a single request to a running daemon and print the responses.
    Probe {
        #[command(subcommand)]
        command: probe::ProbeCommand,
    },
}

fn app_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot resolve home directory")?;
    Ok(home.join("Library/Application Support/temporald"))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let app_dir = app_dir()?;
    let socket_path = cli.socket.unwrap_or_else(|| app_dir.join("temporald.sock"));

    match cli.command.unwrap_or(Command::Run { db: None }) {
        Command::Run { db } => {
            let db_path = db.unwrap_or_else(|| app_dir.join("temporal.db"));
            std::fs::create_dir_all(&app_dir)?;
            let storage = Arc::new(Storage::open(&db_path)?);
            let embedder = match temporal_semantic::Embedder::load(
                &app_dir.join("models/bge-small-en-v1.5"),
            ) {
                Ok(embedder) => Some(Arc::new(std::sync::Mutex::new(embedder))),
                Err(e) => {
                    tracing::warn!(error = %e, "semantic search disabled (recency fallback)");
                    None
                }
            };
            let tagger = match temporal_semantic::Tagger::load(
                &app_dir.join("models/qwen3-1.7b/Qwen3-1.7B-Q8_0.gguf"),
            ) {
                Ok(tagger) => Some(Arc::new(std::sync::Mutex::new(tagger))),
                Err(e) => {
                    tracing::warn!(error = %e, "llm tagging disabled (heuristic tags only)");
                    None
                }
            };
            let handler = Arc::new(handler::DaemonHandler::new(storage, embedder, tagger));
            info!(socket = %socket_path.display(), db = %db_path.display(), "temporald starting");

            tokio::select! {
                served = temporal_ipc::serve(&socket_path, handler) => {
                    served.context("ipc server failed")?;
                }
                _ = tokio::signal::ctrl_c() => {
                    info!("shutting down");
                }
            }
            // Leave no stale socket behind on clean shutdown.
            let _ = std::fs::remove_file(&socket_path);
            Ok(())
        }
        Command::Probe { command } => probe::run_probe(&socket_path, command).await,
    }
}
