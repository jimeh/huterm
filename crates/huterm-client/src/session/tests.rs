//! Session tests against the real server's sans-IO `Connection`, joined in
//! memory with an injected clock.

use std::sync::{Arc, Mutex};

use huterm_core::Mux;
use huterm_protocol::{
    CellSize, DigestProfile, GridSize, ScrollbackLimit, SessionId, TabId,
    ViewerId,
};
use huterm_server::runtime::execute;
use huterm_server::test_feed::TestSource;
use huterm_server::test_support::{flush, offer, quiet_command, run};
use huterm_server::{Connection, Server, ServerConfig};
use huterm_wire::frame::{
    DEFAULT_MAX_FRAME, Frame, FrameEnd, PREAMBLE_MAGIC, preamble,
};
use huterm_wire::handshake::{ClientKind, ServerId, ServerOffer};
use huterm_wire::request::MAX_REQUEST_NAME_BYTES;
use huterm_wire::response::Selection;
use huterm_wire::test_minor::{TEST_MINOR, TestEvent};

use super::*;

const T0: Duration = Duration::ZERO;
const TICK: Duration = Duration::from_nanos(1);

/// The instant just before `deadline`.
fn just_before(deadline: Duration) -> Duration {
    deadline.checked_sub(TICK).expect("a deadline after zero")
}

fn config() -> ClientConfig {
    ClientConfig::local(ClientKind::Desktop, "test", "test")
}

/// A server over a fresh `Mux` at minor version 0.
fn fresh_server(config: ServerConfig) -> Arc<Server> {
    Arc::new(Server::new(
        Arc::new(Mutex::new(Mux::default())),
        offer(0),
        config,
    ))
}

/// A `Mux` holding one session, one workspace, and two quiet tabs, with
/// a test source that starts from its hierarchy.
fn two_tabs() -> (Arc<Mutex<Mux>>, TabId, TabId, TestSource) {
    let mux = Arc::new(Mutex::new(Mux::default()));
    let (first, second, state) = {
        let mut mux = mux.lock().unwrap();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let mut open =
            || mux.open_tab(workspace, &quiet_command()).unwrap().tab.id;
        let (first, second) = (open(), open());
        (first, second, mux.subscribe_hierarchy().0)
    };
    (mux, first, second, TestSource::new(state))
}

fn shut_down(server: &Server) {
    let _ = server.lock().shutdown();
}

/// Splits a server's bytes after its preamble into whole frames.
fn split_frames(mut bytes: &[u8]) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    while let Some(prefix) = bytes.first_chunk::<4>() {
        let total = 4 + u32::from_le_bytes(*prefix) as usize;
        frames.push(bytes[..total].to_vec());
        bytes = &bytes[total..];
    }
    frames
}

fn parse_frame(bytes: &[u8]) -> Frame {
    let mut decoder = FrameDecoder::new();
    decoder.set_limit(u32::MAX);
    decoder.push(bytes);
    let frame = decoder.next_frame().unwrap().expect("a whole frame");
    assert_eq!(frame.end, FrameEnd::Final, "a single-frame message");
    frame
}

fn frame_kind(bytes: &[u8]) -> MessageKind {
    let number = Reader::new(&parse_frame(bytes).payload).u64().unwrap();
    MessageKind::from_number(number).unwrap()
}

/// Every message the client's `bytes` carry, decoded in `runtime`'s scope.
fn client_messages(bytes: &[u8], runtime: RuntimeId) -> Vec<Message> {
    let mut bytes = bytes;
    if bytes.starts_with(&PREAMBLE_MAGIC) {
        bytes = &bytes[PREAMBLE_LEN..];
    }
    let mut decoder = FrameDecoder::new();
    decoder.set_limit(DEFAULT_MAX_FRAME);
    decoder.push(bytes);
    let mut reassembler = Reassembler::new(
        MessageCaps::for_minor(0),
        Direction::ClientToServer,
        usize::MAX,
    );
    let context = DecodeContext {
        hierarchy: HierarchyContext {
            stream: StreamId::new(runtime),
        },
        caps: MessageCaps::for_minor(0),
    };
    let mut messages = Vec::new();
    while let Some(frame) = decoder.next_frame().unwrap() {
        if let Some(assembled) = reassembler.accept(frame, T0).unwrap() {
            messages.push(
                decode_message(
                    &assembled.bytes,
                    Direction::ClientToServer,
                    context,
                )
                .unwrap(),
            );
        }
    }
    messages
}

/// A session and one server connection, joined in memory.
struct Pair {
    server: Arc<Server>,
    connection: Connection,
    session: ClientSession,
    now: Duration,
    log: Vec<ClientEvent>,
}

impl Pair {
    /// Connects `session` to `server` and completes the handshake.
    fn new(server: &Arc<Server>, session: ClientSession) -> Self {
        let mut pair = Self {
            server: Arc::clone(server),
            connection: server.connection(T0),
            session,
            now: T0,
            log: Vec::new(),
        };
        pair.session.connect(T0);
        pair.pump();
        pair
    }

