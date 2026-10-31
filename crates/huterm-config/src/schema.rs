//! Deterministic Draft 7 schemas for TOML editors.
use std::collections::BTreeMap;

use huterm_protocol::{ArgumentKind, Requirement, catalog};
use schemars::{JsonSchema, Schema, SchemaGenerator, generate::SchemaSettings};
use serde_json::{Map, Value, json};

use crate::{LinkModifiers, PaneDrag, RawConfig, ScrollbackSize, ThemeFile};

pub const CONFIG_ASSET: &str = "huterm.schema.json";
pub const THEME_ASSET: &str = "huterm-theme.schema.json";

impl JsonSchema for LinkModifiers {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "LinkModifiers".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        let mut combinations = Vec::new();
        modifier_combinations(&mut combinations, &mut Vec::new());
        schemars::json_schema!({"type":"string", "enum":combinations,
            "description":"Modifiers joined with hyphens, without duplicates. Defaults to cmd on macOS and ctrl on Linux. macOS rejects ctrl; this platform restriction is checked at runtime."})
    }
}
impl JsonSchema for PaneDrag {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "PaneDrag".into()
    }
    // Inline, so `drag_modifier` keeps its own description beside the
    // pattern.
    fn inline_schema() -> bool {
        true
    }
    // An enum cannot hold both spellings of `cmd` in every order, so the
    // pattern admits `off` or two to four tokens and the load checks the
    // rest.
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({"type":"string",
            "pattern":"^(off|(cmd|super|ctrl|alt|shift)(-(cmd|super|ctrl|alt|shift)){1,3})$"})
    }
}
impl JsonSchema for ScrollbackSize {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ScrollbackSize".into()
    }
    // Inline, so `scrollback_bytes` holds the schema itself rather than a
    // reference to a definition.
    fn inline_schema() -> bool {
        true
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        Schema::try_from(scrollback_size_schema())
            .expect("the scrollback size schema is an object")
    }
}

/// One object for either form: `minimum` and `maximum` apply only to
/// integers and `pattern` only to strings. JSON Schema patterns take no
/// flags, so the unit spells out both cases. The 16 GiB bound on strings is
/// checked at runtime.
fn scrollback_size_schema() -> Value {
    json!({
        "type": ["integer", "string"],
        "minimum": 0,
        "maximum": huterm_protocol::ScrollbackLimit::MAX_BYTES,
        "pattern": "^[0-9]+( ?([bB]|[kKmMgG][iI]?[bB]))?$",
    })
}

fn modifier_combinations(
    output: &mut Vec<String>,
    prefix: &mut Vec<&'static str>,
) {
    if !prefix.is_empty() {
        output.push(prefix.join("-"));
    }
    for token in ["cmd", "ctrl", "alt", "shift"] {
        if !prefix.contains(&token) {
            prefix.push(token);
            modifier_combinations(output, prefix);
            prefix.pop();
        }
    }
}

/// The schema for one configurable argument, or `None` for runtime
/// identities that a binding cannot set.
fn argument_schema(kind: ArgumentKind) -> Option<Value> {
    Some(match kind {
        ArgumentKind::Bool => json!({"type":"boolean"}),
        ArgumentKind::Text => json!({"type":"string"}),
        ArgumentKind::QuakeProfile => json!({
            "type":"string",
            "description":"Configured quake profile name; defaults to `default`."
        }),
        ArgumentKind::Integer { min, max } => {
            json!({"type":"integer", "minimum":min, "maximum":max})
        }
        ArgumentKind::Side => json!({
            "type":"string",
            "enum": huterm_protocol::SIDES,
        }),
        ArgumentKind::Layout => {
            let mut schema = layout_name();
            schema["description"] = layout_description().into();
            schema
        }
        ArgumentKind::Tab
        | ArgumentKind::Workspace
        | ArgumentKind::Session
        | ArgumentKind::Pane => return None,
    })
}

/// Names the layouts a `layout` argument may take.
fn layout_description() -> String {
    format!(
        "Pane layout name: {}, or a name from `[panes.layouts]`.",
        built_in_layout_names().join(", ")
    )
}

/// The built-in layout names, each in backticks.
fn built_in_layout_names() -> Vec<String> {
    huterm_protocol::BUILT_IN_LAYOUTS
        .iter()
        .map(|layout| format!("`{}`", layout.name))
        .collect()
}

/// A layout name: a built-in's, or a user layout's.
pub(crate) fn layout_name() -> Value {
    json!({
        "type": "string",
        "pattern": format!(
            "^[a-z][a-z0-9_]{{0,{}}}$",
            huterm_protocol::MAX_LAYOUT_NAME_LEN - 1
        ),
    })
}

