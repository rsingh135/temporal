//! Pure geometry helpers for rehydration. No macOS FFI here — display
//! enumeration lives in `macos-ffi`; this module only reasons about rectangles
//! so it can be unit-tested without a GUI session.

use crate::WindowGeometry;

/// Clamps a captured window's geometry to fit within one of the currently
/// active displays. If `g` already overlaps any display (even partially),
/// it's returned unchanged — the window is at least partly reachable. If it
/// overlaps none (e.g. it was captured on a monitor that's since been
/// unplugged), it's resized to fit and repositioned onto the nearest display
/// by center distance. An empty `displays` list (enumeration failed) leaves
/// `g` unchanged rather than guessing.
pub fn clamp_to_displays(g: WindowGeometry, displays: &[WindowGeometry]) -> WindowGeometry {
    if displays.is_empty() {
        return g;
    }
    if displays.iter().any(|d| overlaps(&g, d)) {
        return g;
    }
    let nearest = nearest_display(&g, displays);
    let width = g.width.min(nearest.width);
    let height = g.height.min(nearest.height);
    let x = g.x.clamp(nearest.x, nearest.x + nearest.width - width);
    let y = g.y.clamp(nearest.y, nearest.y + nearest.height - height);
    WindowGeometry { x, y, width, height }
}

fn overlaps(g: &WindowGeometry, d: &WindowGeometry) -> bool {
    g.x < d.x + d.width && g.x + g.width > d.x && g.y < d.y + d.height && g.y + g.height > d.y
}

fn nearest_display<'a>(g: &WindowGeometry, displays: &'a [WindowGeometry]) -> &'a WindowGeometry {
    let center_x = g.x + g.width / 2.0;
    let center_y = g.y + g.height / 2.0;
    displays
        .iter()
        .min_by(|a, b| {
            dist_sq(center_x, center_y, a)
                .partial_cmp(&dist_sq(center_x, center_y, b))
                .expect("distances are finite")
        })
        .expect("displays is non-empty")
}

fn dist_sq(cx: f64, cy: f64, d: &WindowGeometry) -> f64 {
    let dcx = d.x + d.width / 2.0 - cx;
    let dcy = d.y + d.height / 2.0 - cy;
    dcx * dcx + dcy * dcy
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geo(x: f64, y: f64, width: f64, height: f64) -> WindowGeometry {
        WindowGeometry { x, y, width, height }
    }

    #[test]
    fn empty_display_list_leaves_geometry_unchanged() {
        let g = geo(100.0, 100.0, 800.0, 600.0);
        assert_eq!(clamp_to_displays(g, &[]), g);
    }

    #[test]
    fn in_bounds_geometry_is_unchanged() {
        let display = geo(0.0, 0.0, 1920.0, 1080.0);
        let g = geo(100.0, 100.0, 800.0, 600.0);
        assert_eq!(clamp_to_displays(g, &[display]), g);
    }

    #[test]
    fn straddling_an_edge_is_left_unchanged() {
        // Partially on-screen: still reachable, so no rewrite.
        let display = geo(0.0, 0.0, 1920.0, 1080.0);
        let g = geo(1900.0, 100.0, 800.0, 600.0);
        assert_eq!(clamp_to_displays(g, &[display]), g);
    }

    #[test]
    fn fully_outside_every_display_is_clamped_to_nearest() {
        // Captured on a second monitor to the right that's since been unplugged.
        let primary = geo(0.0, 0.0, 1920.0, 1080.0);
        let g = geo(2100.0, 100.0, 800.0, 600.0);
        let clamped = clamp_to_displays(g, &[primary]);
        assert!(clamped.x >= primary.x && clamped.x + clamped.width <= primary.x + primary.width);
        assert!(clamped.y >= primary.y && clamped.y + clamped.height <= primary.y + primary.height);
        assert_eq!(clamped.width, 800.0);
        assert_eq!(clamped.height, 600.0);
    }

    #[test]
    fn oversized_window_is_shrunk_to_fit_nearest_display() {
        let small = geo(0.0, 0.0, 1024.0, 768.0);
        let g = geo(5000.0, 5000.0, 1920.0, 1080.0);
        let clamped = clamp_to_displays(g, &[small]);
        assert_eq!(clamped.width, 1024.0);
        assert_eq!(clamped.height, 768.0);
    }

    #[test]
    fn picks_the_nearest_of_multiple_displays() {
        let left = geo(0.0, 0.0, 1920.0, 1080.0);
        let right = geo(4000.0, 0.0, 1920.0, 1080.0);
        // Captured far to the right of `right`, closer to it than to `left`.
        let g = geo(6000.0, 100.0, 800.0, 600.0);
        let clamped = clamp_to_displays(g, &[left, right]);
        assert!(clamped.x >= right.x && clamped.x + clamped.width <= right.x + right.width);
    }
}
