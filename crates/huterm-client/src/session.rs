//! The client's protocol as a sans-IO state machine that lives across
//! connections.
//!
//! A [`ClientSession`] keeps what survives a reconnect: the map from server
//! incarnations to client-local runtimes, the hierarchy projection, the
//! sequence waiters, and the queries that wait for the next connection. Per
//! connection it owns the preamble, the handshake and its deadline, the
//! advertised transport profile, framing and reassembly, the request table,
//! the control lane, and the progress deadline. Every call that can arm or
//! fire a deadline takes `now` from an injected clock:
//!
//! - [`ClientSession::connect`] opens a connection: the preamble and
//!   `Hello` are the first [`Transmit`].
//! - [`ClientSession::receive`] takes bytes read from the transport, and
//!   [`ClientSession::poll_transmit`] hands out what to write.
//! - [`ClientSession::disconnected`] reports a broken transport.
//! - [`ClientSession::next_deadline`] and [`ClientSession::tick`] drive the
//!   handshake, stream idle, and progress deadlines.
//! - [`ClientSession::request`] and [`ClientSession::wait_for`] take work;
//!   [`ClientSession::poll_event`] reports results, waiter resolutions,
//!   hierarchy changes, and connection state.
//!
//! When the session ends a connection itself, after a protocol violation,
//! a contract fault, or a deadline, it reports [`ClientEvent::Closed`] and
//! [`ClientSession::is_connected`] turns false; the adapter then shuts the
//! transport down.
//!
//! Requests carry a session-wide [`CallId`]. Each connection numbers what it
//! sends with its own `RequestId` from 1, so a query sent again on the next
//! connection keeps its call and gets a new request number.

mod viewers;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::Duration;

use huterm_protocol::{
    HierarchyState, RuntimeId, ServerIncarnation, StreamId, Touched, ViewerId,
};

pub use self::viewers::ViewerEvent;
use self::viewers::{Carried, ClientViewer, EndedViewers, Subscribing};
use huterm_wire::caps::{Direction, MessageCaps, MessageKind};
use huterm_wire::codec::{EncodeError, IdScope, Reader, Writer};
use huterm_wire::error::{ErrorCode, WireError};
use huterm_wire::frame::{
    FrameDecoder, HANDSHAKE_FRAME_LIMIT, PREAMBLE_LEN, check_preamble,
};
use huterm_wire::handshake::{
    Negotiated, Reject, TransportProfile, Welcome, accept_welcome,
    decode_reject, decode_welcome,
};
use huterm_wire::hierarchy::HierarchyContext;
use huterm_wire::message::{
    DecodeContext, Message, decode_message, encode_message,
};
use huterm_wire::request::{
    Cancel, Request, RequestId, RequestKind, RequestMessage, RetryClass,
    encode_cancel, encode_request,
};
use huterm_wire::response::{Committed, RequestResult, Response, Success};
use huterm_wire::stream::{Assembled, MAX_OPEN_STREAMS, Reassembler};

use crate::config::ClientConfig;
use crate::outbound::Transmit;
use crate::projection::{
    Checkpoints, Outcome, Projection, ProjectionKind, Waiters,
};

/// The most structural digest checkpoints a session keeps for sequences
/// its projection has not reached. A result frees its request slot while
/// its checkpoint waits for the hierarchy, so in-flight requests do not
/// bound them. Core queues 1,024 hierarchy events per subscription before
/// the server lags and resynchronizes, and the resync snapshot drops the
/// checkpoints it covers; four times that leaves room for the events the
/// server's lane and the socket hold. Past it, the oldest checkpoint is
/// dropped and counted in [`SessionStats::checkpoints_dropped`], and the
/// session sends one `ResyncHierarchy` of its own whose snapshot covers
/// the dropped sequence, so the check still happens: the snapshot's digest
/// is checked against its state.
pub const MAX_PENDING_CHECKPOINTS: usize = 4 * 1024;

/// A request's identity for the life of its session, across connections.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CallId(u64);

impl CallId {
    /// The identity's number; calls are numbered from 1 in issue order.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A sequence waiter's identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WaitId(u64);

impl WaitId {
    /// The identity's number; waiters are numbered from 1.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// How a sequence waiter resolved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resolution {
    /// The projection reached the awaited sequence.
    Ready,
    /// A stream change, or the session giving up, ended the wait; the
    /// projection may never reach the sequence.
    Cancelled,
}

/// Why a connection ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CloseReason {
    /// The transport broke, or the caller replaced or closed it.
    Disconnected,
    /// The server refused the `Hello`.
    Rejected(Reject),
    /// Neither `Welcome` nor `Reject` arrived within the handshake
    /// deadline.
    HandshakeTimeout,
    /// No frame arrived within the progress deadline while something was
    /// outstanding.
    ProgressTimeout,
    /// The server broke the protocol.
    Violation(String),
    /// A hierarchy snapshot's digest disagreed with its own state. Another
    /// snapshot would disagree the same way, so the client disconnects
    /// instead of resynchronizing.
    ContractFault(String),
}

/// A connection whose handshake completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Connected {
    /// The server incarnation that scopes the connection's identities.
    pub incarnation: ServerIncarnation,
    /// The client-local runtime the incarnation maps to.
    pub runtime: RuntimeId,
    /// The negotiated minor version.
    pub minor: u16,
    /// Whether the incarnation differs from the previous connection's, so
    /// identities from before are foreign and sequence waiters were
    /// cancelled.
    pub new_incarnation: bool,
}

