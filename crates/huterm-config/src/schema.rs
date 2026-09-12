//! Deterministic Draft 7 schemas for TOML editors.
use huterm_protocol::{ArgumentKind, catalog};
use schemars::{JsonSchema, Schema, SchemaGenerator, generate::SchemaSettings};
use serde_json::{Map, Value, json};

use crate::{LinkModifiers, RawConfig, ThemeFile};

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
        for arg in command.args {
            let schema = match arg.kind {
                ArgumentKind::Bool => json!({"type":"boolean"}),
                ArgumentKind::Text => json!({"type":"string"}),
                ArgumentKind::Integer { min, max } => {
                    json!({"type":"integer", "minimum":min, "maximum":max})
                }
                ArgumentKind::Tab
                | ArgumentKind::Workspace
                | ArgumentKind::Session => continue,
            };
            properties.insert(arg.name.into(), schema);
            if arg.required {
                required.push(arg.name);
            }
        }
        let mut alternative =
            binding(base, command.id.as_str(), command.description, !global);
        alternative["properties"]["args"] = json!({"type":"object", "additionalProperties":false, "properties":properties, "required":required});
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
    schema["properties"]
        .as_object_mut()
        .map(|fields| fields.remove("when"));
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
        constrain_theme(&mut schema["definitions"]["ThemeDefinition"]);
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
    config["properties"]["theme"] =
        json!({"$ref":"#/definitions/SelectedTheme"});
    config["properties"]["themes"]["propertyNames"] =
        json!({"pattern":"^[A-Za-z0-9_-]+$"});
    let definitions = &mut config["definitions"];
    definitions["Config"]["properties"]["profiles"]["propertyNames"] =
        json!({"pattern":"\\S"});
    definitions["Config"]["description"] = json!(
        "Named quake windows. The built-in default profile remains available when omitted. Profile references and display identifiers are resolved at runtime."
    );
    for name in ["width", "height"] {
        definitions["Profile"]["properties"][name]["exclusiveMinimum"] =
            json!(0);
        definitions["Profile"]["properties"][name]["maximum"] = json!(1);
        definitions["Profile"]["properties"][name]["description"] = json!(
            "Fraction of the display work area, greater than zero and at most one. Fullscreen uses the full display frame."
        );
    }
    definitions["Profile"]["properties"]["animation_ms"]["maximum"] =
        json!(1000);
    definitions["Profile"]["properties"]["animation_ms"]["description"] =
        json!("Animation duration in milliseconds, from 0 to 1000.");
    definitions["Profile"]["properties"]["display"]["pattern"] =
        json!("^(active|pointer|primary|id:.*\\S.*)$");
    definitions["Profile"]["properties"]["display"]["description"] = json!(
        "Active, pointer, primary, or id:<platform display identifier>. Display availability is checked at runtime."
    );
    definitions["RawFont"]["properties"]["family"]["pattern"] = json!("\\S");
    definitions["RawFont"]["properties"]["family"]["description"] = json!(
        "Font family. Defaults to Menlo on macOS and monospace on Linux."
    );
    definitions["RawFont"]["properties"]["family"]
        .as_object_mut()
        .map(|field| field.remove("default"));
    definitions["RawFont"]["properties"]["size"]["minimum"] = json!(6);
    definitions["RawFont"]["properties"]["size"]["maximum"] = json!(96);
    definitions["UpdateConfig"]["properties"]["automatic_checks"]["description"] = json!(
        "Enable or disable scheduled update checks without Sparkle's consent prompt. Omit to preserve Sparkle's stored choice; a fresh macOS profile asks on its second launch."
    );
    definitions["UpdateConfig"]["properties"]["check_interval_hours"]["minimum"] =
        json!(1);
    definitions["UpdateConfig"]["properties"]["check_interval_hours"]["description"] = json!(
        "Scheduled update-check interval in whole hours. Omit to preserve Sparkle's stored interval; a fresh profile uses 24 hours."
    );
    for name in ["padding_x", "padding_y"] {
        definitions["WindowConfig"]["properties"][name]["minimum"] = json!(0);
        definitions["WindowConfig"]["properties"][name]["maximum"] = json!(256);
    }
    definitions["RawTerminal"]["properties"]["link_modifiers"]
        .as_object_mut()
        .map(|field| field.remove("default"));
    definitions["RawTerminal"]["properties"]["engine"]["enum"] =
        json!(["alacritty", "ghostty"]);
    definitions["RawTerminal"]["properties"]["engine"]["description"] = json!(
        "Terminal engine. Both ship in every build; reload changes newly created terminals only."
    );
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
fn constrain_theme(schema: &mut Value) {
    if let Some(fields) = schema["properties"].as_object_mut() {
        fields.remove("name");
        for (name, value) in fields {
            if name == "extends" {
                value["pattern"] = json!("^[A-Za-z0-9_-]+$");
            } else if name == "ansi" {
                value["minItems"] = json!(16);
                value["maxItems"] = json!(16);
                value["items"]["pattern"] = json!("^#[0-9a-fA-F]{6}$");
            } else {
                value["pattern"] = json!("^#[0-9a-fA-F]{6}$");
            }
        }
    }
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
