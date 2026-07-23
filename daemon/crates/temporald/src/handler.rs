//! Bridges IPC frames to daemon behavior: decode request, act, encode
//! responses. All wire JSON is serde over the shared domain types.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use temporal_domain::wire::{
    request_from_wire, response_to_wire, tags_to_wire, workspace_from_wire, workspace_to_wire,
};
use temporal_domain::{
    catalog, grouping, planning, tagging, AdapterKind, CandidateKind, IpcRequest, IpcResponse,
    NodePayload, QueryCandidate, RehydrationPayload, WindowGeometry, WindowNode, WorkspaceState,
};
use temporal_ipc::{Handler, Responder};
use temporal_storage::{AppCatalogRecord, ItemRecord, Storage, WorkspaceRecord};
use tracing::{error, info, warn};

/// Watches a detached `spawn_blocking` task and logs if it *panicked* — the
/// tasks log their own returned errors, but a panic would otherwise vanish
/// silently because the JoinHandle is dropped.
fn supervise(handle: tokio::task::JoinHandle<()>, name: &'static str) {
    tokio::spawn(async move {
        if let Err(e) = handle.await {
            error!(task = name, error = %e, "detached task panicked");
        }
    });
}

/// Upper bounds on a rehydration payload. A real desktop is a few dozen
/// windows with a few dozen tabs each; these are generous but reject
/// pathological client payloads before any subprocess/JXA work.
const MAX_REHYDRATE_NODES: usize = 200;
const MAX_TABS_PER_NODE: usize = 500;

/// Rejects an oversized rehydration payload. Content is not otherwise
/// validated — see the trust-boundary note in SECURITY.md.
fn validate_payload_size(payload: &RehydrationPayload) -> Result<(), String> {
    let nodes = &payload.workspace.nodes;
    if nodes.len() > MAX_REHYDRATE_NODES {
        return Err(format!(
            "payload has {} nodes; maximum is {MAX_REHYDRATE_NODES}",
            nodes.len()
        ));
    }
    for node in nodes {
        let tabs = match &node.payload {
            NodePayload::Browser { tabs, .. } => tabs.len(),
            NodePayload::Terminal { tabs } => tabs.len(),
            NodePayload::Editor { .. } | NodePayload::Generic => 0,
        };
        if tabs > MAX_TABS_PER_NODE {
            return Err(format!(
                "node {:?} has {tabs} tabs; maximum is {MAX_TABS_PER_NODE}",
                node.node_id
            ));
        }
    }
    Ok(())
}

/// How many most-recent snapshots the scheduled auto-freeze retains; older ones
/// are pruned after each capture so passive freezing can't grow the DB forever.
// Used by the binary (main.rs). The integration tests include this file via
// `#[path]` without main.rs, so allow the false "unused" there.
#[allow(dead_code)]
const AUTO_FREEZE_KEEP: usize = 100;

/// Distinguishes a task/join failure (E_INTERNAL) from a storage failure
/// (E_STORAGE) so the freeze path keeps its original error codes after being
/// shared between the interactive and scheduled callers.
enum FreezeError {
    Internal(tokio::task::JoinError),
    Storage(anyhow::Error),
}

/// Persists a captured workspace: builds semantic groups + item vectors (when
/// an embedder is present), writes the record, its embedding, and its items.
/// Runs inside `spawn_blocking`.
fn persist_workspace(
    storage: Arc<Storage>,
    embedder: Option<SharedEmbedder>,
    mut workspace: WorkspaceState,
) -> anyhow::Result<()> {
    // Groups must exist before the record is built: they live inside
    // payload_json. Item vectors land alongside for prompt assembly.
    let mut item_rows: Vec<(ItemRecord, Vec<f32>)> = Vec::new();
    if let Some(embedder) = &embedder {
        let items = grouping::items_for(&workspace);
        let mut guard = embedder.lock().expect("embedder mutex poisoned");
        let mut vectors = Vec::with_capacity(items.len());
        for item in &items {
            vectors.push(guard.embed_document(&item.embed_text)?);
        }
        drop(guard);
        workspace.groups = grouping::build_groups(&items, &vectors);
        item_rows = items
            .iter()
            .zip(vectors)
            .map(|(item, vector)| {
                (item_record(&workspace.workspace_id, workspace.captured_at_unix_ms, item), vector)
            })
            .collect();
    }
    let record = WorkspaceRecord {
        workspace_id: workspace.workspace_id.clone(),
        captured_at_unix_ms: workspace.captured_at_unix_ms,
        summary: workspace.summary.clone(),
        tags_json: tags_to_wire(&workspace),
        payload_json: workspace_to_wire(&workspace),
    };
    storage.upsert_workspace(&record)?;
    if let Some(embedder) = &embedder {
        let vector = embedder
            .lock()
            .expect("embedder mutex poisoned")
            .embed_document(&tagging::embedding_text(&workspace))?;
        storage.upsert_embedding(&workspace.workspace_id, &vector)?;
        storage.replace_items(&workspace.workspace_id, &item_rows)?;
    }
    Ok(())
}

/// The ort session is not Sync; a std Mutex serializes embeddings (they take
/// ~10ms and happen once per freeze/query).
pub type SharedEmbedder = Arc<std::sync::Mutex<temporal_semantic::Embedder>>;
/// Generation takes seconds; the Mutex serializes concurrent freezes.
pub type SharedTagger = Arc<std::sync::Mutex<temporal_semantic::Tagger>>;

pub struct DaemonHandler {
    storage: Arc<Storage>,
    embedder: Option<SharedEmbedder>,
    tagger: Option<SharedTagger>,
}

impl DaemonHandler {
    pub fn new(
        storage: Arc<Storage>,
        embedder: Option<SharedEmbedder>,
        tagger: Option<SharedTagger>,
    ) -> Self {
        Self { storage, embedder, tagger }
    }