/// A change to the projection.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "an update lives only until it is polled; boxing the summary \
              would allocate per event"
)]
pub enum HierarchyUpdate {
    /// A snapshot replaced the projection: a subscription, a resync, or a
    /// projection of a newer event onto this connection's version.
    Installed {
        /// The snapshot's sequence.
        seq: u64,
    },
    /// One event applied.
    Applied {
        /// The event's sequence.
        seq: u64,
        /// What it changed.
        touched: Touched,
    },
}

/// Something the session reports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientEvent {
    /// A connection completed its handshake.
    Connected(Connected),
    /// A connection ended.
    Closed(CloseReason),
    /// A request's result, from the server or refused locally.
    Result {
        /// The request.
        call: CallId,
        /// Its success or failure.
        outcome: Result<Success, WireError>,
    },
    /// The projection changed.
    Hierarchy(HierarchyUpdate),
    /// A sequence waiter resolved.
    Waiter {
        /// The waiter.
        wait: WaitId,
        /// How.
        resolution: Resolution,
    },
    /// Something happened to a viewer.
    Viewer(ViewerEvent),
    /// The native client's event channel filled, so it dropped the events
    /// after the last one it delivered: re-read the projection, and the
    /// connection state if it matters, as after core's subscription lag.
    /// Unlike core's resubscribe, the re-read is not atomic with the
    /// channel: events queued behind the marker may already be in the
    /// re-read projection, so skip an [`HierarchyUpdate::Applied`] whose
    /// sequence is at most the re-read projection's. The session itself
    /// never reports it.
    Lagged,
}

/// Counts tests and diagnostics read.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionStats {
    /// `ResyncHierarchy` requests the session sent on its own, after a
    /// digest mismatch, a gap, an inconsistent event, or a checkpoint
    /// dropped at [`MAX_PENDING_CHECKPOINTS`].
    pub resync_requests: u64,
    /// Digests that disagreed with the projection: on events and at
    /// structural checkpoints.
    pub digest_mismatches: u64,
    /// Checkpoints dropped at [`MAX_PENDING_CHECKPOINTS`], each left to a
    /// snapshot that covers it.
    pub checkpoints_dropped: u64,
    /// `ResyncViewer` messages the session sent after a delta did not
    /// apply or its digest differed.
    pub viewer_resyncs: u64,
}

#[derive(Debug)]
struct Call {
    kind: RequestKind,
    /// The request while unsent, and a query's for sending again.
    request: Option<Request>,
    /// The session's own subscribe or resync; nobody awaits its result.
    internal: bool,
    /// The request number on the current connection once sent.
    sent: Option<RequestId>,
    /// The caller cancelled it after it was sent, so a disconnect resolves
    /// it `Cancelled` instead of sending it again.
    cancelled: bool,
}

/// A hierarchy subscription or resync result, held until the projection
/// reaches its sequence.
#[derive(Debug)]
struct Held {
    seq: u64,
    call: CallId,
    success: Success,
}

#[derive(Debug)]
struct Open {
    welcome: Welcome,
    negotiated: Negotiated,
    context: DecodeContext,
    max_in_flight: usize,
    request_cap: usize,
    cancel_cap: usize,
}

impl Open {
    fn scope(&self) -> IdScope {
        self.context.hierarchy.scope()
    }

    fn minor(&self) -> u16 {
        self.negotiated.minor
    }
}

#[derive(Debug)]
struct Connection {
    opened_at: Duration,
    preamble: Vec<u8>,
    decoder: FrameDecoder,
    reassembler: Reassembler,
    open: Option<Open>,
    outbound: VecDeque<Transmit>,
    sent: HashMap<RequestId, CallId>,
    next_id: Option<RequestId>,
    progress_since: Option<Duration>,
    /// Whether a snapshot installed on this connection, after which a
    /// snapshot behind the projection is a violation.
    installed: bool,
}

/// The client's protocol across connections.
#[derive(Debug)]
pub struct ClientSession {
    config: ClientConfig,
    kind: ProjectionKind,
    runtimes: BTreeMap<ServerIncarnation, RuntimeId>,
    stream: Option<StreamId>,
    minor: u16,
    projection: Option<Projection>,
    waiters: Waiters,
    checkpoints: Checkpoints,
    calls: HashMap<CallId, Call>,
    queue: VecDeque<CallId>,
    held: Vec<Held>,
    /// A subscription succeeded, so every later connection subscribes
    /// again.
    subscribed: bool,
    /// A `ResyncHierarchy` of the session's own is outstanding.
    resyncing: bool,
    /// The highest sequence whose checkpoint was dropped at the bound and
    /// that no snapshot installed since has covered.
    uncovered: Option<u64>,
    stats: SessionStats,
    last_call: u64,
    last_wait: u64,
    connection: Option<Connection>,
    connected: Option<Connected>,
    events: VecDeque<ClientEvent>,
    viewers: BTreeMap<ViewerId, ClientViewer>,
    subscribing: HashMap<CallId, Subscribing>,
    /// Status baselines and final views kept for the current incarnation.
    carried: Carried,
    /// Viewers that ended on the current connection.
    ended: EndedViewers,
}

