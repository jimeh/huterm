//! The server: one incarnation's runtime and its client connections.
//!
//! Core state (hierarchy, terminals, host-effect queues) is a plain model.
//! Each connection holds viewers with baselines and credit, a hierarchy
//! subscription, input staging, and an effect lane, and sends through a
//! bounded outbound queue.

use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use crate::frame::{FrameReceiver, FrameSender};
use crate::message::{
    Body, ClientKind, ClientMsg, ErrorCode, MAJOR, Request, RetryClass, RowOp,
    ServerMsg, Update, client_cap, decode_client, encode_server,
};
use crate::model::{
    Cell, Downgrade, Event, FAILURE_CAPACITY, Hierarchy, MINOR_1, Snapshot,
    Status, TabInfo,
};
use crate::profile::Profile;
use crate::rng::Rng;

/// Results and handshake replies.
const LANE_CONTROL: u64 = 0;
/// Results that may span many frames, so small results do not wait.
const LANE_LARGE: u64 = 1;
/// Hierarchy snapshots and events, in sequence order.
const LANE_HIERARCHY: u64 = 2;
/// Host effects.
const LANE_EFFECTS: u64 = 3;

/// Each viewer's updates, status, and input acknowledgements.
fn viewer_lane(viewer: u64) -> u64 {
    1_000 + viewer
}

/// An effect core admitted and has not delivered or dropped.
#[derive(Clone, Debug)]
pub struct PendingEffect {
    /// Origin sequence within the terminal.
    pub origin: u64,
    /// Terminal.
    pub terminal: u64,
    /// Selected connection and viewer.
    pub conn: usize,
    /// Selected viewer.
    pub viewer: u64,
    /// Permission generation at admission.
    pub generation: u64,
    /// Clipboard text.
    pub payload: String,
    /// Admission time.
    pub admitted: u64,
}

#[derive(Debug)]
struct Recipient {
    conn: usize,
    viewer: u64,
    allowed: bool,
    generation: u64,
    activity: u64,
}

#[derive(Debug)]
struct Terminal {
    snapshot: Rc<Snapshot>,
    status: Status,
    stopped: bool,
    core_input: VecDeque<(usize, u64, u64, usize)>,
    effects: VecDeque<PendingEffect>,
    recipients: Vec<Recipient>,
    next_origin: u64,
    next_failure: u64,
}

#[derive(Debug)]
struct LogEntry {
    seq: u64,
    event: Event,
    after: Hierarchy,
    digests: Option<[u64; 2]>,
}

#[derive(Debug)]
struct Op {
    due: u64,
    conn: usize,
    client: u64,
    id: u64,
    request: Request,
}

#[derive(Debug)]
struct RemoteViewer {
    tab: u64,
    opened_by: Option<u64>,
    credit: u64,
    baseline: Option<Rc<Snapshot>>,
    update_seq: u64,
    notified: bool,
    full_uncredited: bool,
    sent_status: Option<u64>,
    staging: VecDeque<(u64, String)>,
    staging_bytes: usize,
    expect: u64,
    admitted: u64,
    acked: u64,
    deltas: u64,
    acked_update: u64,
}

#[derive(Debug)]
struct Lane {
    effect: PendingEffect,
    serial: u64,
    head_at: u64,
    first_mark: Option<u64>,
    last_mark: Option<u64>,
}

/// One client connection.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent connection states"
)]
pub struct Conn {
    client: u64,
    minor: u16,
    kind: ClientKind,
    open: bool,
    closed: bool,
    since: u64,
    rx: FrameReceiver,
    tx: FrameSender,
    out: VecDeque<u8>,
    enqueued: u64,
    taken: u64,
    results: VecDeque<ServerMsg>,
    inflight: usize,
    large_inflight: bool,
    hier: Option<u64>,
    hier_resync: bool,
    viewers: BTreeMap<u64, RemoteViewer>,
    lane: Option<Lane>,
    no_progress_since: Option<u64>,
    over_budget_since: Option<u64>,
    reading_paused: bool,
}

/// Facts the invariants check after a run.
#[derive(Debug, Default)]
pub struct Record {
    /// Mutations executed: (client, request id).
    pub executed: Vec<(u64, u64)>,
    /// Input reaching the PTY: (tab, client, viewer, seq, bytes).
    pub pty: Vec<(u64, u64, u64, u64, usize)>,
    /// Effects admitted: (terminal, origin, conn, viewer).
    pub admitted: Vec<(u64, u64, usize, u64)>,
    /// Effects delivered in full: (terminal, origin).
    pub delivered: Vec<(u64, u64)>,
    /// Effects dropped by a deadline or revocation.
    pub dropped: u64,
    /// Connections closed for a protocol violation or a stall.
    pub violations: Vec<(usize, String)>,
    /// Largest outbound queue seen beyond the budget, in bytes.
    pub max_over_budget: usize,
    /// Largest reassembly buffer seen.
    pub max_reassembly: usize,
}

/// One server incarnation.
#[derive(Debug)]
pub struct Server {
    /// This incarnation.
    pub incarnation: u64,
    /// Stable across restarts.
    pub server_id: u64,
    /// The server's own minor version.
    pub minor: u16,
    profile: Rc<Profile>,
    /// Canonical structure.
    pub hierarchy: Hierarchy,
    next_tab: u64,
    log: VecDeque<LogEntry>,
    terminals: BTreeMap<u64, Terminal>,
    /// Connections by link ID.
    pub conns: BTreeMap<usize, Conn>,
    ops: Vec<Op>,
    next_viewer: u64,
    activity: u64,
    /// What happened, for invariants.
    pub record: Record,
}

