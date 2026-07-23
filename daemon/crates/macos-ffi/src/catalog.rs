//! Enumerates installed `.app` bundles for the app catalog.
//!
//! Pure filesystem + Info.plist read (the same parsing as bundle identity); no
//! AppKit, so it is safe in the headless LaunchAgent. One shallow pass over the
//! standard application directories — deeply nested bundles are rare and not
//! worth a full-disk walk.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::bundle::read_bundle_identity;
use crate::AppIdentity;

/// Directories scanned for installed apps. `~/Applications` is appended at
/// runtime (per-user installs).
const SEARCH_ROOTS: &[&str] = &[
    "/Applications",
    "/Applications/Utilities",
    "/System/Applications",
    "/System/Applications/Utilities",
];

/// Every installed app resolvable to a bundle id, deduped by bundle id (the
/// first directory in `SEARCH_ROOTS` wins). `AppIdentity::bundle_path` is
/// always populated here.
pub fn scan_installed_apps() -> Vec<AppIdentity> {
    let mut roots: Vec<PathBuf> = SEARCH_ROOTS.iter().map(PathBuf::from).collect();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(Path::new(&home).join("Applications"));
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut apps = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().is_some_and(|e| e == "app") {
                continue;
            }
            let Some(identity) = read_bundle_identity(&path) else { continue };
            if !identity.bundle_id.is_empty() && seen.insert(identity.bundle_id.clone()) {
                apps.push(identity);
            }
        }
    }
    apps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_the_system_applications_and_dedupes() {
        // /System/Applications ships on every macOS host the CI runs on.
        let apps = scan_installed_apps();
        assert!(!apps.is_empty(), "expected at least the system apps");
        // Bundle ids are unique (dedup held) and paths are populated.
        let mut ids: Vec<&str> = apps.iter().map(|a| a.bundle_id.as_str()).collect();
        ids.sort_unstable();
        let unique = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), unique, "duplicate bundle ids leaked through");
        assert!(apps.iter().all(|a| a.bundle_path.is_some()));
    }
}
