//! Primitive encodings: varints, strings, sequences, options, records, and
//! enum tags.
//!
//! A [`Writer`] appends canonical bytes; a [`Reader`] checks every length
//! and count against a caller-supplied maximum before it allocates, so a
//! peer's length prefix never decides how much memory a decoder reserves.
//!
//! A record is a varint byte length followed by fields in declared order. A
//! decoder reads the fields it knows and skips the rest of the record, which
//! belongs to fields a newer peer appended; [`Reader::appended`] reads a
//! field added in a later minor version, taking its default when an older
//! peer's record ends before it.

use std::fmt;

use huterm_protocol::RuntimeId;

/// The longest LEB128 encoding of a `u64`.
pub const MAX_VARINT_BYTES: usize = 10;

/// Why bytes could not be decoded. Every decode failure is a protocol
/// violation that ends the connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodeError {
    /// The input ended inside a value.
    Truncated,
    /// A varint is longer than its value needs, or longer than a `u64`.
    OverlongVarint,
    /// An integer does not fit its field's width or range.
    OutOfRange,
    /// A boolean byte other than 0 or 1.
    InvalidBool(u8),
    /// A string is not UTF-8.
    InvalidUtf8,
    /// A length or count is larger than its field allows.
    TooLong {
        /// The field.
        field: &'static str,
    },
    /// An enum tag or option tag that the negotiated version does not
    /// define.
    UnknownTag {
        /// The field.
        field: &'static str,
        /// The tag read.
        tag: u64,
    },
    /// A message kind the receiver does not accept in this direction or
    /// version.
    UnknownKind(u64),
    /// A value that decoded but breaks an invariant of its type.
    Invalid(&'static str),
    /// Bytes remain after a complete message.
    TrailingBytes,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("input ended inside a value"),
            Self::OverlongVarint => f.write_str("overlong varint"),
            Self::OutOfRange => f.write_str("integer out of range"),
            Self::InvalidBool(byte) => write!(f, "invalid boolean byte {byte}"),
            Self::InvalidUtf8 => f.write_str("string is not UTF-8"),
            Self::TooLong { field } => write!(f, "{field} is too long"),
            Self::UnknownTag { field, tag } => {
                write!(f, "unknown {field} tag {tag}")
            }
            Self::UnknownKind(kind) => {
                write!(f, "unexpected message kind {kind}")
            }
            Self::Invalid(what) => write!(f, "invalid {what}"),
            Self::TrailingBytes => f.write_str("bytes after the message"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Why a value could not be encoded. The sender refuses locally and sends
/// nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EncodeError {
    /// A payload or field is larger than its limit.
    LimitExceeded {
        /// The field.
        field: &'static str,
    },
    /// An identity belongs to another runtime scope than the connection's.
    StaleTarget,
    /// The value has no encoding in this version.
    Unsupported,
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LimitExceeded { field } => {
                write!(f, "{field} is larger than its limit")
            }
            Self::StaleTarget => {
                f.write_str("identity belongs to another connection's scope")
            }
            Self::Unsupported => f.write_str("value has no wire encoding"),
        }
    }
}

impl std::error::Error for EncodeError {}

/// The number of bytes the LEB128 encoding of `value` takes.
#[must_use]
pub const fn varint_len(value: u64) -> usize {
    let bits = 64 - value.leading_zeros() as usize;
    if bits == 0 { 1 } else { bits.div_ceil(7) }
}

