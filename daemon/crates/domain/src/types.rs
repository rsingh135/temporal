//! Domain model. serde attributes ARE the wire format — change them and you
//! change what goes over the socket and into the DB. Tag/rename choices
//! deliberately match the original F# codec so pre-migration records decode.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Geometry of a single window in global (screen) coordinates, in points.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WindowGeometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Which adapter captured (and can rehydrate) a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum AdapterKind {
    Chrome,
    TerminalApp,
    #[serde(rename = "vscode")]
    VsCode,
    Cursor,
    Generic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BrowserTab {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TerminalTab {
    pub tty: String,
    /// Working directory of the shell attached to the tty.
    pub cwd: String,
}

/// Adapter-specific state carried by a window node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "lowercase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum NodePayload {
    Browser { tabs: Vec<BrowserTab>, active_tab_index: i32 },
    Terminal { tabs: Vec<TerminalTab> },
    Editor { folder_path: String, open_files: Vec<String> },
    Generic,
}

/// One captured window: the unit the user can toggle in the staging preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WindowNode {
    pub node_id: String,
    pub bundle_id: String,
    pub app_name: String,
    pub window_title: String,
    pub geometry: WindowGeometry,
    pub adapter: AdapterKind,
    pub payload: NodePayload,
}

/// A frozen desktop: flat record, overwritten in place (no history).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkspaceState {
    pub workspace_id: String,
    /// i64 on the Rust side, but the wire is JSON and the UI consumes it via
    /// JSON.parse — so the TS type is number (unix ms fits in a double).
    #[ts(type = "number")]
    pub captured_at_unix_ms: i64,
    pub summary: String,
    pub tags: Vec<String>,
    pub nodes: Vec<WindowNode>,
}

/// What the user approved in the staging preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RehydrationPayload {
    pub workspace: WorkspaceState,
    pub excluded_node_ids: Vec<String>,
}

/// One semantic search hit; score is cosine similarity in [0, 1].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueryCandidate {
    pub workspace: WorkspaceState,
    pub score: f64,
}

/// UI -> daemon requests over the Unix domain socket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum IpcRequest {
    Ping,
    Freeze,
    Query { text: String, limit: i32 },
    Rehydrate { payload: RehydrationPayload },
}

/// Daemon -> UI responses. Progress may stream multiple times before Done.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum IpcResponse {
    Pong,
    FreezeStarted { workspace_id: String },
    QueryResults { candidates: Vec<QueryCandidate> },
    RehydrateStarted,
    Progress { stage: String, detail: String, percent: i32 },
    Done { message: String },
    Error { code: String, message: String },
}
