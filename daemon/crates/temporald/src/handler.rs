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
    planning, tagging, IpcRequest, IpcResponse, QueryCandidate, RehydrationPayload, WorkspaceState,
};
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
        let workspace_id = format!("ws-{}", uuid::Uuid::new_v4());
        let now_ms = unix_ms();
        Self::respond(&responder, IpcResponse::FreezeStarted { workspace_id: workspace_id.clone() })
            .await;
        Self::respond(
            &responder,
            IpcResponse::Progress {
                stage: "extract".into(),
                detail: "desktop state".into(),
                percent: 10,
            },
        )
        .await;

        let report = match tokio::task::spawn_blocking(temporal_adapters::extract_workspace).await {
            Ok(report) => report,
            Err(join_err) => return Self::error(&responder, "E_INTERNAL", join_err).await,
        };
        for warning in &report.warnings {
            warn!(warning, "extraction warning");
        }
        let node_count = report.nodes.len();
        Self::respond(
            &responder,
            IpcResponse::Progress {
                stage: "tag".into(),
                detail: format!("{node_count} windows captured"),
                percent: 60,
            },
        )
        .await;

        let workspace = tagging::enrich(WorkspaceState {
            workspace_id: workspace_id.clone(),
            captured_at_unix_ms: now_ms,
            summary: String::new(),
            tags: Vec::new(),
            nodes: report.nodes,
        });

        Self::respond(
            &responder,
            IpcResponse::Progress { stage: "persist".into(), detail: String::new(), percent: 90 },
        )
        .await;
        let record = WorkspaceRecord {
            workspace_id: workspace_id.clone(),
            captured_at_unix_ms: now_ms,
            summary: workspace.summary.clone(),
            tags_json: tags_to_wire(&workspace),
            payload_json: workspace_to_wire(&workspace),
        };
        let embedding_text = tagging::embedding_text(&workspace);
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
                Self::respond(
                    &responder,
                    IpcResponse::Done {
                        message: format!("froze workspace {workspace_id} ({node_count} windows)"),
                    },
                )
                .await;
                // LLM enrichment runs after Done: the freeze stays instant and
                // the record upgrades in place when generation finishes.
                self.spawn_llm_enrichment(workspace_id);
            }
            Ok(Err(e)) => Self::error(&responder, "E_STORAGE", e).await,
            Err(join_err) => Self::error(&responder, "E_INTERNAL", join_err).await,
        }
    }

    /// Detached: generate LLM summary/tags, merge over the heuristics,
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
                let workspace = workspace_from_wire(&record.payload_json)
                    .map_err(|e| anyhow::anyhow!("stored payload undecodable: {e}"))?;
                let context = tagging::embedding_text(&workspace);
                let result = tagger.lock().expect("tagger mutex poisoned").generate(&context)?;
                info!(workspace_id, summary = %result.summary, tags = ?result.tags, "llm tags generated");

                let enriched = tagging::apply_llm_tags(&result.summary, &result.tags, workspace);
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
    }

    async fn handle_query(&self, text: String, limit: i32, responder: Responder) {
        let storage = Arc::clone(&self.storage);
        let embedder = self.embedder.clone();
        let limit_n = limit.max(0) as usize;
        // Semantic KNN when the model is available; recency order (score 0)
        // otherwise, and for empty queries ("show me recent workspaces").
        let listed =
            tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<(WorkspaceRecord, f64)>> {
                match embedder {
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
            Ok(Err(e)) => return Self::error(&responder, "E_STORAGE", e).await,
            Err(join_err) => return Self::error(&responder, "E_INTERNAL", join_err).await,
        };

        let mut candidates = Vec::new();
        for (record, score) in records {
            match workspace_from_wire(&record.payload_json) {
                Ok(workspace) => candidates.push(QueryCandidate { workspace, score }),
                Err(e) => {
                    // A payload we wrote that no longer decodes is a bug, not
                    // a user error; surface loudly but keep serving the rest.
                    warn!(workspace_id = %record.workspace_id, error = %e, "stored payload failed to decode");
                }
            }
        }
        Self::respond(&responder, IpcResponse::QueryResults { candidates }).await;
    }

    async fn handle_rehydrate(&self, payload: RehydrationPayload, responder: Responder) {
        Self::respond(&responder, IpcResponse::RehydrateStarted).await;
        let nodes = planning::included_nodes(&payload.workspace, &payload.excluded_node_ids);
        let total = nodes.len();

        let (tx, mut rx) = tokio::sync::mpsc::channel::<(usize, String)>(32);
        let work = tokio::task::spawn_blocking(move || {
            temporal_adapters::rehydrate::rehydrate_nodes(&nodes, |i, label| {
                let _ = tx.blocking_send((i, label.to_string()));
            })
        });
        while let Some((i, label)) = rx.recv().await {
            let percent = (i * 100).checked_div(total).unwrap_or(100) as i32;
            Self::respond(
                &responder,
                IpcResponse::Progress { stage: "launch".into(), detail: label, percent },
            )
            .await;
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

    async fn handle_request(&self, request_json: String, responder: Responder) {
        let request = match request_from_wire(&request_json) {
            Ok(request) => request,
            Err(e) => return Self::error(&responder, "E_DECODE", e).await,
        };
        match request {
            IpcRequest::Ping => Self::respond(&responder, IpcResponse::Pong).await,
            IpcRequest::Freeze => self.handle_freeze(responder).await,
            IpcRequest::Query { text, limit } => self.handle_query(text, limit, responder).await,
            IpcRequest::Rehydrate { payload } => self.handle_rehydrate(payload, responder).await,
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
