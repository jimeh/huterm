//! The settings a Settings window shows, read from the `x-huterm-*`
//! metadata in the committed configuration schema.
//!
//! A schema property carrying `x-huterm-label` is a setting. An object
//! property without one is a namespace, such as `tabs` or `terminal.bell`,
//! and its properties are read in turn. Any other property is an error, so a
//! field added without metadata fails [`SettingsModel::load`].
use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

use serde_json::{Map, Value};

/// The committed configuration schema.
pub const SCHEMA: &str = include_str!("../../../schemas/huterm.schema.json");

const PREFIX: &str = "x-huterm-";
const KEYS: [&str; 12] = [
    "section",
    "group",
    "label",
    "order",
    "apply",
    "unit",
    "step",
    "control",
    "platform",
    "advanced",
    "visible-when",
    "zero-label",
];

/// A sidebar section, in sidebar order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SectionId {
    General,
    Appearance,
    Windows,
    Tabs,
    Panes,
    Terminal,
    Scrollback,
    Palette,
    Quake,
    Keybindings,
    Updates,
}

impl SectionId {
    pub const ALL: [Self; 11] = [
        Self::General,
        Self::Appearance,
        Self::Windows,
        Self::Tabs,
        Self::Panes,
        Self::Terminal,
        Self::Scrollback,
        Self::Palette,
        Self::Quake,
        Self::Keybindings,
        Self::Updates,
    ];

    /// The `x-huterm-section` value.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Appearance => "appearance",
            Self::Tabs => "tabs",
            Self::Windows => "windows",
            Self::Panes => "panes",
            Self::Terminal => "terminal",
            Self::Scrollback => "scrollback",
            Self::Palette => "palette",
            Self::Quake => "quake",
            Self::Keybindings => "keybindings",
            Self::Updates => "updates",
        }
    }

    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Tabs => "Tabs",
            Self::Windows => "Windows",
            Self::Panes => "Panes",
            Self::Terminal => "Terminal",
            Self::Scrollback => "Scrollback",
            Self::Palette => "Command palette",
            Self::Quake => "Quake windows",
            Self::Keybindings => "Keybindings",
            Self::Updates => "Updates",
        }
    }

    fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|section| section.id() == id)
    }
}

/// When a saved change takes effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Apply {
    /// Open windows and terminals change at once.
    Live,
    /// Tabs and panes opened afterwards use it. New windows do not.
    NewTabsAndPanes,
    /// Terminals started afterwards use it.
    NewTerminals,
    /// Later events use it, such as the next exit, key, quake summon, or
    /// fullscreen toggle.
    LaterEvents,
    /// The command palette uses it the next time it opens.
    NextPaletteOpen,
}

impl Apply {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "live" => Self::Live,
            "new_tabs_and_panes" => Self::NewTabsAndPanes,
            "new_terminals" => Self::NewTerminals,
            "later_events" => Self::LaterEvents,
            "next_palette_open" => Self::NextPaletteOpen,
            _ => return None,
        })
    }
}

/// The only platform a setting applies to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    Macos,
    Linux,
}

impl Platform {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "macos" => Some(Self::Macos),
            "linux" => Some(Self::Linux),
            _ => None,
        }
    }
}

/// One option of a choice setting.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    /// The value written to the file.
    pub value: String,
    pub label: Option<String>,
    pub description: Option<String>,
}

/// The value a setting holds, and so the generic control that edits it.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingKind {
    Bool,
    Choice(Vec<Choice>),
    Integer {
        min: Option<i64>,
        max: Option<i64>,
        /// The largest value the schema's unsigned `format` stores, such as
        /// `u32::MAX` for `uint32`. It caps a setting without a `maximum`.
        limit: Option<i64>,
    },
    Number {
        min: Option<f64>,
        max: Option<f64>,
        exclusive_min: Option<f64>,
    },
    Text {
        pattern: Option<String>,
    },
    /// A table, map, list, or a value of several types. No generic control
    /// edits it.
    Compound,
}

/// A control a setting asks for in place of its kind's generic one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Control {
    /// A slider along the setting's range and step, for a number with both
    /// bounds.
    Slider,
}

impl Control {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "slider" => Some(Self::Slider),
            _ => None,
        }
    }
}

/// One condition under which a setting shows: another setting holds
/// `equals`.
#[derive(Clone, Debug, PartialEq)]
pub struct VisibleWhen {
    pub setting: String,
    pub equals: toml::Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Setting {
    /// The dotted path in `config.toml`, such as `tabs.style`.
    pub path: String,
    pub section: SectionId,
    pub group: String,
    pub label: String,
    pub order: i64,
    pub description: Option<String>,
    pub kind: SettingKind,
    /// The schema's default. Settings whose default differs by platform,
    /// and optional settings, have none.
    pub default: Option<toml::Value>,
    pub unit: Option<String>,
    /// The distance a number steps by, from `x-huterm-step`; `None` steps
    /// by one.
    pub step: Option<f64>,
    /// The control `x-huterm-control` asks for, if any.
    pub control: Option<Control>,
    /// What a slider shows at 0 in place of its value, from
    /// `x-huterm-zero-label`, such as "Off".
    pub zero_label: Option<String>,
    pub apply: Apply,
    pub platform: Option<Platform>,
    pub advanced: bool,
    /// The setting shows while any of these holds, and always when there
    /// are none.
    pub visible_when: Vec<VisibleWhen>,
    pub deprecated: bool,
}

/// Settings sharing a card, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub title: String,
    pub settings: Vec<Setting>,
}

