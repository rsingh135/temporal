//! Generic fallback: any visible window not owned by a specialized adapter
//! becomes a bundle-id + geometry node.

use std::collections::HashMap;

use temporal_domain::{AdapterKind, NodePayload, WindowGeometry, WindowNode};
use temporal_macos_ffi::{AppIdentity, WindowInfo};

use crate::ExtractionReport;

/// Windows smaller than this are status items, tooltips or popovers.
const MIN_WIDTH: f64 = 100.0;
const MIN_HEIGHT: f64 = 80.0;

pub fn extract(
    windows: &[WindowInfo],
    identities: &HashMap<i32, AppIdentity>,
    consumed_bundle_ids: &[&str],
    report: &mut ExtractionReport,
) {
    for w in windows {
        if w.width < MIN_WIDTH || w.height < MIN_HEIGHT {
            continue;
        }
        let Some(identity) = identities.get(&w.owner_pid) else { continue };
        // Non-bundled processes can't be relaunched meaningfully.
        if identity.bundle_id.is_empty() {
            continue;
        }
        if consumed_bundle_ids.contains(&identity.bundle_id.as_str()) {
            continue;
        }
        let window_title =
            if w.title.is_empty() { identity.app_name.clone() } else { w.title.clone() };
        report.nodes.push(WindowNode {
            node_id: String::new(),
            bundle_id: identity.bundle_id.clone(),
            app_name: identity.app_name.clone(),
            window_title,
            geometry: WindowGeometry { x: w.x, y: w.y, width: w.width, height: w.height },
            adapter: AdapterKind::Generic,
            payload: NodePayload::Generic,
        });
    }
}
