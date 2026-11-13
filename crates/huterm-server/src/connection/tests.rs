use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use huterm_core::Mux;
use huterm_protocol::{
    ApplyOutcome, HierarchyEnvelope, HierarchyState, PaneId, PaneNode,
    SPLIT_TOTAL, SessionId, SplitDirection, TabId, WorkspaceId,
};
use huterm_wire::codec::Writer;
use huterm_wire::error::Retry;
use huterm_wire::frame::{Frame, FrameEnd};
use huterm_wire::handshake::{ClientKind, Hello, RejectReason};
use huterm_wire::request::RestoreDirection as Way;
use huterm_wire::test_minor::TEST_MINOR;

use super::*;
use crate::config::{
    HIERARCHY_LANE_BUDGET, MAX_CLOSE_TICKETS, MAX_HISTORY_SLOTS,
    OUTBOUND_BUDGET,
};
use crate::runtime::execute;
use crate::server::Server;
use crate::test_support::{
    TestPeer, command, flush, frames, offer, quiet_command, run, server,
};

const T0: Duration = Duration::ZERO;
const TICK: Duration = Duration::from_nanos(1);

/// The instant just before `deadline`.
fn just_before(deadline: Duration) -> Duration {
    deadline.checked_sub(TICK).expect("a deadline after zero")
}

/// A hierarchy projection that follows what a client receives.
#[derive(Debug, Default)]
struct Follower {
    state: Option<HierarchyState>,
    resyncs: usize,
}

impl Follower {
    fn follow(&mut self, messages: &[Message]) {
        for message in messages {
            match message {
                Message::HierarchySnapshot(snapshot)
                | Message::HierarchyResync(snapshot) => {
                    assert_eq!(snapshot.state.digest(), snapshot.digest);
                    if let Some(state) = &self.state {
                        assert!(snapshot.state.seq() >= state.seq());
                    }
                    if matches!(message, Message::HierarchyResync(_)) {
                        self.resyncs += 1;
                    }
                    self.state = Some(snapshot.state.clone());
                }
                Message::HierarchyEvent(event) => {
                    let state = self.state.as_mut().expect("a snapshot first");
                    let outcome = state.apply(HierarchyEnvelope {
                        stream: state.stream(),
                        seq: event.seq,
                        event: event.event.clone(),
                    });
                    assert!(
                        matches!(outcome, ApplyOutcome::Applied(_)),
                        "{event:?} gave {outcome:?}"
                    );
                    if let Some(digest) = event.digest {
                        assert_eq!(state.digest(), digest);
                    }
                }
                _ => {}
            }
        }
    }

    fn state(&self) -> &HierarchyState {
        self.state.as_ref().expect("a snapshot")
    }
}

/// One client: its server-side connection and its raw peer.
struct Client {
    connection: Connection,
    peer: TestPeer,
    inbox: Vec<Message>,
    follower: Follower,
    now: Duration,
}

impl Client {
    fn connect(server: &Server, now: Duration) -> Self {
        Self::connect_with(server, TestPeer::new(server.runtime()), now)
    }

    fn connect_with(server: &Server, peer: TestPeer, now: Duration) -> Self {
        let mut client = Self {
            connection: server.connection(now),
            peer,
            inbox: Vec::new(),
            follower: Follower::default(),
            now,
        };
        let opening = client.peer.opening();
        client.connection.receive(&opening, now);
        client.settle(server);
        assert!(
            matches!(client.inbox.as_slice(), [Message::Welcome(_)]),
            "{:?}",
            client.inbox
        );
        client.inbox.clear();
        client
    }

    fn deliver(&mut self, bytes: &[u8]) {
        self.connection.receive(bytes, self.now);
    }

    fn send(&mut self, request: Request) -> RequestId {
        let (id, bytes) = self.peer.request(request);
        self.deliver(&bytes);
        id
    }

    /// Takes everything the connection has queued, without running work.
    fn read(&mut self) {
        let bytes = flush(&mut self.connection, self.now);
        let messages = self.peer.receive(&bytes);
        self.follower.follow(&messages);
        self.inbox.extend(messages);
    }

    /// Runs work and reads until nothing is left.
    fn settle(&mut self, server: &Server) {
        loop {
            let ran = run(server, &mut self.connection, self.now);
            let before = self.inbox.len();
            self.read();
            if ran == 0 && self.inbox.len() == before {
                return;
            }
        }
    }

    fn result(&mut self, id: RequestId) -> Option<Result<Success, WireError>> {
        let index = self.inbox.iter().position(|message| {
            matches!(message, Message::Result(result) if result.request == id)
        })?;
        match self.inbox.remove(index) {
            Message::Result(result) => Some(result.outcome),
            _ => unreachable!(),
        }
    }

    fn call(
        &mut self,
        server: &Server,
        request: Request,
    ) -> Result<Success, WireError> {
        let id = self.send(request);
        self.settle(server);
        self.result(id).expect("a result")
    }

    fn results(&self) -> usize {
        self.inbox
            .iter()
            .filter(|message| matches!(message, Message::Result(_)))
            .count()
    }
}

struct Fixture {
    server: Server,
    client: Client,
}

