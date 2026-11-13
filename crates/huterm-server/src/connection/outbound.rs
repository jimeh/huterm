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
//!
//! Host effects travel apart from the lanes, on the stream the server keeps
//! for them, one effect at a time, with at most one of its frames with the
//! writer at once: a slot of its own, outside the budget. An effect's first
//! frame goes ahead of every lane's, so a busy connection delays it by at
//! most what the writer already holds: one batch, which the native adapter
//! keeps to 256 KiB, a few milliseconds at the transport profile's minimum
//! throughput. Its later frames lead every other batch while lanes have
//! work, and every batch otherwise. The text streams from the effect
//! itself, which the queue drops, releasing its reservations, once its last
//! frame is cut or the effect is aborted. Between effects, the slot yields:
//! the next effect starts only once every lane that held a partly sent
//! message when the last one ended has cut a frame, at most one per
//! ordinary stream, so effects back to back never leave a started message
//! waiting; one effect holds the link at most until its completion
//! deadline.

use std::collections::{BTreeMap, VecDeque};
use std::io::IoSlice;
use std::sync::Arc;

use huterm_protocol::{TerminalStatus, ViewerId};
use huterm_wire::caps::{MessageCaps, MessageKind};
use huterm_wire::codec::{EncodeError, IdScope, Writer};
use huterm_wire::frame::Frame;
use huterm_wire::frame::FrameEnd;
use huterm_wire::status::encode_status;
use huterm_wire::stream::{HOST_EFFECT_STREAM, OutboundStreams};
use huterm_wire::update::{PlannedUpdate, UpdateFrames};
use huterm_wire::viewer::{InputAck, encode_input_ack};

use crate::viewer::PendingEffect;

/// The frame flag that ends a message, as `huterm-wire` frames it.
const FINAL: u8 = 0b01;

/// The frame flag that abandons a message, as `huterm-wire` frames it.
const ABORT: u8 = 0b10;

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
    /// Whether the message's bytes count whole until its last frame is
    /// written, rather than frame by frame from each cut.
    admitted: bool,
    /// Whether those whole bytes are already counted: from admission for
    /// an encoded message, or not yet for a status or acknowledgement
    /// encoded as it starts.
    counted: bool,
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
    /// A message on the lane was written whole: for a viewer's lane, the
    /// subscription result that opens it.
    announced: bool,
}

impl LaneQueue {
    fn has_work(&self) -> bool {
        self.current.is_some() || !self.queue.is_empty()
    }

