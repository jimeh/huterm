//! Message streams: reassembling messages from interleaved frames, and
//! splitting outgoing messages into frames.
//!
//! A message is the concatenation of the bytes its stream's frames carry,
//! ending with the frame that sets the final flag, so any message may span
//! frames and frame size never limits what can be encoded. Each direction
//! holds at most [`MAX_OPEN_STREAMS`] unfinished messages. A receiver checks
//! the message's kind and declared length against its cap before buffering,
//! and checks the cap and the reassembly budget again as each frame
//! arrives, so it never reserves memory on a sender's word. A conforming
//! sender waits instead of opening a fifth stream or passing the budget.

use std::fmt;
use std::time::Duration;

use crate::caps::{Direction, MessageCaps, MessageKind};
use crate::codec::{DecodeError, Reader};
use crate::frame::{Frame, FrameEnd};

/// The most unfinished messages one direction of a connection holds.
pub const MAX_OPEN_STREAMS: usize = 4;

/// How long a stream may go without a frame before the receiver ends the
/// connection.
pub const STREAM_IDLE_DEADLINE: Duration = Duration::from_secs(30);

/// The stream the server keeps for host effects, so an effect never waits
/// for a stream behind queued messages.
pub const HOST_EFFECT_STREAM: u32 = 3;
const _: () = assert!(HOST_EFFECT_STREAM as usize == MAX_OPEN_STREAMS - 1);

/// The most bytes a message's header takes: its kind varint, its record
/// length prefix, and, for a terminal update, the grid whose size bounds
/// the update.
const MAX_HEADER_BYTES: usize = 20 + 2 * 3;

/// Why a received frame broke the stream rules. Every one ends the
/// connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamError {
    /// A frame would open more than [`MAX_OPEN_STREAMS`] messages.
    TooManyStreams,
    /// An abort names a stream with no message in flight.
    AbortWithoutMessage(u32),
    /// The message's kind or record header does not decode.
    Header(DecodeError),
    /// The receiver does not accept this kind from its peer, or not at the
    /// negotiated version.
    UnexpectedKind(u64),
    /// The message declares, or has received, more bytes than its kind's
    /// cap.
    OverCap {
        /// The message's kind.
        kind: MessageKind,
    },
    /// Frames carried more bytes than the message declared, or the final
    /// frame arrived before all of them.
    LengthMismatch(u32),
    /// Unfinished messages hold more bytes than the advertised reassembly
    /// budget.
    OverBudget,
    /// A stream received no frame for [`STREAM_IDLE_DEADLINE`].
    Idle(u32),
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyStreams => f.write_str("too many open streams"),
            Self::AbortWithoutMessage(stream) => {
                write!(f, "abort on stream {stream} without a message")
            }
            Self::Header(error) => write!(f, "invalid message header: {error}"),
            Self::UnexpectedKind(kind) => {
                write!(f, "unexpected message kind {kind}")
            }
            Self::OverCap { kind } => {
                write!(f, "{kind:?} message exceeds its cap")
            }
            Self::LengthMismatch(stream) => {
                write!(
                    f,
                    "stream {stream} carried a different length than it declared"
                )
            }
            Self::OverBudget => f.write_str("reassembly budget exceeded"),
            Self::Idle(stream) => write!(f, "stream {stream} went idle"),
        }
    }
}

impl std::error::Error for StreamError {}

/// A complete message: its kind and every byte, the kind varint included.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Assembled {
    /// The message's kind.
    pub kind: MessageKind,
    /// The message's bytes.
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
struct Inbound {
    stream: u32,
    /// The kind and the message's total length once its header arrived.
    declared: Option<(MessageKind, usize)>,
    bytes: Vec<u8>,
    last_frame: Duration,
}

/// Reassembles one direction's messages from frames.
#[derive(Debug)]
pub struct Reassembler {
    caps: MessageCaps,
    from: Direction,
    budget: usize,
    open: Vec<Inbound>,
    buffered: usize,
    paused: Option<Duration>,
}

impl Reassembler {
    /// Reassembles messages that `from`'s side sends, each within its cap
    /// under `caps`, with at most `budget` bytes of unfinished messages.
    #[must_use]
    pub fn new(caps: MessageCaps, from: Direction, budget: usize) -> Self {
        Self {
            caps,
            from,
            budget,
            open: Vec::new(),
            buffered: 0,
            paused: None,
        }
    }

    /// Replaces the caps, once the handshake negotiates a version.
    pub fn set_caps(&mut self, caps: MessageCaps, budget: usize) {
        self.caps = caps;
        self.budget = budget;
    }

