//! Semantic sub-grouping and virtual-workspace assembly: the pure rules.
//!
//! An *item* is the unit of semantic search — one Chrome tab, or one whole
//! non-browser window (Chrome is the only adapter whose windows split; the
//! user confirmed tab-level grouping for Chrome specifically). Embeddings and
//! clustering happen in the daemon; everything here is deterministic math and
//! bookkeeping so it can be unit-tested without models.

use crate::tagging::{base_name, url_host};
use crate::types::{ItemRef, NodePayload, WindowNode, WorkspaceGroup, WorkspaceState};

/// Minimum average pairwise cosine similarity for two clusters to merge.
/// The one empirical knob: measured on a real desktop's bge-small vectors,
/// same-activity items pair at ~0.65-0.82 and cross-activity at ~0.45-0.63.
/// Tune if groups look too coarse (raise) or too fragmented (lower).
pub const GROUP_SIMILARITY_THRESHOLD: f32 = 0.65;

/// Hard cap on groups per workspace; overflow merges into the last group.
pub const MAX_GROUPS: usize = 6;

/// What kind of desktop entity an item is (not on the wire; storage keeps it
/// as a lowercase string).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Tab,
    Terminal,
    Editor,
    Generic,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::Tab => "tab",
            ItemKind::Terminal => "terminal",
            ItemKind::Editor => "editor",
            ItemKind::Generic => "generic",
        }
    }
}

/// One embeddable/deduplicatable unit extracted from a workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceItem {
    pub item_ref: ItemRef,
    pub kind: ItemKind,
    /// Identity across snapshots: the same tab URL / editor folder / terminal
    /// cwd set captured twice is one item as far as assembly is concerned.
    pub dedup_key: String,
    /// Short human label (UI rows, labeling prompts).
    pub title: String,
    /// Text whose embedding represents this item: content first, light app/
    /// host context after. No generic kind prefix ("browser tab:", …) — on
    /// real desktops such prefixes dominate the vector and make clustering
    /// collapse by item kind instead of by topic.
    pub embed_text: String,
}