    async fn respond(responder: &Responder, response: IpcResponse) {
        if responder.send(response_to_wire(&response)).await.is_err() {
            warn!("client went away mid-response");
        }
    }

    async fn error(responder: &Responder, code: &str, message: impl ToString) {
        Self::respond(
            responder,
            IpcResponse::Error { code: code.to_string(), message: message.to_string() },
        )
        .await;
    }

    async fn handle_freeze(&self, responder: Responder) {
        match self.freeze_core(Some(&responder)).await {
            Ok((workspace_id, node_count)) => {
                Self::respond(
                    &responder,
                    IpcResponse::Done {
                        message: format!("froze workspace {workspace_id} ({node_count} windows)"),
                    },
                )
                .await;
            }
            Err(FreezeError::Storage(e)) => Self::error(&responder, "E_STORAGE", e).await,
            Err(FreezeError::Internal(e)) => Self::error(&responder, "E_INTERNAL", e).await,
        }
    }

    /// Captures the desktop and persists it, returning `(workspace_id,
    /// node_count)`. Shared by the interactive `Freeze` IPC (which passes a
    /// responder to stream FreezeStarted/Progress) and the scheduled
    /// auto-freeze (which passes `None`). Spawns detached LLM enrichment on
    /// success so the freeze itself stays fast.
    async fn freeze_core(&self, progress: Option<&Responder>) -> Result<(String, usize), FreezeError> {
        let workspace_id = format!("ws-{}", uuid::Uuid::new_v4());
        let now_ms = unix_ms();
        if let Some(r) = progress {
            Self::respond(r, IpcResponse::FreezeStarted { workspace_id: workspace_id.clone() }).await;
            Self::respond(
                r,
                IpcResponse::Progress {
                    stage: "extract".into(),
                    detail: "desktop state".into(),
                    percent: 10,
                },
            )
            .await;
        }

        let report = tokio::task::spawn_blocking(temporal_adapters::extract_workspace)
            .await
            .map_err(FreezeError::Internal)?;
        for warning in &report.warnings {
            warn!(warning, "extraction warning");
        }
        let node_count = report.nodes.len();
        if let Some(r) = progress {
            Self::respond(
                r,
                IpcResponse::Progress {
                    stage: "tag".into(),
                    detail: format!("{node_count} windows captured"),
                    percent: 60,
                },
            )
            .await;
        }

        let workspace = tagging::enrich(WorkspaceState {
            workspace_id: workspace_id.clone(),
            captured_at_unix_ms: now_ms,
            summary: String::new(),
            tags: Vec::new(),
            nodes: report.nodes,
            groups: Vec::new(),
        });

        if let Some(r) = progress {
            Self::respond(
                r,
                IpcResponse::Progress { stage: "persist".into(), detail: String::new(), percent: 90 },
            )
            .await;
        }
        let storage = Arc::clone(&self.storage);
        let embedder = self.embedder.clone();
        tokio::task::spawn_blocking(move || persist_workspace(storage, embedder, workspace))
            .await
            .map_err(FreezeError::Internal)?
            .map_err(FreezeError::Storage)?;

        info!(workspace_id, node_count, "workspace frozen");
        // LLM enrichment runs detached: the freeze stays instant and the record
        // upgrades in place when generation finishes.
        self.spawn_llm_enrichment(workspace_id.clone());
        Ok((workspace_id, node_count))
    }

