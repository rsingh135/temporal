//! Thin, safe wrappers over the macOS APIs temporald needs.
//!
//! Runs inside a headless LaunchAgent: nothing here may require the main
//! thread or AppKit. Bundle identity is resolved from process executable
//! paths + Info.plist instead of NSRunningApplication for exactly that reason.

pub mod ax;
pub mod bundle;
pub mod display;
pub mod permissions;
pub mod window_list;

pub use bundle::AppIdentity;
pub use window_list::WindowInfo;

#[derive(Debug, thiserror::Error)]
pub enum FfiError {
    #[error("CGWindowListCopyWindowInfo returned NULL (no window server session?)")]
    WindowListUnavailable,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
