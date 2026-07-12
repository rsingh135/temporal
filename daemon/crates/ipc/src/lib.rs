//! Unix-domain-socket IPC server for temporald.
//!
//! Wire protocol: each frame is a 4-byte big-endian length prefix followed by
//! that many bytes of UTF-8 JSON. The JSON itself is produced/consumed by the
//! Fable-generated codec in `temporal-core`, so this crate never inspects it.
//!
//! Per request, a handler may send any number of response frames (progress
//! events) before finishing; frames are written in send order.

use std::future::Future;
use std::io;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

/// Upper bound on a single frame; a full workspace payload is a few KB, so
/// this is generous while still rejecting garbage length prefixes.
pub const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

/// Sends response frames for one request. Dropping it ends the response
/// stream for that request.
pub type Responder = mpsc::Sender<String>;

/// One request in, zero or more response frames out (via the responder).
pub trait Handler: Send + Sync + 'static {
    fn handle(
        &self,
        request_json: String,
        responder: Responder,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// Reads one length-prefixed frame; `None` on clean EOF at a frame boundary.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes exceeds limit of {MAX_FRAME_BYTES}"),
        ));
    }
    let mut payload = vec![0u8; len as usize];
    reader.read_exact(&mut payload).await?;
    Ok(Some(payload))
}

pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame too large"))?;
    if len > MAX_FRAME_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(payload).await?;
    writer.flush().await
}

/// Binds `socket_path` (replacing any stale socket file), restricts it to the
/// current user, and serves connections until the task is aborted.
pub async fn serve(socket_path: &Path, handler: Arc<dyn Handler>) -> io::Result<()> {
    if socket_path.exists() {
        // A previous daemon instance may have crashed without cleanup; the
        // bind would otherwise fail with AddrInUse.
        std::fs::remove_file(socket_path)?;
    }
    let listener = UnixListener::bind(socket_path)?;
    restrict_to_owner(socket_path)?;
    info!(path = %socket_path.display(), "ipc server listening");

    loop {
        let (stream, _addr) = listener.accept().await?;
        let handler = Arc::clone(&handler);
        tokio::spawn(async move {
            if let Err(e) = serve_connection(stream, handler).await {
                warn!(error = %e, "connection ended with error");
            }
        });
    }
}

fn restrict_to_owner(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

async fn serve_connection(stream: UnixStream, handler: Arc<dyn Handler>) -> io::Result<()> {
    let (mut reader, mut writer) = stream.into_split();
    debug!("client connected");
    while let Some(frame) = read_frame(&mut reader).await? {
        let request_json = match String::from_utf8(frame) {
            Ok(s) => s,
            Err(_) => {
                error!("dropping non-UTF-8 frame");
                continue;
            }
        };
        debug!(request = %request_json, "request");
        let (tx, mut rx) = mpsc::channel::<String>(32);
        let handler = Arc::clone(&handler);
        let work = async move { handler.handle(request_json, tx).await };
        // Run the handler concurrently with draining its responses so
        // progress frames flush as they are produced.
        let drain = async {
            while let Some(response) = rx.recv().await {
                debug!(response = %response, "response");
                write_frame(&mut writer, response.as_bytes()).await?;
            }
            Ok::<(), io::Error>(())
        };
        let ((), drained) = tokio::join!(work, drain);
        drained?;
    }
    debug!("client disconnected");
    Ok(())
}