/// A section's groups, ordered by their first setting.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub id: SectionId,
    pub groups: Vec<Group>,
}

/// Every setting, by section and group.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsModel {
    sections: Vec<Section>,
    profile: Vec<Setting>,
}

/// Metadata the model cannot use, naming the schema property at fault.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelError {
    pub path: String,
    pub message: String,
}

impl fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for ModelError {}

impl SettingsModel {
    /// The model of the committed schema, parsed on first use.
    ///
    /// # Errors
    /// Returns an error when the schema is not JSON or its metadata is
    /// incomplete; tests keep the committed schema valid.
    pub fn load() -> Result<&'static Self, ModelError> {
        static MODEL: OnceLock<Result<SettingsModel, ModelError>> =
            OnceLock::new();
        MODEL
            .get_or_init(|| {
                let schema = serde_json::from_str(SCHEMA).map_err(|error| {
                    ModelError {
                        path: "$".into(),
                        message: error.to_string(),
                    }
                })?;
                Self::from_schema(&schema)
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Reads a configuration schema.
    ///
    /// # Errors
    /// Returns the first property whose metadata is missing, malformed, or
    /// inconsistent with the other settings.
    pub fn from_schema(schema: &Value) -> Result<Self, ModelError> {
        let definitions = schema.get("definitions").and_then(Value::as_object);
        let mut settings = Vec::new();
        collect(schema, definitions, "", &mut settings)?;
        check(&settings)?;
        let mut profile = Vec::new();
        if let Some(definition) =
            definitions.and_then(|definitions| definitions.get("Profile"))
        {
            collect(definition, definitions, "", &mut profile)?;
            check(&profile)?;
            profile.sort_by_key(|setting| setting.order);
        }
        let mut sections: Vec<Section> = SectionId::ALL
            .into_iter()
            .map(|id| Section {
                id,
                groups: Vec::new(),
            })
            .collect();
        settings.sort_by_key(|setting| setting.order);
        for setting in settings {
            let section = &mut sections[setting.section as usize];
            match section
                .groups
                .iter_mut()
                .find(|group| group.title == setting.group)
            {
                Some(group) => group.settings.push(setting),
                None => section.groups.push(Group {
                    title: setting.group.clone(),
                    settings: vec![setting],
                }),
            }
        }
        Ok(Self { sections, profile })
    }

    /// The sections in sidebar order, including any without settings.
    #[must_use]
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// Every setting, in section and group order.
    pub fn settings(&self) -> impl Iterator<Item = &Setting> {
        self.sections
            .iter()
            .flat_map(|section| &section.groups)
            .flat_map(|group| &group.settings)
    }

    #[must_use]
    pub fn setting(&self, path: &str) -> Option<&Setting> {
        self.settings().find(|setting| setting.path == path)
    }

    /// The settings of one quake profile, in order, with paths relative to
    /// its `[quake.profiles.<name>]` table. `visible_when` names a sibling
    /// field the same way.
    #[must_use]
    pub fn profile_settings(&self) -> &[Setting] {
        &self.profile
    }
}

fn error(path: &str, message: impl Into<String>) -> ModelError {
    ModelError {
        path: path.into(),
        message: message.into(),
    }
}

fn collect(
    node: &Value,
    definitions: Option<&Map<String, Value>>,
    prefix: &str,
    settings: &mut Vec<Setting>,
) -> Result<(), ModelError> {
    let Some(properties) = node.get("properties").and_then(Value::as_object)
    else {
        return Ok(());
    };
    for (name, property) in properties {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        let target = resolve(property, definitions, &path)?;
        if property.get("x-huterm-label").is_some() {
            settings.push(setting(&path, property, target)?);
        } else if target.get("properties").is_some() {
            if let Some(key) = metadata_keys(property).next() {
                return Err(error(
                    &path,
                    format!("{key} needs x-huterm-label to make a setting"),
                ));
            }
            collect(target, definitions, &path, settings)?;
        } else {
            return Err(error(&path, "has no x-huterm-label"));
        }
    }
    Ok(())
}

/// Follows a property's reference, written bare or as a single `allOf`
/// member, to the definition that describes its value.
fn resolve<'a>(
    property: &'a Value,
    definitions: Option<&'a Map<String, Value>>,
    path: &str,
) -> Result<&'a Value, ModelError> {
    let reference = property.get("$ref").or_else(|| {
        match property.get("allOf").and_then(Value::as_array) {
            Some(members) if members.len() == 1 => members[0].get("$ref"),
            _ => None,
        }
    });
    let Some(reference) = reference else {
        return Ok(property);
    };
    reference
        .as_str()
        .and_then(|reference| reference.strip_prefix("#/definitions/"))
        .and_then(|name| definitions?.get(name))
        .ok_or_else(|| error(path, format!("cannot resolve {reference}")))
}

