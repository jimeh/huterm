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
const KEYS: [&str; 9] = [
    "section",
    "group",
    "label",
    "order",
    "apply",
    "unit",
    "platform",
    "advanced",
    "visible-when",
];

/// A sidebar section, in sidebar order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SectionId {
    General,
    Appearance,
    Tabs,
    Window,
    Terminal,
    Scrollback,
    Palette,
    Quake,
    Keybindings,
    Updates,
}

impl SectionId {
    pub const ALL: [Self; 10] = [
        Self::General,
        Self::Appearance,
        Self::Tabs,
        Self::Window,
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
            Self::Window => "window",
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
            Self::Window => "Window and panes",
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

/// Shows a setting only while another setting holds `equals`.
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
    pub apply: Apply,
    pub platform: Option<Platform>,
    pub advanced: bool,
    pub visible_when: Option<VisibleWhen>,
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
        Ok(Self { sections })
    }

    /// The ten sections in sidebar order, including any without settings.
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
    let visible_when = property
        .get("x-huterm-visible-when")
        .map(|condition| visible_when(path, condition))
        .transpose()?;
    let field = |key: &str| property.get(key).or_else(|| target.get(key));
    Ok(Setting {
        path: path.into(),
        section,
        group: required("group")?,
        label: required("label")?,
        order,
        description: field("description").and_then(Value::as_str).map(prose),
        kind: kind(path, property, target)?,
        default: property
            .get("default")
            .and_then(|default| toml::Value::try_from(default).ok()),
        unit: text("unit")?,
        apply,
        platform,
        advanced,
        visible_when,
        deprecated: field("deprecated") == Some(&Value::Bool(true)),
    })
}

fn visible_when(
    path: &str,
    condition: &Value,
) -> Result<VisibleWhen, ModelError> {
    let malformed = || {
        error(
            path,
            "x-huterm-visible-when must be {\"setting\": <path>, \"equals\": <value>}",
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
    for setting in settings {
        let Some(condition) = &setting.visible_when else {
            continue;
        };
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
            Some(VisibleWhen {
                setting: "tabs.style".into(),
                equals: toml::Value::String("pill".into()),
            })
        );
        assert_eq!(
            model.setting("terminal.bell.visual").unwrap().kind,
            SettingKind::Bool
        );
        let rows = model.setting("terminal.scrollback_rows").unwrap();
        assert_eq!(
            rows.kind,
            SettingKind::Integer {
                min: Some(0),
                max: Some(50_000)
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
}