    /// Breaks the transport, ends the server's side, and connects to
    /// `server` without pumping.
    fn reconnect_quietly(&mut self, server: &Arc<Server>) {
        self.session.disconnected(self.now);
        self.drain();
        self.end_server();
        self.server = Arc::clone(server);
        self.connection = server.connection(self.now);
        self.session.connect(self.now);
    }

    fn reconnect(&mut self, server: &Arc<Server>) {
        self.reconnect_quietly(server);
        self.pump();
    }

    /// Ends the server's connection and runs its cleanup.
    fn end_server(&mut self) {
        for work in self.connection.close(self.now) {
            let completion = execute(&self.server, work.job);
            self.connection.complete(work.id, completion, self.now);
        }
        run(&self.server, &mut self.connection, self.now);
    }

    /// Writes everything the session queued to the server.
    fn send(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();
        while let Some(transmit) = self.session.poll_transmit() {
            bytes.extend(transmit.to_bytes());
        }
        self.connection.receive(&bytes, self.now);
        bytes
    }

    fn run(&mut self) -> usize {
        run(&self.server, &mut self.connection, self.now)
    }

    /// The server's queued bytes, written in full.
    fn take(&mut self) -> Vec<u8> {
        self.connection.pump(self.now);
        flush(&mut self.connection, self.now)
    }

    fn deliver(&mut self) -> usize {
        let bytes = self.take();
        self.session.receive(&bytes, self.now);
        self.drain();
        bytes.len()
    }

    fn deliver_frame(&mut self, frame: &[u8]) {
        self.session.receive(frame, self.now);
        self.drain();
    }

    fn drain(&mut self) {
        while let Some(event) = self.session.poll_event() {
            self.log.push(event);
        }
    }

    /// Exchanges bytes and runs work until nothing moves, returning every
    /// byte the client sent.
    fn pump(&mut self) -> Vec<u8> {
        let mut all = Vec::new();
        loop {
            let sent = self.send();
            let ran = self.run();
            let delivered = self.deliver();
            if sent.is_empty() && ran == 0 && delivered == 0 {
                return all;
            }
            all.extend(sent);
        }
    }

    fn call(&mut self, request: Request) -> Result<Success, WireError> {
        let call = self.session.request(request, self.now);
        self.pump();
        self.outcome(call).expect("a result")
    }

    fn outcome(&self, call: CallId) -> Option<Result<Success, WireError>> {
        self.log.iter().find_map(|event| match event {
            ClientEvent::Result { call: c, outcome } if *c == call => {
                Some(outcome.clone())
            }
            _ => None,
        })
    }

    fn resolution(&self, wait: WaitId) -> Option<Resolution> {
        self.log.iter().find_map(|event| match event {
            ClientEvent::Waiter {
                wait: w,
                resolution,
            } if *w == wait => Some(*resolution),
            _ => None,
        })
    }

    fn closes(&self) -> Vec<&CloseReason> {
        self.log
            .iter()
            .filter_map(|event| match event {
                ClientEvent::Closed(reason) => Some(reason),
                _ => None,
            })
            .collect()
    }

    fn updates(&self, since: usize) -> Vec<&HierarchyUpdate> {
        self.log[since..]
            .iter()
            .filter_map(|event| match event {
                ClientEvent::Hierarchy(update) => Some(update),
                _ => None,
            })
            .collect()
    }

    fn runtime(&self) -> RuntimeId {
        self.session.stream().unwrap().runtime()
    }

    fn context(&self) -> DecodeContext {
        DecodeContext {
            hierarchy: HierarchyContext {
                stream: self.session.stream().unwrap(),
            },
            caps: MessageCaps::for_minor(
                self.session.connected().unwrap().minor,
            ),
        }
    }

    /// `frame`'s message, decoded as the client reads it.
    fn decode(&self, frame: &[u8]) -> (u32, Message) {
        let frame = parse_frame(frame);
        let message = decode_message(
            &frame.payload,
            Direction::ServerToClient,
            self.context(),
        )
        .unwrap();
        (frame.stream, message)
    }

    /// `frame`'s message, changed by `change` and framed again.
    fn rewrite(
        &self,
        frame: &[u8],
        change: impl FnOnce(&mut Message),
    ) -> Vec<u8> {
        let (stream, mut message) = self.decode(frame);
        change(&mut message);
        let context = self.context();
        let payload =
            encode_message(&message, context.hierarchy.scope(), context.caps)
                .unwrap();
        Frame {
            stream,
            end: FrameEnd::Final,
            payload,
        }
        .encode(DEFAULT_MAX_FRAME)
        .unwrap()
    }

    /// A subscribed pair with one session to rename, and the session.
    fn subscribed(server: &Arc<Server>) -> (Self, SessionId) {
        let mut pair = Self::new(server, ClientSession::new(config()));
        pair.call(Request::SubscribeHierarchy).unwrap();
        let Response::Session(session) = pair
            .call(Request::CreateSession { name: None })
            .unwrap()
            .response
        else {
            panic!("a created session");
        };
        (pair, session)
    }

