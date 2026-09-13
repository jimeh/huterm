//! Drives application visibility through the production desktop command path.

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    huterm_gpui::run_clipboard_smoke()
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("clipboard application smoke requires macOS");
    std::process::exit(1);
}