/// A user layout's name, which may not be a built-in's.
pub(crate) fn user_layout_name() -> Value {
    let mut schema = layout_name();
    schema["not"] = json!({
        "enum": huterm_protocol::BUILT_IN_LAYOUTS
            .iter()
            .map(|layout| layout.name.as_str())
            .collect::<Vec<_>>(),
    });
    schema
}

/// A user layout's template.
pub(crate) fn layout_template() -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "maxLength": huterm_protocol::MAX_TEMPLATE_LEN,
        "description": "A layout template, such as `columns(pane 60%, rows(*))`.",
    })
}

fn keybindings(base: &Value, global: bool) -> Value {
    let mut alternatives = Vec::new();
    for command in catalog().iter().filter(|spec| {
        !global
            || matches!(
                spec.id.as_str(),
                "show_quake" | "hide_quake" | "toggle_quake"
            )
    }) {
        let mut properties = Map::new();
        let mut required = Vec::new();
        let mut one_of_groups = BTreeMap::<_, Vec<_>>::new();
        for arg in command.args {
            let Some(schema) = argument_schema(arg.kind) else {
                continue;
            };
            properties.insert(arg.name.into(), schema);
            match arg.required {
                // Prompted arguments may be omitted from a binding: an
                // interactive caller collects them through the palette.
                Requirement::Always if !arg.prompt => required.push(arg.name),
                Requirement::Always | Requirement::Optional => {}
                Requirement::OneOf(group) => {
                    one_of_groups.entry(group).or_default().push(arg.name);
                }
            }
        }
        let mut alternative =
            binding(base, command.id.as_str(), command.description, !global);
        let mut args_schema = json!({
            "type":"object",
            "additionalProperties":false,
            "properties":properties,
            "required":required
        });
        let mut exclusivity = one_of_groups
            .values()
            .filter(|members| members.len() > 1)
            .map(|members| json!({"not":{"required":members}}));
        if let Some(first) = exclusivity.next() {
            args_schema["not"] = first["not"].clone();
            let remaining: Vec<_> = exclusivity.collect();
            if !remaining.is_empty() {
                args_schema["allOf"] = json!(remaining);
            }
        }
        alternative["properties"]["args"] = args_schema;
        if !required.is_empty() {
            alternative["required"] = json!(["key", "command", "args"]);
        }
        alternatives.push(json!({"if":{"properties":{"command":{"const":command.id.as_str()}},"required":["command"]},"then":alternative}));
    }
    if !global {
        let mut unbind = binding(
            base,
            "unbind",
            "Remove earlier bindings for this key.",
            false,
        );
        unbind["properties"]["args"] =
            json!({"type":"object", "additionalProperties":false});
        // Runtime rejects `disabled` on unbind, even when false.
        unbind["properties"]
            .as_object_mut()
            .map(|fields| fields.remove("disabled"));
        alternatives.push(json!({"if":{"properties":{"command":{"const":"unbind"}},"required":["command"]},"then":unbind}));
    }
    let mut common = base.clone();
    let mut commands: Vec<_> = catalog()
        .iter()
        .filter(|spec| {
            !global
                || matches!(
                    spec.id.as_str(),
                    "show_quake" | "hide_quake" | "toggle_quake"
                )
        })
        .map(|spec| spec.id.as_str())
        .collect();
    if !global {
        commands.push("unbind");
    }
    common["properties"]["command"]["enum"] = json!(commands);
    if global {
        for alternative in &mut alternatives {
            global_fields(&mut alternative["then"]);
        }
        global_fields(&mut common);
    }
    // Taplo skips root properties beside allOf. Keep the derived fields in
    // the first member so ordinary field and command completion remain usable.
    alternatives.insert(0, common);
    json!({"allOf": alternatives})
}
fn global_fields(schema: &mut Value) {
    if let Some(fields) = schema["properties"].as_object_mut() {
        fields.remove("when");
    }
    if schema["properties"].get("disabled").is_some() {
        schema["properties"]["disabled"]["description"] = json!(
            "true keeps the shortcut in the file without registering it."
        );
    }
    schema["properties"]["key"]["pattern"] = json!("^\\S+$");
    schema["properties"]["key"]["description"] = json!(
        "One global keystroke in GPUI syntax. Native key support, duplicate grabs, local conflicts, and OS registration are checked at runtime."
    );
    schema["properties"]["description"]["pattern"] = json!("\\S");
}
fn binding(
    base: &Value,
    command: &str,
    description: &str,
    when: bool,
) -> Value {
    let mut schema = base.clone();
    schema["description"] = json!(description);
    schema["properties"]["key"]["pattern"] = json!("\\S");
    schema["properties"]["key"]["description"] = json!(
        "GPUI keystroke or space-separated chord sequence. Keystroke syntax is checked at runtime."
    );
    schema["properties"]["command"]["const"] = json!(command);
    schema["properties"]["command"]["description"] = json!(description);
    schema["properties"]["description"]["pattern"] = json!("\\S");
    if when {
        schema["properties"]["when"]["description"] =
            json!("GPUI key-context predicate, parsed at runtime.");
    } else if let Some(properties) = schema["properties"].as_object_mut() {
        properties.remove("when");
    }
    schema
}

