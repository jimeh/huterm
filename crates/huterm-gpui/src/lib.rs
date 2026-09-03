//! Native GPUI terminal client.

#![deny(missing_docs)]

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod config;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod desktop;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod renderer;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod scroll;

/// Stable application identifier used by runtime diagnostics and packaging.
pub const APP_ID: &str = "app.huterm.dev";

/// Starts the Huterm desktop client.
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
    anyhow::bail!("the current Huterm desktop build requires macOS or Linux")
}

#[cfg(test)]
mod package_tests {
    use super::APP_ID;

    #[test]
    fn package_metadata_matches_runtime_identity() {
        let manifest: toml::Value =
            toml::from_str(include_str!("../../../Packager.toml"))
                .expect("Packager.toml should parse");

        assert_eq!(manifest["identifier"].as_str(), Some(APP_ID));
        assert_eq!(manifest["productName"].as_str(), Some("Huterm"));
        assert_eq!(
            manifest["targetTriple"].as_str(),
            Some("aarch64-apple-darwin")
        );
        assert!(manifest.get("version").is_none());
        assert!(!env!("CARGO_PKG_VERSION").is_empty());
        assert_eq!(manifest["icons"][0].as_str(), Some("assets/Huterm.icns"));
        let icon = include_bytes!("../../../assets/Huterm.icns");
        assert_eq!(&icon[..4], b"icns");
    }
}