    /// One scheduled capture: freeze silently, then bound DB growth by keeping
    /// only the most recent snapshots. All failures log and are swallowed so
    /// the timer loop keeps running.
    // Called by the binary's interval loop; see the AUTO_FREEZE_KEEP note.
    #[allow(dead_code)]
    pub async fn auto_freeze(&self) {
        match self.freeze_core(None).await {
            Ok((workspace_id, node_count)) => {
                info!(workspace_id, node_count, "auto-freeze stored");
                let storage = Arc::clone(&self.storage);
                match tokio::task::spawn_blocking(move || storage.prune_keep_latest(AUTO_FREEZE_KEEP))
                    .await
                {
                    Ok(Ok(removed)) if removed > 0 => {
                        info!(removed, "auto-freeze pruned old snapshots")
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => warn!(error = %e, "auto-freeze prune failed"),
                    Err(e) => warn!(error = %e, "auto-freeze prune task panicked"),
                }
            }
            Err(FreezeError::Storage(e)) => warn!(error = %e, "auto-freeze failed"),
            Err(FreezeError::Internal(e)) => warn!(error = %e, "auto-freeze task panicked"),
        }
    }

    /// Detached: generate LLM summary/tags, merge over the heuristics,
    /// re-persist and re-embed. Failures only log — heuristics remain.
    fn spawn_llm_enrichment(&self, workspace_id: String) {
        let Some(tagger) = self.tagger.clone() else { return };
        let storage = Arc::clone(&self.storage);
        let embedder = self.embedder.clone();
        let handle = tokio::task::spawn_blocking(move || {
            let run = || -> anyhow::Result<()> {
                let Some(record) = storage.get_workspace(&workspace_id)? else {
                    return Ok(()); // deleted/overwritten in the meantime
                };
                let workspace = workspace_from_wire(&record.payload_json)
                    .map_err(|e| anyhow::anyhow!("stored payload undecodable: {e}"))?;
                let context = tagging::embedding_text(&workspace);
                let result = tagger.lock().expect("tagger mutex poisoned").generate(&context)?;
                info!(workspace_id, summary = %result.summary, tags = ?result.tags, "llm tags generated");

                let mut enriched = tagging::apply_llm_tags(&result.summary, &result.tags, workspace);
                // Upgrade heuristic group labels; membership is fixed at
                // freeze so ids stay stable and the UI never reshuffles.
                if enriched.groups.len() >= 2 {
                    let items = grouping::items_for(&enriched);
                    let contexts: Vec<String> = enriched
                        .groups
                        .iter()
                        .map(|group| {
                            group
                                .items
                                .iter()
                                .filter_map(|r| {
                                    items.iter().find(|item| item.item_ref == *r)
                                })
                                .map(|item| item.title.clone())
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .collect();
                    match tagger.lock().expect("tagger mutex poisoned").label_groups(&contexts) {
                        Ok(labels) => {
                            for (group, label) in enriched.groups.iter_mut().zip(labels) {
                                group.label = label;
                            }
                            info!(workspace_id, groups = enriched.groups.len(), "llm group labels applied");
                        }
                        Err(e) => {
                            warn!(error = %e, "group labeling failed; heuristic labels kept");
                        }
                    }
                }
                let updated = WorkspaceRecord {
                    workspace_id: workspace_id.clone(),
                    captured_at_unix_ms: record.captured_at_unix_ms,
                    summary: enriched.summary.clone(),
                    tags_json: tags_to_wire(&enriched),
                    payload_json: workspace_to_wire(&enriched),
                };
                storage.upsert_workspace(&updated)?;
                if let Some(embedder) = embedder {
                    let vector = embedder
                        .lock()
                        .expect("embedder mutex poisoned")
                        .embed_document(&tagging::embedding_text(&enriched))?;
                    storage.upsert_embedding(&workspace_id, &vector)?;
                }
                Ok(())
            };
            if let Err(e) = run() {
                warn!(error = %e, "llm enrichment failed; heuristic tags kept");
            }
        });
        supervise(handle, "llm enrichment");
    }

    async fn handle_query(&self, text: String, limit: i32, responder: Responder) {
        let storage = Arc::clone(&self.storage);
        let embedder = self.embedder.clone();
        let limit_n = limit.max(0) as usize;
        let built = tokio::task::spawn_blocking(move || {
            build_candidates(&storage, embedder.as_ref(), &text, limit_n)
        })
        .await;
        match built {
            Ok(Ok(candidates)) => {
                Self::respond(&responder, IpcResponse::QueryResults { candidates }).await
            }
            Ok(Err(e)) => Self::error(&responder, "E_STORAGE", e).await,
            Err(join_err) => Self::error(&responder, "E_INTERNAL", join_err).await,
        }
    }

    /// Intent synthesis: assemble a workspace for a stated intent from the live
    /// desktop, history, and installed apps — the forward-looking counterpart to
    /// `handle_query`'s retrieval. Blocking work (live desktop capture, embedding,
    /// LLM capability inference) runs off the reactor.
    async fn handle_summon(&self, text: String, responder: Responder) {
        let storage = Arc::clone(&self.storage);
        let embedder = self.embedder.clone();
        let tagger = self.tagger.clone();
        let built = tokio::task::spawn_blocking(move || {
            build_summon_candidates(&storage, embedder.as_ref(), tagger.as_ref(), &text)
        })
        .await;
        match built {
            Ok(Ok(candidates)) => {
                Self::respond(&responder, IpcResponse::QueryResults { candidates }).await
            }
            Ok(Err(e)) => Self::error(&responder, "E_STORAGE", e).await,
            Err(join_err) => Self::error(&responder, "E_INTERNAL", join_err).await,
        }
    }

    async fn handle_rehydrate(&self, payload: RehydrationPayload, responder: Responder) {
        // The payload is client-supplied and not checked against storage
        // (synthesized Group/Assembled candidates never persist verbatim), so
        // cap its size to bound resource use / JXA-script generation before any
        // work. The real trust boundary is the owner-only socket (see SECURITY.md).
        if let Err(msg) = validate_payload_size(&payload) {
            return Self::error(&responder, "E_INVALID", msg).await;
        }
        Self::respond(&responder, IpcResponse::RehydrateStarted).await;
        let nodes = planning::included_nodes(&payload.workspace, &payload.excluded_node_ids);
        let total = nodes.len();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<temporal_adapters::rehydrate::NodeEvent>(32);
        let work = tokio::task::spawn_blocking(move || {
            temporal_adapters::rehydrate::rehydrate_nodes(&nodes, |event| {
                let _ = tx.blocking_send(event);
            })
        });
        use temporal_adapters::rehydrate::NodeEvent;
        while let Some(event) = rx.recv().await {
            let response = match event {
                NodeEvent::Started { index, app_name } => {
                    let percent = (index * 100).checked_div(total).unwrap_or(100) as i32;
                    IpcResponse::Progress { stage: "launch".into(), detail: app_name, percent }
                }
                NodeEvent::Finished { node_id, app_name, ok, message, .. } => {
                    IpcResponse::NodeResult { node_id, app_name, ok, message }
                }
            };
            Self::respond(&responder, response).await;
        }
        match work.await {
            Ok(outcome) => {
                let message = if outcome.failures.is_empty() {
                    format!("rehydrated {} windows", outcome.restored)
                } else {
                    format!(
                        "rehydrated {} windows; {} failed: {}",
                        outcome.restored,
                        outcome.failures.len(),
                        outcome.failures.join("; ")
                    )
                };
                info!(restored = outcome.restored, failures = outcome.failures.len(), "rehydration finished");
                Self::respond(&responder, IpcResponse::Done { message }).await;
            }
            Err(join_err) => Self::error(&responder, "E_INTERNAL", join_err).await,
        }
    }

    async fn handle_rehydrate_preview(&self, payload: RehydrationPayload, responder: Responder) {
        if let Err(msg) = validate_payload_size(&payload) {
            return Self::error(&responder, "E_INVALID", msg).await;
        }
        let nodes = planning::included_nodes(&payload.workspace, &payload.excluded_node_ids);
        let previewed =
            tokio::task::spawn_blocking(move || temporal_adapters::rehydrate::preflight_nodes(&nodes))
                .await;
        match previewed {
            Ok(nodes) => Self::respond(&responder, IpcResponse::RehydratePreview { nodes }).await,
            Err(join_err) => Self::error(&responder, "E_INTERNAL", join_err).await,
        }
    }

    async fn handle_permission_status(&self, responder: Responder) {
        let screen_recording = temporal_macos_ffi::permissions::preflight().screen_recording;
        let accessibility = temporal_macos_ffi::ax::is_trusted();
        Self::respond(&responder, IpcResponse::PermissionStatus { screen_recording, accessibility })
            .await;
    }

    async fn handle_prune(
        &self,
        older_than_unix_ms: Option<i64>,
        keep_latest: Option<i32>,
        responder: Responder,
    ) {
        let storage = Arc::clone(&self.storage);
        let removed = match (older_than_unix_ms, keep_latest) {
            (Some(cutoff), None) => {
                tokio::task::spawn_blocking(move || storage.prune_older_than(cutoff)).await
            }
            (None, Some(keep)) => {
                let keep = keep.max(0) as usize;
                tokio::task::spawn_blocking(move || storage.prune_keep_latest(keep)).await
            }
            _ => {
                return Self::error(
                    &responder,
                    "E_INVALID",
                    "prune requires exactly one of olderThanUnixMs or keepLatest",
                )
                .await;
            }
        };
        match removed {
            Ok(Ok(n)) => {
                Self::respond(&responder, IpcResponse::Done { message: format!("pruned {n} workspace(s)") })
                    .await
            }
            Ok(Err(e)) => Self::error(&responder, "E_STORAGE", e).await,
            Err(join_err) => Self::error(&responder, "E_INTERNAL", join_err).await,
        }
    }

    async fn handle_request(&self, request_json: String, responder: Responder) {
        let request = match request_from_wire(&request_json) {
            Ok(request) => request,
            Err(e) => return Self::error(&responder, "E_DECODE", e).await,
        };
        match request {
            IpcRequest::Ping => Self::respond(&responder, IpcResponse::Pong).await,
            IpcRequest::Freeze => self.handle_freeze(responder).await,
            IpcRequest::Query { text, limit } => self.handle_query(text, limit, responder).await,
            IpcRequest::SummonIntent { text } => self.handle_summon(text, responder).await,
            IpcRequest::Rehydrate { payload } => self.handle_rehydrate(payload, responder).await,
            IpcRequest::RehydratePreview { payload } => {
                self.handle_rehydrate_preview(payload, responder).await
            }
            IpcRequest::PermissionStatus => self.handle_permission_status(responder).await,
            IpcRequest::Prune { older_than_unix_ms, keep_latest } => {
                self.handle_prune(older_than_unix_ms, keep_latest, responder).await
            }
        }
    }
}

impl Handler for DaemonHandler {
    fn handle(
        &self,
        request_json: String,
        responder: Responder,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.handle_request(request_json, responder))
    }
}

pub fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before 1970")
        .as_millis() as i64
}

/// Detached startup backfill: decomposes workspaces stored before item
/// indexing existed into items + groups, so old snapshots show groups on the
/// main menu and participate in prompt assembly. Per-workspace failures only
/// log; freezes maintain items inline from here on.
// Called by the binary (main.rs); the `#[path]` integration-test include
// compiles this file without main.rs, so allow the false "unused" there.
#[allow(dead_code)]
pub fn spawn_item_backfill(storage: Arc<Storage>, embedder: SharedEmbedder) {
    let handle = tokio::task::spawn_blocking(move || {
        let ids = match storage.workspace_ids_missing_items() {
            Ok(ids) => ids,
            Err(e) => {
                warn!(error = %e, "item backfill worklist failed");
                return;
            }
        };
        if ids.is_empty() {
            return;
        }
        info!(count = ids.len(), "backfilling items/groups for pre-existing workspaces");
        for workspace_id in ids {
            let run = || -> anyhow::Result<()> {
                let Some(record) = storage.get_workspace(&workspace_id)? else {
                    return Ok(()); // deleted in the meantime
                };
                let mut workspace = workspace_from_wire(&record.payload_json)
                    .map_err(|e| anyhow::anyhow!("stored payload undecodable: {e}"))?;
                let items = grouping::items_for(&workspace);
                let mut guard = embedder.lock().expect("embedder mutex poisoned");
                let mut vectors = Vec::with_capacity(items.len());
                for item in &items {
                    vectors.push(guard.embed_document(&item.embed_text)?);
                }
                drop(guard);
                if workspace.groups.is_empty() {
                    workspace.groups = grouping::build_groups(&items, &vectors);
                }
                let item_rows: Vec<(ItemRecord, Vec<f32>)> = items
                    .iter()
                    .zip(vectors)
                    .map(|(item, vector)| {
                        (item_record(&workspace_id, workspace.captured_at_unix_ms, item), vector)
                    })
                    .collect();
                let updated = WorkspaceRecord {
                    workspace_id: workspace_id.clone(),
                    captured_at_unix_ms: record.captured_at_unix_ms,
                    summary: workspace.summary.clone(),
                    tags_json: tags_to_wire(&workspace),
                    payload_json: workspace_to_wire(&workspace),
                };
                storage.upsert_workspace(&updated)?;
                storage.replace_items(&workspace_id, &item_rows)?;
                Ok(())
            };
            if let Err(e) = run() {
                warn!(workspace_id, error = %e, "item backfill failed for workspace");
            }
        }
        info!("item backfill complete");
    });
    supervise(handle, "item backfill");
}

/// Detached startup reconciliation of the installed-app catalog: scans the
/// application directories, drops entries for apps that were uninstalled, and
/// embeds only newly-installed apps (embeddings are deterministic from the
/// name + seed capabilities, so unchanged apps are skipped — the first run
/// embeds everything, later runs are near-free). Feeds Summon's speculative
/// launches. Failures only log; Summon degrades to history + live desktop.
// Called by the binary (main.rs); the `#[path]` integration-test include
// compiles this file without main.rs, so allow the false "unused" there.
#[allow(dead_code)]
pub fn spawn_catalog_sync(storage: Arc<Storage>, embedder: SharedEmbedder) {
    let handle = tokio::task::spawn_blocking(move || {
        let run = || -> anyhow::Result<()> {
            let installed = temporal_macos_ffi::catalog::scan_installed_apps();
            let scanned: std::collections::HashSet<String> =
                installed.iter().map(|a| a.bundle_id.clone()).collect();
            let stored = storage.app_catalog_bundle_ids()?;

            for gone in stored.difference(&scanned) {
                storage.delete_app_catalog_entry(gone)?;
            }
            let fresh: Vec<_> = installed.iter().filter(|a| !stored.contains(&a.bundle_id)).collect();
            if fresh.is_empty() {
                return Ok(());
            }
            info!(count = fresh.len(), "embedding newly-installed apps into catalog");
            for app in fresh {
                let capabilities: Vec<String> = catalog::seed_capabilities(&app.bundle_id)
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                let embed_text = catalog::catalog_embed_text(&app.app_name, &capabilities);
                let vector =
                    embedder.lock().expect("embedder mutex poisoned").embed_document(&embed_text)?;
                let record = AppCatalogRecord {
                    bundle_id: app.bundle_id.clone(),
                    app_name: app.app_name.clone(),
                    capabilities_json: serde_json::to_string(&capabilities)?,
                    embed_text,
                };
                storage.upsert_app_catalog_entry(&record, &vector)?;
            }
            info!("app catalog sync complete");
            Ok(())
        };
        if let Err(e) = run() {
            warn!(error = %e, "app catalog sync failed; summon uses history + live desktop only");
        }
    });
    supervise(handle, "app catalog sync");
}

/// How many raw item hits to pull before dedup/thresholding.
const ITEM_SEARCH_POOL: usize = 48;
/// Items scoring below this never enter an assembled workspace — better no
/// assembled candidate than one padded with weak matches.
const MIN_ASSEMBLED_SCORE: f64 = 0.55;
/// Items must also land within this margin of the query's best item: bge
/// similarity is only meaningful relative to the query (a vague query's top
/// hit may score ~0.6 where a sharp one's scores ~0.85), and off-topic items
/// consistently sit well below on-topic ones. Measured on real embeddings:
/// on-topic items cluster within ~0.17 of the top, off-topic ≥0.25 below.
const ASSEMBLED_SCORE_MARGIN: f64 = 0.18;
/// Upper bound on items in an assembled workspace.
const MAX_ASSEMBLED_ITEMS: usize = 10;

fn item_record(
    workspace_id: &str,
    captured_at_unix_ms: i64,
    item: &grouping::WorkspaceItem,
) -> ItemRecord {
    ItemRecord {
        workspace_id: workspace_id.to_string(),
        node_id: item.item_ref.node_id.clone(),
        tab_index: item.item_ref.tab_index.map(i64::from),
        kind: item.kind.as_str().to_string(),
        dedup_key: item.dedup_key.clone(),
        title: item.title.clone(),
        captured_at_unix_ms,
    }
}

/// Builds the full candidate list for one query. Non-empty query + embedder:
/// workspace KNN plus a prompt-assembled virtual workspace from item search.
/// Otherwise: recency order (score 0), with each workspace's stored groups
/// materialized as selectable sub-candidates on the empty-query main menu.
fn build_candidates(
    storage: &Storage,
    embedder: Option<&SharedEmbedder>,
    text: &str,
    limit: usize,
) -> anyhow::Result<Vec<QueryCandidate>> {
    let trimmed = text.trim();
    let query_vector = match (embedder, trimmed.is_empty()) {
        (Some(embedder), false) => {
            Some(embedder.lock().expect("embedder mutex poisoned").embed_query(trimmed)?)
        }
        _ => None,
    };

    let Some(vector) = query_vector else {
        let mut candidates = Vec::new();
        for record in storage.list_workspaces()?.into_iter().take(limit) {
            let Some(workspace) = decode_payload(&record) else { continue };
            let groups = if trimmed.is_empty() { workspace.groups.clone() } else { Vec::new() };
            let source_id = workspace.workspace_id.clone();
            candidates.push(QueryCandidate {
                workspace,
                score: 0.0,
                kind: CandidateKind::Workspace,
                source_workspace_id: None,
                speculative_node_ids: Vec::new(),
            });
            if groups.len() >= 2 {
                let parent = &candidates.last().expect("just pushed").workspace;
                let materialized: Vec<QueryCandidate> = groups
                    .iter()
                    .filter_map(|group| grouping::materialize_group(parent, &group.group_id))
                    .map(|workspace| QueryCandidate {
                        workspace,
                        score: 0.0,
                        kind: CandidateKind::Group,
                        source_workspace_id: Some(source_id.clone()),
                        speculative_node_ids: Vec::new(),
                    })
                    .collect();
                candidates.extend(materialized);
            }
        }
        return Ok(candidates);
    };

    let mut candidates = Vec::new();
    for (record, score) in storage.search_embeddings(&vector, limit)? {
        let Some(workspace) = decode_payload(&record) else { continue };
        candidates.push(QueryCandidate {
            workspace,
            score,
            kind: CandidateKind::Workspace,
            source_workspace_id: None,
            speculative_node_ids: Vec::new(),
        });
    }
    if let Some(assembled) = assemble_candidate(storage, &vector, trimmed)? {
        candidates.insert(0, assembled);
    }
    Ok(candidates)
}

/// Item-level search across every snapshot: dedup (freshest duplicate wins),
/// threshold, and synthesize the survivors into one virtual workspace. None
/// when fewer than two distinct items match well enough.
fn assemble_candidate(
    storage: &Storage,
    vector: &[f32],
    query_text: &str,
) -> anyhow::Result<Option<QueryCandidate>> {
    let hits = storage.search_items(vector, ITEM_SEARCH_POOL)?;

    // Hits arrive best-first: the first occurrence of an identity fixes its
    // rank, a fresher duplicate later only replaces the record behind it.
    let top_score = hits.first().map(|(_, score)| *score).unwrap_or(0.0);
    let cutoff = MIN_ASSEMBLED_SCORE.max(top_score - ASSEMBLED_SCORE_MARGIN);
    let mut best: Vec<(ItemRecord, f64)> = Vec::new();
    for (item, score) in hits {
        if score < cutoff {
            continue;
        }
        match best.iter_mut().find(|(kept, _)| kept.dedup_key == item.dedup_key) {
            Some((kept, _)) => {
                if item.captured_at_unix_ms > kept.captured_at_unix_ms {
                    *kept = item;
                }
            }
            None => best.push((item, score)),
        }
    }
    best.truncate(MAX_ASSEMBLED_ITEMS);
    if best.len() < 2 {
        return Ok(None);
    }

    // Load each source snapshot once.
    let mut sources: Vec<(String, WorkspaceState)> = Vec::new();
    for (item, _) in &best {
        if sources.iter().any(|(id, _)| id == &item.workspace_id) {
            continue;
        }
        let Some(record) = storage.get_workspace(&item.workspace_id)? else { continue };
        if let Some(workspace) = decode_payload(&record) {
            sources.push((item.workspace_id.clone(), workspace));
        }
    }

    let mut picks = Vec::new();
    let mut used = 0usize;
    let mut score_sum = 0.0;
    for (item, score) in &best {
        let Some((_, source)) = sources.iter().find(|(id, _)| id == &item.workspace_id) else {
            continue;
        };
        let Some(node) = source.nodes.iter().find(|n| n.node_id == item.node_id) else {
            warn!(workspace_id = %item.workspace_id, node_id = %item.node_id,
                  "item row points at a node missing from its payload");
            continue;
        };
        let mut node = node.clone();
        // Snapshots reuse "n0", "n1", …; qualify so picks from different
        // snapshots never merge as if they were one window.
        node.node_id = format!("{}::{}", item.workspace_id, item.node_id);
        picks.push(grouping::NodePick {
            node,
            tab_indices: item.tab_index.map(|index| vec![index as i32]),
        });
        used += 1;
        score_sum += score;
    }
    let nodes = grouping::synthesize_nodes(picks);
    if used < 2 || nodes.is_empty() {
        return Ok(None);
    }

    let mut workspace = WorkspaceState {
        workspace_id: format!("virtual-{}", uuid::Uuid::new_v4()),
        captured_at_unix_ms: unix_ms(),
        summary: format!("Assembled · {used} items for \"{query_text}\""),
        tags: Vec::new(),
        nodes,
        groups: Vec::new(),
    };
    workspace.tags = tagging::derive_tags(&workspace);
    Ok(Some(QueryCandidate {
        workspace,
        score: score_sum / used as f64,
        kind: CandidateKind::Assembled,
        source_workspace_id: None,
        speculative_node_ids: Vec::new(),
    }))
}

/// Past snapshots surfaced alongside a Summoned candidate, so the user can pick
/// "the real thing from last week" instead of the synthesis.
const SUMMON_WORKSPACE_ALTERNATES: usize = 5;
/// Catalog entries pulled by KNN before capability/threshold selection.
const CATALOG_SEARCH_POOL: usize = 24;
/// Most installed apps Summon will propose launching for one intent.
const SUMMON_MAX_CATALOG_PICKS: usize = 3;
/// Live-desktop items are noisier than cross-snapshot search — everything the
/// user has open counts, including unrelated projects — so Summon holds item
/// inclusion to a tighter absolute bar and margin than assembly, keeping the
/// synthesized workspace focused on the intent. A stray high-scoring "hub" tab
/// (bge maps some titles near many queries) can still slip through; the staging
/// preview lets the user uncheck it. Verified live — do not tune tighter
/// without checking multiple real desktops (synthetic tests pass where a real
/// desktop fails).
const SUMMON_MIN_ITEM_SCORE: f64 = 0.62;
const SUMMON_ITEM_MARGIN: f64 = 0.12;

/// A candidate item for the summoned workspace, from the live desktop or a
/// stored snapshot, scored against the intent.
struct ScoredPick {
    score: f64,
    /// Live picks win ties over historical ones: the open window is the truth.
    from_live: bool,
    dedup_key: String,
    pick: grouping::NodePick,
}

/// Cosine similarity of two L2-normalized embeddings, clamped to [0, 1] to
/// match the scores the sqlite-vec index returns.
fn cosine(a: &[f32], b: &[f32]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x * y) as f64).sum::<f64>().clamp(0.0, 1.0)
}

