//! Native GPUI terminal client.

#![deny(missing_docs)]

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod commands;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod config;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod desktop;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod file_drop;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod fullscreen;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod input_queue;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod keymap;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod mouse;
#[cfg(target_os = "macos")]
mod native_fullscreen;
#[cfg(target_os = "macos")]
mod native_quit;
#[cfg(all(target_os = "macos", feature = "macos-updater"))]
mod native_updater;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod quake;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod renderer;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod scroll;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod themes;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod ui;

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

/// Runs the isolated `AppKit` menu verification executable.
#[doc(hidden)]
#[cfg(target_os = "macos")]
pub fn run_native_menu_smoke() {
    desktop::menus_smoke::run();
}

/// Runs the packaged Sparkle controller and command verification executable.
#[doc(hidden)]
#[cfg(all(target_os = "macos", feature = "macos-updater"))]
pub fn run_native_updater_smoke() -> anyhow::Result<()> {
    desktop::updater_smoke::run()
}

/// Runs the isolated native input verification executable.
#[doc(hidden)]
#[cfg(target_os = "macos")]
pub fn run_native_input_smoke() -> anyhow::Result<()> {
    desktop::input_smoke::run()
}

/// Runs the production desktop with the fullscreen smoke command probe.
#[doc(hidden)]
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn run_fullscreen_smoke() -> anyhow::Result<()> {
    desktop::fullscreen_smoke::run()
}

/// Runs the production desktop with the command-palette smoke probe.
#[doc(hidden)]
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn run_palette_smoke() -> anyhow::Result<()> {
    desktop::palette_smoke::run()
}

/// Runs the production quake workflow with native-smoke observations.
#[doc(hidden)]
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn run_quake_smoke() -> anyhow::Result<()> {
    desktop::quake_smoke::run()
}

/// Runs the isolated native terminal graphics fixture.
#[doc(hidden)]
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn run_renderer_smoke() {
    renderer::smoke::run();
}

/// Runs the isolated native links and file-drop verification executable.
#[doc(hidden)]
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn run_integration_smoke() -> anyhow::Result<()> {
    desktop::integration_smoke::run()
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
            Some("target/universal-apple-darwin/release")
        );
        assert_eq!(
            packager["targetTriple"].as_str(),
            Some("universal-apple-darwin")
        );
        assert_eq!(
            packager["beforePackagingCommand"].as_str(),
            Some("mise run package:build:macos")
        );
        assert!(packager.get("version").is_none());
        assert_eq!(
            package["version"].as_str(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(packager["icons"][0].as_str(), Some("assets/Huterm.icns"));
        assert_eq!(packager["binaries"][0]["path"].as_str(), Some("huterm"));
        assert_eq!(packager["binaries"][0]["main"].as_bool(), Some(true));
        assert_eq!(
            packager["macos"]["minimumSystemVersion"].as_str(),
            Some("10.15.7")
        );
        assert!(packager["macos"].get("frameworks").is_none());
        assert!(packager["resources"].as_array().is_some_and(|resources| {
            resources.iter().all(|resource| {
                resource["target"].as_str() != Some("Sparkle-LICENSE")
            })
        }));
        let icon = include_bytes!("../../../assets/Huterm.icns");
        assert_eq!(&icon[..4], b"icns");
    }
}
