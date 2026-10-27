//! Portable configuration definitions and validation shared by the desktop and schemas.

use huterm_protocol::{Rgb, ScrollbackLimit};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

pub mod document;
pub mod quake;

#[cfg(feature = "schema")]
pub mod schema;
#[cfg(feature = "settings")]
pub mod settings;

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
    /// Kept as written: `false` is ordinary, but any value is an error on
    /// `unbind` and on global shortcuts.
    pub disabled: Option<bool>,
}

impl KeybindingEntry {
    /// Whether this entry removes matching earlier bindings instead of
    /// binding its key.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.disabled == Some(true)
    }
}

/// Application-wide behavior.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct GeneralConfig {
    /// Reload the configuration file automatically when it, or a theme file
    /// it uses, changes on disk.
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "general",
        "x-huterm-group" = "Configuration file",
        "x-huterm-label" = "Reload when config.toml changes",
        "x-huterm-order" = 10,
        "x-huterm-apply" = "live",
    )))]
    pub watch_config: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self { watch_config: true }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalConfig {
    pub term: TerminalIdentity,
    pub close_on_exit: bool,
    pub refresh: RefreshMode,
    pub new_tab_directory: NewTabDirectory,
    pub bell: BellConfig,
    pub clipboard_write: ClipboardWritePolicy,
    pub links: bool,
    pub link_modifiers: LinkModifiers,
    pub macos_option_as_alt: MacosOptionAsAlt,
    pub right_click: RightClickAction,
    pub scrollback: ScrollbackLimit,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            term: TerminalIdentity::Auto,
            close_on_exit: true,
            refresh: RefreshMode::default(),
            new_tab_directory: NewTabDirectory::Inherit,
            bell: BellConfig::default(),
            clipboard_write: ClipboardWritePolicy::Allow,
            links: true,
            link_modifiers: LinkModifiers::default(),
            macos_option_as_alt: MacosOptionAsAlt::Off,
            right_click: RightClickAction::Menu,
            scrollback: ScrollbackLimit::DEFAULT,
        }
    }
}

/// What a right-click on the terminal does when no application has taken
/// the mouse, or with Shift held when one has.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum RightClickAction {
    /// Open the terminal's context menu at the pointer.
    #[default]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Open the context menu"))
    )]
    Menu,
    /// Paste the clipboard.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Paste"))
    )]
    Paste,
    /// Copy and clear the selection, or paste when nothing is selected.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Copy or paste"))
    )]
    CopyOrPaste,
    /// Do nothing.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Do nothing"))
    )]
    Ignore,
}

/// Client-side terminal snapshot admission policy.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum RefreshMode {
    /// Admit one snapshot per delivered display frame, plus at most one extra
    /// snapshot for pending viewport changes.
    #[default]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Display frames"))
    )]
    Display,
    /// Admit another dirty snapshot as soon as its predecessor completes.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Unlimited"))
    )]
    Unlimited,
}

/// Working-directory policy for newly opened tabs.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum NewTabDirectory {
    /// Use a usable local directory reported by the active terminal.
    #[default]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Focused pane's directory"))
    )]
    Inherit,
    /// Always use Huterm's platform launch default.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Launch directory"))
    )]
    Default,
}

/// Terminal bell presentation settings.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct BellConfig {
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "terminal",
        "x-huterm-group" = "Bell and clipboard",
        "x-huterm-label" = "Visual bell",
        "x-huterm-order" = 70,
        "x-huterm-apply" = "live",
    )))]
    pub visual: bool,
}

impl Default for BellConfig {
    fn default() -> Self {
        Self { visual: true }
    }
}

/// Terminal identity advertised to local applications.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum TerminalIdentity {
    /// Use `xterm-huterm` when its terminfo entry is available.
    #[default]
    #[serde(rename = "auto")]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Automatic"))
    )]
    Auto,
    /// Advertise Huterm's private terminfo entry.
    #[serde(rename = "xterm-huterm")]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "xterm-huterm"))
    )]
    XtermHuterm,
    /// Use the widely installed compatibility entry.
    #[serde(rename = "xterm-256color")]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "xterm-256color"))
    )]
    Xterm256Color,
}

/// Permission for clipboard writes requested by terminal content.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum ClipboardWritePolicy {
    /// Allow terminal content to replace the system clipboard.
    #[default]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Allow"))
    )]
    Allow,
    /// Drop terminal-content clipboard writes.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Deny"))
    )]
    Deny,
}

impl ClipboardWritePolicy {
    /// Returns whether terminal-content clipboard writes are allowed.
    #[must_use]
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allow)
    }
}

/// A size in bytes: a byte count, or a string of digits followed,
/// optionally after one space, by a unit. B, KB, MB, and GB count in powers
/// of 1000; KiB, MiB, and GiB in powers of 1024. Units ignore case, so `b`
/// also means bytes. Sizes above [`ScrollbackLimit::MAX_BYTES`] are invalid.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ScrollbackSize(u64);

impl ScrollbackSize {
    /// What a valid size looks like, for error messages.
    const EXPECTED: &'static str =
        "a byte count or a size such as \"64MiB\" or \"50MB\", at most 16GiB";

    /// The size in bytes.
    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.0
    }

    /// Parses a size string such as `"64MiB"`.
    ///
    /// # Errors
    /// Returns an error when the text is not a size or exceeds 16 GiB.
    pub fn parse(text: &str) -> Result<Self, String> {
        let invalid =
            || format!("invalid size {text:?}; expected {}", Self::EXPECTED);
        let digits = text.bytes().take_while(u8::is_ascii_digit).count();
        let (number, unit) = text.split_at(digits);
        let unit = unit
            .strip_prefix(' ')
            .filter(|unit| !unit.is_empty())
            .unwrap_or(unit);
        let scale: u64 = match unit.to_ascii_lowercase().as_str() {
            "" | "b" => 1,
            "kb" => 1000,
            "mb" => 1000 * 1000,
            "gb" => 1000 * 1000 * 1000,
            "kib" => 1 << 10,
            "mib" => 1 << 20,
            "gib" => 1 << 30,
            _ => return Err(invalid()),
        };
        number
            .parse::<u64>()
            .ok()
            .and_then(|number| number.checked_mul(scale))
            .and_then(Self::checked)
            .ok_or_else(invalid)
    }

    const fn checked(bytes: u64) -> Option<Self> {
        if bytes > ScrollbackLimit::MAX_BYTES {
            None
        } else {
            Some(Self(bytes))
        }
    }
}

