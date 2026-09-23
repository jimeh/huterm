//! Starts production windows and tabs, then leaves them idle for external sampling.
#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    huterm_gpui::run_idle_bench()
}

#[cfg(not(target_os = "macos"))]
fn main() -> anyhow::Result<()> {
    anyhow::bail!("idle benchmark currently requires macOS")
}
