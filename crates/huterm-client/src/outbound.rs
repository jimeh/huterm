//! What the client writes: whole messages, framed as the writer pulls them.
//!
//! The client sends every message on its control lane, which holds at most
//! one unfinished message, and writes each message's frames in full before
//! the next message's. Its stream is therefore free again before the next
//! message starts, and every message uses stream 0. A [`Transmit`] owns
//! the message, encoded once, and yields each frame as a small header and
//! a slice of that buffer, so no payload is copied before the transport
//! writes it.

use huterm_wire::frame::{Frame, PREAMBLE_LEN, preamble};

/// The stream every client message travels on.
const STREAM: u32 = 0;

/// A frame header on [`STREAM`]: the `u32` length, the flags byte, and the
/// one-byte stream varint.
pub const FRAME_HEADER_LEN: usize = 6;

/// The frame flag that ends a message, as `huterm-wire` frames it.
const FINAL: u8 = 0b01;

/// One message to write, preceded by the preamble when it opens a
/// connection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Transmit {
    preamble: Option<[u8; PREAMBLE_LEN]>,
    message: Vec<u8>,
    /// Message bytes per frame under the frame limit.
    room: usize,
}

impl Transmit {
    /// The preamble and `message`, framed under `limit`.
    pub(crate) fn opening(message: Vec<u8>, limit: u32) -> Self {
        Self {
            preamble: Some(preamble()),
            ..Self::message(message, limit)
        }
    }

    /// `message`, framed under `limit`.
    pub(crate) fn message(message: Vec<u8>, limit: u32) -> Self {
        Self {
            preamble: None,
            message,
            room: Frame::capacity(STREAM, limit).max(1),
        }
    }

    /// The preamble, when this transmit opens a connection.
    #[must_use]
    pub fn preamble(&self) -> Option<&[u8]> {
        self.preamble.as_ref().map(<[u8; PREAMBLE_LEN]>::as_slice)
    }

    /// Each frame's header and message bytes, in order.
    pub fn frames(
        &self,
    ) -> impl Iterator<Item = ([u8; FRAME_HEADER_LEN], &[u8])> + '_ {
        let count = self.frame_count();
        self.message
            .chunks(self.room)
            .enumerate()
            .map(move |(index, chunk)| {
                (header(chunk.len(), index + 1 == count), chunk)
            })
    }

    /// The bytes to write: the preamble, then every frame.
    #[must_use]
    pub fn len(&self) -> usize {
        self.preamble().map_or(0, <[u8]>::len)
            + self.message.len()
            + self.frame_count() * FRAME_HEADER_LEN
    }

    /// Whether there is nothing to write; a transmit always holds a
    /// message.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every byte, copied into one buffer.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.len());
        bytes.extend_from_slice(self.preamble().unwrap_or_default());
        for (header, payload) in self.frames() {
            bytes.extend_from_slice(&header);
            bytes.extend_from_slice(payload);
        }
        bytes
    }

    fn frame_count(&self) -> usize {
        self.message.len().div_ceil(self.room)
    }
}

/// The header of a frame on [`STREAM`] carrying `payload` message bytes.
fn header(payload: usize, last: bool) -> [u8; FRAME_HEADER_LEN] {
    // A frame's payload fits its `u32` limit, so the length always fits.
    let length =
        u32::try_from(Frame::body_len(STREAM, payload)).unwrap_or(u32::MAX);
    let mut header = [0; FRAME_HEADER_LEN];
    header[..4].copy_from_slice(&length.to_le_bytes());
    header[4] = if last { FINAL } else { 0 };
    // Stream 0's varint is the single byte 0.
    header[5] = 0;
    header
}

#[cfg(test)]
mod tests {
    use huterm_wire::frame::{FrameDecoder, FrameEnd, MIN_MAX_FRAME};

    use super::*;

    #[test]
    fn a_message_frames_whole_and_decodes_back_at_any_frame_limit() {
        let message: Vec<u8> = (0..=u8::MAX).cycle().take(150_000).collect();
        for limit in [MIN_MAX_FRAME, 1 << 20] {
            let transmit = Transmit::opening(message.clone(), limit);
            let bytes = transmit.to_bytes();
            assert_eq!(bytes.len(), transmit.len());
            assert_eq!(&bytes[..PREAMBLE_LEN], &preamble());
            let mut decoder = FrameDecoder::new();
            decoder.set_limit(limit);
            decoder.push(&bytes[PREAMBLE_LEN..]);
            let mut received = Vec::new();
            let mut ends = Vec::new();
            while let Some(frame) = decoder.next_frame().unwrap() {
                assert_eq!(frame.stream, STREAM);
                ends.push(frame.end);
                received.extend(frame.payload);
            }
            assert_eq!(received, message);
            assert_eq!(ends.pop(), Some(FrameEnd::Final));
            assert!(ends.iter().all(|end| *end == FrameEnd::More));
        }
    }
}