impl ClientSession {
    /// A session with no connection yet.
    #[must_use]
    pub fn new(config: ClientConfig) -> Self {
        Self::with_kind(config, ProjectionKind::Base)
    }

    /// A session at the test-only minor version, whose projection holds
    /// the test-only hierarchy and computes digests per the negotiated
    /// minor version.
    #[cfg(any(test, feature = "test-minor"))]
    #[must_use]
    pub fn with_test_projection(mut config: ClientConfig) -> Self {
        config.hello.minor = huterm_wire::test_minor::TEST_MINOR;
        Self::with_kind(config, ProjectionKind::Test)
    }

    fn with_kind(config: ClientConfig, kind: ProjectionKind) -> Self {
        Self {
            config,
            kind,
            runtimes: BTreeMap::new(),
            stream: None,
            minor: 0,
            projection: None,
            waiters: Waiters::default(),
            checkpoints: Checkpoints::default(),
            calls: HashMap::new(),
            queue: VecDeque::new(),
            held: Vec::new(),
            subscribed: false,
            resyncing: false,
            uncovered: None,
            stats: SessionStats::default(),
            last_call: 0,
            last_wait: 0,
            connection: None,
            connected: None,
            events: VecDeque::new(),
            viewers: BTreeMap::new(),
            subscribing: HashMap::new(),
            carried: Carried::default(),
            ended: EndedViewers::default(),
        }
    }

    /// Opens a connection at `now`, ending the current one first: the
    /// preamble and `Hello` are the first transmit.
    pub fn connect(&mut self, now: Duration) {
        self.close(CloseReason::Disconnected);
        let hello = encode_message(
            &Message::Hello(self.config.hello.clone()),
            IdScope::new(RuntimeId::new(0)),
            MessageCaps::for_minor(0),
        );
        let mut connection = Connection {
            opened_at: now,
            preamble: Vec::with_capacity(PREAMBLE_LEN),
            decoder: FrameDecoder::new(),
            reassembler: Reassembler::new(
                MessageCaps::for_minor(0),
                Direction::ServerToClient,
                HANDSHAKE_FRAME_LIMIT as usize,
            ),
            open: None,
            outbound: VecDeque::new(),
            sent: HashMap::new(),
            next_id: Some(RequestId::FIRST),
            progress_since: None,
            installed: false,
        };
        match hello {
            Ok(hello) => {
                connection
                    .outbound
                    .push_back(Transmit::opening(hello, HANDSHAKE_FRAME_LIMIT));
                self.connection = Some(connection);
            }
            // A `Hello` past its limits is the caller's configuration
            // error; the connection ends before it starts.
            Err(error) => {
                self.connection = Some(connection);
                self.close(CloseReason::Violation(format!(
                    "the client's Hello does not encode: {error}"
                )));
            }
        }
    }

    /// Takes bytes read from the transport at `now`.
    pub fn receive(&mut self, bytes: &[u8], now: Duration) {
        let Some(connection) = self.connection.as_mut() else {
            return;
        };
        let mut bytes = bytes;
        if connection.preamble.len() < PREAMBLE_LEN {
            let take =
                (PREAMBLE_LEN - connection.preamble.len()).min(bytes.len());
            connection.preamble.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
            if connection.preamble.len() < PREAMBLE_LEN {
                return;
            }
            let mut peer = [0; PREAMBLE_LEN];
            peer.copy_from_slice(&connection.preamble);
            if let Err(error) = check_preamble(peer) {
                self.close(CloseReason::Violation(error.to_string()));
                return;
            }
        }
        connection.decoder.push(bytes);
        self.read_frames(now);
        self.flush_queue();
        self.refresh_progress(now);
    }

    /// The next message to write, if any.
    pub fn poll_transmit(&mut self) -> Option<Transmit> {
        self.connection.as_mut()?.outbound.pop_front()
    }

    /// Reports that the transport broke at `now`. Every mutation in flight
    /// resolves `OutcomeUnknown` and every viewer request `Disconnected`, at
    /// once; queries wait for the next connection.
    pub fn disconnected(&mut self, now: Duration) {
        let _ = now;
        self.close(CloseReason::Disconnected);
    }

    /// Gives up reconnecting: ends the connection, fails every query that
    /// waits for one with `Disconnected`, and cancels every sequence
    /// waiter.
    pub fn abandon(&mut self, now: Duration) {
        self.disconnected(now);
        while let Some(call) = self.queue.pop_front() {
            self.finish(
                call,
                Err(WireError::new(
                    ErrorCode::Disconnected,
                    "the client stopped reconnecting",
                )),
            );
        }
        let events = &mut self.events;
        self.waiters.cancel_all(|wait| {
            events.push_back(ClientEvent::Waiter {
                wait,
                resolution: Resolution::Cancelled,
            });
        });
    }

