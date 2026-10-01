use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::themes;
pub(super) use huterm_config::{
    ClipboardWritePolicy, ConfigError, FontConfig, KeybindingEntry,
    LegacyTerminalEngine, LinkModifiers, MacosFullscreenMode, MacosOptionAsAlt,
    RawConfig, RightClickAction, TabPosition, TabsConfig, TerminalConfig,
    Theme, UpdateConfig, WindowConfig, keybinding_diagnostic,
};

pub(super) const LEGACY_ALACRITTY_WARNING: &str = "terminal.engine = \"alacritty\" is deprecated; Huterm now uses Ghostty. Remove terminal.engine from your configuration.";

pub(super) const DEFAULT_CONFIG: &str = r##"#:schema https://github.com/jimeh/huterm/releases/latest/download/huterm.schema.json

[terminal]
# Terminal identity for new shells: "auto", "xterm-huterm", or
# "xterm-256color". Auto uses Huterm's packaged entry when available.
term = "auto"
# Close tabs quietly when their root shell exits.
# Set false to retain read-only history after exit.
close_on_exit = true
# Snapshot pacing: display follows frame delivery; unlimited removes that cap.
refresh = "display"
# Open new tabs in a usable local directory reported by the active terminal.
new_tab_directory = "inherit"
# Allow terminal content such as tmux to replace the system clipboard.
# Reload applies this to existing terminals: "allow" or "deny".
clipboard_write = "allow"
# Send Option character chords as terminal Meta: "off" or "both".
# Reload applies this to existing terminals. Linux always uses Alt as Meta.
macos_option_as_alt = "off"
# Hold Cmd on macOS or Ctrl on Linux to open HTTP/HTTPS links.
# Also hold Shift when terminal applications request mouse reporting.
links = true
# link_modifiers = "cmd" # Linux default: "ctrl"
# Right-click: "menu", "paste", "copy_or_paste", or "ignore". While a
# terminal application takes the mouse, Shift+right-click applies it.
right_click = "menu"

# Flash active terminals and mark inactive tabs when a bell rings.
[terminal.bell]
visual = true

[font]
family = "Menlo"
size = 14.0

[window]
# Default fullscreen mode on macOS: "native" or "non_native". Linux ignores it.
macos_fullscreen_mode = "non_native"
# Padding in logical points on each side of the terminal.
padding_x = 4.0
padding_y = 4.0
# Split unused column space between left and right instead of only the right.
padding_balance = false
# Show the window menu button in the title bar, or in the tab bar without one.
menu_button = true
# Show key hints on the scroll pill and on dialog and panel buttons.
shortcut_hints = false

[tabs]
# Automatic label: smart, title, process, directory, or process_and_directory.
# Smart shows a running program's own title or name, and the directory at an
# idle shell.
label = "smart"
# How labels show directories: name (huterm), path (~/Projects/huterm), or
# short (~/P/huterm).
directory = "name"
# Tab placement: top, bottom, left, right, or titlebar. Titlebar merges the
# tabs into the title bar: on macOS, or on Linux when the window manager
# grants client-side decorations (GNOME, KDE). Without a title bar
# (fullscreen, Quake windows, or a Linux window manager that draws its own)
# it acts as top. Defaults to titlebar on macOS and top on Linux.
# position = "titlebar"
always_show = false
auto_hide_in_fullscreen = false
# Tab style: pill or strip.
style = "pill"
# Draw an accent bar inside the active pill.
pill_accent = false
# Show tab close buttons on: active, hover, or always.
close_button = "active"
# In fullscreen on a notched display, put a top bar beside the notch: off,
# left, or right.
notch = "left"
# Horizontal tab width: fit or fill.
width = "fit"
# Width bounds in logical points for fit mode.
min_width = 96.0
max_width = 240.0