impl Fixture {
    fn new() -> Self {
        Self::with(ServerConfig::default())
    }

    fn with(config: ServerConfig) -> Self {
        let server = server(config);
        let client = Client::connect(&server, T0);
        Self { server, client }
    }

    fn mux(&self) -> MutexGuard<'_, Mux> {
        self.server.lock()
    }

    fn call(&mut self, request: Request) -> Result<Success, WireError> {
        self.client.call(&self.server, request)
    }

    fn send(&mut self, request: Request) -> RequestId {
        self.client.send(request)
    }

    fn settle(&mut self) {
        self.client.settle(&self.server);
    }

    fn connection(&mut self) -> &mut Connection {
        &mut self.client.connection
    }

    fn session(&self) -> SessionId {
        self.mux().create_session(None).unwrap()
    }

    /// A session with a workspace and a tab of two quiet panes.
    fn two_panes(&self) -> (SessionId, WorkspaceId, TabId, PaneId, PaneId) {
        let mut mux = self.mux();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let tab = mux.open_tab(workspace, &quiet_command()).unwrap().tab;
        let a = tab.first_pane().id;
        let b = mux
            .split_pane(
                workspace,
                tab.id,
                a,
                SplitDirection::Right,
                &quiet_command(),
            )
            .unwrap()
            .pane
            .id;
        (session, workspace, tab.id, a, b)
    }

    fn panes(&self, tab: TabId) -> Vec<PaneId> {
        self.mux()
            .tab(tab)
            .unwrap()
            .layout()
            .panes()
            .into_iter()
            .map(|pane| pane.id)
            .collect()
    }

    fn swap(
        &mut self,
        slot: u32,
        panes: (WorkspaceId, TabId, PaneId, PaneId),
    ) -> Option<u64> {
        let (workspace, tab, a, b) = panes;
        token(self.call(Request::SwapPanes {
            slot,
            workspace,
            tab,
            a,
            b,
        }))
    }

    fn restore(
        &mut self,
        token: u64,
        direction: Way,
    ) -> Result<Success, WireError> {
        self.call(Request::RestoreLayout { token, direction })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.server.lock().shutdown();
    }
}

fn token(outcome: Result<Success, WireError>) -> Option<u64> {
    match outcome.expect("a success").response {
        Response::Undo { token } => token,
        response => panic!("not an undo token: {response:?}"),
    }
}

fn code(outcome: Result<Success, WireError>) -> ErrorCode {
    outcome.expect_err("a failure").code
}

fn bogus_session(server: &Server) -> SessionId {
    SessionId::in_runtime(server.runtime(), 999_999)
}

// Handshake.

#[test]
fn a_hello_gets_a_welcome_with_the_offer_and_the_configured_in_flight_limit() {
    let server = server(ServerConfig {
        max_in_flight_requests: 7,
        ..ServerConfig::default()
    });
    let mut connection = server.connection(T0);
    let mut peer = TestPeer::new(server.runtime());
    connection.receive(&peer.opening(), T0);
    let messages = peer.receive(&flush(&mut connection, T0));
    let [Message::Welcome(welcome)] = messages.as_slice() else {
        panic!("{messages:?}");
    };
    assert_eq!(welcome.incarnation, server.offer().incarnation);
    assert_eq!(welcome.profile.max_in_flight_requests, 7);
    assert_eq!(
        connection.next_deadline(),
        None,
        "an idle connection arms nothing"
    );
}

#[test]
fn a_foreign_major_is_rejected_and_closes_once_the_reject_is_written() {
    let server = server(ServerConfig::default());
    let mut connection = server.connection(T0);
    let mut peer = TestPeer::with_hello(
        server.runtime(),
        Hello {
            major: 99,
            ..Hello::new(ClientKind::Desktop, "test", "test")
        },
    );
    connection.receive(&peer.opening(), T0);
    assert!(!connection.wants_input());
    let transmit = connection.poll_transmit(usize::MAX).unwrap();
    connection.transmitted(transmit.len() - 1, T0);
    assert!(!connection.is_closed(), "the Reject is still being written");
    connection.transmitted(1, T0);
    assert_eq!(connection.close_reason(), Some(&CloseReason::Rejected));
    let messages = peer.receive(&transmit.to_bytes());
    assert!(matches!(
        messages.as_slice(),
        [Message::Reject(reject)]
            if matches!(reject.reason, RejectReason::IncompatibleMajor { .. })
    ));
}

#[test]
fn a_foreign_preamble_or_a_message_before_hello_closes_the_connection() {
    let server = server(ServerConfig::default());
    let mut connection = server.connection(T0);
    connection.receive(b"HTTP/1.1", T0);
    assert!(matches!(
        connection.close_reason(),
        Some(CloseReason::Violation(_))
    ));

    let mut connection = server.connection(T0);
    let mut peer = TestPeer::new(server.runtime());
    let mut bytes = huterm_wire::frame::preamble().to_vec();
    bytes.extend(peer.request(Request::SubscribeHierarchy).1);
    connection.receive(&bytes, T0);
    assert!(matches!(
        connection.close_reason(),
        Some(CloseReason::Violation(_))
    ));
}

