//! Frames: every message may span frames on a stream, split at any byte.
//!
//! A frame is a little-endian `u32` length counting everything after it, a
//! flags byte, a varint stream ID, and message bytes. The receiver checks the
//! message's kind cap as bytes arrive and never trusts a declared size.

use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

use crate::wire::{DecodeError, Reader, Writer, varint_len};

const FINAL: u8 = 1;
const ABORT: u8 = 2;

/// Why a peer's frames broke the contract. The connection closes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Violation {
    /// A frame header was malformed or over the frame limit.
    BadFrame(&'static str),
    /// A message exceeded its kind's cap.
    OverCap,
    /// More streams were open than allowed.
    TooManyStreams,
    /// The server's reassembly budget was exceeded.
    OverBudget,
    /// An unfinished message received no frame for too long.
    Idle,
    /// A message failed to decode.
    Decode(DecodeError),
}

impl From<DecodeError> for Violation {
    fn from(error: DecodeError) -> Self {
        Self::Decode(error)
    }
}

/// Where a message's bytes come from. A lazy source encodes on demand, so
/// the sender holds the message, not its encoding, and builds one frame's
/// bytes at a time.
#[derive(Clone)]
pub enum Source {
    /// Bytes already encoded.
    Eager(Vec<u8>),
    /// An encoder and the encoding's length, from a sizing pass.
    Lazy(Rc<dyn Fn() -> Vec<u8>>, usize),
}

impl fmt::Debug for Source {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Eager(bytes) => write!(formatter, "Eager({})", bytes.len()),
            Self::Lazy(_, len) => write!(formatter, "Lazy({len})"),
        }
    }
}

impl Source {
    fn len(&self) -> usize {
        match self {
            Self::Eager(bytes) => bytes.len(),
            Self::Lazy(_, len) => *len,
        }
    }

    fn slice(&self, start: usize, end: usize) -> Vec<u8> {
        match self {
            Self::Eager(bytes) => bytes[start..end].to_vec(),
            Self::Lazy(encode, _) => encode()[start..end].to_vec(),
        }
    }
}

/// One built frame and what it carries.
#[derive(Debug)]
pub struct Frame {
    /// The frame's bytes, length prefix included.
    pub bytes: Vec<u8>,
    /// The message's serial number.
    pub serial: u64,
    /// Whether this is the message's first frame.
    pub first: bool,
    /// Whether this is the message's final or abort frame.
    pub last: bool,
    /// Whether the message is a priority message.
    pub priority: bool,
    /// The message's ordering lane.
    pub lane: u64,
}

#[derive(Debug)]
struct OutStream {
    data: Source,
    pos: usize,
    priority: bool,
    abort: bool,
    lane: u64,
    serial: u64,
    built: bool,
}

/// Splits outgoing messages into frames, one frame at a time.
///
/// Every message belongs to an ordering lane. A lane holds at most one
/// unfinished message, so its messages complete in the order they started;
/// different lanes interleave freely.
#[derive(Debug)]
pub struct FrameSender {
    max_frame: usize,
    lanes: bool,
    hold: bool,
    streams: Vec<Option<OutStream>>,
    cursor: usize,
    serial: u64,
}

impl FrameSender {
    /// Creates a sender with `max_open` streams and frames of at most
    /// `max_frame` bytes after the length. Without `lanes`, messages of one
    /// lane may overlap, as a design without ordering lanes would allow.
    #[must_use]
    pub fn new(max_frame: usize, max_open: usize, lanes: bool) -> Self {
        Self {
            max_frame,
            lanes,
            hold: false,
            streams: (0..max_open).map(|_| None).collect(),
            cursor: 0,
            serial: 0,
        }
    }

    /// Keeps a stream busy after its last frame is built, until the writer
    /// calls [`Self::release`] once that frame is written. A writer that
    /// reorders frames, as one that sends urgent frames first does, must
    /// not let a new message take a stream whose previous message still
    /// has a queued frame.
    #[must_use]
    pub fn holding_until_written(mut self) -> Self {
        self.hold = true;
        self
    }

