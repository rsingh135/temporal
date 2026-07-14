//! M4 acceptance: freeze three distinct (synthetic) workspaces, query in
//! natural language, assert the semantic ranking. Requires the bge model in
//! the app-support models dir (build/fetch-models.sh); skips otherwise so the
//! rest of the suite stays machine-independent.

use std::sync::{Arc, Mutex};

use temporal_storage::{Storage, WorkspaceRecord};

fn model_dir() -> std::path::PathBuf {
    dirs::home_dir()
        .expect("home")
        .join("Library/Application Support/temporald/models/bge-small-en-v1.5")
}

fn store(
    storage: &Storage,
    embedder: &Arc<Mutex<temporal_semantic::Embedder>>,
    id: &str,
    text: &str,
) {
    storage
        .upsert_workspace(&WorkspaceRecord {
            workspace_id: id.to_string(),
            captured_at_unix_ms: 1,
            summary: text.to_string(),
            tags_json: "[]".to_string(),
            payload_json: "{}".to_string(),
        })
        .unwrap();
    let vector = embedder.lock().unwrap().embed_document(text).unwrap();
    storage.upsert_embedding(id, &vector).unwrap();
}

#[test]
fn natural_language_queries_rank_the_right_workspace_first() {
    let Ok(embedder) = temporal_semantic::Embedder::load(&model_dir()) else {
        eprintln!("skipping: embedding model not downloaded");
        return;
    };
    let embedder = Arc::new(Mutex::new(embedder));
    let storage = Storage::open_in_memory().unwrap();

    store(
        &storage,
        &embedder,
        "ws-daemon",
        "temporal · rust daemon work\nGoogle Chrome\ntokio unix socket ipc\n\
         github.com\ncrates.io\nTerminal\n/Users/dev/temporal\nCursor\nsqlite vector search",
    );
    store(
        &storage,
        &embedder,
        "ws-ios",
        "remy-ios · iphone app\nXcode\nSwiftUI navigation stack\ndeveloper.apple.com\n\
         App Store Connect\nTestFlight build upload\n/Users/dev/remy-ios",
    );
    store(
        &storage,
        &embedder,
        "ws-reading",
        "evening reading\nGoogle Chrome\nVagabond manga chapter 322\nreadvagabond-manga.online\n\
         Ray Tracing in One Weekend\nraytracing.github.io",
    );

    let cases = [
        ("that rust daemon and socket work", "ws-daemon"),
        ("the iphone app I was shipping to testflight", "ws-ios"),
        ("when I was reading manga in the evening", "ws-reading"),
    ];
    for (query, expected_id) in cases {
        let vector = embedder.lock().unwrap().embed_query(query).unwrap();
        let results = storage.search_embeddings(&vector, 3).unwrap();
        assert_eq!(results.len(), 3, "query: {query}");
        assert_eq!(
            results[0].0.workspace_id, expected_id,
            "query '{query}' ranked {:?} first (scores: {:?})",
            results[0].0.workspace_id,
            results.iter().map(|(r, s)| (r.workspace_id.clone(), *s)).collect::<Vec<_>>()
        );
        assert!(results[0].1 > results[1].1, "no separation for query '{query}'");
    }
}

// ---------------------------------------------------------------------------
// Grouping + prompt assembly (model-gated like the ranking test above)
// ---------------------------------------------------------------------------

#[path = "../src/handler.rs"]
mod handler;

use temporal_domain::wire::{
    request_to_wire, response_from_wire, tags_to_wire, workspace_to_wire,
};
use temporal_domain::{
    grouping, tagging, AdapterKind, BrowserTab, CandidateKind, IpcRequest, IpcResponse,
    NodePayload, TerminalTab, WindowGeometry, WindowNode, WorkspaceState,
};
use temporal_ipc::{read_frame, write_frame};
use temporal_storage::ItemRecord;

fn browser_node(id: &str, tabs: &[(&str, &str)]) -> WindowNode {
    WindowNode {
        node_id: id.into(),
        bundle_id: "com.google.Chrome".into(),
        app_name: "Google Chrome".into(),
        window_title: "tabs".into(),
        geometry: WindowGeometry::default(),
        adapter: AdapterKind::Chrome,
        payload: NodePayload::Browser {
            tabs: tabs
                .iter()
                .map(|(url, title)| BrowserTab { url: url.to_string(), title: title.to_string() })
                .collect(),
            active_tab_index: 0,
        },
    }
}

fn terminal_node(id: &str, cwd: &str) -> WindowNode {
    WindowNode {
        node_id: id.into(),
        bundle_id: "com.apple.Terminal".into(),
        app_name: "Terminal".into(),
        window_title: String::new(),
        geometry: WindowGeometry::default(),
        adapter: AdapterKind::TerminalApp,
        payload: NodePayload::Terminal {
            tabs: vec![TerminalTab { tty: "/dev/ttys001".into(), cwd: cwd.into() }],
        },
    }
}

