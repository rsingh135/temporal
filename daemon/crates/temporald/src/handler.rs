//! Bridges IPC frames to the Fable-generated domain: decode request, run the
//! daemon-side behavior, encode responses. All wire JSON comes from the shared
//! F# codec — this file never hand-builds JSON.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use fable_library_rust::List_;
use fable_library_rust::Native_::LrcPtr;
use fable_library_rust::String_::fromString;
use temporal_core::Temporal::Domain::Codecs::{
    requestFromWire, responseToWire, workspaceFromWire, workspaceToWire,
};
use temporal_core::Temporal::Domain::Tagging::{
    applyLlmTags, embeddingText as tagging_embedding_text_raw, enrich as tagging_enrich_raw,
    tagsToWire as tagging_tags_to_wire_raw,
};
use temporal_core::Temporal::Domain::Types::{
    IpcRequest, IpcResponse, QueryCandidate, WorkspaceState,
};

fn tagging_enrich(w: LrcPtr<WorkspaceState>) -> LrcPtr<WorkspaceState> {
    tagging_enrich_raw(w)
}

fn tagging_tags_to_wire(w: LrcPtr<WorkspaceState>) -> String {
    tagging_tags_to_wire_raw(w).to_string()
}

fn tagging_embedding_text(w: LrcPtr<WorkspaceState>) -> String {
    tagging_embedding_text_raw(w).to_string()
}