    /// Whether a priority message, or its abort, still has a frame to
    /// build.
    #[must_use]
    pub fn has_urgent(&self) -> bool {
        self.streams
            .iter()
            .flatten()
            .any(|out| out.priority && !out.built)
    }

    /// Frees the stream of the message with `serial`, whose last frame was
    /// written.
    pub fn release(&mut self, serial: u64) {
        for slot in &mut self.streams {
            if slot
                .as_ref()
                .is_some_and(|out| out.serial == serial && out.built)
            {
                *slot = None;
            }
        }
    }

    /// Whether a stream is free for a new message.
    #[must_use]
    pub fn has_free_stream(&self) -> bool {
        self.streams.iter().any(Option::is_none)
    }

    /// Unfinished messages.
    #[must_use]
    pub fn open(&self) -> usize {
        self.streams
            .iter()
            .filter(|stream| stream.is_some())
            .count()
    }

    /// Whether a message on `lane` may start now: a stream is free and the
    /// lane's previous message has finished.
    #[must_use]
    pub fn can_start(&self, lane: u64) -> bool {
        self.has_free_stream()
            && (!self.lanes
                || !self
                    .streams
                    .iter()
                    .flatten()
                    .any(|out| out.lane == lane && !out.built))
    }

    /// Starts sending `message` on `lane`, or returns `None` so the caller
    /// waits. Priority streams send their frames first. Returns the stream
    /// and the message's serial number.
    pub fn start(
        &mut self,
        lane: u64,
        message: Vec<u8>,
        priority: bool,
    ) -> Option<(usize, u64)> {
        self.start_source(lane, Source::Eager(message), priority)
    }

    /// Starts a message from any source; see [`Self::start`].
    pub fn start_source(
        &mut self,
        lane: u64,
        message: Source,
        priority: bool,
    ) -> Option<(usize, u64)> {
        if !self.can_start(lane) {
            return None;
        }
        let id = self.streams.iter().position(Option::is_none)?;
        self.serial += 1;
        let serial = self.serial;
        self.streams[id] = Some(OutStream {
            data: message,
            pos: 0,
            priority,
            abort: false,
            lane,
            serial,
            built: false,
        });
        Some((id, serial))
    }

    /// Whether the message with `serial` has had every frame built.
    #[must_use]
    pub fn finished(&self, serial: u64) -> bool {
        serial <= self.serial
            && !self
                .streams
                .iter()
                .flatten()
                .any(|out| out.serial == serial && !out.built)
    }

    /// Abandons the message with `serial`, if it is still unfinished. A
    /// stream ID is reused once its message finishes, so aborting by stream
    /// could kill an unrelated message. The source is released now; the
    /// receiver learns from an abort frame, or from nothing when no frame
    /// was sent yet. Returns whether the message was still unfinished.
    pub fn abort(&mut self, serial: u64) -> bool {
        let Some(id) = self.streams.iter().position(|s| {
            s.as_ref()
                .is_some_and(|out| out.serial == serial && !out.built)
        }) else {
            return false;
        };
        let out = self.streams[id].as_mut().unwrap_or_else(|| unreachable!());
        if out.pos == 0 {
            self.streams[id] = None;
        } else {
            out.abort = true;
            out.data = Source::Eager(Vec::new());
        }
        true
    }

    /// Encoded bytes the sender holds for unfinished messages. Lazy
    /// sources hold none.
    #[must_use]
    pub fn retained(&self) -> usize {
        self.streams
            .iter()
            .flatten()
            .map(|out| match &out.data {
                Source::Eager(bytes) => bytes.len() - out.pos.min(bytes.len()),
                Source::Lazy(..) => 0,
            })
            .sum()
    }

    /// Whether `stream` still has a message in flight.
    #[must_use]
    pub fn is_open(&self, stream: usize) -> bool {
        self.streams.get(stream).is_some_and(Option::is_some)
    }

