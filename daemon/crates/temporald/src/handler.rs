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
    enrich as tagging_enrich_raw, tagsToWire as tagging_tags_to_wire_raw,
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
use temporal_ipc::{Handler, Responder};
use temporal_storage::{Storage, WorkspaceRecord};
use tracing::{info, warn};

pub struct DaemonHandler {
    storage: Arc<Storage>,
}

impl DaemonHandler {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
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
            payload_json: workspaceToWire(workspace).to_string(),
        };
        let storage = Arc::clone(&self.storage);
        let stored = tokio::task::spawn_blocking(move || storage.upsert_workspace(&record)).await;
        match stored {
            Ok(Ok(())) => {
                info!(workspace_id, node_count, "workspace frozen");
                Self::respond(&responder, IpcResponse::Done(fromString(format!(
                    "froze workspace {workspace_id} ({node_count} windows)"
                ))))
                .await;
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

    async fn handle_query(&self, _text: String, limit: i32, responder: Responder) {
        // Semantic ranking lands in M4; until then return recency-ordered
        // workspaces with a zero score so the UI flow can be built against
        // real payloads.
        let storage = Arc::clone(&self.storage);
        let listed = tokio::task::spawn_blocking(move || storage.list_workspaces()).await;
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
        for record in records.into_iter().take(limit.max(0) as usize) {
            match workspaceFromWire(fromString(record.payload_json.clone())) {
                Ok(workspace) => {
                    candidates.push(LrcPtr::new(QueryCandidate { Workspace: workspace, Score: 0.0 }));
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
