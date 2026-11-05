//! Drives session restore through the production desktop in an isolated
//! process.
fn main() -> anyhow::Result<()> {
    huterm_gpui::run_restore_smoke()
}