    /// The earliest deadline armed now, if any.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Duration> {
        let connection = self.connection.as_ref()?;
        if connection.open.is_none() {
            return Some(connection.opened_at + self.config.handshake_deadline);
        }
        [
            connection.reassembler.idle_deadline(),
            connection
                .progress_since
                .map(|since| since + self.config.progress_deadline),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Checks the deadlines at `now`, ending the connection at one.
    pub fn tick(&mut self, now: Duration) {
        let Some(connection) = self.connection.as_ref() else {
            return;
        };
        if connection.open.is_none() {
            if now >= connection.opened_at + self.config.handshake_deadline {
                self.close(CloseReason::HandshakeTimeout);
            }
            return;
        }
        if let Err(error) = connection.reassembler.check_idle(now) {
            self.close(CloseReason::Violation(error.to_string()));
            return;
        }
        if connection
            .progress_since
            .is_some_and(|since| now >= since + self.config.progress_deadline)
        {
            self.close(CloseReason::ProgressTimeout);
        }
    }

    /// Issues `request` at `now`. It is sent once a connection's handshake
    /// completes and the advertised in-flight limit has room. A request
    /// the negotiated version lacks answers `Unsupported`, one naming an
    /// identity of another scope `StaleTarget`, and one past a payload
    /// limit `LimitExceeded`, locally, without sending anything. A
    /// mutation or viewer request issued without a connection fails
    /// `Disconnected`.
    pub fn request(&mut self, request: Request, now: Duration) -> CallId {
        let viewer = request.viewer();
        let call = self.issue(request, false);
        if let Some(viewer) = viewer {
            // In order with the viewer's input and focus.
            self.queue.retain(|queued| *queued != call);
            if !self.queue_on_viewer(viewer, call) {
                let error = self.unknown_viewer(viewer);
                self.finish(call, Err(error));
            }
            self.flush_queue();
            self.refresh_progress(now);
            return call;
        }
        if self.connection.is_none()
            && self.calls[&call].kind.retry_class() != RetryClass::Query
        {
            self.queue.retain(|queued| *queued != call);
            self.finish(
                call,
                Err(WireError::new(
                    ErrorCode::Disconnected,
                    "the client is not connected",
                )),
            );
        }
        self.flush_queue();
        self.refresh_progress(now);
        call
    }

    /// Asks the server to abandon `call` if it has not started; a call not
    /// yet sent answers `Cancelled` at once, and a sent query whose
    /// connection ends before its result answers `Cancelled` instead of
    /// waiting for the next connection. The call still gets exactly one
    /// result.
    pub fn cancel(&mut self, call: CallId, now: Duration) {
        let Some(entry) = self.calls.get_mut(&call) else {
            return;
        };
        if let Some(request) = entry.sent {
            entry.cancelled = true;
            self.send_cancel(request);
        } else if let Some(index) =
            self.queue.iter().position(|queued| *queued == call)
        {
            self.queue.remove(index);
            self.finish(
                call,
                Err(WireError::new(
                    ErrorCode::Cancelled,
                    "cancelled before it was sent",
                )),
            );
        }
        self.refresh_progress(now);
    }

    /// Registers a waiter for hierarchy sequence `seq` on the current
    /// stream, reporting it at once when the projection already holds it,
    /// or cancelled when no connection has named a stream yet.
    pub fn wait_for(&mut self, seq: u64, now: Duration) -> WaitId {
        self.last_wait += 1;
        let wait = WaitId(self.last_wait);
        let resolution = match (&self.stream, &self.projection) {
            (None, _) => Some(Resolution::Cancelled),
            (_, Some(projection)) if projection.seq() >= seq => {
                Some(Resolution::Ready)
            }
            _ => None,
        };
        match resolution {
            Some(resolution) => {
                self.events
                    .push_back(ClientEvent::Waiter { wait, resolution });
            }
            None => self.waiters.insert(seq, wait),
        }
        self.refresh_progress(now);
        wait
    }

    /// The next thing to report, if any.
    pub fn poll_event(&mut self) -> Option<ClientEvent> {
        self.events.pop_front()
    }

    /// Whether a connection is open or opening.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.connection.is_some()
    }

    /// The open connection, once its handshake completed.
    #[must_use]
    pub fn connected(&self) -> Option<Connected> {
        self.connected
    }

    /// The open connection's `Welcome`.
    #[must_use]
    pub fn welcome(&self) -> Option<&Welcome> {
        Some(&self.connection.as_ref()?.open.as_ref()?.welcome)
    }

    /// The open connection's transport profile.
    #[must_use]
    pub fn profile(&self) -> Option<&TransportProfile> {
        self.welcome().map(|welcome| &welcome.profile)
    }

    /// The current stream: the last connection's incarnation in its
    /// client-local runtime.
    #[must_use]
    pub fn stream(&self) -> Option<StreamId> {
        self.stream
    }

    /// The projection's minor version 0 state.
    #[must_use]
    pub fn hierarchy(&self) -> Option<&HierarchyState> {
        self.projection.as_ref().map(Projection::state)
    }

    /// The projection's digest at the last negotiated minor version.
    #[must_use]
    pub fn hierarchy_digest(&self) -> Option<u64> {
        Some(self.projection.as_ref()?.digest(self.minor))
    }

    /// The test-only projection.
    #[cfg(any(test, feature = "test-minor"))]
    #[must_use]
    pub fn test_hierarchy(
        &self,
    ) -> Option<&huterm_wire::test_minor::TestHierarchy> {
        match self.projection.as_ref()? {
            Projection::Test(hierarchy) => Some(hierarchy),
            Projection::Base(_) => None,
        }
    }

