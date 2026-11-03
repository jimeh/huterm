//! Shell launch configuration, independent of desktop and engine types.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Automatic shell integration for terminals started after this setting changes.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct ShellIntegrationConfig {
    /// Load Huterm's prompt and directory integration in supported shells.
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "terminal",
        "x-huterm-group" = "Shell integration",
        "x-huterm-label" = "Shell integration",
        "x-huterm-order" = 80,
        "x-huterm-apply" = "new_terminals",
    )))]
    pub mode: ShellIntegrationMode,
    /// Optional capabilities. `path` appends Huterm's executable directory to PATH.
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "terminal",
        "x-huterm-group" = "Shell integration",
        "x-huterm-label" = "Shell integration features",
        "x-huterm-order" = 81,
        "x-huterm-apply" = "new_terminals",
    )))]
    pub features: BTreeSet<ShellFeature>,
}

/// Whether to load shell integration automatically.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ShellIntegrationMode {
    /// Detect supported shells and load integration.
    #[default]
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Detect")))]
    Detect,
    /// Disable automatic loading.
    #[cfg_attr(feature = "schema", schemars(extend("x-huterm-label" = "Off")))]
    Off,
}

/// A known opt-in shell capability.
#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ShellFeature {
    /// Append Huterm's executable directory to PATH once.
    Path,
}

impl ShellIntegrationConfig {
    /// Convert portable configuration into the core launch request.
    #[must_use]
    pub fn to_protocol(&self) -> huterm_protocol::ShellIntegration {
        huterm_protocol::ShellIntegration {
            mode: match self.mode {
                ShellIntegrationMode::Detect => {
                    huterm_protocol::ShellIntegrationMode::Detect
                }
                ShellIntegrationMode::Off => {
                    huterm_protocol::ShellIntegrationMode::Off
                }
            },
            features: self
                .features
                .iter()
                .map(|feature| match feature {
                    ShellFeature::Path => huterm_protocol::ShellFeature::Path,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_integration_defaults_and_known_features() {
        let raw: crate::RawConfig = toml::from_str("").unwrap();
        assert_eq!(raw.shell_integration, ShellIntegrationConfig::default());
        let raw: crate::RawConfig = toml::from_str(
            "[shell_integration]\nmode='off'\nfeatures=['path','path']",
        )
        .unwrap();
        assert_eq!(raw.shell_integration.mode, ShellIntegrationMode::Off);
        assert_eq!(raw.shell_integration.features.len(), 1);
        assert!(
            raw.shell_integration
                .to_protocol()
                .features
                .contains(huterm_protocol::ShellFeature::Path)
        );
    }

    #[test]
    fn shell_integration_rejects_unknown_values_and_fields() {
        for field in ["mode='auto'", "features=['title']", "extra=true"] {
            assert!(
                toml::from_str::<crate::RawConfig>(&format!(
                    "[shell_integration]\n{field}"
                ))
                .is_err()
            );
        }
    }
}
