//! The outbound queue: messages on their ordering lanes, framed lazily as
//! the writer pulls them.
//!
//! A lane holds at most one unfinished message, so its messages complete in
//! the order they started, and lanes take turns frame by frame. The fixed
//! lanes carry control replies and results, large results, and the
//! hierarchy; each viewer has a lane of its own for its result, updates,
//! status, input acknowledgements, request results, and its end.
//!
//! An item becomes bytes in one of two ways. A result or hierarchy message
//! is encoded once, when a producer admits it, into a buffer the queue and
//! the writer share; frames are cut from it in place, so no payload is
//! copied before the transport writes it. An update, a status, an input
//! acknowledgement, and a large text result are encoded only when their
//! message starts: an update and a text one frame at a time, straight from
//! the snapshot or the text, and a status or an acknowledgement whole.
//!
//! The queue counts the bytes it retains: every admitted encoded message,
//! whole, until the transport has accepted its last frame, and every frame
//! of a streamed or late-encoded message from when it is cut until it is
//! written.

use std::collections::{BTreeMap, VecDeque};
use std::io::IoSlice;
use std::sync::Arc;

use huterm_protocol::{TerminalStatus, ViewerId};
use huterm_wire::caps::{MessageCaps, MessageKind};
use huterm_wire::codec::{EncodeError, IdScope, Writer};
use huterm_wire::frame::Frame;
use huterm_wire::status::encode_status;
use huterm_wire::stream::{HOST_EFFECT_STREAM, OutboundStreams};
use huterm_wire::update::{PlannedUpdate, UpdateFrames};
use huterm_wire::viewer::{InputAck, encode_input_ack};

/// The frame flag that ends a message, as `huterm-wire` frames it.
const FINAL: u8 = 0b01;

/// A frame header's most bytes: the length, the flags, and a `u32`
/// stream varint.
const HEADER_BYTES: usize = 4 + 1 + 5;

/// A server ordering lane.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Lane {
    /// Handshake replies and results of requests that name no viewer.
    Control,
    /// Large results, such as selection text.
    Large,
    /// Hierarchy snapshots and events.
    Hierarchy,
    /// One viewer's messages.
    Viewer(ViewerId),
}

/// Something a lane sends.
#[derive(Debug)]
pub(crate) enum Item {
    /// A message encoded when it was admitted, and the frame limit that
    /// takes effect once its last frame is cut, as after `Welcome`.
    Encoded(Arc<Vec<u8>>, Option<u32>),
    /// A viewer's update, encoded frame by frame when it starts.
    Update(PlannedUpdate),
    /// A viewer's whole status, encoded when it starts.
    Status(ViewerId, Arc<TerminalStatus>),
    /// A viewer's highest admitted input, encoded when it starts.
    Ack(ViewerId, u64),
    /// A large result split around its text, which streams from the
    /// text's own buffer.
    Text {
        /// The bytes before the text.
        head: Vec<u8>,
        /// The text.
        text: String,
        /// The bytes after it.
        tail: Vec<u8>,
    },
}

impl Item {
    /// The bytes the queue counts when the item is admitted: an encoded
    /// message's whole length, and nothing for an item encoded later.
    fn admitted(&self) -> usize {
        match self {
            Self::Encoded(bytes, _) => bytes.len(),
            _ => 0,
        }
    }
}

/// A started message's bytes.
#[derive(Debug)]
enum Body {
    /// Shared encoded bytes, cut in place.
    Shared(Arc<Vec<u8>>),
    /// An update encoded one frame at a time.
    Update(Box<UpdateFrames>),
    /// A text result: its head, its text, and its tail, in order.
    Text {
        head: Vec<u8>,
        text: String,
        tail: Vec<u8>,
    },
}

#[derive(Debug)]
struct Current {
    body: Body,
    len: usize,
    stream: u32,
    cut: usize,
    then_limit: Option<u32>,
    /// Whether the bytes were counted when the message was admitted, so
    /// they are released when its last frame is written.
    admitted: bool,
}

