//! Heuristic auto-tagging: deterministic tags and a compact summary derived
//! purely from extracted state. This is the floor the semantic index always
//! has; LLM-generated tags enrich on top of it asynchronously.
//!
//! Ported 1:1 from the original F# `Tagging.fs` so summaries/tags (and the
//! embedding text derived from them) stay consistent with pre-migration data.

use crate::types::{NodePayload, WorkspaceState};

fn url_host(url: &str) -> String {
    let Some(idx) = url.find("://") else { return String::new() };
    let rest = &url[idx + 3..];
    let host = rest.split('/').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("");
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

fn base_name(path: &str) -> String {
    let trimmed = if path.len() > 1 { path.trim_end_matches('/') } else { path };
    trimmed.rsplit('/').next().unwrap_or(trimmed).to_string()
}

/// Order-preserving dedupe that drops empty strings.
fn distinct(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = Vec::new();
    for item in items {
        if !item.is_empty() && !seen.contains(&item) {
            seen.push(item);
        }
    }
    seen
}

/// Tag sources, in priority order: project folders (editors, terminals),
/// URL hosts, app names.
pub fn derive_tags(w: &WorkspaceState) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    for node in &w.nodes {
        match &node.payload {
            NodePayload::Editor { folder_path, .. } => raw.push(base_name(folder_path)),
            NodePayload::Terminal { tabs } => {
                raw.extend(tabs.iter().map(|t| base_name(&t.cwd)));
            }
            _ => {}
        }
    }
    for node in &w.nodes {
        if let NodePayload::Browser { tabs, .. } = &node.payload {
            raw.extend(tabs.iter().map(|t| url_host(&t.url)));
        }
    }
    let mut all: Vec<String> = raw.into_iter().map(|t| t.to_lowercase()).collect();
    all.extend(w.nodes.iter().map(|n| n.app_name.to_lowercase()));
    let mut tags = distinct(all);
    tags.truncate(24);
    tags
}

/// One line the staging UI can show before LLM tags arrive, e.g.
/// "temporal · remy-ios · 23 browser tabs · Finder · Notes"
pub fn derive_summary(w: &WorkspaceState) -> String {
    let projects = distinct(w.nodes.iter().filter_map(|n| match &n.payload {
        NodePayload::Editor { folder_path, .. } => Some(base_name(folder_path)),
        NodePayload::Terminal { tabs } => tabs.first().map(|t| base_name(&t.cwd)),
        _ => None,
    }));
    let tab_count: usize = w
        .nodes
        .iter()
        .map(|n| match &n.payload {
            NodePayload::Browser { tabs, .. } => tabs.len(),
            _ => 0,
        })
        .sum();
    let browser_part = match tab_count {
        0 => None,
        1 => Some("1 browser tab".to_string()),
        n => Some(format!("{n} browser tabs")),
    };
    let mut generic_apps = distinct(w.nodes.iter().filter_map(|n| match &n.payload {
        NodePayload::Generic => Some(n.app_name.clone()),
        _ => None,
    }));
    generic_apps.truncate(4);

    let parts: Vec<String> =
        projects.into_iter().chain(browser_part).chain(generic_apps).collect();
    if parts.is_empty() {
        "empty workspace".to_string()
    } else {
        parts.join(" · ")
    }
}

/// Fills tags and summary from the captured nodes.
pub fn enrich(mut w: WorkspaceState) -> WorkspaceState {
    w.tags = derive_tags(&w);
    w.summary = derive_summary(&w);
    w
}

/// Applies LLM-generated summary/tags on top of the heuristics: the summary
/// replaces (LLM output is richer), tags merge (heuristics keep recall).
pub fn apply_llm_tags(summary: &str, llm_tags: &[String], mut w: WorkspaceState) -> WorkspaceState {
    if !summary.is_empty() {
        w.summary = summary.to_string();
    }
    let mut merged = distinct(
        w.tags.iter().cloned().chain(llm_tags.iter().map(|t| t.to_lowercase())),
    );
    merged.truncate(32);
    w.tags = merged;
    w
}

