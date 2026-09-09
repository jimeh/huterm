use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use huterm_config::{ThemeDefinition, ThemeFile};

use crate::config::{ConfigError, Theme};

pub(super) fn resolve(
    selected: &ThemeDefinition,
    inline: &BTreeMap<String, ThemeDefinition>,
    directory: &Path,
) -> Result<Theme, ConfigError> {
    if selected.extends.is_some() {
        return Err(ConfigError::Invalid("use name in [theme], not extends"));
    }
    // Validate unused inline definitions too, so typos do not remain dormant.
    for (name, definition) in inline {
        validate_name(name)?;
        validate_definition(definition)?;
    }
    let base = match &selected.name {
        Some(name) => resolve_named(name, inline, directory, &mut Vec::new())?,
        None => Theme::default(),
    };
    selected.apply(base)
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
    if definition.name.is_some() {
        return Err(ConfigError::Invalid(
            "named theme definitions use extends, not name",
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
        match fs::read_to_string(&path) {
            Ok(source) => parse_file(&source)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if name == "huterm-dark" {
                    return Ok(Theme::default());
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
    let base = match &definition.extends {
        Some(parent) => resolve_named(parent, inline, directory, stack)?,
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

fn bundled(name: &str) -> Option<&'static str> {
    match name {
        "tokyo-night" => Some(include_str!("../themes/tokyo-night.toml")),
        "tokyo-night-storm" => {
            Some(include_str!("../themes/tokyo-night-storm.toml"))
        }
        "tokyo-night-moon" => {
            Some(include_str!("../themes/tokyo-night-moon.toml"))
        }
        "tokyo-night-day" => {
            Some(include_str!("../themes/tokyo-night-day.toml"))
        }
        "catppuccin-latte" => {
            Some(include_str!("../themes/catppuccin-latte.toml"))
        }
        "catppuccin-frappe" => {
            Some(include_str!("../themes/catppuccin-frappe.toml"))
        }
        "catppuccin-macchiato" => {
            Some(include_str!("../themes/catppuccin-macchiato.toml"))
        }
        "catppuccin-mocha" => {
            Some(include_str!("../themes/catppuccin-mocha.toml"))
        }
        "dracula" => Some(include_str!("../themes/dracula.toml")),
        "nord" => Some(include_str!("../themes/nord.toml")),
        "one-dark-pro" => Some(include_str!("../themes/one-dark-pro.toml")),
        "tomorrow-night" => Some(include_str!("../themes/tomorrow-night.toml")),
        "tango-with-monokai" => {
            Some(include_str!("../themes/tango-with-monokai.toml"))
        }
        _ => None,
    }
}
