//! Shell launch configuration, independent of desktop and engine types.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Automatic shell integration for terminals started after this setting changes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
    /// Optional capabilities. `path` appends the directory holding the
    /// `huterm` command to PATH. `ssh_env` wraps `ssh` to forward
    /// `COLORTERM`, `TERM_PROGRAM`, and `TERM_PROGRAM_VERSION`.
    /// `ssh_terminfo` wraps `ssh` to install the `xterm-huterm` terminfo
    /// entry in `~/.terminfo` on remote hosts. `sudo` wraps `sudo` to use
    /// `TERM=xterm-256color` when the system lacks `xterm-huterm`. The
    /// default is `["path", "ssh_env", "sudo"]`.
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

impl Default for ShellIntegrationConfig {
    fn default() -> Self {
        Self {
            mode: ShellIntegrationMode::default(),
            features: BTreeSet::from([
                ShellFeature::Path,
                ShellFeature::SshEnv,
                ShellFeature::Sudo,
            ]),
        }
    }
}

/// A known optional shell capability.
#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ShellFeature {
    /// Append the directory holding the `huterm` command to PATH once.
    Path,
    /// Wrap `ssh` to forward `COLORTERM`, `TERM_PROGRAM`, and
    /// `TERM_PROGRAM_VERSION`, and to use `TERM=xterm-256color` on hosts
    /// without the `xterm-huterm` entry.
    SshEnv,
    /// Wrap `ssh` to install the `xterm-huterm` terminfo entry on remote
    /// hosts, in `~/.terminfo`.
    SshTerminfo,
    /// Wrap `sudo` to use `TERM=xterm-256color` when the system terminfo
    /// lacks `xterm-huterm`.
    Sudo,
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
                    ShellFeature::SshEnv => {
                        huterm_protocol::ShellFeature::SshEnv
                    }
                    ShellFeature::SshTerminfo => {
                        huterm_protocol::ShellFeature::SshTerminfo
                    }
                    ShellFeature::Sudo => huterm_protocol::ShellFeature::Sudo,
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
        let defaults = BTreeSet::from([
            ShellFeature::Path,
            ShellFeature::SshEnv,
            ShellFeature::Sudo,
        ]);
        assert_eq!(raw.shell_integration.features, defaults, "on by default");
        let raw: crate::RawConfig =
            toml::from_str("[shell_integration]\nmode='detect'").unwrap();
        assert_eq!(raw.shell_integration.features, defaults, "an omitted list");
        let raw: crate::RawConfig = toml::from_str(
            "[shell_integration]\nfeatures=['sudo','ssh_terminfo','ssh_env']",
        )
        .unwrap();
        assert_eq!(
            raw.shell_integration
                .to_protocol()
                .features
                .iter()
                .collect::<Vec<_>>(),
            [
                huterm_protocol::ShellFeature::SshEnv,
                huterm_protocol::ShellFeature::SshTerminfo,
                huterm_protocol::ShellFeature::Sudo,
            ],
            "every feature maps to the protocol"
        );
        let raw: crate::RawConfig =
            toml::from_str("[shell_integration]\nfeatures=[]").unwrap();
        assert!(raw.shell_integration.features.is_empty(), "opted out");
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
        for field in [
            "mode='auto'",
            "features=['title']",
            "features=['ssh-env']",
            "features=['sshEnv']",
            "extra=true",
        ] {
            assert!(
                toml::from_str::<crate::RawConfig>(&format!(
                    "[shell_integration]\n{field}"
                ))
                .is_err()
            );
        }
    }
}
