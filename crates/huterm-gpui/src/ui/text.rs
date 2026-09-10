//! UTF-8 and UTF-16 range conversion shared by native text handlers.

use std::ops::Range;

#[cfg(any(target_os = "macos", test))]
pub(crate) fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn offset_from_utf16(text: &str, offset: usize, round_up: bool) -> usize {
    let mut bytes = 0;
    let mut units = 0;
    for ch in text.chars() {
        if units >= offset {
            break;
        }
        let next_units = units + ch.len_utf16();
        if offset < next_units && !round_up {
            break;
        }
        units = next_units;
        bytes += ch.len_utf8();
    }
    bytes.min(text.len())
}

fn offset_to_utf16(text: &str, offset: usize, round_up: bool) -> usize {
    let mut bytes = 0;
    let mut units = 0;
    for ch in text.chars() {
        if bytes >= offset {
            break;
        }
        let next_bytes = bytes + ch.len_utf8();
        if offset < next_bytes && !round_up {
            break;
        }
        bytes = next_bytes;
        units += ch.len_utf16();
    }
    units
}

pub(crate) fn range_from_utf16(
    text: &str,
    range: Range<usize>,
) -> Range<usize> {
    let start = offset_from_utf16(text, range.start, false);
    if range.end < range.start {
        return start..start;
    }
    start..offset_from_utf16(text, range.end, true)
}

pub(crate) fn range_to_utf16(text: &str, range: Range<usize>) -> Range<usize> {
    offset_to_utf16(text, range.start, false)
        ..offset_to_utf16(text, range.end, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_do_not_split_utf8_or_surrogate_pairs() {
        let text = "a😀é";
        assert_eq!(utf16_len(text), 4);
        assert_eq!(range_from_utf16(text, 1..3), 1..5);
        assert_eq!(range_to_utf16(text, 1..5), 1..3);
        assert_eq!(range_from_utf16(text, 2..2), 1..5);
        assert_eq!(range_to_utf16(text, 3..3), 1..3);
    }

    #[test]
    fn reversed_utf16_ranges_collapse_at_the_adjusted_start() {
        let text = "a😀é";
        let reversed = Range { start: 3, end: 1 };
        assert_eq!(range_from_utf16(text, reversed), 5..5);
    }
}
