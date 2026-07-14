//! Runs a child process with a hard deadline. Some of our shell-outs
//! (osascript, lsof) can hang if the target app is unresponsive; without a
//! timeout that blocks an entire freeze or rehydrate indefinitely.

use std::io;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Spawns `cmd`, waits up to `timeout` for it to exit, and kills it (then
/// reports a timeout error) if the deadline passes. Captures stdout/stderr
/// like `Command::output()` does, regardless of how the caller configured
/// `cmd`'s stdio.
pub fn run_with_timeout(mut cmd: Command, timeout: Duration) -> io::Result<Output> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child: Child = cmd.spawn()?;
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            let stdout = take_pipe_output(child.stdout.take());
            let stderr = take_pipe_output(child.stderr.take());
            return Ok(Output { status, stdout, stderr });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("process did not exit within {timeout:?}"),
            ));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn take_pipe_output(pipe: Option<impl io::Read>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut p) = pipe {
        let _ = p.read_to_end(&mut buf);
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_command_returns_its_output() {
        let mut cmd = Command::new("/bin/echo");
        cmd.arg("hi").stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        let output = run_with_timeout(cmd, Duration::from_secs(2)).unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hi");
    }

    #[test]
    fn slow_command_is_killed_and_reported_as_timed_out() {
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("5").stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        let err = run_with_timeout(cmd, Duration::from_millis(100)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }
}
