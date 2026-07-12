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
