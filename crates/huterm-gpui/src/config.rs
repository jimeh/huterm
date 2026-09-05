use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::themes::{self, ThemeDefinition};
use huterm_protocol::Rgb;
use serde::Deserialize;

pub(super) const DEFAULT_CONFIG: &str = r##"[font]
family = "Menlo"
size = 14.0

[window]
# Padding in logical points on each side of the terminal.
padding_x = 4.0
padding_y = 4.0
# Split unused column space between left and right instead of only the right.
padding_balance = false
# Tab placement: top, bottom, left, or right.
tab_position = "top"

[theme]
name = "huterm-dark"
# Override individual colors without replacing the whole palette:
# background = "#1d1f21"
# ansi_red = "#cc6666"
# ansi_bright_red = "#d54e53"
"##;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Config {
    pub(super) font: FontConfig,
    pub(super) window: WindowConfig,
    pub(super) theme: Theme,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(super) struct WindowConfig {
    pub(super) padding_x: f32,
    pub(super) padding_y: f32,
    pub(super) padding_balance: bool,
    pub(super) tab_position: TabPosition,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            padding_x: 4.0,
            padding_y: 4.0,
            padding_balance: false,
            tab_position: TabPosition::Top,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum TabPosition {
    #[default]
    Top,
    Bottom,
    Left,
    Right,
}

impl TabPosition {
    pub(super) fn vertical(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct FontConfig {
    pub(super) family: String,
    pub(super) size: f32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Theme {
    pub(super) foreground: Rgb,
    pub(super) background: Rgb,
    pub(super) cursor: Rgb,
    pub(super) selection: Rgb,
    pub(super) selection_foreground: Option<Rgb>,
    pub(super) ansi: [Rgb; 16],
}

#[derive(Debug)]
pub(super) struct LoadedConfig {
    pub(super) config: Config,
    pub(super) path: PathBuf,
    pub(super) error: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        let family = if cfg!(target_os = "macos") {
            "Menlo"
        } else {
            "monospace"
        };
        Self {
            font: FontConfig {
                family: family.into(),
                size: 14.0,
            },
            theme: Theme::default(),
            window: WindowConfig::default(),
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
    pub(super) fn indexed(&self, index: u8) -> Rgb {
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
            },
            Err(error) => LoadedConfig {
                config: Config::default(),
                error: Some(format!("{}: {error}", path.display())),
                path,
            },
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            LoadedConfig {
                config: Config::default(),
                path,
                error: None,
            }
        }
        Err(error) => LoadedConfig {
            config: Config::default(),
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

fn parse_at(source: &str, path: &Path) -> Result<Config, ConfigError> {
    let raw: RawConfig = toml::from_str(source).map_err(ConfigError::Toml)?;
    if raw.font.family.trim().is_empty() {
        return Err(ConfigError::Invalid("font.family must not be empty"));
    }
    if !raw.font.size.is_finite() || !(6.0..=96.0).contains(&raw.font.size) {
        return Err(ConfigError::Invalid("font.size must be between 6 and 96"));
    }
    for padding in [raw.window.padding_x, raw.window.padding_y] {
        if !padding.is_finite() || !(0.0..=256.0).contains(&padding) {
            return Err(ConfigError::Invalid(
                "window padding must be between 0 and 256 points",
            ));
        }
    }
    let directory = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("themes");
    let theme = themes::resolve(&raw.theme, &raw.themes, &directory)?;
    Ok(Config {
        window: raw.window,
        font: FontConfig {
            family: raw.font.family,
            size: raw.font.size,
        },
        theme,
    })
}

pub(super) fn parse_color(value: &str) -> Result<Rgb, ConfigError> {
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    font: RawFont,
    #[serde(default)]
    window: WindowConfig,
    #[serde(default)]
    theme: ThemeDefinition,
    #[serde(default)]
    themes: BTreeMap<String, ThemeDefinition>,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawFont {
    family: String,
    size: f32,
}

impl Default for RawFont {
    fn default() -> Self {
        let font = Config::default().font;
        Self {
            family: font.family,
            size: font.size,
        }
    }
}

#[derive(Debug)]
pub(super) enum ConfigError {
    Toml(toml::de::Error),
    Color(String),
    Invalid(&'static str),
    Theme(String),
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
            Self::Theme(message) => formatter.write_str(message),
        }
    }
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

    fn parse(source: &str) -> Result<Config, ConfigError> {
        // Keep bundled-theme tests independent of the crate's themes directory.
        let directory = test_directory();
        fs::create_dir(&directory).expect("empty config directory");
        let result = parse_at(source, &directory.join("config.toml"));
        fs::remove_dir(directory).expect("remove empty config directory");
        result
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
            }
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
