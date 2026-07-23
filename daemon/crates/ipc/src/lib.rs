//! Unix-domain-socket IPC server for temporald.
//!
//! Wire protocol: each frame is a 4-byte big-endian length prefix followed by
//! that many bytes of UTF-8 JSON. The JSON itself is produced/consumed by the
//! serde codec in the `temporal-domain` crate, so this crate never inspects it.
//!
//! Access control is owner-only: a restrictive umask makes the socket owner-
//! only (0700) at creation so there is no window during which group/other can
//! connect, an explicit chmod then tightens it to 0600, and every accepted
//! connection's peer uid is checked against the daemon's own uid.
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
use tokio::sync::{Semaphore, mpsc};
use tracing::{debug, error, info, warn};

/// Upper bound on a single frame; a full workspace payload is a few KB, so
/// this is generous while still rejecting garbage length prefixes.
pub const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

/// Cap on concurrently served connections. Normal use is a single client
/// (the Tauri shell); this bounds fan-out from a local misbehaving client
/// without affecting the common case.
const MAX_CONNECTIONS: usize = 64;

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
    // Set a restrictive umask *before* bind so the socket is created owner-only
    // (0700 — a socket's base mode is 0777) with no window during which it is
    // group/world-accessible, then restore the previous mask. `restrict_to_owner`
    // afterwards tightens it to 0600.
    let listener = {
        let prev = unsafe { libc::umask(0o077) };
        let listener = UnixListener::bind(socket_path);
        unsafe { libc::umask(prev) };
        listener?
    };
    restrict_to_owner(socket_path)?;
    info!(path = %socket_path.display(), "ipc server listening");

    let connections = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        // Acquire a permit before accepting so excess connections apply
        // backpressure at the accept queue instead of unbounded task spawn.
        let permit = Arc::clone(&connections)
            .acquire_owned()
            .await
            .expect("connection semaphore never closed");
        let (stream, _addr) = listener.accept().await?;
        let handler = Arc::clone(&handler);
        tokio::spawn(async move {
            if let Err(e) = serve_connection(stream, handler).await {
                warn!(error = %e, "connection ended with error");
            }
            drop(permit);
        });
    }
}

fn restrict_to_owner(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// Rejects a connection whose peer uid isn't the daemon's own uid. The socket
/// is already 0600, so this is defense-in-depth against a same-machine, other-
/// user process that somehow obtained a descriptor.
fn peer_uid_allowed(stream: &UnixStream) -> bool {
    match stream.peer_cred() {
        Ok(cred) => cred.uid() == unsafe { libc::getuid() },
        Err(e) => {
            warn!(error = %e, "cannot read peer credentials; rejecting connection");
            false
        }
    }
}

async fn serve_connection(stream: UnixStream, handler: Arc<dyn Handler>) -> io::Result<()> {
    if !peer_uid_allowed(&stream) {
        warn!("rejecting connection from a different uid");
        return Ok(());
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn frame_round_trips_and_then_clean_eof() {
        let payload = br#"{"type":"ping"}"#;
        let mut buf = Vec::new();
        write_frame(&mut buf, payload).await.unwrap();

        let mut reader = Cursor::new(buf);
        let frame = read_frame(&mut reader).await.unwrap();
        assert_eq!(frame.as_deref(), Some(&payload[..]));
        // Nothing after a whole frame is a clean EOF, not an error.
        assert!(read_frame(&mut reader).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn empty_reader_is_clean_eof() {
        let mut reader = Cursor::new(Vec::new());
        assert!(read_frame(&mut reader).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn oversize_length_prefix_is_rejected_before_allocating() {
        // Length prefix one byte over the cap; no payload follows on purpose —
        // a correct reader must reject on the prefix without trying to read it.
        let mut framed = (MAX_FRAME_BYTES + 1).to_be_bytes().to_vec();
        framed.extend_from_slice(b"anything");
        let mut reader = Cursor::new(framed);
        let err = read_frame(&mut reader).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn truncated_payload_is_an_error() {
        // Prefix claims 8 bytes but only 3 are present.
        let mut framed = 8u32.to_be_bytes().to_vec();
        framed.extend_from_slice(b"abc");
        let mut reader = Cursor::new(framed);
        let err = read_frame(&mut reader).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn partial_length_prefix_closes_cleanly() {
        // Only 2 of the 4 prefix bytes then EOF. `read_exact` reports
        // UnexpectedEof, which is indistinguishable from a clean boundary EOF,
        // so read_frame closes the connection (None) rather than erroring — the
        // right call for a length-prefixed stream.
        let mut reader = Cursor::new(vec![0u8, 0u8]);
        assert!(read_frame(&mut reader).await.unwrap().is_none());
    }

    #[test]
    fn bind_with_umask_is_never_group_or_world_accessible() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sock");
        // Same sequence serve() uses. A socket's base mode is 0777, so umask
        // 0o077 yields 0700 at creation (the explicit chmod later tightens it
        // to 0600); the security invariant is that no group/other bit is ever
        // set — no window during which another user could connect.
        let prev = unsafe { libc::umask(0o077) };
        let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        unsafe { libc::umask(prev) };
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode & 0o077, 0, "socket must never be group/world accessible");
    }

    #[tokio::test]
    async fn peer_uid_matches_self_for_same_process_socketpair() {
        // A socketpair peer is this same process, so its uid is our uid.
        let (a, _b) = UnixStream::pair().unwrap();
        assert!(peer_uid_allowed(&a));
    }
}
