use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use huterm_protocol::Rgb;
use serde::Deserialize;

pub(super) const DEFAULT_CONFIG: &str = r##"[font]
family = "Menlo"
size = 14.0

[theme]
foreground = "#c5c8c6"
background = "#1d1f21"
cursor = "#ffffff"
selection = "#264f78"
ansi = [
  "#1d1f21", "#cc6666", "#b5bd68", "#f0c674",
  "#81a2be", "#b294bb", "#8abeb7", "#c5c8c6",
  "#666666", "#d54e53", "#b9ca4a", "#e7c547",
  "#7aa6da", "#c397d8", "#70c0b1", "#eaeaea",
]
"##;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Config {
    pub(super) font: FontConfig,
    pub(super) theme: Theme,
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
        Ok(source) => match parse(&source) {
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

fn parse(source: &str) -> Result<Config, ConfigError> {
    let raw: RawConfig = toml::from_str(source).map_err(ConfigError::Toml)?;
    if raw.font.family.trim().is_empty() {
        return Err(ConfigError::Invalid("font.family must not be empty"));
    }
    if !raw.font.size.is_finite() || !(6.0..=96.0).contains(&raw.font.size) {
        return Err(ConfigError::Invalid("font.size must be between 6 and 96"));
    }
    if raw.theme.ansi.len() != 16 {
        return Err(ConfigError::Invalid("theme.ansi must contain 16 colors"));
    }
    let ansi: Vec<Rgb> = raw
        .theme
        .ansi
        .iter()
        .map(|value| parse_color(value))
        .collect::<Result<_, _>>()?;
    let ansi: [Rgb; 16] = ansi.try_into().map_err(|_| {
        ConfigError::Invalid("theme.ansi must contain 16 colors")
    })?;
    Ok(Config {
        font: FontConfig {
            family: raw.font.family,
            size: raw.font.size,
        },
        theme: Theme {
            foreground: parse_color(&raw.theme.foreground)?,
            background: parse_color(&raw.theme.background)?,
            cursor: parse_color(&raw.theme.cursor)?,
            selection: parse_color(&raw.theme.selection)?,
            ansi,
        },
    })
}

fn parse_color(value: &str) -> Result<Rgb, ConfigError> {
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
    font: RawFont,
    theme: RawTheme,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFont {
    family: String,
    size: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTheme {
    foreground: String,
    background: String,
    cursor: String,
    selection: String,
    ansi: Vec<String>,
}

#[derive(Debug)]
enum ConfigError {
    Toml(toml::de::Error),
    Color(String),
    Invalid(&'static str),
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

    #[test]
    fn default_document_should_parse() {
        let parsed = parse(&default_document()).expect("default should parse");
        assert_eq!(parsed.font.family, Config::default().font.family);
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
        assert!(parse(&DEFAULT_CONFIG.replacen("#c5c8c6", "red", 1)).is_err());
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