fn metadata_keys(property: &Value) -> impl Iterator<Item = &str> {
    property
        .as_object()
        .into_iter()
        .flat_map(Map::keys)
        .map(String::as_str)
        .filter(|key| key.starts_with(PREFIX))
}

fn setting(
    path: &str,
    property: &Value,
    target: &Value,
) -> Result<Setting, ModelError> {
    if let Some(key) = metadata_keys(property)
        .find(|key| !KEYS.contains(&&key[PREFIX.len()..]))
    {
        return Err(error(path, format!("unknown metadata key {key}")));
    }
    let text = |key: &str| -> Result<Option<String>, ModelError> {
        match property.get(format!("{PREFIX}{key}")) {
            None => Ok(None),
            Some(Value::String(value)) if !value.trim().is_empty() => {
                Ok(Some(value.clone()))
            }
            Some(_) => Err(error(
                path,
                format!("{PREFIX}{key} must be non-blank text"),
            )),
        }
    };
    let required = |key: &str| {
        text(key)?
            .ok_or_else(|| error(path, format!("{PREFIX}{key} is required")))
    };
    let section = required("section")?;
    let section = SectionId::parse(&section)
        .ok_or_else(|| error(path, format!("unknown section {section}")))?;
    let apply = required("apply")?;
    let apply = Apply::parse(&apply)
        .ok_or_else(|| error(path, format!("unknown apply {apply}")))?;
    let platform = text("platform")?
        .map(|platform| {
            Platform::parse(&platform).ok_or_else(|| {
                error(path, format!("unknown platform {platform}"))
            })
        })
        .transpose()?;
    let order = property
        .get("x-huterm-order")
        .and_then(Value::as_i64)
        .ok_or_else(|| error(path, "x-huterm-order must be an integer"))?;
    let advanced = match property.get("x-huterm-advanced") {
        None => false,
        Some(Value::Bool(advanced)) => *advanced,
        Some(_) => {
            return Err(error(path, "x-huterm-advanced must be a boolean"));
        }
    };
    let visible_when = match property.get("x-huterm-visible-when") {
        None => Vec::new(),
        // A list shows the setting while any of its conditions holds.
        Some(Value::Array(conditions)) if !conditions.is_empty() => conditions
            .iter()
            .map(|condition| visible_when(path, condition))
            .collect::<Result<_, _>>()?,
        Some(condition) => vec![visible_when(path, condition)?],
    };
    let field = |key: &str| property.get(key).or_else(|| target.get(key));
    let kind = kind(path, property, target)?;
    let step = step(path, property, &kind)?;
    let unit = text("unit")?;
    let control = control(path, property, &kind, step, unit.as_deref())?;
    // Only a slider's readout shows it.
    let zero_label = text("zero-label")?;
    if zero_label.is_some() && control != Some(Control::Slider) {
        return Err(error(path, "x-huterm-zero-label needs a slider"));
    }
    Ok(Setting {
        path: path.into(),
        section,
        group: required("group")?,
        label: required("label")?,
        order,
        description: field("description").and_then(Value::as_str).map(prose),
        kind,
        default: property
            .get("default")
            .and_then(|default| toml::Value::try_from(default).ok()),
        unit,
        step,
        control,
        zero_label,
        apply,
        platform,
        advanced,
        visible_when,
        deprecated: field("deprecated") == Some(&Value::Bool(true)),
    })
}

/// The `x-huterm-step` of a number setting: positive, and whole for an
/// integer.
fn step(
    path: &str,
    property: &Value,
    kind: &SettingKind,
) -> Result<Option<f64>, ModelError> {
    let Some(step) = property.get("x-huterm-step") else {
        return Ok(None);
    };
    let step = step
        .as_f64()
        .filter(|step| step.is_finite() && *step > 0.0)
        .ok_or_else(|| {
            error(path, "x-huterm-step must be a positive number")
        })?;
    match kind {
        SettingKind::Number { .. } => Ok(Some(step)),
        SettingKind::Integer { .. } if step.fract() == 0.0 => Ok(Some(step)),
        SettingKind::Integer { .. } => Err(error(
            path,
            "x-huterm-step must be a whole number for an integer",
        )),
        _ => Err(error(path, "x-huterm-step needs a number setting")),
    }
}

