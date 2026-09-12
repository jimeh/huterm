//! Drives the production command palette in an isolated process.

fn main() -> anyhow::Result<()> {
    huterm_gpui::run_palette_smoke()
}
