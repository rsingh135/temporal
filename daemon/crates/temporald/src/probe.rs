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
}

impl ProbeCommand {
    fn to_request(&self) -> LrcPtr<IpcRequest> {
        LrcPtr::new(match self {
            ProbeCommand::Ping => IpcRequest::Ping,
            ProbeCommand::Freeze => IpcRequest::Freeze,
            ProbeCommand::Query { text, limit } => {
                IpcRequest::Query(fromString(text.clone()), *limit)
            }
        })
    }
}

pub async fn run_probe(socket_path: &Path, command: ProbeCommand) -> Result<()> {
    let mut stream = UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("connecting to {} (is temporald running?)", socket_path.display()))?;

    let wire = requestToWire(command.to_request()).to_string();
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