    /// Renames `session` and returns the server's frames: the result and
    /// the event, in either order.
    fn rename_frames(
        &mut self,
        session: SessionId,
        name: &str,
    ) -> Vec<Vec<u8>> {
        self.session.request(
            Request::RenameSession {
                session,
                name: Some(name.into()),
            },
            self.now,
        );
        self.send();
        self.run();
        let frames = split_frames(&self.take());
        assert_eq!(frames.len(), 2, "a result and an event");
        frames
    }
}

/// The frame of `kind` among `frames`.
fn of_kind(frames: &[Vec<u8>], kind: MessageKind) -> Vec<u8> {
    frames
        .iter()
        .find(|frame| frame_kind(frame) == kind)
        .cloned()
        .unwrap()
}

/// Replaces a result's committed digest with a wrong one.
fn wrong_digest(message: &mut Message) {
    let Message::Result(result) = message else {
        panic!("a result");
    };
    let Ok(Success {
        committed: Some(Committed { digest, .. }),
        ..
    }) = &mut result.outcome
    else {
        panic!("a committed success");
    };
    *digest = digest.map(|digest| digest ^ 1);
}

#[test]
fn a_test_minor_server_reaches_a_minor_0_client_through_each_downgrade_path() {
    let (mux, first, second, source) = two_tabs();
    let server = Arc::new(
        Server::new(mux, offer(TEST_MINOR), ServerConfig::default())
            .with_hierarchy_source(Arc::new(source.clone())),
    );
    let mut pair = Pair::new(&server, ClientSession::new(config()));
    assert_eq!(pair.session.connected().unwrap().minor, 0);
    pair.call(Request::SubscribeHierarchy).unwrap();
    let start = pair.session.hierarchy().unwrap().seq();

    let events = [
        TestEvent::TabPinned {
            tab: first,
            pinned: true,
        },
        TestEvent::TabRetitled {
            tab: second,
            custom_name: Some("retitled".into()),
        },
        TestEvent::TabsExchanged { first, second },
    ];
    for (offset, event) in (1..).zip(&events) {
        let seq = start + offset;
        let wait = pair.session.wait_for(seq, pair.now);
        let mark = pair.log.len();
        assert_eq!(source.emit(event), Some(seq));
        pair.deliver();
        // `Skipped` and the renamed tab apply as events; the exchange
        // arrives as a projected resync.
        let updates = pair.updates(mark);
        let delivered = match updates.as_slice() {
            [HierarchyUpdate::Applied { seq: applied, .. }] => {
                assert_eq!(*applied, seq);
                "event"
            }
            [HierarchyUpdate::Installed { seq: installed }] => {
                assert_eq!(*installed, seq);
                "resync"
            }
            other => panic!("one update per event, got {other:?}"),
        };
        assert_eq!(delivered, if offset < 3 { "event" } else { "resync" });
        assert_eq!(pair.resolution(wait), Some(Resolution::Ready));
    }
    assert_eq!(
        pair.session.hierarchy_digest(),
        Some(source.hierarchy().digest(0))
    );
    assert_eq!(pair.session.stats(), SessionStats::default());
    assert!(pair.closes().is_empty(), "{:?}", pair.closes());
    shut_down(&server);
}

#[test]
fn a_test_minor_client_withholds_newer_requests_and_keeps_read_your_writes_at_minor_0()
 {
    let server = fresh_server(ServerConfig::default());
    let mut pair =
        Pair::new(&server, ClientSession::with_test_projection(config()));
    assert_eq!(pair.session.connected().unwrap().minor, 0);
    pair.call(Request::SubscribeHierarchy).unwrap();
    let Response::Session(session) = pair
        .call(Request::CreateSession { name: None })
        .unwrap()
        .response
    else {
        panic!("a created session");
    };
    let Response::Workspace(workspace) = pair
        .call(Request::CreateWorkspace {
            session,
            name: None,
        })
        .unwrap()
        .response
    else {
        panic!("a created workspace");
    };
    let Response::OpenedTab(opened) = pair
        .call(Request::OpenTab {
            workspace,
            command: Box::new(quiet_command()),
        })
        .unwrap()
        .response
    else {
        panic!("an opened tab");
    };

    // The test-only request never reaches a minor version 0 server.
    let pin = pair.session.request(
        Request::PinTab {
            tab: opened.tab,
            pinned: true,
        },
        pair.now,
    );
    assert!(pair.session.poll_transmit().is_none());
    pair.drain();
    assert_eq!(
        pair.outcome(pin).unwrap().unwrap_err().code,
        ErrorCode::Unsupported
    );

    // A structural result's sequence resolves a waiter, and its digest
    // matches the minor version 0 profile.
    let renamed = pair
        .call(Request::RenameTab {
            tab: opened.tab,
            name: Some("renamed".into()),
        })
        .unwrap();
    let committed = renamed.committed.unwrap();
    let wait = pair.session.wait_for(committed.seq, pair.now);
    pair.drain();
    assert_eq!(pair.resolution(wait), Some(Resolution::Ready));
    assert_eq!(pair.session.hierarchy_digest(), committed.digest);
    assert_eq!(
        committed.digest,
        Some(server.lock().hierarchy_digest(DigestProfile::Base))
    );
    assert_eq!(pair.session.stats(), SessionStats::default());
    assert_eq!(pair.closes(), Vec::<&CloseReason>::new());
    assert_eq!(pair.connection.in_flight(), 0);
    shut_down(&server);
}