    /// The bytes unfinished messages hold.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buffered
    }

    /// The number of unfinished messages.
    #[must_use]
    pub fn open_streams(&self) -> usize {
        self.open.len()
    }

    /// Accepts one frame received at `now`, returning the message it
    /// completes.
    ///
    /// # Errors
    /// Returns the stream rule the frame breaks; the connection must end.
    pub fn accept(
        &mut self,
        frame: Frame,
        now: Duration,
    ) -> Result<Option<Assembled>, StreamError> {
        let index = self
            .open
            .iter()
            .position(|open| open.stream == frame.stream);
        if frame.end == FrameEnd::Abort {
            let index =
                index.ok_or(StreamError::AbortWithoutMessage(frame.stream))?;
            let aborted = self.open.swap_remove(index);
            self.buffered -= aborted.bytes.len();
            return Ok(None);
        }
        let index = if let Some(index) = index {
            index
        } else {
            if self.open.len() == MAX_OPEN_STREAMS {
                return Err(StreamError::TooManyStreams);
            }
            self.open.push(Inbound {
                stream: frame.stream,
                declared: None,
                bytes: Vec::new(),
                last_frame: now,
            });
            self.open.len() - 1
        };
        if self.buffered + frame.payload.len() > self.budget {
            return Err(StreamError::OverBudget);
        }
        let inbound = &mut self.open[index];
        // A frame read while the clock is paused counts from the pause, so
        // resuming does not credit the paused span twice.
        inbound.last_frame = self.paused.unwrap_or(now);
        if let Some((_, total)) = inbound.declared
            && inbound.bytes.len() + frame.payload.len() > total
        {
            return Err(StreamError::LengthMismatch(frame.stream));
        }
        let received = frame.payload.len();
        let mut payload = frame.payload;
        inbound.bytes.append(&mut payload);
        self.buffered += received;
        if inbound.declared.is_none() {
            inbound.declared = declared(&inbound.bytes, self.caps, self.from)?;
            if let Some((_, total)) = inbound.declared
                && inbound.bytes.len() > total
            {
                return Err(StreamError::LengthMismatch(frame.stream));
            }
        }
        if frame.end == FrameEnd::More {
            return Ok(None);
        }
        let inbound = self.open.swap_remove(index);
        self.buffered -= inbound.bytes.len();
        match inbound.declared {
            Some((kind, total)) if inbound.bytes.len() == total => {
                Ok(Some(Assembled {
                    kind,
                    bytes: inbound.bytes,
                }))
            }
            Some(_) => Err(StreamError::LengthMismatch(frame.stream)),
            None => Err(StreamError::Header(DecodeError::Truncated)),
        }
    }

    /// Stops the idle clock while the receiver itself is not reading, so a
    /// paused reader never blames its peer.
    pub fn pause(&mut self, now: Duration) {
        self.paused.get_or_insert(now);
    }

    /// Restarts the idle clock, crediting every stream with the paused
    /// time.
    pub fn resume(&mut self, now: Duration) {
        if let Some(since) = self.paused.take() {
            let paused = now.saturating_sub(since);
            for open in &mut self.open {
                open.last_frame += paused;
            }
        }
    }

    /// When the earliest unfinished stream goes idle; `None` while no
    /// message is in flight or the clock is paused, so an idle connection
    /// arms nothing.
    #[must_use]
    pub fn idle_deadline(&self) -> Option<Duration> {
        if self.paused.is_some() {
            return None;
        }
        self.open
            .iter()
            .map(|open| open.last_frame + STREAM_IDLE_DEADLINE)
            .min()
    }

    /// Checks the idle deadline at `now`.
    ///
    /// # Errors
    /// Returns [`StreamError::Idle`] for a stream that received no frame
    /// for [`STREAM_IDLE_DEADLINE`].
    pub fn check_idle(&self, now: Duration) -> Result<(), StreamError> {
        if self.paused.is_some() {
            return Ok(());
        }
        match self
            .open
            .iter()
            .find(|open| now >= open.last_frame + STREAM_IDLE_DEADLINE)
        {
            Some(open) => Err(StreamError::Idle(open.stream)),
            None => Ok(()),
        }
    }
}

/// Reads a message's kind and record length once enough bytes arrived,
/// checking the kind's direction, version, and cap.
fn declared(
    bytes: &[u8],
    caps: MessageCaps,
    from: Direction,
) -> Result<Option<(MessageKind, usize)>, StreamError> {
    let mut reader = Reader::new(&bytes[..bytes.len().min(MAX_HEADER_BYTES)]);
    let header = reader.u64().and_then(|kind| Ok((kind, reader.usize()?)));
    let (number, record) = match header {
        Ok(header) => header,
        Err(DecodeError::Truncated) if bytes.len() < MAX_HEADER_BYTES => {
            return Ok(None);
        }
        Err(error) => return Err(StreamError::Header(error)),
    };
    let kind = MessageKind::from_number(number)
        .filter(|kind| kind.direction() == from)
        .ok_or(StreamError::UnexpectedKind(number))?;
    let mut cap = caps.cap(kind).ok_or(StreamError::UnexpectedKind(number))?;
    let header_len = bytes.len().min(MAX_HEADER_BYTES) - reader.remaining();
    if kind == MessageKind::TerminalUpdate {
        // An update's cap follows the grid that opens its record, so the
        // receiver waits for the grid before it buffers more.
        match crate::update::read_grid(&mut reader) {
            Ok(size) => {
                cap = caps.update_cap(size).ok_or(StreamError::Header(
                    DecodeError::Invalid("grid size"),
                ))?;
            }
            Err(DecodeError::Truncated) if bytes.len() < MAX_HEADER_BYTES => {
                return Ok(None);
            }
            Err(error) => return Err(StreamError::Header(error)),
        }
    }
    let total = header_len
        .checked_add(record)
        .filter(|total| *total <= cap)
        .ok_or(StreamError::OverCap { kind })?;
    Ok(Some((kind, total)))
}

