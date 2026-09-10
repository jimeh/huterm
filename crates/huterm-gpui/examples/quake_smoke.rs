//! Runs the production quake workflow with native-smoke observations.
fn main() -> anyhow::Result<()> {
    huterm_gpui::run_quake_smoke()
}
