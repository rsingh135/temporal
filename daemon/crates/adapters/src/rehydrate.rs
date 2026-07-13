//! Rehydration: recreate captured nodes on the live desktop.
//!
//! Fidelity by adapter:
//!   - Chrome: window + all tabs + active tab + bounds (JXA).
//!   - Terminal.app: one window per captured tab (`do script "cd …"`) with
//!     bounds; AppleScript cannot create tabs in an existing window without
//!     Accessibility-driven keystrokes.
//!   - VS Code / Cursor: `open -a <app> <folder>`; the editor restores its
//!     own per-folder geometry.
//!   - Generic: `open -b <bundle-id>` + best-effort AX window placement.

use std::process::Command;

use serde_json::json;
use temporal_domain::{BrowserTab, NodePayload, TerminalTab, WindowNode};
use tracing::{info, warn};

use crate::osascript::run_jxa_json;

/// Per-node outcome; failures don't abort the rest of the payload.
pub struct RehydrationOutcome {
    pub restored: usize,
    pub failures: Vec<String>,
}

/// Rehydrates nodes in order, reporting progress (index, label) per node.
pub fn rehydrate_nodes(
    nodes: &[WindowNode],
    mut progress: impl FnMut(usize, &str),
) -> RehydrationOutcome {
    let mut outcome = RehydrationOutcome { restored: 0, failures: Vec::new() };
    for (i, node) in nodes.iter().enumerate() {
        progress(i, &node.app_name);
        match rehydrate_node(node) {
            Ok(()) => {
                info!(app = %node.app_name, node = %node.node_id, "node rehydrated");
                outcome.restored += 1;
            }
            Err(e) => {
                warn!(app = %node.app_name, node = %node.node_id, error = %e, "node failed");
                outcome.failures.push(format!("{}: {e}", node.app_name));
            }
        }
    }
    outcome
}

fn rehydrate_node(node: &WindowNode) -> Result<(), String> {
    match &node.payload {
        NodePayload::Browser { tabs, active_tab_index } => {
            rehydrate_chrome(node, tabs, *active_tab_index)
        }
        NodePayload::Terminal { tabs } => rehydrate_terminal(node, tabs),
        NodePayload::Editor { folder_path, .. } => rehydrate_editor(node, folder_path),
        NodePayload::Generic => rehydrate_generic(node),
    }
}

fn rehydrate_chrome(
    node: &WindowNode,
    tabs: &[BrowserTab],
    active_tab_index: i32,
) -> Result<(), String> {
    if tabs.is_empty() {
        return Ok(());
    }
    // Data goes in as JSON so URLs never touch script-string escaping.
    let data = json!({
        "urls": tabs.iter().map(|t| t.url.clone()).collect::<Vec<_>>(),
        "active": active_tab_index + 1, // JXA tab indices are 1-based
        "bounds": {
            "x": node.geometry.x,
            "y": node.geometry.y,
            "width": node.geometry.width,
            "height": node.geometry.height,
        },
    });
    let script = format!(
        r#"
const data = {data};
const chrome = Application("Google Chrome");
chrome.includeStandardAdditions = true;
const win = chrome.Window().make();
win.tabs[0].url = data.urls[0];
for (let i = 1; i < data.urls.length; i++) {{
    win.tabs.push(chrome.Tab({{url: data.urls[i]}}));
}}
win.activeTabIndex = Math.min(data.active, data.urls.length);
if (data.bounds.width > 0) {{ win.bounds = data.bounds; }}
chrome.activate();
JSON.stringify("ok");
"#
    );
    let _: String = run_jxa_json(&script).map_err(|e| e.to_string())?;
    Ok(())
}

fn rehydrate_terminal(node: &WindowNode, tabs: &[TerminalTab]) -> Result<(), String> {
    for tab in tabs {
        if tab.cwd.is_empty() {
            continue;
        }
        let data = json!({
            "cwd": tab.cwd,
            "bounds": {
                "x": node.geometry.x,
                "y": node.geometry.y,
                "width": node.geometry.width,
                "height": node.geometry.height,
            },
        });
        let script = format!(
            r#"
const data = {data};
const term = Application("Terminal");
const quoted = "'" + data.cwd.replace(/'/g, `'\\''`) + "'";
const tab = term.doScript("cd " + quoted);
if (data.bounds.width > 0) {{
    term.windows[0].bounds = data.bounds;
}}
term.activate();
JSON.stringify("ok");
"#
        );
        let _: String = run_jxa_json(&script).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn rehydrate_editor(node: &WindowNode, folder_path: &str) -> Result<(), String> {
    let status = Command::new("/usr/bin/open")
        .args(["-a", &node.app_name, folder_path])
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("open -a {} exited with {status}", node.app_name));
    }
    Ok(())
}

fn rehydrate_generic(node: &WindowNode) -> Result<(), String> {
    let status = Command::new("/usr/bin/open")
        .args(["-b", &node.bundle_id])
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("open -b {} exited with {status}", node.bundle_id));
    }
    // Geometry restore is best-effort and needs the Accessibility grant.
    if node.geometry.width > 0.0
        && temporal_macos_ffi::ax::is_trusted()
        && let Some(pid) = pid_for_bundle(&node.bundle_id)
        && temporal_macos_ffi::ax::wait_for_windows(pid, 1, std::time::Duration::from_secs(5))
    {
        let frame =
            (node.geometry.x, node.geometry.y, node.geometry.width, node.geometry.height);
        if let Err(e) = temporal_macos_ffi::ax::place_windows(pid, &[frame]) {
            warn!(bundle = %node.bundle_id, error = %e, "window placement failed");
        }
    }
    Ok(())
}

fn pid_for_bundle(bundle_id: &str) -> Option<i32> {
    let pids =
        libproc::processes::pids_by_type(libproc::processes::ProcFilter::All).unwrap_or_default();
    for pid in pids {
        let identity = temporal_macos_ffi::bundle::identify_pid(pid as i32);
        if identity.bundle_id == bundle_id {
            return Some(pid as i32);
        }
    }
    None
}
