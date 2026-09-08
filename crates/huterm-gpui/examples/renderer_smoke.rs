//! Exercises production terminal graphics preparation and native painting.

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn main() {
    huterm_gpui::run_renderer_smoke();
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn main() {
    println!("Renderer smoke requires macOS or Linux");
}
