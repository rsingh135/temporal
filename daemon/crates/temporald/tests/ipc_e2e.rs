//! In-process end-to-end test of the daemon IPC surface: real UDS, real
//! storage, real Fable-generated codec on both sides of the wire.

use std::path::PathBuf;
use std::sync::Arc;

use fable_library_rust::List_;
use fable_library_rust::Native_::LrcPtr;
use fable_library_rust::String_::fromString;
use temporal_core::Temporal::Domain::Codecs::{requestToWire, responseFromWire};
use temporal_core::Temporal::Domain::Types::{IpcRequest, IpcResponse};
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
    let handler = Arc::new(handler::DaemonHandler::new(storage));
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

async fn roundtrip(stream: &mut UnixStream, request: IpcRequest) -> Vec<LrcPtr<IpcResponse>> {
    let wire = requestToWire(LrcPtr::new(request)).to_string();
    write_frame(stream, wire.as_bytes()).await.expect("write");
    let mut responses = Vec::new();
    loop {
        let frame = read_frame(stream).await.expect("read").expect("frame");
        let json = String::from_utf8(frame).expect("utf8");
        let response = responseFromWire(fromString(json)).expect("decode");
        let terminal = matches!(
            response.as_ref(),
            IpcResponse::Pong
                | IpcResponse::Done(_)
                | IpcResponse::IpcError(_, _)
                | IpcResponse::QueryResults(_)
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
    assert!(matches!(responses.last().unwrap().as_ref(), IpcResponse::Pong));

    // Freeze: FreezeStarted then Done, and the workspace must be persisted.
    let responses = roundtrip(&mut stream, IpcRequest::Freeze).await;
    assert!(matches!(responses.first().unwrap().as_ref(), IpcResponse::FreezeStarted(_)));
    assert!(matches!(responses.last().unwrap().as_ref(), IpcResponse::Done(_)));

    // Query: returns the single stored workspace, decoded through the codec.
    let responses = roundtrip(&mut stream, IpcRequest::Query(fromString("anything".into()), 5)).await;
    match responses.last().unwrap().as_ref() {
        IpcResponse::QueryResults(candidates) => {
            assert_eq!(List_::length(candidates.clone()), 1);
        }
        other => panic!("expected QueryResults, got {other}"),
    }

    // Undecodable frame surfaces a codec error rather than killing the connection.
    write_frame(&mut stream, b"not json").await.expect("write");
    let frame = read_frame(&mut stream).await.expect("read").expect("frame");
    let json = String::from_utf8(frame).expect("utf8");
    let response = responseFromWire(fromString(json)).expect("decode");
    assert!(matches!(response.as_ref(), IpcResponse::IpcError(_, _)));

    // Connection still usable afterwards.
    let responses = roundtrip(&mut stream, IpcRequest::Ping).await;
    assert!(matches!(responses.last().unwrap().as_ref(), IpcResponse::Pong));
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