    /// Requests sent on the open connection and not yet answered.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.connection
            .as_ref()
            .map_or(0, |connection| connection.sent.len())
    }

    /// Requests waiting to be sent.
    #[must_use]
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// The session's counts.
    #[must_use]
    pub fn stats(&self) -> SessionStats {
        self.stats
    }

    fn issue(&mut self, request: Request, internal: bool) -> CallId {
        self.last_call += 1;
        let call = CallId(self.last_call);
        let kind = request.kind();
        self.calls.insert(
            call,
            Call {
                kind,
                request: Some(request),
                internal,
                sent: None,
                cancelled: false,
            },
        );
        if internal {
            self.queue.push_front(call);
        } else {
            self.queue.push_back(call);
        }
        call
    }

    fn read_frames(&mut self, now: Duration) {
        loop {
            let Some(connection) = self.connection.as_mut() else {
                return;
            };
            let frame = match connection.decoder.next_frame() {
                Ok(Some(frame)) => frame,
                Ok(None) => return,
                Err(error) => {
                    self.close(CloseReason::Violation(error.to_string()));
                    return;
                }
            };
            if connection.progress_since.is_some() {
                connection.progress_since = Some(now);
            }
            match connection.reassembler.accept(frame, now) {
                Ok(None) => {}
                Ok(Some(assembled)) => self.message(&assembled),
                Err(error) => {
                    self.close(CloseReason::Violation(error.to_string()));
                    return;
                }
            }
        }
    }

    fn message(&mut self, assembled: &Assembled) {
        let Some(connection) = self.connection.as_ref() else {
            return;
        };
        let Some(open) = connection.open.as_ref() else {
            self.handshake(assembled);
            return;
        };
        let context = open.context;
        match assembled.kind {
            MessageKind::Result => {
                match decode_message(
                    &assembled.bytes,
                    Direction::ServerToClient,
                    context,
                ) {
                    Ok(Message::Result(result)) => self.result(result),
                    Ok(_) => self.violation("a result that does not decode"),
                    Err(error) => self.violation(&error.to_string()),
                }
            }
            MessageKind::HierarchySnapshot | MessageKind::HierarchyResync => {
                self.snapshot(&assembled.bytes, context);
            }
            MessageKind::HierarchyEvent => {
                self.event(&assembled.bytes, context);
            }
            MessageKind::LargeResult => {
                match decode_message(
                    &assembled.bytes,
                    Direction::ServerToClient,
                    context,
                ) {
                    Ok(Message::LargeResult(result)) => self.result(result),
                    Ok(_) => self.violation("a result that does not decode"),
                    Err(error) => self.violation(&error.to_string()),
                }
            }
            MessageKind::TerminalUpdate
            | MessageKind::ViewerStatus
            | MessageKind::InputAck
            | MessageKind::ViewerStopped
            | MessageKind::ViewerRevoked => {
                match decode_message(
                    &assembled.bytes,
                    Direction::ServerToClient,
                    context,
                ) {
                    Ok(message) => self.viewer_message(message),
                    Err(error) => self.violation(&error.to_string()),
                }
            }
            kind => self.violation(&format!("unexpected {kind:?}")),
        }
    }

    fn violation(&mut self, message: &str) {
        self.close(CloseReason::Violation(message.to_owned()));
    }

    fn handshake(&mut self, assembled: &Assembled) {
        let mut reader = Reader::new(&assembled.bytes);
        let decoded = reader.u64().and_then(|_| match assembled.kind {
            MessageKind::Welcome => decode_welcome(&mut reader).map(Ok),
            MessageKind::Reject => decode_reject(&mut reader).map(Err),
            _ => Err(huterm_wire::codec::DecodeError::UnknownKind(
                assembled.kind.number(),
            )),
        });
        match decoded.and_then(|decoded| reader.finish().map(|()| decoded)) {
            Ok(Ok(welcome)) => self.welcome_received(welcome),
            Ok(Err(reject)) => self.close(CloseReason::Rejected(reject)),
            Err(error) => self.violation(&format!("handshake: {error}")),
        }
    }

    fn welcome_received(&mut self, welcome: Welcome) {
        let negotiated = match accept_welcome(&self.config.hello, &welcome) {
            Ok(negotiated) => negotiated,
            Err(error) => {
                self.violation(&error.to_string());
                return;
            }
        };
        let (request_cap, cancel_cap) =
            match profile_caps(&welcome.profile, negotiated.caps) {
                Ok(caps) => caps,
                Err(message) => {
                    self.violation(message);
                    return;
                }
            };
        let incarnation = negotiated.incarnation;
        let runtime = if let Some(runtime) = self.runtimes.get(&incarnation) {
            *runtime
        } else {
            let Some(runtime) = RuntimeId::allocate() else {
                self.violation("client-local runtimes ran out");
                return;
            };
            self.runtimes.insert(incarnation, runtime);
            runtime
        };
        let stream = StreamId::remote(runtime, incarnation);
        let new_incarnation = self.stream != Some(stream);
        if new_incarnation {
            self.change_stream(stream);
        }
        self.minor = negotiated.minor;
        let caps = negotiated.caps;
        let max_in_flight =
            usize::try_from(welcome.profile.max_in_flight_requests)
                .unwrap_or(usize::MAX);
        let connected = Connected {
            incarnation,
            runtime,
            minor: negotiated.minor,
            new_incarnation,
        };
        let Some(connection) = self.connection.as_mut() else {
            return;
        };
        connection.decoder.set_limit(negotiated.max_frame);
        connection.reassembler.set_caps(
            caps,
            MAX_OPEN_STREAMS
                .saturating_mul(caps.largest(Direction::ServerToClient)),
        );
        connection.open = Some(Open {
            context: DecodeContext {
                hierarchy: HierarchyContext { stream },
                caps,
            },
            welcome,
            negotiated,
            max_in_flight,
            request_cap,
            cancel_cap,
        });
        self.connected = Some(connected);
        self.events.push_back(ClientEvent::Connected(connected));
        let subscribing = self.queue.iter().any(|call| {
            self.calls[call].kind == RequestKind::SubscribeHierarchy
        });
        if self.subscribed && !subscribing {
            self.issue(Request::SubscribeHierarchy, true);
        }
    }

    /// Moves to another incarnation's stream: identities from before are
    /// foreign, the old projection is gone, and every waiter is cancelled.
    fn change_stream(&mut self, stream: StreamId) {
        self.stream = Some(stream);
        self.projection = None;
        // A new incarnation's terminals are new: no baseline carries over.
        self.carried = Carried::default();
        self.checkpoints.clear();
        self.uncovered = None;
        let events = &mut self.events;
        self.waiters.cancel_all(|wait| {
            events.push_back(ClientEvent::Waiter {
                wait,
                resolution: Resolution::Cancelled,
            });
        });
    }

    fn result(&mut self, result: RequestResult) {
        let RequestResult { request, outcome } = result;
        let Some(call) = self
            .connection
            .as_mut()
            .and_then(|connection| connection.sent.remove(&request))
        else {
            self.violation("a result for no request in flight");
            return;
        };
        let Some(entry) = self.calls.get_mut(&call) else {
            return;
        };
        entry.sent = None;
        let kind = entry.kind;
        let committed = match &outcome {
            Ok(success) => success.committed,
            Err(error) => error.committed,
        };
        if let Some(Committed {
            seq,
            digest: Some(digest),
        }) = committed
        {
            self.checkpoint(seq, digest);
        }
        if kind == RequestKind::SubscribeHierarchy && outcome.is_ok() {
            self.subscribed = true;
        }
        match outcome {
            Ok(success)
                if matches!(
                    kind,
                    RequestKind::SubscribeHierarchy
                        | RequestKind::ResyncHierarchy
                ) =>
            {
                let Response::Hierarchy { seq, .. } = success.response else {
                    self.finish(call, Ok(success));
                    return;
                };
                self.held.push(Held { seq, call, success });
                self.release_held();
            }
            Ok(success) if kind == RequestKind::SubscribeTerminal => {
                self.finish(call, Ok(success.clone()));
                self.subscribed(call, &success);
            }
            outcome => {
                self.subscribing.remove(&call);
                self.finish(call, outcome);
            }
        }
    }

    /// Checks a structural result's digest against the projection when it
    /// holds exactly `seq`, keeps it until event `seq` applies when the
    /// projection is behind, and drops it when the projection passed it.
    fn checkpoint(&mut self, seq: u64, digest: u64) {
        match &self.projection {
            Some(projection) if projection.seq() == seq => {
                self.check_digest(digest);
            }
            Some(projection) if projection.seq() > seq => {}
            _ => {
                if let Some(dropped) = self.checkpoints.insert(seq, digest) {
                    self.stats.checkpoints_dropped += 1;
                    self.dropped(dropped);
                }
            }
        }
    }

    /// Owes the checkpoint dropped at `seq` a snapshot that covers it. The
    /// first drop since the last covering snapshot sends the resync; later
    /// ones only raise the sequence it must reach, so a flood of results
    /// sends one resync, not one per result.
    fn dropped(&mut self, seq: u64) {
        let first = self.uncovered.is_none();
        self.uncovered = Some(self.uncovered.map_or(seq, |owed| owed.max(seq)));
        if first {
            self.resync_uncovered();
        }
    }

    /// Sends a resync while a dropped checkpoint is uncovered, once a
    /// snapshot installed on this connection. Before that, the pending
    /// subscription's snapshot comes first. A resync sent after a drop
    /// covers it, because the server committed the dropped sequence before
    /// the client read its result; only a drop while a resync was already
    /// outstanding can outrun that resync's snapshot.
    fn resync_uncovered(&mut self) {
        let installed = self
            .connection
            .as_ref()
            .is_some_and(|connection| connection.installed);
        if installed && self.uncovered.is_some() {
            self.resync();
        }
    }

    fn check_digest(&mut self, digest: u64) {
        let Some(projection) = &self.projection else {
            return;
        };
        if projection.digest(self.minor) != digest {
            self.stats.digest_mismatches += 1;
            self.resync();
        }
    }

    /// Sends one `ResyncHierarchy` of the session's own, unless one is
    /// outstanding.
    fn resync(&mut self) {
        if self.resyncing || self.connection.is_none() {
            return;
        }
        self.resyncing = true;
        self.stats.resync_requests += 1;
        self.issue(Request::ResyncHierarchy, true);
        self.flush_queue();
    }

    fn snapshot(&mut self, bytes: &[u8], context: DecodeContext) {
        let (digest, projection) =
            match self.kind.decode_snapshot(bytes, context) {
                Ok(decoded) => decoded,
                Err(error) => {
                    self.violation(&format!("hierarchy snapshot: {error}"));
                    return;
                }
            };
        if projection.digest(self.minor) != digest {
            self.close(CloseReason::ContractFault(format!(
                "the snapshot at sequence {} disagrees with its digest",
                projection.seq()
            )));
            return;
        }
        let installed = self
            .connection
            .as_ref()
            .is_some_and(|connection| connection.installed);
        let seq = projection.seq();
        if installed
            && self
                .projection
                .as_ref()
                .is_some_and(|current| current.seq() > seq)
        {
            self.violation("a hierarchy snapshot behind the projection");
            return;
        }
        if let Some(connection) = self.connection.as_mut() {
            connection.installed = true;
        }
        self.projection = Some(projection);
        self.checkpoints.covered(seq);
        if self.uncovered.is_some_and(|owed| owed <= seq) {
            self.uncovered = None;
        }
        self.events.push_back(ClientEvent::Hierarchy(
            HierarchyUpdate::Installed { seq },
        ));
        self.advanced(seq);
        self.resync_uncovered();
    }

    fn event(&mut self, bytes: &[u8], context: DecodeContext) {
        let installed = self
            .connection
            .as_ref()
            .is_some_and(|connection| connection.installed);
        let sequenced = match self.kind.decode_event(bytes, context) {
            Ok(sequenced) => sequenced,
            Err(error) => {
                self.violation(&format!("hierarchy event: {error}"));
                return;
            }
        };
        let Some(projection) = self.projection.as_mut().filter(|_| installed)
        else {
            self.violation("a hierarchy event before its snapshot");
            return;
        };
        let seq = sequenced.seq;
        match projection.apply(seq, sequenced.event) {
            Outcome::Applied(touched) => {
                self.events.push_back(ClientEvent::Hierarchy(
                    HierarchyUpdate::Applied { seq, touched },
                ));
                if let Some(digest) = sequenced.digest {
                    self.check_digest(digest);
                }
                let mut due = Vec::new();
                self.checkpoints.reached(seq, |digest| due.push(digest));
                for digest in due {
                    self.check_digest(digest);
                }
                self.advanced(seq);
            }
            Outcome::Stale => {}
            Outcome::Resync => self.resync(),
        }
    }

    /// Releases what the projection reaching `seq` satisfies.
    fn advanced(&mut self, seq: u64) {
        let events = &mut self.events;
        self.waiters.release(seq, |wait| {
            events.push_back(ClientEvent::Waiter {
                wait,
                resolution: Resolution::Ready,
            });
        });
        self.release_held();
    }

    fn release_held(&mut self) {
        let Some(applied) = self.projection.as_ref().map(Projection::seq)
        else {
            return;
        };
        let mut index = 0;
        while index < self.held.len() {
            if self.held[index].seq <= applied {
                let Held { call, success, .. } = self.held.swap_remove(index);
                self.finish(call, Ok(success));
            } else {
                index += 1;
            }
        }
    }

    /// Resolves `call` with `outcome`: reported to the caller, or consumed
    /// for the session's own requests.
    fn finish(&mut self, call: CallId, outcome: Result<Success, WireError>) {
        let Some(entry) = self.calls.remove(&call) else {
            return;
        };
        if entry.internal {
            if entry.kind == RequestKind::ResyncHierarchy {
                self.resyncing = false;
                // A snapshot that installed while this resync was still
                // outstanding may have left a drop uncovered. A failed
                // resync asks for nothing more, so it cannot loop.
                if outcome.is_ok() {
                    self.resync_uncovered();
                }
            }
            return;
        }
        self.events.push_back(ClientEvent::Result { call, outcome });
    }

    /// Sends queued requests, then viewers' waiting messages, while the
    /// advertised in-flight limit has room.
    fn flush_queue(&mut self) {
        self.flush_requests();
        self.flush_viewers();
    }

    fn flush_requests(&mut self) {
        loop {
            let Some(connection) = self.connection.as_ref() else {
                return;
            };
            let Some(open) = connection.open.as_ref() else {
                return;
            };
            if connection.sent.len() >= open.max_in_flight {
                return;
            }
            let Some(call) = self.queue.pop_front() else {
                return;
            };
            if let Err(error) = self.send(call) {
                self.finish(call, Err(error));
            }
        }
    }

    /// Encodes and queues `call`, or returns its local refusal.
    fn send(&mut self, call: CallId) -> Result<(), WireError> {
        let (Some(entry), Some(connection)) =
            (self.calls.get_mut(&call), self.connection.as_mut())
        else {
            return Ok(());
        };
        let Some(open) = connection.open.as_ref() else {
            return Ok(());
        };
        let Some(request) = entry.request.take() else {
            return Ok(());
        };
        let Some(id) = connection.next_id else {
            return Err(WireError::new(
                ErrorCode::LimitExceeded,
                "request numbers ran out on this connection",
            ));
        };
        let message = RequestMessage { id, request };
        let encoded = encode_request_message(&message, open);
        let RequestMessage { request, .. } = message;
        let bytes = encoded.map_err(refusal)?;
        if entry.kind.retry_class() == RetryClass::Query {
            entry.request = Some(request);
        }
        entry.sent = Some(id);
        connection.sent.insert(id, call);
        connection.next_id = id.next();
        connection
            .outbound
            .push_back(Transmit::message(bytes, open.negotiated.max_frame));
        Ok(())
    }

    fn send_cancel(&mut self, request: RequestId) {
        let Some(connection) = self.connection.as_mut() else {
            return;
        };
        let Some(open) = connection.open.as_ref() else {
            return;
        };
        let mut w = Writer::new();
        w.u64(MessageKind::Cancel.number());
        if encode_cancel(&mut w, Cancel { request }).is_ok()
            && w.len() <= open.cancel_cap
        {
            connection.outbound.push_back(Transmit::message(
                w.into_bytes(),
                open.negotiated.max_frame,
            ));
        }
    }

    /// Arms the progress deadline while something is outstanding on an
    /// open connection, and disarms it otherwise.
    fn refresh_progress(&mut self, now: Duration) {
        let waiting = !self.held.is_empty() || !self.waiters.is_empty();
        let Some(connection) = self.connection.as_mut() else {
            return;
        };
        let outstanding = waiting || !connection.sent.is_empty();
        if connection.open.is_none() || !outstanding {
            connection.progress_since = None;
        } else if connection.progress_since.is_none() {
            connection.progress_since = Some(now);
        }
    }

    /// Ends the current connection for `reason` and applies the disconnect
    /// rules: mutations in flight resolve `OutcomeUnknown`, unsent ones and
    /// viewer requests `Disconnected`, queries the caller cancelled after
    /// sending them `Cancelled`, and other queries wait for the next
    /// connection in issue order. Nothing else is ever sent again.
    fn close(&mut self, reason: CloseReason) {
        if self.connection.take().is_none() {
            return;
        }
        self.connected = None;
        self.resyncing = false;
        self.carry_viewers();
        self.checkpoints.clear();
        self.uncovered = None;
        self.held.clear();
        self.queue.clear();
        let mut calls: Vec<CallId> = self.calls.keys().copied().collect();
        calls.sort_unstable();
        for call in calls {
            let entry = &self.calls[&call];
            let failure = match (entry.internal, entry.kind.retry_class()) {
                (true, _) => {
                    self.calls.remove(&call);
                    continue;
                }
                (false, RetryClass::Query) if entry.cancelled => {
                    Some(WireError::new(
                        ErrorCode::Cancelled,
                        "cancelled before the connection ended",
                    ))
                }
                (false, RetryClass::Query) => None,
                (false, RetryClass::Mutation) if entry.sent.is_some() => {
                    Some(WireError::new(
                        ErrorCode::OutcomeUnknown,
                        "the connection ended before the outcome was known",
                    ))
                }
                (false, RetryClass::Mutation) => Some(WireError::new(
                    ErrorCode::Disconnected,
                    "the connection ended before the request was sent",
                )),
                (false, RetryClass::Viewer) => Some(WireError::new(
                    ErrorCode::Disconnected,
                    "the viewer ended with its connection",
                )),
            };
            if let Some(error) = failure {
                self.finish(call, Err(error));
            } else {
                if let Some(entry) = self.calls.get_mut(&call) {
                    entry.sent = None;
                }
                self.queue.push_back(call);
            }
        }
        self.events.push_back(ClientEvent::Closed(reason));
    }
}