/// Why a sender cannot start a message now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendBlocked {
    /// Every stream holds an unfinished message; wait for one to finish.
    NoStream,
    /// The message would take the peer past its reassembly budget; wait
    /// for unfinished messages to finish.
    OverBudget,
}

/// The sender's view of its streams: which hold an unfinished message, and
/// how many bytes those messages hold against the peer's reassembly
/// budget.
#[derive(Debug)]
pub struct OutboundStreams {
    busy: [Option<usize>; MAX_OPEN_STREAMS],
    reserved: Option<u32>,
    budget: usize,
}

impl OutboundStreams {
    /// Streams of a sender whose peer accepts at most `budget` bytes of
    /// unfinished messages, keeping `reserved` for one producer, such as
    /// the server's host-effect lane.
    #[must_use]
    pub fn new(budget: usize, reserved: Option<u32>) -> Self {
        Self {
            busy: [None; MAX_OPEN_STREAMS],
            reserved,
            budget,
        }
    }

    fn pending(&self) -> usize {
        self.busy.iter().flatten().sum()
    }

    /// Takes a free ordinary stream for a message of `len` bytes.
    ///
    /// # Errors
    /// Returns [`SendBlocked`] when the sender must wait.
    pub fn open(&mut self, len: usize) -> Result<u32, SendBlocked> {
        let stream = (0..MAX_OPEN_STREAMS)
            .find(|stream| {
                self.busy[*stream].is_none()
                    && self.reserved != u32::try_from(*stream).ok()
            })
            .ok_or(SendBlocked::NoStream)?;
        self.claim(stream, len)
    }

    /// Takes the reserved stream for a message of `len` bytes.
    ///
    /// # Errors
    /// Returns [`SendBlocked`] when the reserved stream is busy or the
    /// message would pass the budget.
    pub fn open_reserved(&mut self, len: usize) -> Result<u32, SendBlocked> {
        let stream = self.reserved.ok_or(SendBlocked::NoStream)? as usize;
        if self.busy[stream].is_some() {
            return Err(SendBlocked::NoStream);
        }
        self.claim(stream, len)
    }

    fn claim(&mut self, stream: usize, len: usize) -> Result<u32, SendBlocked> {
        if self.pending() + len > self.budget {
            return Err(SendBlocked::OverBudget);
        }
        self.busy[stream] = Some(len);
        Ok(u32::try_from(stream).unwrap_or(u32::MAX))
    }

    /// Frees a stream once its message's last frame, or its abort, is
    /// built.
    pub fn release(&mut self, stream: u32) {
        if let Some(slot) = self.busy.get_mut(stream as usize) {
            *slot = None;
        }
    }
}

/// One outgoing message, split into frames one at a time.
#[derive(Debug)]
pub struct OutboundMessage {
    stream: u32,
    bytes: Vec<u8>,
    sent: usize,
    finished: bool,
}

impl OutboundMessage {
    /// A message of `bytes` on `stream`.
    #[must_use]
    pub fn new(stream: u32, bytes: Vec<u8>) -> Self {
        Self {
            stream,
            bytes,
            sent: 0,
            finished: false,
        }
    }

    /// The message's stream.
    #[must_use]
    pub fn stream(&self) -> u32 {
        self.stream
    }

    /// Whether the last frame has been built.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Builds the next frame under `max_frame`, or `None` once the last
    /// frame has been built.
    #[must_use]
    pub fn next_frame(&mut self, max_frame: u32) -> Option<Frame> {
        if self.finished {
            return None;
        }
        let room = Frame::capacity(self.stream, max_frame).max(1);
        let end = (self.sent + room).min(self.bytes.len());
        let payload = self.bytes[self.sent..end].to_vec();
        self.sent = end;
        self.finished = end == self.bytes.len();
        Some(Frame {
            stream: self.stream,
            end: if self.finished {
                FrameEnd::Final
            } else {
                FrameEnd::More
            },
            payload,
        })
    }

    /// Abandons the message: the abort frame to send when at least one
    /// frame went out and the last has not, or `None` when the receiver
    /// holds nothing of it or already has all of it. Aborting a message
    /// whose frames are all built does nothing; it still arrives.
    #[must_use]
    pub fn abort(&mut self) -> Option<Frame> {
        if self.finished || self.sent == 0 {
            self.finished = true;
            return None;
        }
        self.finished = true;
        Some(Frame {
            stream: self.stream,
            end: FrameEnd::Abort,
            payload: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests;