/// The `x-huterm-control` of a setting. A slider needs a number with both
/// bounds, either minimum inclusive or exclusive, and a step. Without a
/// unit its readout is a percent, so its range must lie within 0 to 1.
fn control(
    path: &str,
    property: &Value,
    kind: &SettingKind,
    step: Option<f64>,
    unit: Option<&str>,
) -> Result<Option<Control>, ModelError> {
    let Some(control) = property.get("x-huterm-control") else {
        return Ok(None);
    };
    let control = control
        .as_str()
        .and_then(Control::parse)
        .ok_or_else(|| error(path, "x-huterm-control must be \"slider\""))?;
    #[expect(
        clippy::cast_precision_loss,
        reason = "slider bounds are far below 2^52"
    )]
    let bounds = match kind {
        SettingKind::Integer { min, max, .. } => {
            min.zip(*max).map(|(min, max)| (min as f64, max as f64))
        }
        SettingKind::Number {
            min,
            max,
            exclusive_min,
        } => min.or(*exclusive_min).zip(*max),
        _ => None,
    };
    let Some((min, max)) = bounds.filter(|_| step.is_some()) else {
        return Err(error(
            path,
            "a slider needs a number with both bounds and x-huterm-step",
        ));
    };
    if unit.is_none() && (min < 0.0 || max > 1.0) {
        return Err(error(
            path,
            "a slider without x-huterm-unit shows a percent, so its range must lie within 0 to 1",
        ));
    }
    Ok(Some(control))
}

fn visible_when(
    path: &str,
    condition: &Value,
) -> Result<VisibleWhen, ModelError> {
    let malformed = || {
        error(
            path,
            "x-huterm-visible-when must be {\"setting\": <path>, \"equals\": <value>}, or a list of them",
        )
    };
    let fields = condition.as_object().ok_or_else(malformed)?;
    if fields.len() != 2 {
        return Err(malformed());
    }
    let setting = fields
        .get("setting")
        .and_then(Value::as_str)
        .ok_or_else(malformed)?;
    let equals = fields
        .get("equals")
        .and_then(|value| toml::Value::try_from(value).ok())
        .ok_or_else(malformed)?;
    Ok(VisibleWhen {
        setting: setting.into(),
        equals,
    })
}

