// Thin binary shim; all setup lives in the library so it stays testable.
fn main() {
    kestrel_tauri_lib::run();
}
