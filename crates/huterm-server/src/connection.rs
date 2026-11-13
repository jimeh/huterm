//! One client connection's protocol as a sans-IO state machine.
//!
//! A [`Connection`] owns the preamble check, the handshake and its
//! deadline, framing and reassembly with the stream idle clock, message
//! decoding, the request table, the outbound queue and its lanes, the
//! hierarchy lane, close tickets, layout history slots, the connection's
//! attachments, and the writer stall deadline. Every call takes `now` from
//! an injected clock, and the connection never touches the `Mux`:
//!
//! - [`Connection::receive`] takes bytes read from the transport; the
//!   adapter stops reading while [`Connection::wants_input`] is false.
//! - [`Connection::poll_work`] hands out [`Work`], which a worker runs with
//!   [`crate::runtime::execute`]; [`Connection::complete`] takes back its
//!   [`Completion`]. A request has started once its work was handed out.
//! - [`Connection::poll_transmit`] cuts frames for the writer, and
//!   [`Connection::transmitted`] reports the bytes the transport accepted.
//! - [`Connection::next_deadline`] and [`Connection::tick`] drive the
//!   handshake, stream idle, and stall deadlines.
//! - [`Connection::pump`] drains the hierarchy feed after it signals
//!   activity.
//! - [`Connection::close`] ends the connection and returns its cleanup
//!   work. Work still running completes into a closed connection, which
//!   releases what it created; [`Connection::is_finished`] says when
//!   nothing remains outstanding.
//!
//! A test or a fault-injection harness drives it with any transport and
//! clock it likes, holding, reordering, or failing work in between.

mod hierarchy;
mod history;
mod outbound;
mod tickets;
mod viewers;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use huterm_protocol::{
    AttachmentId, DigestProfile, RuntimeId, StreamId, TerminalInput, ViewerId,
};
use huterm_wire::caps::{
    Direction, MessageCaps, MessageKind, reassembly_budget,
};
use huterm_wire::codec::IdScope;
use huterm_wire::command::DecodedInvocation;
use huterm_wire::digest_profile;
use huterm_wire::error::{ErrorCode, WireError};
use huterm_wire::frame::{
    FrameDecoder, HANDSHAKE_FRAME_LIMIT, PREAMBLE_LEN, check_preamble, preamble,
};
use huterm_wire::handshake::{
    HANDSHAKE_DEADLINE, ServerOffer, WRITER_STALL_DEADLINE, negotiate,
};
use huterm_wire::hierarchy::HierarchyContext;
use huterm_wire::message::{
    DecodeContext, Message, decode_message, encode_message,
};
use huterm_wire::request::{
    Cancel, CloseRequest, Request, RequestId, RequestMessage,
};
use huterm_wire::response::{Committed, RequestResult, Response, Success};
use huterm_wire::stream::Reassembler;

use self::hierarchy::HierarchyLane;
use self::history::{History, SlotRef};
pub use self::outbound::Transmit;
use self::outbound::{Item, Lane, Outbound};
use self::tickets::Tickets;
use self::viewers::{Ended, Task, Viewer};
use crate::config::{REQUEST_ID_WINDOW, ServerConfig};
use crate::convert;
use crate::feed::{FeedEncoding, FeedError, HierarchyFeed};
use crate::server::{Counters, Live, Record};
use crate::viewer::WakeSource;
pub use crate::work::{Completion, Job, Work, WorkId};
use crate::work::{Consent, Operation, ReplaceJob, RequestJob};

/// Why a connection ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CloseReason {
    /// The transport broke, or the adapter closed it.
    Disconnected,
    /// The peer broke the protocol.
    Violation(String),
    /// No `Hello` arrived within the handshake deadline.
    HandshakeTimeout,
    /// The server answered `Reject` and sent it.
    Rejected,
    /// The client read nothing, or stayed over the outbound budget, for
    /// the writer stall deadline.
    Stalled,
    /// The server failed to encode its own message, which breaks a core
    /// invariant.
    Fault(String),
}

/// Where a request stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    /// Received, not yet started; a cancel still stops it.
    Queued,
    /// Waiting for a hierarchy replacement not yet issued.
    Waiting,
    /// Its work was handed out, or it waits for a replacement in flight.
    Started,
}

/// What a handed-out work item answers.
#[derive(Debug)]
enum Pending {
    Request(RequestId, Kind),
    Replace,
    Cleanup,
    /// A viewer job: its viewer and what it answers.
    Viewer(ViewerId, Task),
}

/// A result waiting for room under the outbound budget, with the lane it
/// travels on and the viewer whose lane it opens.
#[derive(Debug)]
struct Queued {
    lane: Lane,
    opens: Option<ViewerId>,
    result: RequestResult,
}

#[derive(Debug)]
enum Kind {
    Plain,
    Prepare(u32),
    Commit(u32),
    Ending,
    Attach,
    Detach(AttachmentId),
    Rearrange(SlotRef),
    Restore(SlotRef, huterm_core::RestoreDirection),
    Subscribe,
}

/// Request IDs the client used: every ID up to `floor`, and those above it.
#[derive(Debug, Default)]
struct UsedIds {
    floor: u64,
    above: BTreeSet<u64>,
}