#[test]
fn the_handshake_deadline_closes_a_client_that_sends_no_hello() {
    let server = server(ServerConfig::default());
    let mut connection = server.connection(T0);
    connection.receive(&huterm_wire::frame::preamble(), T0);
    assert_eq!(connection.next_deadline(), Some(HANDSHAKE_DEADLINE));
    connection.tick(just_before(HANDSHAKE_DEADLINE));
    assert!(!connection.is_closed());
    connection.tick(HANDSHAKE_DEADLINE);
    assert_eq!(
        connection.close_reason(),
        Some(&CloseReason::HandshakeTimeout)
    );
}

// Requests.

#[test]
fn a_request_past_the_in_flight_limit_answers_busy_without_acting() {
    let mut f = Fixture::new();
    let limit = f.server.config().max_in_flight_requests as usize;
    for _ in 0..limit {
        f.send(Request::CreateSession { name: None });
    }
    let over = f.send(Request::CreateSession { name: None });
    f.client.read();
    let busy = f.client.result(over).expect("an immediate answer");
    let error = busy.expect_err("busy");
    assert_eq!((error.code, error.retry), (ErrorCode::Busy, Retry::Safe));
    assert_eq!(f.client.results(), 0, "the others wait for work");
    f.settle();
    assert_eq!(f.client.results(), limit);
    assert_eq!(f.mux().sessions().len(), limit);
}

#[test]
fn a_cancel_before_its_request_starts_answers_cancelled_and_it_never_runs() {
    let mut f = Fixture::new();
    let id = f.send(Request::CreateSession { name: None });
    let cancel = f.client.peer.cancel(id);
    f.client.deliver(&cancel);
    f.settle();
    let error = f.client.result(id).unwrap().unwrap_err();
    assert_eq!(
        (error.code, error.retry),
        (ErrorCode::Cancelled, Retry::Safe)
    );
    assert_eq!(f.mux().sessions().len(), 0);
    assert_eq!(f.client.results(), 0);
}

#[test]
fn a_cancel_after_its_request_started_changes_nothing() {
    let mut f = Fixture::new();
    let id = f.send(Request::CreateSession { name: None });
    let work = f.connection().poll_work().expect("the request starts");
    let cancel = f.client.peer.cancel(id);
    f.client.deliver(&cancel);
    f.client.read();
    assert_eq!(f.client.results(), 0);
    let completion = execute(&f.server, work.job);
    f.connection().complete(work.id, completion, T0);
    f.settle();
    assert!(f.client.result(id).unwrap().is_ok());
    assert_eq!(f.client.results(), 0, "exactly one result");
    assert_eq!(f.mux().sessions().len(), 1);
}

#[test]
fn malformed_requests_close_the_connection() {
    let raw = |build: &dyn Fn(&mut Writer)| {
        let mut w = Writer::new();
        w.u64(MessageKind::Request.number());
        build(&mut w);
        frames(&w.into_bytes(), 0, huterm_wire::frame::DEFAULT_MAX_FRAME)
    };
    let zero = raw(&|w| {
        w.record(|w| {
            w.u64(0);
            w.tag(0);
            w.record(|_| Ok(()))
        })
        .unwrap();
    });
    let unknown = raw(&|w| {
        w.record(|w| {
            w.u64(1);
            w.tag(9_999);
            w.record(|_| Ok(()))
        })
        .unwrap();
    });
    let cap = MessageCaps::for_minor(0).cap(MessageKind::Request).unwrap();
    let mut header = Writer::new();
    header.u64(MessageKind::Request.number());
    header.usize(cap);
    header.fixed(&[0; 8]);
    let over_cap = Frame {
        stream: 0,
        end: FrameEnd::More,
        payload: header.into_bytes(),
    }
    .encode(huterm_wire::frame::DEFAULT_MAX_FRAME)
    .unwrap();
    for (name, bytes) in [
        ("zero ID", zero),
        ("unknown tag", unknown),
        ("over cap", over_cap),
    ] {
        let mut f = Fixture::new();
        f.client.deliver(&bytes);
        assert!(
            matches!(
                f.client.connection.close_reason(),
                Some(CloseReason::Violation(_))
            ),
            "{name}: {:?}",
            f.client.connection.close_reason()
        );
    }

    let mut f = Fixture::new();
    let id = f.send(Request::SubscribeHierarchy);
    let again = f.client.peer.request_as(id, Request::ResyncHierarchy);
    f.client.deliver(&again);
    assert!(
        matches!(
            f.client.connection.close_reason(),
            Some(CloseReason::Violation(_))
        ),
        "a duplicate ID"
    );
}

#[test]
fn a_request_id_may_arrive_at_most_the_window_above_the_lowest_unused() {
    let mut f = Fixture::new();
    let edge = RequestId::new(REQUEST_ID_WINDOW).unwrap();
    let bytes = f.client.peer.request_as(edge, Request::SubscribeHierarchy);
    f.client.deliver(&bytes);
    assert!(!f.client.connection.is_closed());

    let mut f = Fixture::new();
    let past = RequestId::new(REQUEST_ID_WINDOW + 1).unwrap();
    let bytes = f.client.peer.request_as(past, Request::SubscribeHierarchy);
    f.client.deliver(&bytes);
    assert!(matches!(
        f.client.connection.close_reason(),
        Some(CloseReason::Violation(_))
    ));
}

