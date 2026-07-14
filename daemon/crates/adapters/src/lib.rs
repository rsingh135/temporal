//! State-extraction adapters: turn the live desktop into domain `WindowNode`s.
//!
//! Specialized adapters (Chrome, Terminal.app, VS Code/Cursor) capture deep
//! state; every other visible app degrades to a Generic node (bundle id +
//! window geometry). Extraction is synchronous and blocking — the daemon runs
//! it inside `spawn_blocking`.

mod chrome;
mod editor;
mod generic;
mod osascript;
mod proc_timeout;
pub mod rehydrate;
mod terminal;

use temporal_domain::WindowNode;
use tracing::warn;

/// Extraction always succeeds overall; per-adapter failures become warnings
/// and the affected app simply contributes no (or only generic) nodes.
#[derive(Debug, Default)]
pub struct ExtractionReport {
    pub nodes: Vec<WindowNode>,
    pub warnings: Vec<String>,
}

pub const CHROME_BUNDLE_ID: &str = "com.google.Chrome";
pub const TERMINAL_BUNDLE_ID: &str = "com.apple.Terminal";
pub const VSCODE_BUNDLE_ID: &str = "com.microsoft.VSCode";
pub const CURSOR_BUNDLE_ID: &str = "com.todesktop.230313mzl4w4u92";

pub fn extract_workspace() -> ExtractionReport {
    let mut report = ExtractionReport::default();

    let windows = match temporal_macos_ffi::window_list::onscreen_windows() {
        Ok(windows) => windows,
        Err(e) => {
            warn!(error = %e, "window list unavailable; extraction limited to scriptable apps");
            report.warnings.push(format!("window list unavailable: {e}"));
            Vec::new()
        }
    };

    // Resolve each pid once; identity drives the generic adapter.
    let mut identities: std::collections::HashMap<i32, temporal_macos_ffi::AppIdentity> =
        std::collections::HashMap::new();
    for w in &windows {
        identities
            .entry(w.owner_pid)
            .or_insert_with(|| temporal_macos_ffi::bundle::identify_pid(w.owner_pid));
    }

    // Dispatch on running processes, not visible windows: apps whose windows
    // are fullscreen or on another Space never appear in CGWindowList.
    let running_ids = temporal_macos_ffi::bundle::running_bundle_ids();
    let running = |bundle_id: &str| running_ids.contains(bundle_id);

    // Bundle ids whose windows the specialized adapters own; generic skips them.
    let mut consumed: Vec<&str> = Vec::new();

    if running(CHROME_BUNDLE_ID) {
        consumed.push(CHROME_BUNDLE_ID);
        chrome::extract(&mut report);
    }
    if running(TERMINAL_BUNDLE_ID) {
        consumed.push(TERMINAL_BUNDLE_ID);
        terminal::extract(&mut report);
    }
    if running(VSCODE_BUNDLE_ID) {
        consumed.push(VSCODE_BUNDLE_ID);
        editor::extract(editor::Variant::VsCode, &mut report);
    }
    if running(CURSOR_BUNDLE_ID) {
        consumed.push(CURSOR_BUNDLE_ID);
        editor::extract(editor::Variant::Cursor, &mut report);
    }

    generic::extract(&windows, &identities, &consumed, &mut report);

    // Deterministic ids, assigned once the full node set is known.
    for (i, node) in report.nodes.iter_mut().enumerate() {
        node.node_id = format!("n{i}");
    }
    report
}