impl Server {
    /// Starts an incarnation.
    #[must_use]
    pub fn new(
        incarnation: u64,
        server_id: u64,
        minor: u16,
        profile: Rc<Profile>,
    ) -> Self {
        Self {
            incarnation,
            server_id,
            minor,
            profile,
            hierarchy: Hierarchy::default(),
            next_tab: 0,
            log: VecDeque::new(),
            terminals: BTreeMap::new(),
            conns: BTreeMap::new(),
            ops: Vec::new(),
            next_viewer: 0,
            activity: 0,
            record: Record::default(),
        }
    }

    /// Accepts a connection on link `id`.
    pub fn accept(&mut self, id: usize, now: u64) {
        let profile = &self.profile;
        self.conns.insert(
            id,
            Conn {
                client: 0,
                minor: 0,
                kind: ClientKind::Text,
                open: false,
                closed: false,
                since: now,
                rx: FrameReceiver::new(
                    profile.max_frame,
                    profile.max_open_streams,
                    Some(profile.server_reassembly()),
                ),
                tx: FrameSender::new(
                    profile.max_frame,
                    profile.max_open_streams,
                    profile.design.ordering_lanes,
                ),
                out: VecDeque::new(),
                enqueued: 0,
                taken: 0,
                results: VecDeque::new(),
                inflight: 0,
                large_inflight: false,
                hier: None,
                hier_resync: false,
                viewers: BTreeMap::new(),
                lane: None,
                no_progress_since: None,
                over_budget_since: None,
                reading_paused: false,
            },
        );
    }

    /// Whether the connection on link `id` ended.
    #[must_use]
    pub fn is_closed(&self, id: usize) -> bool {
        self.conns.get(&id).is_none_or(|conn| conn.closed)
    }

    /// Ends a connection: drop viewers (admitted input still runs), remove
    /// its recipients and their queued effects, and keep running its ops.
    pub fn disconnect(&mut self, id: usize) {
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        if conn.closed {
            return;
        }
        conn.closed = true;
        conn.viewers.clear();
        conn.lane = None;
        conn.hier = None;
        conn.out.clear();
        for terminal in self.terminals.values_mut() {
            terminal.recipients.retain(|recipient| recipient.conn != id);
            let before = terminal.effects.len();
            terminal.effects.retain(|effect| effect.conn != id);
            self.record.dropped += (before - terminal.effects.len()) as u64;
        }
    }

    fn violation(&mut self, id: usize, why: String) {
        self.record.violations.push((id, why));
        self.disconnect(id);
    }

    /// Whether the server holds back reading `id`, because its outbound
    /// queue is over budget.
    #[must_use]
    pub fn reading_paused(&self, id: usize) -> bool {
        self.conns.get(&id).is_some_and(|conn| conn.reading_paused)
    }

    /// Bytes arrived from the client on link `id`.
    pub fn receive(&mut self, id: usize, bytes: &[u8], now: u64) {
        let profile = Rc::clone(&self.profile);
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        if conn.closed {
            return;
        }
        let minor = conn.minor;
        let result =
            conn.rx.push(bytes, now, &|kind| client_cap(kind, &profile));
        self.record.max_reassembly =
            self.record.max_reassembly.max(conn.rx.buffered());
        match result {
            Err(violation) => self.violation(id, format!("{violation:?}")),
            Ok(messages) => {
                for bytes in messages {
                    match decode_client(&bytes, &profile, minor) {
                        Ok(message) => self.handle(id, message, now),
                        Err(error) => {
                            self.violation(id, format!("decode {error:?}"));
                            return;
                        }
                    }
                    if self.is_closed(id) {
                        return;
                    }
                }
            }
        }
    }

    #[expect(clippy::too_many_lines, reason = "one arm per client message")]
    fn handle(&mut self, id: usize, message: ClientMsg, now: u64) {
        let profile = Rc::clone(&self.profile);
        let server_minor = self.minor;
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        if !conn.open {
            let ClientMsg::Hello { major, minor, kind } = message else {
                self.violation(id, "message before Hello".into());
                return;
            };
            if major != MAJOR {
                conn.results.push_back(ServerMsg::Reject {
                    server_major: MAJOR,
                    server_minor,
                });
                conn.open = true;
                conn.minor = 0;
                return;
            }
            conn.minor = minor.min(server_minor);
            conn.kind = kind;
            conn.open = true;
            conn.results.push_back(ServerMsg::Welcome {
                minor: conn.minor,
                incarnation: self.incarnation,
                server_id: self.server_id,
            });
            return;
        }
        match message {
            ClientMsg::Hello { .. } => {
                self.violation(id, "second Hello".into());
            }
            ClientMsg::Request {
                id: request_id,
                request,
            } => self.request(id, request_id, request, now),
            ClientMsg::Input { viewer, seq, data } => {
                let Some(remote) = conn.viewers.get_mut(&viewer) else {
                    return;
                };
                if seq != remote.expect {
                    let why = format!(
                        "input out of order: viewer {viewer} got {seq} expected {}",
                        remote.expect
                    );
                    self.violation(id, why);
                    return;
                }
                remote.expect += 1;
                remote.staging_bytes += data.len();
                remote.staging.push_back((seq, data));
                if remote.staging_bytes > profile.viewer_input_budget {
                    self.violation(id, "input window exceeded".into());
                }
            }
            ClientMsg::Credit {
                viewer,
                credits,
                acked,
            } => {
                let Some(remote) = conn.viewers.get_mut(&viewer) else {
                    return;
                };
                remote.credit += credits;
                if acked > remote.update_seq || acked < remote.acked_update {
                    self.violation(id, "bad update acknowledgement".into());
                    return;
                }
                remote.acked_update = acked;
                if remote.credit > profile.credit {
                    self.violation(id, "credit over limit".into());
                }
            }
            ClientMsg::ResyncViewer { viewer } => {
                if let Some(remote) = conn.viewers.get_mut(&viewer) {
                    remote.baseline = None;
                    remote.notified = true;
                    remote.full_uncredited = true;
                }
            }
            ClientMsg::ResyncHierarchy => conn.hier_resync = true,
            ClientMsg::Permission {
                viewer,
                generation,
                allowed,
            } => {
                if let Some(remote) = conn.viewers.get(&viewer) {
                    let tab = remote.tab;
                    if let Some(terminal) = self.terminals.get_mut(&tab) {
                        for recipient in &mut terminal.recipients {
                            if recipient.conn == id
                                && recipient.viewer == viewer
                            {
                                recipient.generation = generation;
                                recipient.allowed = allowed;
                            }
                        }
                        if !allowed {
                            let before = terminal.effects.len();
                            terminal.effects.retain(|effect| {
                                !(effect.conn == id && effect.viewer == viewer)
                            });
                            self.record.dropped +=
                                (before - terminal.effects.len()) as u64;
                        }
                    }
                }
            }
            ClientMsg::Focus { viewer } => {
                self.activity += 1;
                let activity = self.activity;
                for terminal in self.terminals.values_mut() {
                    for recipient in &mut terminal.recipients {
                        if recipient.conn == id && recipient.viewer == viewer {
                            recipient.activity = activity;
                        }
                    }
                }
            }
        }
    }