#[test]
fn an_unknown_command_answers_unknown_command_with_its_text() {
    let mut f = Fixture::new();
    let error = f
        .call(Request::ExecuteCommand {
            invocation: DecodedInvocation::Unknown {
                id: "from.the.future".into(),
                args: Vec::new(),
            },
        })
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::UnknownCommand);
    assert!(
        error.message.contains("from.the.future"),
        "{}",
        error.message
    );
}

#[test]
fn viewer_requests_name_no_viewer_and_unknown_variants_are_unsupported() {
    let mut f = Fixture::new();
    let viewer = huterm_protocol::ViewerId::in_runtime(f.server.runtime(), 1);
    assert_eq!(
        code(f.call(Request::ClearHistory { viewer })),
        ErrorCode::StaleTarget
    );

    let mux = std::sync::Arc::new(Mutex::new(Mux::default()));
    let server = Server::new(mux, offer(TEST_MINOR), ServerConfig::default());
    let peer = TestPeer::with_hello(
        server.runtime(),
        Hello {
            minor: TEST_MINOR,
            ..Hello::new(ClientKind::Desktop, "test", "test")
        },
    );
    let mut client = Client::connect_with(&server, peer, T0);
    let tab = TabId::in_runtime(server.runtime(), 1);
    let outcome = client.call(&server, Request::PinTab { tab, pinned: true });
    assert_eq!(code(outcome), ErrorCode::Unsupported);
}

#[test]
fn toggling_input_sync_twice_cancels_out() {
    let mut f = Fixture::new();
    let (_, workspace, tab, ..) = f.two_panes();
    let first = f.send(Request::ToggleInputSync { workspace, tab });
    let second = f.send(Request::ToggleInputSync { workspace, tab });
    f.settle();
    assert!(f.client.result(first).unwrap().is_ok());
    assert!(f.client.result(second).unwrap().is_ok());
    assert!(!f.mux().tab(tab).unwrap().input_sync());
    f.call(Request::ToggleInputSync { workspace, tab }).unwrap();
    assert!(f.mux().tab(tab).unwrap().input_sync());
}

#[test]
fn structural_results_carry_a_digest_only_once_the_connection_subscribes() {
    let mut f = Fixture::new();
    let before = f.call(Request::CreateSession { name: None }).unwrap();
    assert_eq!(
        before.committed,
        Some(Committed {
            seq: 1,
            digest: None
        })
    );
    f.call(Request::SubscribeHierarchy).unwrap();
    let after = f.call(Request::CreateSession { name: None }).unwrap();
    let mux = f.mux();
    assert_eq!(
        after.committed,
        Some(Committed {
            seq: mux.hierarchy_seq(),
            digest: Some(mux.hierarchy_digest(DigestProfile::Base)),
        })
    );
}

// Close tickets.

#[test]
fn the_seventeenth_close_ticket_answers_limit_exceeded() {
    let mut f = Fixture::new();
    let session = f.session();
    let prepare = |f: &mut Fixture| {
        f.call(Request::PrepareClose {
            request: CloseRequest::Session(session),
        })
    };
    let mut tickets = Vec::new();
    for _ in 0..MAX_CLOSE_TICKETS {
        let Response::CloseAssessment(assessment) =
            prepare(&mut f).unwrap().response
        else {
            panic!("not an assessment");
        };
        tickets.push(assessment.ticket);
    }
    assert_eq!(f.server.live_close_tickets(), MAX_CLOSE_TICKETS);
    assert_eq!(code(prepare(&mut f)), ErrorCode::LimitExceeded);
    f.call(Request::ReleaseCloseTicket { ticket: tickets[0] })
        .unwrap();
    assert_eq!(f.server.live_close_tickets(), MAX_CLOSE_TICKETS - 1);
    assert!(prepare(&mut f).is_ok());
}

fn ticket(outcome: Result<Success, WireError>) -> (u32, bool) {
    match outcome.expect("an assessment").response {
        Response::CloseAssessment(assessment) => {
            (assessment.ticket, assessment.needs_confirmation)
        }
        response => panic!("not an assessment: {response:?}"),
    }
}