    /// Builds the next frame, priority streams first, otherwise round robin.
    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        self.build_next().map(|frame| frame.bytes)
    }

    /// Builds the next frame with what it carries.
    pub fn build_next(&mut self) -> Option<Frame> {
        self.next_where(true)
    }

    /// Builds the next frame, skipping priority messages unless
    /// `priority` allows them, so a writer can build ordinary frames while
    /// an urgent frame waits to be written.
    pub fn next_where(&mut self, priority: bool) -> Option<Frame> {
        let count = self.streams.len();
        let urgent = (0..count)
            .map(|offset| (self.cursor + offset) % count)
            .find(|&id| {
                self.streams[id]
                    .as_ref()
                    .is_some_and(|out| out.priority && !out.built)
            });
        let pick = urgent.filter(|_| priority).or_else(|| {
            (0..count)
                .map(|offset| (self.cursor + offset) % count)
                .find(|&id| {
                    self.streams[id].as_ref().is_some_and(|out| {
                        !out.built && (priority || !out.priority)
                    })
                })
        })?;
        self.cursor = (pick + 1) % count;
        let out = self.streams[pick].as_mut()?;
        let header = 1 + varint_len(pick as u64);
        let mut body = Writer::default();
        let (serial, priority, lane, first) =
            (out.serial, out.priority, out.lane, out.pos == 0);
        if out.abort {
            if self.hold {
                out.built = true;
            } else {
                self.streams[pick] = None;
            }
            let bytes = frame(ABORT, pick, &[], &mut body);
            return Some(Frame {
                bytes,
                serial,
                first: false,
                last: true,
                priority,
                lane,
            });
        }
        let room = self.max_frame.saturating_sub(header).max(1);
        let end = (out.pos + room).min(out.data.len());
        let last = end == out.data.len();
        let payload = out.data.slice(out.pos, end);
        let bytes =
            frame(if last { FINAL } else { 0 }, pick, &payload, &mut body);
        out.pos = end;
        if last {
            if self.hold {
                out.built = true;
            } else {
                self.streams[pick] = None;
            }
        }
        Some(Frame {
            bytes,
            serial,
            first,
            last,
            priority,
            lane,
        })
    }
}

fn frame(
    flags: u8,
    stream: usize,
    payload: &[u8],
    body: &mut Writer,
) -> Vec<u8> {
    let mut inner = std::mem::take(body);
    inner.u(u64::from(flags));
    inner.u(stream as u64);
    let mut bytes = inner.finish();
    bytes.extend_from_slice(payload);
    let mut out = u32::try_from(bytes.len())
        .unwrap_or(u32::MAX)
        .to_le_bytes()
        .to_vec();
    out.extend_from_slice(&bytes);
    out
}

#[derive(Debug)]
struct InStream {
    bytes: Vec<u8>,
    last: u64,
}

/// Reassembles incoming frames into messages, checking every limit as
/// bytes arrive.
#[derive(Debug)]
pub struct FrameReceiver {
    max_frame: usize,
    max_open: usize,
    budget: Option<usize>,
    pending: Vec<u8>,
    streams: BTreeMap<u64, InStream>,
}

impl FrameReceiver {
    /// Creates a receiver. `budget` bounds the bytes of unfinished messages,
    /// as the server's reassembly budget does.
    #[must_use]
    pub fn new(
        max_frame: usize,
        max_open: usize,
        budget: Option<usize>,
    ) -> Self {
        Self {
            max_frame,
            max_open,
            budget,
            pending: Vec::new(),
            streams: BTreeMap::new(),
        }
    }

    /// Changes the frame limit once the handshake settles it.
    pub fn set_max_frame(&mut self, max_frame: usize) {
        self.max_frame = max_frame;
    }

