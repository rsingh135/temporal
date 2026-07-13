//! Terminal.app adapter: window/tab ttys via JXA, working directory of each
//! tty via lsof (cwd of the shell attached to it).

use std::path::Path;
use std::process::Command;

use serde::Deserialize;
use temporal_domain::{AdapterKind, NodePayload, TerminalTab, WindowGeometry, WindowNode};
use tracing::debug;

use crate::osascript::run_jxa_json;
use crate::ExtractionReport;

const SCRIPT: &str = r#"
const app = Application("Terminal");
JSON.stringify(app.windows().map(w => ({
    bounds: w.bounds(),
    tabs: w.tabs().map(t => ({tty: t.tty() || "", selected: t.selected()}))
})));
"#;

#[derive(Deserialize)]
struct JxaWindow {
    bounds: JxaBounds,
    tabs: Vec<JxaTab>,
}

#[derive(Deserialize)]
struct JxaBounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Deserialize)]
struct JxaTab {
    tty: String,
    selected: bool,
}

pub fn extract(report: &mut ExtractionReport) {
    let windows: Vec<JxaWindow> = match run_jxa_json(SCRIPT) {
        Ok(windows) => windows,
        Err(e) => {
            report.warnings.push(format!("terminal: {e}"));
            return;
        }
    };
    for w in windows {
        let mut tabs = Vec::new();
        let mut selected_cwd = String::new();
        for tab in &w.tabs {
            if tab.tty.is_empty() {
                continue;
            }
            let cwd = tty_working_directory(&tab.tty).unwrap_or_default();
            if tab.selected {
                selected_cwd = cwd.clone();
            }
            tabs.push(TerminalTab { tty: tab.tty.clone(), cwd });
        }
        if tabs.is_empty() {
            continue;
        }
        let window_title = Path::new(&selected_cwd)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Terminal".to_string());
        report.nodes.push(WindowNode {
            node_id: String::new(),
            bundle_id: crate::TERMINAL_BUNDLE_ID.to_string(),
            app_name: "Terminal".to_string(),
            window_title,
            geometry: WindowGeometry {
                x: w.bounds.x,
                y: w.bounds.y,
                width: w.bounds.width,
                height: w.bounds.height,
            },
            adapter: AdapterKind::TerminalApp,
            payload: NodePayload::Terminal { tabs },
        });
    }
}

/// cwd of the shell attached to `tty`: the process with the lowest pid that
/// has the tty open is the login shell.
fn tty_working_directory(tty: &str) -> Option<String> {
    let pids = Command::new("/usr/sbin/lsof").arg("-t").arg(tty).output().ok()?;
    let shell_pid = String::from_utf8_lossy(&pids.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .min()?;
    let cwd = Command::new("/usr/sbin/lsof")
        .args(["-a", "-p", &shell_pid.to_string(), "-d", "cwd", "-Fn"])
        .output()
        .ok()?;
    let dir = String::from_utf8_lossy(&cwd.stdout)
        .lines()
        .find(|l| l.starts_with('n'))
        .map(|l| l[1..].to_string())?;
    debug!(tty, shell_pid, cwd = %dir, "resolved terminal cwd");
    Some(dir)
}