#[test]
fn a_new_incarnation_makes_old_ids_foreign_and_cancels_waiters() {
    let first = fresh_server(ServerConfig::default());
    let (mut pair, session) = Pair::subscribed(&first);
    let old_runtime = pair.runtime();
    let ahead = pair.session.hierarchy().unwrap().seq() + 100;
    let wait = pair.session.wait_for(ahead, pair.now);
    pair.drain();
    assert_eq!(pair.resolution(wait), None);

    let second = Arc::new(Server::new(
        Arc::new(Mutex::new(Mux::default())),
        ServerOffer {
            incarnation: ServerIncarnation::from_bytes([8; 16]),
            server_id: ServerId([10; 16]),
            ..offer(0)
        },
        ServerConfig::default(),
    ));
    pair.reconnect(&second);
    let connected = pair.session.connected().unwrap();
    assert!(connected.new_incarnation);
    assert_ne!(connected.runtime, old_runtime);
    assert_eq!(pair.resolution(wait), Some(Resolution::Cancelled));

    let select = pair
        .session
        .request(Request::SelectSession { session }, pair.now);
    assert!(pair.session.poll_transmit().is_none());
    pair.drain();
    assert_eq!(
        pair.outcome(select).unwrap().unwrap_err().code,
        ErrorCode::StaleTarget
    );
    shut_down(&first);
    shut_down(&second);
}

#[test]
fn the_same_incarnation_at_another_minor_keeps_ids_and_waiters_and_takes_the_new_profile()
 {
    let (mux, first, second, source) = two_tabs();
    let at_test_minor = Arc::new(
        Server::new(
            Arc::clone(&mux),
            offer(TEST_MINOR),
            ServerConfig::default(),
        )
        .with_hierarchy_source(Arc::new(source.clone())),
    );
    let mut pair = Pair::new(
        &at_test_minor,
        ClientSession::with_test_projection(config()),
    );
    assert_eq!(pair.session.connected().unwrap().minor, TEST_MINOR);
    pair.call(Request::SubscribeHierarchy).unwrap();
    let pinned = source
        .emit(&TestEvent::TabPinned {
            tab: first,
            pinned: true,
        })
        .unwrap();
    pair.deliver();
    let hierarchy = pair.session.test_hierarchy().unwrap();
    assert_eq!(hierarchy.state.seq(), pinned);
    assert_eq!(hierarchy.pinned.len(), 1);
    // The pinned tab makes the test-only digest differ from minor 0's.
    assert_ne!(
        pair.session.hierarchy_digest(),
        Some(hierarchy.state.digest())
    );
    let runtime = pair.runtime();
    let wait = pair.session.wait_for(pinned + 1, pair.now);

    // A handover: the same incarnation offers minor version 0 and fewer
    // requests in flight.
    let at_minor_0 = Arc::new(
        Server::new(
            Arc::clone(&mux),
            offer(0),
            ServerConfig {
                max_in_flight_requests: 2,
                ..ServerConfig::default()
            },
        )
        .with_hierarchy_source(Arc::new(source.clone())),
    );
    pair.reconnect(&at_minor_0);
    let connected = pair.session.connected().unwrap();
    assert!(!connected.new_incarnation);
    assert_eq!(connected.runtime, runtime);
    assert_eq!(connected.minor, 0);
    assert_eq!(pair.session.profile().unwrap().max_in_flight_requests, 2);
    assert_eq!(pair.resolution(wait), None, "the waiter stays live");
    assert_eq!(
        pair.session.hierarchy_digest(),
        Some(source.hierarchy().digest(0))
    );

    // Identities from before still name the same tabs.
    let tab = TabId::in_runtime(runtime, first.get());
    let selected = pair.call(Request::SelectTab { tab }).unwrap();
    assert!(matches!(
        selected.response,
        Response::Selection(Selection { tab: Some(t), .. }) if t == tab
    ));

    // The waiter resolves when the stream reaches its sequence.
    source
        .emit(&TestEvent::TabRetitled {
            tab: second,
            custom_name: Some("retitled".into()),
        })
        .unwrap();
    pair.deliver();
    assert_eq!(pair.resolution(wait), Some(Resolution::Ready));

    // Requests past the new limit wait in the client.
    let calls: Vec<_> = (0..3)
        .map(|_| pair.session.request(Request::SelectTab { tab }, pair.now))
        .collect();
    pair.send();
    assert_eq!(pair.connection.in_flight(), 2);
    assert_eq!((pair.session.in_flight(), pair.session.queued()), (2, 1));
    pair.pump();
    for call in calls {
        assert!(pair.outcome(call).unwrap().is_ok());
    }
    assert_eq!(pair.session.stats(), SessionStats::default());
    assert!(
        pair.closes()
            .iter()
            .all(|reason| **reason == CloseReason::Disconnected),
        "{:?}",
        pair.closes()
    );
    let _ = mux.lock().unwrap().shutdown();
}