impl<'de> Deserialize<'de> for ScrollbackSize {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = ScrollbackSize;
            fn expecting(
                &self,
                formatter: &mut fmt::Formatter<'_>,
            ) -> fmt::Result {
                formatter.write_str(ScrollbackSize::EXPECTED)
            }
            fn visit_u64<E: serde::de::Error>(
                self,
                bytes: u64,
            ) -> Result<Self::Value, E> {
                ScrollbackSize::checked(bytes).ok_or_else(|| {
                    E::custom(format!(
                        "size {bytes} is too large; expected {}",
                        ScrollbackSize::EXPECTED
                    ))
                })
            }
            fn visit_i64<E: serde::de::Error>(
                self,
                bytes: i64,
            ) -> Result<Self::Value, E> {
                u64::try_from(bytes).map_or_else(
                    |_| {
                        Err(E::custom(format!(
                            "size {bytes} is negative; expected {}",
                            ScrollbackSize::EXPECTED
                        )))
                    },
                    |bytes| self.visit_u64(bytes),
                )
            }
            fn visit_str<E: serde::de::Error>(
                self,
                text: &str,
            ) -> Result<Self::Value, E> {
                ScrollbackSize::parse(text).map_err(E::custom)
            }
        }
        deserializer.deserialize_any(Visitor)
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
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Off"))
    )]
    Off,
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Both Option keys"))
    )]
    Both,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "window",
        "x-huterm-group" = "Window",
        "x-huterm-label" = "Fullscreen mode",
        "x-huterm-order" = 30,
        "x-huterm-apply" = "later_events",
        "x-huterm-platform" = "macos",
    )))]
    pub macos_fullscreen_mode: MacosFullscreenMode,
    #[cfg_attr(feature = "schema", schemars(range(min = 0, max = 256), extend(
        "x-huterm-section" = "appearance",
        "x-huterm-group" = "Padding",
        "x-huterm-label" = "Horizontal padding",
        "x-huterm-order" = 40,
        "x-huterm-apply" = "live",
        "x-huterm-unit" = "pt",
    )))]
    pub padding_x: f32,
    #[cfg_attr(feature = "schema", schemars(range(min = 0, max = 256), extend(
        "x-huterm-section" = "appearance",
        "x-huterm-group" = "Padding",
        "x-huterm-label" = "Vertical padding",
        "x-huterm-order" = 50,
        "x-huterm-apply" = "live",
        "x-huterm-unit" = "pt",
    )))]
    pub padding_y: f32,
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "appearance",
        "x-huterm-group" = "Padding",
        "x-huterm-label" = "Balance horizontal padding",
        "x-huterm-order" = 60,
        "x-huterm-apply" = "live",
    )))]
    pub padding_balance: bool,
    /// Show the window menu button in the title bar, or in the tab bar
    /// when the window has no title bar. Every item stays reachable through
    /// shortcuts or the command palette.
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "window",
        "x-huterm-group" = "Window",
        "x-huterm-label" = "Menu button",
        "x-huterm-order" = 10,
        "x-huterm-apply" = "live",
    )))]
    pub menu_button: bool,
    /// Show key hints on the scroll pill and on dialog and panel buttons.
    /// The command palette and menus always show their shortcuts.
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "window",
        "x-huterm-group" = "Window",
        "x-huterm-label" = "Shortcut hints",
        "x-huterm-order" = 20,
        "x-huterm-apply" = "live",
    )))]
    pub shortcut_hints: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct TabsConfig {
    /// Where tabs sit: top, bottom, left, right, or titlebar. Defaults to
    /// titlebar on macOS and top on Linux.
    #[cfg_attr(feature = "schema", schemars(transform = schema::without_default, extend(
        "x-huterm-section" = "tabs",
        "x-huterm-group" = "Tab bar",
        "x-huterm-label" = "Position",
        "x-huterm-order" = 10,
        "x-huterm-apply" = "live",
    )))]
    pub position: TabPosition,
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "tabs",
        "x-huterm-group" = "Visibility",
        "x-huterm-label" = "Show the tab bar with one tab",
        "x-huterm-order" = 50,
        "x-huterm-apply" = "live",
    )))]
    pub always_show: bool,
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "tabs",
        "x-huterm-group" = "Visibility",
        "x-huterm-label" = "Auto-hide in fullscreen",
        "x-huterm-order" = 60,
        "x-huterm-apply" = "live",
    )))]
    pub auto_hide_in_fullscreen: bool,
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "tabs",
        "x-huterm-group" = "Tab bar",
        "x-huterm-label" = "Style",
        "x-huterm-order" = 20,
        "x-huterm-apply" = "live",
    )))]
    pub style: TabStyle,
    /// Draws the accent bar inside the active pill; ignored by other styles.
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "tabs",
        "x-huterm-group" = "Tab bar",
        "x-huterm-label" = "Accent bar in the active pill",
        "x-huterm-order" = 30,
        "x-huterm-apply" = "live",
        "x-huterm-visible-when" = {"setting": "tabs.style", "equals": "pill"},
    )))]
    pub pill_accent: bool,
    /// When a tab shows its close button; hovering a tab always shows it.
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "tabs",
        "x-huterm-group" = "Tab bar",
        "x-huterm-label" = "Close buttons",
        "x-huterm-order" = 40,
        "x-huterm-apply" = "live",
    )))]
    pub close_button: TabCloseButton,
    /// Put a top bar beside a display notch in non-native fullscreen.
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "tabs",
        "x-huterm-group" = "Visibility",
        "x-huterm-label" = "Beside the display notch",
        "x-huterm-order" = 70,
        "x-huterm-apply" = "live",
        "x-huterm-platform" = "macos",
        "x-huterm-advanced" = true,
    )))]
    pub notch: TabNotch,
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "tabs",
        "x-huterm-group" = "Sizing",
        "x-huterm-label" = "Tab width",
        "x-huterm-order" = 100,
        "x-huterm-apply" = "live",
    )))]
    pub width: TabWidth,
    #[cfg_attr(feature = "schema", schemars(range(min = 48, max = 600), extend(
        "x-huterm-section" = "tabs",
        "x-huterm-group" = "Sizing",
        "x-huterm-label" = "Minimum width",
        "x-huterm-order" = 110,
        "x-huterm-apply" = "live",
        "x-huterm-unit" = "pt",
        "x-huterm-visible-when" = {"setting": "tabs.width", "equals": "fit"},
    )))]
    pub min_width: f32,
    #[cfg_attr(feature = "schema", schemars(range(min = 48, max = 600), extend(
        "x-huterm-section" = "tabs",
        "x-huterm-group" = "Sizing",
        "x-huterm-label" = "Maximum width",
        "x-huterm-order" = 120,
        "x-huterm-apply" = "live",
        "x-huterm-unit" = "pt",
        "x-huterm-visible-when" = {"setting": "tabs.width", "equals": "fit"},
    )))]
    pub max_width: f32,
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "tabs",
        "x-huterm-group" = "Labels",
        "x-huterm-label" = "Tab label",
        "x-huterm-order" = 80,
        "x-huterm-apply" = "live",
    )))]
    pub label: TabLabel,
    /// How labels show a directory.
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "tabs",
        "x-huterm-group" = "Labels",
        "x-huterm-label" = "Directory format",
        "x-huterm-order" = 90,
        "x-huterm-apply" = "live",
    )))]
    pub directory: TabDirectory,
}