fn editor_node(id: &str, folder: &str) -> WindowNode {
    WindowNode {
        node_id: id.into(),
        bundle_id: "com.microsoft.VSCode".into(),
        app_name: "Visual Studio Code".into(),
        window_title: "temporal".into(),
        geometry: WindowGeometry::default(),
        adapter: AdapterKind::VsCode,
        payload: NodePayload::Editor { folder_path: folder.into(), open_files: vec![] },
    }
}

fn generic_node(id: &str, app: &str, title: &str) -> WindowNode {
    WindowNode {
        node_id: id.into(),
        bundle_id: format!("com.example.{app}"),
        app_name: app.into(),
        window_title: title.into(),
        geometry: WindowGeometry::default(),
        adapter: AdapterKind::Generic,
        payload: NodePayload::Generic,
    }
}

fn workspace(id: &str, captured_at: i64, nodes: Vec<WindowNode>) -> WorkspaceState {
    tagging::enrich(WorkspaceState {
        workspace_id: id.into(),
        captured_at_unix_ms: captured_at,
        summary: String::new(),
        tags: vec![],
        nodes,
        groups: vec![],
    })
}

fn embed_items(
    embedder: &Arc<Mutex<temporal_semantic::Embedder>>,
    items: &[grouping::WorkspaceItem],
) -> Vec<Vec<f32>> {
    let mut guard = embedder.lock().unwrap();
    items.iter().map(|item| guard.embed_document(&item.embed_text).unwrap()).collect()
}

/// Persists a workspace exactly the way handle_freeze does: groups computed
/// from item embeddings, payload + workspace vector + item rows stored.
fn seed(
    storage: &Storage,
    embedder: &Arc<Mutex<temporal_semantic::Embedder>>,
    mut ws: WorkspaceState,
) -> WorkspaceState {
    let items = grouping::items_for(&ws);
    let vectors = embed_items(embedder, &items);
    ws.groups = grouping::build_groups(&items, &vectors);
    storage
        .upsert_workspace(&WorkspaceRecord {
            workspace_id: ws.workspace_id.clone(),
            captured_at_unix_ms: ws.captured_at_unix_ms,
            summary: ws.summary.clone(),
            tags_json: tags_to_wire(&ws),
            payload_json: workspace_to_wire(&ws),
        })
        .unwrap();
    let vector =
        embedder.lock().unwrap().embed_document(&tagging::embedding_text(&ws)).unwrap();
    storage.upsert_embedding(&ws.workspace_id, &vector).unwrap();
    let rows: Vec<(ItemRecord, Vec<f32>)> = items
        .iter()
        .zip(vectors)
        .map(|(item, vector)| {
            (
                ItemRecord {
                    workspace_id: ws.workspace_id.clone(),
                    node_id: item.item_ref.node_id.clone(),
                    tab_index: item.item_ref.tab_index.map(i64::from),
                    kind: item.kind.as_str().to_string(),
                    dedup_key: item.dedup_key.clone(),
                    title: item.title.clone(),
                    captured_at_unix_ms: ws.captured_at_unix_ms,
                },
                vector,
            )
        })
        .collect();
    storage.replace_items(&ws.workspace_id, &rows).unwrap();
    ws
}

/// A desktop mixing a coding context with entertainment: the clusters the
/// user actually thinks in.
fn mixed_workspace() -> WorkspaceState {
    workspace(
        "ws-mixed",
        1_000,
        vec![
            browser_node(
                "n0",
                &[
                    ("https://github.com/rsingh135/temporal", "temporal rust daemon repo"),
                    ("https://readvagabond-manga.online/chapter-322", "Vagabond manga chapter 322"),
                ],
            ),
            terminal_node("n1", "/Users/dev/temporal"),
            editor_node("n2", "/Users/dev/temporal"),
            generic_node("n3", "Spotify", "lofi beats playlist"),
        ],
    )
}

#[test]
fn clustering_separates_coding_from_entertainment() {
    let Ok(embedder) = temporal_semantic::Embedder::load(&model_dir()) else {
        eprintln!("skipping: embedding model not downloaded");
        return;
    };
    let embedder = Arc::new(Mutex::new(embedder));

    let ws = mixed_workspace();
    let items = grouping::items_for(&ws);
    let vectors = embed_items(&embedder, &items);
    let groups = grouping::build_groups(&items, &vectors);

    assert!(
        groups.len() >= 2,
        "expected the mixed desktop to split into groups, got {groups:?}"
    );
    let group_of = |node_id: &str, tab_index: Option<i32>| -> usize {
        groups
            .iter()
            .position(|g| {
                g.items.iter().any(|r| r.node_id == node_id && r.tab_index == tab_index)
            })
            .unwrap_or_else(|| panic!("item {node_id}/{tab_index:?} missing from all groups"))
    };
    let github_tab = group_of("n0", Some(0));
    let terminal = group_of("n1", None);
    let editor = group_of("n2", None);
    let manga_tab = group_of("n0", Some(1));
    assert_eq!(github_tab, terminal, "github tab and temporal terminal should share a group");
    assert_eq!(github_tab, editor, "github tab and temporal editor should share a group");
    assert_ne!(
        github_tab, manga_tab,
        "manga tab should not sit in the coding group (groups: {groups:?})"
    );
}