#[test]
fn requests_in_flight_at_disconnect_follow_their_retry_class() {
    let server = fresh_server(ServerConfig::default());
    let mut pair = Pair::new(&server, ClientSession::new(config()));
    let Response::Session(session) = pair
        .call(Request::CreateSession { name: None })
        .unwrap()
        .response
    else {
        panic!("a created session");
    };
    let runtime = pair.runtime();
    let viewer = ViewerId::in_runtime(runtime, 1);
    let rename = pair.session.request(
        Request::RenameSession {
            session,
            name: Some("renamed".into()),
        },
        pair.now,
    );
    let scrollback = pair.session.request(
        Request::SetScrollback {
            viewer,
            limit: ScrollbackLimit::DEFAULT,
        },
        pair.now,
    );
    let select = pair
        .session
        .request(Request::SelectSession { session }, pair.now);
    let geometry = pair.session.request(
        Request::ReportGeometry {
            viewer,
            grid: GridSize::clamped(80, 24),
            cell: CellSize {
                width: 8,
                height: 16,
            },
        },
        pair.now,
    );
    // The server receives all four and runs none before the break.
    pair.send();
    assert_eq!(pair.connection.in_flight(), 4);

    pair.session.disconnected(pair.now);
    pair.drain();
    let code =
        |pair: &Pair, call| pair.outcome(call).unwrap().unwrap_err().code;
    assert_eq!(code(&pair, rename), ErrorCode::OutcomeUnknown);
    assert_eq!(code(&pair, scrollback), ErrorCode::OutcomeUnknown);
    assert_eq!(code(&pair, geometry), ErrorCode::Disconnected);
    assert_eq!(pair.outcome(select), None, "the query waits");

    pair.reconnect_quietly(&server);
    let sent = pair.pump();
    let requests: Vec<_> = client_messages(&sent, runtime)
        .into_iter()
        .filter_map(|message| match message {
            Message::Request(request) => Some(request),
            _ => None,
        })
        .collect();
    assert_eq!(
        requests,
        [RequestMessage {
            id: RequestId::FIRST,
            request: Request::SelectSession { session },
        }]
    );
    assert!(matches!(
        pair.outcome(select).unwrap().unwrap().response,
        Response::Selection(Selection { session: s, .. }) if s == session
    ));
    shut_down(&server);
}

#[test]
fn the_progress_deadline_fires_only_while_something_is_outstanding() {
    let server = fresh_server(ServerConfig::default());
    let deadline = Duration::from_secs(10);
    let mut pair = Pair::new(
        &server,
        ClientSession::new(ClientConfig {
            progress_deadline: deadline,
            ..config()
        }),
    );
    assert_eq!(pair.session.next_deadline(), None, "idle arms nothing");

    pair.now = Duration::from_secs(1);
    let create = pair
        .session
        .request(Request::CreateSession { name: None }, pair.now);
    pair.send();
    let fires = pair.now + deadline;
    assert_eq!(pair.session.next_deadline(), Some(fires));
    pair.session.tick(just_before(fires));
    assert!(pair.session.is_connected());
    pair.session.tick(fires);
    pair.drain();
    assert_eq!(pair.closes(), [&CloseReason::ProgressTimeout]);
    assert_eq!(
        pair.outcome(create).unwrap().unwrap_err().code,
        ErrorCode::OutcomeUnknown
    );
    shut_down(&server);
}

#[test]
fn an_inbound_frame_restarts_the_progress_deadline() {
    let server = fresh_server(ServerConfig::default());
    let deadline = Duration::from_secs(10);
    let mut pair = Pair::new(
        &server,
        ClientSession::new(ClientConfig {
            progress_deadline: deadline,
            ..config()
        }),
    );
    pair.now = Duration::from_secs(1);
    pair.session
        .request(Request::CreateSession { name: None }, pair.now);
    pair.session
        .request(Request::CreateSession { name: None }, pair.now);
    pair.send();
    pair.run();
    let frames = split_frames(&pair.take());
    assert_eq!(frames.len(), 2);
    pair.now = Duration::from_secs(5);
    pair.deliver_frame(&frames[0]);
    assert_eq!(pair.session.next_deadline(), Some(pair.now + deadline));
    pair.deliver_frame(&frames[1]);
    assert_eq!(pair.session.next_deadline(), None);
    shut_down(&server);
}

