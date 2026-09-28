//! Regenerates `huterm-ghostty`'s committed FFI declarations from the pinned
//! Ghostty headers.
//!
//! Three files are written under `crates/huterm-ghostty/src/ffi/`:
//!
//! - `bindings.rs`: bindgen output limited to the functions and types that
//!   Huterm uses.
//! - `keys.rs`: one marker type per getter or option key, carrying the Rust
//!   type named by the header's `Output type:`, `Input type:`, or
//!   parenthesized annotation. A pin bump that changes a key's type changes
//!   this file.
//! - `layout.rs`: a test-only table of every struct, enum, alias, and handle
//!   in `bindings.rs`, which the ABI test compares with the linked library's
//!   `ghostty_type_json()` manifest.
//!
//! Run `mise run ghostty:bindings` to write them and
//! `mise run ghostty:bindings:check` to compare them byte for byte. Both need
//! libclang at run time, which bindgen loads dynamically.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

/// Functions Huterm calls. Their parameter and return types are pulled in
/// transitively.
const FUNCTIONS: &[&str] = &[
    "ghostty_alloc",
    "ghostty_free",
    "ghostty_type_json",
    "ghostty_build_info",
    "ghostty_terminal_new",
    "ghostty_terminal_free",
    "ghostty_terminal_reset",
    "ghostty_terminal_resize",
    "ghostty_terminal_set",
    "ghostty_terminal_get",
    "ghostty_terminal_vt_write",
    "ghostty_terminal_scroll_viewport",
    "ghostty_terminal_grid_ref",
    "ghostty_terminal_selection_format_alloc",
    "ghostty_render_state_new",
    "ghostty_render_state_free",
    "ghostty_render_state_update",
    "ghostty_render_state_get",
    "ghostty_render_state_clean",
    "ghostty_render_state_row_iterator_new",
    "ghostty_render_state_row_iterator_free",
    "ghostty_render_state_row_iterator_next",
    "ghostty_render_state_row_get",
    "ghostty_render_state_row_cells_new",
    "ghostty_render_state_row_cells_free",
    "ghostty_render_state_row_cells_next",
    "ghostty_render_state_row_cells_get",
    "ghostty_cell_get",
    "ghostty_row_get",
    "ghostty_grid_ref_cell",
    "ghostty_grid_ref_row",
    "ghostty_grid_ref_graphemes",
    "ghostty_grid_ref_hyperlink_uri",
    "ghostty_mouse_encoder_new",
    "ghostty_mouse_encoder_free",
    "ghostty_mouse_encoder_setopt",
    "ghostty_mouse_encoder_setopt_from_terminal",
    "ghostty_mouse_encoder_encode",
    "ghostty_mouse_event_new",
    "ghostty_mouse_event_free",
    "ghostty_mouse_event_set_action",
    "ghostty_mouse_event_set_button",
    "ghostty_mouse_event_clear_button",
    "ghostty_mouse_event_set_position",
];

/// Types that no allowlisted function signature mentions: callback
/// signatures, callback payloads, and values passed through `void*`.
const TYPES: &[&str] = &[
    "GhosttyAllocator",
    "GhosttyAllocatorVtable",
    "GhosttyBuffer",
    "GhosttyCellContentTag",
    "GhosttyCellWide",
    "GhosttyClipboardContent",
    "GhosttyClipboardLocation",
    "GhosttyClipboardWrite",
    "GhosttyClipboardWriteReply",
    "GhosttyClipboardWriteResult",
    "GhosttyColorPaletteIndex",
    "GhosttyColorRgb",
    "GhosttyColorScheme",
    "GhosttyDeviceAttributes",
    "GhosttyMouseEncoderSize",
    "GhosttyMouseFormat",
    "GhosttyMouseTrackingMode",
    "GhosttyOptimizeMode",
    "GhosttyRenderStateColors",
    "GhosttyRenderStateCursor",
    "GhosttyRenderStateCursorVisualStyle",
    "GhosttyRenderStateDirty",
    "GhosttySelection",
    "GhosttySgrUnderline",
    "GhosttySizeReportSize",
    "GhosttyStyle",
    "GhosttyTerminalBellFn",
    "GhosttyTerminalClipboardWriteFn",
    "GhosttyTerminalColorSchemeFn",
    "GhosttyTerminalDeviceAttributesFn",
    "GhosttyTerminalModeConfig",
    "GhosttyTerminalPwdChangedFn",
    "GhosttyTerminalScreen",
    "GhosttyTerminalScrollbar",
    "GhosttyTerminalSizeFn",
    "GhosttyTerminalTitleChangedFn",
    "GhosttyTerminalWritePtyFn",
    "GhosttyTerminalXtversionFn",
];

