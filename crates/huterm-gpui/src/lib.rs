//! Native GPUI terminal client.

#![deny(missing_docs)]

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod desktop;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod renderer;

/// Starts the `HUTerm` desktop client.
///
/// # Errors
///
/// Returns an error when the current platform is not part of the initial
/// milestone or when terminal startup fails.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn run() -> anyhow::Result<()> {
    desktop::run()
}

/// Reports the current desktop client's platform requirement.
///
/// # Errors
///
/// This function always returns an error outside macOS and Linux.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn run() -> anyhow::Result<()> {
    anyhow::bail!("the current HUTerm desktop build requires macOS or Linux")
}
