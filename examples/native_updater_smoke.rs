//! Checks Sparkle through a fixture application bundle.

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    huterm_gpui::run_native_updater_smoke()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("Native updater smoke skipped: Sparkle requires macOS");
}
