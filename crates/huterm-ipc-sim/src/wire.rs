//! Codec primitives: varints, capped strings and bytes, and length-prefixed
//! records whose trailing bytes an older decoder skips.

use std::fmt;

/// Why bytes failed to decode. Every variant is a protocol violation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeError {
    /// The input ended inside a value.
    Truncated,
    /// A varint used more than ten bytes or overflowed `u64`.
    Overlong,
    /// A value exceeded the field's declared maximum.
    TooLarge,
    /// A `bool` byte was neither 0 nor 1.
    BadBool,
    /// A string was not UTF-8.
    BadUtf8,
    /// An enum tag or message kind was not negotiated.
    UnknownTag(u64),
    /// A decoded value broke an invariant.
    Invalid(&'static str),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

/// What a writer keeps of the bytes it is given.
#[derive(Clone, Copy, Debug, Default)]
enum Keep {
    /// Every byte.
    #[default]
    All,
    /// None: the writer only counts, for a sizing pass.
    Nothing,
    /// Only bytes at offsets in `start..end`, to build one frame.
    Window(usize, usize),
}

#[cfg(test)]
thread_local! {
    static PEAK_STORED: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

/// The most bytes any writer on this thread stored since the last call,
/// and resets it, so tests can bound encoding memory.
#[cfg(test)]
pub(crate) fn take_peak_stored() -> usize {
    PEAK_STORED.replace(0)
}

/// Appends encoded values to a buffer. A counting writer stores nothing and
/// a windowed writer stores one byte range, so a message can be sized and
/// framed without ever holding its whole encoding.
#[derive(Debug, Default)]
pub struct Writer {
    buf: Vec<u8>,
    keep: Keep,
    written: usize,
}

impl Writer {
    /// A writer that only counts bytes, for a sizing pass.
    #[must_use]
    pub fn counting() -> Self {
        Self {
            keep: Keep::Nothing,
            ..Self::default()
        }
    }

    /// A writer that keeps only the bytes at offsets `start..end`.
    #[must_use]
    pub fn window(start: usize, end: usize) -> Self {
        Self {
            keep: Keep::Window(start, end),
            ..Self::default()
        }
    }

    /// Bytes written so far, kept or not.
    #[must_use]
    pub fn written(&self) -> usize {
        self.written
    }

    /// Returns the kept bytes.
    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }

    fn push(&mut self, byte: u8) {
        let keep = match self.keep {
            Keep::All => true,
            Keep::Nothing => false,
            Keep::Window(start, end) => (start..end).contains(&self.written),
        };
        if keep {
            self.buf.push(byte);
            #[cfg(test)]
            PEAK_STORED.with(|peak| peak.set(peak.get().max(self.buf.len())));
        }
        self.written += 1;
    }

    /// Writes an unsigned LEB128 varint.
    pub fn u(&mut self, mut value: u64) {
        loop {
            let byte = u8::try_from(value & 0x7f).unwrap_or(0);
            value >>= 7;
            if value == 0 {
                self.push(byte);
                return;
            }
            self.push(byte | 0x80);
        }
    }

    /// Writes a usize as a varint.
    pub fn len(&mut self, value: usize) {
        self.u(value as u64);
    }

    /// Writes a bool as one byte.
    pub fn bool(&mut self, value: bool) {
        self.push(u8::from(value));
    }

    /// Writes a length-prefixed byte string.
    pub fn bytes(&mut self, value: &[u8]) {
        self.len(value.len());
        for byte in value {
            self.push(*byte);
        }
    }

    /// Writes a length-prefixed UTF-8 string.
    pub fn str(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }

    /// Writes a record: its byte length, from a counting pass over `fill`,
    /// then the fields `fill` writes.
    pub fn record(&mut self, fill: impl Fn(&mut Writer)) {
        let mut counter = Writer::counting();
        fill(&mut counter);
        self.len(counter.written);
        fill(self);
    }
}

/// The number of bytes `value` takes as a varint.
#[must_use]
pub fn varint_len(value: u64) -> usize {
    let bits = 64 - value.leading_zeros() as usize;
    bits.div_ceil(7).max(1)
}

/// Reads encoded values from a slice.
#[derive(Debug)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Reads from `buf`.
    #[must_use]
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Bytes read so far.
    #[must_use]
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Whether every byte was read.
    #[must_use]
    pub fn at_end(&self) -> bool {
        self.pos == self.buf.len()
    }

    /// Fails unless every byte was read.
    ///
    /// # Errors
    /// Returns [`DecodeError::Invalid`] when bytes remain.
    pub fn finish(&self) -> Result<(), DecodeError> {
        if self.at_end() {
            Ok(())
        } else {
            Err(DecodeError::Invalid("trailing bytes"))
        }
    }

    /// Reads a varint.
    ///
    /// # Errors
    /// Fails on truncation or an overlong encoding.
    pub fn u(&mut self) -> Result<u64, DecodeError> {
        let mut value = 0_u64;
        for index in 0..10 {
            let byte = *self.buf.get(self.pos).ok_or(DecodeError::Truncated)?;
            self.pos += 1;
            let part = u64::from(byte & 0x7f);
            if index == 9 && part > 1 {
                return Err(DecodeError::Overlong);
            }
            value |= part << (7 * index);
            if byte & 0x80 == 0 {
                if index > 0 && byte == 0 {
                    return Err(DecodeError::Overlong);
                }
                return Ok(value);
            }
        }
        Err(DecodeError::Overlong)
    }

    /// Reads a varint no larger than `max`.
    ///
    /// # Errors
    /// Fails like [`Self::u`], or with [`DecodeError::TooLarge`].
    pub fn u_max(&mut self, max: u64) -> Result<u64, DecodeError> {
        let value = self.u()?;
        if value > max {
            Err(DecodeError::TooLarge)
        } else {
            Ok(value)
        }
    }

    /// Reads a length or count no larger than `max`.
    ///
    /// # Errors
    /// Fails like [`Self::u_max`].
    pub fn len(&mut self, max: usize) -> Result<usize, DecodeError> {
        usize::try_from(self.u_max(max as u64)?)
            .map_err(|_| DecodeError::TooLarge)
    }

    /// Reads a bool.
    ///
    /// # Errors
    /// Fails on truncation or a byte other than 0 or 1.
    pub fn bool(&mut self) -> Result<bool, DecodeError> {
        let byte = *self.buf.get(self.pos).ok_or(DecodeError::Truncated)?;
        self.pos += 1;
        match byte {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError::BadBool),
        }
    }

    /// Reads a byte string no longer than `max`, checked before copying.
    ///
    /// # Errors
    /// Fails on truncation or an oversized length.
    pub fn bytes(&mut self, max: usize) -> Result<&'a [u8], DecodeError> {
        let len = self.len(max)?;
        let end = self.pos.checked_add(len).ok_or(DecodeError::TooLarge)?;
        let slice =
            self.buf.get(self.pos..end).ok_or(DecodeError::Truncated)?;
        self.pos = end;
        Ok(slice)
    }

    /// Reads a UTF-8 string no longer than `max` bytes.
    ///
    /// # Errors
    /// Fails like [`Self::bytes`], or on invalid UTF-8.
    pub fn str(&mut self, max: usize) -> Result<String, DecodeError> {
        let bytes = self.bytes(max)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| DecodeError::BadUtf8)
    }

    /// Reads a record no longer than `max` bytes. `read` decodes the fields
    /// it knows; any bytes it leaves belong to fields a newer peer appended
    /// and are skipped.
    ///
    /// # Errors
    /// Fails like [`Self::bytes`], or with whatever `read` returns.
    pub fn record<T>(
        &mut self,
        max: usize,
        read: impl FnOnce(&mut Reader<'a>) -> Result<T, DecodeError>,
    ) -> Result<T, DecodeError> {
        let body = self.bytes(max)?;
        let mut inner = Reader::new(body);
        read(&mut inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_round_trip_and_reject_overlong_forms() {
        for value in [0, 1, 127, 128, 300, u64::from(u32::MAX), u64::MAX] {
            let mut writer = Writer::default();
            writer.u(value);
            let bytes = writer.finish();
            assert_eq!(bytes.len(), varint_len(value));
            assert_eq!(Reader::new(&bytes).u(), Ok(value));
        }
        assert_eq!(Reader::new(&[0x80, 0x00]).u(), Err(DecodeError::Overlong));
        assert_eq!(Reader::new(&[0xff; 11]).u(), Err(DecodeError::Overlong));
        assert_eq!(Reader::new(&[0x80]).u(), Err(DecodeError::Truncated));
    }

    #[test]
    fn records_skip_appended_fields_and_strings_check_caps() {
        let mut writer = Writer::default();
        writer.record(|record| {
            record.u(7);
            record.str("appended by a newer peer");
        });
        let bytes = writer.finish();
        let value = Reader::new(&bytes).record(64, Reader::u);
        assert_eq!(value, Ok(7));
        assert_eq!(
            Reader::new(&bytes).record(4, Reader::u),
            Err(DecodeError::TooLarge)
        );
        let mut writer = Writer::default();
        writer.str("hello");
        let bytes = writer.finish();
        assert_eq!(Reader::new(&bytes).str(4), Err(DecodeError::TooLarge));
        assert_eq!(
            Reader::new(&[2, 0xff, 0xfe]).str(4),
            Err(DecodeError::BadUtf8)
        );
        assert_eq!(Reader::new(&[2]).bool(), Err(DecodeError::BadBool));
    }
}
