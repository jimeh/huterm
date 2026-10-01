//! Regenerates `huterm-ghostty`'s committed FFI declarations from the pinned
//! Ghostty headers.
//!
//! Three files are written under `crates/huterm-ghostty/src/ffi/`:
//!
//! - `bindings.rs`: bindgen output limited to the functions and types that
//!   Huterm uses.
//! - `keys.rs`: one marker type per getter or option key, carrying the Rust
//!   type named by the key's header annotation. Each header set declares
//!   where its annotations sit, and a comment without one in that position
//!   fails generation. A pin bump that changes a key's type changes this
//!   file. Getter outputs that carry a mutable pointer, which the library
//!   writes through, get a separate trait so only dedicated wrappers use
//!   them.
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
    /// A getter: `out` points to the annotated type. Outputs that carry a
    /// mutable pointer implement `populate` instead of `trait`.
    Get {
        r#trait: &'static str,
        populate: Option<&'static str>,
    },
    /// A setter: `value` points to the annotated type, except function
    /// pointers and `void*`, which the header passes directly.
    Set {
        value: &'static str,
        callback: Option<&'static str>,
        pointer: Option<&'static str>,
    },
}

/// Where a header documents each key's type.
#[derive(Clone, Copy)]
enum Style {
    /// The comment's last line: `Output type: T *`, `Input type: T*`,
    /// `Input/output type: T *`, or a callback or `void*` input.
    Labeled,
    /// A parenthesized type, optionally `value:`-prefixed, that ends the
    /// comment's first sentence: `Viewport width in cells (uint16_t).`
    Sentence,
}

/// The keys Huterm uses from one key enum.
struct KeySet {
    header: &'static str,
    r#enum: &'static str,
    prefix: &'static str,
    module: &'static str,
    style: Style,
    family: Family,
    keys: &'static [&'static str],
}