/// The caps the client holds itself to for requests and cancels: the
/// smaller of its own and the advertised ones, within the advertised
/// reassembly budget. A profile this client cannot honour is refused.
fn profile_caps(
    profile: &TransportProfile,
    caps: MessageCaps,
) -> Result<(usize, usize), &'static str> {
    if profile.suspend_grace != Duration::ZERO {
        return Err(
            "the server offers a suspend grace this client cannot resume",
        );
    }
    if profile.max_in_flight_requests == 0 {
        return Err("the server allows no requests in flight");
    }
    let budget =
        usize::try_from(profile.reassembly_budget).unwrap_or(usize::MAX);
    let cap = |kind: MessageKind| {
        let advertised = profile
            .client_caps
            .iter()
            .find(|(advertised, _)| *advertised == kind)
            .map(|(_, cap)| usize::try_from(*cap).unwrap_or(usize::MAX))?;
        Some(advertised.min(caps.cap(kind)?).min(budget))
    };
    match (cap(MessageKind::Request), cap(MessageKind::Cancel)) {
        (Some(request), Some(cancel)) => Ok((request, cancel)),
        _ => Err("the server advertises no cap for requests or cancels"),
    }
}

fn encode_request_message(
    message: &RequestMessage,
    open: &Open,
) -> Result<Vec<u8>, EncodeError> {
    if message.request.kind().since() > open.minor() {
        return Err(EncodeError::Unsupported);
    }
    let mut w = Writer::new();
    w.u64(MessageKind::Request.number());
    encode_request(&mut w, open.scope(), message, open.minor())?;
    if w.len() > open.request_cap {
        return Err(EncodeError::LimitExceeded { field: "request" });
    }
    Ok(w.into_bytes())
}

/// The local refusal for a request that does not encode.
fn refusal(error: EncodeError) -> WireError {
    let code = match error {
        EncodeError::LimitExceeded { .. } => ErrorCode::LimitExceeded,
        EncodeError::StaleTarget => ErrorCode::StaleTarget,
        EncodeError::Unsupported => ErrorCode::Unsupported,
        EncodeError::Invalid(_) => ErrorCode::InvalidArgument,
    };
    WireError::new(code, error.to_string())
}

#[cfg(test)]
mod tests;
