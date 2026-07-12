//! Runs JXA (JavaScript for Automation) snippets via /usr/bin/osascript.
//! Requires the per-app Automation (AppleEvents) TCC grant on first use.

use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum OsaError {
    #[error("osascript failed to spawn: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("osascript exited with {status}: {stderr}")]
    Failed { status: i32, stderr: String },
    #[error("osascript output was not valid JSON: {0}")]
    BadJson(#[from] serde_json::Error),
}

/// Runs a JXA script whose final expression is a JSON string and parses it.
pub fn run_jxa_json<T: serde::de::DeserializeOwned>(script: &str) -> Result<T, OsaError> {
    let output = Command::new("/usr/bin/osascript")
        .arg("-l")
        .arg("JavaScript")
        .arg("-e")
        .arg(script)
        .output()?;
    if !output.status.success() {
        return Err(OsaError::Failed {
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(serde_json::from_str(stdout.trim())?)
}
