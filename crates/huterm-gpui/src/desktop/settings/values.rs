//! What a setting holds in `config.toml`, what Settings shows for it, and
//! the patch a change writes, without GPUI.

use huterm_config::RawConfig;
use huterm_config::document::Patch;
use huterm_config::settings::{Setting, SettingKind};
use toml::{Table, Value};

/// The values one snapshot of the file sets.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FileValues(Table);

impl FileValues {
    /// Reads a snapshot's text; a missing file sets nothing.
    ///
    /// # Errors
    /// Returns the parser's message, with the line and column at fault,
    /// when the text is not TOML.
    pub(crate) fn parse(text: Option<&str>) -> Result<Self, String> {
        match text {
            None => Ok(Self::default()),
            Some(text) => text
                .parse::<Table>()
                .map(Self)
                .map_err(|error| error.to_string()),
        }
    }

    pub(crate) fn get(&self, path: &str) -> Option<&Value> {
        lookup(&self.0, path)
    }

    pub(crate) fn is_set(&self, path: &str) -> bool {
        self.get(path).is_some()
    }
}

/// This platform's defaults, which also cover settings whose schema
/// default differs by platform.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Defaults(Table);

impl Defaults {
    pub(crate) fn current() -> Self {
        let defaults: RawConfig =
            toml::from_str("").expect("an empty file holds the defaults");
        Self(
            Table::try_from(defaults)
                .expect("the default configuration serializes"),
        )
    }

    pub(crate) fn get(&self, path: &str) -> Option<&Value> {
        lookup(&self.0, path)
    }
}

fn lookup<'a>(table: &'a Table, path: &str) -> Option<&'a Value> {
    let mut parts = path.split('.');
    let mut value = table.get(parts.next()?)?;
    for part in parts {
        value = value.as_table()?.get(part)?;
    }
    Some(value)
}

/// What a setting's control shows: the file's value, else the default.
/// `None` for an optional setting the file leaves unset.
pub(crate) fn shown(
    setting: &Setting,
    file: &FileValues,
    defaults: &Defaults,
) -> Option<Value> {
    file.get(&setting.path)
        .or_else(|| defaults.get(&setting.path))
        .or(setting.default.as_ref())
        .cloned()
}

/// Checks a value a control produced against the setting's kind. Patterns
/// and rules spanning settings are left to the validator the patch service
/// runs.
///
/// # Errors
/// Returns the message shown under the row.
pub(crate) fn check(setting: &Setting, value: &Value) -> Result<(), String> {
    let unit = setting
        .unit
        .as_deref()
        .map_or(String::new(), |unit| format!(" {unit}"));
    match (&setting.kind, value) {
        (SettingKind::Bool, Value::Boolean(_))
        | (SettingKind::Text { .. }, Value::String(_)) => Ok(()),
        (SettingKind::Choice(choices), Value::String(value)) => {
            if choices.iter().any(|choice| &choice.value == value) {
                Ok(())
            } else {
                Err(format!("{value} is not an option"))
            }
        }
        (SettingKind::Integer { min, max }, Value::Integer(value)) => {
            if let Some(min) = min
                && value < min
            {
                Err(format!("Must be at least {min}{unit}"))
            } else if let Some(max) = max
                && value > max
            {
                Err(format!("Must be at most {max}{unit}"))
            } else {
                Ok(())
            }
        }
        (
            SettingKind::Number {
                min,
                max,
                exclusive_min,
            },
            Value::Float(value),
        ) => {
            if let Some(min) = min
                && value < min
            {
                Err(format!("Must be at least {}{unit}", number(*min)))
            } else if let Some(min) = exclusive_min
                && value <= min
            {
                Err(format!("Must be more than {}{unit}", number(*min)))
            } else if let Some(max) = max
                && value > max
            {
                Err(format!("Must be at most {}{unit}", number(*max)))
            } else if value.is_finite() {
                Ok(())
            } else {
                Err("Must be a number".into())
            }
        }
        _ => Err(format!("{value} does not fit this setting")),
    }
}

