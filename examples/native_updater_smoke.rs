//! Checks Sparkle through a fixture application bundle.

#[cfg(all(target_os = "macos", feature = "macos-updater"))]
fn main() -> anyhow::Result<()> {
    huterm_gpui::run_native_updater_smoke()
}

#[cfg(not(all(target_os = "macos", feature = "macos-updater")))]
fn main() {
    println!("Native updater smoke skipped: enable macos-updater on macOS");
}