[updates]
# true enables scheduled checks and false disables them. Omit this setting to
# let Sparkle ask on the second launch of a fresh macOS profile. Removing an
# explicit value later preserves Sparkle's current stored preference.
# automatic_checks = true
# Minimum 1 hour. Omit this setting for Sparkle's stored interval, which is 24
# hours on a fresh profile.
# check_interval_hours = 24

[theme]
name = "huterm-dark"
# Override individual colors without replacing the whole palette:
# background = "#1d1f21"
# ansi_red = "#cc6666"
# ansi_bright_red = "#d54e53"

# Keybindings extend the platform defaults; later entries win for the same key.
# `key` uses GPUI keystroke syntax (cmd, ctrl, alt, shift, fn); `when` is an
# optional key-context predicate; `args` is a table of named arguments.
# [[keybinding]]
# key = "cmd-1"
# command = "select_tab"
# args = { index = 1 }
# when = "Terminal && !confirming"
# description = "Select the first tab"

# Remove a default binding so the terminal receives the key instead:
# [[keybinding]]
# key = "shift-pageup"
# command = "unbind"
"##;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Config {
    pub(super) warning: Option<String>,
    pub(super) font: FontConfig,
    pub(super) window: WindowConfig,
    pub(super) tabs: TabsConfig,
    pub(super) updates: UpdateConfig,
    pub(super) palette: huterm_config::PaletteConfig,
    pub(super) terminal: TerminalConfig,
    pub(super) theme: Theme,
    pub(super) keybindings: Vec<KeybindingEntry>,
    pub(super) quake: huterm_config::quake::Config,
    pub(super) global_keybindings: Vec<KeybindingEntry>,
}

#[derive(Debug)]
pub(super) struct LoadedConfig {
    pub(super) config: Config,
    pub(super) path: PathBuf,
    pub(super) error: Option<String>,
    pub(super) warning: Option<String>,
    pub(super) fatal: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            warning: None,
            font: FontConfig::default(),
            theme: Theme::huterm_dark(),
            window: WindowConfig::default(),
            tabs: TabsConfig::default(),
            updates: UpdateConfig::default(),
            palette: huterm_config::PaletteConfig::default(),
            terminal: TerminalConfig::default(),
            keybindings: Vec::new(),
            quake: crate::quake::Config {
                profiles: BTreeMap::from([(
                    "default".into(),
                    crate::quake::Profile::default(),
                )]),
            },
            global_keybindings: Vec::new(),
        }
    }
}

pub(super) fn load() -> LoadedConfig {
    let path = config_path();
    load_path(path)
}

fn load_path(path: PathBuf) -> LoadedConfig {
    match fs::read_to_string(&path) {
        Ok(source) => match parse_at(&source, &path) {
            Ok(config) => LoadedConfig {
                warning: config.warning.clone(),
                config,
                path,
                error: None,
                fatal: false,
            },
            Err(error) => {
                let (warning, clipboard_write, fatal, error) =
                    match fallback_terminal(&source) {
                        Ok((warning, clipboard_write)) => {
                            (warning, clipboard_write, false, error)
                        }
                        Err(error) => {
                            (None, ClipboardWritePolicy::default(), true, error)
                        }
                    };
                let terminal = TerminalConfig {
                    clipboard_write,
                    ..TerminalConfig::default()
                };
                LoadedConfig {
                    config: Config {
                        warning: warning.clone(),
                        terminal,
                        ..Config::default()
                    },
                    fatal,
                    error: Some(format!("{}: {error}", path.display())),
                    warning,
                    path,
                }
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            LoadedConfig {
                config: Config::default(),
                path,
                error: None,
                warning: None,
                fatal: false,
            }
        }
        Err(error) => LoadedConfig {
            config: Config::default(),
            fatal: false,
            error: Some(format!("{}: {error}", path.display())),
            warning: None,
            path,
        },
    }
}

pub(super) fn create_default(path: &Path) -> Result<(), ConfigFileError> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(ConfigFileError::Io)?;
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            use std::io::Write;
            file.write_all(default_document().as_bytes())
                .map_err(ConfigFileError::Io)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(())
        }
        Err(error) => Err(ConfigFileError::Io(error)),
    }
}