impl UsedIds {
    /// Records `id`, or returns false for a reused one or one past the
    /// window.
    fn record(&mut self, id: RequestId) -> bool {
        let id = id.get();
        if id <= self.floor
            || id > self.floor.saturating_add(REQUEST_ID_WINDOW)
            || !self.above.insert(id)
        {
            return false;
        }
        while self.above.remove(&(self.floor + 1)) {
            self.floor += 1;
        }
        true
    }
}

/// What the handshake settled.
#[derive(Clone, Copy, Debug)]
struct Negotiation {
    caps: MessageCaps,
    context: DecodeContext,
    encoding: FeedEncoding,
}

/// One client connection's protocol state.
pub struct Connection {
    offer: ServerOffer,
    config: ServerConfig,
    scope: IdScope,
    counters: Arc<Counters>,
    opened_at: Duration,
    preamble: Option<Vec<u8>>,
    decoder: FrameDecoder,
    reassembler: Reassembler,
    negotiated: Option<Negotiation>,
    rejecting: bool,
    closed: Option<CloseReason>,
    outbound: Outbound,
    requests: HashMap<RequestId, State>,
    queued: VecDeque<RequestMessage>,
    used: UsedIds,
    large: Option<RequestId>,
    results: VecDeque<Queued>,
    last_work: u64,
    works: HashMap<WorkId, Pending>,
    ready: VecDeque<Work>,
    hierarchy: HierarchyLane,
    tickets: Tickets,
    history: History,
    /// The attachments this connection made that it has not seen end.
    /// Attach, commit, and session-close jobs read under the lock which of
    /// them still exist, so ones another caller ended leave at the next
    /// such job and the set stays near the connection's live attachments.
    attachments: BTreeSet<AttachmentId>,
    reading: bool,
    stalled_since: Option<Duration>,
    over_since: Option<Duration>,
    /// The latest time any call passed in.
    clock: Duration,
    viewers: BTreeMap<ViewerId, Viewer>,
    /// Recently ended viewers and why, oldest first.
    ended: VecDeque<(ViewerId, Ended)>,
    /// Ended viewers whose lanes close once the results still waiting for
    /// them are on the lane: each one's encoded end notice, if it has
    /// one, and whether the lane still sends views not yet started.
    lane_ends: BTreeMap<ViewerId, (Option<Vec<u8>>, bool)>,
    /// Subscriptions in flight, which count toward the viewer limit.
    subscribing: usize,
    /// Viewer jobs handed out and not completed. Disconnect cleanup waits
    /// for them, because a job may hold a viewer that detaching would
    /// revoke.
    viewer_works: usize,
    /// Attachments to detach once no viewer job remains.
    deferred_detach: Vec<AttachmentId>,
    watches: VecDeque<(ViewerId, Box<dyn WakeSource>)>,
    unwatches: VecDeque<ViewerId>,
    /// When staged ordered messages are offered to core again.
    retry_at: Option<Duration>,
    /// A pump found the outbound queue at its budget.
    pumps_waiting: bool,
}

impl fmt::Debug for Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection")
            .field("negotiated", &self.negotiated.map(|n| n.caps.minor()))
            .field("closed", &self.closed)
            .field("requests", &self.requests.len())
            .field("works", &self.works.len())
            .field("viewers", &self.viewers.len())
            .field("retained", &self.outbound.retained())
            .finish_non_exhaustive()
    }
}

impl Connection {
    pub(crate) fn new(
        offer: ServerOffer,
        config: ServerConfig,
        runtime: RuntimeId,
        counters: Arc<Counters>,
        now: Duration,
    ) -> Self {
        Self {
            offer,
            config,
            scope: IdScope::new(runtime),
            counters: Arc::clone(&counters),
            opened_at: now,
            preamble: Some(Vec::with_capacity(PREAMBLE_LEN)),
            decoder: FrameDecoder::new(),
            reassembler: Reassembler::new(
                MessageCaps::for_minor(0),
                Direction::ClientToServer,
                HANDSHAKE_FRAME_LIMIT as usize,
            ),
            negotiated: None,
            rejecting: false,
            closed: None,
            outbound: Outbound::new(
                preamble().to_vec(),
                HANDSHAKE_FRAME_LIMIT,
                config.outbound_budget,
            ),
            requests: HashMap::new(),
            queued: VecDeque::new(),
            used: UsedIds::default(),
            large: None,
            results: VecDeque::new(),
            last_work: 0,
            works: HashMap::new(),
            ready: VecDeque::new(),
            hierarchy: HierarchyLane::default(),
            tickets: Tickets::default(),
            history: History::new(counters),
            attachments: BTreeSet::new(),
            reading: true,
            stalled_since: None,
            over_since: None,
            clock: now,
            viewers: BTreeMap::new(),
            ended: VecDeque::new(),
            lane_ends: BTreeMap::new(),
            subscribing: 0,
            viewer_works: 0,
            deferred_detach: Vec::new(),
            watches: VecDeque::new(),
            unwatches: VecDeque::new(),
            retry_at: None,
            pumps_waiting: false,
        }
    }

