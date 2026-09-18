//! Measures production terminal row preparation and native painting.

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn main() -> anyhow::Result<()> {
    huterm_gpui::run_renderer_bench()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn main() {
    println!("Renderer benchmark requires macOS or Linux");
}