    /// Whether the lane's next frame waits: over the budget, a message
    /// encoded as it goes starts only when nothing else drains, and one
    /// already started cuts a frame only while the writer holds none; and a
    /// frame other than a batch's `first` waits for the next batch unless
    /// it fits `space` whole.
    fn waits(&self, first: bool, space: usize, limit: u32, over: Over) -> bool {
        // A started message holds a stream that messages admitted earlier
        // may need, so it keeps going over the budget, a frame at a time.
        let late = self.current.is_none()
            && !matches!(self.queue.front(), Some(Item::Encoded(..)));
        let streaming = self
            .current
            .as_ref()
            .is_some_and(|current| !current.admitted);
        let wait = match over {
            Over::Under | Over::Stuck => false,
            Over::Writing => streaming || late,
            Over::Started => late,
        };
        if wait {
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

/// Where the queue stands against its budget, which decides what may cut
/// a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Over {
    Under,
    /// Over, with frames at the writer: only messages whose bytes already
    /// count whole cut frames until it writes them.
    Writing,
    /// Over, with nothing at the writer and a message started: started
    /// messages cut a frame.
    Started,
    /// Over, with nothing at the writer and nothing started: everything
    /// retained waits behind messages encoded as they go, so one starts.
    Stuck,
}

/// A frame handed to the writer and not yet fully written.
#[derive(Debug)]
struct Handed {
    len: usize,
    /// Bytes the queue stops retaining once this frame is written.
    release: usize,
    lane: Option<Lane>,
    /// A host effect's frame, which frees the effect's slot once written.
    effect: Option<EffectFrame>,
}

/// What a host effect's frame was.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EffectFrame {
    /// A frame of the effect's message; `last` ends it.
    Message { first: bool, last: bool },
    /// The frame that abandons it.
    Abort,
}

/// The host effect in the effect slot.
#[derive(Debug)]
struct EffectMessage {
    head: Vec<u8>,
    /// The effect, whose text streams after the head, until its last
    /// frame is cut.
    source: Option<Box<dyn PendingEffect>>,
    len: usize,
    cut: usize,
    stream: u32,
    first_written: bool,
}

impl EffectMessage {
    /// The message's next `len` bytes, copied from the head and the
    /// effect's text.
    fn fill(&self, len: usize) -> Result<Vec<u8>, String> {
        let text = self
            .source
            .as_ref()
            .and_then(|source| source.payload())
            .map_or("", |payload| payload.streamed());
        let mut out = Vec::with_capacity(len);
        let mut at = self.cut;
        for part in [self.head.as_slice(), text.as_bytes()] {
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
        if out.len() == len {
            Ok(out)
        } else {
            Err("a host effect ended early".into())
        }
    }
}

/// Where the effect slot stands, for the connection's deadlines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EffectProgress {
    /// No effect holds the slot: the next may start.
    Idle,
    /// An effect's message is under way.
    Sending {
        /// Its first frame was written.
        first_written: bool,
    },
    /// An abandoned effect's last frame or abort is still with the writer.
    Draining,
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
    end: FrameEnd,
) -> ([u8; HEADER_BYTES], u8) {
    let mut header = [0; HEADER_BYTES];
    let body = u32::try_from(body).unwrap_or(u32::MAX);
    header[..4].copy_from_slice(&body.to_le_bytes());
    header[4] = match end {
        FrameEnd::More => 0,
        FrameEnd::Final => FINAL,
        FrameEnd::Abort => ABORT,
    };
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

/// How a message's frame ends: its last frame ends the message.
const fn frame_end(last: bool) -> FrameEnd {
    if last {
        FrameEnd::Final
    } else {
        FrameEnd::More
    }
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
    /// The host effect being sent, if any.
    effect: Option<EffectMessage>,
    /// An abandoned effect's abort frame, waiting for the slot.
    effect_abort: Option<u32>,
    /// The bytes of the effect frame with the writer, outside the budget.
    effect_handed: usize,
    /// Lanes that held a partly sent message when the last effect ended,
    /// and have not cut a frame since; the next effect waits for them.
    effect_owed: Vec<Lane>,
    /// The last batch opened with one of a streaming effect's later frames,
    /// so the next opens with the lanes'.
    effect_led: bool,
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
            effect: None,
            effect_abort: None,
            effect_handed: 0,
            effect_owed: Vec::new(),
            effect_led: false,
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

    /// The bytes of the effect frame with the writer, which the slot holds
    /// outside the budget.
    pub(crate) fn effect_retained(&self) -> usize {
        self.effect_handed
    }

    /// Where the effect slot stands.
    pub(crate) fn effect_progress(&self) -> EffectProgress {
        match &self.effect {
            Some(effect) => EffectProgress::Sending {
                first_written: effect.first_written,
            },
            None if self.effect_abort.is_some()
                || self.effect_handed > 0
                || self.effect_owed.iter().any(|lane| {
                    self.queue(*lane).is_some_and(LaneQueue::has_work)
                }) =>
            {
                EffectProgress::Draining
            }
            None => EffectProgress::Idle,
        }
    }

    /// Starts sending `source` as a `HostEffect` message whose bytes before
    /// its streamed text are `head`. The slot must be idle.
    pub(crate) fn start_effect(
        &mut self,
        head: Vec<u8>,
        source: Box<dyn PendingEffect>,
    ) {
        let text = source
            .payload()
            .map_or(0, |payload| payload.streamed().len());
        let len = head.len() + text;
        let Ok(stream) = self.streams.open_reserved(len) else {
            self.fault = Some("the host effect stream was busy".into());
            return;
        };
        self.effect = Some(EffectMessage {
            head,
            source: Some(source),
            len,
            cut: 0,
            stream,
            first_written: false,
        });
    }

    /// Ends the effect's turn: every lane holding a partly sent message
    /// cuts a frame before the next effect starts. Only started messages
    /// count, which hold at most the ordinary streams, so the wait stays a
    /// few frames even with hundreds of viewers.
    fn yield_effect_turn(&mut self) {
        self.effect_owed = [Lane::Control, Lane::Large, Lane::Hierarchy]
            .into_iter()
            .chain(self.viewers.keys().copied().map(Lane::Viewer))
            .filter(|lane| {
                self.queue(*lane)
                    .is_some_and(|queue| queue.current.is_some())
            })
            .collect();
    }

    /// Abandons the effect being sent, dropping it and its reservations at
    /// once. A message not yet begun leaves nothing; one partly cut ends
    /// with an abort frame after the frame already with the writer; one
    /// whose frames are all cut still arrives whole. Returns whether an
    /// effect was abandoned before its last frame was cut.
    pub(crate) fn abort_effect(&mut self) -> bool {
        let Some(effect) = self.effect.take() else {
            return false;
        };
        if effect.cut == effect.len {
            // Its last frame is with the writer, and its source is gone.
            return false;
        }
        if effect.cut > 0 {
            self.effect_abort = Some(effect.stream);
        } else {
            self.streams.release(effect.stream);
        }
        true
    }

    /// Whether anything waits to be written: bytes the queue retains, or an
    /// item not yet encoded.
    pub(crate) fn pending(&self) -> bool {
        self.retained > 0
            || self.effect.is_some()
            || self.effect_abort.is_some()
            || self.effect_handed > 0
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

    /// Whether a viewer's subscription result, the first message on its
    /// lane, was written whole, so the client knows the viewer.
    pub(crate) fn viewer_announced(&self, viewer: ViewerId) -> bool {
        self.viewers
            .get(&viewer)
            .is_some_and(|queue| queue.announced)
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

    /// Stops a viewer's lane: it takes no new items, what it holds still
    /// goes out unless `views` is false, which drops updates, statuses, and
    /// acknowledgements not yet started, and the lane goes away once
    /// drained.
    pub(crate) fn close_viewer(&mut self, viewer: ViewerId, views: bool) {
        let Some(queue) = self.viewers.get_mut(&viewer) else {
            return;
        };
        queue.closing = true;
        // A viewer the client closed has no use for views not yet started;
        // everything else queued, its end notice and results included,
        // still goes out. Unstarted views hold no admitted bytes.
        if !views {
            queue.queue.retain(|item| matches!(item, Item::Encoded(..)));
        }
        if !queue.has_work() && queue.retained == 0 {
            self.viewers.remove(&viewer);
        }
    }

    /// Whether a large result is queued or not yet fully written, so
    /// another may not start.
    pub(crate) fn large_busy(&self) -> bool {
        self.large.has_work() || self.large.retained > 0
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
                effect: None,
            });
        }
        let mut first = true;
        // An effect's first frame and its abort go ahead of every lane's.
        // Its later frames lead every other batch while lanes have work,
        // and follow the lanes' frames in between, so a streaming effect
        // never holds back a lane's started message for long.
        let lead = self.effect_handed == 0
            && (self.effect_starting()
                || !self.effect_led
                || !self.lanes_have_work());
        self.effect_led = false;
        if lead && let Some(chunk) = self.cut_effect(room.max(HEADER_BYTES + 1))
        {
            first = false;
            self.effect_led = true;
            transmit.push(chunk);
        }
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
                    self.effect_owed.retain(|owed| *owed != lane);
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
        if !lead
            && self.effect_handed == 0
            && room.saturating_sub(transmit.len()) > HEADER_BYTES
            && let Some(chunk) = self.cut_effect(room - transmit.len())
        {
            transmit.push(chunk);
        }
        self.dedupe_ring();
        (!transmit.is_empty()).then_some(transmit)
    }

    /// Whether the effect slot's next frame starts an effect or abandons
    /// one, which always goes ahead of the lanes.
    fn effect_starting(&self) -> bool {
        self.effect_abort.is_some()
            || self.effect.as_ref().is_some_and(|effect| effect.cut == 0)
    }

    /// Whether any lane has something to send.
    fn lanes_have_work(&self) -> bool {
        [&self.control, &self.large, &self.hierarchy]
            .into_iter()
            .chain(self.viewers.values())
            .any(LaneQueue::has_work)
    }

    /// Cuts the effect slot's next frame within `space`: an abandoned
    /// effect's abort, or the effect's next frame.
    fn cut_effect(&mut self, space: usize) -> Option<Chunk> {
        let limit = self.limit;
        if let Some(stream) = self.effect_abort.take() {
            let body = Frame::body_len(stream, 0);
            let (header, header_len) =
                frame_header(stream, body, FrameEnd::Abort);
            self.streams.release(stream);
            self.yield_effect_turn();
            return Some(self.hand_effect(
                Chunk {
                    header,
                    header_len,
                    payload: Arc::new(Vec::new()),
                    start: 0,
                    end: 0,
                },
                EffectFrame::Abort,
            ));
        }
        let effect = self.effect.as_mut()?;
        if effect.cut == effect.len {
            return None;
        }
        let stream = effect.stream;
        let empty = Frame::body_len(stream, 0);
        let payload_len = Frame::capacity(stream, limit)
            .min(effect.len - effect.cut)
            .min(space.saturating_sub(4 + empty))
            .max(1);
        let bytes = match effect.fill(payload_len) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.fault = Some(error);
                return None;
            }
        };
        let first = effect.cut == 0;
        effect.cut += payload_len;
        let last = effect.cut == effect.len;
        if last {
            // The rest is copied into frames; the effect's reservations go.
            effect.source = None;
            self.streams.release(stream);
            self.yield_effect_turn();
        }
        let (header, header_len) =
            frame_header(stream, empty + payload_len, frame_end(last));
        Some(self.hand_effect(
            Chunk {
                header,
                header_len,
                payload: Arc::new(bytes),
                start: 0,
                end: payload_len,
            },
            EffectFrame::Message { first, last },
        ))
    }

    fn hand_effect(&mut self, chunk: Chunk, frame: EffectFrame) -> Chunk {
        self.effect_handed = chunk.len();
        self.handed.push_back(Handed {
            len: chunk.len(),
            release: 0,
            lane: None,
            effect: Some(frame),
        });
        chunk
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
                counted: true,
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
                    counted: true,
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
                    admitted: true,
                    counted: false,
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
                    admitted: true,
                    counted: false,
                }
            }
            Item::Text { head, text, tail } => Current {
                len,
                body: Body::Text { head, text, tail },
                stream,
                cut: 0,
                then_limit: None,
                admitted: false,
                counted: true,
            },
        };
        Ok(Ok(current))
    }

    fn over(&self) -> Over {
        if self.retained < self.budget {
            Over::Under
        } else if !self.handed.is_empty() {
            Over::Writing
        } else if [&self.control, &self.large, &self.hierarchy]
            .into_iter()
            .chain(self.viewers.values())
            .any(|queue| queue.current.is_some())
        {
            Over::Started
        } else {
            Over::Stuck
        }
    }

    /// Cuts `lane`'s next frame within `space`, or, unless it is the
    /// `first` frame of its transmit, nothing when the whole frame does
    /// not fit.
    fn cut(&mut self, lane: Lane, space: usize, first: bool) -> Turn {
        let limit = self.limit;
        let encoding = self.encoding;
        let over = self.over();
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
        if queue.waits(first, space, limit, over) {
            return Turn::Wait;
        }
        if queue.current.is_none() {
            let Some(item) = queue.queue.pop_front() else {
                return Turn::Empty;
            };
            match Self::start(streams, encoding, item) {
                Ok(Ok(mut current)) => {
                    if !current.counted {
                        current.counted = true;
                        queue.retained += current.len;
                        self.retained += current.len;
                    }
                    queue.current = Some(current);
                }
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
            frame_header(stream, empty + payload_len, frame_end(last));
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
            effect: None,
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
            if let Some(frame) = done.effect {
                self.effect_handed = 0;
                if let EffectFrame::Message { first, last } = frame
                    && let Some(effect) = self.effect.as_mut()
                {
                    effect.first_written |= first;
                    if last {
                        self.effect = None;
                    }
                }
            }
            self.retained -= done.release;
            if let Some(lane) = done.lane {
                let remove = match self.queue_mut(lane) {
                    Some(queue) => {
                        // Only a message's last frame, or a streamed frame
                        // after the lane's first message ended, releases.
                        queue.announced |= done.release > 0;
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
        self.effect = None;
        self.effect_abort = None;
        self.effect_handed = 0;
        self.effect_owed.clear();
        self.effect_led = false;
    }
}

#[cfg(test)]
mod tests {
    use huterm_protocol::RuntimeId;

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

    #[test]
    fn streamed_messages_holding_every_stream_finish_while_encoded_ones_pass_the_budget()
     {
        let budget = 100;
        let mut outbound = Outbound::new(Vec::new(), 1 << 16, budget);
        let runtime = RuntimeId::new(1);
        outbound.set_encoding(IdScope::new(runtime), MessageCaps::for_minor(0));
        let viewers = [
            ViewerId::in_runtime(runtime, 1),
            ViewerId::in_runtime(runtime, 2),
        ];
        let lanes = [
            Lane::Large,
            Lane::Viewer(viewers[0]),
            Lane::Viewer(viewers[1]),
        ];
        for viewer in viewers {
            outbound.open_viewer(viewer);
        }
        for lane in lanes {
            outbound.push(
                lane,
                Item::Text {
                    head: Vec::new(),
                    text: "t".repeat(1000),
                    tail: Vec::new(),
                },
            );
        }
        let mut delivered = 0;
        let mut drain = |outbound: &mut Outbound, room| {
            let Some(batch) = outbound.take(room) else {
                return false;
            };
            delivered +=
                payloads(&batch).iter().map(|p| p.len()).sum::<usize>();
            outbound.written(batch.len());
            true
        };
        // Each text's first frame takes one of the three ordinary streams.
        while !(outbound.large.current.is_some()
            && viewers
                .iter()
                .all(|viewer| outbound.viewers[viewer].current.is_some()))
        {
            assert!(drain(&mut outbound, HEADER_BYTES + 20));
        }
        assert!(outbound.streams.open(0).is_err(), "every stream is held");
        // An encoded result past the budget waits for a stream.
        outbound.push_encoded(Lane::Control, vec![7; 2 * budget], None);
        let mut turns = 0;
        while drain(&mut outbound, 64) {
            turns += 1;
            assert!(turns < 10_000, "the queue stopped moving");
        }
        assert_eq!(delivered, 3 * 1000 + 2 * budget);
        assert_eq!(outbound.retained(), 0);
    }

    #[test]
    fn encoded_messages_over_the_budget_behind_a_streamed_one_still_go_out() {
        let budget = 100;
        // The preamble goes out first, while the rest waits over the budget.
        let mut outbound = Outbound::new(b"hi".to_vec(), 1 << 16, budget);
        let runtime = RuntimeId::new(1);
        outbound.set_encoding(IdScope::new(runtime), MessageCaps::for_minor(0));
        let viewer = ViewerId::in_runtime(runtime, 1);
        outbound.open_viewer(viewer);
        // Every retained byte waits behind a message encoded as it goes.
        outbound.push(
            Lane::Viewer(viewer),
            Item::Text {
                head: Vec::new(),
                text: "t".repeat(10),
                tail: Vec::new(),
            },
        );
        outbound.push_encoded(Lane::Viewer(viewer), vec![7; 2 * budget], None);
        let mut delivered = 0;
        let mut turns = 0;
        while let Some(batch) = outbound.take(64) {
            delivered +=
                payloads(&batch).iter().map(|p| p.len()).sum::<usize>();
            outbound.written(batch.len());
            turns += 1;
            assert!(turns < 10_000, "the queue stopped moving");
        }
        assert_eq!(delivered, 2 + 10 + 2 * budget);
        assert_eq!(outbound.retained(), 0);
    }

    #[test]
    fn a_started_streamed_message_over_the_budget_cuts_a_frame_per_write() {
        let budget = 100;
        let limit = 64 << 10;
        let mut outbound = Outbound::new(b"hi".to_vec(), limit, budget);
        let runtime = RuntimeId::new(1);
        outbound.set_encoding(IdScope::new(runtime), MessageCaps::for_minor(0));
        outbound.push(
            Lane::Large,
            Item::Text {
                head: Vec::new(),
                text: "t".repeat(1 << 20),
                tail: Vec::new(),
            },
        );
        let mut delivered = 0;
        let mut most = 0;
        while let Some(batch) = outbound.take(usize::MAX) {
            most = most.max(outbound.retained());
            delivered +=
                payloads(&batch).iter().map(|p| p.len()).sum::<usize>();
            outbound.written(batch.len());
        }
        assert_eq!(delivered, 2 + (1 << 20));
        let frame = usize::try_from(limit).unwrap();
        assert!(
            most <= budget + frame,
            "{most} retained, over {budget} plus a frame"
        );
    }
}
