//! Compares every FFI declaration with the linked library's own ABI
//! manifest from `ghostty_type_json()`.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::ffi::layout;
use crate::native;

/// The manifest schema this binding understands (types.h).
const SCHEMA: u64 = 1;

fn manifest(json: &str) -> Result<Value, String> {
    let value: Value =
        serde_json::from_str(json).map_err(|error| error.to_string())?;
    match value.get("schema").and_then(Value::as_u64) {
        Some(SCHEMA) => Ok(value),
        other => Err(format!("unsupported ABI manifest schema {other:?}")),
    }
}

fn number(value: &Value, key: &str, context: &str) -> u64 {
    value
        .get(key)
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("{context}: missing {key}"))
}

fn widen(value: usize) -> u64 {
    u64::try_from(value).unwrap()
}

/// Asserts a type's kind, size, and alignment, returning its descriptor.
fn descriptor<'a>(
    types: &'a Value,
    name: &str,
    kinds: &[&str],
    size: usize,
    align: usize,
) -> &'a Value {
    let descriptor = types
        .get(name)
        .unwrap_or_else(|| panic!("{name} is absent from the ABI manifest"));
    let kind = descriptor
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        kinds.contains(&kind),
        "{name}: kind {kind}, expected {kinds:?}"
    );
    assert_eq!(
        (
            number(descriptor, "size", name),
            number(descriptor, "align", name)
        ),
        (widen(size), widen(align)),
        "{name} size and alignment"
    );
    descriptor
}

fn check_structs(types: &Value) -> usize {
    for item in layout::STRUCTS {
        let descriptor =
            descriptor(types, item.name, &["struct"], item.size, item.align);
        let fields = descriptor["fields"]
            .as_object()
            .unwrap_or_else(|| panic!("{}: no fields", item.name));
        let native: BTreeSet<&str> =
            fields.keys().map(String::as_str).collect();
        let rust: BTreeSet<&str> =
            item.fields.iter().map(|(name, _)| *name).collect();
        assert_eq!(native, rust, "{} fields", item.name);
        for (field, offset) in item.fields {
            assert_eq!(
                number(&fields[*field], "offset", item.name),
                widen(*offset),
                "{}.{field}",
                item.name
            );
        }
    }
    layout::STRUCTS.len()
}

fn check_enums(types: &Value) -> usize {
    for item in layout::ENUMS {
        let descriptor =
            descriptor(types, item.name, &["enum"], item.size, item.align);
        let prefix = descriptor["prefix"].as_str().unwrap_or_default();
        let values = descriptor["values"]
            .as_object()
            .unwrap_or_else(|| panic!("{}: no values", item.name));
        let native: BTreeSet<String> =
            values.keys().map(|key| format!("{prefix}{key}")).collect();
        let rust: BTreeSet<String> = item
            .values
            .iter()
            .map(|(name, _)| (*name).to_owned())
            .collect();
        assert_eq!(native, rust, "{} constants", item.name);
        for (name, value) in item.values {
            let key = name.strip_prefix(prefix).unwrap();
            assert_eq!(values[key].as_i64(), Some(*value), "{name}");
        }
    }
    layout::ENUMS.len()
}

fn check_sized(
    types: &Value,
    items: &[layout::Sized],
    kinds: &[&str],
) -> usize {
    for item in items {
        descriptor(types, item.name, kinds, item.size, item.align);
    }
    items.len()
}

/// Every type name the layout tables check.
fn checked_names() -> BTreeSet<&'static str> {
    let structs = layout::STRUCTS.iter().map(|item| item.name);
    let enums = layout::ENUMS.iter().map(|item| item.name);
    let sized = [layout::UNIONS, layout::ALIASES, layout::HANDLES]
        .into_iter()
        .flatten()
        .map(|item| item.name);
    structs.chain(enums).chain(sized).collect()
}

/// Type names declared in Rust source: `pub struct`, `pub union`, and
/// `pub type` items at the start of a line, as rustfmt lays out bindgen
/// output.
fn declared_types(source: &str) -> BTreeSet<&str> {
    source
        .lines()
        .filter_map(|line| {
            ["pub struct ", "pub union ", "pub type "]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
        })
        .map(|rest| {
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            &rest[..end]
        })
        .collect()
}

/// Cross-checks the generated layout tables against two sources the
/// generator's classification does not produce: the committed
/// `bindings.rs` and the library's manifest. Every declared type must be
/// checked or deliberately skipped, and nothing the manifest describes may
/// be skipped.
fn covers_every_committed_type(types: &Value) {
    let checked = checked_names();
    let unchecked: BTreeSet<&str> = layout::UNCHECKED.iter().copied().collect();
    assert!(checked.is_disjoint(&unchecked));
    let declared = declared_types(include_str!("ffi/bindings.rs"));
    let covered: BTreeSet<&str> = checked.union(&unchecked).copied().collect();
    let missing: Vec<_> = declared.difference(&covered).collect();
    let stale: Vec<_> = covered.difference(&declared).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "bindings.rs declares {missing:?}, which the ABI test neither \
         checks nor skips; it lists {stale:?}, which bindings.rs lacks"
    );
    let described: Vec<&str> = declared
        .iter()
        .copied()
        .filter(|name| types.get(*name).is_some())
        .filter(|name| !checked.contains(name))
        .collect();
    assert!(
        described.is_empty(),
        "the manifest describes {described:?}, but the ABI test skips them"
    );
}

#[test]
fn declared_types_reads_rustfmt_bindgen_items() {
    let source = "pub type GhosttyCell = u64;\n#[repr(C)]\npub struct GhosttyA {\n    pub size: usize,\n}\npub union GhosttyB {\n}\npub const GHOSTTY_X: GhosttyCell = 1;\n    pub fn ghostty_f();\n";
    assert_eq!(
        declared_types(source),
        BTreeSet::from(["GhosttyA", "GhosttyB", "GhosttyCell"])
    );
}

#[test]
fn manifest_rejects_unknown_schemas() {
    assert!(manifest(r#"{"schema": 1, "types": {}}"#).is_ok());
    for json in [r#"{"schema": 2}"#, r#"{"schema": "1"}"#, "{}", "not json"] {
        assert!(manifest(json).is_err(), "{json}");
    }
}

#[test]
fn linked_library_abi_matches_every_ffi_declaration() {
    let manifest = manifest(native::type_json()).unwrap();
    let abi = &manifest["abi"];
    assert_eq!(
        number(abi, "pointer_size", "abi"),
        widen(size_of::<*const u8>())
    );
    assert_eq!(number(abi, "usize_size", "abi"), widen(size_of::<usize>()));
    let endian = if cfg!(target_endian = "little") {
        "little"
    } else {
        "big"
    };
    assert_eq!(abi["endian"], endian);
    let types = &manifest["types"];
    let checked = check_structs(types)
        + check_enums(types)
        + check_sized(types, layout::UNIONS, &["union"])
        // `GhosttyCell` is a packed integer; other typedefs are aliases.
        + check_sized(types, layout::ALIASES, &["alias", "packed"])
        + check_sized(types, layout::HANDLES, &["opaque"]);
    assert_eq!(checked, checked_names().len());
    covers_every_committed_type(types);
}
