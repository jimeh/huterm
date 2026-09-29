//! Declared byte layouts of FFI types, taken from the linked library's own
//! `ghostty_type_json()` manifest, so tests read native-written memory only
//! where a field lives. Padding, and union storage outside the member a tag
//! selects, may be left uninitialized by a foreign write.

use std::ops::Range;
use std::sync::OnceLock;

use serde_json::Value;

use crate::native;

/// Bytes that belong to a declared field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Part {
    /// Always part of the value.
    Bytes(Range<usize>),
    /// A tagged union: the integer tag, then each tag value's active member.
    Tagged {
        tag: Range<usize>,
        arms: Vec<(i64, Vec<Self>)>,
    },
}

fn types() -> &'static Value {
    static TYPES: OnceLock<Value> = OnceLock::new();
    TYPES.get_or_init(|| {
        let manifest: Value =
            serde_json::from_str(native::type_json()).unwrap();
        manifest["types"].clone()
    })
}

fn number(value: &Value) -> usize {
    usize::try_from(value.as_u64().unwrap()).unwrap()
}

/// The declared parts of `T`, found through its Rust type name.
///
/// # Panics
///
/// If `T` is a binding type the manifest does not describe, or contains a
/// union without a tag, whose active member cannot be known.
pub(crate) fn of<T>() -> Vec<Part> {
    rust_parts(std::any::type_name::<T>(), size_of::<T>())
}

fn rust_parts(name: &str, size: usize) -> Vec<Part> {
    if let Some(inner) = name
        .strip_prefix('[')
        .and_then(|name| name.strip_suffix(']'))
    {
        let (element, count) = inner.rsplit_once("; ").unwrap();
        let count: usize = count.parse().unwrap();
        let stride = size / count;
        let element = rust_parts(element, stride);
        return (0..count)
            .flat_map(|index| shift(&element, index * stride))
            .collect();
    }
    // Primitives and raw pointers have no padding; binding types are paths.
    if name.starts_with('*') || !name.contains("::") {
        return vec![Part::Bytes(0..size)];
    }
    let short = name.rsplit("::").next().unwrap();
    assert!(
        types().get(short).is_some(),
        "{name} is absent from the ABI manifest"
    );
    manifest_parts(short, size)
}

fn shift(parts: &[Part], by: usize) -> Vec<Part> {
    parts
        .iter()
        .map(|part| match part {
            Part::Bytes(range) => Part::Bytes(range.start + by..range.end + by),
            Part::Tagged { tag, arms } => Part::Tagged {
                tag: tag.start + by..tag.end + by,
                arms: arms
                    .iter()
                    .map(|(value, parts)| (*value, shift(parts, by)))
                    .collect(),
            },
        })
        .collect()
}

/// The parts of manifest type `name`, or of a primitive `size` bytes wide.
fn manifest_parts(name: &str, size: usize) -> Vec<Part> {
    let Some(descriptor) = types().get(name) else {
        return vec![Part::Bytes(0..size)];
    };
    match descriptor["kind"].as_str().unwrap() {
        "struct" => struct_parts(name, descriptor),
        "union" => panic!("{name} is an untagged union"),
        _ => vec![Part::Bytes(0..number(&descriptor["size"]))],
    }
}

fn struct_parts(name: &str, descriptor: &Value) -> Vec<Part> {
    let fields = descriptor["fields"].as_object().unwrap();
    let mut parts = Vec::new();
    for field in fields.values() {
        let offset = number(&field["offset"]);
        let size = number(&field["size"]);
        let kind = field["type"].as_str().unwrap();
        if let Some(tag) = field.get("tag").and_then(Value::as_str) {
            parts.push(tagged(name, fields, field, tag, offset));
        } else if kind == "array" {
            let count = number(&field["count"]);
            let element =
                manifest_parts(field["elem"].as_str().unwrap(), size / count);
            parts.extend((0..count).flat_map(|index| {
                shift(&element, offset + index * (size / count))
            }));
        } else if kind == "pointer" {
            parts.push(Part::Bytes(offset..offset + size));
        } else {
            parts.extend(shift(&manifest_parts(kind, size), offset));
        }
    }
    parts
}

fn tagged(
    name: &str,
    fields: &serde_json::Map<String, Value>,
    field: &Value,
    tag: &str,
    offset: usize,
) -> Part {
    let tag_field = &fields[tag];
    let tag_offset = number(&tag_field["offset"]);
    let values = &types()[tag_field["type"].as_str().unwrap()]["values"];
    let union = &types()[field["type"].as_str().unwrap()];
    let arms = field["arms"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(arm, member)| {
            let value = values[arm]
                .as_i64()
                .unwrap_or_else(|| panic!("{name}: unknown tag {arm}"));
            let parts = member.as_str().map_or_else(Vec::new, |member| {
                let member = &union["fields"][member];
                shift(
                    &manifest_parts(
                        member["type"].as_str().unwrap(),
                        number(&member["size"]),
                    ),
                    offset + number(&member["offset"]),
                )
            });
            (value, parts)
        })
        .collect();
    Part::Tagged {
        tag: tag_offset..tag_offset + number(&tag_field["size"]),
        arms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffi;

    /// Every byte offset a part may cover, whatever its tag.
    fn covered(parts: &[Part], out: &mut Vec<usize>) {
        for part in parts {
            match part {
                Part::Bytes(range) => out.extend(range.clone()),
                Part::Tagged { tag, arms } => {
                    out.extend(tag.clone());
                    for (_, parts) in arms {
                        covered(parts, out);
                    }
                }
            }
        }
    }

    #[test]
    fn layouts_leave_out_padding_and_inactive_union_storage() {
        let mut cursor = Vec::new();
        covered(&of::<ffi::GhosttyRenderStateCursor>(), &mut cursor);
        for padding in [9, 18, 19] {
            assert!(!cursor.contains(&padding), "{padding}");
        }
        assert!(cursor.contains(&8) && cursor.contains(&23));

        // `fg_color` is a tag at 8..12, padding at 12..16, and an 8-byte
        // union whose widest member, RGB, spans 16..19.
        let style = of::<ffi::GhosttyStyle>();
        let fg = style
            .iter()
            .find_map(|part| match part {
                Part::Tagged { tag, arms } if tag.start == 8 => Some(arms),
                _ => None,
            })
            .unwrap();
        let mut union = Vec::new();
        for (_, parts) in fg {
            covered(parts, &mut union);
        }
        union.sort_unstable();
        union.dedup();
        assert_eq!(union, [16, 17, 18]);
        let mut all = Vec::new();
        covered(&style, &mut all);
        assert!(!(12..16).any(|offset| all.contains(&offset)));
        assert!(!(19..24).any(|offset| all.contains(&offset)));

        assert_eq!(of::<u16>(), [Part::Bytes(0..2)]);
        let mut pair = Vec::new();
        covered(&of::<[ffi::GhosttyColorRgb; 2]>(), &mut pair);
        pair.sort_unstable();
        assert_eq!(pair, [0, 1, 2, 3, 4, 5]);
    }
}
