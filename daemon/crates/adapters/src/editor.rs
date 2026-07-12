//! VS Code / Cursor adapter. Both share the same storage layout:
//! `~/Library/Application Support/<dir>/User/globalStorage/storage.json`
//! carries `windowsState` with folder URIs and window geometry (`uiState`).

use std::path::{Path, PathBuf};

use percent_encoding::percent_decode_str;
use serde::Deserialize;

use crate::{AdapterKind, ExtractedNode, ExtractionReport, Geometry, Payload};

#[derive(Debug, Clone, Copy)]
pub enum Variant {
    VSCode,
    Cursor,
}

impl Variant {
    fn bundle_id(self) -> &'static str {
        match self {
            Variant::VSCode => crate::VSCODE_BUNDLE_ID,
            Variant::Cursor => crate::CURSOR_BUNDLE_ID,
        }
    }
    fn app_name(self) -> &'static str {
        match self {
            Variant::VSCode => "Visual Studio Code",
            Variant::Cursor => "Cursor",
        }
    }
    fn support_dir(self) -> &'static str {
        match self {
            Variant::VSCode => "Code",
            Variant::Cursor => "Cursor",
        }
    }
    fn kind(self) -> AdapterKind {
        match self {
            Variant::VSCode => AdapterKind::VSCode,
            Variant::Cursor => AdapterKind::Cursor,
        }
    }
}

#[derive(Deserialize)]
struct StorageJson {
    #[serde(rename = "windowsState")]
    windows_state: Option<WindowsState>,
}

#[derive(Deserialize)]
struct WindowsState {
    #[serde(rename = "lastActiveWindow")]
    last_active_window: Option<WindowState>,
    #[serde(rename = "openedWindows", default)]
    opened_windows: Vec<WindowState>,
}

#[derive(Deserialize)]
struct WindowState {
    folder: Option<String>,
    #[serde(rename = "uiState")]
    ui_state: Option<UiState>,
}

#[derive(Deserialize)]
struct UiState {
    x: Option<f64>,
    y: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
}

pub fn extract(variant: Variant, report: &mut ExtractionReport) {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        report.warnings.push("editor: HOME not set".to_string());
        return;
    };
    let storage_path = home
        .join("Library/Application Support")
        .join(variant.support_dir())
        .join("User/globalStorage/storage.json");
    extract_from_file(variant, &storage_path, report);
}

fn extract_from_file(variant: Variant, storage_path: &Path, report: &mut ExtractionReport) {
    let raw = match std::fs::read_to_string(storage_path) {
        Ok(raw) => raw,
        Err(e) => {
            report.warnings.push(format!("{}: cannot read {}: {e}", variant.app_name(), storage_path.display()));
            return;
        }
    };
    let parsed: StorageJson = match serde_json::from_str(&raw) {
        Ok(parsed) => parsed,
        Err(e) => {
            report.warnings.push(format!("{}: bad storage.json: {e}", variant.app_name()));
            return;
        }
    };
    let Some(state) = parsed.windows_state else { return };

    // openedWindows lists all windows at last multi-window save; a lone window
    // often lives only in lastActiveWindow. Dedupe by folder.
    let mut windows: Vec<&WindowState> = state.opened_windows.iter().collect();
    if let Some(last) = &state.last_active_window {
        windows.push(last);
    }
    let mut seen = std::collections::HashSet::new();
    for w in windows {
        let Some(folder_uri) = &w.folder else { continue };
        let Some(folder_path) = file_uri_to_path(folder_uri) else {
            report.warnings.push(format!("{}: unsupported folder URI {folder_uri}", variant.app_name()));
            continue;
        };
        if !seen.insert(folder_path.clone()) {
            continue;
        }
        let geometry = w
            .ui_state
            .as_ref()
            .map(|u| Geometry {
                x: u.x.unwrap_or(0.0),
                y: u.y.unwrap_or(0.0),
                width: u.width.unwrap_or(0.0),
                height: u.height.unwrap_or(0.0),
            })
            .unwrap_or_default();
        let window_title = Path::new(&folder_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| folder_path.clone());
        report.nodes.push(ExtractedNode {
            node_id: String::new(),
            bundle_id: variant.bundle_id().to_string(),
            app_name: variant.app_name().to_string(),
            window_title,
            geometry,
            kind: variant.kind(),
            payload: Payload::Editor { folder_path, open_files: Vec::new() },
        });
    }
}

fn file_uri_to_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    Some(percent_decode_str(rest).decode_utf8().ok()?.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_storage_json_last_active_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("storage.json");
        std::fs::write(
            &path,
            r#"{"windowsState":{"lastActiveWindow":{"folder":"file:///Users/dev/my%20repo","uiState":{"mode":3,"x":254,"y":189,"width":1280,"height":800}},"openedWindows":[]}}"#,
        )
        .unwrap();
        let mut report = ExtractionReport::default();
        extract_from_file(Variant::Cursor, &path, &mut report);
        assert_eq!(report.nodes.len(), 1);
        let node = &report.nodes[0];
        assert_eq!(node.window_title, "my repo");
        assert_eq!(node.geometry.width, 1280.0);
        match &node.payload {
            Payload::Editor { folder_path, .. } => assert_eq!(folder_path, "/Users/dev/my repo"),
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn dedupes_last_active_against_opened_windows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("storage.json");
        std::fs::write(
            &path,
            r#"{"windowsState":{"lastActiveWindow":{"folder":"file:///a"},"openedWindows":[{"folder":"file:///a"},{"folder":"file:///b"}]}}"#,
        )
        .unwrap();
        let mut report = ExtractionReport::default();
        extract_from_file(Variant::VSCode, &path, &mut report);
        assert_eq!(report.nodes.len(), 2);
    }
}