#[test]
fn a_result_ahead_of_its_event_is_checked_when_the_event_applies() {
    let server = fresh_server(ServerConfig::default());
    let (mut pair, session) = Pair::subscribed(&server);

    let frames = pair.rename_frames(session, "first");
    pair.deliver_frame(&of_kind(&frames, MessageKind::Result));
    pair.deliver_frame(&of_kind(&frames, MessageKind::HierarchyEvent));
    assert_eq!(pair.session.stats(), SessionStats::default());

    let frames = pair.rename_frames(session, "second");
    let result = of_kind(&frames, MessageKind::Result);
    pair.deliver_frame(&pair.rewrite(&result, wrong_digest));
    assert_eq!(pair.session.stats().digest_mismatches, 0, "not yet checked");
    pair.deliver_frame(&of_kind(&frames, MessageKind::HierarchyEvent));
    assert_eq!(
        pair.session.stats(),
        SessionStats {
            resync_requests: 1,
            digest_mismatches: 1,
            ..SessionStats::default()
        }
    );
    shut_down(&server);
}

#[test]
fn a_result_behind_its_event_is_checked_at_once() {
    let server = fresh_server(ServerConfig::default());
    let (mut pair, session) = Pair::subscribed(&server);

    let frames = pair.rename_frames(session, "first");
    pair.deliver_frame(&of_kind(&frames, MessageKind::HierarchyEvent));
    pair.deliver_frame(&of_kind(&frames, MessageKind::Result));
    assert_eq!(pair.session.stats(), SessionStats::default());

    let frames = pair.rename_frames(session, "second");
    pair.deliver_frame(&of_kind(&frames, MessageKind::HierarchyEvent));
    let result = of_kind(&frames, MessageKind::Result);
    pair.deliver_frame(&pair.rewrite(&result, wrong_digest));
    assert_eq!(
        pair.session.stats(),
        SessionStats {
            resync_requests: 1,
            digest_mismatches: 1,
            ..SessionStats::default()
        }
    );
    shut_down(&server);
}

#[test]
fn a_checkpoint_a_resync_covered_is_dropped_unchecked() {
    let server = fresh_server(ServerConfig::default());
    let (mut pair, session) = Pair::subscribed(&server);
    let frames = pair.rename_frames(session, "renamed");
    let result = of_kind(&frames, MessageKind::Result);
    let (_, Message::Result(RequestResult { outcome, .. })) =
        pair.decode(&result)
    else {
        panic!("a result");
    };
    let committed = outcome.unwrap().committed.unwrap();
    // The result arrives ahead of its event, with a digest no state at its
    // sequence has, and the event never does: a lagging server replaces it
    // with a resync snapshot.
    pair.deliver_frame(&pair.rewrite(&result, wrong_digest));
    let resync = pair.session.request(Request::ResyncHierarchy, pair.now);
    pair.send();
    pair.run();
    let frames = split_frames(&pair.take());
    let snapshot = of_kind(&frames, MessageKind::HierarchyResync);
    let mark = pair.log.len();
    pair.deliver_frame(&snapshot);
    assert_eq!(
        pair.updates(mark),
        [&HierarchyUpdate::Installed { seq: committed.seq }]
    );
    pair.deliver_frame(&of_kind(&frames, MessageKind::Result));
    assert!(pair.outcome(resync).unwrap().is_ok());
    assert_eq!(pair.session.stats(), SessionStats::default());
    shut_down(&server);
}

#[test]
fn a_committed_failure_digest_is_checked_like_a_success() {
    let server = fresh_server(ServerConfig::default());
    let (mut pair, session) = Pair::subscribed(&server);
    // A close whose cleanup failed after it committed reports its sequence
    // and digest on the failure; a rename's result stands in for it.
    let as_failure = |flip: u64| {
        move |message: &mut Message| {
            let Message::Result(result) = message else {
                panic!("a result");
            };
            let committed = result.outcome.as_ref().unwrap().committed.unwrap();
            result.outcome = Err(WireError {
                committed: Some(Committed {
                    digest: committed.digest.map(|digest| digest ^ flip),
                    ..committed
                }),
                ..WireError::new(ErrorCode::Internal, "cleanup failed")
            });
        }
    };
    for (flip, expected) in [(0, 0), (1, 1)] {
        let frames = pair.rename_frames(session, &format!("name {flip}"));
        pair.deliver_frame(&of_kind(&frames, MessageKind::HierarchyEvent));
        let result = of_kind(&frames, MessageKind::Result);
        pair.deliver_frame(&pair.rewrite(&result, as_failure(flip)));
        assert_eq!(pair.session.stats().digest_mismatches, expected);
    }
    shut_down(&server);
}