/// Builds the Summon response: a synthesized `Summoned` candidate (when the
/// intent resolves to anything) followed by the closest past snapshots as
/// alternates. With no embedder, degrades to the plain retrieval path.
fn build_summon_candidates(
    storage: &Storage,
    embedder: Option<&SharedEmbedder>,
    tagger: Option<&SharedTagger>,
    text: &str,
) -> anyhow::Result<Vec<QueryCandidate>> {
    let trimmed = text.trim();
    let (Some(embedder), false) = (embedder, trimmed.is_empty()) else {
        return build_candidates(storage, embedder, text, SUMMON_WORKSPACE_ALTERNATES);
    };

    let query_vector =
        embedder.lock().expect("embedder mutex poisoned").embed_query(trimmed)?;

    // Closest past snapshots as alternates to the synthesis.
    let mut candidates = Vec::new();
    for (record, score) in storage.search_embeddings(&query_vector, SUMMON_WORKSPACE_ALTERNATES)? {
        if let Some(workspace) = decode_payload(&record) {
            candidates.push(QueryCandidate {
                workspace,
                score,
                kind: CandidateKind::Workspace,
                source_workspace_id: None,
                speculative_node_ids: Vec::new(),
            });
        }
    }

    if let Some(summoned) =
        compose_summoned(storage, embedder, tagger, &query_vector, trimmed)?
    {
        candidates.insert(0, summoned);
    }
    Ok(candidates)
}

