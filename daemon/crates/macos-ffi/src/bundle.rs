//! Resolves a pid to its app bundle identity by walking the executable path
//! up to the enclosing `.app` bundle and reading Info.plist. Headless-safe
//! (no AppKit) and cached per pid by callers if needed.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct AppIdentity {
    pub bundle_id: String,
    pub app_name: String,
    pub bundle_path: Option<PathBuf>,
}

/// Best-effort identity; falls back to the process name with an empty bundle
/// id for non-bundled processes (daemons, CLI tools).
pub fn identify_pid(pid: i32) -> AppIdentity {
    let exe = libproc::proc_pid::pidpath(pid).unwrap_or_default();
    identify_executable(Path::new(&exe), pid)
}

/// Bundle ids of every running .app process, whether or not it has windows on
/// the current Space. CGWindowList only sees the active Space, so adapter
/// dispatch must use this instead.
pub fn running_bundle_ids() -> std::collections::HashSet<String> {
    let mut by_bundle_path: std::collections::HashMap<PathBuf, Option<String>> =
        std::collections::HashMap::new();
    let mut out = std::collections::HashSet::new();
    let pids =
        libproc::processes::pids_by_type(libproc::processes::ProcFilter::All).unwrap_or_default();
    for pid in pids {
        let Ok(exe) = libproc::proc_pid::pidpath(pid as i32) else { continue };
        let Some(bundle_root) = enclosing_app_bundle(Path::new(&exe)) else { continue };
        let bundle_id = by_bundle_path
            .entry(bundle_root.clone())
            .or_insert_with(|| read_bundle_identity(&bundle_root).map(|i| i.bundle_id));
        if let Some(bundle_id) = bundle_id {
            out.insert(bundle_id.clone());
        }
    }
    out
}

fn identify_executable(exe: &Path, pid: i32) -> AppIdentity {
    if let Some(bundle_root) = enclosing_app_bundle(exe)
        && let Some(identity) = read_bundle_identity(&bundle_root)
    {
        return identity;
    }
    let fallback_name = exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .or_else(|| libproc::proc_pid::name(pid).ok())
        .unwrap_or_default();
    AppIdentity { bundle_id: String::new(), app_name: fallback_name, bundle_path: None }
}

/// The OUTERMOST `.app` ancestor, so helper processes (e.g.
/// `Chrome.app/.../Google Chrome Helper.app`) attribute to the application
/// that owns them.
fn enclosing_app_bundle(exe: &Path) -> Option<PathBuf> {
    let mut current: Option<&Path> = Some(exe);
    let mut outermost: Option<PathBuf> = None;
    while let Some(path) = current {
        if path.extension().is_some_and(|e| e == "app") {
            outermost = Some(path.to_path_buf());
        }
        current = path.parent();
    }
    outermost
}

fn read_bundle_identity(bundle_root: &Path) -> Option<AppIdentity> {
    let info = plist::Value::from_file(bundle_root.join("Contents/Info.plist")).ok()?;
    let dict = info.as_dictionary()?;
    let bundle_id = dict.get("CFBundleIdentifier")?.as_string()?.to_string();
    let app_name = dict
        .get("CFBundleDisplayName")
        .or_else(|| dict.get("CFBundleName"))
        .and_then(|v| v.as_string())
        .map(str::to_string)
        .unwrap_or_else(|| {
            bundle_root.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        });
    Some(AppIdentity { bundle_id, app_name, bundle_path: Some(bundle_root.to_path_buf()) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finder_identifies_by_bundle() {
        let identity = identify_executable(
            Path::new("/System/Library/CoreServices/Finder.app/Contents/MacOS/Finder"),
            0,
        );
        assert_eq!(identity.bundle_id, "com.apple.finder");
    }

    #[test]
    fn helper_attributes_to_outermost_app() {
        let path = Path::new(
            "/Applications/Google Chrome.app/Contents/Frameworks/Google Chrome Framework.framework/Versions/1/Helpers/Google Chrome Helper.app/Contents/MacOS/Google Chrome Helper",
        );
        let bundle = enclosing_app_bundle(path).unwrap();
        assert_eq!(bundle, PathBuf::from("/Applications/Google Chrome.app"));
    }

    #[test]
    fn non_bundled_process_falls_back_to_executable_name() {
        let identity = identify_executable(Path::new("/usr/bin/lsof"), -1);
        assert_eq!(identity.bundle_id, "");
        assert_eq!(identity.app_name, "lsof");
    }
}