/// Optional overrides for the platform updater.
///
/// Leaving either field unset preserves the updater's stored preference. On a
/// fresh macOS profile, Sparkle asks about automatic checks on the second
/// launch and uses a 24-hour interval.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct UpdateConfig {
    /// Enable or disable scheduled update checks without Sparkle's consent
    /// prompt. Omit to preserve Sparkle's stored choice; a fresh macOS
    /// profile asks on its second launch.
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "updates",
        "x-huterm-group" = "Updates",
        "x-huterm-label" = "Check automatically",
        "x-huterm-order" = 10,
        "x-huterm-apply" = "live",
        "x-huterm-platform" = "macos",
    )))]
    pub automatic_checks: Option<bool>,
    /// Scheduled update-check interval in whole hours. Omit to preserve
    /// Sparkle's stored interval; a fresh profile uses 24 hours.
    #[cfg_attr(feature = "schema", schemars(range(min = 1), extend(
        "x-huterm-section" = "updates",
        "x-huterm-group" = "Updates",
        "x-huterm-label" = "Check interval",
        "x-huterm-order" = 20,
        "x-huterm-apply" = "live",
        "x-huterm-unit" = "h",
        "x-huterm-platform" = "macos",
    )))]
    pub check_interval_hours: Option<u32>,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            macos_fullscreen_mode: MacosFullscreenMode::NonNative,
            padding_x: 4.0,
            padding_y: 4.0,
            padding_balance: false,
            menu_button: true,
            shortcut_hints: false,
        }
    }
}

impl Default for TabsConfig {
    fn default() -> Self {
        Self {
            // The macOS title bar holds the tabs; Linux keeps the window
            // manager's title bar unless configured otherwise.
            position: if cfg!(target_os = "macos") {
                TabPosition::Titlebar
            } else {
                TabPosition::Top
            },
            always_show: false,
            auto_hide_in_fullscreen: false,
            style: TabStyle::Pill,
            pill_accent: false,
            close_button: TabCloseButton::Active,
            notch: TabNotch::Left,
            width: TabWidth::Fit,
            min_width: 96.0,
            max_width: 240.0,
            label: TabLabel::Smart,
            directory: TabDirectory::Name,
        }
    }
}

/// Metadata used to resolve automatic tab labels. A custom tab name always
/// wins; each mode falls back to the terminal title, then the launched
/// program.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TabLabel {
    /// A running program's own title, else its name; at an idle shell, the
    /// directory.
    #[default]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Smart"))
    )]
    Smart,
    /// The title set by the shell or program.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Title"))
    )]
    Title,
    /// The foreground program's name.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Process"))
    )]
    Process,
    /// The current directory.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Directory"))
    )]
    Directory,
    /// The foreground program's name and the current directory.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Process and directory"))
    )]
    ProcessAndDirectory,
}

/// How tab labels show a directory. The local home directory is always `~`.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TabDirectory {
    /// The last path component, such as `huterm`.
    #[default]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Name"))
    )]
    Name,
    /// The full path, relative to home when under it, such as
    /// `~/Projects/huterm`.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Path"))
    )]
    Path,
    /// The path with parent components shortened to their first character,
    /// such as `~/P/huterm`.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Short"))
    )]
    Short,
}

/// Who owns a pane's zoom.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum PaneZoomScope {
    /// Each window zooms panes on its own.
    #[default]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Each window"))
    )]
    View,
    /// The tab owns the zoom: every window set to `shared` shows the same
    /// zoomed pane, and windows set to `view` ignore it.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Shared"))
    )]
    Shared,
}

/// Split pane behavior.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct PanesConfig {
    /// Who owns a pane's zoom: `view` or `shared`.
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "window",
        "x-huterm-group" = "Panes",
        "x-huterm-label" = "Pane zoom",
        "x-huterm-order" = 40,
        "x-huterm-apply" = "live",
    )))]
    pub zoom: PaneZoomScope,
}

/// Where the command palette sits within its window.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum PalettePlacement {
    /// Anchored near the top edge of the window.
    #[default]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Top"))
    )]
    Top,
    /// Centred on the window at the palette's full height, so filtering
    /// results never moves the input line.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Center"))
    )]
    Center,
}

/// Command palette behavior.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct PaletteConfig {
    /// Where the palette sits within the window: `top` or `center`.
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "palette",
        "x-huterm-group" = "Palette",
        "x-huterm-label" = "Position",
        "x-huterm-order" = 10,
        "x-huterm-apply" = "live",
    )))]
    pub placement: PalettePlacement,
    /// Keep the search text after the palette is cancelled, so reopening
    /// within `retain_query_seconds` restores it selected.
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "palette",
        "x-huterm-group" = "Palette",
        "x-huterm-label" = "Keep the search after cancelling",
        "x-huterm-order" = 20,
        "x-huterm-apply" = "next_palette_open",
    )))]
    pub retain_query: bool,
    /// How long a cancelled palette's search text is retained, 0 to 3600.
    #[cfg_attr(feature = "schema", schemars(range(max = 3600), extend(
        "x-huterm-section" = "palette",
        "x-huterm-group" = "Palette",
        "x-huterm-label" = "Keep it for",
        "x-huterm-order" = 30,
        "x-huterm-apply" = "next_palette_open",
        "x-huterm-unit" = "s",
        "x-huterm-visible-when" = {"setting": "palette.retain_query", "equals": true},
    )))]
    pub retain_query_seconds: u32,
}

impl Default for PaletteConfig {
    fn default() -> Self {
        Self {
            placement: PalettePlacement::Top,
            retain_query: true,
            retain_query_seconds: 15,
        }
    }
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum MacosFullscreenMode {
    #[serde(rename = "native")]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Separate Space"))
    )]
    Native,
    #[default]
    #[serde(rename = "non_native")]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Current Space"))
    )]
    NonNative,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum TabPosition {
    #[default]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Top"))
    )]
    Top,
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Bottom"))
    )]
    Bottom,
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Left"))
    )]
    Left,
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Right"))
    )]
    Right,
    /// Tabs share the window's title-bar row. Where no title bar exists
    /// (fullscreen, Quake windows, or a window manager drawing the title
    /// bar) the tabs take a top bar instead.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Title bar"))
    )]
    Titlebar,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum TabStyle {
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Strip"))
    )]
    Strip,
    #[default]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Pill"))
    )]
    Pill,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum TabWidth {
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Fill the bar"))
    )]
    Fill,
    #[default]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Fit titles"))
    )]
    Fit,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum TabCloseButton {
    /// Only while the pointer is over the tab.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Hovered tab only"))
    )]
    Hover,
    /// On the active tab, and on any hovered tab.
    #[default]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Active and hovered tabs"))
    )]
    Active,
    /// On every tab.
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Every tab"))
    )]
    Always,
}