#[test]
fn a_commit_keeps_its_ticket_only_when_confirmation_is_required() {
    let mut f = Fixture::new();
    let (workspace, tab) = {
        let mut mux = f.mux();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let busy = command("set -m; sleep 30 & read value");
        (workspace, mux.open_tab(workspace, &busy).unwrap().tab.id)
    };
    let close = Request::PrepareClose {
        request: CloseRequest::Tab { workspace, tab },
    };
    // The job starts on its own schedule; poll its assessment.
    let deadline = Instant::now() + Duration::from_secs(20);
    let busy = loop {
        let (ticket, needs_confirmation) = ticket(f.call(close.clone()));
        if needs_confirmation {
            break ticket;
        }
        f.call(Request::ReleaseCloseTicket { ticket }).unwrap();
        assert!(
            Instant::now() < deadline,
            "the background job never started"
        );
        std::thread::yield_now();
    };
    let commit = |confirmed| Request::CommitClose {
        ticket: busy,
        confirmed,
    };
    assert_eq!(code(f.call(commit(false))), ErrorCode::ConfirmationRequired);
    assert_eq!(f.server.live_close_tickets(), 1, "the ticket stays");
    f.call(commit(true)).unwrap();
    assert!(f.mux().tab(tab).is_none());
    assert_eq!(f.server.live_close_tickets(), 0);
    assert_eq!(code(f.call(commit(true))), ErrorCode::StaleTarget);

    let (_, workspace, tab, ..) = f.two_panes();
    let (stale, _) = ticket(f.call(Request::PrepareClose {
        request: CloseRequest::Tab { workspace, tab },
    }));
    let session = f.mux().create_session(None).unwrap();
    f.mux().create_workspace(session, None).unwrap();
    let commit = Request::CommitClose {
        ticket: stale,
        confirmed: true,
    };
    assert_eq!(code(f.call(commit.clone())), ErrorCode::StaleClose);
    assert_eq!(code(f.call(commit)), ErrorCode::StaleTarget, "consumed");
}

#[test]
fn committing_a_ticket_a_running_commit_holds_answers_busy() {
    let mut f = Fixture::new();
    let session = f.session();
    let (held, _) = ticket(f.call(Request::PrepareClose {
        request: CloseRequest::Session(session),
    }));
    let commit = Request::CommitClose {
        ticket: held,
        confirmed: true,
    };
    let first = f.send(commit.clone());
    let work = f.connection().poll_work().unwrap();
    let second = f.send(commit);
    f.settle();
    let error = f.client.result(second).unwrap().unwrap_err();
    assert_eq!((error.code, error.retry), (ErrorCode::Busy, Retry::Safe));
    let completion = execute(&f.server, work.job);
    f.connection().complete(work.id, completion, T0);
    f.settle();
    assert!(f.client.result(first).unwrap().is_ok());
    assert!(f.mux().session(session).is_none());
}

// Layout history.

#[test]
fn using_more_than_max_history_slots_evicts_the_least_recently_used() {
    let mut f = Fixture::new();
    let (_, workspace, tab, a, b) = f.two_panes();
    let panes = (workspace, tab, a, b);
    let tokens: Vec<u64> = (0..=u32::try_from(MAX_HISTORY_SLOTS).unwrap())
        .map(|slot| f.swap(slot, panes).expect("a token"))
        .collect();
    assert_eq!(f.server.live_history_records(), MAX_HISTORY_SLOTS);
    let seq = f.mux().hierarchy_seq();
    assert_eq!(code(f.restore(tokens[0], Way::Undo)), ErrorCode::StaleUndo);
    assert_eq!(
        f.mux().hierarchy_seq(),
        seq,
        "an evicted token runs nothing"
    );
    // The newest two slots survive and restore in turn.
    f.restore(tokens[MAX_HISTORY_SLOTS], Way::Undo).unwrap();
    f.restore(tokens[MAX_HISTORY_SLOTS - 1], Way::Undo).unwrap();
    assert!(f.server.live_history_records() <= 2 * MAX_HISTORY_SLOTS);
}

#[test]
fn a_rearrangement_older_than_its_slots_newest_entry_files_nothing() {
    let mut f = Fixture::new();
    let (_, workspace, tab, a, b) = f.two_panes();
    let swap = Request::SwapPanes {
        slot: 0,
        workspace,
        tab,
        a,
        b,
    };
    let older = f.send(swap.clone());
    let newer = f.send(swap);
    let first = f.connection().poll_work().unwrap();
    let second = f.connection().poll_work().unwrap();
    let first_done = execute(&f.server, first.job);
    let second_done = execute(&f.server, second.job);
    f.connection().complete(second.id, second_done, T0);
    f.connection().complete(first.id, first_done, T0);
    f.settle();
    let newest = token(f.client.result(newer).unwrap()).expect("filed");
    assert_eq!(token(f.client.result(older).unwrap()), None);
    f.restore(newest, Way::Undo).unwrap();
}

#[test]
fn an_empty_change_keeps_its_slots_entries() {
    let mut f = Fixture::new();
    let (_, workspace, tab, a, b) = f.two_panes();
    let filed = f.swap(0, (workspace, tab, a, b)).unwrap();
    assert_eq!(f.swap(0, (workspace, tab, a, a)), None);
    f.restore(filed, Way::Undo).unwrap();
    assert_eq!(f.panes(tab), vec![a, b]);
}

#[test]
fn a_restore_whose_slot_was_evicted_while_it_ran_files_nothing() {
    let mut f = Fixture::new();
    let (_, workspace, tab, a, b) = f.two_panes();
    let panes = (workspace, tab, a, b);
    let filed = f.swap(0, panes).unwrap();
    let restore = f.send(Request::RestoreLayout {
        token: filed,
        direction: Way::Undo,
    });
    let work = f.connection().poll_work().unwrap();
    let done = execute(&f.server, work.job);
    for slot in 1..=u32::try_from(MAX_HISTORY_SLOTS).unwrap() {
        f.swap(slot, panes).unwrap();
    }
    assert_eq!(
        f.server.live_history_records(),
        MAX_HISTORY_SLOTS + 1,
        "the running restore holds its record"
    );
    f.connection().complete(work.id, done, T0);
    f.settle();
    assert_eq!(token(f.client.result(restore).unwrap()), None);
    assert_eq!(f.server.live_history_records(), MAX_HISTORY_SLOTS);
}