fn tagging_apply_llm(
    summary: String,
    tags: Vec<String>,
    w: LrcPtr<WorkspaceState>,
) -> LrcPtr<WorkspaceState> {
    applyLlmTags(
        fromString(summary),
        crate::convert::to_list(tags.into_iter().map(fromString).collect()),
        w,
    )
}
use temporal_ipc::{Handler, Responder};
use temporal_storage::{Storage, WorkspaceRecord};
use tracing::{info, warn};

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
        let wire = responseToWire(LrcPtr::new(response)).to_string();
        if responder.send(wire).await.is_err() {
            warn!("client went away mid-response");
        }
    }

    async fn handle_freeze(&self, responder: Responder) {
        let workspace_id = format!("ws-{}", uuid::Uuid::new_v4());
        let now_ms = unix_ms();
        Self::respond(&responder, IpcResponse::FreezeStarted(fromString(workspace_id.clone()))).await;
        Self::respond(
            &responder,
            IpcResponse::Progress(fromString("extract".into()), fromString("desktop state".into()), 10),
        )
        .await;

        let report = match tokio::task::spawn_blocking(temporal_adapters::extract_workspace).await {
            Ok(report) => report,
            Err(join_err) => {
                Self::respond(
                    &responder,
                    IpcResponse::IpcError(fromString("E_INTERNAL".into()), fromString(join_err.to_string())),
                )
                .await;
                return;
            }
        };
        for warning in &report.warnings {
            warn!(warning, "extraction warning");
        }
        let node_count = report.nodes.len();
        Self::respond(
            &responder,
            IpcResponse::Progress(
                fromString("tag".into()),
                fromString(format!("{node_count} windows captured")),
                60,
            ),
        )
        .await;

        let workspace = tagging_enrich(crate::convert::to_workspace(
            workspace_id.clone(),
            now_ms,
            report.nodes,
        ));

        Self::respond(
            &responder,
            IpcResponse::Progress(fromString("persist".into()), fromString(String::new()), 90),
        )
        .await;
        let record = WorkspaceRecord {
            workspace_id: workspace_id.clone(),
            captured_at_unix_ms: now_ms,
            summary: workspace.Summary.to_string(),
            tags_json: tagging_tags_to_wire(workspace.clone()),
            payload_json: workspaceToWire(workspace.clone()).to_string(),
        };
        let embedding_text = tagging_embedding_text(workspace);
        let storage = Arc::clone(&self.storage);
        let embedder = self.embedder.clone();
        let id_for_store = workspace_id.clone();
        let stored = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            storage.upsert_workspace(&record)?;
            if let Some(embedder) = embedder {
                let vector = embedder
                    .lock()
                    .expect("embedder mutex poisoned")
                    .embed_document(&embedding_text)?;
                storage.upsert_embedding(&id_for_store, &vector)?;
            }
            Ok(())
        })
        .await;
        match stored {
            Ok(Ok(())) => {
                info!(workspace_id, node_count, "workspace frozen");
                Self::respond(&responder, IpcResponse::Done(fromString(format!(
                    "froze workspace {workspace_id} ({node_count} windows)"
                ))))
                .await;
                // LLM enrichment runs after Done: the freeze stays instant and
                // the record upgrades in place when generation finishes.
                self.spawn_llm_enrichment(workspace_id);
            }
            Ok(Err(e)) => {
                Self::respond(
                    &responder,
                    IpcResponse::IpcError(fromString("E_STORAGE".into()), fromString(e.to_string())),
                )
                .await;
            }
            Err(join_err) => {
                Self::respond(
                    &responder,
                    IpcResponse::IpcError(fromString("E_INTERNAL".into()), fromString(join_err.to_string())),
                )
                .await;
            }
        }
    }

    /// Detached: generate LLM summary/tags, merge via the shared F# logic,
    /// re-persist and re-embed. Failures only log — heuristics remain.
    fn spawn_llm_enrichment(&self, workspace_id: String) {
        let Some(tagger) = self.tagger.clone() else { return };
        let storage = Arc::clone(&self.storage);
        let embedder = self.embedder.clone();
        tokio::task::spawn_blocking(move || {
            let run = || -> anyhow::Result<()> {
                let Some(record) = storage.get_workspace(&workspace_id)? else {
                    return Ok(()); // deleted/overwritten in the meantime
                };
                let workspace = workspaceFromWire(fromString(record.payload_json.clone()))
                    .map_err(|e| anyhow::anyhow!("stored payload undecodable: {e}"))?;
                let context = tagging_embedding_text(workspace.clone());
                let result = tagger.lock().expect("tagger mutex poisoned").generate(&context)?;
                info!(workspace_id, summary = %result.summary, tags = ?result.tags, "llm tags generated");

                let enriched = tagging_apply_llm(
                    result.summary,
                    result.tags,
                    workspace,
                );
                let updated = WorkspaceRecord {
                    workspace_id: workspace_id.clone(),
                    captured_at_unix_ms: record.captured_at_unix_ms,
                    summary: enriched.Summary.to_string(),
                    tags_json: tagging_tags_to_wire(enriched.clone()),
                    payload_json: workspaceToWire(enriched.clone()).to_string(),
                };
                storage.upsert_workspace(&updated)?;
                if let Some(embedder) = embedder {
                    let vector = embedder
                        .lock()
                        .expect("embedder mutex poisoned")
                        .embed_document(&tagging_embedding_text(enriched))?;
                    storage.upsert_embedding(&workspace_id, &vector)?;
                }
                Ok(())
            };
            if let Err(e) = run() {
                warn!(error = %e, "llm enrichment failed; heuristic tags kept");
            }
        });
    }

    async fn handle_query(&self, text: String, limit: i32, responder: Responder) {
        let storage = Arc::clone(&self.storage);
        let embedder = self.embedder.clone();
        let limit_n = limit.max(0) as usize;
        // Semantic KNN when the model is available; recency order (score 0)
        // otherwise, so the UI keeps working without the model download.
        let listed = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<(WorkspaceRecord, f64)>> {
            match embedder {
                // An empty query means "show me recent workspaces", not
                // "rank by similarity to the empty string".
                Some(_) | None if text.trim().is_empty() => Ok(storage
                    .list_workspaces()?
                    .into_iter()
                    .take(limit_n)
                    .map(|r| (r, 0.0))
                    .collect()),
                Some(embedder) => {
                    let vector =
                        embedder.lock().expect("embedder mutex poisoned").embed_query(&text)?;
                    Ok(storage.search_embeddings(&vector, limit_n)?)
                }
                None => Ok(storage
                    .list_workspaces()?
                    .into_iter()
                    .take(limit_n)
                    .map(|r| (r, 0.0))
                    .collect()),
            }
        })
        .await;
        let records = match listed {
            Ok(Ok(records)) => records,
            Ok(Err(e)) => {
                Self::respond(
                    &responder,
                    IpcResponse::IpcError(fromString("E_STORAGE".into()), fromString(e.to_string())),
                )
                .await;
                return;
            }
            Err(join_err) => {
                Self::respond(
                    &responder,
                    IpcResponse::IpcError(fromString("E_INTERNAL".into()), fromString(join_err.to_string())),
                )
                .await;
                return;
            }
        };

        let mut candidates: Vec<LrcPtr<QueryCandidate>> = Vec::new();
        for (record, score) in records {
            match workspaceFromWire(fromString(record.payload_json.clone())) {
                Ok(workspace) => {
                    candidates.push(LrcPtr::new(QueryCandidate { Workspace: workspace, Score: score }));
                }
                Err(e) => {
                    // A payload we wrote that no longer decodes is a bug, not
                    // a user error; surface loudly but keep serving the rest.
                    warn!(workspace_id = %record.workspace_id, error = %e, "stored payload failed to decode");
                }
            }
        }
        let mut list = List_::empty();
        for candidate in candidates.into_iter().rev() {
            list = List_::cons(candidate, list);
        }
        Self::respond(&responder, IpcResponse::QueryResults(list)).await;
    }

    async fn handle_request(&self, request_json: String, responder: Responder) {
        let request = match requestFromWire(fromString(request_json)) {
            Ok(request) => request,
            Err(e) => {
                Self::respond(
                    &responder,
                    IpcResponse::IpcError(fromString("E_DECODE".into()), fromString(e.to_string())),
                )
                .await;
                return;
            }
        };
        match request.as_ref() {
            IpcRequest::Ping => Self::respond(&responder, IpcResponse::Pong).await,
            IpcRequest::Freeze => self.handle_freeze(responder).await,
            IpcRequest::Query(text, limit) => {
                self.handle_query(text.to_string(), *limit, responder).await
            }
            IpcRequest::Rehydrate(_payload) => {
                Self::respond(
                    &responder,
                    IpcResponse::IpcError(
                        fromString("E_NOT_IMPLEMENTED".into()),
                        fromString("rehydration lands in M6".into()),
                    ),
                )
                .await;
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
