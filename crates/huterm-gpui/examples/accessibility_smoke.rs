//! Runs a production window whose active terminal an external Accessibility
//! client inspects; `scripts/check-accessibility.ts` drives it.

fn main() -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    return huterm_gpui::run_accessibility_smoke();
    #[cfg(not(target_os = "macos"))]
    anyhow::bail!("the accessibility smoke requires macOS");
}
