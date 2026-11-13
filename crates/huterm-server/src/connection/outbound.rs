//! The outbound queue: encoded messages on their ordering lanes, framed
//! lazily as the writer pulls them.
//!
//! A message is encoded once, when a producer admits it, into a buffer the
//! queue and the writer share. Frames are cut from it in place: the writer
//! gets each frame's small header and a range of the shared buffer, so no
//! payload is copied before the transport writes it. Each lane holds at most
//! one unfinished message, so its messages complete in the order they
//! started; lanes take turns frame by frame.
//!
//! The queue counts the bytes it retains: every admitted message, whole,
//! until the transport has accepted its last frame.

use std::collections::VecDeque;
use std::io::IoSlice;
use std::sync::Arc;

use huterm_wire::frame::Frame;
use huterm_wire::stream::{HOST_EFFECT_STREAM, OutboundStreams};

/// The frame flag that ends a message, as `huterm-wire` frames it.
const FINAL: u8 = 0b01;

/// A frame header's most bytes: the length, the flags, and a `u32`
/// stream varint.
const HEADER_BYTES: usize = 4 + 1 + 5;

/// A server ordering lane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Lane {
    /// Handshake replies and results.
    Control,
    /// Hierarchy snapshots and events.
    Hierarchy,
}

const LANES: usize = 2;

impl Lane {
    const fn index(self) -> usize {
        match self {
            Self::Control => 0,
            Self::Hierarchy => 1,
        }
    }
}

#[derive(Debug)]
struct Queued {
    bytes: Arc<Vec<u8>>,
    /// The frame limit that takes effect once this message's last frame is
    /// cut, as after `Welcome`.
    then_limit: Option<u32>,
}

#[derive(Debug)]
struct Current {
    message: Queued,
    stream: u32,
    cut: usize,
}

#[derive(Debug, Default)]
struct LaneQueue {
    queue: VecDeque<Queued>,
    current: Option<Current>,
    retained: usize,
}

/// A frame handed to the writer and not yet fully written.
#[derive(Debug)]
struct Handed {
    len: usize,
    /// Bytes the queue stops retaining once this frame is written: its
    /// message's length for the message's last frame, else 0.
    release: usize,
    lane: Option<Lane>,
}

/// One frame for the writer: its header and a range of its message's
/// shared buffer.
#[derive(Clone, Debug)]
struct Chunk {
    header: [u8; HEADER_BYTES],
    header_len: u8,
    payload: Arc<Vec<u8>>,
    start: usize,
    end: usize,
}

impl Chunk {
    fn header(&self) -> &[u8] {
        &self.header[..usize::from(self.header_len)]
    }

    fn payload(&self) -> &[u8] {
        &self.payload[self.start..self.end]
    }

    fn len(&self) -> usize {
        usize::from(self.header_len) + self.end - self.start
    }
}

/// Frames for the writer to send in order. The writer reports each write's
/// accepted bytes back to the connection, which releases a message once its
/// last frame is written.
#[derive(Clone, Debug, Default)]
pub struct Transmit {
    chunks: Vec<Chunk>,
    len: usize,
}

impl Transmit {
    /// The bytes to write.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether there is nothing to write.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The bytes after the first `written`, as slices for a vectored
    /// write.
    #[must_use]
    pub fn io_slices(&self, written: usize) -> Vec<IoSlice<'_>> {
        let mut skip = written;
        let mut slices = Vec::with_capacity(self.chunks.len() * 2);
        for chunk in &self.chunks {
            for part in [chunk.header(), chunk.payload()] {
                if skip >= part.len() {
                    skip -= part.len();
                } else {
                    slices.push(IoSlice::new(&part[skip..]));
                    skip = 0;
                }
            }
        }
        slices
    }

    /// Every byte, copied into one buffer.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.len);
        for chunk in &self.chunks {
            bytes.extend_from_slice(chunk.header());
            bytes.extend_from_slice(chunk.payload());
        }
        bytes
    }

    fn push(&mut self, chunk: Chunk) {
        self.len += chunk.len();
        self.chunks.push(chunk);
    }
}

#[derive(Debug)]
pub(crate) struct Outbound {
    raw: Option<Arc<Vec<u8>>>,
    lanes: [LaneQueue; LANES],
    streams: OutboundStreams,
    limit: u32,
    retained: usize,
    handed: VecDeque<Handed>,
    written_into_front: usize,
    cursor: usize,
}

impl Outbound {
    /// A queue that sends `raw` first, unframed, and then frames under
    /// `limit`.
    pub(crate) fn new(raw: Vec<u8>, limit: u32) -> Self {
        let retained = raw.len();
        Self {
            raw: Some(Arc::new(raw)),
            lanes: Default::default(),
            // The client holds whatever its caps allow on each stream; the
            // host effect stream stays reserved for slice 4.
            streams: OutboundStreams::new(usize::MAX, Some(HOST_EFFECT_STREAM)),
            limit,
            retained,
            handed: VecDeque::new(),
            written_into_front: 0,
            cursor: 0,
        }
    }