/// Where a top tab bar goes on a notched display in non-native fullscreen:
/// below the notch across the window, or beside the camera housing on one
/// side, giving that height back to the terminal.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum TabNotch {
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Off"))
    )]
    Off,
    #[default]
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Left"))
    )]
    Left,
    #[cfg_attr(
        feature = "schema",
        schemars(extend("x-huterm-label" = "Right"))
    )]
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
    pub tab_bar_background: Option<Rgb>,
    pub tab_active_background: Option<Rgb>,
    pub tab_foreground: Option<Rgb>,
    pub tab_inactive_foreground: Option<Rgb>,
    pub tab_border: Option<Rgb>,
    pub tab_accent: Option<Rgb>,
    pub tab_hover_background: Option<Rgba>,
    pub scrollbar_thumb: Option<Rgba>,
    pub scrollbar_track: Option<Rgba>,
}

/// A color with straight alpha, for overlays drawn over varying surfaces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgba {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl Rgba {
    #[must_use]
    pub const fn opaque(rgb: Rgb) -> Self {
        Self::with_alpha(rgb, 0xff)
    }

    #[must_use]
    pub const fn with_alpha(rgb: Rgb, alpha: u8) -> Self {
        Self {
            red: rgb.red,
            green: rgb.green,
            blue: rgb.blue,
            alpha,
        }
    }
}

/// Window chrome colors with every unset theme value derived.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiColors {
    pub tab_bar_background: Rgb,
    pub tab_active_background: Rgb,
    pub tab_foreground: Rgb,
    pub tab_inactive_foreground: Rgb,
    pub tab_border: Rgb,
    pub tab_accent: Rgb,
    /// Overlay on a hovered tab or chrome control.
    pub tab_hover_background: Rgba,
    /// Overlay scrollbar thumb, over the terminal, lists, and tab bars.
    pub scrollbar_thumb: Rgba,
    /// Track behind an expanded scrollbar thumb.
    pub scrollbar_track: Rgba,
}

