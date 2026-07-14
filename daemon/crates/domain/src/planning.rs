//! Rehydration planning: which captured nodes survive the user's staging
//! exclusions. (Toggle bookkeeping itself lives in the UI; this is the rule
//! the daemon enforces.)

use crate::types::{WindowNode, WorkspaceState};

/// The nodes that will actually be rehydrated.
pub fn included_nodes(workspace: &WorkspaceState, excluded_node_ids: &[String]) -> Vec<WindowNode> {
    workspace
        .nodes
        .iter()
        .filter(|n| !excluded_node_ids.contains(&n.node_id))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn node(id: &str) -> WindowNode {
        WindowNode {
            node_id: id.into(),
            bundle_id: String::new(),
            app_name: String::new(),
            window_title: String::new(),
            geometry: WindowGeometry::default(),
            adapter: AdapterKind::Generic,
            payload: NodePayload::Generic,
        }
    }

    #[test]
    fn drops_excluded_and_ignores_stale_ids() {
        let ws = WorkspaceState {
            workspace_id: "ws".into(),
            captured_at_unix_ms: 0,
            summary: String::new(),
            tags: vec![],
            nodes: vec![node("n0"), node("n1"), node("n2")],
            groups: vec![],
        };
        let included = included_nodes(&ws, &["n1".into(), "ghost".into()]);
        let ids: Vec<&str> = included.iter().map(|n| n.node_id.as_str()).collect();
        assert_eq!(ids, vec!["n0", "n2"]);
    }
}
