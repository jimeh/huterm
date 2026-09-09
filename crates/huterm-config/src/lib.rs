//! Portable configuration definitions and validation shared by the desktop and schemas.

use huterm_protocol::Rgb;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

#[cfg(feature = "schema")]
pub mod schema;

/// One `[[keybinding]]` entry as written in the configuration file.
///
/// Only structure is validated here; keystroke, predicate, command, and
/// argument semantics belong to the keymap compiler.
#[derive(Clone, Debug, PartialEq)]
pub struct KeybindingEntry {
    pub key: String,
    pub command: String,
    pub args: Option<toml::Table>,
    pub when: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct TerminalConfig {
    pub close_on_exit: bool,
    pub links: bool,
    pub link_modifiers: LinkModifiers,
    pub macos_option_as_alt: MacosOptionAsAlt,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            close_on_exit: true,
            links: true,
            link_modifiers: LinkModifiers::default(),
            macos_option_as_alt: MacosOptionAsAlt::Off,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "String")]
pub struct LinkModifiers(u8);

impl Default for LinkModifiers {
    fn default() -> Self {
        Self(if cfg!(target_os = "macos") { 1 } else { 2 })
    }
}
impl TryFrom<String> for LinkModifiers {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value, cfg!(target_os = "macos"))
    }
}
impl LinkModifiers {
    /// Parses a modifier combination for the selected platform.
    ///
    /// # Errors
    /// Returns an error when the input violates the configuration contract.
    pub fn parse(value: &str, macos: bool) -> Result<Self, &'static str> {
        let mut bits = 0;
        for token in value.split('-') {
            let bit = match token {
                "cmd" => 1,
                "ctrl" => 2,
                "alt" => 4,
                "shift" => 8,
                _ => {
                    return Err(
                        "terminal.link_modifiers must contain only cmd, ctrl, alt, or shift",
                    );
                }
            };
            if bits & bit != 0 {
                return Err(
                    "terminal.link_modifiers cannot contain duplicate modifiers",
                );
            }
            bits |= bit;
        }
        if macos && bits & 2 != 0 {
            return Err(
                "terminal.link_modifiers cannot use ctrl on macOS: GPUI converts Control-left into Right and removes Control",
            );
        }
        Ok(Self(bits))
    }
    #[must_use]
    pub fn matches_bits(self, actual: u8, mouse_reporting: bool) -> bool {
        actual == (self.0 | if mouse_reporting { 8 } else { 0 })
    }
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum MacosOptionAsAlt {
    #[default]
    Off,
    Both,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub macos_fullscreen_mode: MacosFullscreenMode,
    pub padding_x: f32,
    pub padding_y: f32,
    pub padding_balance: bool,
    pub tab_position: TabPosition,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            macos_fullscreen_mode: MacosFullscreenMode::NonNative,
            padding_x: 4.0,
            padding_y: 4.0,
            padding_balance: false,
            tab_position: TabPosition::Top,
        }
    }
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum MacosFullscreenMode {
    #[serde(rename = "native")]
    Native,
    #[default]
    #[serde(rename = "non_native")]
    NonNative,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum TabPosition {
    #[default]
    Top,
    Bottom,
    Left,
    Right,
}

impl TabPosition {
    #[must_use]
    pub fn vertical(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Theme {
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub selection: Rgb,
    pub selection_foreground: Option<Rgb>,
    pub ansi: [Rgb; 16],
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            foreground: rgb(0x00c5_c8c6),
            background: rgb(0x001d_1f21),
            cursor: rgb(0x00ff_ffff),
            selection: rgb(0x0026_4f78),
            selection_foreground: None,
            ansi: [
                rgb(0x001d_1f21),
                rgb(0x00cc_6666),
                rgb(0x00b5_bd68),
                rgb(0x00f0_c674),
                rgb(0x0081_a2be),
                rgb(0x00b2_94bb),
                rgb(0x008a_beb7),
                rgb(0x00c5_c8c6),
                rgb(0x0066_6666),
                rgb(0x00d5_4e53),
                rgb(0x00b9_ca4a),
                rgb(0x00e7_c547),
                rgb(0x007a_a6da),
                rgb(0x00c3_97d8),
                rgb(0x0070_c0b1),
                rgb(0x00ea_eaea),
            ],
        }
    }
}

impl Theme {
    #[must_use]
    pub fn indexed(&self, index: u8) -> Rgb {
        if let Some(color) = self.ansi.get(usize::from(index)) {
            return *color;
        }
        if index >= 232 {
            let level = 8_u8.saturating_add((index - 232).saturating_mul(10));
            return Rgb {
                red: level,
                green: level,
                blue: level,
            };
        }
        let value = index - 16;
        let channel = |component: u8| {
            if component == 0 {
                0
            } else {
                55 + component * 40
            }
        };
        Rgb {
            red: channel(value / 36),
            green: channel((value / 6) % 6),
            blue: channel(value % 6),
        }
    }
}

/// Formats a keybinding diagnostic with its 1-based file position and key.
pub fn keybinding_diagnostic(
    index: usize,
    key: &str,
    message: impl fmt::Display,
) -> String {
    format!("keybinding {index} ({key:?}): {message}")
}

/// Parses a six-digit RGB color.
///
/// # Errors
/// Returns an error for colors other than `#rrggbb`.
pub fn parse_color(value: &str) -> Result<Rgb, ConfigError> {
    let Some(hex) = value.strip_prefix('#') else {
        return Err(ConfigError::Color(value.into()));
    };
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ConfigError::Color(value.into()));
    }
    let value = u32::from_str_radix(hex, 16)
        .map_err(|_| ConfigError::Color(value.into()))?;
    Ok(rgb(value))
}