    fn request(
        &mut self,
        id: usize,
        request_id: u64,
        request: Request,
        now: u64,
    ) {
        let profile = Rc::clone(&self.profile);
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        if request.minor() > conn.minor {
            self.violation(id, "request newer than negotiated".into());
            return;
        }
        let large = matches!(request, Request::Selection { .. });
        if conn.inflight >= profile.max_in_flight
            || (large && conn.large_inflight)
        {
            conn.results.push_back(ServerMsg::Result {
                id: request_id,
                outcome: Err((ErrorCode::Busy, None)),
            });
            return;
        }
        match request.class() {
            RetryClass::Mutation => {
                conn.inflight += 1;
                let client = conn.client;
                let delay = (request_id * 7) % 13;
                self.ops.push(Op {
                    due: now + delay,
                    conn: id,
                    client,
                    id: request_id,
                    request,
                });
            }
            RetryClass::RuntimeQuery => {
                let minor = conn.minor;
                let outcome = if request == Request::ListTabs {
                    Ok(Body::Tabs(self.hierarchy.project(minor).tabs))
                } else {
                    // The snapshot travels on the hierarchy lane, ahead
                    // of the events that follow it.
                    conn.hier = Some(self.hierarchy.seq);
                    conn.hier_resync = true;
                    Ok(Body::Done {
                        committed: self.hierarchy.seq,
                    })
                };
                conn.results.push_back(ServerMsg::Result {
                    id: request_id,
                    outcome,
                });
            }
            RetryClass::Viewer => self.viewer_request(id, request_id, &request),
        }
    }

    fn viewer_request(
        &mut self,
        id: usize,
        request_id: u64,
        request: &Request,
    ) {
        let profile = Rc::clone(&self.profile);
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        let outcome = match *request {
            Request::SubscribeTerminal { tab, effects } => {
                match self.terminals.get_mut(&tab) {
                    None => Err((ErrorCode::StaleTarget, None)),
                    Some(terminal) if terminal.stopped => Ok(Body::Stopped {
                        status: terminal.status.clone(),
                    }),
                    Some(_) if conn.viewers.len() >= profile.max_viewers => {
                        Err((ErrorCode::LimitExceeded, None))
                    }
                    Some(terminal) => {
                        self.next_viewer += 1;
                        let viewer = self.next_viewer;
                        conn.viewers.insert(
                            viewer,
                            RemoteViewer {
                                tab,
                                opened_by: None,
                                credit: 0,
                                baseline: None,
                                update_seq: 0,
                                notified: true,
                                full_uncredited: true,
                                sent_status: Some(terminal.status.revision),
                                staging: VecDeque::new(),
                                staging_bytes: 0,
                                expect: 1,
                                admitted: 0,
                                acked: 0,
                                deltas: 0,
                                acked_update: 0,
                            },
                        );
                        if effects && conn.kind == ClientKind::Desktop {
                            terminal.recipients.push(Recipient {
                                conn: id,
                                viewer,
                                allowed: false,
                                generation: 0,
                                activity: 0,
                            });
                        }
                        Ok(Body::Viewer {
                            viewer,
                            status: terminal.status.clone(),
                        })
                    }
                }
            }
            Request::Selection { viewer } => match conn.viewers.get(&viewer) {
                None => Err((ErrorCode::StaleTarget, None)),
                Some(remote) => {
                    let text: String = self
                        .terminals
                        .get(&remote.tab)
                        .map(|terminal| {
                            terminal
                                .snapshot
                                .rows
                                .iter()
                                .flat_map(|row| row.iter().map(|cell| cell.ch))
                                .collect()
                        })
                        .unwrap_or_default();
                    let mut text = text.repeat(4);
                    let mut end = profile.selection_payload.min(text.len());
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    text.truncate(end);
                    conn.large_inflight = true;
                    Ok(Body::Selection(text))
                }
            },
            _ => Err((ErrorCode::Invalid, None)),
        };
        conn.results.push_back(ServerMsg::Result {
            id: request_id,
            outcome,
        });
    }

