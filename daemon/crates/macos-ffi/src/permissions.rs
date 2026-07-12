//! TCC permission preflights. The daemon degrades gracefully when a
//! permission is missing; these checks let it report *which* capability is
//! reduced instead of failing mysteriously.

use objc2_core_graphics::CGPreflightScreenCaptureAccess;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionStatus {
    /// Window titles in CGWindowList require Screen Recording.
    pub screen_recording: bool,
}

pub fn preflight() -> PermissionStatus {
    PermissionStatus { screen_recording: CGPreflightScreenCaptureAccess() }
}
