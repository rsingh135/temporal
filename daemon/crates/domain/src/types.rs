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

/// Points at one embeddable/rehydratable unit inside a workspace: a whole
/// window node, or (for Browser nodes) a single tab within it. Tab indices
/// are stable because nodes are immutable after capture — enrichment only
/// rewrites summaries/tags/labels, never nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ItemRef {
    pub node_id: String,
    /// Some(i) = the i-th tab of a Browser node; None = the whole node.
    pub tab_index: Option<i32>,
}

/// A semantic activity cluster within one workspace ("coding", "writing").
/// Membership is fixed at freeze time; only the label upgrades later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkspaceGroup {
    /// "g0", "g1", … ordered by member count descending.
    pub group_id: String,
    pub label: String,
    pub items: Vec<ItemRef>,
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
    /// Semantic sub-groups; empty when clustering found no meaningful split.
    /// default keeps pre-grouping DB records and IPC frames decoding.
    #[serde(default)]
    pub groups: Vec<WorkspaceGroup>,
}

/// What the user approved in the staging preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RehydrationPayload {
    pub workspace: WorkspaceState,
    pub excluded_node_ids: Vec<String>,
}

/// Per-node dry-run of what rehydration would do, without launching anything.
/// Lets the UI warn about apps that vanished, windows that will be moved, and
/// tabs that will be skipped before the user commits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PreflightNode {
    pub node_id: String,
    pub app_name: String,
    pub adapter: AdapterKind,
    /// Whether the owning app appears installed (Spotlight-resolvable bundle id).
    pub installed: bool,
    /// Whether the captured window would be repositioned to fit an active
    /// display (e.g. its original monitor is unplugged).
    pub geometry_clamped: bool,
    /// Browser tabs captured vs. tabs that will be skipped for a disallowed URL
    /// scheme; both zero for non-browser nodes.
    pub total_tabs: u32,
    pub skipped_tabs: u32,
    /// Human-readable notes for anything the user should know before committing.
    pub issues: Vec<String>,
}

/// What a query candidate represents. `Group` and `Assembled` candidates
/// carry an already-materialized workspace, so staging/rehydration treat
/// every kind identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum CandidateKind {
    /// A whole frozen snapshot.
    #[default]
    Workspace,
    /// One semantic sub-group of a snapshot, materialized standalone.
    Group,
    /// A virtual workspace assembled from cross-snapshot item search.
    Assembled,
}

/// One semantic search hit; score is cosine similarity in [0, 1].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QueryCandidate {
    pub workspace: WorkspaceState,
    pub score: f64,
    #[serde(default)]
    pub kind: CandidateKind,
    /// For Group candidates: the snapshot this group was carved from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_workspace_id: Option<String>,
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
    /// Dry-run a rehydration: report per-node preflight without launching.
    RehydratePreview { payload: RehydrationPayload },
    /// Screen Recording / Accessibility permission check.
    PermissionStatus,
    /// Delete old workspace records. Exactly one field should be set.
    Prune {
        #[ts(type = "number | null")]
        older_than_unix_ms: Option<i64>,
        keep_latest: Option<i32>,
    },
}

/// Daemon -> UI responses. Progress and NodeResult may stream multiple times
/// before Done.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum IpcResponse {
    Pong,
    FreezeStarted { workspace_id: String },
    QueryResults { candidates: Vec<QueryCandidate> },
    RehydrateStarted,
    /// Result of a RehydratePreview: what each included node would do.
    RehydratePreview { nodes: Vec<PreflightNode> },
    Progress { stage: String, detail: String, percent: i32 },
    /// One rehydrated node's outcome, streamed as each node finishes.
    NodeResult { node_id: String, app_name: String, ok: bool, message: Option<String> },
    PermissionStatus { screen_recording: bool, accessibility: bool },
    Done { message: String },
    Error { code: String, message: String },
}
