use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};

use crate::themes::{self, ThemePair};
pub(super) use huterm_config::{
    ClipboardWritePolicy, ConfigError, FontConfig, GeneralConfig,
    KeybindingEntry, LegacyTerminalEngine, LinkModifiers, MacosFullscreenMode,
    MacosOptionAsAlt, RawConfig, RightClickAction, TabPosition, TabsConfig,
    TerminalConfig, Theme, UpdateConfig, WindowConfig, keybinding_diagnostic,
};

/// Serializes every desktop configuration writer across windows and Settings.
/// Hold this on a background worker through load, validation, and apply;
/// `ConfigFile`'s final recheck and rename require one writer per process.
pub(crate) fn write_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &LOCK
}

pub(super) const LEGACY_ALACRITTY_WARNING: &str = "terminal.engine = \"alacritty\" is deprecated; Huterm now uses Ghostty. Remove terminal.engine from your configuration.";

pub(super) const LINUX_AUDIBLE_BELL_WARNING: &str =
    "terminal.bell.audio plays sounds only on macOS; Linux bells stay silent.";

pub(super) const DEFAULT_CONFIG: &str = r##"#:schema https://github.com/jimeh/huterm/releases/latest/download/huterm.schema.json

[general]
# Reload this file automatically when it changes on disk.
watch_config = true

[terminal]
# Terminal identity for new shells: "xterm-256color", "auto", or
# "xterm-huterm". Auto uses xterm-huterm when any local terminfo entry for
# it exists, including Huterm's packaged one. SSH hosts need that entry too,
# or line editing there may break.
term = "xterm-256color"
# Close tabs quietly when their root shell exits.
# Set false to retain read-only history after exit.
close_on_exit = true
# Live history per terminal in screen rows, 0 to 100000; 0 keeps none.
# Reload applies it to open tabs; shrinking it discards the oldest rows.
scrollback_rows = 10000
# Optionally also limit history by size, such as "64MiB" or "50MB", up to
# "16GiB". Whichever limit is reached first applies.
# scrollback_bytes = "64MiB"
# Compress history while the terminal is idle, which keeps about a tenth of
# its memory for text. The first width change afterwards takes longer.
scrollback_compression = true
# Snapshot pacing: display follows frame delivery; unlimited removes that cap.
refresh = "display"
# Open new tabs in a usable local directory reported by the active terminal.
new_tab_directory = "inherit"
# Allow terminal content such as tmux to replace the system clipboard.
# Reload applies this to existing terminals: "allow" or "deny".
clipboard_write = "allow"
# Send Option character chords as terminal Meta: "off", "both", "left", or
# "right"; the other Option key still types macOS characters. Reload applies
# this to existing terminals. Linux always uses Alt as Meta.
macos_option_as_alt = "off"
# Hold Cmd on macOS or Ctrl on Linux to open HTTP/HTTPS links.
# Also hold Shift when terminal applications request mouse reporting.
links = true
# link_modifiers = "cmd" # Linux default: "ctrl"
# Right-click: "menu", "paste", "copy_or_paste", or "ignore". While a
# terminal application takes the mouse, Shift+right-click applies it.
right_click = "menu"

[terminal.bell]
# Flash active terminals and mark inactive tabs when a bell rings.
visual = false
# macOS plays a sound when a bell rings: "system" (the macOS alert sound) or a
# sound from /System/Library/Sounds, such as "glass". volume (0 to 1) applies
# to named sounds; the alert sound follows the macOS alert volume. Linux has
# no audible bell.
# audio = true # macOS default; Linux default: false
sound = "system"
volume = 0.5

[font]
family = "Menlo"
size = 14.0
# The interface font for tabs, Settings, menus, and dialogs. An empty family
# uses the system font; the size runs from 10 to 16 points.
# ui_family = ""
# ui_size = 13.0

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
# Open the window, tab, pane, and terminal menus as native macOS menus. Linux
# ignores it and always draws them in Huterm's style.
macos_native_menus = true

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