    fn emit(&mut self, event: Event) {
        self.hierarchy.seq += 1;
        let seq = self.hierarchy.seq;
        let versioned = self.profile.design.versioned_digests;
        let digests = seq
            .is_multiple_of(self.profile.hierarchy_digest_every)
            .then(|| {
                [
                    self.hierarchy.digest(if versioned { 0 } else { MINOR_1 }),
                    self.hierarchy.digest(MINOR_1),
                ]
            });
        self.log.push_back(LogEntry {
            seq,
            event,
            after: self.hierarchy.clone(),
            digests,
        });
        while self.log.len() > self.profile.hierarchy_queue * 4 {
            self.log.pop_front();
        }
    }

    fn execute(&mut self, op: Op) -> Result<u64, ErrorCode> {
        let profile = Rc::clone(&self.profile);
        let name_ok = |name: &str| {
            !name.trim().is_empty() && name.len() <= profile.max_name_bytes
        };
        let event = match op.request {
            Request::Open { name } => {
                if !name_ok(&name) {
                    return Err(ErrorCode::Invalid);
                }
                if self.hierarchy.tabs.len() >= profile.max_tabs {
                    return Err(ErrorCode::LimitExceeded);
                }
                self.next_tab += 1;
                let tab = self.next_tab;
                self.terminals.insert(tab, new_terminal(&profile));
                Event::Opened {
                    tab: TabInfo {
                        id: tab,
                        name,
                        pinned: false,
                    },
                    index: self.hierarchy.tabs.len(),
                }
            }
            Request::Rename { tab, name } => {
                if !name_ok(&name) {
                    return Err(ErrorCode::Invalid);
                }
                if !self.hierarchy.tabs.iter().any(|info| info.id == tab) {
                    return Err(ErrorCode::StaleTarget);
                }
                if self.minor >= MINOR_1 && name.len() % 2 == 0 {
                    Event::Retitled { tab, name }
                } else {
                    Event::Renamed { tab, name }
                }
            }
            Request::Move { tab, index } => {
                if !self.hierarchy.tabs.iter().any(|info| info.id == tab)
                    || index >= self.hierarchy.tabs.len()
                {
                    return Err(ErrorCode::StaleTarget);
                }
                Event::Moved { tab, index }
            }
            Request::Close { tab } => {
                if self.terminals.remove(&tab).is_none() {
                    return Err(ErrorCode::StaleTarget);
                }
                for conn in self.conns.values_mut() {
                    let closed: Vec<u64> = conn
                        .viewers
                        .iter()
                        .filter(|(_, v)| v.tab == tab)
                        .map(|(id, _)| *id)
                        .collect();
                    for viewer in closed {
                        conn.viewers.remove(&viewer);
                        if !conn.closed {
                            conn.results
                                .push_back(ServerMsg::ViewerClosed { viewer });
                        }
                    }
                }
                Event::Closed { tab }
            }
            Request::Pin { tab, pinned } => {
                if !self.hierarchy.tabs.iter().any(|info| info.id == tab) {
                    return Err(ErrorCode::StaleTarget);
                }
                Event::Pinned { tab, pinned }
            }
            Request::Reverse => Event::Reversed,
            _ => return Err(ErrorCode::Invalid),
        };
        self.hierarchy.mutate(&event);
        self.emit(event);
        Ok(self.hierarchy.seq)
    }

    /// Runs due operations, staged input, and every connection's pumps.
    pub fn tick(&mut self, now: u64) {
        let due: Vec<Op> = {
            let (due, later) = std::mem::take(&mut self.ops)
                .into_iter()
                .partition(|op| op.due <= now);
            self.ops = later;
            due
        };
        for op in due {
            let (conn_id, client, id) = (op.conn, op.client, op.id);
            let result = self.execute(op);
            if result.is_ok() {
                self.record.executed.push((client, id));
            }
            if let Some(conn) = self.conns.get_mut(&conn_id) {
                conn.inflight = conn.inflight.saturating_sub(1);
                if !conn.closed {
                    let outcome = result
                        .map(|committed| Body::Done { committed })
                        .map_err(|code| (code, None));
                    conn.results.push_back(ServerMsg::Result { id, outcome });
                }
            }
        }
        self.drain_input();
        let ids: Vec<usize> = self.conns.keys().copied().collect();
        for id in ids {
            self.pump(id, now);
        }
    }

    fn drain_input(&mut self) {
        let profile = Rc::clone(&self.profile);
        // Staged input enters core while the viewer's core budget allows.
        for (&conn_id, conn) in &mut self.conns {
            for (&viewer, remote) in &mut conn.viewers {
                let Some(terminal) = self.terminals.get_mut(&remote.tab) else {
                    continue;
                };
                let queued: usize = terminal
                    .core_input
                    .iter()
                    .filter(|(c, v, _, _)| *c == conn_id && *v == viewer)
                    .map(|entry| entry.3)
                    .sum();
                let mut room =
                    profile.viewer_input_budget.saturating_sub(queued);
                while let Some((seq, data)) = remote.staging.front() {
                    if data.len() > room || terminal.stopped {
                        break;
                    }
                    room -= data.len();
                    terminal.core_input.push_back((
                        conn_id,
                        viewer,
                        *seq,
                        data.len(),
                    ));
                    remote.admitted = *seq;
                    remote.staging_bytes -= data.len();
                    remote.staging.pop_front();
                }
            }
        }
        for (&tab, terminal) in &mut self.terminals {
            let mut budget = profile.pty_drain_per_tick;
            while let Some(&(conn_id, viewer, seq, len)) =
                terminal.core_input.front()
            {
                if budget == 0 {
                    break;
                }
                budget = budget.saturating_sub(len);
                terminal.core_input.pop_front();
                let client =
                    self.conns.get(&conn_id).map_or(0, |conn| conn.client);
                self.record.pty.push((tab, client, viewer, seq, len));
                terminal.snapshot = Rc::new(Snapshot {
                    revision: terminal.snapshot.revision + 1,
                    cursor: (
                        (terminal.snapshot.cursor.0 + 1)
                            % terminal.snapshot.rows.len(),
                        terminal.snapshot.cursor.1,
                    ),
                    ..(*terminal.snapshot).clone()
                });
                notify(&mut self.conns, tab);
            }
        }
    }