const BLACK: Rgb = rgb(0x0000_0000);
const WHITE: Rgb = rgb(0x00ff_ffff);

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the rounded value is clamped to the u8 range"
)]
fn mix(from: Rgb, to: Rgb, amount: f32) -> Rgb {
    let channel = |from: u8, to: u8| {
        let from = f32::from(from);
        (from + (f32::from(to) - from) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Rgb {
        red: channel(from.red, to.red),
        green: channel(from.green, to.green),
        blue: channel(from.blue, to.blue),
    }
}

fn luminance(color: Rgb) -> f32 {
    (0.2126 * f32::from(color.red)
        + 0.7152 * f32::from(color.green)
        + 0.0722 * f32::from(color.blue))
        / 255.0
}

impl Theme {
    /// The built-in `huterm-dark` theme: the default colors plus hand-picked
    /// tab chrome. `Default` leaves the chrome unset so a theme built on it
    /// derives chrome from its own colors.
    #[must_use]
    pub fn huterm_dark() -> Self {
        Self {
            tab_bar_background: Some(rgb(0x0016_1719)),
            tab_active_background: Some(rgb(0x0028_2a2e)),
            tab_foreground: Some(rgb(0x00c5_c8c6)),
            tab_inactive_foreground: Some(rgb(0x0096_9896)),
            tab_border: Some(rgb(0x002a_2c30)),
            tab_accent: Some(rgb(0x0081_a2be)),
            ..Self::default()
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            foreground: rgb(0x00c5_c8c6),
            background: rgb(0x001d_1f21),
            cursor: rgb(0x00ff_ffff),
            selection: rgb(0x0026_4f78),
            selection_foreground: None,
            tab_bar_background: None,
            tab_active_background: None,
            tab_foreground: None,
            tab_inactive_foreground: None,
            tab_border: None,
            tab_accent: None,
            tab_hover_background: None,
            scrollbar_thumb: None,
            scrollbar_track: None,
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
    /// Resolves chrome colors, deriving any value the theme does not set.
    #[must_use]
    pub fn ui(&self) -> UiColors {
        let background = self.background;
        let light = luminance(background) > 0.5;
        let bar = self.tab_bar_background.unwrap_or_else(|| {
            if light {
                mix(background, BLACK, 0.07)
            } else if luminance(background) < 0.04 {
                // A near-black background cannot get darker.
                mix(background, self.foreground, 0.08)
            } else {
                mix(background, BLACK, 0.18)
            }
        });
        UiColors {
            tab_bar_background: bar,
            tab_active_background: self.tab_active_background.unwrap_or_else(
                || {
                    if light {
                        mix(background, WHITE, 0.6)
                    } else {
                        mix(background, self.foreground, 0.1)
                    }
                },
            ),
            tab_foreground: self.tab_foreground.unwrap_or(self.foreground),
            tab_inactive_foreground: self
                .tab_inactive_foreground
                .unwrap_or_else(|| mix(self.foreground, bar, 0.45)),
            tab_border: self
                .tab_border
                .unwrap_or_else(|| mix(bar, self.foreground, 0.1)),
            tab_accent: self.tab_accent.unwrap_or(self.ansi[4]),
            tab_hover_background: self
                .tab_hover_background
                .unwrap_or(Rgba::with_alpha(self.foreground, 0x0a)),
            scrollbar_thumb: self
                .scrollbar_thumb
                .unwrap_or(Rgba::with_alpha(self.foreground, 0xbb)),
            scrollbar_track: self
                .scrollbar_track
                .unwrap_or(Rgba::with_alpha(self.foreground, 0x14)),
        }
    }

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

/// Parses a color with optional alpha: `#rrggbb` or `#rrggbbaa`.
///
/// # Errors
/// Returns an error for any other form.
pub fn parse_color_alpha(value: &str) -> Result<Rgba, ConfigError> {
    let Some(hex) = value.strip_prefix('#') else {
        return Err(ConfigError::ColorAlpha(value.into()));
    };
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ConfigError::ColorAlpha(value.into()));
    }
    let (rgb, alpha) = match hex.len() {
        6 => (hex, "ff"),
        8 => hex.split_at(6),
        _ => return Err(ConfigError::ColorAlpha(value.into())),
    };
    let rgb = u32::from_str_radix(rgb, 16)
        .map_err(|_| ConfigError::ColorAlpha(value.into()))?;
    let alpha = u8::from_str_radix(alpha, 16)
        .map_err(|_| ConfigError::ColorAlpha(value.into()))?;
    Ok(Rgba::with_alpha(self::rgb(rgb), alpha))
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
    pub general: GeneralConfig,
    #[serde(default)]
    pub terminal: RawTerminal,
    #[serde(default)]
    pub font: RawFont,
    #[serde(default)]
    pub window: WindowConfig,
    #[serde(default)]
    pub tabs: TabsConfig,
    #[serde(default)]
    pub updates: UpdateConfig,
    #[serde(default)]
    pub palette: PaletteConfig,
    #[serde(default)]
    pub panes: PanesConfig,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "appearance",
        "x-huterm-group" = "Theme",
        "x-huterm-label" = "Theme",
        "x-huterm-order" = 30,
        "x-huterm-apply" = "live",
    )))]
    pub theme: ThemeDefinition,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "appearance",
        "x-huterm-group" = "Theme",
        "x-huterm-label" = "Custom themes",
        "x-huterm-order" = 35,
        "x-huterm-apply" = "live",
        "propertyNames" = {"pattern": "^[A-Za-z0-9_-]+$"},
    )))]
    pub themes: BTreeMap<String, ThemeDefinition>,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "keybindings",
        "x-huterm-group" = "Keybindings",
        "x-huterm-label" = "Keybindings",
        "x-huterm-order" = 10,
        "x-huterm-apply" = "live",
    )))]
    pub keybinding: Vec<RawKeybinding>,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "quake",
        "x-huterm-group" = "Profiles",
        "x-huterm-label" = "Quake profiles",
        "x-huterm-order" = 10,
        "x-huterm-apply" = "live",
    )))]
    pub quake: quake::Config,
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "quake",
        "x-huterm-group" = "Global shortcuts",
        "x-huterm-label" = "Global shortcuts",
        "x-huterm-order" = 20,
        "x-huterm-apply" = "live",
    )))]
    pub global_keybinding: Vec<RawKeybinding>,
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
    /// Remove earlier bindings with the same key, command, `when`, and any
    /// given `args`, and bind nothing.
    pub disabled: Option<bool>,
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
            disabled: self.disabled,
        })
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct RawTerminal {
    /// Deprecated compatibility setting. Both legacy values use Ghostty;
    /// remove this field.
    #[cfg_attr(feature = "schema", schemars(with = "LegacyTerminalEngine", extend(
        "x-huterm-section" = "terminal",
        "x-huterm-group" = "Compatibility",
        "x-huterm-label" = "Engine",
        "x-huterm-order" = 110,
        "x-huterm-apply" = "new_terminals",
        "x-huterm-advanced" = true,
        "deprecated" = true,
    )))]
    pub engine: Option<LegacyTerminalEngine>,
    /// Terminal identity for newly spawned shells.
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "terminal",
        "x-huterm-group" = "Compatibility",
        "x-huterm-label" = "TERM",
        "x-huterm-order" = 90,
        "x-huterm-apply" = "new_terminals",
        "x-huterm-advanced" = true,
    )))]
    pub term: TerminalIdentity,
    /// Allow or deny system clipboard writes requested by terminal content.
    /// Reload applies to existing terminals; explicit Copy and Paste commands
    /// are unaffected.
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "terminal",
        "x-huterm-group" = "Bell and clipboard",
        "x-huterm-label" = "Clipboard writes from programs",
        "x-huterm-order" = 80,
        "x-huterm-apply" = "live",
    )))]
    pub clipboard_write: ClipboardWritePolicy,
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "terminal",
        "x-huterm-group" = "Mouse",
        "x-huterm-label" = "Clickable links",
        "x-huterm-order" = 50,
        "x-huterm-apply" = "live",
    )))]
    pub links: bool,
    #[cfg_attr(feature = "schema", schemars(transform = schema::without_default, extend(
        "x-huterm-section" = "terminal",
        "x-huterm-group" = "Mouse",
        "x-huterm-label" = "Link modifier",
        "x-huterm-order" = 60,
        "x-huterm-apply" = "live",
        "x-huterm-visible-when" = {"setting": "terminal.links", "equals": true},
    )))]
    pub link_modifiers: LinkModifiers,
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "terminal",
        "x-huterm-group" = "Shell",
        "x-huterm-label" = "Close a pane when its shell exits",
        "x-huterm-order" = 20,
        "x-huterm-apply" = "later_events",
    )))]
    pub close_on_exit: bool,
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "terminal",
        "x-huterm-group" = "Compatibility",
        "x-huterm-label" = "Snapshot refresh",
        "x-huterm-order" = 100,
        "x-huterm-apply" = "live",
        "x-huterm-advanced" = true,
    )))]
    pub refresh: RefreshMode,
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "terminal",
        "x-huterm-group" = "Shell",
        "x-huterm-label" = "New tabs start in",
        "x-huterm-order" = 10,
        "x-huterm-apply" = "new_tabs",
    )))]
    pub new_tab_directory: NewTabDirectory,
    pub bell: BellConfig,
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "terminal",
        "x-huterm-group" = "Keyboard",
        "x-huterm-label" = "Option as Meta",
        "x-huterm-order" = 30,
        "x-huterm-apply" = "later_events",
        "x-huterm-platform" = "macos",
    )))]
    pub macos_option_as_alt: MacosOptionAsAlt,
    /// What a right-click on the terminal does: open the context menu,
    /// paste, copy the selection or else paste, or nothing. While an
    /// application takes the mouse, Shift+right-click applies it.
    #[cfg_attr(feature = "schema", schemars(extend(
        "x-huterm-section" = "terminal",
        "x-huterm-group" = "Mouse",
        "x-huterm-label" = "Right-click",
        "x-huterm-order" = 40,
        "x-huterm-apply" = "live",
    )))]
    pub right_click: RightClickAction,
    /// Live history each terminal keeps above its screen, in screen rows: a
    /// line that wraps counts once for each row it fills. 0 keeps none,
    /// whatever `scrollback_bytes` says. The oldest history is freed a page
    /// at a time, about 570 rows at 80 columns, so a terminal can hold up to
    /// a page fewer, and a smaller nonzero limit keeps up to one page. It
    /// counts rows; `scrollback_bytes` limits size. Reload applies it to open
    /// terminals; lowering it discards the oldest rows at once.
    #[cfg_attr(feature = "schema", schemars(range(max = ScrollbackLimit::MAX.rows()), extend(
        "x-huterm-section" = "scrollback",
        "x-huterm-group" = "Limits",
        "x-huterm-label" = "Scrollback rows",
        "x-huterm-order" = 10,
        "x-huterm-apply" = "live",
        "x-huterm-unit" = "rows",
    )))]
    pub scrollback_rows: u32,
    /// Optional size limit on each terminal's screen and history: a byte count,
    /// or digits and a unit such as "64MiB" or "50MB" (B, KB, MB, GB in powers
    /// of 1000; KiB, MiB, GiB in powers of 1024; units ignore case, so b also
    /// means bytes), at most 16GiB. 0 keeps no history. Whichever of it and
    /// `scrollback_rows` is reached first applies. Ghostty frees history a page
    /// at a time and always allows the visible screen rounded up to whole pages
    /// plus one more, so a tiny value keeps up to about two pages. Reload
    /// applies it to open terminals.
    #[cfg_attr(feature = "schema", schemars(with = "ScrollbackSize", extend(
        "x-huterm-section" = "scrollback",
        "x-huterm-group" = "Limits",
        "x-huterm-label" = "Size limit",
        "x-huterm-order" = 20,
        "x-huterm-apply" = "live",
        "x-huterm-advanced" = true,
    )))]
    pub scrollback_bytes: Option<ScrollbackSize>,
}
impl Default for RawTerminal {
    fn default() -> Self {
        Self {
            engine: None,
            term: TerminalIdentity::Auto,
            clipboard_write: ClipboardWritePolicy::Allow,
            links: true,
            link_modifiers: LinkModifiers::default(),
            close_on_exit: TerminalConfig::default().close_on_exit,
            refresh: RefreshMode::default(),
            new_tab_directory: NewTabDirectory::Inherit,
            bell: BellConfig::default(),
            macos_option_as_alt: MacosOptionAsAlt::Off,
            right_click: RightClickAction::Menu,
            scrollback_rows: ScrollbackLimit::DEFAULT.rows(),
            scrollback_bytes: None,
        }
    }
}