/// Appends canonical encodings to a byte buffer.
#[derive(Debug, Default)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    /// Creates an empty writer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the bytes written so far.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// The number of bytes written so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether nothing has been written.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Writes an unsigned LEB128 varint.
    pub fn u64(&mut self, mut value: u64) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                self.bytes.push(byte);
                return;
            }
            self.bytes.push(byte | 0x80);
        }
    }

    /// Writes a `u32` as a varint.
    pub fn u32(&mut self, value: u32) {
        self.u64(u64::from(value));
    }

    /// Writes a `u16` as a varint.
    pub fn u16(&mut self, value: u16) {
        self.u64(u64::from(value));
    }

    /// Writes a `usize` as a `u64` varint.
    pub fn usize(&mut self, value: usize) {
        // A usize always fits in u64 on supported targets.
        self.u64(u64::try_from(value).unwrap_or(u64::MAX));
    }

    /// Writes a signed integer as a zigzag varint.
    pub fn i64(&mut self, value: i64) {
        self.u64(value.cast_unsigned() << 1 ^ (value >> 63).cast_unsigned());
    }

    /// Writes a boolean as one byte.
    pub fn bool(&mut self, value: bool) {
        self.bytes.push(u8::from(value));
    }

    /// Writes raw bytes of a fixed-size value.
    pub fn fixed(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// Writes an enum tag.
    pub fn tag(&mut self, tag: u64) {
        self.u64(tag);
    }

    /// Writes a length-prefixed byte string of at most `max` bytes.
    ///
    /// # Errors
    /// Refuses a longer value with [`EncodeError::LimitExceeded`].
    pub fn bytes(
        &mut self,
        value: &[u8],
        max: usize,
        field: &'static str,
    ) -> Result<(), EncodeError> {
        if value.len() > max {
            return Err(EncodeError::LimitExceeded { field });
        }
        self.usize(value.len());
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    /// Writes a length-prefixed UTF-8 string of at most `max` bytes.
    ///
    /// # Errors
    /// Refuses a longer value with [`EncodeError::LimitExceeded`].
    pub fn str(
        &mut self,
        value: &str,
        max: usize,
        field: &'static str,
    ) -> Result<(), EncodeError> {
        self.bytes(value.as_bytes(), max, field)
    }

    /// Writes an optional string.
    ///
    /// # Errors
    /// Refuses a longer value with [`EncodeError::LimitExceeded`].
    pub fn optional_str(
        &mut self,
        value: Option<&str>,
        max: usize,
        field: &'static str,
    ) -> Result<(), EncodeError> {
        self.option(value, |w, value| w.str(value, max, field))
    }

    /// Writes an option tag, then the value when present.
    ///
    /// # Errors
    /// Returns the value encoder's error.
    pub fn option<T>(
        &mut self,
        value: Option<T>,
        encode: impl FnOnce(&mut Self, T) -> Result<(), EncodeError>,
    ) -> Result<(), EncodeError> {
        match value {
            None => {
                self.bytes.push(0);
                Ok(())
            }
            Some(value) => {
                self.bytes.push(1);
                encode(self, value)
            }
        }
    }

    /// Writes a count of at most `max`, then each item.
    ///
    /// # Errors
    /// Refuses a longer sequence with [`EncodeError::LimitExceeded`], or
    /// returns an item encoder's error.
    pub fn seq<I: ExactSizeIterator>(
        &mut self,
        items: I,
        max: usize,
        field: &'static str,
        mut encode: impl FnMut(&mut Self, I::Item) -> Result<(), EncodeError>,
    ) -> Result<(), EncodeError> {
        if items.len() > max {
            return Err(EncodeError::LimitExceeded { field });
        }
        self.usize(items.len());
        for item in items {
            encode(self, item)?;
        }
        Ok(())
    }

    /// Writes a record: its byte length, then the fields `encode` writes.
    ///
    /// # Errors
    /// Returns the field encoder's error.
    pub fn record(
        &mut self,
        encode: impl FnOnce(&mut Self) -> Result<(), EncodeError>,
    ) -> Result<(), EncodeError> {
        let start = self.bytes.len();
        encode(self)?;
        let length = self.bytes.len() - start;
        let mut prefix = Self::new();
        prefix.usize(length);
        self.bytes.splice(start..start, prefix.bytes);
        Ok(())
    }

    /// Writes an identity as its bare numeric value in the connection's
    /// scope.
    ///
    /// # Errors
    /// Refuses an identity of another runtime with
    /// [`EncodeError::StaleTarget`]; it is never sent.
    pub fn id<T: WireId>(
        &mut self,
        scope: IdScope,
        id: T,
    ) -> Result<(), EncodeError> {
        if id.runtime() != scope.runtime {
            return Err(EncodeError::StaleTarget);
        }
        self.u64(id.value());
        Ok(())
    }
}

/// Reads canonical encodings from a byte slice.
#[derive(Debug)]
pub struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    /// Reads from `bytes`.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// Whether every byte has been read.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The bytes not yet read, without consuming them.
    #[must_use]
    pub const fn peek(&self) -> &'a [u8] {
        self.bytes
    }

    /// The number of bytes not yet read.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.bytes.len()
    }

    /// Fails unless every byte has been read.
    ///
    /// # Errors
    /// Returns [`DecodeError::TrailingBytes`] when bytes remain.
    pub const fn finish(&self) -> Result<(), DecodeError> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(DecodeError::TrailingBytes)
        }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], DecodeError> {
        if count > self.bytes.len() {
            return Err(DecodeError::Truncated);
        }
        let (taken, rest) = self.bytes.split_at(count);
        self.bytes = rest;
        Ok(taken)
    }

    /// Reads an unsigned LEB128 varint, rejecting a longer encoding than its
    /// value needs.
    ///
    /// # Errors
    /// Returns [`DecodeError::Truncated`] or [`DecodeError::OverlongVarint`].
    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        let mut value = 0_u64;
        for index in 0..MAX_VARINT_BYTES {
            let Some((&byte, rest)) = self.bytes.split_first() else {
                return Err(DecodeError::Truncated);
            };
            self.bytes = rest;
            let low = u64::from(byte & 0x7f);
            if index == MAX_VARINT_BYTES - 1 && low > 1 {
                return Err(DecodeError::OverlongVarint);
            }
            value |= low << (7 * index);
            if byte & 0x80 == 0 {
                if index > 0 && byte == 0 {
                    return Err(DecodeError::OverlongVarint);
                }
                return Ok(value);
            }
        }
        Err(DecodeError::OverlongVarint)
    }

    /// Reads a varint that must fit `u32`.
    ///
    /// # Errors
    /// Returns a varint error or [`DecodeError::OutOfRange`].
    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        u32::try_from(self.u64()?).map_err(|_| DecodeError::OutOfRange)
    }

    /// Reads a varint that must fit `u16`.
    ///
    /// # Errors
    /// Returns a varint error or [`DecodeError::OutOfRange`].
    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        u16::try_from(self.u64()?).map_err(|_| DecodeError::OutOfRange)
    }

    /// Reads a `u64` varint that must fit this target's `usize`.
    ///
    /// # Errors
    /// Returns a varint error or [`DecodeError::OutOfRange`].
    pub fn usize(&mut self) -> Result<usize, DecodeError> {
        usize::try_from(self.u64()?).map_err(|_| DecodeError::OutOfRange)
    }

    /// Reads a zigzag varint.
    ///
    /// # Errors
    /// Returns a varint error.
    pub fn i64(&mut self) -> Result<i64, DecodeError> {
        let value = self.u64()?;
        Ok((value >> 1).cast_signed() ^ -(value & 1).cast_signed())
    }

    /// Reads a boolean byte.
    ///
    /// # Errors
    /// Returns [`DecodeError::Truncated`] or [`DecodeError::InvalidBool`].
    pub fn bool(&mut self) -> Result<bool, DecodeError> {
        match self.take(1)?[0] {
            0 => Ok(false),
            1 => Ok(true),
            byte => Err(DecodeError::InvalidBool(byte)),
        }
    }

    /// Reads a fixed-size raw value.
    ///
    /// # Errors
    /// Returns [`DecodeError::Truncated`].
    pub fn fixed<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut value = [0; N];
        value.copy_from_slice(self.take(N)?);
        Ok(value)
    }

    /// Reads an enum tag.
    ///
    /// # Errors
    /// Returns a varint error.
    pub fn tag(&mut self) -> Result<u64, DecodeError> {
        self.u64()
    }

    /// Reads a length-prefixed byte string of at most `max` bytes, checking
    /// the length before taking any bytes.
    ///
    /// # Errors
    /// Returns [`DecodeError::TooLong`] or [`DecodeError::Truncated`].
    pub fn bytes(
        &mut self,
        max: usize,
        field: &'static str,
    ) -> Result<&'a [u8], DecodeError> {
        let length = self.u64()?;
        if usize::try_from(length).map_or(true, |length| length > max) {
            return Err(DecodeError::TooLong { field });
        }
        self.take(usize::try_from(length).map_err(|_| DecodeError::OutOfRange)?)
    }

    /// Reads a length-prefixed UTF-8 string of at most `max` bytes.
    ///
    /// # Errors
    /// Returns a length error or [`DecodeError::InvalidUtf8`].
    pub fn str(
        &mut self,
        max: usize,
        field: &'static str,
    ) -> Result<&'a str, DecodeError> {
        std::str::from_utf8(self.bytes(max, field)?)
            .map_err(|_| DecodeError::InvalidUtf8)
    }

    /// Reads an owned string of at most `max` bytes.
    ///
    /// # Errors
    /// Returns a length error or [`DecodeError::InvalidUtf8`].
    pub fn string(
        &mut self,
        max: usize,
        field: &'static str,
    ) -> Result<String, DecodeError> {
        self.str(max, field).map(str::to_owned)
    }

    /// Reads an optional owned string of at most `max` bytes.
    ///
    /// # Errors
    /// Returns a tag, length, or UTF-8 error.
    pub fn optional_string(
        &mut self,
        max: usize,
        field: &'static str,
    ) -> Result<Option<String>, DecodeError> {
        self.option(field, |r| r.string(max, field))
    }

    /// Reads an option tag, then the value when present.
    ///
    /// # Errors
    /// Returns [`DecodeError::UnknownTag`] for a tag other than 0 or 1, or
    /// the value decoder's error.
    pub fn option<T>(
        &mut self,
        field: &'static str,
        decode: impl FnOnce(&mut Self) -> Result<T, DecodeError>,
    ) -> Result<Option<T>, DecodeError> {
        match self.take(1)?[0] {
            0 => Ok(None),
            1 => decode(self).map(Some),
            tag => Err(DecodeError::UnknownTag {
                field,
                tag: u64::from(tag),
            }),
        }
    }

    /// Reads a count of at most `max`, then each item. Capacity is reserved
    /// only for as many items as the remaining bytes could hold, so a count
    /// never reserves memory the input cannot back.
    ///
    /// # Errors
    /// Returns [`DecodeError::TooLong`] or an item decoder's error.
    pub fn seq<T>(
        &mut self,
        max: usize,
        field: &'static str,
        mut decode: impl FnMut(&mut Self) -> Result<T, DecodeError>,
    ) -> Result<Vec<T>, DecodeError> {
        let count = self.u64()?;
        let count = usize::try_from(count)
            .ok()
            .filter(|count| *count <= max)
            .ok_or(DecodeError::TooLong { field })?;
        let mut items = Vec::with_capacity(count.min(self.bytes.len()));
        for _ in 0..count {
            items.push(decode(self)?);
        }
        Ok(items)
    }

    /// Reads a record: its byte length, then the fields `decode` reads from
    /// exactly those bytes. Bytes `decode` leaves unread belong to fields a
    /// newer peer appended and are skipped.
    ///
    /// # Errors
    /// Returns [`DecodeError::Truncated`] when the record is longer than the
    /// input, or the field decoder's error.
    pub fn record<T>(
        &mut self,
        decode: impl FnOnce(&mut Reader<'a>) -> Result<T, DecodeError>,
    ) -> Result<T, DecodeError> {
        let length = self.usize()?;
        let mut fields = Reader::new(self.take(length)?);
        decode(&mut fields)
    }

    /// Reads a field added in a later minor version: `default` when the
    /// record ends before it, as an older peer's record does.
    ///
    /// # Errors
    /// Returns the field decoder's error.
    pub fn appended<T>(
        &mut self,
        default: T,
        decode: impl FnOnce(&mut Self) -> Result<T, DecodeError>,
    ) -> Result<T, DecodeError> {
        if self.is_empty() {
            Ok(default)
        } else {
            decode(self)
        }
    }

    /// Reads an identity's bare numeric value into the connection's scope.
    ///
    /// # Errors
    /// Returns a varint error.
    pub fn id<T: WireId>(&mut self, scope: IdScope) -> Result<T, DecodeError> {
        Ok(T::scoped(scope.runtime, self.u64()?))
    }
}