impl Current {
    /// The message's next `len` bytes: a range of its shared buffer, or a
    /// buffer of their own for a message encoded as it goes.
    fn fill(
        &mut self,
        len: usize,
    ) -> Result<(Arc<Vec<u8>>, std::ops::Range<usize>), String> {
        let start = self.cut;
        let out = match &mut self.body {
            Body::Shared(bytes) => {
                return Ok((Arc::clone(bytes), start..start + len));
            }
            Body::Update(frames) => {
                let mut out = Vec::with_capacity(len);
                frames
                    .fill(&mut out, len)
                    .map_err(|error| error.to_string())?;
                out
            }
            Body::Text { head, text, tail } => {
                let mut out = Vec::with_capacity(len);
                let mut at = start;
                for part in [head.as_slice(), text.as_bytes(), tail.as_slice()]
                {
                    if out.len() == len {
                        break;
                    }
                    if at >= part.len() {
                        at -= part.len();
                        continue;
                    }
                    let take = (part.len() - at).min(len - out.len());
                    out.extend_from_slice(&part[at..at + take]);
                    at = 0;
                }
                out
            }
        };
        if out.len() != len {
            return Err("a streamed message ended early".into());
        }
        Ok((Arc::new(out), 0..len))
    }
}

#[derive(Debug, Default)]
struct LaneQueue {
    queue: VecDeque<Item>,
    current: Option<Current>,
    retained: usize,
    /// The lane takes no new items and goes away once empty.
    closing: bool,
}

impl LaneQueue {
    fn has_work(&self) -> bool {
        self.current.is_some() || !self.queue.is_empty()
    }

    /// Whether the lane's next frame waits: a message encoded as it goes
    /// produces frames only under the budget, so retained bytes pass it by
    /// at most one frame; and a frame other than a batch's `first` waits
    /// for the next batch unless it fits `space` whole.
    fn waits(&self, first: bool, space: usize, limit: u32, over: bool) -> bool {
        let streamed = match &self.current {
            Some(current) => !current.admitted,
            None => !matches!(self.queue.front(), Some(Item::Encoded(..))),
        };
        if streamed && over {
            return true;
        }
        if first {
            return false;
        }
        let remaining = match &self.current {
            Some(current) => current.len - current.cut,
            None => match self.queue.front() {
                Some(Item::Encoded(bytes, _)) => bytes.len(),
                // A late-encoded item's length is unknown until it starts;
                // its first frame takes what room there is.
                _ => 0,
            },
        };
        let max = usize::try_from(limit).unwrap_or(usize::MAX);
        remaining.min(max) + HEADER_BYTES > space
    }
}

/// A frame handed to the writer and not yet fully written.
#[derive(Debug)]
struct Handed {
    len: usize,
    /// Bytes the queue stops retaining once this frame is written.
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

/// A frame's header: its length, its flags, and its stream varint.
fn frame_header(
    stream: u32,
    body: usize,
    last: bool,
) -> ([u8; HEADER_BYTES], u8) {
    let mut header = [0; HEADER_BYTES];
    let body = u32::try_from(body).unwrap_or(u32::MAX);
    header[..4].copy_from_slice(&body.to_le_bytes());
    header[4] = if last { FINAL } else { 0 };
    let mut len = 5;
    let mut value = stream;
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            header[len] = byte;
            len += 1;
            break;
        }
        header[len] = byte | 0x80;
        len += 1;
    }
    (header, u8::try_from(len).unwrap_or(u8::MAX))
}

/// What a lane's turn produced.
enum Turn {
    /// A frame.
    Cut(Chunk),
    /// Nothing now: no stream is free, or the frame waits for the next
    /// batch.
    Wait,
    /// The lane has nothing to send.
    Empty,
}

#[derive(Debug)]
pub(crate) struct Outbound {
    raw: Option<Arc<Vec<u8>>>,
    control: LaneQueue,
    large: LaneQueue,
    hierarchy: LaneQueue,
    viewers: BTreeMap<ViewerId, LaneQueue>,
    /// Lanes that may have work, in turn order.
    ring: VecDeque<Lane>,
    streams: OutboundStreams,
    limit: u32,
    retained: usize,
    handed: VecDeque<Handed>,
    written_into_front: usize,
    /// How late-encoded items encode, once the handshake settles it.
    encoding: Option<(IdScope, MessageCaps)>,
    /// Why the queue could not encode its own message, which ends the
    /// connection.
    fault: Option<String>,
    /// The retained bytes at which streamed messages stop producing frames
    /// until the writer catches up.
    budget: usize,
}

impl Outbound {
    /// A queue that sends `raw` first, unframed, and then frames under
    /// `limit`.
    pub(crate) fn new(raw: Vec<u8>, limit: u32, budget: usize) -> Self {
        let retained = raw.len();
        Self {
            raw: Some(Arc::new(raw)),
            control: LaneQueue::default(),
            large: LaneQueue::default(),
            hierarchy: LaneQueue::default(),
            viewers: BTreeMap::new(),
            ring: VecDeque::new(),
            // The client holds whatever its caps allow on each stream; the
            // host effect stream stays reserved for slice 4.
            streams: OutboundStreams::new(usize::MAX, Some(HOST_EFFECT_STREAM)),
            limit,
            retained,
            handed: VecDeque::new(),
            written_into_front: 0,
            encoding: None,
            fault: None,
            budget,
        }
    }

