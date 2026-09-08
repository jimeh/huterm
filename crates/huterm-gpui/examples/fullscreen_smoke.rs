//! Drives the production desktop fullscreen controller in an isolated process.
fn main() -> anyhow::Result<()> {
    huterm_gpui::run_fullscreen_smoke()
}