/// Joins the lines of each paragraph, since doc comments wrap at the
/// source's line width and a control wraps at its own.
fn prose(text: &str) -> String {
    text.split("\n\n")
        .map(|paragraph| {
            paragraph
                .lines()
                .map(str::trim)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn kind(
    path: &str,
    property: &Value,
    target: &Value,
) -> Result<SettingKind, ModelError> {
    if let Some(choices) = choices(path, target)? {
        return Ok(SettingKind::Choice(choices));
    }
    let number = |key: &str| {
        property
            .get(key)
            .or_else(|| target.get(key))
            .and_then(Value::as_f64)
    };
    let integer = |key: &str| {
        property
            .get(key)
            .or_else(|| target.get(key))
            .and_then(Value::as_i64)
    };
    Ok(match target.get("type").and_then(Value::as_str) {
        Some("boolean") => SettingKind::Bool,
        Some("integer") => SettingKind::Integer {
            min: integer("minimum"),
            max: integer("maximum"),
            limit: target
                .get("format")
                .and_then(Value::as_str)
                .and_then(unsigned_limit),
        },
        Some("number") => SettingKind::Number {
            min: number("minimum"),
            max: number("maximum"),
            exclusive_min: number("exclusiveMinimum"),
        },
        Some("string") => SettingKind::Text {
            pattern: target
                .get("pattern")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
        _ => SettingKind::Compound,
    })
}

/// The largest value an unsigned integer `format` stores.
fn unsigned_limit(format: &str) -> Option<i64> {
    match format {
        "uint8" => Some(u8::MAX.into()),
        "uint16" => Some(u16::MAX.into()),
        "uint32" => Some(u32::MAX.into()),
        _ => None,
    }
}

/// The options of a string enum, written as `enum`, as `oneOf` members with
/// `const` and an optional label, or as a mix of both. `None` when the
/// schema is not a string enum.
fn choices(
    path: &str,
    schema: &Value,
) -> Result<Option<Vec<Choice>>, ModelError> {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return Ok(values
            .iter()
            .map(|value| {
                Some(Choice {
                    value: value.as_str()?.to_owned(),
                    label: None,
                    description: None,
                })
            })
            .collect());
    }
    let Some(members) = schema.get("oneOf").and_then(Value::as_array) else {
        return Ok(None);
    };
    let mut choices = Vec::new();
    for member in members {
        let Some(value) = member.get("const") else {
            match self::choices(path, member)? {
                Some(nested) => choices.extend(nested),
                None => return Ok(None),
            }
            continue;
        };
        let Some(value) = value.as_str() else {
            return Ok(None);
        };
        let invalid = |message: &str| {
            Err(error(path, format!("option {value}: {message}")))
        };
        if let Some(key) =
            metadata_keys(member).find(|key| *key != "x-huterm-label")
        {
            return invalid(&format!("unknown metadata key {key}"));
        }
        let label = match member.get("x-huterm-label") {
            None => None,
            Some(Value::String(label)) if !label.trim().is_empty() => {
                Some(label.clone())
            }
            Some(_) => return invalid("x-huterm-label must be non-blank text"),
        };
        choices.push(Choice {
            value: value.to_owned(),
            label,
            description: member
                .get("description")
                .and_then(Value::as_str)
                .map(prose),
        });
    }
    Ok(Some(choices))
}

/// Checks what no single property can: unique orders within a section, and
/// conditions that name another setting by a value it can hold.
fn check(settings: &[Setting]) -> Result<(), ModelError> {
    let mut orders = BTreeMap::new();
    for setting in settings {
        if let Some(other) =
            orders.insert((setting.section, setting.order), &setting.path)
        {
            return Err(error(
                &setting.path,
                format!("x-huterm-order {} repeats {other}", setting.order),
            ));
        }
    }
    for (setting, condition) in settings.iter().flat_map(|setting| {
        setting
            .visible_when
            .iter()
            .map(move |condition| (setting, condition))
    }) {
        let target = settings
            .iter()
            .find(|other| other.path == condition.setting)
            .filter(|other| other.path != setting.path)
            .ok_or_else(|| {
                error(
                    &setting.path,
                    format!(
                        "x-huterm-visible-when names unknown setting {}",
                        condition.setting
                    ),
                )
            })?;
        let holds = match (&target.kind, &condition.equals) {
            (SettingKind::Bool, toml::Value::Boolean(_)) => true,
            (SettingKind::Choice(choices), toml::Value::String(value)) => {
                choices.iter().any(|choice| &choice.value == value)
            }
            _ => false,
        };
        if !holds {
            return Err(error(
                &setting.path,
                format!(
                    "x-huterm-visible-when compares {} with a value it cannot hold",
                    condition.setting
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_sidebar_lists_windows_above_tabs() {
        assert_eq!(
            SectionId::ALL.map(SectionId::title),
            [
                "General",
                "Appearance",
                "Windows",
                "Tabs",
                "Panes",
                "Terminal",
                "Scrollback",
                "Command palette",
                "Quake windows",
                "Keybindings",
                "Updates",
            ]
        );
    }

    #[test]
    fn the_committed_schema_lists_every_setting_by_section() {
        let model = SettingsModel::load().unwrap();
        let sections: Vec<_> =
            model.sections().iter().map(|section| section.id).collect();
        assert_eq!(sections, SectionId::ALL);
        assert!(
            model
                .sections()
                .iter()
                .all(|section| !section.groups.is_empty())
        );
        let tabs = &model.sections()[SectionId::Tabs as usize];
        let groups: Vec<_> = tabs
            .groups
            .iter()
            .map(|group| group.title.as_str())
            .collect();
        assert_eq!(groups, ["Tab bar", "Visibility", "Labels", "Sizing"]);

        let size = model.setting("font.size").unwrap();
        assert_eq!(
            size.kind,
            SettingKind::Number {
                min: Some(6.0),
                max: Some(96.0),
                exclusive_min: None
            }
        );
        assert_eq!(size.unit.as_deref(), Some("pt"));
        assert_eq!(size.default, Some(toml::Value::Float(14.0)));
        let SettingKind::Choice(positions) =
            &model.setting("tabs.position").unwrap().kind
        else {
            panic!("tabs.position is a choice");
        };
        let titlebar = positions
            .iter()
            .find(|choice| choice.value == "titlebar")
            .unwrap();
        assert_eq!(titlebar.label.as_deref(), Some("Title bar"));
        assert!(titlebar.description.is_some());
        assert_eq!(model.setting("tabs.position").unwrap().default, None);
        assert_eq!(
            model.setting("tabs.pill_accent").unwrap().visible_when,
            vec![VisibleWhen {
                setting: "tabs.style".into(),
                equals: toml::Value::String("pill".into()),
            }]
        );
        assert_eq!(
            model.setting("terminal.bell.visual").unwrap().kind,
            SettingKind::Bool
        );
        let compression =
            model.setting("terminal.scrollback_compression").unwrap();
        assert_eq!(compression.kind, SettingKind::Bool);
        assert_eq!(compression.section, SectionId::Scrollback);
        assert_eq!(compression.label, "Compress idle history");
        assert_eq!(compression.default, Some(toml::Value::Boolean(true)));
        assert_eq!(compression.apply, Apply::Live);
        assert!(!compression.advanced);
        let rows = model.setting("terminal.scrollback_rows").unwrap();
        assert_eq!(
            rows.kind,
            SettingKind::Integer {
                min: Some(0),
                max: Some(100_000),
                limit: Some(u32::MAX.into()),
            }
        );
        for path in [
            "theme",
            "themes",
            "keybinding",
            "global_keybinding",
            "quake",
            "terminal.scrollback_bytes",
        ] {
            assert_eq!(
                model.setting(path).unwrap().kind,
                SettingKind::Compound,
                "{path}"
            );
        }
        assert!(model.setting("terminal.engine").unwrap().deprecated);
        assert_eq!(
            model.setting("updates.automatic_checks").unwrap().platform,
            Some(Platform::Macos)
        );
        assert_eq!(
            model.setting("terminal.term").unwrap().apply,
            Apply::NewTerminals
        );
        let rows = rows.description.as_deref().unwrap();
        assert!(rows.starts_with("Live history each terminal keeps"));
        assert!(!rows.contains('\n'), "wrapped lines join: {rows}");
    }

    #[test]
    fn window_and_pane_settings_have_their_own_sections() {
        let model = SettingsModel::load().unwrap();
        let mut paths = BTreeMap::new();
        for section in model.sections() {
            for setting in section.groups.iter().flat_map(|g| &g.settings) {
                assert_eq!(setting.section, section.id, "{}", setting.path);
                let seen = paths.insert(setting.path.as_str(), section.id);
                assert_eq!(seen, None, "{} is listed twice", setting.path);
                let expected = match setting.path.split_once('.') {
                    // Padding draws with the terminal's appearance.
                    Some(("window", key)) if key.starts_with("padding_") => {
                        SectionId::Appearance
                    }
                    Some(("window", _)) => SectionId::Windows,
                    Some(("panes", _)) => SectionId::Panes,
                    _ => continue,
                };
                assert_eq!(setting.section, expected, "{}", setting.path);
            }
        }
        assert_eq!(paths.len(), model.settings().count());
        let groups = |id: SectionId| -> Vec<&str> {
            model.sections()[id as usize]
                .groups
                .iter()
                .map(|group| group.title.as_str())
                .collect()
        };
        assert_eq!(groups(SectionId::Windows), ["Window"]);
        assert_eq!(groups(SectionId::Panes), ["Panes", "Pane layouts"]);
        // Linux hides macOS-only settings, and Windows keeps others there.
        assert!(
            model.sections()[SectionId::Windows as usize].groups[0]
                .settings
                .iter()
                .any(|setting| setting.platform.is_none())
        );
    }

    #[test]
    fn terminal_groups_put_clipboard_before_bell() {
        let model = SettingsModel::load().unwrap();
        let groups: Vec<(&str, Vec<&str>)> = model.sections()
            [SectionId::Terminal as usize]
            .groups
            .iter()
            .map(|group| {
                let paths = group
                    .settings
                    .iter()
                    .map(|setting| setting.path.as_str())
                    .collect();
                (group.title.as_str(), paths)
            })
            .collect();
        let titles: Vec<_> = groups.iter().map(|(title, _)| *title).collect();
        assert_eq!(
            titles,
            [
                "Shell",
                "Keyboard",
                "Mouse",
                "Clipboard",
                "Bell",
                "Compatibility"
            ]
        );
        assert_eq!(groups[3].1, ["terminal.clipboard_write"]);
        assert_eq!(
            groups[4].1,
            [
                "terminal.bell.visual",
                "terminal.bell.audio",
                "terminal.bell.sound",
                "terminal.bell.volume",
            ]
        );
        assert_eq!(model.setting("terminal.bell.audio").unwrap().default, None);
    }

    #[test]
    fn every_profile_field_has_a_setting_with_its_default() {
        let model = SettingsModel::load().unwrap();
        let defaults =
            toml::Table::try_from(crate::quake::Profile::default()).unwrap();
        let mut paths: Vec<_> = model
            .profile_settings()
            .iter()
            .map(|setting| setting.path.as_str())
            .collect();
        paths.sort_unstable();
        let mut fields: Vec<_> = defaults.keys().map(String::as_str).collect();
        fields.sort_unstable();
        assert_eq!(paths, fields);
        for setting in model.profile_settings() {
            assert_eq!(
                setting.default.as_ref(),
                defaults.get(&setting.path),
                "{}",
                setting.path
            );
            assert_eq!(setting.section, SectionId::Quake);
            assert_eq!(setting.apply, Apply::LaterEvents);
        }
        let width = &model.profile_settings()[2];
        assert_eq!(width.path, "width");
        // Each profile's copy carries the template's slider.
        assert_eq!(
            (width.control, width.step),
            (Some(Control::Slider), Some(0.05))
        );
        assert_eq!(
            width.visible_when,
            vec![VisibleWhen {
                setting: "fullscreen".into(),
                equals: toml::Value::Boolean(false),
            }]
        );
        // A fullscreen window still slides from its position's edge.
        let position = &model.profile_settings()[1];
        assert_eq!(position.path, "position");
        assert_eq!(
            position.visible_when,
            vec![
                VisibleWhen {
                    setting: "fullscreen".into(),
                    equals: toml::Value::Boolean(false),
                },
                VisibleWhen {
                    setting: "animation".into(),
                    equals: toml::Value::String("slide".into()),
                },
            ]
        );
        assert_eq!(
            width.kind,
            SettingKind::Number {
                min: None,
                max: Some(1.0),
                exclusive_min: Some(0.0)
            }
        );
    }

    /// Every value a default configuration writes belongs to a setting, so
    /// the walk misses no field the schema defines.
    #[test]
    fn every_default_value_belongs_to_a_setting() {
        fn leaves(value: &toml::Value, path: &str, out: &mut Vec<String>) {
            match value {
                toml::Value::Table(table) if !table.is_empty() => {
                    for (key, value) in table {
                        let path = if path.is_empty() {
                            key.clone()
                        } else {
                            format!("{path}.{key}")
                        };
                        leaves(value, &path, out);
                    }
                }
                _ => out.push(path.to_owned()),
            }
        }
        let model = SettingsModel::load().unwrap();
        let defaults = toml::Value::try_from(crate::RawConfig {
            general: crate::GeneralConfig::default(),
            settings: crate::SettingsConfig::default(),
            terminal: crate::RawTerminal::default(),
            font: crate::RawFont::default(),
            window: crate::WindowConfig::default(),
            tabs: crate::TabsConfig::default(),
            updates: crate::UpdateConfig {
                automatic_checks: Some(true),
                check_interval_hours: Some(24),
            },
            palette: crate::PaletteConfig::default(),
            panes: crate::PanesConfig::default(),
            theme: crate::ThemeDefinition::default(),
            themes: BTreeMap::new(),
            keybinding: Vec::new(),
            quake: crate::quake::Config::default(),
            global_keybinding: Vec::new(),
        })
        .unwrap();
        let mut paths = Vec::new();
        leaves(&defaults, "", &mut paths);
        assert!(paths.len() > 40, "{paths:?}");
        for path in paths {
            let owned = std::iter::successors(Some(path.as_str()), |path| {
                path.rsplit_once('.').map(|(parent, _)| parent)
            })
            .any(|path| model.setting(path).is_some());
            assert!(owned, "{path} belongs to no setting");
        }
    }

    fn schema(properties: &Value) -> Value {
        json!({
            "properties": properties,
            "definitions": {
                "Style": {"oneOf": [{"const": "pill"}, {"const": "strip"}]}
            }
        })
    }

    fn setting(extra: &Value) -> Value {
        let mut setting = json!({
            "type": "boolean",
            "x-huterm-section": "tabs",
            "x-huterm-group": "Tab bar",
            "x-huterm-label": "Label",
            "x-huterm-order": 10,
            "x-huterm-apply": "live",
        });
        for (key, value) in extra.as_object().unwrap() {
            if value.is_null() {
                setting.as_object_mut().unwrap().remove(key);
            } else {
                setting[key] = value.clone();
            }
        }
        setting
    }

    #[test]
    fn focus_indicators_are_sliders_that_read_off_at_zero() {
        let model = SettingsModel::load().unwrap();
        for (path, max) in
            [("panes.focus_accent", 1.0), ("panes.inactive_dim", 0.8)]
        {
            let setting = model.setting(path).unwrap();
            assert_eq!(setting.control, Some(Control::Slider), "{path}");
            assert_eq!(setting.zero_label.as_deref(), Some("Off"), "{path}");
            assert!(
                matches!(
                    setting.kind,
                    SettingKind::Number { min: Some(0.0), max: Some(found), .. }
                        if (found - max).abs() < 1e-6
                ),
                "{path}: {:?}",
                setting.kind
            );
        }
        // The accent draws only where a bar can be off screen.
        let mode = |value: &str| VisibleWhen {
            setting: "panes.title_bar".into(),
            equals: toml::Value::String(value.into()),
        };
        assert_eq!(
            model.setting("panes.focus_accent").unwrap().visible_when,
            vec![mode("hover"), mode("hidden")]
        );
        // The position also places the accent under hidden bars, so it
        // shows in every mode, as dimming does.
        for path in ["panes.title_bar_position", "panes.inactive_dim"] {
            assert_eq!(
                model.setting(path).unwrap().visible_when,
                Vec::new(),
                "{path}"
            );
        }
    }

    #[test]
    fn timing_rows_show_only_where_their_setting_acts() {
        let model = SettingsModel::load().unwrap();
        let equals = |setting: &str, value: &str| VisibleWhen {
            setting: setting.into(),
            equals: toml::Value::String(value.into()),
        };
        for (path, max) in [
            ("panes.title_bar_hover_delay_ms", 2000),
            ("panes.title_bar_fade_ms", 1000),
            ("panes.title_bar_hide_delay_ms", 5000),
        ] {
            let setting = model.setting(path).unwrap();
            assert_eq!(setting.unit.as_deref(), Some("ms"), "{path}");
            assert!(
                matches!(
                    setting.kind,
                    SettingKind::Integer { min: Some(0), max: Some(found), .. }
                        if found == max
                ),
                "{path}: {:?}",
                setting.kind
            );
            assert_eq!(
                setting.visible_when,
                vec![equals("panes.title_bar", "hover")],
                "{path}"
            );
        }
        assert_eq!(
            model
                .setting("window.size_indicator_linger_ms")
                .unwrap()
                .visible_when,
            vec![
                equals("window.size_indicator", "full"),
                equals("window.size_indicator", "compact")
            ]
        );
    }

    #[test]
    fn malformed_metadata_names_the_setting() {
        let style = || {
            let mut style =
                setting(&json!({"type": null, "x-huterm-order": 20}));
            style["allOf"] = json!([{"$ref": "#/definitions/Style"}]);
            style
        };
        for (properties, path, message) in [
            (
                json!({"tabs": {"properties": {"accent": {"type": "boolean"}}}}),
                "tabs.accent",
                "has no x-huterm-label",
            ),
            (
                json!({"a": setting(&json!({"x-huterm-section": "sidebar"}))}),
                "a",
                "unknown section sidebar",
            ),
            (
                json!({"a": setting(&json!({"x-huterm-apply": null}))}),
                "a",
                "x-huterm-apply is required",
            ),
            (
                json!({"a": setting(&json!({"x-huterm-apply": "soon"}))}),
                "a",
                "unknown apply soon",
            ),
            (
                json!({"a": setting(&json!({"x-huterm-platform": "windows"}))}),
                "a",
                "unknown platform windows",
            ),
            (
                json!({"tabs": {
                    "x-huterm-group": "Tab bar",
                    "properties": {"a": setting(&json!({}))}
                }}),
                "tabs",
                "x-huterm-group needs x-huterm-label to make a setting",
            ),
            (
                json!({"a": setting(&json!({
                    "type": null,
                    "oneOf": [{"const": "pill", "x-huterm-label": " "}]
                }))}),
                "a",
                "option pill: x-huterm-label must be non-blank text",
            ),
            (
                json!({"a": setting(&json!({
                    "type": null,
                    "oneOf": [{"const": "pill", "x-huterm-lable": "Pill"}]
                }))}),
                "a",
                "option pill: unknown metadata key x-huterm-lable",
            ),
            (
                json!({"a": setting(&json!({"x-huterm-lable": "Typo"}))}),
                "a",
                "unknown metadata key x-huterm-lable",
            ),
            (
                json!({"a": setting(&json!({})), "b": setting(&json!({}))}),
                "b",
                "x-huterm-order 10 repeats a",
            ),
            (
                json!({"a": setting(&json!({
                    "x-huterm-visible-when": {"setting": "missing", "equals": true}
                }))}),
                "a",
                "x-huterm-visible-when names unknown setting missing",
            ),
            (
                json!({"style": style(), "a": setting(&json!({
                    "x-huterm-visible-when": {"setting": "style", "equals": "round"}
                }))}),
                "a",
                "x-huterm-visible-when compares style with a value it cannot hold",
            ),
        ] {
            assert_eq!(
                SettingsModel::from_schema(&schema(&properties)),
                Err(error(path, message)),
            );
        }
        let model = SettingsModel::from_schema(&schema(&json!({
            "style": style(),
            "a": setting(&json!({
                "x-huterm-visible-when": {"setting": "style", "equals": "pill"}
            })),
        })))
        .unwrap();
        assert_eq!(model.settings().count(), 2);
    }

    #[test]
    fn malformed_step_and_control_metadata_names_the_setting() {
        for (properties, path, message) in [
            (
                json!({"a": setting(&json!({
                    "type": "number", "x-huterm-step": 0
                }))}),
                "a",
                "x-huterm-step must be a positive number",
            ),
            (
                json!({"a": setting(&json!({
                    "type": "integer", "x-huterm-step": 0.5
                }))}),
                "a",
                "x-huterm-step must be a whole number for an integer",
            ),
            (
                json!({"a": setting(&json!({"x-huterm-step": 1}))}),
                "a",
                "x-huterm-step needs a number setting",
            ),
            (
                json!({"a": setting(&json!({
                    "type": "number", "maximum": 1,
                    "x-huterm-step": 0.1, "x-huterm-control": "dial"
                }))}),
                "a",
                "x-huterm-control must be \"slider\"",
            ),
            (
                json!({"a": setting(&json!({
                    "type": "number", "minimum": 0,
                    "x-huterm-step": 0.1, "x-huterm-control": "slider"
                }))}),
                "a",
                "a slider needs a number with both bounds and x-huterm-step",
            ),
            (
                json!({"a": setting(&json!({
                    "type": "number", "minimum": 0, "maximum": 1,
                    "x-huterm-control": "slider"
                }))}),
                "a",
                "a slider needs a number with both bounds and x-huterm-step",
            ),
            (
                json!({"a": setting(&json!({
                    "type": "number", "minimum": 0, "maximum": 100,
                    "x-huterm-step": 5, "x-huterm-control": "slider"
                }))}),
                "a",
                "a slider without x-huterm-unit shows a percent, so its range must lie within 0 to 1",
            ),
            (
                json!({"a": setting(&json!({
                    "type": "number", "x-huterm-zero-label": " "
                }))}),
                "a",
                "x-huterm-zero-label must be non-blank text",
            ),
            (
                json!({"a": setting(&json!({"x-huterm-zero-label": "Off"}))}),
                "a",
                "x-huterm-zero-label needs a slider",
            ),
            (
                json!({"a": setting(&json!({
                    "type": "number", "x-huterm-zero-label": "Off"
                }))}),
                "a",
                "x-huterm-zero-label needs a slider",
            ),
        ] {
            assert_eq!(
                SettingsModel::from_schema(&schema(&properties)),
                Err(error(path, message)),
            );
        }
    }
}