/// Preprocessor constants used for device-attribute answers.
const VARS: &[&str] = &[
    "GHOSTTY_DA_CONFORMANCE_VT220",
    "GHOSTTY_DA_FEATURE_ANSI_COLOR",
    "GHOSTTY_DA_DEVICE_TYPE_VT220",
];

/// How a key's annotated type is passed.
#[derive(Clone, Copy)]
enum Family {
    /// A getter: `out` points to the annotated type.
    Get { r#trait: &'static str },
    /// A setter: `value` points to the annotated type, except function
    /// pointers and `void*`, which the header passes directly.
    Set {
        value: &'static str,
        callback: Option<&'static str>,
        pointer: Option<&'static str>,
    },
}

/// The keys Huterm uses from one key enum.
struct KeySet {
    header: &'static str,
    r#enum: &'static str,
    prefix: &'static str,
    module: &'static str,
    family: Family,
    keys: &'static [&'static str],
}

const KEY_SETS: &[KeySet] = &[
    KeySet {
        header: "terminal.h",
        r#enum: "GhosttyTerminalData",
        prefix: "GHOSTTY_TERMINAL_DATA_",
        module: "terminal_data",
        family: Family::Get {
            r#trait: "TerminalData",
        },
        keys: &[
            "COLS",
            "ROWS",
            "ACTIVE_SCREEN",
            "SCROLLBAR",
            "MOUSE_TRACKING",
            "TITLE",
            "PWD",
            "TOTAL_ROWS",
            "SCROLLBACK_ROWS",
            "COLOR_FOREGROUND",
            "COLOR_BACKGROUND",
            "COLOR_CURSOR",
            "COLOR_PALETTE",
            "COLOR_FOREGROUND_DEFAULT",
            "COLOR_BACKGROUND_DEFAULT",
            "COLOR_CURSOR_DEFAULT",
            "COLOR_PALETTE_DEFAULT",
            "KITTY_IMAGE_STORAGE_LIMIT",
            "SCROLLBACK_MAX_BYTES",
            "MODE",
            "VT_GROUND",
        ],
    },
    KeySet {
        header: "terminal.h",
        r#enum: "GhosttyTerminalOption",
        prefix: "GHOSTTY_TERMINAL_OPT_",
        module: "terminal_option",
        family: Family::Set {
            value: "TerminalOption",
            callback: Some("TerminalCallback"),
            pointer: Some("TerminalPointer"),
        },
        keys: &[
            "USERDATA",
            "WRITE_PTY",
            "BELL",
            "XTVERSION",
            "TITLE_CHANGED",
            "SIZE",
            "COLOR_SCHEME",
            "DEVICE_ATTRIBUTES",
            "COLOR_FOREGROUND",
            "COLOR_BACKGROUND",
            "COLOR_CURSOR",
            "COLOR_PALETTE",
            "KITTY_IMAGE_STORAGE_LIMIT",
            "APC_MAX_BYTES",
            "GLYPH_PROTOCOL",
            "PWD_CHANGED",
            "CLIPBOARD_WRITE",
            "SCROLLBACK_MAX_BYTES",
            "CLIPBOARD_WRITE_MAX_BYTES",
        ],
    },
    KeySet {
        header: "render.h",
        r#enum: "GhosttyRenderStateData",
        prefix: "GHOSTTY_RENDER_STATE_DATA_",
        module: "render_state_data",
        family: Family::Get {
            r#trait: "RenderStateData",
        },
        keys: &["DIRTY", "ROW_ITERATOR", "CURSOR", "COLORS"],
    },
    KeySet {
        header: "render.h",
        r#enum: "GhosttyRenderStateRowData",
        prefix: "GHOSTTY_RENDER_STATE_ROW_DATA_",
        module: "render_row_data",
        family: Family::Get {
            r#trait: "RenderRowData",
        },
        keys: &["DIRTY", "RAW", "CELLS"],
    },
    KeySet {
        header: "render.h",
        r#enum: "GhosttyRenderStateRowCellsData",
        prefix: "GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_",
        module: "render_cell_data",
        family: Family::Get {
            r#trait: "RenderCellData",
        },
        keys: &["RAW", "STYLE", "GRAPHEMES_UTF8"],
    },
    KeySet {
        header: "screen.h",
        r#enum: "GhosttyCellData",
        prefix: "GHOSTTY_CELL_DATA_",
        module: "cell_data",
        family: Family::Get {
            r#trait: "CellData",
        },
        keys: &[
            "CODEPOINT",
            "CONTENT_TAG",
            "WIDE",
            "HAS_TEXT",
            "HAS_STYLING",
            "HAS_HYPERLINK",
            "COLOR_PALETTE",
            "COLOR_RGB",
        ],
    },
    KeySet {
        header: "screen.h",
        r#enum: "GhosttyRowData",
        prefix: "GHOSTTY_ROW_DATA_",
        module: "row_data",
        family: Family::Get { r#trait: "RowData" },
        keys: &["WRAP", "GRAPHEME", "STYLED"],
    },
    KeySet {
        header: "mouse/encoder.h",
        r#enum: "GhosttyMouseEncoderOption",
        prefix: "GHOSTTY_MOUSE_ENCODER_OPT_",
        module: "mouse_encoder_option",
        family: Family::Set {
            value: "MouseEncoderOption",
            callback: None,
            pointer: None,
        },
        keys: &["EVENT", "SIZE", "ANY_BUTTON_PRESSED", "TRACK_LAST_CELL"],
    },
    KeySet {
        header: "build_info.h",
        r#enum: "GhosttyBuildInfo",
        prefix: "GHOSTTY_BUILD_INFO_",
        module: "build_info",
        family: Family::Get {
            r#trait: "BuildInfo",
        },
        keys: &["OPTIMIZE"],
    },
];

type Error = String;

fn main() -> ExitCode {
    let check = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("--check") => true,
        Some(other) => {
            eprintln!(
                "usage: huterm-ghostty-bindgen [--check] (got {other:?})"
            );
            return ExitCode::from(2);
        }
    };
    match run(check) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("ghostty bindings failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(check: bool) -> Result<bool, Error> {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("tool must live inside crates/huterm-ghostty")?
        .to_path_buf();
    let workspace = crate_dir
        .parent()
        .and_then(Path::parent)
        .ok_or("crate must live inside the workspace")?
        .to_path_buf();
    let source = std::env::var_os("GHOSTTY_SOURCE_DIR")
        .map(PathBuf::from)
        .ok_or(
            "GHOSTTY_SOURCE_DIR must be set; run through `mise run \
             ghostty:bindings`",
        )?;
    let include = source.join("include");
    let config = workspace.join("rustfmt.toml");

    let bindings = generate_bindings(&include)?;
    let file = syn::parse_file(&bindings)
        .map_err(|error| format!("parsing bindgen output: {error}"))?;
    let keys = generate_keys(&include.join("ghostty/vt"))?;
    let sized = sized_structs(&file);
    let layout = generate_layout(&file)?;

    let outputs = [
        (
            "bindings.rs",
            format!("{}{bindings}", header("bindgen output")),
        ),
        (
            "keys.rs",
            format!("{}{keys}{sized}", header("header key annotations")),
        ),
        ("layout.rs", format!("{}{layout}", header("bindgen output"))),
    ];
    let directory = crate_dir.join("src/ffi");
    let mut current = true;
    for (name, contents) in outputs {
        let formatted = rustfmt(&contents, &config)?;
        let path = directory.join(name);
        if check {
            let existing = std::fs::read(&path).unwrap_or_default();
            if existing != formatted.as_bytes() {
                eprintln!(
                    "{} is stale; run `mise run ghostty:bindings`",
                    path.display()
                );
                current = false;
            }
        } else {
            std::fs::write(&path, formatted).map_err(|error| {
                format!("writing {}: {error}", path.display())
            })?;
            println!("wrote {}", path.display());
        }
    }
    Ok(current)
}

fn header(source: &str) -> String {
    format!(
        "// @generated by huterm-ghostty-bindgen from Ghostty's C headers \
         ({source}).\n// Do not edit; run `mise run ghostty:bindings`.\n\n"
    )
}

fn generate_bindings(include: &Path) -> Result<String, Error> {
    let mut builder = bindgen::Builder::default()
        .header_contents("huterm-ghostty.h", "#include <ghostty/vt.h>\n")
        .clang_arg(format!("-I{}", include.display()))
        .clang_arg("-DGHOSTTY_STATIC")
        // types.h gives every enum an `int` underlying type in C++11 and
        // C23, but older C compilers pick one, and libclang 14 (Ubuntu
        // 22.04) chooses `unsigned int`. C++ mode matches the library's
        // `c_int` enums on every libclang.
        .clang_args(["-x", "c++", "-std=c++17"])
        .rust_target(
            bindgen::RustTarget::stable(85, 0)
                .map_err(|error| error.to_string())?,
        )
        .rust_edition(bindgen::RustEdition::Edition2024)
        .default_enum_style(bindgen::EnumVariation::Consts)
        .prepend_enum_name(false)
        .ctypes_prefix("::core::ffi")
        .use_core()
        .derive_default(true)
        .derive_debug(true)
        .derive_copy(true)
        .impl_debug(false)
        .layout_tests(false)
        .generate_comments(false)
        .disable_header_comment()
        .merge_extern_blocks(true)
        .sort_semantically(true)
        .formatter(bindgen::Formatter::None);
    for function in FUNCTIONS {
        builder = builder.allowlist_function(format!("^{function}$"));
    }
    for r#type in TYPES
        .iter()
        .copied()
        .chain(KEY_SETS.iter().map(|set| set.r#enum))
    {
        builder = builder.allowlist_type(format!("^{type}$"));
    }
    for var in VARS {
        builder = builder.allowlist_var(format!("^{var}$"));
    }
    let bindings = builder
        .generate()
        .map_err(|error| format!("bindgen: {error}"))?;
    Ok(bindings.to_string())
}

/// One constant parsed from a header's `typedef enum` block.
struct Constant {
    doc: String,
}

/// Collects documented constants from every `typedef enum` in `header`.
fn header_constants(header: &str) -> Result<BTreeMap<String, Constant>, Error> {
    let mut constants = BTreeMap::new();
    let mut rest = header;
    while let Some(start) = rest.find("typedef enum") {
        rest = &rest[start..];
        let open = rest.find('{').ok_or("unterminated enum")?;
        let close = rest.find('}').ok_or("unterminated enum")?;
        let body = &rest[open + 1..close];
        rest = &rest[close..];
        let mut doc = String::new();
        let mut index = 0;
        while index < body.len() {
            let tail = &body[index..];
            if let Some(comment) = tail.strip_prefix("/*") {
                let end = comment.find("*/").ok_or("unterminated comment")?;
                doc = normalize_doc(&comment[..end]);
                index += 2 + end + 2;
            } else if tail.starts_with("GHOSTTY_") {
                let end = tail
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(tail.len());
                constants.insert(
                    tail[..end].to_owned(),
                    Constant {
                        doc: std::mem::take(&mut doc),
                    },
                );
                let next = tail.find(',').unwrap_or(tail.len());
                index += next.max(end);
            } else {
                index += tail.chars().next().map_or(1, char::len_utf8);
            }
        }
    }
    Ok(constants)
}

fn normalize_doc(comment: &str) -> String {
    comment
        .lines()
        .map(|line| {
            line.trim()
                .trim_start_matches('*')
                .trim_start_matches('<')
                .trim()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The type a key annotation names, and how the C API passes it.
#[derive(Debug, Eq, PartialEq)]
enum Annotated {
    /// `out`/`value` points to this C type.
    Pointee(String),
    /// The pointer value itself is the argument: a callback or `void*`.
    Direct(String),
}

fn annotation(doc: &str) -> Option<Annotated> {
    for label in ["Input/output type:", "Output type:", "Input type:"] {
        if let Some(position) = doc.find(label) {
            let text = doc[position + label.len()..].trim();
            // `GhosttyKittyKeyFlags * (uint8_t *)` names the typedef first.
            let text = text.split(" (").next().unwrap_or(text).trim();
            let text = text.split_whitespace().collect::<String>();
            if text == "void*" {
                return Some(Annotated::Direct(text));
            }
            return Some(match text.strip_suffix('*') {
                Some(pointee) => Annotated::Pointee(pointee.to_owned()),
                None => Annotated::Direct(text),
            });
        }
    }
    // render.h and mouse/encoder.h put the value type in parentheses.
    let mut rest = doc;
    while let Some(open) = rest.find('(') {
        let tail = &rest[open + 1..];
        let close = tail.find(')')?;
        let inner = tail[..close].trim();
        let inner = inner.strip_prefix("value:").unwrap_or(inner).trim();
        if is_c_type(inner) {
            return Some(Annotated::Pointee(inner.to_owned()));
        }
        rest = &tail[close + 1..];
    }
    None
}

fn is_c_type(text: &str) -> bool {
    let base = text.split('[').next().unwrap_or(text);
    let scalar = matches!(
        base,
        "bool" | "uint8_t" | "uint16_t" | "uint32_t" | "uint64_t" | "size_t"
    );
    let named = base.starts_with("Ghostty")
        && base.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    (scalar || named)
        && text[base.len()..]
            .chars()
            .all(|c| c.is_ascii_digit() || c == '[' || c == ']')
}

fn rust_type(c_type: &str) -> Result<String, Error> {
    if let Some((element, count)) = c_type.split_once('[') {
        let count = count
            .strip_suffix(']')
            .ok_or_else(|| format!("malformed array type {c_type}"))?;
        return Ok(format!("[{}; {count}]", rust_type(element)?));
    }
    Ok(match c_type {
        "bool" => "bool".to_owned(),
        "uint8_t" => "u8".to_owned(),
        "uint16_t" => "u16".to_owned(),
        "uint32_t" => "u32".to_owned(),
        "uint64_t" => "u64".to_owned(),
        "size_t" => "usize".to_owned(),
        "void*" => "*mut ::core::ffi::c_void".to_owned(),
        named if named.starts_with("Ghostty") => format!("ffi::{named}"),
        other => return Err(format!("unsupported annotated type {other}")),
    })
}

fn marker_name(key: &str) -> String {
    key.split('_')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_string() + &chars.as_str().to_ascii_lowercase()
            })
        })
        .collect()
}

fn generate_keys(headers: &Path) -> Result<String, Error> {
    let mut out = String::from(
        "//! Key markers and the Rust type each header annotation names.\n\n\
         use super::bindings as ffi;\n",
    );
    for set in KEY_SETS {
        let path = headers.join(set.header);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("reading {}: {error}", path.display()))?;
        let constants = header_constants(&text)?;
        let _ = write!(
            out,
            "\n/// Keys of `{}` from `{}`.\npub(crate) mod {} {{\n    use super::ffi;\n",
            set.r#enum, set.header, set.module
        );
        for key in set.keys {
            let name = format!("{}{key}", set.prefix);
            let constant = constants.get(&name).ok_or_else(|| {
                format!("{name} is missing from {}", set.header)
            })?;
            let annotated = annotation(&constant.doc).ok_or_else(|| {
                format!("{name} in {} has no type annotation", set.header)
            })?;
            let marker = marker_name(key);
            let (r#trait, item, rust) = match (set.family, annotated) {
                (Family::Get { r#trait }, Annotated::Pointee(c_type)) => {
                    (r#trait, "Out", rust_type(&c_type)?)
                }
                (Family::Set { value, .. }, Annotated::Pointee(c_type)) => {
                    (value, "Value", rust_type(&c_type)?)
                }
                (
                    Family::Set {
                        pointer: Some(pointer),
                        ..
                    },
                    Annotated::Direct(c_type),
                ) if c_type == "void*" => (pointer, "", String::new()),
                (
                    Family::Set {
                        callback: Some(callback),
                        ..
                    },
                    Annotated::Direct(c_type),
                ) if c_type.starts_with("Ghostty")
                    && c_type.ends_with("Fn") =>
                {
                    (callback, "Callback", rust_type(&c_type)?)
                }
                (_, other) => {
                    return Err(format!(
                        "{name} has an unsupported annotation {other:?}"
                    ));
                }
            };
            let _ = write!(
                out,
                "\n    /// `{name}`.\n    #[derive(Debug)]\n    pub(crate) enum {marker} {{}}\n\n    \
                 impl super::super::{trait} for {marker} {{\n        \
                 const KEY: ffi::{enum} = ffi::{name};\n",
                r#trait = r#trait,
                r#enum = set.r#enum,
            );
            if !item.is_empty() {
                let _ = writeln!(out, "        type {item} = {rust};");
            }
            out.push_str("    }\n");
        }
        out.push_str("}\n");
    }
    Ok(out)
}

/// Implements `SizedStruct` for every struct whose first field is
/// `size: usize`, the header's sized-struct convention.
fn sized_structs(file: &syn::File) -> String {
    let mut out = String::new();
    for item in &file.items {
        if let syn::Item::Struct(item) = item
            && let syn::Fields::Named(fields) = &item.fields
            && let Some(first) = fields.named.first()
            && first.ident.as_ref().is_some_and(|ident| ident == "size")
            && type_text(&first.ty) == "usize"
        {
            let name = &item.ident;
            let _ = write!(
                out,
                "\nimpl super::SizedStruct for ffi::{name} {{\n    \
                 fn size_mut(&mut self) -> &mut usize {{\n        &mut self.size\n    }}\n}}\n"
            );
        }
    }
    out
}

fn type_text(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(path) => path
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>()
            .join("::"),
        syn::Type::Ptr(_) => "*".to_owned(),
        syn::Type::Array(_) => "[]".to_owned(),
        _ => "?".to_owned(),
    }
}

/// Items in the bindings, grouped by how the ABI test checks them.
#[derive(Default)]
struct Items {
    structs: Vec<(String, Vec<String>)>,
    unions: Vec<String>,
    /// Enums: `c_int` typedefs with constants of that type.
    enums: Vec<(String, Vec<String>)>,
    aliases: Vec<String>,
    handles: Vec<String>,
}

fn collect_items(file: &syn::File) -> Result<Items, Error> {
    let mut items = Items::default();
    let mut typedefs = Vec::new();
    let mut constants: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for item in &file.items {
        match item {
            syn::Item::Struct(item) => {
                let syn::Fields::Named(fields) = &item.fields else {
                    return Err(format!("{} has unnamed fields", item.ident));
                };
                let names: Vec<String> = fields
                    .named
                    .iter()
                    .filter_map(|field| {
                        field.ident.as_ref().map(ToString::to_string)
                    })
                    .collect();
                // Opaque handle targets are zero-sized placeholders.
                if !names.iter().any(|name| name == "_unused") {
                    items.structs.push((item.ident.to_string(), names));
                }
            }
            syn::Item::Union(item) => items.unions.push(item.ident.to_string()),
            syn::Item::Const(item) => {
                constants
                    .entry(type_text(&item.ty))
                    .or_default()
                    .push(item.ident.to_string());
            }
            syn::Item::Type(item) => match &*item.ty {
                syn::Type::Ptr(pointer) => {
                    if let syn::Type::Path(path) = &*pointer.elem
                        && path.path.segments.last().is_some_and(|segment| {
                            segment.ident.to_string().ends_with("Impl")
                        })
                    {
                        items.handles.push(item.ident.to_string());
                    }
                }
                syn::Type::Path(_) => {
                    typedefs
                        .push((item.ident.to_string(), type_text(&item.ty)));
                }
                _ => {}
            },
            _ => {}
        }
    }
    for (name, target) in typedefs {
        if let Some(values) = constants.remove(&name) {
            items.enums.push((name, values));
        } else if !target.ends_with("Option") {
            // Function-pointer typedefs appear as `Option<...>`; the
            // manifest does not describe them.
            items.aliases.push(name);
        }
    }
    if items.handles.is_empty() {
        return Err("no opaque handle types found in the bindings".to_owned());
    }
    Ok(items)
}

fn emit_sized(out: &mut String, doc: &str, constant: &str, names: &[String]) {
    let _ = write!(
        out,
        "\n/// {doc}\npub(crate) const {constant}: &[Sized] = &[\n"
    );
    for name in names {
        let _ = writeln!(
            out,
            "    Sized {{ name: \"{name}\", size: size_of::<ffi::{name}>(), align: align_of::<ffi::{name}>() }},"
        );
    }
    out.push_str("];\n");
}

/// Emits size, alignment, field offsets, enum values, alias sizes, and
/// handle sizes for every item in the bindings.
fn generate_layout(file: &syn::File) -> Result<String, Error> {
    let items = collect_items(file)?;
    let mut out = String::from(
        "//! Rust layouts of every FFI type, compared with the linked library's\n\
         //! `ghostty_type_json()` manifest by the ABI test.\n\n\
         use core::mem::{align_of, offset_of, size_of};\n\n\
         use super::bindings as ffi;\n\n\
         pub(crate) struct Struct {\n    pub(crate) name: &'static str,\n    \
         pub(crate) size: usize,\n    pub(crate) align: usize,\n    \
         pub(crate) fields: &'static [(&'static str, usize)],\n}\n\n\
         pub(crate) struct Sized {\n    pub(crate) name: &'static str,\n    \
         pub(crate) size: usize,\n    pub(crate) align: usize,\n}\n\n\
         pub(crate) struct Enum {\n    pub(crate) name: &'static str,\n    \
         pub(crate) size: usize,\n    pub(crate) align: usize,\n    \
         pub(crate) values: &'static [(&'static str, i64)],\n}\n\n\
         pub(crate) const STRUCTS: &[Struct] = &[\n",
    );
    for (name, fields) in &items.structs {
        let _ = write!(
            out,
            "    Struct {{\n        name: \"{name}\",\n        size: size_of::<ffi::{name}>(),\n        \
             align: align_of::<ffi::{name}>(),\n        fields: &["
        );
        for field in fields {
            let _ = write!(
                out,
                "(\"{field}\", offset_of!(ffi::{name}, {field})), "
            );
        }
        out.push_str("],\n    },\n");
    }
    out.push_str("];\n\npub(crate) const ENUMS: &[Enum] = &[\n");
    for (name, values) in &items.enums {
        let _ = write!(
            out,
            "    Enum {{\n        name: \"{name}\",\n        size: size_of::<ffi::{name}>(),\n        \
             align: align_of::<ffi::{name}>(),\n        values: &["
        );
        for value in values {
            let _ = write!(out, "(\"{value}\", ffi::{value} as i64), ");
        }
        out.push_str("],\n    },\n");
    }
    out.push_str("];\n");
    emit_sized(
        &mut out,
        "Unions: size and alignment only; every field sits at offset zero.",
        "UNIONS",
        &items.unions,
    );
    emit_sized(
        &mut out,
        "Scalar typedefs such as `GhosttyCell` and `GhosttyMode`.",
        "ALIASES",
        &items.aliases,
    );
    emit_sized(
        &mut out,
        "Opaque handle typedefs.",
        "HANDLES",
        &items.handles,
    );
    Ok(out)
}

fn rustfmt(source: &str, config: &Path) -> Result<String, Error> {
    let rustfmt =
        std::env::var_os("RUSTFMT").unwrap_or_else(|| "rustfmt".into());
    let mut child = Command::new(rustfmt)
        .args(["--edition", "2024", "--emit", "stdout", "--config-path"])
        .arg(config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("running rustfmt: {error}"))?;
    child
        .stdin
        .take()
        .ok_or("rustfmt stdin unavailable")?
        .write_all(source.as_bytes())
        .map_err(|error| format!("writing to rustfmt: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("waiting for rustfmt: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "rustfmt failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotations_cover_every_header_style() {
        let cases = [
            (
                "Terminal width in cells. Output type: uint16_t *",
                Annotated::Pointee("uint16_t".into()),
            ),
            (
                "Current Kitty keyboard protocol flags. Output type: GhosttyKittyKeyFlags * (uint8_t *)",
                Annotated::Pointee("GhosttyKittyKeyFlags".into()),
            ),
            (
                "Get a mode. Input/output type: GhosttyTerminalModeConfig *",
                Annotated::Pointee("GhosttyTerminalModeConfig".into()),
            ),
            (
                "Input type: GhosttyColorRgb[256]*",
                Annotated::Pointee("GhosttyColorRgb[256]".into()),
            ),
            (
                "Input type: GhosttyTerminalWritePtyFn",
                Annotated::Direct("GhosttyTerminalWritePtyFn".into()),
            ),
            (
                "Opaque userdata. Input type: void*",
                Annotated::Direct("void*".into()),
            ),
            (
                "Viewport width in cells (uint16_t).",
                Annotated::Pointee("uint16_t".into()),
            ),
            (
                "Populate a pre-allocated GhosttyRenderStateRowIterator with row data (GhosttyRenderStateRowIterator). Row data is only valid (sometimes).",
                Annotated::Pointee("GhosttyRenderStateRowIterator".into()),
            ),
            (
                "Mouse tracking mode (value: GhosttyMouseTrackingMode).",
                Annotated::Pointee("GhosttyMouseTrackingMode".into()),
            ),
        ];
        for (doc, expected) in cases {
            assert_eq!(annotation(doc), Some(expected), "{doc}");
        }
        assert_eq!(
            annotation("Write codepoints into a buffer (uint32_t*)."),
            None
        );
        assert_eq!(annotation("Invalid data type."), None);
    }

    #[test]
    fn header_constants_attach_the_preceding_comment() {
        let header = "typedef enum GHOSTTY_ENUM_TYPED {\n  /** Invalid. */\n  GHOSTTY_X_INVALID = 0,\n\n  /**\n   * Width.\n   *\n   * Output type: uint16_t *\n   */\n  GHOSTTY_X_COLS = 1,\n  GHOSTTY_X_MAX_VALUE = GHOSTTY_ENUM_MAX_VALUE,\n} GhosttyX;";
        let constants = header_constants(header).unwrap();
        assert!(constants["GHOSTTY_X_COLS"].doc.contains("Width."));
        assert_eq!(
            annotation(&constants["GHOSTTY_X_COLS"].doc),
            Some(Annotated::Pointee("uint16_t".into()))
        );
        assert!(constants["GHOSTTY_X_MAX_VALUE"].doc.is_empty());
    }

    #[test]
    fn types_and_markers_follow_the_c_names() {
        assert_eq!(
            rust_type("GhosttyColorRgb[256]").unwrap(),
            "[ffi::GhosttyColorRgb; 256]"
        );
        assert_eq!(rust_type("size_t").unwrap(), "usize");
        assert!(rust_type("int").is_err());
        assert_eq!(marker_name("COLOR_PALETTE_DEFAULT"), "ColorPaletteDefault");
        assert_eq!(marker_name("VT_GROUND"), "VtGround");
    }
}
