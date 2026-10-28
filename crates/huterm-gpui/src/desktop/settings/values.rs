//! What a setting holds in `config.toml`, what Settings shows for it, and
//! the patch a change writes, without GPUI.

use huterm_config::RawConfig;
use huterm_config::document::Patch;
use huterm_config::settings::{Setting, SettingKind};
use huterm_ui::steps::Steps;
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
/// for `Integer`, a float for `Number`, clamped to the setting's range, as
/// a number field clamps what is typed. An empty field is `None`.
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
        SettingKind::Integer { min, max } => {
            text.parse::<i64>()
                .map(|value| {
                    Value::Integer(value.clamp(
                        min.unwrap_or(i64::MIN),
                        max.unwrap_or(i64::MAX),
                    ))
                })
                .map_err(|_| "Must be a whole number".to_owned())?
        }
        SettingKind::Number { .. } => text
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .zip(steps(setting))
            .map(|(value, steps)| Value::Float(steps.clamp(value)))
            .ok_or_else(|| "Must be a number".to_owned())?,
        _ => return Err("Not a number setting".into()),
    };
    check(setting, &value).map(|()| Some(value))
}

/// How many steps from zero an unbounded end of a range lies.
const UNBOUNDED_STEPS: f64 = 1_099_511_627_776.0;

/// The range and grid a number setting steps along: its schema bounds, an
/// exclusive minimum as the first grid value above it, and its
/// `x-huterm-step`, else one. A missing bound lies far enough away to never
/// be reached. A bounded range pages by about a tenth of itself, as sliders
/// conventionally do, and an unbounded one by ten steps. `None` for any
/// other kind.
pub(crate) fn steps(setting: &Setting) -> Option<Steps> {
    let step = setting.step.unwrap_or(1.0);
    let (min, max) = match &setting.kind {
        SettingKind::Integer { min, max } => {
            (min.map(integer_number), max.map(integer_number))
        }
        SettingKind::Number {
            min,
            max,
            exclusive_min,
        } => (
            (*min)
                .into_iter()
                .chain(exclusive_min.map(|bound| bound + step))
                .reduce(f64::max),
            *max,
        ),
        _ => return None,
    };
    let far = step * UNBOUNDED_STEPS;
    let steps = Steps::new(min.unwrap_or(-far), max.unwrap_or(far), step);
    Some(match (min, max) {
        (Some(min), Some(max)) => {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "a tenth of a bounded range's step count fits a u32"
            )]
            let tenth = ((max - min) / step / 10.0).round().max(1.0) as u32;
            steps.page(tenth)
        }
        _ => steps,
    })
}

/// `value` as the setting stores it: rounded to an integer for `Integer`,
/// else a float.
#[expect(
    clippy::cast_possible_truncation,
    reason = "setting values lie far inside the i64 range"
)]
pub(crate) fn number_value(setting: &Setting, value: f64) -> Value {
    match setting.kind {
        SettingKind::Integer { .. } => Value::Integer(value.round() as i64),
        _ => Value::Float(value),
    }
}

/// A stored number as a float, from either TOML form.
pub(crate) fn as_number(value: &Value) -> Option<f64> {
    match value {
        Value::Float(value) => Some(*value),
        Value::Integer(value) => Some(integer_number(*value)),
        _ => None,
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "setting values are far below 2^52"
)]
fn integer_number(value: i64) -> f64 {
    value as f64
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
        assert_eq!(parse_number(size, "big"), Err("Must be a number".into()));
        assert_eq!(parse_number(size, "inf"), Err("Must be a number".into()));
        assert_eq!(parse_number(size, ""), Ok(None));
        let rows = setting("terminal.scrollback_rows");
        assert_eq!(parse_number(rows, "2000"), Ok(Some(Value::Integer(2000))));
        assert_eq!(
            parse_number(rows, "1.5"),
            Err("Must be a whole number".into())
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
    fn typed_numbers_clamp_to_the_range_as_a_number_field_does() {
        // D7: a number setting never saves a value outside its range.
        let size = setting("font.size");
        assert_eq!(parse_number(size, "200"), Ok(Some(Value::Float(96.0))));
        assert_eq!(parse_number(size, "1"), Ok(Some(Value::Float(6.0))));
        let rows = setting("terminal.scrollback_rows");
        assert_eq!(parse_number(rows, "-1"), Ok(Some(Value::Integer(0))));
        let grid = steps(size).unwrap();
        assert_eq!((grid.min(), grid.max(), grid.step()), (6.0, 96.0, 1.0));
        // An exclusive minimum is the first step above it, so 0 is never
        // written.
        let width = SettingsModel::load()
            .unwrap()
            .profile_settings()
            .iter()
            .find(|setting| setting.path == "width")
            .unwrap();
        let grid = steps(width).unwrap();
        assert_eq!((grid.min(), grid.max(), grid.step()), (0.05, 1.0, 0.05));
        assert_eq!(parse_number(width, "0"), Ok(Some(Value::Float(0.05))));
        // A missing bound is never reached.
        let hours = steps(setting("updates.check_interval_hours")).unwrap();
        assert_eq!(hours.increment(1_000_000.0), 1_000_001.0);
        assert_eq!(
            number_value(setting("updates.check_interval_hours"), 2.0),
            Value::Integer(2)
        );
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
