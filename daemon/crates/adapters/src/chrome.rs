//! Chrome adapter: windows, tabs, active tab and bounds via JXA.

use serde::Deserialize;
use temporal_domain::{AdapterKind, BrowserTab, NodePayload, WindowGeometry, WindowNode};

use crate::osascript::run_jxa_json_retrying;
use crate::ExtractionReport;

const SCRIPT: &str = r#"
const app = Application("Google Chrome");
JSON.stringify(app.windows().map(w => ({
    bounds: w.bounds(),
    activeTabIndex: w.activeTabIndex(),
    tabs: w.tabs().map(t => ({url: t.url() || "", title: t.title() || ""}))
})));
"#;

#[derive(Deserialize)]
struct JxaWindow {
    bounds: JxaBounds,
    #[serde(rename = "activeTabIndex")]
    active_tab_index: i32,
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
    url: String,
    title: String,
}

pub fn extract(report: &mut ExtractionReport) {
    let windows: Vec<JxaWindow> = match run_jxa_json_retrying(SCRIPT) {
        Ok(windows) => windows,
        Err(e) => {
            report.warnings.push(format!("chrome: {e}"));
            return;
        }
    };
    for w in windows {
        if w.tabs.is_empty() {
            continue;
        }
        // AppleScript tab indices are 1-based; the wire format is 0-based.
        let active = (w.active_tab_index - 1).clamp(0, w.tabs.len() as i32 - 1);
        let window_title = w.tabs[active as usize].title.clone();
        report.nodes.push(WindowNode {
            node_id: String::new(),
            bundle_id: crate::CHROME_BUNDLE_ID.to_string(),
            app_name: "Google Chrome".to_string(),
            window_title,
            geometry: WindowGeometry {
                x: w.bounds.x,
                y: w.bounds.y,
                width: w.bounds.width,
                height: w.bounds.height,
            },
            adapter: AdapterKind::Chrome,
            payload: NodePayload::Browser {
                tabs: w
                    .tabs
                    .into_iter()
                    .map(|t| BrowserTab { url: t.url, title: t.title })
                    .collect(),
                active_tab_index: active,
            },
        });
    }
}