/// Synthesizes one workspace for the intent: item picks from the live desktop
/// and history (deduped, thresholded, tab-folded), plus speculative launches of
/// installed apps the intent implies. None when nothing clears the bar.
fn compose_summoned(
    storage: &Storage,
    embedder: &SharedEmbedder,
    tagger: Option<&SharedTagger>,
    query_vector: &[f32],
    text: &str,
) -> anyhow::Result<Option<QueryCandidate>> {
    let mut pool: Vec<ScoredPick> = Vec::new();

    // --- Live desktop: embed each open item, score against the intent. ---
    let live = temporal_adapters::extract_workspace();
    for warning in &live.warnings {
        warn!(warning, "summon live-extraction warning");
    }
    let live_ws = WorkspaceState {
        workspace_id: "live".into(),
        captured_at_unix_ms: unix_ms(),
        summary: String::new(),
        tags: Vec::new(),
        nodes: live.nodes,
        groups: Vec::new(),
    };
    let live_items = grouping::items_for(&live_ws);
    if !live_items.is_empty() {
        let mut guard = embedder.lock().expect("embedder mutex poisoned");
        for item in &live_items {
            let vector = guard.embed_document(&item.embed_text)?;
            if let Some(pick) = live_pick(&live_ws, &item.item_ref) {
                pool.push(ScoredPick {
                    score: cosine(query_vector, &vector),
                    from_live: true,
                    dedup_key: item.dedup_key.clone(),
                    pick,
                });
            }
        }
    }

    // --- History: item KNN across every snapshot. ---
    let hits = storage.search_items(query_vector, ITEM_SEARCH_POOL)?;
    let mut sources: Vec<(String, WorkspaceState)> = Vec::new();
    for (item, score) in hits {
        if !sources.iter().any(|(id, _)| id == &item.workspace_id)
            && let Some(record) = storage.get_workspace(&item.workspace_id)?
            && let Some(workspace) = decode_payload(&record)
        {
            sources.push((item.workspace_id.clone(), workspace));
        }
        let Some((_, source)) = sources.iter().find(|(id, _)| id == &item.workspace_id) else {
            continue;
        };
        let Some(node) = source.nodes.iter().find(|n| n.node_id == item.node_id) else {
            continue;
        };
        let mut node = node.clone();
        // Qualify so picks from different snapshots never merge as one window.
        node.node_id = format!("{}::{}", item.workspace_id, item.node_id);
        pool.push(ScoredPick {
            score,
            from_live: false,
            dedup_key: item.dedup_key.clone(),
            pick: grouping::NodePick {
                node,
                tab_indices: item.tab_index.map(|index| vec![index as i32]),
            },
        });
    }

    // --- Dedup (live wins, else higher score), threshold, cap. ---
    let picks = select_item_picks(pool);
    let mut nodes = grouping::synthesize_nodes(picks);
    let item_count = nodes.len();

    // --- Catalog: apps to launch that the intent implies but aren't open. ---
    let mut present = temporal_macos_ffi::bundle::running_bundle_ids();
    for node in &nodes {
        present.insert(node.bundle_id.clone());
    }
    let catalog_picks =
        catalog_launch_picks(storage, tagger, query_vector, text, &present)?;
    let mut speculative_node_ids = Vec::new();
    for cand in &catalog_picks {
        let node_id = format!("n{}", nodes.len());
        speculative_node_ids.push(node_id.clone());
        nodes.push(WindowNode {
            node_id,
            bundle_id: cand.bundle_id.clone(),
            app_name: cand.app_name.clone(),
            window_title: String::new(),
            geometry: WindowGeometry::default(),
            adapter: AdapterKind::Generic,
            payload: NodePayload::Generic,
        });
    }

    if nodes.is_empty() {
        return Ok(None);
    }

    let app_count = catalog_picks.len();
    let apps_clause =
        if app_count > 0 { format!(", {app_count} apps to open") } else { String::new() };
    let mut workspace = WorkspaceState {
        workspace_id: format!("summoned-{}", uuid::Uuid::new_v4()),
        captured_at_unix_ms: unix_ms(),
        summary: format!("Summoned for \"{text}\" · {item_count} items{apps_clause}"),
        tags: Vec::new(),
        nodes,
        groups: Vec::new(),
    };
    workspace.tags = tagging::derive_tags(&workspace);
    Ok(Some(QueryCandidate {
        workspace,
        score: 0.0,
        kind: CandidateKind::Summoned,
        source_workspace_id: None,
        speculative_node_ids,
    }))
}

