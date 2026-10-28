//! UTF-8 and UTF-16 range conversion shared by native text handlers.
//!
//! Platform text input speaks UTF-16 offsets, while Rust strings index
//! bytes. These conversions never split a UTF-8 sequence or a surrogate
//! pair: a start rounds down and an end rounds up to a character boundary.

use std::ops::Range;

/// The length of `text` in UTF-16 code units.
#[must_use]
pub fn utf16_len(text: &str) -> usize {
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

/// The byte range of `text` that the UTF-16 `range` covers. A reversed
/// range collapses at its start.
#[must_use]
pub fn range_from_utf16(text: &str, range: Range<usize>) -> Range<usize> {
    let start = offset_from_utf16(text, range.start, false);
    if range.end < range.start {
        return start..start;
    }
    start..offset_from_utf16(text, range.end, true)
}

/// The UTF-16 range that the byte `range` of `text` covers.
#[must_use]
pub fn range_to_utf16(text: &str, range: Range<usize>) -> Range<usize> {
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
