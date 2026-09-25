use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;

/// Bytes stored without allocation. Together with the length and variant tag,
/// this keeps [`CellText`] no larger than `String`.
const INLINE_CAPACITY: usize = 22;

/// Every ASCII character in order. Single-byte cells borrow from it, because
/// clients read cell text several times per frame and decoding stored bytes
/// costs several times more than a slice.
const ASCII: &str = "\0\x01\x02\x03\x04\x05\x06\x07\x08\t\n\x0b\x0c\r\x0e\x0f\
\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f \
!\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\x7f";

/// Text of one terminal cell, including combining characters.
///
/// Single scalars and short grapheme clusters are stored inline. Only clusters
/// longer than the inline capacity allocate. Text that fits inline is always
/// stored inline with zeroed padding, so equal text has one representation and
/// equality compares stored bytes without decoding them.
#[derive(Clone, Eq, PartialEq)]
pub struct CellText(Repr);

#[derive(Clone, Eq, PartialEq)]
enum Repr {
    /// UTF-8 bytes; unused trailing bytes are zero.
    Inline {
        len: u8,
        bytes: [u8; INLINE_CAPACITY],
    },
    Heap(Box<str>),
}

impl CellText {
    /// A single space, used for cells without text.
    pub const BLANK: Self = Self::inline_ascii(b' ');

    const fn inline_ascii(byte: u8) -> Self {
        let mut bytes = [0; INLINE_CAPACITY];
        bytes[0] = byte;
        Self(Repr::Inline { len: 1, bytes })
    }

    /// Stores `text`, inline when it fits.
    #[must_use]
    pub fn new(text: &str) -> Self {
        let Ok(len) = u8::try_from(text.len()) else {
            return Self(Repr::Heap(text.into()));
        };
        if usize::from(len) > INLINE_CAPACITY {
            return Self(Repr::Heap(text.into()));
        }
        let mut bytes = [0; INLINE_CAPACITY];
        bytes[..text.len()].copy_from_slice(text.as_bytes());
        Self(Repr::Inline { len, bytes })
    }

    /// Returns the cell text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match &self.0 {
            // Construction only stores complete UTF-8 strings, so the
            // fallback is unreachable.
            Repr::Inline { len: 1, bytes } if bytes[0].is_ascii() => {
                let index = usize::from(bytes[0]);
                ASCII.get(index..=index).unwrap_or_default()
            }
            Repr::Inline { len, bytes } => {
                std::str::from_utf8(&bytes[..usize::from(*len)])
                    .unwrap_or_default()
            }
            Repr::Heap(text) => text,
        }
    }

    /// Reports whether the cell has no text.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        matches!(self.0, Repr::Inline { len: 0, .. })
    }

    /// Returns the text's only scalar value, if it has exactly one.
    ///
    /// Decodes stored bytes directly; clients that key glyphs by scalar avoid
    /// the cost of borrowing validated text.
    #[must_use]
    pub fn as_char(&self) -> Option<char> {
        let Repr::Inline { len, bytes } = &self.0 else {
            // Heap text exceeds any single scalar's four bytes.
            return None;
        };
        let continuation = |index: usize| u32::from(bytes[index] & 0x3f);
        let first = u32::from(bytes[0]);
        let scalar = match (len, bytes[0]) {
            (1, 0x00..=0x7f) => first,
            (2, 0xc0..=0xdf) => (first & 0x1f) << 6 | continuation(1),
            (3, 0xe0..=0xef) => {
                (first & 0x0f) << 12 | continuation(1) << 6 | continuation(2)
            }
            (4, 0xf0..=0xf7) => {
                (first & 0x07) << 18
                    | continuation(1) << 12
                    | continuation(2) << 6
                    | continuation(3)
            }
            _ => return None,
        };
        char::from_u32(scalar)
    }
}

impl Deref for CellText {
    type Target = str;

    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for CellText {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<char> for CellText {
    fn from(character: char) -> Self {
        Self::new(character.encode_utf8(&mut [0; 4]))
    }
}

impl From<&str> for CellText {
    fn from(text: &str) -> Self {
        Self::new(text)
    }
}

impl From<String> for CellText {
    fn from(text: String) -> Self {
        if text.len() > INLINE_CAPACITY {
            return Self(Repr::Heap(text.into_boxed_str()));
        }
        Self::new(&text)
    }
}

impl PartialEq<str> for CellText {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for CellText {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl Hash for CellText {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl fmt::Debug for CellText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), formatter)
    }
}

impl fmt::Display for CellText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;

    fn hash(value: &impl Hash) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    fn is_inline(text: &CellText) -> bool {
        matches!(text.0, Repr::Inline { .. })
    }

    #[test]
    fn ascii_table_maps_every_byte_to_itself() {
        assert_eq!(ASCII.len(), 128);
        for byte in 0..=127_u8 {
            let text = char::from(byte).to_string();
            assert_eq!(CellText::from(text.as_str()).as_str(), text);
        }
    }

    #[test]
    fn as_char_reports_only_single_scalars() {
        for character in ['a', ' ', 'é', '界', '▀', '\u{e0b0}', '🙂'] {
            assert_eq!(CellText::from(character).as_char(), Some(character));
        }
        for text in ["", "ab", "e\u{301}", "▐\u{fe0f}", "👩\u{200d}💻"] {
            assert_eq!(CellText::from(text).as_char(), None, "{text:?}");
        }
        assert!(CellText::from("").is_empty());
        assert!(!CellText::BLANK.is_empty());
    }

    #[test]
    fn cell_text_is_no_larger_than_string() {
        assert!(size_of::<CellText>() <= size_of::<String>());
    }

    #[test]
    fn scalars_and_short_clusters_are_inline() {
        for text in ["", " ", "a", "界", "🙂", "e\u{301}", "👩\u{200d}💻"]
        {
            let cell = CellText::from(text);
            assert!(is_inline(&cell), "{text:?}");
            assert_eq!(cell, text);
        }
        assert!(is_inline(&CellText::from('🙂')));
        assert_eq!(CellText::BLANK, " ");
    }

    #[test]
    fn text_at_the_inline_boundary_switches_storage() {
        let fits = "a".repeat(INLINE_CAPACITY);
        let spills = "a".repeat(INLINE_CAPACITY + 1);
        assert!(is_inline(&CellText::from(fits.as_str())));
        assert!(is_inline(&CellText::from(fits.clone())));
        assert!(!is_inline(&CellText::from(spills.as_str())));
        assert!(!is_inline(&CellText::from(spills.clone())));
        assert_eq!(CellText::from(spills.clone()).as_str(), spills);
        // A family emoji ZWJ sequence exceeds the inline capacity.
        let family = "👨\u{200d}👩\u{200d}👧\u{200d}👦";
        assert!(family.len() > INLINE_CAPACITY);
        assert_eq!(CellText::from(family), family);
    }

    #[test]
    fn equality_and_hashing_ignore_construction() {
        for text in ["x", "e\u{301}", "👨\u{200d}👩\u{200d}👧\u{200d}👦"]
        {
            let borrowed = CellText::from(text);
            let owned = CellText::from(text.to_owned());
            assert_eq!(borrowed, owned);
            assert_eq!(hash(&borrowed), hash(&owned));
            assert_eq!(hash(&borrowed), hash(&text));
        }
        assert_eq!(CellText::from('x'), CellText::from("x"));
        assert_ne!(CellText::from("x"), CellText::from("y"));
    }
}