/// A live item's node, id-qualified so it can't collide with historical picks.
fn live_pick(live_ws: &WorkspaceState, item_ref: &temporal_domain::ItemRef) -> Option<grouping::NodePick> {
    let mut node = live_ws.nodes.iter().find(|n| n.node_id == item_ref.node_id)?.clone();
    node.node_id = format!("live::{}", item_ref.node_id);
    Some(grouping::NodePick { node, tab_indices: item_ref.tab_index.map(|index| vec![index]) })
}

/// Dedups the scored pool by identity (live beats historical, then higher
/// score), keeps only picks within the relevance margin of the best, and caps
/// the count — reusing the assembled-workspace thresholds.
fn select_item_picks(pool: Vec<ScoredPick>) -> Vec<grouping::NodePick> {
    let mut best: Vec<ScoredPick> = Vec::new();
    for cand in pool {
        match best.iter_mut().find(|k| k.dedup_key == cand.dedup_key) {
            Some(kept) => {
                let better = (cand.from_live, cand.score) > (kept.from_live, kept.score);
                if better {
                    *kept = cand;
                }
            }
            None => best.push(cand),
        }
    }
    best.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    let top = best.first().map(|c| c.score).unwrap_or(0.0);
    let cutoff = SUMMON_MIN_ITEM_SCORE.max(top - SUMMON_ITEM_MARGIN);
    best.into_iter()
        .filter(|c| c.score >= cutoff)
        .take(MAX_ASSEMBLED_ITEMS)
        .map(|c| c.pick)
        .collect()
}