    fn pump(&mut self, id: usize, now: u64) {
        let profile = Rc::clone(&self.profile);
        self.forward_hierarchy(id);
        self.pump_viewers(id);
        self.pump_effects(id, now);
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        if conn.closed {
            return;
        }
        if !conn.open && now.saturating_sub(conn.since) > profile.handshake_ms {
            self.violation(id, "no Hello".into());
            return;
        }
        // Results, in order on their lane; large ones on their own lane.
        while conn.out.len() < profile.outbound_budget {
            let Some(message) = conn.results.front() else {
                break;
            };
            let large = matches!(
                message,
                ServerMsg::Result {
                    outcome: Ok(Body::Selection(_)),
                    ..
                }
            );
            let lane = if large { LANE_LARGE } else { LANE_CONTROL };
            if !conn.tx.can_start(lane) {
                break;
            }
            let message =
                conn.results.pop_front().unwrap_or_else(|| unreachable!());
            if large {
                conn.large_inflight = false;
            }
            let opened = match &message {
                ServerMsg::Result {
                    outcome: Ok(Body::Viewer { viewer, .. }),
                    ..
                } => Some(*viewer),
                _ => None,
            };
            let reject = matches!(message, ServerMsg::Reject { .. });
            let started =
                conn.tx
                    .start(lane, encode_server(&message, conn.minor), false);
            if let (Some(viewer), Some((_, serial))) = (opened, started)
                && let Some(remote) = conn.viewers.get_mut(&viewer)
            {
                remote.opened_by = Some(serial);
            }
            if reject {
                conn.closed = true;
            }
        }
        // Frames: effect frames first, and one effect frame may exceed the
        // budget; other frames wait for room.
        loop {
            let priority = conn
                .lane
                .as_ref()
                .is_some_and(|lane| !conn.tx.finished(lane.serial));
            let limit = if priority {
                profile.outbound_budget + profile.max_frame + 4
            } else {
                profile.outbound_budget
            };
            if conn.out.len() >= limit {
                break;
            }
            let effect_open = conn
                .lane
                .as_ref()
                .is_some_and(|lane| !conn.tx.finished(lane.serial));
            let Some(frame) = conn.tx.next_frame() else {
                break;
            };
            conn.enqueued += frame.len() as u64;
            // Priority frames go first, so while the effect is unfinished the
            // next frame built is the effect's.
            if effect_open && let Some(lane) = conn.lane.as_mut() {
                lane.first_mark.get_or_insert(conn.enqueued);
                if conn.tx.finished(lane.serial) {
                    lane.last_mark = Some(conn.enqueued);
                }
            }
            conn.out.extend(frame);
        }
        let over = conn.out.len().saturating_sub(profile.outbound_budget);
        self.record.max_over_budget = self.record.max_over_budget.max(over);
        conn.reading_paused = conn.out.len() > profile.outbound_budget;
        if conn.reading_paused {
            conn.rx.touch(now);
        } else if let Err(violation) = conn.rx.check_idle(now, profile.idle_ms)
        {
            self.violation(id, format!("{violation:?}"));
        }
    }

    fn forward_hierarchy(&mut self, id: usize) {
        let profile = Rc::clone(&self.profile);
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        if conn.closed {
            return;
        }
        let Some(cursor) = conn.hier else { return };
        let lagged =
            self.log.front().is_some_and(|entry| entry.seq > cursor + 1);
        if (conn.hier_resync || lagged)
            && conn.tx.can_start(LANE_HIERARCHY)
            && conn.out.len() < profile.hierarchy_lane
        {
            let state = self.hierarchy.project(conn.minor);
            let digest = self.hierarchy.digest(conn.minor);
            conn.tx.start(
                LANE_HIERARCHY,
                encode_server(
                    &ServerMsg::HierarchyResync { state, digest },
                    conn.minor,
                ),
                false,
            );
            conn.hier = Some(self.hierarchy.seq);
            conn.hier_resync = false;
            return;
        }
        if conn.hier_resync {
            return;
        }
        let mut cursor = cursor;
        while conn.tx.can_start(LANE_HIERARCHY)
            && conn.out.len() < profile.hierarchy_lane
        {
            let Some(entry) =
                self.log.iter().find(|entry| entry.seq == cursor + 1)
            else {
                break;
            };
            let digest = entry
                .digests
                .map(|digests| digests[usize::from(conn.minor >= MINOR_1)]);
            let message = match entry.event.downgrade(conn.minor) {
                Downgrade::Send(event) => ServerMsg::Event {
                    seq: entry.seq,
                    event,
                    digest,
                },
                Downgrade::Resync => {
                    let state = entry.after.project(conn.minor);
                    let digest = state.digest(conn.minor);
                    ServerMsg::HierarchyResync { state, digest }
                }
            };
            conn.tx.start(
                LANE_HIERARCHY,
                encode_server(&message, conn.minor),
                false,
            );
            cursor = entry.seq;
        }
        conn.hier = Some(cursor);
    }