/// Generates both release files, including their final newline.
///
/// # Errors
/// Returns a JSON encoding error if serialization fails.
pub fn documents() -> Result<[(&'static str, String); 2], serde_json::Error> {
    let mut config = generate::<RawConfig>()?;
    let mut theme = generate::<ThemeFile>()?;
    for (schema, asset) in
        [(&mut config, CONFIG_ASSET), (&mut theme, THEME_ASSET)]
    {
        schema["$id"] = json!(format!(
            "https://github.com/jimeh/huterm/releases/latest/download/{asset}"
        ));
        clean(schema);
    }
    let binding_base = config["definitions"]["RawKeybinding"].clone();
    config["definitions"]["RawKeybinding"] = keybindings(&binding_base, false);
    config["definitions"]["GlobalKeybinding"] =
        keybindings(&binding_base, true);
    config["properties"]["global_keybinding"]["items"] =
        json!({"$ref":"#/definitions/GlobalKeybinding"});
    config["title"] = json!("Huterm configuration");
    theme["title"] = json!("Huterm standalone theme");
    let mut selected = config["definitions"]["ThemeDefinition"].clone();
    // Selected themes use name; named definitions and files use extends.
    selected["properties"]
        .as_object_mut()
        .map(|fields| fields.remove("extends"));
    selected["properties"]["name"] = json!({"type":"string", "pattern":"^[A-Za-z0-9_-]+$", "description":"Bundled, inline, or adjacent theme name. Existence is checked at runtime."});
    config["definitions"]["SelectedTheme"] = selected;
    config["properties"]["theme"]["allOf"] =
        json!([{"$ref":"#/definitions/SelectedTheme"}]);
    Ok([
        (
            CONFIG_ASSET,
            format!("{}\n", serde_json::to_string_pretty(&config)?),
        ),
        (
            THEME_ASSET,
            format!("{}\n", serde_json::to_string_pretty(&theme)?),
        ),
    ])
}
fn generate<T: JsonSchema>() -> Result<Value, serde_json::Error> {
    serde_json::to_value(
        SchemaSettings::draft07()
            .for_deserialize()
            .into_generator()
            .into_root_schema_for::<T>(),
    )
}
/// Restricts theme colors to hex values. The selected theme gets its `name`
/// back in [`documents`].
pub(crate) fn constrain_theme(schema: &mut Schema) {
    if let Some(fields) =
        schema.get_mut("properties").and_then(Value::as_object_mut)
    {
        fields.remove("name");
        for (name, value) in fields {
            if name == "extends" {
                value["pattern"] = json!("^[A-Za-z0-9_-]+$");
            } else if name == "ansi" {
                value["minItems"] = json!(16);
                value["maxItems"] = json!(16);
                value["items"]["pattern"] = json!("^#[0-9a-fA-F]{6}$");
            } else if matches!(
                name.as_str(),
                "tab_hover_background" | "scrollbar_thumb" | "scrollbar_track"
            ) {
                // Overlays may carry alpha.
                value["pattern"] = json!("^#[0-9a-fA-F]{6}([0-9a-fA-F]{2})?$");
            } else {
                value["pattern"] = json!("^#[0-9a-fA-F]{6}$");
            }
        }
    }
}
/// Drops a default that differs by platform, so the committed schema does
/// not depend on where it was generated. The field's description names
/// both defaults.
pub(crate) fn without_default(schema: &mut Schema) {
    schema.remove("default");
}

// TOML has no null. Container defaults can contain host-specific values.
fn clean(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if object
                .get("default")
                .is_some_and(|v| v.is_object() || v.is_null())
            {
                object.remove("default");
            }
            if let Some(Value::Array(types)) = object.get_mut("type") {
                types.retain(|v| v != "null");
                if types.len() == 1 {
                    let only = types[0].clone();
                    object.insert("type".into(), only);
                }
            }
            for value in object.values_mut() {
                clean(value);
            }
        }
        Value::Array(array) => {
            for value in array {
                clean(value);
            }
        }
        _ => {}
    }
}