    /// Bytes held for unfinished messages and partial frames.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.pending.len()
            + self.streams.values().map(|s| s.bytes.len()).sum::<usize>()
    }

    /// Unfinished messages.
    #[must_use]
    pub fn open(&self) -> usize {
        self.streams.len()
    }

    /// Accepts bytes and returns every message they complete. `cap` gives a
    /// message kind's byte cap, or `None` for a kind that was not negotiated.
    ///
    /// # Errors
    /// Returns the first violation; the caller closes the connection.
    pub fn push(
        &mut self,
        bytes: &[u8],
        now: u64,
        cap: &dyn Fn(u64) -> Option<usize>,
    ) -> Result<Vec<Vec<u8>>, Violation> {
        self.pending.extend_from_slice(bytes);
        let mut done = Vec::new();
        while let Some(header) = self.pending.get(..4) {
            let len = u32::from_le_bytes([
                header[0], header[1], header[2], header[3],
            ]) as usize;
            if len == 0 || len > self.max_frame {
                return Err(Violation::BadFrame("length"));
            }
            if self.pending.len() < 4 + len {
                break;
            }
            let body: Vec<u8> = self.pending.drain(..4 + len).skip(4).collect();
            let mut reader = Reader::new(&body);
            let flags =
                reader.u_max(3).map_err(|_| Violation::BadFrame("flags"))?;
            let stream = reader.u()?;
            let payload = &body[reader.position()..];
            if flags & u64::from(ABORT) != 0 {
                if !payload.is_empty() || self.streams.remove(&stream).is_none()
                {
                    return Err(Violation::BadFrame("abort"));
                }
                continue;
            }
            if !self.streams.contains_key(&stream)
                && self.streams.len() >= self.max_open
            {
                return Err(Violation::TooManyStreams);
            }
            let entry = self.streams.entry(stream).or_insert(InStream {
                bytes: Vec::new(),
                last: now,
            });
            entry.bytes.extend_from_slice(payload);
            entry.last = now;
            check_cap(&entry.bytes, cap)?;
            if let Some(budget) = self.budget
                && self.streams.values().map(|s| s.bytes.len()).sum::<usize>()
                    > budget
            {
                return Err(Violation::OverBudget);
            }
            if flags & u64::from(FINAL) != 0 {
                let message = self
                    .streams
                    .remove(&stream)
                    .map(|s| s.bytes)
                    .unwrap_or_default();
                done.push(message);
            }
        }
        Ok(done)
    }

    /// Fails when an unfinished message has had no frame for `deadline`
    /// milliseconds. The caller skips this while it has paused reading.
    ///
    /// # Errors
    /// Returns [`Violation::Idle`].
    pub fn check_idle(&self, now: u64, deadline: u64) -> Result<(), Violation> {
        if self
            .streams
            .values()
            .any(|s| now.saturating_sub(s.last) > deadline)
        {
            Err(Violation::Idle)
        } else {
            Ok(())
        }
    }

    /// Restarts every idle clock, after a pause in reading.
    pub fn touch(&mut self, now: u64) {
        for stream in self.streams.values_mut() {
            stream.last = now;
        }
    }
}