    fn pump_viewers(&mut self, id: usize) {
        let profile = Rc::clone(&self.profile);
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        if conn.closed || !conn.open {
            return;
        }
        let minor = conn.minor;
        let ids: Vec<u64> = conn.viewers.keys().copied().collect();
        for viewer in ids {
            let Some(remote) = conn.viewers.get_mut(&viewer) else {
                continue;
            };
            let Some(terminal) = self.terminals.get(&remote.tab) else {
                continue;
            };
            // The viewer's lane opens once the result that created it is
            // queued, so the client knows the viewer before its messages.
            if profile.design.ordering_lanes
                && !remote
                    .opened_by
                    .is_some_and(|serial| conn.tx.finished(serial))
            {
                continue;
            }
            let lane = viewer_lane(viewer);
            if remote.admitted > remote.acked && conn.tx.can_start(lane) {
                remote.acked = remote.admitted;
                let seq = remote.acked;
                conn.tx.start(
                    lane,
                    encode_server(&ServerMsg::InputAck { viewer, seq }, minor),
                    false,
                );
            }
            if remote.sent_status != Some(terminal.status.revision)
                && conn.tx.can_start(lane)
                && conn.out.len() < profile.outbound_budget
            {
                remote.sent_status = Some(terminal.status.revision);
                let status = terminal.status.clone();
                conn.tx.start(
                    lane,
                    encode_server(&ServerMsg::Status { viewer, status }, minor),
                    false,
                );
            }
            if terminal.stopped {
                continue;
            }
            if !remote.notified
                || !(remote.credit > 0 || remote.full_uncredited)
            {
                continue;
            }
            if !conn.tx.can_start(lane)
                || conn.out.len() >= profile.outbound_budget
            {
                continue;
            }
            let current = Rc::clone(&terminal.snapshot);
            let full = remote.full_uncredited
                || remote
                    .baseline
                    .as_ref()
                    .is_none_or(|base| base.geometry != current.geometry);
            let credited = !remote.full_uncredited;
            remote.full_uncredited = false;
            if credited {
                remote.credit -= 1;
            }
            remote.deltas = if full { 0 } else { remote.deltas + 1 };
            let with_digest =
                full || remote.deltas % profile.update_digest_every == 0;
            let update = build_update(
                viewer,
                remote,
                &current,
                full,
                credited,
                with_digest,
                minor,
            );
            remote.baseline = Some(current);
            remote.notified = false;
            conn.tx.start(
                lane,
                encode_server(&ServerMsg::Update(update), minor),
                false,
            );
        }
    }

    fn pump_effects(&mut self, id: usize, now: u64) {
        let profile = Rc::clone(&self.profile);
        let Some(conn) = self.conns.get_mut(&id) else {
            return;
        };
        if conn.closed || !conn.open {
            return;
        }
        if let Some(lane) = &conn.lane {
            let total = lane.effect.payload.len() as u64;
            let first_ok =
                lane.first_mark.is_some_and(|mark| conn.taken >= mark);
            let built = conn.tx.finished(lane.serial);
            let done =
                built && conn.taken >= lane.last_mark.unwrap_or(u64::MAX);
            let complete_by = lane.head_at
                + profile.effect_first_ms
                + total / profile.effect_bytes_per_ms.max(1);
            if done {
                self.record
                    .delivered
                    .push((lane.effect.terminal, lane.effect.origin));
                conn.lane = None;
            } else if (!first_ok
                && now > lane.head_at + profile.effect_first_ms)
                || now > complete_by
            {
                // Frames already queued still arrive; only unsent ones stop.
                conn.tx.abort(lane.serial);
                self.record.dropped += 1;
                conn.lane = None;
            } else {
                return;
            }
        }
        // Take the oldest queued effect for this connection.
        let next = self
            .terminals
            .iter()
            .flat_map(|(tab, terminal)| {
                terminal.effects.iter().map(move |effect| (*tab, effect))
            })
            .filter(|(_, effect)| effect.conn == id)
            .min_by_key(|(_, effect)| (effect.admitted, effect.origin))
            .map(|(tab, effect)| (tab, effect.origin));
        let Some((tab, origin)) = next else { return };
        let Some(terminal) = self.terminals.get_mut(&tab) else {
            return;
        };
        let Some(at) = terminal
            .effects
            .iter()
            .position(|effect| effect.origin == origin)
        else {
            return;
        };
        let effect = terminal
            .effects
            .remove(at)
            .unwrap_or_else(|| unreachable!());
        let current = terminal.recipients.iter().any(|r| {
            r.conn == id
                && r.viewer == effect.viewer
                && r.allowed
                && r.generation == effect.generation
        });
        if !current {
            self.record.dropped += 1;
            return;
        }
        if !conn.tx.can_start(LANE_EFFECTS) {
            terminal.effects.insert(at, effect);
            return;
        }
        let message = ServerMsg::Effect {
            effect: origin,
            viewer: effect.viewer,
            generation: effect.generation,
            terminal: tab,
            origin,
            payload: effect.payload.clone(),
        };
        let serial = conn
            .tx
            .start(LANE_EFFECTS, encode_server(&message, conn.minor), true)
            .map_or(0, |(_, serial)| serial);
        conn.lane = Some(Lane {
            effect,
            serial,
            head_at: now,
            first_mark: None,
            last_mark: None,
        });
    }

    /// Bytes the link may take from connection `id`'s outbound queue.
    pub fn take(&mut self, id: usize, room: usize, now: u64) -> Vec<u8> {
        let profile = Rc::clone(&self.profile);
        let Some(conn) = self.conns.get_mut(&id) else {
            return Vec::new();
        };
        let count = room.min(conn.out.len());
        let bytes: Vec<u8> = conn.out.drain(..count).collect();
        conn.taken += count as u64;
        if conn.closed {
            return bytes;
        }
        conn.no_progress_since = if conn.out.is_empty() || count > 0 {
            None
        } else {
            conn.no_progress_since.or(Some(now))
        };
        conn.over_budget_since = if conn.out.len() > profile.outbound_budget {
            conn.over_budget_since.or(Some(now))
        } else {
            None
        };
        let stalled = |since: Option<u64>| {
            since.is_some_and(|since| {
                now.saturating_sub(since) > profile.stall_ms
            })
        };
        if stalled(conn.no_progress_since) || stalled(conn.over_budget_since) {
            self.violation(id, "writer stalled".into());
        }
        bytes
    }

