use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use huterm_config::{ThemeDefinition, ThemeFile};

use crate::config::{ConfigError, Consumed, Theme};

/// The theme an empty `[theme]` selects.
pub(super) const DEFAULT: &str = "tokyo-night";

/// The themes the selected `[theme]` resolves to while the system is in
/// light and in dark mode. Without `light` or `dark` both are the same.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ThemePair {
    pub(crate) light: Theme,
    pub(crate) dark: Theme,
}

impl ThemePair {
    /// The theme for a `dark` or light system appearance.
    pub(crate) fn get(&self, dark: bool) -> &Theme {
        if dark { &self.dark } else { &self.light }
    }
}

/// Resolves the selected theme for both system appearances, recording
/// every external theme file it reads or probes in `consumed`. `light`
/// and `dark` fall back to `name`, and `name` to [`DEFAULT`]; the color
/// overrides apply to both.
pub(super) fn resolve(
    selected: &ThemeDefinition,
    inline: &BTreeMap<String, ThemeDefinition>,
    directory: &Path,
    consumed: &mut Consumed,
) -> Result<ThemePair, ConfigError> {
    if selected.extends.is_some() {
        return Err(ConfigError::Invalid("use name in [theme], not extends"));
    }
    // Validate unused inline definitions too, so typos do not remain dormant.
    for (name, definition) in inline {
        validate_name(name)?;
        validate_definition(definition)?;
    }
    let name = selected.name.as_deref().unwrap_or(DEFAULT);
    let light = selected.light.as_deref().unwrap_or(name);
    let dark = selected.dark.as_deref().unwrap_or(name);
    let overrides = ThemeDefinition {
        name: None,
        light: None,
        dark: None,
        ..selected.clone()
    };
    let mut resolve = |name: &str| {
        let base =
            resolve_named(name, inline, directory, consumed, &mut Vec::new())?;
        overrides.apply(base)
    };
    let light_theme = resolve(light)?;
    let dark_theme = if dark == light {
        light_theme.clone()
    } else {
        resolve(dark)?
    };
    Ok(ThemePair {
        light: light_theme,
        dark: dark_theme,
    })
}

/// The bundled [`DEFAULT`] theme, for a configuration that sets none.
pub(super) fn default_theme() -> Theme {
    let source = bundled(DEFAULT).expect("the default theme is bundled");
    parse_file(source)
        .and_then(|definition| definition.apply(Theme::default()))
        .expect("the default theme parses")
}

fn validate_name(name: &str) -> Result<(), ConfigError> {
    if name.is_empty()
        || !name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
        })
    {
        return Err(ConfigError::Theme(format!(
            "invalid theme name {name:?}; use letters, digits, - or _"
        )));
    }
    Ok(())
}

fn validate_definition(
    definition: &ThemeDefinition,
) -> Result<(), ConfigError> {
    if definition.name.is_some()
        || definition.light.is_some()
        || definition.dark.is_some()
    {
        return Err(ConfigError::Invalid(
            "named theme definitions use extends, not name, light, or dark",
        ));
    }
    if let Some(parent) = &definition.extends {
        validate_name(parent)?;
    }
    definition.apply(Theme::default())?;
    Ok(())
}

