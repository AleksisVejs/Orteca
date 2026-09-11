#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Wired to Tauri commands in Milestone 3, when providers land.
#[allow(dead_code)]
mod proc;

fn main() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("failed to start Orteca");
}
