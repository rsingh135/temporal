//! Converts adapter DTOs into the Fable-generated domain types. This is the
//! only place the two type worlds meet.

use fable_library_rust::List_::{self, List};
use fable_library_rust::Native_::LrcPtr;
use fable_library_rust::String_::fromString;
use temporal_adapters::{AdapterKind, ExtractedNode, Payload};
use temporal_core::Temporal::Domain::Types as domain;

pub fn to_list<T: Clone + 'static>(items: Vec<T>) -> List<T> {
    let mut list = List_::empty();
    for item in items.into_iter().rev() {
        list = List_::cons(item, list);
    }
    list
}

fn to_adapter_kind(kind: AdapterKind) -> domain::AdapterKind {
    match kind {
        AdapterKind::Chrome => domain::AdapterKind::Chrome,
        AdapterKind::TerminalApp => domain::AdapterKind::TerminalApp,
        AdapterKind::VSCode => domain::AdapterKind::VSCode,
        AdapterKind::Cursor => domain::AdapterKind::Cursor,
        AdapterKind::Generic => domain::AdapterKind::Generic,
    }
}

fn to_payload(payload: Payload) -> domain::NodePayload {
    match payload {
        Payload::Browser { tabs, active_tab_index } => domain::NodePayload::BrowserWindow(
            to_list(
                tabs.into_iter()
                    .map(|t| {
                        LrcPtr::new(domain::BrowserTab {
                            Url: fromString(t.url),
                            Title: fromString(t.title),
                        })
                    })
                    .collect(),
            ),
            active_tab_index,
        ),
        Payload::Terminal { tabs } => domain::NodePayload::TerminalWindow(to_list(
            tabs.into_iter()
                .map(|t| {
                    LrcPtr::new(domain::TerminalTab {
                        Tty: fromString(t.tty),
                        WorkingDirectory: fromString(t.working_directory),
                    })
                })
                .collect(),
        )),
        Payload::Editor { folder_path, open_files } => domain::NodePayload::EditorWindow(
            fromString(folder_path),
            to_list(open_files.into_iter().map(fromString).collect()),
        ),
        Payload::Generic => domain::NodePayload::GenericWindow,
    }
}

fn to_window_node(node: ExtractedNode) -> LrcPtr<domain::WindowNode> {
    LrcPtr::new(domain::WindowNode {
        NodeId: fromString(node.node_id),
        BundleId: fromString(node.bundle_id),
        AppName: fromString(node.app_name),
        WindowTitle: fromString(node.window_title),
        Geometry: LrcPtr::new(domain::WindowGeometry {
            X: node.geometry.x,
            Y: node.geometry.y,
            Width: node.geometry.width,
            Height: node.geometry.height,
        }),
        Adapter: LrcPtr::new(to_adapter_kind(node.kind)),
        Payload: LrcPtr::new(to_payload(node.payload)),
    })
}

fn from_list<T: Clone + 'static>(list: List<T>) -> Vec<T> {
    List_::toArray(list).get().clone()
}

fn from_payload(payload: &domain::NodePayload) -> Payload {
    match payload {
        domain::NodePayload::BrowserWindow(tabs, active) => Payload::Browser {
            tabs: from_list(tabs.clone())
                .into_iter()
                .map(|t| temporal_adapters::BrowserTab {
                    url: t.Url.to_string(),
                    title: t.Title.to_string(),
                })
                .collect(),
            active_tab_index: *active,
        },
        domain::NodePayload::TerminalWindow(tabs) => Payload::Terminal {
            tabs: from_list(tabs.clone())
                .into_iter()
                .map(|t| temporal_adapters::TerminalTab {
                    tty: t.Tty.to_string(),
                    working_directory: t.WorkingDirectory.to_string(),
                })
                .collect(),
        },
        domain::NodePayload::EditorWindow(folder_path, open_files) => Payload::Editor {
            folder_path: folder_path.to_string(),
            open_files: from_list(open_files.clone()).into_iter().map(|s| s.to_string()).collect(),
        },
        domain::NodePayload::GenericWindow => Payload::Generic,
    }
}

fn from_adapter_kind(kind: &domain::AdapterKind) -> AdapterKind {
    match kind {
        domain::AdapterKind::Chrome => AdapterKind::Chrome,
        domain::AdapterKind::TerminalApp => AdapterKind::TerminalApp,
        domain::AdapterKind::VSCode => AdapterKind::VSCode,
        domain::AdapterKind::Cursor => AdapterKind::Cursor,
        domain::AdapterKind::Generic => AdapterKind::Generic,
    }
}

/// Domain nodes (already filtered by Planning::includedNodes) back to
/// adapter DTOs for rehydration.
pub fn from_nodes(nodes: List<LrcPtr<domain::WindowNode>>) -> Vec<ExtractedNode> {
    from_list(nodes)
        .into_iter()
        .map(|n| ExtractedNode {
            node_id: n.NodeId.to_string(),
            bundle_id: n.BundleId.to_string(),
            app_name: n.AppName.to_string(),
            window_title: n.WindowTitle.to_string(),
            geometry: temporal_adapters::Geometry {
                x: n.Geometry.X,
                y: n.Geometry.Y,
                width: n.Geometry.Width,
                height: n.Geometry.Height,
            },
            kind: from_adapter_kind(&n.Adapter),
            payload: from_payload(&n.Payload),
        })
        .collect()
}

/// Untagged workspace (Summary/Tags empty); run it through the generated
/// `Tagging::enrich` afterwards.
pub fn to_workspace(
    workspace_id: String,
    captured_at_unix_ms: i64,
    nodes: Vec<ExtractedNode>,
) -> LrcPtr<domain::WorkspaceState> {
    LrcPtr::new(domain::WorkspaceState {
        WorkspaceId: fromString(workspace_id),
        CapturedAtUnixMs: captured_at_unix_ms,
        Summary: fromString(String::new()),
        Tags: List_::empty(),
        Nodes: to_list(nodes.into_iter().map(to_window_node).collect()),
    })
}
