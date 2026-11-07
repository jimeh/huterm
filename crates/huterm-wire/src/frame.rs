//! The connection preamble and frames.
//!
//! Each side opens a connection with an 8-byte preamble: `HUTRM`, a zero
//! byte, and a little-endian `u16` framing version. The preamble never
//! changes, so a peer rejects a foreign or future-framing endpoint before it
//! parses a frame.
//!
//! A frame is a little-endian `u32` length, then a flags byte, a varint
//! stream ID, and the stream's next message bytes. The length counts
//! everything after itself and never exceeds the frame limit. A zero length,
//! a length above the limit, or an unknown flag ends the connection, because
//! a byte stream cannot resynchronize after a bad header.

use std::fmt;

use crate::codec::{DecodeError, Reader, Writer, varint_len};

/// The bytes that open every connection, before the framing version.
pub const PREAMBLE_MAGIC: [u8; 6] = *b"HUTRM\0";

/// The preamble's length: the magic and a `u16` framing version.
pub const PREAMBLE_LEN: usize = 8;

/// The framing version this build speaks.
pub const FRAMING_VERSION: u16 = 1;

/// The frame limit until the handshake completes.
pub const HANDSHAKE_FRAME_LIMIT: u32 = 16 << 10;

/// The `max_frame` a peer offers unless configured otherwise.
pub const DEFAULT_MAX_FRAME: u32 = 1 << 20;

/// The smallest `max_frame` a peer may offer.
pub const MIN_MAX_FRAME: u32 = 64 << 10;

/// The flag that ends the stream's message with this frame.
const FINAL: u8 = 0b01;
/// The flag that abandons the stream's unfinished message.
const ABORT: u8 = 0b10;

/// The length prefix's size.
const LENGTH_BYTES: usize = 4;

/// Why a preamble or frame broke the framing rules. Every one ends the
/// connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameError {
    /// The preamble's magic bytes are not Huterm's.
    ForeignPreamble,
    /// The peer frames with a version this build does not speak.
    UnsupportedFraming(u16),
    /// A frame length of zero.
    EmptyFrame,
    /// A frame length above the frame limit.
    OverLimit {
        /// The declared length.
        length: u32,
        /// The limit in force.
        limit: u32,
    },
    /// A flag bit this version does not define, or abort together with
    /// final.
    InvalidFlags(u8),
    /// An abort frame that carries message bytes.
    AbortWithPayload,
    /// The stream ID does not decode.
    InvalidStream(DecodeError),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ForeignPreamble => f.write_str("not a Huterm endpoint"),
            Self::UnsupportedFraming(version) => {
                write!(f, "unsupported framing version {version}")
            }
            Self::EmptyFrame => f.write_str("frame length is zero"),
            Self::OverLimit { length, limit } => {
                write!(f, "frame length {length} exceeds the limit {limit}")
            }
            Self::InvalidFlags(flags) => {
                write!(f, "invalid frame flags {flags:#04x}")
            }
            Self::AbortWithPayload => f.write_str("abort frame carries bytes"),
            Self::InvalidStream(error) => {
                write!(f, "invalid stream ID: {error}")
            }
        }
    }
}

impl std::error::Error for FrameError {}

/// The preamble this build writes.
#[must_use]
pub fn preamble() -> [u8; PREAMBLE_LEN] {
    let mut bytes = [0; PREAMBLE_LEN];
    bytes[..PREAMBLE_MAGIC.len()].copy_from_slice(&PREAMBLE_MAGIC);
    bytes[PREAMBLE_MAGIC.len()..]
        .copy_from_slice(&FRAMING_VERSION.to_le_bytes());
    bytes
}

/// Checks a peer's preamble.
///
/// # Errors
/// Rejects foreign magic and any framing version but this build's.
pub fn check_preamble(bytes: [u8; PREAMBLE_LEN]) -> Result<(), FrameError> {
    if bytes[..PREAMBLE_MAGIC.len()] != PREAMBLE_MAGIC {
        return Err(FrameError::ForeignPreamble);
    }
    let version = u16::from_le_bytes([bytes[6], bytes[7]]);
    if version != FRAMING_VERSION {
        return Err(FrameError::UnsupportedFraming(version));
    }
    Ok(())
}

/// What a frame does to its stream's message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameEnd {
    /// More frames of the message follow.
    More,
    /// The message ends with this frame.
    Final,
    /// The sender abandoned the message; the frame carries no bytes.
    Abort,
}