/// The text whose embedding represents this workspace in the vector index.
/// Order matters: summary and tags first (highest signal), then window
/// titles, project folders and tab titles/hosts.
pub fn embedding_text(w: &WorkspaceState) -> String {
    let mut parts: Vec<String> = vec![w.summary.clone()];
    parts.extend(w.tags.iter().cloned());
    for node in &w.nodes {
        parts.push(node.app_name.clone());
        parts.push(node.window_title.clone());
        match &node.payload {
            NodePayload::Browser { tabs, .. } => {
                for tab in tabs {
                    parts.push(tab.title.clone());
                    parts.push(url_host(&tab.url));
                }
            }
            NodePayload::Terminal { tabs } => {
                parts.extend(tabs.iter().map(|t| t.cwd.clone()));
            }
            NodePayload::Editor { folder_path, open_files } => {
                parts.push(folder_path.clone());
                parts.extend(open_files.iter().cloned());
            }
            NodePayload::Generic => {}
        }
    }
    distinct(parts).join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn browser_node(urls: &[(&str, &str)]) -> WindowNode {
        WindowNode {
            node_id: "n0".into(),
            bundle_id: "com.google.Chrome".into(),
            app_name: "Google Chrome".into(),
            window_title: "tabs".into(),
            geometry: WindowGeometry::default(),
            adapter: AdapterKind::Chrome,
            payload: NodePayload::Browser {
                tabs: urls
                    .iter()
                    .map(|(u, t)| BrowserTab { url: u.to_string(), title: t.to_string() })
                    .collect(),
                active_tab_index: 0,
            },
        }
    }

    fn workspace(nodes: Vec<WindowNode>) -> WorkspaceState {
        WorkspaceState {
            workspace_id: "ws".into(),
            captured_at_unix_ms: 0,
            summary: String::new(),
            tags: vec![],
            nodes,
        }
    }

    fn rich() -> WorkspaceState {
        workspace(vec![
            browser_node(&[
                ("https://fable.io/docs/", "Fable docs"),
                ("https://github.com/fable-compiler/Fable", "GitHub"),
            ]),
            WindowNode {
                node_id: "n1".into(),
                bundle_id: "com.apple.Terminal".into(),
                app_name: "Terminal".into(),
                window_title: "temporal".into(),
                geometry: WindowGeometry::default(),
                adapter: AdapterKind::TerminalApp,
                payload: NodePayload::Terminal {
                    tabs: vec![TerminalTab { tty: "/dev/ttys003".into(), cwd: "/Users/dev/temporal".into() }],
                },
            },
            WindowNode {
                node_id: "n2".into(),
                bundle_id: "com.microsoft.VSCode".into(),
                app_name: "Visual Studio Code".into(),
                window_title: "temporal".into(),
                geometry: WindowGeometry::default(),
                adapter: AdapterKind::VsCode,
                payload: NodePayload::Editor {
                    folder_path: "/Users/dev/temporal".into(),
                    open_files: vec![],
                },
            },
            WindowNode {
                node_id: "n3".into(),
                bundle_id: "com.apple.finder".into(),
                app_name: "Finder".into(),
                window_title: String::new(),
                geometry: WindowGeometry::default(),
                adapter: AdapterKind::Generic,
                payload: NodePayload::Generic,
            },
        ])
    }

    #[test]
    fn rich_workspace_derives_project_host_and_app_tags() {
        let tags = derive_tags(&rich());
        assert!(tags.contains(&"temporal".to_string()));
        assert!(tags.contains(&"fable.io".to_string()));
        assert!(tags.contains(&"github.com".to_string()));
        assert!(tags.contains(&"finder".to_string()));
        let mut deduped = tags.clone();
        deduped.dedup();
        assert_eq!(tags.len(), deduped.len());
    }

    #[test]
    fn summary_mentions_projects_and_tab_count() {
        let summary = derive_summary(&rich());
        assert!(summary.contains("temporal"));
        assert!(summary.contains("2 browser tabs"));
        assert!(summary.contains("Finder"));
    }

    #[test]
    fn empty_workspace_summary_and_tags() {
        let empty = workspace(vec![]);
        assert_eq!(derive_summary(&empty), "empty workspace");
        assert!(derive_tags(&empty).is_empty());
    }

    #[test]
    fn enrich_fills_tags_and_summary_without_touching_nodes() {
        let before = rich();
        let after = enrich(before.clone());
        assert!(!after.tags.is_empty());
        assert!(!after.summary.is_empty());
        assert_eq!(before.nodes, after.nodes);
    }

    #[test]
    fn apply_llm_tags_replaces_summary_and_merges_tags() {
        let base = enrich(rich());
        let heuristic_count = base.tags.len();
        let out = apply_llm_tags("Working on temporal", &["Fable".into(), "temporal".into()], base);
        assert_eq!(out.summary, "Working on temporal");
        assert!(out.tags.contains(&"fable".to_string()));
        // "temporal" already present: merged without duplication.
        assert_eq!(out.tags.len(), heuristic_count + 1);
    }

    #[test]
    fn empty_llm_summary_keeps_heuristic_one() {
        let base = enrich(rich());
        let summary = base.summary.clone();
        let out = apply_llm_tags("", &[], base);
        assert_eq!(out.summary, summary);
    }

    #[test]
    fn hosts_strip_www_and_ports() {
        let ws = workspace(vec![browser_node(&[
            ("https://www.example.com/x", ""),
            ("http://localhost:3000/", ""),
        ])]);
        let tags = derive_tags(&ws);
        assert!(tags.contains(&"example.com".to_string()));
        assert!(tags.contains(&"localhost".to_string()));
    }

    #[test]
    fn embedding_text_leads_with_summary_and_tags() {
        let ws = enrich(rich());
        let text = embedding_text(&ws);
        let first_line = text.lines().next().unwrap();
        assert_eq!(first_line, ws.summary);
        assert!(text.contains("fable.io"));
        assert!(text.contains("/Users/dev/temporal"));
    }
}