    /// Whether a connection has queued bytes or pending work.
    #[must_use]
    pub fn busy(&self, id: usize) -> bool {
        self.conns.get(&id).is_some_and(|conn| {
            !conn.closed
                && (!conn.out.is_empty()
                    || conn.tx.open() > 0
                    || !conn.results.is_empty()
                    || conn.lane.is_some())
        })
    }

    /// Whether any operation or input is still pending.
    #[must_use]
    pub fn pending_work(&self) -> bool {
        !self.ops.is_empty()
            || self
                .terminals
                .values()
                .any(|t| !t.core_input.is_empty() || !t.effects.is_empty())
    }

    /// The tab IDs that have terminals.
    #[must_use]
    pub fn tabs(&self) -> Vec<u64> {
        self.terminals.keys().copied().collect()
    }

    /// The current view of a tab's terminal at `minor`, with its status.
    #[must_use]
    pub fn terminal(
        &self,
        tab: u64,
        minor: u16,
    ) -> Option<(crate::model::View, &Status, bool)> {
        self.terminals
            .get(&tab)
            .map(|t| (t.snapshot.view(minor), &t.status, t.stopped))
    }

    /// The connection's client ID and negotiated minor version.
    #[must_use]
    pub fn conn_info(&self, id: usize) -> Option<(u64, u16, bool)> {
        self.conns
            .get(&id)
            .map(|conn| (conn.client, conn.minor, conn.open && !conn.closed))
    }

    /// Debug state of a connection's viewers.
    #[must_use]
    pub fn debug_viewers(&self, id: usize) -> String {
        let Some(conn) = self.conns.get(&id) else {
            return "no conn".into();
        };
        let viewers: Vec<String> = conn
            .viewers
            .iter()
            .map(|(v, r)| {
                format!(
                    "v{v} tab {} credit {} notified {} full {} seq {} opened {:?} finished {}",
                    r.tab,
                    r.credit,
                    r.notified,
                    r.full_uncredited,
                    r.update_seq,
                    r.opened_by,
                    r.opened_by.is_some_and(|s| conn.tx.finished(s))
                )
            })
            .collect();
        format!(
            "open {} closed {} out {} streams {} results {} viewers [{}]",
            conn.open,
            conn.closed,
            conn.out.len(),
            conn.tx.open(),
            conn.results.len(),
            viewers.join(", ")
        )
    }

    /// Records which client a connection serves.
    pub fn set_client(&mut self, id: usize, client: u64) {
        if let Some(conn) = self.conns.get_mut(&id) {
            conn.client = client;
        }
    }

    /// Changes one terminal at random, as PTY output would.
    #[expect(clippy::too_many_lines, reason = "one arm per kind of output")]
    pub fn terminal_activity(&mut self, rng: &mut Rng, now: u64) {
        let tabs = self.tabs();
        if tabs.is_empty() {
            return;
        }
        let tab = tabs[rng.index(tabs.len())];
        let minor = self.minor;
        let profile = Rc::clone(&self.profile);
        let Some(terminal) = self.terminals.get_mut(&tab) else {
            return;
        };
        if terminal.stopped {
            return;
        }
        let mut snapshot = (*terminal.snapshot).clone();
        let mut status_changed = false;
        match rng.below(100) {
            0..=39 => {
                let row = rng.index(snapshot.rows.len());
                snapshot.rows[row] = random_row(rng, snapshot.columns, minor);
            }
            40..=59 => {
                snapshot.rows.remove(0);
                snapshot.rows.push(random_row(rng, snapshot.columns, minor));
            }
            60..=69 => {
                snapshot.cursor = (
                    rng.index(snapshot.rows.len()),
                    rng.index(snapshot.columns),
                );
                snapshot.modes ^= 1 << rng.below(3);
            }
            70..=72 => {
                let columns = 2 + rng.index(profile.max_columns - 1);
                snapshot.columns = columns;
                snapshot.geometry += 1;
                snapshot.rows = (0..snapshot.rows.len())
                    .map(|_| random_row(rng, columns, minor))
                    .collect();
                snapshot.cursor = (0, 0);
            }
            73..=79 => {
                terminal.status.title =
                    random_text(rng, profile.max_status_text);
                status_changed = true;
            }
            80..=84 => {
                terminal.status.bells += 1;
                status_changed = true;
            }
            85..=87 => {
                terminal.next_failure += 1;
                terminal.status.failures.push((
                    terminal.next_failure,
                    random_text(rng, profile.max_status_text),
                ));
                if terminal.status.failures.len() > FAILURE_CAPACITY {
                    terminal.status.failures.remove(0);
                }
                status_changed = true;
            }
            88 if terminal.status.exited.is_none() && rng.chance(300) => {
                terminal.status.exited =
                    Some(u8::try_from(rng.below(3)).unwrap_or(0));
                status_changed = true;
            }
            89 if rng.chance(150) => {
                terminal.stopped = true;
                terminal.next_failure += 1;
                terminal
                    .status
                    .failures
                    .push((terminal.next_failure, "fatal".into()));
                if terminal.status.failures.len() > FAILURE_CAPACITY {
                    terminal.status.failures.remove(0);
                }
                status_changed = true;
                terminal.effects.clear();
                for conn in self.conns.values_mut() {
                    let gone: Vec<u64> = conn
                        .viewers
                        .iter()
                        .filter(|(_, v)| v.tab == tab)
                        .map(|(id, _)| *id)
                        .collect();
                    for viewer in gone {
                        conn.viewers.remove(&viewer);
                        if !conn.closed {
                            conn.results
                                .push_back(ServerMsg::ViewerClosed { viewer });
                        }
                    }
                }
            }
            _ => {
                self.admit_effect(tab, rng, now);
                return;
            }
        }
        if status_changed {
            terminal.status.revision += 1;
            return;
        }
        snapshot.revision += 1;
        terminal.snapshot = Rc::new(snapshot);
        notify(&mut self.conns, tab);
    }