fn default_document() -> String {
    let family = Config::default().font.family;
    DEFAULT_CONFIG.replacen(
        "family = \"Menlo\"",
        &format!("family = \"{family}\""),
        1,
    )
}

fn config_path() -> PathBuf {
    config_path_from(|name| std::env::var_os(name))
}

fn config_path_from(
    environment: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> PathBuf {
    if let Some(path) = environment("HUTERM_CONFIG_FILE") {
        return PathBuf::from(path);
    }
    if let Some(directory) = environment("XDG_CONFIG_HOME") {
        return PathBuf::from(directory).join("huterm/config.toml");
    }
    environment("HOME")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join(".config/huterm/config.toml")
}

pub(super) fn home_directory() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

pub(super) fn reload(path: &Path) -> Result<Config, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    parse_at(&source, path)
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn fallback_terminal(
    source: &str,
) -> Result<(Option<String>, ClipboardWritePolicy), ConfigError> {
    let value: toml::Value =
        toml::from_str(source).map_err(ConfigError::Toml)?;
    let terminal = value.get("terminal");
    let warning = terminal
        .and_then(|terminal| terminal.get("engine"))
        .map_or_else(
            || Ok(None),
            |engine| {
                parse_engine(engine.as_str().ok_or_else(|| {
                    ConfigError::Engine(
                        "terminal.engine must be a string".into(),
                    )
                })?)
            },
        )?;
    let clipboard_write = terminal
        .and_then(|terminal| terminal.get("clipboard_write"))
        .cloned()
        .map_or_else(
            || Ok(ClipboardWritePolicy::default()),
            |value| value.try_into().map_err(ConfigError::Toml),
        )?;
    Ok((warning, clipboard_write))
}

fn parse_engine(name: &str) -> Result<Option<String>, ConfigError> {
    match name {
        "alacritty" => Ok(Some(LEGACY_ALACRITTY_WARNING.to_owned())),
        "ghostty" => Ok(None),
        name => Err(ConfigError::Engine(format!(
            "unknown terminal engine {name:?}"
        ))),
    }
}

fn parse_at(source: &str, path: &Path) -> Result<Config, ConfigError> {
    let value: toml::Value =
        toml::from_str(source).map_err(ConfigError::Toml)?;
    reject_moved_tab_settings(&value)?;
    // Deserialize from source, not the parsed value, so errors keep spans.
    let raw: RawConfig = toml::from_str(source).map_err(ConfigError::Toml)?;
    let warning = match raw.terminal.engine {
        Some(LegacyTerminalEngine::Alacritty) => {
            Some(LEGACY_ALACRITTY_WARNING.to_owned())
        }
        Some(LegacyTerminalEngine::Ghostty) | None => None,
    };
    raw.validate_values()?;
    let directory = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("themes");
    let theme = themes::resolve(&raw.theme, &raw.themes, &directory)?;
    let keybindings = raw
        .keybinding
        .into_iter()
        .enumerate()
        .map(|(index, entry)| entry.validate(index + 1))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Config {
        warning,
        window: raw.window,
        tabs: raw.tabs,
        updates: raw.updates,
        palette: raw.palette,
        terminal: TerminalConfig {
            term: raw.terminal.term,
            close_on_exit: raw.terminal.close_on_exit,
            refresh: raw.terminal.refresh,
            new_tab_directory: raw.terminal.new_tab_directory,
            bell: raw.terminal.bell,
            clipboard_write: raw.terminal.clipboard_write,
            links: raw.terminal.links,
            link_modifiers: raw.terminal.link_modifiers,
            macos_option_as_alt: raw.terminal.macos_option_as_alt,
            right_click: raw.terminal.right_click,
        },
        font: FontConfig {
            family: raw.font.family,
            size: raw.font.size,
        },
        theme,
        keybindings,
        quake: raw.quake.validated().map_err(ConfigError::Quake)?,
        global_keybindings: raw
            .global_keybinding
            .into_iter()
            .enumerate()
            .map(|(index, entry)| entry.validate(index + 1))
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn reject_moved_tab_settings(value: &toml::Value) -> Result<(), ConfigError> {
    let Some(window) = value.get("window").and_then(toml::Value::as_table)
    else {
        return Ok(());
    };
    for (legacy, message) in [
        ("tab_position", "window.tab_position moved to tabs.position"),
        (
            "always_show_tab_bar",
            "window.always_show_tab_bar moved to tabs.always_show",
        ),
        (
            "auto_hide_tab_bar_in_fullscreen",
            "window.auto_hide_tab_bar_in_fullscreen moved to tabs.auto_hide_in_fullscreen",
        ),
    ] {
        if window.contains_key(legacy) {
            return Err(ConfigError::Invalid(message));
        }
    }
    Ok(())
}

#[derive(Debug)]
pub(super) enum ConfigFileError {
    Io(std::io::Error),
}

impl fmt::Display for ConfigFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

/// Matches native modifiers without exposing GPUI through portable config.
pub(super) trait LinkModifiersExt {
    fn matches(self, modifiers: gpui::Modifiers, mouse_reporting: bool)
    -> bool;
}
impl LinkModifiersExt for LinkModifiers {
    fn matches(
        self,
        modifiers: gpui::Modifiers,
        mouse_reporting: bool,
    ) -> bool {
        let actual = u8::from(modifiers.platform)
            | (u8::from(modifiers.control) << 1)
            | (u8::from(modifiers.alt) << 2)
            | (u8::from(modifiers.shift) << 3);
        !modifiers.function && self.matches_bits(actual, mouse_reporting)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod link_config_tests {
    use super::*;
    #[test]
    fn link_modifier_combinations_match_exactly_and_add_mouse_shift() {
        let cmd = LinkModifiers::parse("cmd", true).unwrap();
        let mut modifiers = gpui::Modifiers {
            platform: true,
            ..gpui::Modifiers::default()
        };
        assert!(cmd.matches(modifiers, false));
        assert!(!cmd.matches(modifiers, true));
        modifiers.shift = true;
        assert!(cmd.matches(modifiers, true));
        assert!(!cmd.matches(modifiers, false));
        let configured = LinkModifiers::parse("cmd-shift", true).unwrap();
        assert!(configured.matches(modifiers, true));
        assert!(configured.matches(modifiers, false));
        modifiers.alt = true;
        assert!(!configured.matches(modifiers, true));
        assert!(LinkModifiers::parse("ctrl", false).unwrap().matches(
            gpui::Modifiers {
                control: true,
                ..gpui::Modifiers::default()
            },
            false
        ));
    }
    #[test]
    fn link_config_rejects_mac_control_duplicates_empty_and_keys() {
        for value in ["ctrl", "cmd-ctrl"] {
            assert!(
                LinkModifiers::parse(value, true)
                    .unwrap_err()
                    .contains("Control-left")
            );
        }
        for value in ["", "cmd-cmd", "cmd-r", "fn", "cmd-", "command"] {
            assert!(LinkModifiers::parse(value, false).is_err(), "{value}");
        }
        let good = parse_at(
            "[terminal]\nlinks=false\nlink_modifiers='alt-shift'",
            Path::new("config.toml"),
        )
        .unwrap();
        assert!(!good.terminal.links);
        assert_eq!(
            good.terminal.link_modifiers,
            LinkModifiers::parse("alt-shift", false).unwrap()
        );
        assert!(
            parse_at(
                "[terminal]\nlink_modifiers='cmd-cmd'",
                Path::new("config.toml")
            )
            .is_err()
        );
    }
}