#[test]
fn a_snapshot_whose_digest_disagrees_is_a_contract_fault() {
    let server = fresh_server(ServerConfig::default());
    let mut pair = Pair::new(&server, ClientSession::new(config()));
    pair.session.request(Request::SubscribeHierarchy, pair.now);
    pair.send();
    pair.run();
    let frames = split_frames(&pair.take());
    let snapshot = of_kind(&frames, MessageKind::HierarchySnapshot);
    pair.deliver_frame(&pair.rewrite(&snapshot, |message| {
        let Message::HierarchySnapshot(snapshot) = message else {
            panic!("a snapshot");
        };
        snapshot.digest ^= 1;
    }));
    assert!(matches!(
        pair.closes().as_slice(),
        [CloseReason::ContractFault(_)]
    ));
    assert_eq!(pair.session.stats().resync_requests, 0);
    assert!(!pair.session.is_connected());
    shut_down(&server);
}

#[test]
fn a_snapshot_behind_the_projection_on_one_connection_is_a_violation() {
    let server = fresh_server(ServerConfig::default());
    let mut pair = Pair::new(&server, ClientSession::new(config()));
    pair.session.request(Request::SubscribeHierarchy, pair.now);
    pair.send();
    pair.run();
    let frames = split_frames(&pair.take());
    let snapshot = of_kind(&frames, MessageKind::HierarchySnapshot);
    for frame in &frames {
        pair.deliver_frame(frame);
    }
    pair.call(Request::CreateSession { name: None }).unwrap();
    assert_eq!(pair.closes(), Vec::<&CloseReason>::new());
    pair.deliver_frame(&snapshot);
    assert!(matches!(
        pair.closes().as_slice(),
        [CloseReason::Violation(message)] if message.contains("behind")
    ));
    shut_down(&server);
}

#[test]
fn a_gap_sends_one_resync_whose_snapshot_restores_the_projection() {
    let server = fresh_server(ServerConfig::default());
    let (mut pair, session) = Pair::subscribed(&server);
    let mut events = Vec::new();
    for name in ["a", "b", "c"] {
        let frames = pair.rename_frames(session, name);
        events.push(of_kind(&frames, MessageKind::HierarchyEvent));
        pair.deliver_frame(&of_kind(&frames, MessageKind::Result));
    }
    // The first event is lost: the second leaves a gap, and the third
    // asks for nothing more.
    pair.deliver_frame(&events[1]);
    pair.deliver_frame(&events[2]);
    assert_eq!(pair.session.stats().resync_requests, 1);
    pair.pump();
    assert_eq!(
        pair.session.hierarchy_digest(),
        Some(server.lock().hierarchy_digest(DigestProfile::Base))
    );
    assert_eq!(pair.session.stats().resync_requests, 1);
    assert!(!pair.session.resyncing, "its result resolved");
    shut_down(&server);
}

#[test]
fn the_handshake_deadline_ends_a_silent_server() {
    let mut session = ClientSession::new(config());
    session.connect(T0);
    let deadline = config().handshake_deadline;
    assert_eq!(session.next_deadline(), Some(deadline));
    session.tick(just_before(deadline));
    assert!(session.is_connected());
    session.tick(deadline);
    assert_eq!(
        session.poll_event(),
        Some(ClientEvent::Closed(CloseReason::HandshakeTimeout))
    );
}

#[test]
fn a_request_past_a_payload_limit_is_refused_locally() {
    let server = fresh_server(ServerConfig::default());
    let mut pair = Pair::new(&server, ClientSession::new(config()));
    let session = SessionId::in_runtime(pair.runtime(), 1);
    let rename = pair.session.request(
        Request::RenameSession {
            session,
            name: Some("x".repeat(MAX_REQUEST_NAME_BYTES + 1)),
        },
        pair.now,
    );
    assert!(pair.session.poll_transmit().is_none());
    pair.drain();
    assert_eq!(
        pair.outcome(rename).unwrap().unwrap_err().code,
        ErrorCode::LimitExceeded
    );
    shut_down(&server);
}

#[test]
fn a_profile_the_client_cannot_honour_is_refused() {
    let welcome = Welcome {
        major: huterm_wire::MAJOR,
        minor: 0,
        features: std::collections::BTreeSet::new(),
        incarnation: ServerIncarnation::from_bytes([7; 16]),
        server_id: ServerId([9; 16]),
        host_name: "test-host".into(),
        socket_name: "test".into(),
        build_version: "test".into(),
        process_id: 1,
        max_frame: DEFAULT_MAX_FRAME,
        profile: TransportProfile {
            suspend_grace: Duration::from_secs(1),
            ..TransportProfile::local(0)
        },
    };
    let mut bytes = preamble().to_vec();
    bytes.extend(
        Frame {
            stream: 0,
            end: FrameEnd::Final,
            payload: encode_message(
                &Message::Welcome(welcome),
                IdScope::new(RuntimeId::new(0)),
                MessageCaps::for_minor(0),
            )
            .unwrap(),
        }
        .encode(HANDSHAKE_FRAME_LIMIT)
        .unwrap(),
    );
    let mut session = ClientSession::new(config());
    session.connect(T0);
    session.receive(&bytes, T0);
    assert!(matches!(
        session.poll_event(),
        Some(ClientEvent::Closed(CloseReason::Violation(message)))
            if message.contains("suspend")
    ));
    assert!(session.connected().is_none());
}

