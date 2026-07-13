//! Wire helpers: the socket frames and the DB `payload_json` column both
//! carry serde_json of the domain types.

use crate::types::{IpcRequest, IpcResponse, WorkspaceState};

pub fn request_to_wire(request: &IpcRequest) -> String {
    serde_json::to_string(request).expect("domain types always serialize")
}

pub fn request_from_wire(json: &str) -> Result<IpcRequest, String> {
    serde_json::from_str(json).map_err(|e| e.to_string())
}

pub fn response_to_wire(response: &IpcResponse) -> String {
    serde_json::to_string(response).expect("domain types always serialize")
}

pub fn response_from_wire(json: &str) -> Result<IpcResponse, String> {
    serde_json::from_str(json).map_err(|e| e.to_string())
}

pub fn workspace_to_wire(workspace: &WorkspaceState) -> String {
    serde_json::to_string(workspace).expect("domain types always serialize")
}

pub fn workspace_from_wire(json: &str) -> Result<WorkspaceState, String> {
    serde_json::from_str(json).map_err(|e| e.to_string())
}

/// The tags list as JSON, for the denormalized DB column.
pub fn tags_to_wire(workspace: &WorkspaceState) -> String {
    serde_json::to_string(&workspace.tags).expect("strings always serialize")
}