[panes]
# Whether a split tab's panes show title bars: visible, hover, or hidden.
# Hover bars take no space and fade in over the terminal while the pointer
# rests where they would be; hidden bars never show, and close, zoom, and move
# remain commands.
title_bar = "visible"
# Which edge of a pane its title bar, or the focus accent, runs along: top or
# bottom.
title_bar_position = "top"
# How strongly an accent line marks the focused pane while its title bar is
# off screen under hover or hidden bars, from 0 (off) to 1.
focus_accent = 0.0
# How much panes without focus dim, from 0 (off) to 0.8.
inactive_dim = 0.2
# How much panes without focus darken toward black, from 0 (off) to 0.5.
inactive_dim_background = 0.0
# Hold these modifiers and press anywhere in a split tab's pane to drag the
# pane, as its title bar does: two or more of cmd, ctrl, alt, and shift, or
# "off". Defaults to cmd-ctrl on macOS and ctrl-alt on Linux.
# drag_modifier = "cmd-ctrl"
# Who owns a pane's zoom: view, where each window zooms on its own, or
# shared, where the tab owns it and every window set to shared shows it.
zoom = "view"

[updates]
# true enables scheduled checks and false disables them. Omit this setting to
# let Sparkle ask on the second launch of a fresh macOS profile. Removing an
# explicit value later preserves Sparkle's current stored preference.
# automatic_checks = true
# Minimum 1 hour. Omit this setting for Sparkle's stored interval, which is 24
# hours on a fresh profile.
# check_interval_hours = 24

[theme]
name = "tokyo-night"
# Follow the system's light or dark mode with a theme for each; setting
# either replaces name for that mode:
# light = "tokyo-night-day"
# dark = "tokyo-night"
# Override individual colors without replacing the whole palette:
# background = "#1a1b26"
# ansi_red = "#f7768e"
# ansi_bright_red = "#ff899d"

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
    pub(super) general: GeneralConfig,
    pub(super) font: FontConfig,
    pub(super) window: WindowConfig,
    pub(super) tabs: TabsConfig,
    pub(super) updates: UpdateConfig,
    pub(super) palette: huterm_config::PaletteConfig,
    pub(super) panes: huterm_config::PanesConfig,
    pub(super) terminal: TerminalConfig,
    /// The theme in use: one of `themes`, chosen by the system appearance
    /// through [`Config::follow_appearance`].
    pub(super) theme: Theme,
    pub(super) themes: ThemePair,
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
    pub(super) inputs: Inputs,
}

/// What one load or reload consumed: the theme files it read or probed, in
/// order, and a fingerprint of those bytes and the configuration's. A
/// change to any of them changes the fingerprint.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct Inputs {
    pub(super) themes: Vec<PathBuf>,
    pub(super) fingerprint: Fingerprint,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(super) struct Fingerprint(u64);

/// Records the bytes a load consumes, in the order it consumes them.
pub(super) struct Consumed {
    hasher: DefaultHasher,
    themes: Vec<PathBuf>,
}

impl Consumed {
    pub(super) fn new() -> Self {
        Self {
            hasher: DefaultHasher::new(),
            themes: Vec::new(),
        }
    }

    /// The configuration file's bytes, or `None` when it could not be read.
    fn config(&mut self, bytes: Option<&[u8]>) {
        0_u8.hash(&mut self.hasher);
        bytes.hash(&mut self.hasher);
    }

    /// An external theme file's bytes, or `None` when it was absent or
    /// unreadable.
    pub(super) fn theme(&mut self, path: &Path, bytes: Option<&[u8]>) {
        1_u8.hash(&mut self.hasher);
        path.hash(&mut self.hasher);
        bytes.hash(&mut self.hasher);
        self.themes.push(path.to_owned());
    }

    fn finish(self) -> Inputs {
        Inputs {
            themes: self.themes,
            fingerprint: Fingerprint(self.hasher.finish()),
        }
    }
}