    /// Takes bytes read from the transport at `now`.
    pub fn receive(&mut self, bytes: &[u8], now: Duration) {
        if self.closed.is_some() || self.rejecting {
            return;
        }
        let mut bytes = bytes;
        if let Some(received) = &mut self.preamble {
            let take = (PREAMBLE_LEN - received.len()).min(bytes.len());
            received.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if received.len() < PREAMBLE_LEN {
                self.progress(now);
                return;
            }
            let mut peer = [0; PREAMBLE_LEN];
            peer.copy_from_slice(received);
            self.preamble = None;
            if let Err(error) = check_preamble(peer) {
                self.shut(CloseReason::Violation(error.to_string()));
                return;
            }
        }
        self.decoder.push(bytes);
        self.progress(now);
    }

    /// Whether the adapter should read more. False while the outbound
    /// queue is at its budget, so a client that floods requests without
    /// reading meets socket backpressure, and once the connection ends.
    #[must_use]
    pub fn wants_input(&self) -> bool {
        self.closed.is_none() && !self.rejecting && !self.at_budget()
    }

    /// Hands out the next job to run, if any.
    pub fn poll_work(&mut self) -> Option<Work> {
        loop {
            if let Some(work) = self.ready.pop_front() {
                return Some(work);
            }
            if self.closed.is_some() {
                return None;
            }
            if let Some(work) = self.issue_replacement() {
                return Some(work);
            }
            let message = self.queued.pop_front()?;
            if let Some(work) = self.start(message) {
                return Some(work);
            }
        }
    }

    /// Takes the completion of the work `id` at `now`.
    pub fn complete(
        &mut self,
        id: WorkId,
        completion: Completion,
        now: Duration,
    ) {
        let Some(pending) = self.works.remove(&id) else {
            return;
        };
        self.clock = now;
        match pending {
            Pending::Request(request, Kind::Subscribe) => match completion {
                Completion::Subscribed(outcome) => {
                    self.subscribed(request, outcome);
                }
                other => {
                    self.subscribing -= 1;
                    self.finish(request, Kind::Subscribe, other);
                }
            },
            Pending::Request(request, kind) => {
                self.finish(request, kind, completion);
            }
            Pending::Replace => self.replaced(id, completion),
            Pending::Cleanup => {}
            Pending::Viewer(viewer, task) => {
                self.viewer_completed(viewer, task, completion);
                if self.closed.is_some() && self.viewer_works == 0 {
                    self.detach_deferred();
                }
            }
        }
        self.progress(now);
    }

    /// Cuts frames for the writer, about `room` bytes.
    pub fn poll_transmit(&mut self, room: usize) -> Option<Transmit> {
        if self.closed.is_some() {
            return None;
        }
        self.outbound.take(room)
    }

    /// Records `written` bytes the transport accepted at `now`.
    pub fn transmitted(&mut self, written: usize, now: Duration) {
        if self.closed.is_some() {
            return;
        }
        self.outbound.written(written);
        if written > 0 {
            self.stalled_since = Some(now);
        }
        self.progress(now);
    }

    /// Drains the hierarchy feed after it signalled activity.
    pub fn pump(&mut self, now: Duration) {
        self.progress(now);
    }

    /// The earliest deadline armed now, if any.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Duration> {
        if self.closed.is_some() {
            return None;
        }
        let handshake = self
            .negotiated
            .is_none()
            .then_some(self.opened_at + HANDSHAKE_DEADLINE);
        [
            handshake,
            self.reassembler.idle_deadline(),
            self.stalled_since
                .map(|since| since + WRITER_STALL_DEADLINE),
            self.over_since.map(|since| since + WRITER_STALL_DEADLINE),
            self.retry_at,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Checks the deadlines at `now`, ending the connection at one.
    pub fn tick(&mut self, now: Duration) {
        if self.closed.is_some() {
            return;
        }
        self.clock = now;
        if self.retry_at.is_some_and(|at| now >= at) {
            self.retry_at = None;
            self.retry_staged();
        }
        if self.negotiated.is_none()
            && now >= self.opened_at + HANDSHAKE_DEADLINE
        {
            self.shut(CloseReason::HandshakeTimeout);
            return;
        }
        if let Err(error) = self.reassembler.check_idle(now) {
            self.shut(CloseReason::Violation(error.to_string()));
            return;
        }
        let stalled = |since: Option<Duration>| {
            since.is_some_and(|since| now >= since + WRITER_STALL_DEADLINE)
        };
        if stalled(self.stalled_since) || stalled(self.over_since) {
            self.shut(CloseReason::Stalled);
            return;
        }
        self.progress(now);
    }

    /// Ends the connection, returning the cleanup work to run: detaching
    /// its attachments. Its tickets, history, queued requests, and outbound
    /// bytes are dropped. Later completions release what they created and
    /// may hand out more cleanup through [`Self::poll_work`].
    pub fn close(&mut self, _now: Duration) -> Vec<Work> {
        self.shut(CloseReason::Disconnected);
        self.ready.drain(..).collect()
    }

    /// Whether the connection ended.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed.is_some()
    }

    /// Why the connection ended.
    #[must_use]
    pub fn close_reason(&self) -> Option<&CloseReason> {
        self.closed.as_ref()
    }