#[test]
fn undo_then_redo_survive_a_permitted_weight_change() {
    let mut f = Fixture::new();
    let (_, workspace, tab, a, b) = f.two_panes();
    let filed = f.swap(0, (workspace, tab, a, b)).unwrap();
    let (split, children) = {
        let mux = f.mux();
        let PaneNode::Split(split) =
            mux.tab(tab).unwrap().layout().root().clone()
        else {
            panic!("not split");
        };
        let children =
            split.children.iter().map(|child| child.node.id()).collect();
        (split.id, children)
    };
    f.call(Request::ResizeSplit {
        workspace,
        tab,
        split,
        children,
        weights: vec![SPLIT_TOTAL / 4, SPLIT_TOTAL - SPLIT_TOTAL / 4],
    })
    .unwrap();
    let redo = token(f.restore(filed, Way::Undo)).expect("a redo token");
    assert_eq!(f.panes(tab), vec![a, b]);
    let undo = token(f.restore(redo, Way::Redo)).expect("an undo token");
    assert_eq!(f.panes(tab), vec![b, a]);
    assert_eq!(code(f.restore(redo, Way::Redo)), ErrorCode::StaleUndo);
    f.restore(undo, Way::Undo).unwrap();
}

// Attachments.

#[test]
fn only_the_connection_that_attached_may_detach_retarget_or_close_its_window() {
    let mut f = Fixture::new();
    let session = f.session();
    let workspace = f.mux().create_workspace(session, None).unwrap();
    let other_session = f.session();
    let Response::Attachment(attachment) =
        f.call(Request::AttachSession { session }).unwrap().response
    else {
        panic!("not an attachment");
    };
    let mut other = Client::connect(&f.server, T0);
    for request in [
        Request::DetachSession { attachment },
        Request::RetargetAttachment {
            attachment,
            session: other_session,
        },
        Request::PrepareClose {
            request: CloseRequest::Window(attachment),
        },
        Request::PrepareClose {
            request: CloseRequest::EmptiedWindow {
                attachment,
                workspace,
            },
        },
    ] {
        assert_eq!(
            code(other.call(&f.server, request.clone())),
            ErrorCode::NotPermitted,
            "{request:?}"
        );
    }
    f.call(Request::RetargetAttachment {
        attachment,
        session: other_session,
    })
    .unwrap();
    f.call(Request::CloseSession {
        session: other_session,
    })
    .unwrap();
    assert_eq!(
        code(f.call(Request::DetachSession { attachment })),
        ErrorCode::NotPermitted,
        "the close ended it"
    );
}

// Disconnect.

#[test]
fn work_still_running_at_disconnect_releases_what_it_created() {
    let mut f = Fixture::new();
    let (session, workspace, tab, a, b) = f.two_panes();
    let filed = f.swap(0, (workspace, tab, a, b)).unwrap();
    let spare = f.session();
    let (spare_ticket, _) = ticket(f.call(Request::PrepareClose {
        request: CloseRequest::Session(spare),
    }));
    for request in [
        Request::AttachSession { session },
        Request::PrepareClose {
            request: CloseRequest::Session(session),
        },
        Request::RestoreLayout {
            token: filed,
            direction: Way::Undo,
        },
        Request::CommitClose {
            ticket: spare_ticket,
            confirmed: true,
        },
    ] {
        f.send(request);
    }
    let held: Vec<Work> =
        std::iter::from_fn(|| f.connection().poll_work()).collect();
    assert_eq!(held.len(), 4);
    assert_eq!(f.server.live_history_records(), 1, "held by the restore");
    assert_eq!(f.server.live_close_tickets(), 1, "held by the commit");
    let done: Vec<(WorkId, Completion)> = held
        .into_iter()
        .map(|work| (work.id, execute(&f.server, work.job)))
        .collect();
    assert!(f.connection().close(T0).is_empty(), "nothing attached yet");
    assert!(!f.connection().is_finished());
    for (id, completion) in done {
        f.connection().complete(id, completion, T0);
    }
    let attached = f.mux().session_attached(session);
    assert!(attached, "the attach committed");
    run(&f.server, &mut f.client.connection, T0);
    assert!(!f.mux().session_attached(session), "and was detached");
    assert!(f.connection().is_finished());
    assert_eq!(f.server.live_history_records(), 0);
    assert_eq!(f.server.live_close_tickets(), 0);
}

// The hierarchy lane.