    fn admit_effect(&mut self, tab: u64, rng: &mut Rng, now: u64) {
        let profile = Rc::clone(&self.profile);
        let Some(terminal) = self.terminals.get_mut(&tab) else {
            return;
        };
        let conns = &self.conns;
        let eligible = |r: &&Recipient| {
            r.allowed
                && conns.get(&r.conn).is_some_and(|c| {
                    c.open && !c.closed && c.kind == ClientKind::Desktop
                })
        };
        let Some(chosen) = terminal
            .recipients
            .iter()
            .filter(eligible)
            .max_by_key(|r| (r.activity, r.viewer))
        else {
            return;
        };
        let size = if rng.chance(200) {
            profile.clipboard_payload
        } else {
            1 + rng.index(profile.clipboard_payload)
        };
        let bytes: usize =
            terminal.effects.iter().map(|e| e.payload.len()).sum();
        let connections = {
            let mut ids: Vec<usize> =
                terminal.recipients.iter().map(|r| r.conn).collect();
            ids.sort_unstable();
            ids.dedup();
            ids.len()
        };
        let mine = terminal
            .effects
            .iter()
            .filter(|e| e.conn == chosen.conn)
            .count();
        if terminal.effects.len() >= profile.terminal_effects
            || bytes + size > profile.terminal_effect_bytes
            || mine >= profile.effect_share(connections)
        {
            return;
        }
        terminal.next_origin += 1;
        let effect = PendingEffect {
            origin: terminal.next_origin,
            terminal: tab,
            conn: chosen.conn,
            viewer: chosen.viewer,
            generation: chosen.generation,
            payload: "x".repeat(size),
            admitted: now,
        };
        self.record.admitted.push((
            tab,
            effect.origin,
            effect.conn,
            effect.viewer,
        ));
        terminal.effects.push_back(effect);
    }
}

fn notify(conns: &mut BTreeMap<usize, Conn>, tab: u64) {
    for conn in conns.values_mut() {
        for remote in conn.viewers.values_mut() {
            if remote.tab == tab {
                remote.notified = true;
            }
        }
    }
}

fn new_terminal(profile: &Profile) -> Terminal {
    let columns = profile.max_columns;
    let rows = (0..profile.max_rows)
        .map(|_| Rc::new(vec![Cell::BLANK; columns]))
        .collect();
    Terminal {
        snapshot: Rc::new(Snapshot {
            revision: 1,
            geometry: 1,
            columns,
            rows,
            cursor: (0, 0),
            modes: 0,
        }),
        status: Status {
            revision: 1,
            ..Status::default()
        },
        stopped: false,
        core_input: VecDeque::new(),
        effects: VecDeque::new(),
        recipients: Vec::new(),
        next_origin: 0,
        next_failure: 0,
    }
}

fn random_row(rng: &mut Rng, columns: usize, minor: u16) -> crate::model::Row {
    let styles: u64 = if minor >= MINOR_1 { 8 } else { 4 };
    Rc::new(
        (0..columns)
            .map(|_| Cell {
                ch: ['a', 'b', 'é', '日', '🙂', ' '][rng.index(6)],
                style: u8::try_from(rng.below(styles)).unwrap_or(0),
            })
            .collect(),
    )
}

fn random_text(rng: &mut Rng, max: usize) -> String {
    let len = rng.index(max + 1);
    (0..len)
        .map(|_| char::from(b'a' + u8::try_from(rng.below(26)).unwrap_or(0)))
        .collect()
}

fn build_update(
    viewer: u64,
    remote: &mut RemoteViewer,
    current: &Rc<Snapshot>,
    full: bool,
    credited: bool,
    with_digest: bool,
    minor: u16,
) -> Update {
    let base = if full { None } else { Some(remote.update_seq) };
    remote.update_seq += 1;
    let mut styles: Vec<u8> = Vec::new();
    let rows = current
        .rows
        .iter()
        .map(|row| {
            if !full
                && let Some(baseline) = &remote.baseline
                && let Some(index) =
                    baseline.rows.iter().position(|old| Rc::ptr_eq(old, row))
            {
                return RowOp::Keep(index);
            }
            let mut runs: Vec<(usize, Vec<char>)> = Vec::new();
            for cell in row.iter().map(|cell| cell.project(minor)) {
                let style = styles
                    .iter()
                    .position(|s| *s == cell.style)
                    .unwrap_or_else(|| {
                        styles.push(cell.style);
                        styles.len() - 1
                    });
                match runs.last_mut() {
                    Some((last, chars)) if *last == style => {
                        chars.push(cell.ch);
                    }
                    _ => runs.push((style, vec![cell.ch])),
                }
            }
            RowOp::Full(runs)
        })
        .collect();
    Update {
        viewer,
        seq: remote.update_seq,
        base,
        credited,
        revision: current.revision,
        geometry: current.geometry,
        columns: current.columns,
        cursor: current.cursor,
        modes: current.modes,
        styles,
        rows,
        digest: with_digest.then(|| current.view(minor).digest()),
    }
}