/// Reads a number field's text as the value its setting stores: an integer
/// for `Integer`, a float for `Number`. An empty field is `None`.
///
/// # Errors
/// Returns the message shown under the row.
pub(crate) fn parse_number(
    setting: &Setting,
    text: &str,
) -> Result<Option<Value>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let value = match setting.kind {
        SettingKind::Integer { .. } => text
            .parse::<i64>()
            .map(Value::Integer)
            .map_err(|_| "Must be a whole number".to_owned())?,
        SettingKind::Number { .. } => text
            .parse::<f64>()
            .map(Value::Float)
            .map_err(|_| "Must be a number".to_owned())?,
        _ => return Err("Not a number setting".into()),
    };
    check(setting, &value).map(|()| Some(value))
}

/// A number as a field shows it: `14` rather than `14.0`.
pub(crate) fn number(value: f64) -> String {
    let text = value.to_string();
    text.strip_suffix(".0").map_or(text.clone(), str::to_owned)
}

/// The patch that stores `value`, or resets the setting when `None`.
pub(crate) fn patch(setting: &Setting, value: Option<Value>) -> Patch {
    patch_at(&setting.path, value)
}

/// The patch that stores `value` at a dotted path inside a setting, such as
/// `theme.name`, or removes it when `None`.
pub(crate) fn patch_at(path: &str, value: Option<Value>) -> Patch {
    let path = path.split('.').map(str::to_owned).collect();
    match value {
        Some(value) => Patch::Set { path, value },
        None => Patch::Remove { path },
    }
}

#[cfg(test)]
mod tests {
    use huterm_config::settings::SettingsModel;

    use super::*;

    fn setting(path: &str) -> &'static Setting {
        SettingsModel::load().unwrap().setting(path).unwrap()
    }

    #[test]
    fn a_control_shows_the_file_value_else_the_platform_default() {
        let file = FileValues::parse(Some(
            "[font]\nsize = 13.0 # small\n[tabs]\nstyle = \"strip\"\n",
        ))
        .unwrap();
        let defaults = Defaults::current();
        assert!(file.is_set("font.size"));
        assert!(!file.is_set("font.family"));
        assert_eq!(
            shown(setting("font.size"), &file, &defaults),
            Some(Value::Float(13.0))
        );
        let family = if cfg!(target_os = "macos") {
            "Menlo"
        } else {
            "monospace"
        };
        assert_eq!(
            shown(setting("font.family"), &file, &defaults),
            Some(Value::String(family.into()))
        );
        assert_eq!(
            shown(setting("terminal.bell.visual"), &file, &defaults),
            Some(Value::Boolean(true))
        );
        assert_eq!(
            shown(setting("updates.check_interval_hours"), &file, &defaults),
            None
        );
        assert_eq!(FileValues::parse(None), Ok(FileValues::default()));
        let error = FileValues::parse(Some("[font]\nsize 13.0\n")).unwrap_err();
        assert!(error.contains("line 2"), "{error}");
    }

    #[test]
    fn inputs_are_checked_against_their_kind_before_writing() {
        let size = setting("font.size");
        assert_eq!(parse_number(size, " 14 "), Ok(Some(Value::Float(14.0))));
        assert_eq!(
            parse_number(size, "200"),
            Err("Must be at most 96 pt".into())
        );
        assert_eq!(parse_number(size, "big"), Err("Must be a number".into()));
        assert_eq!(parse_number(size, ""), Ok(None));
        let rows = setting("terminal.scrollback_rows");
        assert_eq!(parse_number(rows, "2000"), Ok(Some(Value::Integer(2000))));
        assert_eq!(
            parse_number(rows, "1.5"),
            Err("Must be a whole number".into())
        );
        assert_eq!(
            parse_number(rows, "-1"),
            Err("Must be at least 0 rows".into())
        );
        assert_eq!(
            check(setting("tabs.style"), &Value::String("round".into())),
            Err("round is not an option".into())
        );
        assert_eq!(
            check(setting("tabs.style"), &Value::String("strip".into())),
            Ok(())
        );
        assert!(
            check(setting("tabs.always_show"), &Value::Integer(1)).is_err()
        );
        assert_eq!(number(14.0), "14");
        assert_eq!(number(0.5), "0.5");
    }

    #[test]
    fn a_change_patches_only_its_path_and_a_reset_removes_it() {
        let size = setting("font.size");
        assert_eq!(
            patch(size, Some(Value::Float(15.0))),
            Patch::Set {
                path: vec!["font".into(), "size".into()],
                value: Value::Float(15.0),
            }
        );
        assert_eq!(
            patch(setting("terminal.bell.visual"), None),
            Patch::Remove {
                path: vec!["terminal".into(), "bell".into(), "visual".into()],
            }
        );
    }
}