fn check_cap(
    bytes: &[u8],
    cap: &dyn Fn(u64) -> Option<usize>,
) -> Result<(), Violation> {
    let mut reader = Reader::new(bytes);
    match reader.u() {
        Ok(kind) => {
            let limit = cap(kind)
                .ok_or(Violation::Decode(DecodeError::UnknownTag(kind)))?;
            if bytes.len() > limit {
                return Err(Violation::OverCap);
            }
            if let Ok(len) = reader.u() {
                let declared = varint_len(kind)
                    + varint_len(len)
                    + usize::try_from(len).unwrap_or(usize::MAX);
                if declared > limit {
                    return Err(Violation::OverCap);
                }
            }
            Ok(())
        }
        Err(DecodeError::Truncated) => Ok(()),
        Err(error) => Err(Violation::Decode(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(kind: u64, body: &[u8]) -> Vec<u8> {
        let mut writer = Writer::default();
        writer.u(kind);
        writer.bytes(body);
        writer.finish()
    }

    fn frames(sender: &mut FrameSender) -> Vec<u8> {
        let mut bytes = Vec::new();
        while let Some(frame) = sender.next_frame() {
            bytes.extend(frame);
        }
        bytes
    }

    #[test]
    fn messages_survive_any_split_and_lanes_keep_their_order() {
        let mut sender = FrameSender::new(8, 4, true);
        let first = message(1, b"a long message spanning frames");
        let second = message(1, b"next");
        sender.start(5, first.clone(), false);
        assert!(!sender.can_start(5), "a lane holds one unfinished message");
        let other = message(1, b"other lane");
        sender.start(6, other.clone(), false);
        let mut bytes = frames(&mut sender);
        sender.start(5, second.clone(), false);
        bytes.extend(frames(&mut sender));
        for split in 0..=bytes.len() {
            let mut receiver = FrameReceiver::new(8, 4, None);
            let cap = |_| Some(64);
            let mut got =
                receiver.push(&bytes[..split], 0, &cap).unwrap_or_default();
            got.extend(
                receiver.push(&bytes[split..], 0, &cap).unwrap_or_default(),
            );
            let lane5: Vec<_> =
                got.iter().filter(|m| **m != other).cloned().collect();
            assert_eq!(
                lane5,
                vec![first.clone(), second.clone()],
                "split at {split}"
            );
            assert!(got.contains(&other));
        }
    }

    #[test]
    fn receivers_enforce_caps_streams_budgets_and_idle_time() {
        let mut sender = FrameSender::new(8, 4, true);
        sender.start(1, message(1, &[7; 40]), false);
        let bytes = frames(&mut sender);
        let mut receiver = FrameReceiver::new(8, 4, None);
        assert_eq!(
            receiver.push(&bytes, 0, &|_| Some(20)),
            Err(Violation::OverCap)
        );
        let mut receiver = FrameReceiver::new(8, 4, None);
        assert!(matches!(
            receiver.push(&bytes, 0, &|_| None),
            Err(Violation::Decode(_))
        ));

        let mut sender = FrameSender::new(8, 5, false);
        for lane in 0..5 {
            sender.start(lane, message(1, &[1; 20]), false);
        }
        // Five streams' first frames arrive before any finishes.
        let first_frames: Vec<u8> = (0..5)
            .flat_map(|_| sender.next_frame().unwrap_or_default())
            .collect();
        let mut receiver = FrameReceiver::new(8, 4, None);
        assert_eq!(
            receiver.push(&first_frames, 0, &|_| Some(64)),
            Err(Violation::TooManyStreams)
        );
        let mut receiver = FrameReceiver::new(8, 5, Some(10));
        assert_eq!(
            receiver.push(&first_frames, 0, &|_| Some(64)),
            Err(Violation::OverBudget)
        );

        let mut sender = FrameSender::new(8, 4, true);
        let (_, serial) = sender
            .start(1, message(1, &[1; 20]), false)
            .unwrap_or_default();
        let opening = sender.next_frame().unwrap_or_default();
        let mut receiver = FrameReceiver::new(8, 4, None);
        receiver
            .push(&opening, 0, &|_| Some(64))
            .unwrap_or_default();
        assert_eq!(receiver.check_idle(100, 50), Err(Violation::Idle));
        assert!(sender.abort(serial));
        let abort = sender.next_frame().unwrap_or_default();
        assert_eq!(receiver.push(&abort, 1, &|_| Some(64)), Ok(Vec::new()));
        assert_eq!(receiver.open(), 0);
        assert_eq!(receiver.check_idle(1_000, 50), Ok(()));

        // Once a message finishes, its stream is reused; aborting the old
        // message must not touch the new one.
        let (stream, finished) = sender
            .start(1, message(1, b"done"), false)
            .unwrap_or_default();
        frames(&mut sender);
        let (reused, _) = sender
            .start(1, message(1, b"live"), false)
            .unwrap_or_default();
        assert_eq!(stream, reused);
        assert!(!sender.abort(finished));
        assert!(sender.is_open(reused));
    }
}