fn resolve_named(
    name: &str,
    inline: &BTreeMap<String, ThemeDefinition>,
    directory: &Path,
    consumed: &mut Consumed,
    stack: &mut Vec<String>,
) -> Result<Theme, ConfigError> {
    validate_name(name)?;
    if stack.iter().any(|entry| entry == name) || stack.len() >= 64 {
        return Err(ConfigError::Theme(format!(
            "theme inheritance cycle or depth limit: {} -> {name}",
            stack.join(" -> ")
        )));
    }
    stack.push(name.to_owned());
    let definition = if let Some(definition) = inline.get(name) {
        definition.clone()
    } else {
        let path = directory.join(format!("{name}.toml"));
        let read = fs::read(&path);
        consumed.theme(&path, read.as_deref().ok());
        match read.and_then(|bytes| {
            String::from_utf8(bytes).map_err(|error| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, error)
            })
        }) {
            Ok(source) => parse_file(&source)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if name == "huterm-dark" {
                    return Ok(Theme::huterm_dark());
                }
                let source = bundled(name).ok_or_else(|| {
                    ConfigError::Theme(format!("unknown theme {name:?}"))
                })?;
                parse_file(source)?
            }
            Err(error) => {
                return Err(ConfigError::Theme(format!(
                    "{}: {error}",
                    path.display()
                )));
            }
        }
    };
    validate_definition(&definition)?;
    // An empty definition is huterm-dark itself.
    if definition == ThemeDefinition::default() {
        stack.pop();
        return Ok(Theme::huterm_dark());
    }
    let base = match &definition.extends {
        Some(parent) => {
            resolve_named(parent, inline, directory, consumed, stack)?
        }
        None => Theme::default(),
    };
    let theme = definition.apply(base)?;
    stack.pop();
    Ok(theme)
}

fn parse_file(source: &str) -> Result<ThemeDefinition, ConfigError> {
    toml::from_str::<ThemeFile>(source)
        .map(|file| file.theme)
        .map_err(ConfigError::Toml)
}

/// The themes Huterm ships, besides the built-in `huterm-dark`.
const BUNDLED: [&str; 31] = [
    "ayu-dark",
    "ayu-light",
    "ayu-mirage",
    "catppuccin-frappe",
    "catppuccin-latte",
    "catppuccin-macchiato",
    "catppuccin-mocha",
    "dracula",
    "everforest-dark",
    "everforest-light",
    "github-dark",
    "github-light",
    "gruvbox-dark",
    "gruvbox-light",
    "kanagawa-dragon",
    "kanagawa-lotus",
    "kanagawa-wave",
    "nord",
    "one-dark-pro",
    "one-light",
    "rose-pine",
    "rose-pine-dawn",
    "rose-pine-moon",
    "solarized-dark",
    "solarized-light",
    "tango-with-monokai",
    "tokyo-night",
    "tokyo-night-day",
    "tokyo-night-moon",
    "tokyo-night-storm",
    "tomorrow-night",
];

/// The theme names `[theme] name` can select: `huterm-dark`, the bundled
/// themes, and the files in `directory`, sorted without repeats.
pub(super) fn names(directory: Option<&Path>) -> Vec<String> {
    let mut names: Vec<String> = std::iter::once("huterm-dark")
        .chain(BUNDLED)
        .map(str::to_owned)
        .collect();
    if let Some(entries) = directory.and_then(|path| fs::read_dir(path).ok()) {
        // Only stems that are valid theme names can be selected.
        names.extend(entries.filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "toml" {
                return None;
            }
            let stem = path.file_stem()?.to_str()?;
            validate_name(stem).is_ok().then(|| stem.to_owned())
        }));
    }
    names.sort();
    names.dedup();
    names
}

/// A named theme's colors for a preview, or `None` when it does not
/// resolve. Reads only the theme's own files.
pub(super) fn preview(name: &str, directory: &Path) -> Option<Theme> {
    resolve_named(
        name,
        &BTreeMap::new(),
        directory,
        &mut Consumed::new(),
        &mut Vec::new(),
    )
    .ok()
}