/// Recomputes the fingerprint a load of `config` would record if it read
/// the same theme files as `themes`. `None` when the configuration file is
/// missing, as it is for a moment while an editor saves by renaming.
pub(super) fn fingerprint(
    config: &Path,
    themes: &[PathBuf],
) -> Option<Fingerprint> {
    let mut consumed = Consumed::new();
    match fs::read(config) {
        Ok(bytes) => consumed.config(Some(&bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(_) => consumed.config(None),
    }
    for theme in themes {
        consumed.theme(theme, fs::read(theme).ok().as_deref());
    }
    Some(consumed.finish().fingerprint)
}

impl Config {
    /// Uses the theme for a `dark` or light system appearance, returning
    /// whether the theme in use changed. Without `[theme] light` or `dark`
    /// both are the same theme and nothing changes.
    pub(super) fn follow_appearance(&mut self, dark: bool) -> bool {
        let theme = self.themes.get(dark);
        if self.theme == *theme {
            return false;
        }
        self.theme = theme.clone();
        true
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            warning: None,
            general: GeneralConfig::default(),
            font: FontConfig::default(),
            theme: themes::default_theme(),
            themes: ThemePair::default_pair(),
            window: WindowConfig::default(),
            tabs: TabsConfig::default(),
            updates: UpdateConfig::default(),
            palette: huterm_config::PaletteConfig::default(),
            panes: huterm_config::PanesConfig::default(),
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
    let mut consumed = Consumed::new();
    let source = read_config(&path, &mut consumed);
    match source {
        Ok(source) => match parse_consumed(&source, &path, &mut consumed) {
            Ok(config) => LoadedConfig {
                warning: config.warning.clone(),
                config,
                path,
                error: None,
                fatal: false,
                inputs: consumed.finish(),
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
                        general: fallback_general(&source),
                        terminal,
                        ..Config::default()
                    },
                    fatal,
                    error: Some(format!("{}: {error}", path.display())),
                    warning,
                    path,
                    inputs: consumed.finish(),
                }
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => LoadedConfig {
            config: Config::default(),
            path,
            error: None,
            warning: None,
            fatal: false,
            inputs: consumed.finish(),
        },
        Err(error) => LoadedConfig {
            config: Config::default(),
            fatal: false,
            error: Some(format!("{}: {error}", path.display())),
            warning: None,
            path,
            inputs: consumed.finish(),
        },
    }
}

/// Reads the configuration file as text, recording its bytes.
fn read_config(path: &Path, consumed: &mut Consumed) -> io::Result<String> {
    let bytes = fs::read(path).inspect_err(|_| consumed.config(None))?;
    consumed.config(Some(&bytes));
    String::from_utf8(bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub(crate) fn create_default(path: &Path) -> Result<(), ConfigFileError> {
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

/// The outcome of rereading the configuration.
pub(super) struct Reloaded {
    pub(super) config: Result<Config, String>,
    pub(super) inputs: Inputs,
    /// The file was missing, as it is for a moment while an editor saves
    /// by renaming.
    pub(super) missing: bool,
}

pub(super) fn reload(path: &Path) -> Reloaded {
    let mut consumed = Consumed::new();
    let (config, missing) = match read_config(path, &mut consumed) {
        Ok(source) => (
            parse_consumed(&source, path, &mut consumed)
                .map_err(|error| format!("{}: {error}", path.display())),
            false,
        ),
        Err(error) => (
            Err(format!("{}: {error}", path.display())),
            error.kind() == io::ErrorKind::NotFound,
        ),
    };
    Reloaded {
        config,
        inputs: consumed.finish(),
        missing,
    }
}

/// Reads `[general] watch_config` from a file that failed to load but is
/// valid TOML, so a fix saved in an editor still loads on its own. Anything
/// unreadable keeps the default.
fn fallback_general(source: &str) -> GeneralConfig {
    let watch_config = toml::from_str::<toml::Value>(source)
        .ok()
        .and_then(|value| value.get("general")?.get("watch_config")?.as_bool())
        .unwrap_or(GeneralConfig::default().watch_config);
    GeneralConfig { watch_config }
}

/// Checks a candidate file the way a reload does, without registering
/// anything: parsing, values, theme resolution relative to `path`, the
/// local keymap, and global shortcuts. Font resolution needs the GPUI text
/// system and is left to the reload.
pub(super) fn validator(
    path: PathBuf,
) -> impl Fn(&str) -> Result<(), String> + Send + Sync {
    move |source| {
        let config =
            parse_at(source, &path).map_err(|error| error.to_string())?;
        let local = crate::keymap::compile(
            crate::keymap::Platform::current(),
            &config.keybindings,
        )
        .map_err(|error| error.to_string())?;
        crate::quake::hotkeys::compile(
            &config.global_keybindings,
            &config.quake,
            &local,
        )?;
        Ok(())
    }
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

pub(crate) fn parse_at(
    source: &str,
    path: &Path,
) -> Result<Config, ConfigError> {
    parse_consumed(source, path, &mut Consumed::new())
}

fn parse_consumed(
    source: &str,
    path: &Path,
    consumed: &mut Consumed,
) -> Result<Config, ConfigError> {
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
    let warning = join_warnings(
        warning,
        (cfg!(target_os = "linux") && raw.terminal.bell.audio)
            .then_some(LINUX_AUDIBLE_BELL_WARNING),
    );
    raw.validate_values()?;
    let directory = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("themes");
    let themes =
        themes::resolve(&raw.theme, &raw.themes, &directory, consumed)?;
    let keybindings = raw
        .keybinding
        .into_iter()
        .enumerate()
        .map(|(index, entry)| entry.validate(index + 1))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Config {
        warning,
        general: raw.general,
        window: raw.window,
        tabs: raw.tabs,
        updates: raw.updates,
        palette: raw.palette,
        panes: raw.panes,
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
            scrollback: raw.terminal.scrollback()?,
            scrollback_compression: raw.terminal.scrollback_compression,
        },
        font: FontConfig {
            family: raw.font.family,
            size: raw.font.size,
            ui_family: Some(raw.font.ui_family.trim().to_owned())
                .filter(|family| !family.is_empty()),
            ui_size: raw.font.ui_size,
        },
        // Until the caller follows the system appearance.
        theme: themes.dark.clone(),
        themes,
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

/// `warning` with `another` after it, so neither replaces the other.
fn join_warnings(
    warning: Option<String>,
    another: Option<&str>,
) -> Option<String> {
    match (warning, another) {
        (Some(warning), Some(another)) => Some(format!("{warning} {another}")),
        (warning, another) => warning.or_else(|| another.map(str::to_owned)),
    }
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

/// `modifiers` as the bits pointer modifier settings number them, or
/// `None` while Fn is held, which no setting names.
pub(super) fn modifier_bits(modifiers: gpui::Modifiers) -> Option<u8> {
    (!modifiers.function).then(|| {
        u8::from(modifiers.platform)
            | (u8::from(modifiers.control) << 1)
            | (u8::from(modifiers.alt) << 2)
            | (u8::from(modifiers.shift) << 3)
    })
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
        modifier_bits(modifiers)
            .is_some_and(|actual| self.matches_bits(actual, mouse_reporting))
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

    #[test]
    fn a_drag_modifier_on_the_link_chord_fails_load_and_the_settings_check() {
        let validate = validator(PathBuf::from("config.toml"));
        let load = |source: &str| {
            let loaded = parse_at(source, Path::new("config.toml"))
                .map_err(|error| error.to_string());
            // Settings refuses a write that a reload would refuse.
            assert_eq!(
                validate(source),
                loaded.as_ref().map(|_| ()).map_err(Clone::clone)
            );
            loaded
        };
        // A custom chord, plainly and plus Shift.
        let custom = "[terminal]\nlink_modifiers = 'cmd-alt'\n";
        for drag in ["alt-cmd", "cmd-alt-shift"] {
            let message =
                load(&format!("{custom}[panes]\ndrag_modifier = '{drag}'\n"))
                    .unwrap_err();
            assert!(
                message.contains("panes.drag_modifier")
                    && message.contains("terminal.link_modifiers"),
                "{message}"
            );
        }
        let moved = load(&format!(
            "{custom}[panes]\ndrag_modifier = 'ctrl-alt-shift'\n"
        ))
        .unwrap();
        assert_eq!(
            moved
                .panes
                .drag_modifiers(
                    moved.terminal.link_modifiers,
                    cfg!(target_os = "macos"),
                )
                .map(huterm_config::PaneDragModifiers::bits),
            Some(14)
        );
        // Keeping the link chord unchanged keeps its own messages.
        assert!(
            load("[terminal]\nlink_modifiers = 'cmd-cmd'\n")
                .unwrap_err()
                .contains("terminal.link_modifiers cannot contain duplicate")
        );
    }
}