impl RawTerminal {
    /// The validated `scrollback_rows` limit, with `scrollback_bytes` when
    /// it is set.
    ///
    /// # Errors
    /// Returns an error when the row limit exceeds [`ScrollbackLimit::MAX`].
    pub fn scrollback(&self) -> Result<ScrollbackLimit, ConfigError> {
        let rows = ScrollbackLimit::new(self.scrollback_rows).ok_or(
            ConfigError::Invalid(
                "terminal.scrollback_rows must be between 0 and 50000",
            ),
        )?;
        match self.scrollback_bytes {
            None => Ok(rows),
            Some(size) => {
                rows.with_bytes(size.bytes()).ok_or(ConfigError::Invalid(
                    "terminal.scrollback_bytes must be at most 16GiB",
                ))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(inline))]
#[serde(rename_all = "lowercase")]
pub enum LegacyTerminalEngine {
    Alacritty,
    Ghostty,
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct RawFont {
    /// Font family. Defaults to Menlo on macOS and monospace on Linux.
    #[cfg_attr(feature = "schema", schemars(regex(pattern = r"\S"), transform = schema::without_default, extend(
        "x-huterm-section" = "appearance",
        "x-huterm-group" = "Font",
        "x-huterm-label" = "Font family",
        "x-huterm-order" = 10,
        "x-huterm-apply" = "live",
    )))]
    pub family: String,
    #[cfg_attr(feature = "schema", schemars(range(min = 6, max = 96), extend(
        "x-huterm-section" = "appearance",
        "x-huterm-group" = "Font",
        "x-huterm-label" = "Font size",
        "x-huterm-order" = 20,
        "x-huterm-apply" = "live",
        "x-huterm-unit" = "pt",
    )))]
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
    /// A color that may carry alpha was malformed.
    ColorAlpha(String),
    Invalid(&'static str),
    Theme(String),
    Engine(String),
    Keybinding(String),
    Quake(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toml(error) => error.fmt(formatter),
            Self::Color(value) => write!(
                formatter,
                "invalid RGB color {value:?}; expected #rrggbb"
            ),
            Self::ColorAlpha(value) => write!(
                formatter,
                "invalid color {value:?}; expected #rrggbb or #rrggbbaa"
            ),
            Self::Invalid(message) => formatter.write_str(message),
            Self::Theme(message)
            | Self::Engine(message)
            | Self::Keybinding(message)
            | Self::Quake(message) => formatter.write_str(message),
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
        self.quake.validate().map_err(ConfigError::Quake)?;
        if self.palette.retain_query_seconds > 3600 {
            return Err(ConfigError::Invalid(
                "palette.retain_query_seconds must be between 0 and 3600",
            ));
        }
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
        if !self.tabs.min_width.is_finite() || self.tabs.min_width < 48.0 {
            return Err(ConfigError::Invalid(
                "tabs.min_width must be at least 48 points",
            ));
        }
        if !self.tabs.max_width.is_finite() || self.tabs.max_width > 600.0 {
            return Err(ConfigError::Invalid(
                "tabs.max_width must be at most 600 points",
            ));
        }
        if self.tabs.min_width > self.tabs.max_width {
            return Err(ConfigError::Invalid(
                "tabs.min_width must not exceed tabs.max_width",
            ));
        }
        self.terminal.scrollback()?;
        if self.updates.check_interval_hours == Some(0) {
            return Err(ConfigError::Invalid(
                "updates.check_interval_hours must be at least 1",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(transform = schema::constrain_theme))]
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
    pub tab_bar_background: Option<String>,
    pub tab_active_background: Option<String>,
    pub tab_foreground: Option<String>,
    pub tab_inactive_foreground: Option<String>,
    pub tab_border: Option<String>,
    pub tab_accent: Option<String>,
    pub tab_hover_background: Option<String>,
    pub scrollbar_thumb: Option<String>,
    pub scrollbar_track: Option<String>,
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
            (&mut theme.tab_bar_background, &self.tab_bar_background),
            (
                &mut theme.tab_active_background,
                &self.tab_active_background,
            ),
            (&mut theme.tab_foreground, &self.tab_foreground),
            (
                &mut theme.tab_inactive_foreground,
                &self.tab_inactive_foreground,
            ),
            (&mut theme.tab_border, &self.tab_border),
            (&mut theme.tab_accent, &self.tab_accent),
        ] {
            if let Some(value) = value {
                *target = Some(parse_color(value)?);
            }
        }
        for (target, value) in [
            (&mut theme.tab_hover_background, &self.tab_hover_background),
            (&mut theme.scrollbar_thumb, &self.scrollbar_thumb),
            (&mut theme.scrollbar_track, &self.scrollbar_track),
        ] {
            if let Some(value) = value {
                *target = Some(parse_color_alpha(value)?);
            }
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

#[cfg(test)]
mod update_tests {
    use super::*;

    #[test]
    fn update_settings_preserve_omitted_and_explicit_states() {
        let omitted: RawConfig = toml::from_str("").expect("default config");
        assert_eq!(omitted.updates.automatic_checks, None);
        assert_eq!(omitted.updates.check_interval_hours, None);

        for enabled in [false, true] {
            let configured: RawConfig = toml::from_str(&format!(
                "[updates]\nautomatic_checks = {enabled}\ncheck_interval_hours = 6"
            ))
            .expect("explicit updates");
            assert_eq!(configured.updates.automatic_checks, Some(enabled));
            assert_eq!(configured.updates.check_interval_hours, Some(6));
            configured.validate_values().expect("valid updates");
        }
    }

    #[test]
    fn update_interval_requires_at_least_one_hour() {
        let configured: RawConfig =
            toml::from_str("[updates]\ncheck_interval_hours = 0")
                .expect("syntactically valid updates");
        assert_eq!(
            configured
                .validate_values()
                .expect_err("zero interval must fail")
                .to_string(),
            "updates.check_interval_hours must be at least 1"
        );
    }
}

#[cfg(test)]
mod theme_ui_tests {
    use super::*;

    fn unset(background: u32, foreground: u32) -> Theme {
        Theme {
            background: rgb(background),
            foreground: rgb(foreground),
            tab_bar_background: None,
            tab_active_background: None,
            tab_foreground: None,
            tab_inactive_foreground: None,
            tab_border: None,
            tab_accent: None,
            tab_hover_background: None,
            scrollbar_thumb: None,
            scrollbar_track: None,
            ..Theme::default()
        }
    }

    #[test]
    fn dark_themes_derive_a_darker_bar_and_a_lighter_active_tab() {
        let theme = unset(0x001a_1b26, 0x00c0_caf5);
        let ui = theme.ui();
        assert!(luminance(ui.tab_bar_background) < luminance(theme.background));
        assert!(
            luminance(ui.tab_active_background) > luminance(theme.background)
        );
        let inactive = luminance(ui.tab_inactive_foreground);
        assert!(inactive > luminance(ui.tab_bar_background));
        assert!(inactive < luminance(theme.foreground));
        assert_ne!(ui.tab_border, ui.tab_bar_background);
        assert_eq!(ui.tab_foreground, theme.foreground);
        assert_eq!(ui.tab_accent, theme.ansi[4]);
        // Overlays derive from the foreground at fixed alphas.
        assert_eq!(
            ui.tab_hover_background,
            Rgba::with_alpha(theme.foreground, 0x0a)
        );
        assert_eq!(
            ui.scrollbar_thumb,
            Rgba::with_alpha(theme.foreground, 0xbb)
        );
        assert_eq!(
            ui.scrollbar_track,
            Rgba::with_alpha(theme.foreground, 0x14)
        );
    }

    #[test]
    fn light_and_black_themes_keep_the_bar_distinct_from_the_terminal() {
        let light = unset(0x00ef_f1f5, 0x004c_4f69);
        let ui = light.ui();
        assert!(luminance(ui.tab_bar_background) < luminance(light.background));
        assert!(
            luminance(ui.tab_active_background) > luminance(light.background)
        );
        let black = unset(0x0000_0000, 0x00ff_ffff);
        assert!(
            luminance(black.ui().tab_bar_background)
                > luminance(black.background)
        );
    }

    #[test]
    fn explicit_colors_win_and_derived_colors_follow_an_explicit_bar() {
        let mut theme = unset(0x001a_1b26, 0x00c0_caf5);
        let derived = theme.ui();
        theme.tab_bar_background = Some(rgb(0x0030_3030));
        theme.tab_accent = Some(rgb(0x0012_3456));
        let ui = theme.ui();
        assert_eq!(ui.tab_bar_background, rgb(0x0030_3030));
        assert_eq!(ui.tab_accent, rgb(0x0012_3456));
        assert_ne!(ui.tab_inactive_foreground, derived.tab_inactive_foreground);
        assert_ne!(ui.tab_border, derived.tab_border);
    }

    #[test]
    fn definitions_apply_and_validate_every_ui_color() {
        let definition: ThemeDefinition = toml::from_str(
            "tab_bar_background = '#010203'\ntab_active_background = '#040506'\ntab_foreground = '#070809'\ntab_inactive_foreground = '#0a0b0c'\ntab_border = '#0d0e0f'\ntab_accent = '#101112'",
        )
        .expect("ui colors");
        let ui = definition.apply(unset(0, 0x00ff_ffff)).unwrap().ui();
        let white = rgb(0x00ff_ffff);
        assert_eq!(
            ui,
            UiColors {
                tab_bar_background: rgb(0x0001_0203),
                tab_active_background: rgb(0x0004_0506),
                tab_foreground: rgb(0x0007_0809),
                tab_inactive_foreground: rgb(0x000a_0b0c),
                tab_border: rgb(0x000d_0e0f),
                tab_accent: rgb(0x0010_1112),
                tab_hover_background: Rgba::with_alpha(white, 0x0a),
                scrollbar_thumb: Rgba::with_alpha(white, 0xbb),
                scrollbar_track: Rgba::with_alpha(white, 0x14),
            }
        );
        let invalid: ThemeDefinition =
            toml::from_str("tab_border = '#12345z'").expect("string color");
        assert!(invalid.apply(Theme::default()).is_err());
    }

    #[test]
    fn overlay_colors_accept_six_or_eight_hex_digits() {
        let definition: ThemeDefinition = toml::from_str(
            "tab_hover_background = '#10203040'\nscrollbar_thumb = '#506070'\nscrollbar_track = '#8090a0b0'",
        )
        .expect("overlay colors");
        let ui = definition.apply(Theme::default()).unwrap().ui();
        assert_eq!(
            ui.tab_hover_background,
            Rgba::with_alpha(rgb(0x0010_2030), 0x40)
        );
        assert_eq!(ui.scrollbar_thumb, Rgba::opaque(rgb(0x0050_6070)));
        assert_eq!(
            ui.scrollbar_track,
            Rgba::with_alpha(rgb(0x0080_90a0), 0xb0)
        );
        for invalid in ["#12345", "#1234567", "#123456789", "123456", "#12345g"]
        {
            assert!(parse_color_alpha(invalid).is_err(), "{invalid}");
        }
    }
}

#[cfg(test)]
mod tabs_tests {
    use super::*;

    #[test]
    fn tab_settings_use_settled_defaults() {
        let configured: RawConfig = toml::from_str("").expect("default config");
        assert_eq!(configured.tabs, TabsConfig::default());
        assert_eq!(
            configured.tabs.position,
            if cfg!(target_os = "macos") {
                TabPosition::Titlebar
            } else {
                TabPosition::Top
            },
            "macOS holds tabs in the title bar; Linux keeps the WM's"
        );
        assert!(!configured.window.shortcut_hints);
    }

    #[test]
    fn tab_settings_accept_every_override() {
        let configured: RawConfig = toml::from_str(
            "[tabs]\nposition = 'right'\nalways_show = true\nauto_hide_in_fullscreen = true\nstyle = 'pill'\npill_accent = true\nclose_button = 'always'\nnotch = 'right'\nwidth = 'fit'\nmin_width = 72\nmax_width = 480\nlabel = 'process_and_directory'\ndirectory = 'short'",
        )
        .expect("tab settings");
        configured.validate_values().expect("valid tab settings");
        assert_eq!(
            configured.tabs,
            TabsConfig {
                position: TabPosition::Right,
                always_show: true,
                auto_hide_in_fullscreen: true,
                style: TabStyle::Pill,
                pill_accent: true,
                close_button: TabCloseButton::Always,
                notch: TabNotch::Right,
                width: TabWidth::Fit,
                min_width: 72.0,
                max_width: 480.0,
                label: TabLabel::ProcessAndDirectory,
                directory: TabDirectory::Short,
            }
        );
    }

    #[test]
    fn metadata_and_bell_settings_use_safe_defaults_and_accept_rollback_values()
    {
        let defaults: RawConfig = toml::from_str("").expect("default config");
        assert_eq!(defaults.tabs.label, TabLabel::Smart);
        assert_eq!(defaults.tabs.directory, TabDirectory::Name);
        assert_eq!(
            defaults.terminal.new_tab_directory,
            NewTabDirectory::Inherit
        );
        assert!(defaults.terminal.bell.visual);

        let configured: RawConfig = toml::from_str(
            "[terminal]\nnew_tab_directory = 'default'\n[terminal.bell]\nvisual = false\n[tabs]\nlabel = 'directory'",
        )
        .expect("metadata settings");
        assert_eq!(configured.tabs.label, TabLabel::Directory);
        assert_eq!(
            configured.terminal.new_tab_directory,
            NewTabDirectory::Default
        );
        assert!(!configured.terminal.bell.visual);
    }

    #[test]
    fn tab_minimum_width_accepts_48_and_rejects_smaller_values() {
        let boundary: RawConfig =
            toml::from_str("[tabs]\nmin_width = 48").expect("minimum boundary");
        boundary.validate_values().expect("minimum is inclusive");
        for value in ["47.99", "nan"] {
            let configured: RawConfig =
                toml::from_str(&format!("[tabs]\nmin_width = {value}"))
                    .expect("numeric minimum");
            assert_eq!(
                configured.validate_values().unwrap_err().to_string(),
                "tabs.min_width must be at least 48 points"
            );
        }
    }

    #[test]
    fn tab_maximum_width_accepts_600_and_rejects_larger_values() {
        let boundary: RawConfig = toml::from_str("[tabs]\nmax_width = 600")
            .expect("maximum boundary");
        boundary.validate_values().expect("maximum is inclusive");
        for value in ["600.01", "inf"] {
            let configured: RawConfig =
                toml::from_str(&format!("[tabs]\nmax_width = {value}"))
                    .expect("numeric maximum");
            assert_eq!(
                configured.validate_values().unwrap_err().to_string(),
                "tabs.max_width must be at most 600 points"
            );
        }
    }

    #[test]
    fn tab_minimum_width_cannot_exceed_maximum_width() {
        let configured: RawConfig =
            toml::from_str("[tabs]\nmin_width = 241\nmax_width = 240")
                .expect("ordered bounds");
        assert_eq!(
            configured.validate_values().unwrap_err().to_string(),
            "tabs.min_width must not exceed tabs.max_width"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_defaults_and_bounds() {
        let defaults: RawConfig = toml::from_str("").unwrap();
        assert_eq!(defaults.palette, PaletteConfig::default());

        let maximum: RawConfig = toml::from_str(
            "[palette]\nplacement = \"center\"\nretain_query = false\nretain_query_seconds = 3600",
        )
        .unwrap();
        assert_eq!(maximum.palette.placement, PalettePlacement::Center);
        assert!(!maximum.palette.retain_query);
        assert_eq!(maximum.palette.retain_query_seconds, 3600);
        assert!(maximum.validate_values().is_ok());

        let above_maximum: RawConfig =
            toml::from_str("[palette]\nretain_query_seconds = 3601").unwrap();
        assert_eq!(
            above_maximum.validate_values().unwrap_err().to_string(),
            "palette.retain_query_seconds must be between 0 and 3600"
        );

        assert!(toml::from_str::<RawConfig>("[palette]\nvim = true").is_err());
    }

    #[test]
    fn scrollback_rows_default_to_10000_and_accept_0_through_50000() {
        let defaults: RawConfig = toml::from_str("").unwrap();
        assert_eq!(defaults.terminal.scrollback_rows, 10_000);
        for rows in [0, 1, 50_000] {
            let raw: RawConfig = toml::from_str(&format!(
                "[terminal]\nscrollback_rows = {rows}"
            ))
            .unwrap();
            assert_eq!(raw.terminal.scrollback_rows, rows);
            assert!(raw.validate_values().is_ok(), "{rows}");
        }

        let above: RawConfig =
            toml::from_str("[terminal]\nscrollback_rows = 50001").unwrap();
        assert_eq!(
            above.validate_values().unwrap_err().to_string(),
            "terminal.scrollback_rows must be between 0 and 50000"
        );
        for invalid in ["-1", "1.5", "'many'", "4294967296"] {
            assert!(
                toml::from_str::<RawConfig>(&format!(
                    "[terminal]\nscrollback_rows = {invalid}"
                ))
                .is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn scrollback_bytes_accept_counts_and_sizes_up_to_16_gib() {
        let bytes = |value: &str| {
            let raw: RawConfig = toml::from_str(&format!(
                "[terminal]\nscrollback_bytes = {value}"
            ))?;
            raw.validate_values().unwrap();
            Ok::<_, toml::de::Error>(
                raw.terminal.scrollback().unwrap().bytes().unwrap(),
            )
        };
        for (value, expected) in [
            ("0", 0),
            ("1048576", 1 << 20),
            ("'2048'", 2048),
            ("'0B'", 0),
            ("'64MiB'", 64 << 20),
            ("'64 MiB'", 64 << 20),
            ("'64mib'", 64 << 20),
            ("'50MB'", 50_000_000),
            ("'50mb'", 50_000_000),
            ("'3KB'", 3000),
            ("'3KiB'", 3072),
            ("'2GB'", 2_000_000_000),
            ("'16GiB'", 16 << 30),
            ("17179869184", 16 << 30),
        ] {
            assert_eq!(bytes(value).unwrap(), expected, "{value}");
        }
        let defaults: RawConfig = toml::from_str("").unwrap();
        assert_eq!(defaults.terminal.scrollback().unwrap().bytes(), None);

        for invalid in [
            "-1",
            "17179869185",
            "'17GiB'",
            "'1.5GiB'",
            "'64XB'",
            "'64 '",
            "' 64MiB'",
            "'64  MiB'",
            "'MiB'",
            "''",
            "'99999999999999999999'",
            "true",
        ] {
            let error = bytes(invalid).unwrap_err().to_string();
            assert!(
                error.contains("scrollback_bytes")
                    && error.contains("\"64MiB\""),
                "{invalid}: {error}"
            );
        }
    }

    #[test]
    fn clipboard_write_policy_defaults_to_allow_and_rejects_unknown_values() {
        let defaults: RawConfig = toml::from_str("").unwrap();
        assert_eq!(
            defaults.terminal.clipboard_write,
            ClipboardWritePolicy::Allow
        );

        let denied: RawConfig =
            toml::from_str("[terminal]\nclipboard_write = 'deny'").unwrap();
        assert_eq!(denied.terminal.clipboard_write, ClipboardWritePolicy::Deny);

        assert!(
            toml::from_str::<RawConfig>(
                "[terminal]\nclipboard_write = 'prompt'"
            )
            .is_err()
        );
    }
}
