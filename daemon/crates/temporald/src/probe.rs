//! Dev/verification client: sends one request over the daemon socket and
//! prints every response frame until the terminal one.

use std::path::Path;

use anyhow::{bail, Context, Result};
use fable_library_rust::List_;
use fable_library_rust::Native_::LrcPtr;
use fable_library_rust::String_::fromString;
use temporal_core::Temporal::Domain::Codecs::{requestToWire, responseFromWire};
use temporal_core::Temporal::Domain::Types::{IpcRequest, IpcResponse};
use tokio::net::UnixStream;

use temporal_ipc::{read_frame, write_frame};

/// Asks the daemon for its recent workspaces and returns the one with the
/// given id.
async fn fetch_workspace(
    stream: &mut UnixStream,
    workspace_id: &str,
) -> Result<LrcPtr<temporal_core::Temporal::Domain::Types::WorkspaceState>> {
    let request = requestToWire(LrcPtr::new(IpcRequest::Query(fromString(String::new()), 100)));
    write_frame(stream, request.to_string().as_bytes()).await?;
    let Some(frame) = read_frame(stream).await? else {
        bail!("daemon closed the connection during workspace lookup");
    };
    let json = String::from_utf8(frame).context("non-UTF-8 response frame")?;
    let response = responseFromWire(fromString(json))
        .map_err(|e| anyhow::anyhow!("undecodable response: {e}"))?;
    match response.as_ref() {
        IpcResponse::QueryResults(candidates) => {
            for candidate in fable_library_rust::List_::toArray(candidates.clone()).get().iter() {
                if candidate.Workspace.WorkspaceId.to_string() == workspace_id {
                    return Ok(candidate.Workspace.clone());
                }
            }
            bail!("no stored workspace with id {workspace_id}");
        }
        other => bail!("unexpected response during lookup: {other}"),
    }
}

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

pub async fn run_probe(socket_path: &Path, command: ProbeCommand) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("connecting to {} (is temporald running?)", socket_path.display()))?;

    let request = match &command {
        ProbeCommand::Ping => LrcPtr::new(IpcRequest::Ping),
        ProbeCommand::Freeze => LrcPtr::new(IpcRequest::Freeze),
        ProbeCommand::Query { text, limit } => {
            LrcPtr::new(IpcRequest::Query(fromString(text.clone()), *limit))
        }
        ProbeCommand::Rehydrate { workspace_id } => {
            // Fetch the workspace through the daemon (recency list) so the
            // probe needs no direct storage access.
            let workspace = fetch_workspace(&mut stream, workspace_id).await?;
            LrcPtr::new(IpcRequest::Rehydrate(LrcPtr::new(
                temporal_core::Temporal::Domain::Types::RehydrationPayload {
                    Workspace: workspace,
                    ExcludedNodeIds: fable_library_rust::List_::empty(),
                },
            )))
        }
    };
    let wire = requestToWire(request).to_string();
    write_frame(&mut stream, wire.as_bytes()).await?;

    loop {
        let Some(frame) = read_frame(&mut stream).await? else {
            bail!("daemon closed the connection before a terminal response");
        };
        let json = String::from_utf8(frame).context("non-UTF-8 response frame")?;
        println!("{json}");
        let response = responseFromWire(fromString(json))
            .map_err(|e| anyhow::anyhow!("undecodable response: {e}"))?;
        match response.as_ref() {
            IpcResponse::Pong | IpcResponse::Done(_) | IpcResponse::IpcError(_, _) => break,
            IpcResponse::QueryResults(candidates) => {
                eprintln!("({} candidates)", List_::length(candidates.clone()));
                break;
            }
            IpcResponse::FreezeStarted(_)
            | IpcResponse::RehydrateStarted
            | IpcResponse::Progress(_, _, _) => continue,
        }
    }
    Ok(())
}