/// A bundled theme's source, embedded in the binary.
fn bundled(name: &str) -> Option<&'static str> {
    Some(match name {
        "ayu-dark" => include_str!("../themes/ayu-dark.toml"),
        "ayu-light" => include_str!("../themes/ayu-light.toml"),
        "ayu-mirage" => include_str!("../themes/ayu-mirage.toml"),
        "catppuccin-frappe" => include_str!("../themes/catppuccin-frappe.toml"),
        "catppuccin-latte" => include_str!("../themes/catppuccin-latte.toml"),
        "catppuccin-macchiato" => {
            include_str!("../themes/catppuccin-macchiato.toml")
        }
        "catppuccin-mocha" => include_str!("../themes/catppuccin-mocha.toml"),
        "dracula" => include_str!("../themes/dracula.toml"),
        "everforest-dark" => include_str!("../themes/everforest-dark.toml"),
        "everforest-light" => include_str!("../themes/everforest-light.toml"),
        "github-dark" => include_str!("../themes/github-dark.toml"),
        "github-light" => include_str!("../themes/github-light.toml"),
        "gruvbox-dark" => include_str!("../themes/gruvbox-dark.toml"),
        "gruvbox-light" => include_str!("../themes/gruvbox-light.toml"),
        "kanagawa-dragon" => include_str!("../themes/kanagawa-dragon.toml"),
        "kanagawa-lotus" => include_str!("../themes/kanagawa-lotus.toml"),
        "kanagawa-wave" => include_str!("../themes/kanagawa-wave.toml"),
        "nord" => include_str!("../themes/nord.toml"),
        "one-dark-pro" => include_str!("../themes/one-dark-pro.toml"),
        "one-light" => include_str!("../themes/one-light.toml"),
        "rose-pine" => include_str!("../themes/rose-pine.toml"),
        "rose-pine-dawn" => include_str!("../themes/rose-pine-dawn.toml"),
        "rose-pine-moon" => include_str!("../themes/rose-pine-moon.toml"),
        "solarized-dark" => include_str!("../themes/solarized-dark.toml"),
        "solarized-light" => include_str!("../themes/solarized-light.toml"),
        "tango-with-monokai" => {
            include_str!("../themes/tango-with-monokai.toml")
        }
        "tokyo-night" => include_str!("../themes/tokyo-night.toml"),
        "tokyo-night-day" => include_str!("../themes/tokyo-night-day.toml"),
        "tokyo-night-moon" => include_str!("../themes/tokyo-night-moon.toml"),
        "tokyo-night-storm" => include_str!("../themes/tokyo-night-storm.toml"),
        "tomorrow-night" => include_str!("../themes/tomorrow-night.toml"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_theme_is_registered_and_sets_every_ui_color() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("themes");
        let mut count = 0;
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|extension| extension != "toml") {
                continue;
            }
            let name = path.file_stem().unwrap().to_str().unwrap();
            let source = bundled(name)
                .unwrap_or_else(|| panic!("{name} is not in bundled()"));
            assert!(BUNDLED.contains(&name), "{name} is not in BUNDLED");
            let definition = parse_file(source).unwrap();
            for (key, value) in [
                ("tab_bar_background", &definition.tab_bar_background),
                ("tab_active_background", &definition.tab_active_background),
                ("tab_foreground", &definition.tab_foreground),
                (
                    "tab_inactive_foreground",
                    &definition.tab_inactive_foreground,
                ),
                ("tab_border", &definition.tab_border),
                ("tab_accent", &definition.tab_accent),
            ] {
                assert!(value.is_some(), "{name} does not set {key}");
            }
            count += 1;
        }
        assert_eq!(count, BUNDLED.len());
        let default = Theme::huterm_dark();
        assert!(
            [
                default.tab_bar_background,
                default.tab_active_background,
                default.tab_foreground,
                default.tab_inactive_foreground,
                default.tab_border,
                default.tab_accent,
            ]
            .iter()
            .all(Option::is_some),
            "huterm-dark must set every UI color"
        );
    }

    #[test]
    fn names_list_only_theme_files_that_can_be_selected() {
        let directory = std::env::temp_dir()
            .join(format!("huterm-theme-names-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        for file in ["mine.toml", "my theme.toml", "notes.txt"] {
            fs::write(directory.join(file), "").unwrap();
        }
        let names = names(Some(&directory));
        fs::remove_dir_all(&directory).unwrap();
        assert!(names.iter().any(|name| name == "mine"));
        assert!(
            !names
                .iter()
                .any(|name| name == "my theme" || name == "notes")
        );
    }
}
