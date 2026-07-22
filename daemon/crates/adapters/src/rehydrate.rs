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

/// URL schemes Chrome is allowed to reopen. The rehydration payload arrives
/// over the socket and is not validated against storage, so a client could
/// otherwise ask Chrome to open `file://`, `javascript:`, or `data:` URLs.
const ALLOWED_URL_SCHEMES: &[&str] = &["http", "https", "chrome", "chrome-extension", "about"];

/// Extracts the lowercased URI scheme per RFC 3986
/// (`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )` up to the first `:`), or
/// `None` if the string doesn't start with a valid scheme. Deliberately does
/// not trim leading whitespace: a leading space means no scheme, so a
/// `" javascript:"` smuggling attempt is rejected rather than normalized.
fn uri_scheme(url: &str) -> Option<String> {
    let (scheme, _rest) = url.split_once(':')?;
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    Some(scheme.to_ascii_lowercase())
}

/// True if `url` has an allowlisted scheme safe for Chrome to reopen.
fn is_allowed_url(url: &str) -> bool {
    uri_scheme(url).is_some_and(|s| ALLOWED_URL_SCHEMES.contains(&s.as_str()))
}

/// True if `s` has the shape of a macOS bundle identifier (reverse-DNS:
/// alphanumerics, dots, and hyphens, containing at least one dot, not starting
/// with `-`). Rejects flag-injection-shaped strings (`-x`) and quote characters
/// that would break the `mdfind`/`open` argument, without hardcoding an app
/// allowlist — the generic adapter can still launch any legitimately-shaped id.
fn looks_like_bundle_id(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.contains('.')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
}

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
        NodePayload::Editor { folder_path, open_files } => {
            rehydrate_editor(node, folder_path, open_files)
        }
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
    // Drop tabs whose URL scheme isn't allowlisted; keep restoring the rest.
    let total = tabs.len();
    let urls: Vec<String> = tabs
        .iter()
        .filter(|t| {
            let ok = is_allowed_url(&t.url);
            if !ok {
                warn!(url = %t.url, node = %node.node_id, "skipping tab with disallowed URL scheme");
            }
            ok
        })
        .map(|t| t.url.clone())
        .collect();
    if urls.is_empty() {
        return Err(format!("all {total} tab(s) had a disallowed URL scheme"));
    }
    let (x, y, width, height) = clamped_frame(node);
    // Data goes in as JSON so URLs never touch script-string escaping.
    let data = json!({
        "urls": urls,
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

fn rehydrate_editor(
    node: &WindowNode,
    folder_path: &str,
    open_files: &[String],
) -> Result<(), String> {
    // `--` stops `open` from parsing a folder_path that begins with `-` as a
    // flag (no shell is involved, so this is argument-injection hardening only).
    let status = Command::new("/usr/bin/open")
        .args(["-a", &node.app_name, "--", folder_path])
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("open -a {} exited with {status}", node.app_name));
    }
    // Reopen the previously-open files in the just-restored window. `-g` keeps
    // the editor from stealing focus per file; VS Code / Cursor add each file
    // to the folder's window. Best-effort: a failure here doesn't fail the node
    // (the folder is already open), it's just logged.
    if !open_files.is_empty() {
        let mut cmd = Command::new("/usr/bin/open");
        cmd.args(["-a", &node.app_name, "-g", "--"]);
        cmd.args(open_files);
        match proc_timeout::run_with_timeout(cmd, OPEN_TIMEOUT) {
            Ok(output) if !output.status.success() => {
                warn!(
                    app = %node.app_name,
                    stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                    "reopening editor files failed"
                );
            }
            Err(e) => warn!(app = %node.app_name, error = %e, "reopening editor files failed"),
            Ok(_) => {}
        }
    }
    Ok(())
}

fn rehydrate_generic(node: &WindowNode) -> Result<(), String> {
    if !looks_like_bundle_id(&node.bundle_id) {
        return Err(format!("{:?} is not a valid bundle id", node.bundle_id));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_scheme_extracts_and_lowercases() {
        assert_eq!(uri_scheme("HTTPS://example.com").as_deref(), Some("https"));
        assert_eq!(uri_scheme("chrome-extension://abc/x").as_deref(), Some("chrome-extension"));
    }

    #[test]
    fn uri_scheme_rejects_no_or_malformed_scheme() {
        assert_eq!(uri_scheme("example.com/path"), None); // no colon
        assert_eq!(uri_scheme(" javascript:alert(1)"), None); // leading space, no valid scheme
        assert_eq!(uri_scheme("1http://x"), None); // scheme must start with a letter
        assert_eq!(uri_scheme("//host/path"), None);
    }

    #[test]
    fn allowed_url_permits_web_and_chrome_schemes() {
        assert!(is_allowed_url("https://example.com"));
        assert!(is_allowed_url("http://example.com"));
        assert!(is_allowed_url("chrome://newtab"));
        assert!(is_allowed_url("about:blank"));
    }

    #[test]
    fn allowed_url_rejects_dangerous_schemes() {
        assert!(!is_allowed_url("file:///etc/passwd"));
        assert!(!is_allowed_url("javascript:alert(1)"));
        assert!(!is_allowed_url("data:text/html,<script>x</script>"));
        assert!(!is_allowed_url(" javascript:alert(1)"));
        assert!(!is_allowed_url("example.com")); // no scheme at all
    }

    #[test]
    fn looks_like_bundle_id_accepts_real_ids() {
        assert!(looks_like_bundle_id("com.google.Chrome"));
        assert!(looks_like_bundle_id("com.todesktop.230313mzl4w4u92"));
        assert!(looks_like_bundle_id("com.apple.Terminal"));
    }

    #[test]
    fn looks_like_bundle_id_rejects_injection_shapes() {
        assert!(!looks_like_bundle_id("")); // empty
        assert!(!looks_like_bundle_id("-a")); // flag shape
        assert!(!looks_like_bundle_id("noDotHere")); // not reverse-DNS
        assert!(!looks_like_bundle_id("com.evil'; rm -rf")); // quote + space
        assert!(!looks_like_bundle_id("com.foo bar")); // space
    }
}