/// Selects installed apps to speculatively launch: KNN over the catalog, LLM
/// capability inference (when a tagger is loaded), and the pure selection rule.
fn catalog_launch_picks(
    storage: &Storage,
    tagger: Option<&SharedTagger>,
    query_vector: &[f32],
    text: &str,
    present: &std::collections::HashSet<String>,
) -> anyhow::Result<Vec<catalog::CatalogCandidate>> {
    let raw = storage.search_app_catalog(query_vector, CATALOG_SEARCH_POOL)?;
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let inferred_caps = match tagger {
        Some(tagger) => match tagger
            .lock()
            .expect("tagger mutex poisoned")
            .infer_capabilities(text, catalog::CAPABILITIES)
        {
            Ok(caps) => caps,
            Err(e) => {
                warn!(error = %e, "capability inference failed; catalog uses embedding match only");
                Vec::new()
            }
        },
        None => Vec::new(),
    };
    let candidates: Vec<catalog::CatalogCandidate> = raw
        .into_iter()
        .map(|(record, score)| catalog::CatalogCandidate {
            bundle_id: record.bundle_id,
            app_name: record.app_name,
            capabilities: serde_json::from_str(&record.capabilities_json).unwrap_or_default(),
            score,
        })
        .collect();
    Ok(catalog::select_catalog_picks(
        &candidates,
        &inferred_caps,
        present,
        SUMMON_MAX_CATALOG_PICKS,
    ))
}

