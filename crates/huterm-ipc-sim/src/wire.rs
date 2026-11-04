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

/// Appends encoded values to a buffer.
#[derive(Debug, Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// Returns the encoded bytes.
    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }

    /// Writes an unsigned LEB128 varint.
    pub fn u(&mut self, mut value: u64) {
        loop {
            let byte = u8::try_from(value & 0x7f).unwrap_or(0);
            value >>= 7;
            if value == 0 {
                self.buf.push(byte);
                return;
            }
            self.buf.push(byte | 0x80);
        }
    }

    /// Writes a usize as a varint.
    pub fn len(&mut self, value: usize) {
        self.u(value as u64);
    }

    /// Writes a bool as one byte.
    pub fn bool(&mut self, value: bool) {
        self.buf.push(u8::from(value));
    }

    /// Writes a length-prefixed byte string.
    pub fn bytes(&mut self, value: &[u8]) {
        self.len(value.len());
        self.buf.extend_from_slice(value);
    }

    /// Writes a length-prefixed UTF-8 string.
    pub fn str(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }

    /// Writes a record: its byte length, then the fields `fill` writes.
    pub fn record(&mut self, fill: impl FnOnce(&mut Writer)) {
        let mut inner = Writer::default();
        fill(&mut inner);
        self.bytes(&inner.buf);
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
