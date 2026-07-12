//! Tauri shell: global hotkey, frameless panel, and a passthrough to the
//! daemon's Unix socket. The shell never interprets the JSON it ferries —
//! both ends speak the shared F# codec.

use std::path::PathBuf;

use tauri::{Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

fn socket_path() -> PathBuf {
    dirs::home_dir()
        .expect("home directory")
        .join("Library/Application Support/temporald/temporald.sock")
}

async fn read_frame(stream: &mut UnixStream) -> Result<Option<Vec<u8>>, String> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.to_string()),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 16 * 1024 * 1024 {
        return Err("oversized frame from daemon".to_string());
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).await.map_err(|e| e.to_string())?;
    Ok(Some(payload))
}

/// True when the response type ends a request's stream (mirrors the daemon's
/// response contract; probe.rs uses the same set).
fn is_terminal(response_json: &str) -> bool {
    let t = serde_json::from_str::<serde_json::Value>(response_json)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string));
    matches!(t.as_deref(), Some("pong" | "done" | "error" | "query-results"))
}

/// Sends one request; emits each response frame as a `daemon-response` event
/// (for progress) and resolves with every frame once the terminal one arrives.
#[tauri::command]
async fn daemon_request(app: tauri::AppHandle, request_json: String) -> Result<Vec<String>, String> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path).await.map_err(|e| {
        format!("cannot reach temporald at {} ({e}); is the daemon running?", path.display())
    })?;
    let bytes = request_json.as_bytes();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await
        .map_err(|e| e.to_string())?;
    stream.write_all(bytes).await.map_err(|e| e.to_string())?;
    stream.flush().await.map_err(|e| e.to_string())?;

    let mut responses = Vec::new();
    loop {
        let Some(frame) = read_frame(&mut stream).await? else {
            return Err("daemon closed the connection early".to_string());
        };
        let response = String::from_utf8(frame).map_err(|e| e.to_string())?;
        let _ = app.emit("daemon-response", &response);
        let terminal = is_terminal(&response);
        responses.push(response);
        if terminal {
            return Ok(responses);
        }
    }
}

#[tauri::command]
fn hide_panel(window: tauri::WebviewWindow) {
    let _ = window.hide();
}

/// Frontend diagnostics land on the shell's stderr (webview console output is
/// otherwise invisible when launched headless).
#[tauri::command]
fn ui_log(message: String) {
    eprintln!("[ui] {message}");
}

fn toggle_panel(app: &tauri::AppHandle) {
    match app.get_webview_window("main") {
        Some(window) => {
            let visible = window.is_visible();
            eprintln!("[shell] toggle_panel: visible={visible:?}");
            if visible.unwrap_or(false) {
                let _ = window.hide();
            } else {
                let _ = window.center();
                if let Err(e) = window.show() {
                    eprintln!("[shell] show failed: {e}");
                }
                let _ = window.set_focus();
                let _ = app.emit("panel-shown", ());
            }
        }
        None => eprintln!("[shell] toggle_panel: no 'main' window"),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};

    let hotkey = Shortcut::new(Some(Modifiers::ALT), Code::Space);
    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_shortcuts([hotkey])
                .expect("valid shortcut")
                .with_handler(move |app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed && *shortcut == hotkey {
                        toggle_panel(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![daemon_request, hide_panel, ui_log])
        .setup(|app| {
            // Debug builds open visible so the panel can be exercised without
            // the hotkey; release stays hidden until ⌥Space.
            #[cfg(debug_assertions)]
            toggle_panel(app.handle());
            let _ = app;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running temporal ui");
}
