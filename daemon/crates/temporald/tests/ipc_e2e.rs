//! In-process end-to-end test of the daemon IPC surface: real UDS, real
//! storage, real serde wire codec on both sides.

use std::path::PathBuf;
use std::sync::Arc;

use temporal_domain::wire::{request_to_wire, response_from_wire};
use temporal_domain::{IpcRequest, IpcResponse};
use temporal_ipc::{read_frame, write_frame};
use tokio::net::UnixStream;

#[path = "../src/handler.rs"]
mod handler;

struct TestDaemon {
    socket_path: PathBuf,
    _dir: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn start_daemon() -> TestDaemon {
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("t.sock");
    let storage = Arc::new(temporal_storage::Storage::open(&dir.path().join("t.db")).expect("db"));
    let handler = Arc::new(handler::DaemonHandler::new(storage, None, None));
    let server = {
        let socket_path = socket_path.clone();
        tokio::spawn(async move {
            let _ = temporal_ipc::serve(&socket_path, handler).await;
        })
    };
    // Wait for the socket to appear.
    for _ in 0..100 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    TestDaemon { socket_path, _dir: dir, server }
}

async fn roundtrip(stream: &mut UnixStream, request: IpcRequest) -> Vec<IpcResponse> {
    write_frame(stream, request_to_wire(&request).as_bytes()).await.expect("write");
    let mut responses = Vec::new();
    loop {
        let frame = read_frame(stream).await.expect("read").expect("frame");
        let json = String::from_utf8(frame).expect("utf8");
        let response = response_from_wire(&json).expect("decode");
        let terminal = matches!(
            response,
            IpcResponse::Pong
                | IpcResponse::Done { .. }
                | IpcResponse::Error { .. }
                | IpcResponse::QueryResults { .. }
        );
        responses.push(response);
        if terminal {
            return responses;
        }
    }
}

#[tokio::test]
async fn ping_freeze_query_flow() {
    let daemon = start_daemon().await;
    let mut stream = UnixStream::connect(&daemon.socket_path).await.expect("connect");

    // Ping
    let responses = roundtrip(&mut stream, IpcRequest::Ping).await;
    assert!(matches!(responses.last().unwrap(), IpcResponse::Pong));

    // Freeze: FreezeStarted then Done, and the workspace must be persisted.
    let responses = roundtrip(&mut stream, IpcRequest::Freeze).await;
    assert!(matches!(responses.first().unwrap(), IpcResponse::FreezeStarted { .. }));
    assert!(matches!(responses.last().unwrap(), IpcResponse::Done { .. }));

    // Query: returns the single stored workspace decoded through the codec.
    let responses =
        roundtrip(&mut stream, IpcRequest::Query { text: "anything".into(), limit: 5 }).await;
    match responses.last().unwrap() {
        IpcResponse::QueryResults { candidates } => assert_eq!(candidates.len(), 1),
        other => panic!("expected QueryResults, got {other:?}"),
    }

    // Undecodable frame surfaces a codec error rather than killing the connection.
    write_frame(&mut stream, b"not json").await.expect("write");
    let frame = read_frame(&mut stream).await.expect("read").expect("frame");
    let json = String::from_utf8(frame).expect("utf8");
    assert!(matches!(response_from_wire(&json).expect("decode"), IpcResponse::Error { .. }));

    // Connection still usable afterwards.
    let responses = roundtrip(&mut stream, IpcRequest::Ping).await;
    assert!(matches!(responses.last().unwrap(), IpcResponse::Pong));
}

#[tokio::test]
async fn rejects_oversized_frame_length() {
    let daemon = start_daemon().await;
    let mut stream = UnixStream::connect(&daemon.socket_path).await.expect("connect");
    use tokio::io::AsyncWriteExt;
    stream.write_all(&u32::MAX.to_be_bytes()).await.expect("write");
    // Server must drop the connection rather than allocate 4GB.
    let read = read_frame(&mut stream).await;
    match read {
        Ok(None) | Err(_) => {}
        Ok(Some(_)) => panic!("expected connection drop"),
    }
}