    /// The bytes every admitted message not yet fully written holds.
    pub(crate) fn retained(&self) -> usize {
        self.retained
    }

    /// The bytes `lane`'s messages hold.
    pub(crate) fn lane_retained(&self, lane: Lane) -> usize {
        self.lanes[lane.index()].retained
    }

    /// Admits an encoded message to `lane`, switching to `then_limit` once
    /// its last frame is cut.
    pub(crate) fn push(
        &mut self,
        lane: Lane,
        bytes: Vec<u8>,
        then_limit: Option<u32>,
    ) {
        let len = bytes.len();
        let queue = &mut self.lanes[lane.index()];
        queue.queue.push_back(Queued {
            bytes: Arc::new(bytes),
            then_limit,
        });
        queue.retained += len;
        self.retained += len;
    }

    /// Cuts frames, lanes taking turns, until about `room` bytes.
    pub(crate) fn take(&mut self, room: usize) -> Option<Transmit> {
        let mut transmit = Transmit::default();
        if let Some(raw) = self.raw.take() {
            let len = raw.len();
            transmit.push(Chunk {
                header: [0; HEADER_BYTES],
                header_len: 0,
                payload: raw,
                start: 0,
                end: len,
            });
            self.handed.push_back(Handed {
                len,
                release: len,
                lane: None,
            });
        }
        let mut idle = 0;
        while idle < LANES && transmit.len() < room {
            let lane = self.cursor;
            self.cursor = (self.cursor + 1) % LANES;
            // At least one byte of payload fits, so a frame always goes
            // out when one is ready.
            let space = (room - transmit.len()).max(HEADER_BYTES + 1);
            match self.cut(lane, space) {
                Some(chunk) => {
                    idle = 0;
                    transmit.push(chunk);
                }
                None => idle += 1,
            }
            if transmit.len() >= room {
                break;
            }
        }
        (!transmit.is_empty()).then_some(transmit)
    }

    fn cut(&mut self, index: usize, space: usize) -> Option<Chunk> {
        let lane = [Lane::Control, Lane::Hierarchy][index];
        let queue = &mut self.lanes[index];
        if queue.current.is_none() {
            let next = queue.queue.front()?;
            let stream = self.streams.open(next.bytes.len()).ok()?;
            let message = queue.queue.pop_front()?;
            queue.current = Some(Current {
                message,
                stream,
                cut: 0,
            });
        }
        let current = queue.current.as_mut()?;
        let stream = current.stream;
        let total = current.message.bytes.len();
        let empty = Frame::body_len(stream, 0);
        let payload = Frame::capacity(stream, self.limit)
            .min(total - current.cut)
            .min(space.saturating_sub(4 + empty))
            .max(1);
        let start = current.cut;
        current.cut += payload;
        let last = current.cut == total;
        let mut header = [0; HEADER_BYTES];
        let body = u32::try_from(empty + payload).unwrap_or(u32::MAX);
        header[..4].copy_from_slice(&body.to_le_bytes());
        header[4] = if last { FINAL } else { 0 };
        let mut header_len = 5;
        let mut value = stream;
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                header[header_len] = byte;
                header_len += 1;
                break;
            }
            header[header_len] = byte | 0x80;
            header_len += 1;
        }
        let chunk = Chunk {
            header,
            header_len: u8::try_from(header_len).unwrap_or(u8::MAX),
            payload: Arc::clone(&current.message.bytes),
            start,
            end: start + payload,
        };
        self.handed.push_back(Handed {
            len: chunk.len(),
            release: if last { total } else { 0 },
            lane: Some(lane),
        });
        if last {
            let finished = queue.current.take()?;
            self.streams.release(stream);
            if let Some(limit) = finished.message.then_limit {
                self.limit = limit;
            }
        }
        Some(chunk)
    }

    /// Records `written` bytes the transport accepted, in the order frames
    /// were handed out, releasing each message whose last frame is written.
    pub(crate) fn written(&mut self, mut written: usize) {
        while written > 0 {
            let Some(front) = self.handed.front() else {
                return;
            };
            let left = front.len - self.written_into_front;
            if written < left {
                self.written_into_front += written;
                return;
            }
            written -= left;
            self.written_into_front = 0;
            let Some(done) = self.handed.pop_front() else {
                return;
            };
            self.retained -= done.release;
            if let Some(lane) = done.lane {
                self.lanes[lane.index()].retained -= done.release;
            }
        }
    }

    /// Drops everything, as a closed connection does.
    pub(crate) fn clear(&mut self) {
        self.raw = None;
        self.lanes = Default::default();
        self.handed.clear();
        self.written_into_front = 0;
        self.retained = 0;
    }
}