fn decode_payload(record: &WorkspaceRecord) -> Option<WorkspaceState> {
    match workspace_from_wire(&record.payload_json) {
        Ok(workspace) => Some(workspace),
        Err(e) => {
            // A payload we wrote that no longer decodes is a bug, not a user
            // error; surface loudly but keep serving the rest.
            warn!(workspace_id = %record.workspace_id, error = %e, "stored payload failed to decode");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use temporal_domain::{
        AdapterKind, BrowserTab, RehydrationPayload, WindowGeometry, WindowNode, WorkspaceState,
    };

    fn browser_node(id: &str, tab_count: usize) -> WindowNode {
        WindowNode {
            node_id: id.into(),
            bundle_id: "com.google.Chrome".into(),
            app_name: "Google Chrome".into(),
            window_title: String::new(),
            geometry: WindowGeometry::default(),
            adapter: AdapterKind::Chrome,
            payload: NodePayload::Browser {
                tabs: vec![BrowserTab { url: "https://x".into(), title: String::new() }; tab_count],
                active_tab_index: 0,
            },
        }
    }

    fn payload(nodes: Vec<WindowNode>) -> RehydrationPayload {
        RehydrationPayload {
            workspace: WorkspaceState {
                workspace_id: "w".into(),
                captured_at_unix_ms: 0,
                summary: String::new(),
                tags: Vec::new(),
                nodes,
                groups: Vec::new(),
            },
            excluded_node_ids: Vec::new(),
        }
    }

    #[test]
    fn accepts_a_normal_payload() {
        let nodes = (0..10).map(|i| browser_node(&i.to_string(), 20)).collect();
        assert!(validate_payload_size(&payload(nodes)).is_ok());
    }

    #[test]
    fn rejects_too_many_nodes() {
        let nodes = (0..MAX_REHYDRATE_NODES + 1).map(|i| browser_node(&i.to_string(), 0)).collect();
        assert!(validate_payload_size(&payload(nodes)).is_err());
    }

    #[test]
    fn rejects_too_many_tabs_in_one_node() {
        let node = browser_node("big", MAX_TABS_PER_NODE + 1);
        assert!(validate_payload_size(&payload(vec![node])).is_err());
    }

    fn generic_node(id: &str) -> WindowNode {
        WindowNode {
            node_id: id.into(),
            bundle_id: "com.example.app".into(),
            app_name: "App".into(),
            window_title: String::new(),
            geometry: WindowGeometry::default(),
            adapter: AdapterKind::Generic,
            payload: NodePayload::Generic,
        }
    }

    fn scored(score: f64, from_live: bool, key: &str, id: &str) -> ScoredPick {
        ScoredPick {
            score,
            from_live,
            dedup_key: key.into(),
            pick: grouping::NodePick { node: generic_node(id), tab_indices: None },
        }
    }

    #[test]
    fn select_item_picks_prefers_live_and_thresholds() {
        let pool = vec![
            scored(0.90, false, "A", "hist-a"), // historical A, high score
            scored(0.72, true, "A", "live-a"),  // live A — wins dedup despite lower score
            scored(0.80, false, "B", "hist-b"), // within margin of the top
            scored(0.40, false, "C", "hist-c"), // below the relevance floor — dropped
        ];
        let picks = select_item_picks(pool);
        let ids: Vec<&str> = picks.iter().map(|p| p.node.node_id.as_str()).collect();
        // B (0.80) leads; A resolves to its live copy (0.72); C falls below the
        // cutoff = max(0.62, 0.80 - 0.12) = 0.68.
        assert_eq!(ids, vec!["hist-b", "live-a"]);
    }
}
