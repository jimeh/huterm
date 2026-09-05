//! Native GPUI terminal client.

#![deny(missing_docs)]

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod config;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod desktop;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod input_queue;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod mouse;
#[cfg(target_os = "macos")]
mod native_quit;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod renderer;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod scroll;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod themes;

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
            toml::from_str(include_str!("../../../Cargo.toml"))
                .expect("Cargo.toml should parse");
        let package = &manifest["package"];
        let packager = &package["metadata"]["packager"];

        assert_eq!(packager["identifier"].as_str(), Some(APP_ID));
        assert_eq!(packager["productName"].as_str(), Some("Huterm"));
        assert_eq!(packager["category"].as_str(), Some("Developer Tool"));
        assert_eq!(packager["formats"][0].as_str(), Some("app"));
        assert_eq!(packager["outDir"].as_str(), Some("target/release/bundle"));
        assert_eq!(
            packager["binariesDir"].as_str(),
            Some("target/aarch64-apple-darwin/release")
        );
        assert_eq!(
            packager["targetTriple"].as_str(),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(
            packager["beforePackagingCommand"].as_str(),
            Some(
                "cargo build --release --locked --target aarch64-apple-darwin -p huterm"
            )
        );
        assert!(packager.get("version").is_none());
        assert_eq!(package["version"]["workspace"].as_bool(), Some(true));
        assert_eq!(
            manifest["workspace"]["package"]["version"].as_str(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(packager["icons"][0].as_str(), Some("assets/Huterm.icns"));
        assert_eq!(packager["binaries"][0]["path"].as_str(), Some("huterm"));
        assert_eq!(packager["binaries"][0]["main"].as_bool(), Some(true));
        let icon = include_bytes!("../../../assets/Huterm.icns");
        assert_eq!(&icon[..4], b"icns");
    }
}
