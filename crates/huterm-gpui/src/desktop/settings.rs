//! How the Settings window edits each setting the settings model lists: a
//! generic control for its kind, a special control from [`OVERRIDES`], or
//! nothing for settings listed in [`FILE_ONLY`].
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "the Settings window is the consumer")
)]

use huterm_config::settings::{Setting, SettingKind};

/// A control the generic kit does not provide.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Special {
    /// Font families installed on this system.
    FontPicker,
    /// Bundled and custom themes, with color overrides on top.
    ThemePicker,
    /// Modifier chips.
    Modifiers,
    /// A byte size with a unit, or unset.
    Size,
    /// The keybinding editor.
    Keybindings,
    /// Global quake shortcuts.
    GlobalShortcuts,
    /// The quake profile editor.
    QuakeProfiles,
}

const OVERRIDES: &[(&str, Special)] = &[
    ("font.family", Special::FontPicker),
    ("theme", Special::ThemePicker),
    ("terminal.link_modifiers", Special::Modifiers),
    ("terminal.scrollback_bytes", Special::Size),
    ("keybinding", Special::Keybindings),
    ("global_keybinding", Special::GlobalShortcuts),
    ("quake", Special::QuakeProfiles),
];

/// Settings that Settings never edits, leaving them to `config.toml`.
const FILE_ONLY: &[&str] = &["themes", "terminal.engine"];

/// The special control for `path`, if any.
pub(super) fn special(path: &str) -> Option<Special> {
    OVERRIDES
        .iter()
        .find(|(overridden, _)| *overridden == path)
        .map(|(_, special)| *special)
}

pub(super) fn file_only(path: &str) -> bool {
    FILE_ONLY.contains(&path)
}

/// Whether a generic control can edit `setting`.
pub(super) fn generic(setting: &Setting) -> bool {
    !matches!(setting.kind, SettingKind::Compound)
}

#[cfg(test)]
mod tests {
    use huterm_config::settings::SettingsModel;

    use super::*;

    #[test]
    fn every_setting_has_a_control_or_stays_in_the_file() {
        let model = SettingsModel::load().unwrap();
        for setting in model.settings() {
            let path = setting.path.as_str();
            let overridden = special(path).is_some();
            assert!(
                !(overridden && file_only(path)),
                "{path} is both overridden and file-only"
            );
            assert!(
                !setting.deprecated || file_only(path),
                "deprecated {path} must stay file-only"
            );
            if overridden || file_only(path) {
                continue;
            }
            assert!(
                generic(setting),
                "{path} needs a generic control, an override, or a file-only entry"
            );
            if let SettingKind::Choice(choices) = &setting.kind {
                for choice in choices {
                    assert!(
                        choice.label.is_some(),
                        "{path} option {} needs x-huterm-label",
                        choice.value
                    );
                }
            }
        }
        for path in OVERRIDES
            .iter()
            .map(|(path, _)| *path)
            .chain(FILE_ONLY.iter().copied())
        {
            assert!(model.setting(path).is_some(), "{path} is not a setting");
        }
    }
}
