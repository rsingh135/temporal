//! Accessibility (AXUIElement) window placement. Requires the Accessibility
//! TCC grant; every entry point degrades to a clear error when untrusted so
//! rehydration proceeds without geometry restore.

use accessibility_sys::{
    kAXPositionAttribute, kAXSizeAttribute, kAXValueTypeCGPoint, kAXValueTypeCGSize,
    kAXWindowsAttribute, AXIsProcessTrusted, AXUIElementCopyAttributeValue,
    AXUIElementCreateApplication, AXUIElementRef, AXUIElementSetAttributeValue, AXValueCreate,
};
use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::string::CFString;

#[repr(C)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
struct CGSize {
    width: f64,
    height: f64,
}

pub fn is_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// Moves/resizes the app's windows (front-to-back order) to `frames`
/// ((x, y, w, h), global coordinates). Returns how many were placed.
pub fn place_windows(pid: i32, frames: &[(f64, f64, f64, f64)]) -> Result<usize, String> {
    if !is_trusted() {
        return Err("accessibility permission not granted".to_string());
    }
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return Err("AXUIElementCreateApplication returned null".to_string());
        }
        let attr = CFString::new(kAXWindowsAttribute);
        let mut windows_ref: core_foundation::base::CFTypeRef = std::ptr::null();
        let err = AXUIElementCopyAttributeValue(
            app,
            attr.as_concrete_TypeRef(),
            &mut windows_ref as *mut _,
        );
        core_foundation::base::CFRelease(app as _);
        if err != accessibility_sys::kAXErrorSuccess {
            return Err(format!("cannot read windows of pid {pid} (AXError {err})"));
        }
        let windows: CFArray<CFType> = CFArray::wrap_under_create_rule(windows_ref as _);
        let mut placed = 0;
        for (i, frame) in frames.iter().enumerate() {
            if i as isize >= windows.len() {
                break;
            }
            let Some(item) = windows.get(i as isize) else { break };
            let window = item.as_CFTypeRef() as AXUIElementRef;
            let (x, y, w, h) = *frame;
            set_frame_retrying(window, x, y, w, h)?;
            placed += 1;
        }
        Ok(placed)
    }
}

/// `set_frame` with bounded retries: a just-launched app's window can
/// transiently reject AX writes before it's fully interactive.
unsafe fn set_frame_retrying(
    window: AXUIElementRef,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
) -> Result<(), String> {
    const ATTEMPTS: u32 = 3;
    const BACKOFF: std::time::Duration = std::time::Duration::from_millis(100);
    let mut last_err = String::new();
    for attempt in 0..ATTEMPTS {
        match unsafe { set_frame(window, x, y, w, h) } {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = e;
                if attempt + 1 < ATTEMPTS {
                    std::thread::sleep(BACKOFF);
                }
            }
        }
    }
    Err(last_err)
}

unsafe fn set_frame(window: AXUIElementRef, x: f64, y: f64, w: f64, h: f64) -> Result<(), String> {
    let point = CGPoint { x, y };
    let size = CGSize { width: w, height: h };
    unsafe {
        let pos_value = AXValueCreate(kAXValueTypeCGPoint, &point as *const _ as *const _);
        let size_value = AXValueCreate(kAXValueTypeCGSize, &size as *const _ as *const _);
        if pos_value.is_null() || size_value.is_null() {
            return Err("AXValueCreate failed".to_string());
        }
        let pos_attr = CFString::new(kAXPositionAttribute);
        let size_attr = CFString::new(kAXSizeAttribute);
        let e1 = AXUIElementSetAttributeValue(window, pos_attr.as_concrete_TypeRef(), pos_value as _);
        let e2 = AXUIElementSetAttributeValue(window, size_attr.as_concrete_TypeRef(), size_value as _);
        core_foundation::base::CFRelease(pos_value as _);
        core_foundation::base::CFRelease(size_value as _);
        if e1 != accessibility_sys::kAXErrorSuccess || e2 != accessibility_sys::kAXErrorSuccess {
            return Err(format!("AXUIElementSetAttributeValue failed ({e1}/{e2})"));
        }
    }
    Ok(())
}

/// Polls until the pid has at least `min_windows` on-screen windows or the
/// timeout passes; used after launching an app before placing its windows.
pub fn wait_for_windows(pid: i32, min_windows: usize, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let count = crate::window_list::onscreen_windows()
            .map(|ws| ws.iter().filter(|w| w.owner_pid == pid).count())
            .unwrap_or(0);
        if count >= min_windows {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}