    /// Whether the connection ended and no work it handed out, or cleanup
    /// it owes, remains.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.closed.is_some()
            && self.works.is_empty()
            && self.ready.is_empty()
            && self.deferred_detach.is_empty()
    }

    /// The encoded bytes the connection retains.
    #[must_use]
    pub fn retained(&self) -> usize {
        self.outbound.retained()
    }

    /// The most bytes this connection ever retains at its negotiated caps.
    #[must_use]
    pub fn retained_bound(&self) -> usize {
        let caps = self
            .negotiated
            .map_or(MessageCaps::for_minor(0), |negotiated| negotiated.caps);
        self.config.retained_bound(caps, self.offer.max_frame)
    }

    /// The hierarchy feed being drained, with its generation, so an adapter
    /// can wait for its activity; a new generation replaces the old feed.
    #[must_use]
    pub fn hierarchy_feed(&self) -> Option<(u64, Arc<dyn HierarchyFeed>)> {
        if self.closed.is_some() {
            return None;
        }
        self.hierarchy.feed()
    }

    /// Requests received and not yet answered.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.requests.len()
    }

    /// Whether a large result is in flight: its request unanswered, or its
    /// text waiting for room or not yet written. Only one may be, so the
    /// texts a connection holds stay bounded.
    fn large_pending(&self) -> bool {
        self.large.is_some()
            || self.results.iter().any(|queued| queued.lane == Lane::Large)
            || self.outbound.large_busy()
    }

    fn at_budget(&self) -> bool {
        self.outbound.retained() >= self.config.outbound_budget
    }

    /// Advances everything a call may have unblocked.
    fn progress(&mut self, now: Duration) {
        self.clock = self.clock.max(now);
        if self.closed.is_none() {
            self.admit_results();
            self.read_frames(now);
            self.admit_results();
            self.drain_hierarchy();
            if self.pumps_waiting && !self.at_budget() {
                self.pumps_waiting = false;
                self.pump_all();
            }
            if let Some(fault) = self.outbound.fault() {
                let fault = fault.to_owned();
                self.shut(CloseReason::Fault(fault));
            }
            if self.rejecting && self.outbound.retained() == 0 {
                self.shut(CloseReason::Rejected);
            }
        }
        self.update_flow(now);
    }

    fn update_flow(&mut self, now: Duration) {
        let reading = self.wants_input();
        if reading != self.reading {
            if reading {
                self.reassembler.resume(now);
            } else {
                self.reassembler.pause(now);
            }
            self.reading = reading;
        }
        if self.closed.is_some() || !self.outbound.pending() {
            self.stalled_since = None;
        } else {
            self.stalled_since.get_or_insert(now);
        }
        if self.closed.is_none() && self.at_budget() {
            self.over_since.get_or_insert(now);
        } else {
            self.over_since = None;
        }
    }

    fn read_frames(&mut self, now: Duration) {
        while self.wants_input() {
            let frame = match self.decoder.next_frame() {
                Ok(Some(frame)) => frame,
                Ok(None) => return,
                Err(error) => {
                    self.shut(CloseReason::Violation(error.to_string()));
                    return;
                }
            };
            match self.reassembler.accept(frame, now) {
                Ok(Some(assembled)) => self.message(&assembled.bytes),
                Ok(None) => {}
                Err(error) => {
                    self.shut(CloseReason::Violation(error.to_string()));
                    return;
                }
            }
        }
    }

    fn message(&mut self, bytes: &[u8]) {
        let Some(negotiated) = self.negotiated else {
            self.hello(bytes);
            return;
        };
        let message = match decode_message(
            bytes,
            Direction::ClientToServer,
            negotiated.context,
        ) {
            Ok(message) => message,
            Err(error) => {
                self.shut(CloseReason::Violation(error.to_string()));
                return;
            }
        };
        match message {
            Message::Request(request) => self.request(request),
            Message::Cancel(cancel) => self.cancel(cancel),
            Message::Input(input) => self.viewer_input(
                input.viewer,
                input.seq,
                input.stamp,
                input.input,
            ),
            Message::Paste(paste) => self.viewer_input(
                paste.viewer,
                paste.seq,
                paste.stamp,
                TerminalInput::Paste(paste.text),
            ),
            Message::Focus(focus) => {
                self.viewer_focus(focus.viewer, focus.focused);
            }
            Message::Credit(credit) => self.viewer_credit(credit),
            Message::ResyncViewer(resync) => self.viewer_resync(resync.viewer),
            Message::Hello(_) => {
                self.shut(CloseReason::Violation("a second Hello".into()));
            }
            // Decoding accepts only what a client sends.
            _ => {}
        }
    }

    fn hello(&mut self, bytes: &[u8]) {
        let is_hello = huterm_wire::codec::Reader::new(bytes)
            .u64()
            .is_ok_and(|kind| kind == MessageKind::Hello.number());
        if !is_hello {
            self.shut(CloseReason::Violation("a message before Hello".into()));
            return;
        }
        match negotiate(bytes, &self.offer) {
            Ok(mut welcome) => {
                welcome.profile.max_in_flight_requests =
                    self.config.max_in_flight_requests;
                let caps = MessageCaps::for_minor(welcome.minor);
                let max_frame = welcome.max_frame;
                let encoded = encode_message(
                    &Message::Welcome(welcome),
                    self.scope,
                    caps,
                );
                let encoded = match encoded {
                    Ok(encoded) => encoded,
                    Err(error) => {
                        self.shut(CloseReason::Fault(error.to_string()));
                        return;
                    }
                };
                self.outbound.push_encoded(
                    Lane::Control,
                    encoded,
                    Some(max_frame),
                );
                self.outbound.set_encoding(self.scope, caps);
                self.decoder.set_limit(max_frame);
                self.reassembler
                    .set_caps(caps, reassembly_budget(caps.minor()));
                self.negotiated = Some(Negotiation {
                    caps,
                    context: DecodeContext {
                        hierarchy: HierarchyContext {
                            stream: StreamId::new(self.scope.runtime()),
                        },
                        caps,
                    },
                    encoding: FeedEncoding {
                        scope: self.scope,
                        caps,
                    },
                });
            }
            Err(reject) => {
                match encode_message(
                    &Message::Reject(reject),
                    self.scope,
                    MessageCaps::for_minor(0),
                ) {
                    Ok(encoded) => {
                        self.outbound.push_encoded(
                            Lane::Control,
                            encoded,
                            None,
                        );
                        self.rejecting = true;
                    }
                    Err(error) => {
                        self.shut(CloseReason::Fault(error.to_string()));
                    }
                }
            }
        }
    }

    fn request(&mut self, message: RequestMessage) {
        let id = message.id;
        if !self.used.record(id) {
            self.shut(CloseReason::Violation(format!(
                "request ID {} reused or out of range",
                id.get()
            )));
            return;
        }
        let large = message.request.kind().large_result();
        if self.requests.len() >= self.config.max_in_flight_requests as usize
            || (large && self.large_pending())
        {
            self.answer_now(
                id,
                Err(WireError::new(
                    ErrorCode::Busy,
                    "too many requests in flight",
                )),
            );
            return;
        }
        if large {
            self.large = Some(id);
        }
        if message.request.viewer().is_some() {
            // Handled as it arrives, in order with the viewer's input.
            self.requests.insert(id, State::Started);
            self.viewer_request(id, message.request);
            return;
        }
        self.requests.insert(id, State::Queued);
        self.queued.push_back(message);
    }

    fn cancel(&mut self, cancel: Cancel) {
        let id = cancel.request;
        let cancelled = match self.requests.get(&id) {
            Some(State::Queued) => {
                self.queued.retain(|queued| queued.id != id);
                true
            }
            Some(State::Waiting) => self.hierarchy.cancel(id),
            _ => false,
        };
        if cancelled {
            self.answer(
                id,
                Err(WireError::new(
                    ErrorCode::Cancelled,
                    "cancelled before it started",
                )),
            );
        }
    }

    /// Answers a request on the control lane, whether or not it was in the
    /// table.
    fn answer_now(
        &mut self,
        request: RequestId,
        outcome: Result<Success, WireError>,
    ) {
        self.results.push_back(Queued {
            lane: Lane::Control,
            opens: None,
            result: RequestResult { request, outcome },
        });
        self.admit_results();
    }

    fn answer(
        &mut self,
        request: RequestId,
        outcome: Result<Success, WireError>,
    ) {
        self.answer_on(Lane::Control, request, outcome);
    }

    /// Answers a request on `lane`: a viewer's lane for a request that
    /// names the viewer, the large-result lane for selection text.
    fn answer_on(
        &mut self,
        lane: Lane,
        request: RequestId,
        outcome: Result<Success, WireError>,
    ) {
        self.requests.remove(&request);
        if self.large == Some(request) {
            self.large = None;
        }
        self.results.push_back(Queued {
            lane,
            opens: None,
            result: RequestResult { request, outcome },
        });
        self.admit_results();
    }

    /// Queues waiting results while the outbound queue is under budget.
    fn admit_results(&mut self) {
        while self.closed.is_none() && !self.at_budget() {
            let Some(Queued {
                lane,
                opens,
                result,
            }) = self.results.pop_front()
            else {
                return;
            };
            let Some(negotiated) = self.negotiated else {
                return;
            };
            // A viewer's lane that closed meanwhile cannot carry the
            // result; the control lane still answers the request.
            let lane = match lane {
                Lane::Viewer(viewer) if !self.outbound.viewer_open(viewer) => {
                    Lane::Control
                }
                lane => lane,
            };
            let request = result.request;
            if lane == Lane::Large
                && let Ok(Success {
                    response: Response::Text { text: Some(text) },
                    committed: None,
                }) = &result.outcome
            {
                match huterm_wire::response::text_result_parts(request, text) {
                    Ok((head, tail)) => {
                        let Ok(Success {
                            response: Response::Text { text: Some(text) },
                            ..
                        }) = result.outcome
                        else {
                            unreachable!("matched above");
                        };
                        self.outbound
                            .push(lane, Item::Text { head, text, tail });
                        continue;
                    }
                    Err(error) => {
                        self.results.push_front(Queued {
                            lane,
                            opens,
                            result: RequestResult {
                                request,
                                outcome: Err(WireError::new(
                                    ErrorCode::LimitExceeded,
                                    error.to_string(),
                                )),
                            },
                        });
                        continue;
                    }
                }
            }
            let message = if lane == Lane::Large {
                Message::LargeResult(result)
            } else {
                Message::Result(result)
            };
            let encoded = encode_message(&message, self.scope, negotiated.caps)
                .or_else(|error| {
                    encode_message(
                        &Message::Result(RequestResult {
                            request,
                            outcome: Err(WireError::new(
                                ErrorCode::Internal,
                                format!("the result did not encode: {error}"),
                            )),
                        }),
                        self.scope,
                        negotiated.caps,
                    )
                });
            match encoded {
                Ok(encoded) => self.outbound.push_encoded(lane, encoded, None),
                Err(error) => {
                    self.shut(CloseReason::Fault(error.to_string()));
                    return;
                }
            }
            if let Some(viewer) = opens {
                self.open_viewer(viewer);
            }
            if let Lane::Viewer(viewer) = lane {
                self.end_lane_if_answered(viewer);
            }
        }
    }

    fn digest(&self) -> Option<DigestProfile> {
        let negotiated = self.negotiated?;
        self.hierarchy
            .requested()
            .then(|| digest_profile(negotiated.caps.minor()))
    }

    fn work(&mut self, pending: Pending, job: Job) -> Work {
        if matches!(pending, Pending::Viewer(..)) {
            self.viewer_works += 1;
        }
        self.last_work += 1;
        let id = WorkId(self.last_work);
        self.works.insert(id, pending);
        Work { id, job }
    }

    fn request_work(
        &mut self,
        id: RequestId,
        kind: Kind,
        operation: Operation,
    ) -> Work {
        let digest = self.digest();
        self.work(
            Pending::Request(id, kind),
            Job::Request(RequestJob { operation, digest }),
        )
    }

    fn issue_replacement(&mut self) -> Option<Work> {
        let negotiated = self.negotiated?;
        if !self.hierarchy.wants_replacement()
            || self.at_budget()
            || self.outbound.lane_retained(Lane::Hierarchy)
                >= self.config.hierarchy_lane_budget
        {
            return None;
        }
        self.last_work += 1;
        let id = WorkId(self.last_work);
        let kind = self.hierarchy.issue(id);
        self.works.insert(id, Pending::Replace);
        Some(Work {
            id,
            job: Job::Replace(ReplaceJob {
                encoding: negotiated.encoding,
                kind,
            }),
        })
    }

    fn owns(&self, attachment: AttachmentId) -> bool {
        self.attachments.contains(&attachment)
    }

    fn not_permitted(attachment: AttachmentId) -> WireError {
        WireError::new(
            ErrorCode::NotPermitted,
            format!("attachment {} is not this connection's", attachment.get()),
        )
    }

    /// Starts a request: hands out its work, or answers it at once.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per request that needs the connection's tables"
    )]
    fn start(&mut self, message: RequestMessage) -> Option<Work> {
        let RequestMessage { id, request } = message;
        self.requests.insert(id, State::Started);
        let refuse = |code, message: &str| Err(WireError::new(code, message));
        match request {
            Request::SubscribeHierarchy | Request::ResyncHierarchy => {
                self.requests.insert(id, State::Waiting);
                self.hierarchy.want(id);
                None
            }
            Request::ReleaseCloseTicket { ticket } => {
                let outcome = self.tickets.release(ticket).map(|()| Success {
                    response: Response::Unit,
                    committed: None,
                });
                self.answer(id, outcome);
                None
            }
            Request::ExecuteCommand {
                invocation: DecodedInvocation::Unknown { id: text, .. },
            } => {
                self.answer(
                    id,
                    refuse(
                        ErrorCode::UnknownCommand,
                        &format!("unknown command `{text}`"),
                    ),
                );
                None
            }
            Request::SubscribeTerminal {
                attachment,
                terminal,
                options,
            } => {
                if options.capabilities.host_effects {
                    self.answer(
                        id,
                        refuse(
                            ErrorCode::Unsupported,
                            "host effects arrive with a later version",
                        ),
                    );
                    return None;
                }
                if self.viewers.len() + self.subscribing
                    >= self.config.max_viewers
                {
                    self.answer(
                        id,
                        refuse(
                            ErrorCode::LimitExceeded,
                            "the connection holds its most viewers",
                        ),
                    );
                    return None;
                }
                self.subscribing += 1;
                Some(self.request_work(
                    id,
                    Kind::Subscribe,
                    Operation::Subscribe {
                        attachment,
                        terminal,
                        options: Box::new(convert::viewer_options(options)),
                    },
                ))
            }
            Request::PrepareClose { request } => {
                let named = match &request {
                    CloseRequest::Window(attachment)
                    | CloseRequest::EmptiedWindow { attachment, .. } => {
                        Some(*attachment)
                    }
                    _ => None,
                };
                if let Some(attachment) = named.filter(|a| !self.owns(*a)) {
                    self.answer(id, Err(Self::not_permitted(attachment)));
                    return None;
                }
                match self.tickets.reserve() {
                    Ok(ticket) => Some(self.request_work(
                        id,
                        Kind::Prepare(ticket),
                        Operation::PrepareClose {
                            ticket,
                            request: convert::close_request(request),
                        },
                    )),
                    Err(error) => {
                        self.answer(id, Err(error));
                        None
                    }
                }
            }
            Request::CommitClose { ticket, confirmed } => {
                match self.tickets.take(ticket) {
                    Ok(consent) => {
                        let attachments =
                            self.attachments.iter().copied().collect();
                        Some(self.request_work(
                            id,
                            Kind::Commit(ticket),
                            Operation::CommitClose {
                                consent,
                                confirmed,
                                attachments,
                            },
                        ))
                    }
                    Err(error) => {
                        self.answer(id, Err(error));
                        None
                    }
                }
            }
            Request::CloseSession { session } => {
                let attachments = self.attachments.iter().copied().collect();
                Some(self.request_work(
                    id,
                    Kind::Ending,
                    Operation::CloseSession {
                        session,
                        attachments,
                    },
                ))
            }
            Request::DetachSession { attachment } => {
                if !self.owns(attachment) {
                    self.answer(id, Err(Self::not_permitted(attachment)));
                    return None;
                }
                Some(self.request_work(
                    id,
                    Kind::Detach(attachment),
                    Operation::Plain(request),
                ))
            }
            Request::RetargetAttachment { attachment, .. } => {
                if !self.owns(attachment) {
                    self.answer(id, Err(Self::not_permitted(attachment)));
                    return None;
                }
                Some(self.request_work(
                    id,
                    Kind::Plain,
                    Operation::Plain(request),
                ))
            }
            Request::AttachSession { session } => {
                let attachments = self.attachments.iter().copied().collect();
                Some(self.request_work(
                    id,
                    Kind::Attach,
                    Operation::AttachSession {
                        session,
                        attachments,
                    },
                ))
            }
            Request::SwapPanes { slot, .. }
            | Request::MovePane { slot, .. }
            | Request::MoveTabToPane { slot, .. }
            | Request::ArrangePanes { slot, .. } => {
                let slot = self.history.touch(slot);
                Some(self.request_work(
                    id,
                    Kind::Rearrange(slot),
                    Operation::Plain(request),
                ))
            }
            Request::RestoreLayout { token, direction } => {
                let direction = convert::direction(direction);
                if let Some((slot, record)) =
                    self.history.take(token, direction)
                {
                    return Some(self.request_work(
                        id,
                        Kind::Restore(slot, direction),
                        Operation::Restore { record, direction },
                    ));
                }
                self.answer(
                    id,
                    refuse(
                        ErrorCode::StaleUndo,
                        "the undo token names no current entry",
                    ),
                );
                None
            }
            Request::SelectSession { .. }
            | Request::SelectWorkspace { .. }
            | Request::SelectTab { .. }
            | Request::SelectPane { .. }
            | Request::CreateSession { .. }
            | Request::CreateWorkspace { .. }
            | Request::CreateWorkspaceWithTab { .. }
            | Request::OpenTab { .. }
            | Request::SplitPane { .. }
            | Request::ClosePane { .. }
            | Request::CloseTab { .. }
            | Request::CloseWorkspace { .. }
            | Request::RenameSession { .. }
            | Request::RenameWorkspace { .. }
            | Request::RenameTab { .. }
            | Request::RenamePane { .. }
            | Request::SetWorkspacePinned { .. }
            | Request::ToggleInputSync { .. }
            | Request::CloseEmptiedWorkspace { .. }
            | Request::ResizeSplit { .. }
            | Request::SetPaneZoom { .. }
            | Request::ReorderTab { .. }
            | Request::MoveTab { .. }
            | Request::MoveSession { .. }
            | Request::MoveWorkspace { .. }
            | Request::MoveWorkspaceToNewSession { .. }
            | Request::ExecuteCommand { .. } => Some(self.request_work(
                id,
                Kind::Plain,
                Operation::Plain(request),
            )),
            // A variant this server cannot serve, such as the test-only
            // minor version's request.
            _ => {
                self.answer(
                    id,
                    refuse(
                        ErrorCode::Unsupported,
                        "this server does not serve that request",
                    ),
                );
                None
            }
        }
    }

    /// Ends a request's work.
    fn finish(&mut self, id: RequestId, kind: Kind, completion: Completion) {
        if self.closed.is_some() {
            self.release(&completion);
            return;
        }
        let outcome = match (kind, completion) {
            (Kind::Plain, Completion::Answered(outcome)) => outcome,
            (Kind::Detach(attachment), Completion::Answered(outcome)) => {
                if outcome.is_ok() {
                    self.attachments.remove(&attachment);
                }
                outcome
            }
            (Kind::Prepare(ticket), Completion::Prepared(outcome)) => {
                match outcome {
                    Ok((success, assessment)) => {
                        let consent = Consent::new(
                            assessment,
                            Live::new(&self.counters, Record::Ticket),
                        );
                        if self.tickets.prepared(ticket, consent) {
                            Ok(success)
                        } else {
                            Err(WireError::new(
                                ErrorCode::StaleTarget,
                                "the close ticket was released",
                            ))
                        }
                    }
                    Err(error) => {
                        self.tickets.forget(ticket);
                        Err(error)
                    }
                }
            }
            (
                Kind::Commit(ticket),
                Completion::Committed {
                    outcome,
                    consent,
                    ended,
                },
            ) => {
                self.tickets.committed(ticket, consent);
                self.ended(&ended);
                outcome
            }
            (Kind::Ending, Completion::Ended { outcome, ended }) => {
                self.ended(&ended);
                outcome
            }
            (Kind::Attach, Completion::Attached { outcome, ended }) => {
                self.ended(&ended);
                outcome.map(|(success, attachment)| {
                    self.attachments.insert(attachment);
                    success
                })
            }
            (Kind::Rearrange(slot), Completion::Rearranged(outcome)) => outcome
                .map(|(mut success, change)| {
                    let token = success.committed.and_then(|committed| {
                        self.history.file(slot, change, committed.seq)
                    });
                    success.response = Response::Undo { token };
                    success
                }),
            (
                Kind::Restore(slot, direction),
                Completion::Restored { outcome, record },
            ) => outcome.map(|mut success| {
                let token = success.committed.and_then(|committed| {
                    self.history.refile(slot, direction, record, committed.seq)
                });
                success.response = Response::Undo { token };
                success
            }),
            (kind, completion) => Err(WireError::new(
                ErrorCode::Internal,
                format!("{kind:?} completed as {completion:?}"),
            )),
        };
        self.answer(id, outcome);
    }

    fn ended(&mut self, ended: &[AttachmentId]) {
        for attachment in ended {
            self.attachments.remove(attachment);
        }
    }

    /// Releases what a job created for a connection that has ended: an
    /// attachment is detached, everything else is dropped.
    fn release(&mut self, completion: &Completion) {
        if let Completion::Attached {
            outcome: Ok((_, attachment)),
            ..
        } = completion
        {
            let work =
                self.work(Pending::Cleanup, Job::Detach(vec![*attachment]));
            self.ready.push_back(work);
        }
    }

    fn replaced(&mut self, id: WorkId, completion: Completion) {
        let Some((generation, waiters)) = self.hierarchy.complete(id) else {
            return;
        };
        if self.closed.is_some() {
            return;
        }
        let Completion::Replaced(outcome) = completion else {
            return;
        };
        match outcome {
            Ok(replaced) => {
                // The snapshot was admitted when the replacement was
                // issued, so it is queued whatever the queue holds now.
                self.outbound.push_encoded(
                    Lane::Hierarchy,
                    replaced.message,
                    None,
                );
                self.hierarchy.install(generation, replaced.feed);
                let committed = Committed {
                    seq: replaced.seq,
                    digest: Some(replaced.digest),
                };
                for waiter in waiters {
                    self.answer(
                        waiter,
                        Ok(Success {
                            response: Response::Hierarchy {
                                seq: replaced.seq,
                                digest: replaced.digest,
                            },
                            committed: Some(committed),
                        }),
                    );
                }
            }
            Err(error) => {
                self.hierarchy.failed();
                for waiter in waiters {
                    self.answer(waiter, Err(error.clone()));
                }
            }
        }
    }

    /// Forwards hierarchy events while the lane and the queue have room.
    fn drain_hierarchy(&mut self) {
        loop {
            if self.at_budget()
                || self.outbound.lane_retained(Lane::Hierarchy)
                    >= self.config.hierarchy_lane_budget
            {
                return;
            }
            let Some(feed) = self.hierarchy.draining() else {
                return;
            };
            match feed.next() {
                Ok(encoded) => {
                    self.outbound.push_encoded(Lane::Hierarchy, encoded, None);
                }
                Err(FeedError::Empty) => return,
                Err(FeedError::Lagged) => {
                    self.hierarchy.lag();
                    return;
                }
                Err(FeedError::Closed) => {
                    self.hierarchy.closed();
                    return;
                }
                Err(FeedError::Encode(error)) => {
                    self.shut(CloseReason::Fault(error.to_string()));
                    return;
                }
            }
        }
    }

    /// Ends the connection for `reason`, queueing its cleanup.
    fn shut(&mut self, reason: CloseReason) {
        if self.closed.is_some() {
            return;
        }
        self.closed = Some(reason);
        self.queued.clear();
        self.requests.clear();
        self.results.clear();
        self.lane_ends.clear();
        self.hierarchy.clear();
        self.tickets.clear();
        self.history.clear();
        // Viewers drop first, so their admitted input still runs and the
        // detach below finds them dropped rather than revoking them.
        self.drop_viewers();
        // Viewer jobs not yet handed out hold viewers; drop them too.
        let works = &mut self.works;
        let mut dropped = 0;
        self.ready.retain(|work| {
            let viewer = matches!(work.job, Job::Viewer(_));
            if viewer {
                works.remove(&work.id);
                dropped += 1;
            }
            !viewer
        });
        self.viewer_works -= dropped;
        self.outbound.clear();
        self.stalled_since = None;
        self.over_since = None;
        self.retry_at = None;
        if !self.attachments.is_empty() {
            let attachments = std::mem::take(&mut self.attachments);
            self.deferred_detach = attachments.into_iter().collect();
            if self.viewer_works == 0 {
                self.detach_deferred();
            }
        }
    }

    /// Hands out the disconnect's detach once no viewer job holds a viewer.
    fn detach_deferred(&mut self) {
        if self.deferred_detach.is_empty() {
            return;
        }
        let attachments = std::mem::take(&mut self.deferred_detach);
        let work = self.work(Pending::Cleanup, Job::Detach(attachments));
        self.ready.push_back(work);
    }
}

#[cfg(test)]
mod tests;
