// Phase 1 scaffold: empty window only. Depends on `kestrel-fs` by path
// (see Cargo.toml) but does not call into it yet; IPC is a later phase.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
