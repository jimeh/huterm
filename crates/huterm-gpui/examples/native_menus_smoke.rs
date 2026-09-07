//! Checks real `AppKit` menu shortcuts through Huterm's production installer.

#[cfg(target_os = "macos")]
fn main() {
    huterm_gpui::run_native_menu_smoke();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("Native menu smoke skipped: AppKit requires macOS");
}