/// One frame: a stream, what it does to that stream's message, and the
/// message bytes it carries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    /// The message stream.
    pub stream: u32,
    /// Whether the message continues, ends, or is abandoned.
    pub end: FrameEnd,
    /// Message bytes; empty for an abort.
    pub payload: Vec<u8>,
}

impl Frame {
    /// The bytes a frame of `payload` on `stream` takes after its length
    /// prefix.
    #[must_use]
    pub fn body_len(stream: u32, payload: usize) -> usize {
        1 + varint_len(u64::from(stream)) + payload
    }

    /// The most message bytes one frame on `stream` carries under `limit`.
    #[must_use]
    pub fn capacity(stream: u32, limit: u32) -> usize {
        (limit as usize).saturating_sub(Self::body_len(stream, 0))
    }

    /// Encodes the frame with its length prefix.
    ///
    /// # Errors
    /// Refuses a frame longer than `limit`, or an abort that carries bytes.
    pub fn encode(&self, limit: u32) -> Result<Vec<u8>, FrameError> {
        if self.end == FrameEnd::Abort && !self.payload.is_empty() {
            return Err(FrameError::AbortWithPayload);
        }
        let body = Self::body_len(self.stream, self.payload.len());
        let length = u32::try_from(body).unwrap_or(u32::MAX);
        if length > limit {
            return Err(FrameError::OverLimit { length, limit });
        }
        let mut bytes = Vec::with_capacity(LENGTH_BYTES + body);
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.push(match self.end {
            FrameEnd::More => 0,
            FrameEnd::Final => FINAL,
            FrameEnd::Abort => ABORT,
        });
        let mut stream = Writer::new();
        stream.u32(self.stream);
        bytes.extend_from_slice(&stream.into_bytes());
        bytes.extend_from_slice(&self.payload);
        Ok(bytes)
    }
}

/// Splits a byte stream into frames, checking each header as soon as its
/// length arrives, before buffering the frame.
#[derive(Debug)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
    start: usize,
    limit: u32,
    failed: Option<FrameError>,
}

impl FrameDecoder {
    /// A decoder that starts at the handshake frame limit.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            start: 0,
            limit: HANDSHAKE_FRAME_LIMIT,
            failed: None,
        }
    }

    /// Sets the frame limit, the negotiated `max_frame` once the
    /// handshake completes.
    pub fn set_limit(&mut self, limit: u32) {
        self.limit = limit;
    }

    /// The frame limit in force.
    #[must_use]
    pub fn limit(&self) -> u32 {
        self.limit
    }

    /// Appends bytes read from the connection.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.start > 0 && self.start == self.buffer.len() {
            self.buffer.clear();
            self.start = 0;
        }
        self.buffer.extend_from_slice(bytes);
    }

    /// The bytes received but not yet returned in a frame.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buffer.len() - self.start
    }

    /// Takes the next complete frame, if one has arrived.
    ///
    /// # Errors
    /// Returns the first framing violation, and the same error on every
    /// later call: the connection cannot continue.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        let result = self.decode();
        if let Err(error) = result {
            self.failed = Some(error);
        }
        result
    }

    fn decode(&mut self) -> Result<Option<Frame>, FrameError> {
        let pending = &self.buffer[self.start..];
        let Some(prefix) = pending.first_chunk::<LENGTH_BYTES>() else {
            return Ok(None);
        };
        let length = u32::from_le_bytes(*prefix);
        if length == 0 {
            return Err(FrameError::EmptyFrame);
        }
        if length > self.limit {
            return Err(FrameError::OverLimit {
                length,
                limit: self.limit,
            });
        }
        let total = LENGTH_BYTES + length as usize;
        if pending.len() < total {
            return Ok(None);
        }
        let body = &pending[LENGTH_BYTES..total];
        let end = match body[0] {
            0 => FrameEnd::More,
            FINAL => FrameEnd::Final,
            ABORT => FrameEnd::Abort,
            flags => return Err(FrameError::InvalidFlags(flags)),
        };
        let mut reader = Reader::new(&body[1..]);
        let stream = reader.u32().map_err(FrameError::InvalidStream)?;
        let payload = body[body.len() - reader.remaining()..].to_vec();
        if end == FrameEnd::Abort && !payload.is_empty() {
            return Err(FrameError::AbortWithPayload);
        }
        self.start += total;
        if self.start == self.buffer.len() {
            self.buffer.clear();
            self.start = 0;
        } else if self.start > self.buffer.len() / 2 {
            self.buffer.drain(..self.start);
            self.start = 0;
        }
        Ok(Some(Frame {
            stream,
            end,
            payload,
        }))
    }
}

impl Default for FrameDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