#[test]
fn the_snapshot_goes_out_ahead_of_every_later_event() {
    let mut f = Fixture::new();
    let id = f.send(Request::SubscribeHierarchy);
    let work = f.connection().poll_work().unwrap();
    let done = execute(&f.server, work.job);
    let session = f.session();
    f.connection().complete(work.id, done, T0);
    f.settle();
    let hierarchy: Vec<&Message> = f
        .client
        .inbox
        .iter()
        .filter(|message| !matches!(message, Message::Result(_)))
        .collect();
    assert!(matches!(
        hierarchy.as_slice(),
        [Message::HierarchySnapshot(snapshot), Message::HierarchyEvent(event)]
            if event.seq == snapshot.state.seq() + 1
    ));
    let Response::Hierarchy { seq, digest } =
        f.client.result(id).unwrap().unwrap().response
    else {
        panic!("not a subscription");
    };
    assert_eq!(
        (seq, digest),
        (0, HierarchyState::new(f.client.peer.stream()).digest())
    );
    assert!(f.client.follower.state().session(session).is_some());
}

#[test]
fn a_forced_lag_resynchronizes_a_slow_client_without_losing_a_change() {
    let mut f = Fixture::with(ServerConfig {
        hierarchy_lane_budget: 1,
        ..ServerConfig::default()
    });
    let session = f.session();
    f.call(Request::SubscribeHierarchy).unwrap();
    // The client reads nothing while more events commit than core queues.
    for index in 0..1_100 {
        f.mux()
            .rename_session(session, Some(&format!("name {index}")))
            .unwrap();
        f.connection().pump(T0);
    }
    let renamed = f.send(Request::RenameSession {
        session,
        name: Some("last".into()),
    });
    run(&f.server, &mut f.client.connection, T0);
    f.settle();
    let committed = f
        .client
        .result(renamed)
        .unwrap()
        .unwrap()
        .committed
        .unwrap();
    let mux = f.mux();
    let state = f.client.follower.state();
    assert_eq!(f.client.follower.resyncs, 1);
    assert_eq!(state.seq(), mux.hierarchy_seq());
    assert!(committed.seq <= state.seq(), "the waiter resolves");
    assert_eq!(state.digest(), mux.hierarchy_digest(DigestProfile::Base));
    assert_eq!(
        state.session(session).unwrap().custom_name.as_deref(),
        Some("last")
    );
}

#[test]
fn replacements_coalesce_and_a_late_completion_never_rolls_back() {
    let mut f = Fixture::new();
    let subscribe = f.send(Request::SubscribeHierarchy);
    let first = f.connection().poll_work().unwrap();
    let Job::Replace(first_job) = first.job else {
        panic!("not a replacement");
    };
    // Captured early, delivered late.
    let stale = execute(&f.server, Job::Replace(first_job));
    let resync = f.send(Request::ResyncHierarchy);
    assert!(
        f.connection().poll_work().is_none(),
        "one replacement in flight"
    );
    f.session();
    let done = execute(&f.server, Job::Replace(first_job));
    f.connection().complete(first.id, done, T0);
    f.settle();
    let answers = [subscribe, resync]
        .map(|id| f.client.result(id).unwrap().unwrap().response);
    assert_eq!(answers[0], answers[1], "one replacement answers both");

    f.session();
    f.send(Request::ResyncHierarchy);
    let second = f.connection().poll_work().unwrap();
    let done = execute(&f.server, second.job);
    f.connection().complete(second.id, done, T0);
    f.settle();
    let seq = f.client.follower.state().seq();
    assert_eq!(seq, f.mux().hierarchy_seq());
    let received = f.client.inbox.len();
    // The first replacement's older snapshot arrives after the second's.
    f.connection().complete(first.id, stale, T0);
    f.settle();
    assert_eq!(f.client.inbox.len(), received, "nothing more was sent");
    assert_eq!(f.client.follower.state().seq(), seq, "nothing rolled back");
}

#[test]
fn the_hierarchy_lane_stops_draining_at_its_budget() {
    assert_eq!(
        ServerConfig::default().hierarchy_lane_budget,
        HIERARCHY_LANE_BUDGET
    );
    let mut f = Fixture::with(ServerConfig {
        hierarchy_lane_budget: 1,
        ..ServerConfig::default()
    });
    let session = f.session();
    f.call(Request::SubscribeHierarchy).unwrap();
    for name in ["a", "b", "c"] {
        f.mux().rename_session(session, Some(name)).unwrap();
    }
    f.connection().pump(T0);
    for event in 0..3 {
        let lane = f.client.connection.outbound.lane_retained(Lane::Hierarchy);
        assert!(
            lane >= 1,
            "event {event} fits while the lane is under budget"
        );
        let transmit = f.connection().poll_transmit(usize::MAX).unwrap();
        let messages = f.client.peer.receive(&transmit.to_bytes());
        assert!(
            matches!(messages.as_slice(), [Message::HierarchyEvent(_)]),
            "only one: {messages:?}"
        );
        f.connection().transmitted(transmit.len(), T0);
    }
    assert!(f.connection().poll_transmit(usize::MAX).is_none());
}

// Outbound budget and stalls.

