//! Runs JXA (JavaScript for Automation) snippets via /usr/bin/osascript.
//! Requires the per-app Automation (AppleEvents) TCC grant on first use.

use std::process::Command;
use std::time::Duration;

use crate::proc_timeout;

const TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_ATTEMPTS: u32 = 3;
const RETRY_BACKOFF: Duration = Duration::from_millis(300);

#[derive(Debug, thiserror::Error)]
pub enum OsaError {
    #[error("osascript failed to spawn: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("osascript exited with {status}: {stderr}")]
    Failed { status: i32, stderr: String },
    #[error("osascript output was not valid JSON: {0}")]
    BadJson(#[from] serde_json::Error),
}

impl OsaError {
    /// Whether retrying might help: a transient spawn/exit failure can
    /// succeed on a later attempt, but bad output from a successful run
    /// won't change.
    fn is_retryable(&self) -> bool {
        !matches!(self, OsaError::BadJson(_))
    }
}

/// Runs a JXA script whose final expression is a JSON string and parses it.
pub fn run_jxa_json<T: serde::de::DeserializeOwned>(script: &str) -> Result<T, OsaError> {
    let mut cmd = Command::new("/usr/bin/osascript");
    cmd.arg("-l").arg("JavaScript").arg("-e").arg(script);
    let output = proc_timeout::run_with_timeout(cmd, TIMEOUT)?;
    if !output.status.success() {
        return Err(OsaError::Failed {
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(serde_json::from_str(stdout.trim())?)
}

/// `run_jxa_json` with bounded retries for transient spawn/exit failures
/// (e.g. the target app not yet ready to receive Apple Events).
pub fn run_jxa_json_retrying<T: serde::de::DeserializeOwned>(script: &str) -> Result<T, OsaError> {
    let mut attempt = 0;
    loop {
        match run_jxa_json(script) {
            Ok(value) => return Ok(value),
            Err(e) if e.is_retryable() && attempt + 1 < RETRY_ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(RETRY_BACKOFF);
            }
            Err(e) => return Err(e),
        }
    }
}
