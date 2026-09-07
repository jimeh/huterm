//! Drives native events through the production desktop in an isolated process.
#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    huterm_gpui::run_native_input_smoke()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("native input smoke requires macOS");
    std::process::exit(1);
}