#[test]
fn reading_pauses_at_the_outbound_budget_and_resumes_below_it() {
    assert_eq!(ServerConfig::default().outbound_budget, OUTBOUND_BUDGET);
    let mut f = Fixture::new();
    let session = bogus_session(&f.server);
    let select = Request::SelectSession { session };
    f.send(select.clone());
    run(&f.server, &mut f.client.connection, T0);
    let result = f.client.connection.retained();
    f.client.connection.config.outbound_budget = 3 * result;
    for expected in 2..=3 {
        assert!(f.client.connection.wants_input());
        f.send(select.clone());
        run(&f.server, &mut f.client.connection, T0);
        assert_eq!(f.client.connection.retained(), expected * result);
    }
    assert!(!f.client.connection.wants_input(), "at the budget");
    assert!(
        f.client.connection.retained() <= f.client.connection.retained_bound()
    );
    f.send(select);
    assert_eq!(f.client.connection.in_flight(), 0, "the fourth is not read");
    let transmit = f.connection().poll_transmit(usize::MAX).unwrap();
    f.connection().transmitted(transmit.len() / 3, T0);
    assert_eq!(f.client.connection.retained(), 2 * result);
    assert_eq!(f.client.connection.in_flight(), 1, "reading resumed");
}

#[test]
fn a_client_that_stops_reading_under_budget_is_disconnected_at_the_stall_deadline()
 {
    let mut f = Fixture::new();
    let at = Duration::from_secs(5);
    f.client.now = at;
    f.send(Request::CreateSession { name: None });
    run(&f.server, &mut f.client.connection, at);
    assert!(f.client.connection.retained() > 0);
    let deadline = at + WRITER_STALL_DEADLINE;
    assert_eq!(f.client.connection.next_deadline(), Some(deadline));
    f.connection().tick(just_before(deadline));
    assert!(!f.client.connection.is_closed());
    f.connection().tick(deadline);
    assert_eq!(
        f.client.connection.close_reason(),
        Some(&CloseReason::Stalled)
    );
}

#[test]
fn a_client_reading_a_byte_at_a_time_over_budget_is_disconnected_at_the_stall_deadline()
 {
    let mut f = Fixture::with(ServerConfig {
        outbound_budget: 64,
        ..ServerConfig::default()
    });
    let long = "n".repeat(huterm_protocol::MAX_NAME_BYTES);
    f.mux().create_session(Some(&long)).unwrap();
    let at = Duration::from_secs(1);
    f.client.now = at;
    f.send(Request::SubscribeHierarchy);
    run(&f.server, &mut f.client.connection, at);
    assert!(
        !f.client.connection.wants_input(),
        "the snapshot passed the budget"
    );
    assert!(
        f.client.connection.retained() <= f.client.connection.retained_bound()
    );
    let deadline = at + WRITER_STALL_DEADLINE;
    let _transmit = f.connection().poll_transmit(usize::MAX).unwrap();
    let mut now = at;
    while now + Duration::from_secs(1) < deadline {
        now += Duration::from_secs(1);
        f.connection().transmitted(1, now);
        f.connection().tick(now);
        assert!(
            !f.client.connection.is_closed(),
            "steady progress at {now:?}"
        );
        assert!(!f.client.connection.wants_input());
    }
    f.connection().tick(just_before(deadline));
    assert!(!f.client.connection.is_closed());
    f.connection().tick(deadline);
    assert_eq!(
        f.client.connection.close_reason(),
        Some(&CloseReason::Stalled)
    );
}

#[test]
fn the_stream_idle_clock_stops_while_reading_is_paused() {
    let mut f = Fixture::with(ServerConfig {
        outbound_budget: 64,
        ..ServerConfig::default()
    });
    let long = "n".repeat(huterm_protocol::MAX_NAME_BYTES);
    f.mux().create_session(Some(&long)).unwrap();
    // Half a request on stream 1, at 0.
    let request = encode_message(
        &Message::Request(RequestMessage {
            id: RequestId::new(100).unwrap(),
            request: Request::SubscribeHierarchy,
        }),
        f.client.connection.scope,
        MessageCaps::for_minor(0),
    )
    .unwrap();
    let half = Frame {
        stream: 1,
        end: FrameEnd::More,
        payload: request[..2].to_vec(),
    }
    .encode(huterm_wire::frame::DEFAULT_MAX_FRAME)
    .unwrap();
    f.client.deliver(&half);
    let idle = huterm_wire::stream::STREAM_IDLE_DEADLINE;
    assert_eq!(f.client.connection.next_deadline(), Some(idle));
    // Paused from 10 s to 35 s by a snapshot past the budget.
    let paused = Duration::from_secs(10);
    f.client.now = paused;
    f.send(Request::SubscribeHierarchy);
    run(&f.server, &mut f.client.connection, paused);
    assert!(!f.client.connection.wants_input());
    let resumed = Duration::from_secs(35);
    f.client.now = resumed;
    f.client.read();
    assert!(f.client.connection.wants_input());
    let deadline = idle + resumed.abs_diff(paused);
    assert_eq!(f.client.connection.next_deadline(), Some(deadline));
    f.connection().tick(just_before(deadline));
    assert!(!f.client.connection.is_closed());
    f.connection().tick(deadline);
    assert!(matches!(
        f.client.connection.close_reason(),
        Some(CloseReason::Violation(_))
    ));
}
