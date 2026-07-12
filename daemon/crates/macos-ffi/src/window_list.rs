//! Snapshot of on-screen windows via CGWindowListCopyWindowInfo.
//!
//! Window *titles* (kCGWindowName) are only populated when the process has
//! Screen Recording permission; everything else (owner, pid, bounds, layer)
//! is available without it. Callers must treat `title` as best-effort.

use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString};
use objc2_core_graphics::{CGWindowListCopyWindowInfo, CGWindowListOption};

use crate::FfiError;

#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    pub window_id: u32,
    pub owner_pid: i32,
    /// Application name as reported by the window server (kCGWindowOwnerName).
    pub owner_name: String,
    /// Window title; empty unless Screen Recording permission is granted.
    pub title: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub layer: i32,
}

/// All on-screen, layer-0 (normal document) windows, front-to-back.
pub fn onscreen_windows() -> Result<Vec<WindowInfo>, FfiError> {
    let raw = list_windows()?;
    Ok(raw.into_iter().filter(|w| w.layer == 0).collect())
}

fn list_windows() -> Result<Vec<WindowInfo>, FfiError> {
    let options =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let array: CFRetained<CFArray> =
        CGWindowListCopyWindowInfo(options, 0).ok_or(FfiError::WindowListUnavailable)?;

    let mut windows = Vec::new();
    for i in 0..array.count() {
        let dict = unsafe { array.value_at_index(i) } as *const CFDictionary;
        if dict.is_null() {
            continue;
        }
        let dict = unsafe { &*dict };
        let Some(window_id) = get_number(dict, "kCGWindowNumber") else { continue };
        let Some(owner_pid) = get_number(dict, "kCGWindowOwnerPID") else { continue };
        let layer = get_number(dict, "kCGWindowLayer").unwrap_or(0.0) as i32;
        let owner_name = get_string(dict, "kCGWindowOwnerName").unwrap_or_default();
        let title = get_string(dict, "kCGWindowName").unwrap_or_default();
        let (x, y, width, height) = get_bounds(dict).unwrap_or((0.0, 0.0, 0.0, 0.0));
        windows.push(WindowInfo {
            window_id: window_id as u32,
            owner_pid: owner_pid as i32,
            owner_name,
            title,
            x,
            y,
            width,
            height,
            layer,
        });
    }
    Ok(windows)
}

fn get_value(dict: &CFDictionary, key: &str) -> *const std::ffi::c_void {
    let key = CFString::from_str(key);
    unsafe { dict.value(&*key as *const CFString as *const std::ffi::c_void) }
}

fn get_number(dict: &CFDictionary, key: &str) -> Option<f64> {
    let value = get_value(dict, key);
    if value.is_null() {
        return None;
    }
    let number = unsafe { &*(value as *const CFNumber) };
    number.as_f64()
}

fn get_string(dict: &CFDictionary, key: &str) -> Option<String> {
    let value = get_value(dict, key);
    if value.is_null() {
        return None;
    }
    let s = unsafe { &*(value as *const CFString) };
    Some(s.to_string())
}

fn get_bounds(dict: &CFDictionary) -> Option<(f64, f64, f64, f64)> {
    let value = get_value(dict, "kCGWindowBounds");
    if value.is_null() {
        return None;
    }
    let bounds = unsafe { &*(value as *const CFDictionary) };
    Some((
        get_number(bounds, "X")?,
        get_number(bounds, "Y")?,
        get_number(bounds, "Width")?,
        get_number(bounds, "Height")?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Runs in a GUI session on the dev machine; in a headless CI session the
    // call may legitimately fail, so only assert the happy path when possible.
    #[test]
    fn window_list_is_readable_or_reports_unavailable() {
        match onscreen_windows() {
            Ok(windows) => {
                for w in &windows {
                    assert!(w.owner_pid > 0);
                    assert_eq!(w.layer, 0);
                }
            }
            Err(FfiError::WindowListUnavailable) => {}
            Err(other) => panic!("unexpected error: {other}"),
        }
    }
}
