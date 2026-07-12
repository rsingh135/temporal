// Prevents an extra console window on Windows; harmless on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    temporal_ui_lib::run()
}
