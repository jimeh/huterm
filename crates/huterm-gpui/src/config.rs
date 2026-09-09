use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::themes;
pub(super) use huterm_config::{
    ConfigError, FontConfig, KeybindingEntry, LinkModifiers,
    MacosFullscreenMode, MacosOptionAsAlt, RawConfig, TabPosition,
    TerminalConfig, Theme, WindowConfig, keybinding_diagnostic,
};
use huterm_protocol::TerminalEngineKind;

pub(super) const DEFAULT_CONFIG: &str = r##"#:schema https://github.com/jimeh/huterm/releases/latest/download/huterm.schema.json

[terminal]
# Changes apply to newly created terminals. Both engines are included in every build.
engine = "alacritty"
# Close tabs quietly when their root shell exits.
# Set false to retain read-only history after exit.
close_on_exit = true
# Send Option character chords as terminal Meta: "off" or "both".
# Reload applies this to existing terminals. Linux always uses Alt as Meta.
macos_option_as_alt = "off"
# Hold Cmd on macOS or Ctrl on Linux to open HTTP/HTTPS links.
# Also hold Shift when terminal applications request mouse reporting.
links = true
# link_modifiers = "cmd" # Linux default: "ctrl"

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
# Tab placement: top, bottom, left, or right.
tab_position = "top"
always_show_tab_bar = false
auto_hide_tab_bar_in_fullscreen = false

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
    pub(super) engine: TerminalEngineKind,
    pub(super) font: FontConfig,
    pub(super) window: WindowConfig,
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
    pub(super) fatal: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            engine: TerminalEngineKind::Alacritty,
            font: FontConfig::default(),
            theme: Theme::default(),
            window: WindowConfig::default(),
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
                config,
                path,
                error: None,
                fatal: false,
            },
            Err(error) => {
                let (engine, fatal, error) = match fallback_engine(&source) {
                    Ok(engine) => (engine, false, error),
                    Err(error) => (TerminalEngineKind::default(), true, error),
                };
                LoadedConfig {
                    config: Config {
                        engine,
                        ..Config::default()
                    },
                    fatal,
                    error: Some(format!("{}: {error}", path.display())),
                    path,
                }
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            LoadedConfig {
                config: Config::default(),
                path,
                error: None,
                fatal: false,
            }
        }
        Err(error) => LoadedConfig {
            config: Config::default(),
            fatal: false,
            error: Some(format!("{}: {error}", path.display())),
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

fn fallback_engine(source: &str) -> Result<TerminalEngineKind, ConfigError> {
    let value: toml::Value =
        toml::from_str(source).map_err(ConfigError::Toml)?;
    let Some(engine) = value
        .get("terminal")
        .and_then(|terminal| terminal.get("engine"))
    else {
        return Ok(TerminalEngineKind::default());
    };
    parse_engine(engine.as_str().ok_or_else(|| {
        ConfigError::Engine("terminal.engine must be a string".into())
    })?)
}

fn parse_engine(name: &str) -> Result<TerminalEngineKind, ConfigError> {
    match name {
        "alacritty" => Ok(TerminalEngineKind::Alacritty),
        "ghostty" => Ok(TerminalEngineKind::Ghostty),
        name => Err(ConfigError::Engine(format!(
            "unknown terminal engine {name:?}"
        ))),
    }
}

fn parse_at(source: &str, path: &Path) -> Result<Config, ConfigError> {
    let raw: RawConfig = toml::from_str(source).map_err(ConfigError::Toml)?;
    let engine = parse_engine(&raw.terminal.engine)?;
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
        engine,
        window: raw.window,
        terminal: TerminalConfig {
            close_on_exit: raw.terminal.close_on_exit,
            links: raw.terminal.links,
            link_modifiers: raw.terminal.link_modifiers,
            macos_option_as_alt: raw.terminal.macos_option_as_alt,
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    static TEST_DIRECTORY_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    fn rgb(value: u32) -> huterm_protocol::Rgb {
        let [_, red, green, blue] = value.to_be_bytes();
        huterm_protocol::Rgb { red, green, blue }
    }

    #[test]
    fn schema_fixtures_match_application_parsing_and_keymap_compilation() {
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../../schemas/fixtures.json"
        ))
        .unwrap();
        let fixtures = fixtures.as_array().unwrap();
        assert_eq!(fixtures.len(), 107);
        for fixture in fixtures {
            let source = fixture["toml"].as_str().unwrap();
            let expected = fixture["valid"].as_bool().unwrap();
            let name = fixture["name"].as_str().unwrap();
            let result = if fixture["theme"].as_bool() == Some(true) {
                let directory = test_directory();
                fs::create_dir_all(directory.join("themes")).unwrap();
                fs::write(directory.join("themes/fixture.toml"), source)
                    .unwrap();
                let result = parse_at(
                    "[theme]\nname = 'fixture'",
                    &directory.join("config.toml"),
                );
                fs::remove_dir_all(directory).unwrap();
                result
            } else {
                parse(source)
            };
            let accepted = result.is_ok_and(|config| {
                crate::keymap::compile(
                    crate::keymap::Platform::current(),
                    &config.keybindings,
                )
                .is_ok_and(|local| {
                    crate::quake::hotkeys::compile(
                        &config.global_keybindings,
                        &config.quake,
                        &local,
                    )
                    .is_ok()
                })
            });
            assert_eq!(accepted, expected, "{name}: {source}");
        }
    }

    fn parse(source: &str) -> Result<Config, ConfigError> {
        // Keep bundled-theme tests independent of the crate's themes directory.
        let directory = test_directory();
        fs::create_dir(&directory).expect("empty config directory");
        let result = parse_at(source, &directory.join("config.toml"));
        fs::remove_dir(directory).expect("remove empty config directory");
        result
    }

    #[test]
    fn tab_bar_visibility_defaults_and_overrides() {
        let config = parse(DEFAULT_CONFIG).unwrap();
        assert!(!config.window.always_show_tab_bar);
        assert!(!config.window.auto_hide_tab_bar_in_fullscreen);
        let config = parse(
            &DEFAULT_CONFIG
                .replace(
                    "always_show_tab_bar = false",
                    "always_show_tab_bar = true",
                )
                .replace(
                    "auto_hide_tab_bar_in_fullscreen = false",
                    "auto_hide_tab_bar_in_fullscreen = true",
                ),
        )
        .unwrap();
        assert!(config.window.always_show_tab_bar);
        assert!(config.window.auto_hide_tab_bar_in_fullscreen);
    }

    #[test]
    fn fullscreen_mode_defaults_spelling_reload_and_startup_fallback() {
        assert_eq!(
            parse("").unwrap().window.macos_fullscreen_mode,
            MacosFullscreenMode::NonNative
        );
        for (value, expected) in [
            ("native", MacosFullscreenMode::Native),
            ("non_native", MacosFullscreenMode::NonNative),
        ] {
            assert_eq!(
                parse(&format!("[window]\nmacos_fullscreen_mode = '{value}'"))
                    .unwrap()
                    .window
                    .macos_fullscreen_mode,
                expected
            );
        }
        for value in ["nonnative", "NON_NATIVE", "simple", ""] {
            assert!(
                parse(&format!("[window]\nmacos_fullscreen_mode = '{value}'"))
                    .is_err()
            );
        }
        let directory = test_directory();
        fs::create_dir(&directory).unwrap();
        let path = directory.join("config.toml");
        fs::write(&path, "[window]\nmacos_fullscreen_mode = 'non_native'")
            .unwrap();
        let working = reload(&path).unwrap();
        fs::write(&path, "[terminal]\nengine = 'ghostty'\n[window]\nmacos_fullscreen_mode = 'simple'").unwrap();
        assert!(reload(&path).is_err());
        assert_eq!(
            working.window.macos_fullscreen_mode,
            MacosFullscreenMode::NonNative
        );
        let fallback = load_path(path);
        assert!(!fallback.fatal);
        assert!(fallback.error.is_some());
        assert_eq!(fallback.config.engine, TerminalEngineKind::Ghostty);
        assert_eq!(
            fallback.config.window.macos_fullscreen_mode,
            MacosFullscreenMode::NonNative
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn option_policy_defaults_and_validation_are_explicit() {
        assert_eq!(
            parse("").unwrap().terminal.macos_option_as_alt,
            MacosOptionAsAlt::Off
        );
        for (value, expected) in [
            ("off", MacosOptionAsAlt::Off),
            ("both", MacosOptionAsAlt::Both),
        ] {
            assert_eq!(
                parse(&format!("[terminal]\nmacos_option_as_alt = '{value}'"))
                    .unwrap()
                    .terminal
                    .macos_option_as_alt,
                expected
            );
        }
        for value in ["left", "right", "yes", "BOTH"] {
            let error =
                parse(&format!("[terminal]\nmacos_option_as_alt = '{value}'"))
                    .unwrap_err()
                    .to_string();
            assert!(error.contains("off") && error.contains("both"), "{error}");
        }
    }

    #[test]
    fn option_policy_reload_rejects_invalid_values_without_fallback() {
        let directory = test_directory();
        fs::create_dir(&directory).unwrap();
        let path = directory.join("config.toml");
        fs::write(&path, "[terminal]\nmacos_option_as_alt = 'both'").unwrap();
        let working = reload(&path).unwrap();
        assert_eq!(
            working.terminal.macos_option_as_alt,
            MacosOptionAsAlt::Both
        );
        fs::write(&path, "[terminal]\nmacos_option_as_alt = 'left'").unwrap();
        assert!(reload(&path).is_err());
        assert_eq!(
            working.terminal.macos_option_as_alt,
            MacosOptionAsAlt::Both
        );
        fs::write(&path, "[terminal]\nmacos_option_as_alt = 'off'").unwrap();
        assert_eq!(
            reload(&path).unwrap().terminal.macos_option_as_alt,
            MacosOptionAsAlt::Off
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn terminal_close_on_exit_defaults_true_and_requires_a_boolean() {
        assert!(Config::default().terminal.close_on_exit);
        assert!(parse("").unwrap().terminal.close_on_exit);
        assert!(parse(DEFAULT_CONFIG).unwrap().terminal.close_on_exit);
        assert!(
            !parse("[terminal]\nclose_on_exit = false")
                .unwrap()
                .terminal
                .close_on_exit
        );
        for value in ["0", "\"false\"", "[]"] {
            assert!(
                parse(&format!("[terminal]\nclose_on_exit = {value}")).is_err()
            );
        }
        assert!(parse("[terminal]\nclose_on_exiit = false").is_err());
        for (name, engine) in [
            ("alacritty", TerminalEngineKind::Alacritty),
            ("ghostty", TerminalEngineKind::Ghostty),
        ] {
            let config = parse(&format!(
                "[terminal]\nengine = '{name}'\nclose_on_exit = false"
            ))
            .unwrap();
            assert_eq!(config.engine, engine);
            assert!(!config.terminal.close_on_exit);
        }
    }

    #[test]
    fn both_engines_are_available_and_alacritty_is_default() {
        assert_eq!(
            parse("[terminal]\nengine = 'alacritty'").unwrap().engine,
            TerminalEngineKind::Alacritty
        );
        assert!(matches!(
            parse("[terminal]\nengine = 'unknown'"),
            Err(ConfigError::Engine(_))
        ));
        assert_eq!(parse("").unwrap().engine, TerminalEngineKind::Alacritty);
        assert_eq!(
            parse("[terminal]\nengine = 'ghostty'").unwrap().engine,
            TerminalEngineKind::Ghostty
        );
    }

    #[test]
    fn config_fallback_preserves_only_a_valid_engine_choice() {
        let directory = test_directory();
        fs::create_dir(&directory).unwrap();
        let file = directory.join("config.toml");
        for source in [
            "[terminal]\nengine = 'unknown'",
            "[terminal]\nengine = 12",
            "[terminal]\nengine = 'unknown'\n[font]\nsize =",
            "[terminal]\nengine = 'ghostty'\n[font]\nsize =",
            "[font]\nsize =",
        ] {
            fs::write(&file, source).unwrap();
            let loaded = load_path(file.clone());
            assert!(loaded.fatal, "{source}");
            assert!(loaded.error.is_some());
        }
        for (source, expected, fatal) in [
            (
                "[terminal]\nengine = 'alacritty'\n[font]\nsize = 'bad'",
                TerminalEngineKind::Alacritty,
                false,
            ),
            (
                "[terminal]\nengine = 'ghostty'\n[font]\nsize = 'bad'",
                TerminalEngineKind::Ghostty,
                false,
            ),
            ("[font]\nsize = 'bad'", TerminalEngineKind::Alacritty, false),
        ] {
            fs::write(&file, source).unwrap();
            let loaded = load_path(file.clone());
            assert_eq!(loaded.fatal, fatal, "{source}");
            assert!(loaded.error.is_some());
            if !fatal {
                assert_eq!(loaded.config.engine, expected);
                assert_eq!(loaded.config.font, Config::default().font);
            }
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn tab_placement_accepts_four_edges_and_rejects_other_values() {
        for (name, expected) in [
            ("top", TabPosition::Top),
            ("bottom", TabPosition::Bottom),
            ("left", TabPosition::Left),
            ("right", TabPosition::Right),
        ] {
            let config = parse(&DEFAULT_CONFIG.replace(
                "tab_position = \"top\"",
                &format!("tab_position = \"{name}\""),
            ))
            .unwrap();
            assert_eq!(config.window.tab_position, expected);
        }
        assert!(
            parse(&DEFAULT_CONFIG.replace(
                "tab_position = \"top\"",
                "tab_position = \"middle\""
            ))
            .is_err()
        );
    }

    #[test]
    fn named_themes_layer_individual_colors_and_legacy_ansi() {
        let theme = parse(
            r##"
[theme]
name = "mine"
ansi_red = "#010203"
[themes.mine]
extends = "catppuccin-mocha"
background = "#040506"
"##,
        )
        .unwrap()
        .theme;
        assert_eq!(theme.background, rgb(0x0004_0506));
        assert_eq!(theme.ansi[1], rgb(0x0001_0203));
        assert_eq!(theme.ansi[2], rgb(0x00a6_e3a1));
        assert_eq!(theme.selection_foreground, Some(rgb(0x001e_1e2e)));
        let legacy = format!(
            "[theme]\nansi = [{}]\nansi_red = '#abcdef'",
            ["'#123456'"; 16].join(",")
        );
        let theme = parse(&legacy).unwrap().theme;
        assert_eq!(theme.ansi[0], rgb(0x0012_3456));
        assert_eq!(theme.ansi[1], rgb(0x00ab_cdef));
        assert!(parse("[theme]\nansi = ['#123456']").is_err());
    }

    #[test]
    fn every_builtin_resolves_and_default_stays_unchanged() {
        assert_eq!(parse("").unwrap(), Config::default());
        assert_eq!(
            parse("[theme]\nname = 'huterm-dark'").unwrap().theme,
            Theme::default()
        );
        for name in [
            "tokyo-night",
            "tokyo-night-storm",
            "tokyo-night-moon",
            "tokyo-night-day",
            "catppuccin-latte",
            "catppuccin-frappe",
            "catppuccin-macchiato",
            "catppuccin-mocha",
            "one-dark-pro",
            "tomorrow-night",
            "dracula",
            "nord",
            "tango-with-monokai",
        ] {
            let config = parse(&format!("[theme]\nname = '{name}'")).unwrap();
            assert_ne!(
                config.theme.background, config.theme.foreground,
                "{name}"
            );
        }
        assert_eq!(
            parse("[theme]\nname = 'tango-with-monokai'")
                .unwrap()
                .theme
                .ansi[1],
            rgb(0x00cf_5041)
        );
    }

    #[test]
    fn one_dark_pro_uses_its_terminal_palette_and_opaque_selection() {
        let theme = parse("[theme]\nname = 'one-dark-pro'").unwrap().theme;
        assert_eq!(theme.background, rgb(0x0028_2c34));
        assert_eq!(theme.foreground, rgb(0x00ab_b2bf));
        assert_eq!(theme.cursor, rgb(0x0052_8bff));
        assert_eq!(theme.selection, rgb(0x0041_454e));
        assert_eq!(theme.ansi[1], rgb(0x00e0_5561));
        assert_eq!(theme.ansi[9], rgb(0x00ff_616e));
        assert_eq!(theme.selection_foreground, None);
    }

    #[test]
    fn tomorrow_night_matches_upstream_iterm_without_changing_huterm_dark() {
        let theme = parse("[theme]\nname = 'tomorrow-night'").unwrap().theme;
        assert_eq!(theme.background, rgb(0x001d_1f21));
        assert_eq!(theme.foreground, rgb(0x00c5_c8c6));
        assert_eq!(theme.cursor, theme.foreground);
        assert_eq!(theme.selection, rgb(0x0037_3b41));
        assert_eq!(theme.selection_foreground, Some(theme.foreground));
        let ansi = [
            0x0000_0000,
            0x00cc_6666,
            0x00b5_bd68,
            0x00f0_c674,
            0x0081_a2be,
            0x00b2_94bb,
            0x008a_beb7,
            0x00ff_ffff,
        ]
        .map(rgb);
        assert_eq!(theme.ansi[..8], ansi);
        assert_eq!(theme.ansi[8..], ansi);
        let default = parse("[theme]\nname = 'huterm-dark'").unwrap().theme;
        assert_eq!(default, Theme::default());
        assert_ne!(theme, default);
    }

    #[test]
    fn themes_reject_cycles_unknown_keys_colors_and_path_names() {
        for source in [
            "[theme]\nname = '../escape'",
            "[theme]\nname = 'absent'",
            "[theme]\nansi_pink = '#123456'",
            "[theme]\nansi_red = '#12345z'",
            "[theme]\nextends = 'nord'",
            "[theme]\nname = 'a'\n[themes.a]\nextends = 'b'\n[themes.b]\nextends = 'a'",
            "[themes.unused]\nansi_red = 'invalid'",
            "[themes.unused]\nname = 'nord'",
        ] {
            assert!(parse(source).is_err(), "{source}");
        }
    }

    #[test]
    fn theme_lookup_prefers_inline_then_adjacent_file_then_builtin() {
        let directory = test_directory();
        fs::create_dir_all(directory.join("themes")).unwrap();
        let path = directory.join("config.toml");
        fs::write(
            directory.join("themes/nord.toml"),
            "[theme]\nbackground = '#010203'",
        )
        .unwrap();
        let source = "[theme]\nname = 'nord'";
        assert_eq!(
            parse_at(source, &path).unwrap().theme.background,
            rgb(0x0001_0203)
        );
        let inline = format!("{source}\n[themes.nord]\nforeground = '#040506'");
        let theme = parse_at(&inline, &path).unwrap().theme;
        assert_eq!(theme.foreground, rgb(0x0004_0506));
        assert_eq!(theme.background, Theme::default().background);
        fs::write(
            directory.join("themes/nord.toml"),
            "[theme]\nansi_red = 'bad'",
        )
        .unwrap();
        assert!(parse_at(source, &path).is_err());
        fs::remove_file(directory.join("themes/nord.toml")).unwrap();
        assert_eq!(
            parse_at(source, &path).unwrap().theme.background,
            rgb(0x002e_3440)
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reload_rereads_theme_files_and_never_falls_back_on_errors() {
        let directory = test_directory();
        fs::create_dir_all(directory.join("themes")).unwrap();
        let path = directory.join("config.toml");
        let theme_path = directory.join("themes/custom.toml");
        fs::write(&path, "[theme]\nname = 'custom'").unwrap();
        fs::write(&theme_path, "[theme]\nextends = 'nord'").unwrap();
        let first = reload(&path).unwrap();
        fs::write(&theme_path, "[theme]\nextends = 'dracula'").unwrap();
        assert_ne!(reload(&path).unwrap().theme, first.theme);
        fs::write(&theme_path, "[theme]\nforeground = 'bad'").unwrap();
        assert!(reload(&path).is_err());
        fs::remove_file(&path).unwrap();
        assert!(reload(&path).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn default_document_should_parse() {
        let parsed = parse(&default_document()).expect("default should parse");
        assert_eq!(parsed.font.family, Config::default().font.family);
        assert_eq!(parsed.window, WindowConfig::default());
    }

    #[test]
    fn window_settings_are_optional_and_individually_defaulted() {
        let legacy =
            DEFAULT_CONFIG.split("[window]").next().unwrap().to_owned()
                + "[theme]"
                + DEFAULT_CONFIG.split("[theme]").nth(1).unwrap();
        assert_eq!(parse(&legacy).unwrap().window, WindowConfig::default());
        let balanced =
            parse(&(legacy + "\n[window]\npadding_balance = true\n"))
                .unwrap()
                .window;
        assert_eq!(
            balanced,
            WindowConfig {
                padding_balance: true,
                ..WindowConfig::default()
            }
        );
    }

    #[test]
    fn window_padding_rejects_invalid_values_and_accepts_zero() {
        for key in ["padding_x", "padding_y"] {
            for value in ["-1.0", "257.0", "nan", "inf"] {
                let source = DEFAULT_CONFIG.replace(
                    &format!("{key} = 4.0"),
                    &format!("{key} = {value}"),
                );
                assert!(parse(&source).is_err(), "accepted {key} = {value}");
            }
        }
        let config = parse(&DEFAULT_CONFIG.replace("= 4.0", "= 0.0")).unwrap();
        assert_eq!(
            config.window,
            WindowConfig {
                padding_x: 0.0,
                padding_y: 0.0,
                padding_balance: false,
                tab_position: TabPosition::Top,
                always_show_tab_bar: false,
                auto_hide_tab_bar_in_fullscreen: false,
                macos_fullscreen_mode: MacosFullscreenMode::NonNative,
            }
        );
    }

    #[test]
    fn keybinding_entries_are_structurally_validated() {
        let source = format!(
            "{DEFAULT_CONFIG}\n[[keybinding]]\nkey = \"cmd-1\"\ncommand = \
             \"select_tab\"\nargs = {{ index = 1 }}\nwhen = \"Terminal\"\n\
             description = \"First tab\"\n[[keybinding]]\nkey = \"cmd-w\"\n\
             command = \"unbind\"\n"
        );
        let config = parse(&source).unwrap();
        assert_eq!(config.keybindings.len(), 2);
        let first = &config.keybindings[0];
        assert_eq!(first.key, "cmd-1");
        assert_eq!(first.command, "select_tab");
        assert_eq!(
            first.args.as_ref().and_then(|args| args.get("index")),
            Some(&toml::Value::Integer(1))
        );
        assert_eq!(first.when.as_deref(), Some("Terminal"));
        assert_eq!(first.description.as_deref(), Some("First tab"));
        assert_eq!(config.keybindings[1].args, None);
        assert!(parse(DEFAULT_CONFIG).unwrap().keybindings.is_empty());

        let entry = |body: &str| {
            format!(
                "{DEFAULT_CONFIG}\n[[keybinding]]\nkey = \"cmd-1\"\n{body}\n"
            )
        };
        let error = parse(&entry("command = \"select_tab\"\nargs = [1]"))
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("keybinding 1 (\"cmd-1\"): "), "{error}");
        assert!(error.contains("named arguments index"), "{error}");
        let error = parse(&entry("command = \"select_tab\"\nargs = 1"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("not integer"), "{error}");
        let error =
            parse(&entry("command = \"select_tab\"\ndescription = \"  \""))
                .unwrap_err()
                .to_string();
        assert!(error.contains("description must not be blank"), "{error}");
        assert!(
            parse(&entry("command = \"select_tab\"\nkeys = \"x\"")).is_err()
        );
    }

    #[test]
    fn parser_should_reject_unknown_and_invalid_values() {
        assert!(
            parse(&DEFAULT_CONFIG.replace("size = 14.0", "size = 5.0"))
                .is_err()
        );
        assert!(
            parse(&DEFAULT_CONFIG.replace("[font]", "[font]\nextra = true"))
                .is_err()
        );
        assert!(parse("[theme]\nforeground = 'red'").is_err());
    }

    #[test]
    fn config_path_prefers_explicit_override_then_xdg() {
        let override_path = config_path_from(|name| match name {
            "HUTERM_CONFIG_FILE" => Some("/tmp/huterm-test.toml".into()),
            "XDG_CONFIG_HOME" => Some("/tmp/ignored".into()),
            "HOME" => Some("/tmp/home".into()),
            _ => None,
        });
        let xdg_path = config_path_from(|name| match name {
            "XDG_CONFIG_HOME" => Some("/tmp/xdg".into()),
            "HOME" => Some("/tmp/home".into()),
            _ => None,
        });

        assert_eq!(override_path, PathBuf::from("/tmp/huterm-test.toml"));
        assert_eq!(xdg_path, PathBuf::from("/tmp/xdg/huterm/config.toml"));
    }

    #[test]
    fn loading_missing_and_malformed_files_falls_back_with_context() {
        let directory = test_directory();
        let path = directory.join("config.toml");
        let missing = load_path(path.clone());
        assert_eq!(missing.config, Config::default());
        assert!(missing.error.is_none());

        fs::create_dir_all(&directory).expect("test directory should exist");
        fs::write(&path, "not valid toml = [")
            .expect("malformed config should be written");
        let malformed = load_path(path.clone());
        assert_eq!(malformed.config, Config::default());
        assert!(malformed.error.is_some_and(|error| {
            error.contains(path.to_string_lossy().as_ref())
        }));
        fs::remove_dir_all(directory).expect("test directory should clean up");
    }

    #[test]
    fn default_creation_never_overwrites_an_existing_file() {
        let directory = test_directory();
        let path = directory.join("nested/config.toml");
        create_default(&path).expect("default config should be created");
        assert_eq!(
            fs::read_to_string(&path)
                .expect("default config should be readable"),
            default_document()
        );

        fs::write(&path, "user-owned = true\n")
            .expect("test config should be replaceable");
        create_default(&path).expect("existing config should remain usable");
        assert_eq!(
            fs::read_to_string(&path)
                .expect("existing config should be readable"),
            "user-owned = true\n"
        );
        fs::remove_dir_all(directory).expect("test directory should clean up");
    }

    #[test]
    fn indexed_palette_should_cover_cube_and_grayscale() {
        let theme = Theme::default();
        assert_eq!(theme.indexed(16), rgb(0x0000_0000));
        assert_eq!(theme.indexed(231), rgb(0x00ff_ffff));
        assert_eq!(theme.indexed(232), rgb(0x0008_0808));
        assert_eq!(theme.indexed(255), rgb(0x00ee_eeee));
    }

    fn test_directory() -> PathBuf {
        let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "huterm-config-test-{}-{sequence}",
            std::process::id()
        ))
    }
}

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