async fn query_daemon(socket: &std::path::Path, text: &str, limit: i32) -> Vec<IpcResponse> {
    let mut stream = tokio::net::UnixStream::connect(socket).await.expect("connect");
    let request = IpcRequest::Query { text: text.into(), limit };
    write_frame(&mut stream, request_to_wire(&request).as_bytes()).await.expect("write");
    let mut responses = Vec::new();
    loop {
        let frame = read_frame(&mut stream).await.expect("read").expect("frame");
        let response = response_from_wire(&String::from_utf8(frame).expect("utf8")).expect("decode");
        let terminal =
            matches!(response, IpcResponse::QueryResults { .. } | IpcResponse::Error { .. });
        responses.push(response);
        if terminal {
            return responses;
        }
    }
}

fn candidates_of(responses: &[IpcResponse]) -> Vec<temporal_domain::QueryCandidate> {
    match responses.last().unwrap() {
        IpcResponse::QueryResults { candidates } => candidates.clone(),
        other => panic!("expected QueryResults, got {other:?}"),
    }
}

#[tokio::test]
async fn prompt_assembles_virtual_workspace_and_menu_shows_groups() {
    let Ok(embedder) = temporal_semantic::Embedder::load(&model_dir()) else {
        eprintln!("skipping: embedding model not downloaded");
        return;
    };
    let embedder = Arc::new(Mutex::new(embedder));
    let dir = tempfile::tempdir().expect("tempdir");
    let storage = Arc::new(Storage::open(&dir.path().join("t.db")).expect("db"));

    let mixed = seed(&storage, &embedder, mixed_workspace());
    // A fresher snapshot containing the SAME github tab (newer title) plus a
    // tokio docs tab: assembly must dedup the URL and keep this version.
    seed(
        &storage,
        &embedder,
        workspace(
            "ws-newer",
            2_000,
            vec![browser_node(
                "n0",
                &[
                    ("https://github.com/rsingh135/temporal", "temporal rust daemon repo — pull requests"),
                    ("https://docs.rs/tokio", "tokio async rust documentation"),
                ],
            )],
        ),
    );

    let socket_path = dir.path().join("t.sock");
    let handler = Arc::new(handler::DaemonHandler::new(
        Arc::clone(&storage),
        Some(Arc::clone(&embedder)),
        None,
    ));
    let server = {
        let socket_path = socket_path.clone();
        tokio::spawn(async move {
            let _ = temporal_ipc::serve(&socket_path, handler).await;
        })
    };
    for _ in 0..100 {
        if socket_path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // Empty query = main menu: the mixed snapshot's groups follow it as
    // materialized sub-candidates.
    let candidates = candidates_of(&query_daemon(&socket_path, "", 10).await);
    let mixed_pos = candidates
        .iter()
        .position(|c| c.workspace.workspace_id == "ws-mixed" && c.kind == CandidateKind::Workspace)
        .expect("mixed snapshot listed");
    let group_rows: Vec<_> = candidates
        .iter()
        .filter(|c| c.kind == CandidateKind::Group)
        .collect();
    assert_eq!(group_rows.len(), mixed.groups.len(), "one row per group of the mixed snapshot");
    assert!(
        group_rows.iter().all(|c| c.source_workspace_id.as_deref() == Some("ws-mixed")),
        "groups carry their parent id"
    );
    // Group rows come immediately after their parent.
    let first_group_pos = candidates
        .iter()
        .position(|c| c.kind == CandidateKind::Group)
        .expect("group rows present");
    assert_eq!(first_group_pos, mixed_pos + 1);
    // Each group row is standalone-rehydratable: fewer nodes than the parent.
    for row in &group_rows {
        assert!(!row.workspace.nodes.is_empty());
        assert!(row.workspace.workspace_id.starts_with("ws-mixed::"));
    }

    // Prompt search: assembles the coding items across both snapshots.
    let candidates =
        candidates_of(&query_daemon(&socket_path, "the temporal rust daemon coding work", 8).await);
    let first = candidates.first().expect("candidates returned");
    assert_eq!(
        first.kind,
        CandidateKind::Assembled,
        "expected an assembled candidate first, got {:?} ({})",
        first.kind,
        first.workspace.summary
    );
    let tabs: Vec<&BrowserTab> = first
        .workspace
        .nodes
        .iter()
        .filter_map(|n| match &n.payload {
            NodePayload::Browser { tabs, .. } => Some(tabs.iter()),
            _ => None,
        })
        .flatten()
        .collect();
    let github: Vec<_> = tabs
        .iter()
        .filter(|t| t.url == "https://github.com/rsingh135/temporal")
        .collect();
    assert_eq!(github.len(), 1, "the shared github tab must be deduped (tabs: {tabs:?})");
    assert_eq!(
        github[0].title, "temporal rust daemon repo — pull requests",
        "the fresher snapshot's version wins the dedup"
    );
    assert!(
        !tabs.iter().any(|t| t.url.contains("readvagabond-manga")),
        "the manga tab is not part of the coding prompt's assembly (tabs: {tabs:?})"
    );

    server.abort();
}