#[test]
fn cancelling_answers_an_unsent_request_locally_and_asks_the_server_for_a_sent_one()
 {
    let server = fresh_server(ServerConfig {
        max_in_flight_requests: 1,
        ..ServerConfig::default()
    });
    let mut pair = Pair::new(&server, ClientSession::new(config()));
    let sent = pair
        .session
        .request(Request::CreateSession { name: None }, pair.now);
    let queued = pair
        .session
        .request(Request::CreateSession { name: None }, pair.now);
    let before = server.lock().hierarchy_seq();
    pair.session.cancel(queued, pair.now);
    pair.session.cancel(sent, pair.now);
    pair.drain();
    assert_eq!(
        pair.outcome(queued).unwrap().unwrap_err().code,
        ErrorCode::Cancelled
    );
    pair.pump();
    assert_eq!(
        pair.outcome(sent).unwrap().unwrap_err().code,
        ErrorCode::Cancelled
    );
    assert_eq!(server.lock().hierarchy_seq(), before, "nothing committed");
    shut_down(&server);
}

/// The requests among the bytes a client sent.
fn sent_requests(bytes: &[u8], runtime: RuntimeId) -> Vec<Request> {
    client_messages(bytes, runtime)
        .into_iter()
        .filter_map(|message| match message {
            Message::Request(message) => Some(message.request),
            _ => None,
        })
        .collect()
}

#[test]
fn a_sent_query_cancelled_before_a_disconnect_resolves_cancelled_and_is_not_resent()
 {
    let server = fresh_server(ServerConfig::default());
    let mut pair = Pair::new(&server, ClientSession::new(config()));
    let runtime = pair.runtime();
    let session = SessionId::in_runtime(runtime, 1);
    let select = pair
        .session
        .request(Request::SelectSession { session }, pair.now);
    pair.send();
    pair.session.cancel(select, pair.now);
    pair.session.disconnected(pair.now);
    pair.drain();
    assert_eq!(
        pair.outcome(select)
            .map(|outcome| outcome.unwrap_err().code),
        Some(ErrorCode::Cancelled)
    );
    pair.reconnect_quietly(&server);
    let sent = pair.pump();
    assert_eq!(sent_requests(&sent, runtime), []);
    shut_down(&server);
}

#[test]
fn only_a_subscribe_that_succeeded_makes_later_connections_subscribe() {
    let server = fresh_server(ServerConfig::default());
    let mut session = ClientSession::new(config());
    session.connect(T0);
    // Issued before the handshake completes, so it waits unsent.
    let subscribe = session.request(Request::SubscribeHierarchy, T0);
    session.cancel(subscribe, T0);
    let mut pair = Pair {
        server: Arc::clone(&server),
        connection: server.connection(T0),
        session,
        now: T0,
        log: Vec::new(),
    };
    let sent = pair.pump();
    let runtime = pair.runtime();
    assert_eq!(
        pair.outcome(subscribe)
            .map(|outcome| outcome.unwrap_err().code),
        Some(ErrorCode::Cancelled)
    );
    assert_eq!(sent_requests(&sent, runtime), []);
    pair.reconnect_quietly(&server);
    let sent = pair.pump();
    assert_eq!(sent_requests(&sent, runtime), []);

    pair.call(Request::SubscribeHierarchy).unwrap();
    pair.reconnect_quietly(&server);
    let sent = pair.pump();
    assert_eq!(sent_requests(&sent, runtime), [Request::SubscribeHierarchy]);
    shut_down(&server);
}

#[test]
fn past_the_checkpoint_bound_the_oldest_is_dropped_and_the_rest_still_check() {
    let server = fresh_server(ServerConfig::default());
    let (mut pair, session) = Pair::subscribed(&server);
    // The server answers every rename while its events wait.
    let renames = MAX_PENDING_CHECKPOINTS + 1;
    let mut events = Vec::with_capacity(renames);
    for index in 0..renames {
        let frames = pair.rename_frames(session, &format!("name {index}"));
        let mut result = of_kind(&frames, MessageKind::Result);
        if index == 0 || index == renames - 1 {
            result = pair.rewrite(&result, wrong_digest);
        }
        pair.deliver_frame(&result);
        assert!(pair.session.checkpoints.len() <= MAX_PENDING_CHECKPOINTS);
        events.push(of_kind(&frames, MessageKind::HierarchyEvent));
    }
    assert_eq!(pair.session.checkpoints.len(), MAX_PENDING_CHECKPOINTS);
    assert_eq!(pair.session.stats().checkpoints_dropped, 1);

    for event in &events {
        pair.deliver_frame(event);
    }
    // The first rename's wrong digest was dropped unchecked; the last
    // one's is checked when its event applies.
    assert_eq!(pair.session.stats().digest_mismatches, 1);
    assert_eq!(pair.session.checkpoints.len(), 0);
    shut_down(&server);
}
