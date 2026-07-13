//! Dev/verification client: sends one request over the daemon socket and
//! prints every response frame until the terminal one.

use std::path::Path;

use anyhow::{bail, Context, Result};
use temporal_domain::wire::{request_to_wire, response_from_wire};
use temporal_domain::{IpcRequest, IpcResponse, RehydrationPayload, WorkspaceState};
use tokio::net::UnixStream;

use temporal_ipc::{read_frame, write_frame};

#[derive(Debug, Clone, clap::Subcommand)]
pub enum ProbeCommand {
    /// Liveness check (expects pong).
    Ping,
    /// Freeze the current desktop state.
    Freeze,
    /// Semantic query for stored workspaces.
    Query {
        text: String,
        #[arg(long, default_value_t = 5)]
        limit: i32,
    },
    /// Rehydrate a stored workspace by id (no exclusions).
    Rehydrate { workspace_id: String },
}

/// Asks the daemon for its recent workspaces and returns the one with the
/// given id.
async fn fetch_workspace(stream: &mut UnixStream, workspace_id: &str) -> Result<WorkspaceState> {
    let request = request_to_wire(&IpcRequest::Query { text: String::new(), limit: 100 });
    write_frame(stream, request.as_bytes()).await?;
    let Some(frame) = read_frame(stream).await? else {
        bail!("daemon closed the connection during workspace lookup");
    };
    let json = String::from_utf8(frame).context("non-UTF-8 response frame")?;
    let response =
        response_from_wire(&json).map_err(|e| anyhow::anyhow!("undecodable response: {e}"))?;
    match response {
        IpcResponse::QueryResults { candidates } => candidates
            .into_iter()
            .map(|c| c.workspace)
            .find(|w| w.workspace_id == workspace_id)
            .with_context(|| format!("no stored workspace with id {workspace_id}")),
        other => bail!("unexpected response during lookup: {other:?}"),
    }
}

pub async fn run_probe(socket_path: &Path, command: ProbeCommand) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path).await.with_context(|| {
        format!("connecting to {} (is temporald running?)", socket_path.display())
    })?;

    let request = match &command {
        ProbeCommand::Ping => IpcRequest::Ping,
        ProbeCommand::Freeze => IpcRequest::Freeze,
        ProbeCommand::Query { text, limit } => {
            IpcRequest::Query { text: text.clone(), limit: *limit }
        }
        ProbeCommand::Rehydrate { workspace_id } => {
            let workspace = fetch_workspace(&mut stream, workspace_id).await?;
            IpcRequest::Rehydrate {
                payload: RehydrationPayload { workspace, excluded_node_ids: Vec::new() },
            }
        }
    };
    write_frame(&mut stream, request_to_wire(&request).as_bytes()).await?;

    loop {
        let Some(frame) = read_frame(&mut stream).await? else {
            bail!("daemon closed the connection before a terminal response");
        };
        let json = String::from_utf8(frame).context("non-UTF-8 response frame")?;
        println!("{json}");
        let response =
            response_from_wire(&json).map_err(|e| anyhow::anyhow!("undecodable response: {e}"))?;
        match response {
            IpcResponse::Pong | IpcResponse::Done { .. } | IpcResponse::Error { .. } => break,
            IpcResponse::QueryResults { candidates } => {
                eprintln!("({} candidates)", candidates.len());
                break;
            }
            IpcResponse::FreezeStarted { .. }
            | IpcResponse::RehydrateStarted
            | IpcResponse::Progress { .. } => continue,
        }
    }
    Ok(())
}
