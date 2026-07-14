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
use std::time::Duration;

use serde_json::json;
use temporal_domain::{BrowserTab, NodePayload, TerminalTab, WindowGeometry, WindowNode};
use tracing::{info, warn};

use crate::osascript::run_jxa_json_retrying;
use crate::proc_timeout;

const MDFIND_TIMEOUT: Duration = Duration::from_secs(3);
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// Per-node outcome; failures don't abort the rest of the payload.
pub struct RehydrationOutcome {
    pub restored: usize,
    pub failures: Vec<String>,
}

/// One node's rehydration lifecycle, reported as it happens.
pub enum NodeEvent {
    Started { index: usize, app_name: String },
    Finished { index: usize, node_id: String, app_name: String, ok: bool, message: Option<String> },
}

/// Rehydrates nodes in order, reporting a `Started`/`Finished` event per node.
pub fn rehydrate_nodes(nodes: &[WindowNode], mut on_event: impl FnMut(NodeEvent)) -> RehydrationOutcome {
    let mut outcome = RehydrationOutcome { restored: 0, failures: Vec::new() };
    for (i, node) in nodes.iter().enumerate() {
        on_event(NodeEvent::Started { index: i, app_name: node.app_name.clone() });
        match rehydrate_node(node) {
            Ok(()) => {
                info!(app = %node.app_name, node = %node.node_id, "node rehydrated");
                outcome.restored += 1;
                on_event(NodeEvent::Finished {
                    index: i,
                    node_id: node.node_id.clone(),
                    app_name: node.app_name.clone(),
                    ok: true,
                    message: None,
                });
            }
            Err(e) => {
                warn!(app = %node.app_name, node = %node.node_id, error = %e, "node failed");
                outcome.failures.push(format!("{}: {e}", node.app_name));
                on_event(NodeEvent::Finished {
                    index: i,
                    node_id: node.node_id.clone(),
                    app_name: node.app_name.clone(),
                    ok: false,
                    message: Some(e),
                });
            }
        }
    }
    outcome
}

/// Clamps a captured node's geometry to fit within a currently active
/// display; a captured window on a monitor that's since been unplugged would
/// otherwise be restored off-screen.
fn clamped_frame(node: &WindowNode) -> (f64, f64, f64, f64) {
    let displays: Vec<WindowGeometry> = temporal_macos_ffi::display::active_display_frames()
        .into_iter()
        .map(|(x, y, width, height)| WindowGeometry { x, y, width, height })
        .collect();
    let clamped = temporal_domain::geometry::clamp_to_displays(node.geometry, &displays);
    (clamped.x, clamped.y, clamped.width, clamped.height)
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
    let (x, y, width, height) = clamped_frame(node);
    // Data goes in as JSON so URLs never touch script-string escaping.
    let data = json!({
        "urls": tabs.iter().map(|t| t.url.clone()).collect::<Vec<_>>(),
        "active": active_tab_index + 1, // JXA tab indices are 1-based
        "bounds": { "x": x, "y": y, "width": width, "height": height },
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
    let _: String = run_jxa_json_retrying(&script).map_err(|e| e.to_string())?;
    Ok(())
}

fn rehydrate_terminal(node: &WindowNode, tabs: &[TerminalTab]) -> Result<(), String> {
    let (x, y, width, height) = clamped_frame(node);
    for tab in tabs {
        if tab.cwd.is_empty() {
            continue;
        }
        let data = json!({
            "cwd": tab.cwd,
            "bounds": { "x": x, "y": y, "width": width, "height": height },
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
        let _: String = run_jxa_json_retrying(&script).map_err(|e| e.to_string())?;
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
    if !bundle_is_installed(&node.bundle_id) {
        return Err(format!("{} is not installed", node.bundle_id));
    }
    let mut cmd = Command::new("/usr/bin/open");
    cmd.args(["-b", &node.bundle_id]);
    let output = proc_timeout::run_with_timeout(cmd, OPEN_TIMEOUT).map_err(|e| e.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!("launch failed: {stderr}"));
    }
    // Geometry restore is best-effort and needs the Accessibility grant.
    if node.geometry.width > 0.0
        && temporal_macos_ffi::ax::is_trusted()
        && let Some(pid) = pid_for_bundle(&node.bundle_id)
        && temporal_macos_ffi::ax::wait_for_windows(pid, 1, std::time::Duration::from_secs(5))
    {
        let frame = clamped_frame(node);
        if let Err(e) = temporal_macos_ffi::ax::place_windows(pid, &[frame]) {
            warn!(bundle = %node.bundle_id, error = %e, "window placement failed");
        }
    }
    Ok(())
}

/// `mdfind`'s Spotlight index answers "is a bundle with this id installed?"
/// without launching it, so a missing app can be distinguished from one that
/// launched and immediately failed. If `mdfind` itself fails or times out,
/// don't block the launch attempt on that — fall through to `open`.
fn bundle_is_installed(bundle_id: &str) -> bool {
    let mut cmd = Command::new("/usr/bin/mdfind");
    cmd.arg(format!("kMDItemCFBundleIdentifier == '{bundle_id}'"));
    match proc_timeout::run_with_timeout(cmd, MDFIND_TIMEOUT) {
        Ok(output) => !String::from_utf8_lossy(&output.stdout).trim().is_empty(),
        Err(_) => true,
    }
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