    /// Sets how items encoded at their start encode.
    pub(crate) fn set_encoding(&mut self, scope: IdScope, caps: MessageCaps) {
        self.encoding = Some((scope, caps));
    }

    /// Why the queue failed to encode a message, if it did.
    pub(crate) fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }

    /// The bytes every admitted message not yet fully written holds.
    pub(crate) fn retained(&self) -> usize {
        self.retained
    }

    /// Whether anything waits to be written: bytes the queue retains, or an
    /// item not yet encoded.
    pub(crate) fn pending(&self) -> bool {
        self.retained > 0
            || self.raw.is_some()
            || [&self.control, &self.large, &self.hierarchy]
                .into_iter()
                .chain(self.viewers.values())
                .any(LaneQueue::has_work)
    }

    /// The bytes `lane`'s messages hold.
    pub(crate) fn lane_retained(&self, lane: Lane) -> usize {
        self.queue(lane).map_or(0, |queue| queue.retained)
    }

    fn queue(&self, lane: Lane) -> Option<&LaneQueue> {
        match lane {
            Lane::Control => Some(&self.control),
            Lane::Large => Some(&self.large),
            Lane::Hierarchy => Some(&self.hierarchy),
            Lane::Viewer(viewer) => self.viewers.get(&viewer),
        }
    }

    fn queue_mut(&mut self, lane: Lane) -> Option<&mut LaneQueue> {
        match lane {
            Lane::Control => Some(&mut self.control),
            Lane::Large => Some(&mut self.large),
            Lane::Hierarchy => Some(&mut self.hierarchy),
            Lane::Viewer(viewer) => self.viewers.get_mut(&viewer),
        }
    }

    /// Whether a viewer's lane takes new items.
    pub(crate) fn viewer_open(&self, viewer: ViewerId) -> bool {
        self.viewers
            .get(&viewer)
            .is_some_and(|queue| !queue.closing)
    }

    /// Opens a viewer's lane.
    pub(crate) fn open_viewer(&mut self, viewer: ViewerId) {
        self.viewers.entry(viewer).or_default();
    }

    /// Stops a viewer's lane: its queued items are dropped, an unfinished
    /// message still finishes, and the lane goes away once empty.
    pub(crate) fn close_viewer(&mut self, viewer: ViewerId) {
        let Some(queue) = self.viewers.get_mut(&viewer) else {
            return;
        };
        let dropped: usize = queue.queue.iter().map(Item::admitted).sum();
        queue.queue.clear();
        queue.retained -= dropped;
        self.retained -= dropped;
        queue.closing = true;
        if queue.current.is_none() {
            self.viewers.remove(&viewer);
        }
    }

    /// Admits `item` to the end of `lane`. A closed viewer's lane drops it.
    pub(crate) fn push(&mut self, lane: Lane, item: Item) {
        let admitted = item.admitted();
        let Some(queue) = self.queue_mut(lane) else {
            return;
        };
        if queue.closing {
            return;
        }
        queue.queue.push_back(item);
        queue.retained += admitted;
        self.retained += admitted;
        self.ring.push_back(lane);
        self.dedupe_ring();
    }

    /// Admits an encoded message to `lane`, switching to `then_limit` once
    /// its last frame is cut.
    pub(crate) fn push_encoded(
        &mut self,
        lane: Lane,
        bytes: Vec<u8>,
        then_limit: Option<u32>,
    ) {
        self.push(lane, Item::Encoded(Arc::new(bytes), then_limit));
    }

    /// Puts a viewer's status ahead of every update not yet started:
    /// replacing an unsent status where it stands, or going in before the
    /// first queued update.
    pub(crate) fn set_status(
        &mut self,
        viewer: ViewerId,
        status: Arc<TerminalStatus>,
    ) {
        self.slot(viewer, Item::Status(viewer, status), |item, new| {
            if let (Item::Status(_, old), Item::Status(_, new)) = (item, new) {
                *old = Arc::clone(new);
                true
            } else {
                false
            }
        });
    }

    /// Raises a viewer's unsent input acknowledgement to `seq`, or puts one
    /// ahead of every update not yet started.
    pub(crate) fn set_ack(&mut self, viewer: ViewerId, seq: u64) {
        self.slot(viewer, Item::Ack(viewer, seq), |item, new| {
            if let (Item::Ack(_, old), Item::Ack(_, new)) = (item, new) {
                *old = (*old).max(*new);
                true
            } else {
                false
            }
        });
    }

    fn slot(
        &mut self,
        viewer: ViewerId,
        item: Item,
        replace: impl Fn(&mut Item, &Item) -> bool,
    ) {
        let Some(queue) = self.viewers.get_mut(&viewer) else {
            return;
        };
        if queue.closing {
            return;
        }
        for queued in &mut queue.queue {
            if replace(queued, &item) {
                return;
            }
        }
        let at = queue
            .queue
            .iter()
            .position(|queued| matches!(queued, Item::Update(_)))
            .unwrap_or(queue.queue.len());
        queue.queue.insert(at, item);
        self.ring.push_back(Lane::Viewer(viewer));
        self.dedupe_ring();
    }

    /// Cuts frames, lanes taking turns, until about `room` bytes. Only the
    /// first frame is cut short to fit; a later frame that does not fit
    /// whole waits for the next call, so a message that fits one frame is
    /// never split across two.
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
        let mut first = true;
        let mut idle = 0;
        while transmit.len() < room && idle < self.ring.len() {
            let Some(lane) = self.ring.pop_front() else {
                break;
            };
            // At least one byte of payload fits, so a frame always goes
            // out when one is ready.
            let space = (room - transmit.len()).max(HEADER_BYTES + 1);
            match self.cut(lane, space, first) {
                Turn::Cut(chunk) => {
                    idle = 0;
                    first = false;
                    transmit.push(chunk);
                    self.ring.push_back(lane);
                }
                Turn::Wait => {
                    idle += 1;
                    self.ring.push_back(lane);
                }
                Turn::Empty => {}
            }
            if self.fault.is_some() {
                break;
            }
        }
        self.dedupe_ring();
        (!transmit.is_empty()).then_some(transmit)
    }

    /// Drops repeated entries from the turn ring, keeping each lane's
    /// first, so pushes never grow it past the lanes with work.
    fn dedupe_ring(&mut self) {
        if self.ring.len() <= 2 * (3 + self.viewers.len()) {
            return;
        }
        let mut seen = std::collections::HashSet::new();
        self.ring.retain(|lane| seen.insert(*lane));
    }

    /// Starts `item`'s message on a free stream.
    fn start(
        streams: &mut OutboundStreams,
        encoding: Option<(IdScope, MessageCaps)>,
        item: Item,
    ) -> Result<Result<Current, Item>, EncodeError> {
        let len = match &item {
            Item::Encoded(bytes, _) => bytes.len(),
            Item::Text { head, text, tail } => {
                head.len() + text.len() + tail.len()
            }
            _ => 0,
        };
        // The client takes whatever its caps allow on each stream, so a
        // stream is claimed before a late-encoded item is sized.
        let Ok(stream) = streams.open(len) else {
            return Ok(Err(item));
        };
        let encoding = || {
            encoding.ok_or(EncodeError::Invalid(
                "a late-encoded item before the handshake",
            ))
        };
        let current = match item {
            Item::Encoded(bytes, then_limit) => Current {
                len: bytes.len(),
                body: Body::Shared(bytes),
                stream,
                cut: 0,
                then_limit,
                admitted: true,
            },
            Item::Update(planned) => {
                let (scope, _) = encoding()?;
                let frames = planned.frames(scope)?;
                Current {
                    len: frames.len(),
                    body: Body::Update(Box::new(frames)),
                    stream,
                    cut: 0,
                    then_limit: None,
                    admitted: false,
                }
            }
            Item::Status(viewer, status) => {
                let (scope, caps) = encoding()?;
                let mut w = Writer::new();
                w.u64(MessageKind::ViewerStatus.number());
                w.record(|w| {
                    w.id(scope, viewer)?;
                    encode_status(w, &status)
                })?;
                let cap = caps.cap(MessageKind::ViewerStatus).unwrap_or(0);
                if w.len() > cap {
                    return Err(EncodeError::LimitExceeded { field: "status" });
                }
                let bytes = w.into_bytes();
                Current {
                    len: bytes.len(),
                    body: Body::Shared(Arc::new(bytes)),
                    stream,
                    cut: 0,
                    then_limit: None,
                    admitted: false,
                }
            }
            Item::Ack(viewer, seq) => {
                let (scope, _) = encoding()?;
                let mut w = Writer::new();
                w.u64(MessageKind::InputAck.number());
                encode_input_ack(&mut w, scope, InputAck { viewer, seq })?;
                let bytes = w.into_bytes();
                Current {
                    len: bytes.len(),
                    body: Body::Shared(Arc::new(bytes)),
                    stream,
                    cut: 0,
                    then_limit: None,
                    admitted: false,
                }
            }
            Item::Text { head, text, tail } => Current {
                len,
                body: Body::Text { head, text, tail },
                stream,
                cut: 0,
                then_limit: None,
                admitted: false,
            },
        };
        Ok(Ok(current))
    }

    /// Cuts `lane`'s next frame within `space`, or, unless it is the
    /// `first` frame of its transmit, nothing when the whole frame does
    /// not fit.
    fn cut(&mut self, lane: Lane, space: usize, first: bool) -> Turn {
        let limit = self.limit;
        let encoding = self.encoding;
        let Self {
            control,
            large,
            hierarchy,
            viewers,
            streams,
            ..
        } = self;
        let queue = match lane {
            Lane::Control => control,
            Lane::Large => large,
            Lane::Hierarchy => hierarchy,
            Lane::Viewer(viewer) => match viewers.get_mut(&viewer) {
                Some(queue) => queue,
                None => return Turn::Empty,
            },
        };
        if !queue.has_work() {
            return Turn::Empty;
        }
        if queue.waits(first, space, limit, self.retained >= self.budget) {
            return Turn::Wait;
        }
        if queue.current.is_none() {
            let Some(item) = queue.queue.pop_front() else {
                return Turn::Empty;
            };
            match Self::start(streams, encoding, item) {
                Ok(Ok(current)) => queue.current = Some(current),
                Ok(Err(item)) => {
                    queue.queue.push_front(item);
                    return Turn::Wait;
                }
                Err(error) => {
                    self.fault = Some(error.to_string());
                    return Turn::Empty;
                }
            }
        }
        let Some(current) = queue.current.as_mut() else {
            return Turn::Empty;
        };
        let stream = current.stream;
        let total = current.len;
        let empty = Frame::body_len(stream, 0);
        let payload_len = Frame::capacity(stream, limit)
            .min(total - current.cut)
            .min(space.saturating_sub(4 + empty))
            .max(1);
        let (payload, range) = match current.fill(payload_len) {
            Ok(filled) => filled,
            Err(error) => {
                self.fault = Some(error);
                return Turn::Empty;
            }
        };
        current.cut += payload_len;
        let last = current.cut == total;
        let (header, header_len) =
            frame_header(stream, empty + payload_len, last);
        let chunk = Chunk {
            header,
            header_len,
            payload,
            start: range.start,
            end: range.end,
        };
        // A streamed frame counts from its cut until it is written; an
        // admitted message counts whole until its last frame is written.
        let release = if current.admitted {
            if last { total } else { 0 }
        } else {
            queue.retained += payload_len;
            self.retained += payload_len;
            payload_len
        };
        self.handed.push_back(Handed {
            len: chunk.len(),
            release,
            lane: Some(lane),
        });
        if last {
            let finished = queue.current.take();
            self.streams.release(stream);
            if let Some(limit) =
                finished.and_then(|finished| finished.then_limit)
            {
                self.limit = limit;
            }
        }
        Turn::Cut(chunk)
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
                let remove = match self.queue_mut(lane) {
                    Some(queue) => {
                        queue.retained -= done.release;
                        queue.closing && !queue.has_work()
                    }
                    None => false,
                };
                if let (true, Lane::Viewer(viewer)) = (remove, lane) {
                    self.viewers.remove(&viewer);
                }
            }
        }
    }

    /// Drops everything, as a closed connection does.
    pub(crate) fn clear(&mut self) {
        self.raw = None;
        self.control = LaneQueue::default();
        self.large = LaneQueue::default();
        self.hierarchy = LaneQueue::default();
        self.viewers.clear();
        self.ring.clear();
        self.handed.clear();
        self.written_into_front = 0;
        self.retained = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames the writer would send, as each frame's message bytes.
    fn payloads(transmit: &Transmit) -> Vec<&[u8]> {
        transmit.chunks.iter().map(Chunk::payload).collect()
    }

    #[test]
    fn a_message_that_fits_one_frame_waits_for_the_next_batch_instead_of_splitting()
     {
        let mut outbound = Outbound::new(Vec::new(), 1 << 16, usize::MAX);
        let first = vec![1; 100];
        let second = vec![2; 100];
        outbound.push_encoded(Lane::Control, first.clone(), None);
        outbound.push_encoded(Lane::Control, second.clone(), None);
        let room = first.len() + HEADER_BYTES + second.len() / 2;
        let batch = outbound.take(room).unwrap();
        assert_eq!(payloads(&batch), [&[][..], &first[..]]);
        outbound.written(batch.len());
        let batch = outbound.take(room).unwrap();
        assert_eq!(payloads(&batch), [&second[..]]);
    }
}