/// Strips a URL fragment: the same page scrolled to two anchors is one tab.
fn strip_fragment(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

/// Enumerates the items of a workspace: one per Chrome tab, one per
/// non-browser node. `materialize_group` resolves the resulting ItemRefs, so
/// the two functions must agree on tab-index semantics (see tests).
pub fn items_for(w: &WorkspaceState) -> Vec<WorkspaceItem> {
    let mut items = Vec::new();
    for node in &w.nodes {
        match &node.payload {
            NodePayload::Browser { tabs, .. } => {
                for (index, tab) in tabs.iter().enumerate() {
                    let host = url_host(&tab.url);
                    let title =
                        if tab.title.is_empty() { host.clone() } else { tab.title.clone() };
                    items.push(WorkspaceItem {
                        item_ref: ItemRef {
                            node_id: node.node_id.clone(),
                            tab_index: Some(index as i32),
                        },
                        kind: ItemKind::Tab,
                        dedup_key: strip_fragment(&tab.url).to_string(),
                        embed_text: format!("{title} ({host})"),
                        title,
                    });
                }
            }
            NodePayload::Terminal { tabs } => {
                let mut cwds: Vec<&str> =
                    tabs.iter().map(|t| t.cwd.as_str()).filter(|c| !c.is_empty()).collect();
                cwds.sort_unstable();
                cwds.dedup();
                let first = cwds.first().copied().unwrap_or("");
                items.push(WorkspaceItem {
                    item_ref: ItemRef { node_id: node.node_id.clone(), tab_index: None },
                    kind: ItemKind::Terminal,
                    dedup_key: cwds.join("\n"),
                    title: format!("{} — {}", node.app_name, base_name(first)),
                    embed_text: format!(
                        "{} {} {}",
                        node.app_name,
                        base_name(first),
                        cwds.join(" ")
                    ),
                });
            }
            NodePayload::Editor { folder_path, open_files } => {
                items.push(WorkspaceItem {
                    item_ref: ItemRef { node_id: node.node_id.clone(), tab_index: None },
                    kind: ItemKind::Editor,
                    dedup_key: folder_path.clone(),
                    title: format!("{} — {}", node.app_name, base_name(folder_path)),
                    embed_text: format!(
                        "{}: {} {} {}",
                        node.app_name,
                        base_name(folder_path),
                        folder_path,
                        open_files
                            .iter()
                            .map(|f| base_name(f))
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                    .trim_end()
                    .to_string(),
                });
            }
            NodePayload::Generic => {
                items.push(WorkspaceItem {
                    item_ref: ItemRef { node_id: node.node_id.clone(), tab_index: None },
                    kind: ItemKind::Generic,
                    dedup_key: format!("{}\u{1}{}", node.bundle_id, node.window_title),
                    title: node.app_name.clone(),
                    embed_text: format!("{}: {}", node.app_name, node.window_title)
                        .trim_end_matches([':', ' '])
                        .to_string(),
                });
            }
        }
    }
    items
}

/// Greedy average-linkage agglomerative clustering over unit vectors
/// (cosine similarity = dot product). Merges the highest-average pair while
/// it stays at or above `threshold`; ties break toward the lowest indices so
/// results are deterministic. Returns clusters of input indices, largest
/// first. O(n³) — fine at desktop scale (n ≤ ~150).
pub fn cluster(embeddings: &[Vec<f32>], threshold: f32) -> Vec<Vec<usize>> {
    let n = embeddings.len();
    let mut clusters: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    if n == 0 {
        return clusters;
    }
    let sim = |a: usize, b: usize| -> f32 {
        embeddings[a].iter().zip(&embeddings[b]).map(|(x, y)| x * y).sum()
    };
    loop {
        let mut best: Option<(usize, usize, f32)> = None;
        for i in 0..clusters.len() {
            for j in (i + 1)..clusters.len() {
                let mut total = 0.0f32;
                for &a in &clusters[i] {
                    for &b in &clusters[j] {
                        total += sim(a, b);
                    }
                }
                let average = total / (clusters[i].len() * clusters[j].len()) as f32;
                if best.is_none_or(|(_, _, s)| average > s) {
                    best = Some((i, j, average));
                }
            }
        }
        match best {
            Some((i, j, s)) if s >= threshold => {
                let merged = clusters.remove(j);
                clusters[i].extend(merged);
            }
            _ => break,
        }
    }
    // Largest first; tie-break on first member index for determinism.
    clusters.sort_by_key(|c| (std::cmp::Reverse(c.len()), c.first().copied()));
    if clusters.len() > MAX_GROUPS {
        let overflow: Vec<usize> = clusters.split_off(MAX_GROUPS).into_iter().flatten().collect();
        clusters.last_mut().expect("MAX_GROUPS > 0").extend(overflow);
    }
    clusters
}

/// Cheap deterministic label from member content: project folders and URL
/// hosts first (highest signal), app names as fallback. Same philosophy as
/// `tagging::derive_summary` — the floor the LLM later replaces.
pub fn heuristic_group_label(members: &[&WorkspaceItem]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut push = |value: String| {
        if !value.is_empty() && !parts.contains(&value) && parts.len() < 3 {
            parts.push(value);
        }
    };
    for item in members {
        match item.kind {
            ItemKind::Editor | ItemKind::Terminal => push(base_name(&item.dedup_key)),
            _ => {}
        }
    }
    for item in members {
        if item.kind == ItemKind::Tab {
            push(url_host(&item.dedup_key));
        }
    }
    for item in members {
        if item.kind == ItemKind::Generic {
            push(item.title.clone());
        }
    }
    if parts.is_empty() {
        "group".to_string()
    } else {
        parts.join(" · ")
    }
}

/// Clusters items and packages the result as wire-ready groups. Returns
/// empty unless at least two clusters form — a single group carries no more
/// information than the workspace itself.
pub fn build_groups(items: &[WorkspaceItem], embeddings: &[Vec<f32>]) -> Vec<WorkspaceGroup> {
    debug_assert_eq!(items.len(), embeddings.len());
    let clusters = cluster(embeddings, GROUP_SIMILARITY_THRESHOLD);
    if clusters.len() < 2 {
        return Vec::new();
    }
    clusters
        .into_iter()
        .enumerate()
        .map(|(index, member_indices)| {
            let members: Vec<&WorkspaceItem> =
                member_indices.iter().map(|&i| &items[i]).collect();
            WorkspaceGroup {
                group_id: format!("g{index}"),
                label: heuristic_group_label(&members),
                items: member_indices.iter().map(|&i| items[i].item_ref.clone()).collect(),
            }
        })
        .collect()
}

/// A resolved selection from a source workspace: a node, plus (for Browser
/// nodes) which of its tabs to keep. `tab_indices: None` keeps the node
/// whole.
#[derive(Debug, Clone)]
pub struct NodePick {
    pub node: WindowNode,
    pub tab_indices: Option<Vec<i32>>,
}

/// Merges picks into standalone nodes ready for a synthesized workspace.
/// Multiple Browser picks against the same source node collapse into ONE new
/// Browser node holding only the picked tabs (the originally-active tab stays
/// active if picked, else the first). Non-browser picks pass through. Node
/// ids are reassigned "n0", "n1", … so the result stands alone.
pub fn synthesize_nodes(picks: Vec<NodePick>) -> Vec<WindowNode> {
    // Fold tab picks per source node, preserving first-seen node order.
    let mut order: Vec<String> = Vec::new();
    let mut merged: Vec<(String, NodePick)> = Vec::new();
    for pick in picks {
        let key = pick.node.node_id.clone();
        match merged.iter_mut().find(|(k, _)| *k == key) {
            Some((_, existing)) => {
                if let (Some(current), Some(new)) =
                    (existing.tab_indices.as_mut(), pick.tab_indices)
                {
                    for index in new {
                        if !current.contains(&index) {
                            current.push(index);
                        }
                    }
                }
                // A duplicate whole-node pick (or mixing whole + tabs, which
                // items_for never produces) keeps the first pick's shape.
            }
            None => {
                order.push(key.clone());
                merged.push((key, pick));
            }
        }
    }

    let mut nodes = Vec::new();
    for key in order {
        let (_, pick) = merged.iter().find(|(k, _)| *k == key).expect("just inserted");
        let mut node = pick.node.clone();
        if let (Some(indices), NodePayload::Browser { tabs, active_tab_index }) =
            (&pick.tab_indices, &node.payload)
        {
            let mut sorted = indices.clone();
            sorted.sort_unstable();
            let kept: Vec<(i32, _)> = sorted
                .iter()
                .filter_map(|&i| tabs.get(i as usize).map(|t| (i, t.clone())))
                .collect();
            if kept.is_empty() {
                continue; // every picked index was stale
            }
            let active = kept
                .iter()
                .position(|(i, _)| *i == *active_tab_index)
                .unwrap_or(0) as i32;
            node.payload = NodePayload::Browser {
                tabs: kept.into_iter().map(|(_, t)| t).collect(),
                active_tab_index: active,
            };
        }
        node.node_id = format!("n{}", nodes.len());
        nodes.push(node);
    }
    nodes
}

/// Resolves one group of `ws` into a standalone rehydratable workspace.
/// Stale ItemRefs (node/tab no longer present) are skipped; returns None if
/// the group id is unknown or nothing resolves.
pub fn materialize_group(ws: &WorkspaceState, group_id: &str) -> Option<WorkspaceState> {
    let group = ws.groups.iter().find(|g| g.group_id == group_id)?;
    let picks: Vec<NodePick> = group
        .items
        .iter()
        .filter_map(|item| {
            let node = ws.nodes.iter().find(|n| n.node_id == item.node_id)?;
            match item.tab_index {
                Some(index) => {
                    let NodePayload::Browser { tabs, .. } = &node.payload else { return None };
                    if (index as usize) >= tabs.len() {
                        return None;
                    }
                    Some(NodePick { node: node.clone(), tab_indices: Some(vec![index]) })
                }
                None => Some(NodePick { node: node.clone(), tab_indices: None }),
            }
        })
        .collect();
    let nodes = synthesize_nodes(picks);
    if nodes.is_empty() {
        return None;
    }
    let materialized = WorkspaceState {
        workspace_id: format!("{}::{}", ws.workspace_id, group.group_id),
        captured_at_unix_ms: ws.captured_at_unix_ms,
        summary: group.label.clone(),
        tags: Vec::new(),
        nodes,
        groups: Vec::new(),
    };
    // Tags derived from just the included nodes, so search/UI chips stay
    // truthful to what this slice actually contains.
    Some(WorkspaceState { tags: crate::tagging::derive_tags(&materialized), ..materialized })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AdapterKind, BrowserTab, TerminalTab, WindowGeometry};

    fn browser_node(id: &str, tabs: &[(&str, &str)], active: i32) -> WindowNode {
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
                active_tab_index: active,
            },
        }
    }

    fn terminal_node(id: &str, cwds: &[&str]) -> WindowNode {
        WindowNode {
            node_id: id.into(),
            bundle_id: "com.apple.Terminal".into(),
            app_name: "Terminal".into(),
            window_title: String::new(),
            geometry: WindowGeometry::default(),
            adapter: AdapterKind::TerminalApp,
            payload: NodePayload::Terminal {
                tabs: cwds
                    .iter()
                    .map(|cwd| TerminalTab { tty: "/dev/ttys001".into(), cwd: cwd.to_string() })
                    .collect(),
            },
        }
    }

    fn editor_node(id: &str, folder: &str) -> WindowNode {
        WindowNode {
            node_id: id.into(),
            bundle_id: "com.microsoft.VSCode".into(),
            app_name: "Visual Studio Code".into(),
            window_title: base_name(folder),
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

    fn workspace(nodes: Vec<WindowNode>) -> WorkspaceState {
        WorkspaceState {
            workspace_id: "ws".into(),
            captured_at_unix_ms: 42,
            summary: String::new(),
            tags: vec![],
            nodes,
            groups: vec![],
        }
    }

    /// Unit vector along one of four orthogonal axes, nudged by `jitter`
    /// within the axis pair so same-family vectors are similar but unequal.
    fn axis_vec(axis: usize, jitter: f32) -> Vec<f32> {
        let mut v = vec![0.0f32; 8];
        v[axis * 2] = 1.0;
        v[axis * 2 + 1] = jitter;
        let norm = (1.0 + jitter * jitter).sqrt();
        v.iter().map(|x| x / norm).collect()
    }

    #[test]
    fn items_for_splits_chrome_tabs_and_keeps_others_atomic() {
        let ws = workspace(vec![
            browser_node(
                "n0",
                &[("https://github.com/a/b", "repo"), ("https://docs.rs/serde#derive", "serde")],
                1,
            ),
            terminal_node("n1", &["/Users/dev/temporal", "/Users/dev/temporal"]),
            editor_node("n2", "/Users/dev/temporal"),
            generic_node("n3", "Spotify", "liked songs"),
        ]);
        let items = items_for(&ws);
        assert_eq!(items.len(), 5); // 2 tabs + 3 whole nodes
        assert_eq!(items[0].item_ref, ItemRef { node_id: "n0".into(), tab_index: Some(0) });
        assert_eq!(items[1].item_ref, ItemRef { node_id: "n0".into(), tab_index: Some(1) });
        // Fragment stripped from the tab dedup key.
        assert_eq!(items[1].dedup_key, "https://docs.rs/serde");
        // Terminal cwds sorted+deduped into one key.
        assert_eq!(items[2].dedup_key, "/Users/dev/temporal");
        assert_eq!(items[3].dedup_key, "/Users/dev/temporal"); // editor folder
        assert_eq!(items[4].dedup_key, "com.example.Spotify\u{1}liked songs");
        assert_eq!(items[0].embed_text, "repo (github.com)");
        assert!(items[2].embed_text.starts_with("Terminal temporal"));
    }

    #[test]
    fn cluster_separates_orthogonal_families() {
        let embeddings = vec![
            axis_vec(0, 0.1),
            axis_vec(0, 0.2),
            axis_vec(1, 0.1),
            axis_vec(1, 0.2),
            axis_vec(0, 0.3),
        ];
        let clusters = cluster(&embeddings, 0.6);
        assert_eq!(clusters.len(), 2);
        assert_eq!(clusters[0], vec![0, 1, 4]);
        assert_eq!(clusters[1], vec![2, 3]);
    }

    #[test]
    fn cluster_of_similar_vectors_is_single_and_build_groups_declines() {
        let embeddings: Vec<Vec<f32>> = (0..4).map(|i| axis_vec(0, 0.1 * i as f32)).collect();
        assert_eq!(cluster(&embeddings, 0.6).len(), 1);

        let items: Vec<WorkspaceItem> =
            items_for(&workspace(vec![browser_node(
                "n0",
                &[("https://a.com", "a"), ("https://b.com", "b"), ("https://c.com", "c"), ("https://d.com", "d")],
                0,
            )]));
        assert!(build_groups(&items, &embeddings).is_empty());
    }

    #[test]
    fn cluster_caps_at_max_groups() {
        // 8 mutually-orthogonal singletons (16-dim), threshold high enough
        // that nothing merges naturally.
        let embeddings: Vec<Vec<f32>> = (0..8)
            .map(|i| {
                let mut v = vec![0.0f32; 16];
                v[i] = 1.0;
                v
            })
            .collect();
        let clusters = cluster(&embeddings, 0.6);
        assert_eq!(clusters.len(), MAX_GROUPS);
        let total: usize = clusters.iter().map(Vec::len).sum();
        assert_eq!(total, 8); // overflow merged, nothing dropped
    }

    #[test]
    fn build_groups_assigns_ids_by_size_and_heuristic_labels() {
        let ws = workspace(vec![
            browser_node("n0", &[("https://github.com/x", "x"), ("https://spotify.com", "s")], 0),
            editor_node("n1", "/Users/dev/temporal"),
        ]);
        let items = items_for(&ws);
        // github tab + editor together; spotify tab alone.
        let embeddings =
            vec![axis_vec(0, 0.1), axis_vec(1, 0.1), axis_vec(0, 0.2)];
        let groups = build_groups(&items, &embeddings);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].group_id, "g0");
        assert_eq!(groups[0].items.len(), 2);
        assert!(groups[0].label.contains("temporal"), "label: {}", groups[0].label);
        assert_eq!(groups[1].items, vec![ItemRef { node_id: "n0".into(), tab_index: Some(1) }]);
        assert!(groups[1].label.contains("spotify.com"), "label: {}", groups[1].label);
    }

    #[test]
    fn synthesize_merges_tab_picks_and_remaps_active_tab() {
        let source = browser_node(
            "n7",
            &[("https://a.com", "a"), ("https://b.com", "b"), ("https://c.com", "c")],
            2,
        );
        let picks = vec![
            NodePick { node: source.clone(), tab_indices: Some(vec![2]) },
            NodePick { node: source.clone(), tab_indices: Some(vec![0]) },
            NodePick { node: terminal_node("n8", &["/tmp"]), tab_indices: None },
        ];
        let nodes = synthesize_nodes(picks);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].node_id, "n0");
        assert_eq!(nodes[1].node_id, "n1");
        match &nodes[0].payload {
            NodePayload::Browser { tabs, active_tab_index } => {
                let urls: Vec<&str> = tabs.iter().map(|t| t.url.as_str()).collect();
                assert_eq!(urls, vec!["https://a.com", "https://c.com"]);
                assert_eq!(*active_tab_index, 1); // originally-active c.com kept active
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn synthesize_defaults_active_tab_when_original_not_picked() {
        let source = browser_node("n0", &[("https://a.com", "a"), ("https://b.com", "b")], 1);
        let nodes = synthesize_nodes(vec![NodePick { node: source, tab_indices: Some(vec![0]) }]);
        match &nodes[0].payload {
            NodePayload::Browser { tabs, active_tab_index } => {
                assert_eq!(tabs.len(), 1);
                assert_eq!(*active_tab_index, 0);
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn synthesize_drops_nodes_whose_picked_tabs_are_all_stale() {
        let source = browser_node("n0", &[("https://a.com", "a")], 0);
        let nodes = synthesize_nodes(vec![NodePick { node: source, tab_indices: Some(vec![9]) }]);
        assert!(nodes.is_empty());
    }

    #[test]
    fn materialize_group_builds_standalone_workspace() {
        let mut ws = workspace(vec![
            browser_node("n0", &[("https://github.com/x", "x"), ("https://spotify.com", "s")], 0),
            editor_node("n1", "/Users/dev/temporal"),
        ]);
        ws.groups = vec![
            WorkspaceGroup {
                group_id: "g0".into(),
                label: "coding".into(),
                items: vec![
                    ItemRef { node_id: "n0".into(), tab_index: Some(0) },
                    ItemRef { node_id: "n1".into(), tab_index: None },
                    ItemRef { node_id: "ghost".into(), tab_index: None }, // stale: skipped
                ],
            },
            WorkspaceGroup { group_id: "g1".into(), label: "music".into(), items: vec![] },
        ];
        let coding = materialize_group(&ws, "g0").expect("materializes");
        assert_eq!(coding.workspace_id, "ws::g0");
        assert_eq!(coding.summary, "coding");
        assert_eq!(coding.captured_at_unix_ms, 42);
        assert!(coding.groups.is_empty());
        assert_eq!(coding.nodes.len(), 2);
        match &coding.nodes[0].payload {
            NodePayload::Browser { tabs, .. } => {
                assert_eq!(tabs.len(), 1);
                assert_eq!(tabs[0].url, "https://github.com/x");
            }
            other => panic!("unexpected payload: {other:?}"),
        }
        assert!(coding.tags.contains(&"github.com".to_string()));
        assert!(!coding.tags.contains(&"spotify.com".to_string()));

        assert!(materialize_group(&ws, "g1").is_none(), "empty group yields None");
        assert!(materialize_group(&ws, "gX").is_none(), "unknown id yields None");
    }
}
