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
                | IpcResponse::PermissionStatus { .. }
                | IpcResponse::RehydratePreview { .. }
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
    // Without models the daemon degrades to plain recency: no groups are
    // computed and no assembled candidate appears.
    let responses =
        roundtrip(&mut stream, IpcRequest::Query { text: "anything".into(), limit: 5 }).await;
    match responses.last().unwrap() {
        IpcResponse::QueryResults { candidates } => {
            assert_eq!(candidates.len(), 1);
            assert!(candidates
                .iter()
                .all(|c| c.kind == temporal_domain::CandidateKind::Workspace));
            assert!(candidates.iter().all(|c| c.workspace.groups.is_empty()));
        }
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
async fn permission_status_roundtrip() {
    let daemon = start_daemon().await;
    let mut stream = UnixStream::connect(&daemon.socket_path).await.expect("connect");
    let responses = roundtrip(&mut stream, IpcRequest::PermissionStatus).await;
    // Booleans are environment-dependent (CI has neither grant); only the
    // shape is asserted here.
    assert!(matches!(responses.last().unwrap(), IpcResponse::PermissionStatus { .. }));
}

#[tokio::test]
async fn prune_keep_latest_roundtrip() {
    let daemon = start_daemon().await;
    let mut stream = UnixStream::connect(&daemon.socket_path).await.expect("connect");

    roundtrip(&mut stream, IpcRequest::Freeze).await;
    roundtrip(&mut stream, IpcRequest::Freeze).await;

    let responses = roundtrip(
        &mut stream,
        IpcRequest::Prune { older_than_unix_ms: None, keep_latest: Some(1) },
    )
    .await;
    assert!(matches!(responses.last().unwrap(), IpcResponse::Done { .. }));

    let responses =
        roundtrip(&mut stream, IpcRequest::Query { text: String::new(), limit: 100 }).await;
    match responses.last().unwrap() {
        IpcResponse::QueryResults { candidates } => assert_eq!(candidates.len(), 1),
        other => panic!("expected QueryResults, got {other:?}"),
    }
}

#[tokio::test]
async fn prune_rejects_ambiguous_request() {
    let daemon = start_daemon().await;
    let mut stream = UnixStream::connect(&daemon.socket_path).await.expect("connect");
    let responses =
        roundtrip(&mut stream, IpcRequest::Prune { older_than_unix_ms: None, keep_latest: None })
            .await;
    assert!(matches!(responses.last().unwrap(), IpcResponse::Error { .. }));
}

#[tokio::test]
async fn rehydrate_preview_reports_a_node_per_included_node() {
    use temporal_domain::{
        AdapterKind, NodePayload, RehydrationPayload, WindowGeometry, WindowNode, WorkspaceState,
    };
    let daemon = start_daemon().await;
    let mut stream = UnixStream::connect(&daemon.socket_path).await.expect("connect");

    let node = WindowNode {
        node_id: "n1".into(),
        bundle_id: "com.apple.Terminal".into(),
        app_name: "Terminal".into(),
        window_title: String::new(),
        geometry: WindowGeometry::default(),
        adapter: AdapterKind::Generic,
        payload: NodePayload::Generic,
    };
    let payload = RehydrationPayload {
        workspace: WorkspaceState {
            workspace_id: "w".into(),
            captured_at_unix_ms: 0,
            summary: String::new(),
            tags: Vec::new(),
            nodes: vec![node],
            groups: Vec::new(),
        },
        excluded_node_ids: Vec::new(),
    };

    // Preview never launches anything, so this is safe to run in CI.
    let responses = roundtrip(&mut stream, IpcRequest::RehydratePreview { payload }).await;
    match responses.last().unwrap() {
        IpcResponse::RehydratePreview { nodes } => {
            assert_eq!(nodes.len(), 1);
            assert_eq!(nodes[0].node_id, "n1");
        }
        other => panic!("expected RehydratePreview, got {other:?}"),
    }
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