/// The runtime scope a connection's identities travel in.
///
/// Identities on the wire are bare `u64` values; the client maps the
/// connection's server incarnation onto one client-local [`RuntimeId`] and
/// decodes every identity from that connection into it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IdScope {
    runtime: RuntimeId,
}

impl IdScope {
    /// The scope of identities in `runtime`.
    #[must_use]
    pub const fn new(runtime: RuntimeId) -> Self {
        Self { runtime }
    }

    /// The scope's runtime.
    #[must_use]
    pub const fn runtime(self) -> RuntimeId {
        self.runtime
    }
}

/// A runtime-scoped identity that travels as its bare numeric value.
pub trait WireId: Copy {
    /// Builds the identity in a runtime's scope.
    fn scoped(runtime: RuntimeId, value: u64) -> Self;
    /// The identity's runtime scope.
    fn runtime(self) -> RuntimeId;
    /// The identity's numeric value within its scope.
    fn value(self) -> u64;
}

macro_rules! wire_ids {
    ($($id:ident),* $(,)?) => {
        $(
            impl WireId for huterm_protocol::$id {
                fn scoped(runtime: RuntimeId, value: u64) -> Self {
                    Self::in_runtime(runtime, value)
                }
                fn runtime(self) -> RuntimeId {
                    huterm_protocol::$id::runtime(self)
                }
                fn value(self) -> u64 {
                    self.get()
                }
            }
        )*
    };
}

wire_ids!(
    AttachmentId,
    PaneId,
    SessionId,
    SplitId,
    TabId,
    TerminalId,
    ViewerId,
    WorkspaceId,
);

#[cfg(test)]
mod tests;