const fn rgb(value: u32) -> Rgb {
    Rgb {
        red: ((value >> 16) & 0xff) as u8,
        green: ((value >> 8) & 0xff) as u8,
        blue: (value & 0xff) as u8,
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RawConfig {
    #[serde(default)]
    pub terminal: RawTerminal,
    #[serde(default)]
    pub font: RawFont,
    #[serde(default)]
    pub window: WindowConfig,
    #[serde(default)]
    pub theme: ThemeDefinition,
    #[serde(default)]
    pub themes: BTreeMap<String, ThemeDefinition>,
    #[serde(default)]
    pub keybinding: Vec<RawKeybinding>,
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct RawKeybinding {
    pub key: String,
    pub command: String,
    #[cfg_attr(
        feature = "schema",
        schemars(with = "Option<BTreeMap<String, serde_json::Value>>")
    )]
    pub args: Option<toml::Value>,
    pub when: Option<String>,
    pub description: Option<String>,
}

impl RawKeybinding {
    /// Validates the entry structure and preserves its diagnostic position.
    ///
    /// # Errors
    /// Returns an error when the input violates the configuration contract.
    pub fn validate(
        self,
        index: usize,
    ) -> Result<KeybindingEntry, ConfigError> {
        let diagnostic = |message: String| {
            ConfigError::Keybinding(keybinding_diagnostic(
                index, &self.key, message,
            ))
        };
        let args = match self.args {
            None => None,
            Some(toml::Value::Table(table)) => Some(table),
            Some(toml::Value::Array(_)) => {
                let names = huterm_protocol::lookup(&self.command)
                    .map(|spec| {
                        spec.args
                            .iter()
                            .map(|argument| argument.name)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let expected = if names.is_empty() {
                    "no arguments".to_owned()
                } else {
                    format!("named arguments {}", names.join(", "))
                };
                return Err(diagnostic(format!(
                    "args must be a table, not an array; `{}` takes {expected}",
                    self.command
                )));
            }
            Some(other) => {
                return Err(diagnostic(format!(
                    "args must be a table of named arguments, not {}",
                    other.type_str()
                )));
            }
        };
        if self
            .description
            .as_deref()
            .is_some_and(|description| description.trim().is_empty())
        {
            return Err(diagnostic("description must not be blank".to_owned()));
        }
        Ok(KeybindingEntry {
            key: self.key,
            command: self.command,
            args,
            when: self.when,
            description: self.description,
        })
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct RawTerminal {
    pub engine: String,
    pub links: bool,
    pub link_modifiers: LinkModifiers,
    pub close_on_exit: bool,
    pub macos_option_as_alt: MacosOptionAsAlt,
}
impl Default for RawTerminal {
    fn default() -> Self {
        Self {
            engine: "alacritty".into(),
            links: true,
            link_modifiers: LinkModifiers::default(),
            close_on_exit: TerminalConfig::default().close_on_exit,
            macos_option_as_alt: MacosOptionAsAlt::Off,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct RawFont {
    pub family: String,
    pub size: f32,
}

impl Default for RawFont {
    fn default() -> Self {
        let font = FontConfig::default();
        Self {
            family: font.family,
            size: font.size,
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Toml(toml::de::Error),
    Color(String),
    Invalid(&'static str),
    Theme(String),
    Engine(String),
    Keybinding(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toml(error) => error.fmt(formatter),
            Self::Color(value) => write!(
                formatter,
                "invalid RGB color {value:?}; expected #rrggbb"
            ),
            Self::Invalid(message) => formatter.write_str(message),
            Self::Theme(message)
            | Self::Engine(message)
            | Self::Keybinding(message) => formatter.write_str(message),
        }
    }
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: if cfg!(target_os = "macos") {
                "Menlo"
            } else {
                "monospace"
            }
            .into(),
            size: 14.0,
        }
    }
}

impl RawConfig {
    /// Checks portable numeric and text constraints.
    ///
    /// # Errors
    /// Returns an error when the input violates the configuration contract.
    pub fn validate_values(&self) -> Result<(), ConfigError> {
        if self.font.family.trim().is_empty() {
            return Err(ConfigError::Invalid("font.family must not be empty"));
        }
        if !self.font.size.is_finite()
            || !(6.0..=96.0).contains(&self.font.size)
        {
            return Err(ConfigError::Invalid(
                "font.size must be between 6 and 96",
            ));
        }
        for padding in [self.window.padding_x, self.window.padding_y] {
            if !padding.is_finite() || !(0.0..=256.0).contains(&padding) {
                return Err(ConfigError::Invalid(
                    "window padding must be between 0 and 256 points",
                ));
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct ThemeDefinition {
    pub name: Option<String>,
    pub extends: Option<String>,
    pub foreground: Option<String>,
    pub background: Option<String>,
    pub cursor: Option<String>,
    pub selection: Option<String>,
    pub selection_foreground: Option<String>,
    pub ansi: Option<Vec<String>>,
    pub ansi_black: Option<String>,
    pub ansi_red: Option<String>,
    pub ansi_green: Option<String>,
    pub ansi_yellow: Option<String>,
    pub ansi_blue: Option<String>,
    pub ansi_magenta: Option<String>,
    pub ansi_cyan: Option<String>,
    pub ansi_white: Option<String>,
    pub ansi_bright_black: Option<String>,
    pub ansi_bright_red: Option<String>,
    pub ansi_bright_green: Option<String>,
    pub ansi_bright_yellow: Option<String>,
    pub ansi_bright_blue: Option<String>,
    pub ansi_bright_magenta: Option<String>,
    pub ansi_bright_cyan: Option<String>,
    pub ansi_bright_white: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ThemeFile {
    pub theme: ThemeDefinition,
}

impl ThemeDefinition {
    /// Applies color overrides to a resolved base theme.
    ///
    /// # Errors
    /// Returns an error when the input violates the configuration contract.
    pub fn apply(&self, mut theme: Theme) -> Result<Theme, ConfigError> {
        if let Some(value) = &self.selection_foreground {
            theme.selection_foreground = Some(parse_color(value)?);
        }
        for (target, value) in [
            (&mut theme.foreground, &self.foreground),
            (&mut theme.background, &self.background),
            (&mut theme.cursor, &self.cursor),
            (&mut theme.selection, &self.selection),
        ] {
            if let Some(value) = value {
                *target = parse_color(value)?;
            }
        }
        if let Some(ansi) = &self.ansi {
            if ansi.len() != 16 {
                return Err(ConfigError::Invalid(
                    "theme.ansi must contain 16 colors",
                ));
            }
            for (target, value) in theme.ansi.iter_mut().zip(ansi) {
                *target = parse_color(value)?;
            }
        }
        for (target, value) in theme.ansi.iter_mut().zip([
            &self.ansi_black,
            &self.ansi_red,
            &self.ansi_green,
            &self.ansi_yellow,
            &self.ansi_blue,
            &self.ansi_magenta,
            &self.ansi_cyan,
            &self.ansi_white,
            &self.ansi_bright_black,
            &self.ansi_bright_red,
            &self.ansi_bright_green,
            &self.ansi_bright_yellow,
            &self.ansi_bright_blue,
            &self.ansi_bright_magenta,
            &self.ansi_bright_cyan,
            &self.ansi_bright_white,
        ]) {
            if let Some(value) = value {
                *target = parse_color(value)?;
            }
        }
        Ok(theme)
    }
}