const KEY_SETS: &[KeySet] = &[
    KeySet {
        header: "terminal.h",
        r#enum: "GhosttyTerminalData",
        prefix: "GHOSTTY_TERMINAL_DATA_",
        module: "terminal_data",
        style: Style::Labeled,
        family: Family::Get {
            r#trait: "TerminalData",
            populate: None,
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
            "CLIPBOARD_WRITE_MAX_BYTES",
        ],
    },
    KeySet {
        header: "terminal.h",
        r#enum: "GhosttyTerminalOption",
        prefix: "GHOSTTY_TERMINAL_OPT_",
        module: "terminal_option",
        style: Style::Labeled,
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
        style: Style::Sentence,
        family: Family::Get {
            r#trait: "RenderStateData",
            populate: Some("RenderStatePopulate"),
        },
        keys: &["DIRTY", "ROW_ITERATOR", "CURSOR", "COLORS"],
    },
    KeySet {
        header: "render.h",
        r#enum: "GhosttyRenderStateRowData",
        prefix: "GHOSTTY_RENDER_STATE_ROW_DATA_",
        module: "render_row_data",
        style: Style::Sentence,
        family: Family::Get {
            r#trait: "RenderRowData",
            populate: Some("RenderRowPopulate"),
        },
        keys: &["DIRTY", "RAW", "CELLS"],
    },
    KeySet {
        header: "render.h",
        r#enum: "GhosttyRenderStateRowCellsData",
        prefix: "GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_",
        module: "render_cell_data",
        style: Style::Sentence,
        family: Family::Get {
            r#trait: "RenderCellData",
            populate: Some("RenderCellPopulate"),
        },
        keys: &["RAW", "STYLE", "GRAPHEMES_UTF8"],
    },
    KeySet {
        header: "screen.h",
        r#enum: "GhosttyCellData",
        prefix: "GHOSTTY_CELL_DATA_",
        module: "cell_data",
        style: Style::Labeled,
        family: Family::Get {
            r#trait: "CellData",
            populate: None,
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
        style: Style::Labeled,
        family: Family::Get {
            r#trait: "RowData",
            populate: None,
        },
        keys: &["WRAP", "GRAPHEME", "STYLED"],
    },
    KeySet {
        header: "mouse/encoder.h",
        r#enum: "GhosttyMouseEncoderOption",
        prefix: "GHOSTTY_MOUSE_ENCODER_OPT_",
        module: "mouse_encoder_option",
        style: Style::Sentence,
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
        style: Style::Labeled,
        family: Family::Get {
            r#trait: "BuildInfo",
            populate: None,
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

    let libclang = load_libclang()?;
    match std::env::var("LIBCLANG_PATH") {
        Ok(path) => eprintln!("libclang: {libclang} from {path}"),
        Err(_) => eprintln!("libclang: {libclang}"),
    }
    let bindings = generate_bindings(&include)?;
    let file = syn::parse_file(&bindings)
        .map_err(|error| format!("parsing bindgen output: {error}"))?;
    let keys = generate_keys(&include.join("ghostty/vt"), &file)?;
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

/// Loads libclang and returns its version. bindgen panics when it cannot
/// find the library; this turns that into setup instructions.
fn load_libclang() -> Result<String, Error> {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let loaded = std::panic::catch_unwind(bindgen::clang_version);
    std::panic::set_hook(hook);
    loaded
        .map(|version| version.full)
        .map_err(|payload| libclang_error(&*payload))
}

fn libclang_error(payload: &(dyn std::any::Any + Send)) -> Error {
    let detail = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("no details");
    format!(
        "could not load libclang ({detail}).\n\
         \x20 macOS: select an Xcode or Command Line Tools with `xcode-select`; \
         `mise run ghostty:bindings` uses its libclang.\n\
         \x20 Ubuntu 22.04: sudo apt-get install --no-install-recommends \
         libclang1-14\n\
         \x20 Otherwise: set LIBCLANG_PATH to the directory containing libclang."
    )
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
    /// The preceding doc comment's lines, without comment markers.
    lines: Vec<String>,
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
        let mut lines = Vec::new();
        let mut index = 0;
        while index < body.len() {
            let tail = &body[index..];
            if let Some(comment) = tail.strip_prefix("/*") {
                let end = comment.find("*/").ok_or("unterminated comment")?;
                lines = comment_lines(&comment[..end]);
                index += 2 + end + 2;
            } else if tail.starts_with("GHOSTTY_") {
                let end = tail
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(tail.len());
                constants.insert(
                    tail[..end].to_owned(),
                    Constant {
                        lines: std::mem::take(&mut lines),
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

/// A comment's lines without `*` or `<` markers; blank lines separate
/// paragraphs.
fn comment_lines(comment: &str) -> Vec<String> {
    comment
        .lines()
        .map(|line| {
            line.trim()
                .trim_start_matches('*')
                .trim_start_matches('<')
                .trim()
                .to_owned()
        })
        .collect()
}

/// The type a key annotation names, and how the C API passes it.
#[derive(Debug, Eq, PartialEq)]
enum Annotated {
    /// `out`/`value` points to this C type.
    Pointee(String),
    /// The pointer value itself is the argument: a callback or `void*`.
    Direct(String),
}

const LABELS: [&str; 3] = ["Input/output type:", "Output type:", "Input type:"];

fn annotation(style: Style, lines: &[String]) -> Result<Annotated, Error> {
    match style {
        Style::Labeled => labeled_annotation(lines),
        Style::Sentence => sentence_annotation(lines),
    }
}

/// Reads the comment's last line, which must be the only labeled line.
fn labeled_annotation(lines: &[String]) -> Result<Annotated, Error> {
    let labeled = lines
        .iter()
        .filter(|line| LABELS.iter().any(|label| line.contains(label)))
        .count();
    let last = lines
        .iter()
        .rev()
        .find(|line| !line.is_empty())
        .ok_or("empty comment")?;
    let text = LABELS
        .iter()
        .find_map(|label| last.strip_prefix(label))
        .filter(|_| labeled == 1)
        .ok_or("the last line is not the comment's only type label")?;
    // `GhosttyKittyKeyFlags * (uint8_t *)` names the typedef first and its
    // representation in parentheses.
    let text = match text.split_once('(') {
        Some((named, representation))
            if representation
                .strip_suffix(')')
                .is_some_and(|inner| inner.trim_end().ends_with('*')) =>
        {
            named
        }
        Some(_) => return Err(format!("malformed type label {text:?}")),
        None => text,
    };
    let text = text.split_whitespace().collect::<String>();
    if text == "void*" {
        return Ok(Annotated::Direct(text));
    }
    match text.strip_suffix('*') {
        Some(pointee) if is_c_type(pointee) => {
            Ok(Annotated::Pointee(pointee.to_owned()))
        }
        None if is_c_type(&text) => Ok(Annotated::Direct(text)),
        _ => Err(format!("unsupported labeled type {text:?}")),
    }
}

/// Reads the parenthesized type that ends the first sentence.
fn sentence_annotation(lines: &[String]) -> Result<Annotated, Error> {
    let text = lines
        .iter()
        .filter(|line| !line.is_empty())
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    let mut depth = 0_usize;
    let mut end = None;
    for (index, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '.' if depth == 0
                && text[index + 1..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace) =>
            {
                end = Some(index);
                break;
            }
            _ => {}
        }
    }
    let sentence = &text[..end.ok_or("no complete first sentence")?];
    let inner = sentence
        .strip_suffix(')')
        .and_then(|head| head.rsplit_once('('))
        .map(|(_, inner)| inner.trim())
        .ok_or("the first sentence does not end with a parenthesized type")?;
    let inner = inner.strip_prefix("value:").unwrap_or(inner).trim();
    if is_c_type(inner) {
        Ok(Annotated::Pointee(inner.to_owned()))
    } else {
        Err(format!("unsupported sentence type {inner:?}"))
    }
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

/// Raw pointers reachable inside a type without following them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Pointers {
    any: bool,
    mutable: bool,
}

impl Pointers {
    const fn or(self, other: Self) -> Self {
        Self {
            any: self.any || other.any,
            mutable: self.mutable || other.mutable,
        }
    }
}

/// Finds the raw pointers inside the binding type named `name`, including
/// nested structs, unions, arrays, and handle typedefs.
fn pointers(file: &syn::File, name: &str) -> Pointers {
    let base = name.split('[').next().unwrap_or(name);
    file.items
        .iter()
        .find_map(|item| match item {
            syn::Item::Struct(item) if item.ident == base => Some(
                item.fields
                    .iter()
                    .fold(Pointers::default(), |found, field| {
                        found.or(type_pointers(file, &field.ty))
                    }),
            ),
            syn::Item::Union(item) if item.ident == base => Some(
                item.fields
                    .named
                    .iter()
                    .fold(Pointers::default(), |found, field| {
                        found.or(type_pointers(file, &field.ty))
                    }),
            ),
            syn::Item::Type(item) if item.ident == base => {
                Some(type_pointers(file, &item.ty))
            }
            _ => None,
        })
        .unwrap_or_default()
}

fn type_pointers(file: &syn::File, ty: &syn::Type) -> Pointers {
    match ty {
        syn::Type::Ptr(pointer) => Pointers {
            any: true,
            mutable: matches!(
                pointer.mutability,
                syn::PointerMutability::Mut(_)
            ),
        },
        syn::Type::Array(array) => type_pointers(file, &array.elem),
        syn::Type::Path(path) => match path.path.segments.last() {
            // Callback typedefs are `Option<unsafe extern "C" fn ...>`.
            Some(segment) if segment.ident == "Option" => Pointers {
                any: true,
                mutable: false,
            },
            Some(segment) => pointers(file, &segment.ident.to_string()),
            None => Pointers::default(),
        },
        _ => Pointers::default(),
    }
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

/// Chooses the trait and associated item for one key.
fn key_impl(
    family: Family,
    annotated: Annotated,
    file: &syn::File,
    name: &str,
) -> Result<(&'static str, &'static str, String), Error> {
    Ok(match (family, annotated) {
        (Family::Get { r#trait, populate }, Annotated::Pointee(c_type)) => {
            let r#trait = if pointers(file, &c_type).mutable {
                populate.ok_or_else(|| {
                    format!(
                        "{name} outputs {c_type}, which carries a pointer the \
                         library writes through, but its key set has no \
                         populate trait"
                    )
                })?
            } else {
                r#trait
            };
            (r#trait, "Out", rust_type(&c_type)?)
        }
        (Family::Set { value, .. }, Annotated::Pointee(c_type)) => {
            if pointers(file, &c_type).any {
                return Err(format!(
                    "{name} takes {c_type}, which carries a pointer the \
                     library reads through; give it a dedicated wrapper"
                ));
            }
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
        ) if c_type.starts_with("Ghostty") && c_type.ends_with("Fn") => {
            (callback, "Callback", rust_type(&c_type)?)
        }
        (_, other) => {
            return Err(format!(
                "{name} has an unsupported annotation {other:?}"
            ));
        }
    })
}

fn generate_keys(headers: &Path, file: &syn::File) -> Result<String, Error> {
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
            "\n/// Keys of `{}` from `{}`.\npub(crate) mod {} {{\n    use super::ffi;\n\n    \
             /// Every key below, for tests that must cover them all.\n    \
             #[cfg(test)]\n    pub(crate) const KEYS: &[&str] = &[{}];\n",
            set.r#enum,
            set.header,
            set.module,
            set.keys
                .iter()
                .map(|key| format!("\"{key}\""))
                .collect::<Vec<_>>()
                .join(", ")
        );
        for key in set.keys {
            let name = format!("{}{key}", set.prefix);
            let constant = constants.get(&name).ok_or_else(|| {
                format!("{name} is missing from {}", set.header)
            })?;
            let annotated =
                annotation(set.style, &constant.lines).map_err(|error| {
                    format!("{name} in {}: {error}", set.header)
                })?;
            let marker = marker_name(key);
            let (r#trait, item, rust) =
                key_impl(set.family, annotated, file, &name)?;
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
    /// Types the manifest does not describe: callback typedefs and the
    /// opaque structs behind handles.
    unchecked: Vec<String>,
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
                if names.iter().any(|name| name == "_unused") {
                    items.unchecked.push(item.ident.to_string());
                } else {
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
                syn::Type::Ptr(pointer)
                    if matches!(&*pointer.elem, syn::Type::Path(path)
                    if path.path.segments.last().is_some_and(|segment| {
                        segment.ident.to_string().ends_with("Impl")
                    })) =>
                {
                    items.handles.push(item.ident.to_string());
                }
                syn::Type::Path(_) => {
                    typedefs
                        .push((item.ident.to_string(), type_text(&item.ty)));
                }
                _ => {
                    return Err(format!(
                        "typedef {} has a shape the ABI test cannot check",
                        item.ident
                    ));
                }
            },
            _ => {}
        }
    }
    for (name, target) in typedefs {
        if let Some(values) = constants.remove(&name) {
            items.enums.push((name, values));
        } else if target.ends_with("Option") {
            // Function-pointer typedefs appear as `Option<...>`; the
            // manifest does not describe them.
            items.unchecked.push(name);
        } else {
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
    let _ = write!(
        out,
        "\n/// Types the manifest does not describe: callback typedefs and the\n\
         /// opaque structs behind handles.\n\
         pub(crate) const UNCHECKED: &[&str] = &[{}];\n",
        items
            .unchecked
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", "),
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

    fn annotate(style: Style, comment: &str) -> Result<Annotated, Error> {
        annotation(style, &comment_lines(comment))
    }

    #[test]
    fn labeled_annotations_come_only_from_the_last_line() {
        let cases = [
            (
                "Terminal width in cells.\n\nOutput type: uint16_t *",
                Annotated::Pointee("uint16_t".into()),
            ),
            (
                "Kitty flags.\n\nOutput type: GhosttyKittyKeyFlags * (uint8_t *)",
                Annotated::Pointee("GhosttyKittyKeyFlags".into()),
            ),
            (
                "Get a mode.\n\nInput/output type: GhosttyTerminalModeConfig *",
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
                "Opaque userdata.\n\nInput type: void*\n",
                Annotated::Direct("void*".into()),
            ),
        ];
        for (comment, expected) in cases {
            assert_eq!(
                annotate(Style::Labeled, comment),
                Ok(expected),
                "{comment}"
            );
        }
        for comment in [
            // The label is not the last line.
            "Output type: uint16_t *\n\nThe value is in cells (uint32_t).",
            // Prose mentions a second label.
            "Replaces Output type: bool *.\n\nOutput type: uint16_t *",
            // The label is not at the start of its line.
            "Width. Output type: uint16_t *",
            "Viewport width in cells (uint16_t).",
            "Output type: int *",
            "Output type: uint16_t * (see below)",
        ] {
            assert!(annotate(Style::Labeled, comment).is_err(), "{comment}");
        }
    }

    #[test]
    fn sentence_annotations_must_end_the_first_sentence() {
        let cases = [
            (
                "Viewport width in cells (uint16_t).",
                Annotated::Pointee("uint16_t".into()),
            ),
            (
                "Populate a pre-allocated GhosttyRenderStateRowIterator with row data\n from the render state (GhosttyRenderStateRowIterator). Row data is\n only valid (sometimes).",
                Annotated::Pointee("GhosttyRenderStateRowIterator".into()),
            ),
            (
                "Mouse tracking mode (value: GhosttyMouseTrackingMode).",
                Annotated::Pointee("GhosttyMouseTrackingMode".into()),
            ),
            (
                "\nEncode the cell as UTF-8 into a\ncaller-provided buffer (GhosttyBuffer).\n\nIf ptr is NULL (GhosttyString), fail.\n",
                Annotated::Pointee("GhosttyBuffer".into()),
            ),
        ];
        for (comment, expected) in cases {
            assert_eq!(
                annotate(Style::Sentence, comment),
                Ok(expected),
                "{comment}"
            );
        }
        for comment in [
            // A pointer the library writes through, not a value type.
            "Write codepoints into a buffer (uint32_t*).",
            // The first parenthesized type is not at the sentence's end.
            "Populate (GhosttyRenderStateRowIterator) rows. Returns (bool).",
            "Whether the row is dirty. Output (bool).",
            "The raw row value (GhosttyRow)",
            "Output type: bool *",
        ] {
            assert!(annotate(Style::Sentence, comment).is_err(), "{comment}");
        }
    }

    #[test]
    fn header_constants_attach_the_preceding_comment() {
        let header = "typedef enum GHOSTTY_ENUM_TYPED {\n  /** Invalid. */\n  GHOSTTY_X_INVALID = 0,\n\n  /**\n   * Width.\n   *\n   * Output type: uint16_t *\n   */\n  GHOSTTY_X_COLS = 1,\n  GHOSTTY_X_MAX_VALUE = GHOSTTY_ENUM_MAX_VALUE,\n} GhosttyX;";
        let constants = header_constants(header).unwrap();
        assert!(constants["GHOSTTY_X_COLS"].lines.contains(&"Width.".into()));
        assert_eq!(
            annotation(Style::Labeled, &constants["GHOSTTY_X_COLS"].lines),
            Ok(Annotated::Pointee("uint16_t".into()))
        );
        assert!(constants["GHOSTTY_X_MAX_VALUE"].lines.is_empty());
    }

    #[test]
    fn outputs_the_library_writes_through_need_a_populate_trait() {
        let file = syn::parse_file(
            "pub struct GhosttyBuffer { pub size: usize, pub ptr: *mut u8 }
             pub struct GhosttyString { pub ptr: *const u8, pub len: usize }
             pub struct GhosttyNested { pub buffers: [GhosttyBuffer; 2] }
             pub type GhosttyHandle = *mut GhosttyHandleImpl;
             pub struct GhosttyColorRgb { pub r: u8 }",
        )
        .unwrap();
        let writes = Pointers {
            any: true,
            mutable: true,
        };
        assert_eq!(pointers(&file, "GhosttyBuffer"), writes);
        assert_eq!(pointers(&file, "GhosttyNested"), writes);
        assert_eq!(pointers(&file, "GhosttyHandle"), writes);
        assert_eq!(
            pointers(&file, "GhosttyString"),
            Pointers {
                any: true,
                mutable: false
            }
        );
        assert_eq!(
            pointers(&file, "GhosttyColorRgb[256]"),
            Pointers::default()
        );
        assert_eq!(pointers(&file, "bool"), Pointers::default());

        let get = |populate| Family::Get {
            r#trait: "Data",
            populate,
        };
        let pointee = |name: &str| Annotated::Pointee(name.into());
        assert!(
            key_impl(get(None), pointee("GhosttyBuffer"), &file, "K").is_err()
        );
        assert_eq!(
            key_impl(
                get(Some("Populate")),
                pointee("GhosttyBuffer"),
                &file,
                "K"
            )
            .unwrap()
            .0,
            "Populate"
        );
        assert_eq!(
            key_impl(
                get(Some("Populate")),
                pointee("GhosttyString"),
                &file,
                "K"
            )
            .unwrap()
            .0,
            "Data"
        );
        let set = Family::Set {
            value: "Option",
            callback: None,
            pointer: None,
        };
        assert!(key_impl(set, pointee("GhosttyString"), &file, "K").is_err());
        assert_eq!(
            key_impl(set, pointee("GhosttyColorRgb"), &file, "K")
                .unwrap()
                .0,
            "Option"
        );
    }

    #[test]
    fn libclang_failures_explain_the_fix() {
        let error = libclang_error(&String::from(
            "Unable to find libclang: \"couldn't find any valid shared libraries\"",
        ));
        for expected in [
            "couldn't find any valid shared libraries",
            "xcode-select",
            "libclang1-14",
            "LIBCLANG_PATH",
        ] {
            assert!(error.contains(expected), "{expected} in {error}");
        }
        assert!(libclang_error(&"static detail").contains("static detail"));
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
