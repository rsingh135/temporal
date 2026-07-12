//! Dev utility: print all on-screen windows with owner identity.
fn main() {
    match temporal_macos_ffi::window_list::onscreen_windows() {
        Ok(windows) => {
            for w in windows {
                let identity = temporal_macos_ffi::bundle::identify_pid(w.owner_pid);
                println!(
                    "pid={} owner={:?} bundle={:?} title={:?} [{},{} {}x{}]",
                    w.owner_pid, w.owner_name, identity.bundle_id, w.title, w.x, w.y, w.width, w.height
                );
            }
        }
        Err(e) => eprintln!("error: {e}"),
    }
}
