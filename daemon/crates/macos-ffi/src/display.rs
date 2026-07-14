//! Active display enumeration, so rehydration can tell whether a captured
//! window's geometry still lands on a currently-connected screen.
//!
//! Uses CGDirectDisplay (Quartz), not NSScreen/AppKit, so it stays safe to
//! call from the headless LaunchAgent (see lib.rs doc comment).

use objc2_core_graphics::{CGDisplayBounds, CGError, CGGetActiveDisplayList, CGMainDisplayID};

const MAX_DISPLAYS: u32 = 16;

/// Bounds (x, y, width, height) of every active display, in global screen
/// coordinates. The main display is always first when present. Returns an
/// empty vec if the window server can't be reached (e.g. headless CI).
pub fn active_display_frames() -> Vec<(f64, f64, f64, f64)> {
    let mut ids = [0u32; MAX_DISPLAYS as usize];
    let mut count: u32 = 0;
    let err = unsafe {
        CGGetActiveDisplayList(MAX_DISPLAYS, ids.as_mut_ptr(), &mut count as *mut u32)
    };
    if err != CGError::Success {
        return Vec::new();
    }

    let main_id = CGMainDisplayID();
    let mut ids = ids[..count as usize].to_vec();
    ids.sort_by_key(|&id| if id == main_id { 0 } else { 1 });

    ids.into_iter()
        .map(|id| {
            let rect = CGDisplayBounds(id);
            (rect.origin.x, rect.origin.y, rect.size.width, rect.size.height)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Runs in a GUI session on the dev machine; in a headless CI session the
    // call may legitimately report zero displays.
    #[test]
    fn active_displays_are_readable_or_empty() {
        for (_, _, width, height) in active_display_frames() {
            assert!(width > 0.0 && height > 0.0);
        }
    }
}
