//! A client: one desktop or text client that connects, reconnects, and
//! keeps a projection of the server's state.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use crate::frame::{FrameReceiver, FrameSender};
use crate::message::{
    Body, ClientKind, ClientMsg, ErrorCode, MAJOR, Request, RetryClass, RowOp,
    ServerMsg, Update, decode_server, encode_client, server_cap,
};
use crate::model::{Applied, Cell, Hierarchy, StatusBaseline, View};
use crate::profile::Profile;

/// How a request ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// It succeeded; a structural change is visible in the projection.
    Ok,
    /// The server refused it; nothing changed.
    Err(ErrorCode),
    /// The connection broke after it was sent; it may have committed.
    Unknown,
    /// A viewer request whose viewer is gone.
    Disconnected,
    /// A structural change the server committed, cancelled before the
    /// projection reached it because the connection broke.
    Committed,
}

#[derive(Debug)]
struct Pending {
    request: Request,
    tab: Option<u64>,
    intent: u64,
}

/// What a client saw of one terminal in one incarnation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Seen {
    /// Bells at the first observation, the baseline.
    pub first_bells: u64,
    /// Whether the terminal had exited at the first observation.
    pub first_exited: bool,
    /// The last failure sequence at the first observation.
    pub first_failure: u64,
    /// New bells reported since.
    pub bells: u64,
    /// Exits reported.
    pub exits: u32,
    /// Failure sequences reported, plus counted misses.
    pub failures: u64,
    /// The last status revision seen.
    pub revision: u64,
}

#[derive(Debug)]
struct ClientViewer {
    viewer: u64,
    view: Option<View>,
    update_seq: u64,
    discarding: bool,
    outstanding: u64,
    generation: u64,
    allowed: bool,
    effects: bool,
    next_input: u64,
    unacked: VecDeque<(u64, usize)>,
    unacked_bytes: usize,
    local_queue: VecDeque<String>,
    resyncs: u32,
}

#[derive(Debug)]
struct Connection {
    link: usize,
    rx: FrameReceiver,
    tx: FrameSender,
    out: VecDeque<u8>,
    pending: VecDeque<(u64, Vec<u8>)>,
    open: Option<(u64, u16)>,
    inflight: BTreeMap<u64, Pending>,
}

/// A simulated client.
#[derive(Debug)]
pub struct Client {
    /// Identity across connections.
    pub id: u64,
    /// The client's own minor version.
    pub minor: u16,
    /// Desktop or text.
    pub kind: ClientKind,
    profile: Rc<Profile>,
    conn: Option<Connection>,
    next_request: u64,
    /// The hierarchy projection, scoped to `incarnation`.
    pub hierarchy: Option<Hierarchy>,
    /// The incarnation the projection and viewers belong to.
    pub incarnation: Option<u64>,
    waiters: Vec<(u64, u64, Request)>,
    hierarchy_resyncing: bool,
    viewers: BTreeMap<u64, ClientViewer>,
    shown: BTreeSet<u64>,
    /// The tab whose viewer receives credit; `None` grants every viewer.
    pub visible: Option<u64>,
    baselines: BTreeMap<(u64, u64), StatusBaseline>,
    /// Status facts per (incarnation, tab).
    pub seen: BTreeMap<(u64, u64), Seen>,
    retry: Vec<(Request, u64)>,
    /// How each request ended: (request id, request, outcome).
    pub outcomes: Vec<(u64, Request, Outcome)>,
    /// The intent each request ID carries out. A retry gets a new ID for
    /// the same intent, so an intent that runs twice ran a retry.
    pub intents: BTreeMap<u64, u64>,
    next_intent: u64,
    connected_at: u64,
    /// Final views of terminals that stopped, per (incarnation, tab).
    pub stopped: BTreeMap<(u64, u64), Option<View>>,
    /// Effects executed: (incarnation, link, terminal, origin).
    pub executed: Vec<(u64, usize, u64, u64)>,
    /// Contract faults the client detected.
    pub faults: Vec<String>,
    /// Hierarchy resyncs the client asked for, after a gap or a digest
    /// mismatch. Without corruption there should be none.
    pub hierarchy_resyncs: u32,
    /// Viewer resyncs the client asked for.
    pub viewer_resyncs: u32,
    /// Inputs typed and their fate: (incarnation, tab, viewer, seq).
    pub typed: Vec<(u64, u64, u64, u64)>,
}

impl Client {
    /// Creates a client.
    #[must_use]
    pub fn new(
        id: u64,
        minor: u16,
        kind: ClientKind,
        profile: Rc<Profile>,
    ) -> Self {
        Self {
            id,
            minor,
            kind,
            profile,
            conn: None,
            next_request: 0,
            hierarchy: None,
            incarnation: None,
            waiters: Vec::new(),
            hierarchy_resyncing: false,
            viewers: BTreeMap::new(),
            shown: BTreeSet::new(),
            visible: None,
            baselines: BTreeMap::new(),
            seen: BTreeMap::new(),
            retry: Vec::new(),
            intents: BTreeMap::new(),
            next_intent: 0,
            connected_at: 0,
            stopped: BTreeMap::new(),
            outcomes: Vec::new(),
            executed: Vec::new(),
            faults: Vec::new(),
            hierarchy_resyncs: 0,
            viewer_resyncs: 0,
            typed: Vec::new(),
        }
    }

    /// The current link, if connected.
    #[must_use]
    pub fn link(&self) -> Option<usize> {
        self.conn.as_ref().map(|conn| conn.link)
    }

    /// Whether the handshake completed.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.conn.as_ref().is_some_and(|conn| conn.open.is_some())
    }

    /// The negotiated minor version.
    #[must_use]
    pub fn negotiated(&self) -> Option<u16> {
        self.conn
            .as_ref()
            .and_then(|conn| conn.open.map(|(_, minor)| minor))
    }

    /// Whether the handshake has taken longer than `limit` since `now`'s
    /// connection started, so the client gives up and reconnects.
    #[must_use]
    pub fn handshake_overdue(&self, now: u64, limit: u64) -> bool {
        self.conn.is_some()
            && !self.is_open()
            && now.saturating_sub(self.connected_at) > limit
    }

    /// Starts a connection on `link` at `now`.
    pub fn connect(&mut self, link: usize, now: u64) {
        self.connected_at = now;
        let profile = &self.profile;
        let mut conn = Connection {
            link,
            rx: FrameReceiver::new(
                profile.max_frame,
                profile.max_open_streams,
                None,
            ),
            tx: FrameSender::new(
                profile.max_frame,
                profile.max_open_streams,
                profile.design.ordering_lanes,
            ),
            out: VecDeque::new(),
            pending: VecDeque::new(),
            open: None,
            inflight: BTreeMap::new(),
        };
        let hello = ClientMsg::Hello {
            major: MAJOR,
            minor: self.minor,
            kind: self.kind,
        };
        conn.pending
            .push_back((0, encode_client(&hello, self.minor)));
        self.conn = Some(conn);
    }

    /// The connection broke: resolve in-flight requests and drop viewers.
    pub fn disconnected(&mut self) {
        let Some(conn) = self.conn.take() else { return };
        for (id, pending) in conn.inflight {
            match pending.request.class() {
                RetryClass::Mutation => {
                    if self.profile.design.retry_mutations {
                        // A design that retries mutations, for the test
                        // that shows the invariant catches it.
                        self.retry
                            .push((pending.request.clone(), pending.intent));
                    }
                    self.outcomes.push((id, pending.request, Outcome::Unknown));
                }
                RetryClass::Viewer => self.outcomes.push((
                    id,
                    pending.request,
                    Outcome::Disconnected,
                )),
                RetryClass::RuntimeQuery => {
                    self.retry.push((pending.request, pending.intent));
                }
            }
        }
        // A committed change the projection never reached is not `Ok`.
        for (_, id, request) in self.waiters.drain(..) {
            self.outcomes.push((id, request, Outcome::Committed));
        }
        self.viewers.clear();
        self.hierarchy_resyncing = false;
    }

    fn send(&mut self, message: &ClientMsg) {
        // Senders write their own version's fields; older peers skip them.
        let minor = self.minor;
        // A viewer's messages share its lane, so its input stays in order;
        // everything else uses the control lane.
        let lane = match message {
            ClientMsg::Input { viewer, .. }
            | ClientMsg::Credit { viewer, .. }
            | ClientMsg::ResyncViewer { viewer }
            | ClientMsg::Permission { viewer, .. }
            | ClientMsg::Focus { viewer } => 1_000 + viewer,
            _ => 0,
        };
        if let Some(conn) = self.conn.as_mut() {
            conn.pending
                .push_back((lane, encode_client(message, minor)));
        }
    }

    /// Sends a request, unless the negotiated version lacks it.
    pub fn request(&mut self, request: Request) {
        self.next_intent += 1;
        let intent = self.next_intent;
        self.request_for(request, intent);
    }

    fn request_for(&mut self, request: Request, intent: u64) {
        let Some(minor) = self.negotiated() else {
            return;
        };
        if request.minor() > minor {
            return;
        }
        let tab = match &request {
            Request::SubscribeTerminal { tab, .. } => Some(*tab),
            _ => None,
        };
        self.next_request += 1;
        let id = self.next_request;
        if let Some(conn) = self.conn.as_mut() {
            conn.inflight.insert(
                id,
                Pending {
                    request: request.clone(),
                    tab,
                    intent,
                },
            );
        }
        self.intents.insert(id, intent);
        self.send(&ClientMsg::Request { id, request });
    }

    /// Shows a tab: subscribe to its terminal and keep it subscribed.
    pub fn show(&mut self, tab: u64) {
        if self.shown.insert(tab) {
            self.request(Request::SubscribeTerminal {
                tab,
                effects: self.kind == ClientKind::Desktop,
            });
        }
    }

    /// Focuses a tab: it becomes the visible one and gains focus.
    pub fn focus(&mut self, tab: u64) {
        self.visible = Some(tab);
        if let Some(viewer) = self.viewers.get(&tab).map(|v| v.viewer) {
            self.send(&ClientMsg::Focus { viewer });
        }
    }

    /// Types into a tab's viewer, queueing locally beyond the input window.
    pub fn type_text(&mut self, tab: u64, text: String) {
        if let Some(viewer) = self.viewers.get_mut(&tab) {
            viewer.local_queue.push_back(text);
        }
    }

    /// Changes clipboard permission for a tab's viewer.
    pub fn set_permission(&mut self, tab: u64, allowed: bool) {
        let Some(viewer) = self.viewers.get_mut(&tab) else {
            return;
        };
        if !viewer.effects {
            return;
        }
        viewer.generation += 1;
        viewer.allowed = allowed;
        let message = ClientMsg::Permission {
            viewer: viewer.viewer,
            generation: viewer.generation,
            allowed,
        };
        self.send(&message);
    }

    /// Requests a tab's selection text.
    pub fn select(&mut self, tab: u64) {
        if let Some(viewer) = self.viewers.get(&tab).map(|v| v.viewer) {
            self.request(Request::Selection { viewer });
        }
    }

    /// Tabs the client knows.
    #[must_use]
    pub fn known_tabs(&self) -> Vec<u64> {
        self.hierarchy
            .as_ref()
            .map(|h| h.tabs.iter().map(|t| t.id).collect())
            .unwrap_or_default()
    }

    /// The client's view of a tab, if it has one.
    #[must_use]
    pub fn view(&self, tab: u64) -> Option<&View> {
        self.viewers
            .get(&tab)
            .and_then(|viewer| viewer.view.as_ref())
    }

    /// Debug state of a tab's viewer.
    #[must_use]
    pub fn debug_viewer(&self, tab: u64) -> String {
        self.viewers.get(&tab).map_or("none".into(), |v| {
            format!(
                "v{} seq {} discarding {} outstanding {} resyncs {}",
                v.viewer, v.update_seq, v.discarding, v.outstanding, v.resyncs
            )
        })
    }

    /// Tabs with live viewers.
    #[must_use]
    pub fn viewed_tabs(&self) -> Vec<u64> {
        self.viewers.keys().copied().collect()
    }

    /// Unanswered requests and unresolved waiters.
    #[must_use]
    pub fn unresolved(&self) -> usize {
        self.conn.as_ref().map_or(0, |conn| conn.inflight.len())
            + self.waiters.len()
    }

    /// Bytes arrived from the server.
    ///
    /// # Errors
    /// Returns a fault: a protocol violation, a rejection, or a snapshot
    /// whose digest mismatches. The caller closes the connection.
    pub fn receive(&mut self, bytes: &[u8], now: u64) -> Result<(), String> {
        let profile = Rc::clone(&self.profile);
        let minor = self.negotiated().unwrap_or(self.minor);
        let Some(conn) = self.conn.as_mut() else {
            return Ok(());
        };
        let messages = conn
            .rx
            .push(bytes, now, &|kind| server_cap(kind, &profile))
            .map_err(|v| format!("{v:?}"))?;
        for bytes in messages {
            let minor = self.negotiated().unwrap_or(minor);
            let message = decode_server(&bytes, &profile, minor)
                .map_err(|e| format!("decode {e:?}"))?;
            self.handle(message)?;
        }
        Ok(())
    }

    fn handle(&mut self, message: ServerMsg) -> Result<(), String> {
        match message {
            ServerMsg::Welcome {
                minor, incarnation, ..
            } => {
                if let Some(conn) = self.conn.as_mut() {
                    conn.open = Some((incarnation, minor));
                }
                if self.incarnation != Some(incarnation) {
                    self.hierarchy = None;
                    self.shown.clear();
                }
                self.incarnation = Some(incarnation);
                self.request(Request::SubscribeHierarchy);
                for (request, intent) in std::mem::take(&mut self.retry) {
                    if request != Request::SubscribeHierarchy {
                        self.request_for(request, intent);
                    }
                }
                for tab in std::mem::take(&mut self.shown) {
                    self.show(tab);
                }
            }
            ServerMsg::Reject { .. } => return Err("rejected".into()),
            ServerMsg::Result { id, outcome } => self.result(id, outcome)?,
            ServerMsg::Event { seq, event, digest } => {
                let minor = self.negotiated().unwrap_or(0);
                let Some(state) = self.hierarchy.as_mut() else {
                    return Ok(());
                };
                if self.hierarchy_resyncing {
                    return Ok(());
                }
                match state.apply(seq, &event) {
                    Applied::Applied => {
                        if digest.is_some_and(|d| d != state.digest(minor)) {
                            self.request_hierarchy_resync();
                        }
                    }
                    Applied::Stale => {}
                    Applied::Gap | Applied::Inconsistent => {
                        self.request_hierarchy_resync();
                    }
                }
                self.release_waiters();
            }
            ServerMsg::HierarchyResync { state, digest } => {
                self.install_hierarchy(state, digest)?;
            }
            ServerMsg::Update(update) => self.update(&update),
            ServerMsg::Status { viewer, status } => {
                if let Some(tab) = self.tab_of(viewer) {
                    self.observe(tab, &status);
                }
            }
            ServerMsg::ViewerClosed { viewer, stopped } => {
                if let Some(tab) = self.tab_of(viewer)
                    && let Some(state) = self.viewers.remove(&tab)
                    && stopped
                {
                    // A stopped terminal's final view stays the client's
                    // to show; it is not subscribed again.
                    let incarnation = self.incarnation.unwrap_or(0);
                    self.stopped.insert((incarnation, tab), state.view);
                    self.shown.remove(&tab);
                }
            }
            ServerMsg::InputAck { viewer, seq } => {
                if let Some(tab) = self.tab_of(viewer)
                    && let Some(state) = self.viewers.get_mut(&tab)
                {
                    while state.unacked.front().is_some_and(|(s, _)| *s <= seq)
                    {
                        let (_, len) =
                            state.unacked.pop_front().unwrap_or((0, 0));
                        state.unacked_bytes -= len;
                    }
                }
            }
            ServerMsg::Effect {
                viewer,
                generation,
                terminal,
                origin,
                ..
            } => {
                let link = self.link().unwrap_or(0);
                let incarnation = self.incarnation.unwrap_or(0);
                if let Some(tab) = self.tab_of(viewer)
                    && let Some(state) = self.viewers.get(&tab)
                    && state.allowed
                    && state.generation == generation
                {
                    self.executed.push((incarnation, link, terminal, origin));
                }
            }
        }
        Ok(())
    }

    fn tab_of(&self, viewer: u64) -> Option<u64> {
        self.viewers
            .iter()
            .find(|(_, v)| v.viewer == viewer)
            .map(|(tab, _)| *tab)
    }

    fn request_hierarchy_resync(&mut self) {
        if !self.hierarchy_resyncing {
            self.hierarchy_resyncing = true;
            self.hierarchy_resyncs += 1;
            self.send(&ClientMsg::ResyncHierarchy);
        }
    }

    fn install_hierarchy(
        &mut self,
        state: Hierarchy,
        digest: u64,
    ) -> Result<(), String> {
        let minor = self.negotiated().unwrap_or(0);
        if state.digest(minor) != digest {
            // A snapshot that mismatches would mismatch again: a fault.
            self.faults.push("hierarchy snapshot digest".into());
            return Err("hierarchy snapshot digest".into());
        }
        self.hierarchy = Some(state);
        self.hierarchy_resyncing = false;
        self.release_waiters();
        Ok(())
    }

    fn release_waiters(&mut self) {
        let seq = self.hierarchy.as_ref().map_or(0, |h| h.seq);
        let (ready, waiting): (Vec<_>, Vec<_>) =
            self.waiters.drain(..).partition(|(s, _, _)| *s <= seq);
        self.waiters = waiting;
        for (_, id, request) in ready {
            self.outcomes.push((id, request, Outcome::Ok));
        }
    }

    fn result(
        &mut self,
        id: u64,
        outcome: Result<Body, (ErrorCode, Option<u64>)>,
    ) -> Result<(), String> {
        let Some(pending) = self
            .conn
            .as_mut()
            .and_then(|conn| conn.inflight.remove(&id))
        else {
            return Err(format!("result for unknown request {id}"));
        };
        match outcome {
            Err((code, _)) => {
                if code == ErrorCode::StaleTarget
                    && let Some(tab) = pending.tab
                {
                    self.shown.remove(&tab);
                }
                self.outcomes
                    .push((id, pending.request, Outcome::Err(code)));
            }
            Ok(Body::Done { committed }) => {
                let applied = self.hierarchy.as_ref().map_or(0, |h| h.seq);
                if applied >= committed {
                    self.outcomes.push((id, pending.request, Outcome::Ok));
                } else {
                    self.waiters.push((committed, id, pending.request));
                }
            }
            Ok(Body::Viewer { viewer, status }) => {
                let tab = pending.tab.unwrap_or(0);
                let effects = self.kind == ClientKind::Desktop;
                self.viewers.insert(
                    tab,
                    ClientViewer {
                        viewer,
                        view: None,
                        update_seq: 0,
                        discarding: false,
                        outstanding: 0,
                        generation: 0,
                        allowed: false,
                        effects,
                        next_input: 1,
                        unacked: VecDeque::new(),
                        unacked_bytes: 0,
                        local_queue: VecDeque::new(),
                        resyncs: 0,
                    },
                );
                let key = (self.incarnation.unwrap_or(0), tab);
                if !self.profile.design.carry_status_baseline
                    && self.baselines.contains_key(&key)
                {
                    // Without carry-over, a viewer after a reconnect takes
                    // its first read as a fresh baseline and misses what
                    // happened while the client was away.
                    let mut baseline = StatusBaseline::default();
                    baseline.observe(&status);
                    self.baselines.insert(key, baseline);
                    if let Some(seen) = self.seen.get_mut(&key) {
                        seen.revision = status.revision;
                    }
                }
                self.observe(tab, &status);
                if effects {
                    self.set_permission(tab, true);
                }
                self.outcomes.push((id, pending.request, Outcome::Ok));
            }
            Ok(Body::Stopped { status }) => {
                if let Some(tab) = pending.tab {
                    self.observe(tab, &status);
                    self.shown.remove(&tab);
                }
                self.outcomes.push((
                    id,
                    pending.request,
                    Outcome::Err(ErrorCode::Stopped),
                ));
            }
            Ok(Body::Tabs(_) | Body::Selection(_)) => {
                self.outcomes.push((id, pending.request, Outcome::Ok));
            }
        }
        Ok(())
    }

    fn observe(&mut self, tab: u64, status: &crate::model::Status) {
        let incarnation = self.incarnation.unwrap_or(0);
        let first = !self.baselines.contains_key(&(incarnation, tab));
        let baseline = self.baselines.entry((incarnation, tab)).or_default();
        let derived = baseline.observe(status);
        let seen = self.seen.entry((incarnation, tab)).or_default();
        if first {
            // A viewer's first read is its baseline: it reports nothing
            // that happened before it.
            seen.first_bells = status.bells;
            seen.first_exited = status.exited.is_some();
            seen.first_failure = status.failures.last().map_or(0, |f| f.0);
            seen.revision = status.revision;
            return;
        }
        seen.bells += derived.bells;
        seen.exits += u32::from(derived.exit.is_some());
        seen.failures += derived.failures.len() as u64 + derived.missed;
        seen.revision = seen.revision.max(status.revision);
    }

    fn update(&mut self, update: &Update) {
        let minor = self.negotiated().unwrap_or(0);
        let Some(tab) = self.tab_of(update.viewer) else {
            return;
        };
        let Some(state) = self.viewers.get_mut(&tab) else {
            return;
        };
        let credit_on_discard = self.profile.design.credit_on_discard;
        if update.credited && credit_on_discard {
            state.outstanding = state.outstanding.saturating_sub(1);
        }
        let full = update.base.is_none();
        if !full && (state.discarding || update.base != Some(state.update_seq))
        {
            if !state.discarding {
                state.discarding = true;
                state.resyncs += 1;
                self.viewer_resyncs += 1;
                let viewer = state.viewer;
                self.send(&ClientMsg::ResyncViewer { viewer });
            }
            return;
        }
        let rows: Option<Vec<Vec<Cell>>> = update
            .rows
            .iter()
            .map(|op| match op {
                RowOp::Keep(index) => state
                    .view
                    .as_ref()
                    .and_then(|view| view.rows.get(*index).cloned()),
                RowOp::Full(runs) => Some(
                    runs.iter()
                        .flat_map(|(style, chars)| {
                            let style = update.styles[*style];
                            chars.iter().map(move |ch| Cell { ch: *ch, style })
                        })
                        .collect(),
                ),
            })
            .collect();
        let Some(rows) = rows else {
            self.faults.push("keep outside baseline".into());
            state.discarding = true;
            let viewer = state.viewer;
            self.send(&ClientMsg::ResyncViewer { viewer });
            return;
        };
        let view = View {
            revision: update.revision,
            geometry: update.geometry,
            columns: update.columns,
            rows,
            cursor: update.cursor,
            modes: update.modes,
        };
        let _ = minor;
        if let Some(digest) = update.digest
            && digest != view.digest()
        {
            if full {
                self.faults.push("complete snapshot digest".into());
                self.viewers.remove(&tab);
                return;
            }
            state.discarding = true;
            state.resyncs += 1;
            self.viewer_resyncs += 1;
            let viewer = state.viewer;
            self.send(&ClientMsg::ResyncViewer { viewer });
            return;
        }
        if update.credited && !credit_on_discard {
            state.outstanding = state.outstanding.saturating_sub(1);
        }
        state.view = Some(view);
        state.update_seq = update.seq;
        state.discarding = false;
    }

    /// Grants credit, sends queued input within the window, and moves
    /// pending messages into frames.
    pub fn tick(&mut self) {
        let profile = Rc::clone(&self.profile);
        if self.is_open() {
            let tabs: Vec<u64> = self.viewers.keys().copied().collect();
            let incarnation = self.incarnation.unwrap_or(0);
            for tab in tabs {
                let mut messages = Vec::new();
                let visible = self.visible.is_none_or(|v| v == tab);
                let Some(state) = self.viewers.get_mut(&tab) else {
                    continue;
                };
                if visible && state.outstanding < profile.credit {
                    let credits = profile.credit - state.outstanding;
                    state.outstanding = profile.credit;
                    messages.push(ClientMsg::Credit {
                        viewer: state.viewer,
                        credits,
                        acked: state.update_seq,
                    });
                }
                while let Some(text) = state.local_queue.front() {
                    if state.unacked_bytes + text.len()
                        > profile.viewer_input_budget
                    {
                        break;
                    }
                    let text =
                        state.local_queue.pop_front().unwrap_or_default();
                    let seq = state.next_input;
                    state.next_input += 1;
                    state.unacked_bytes += text.len();
                    state.unacked.push_back((seq, text.len()));
                    self.typed.push((incarnation, tab, state.viewer, seq));
                    messages.push(ClientMsg::Input {
                        viewer: state.viewer,
                        seq,
                        data: text,
                    });
                }
                for message in messages {
                    self.send(&message);
                }
            }
        }
        let Some(conn) = self.conn.as_mut() else {
            return;
        };
        let mut blocked = BTreeSet::new();
        let mut index = 0;
        while index < conn.pending.len() && conn.tx.has_free_stream() {
            let lane = conn.pending[index].0;
            if blocked.contains(&lane) || !conn.tx.can_start(lane) {
                blocked.insert(lane);
                index += 1;
                continue;
            }
            let (lane, bytes) = conn.pending.remove(index).unwrap_or_default();
            conn.tx.start(lane, bytes, false);
        }
        while let Some(frame) = conn.tx.next_frame() {
            conn.out.extend(frame);
        }
    }

    /// Bytes the link may take.
    pub fn take(&mut self, room: usize) -> Vec<u8> {
        let Some(conn) = self.conn.as_mut() else {
            return Vec::new();
        };
        let count = room.min(conn.out.len());
        conn.out.drain(..count).collect()
    }

    /// Whether anything waits to be sent.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.conn.as_ref().is_some_and(|conn| {
            !conn.out.is_empty()
                || !conn.pending.is_empty()
                || conn.tx.open() > 0
        }) || self
            .viewers
            .values()
            .any(|v| !v.local_queue.is_empty() || !v.unacked.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::decode_client;
    use crate::model::Status;
    use crate::profile::Design;

    fn credits_sent(client: &mut Client) -> u64 {
        let profile = Rc::clone(&client.profile);
        let bytes = client.take(usize::MAX);
        let mut receiver = FrameReceiver::new(
            profile.max_frame,
            profile.max_open_streams,
            None,
        );
        let messages = receiver
            .push(&bytes, 0, &|kind| {
                crate::message::client_cap(kind, &profile)
            })
            .unwrap_or_default();
        messages
            .iter()
            .filter_map(|bytes| match decode_client(bytes, &profile, 0) {
                Ok(ClientMsg::Credit { credits, .. }) => Some(credits),
                _ => None,
            })
            .sum()
    }

    fn update(seq: u64, base: Option<u64>, credited: bool) -> ServerMsg {
        ServerMsg::Update(Update {
            viewer: 9,
            seq,
            base,
            credited,
            revision: seq,
            geometry: 1,
            columns: 1,
            cursor: (0, 0),
            modes: 0,
            styles: vec![0],
            rows: vec![RowOp::Full(vec![(0, vec!['a'])])],
            digest: None,
        })
    }

    fn credit_after_discard(design: Design) -> u64 {
        let profile = Rc::new(Profile {
            design,
            ..Profile::default()
        });
        let mut client = Client::new(1, 0, ClientKind::Text, profile);
        client.connect(1, 0);
        client
            .handle(ServerMsg::Welcome {
                minor: 0,
                incarnation: 1,
                server_id: 7,
            })
            .unwrap_or_default();
        client.show(5);
        let status = Status::default();
        client
            .handle(ServerMsg::Result {
                id: 2,
                outcome: Ok(Body::Viewer { viewer: 9, status }),
            })
            .unwrap_or_default();
        client.tick();
        assert_eq!(
            credits_sent(&mut client),
            2,
            "the viewer starts with full credit"
        );
        client.handle(update(1, None, false)).unwrap_or_default();
        // A credited delta on the wrong baseline is discarded.
        client.handle(update(2, Some(7), true)).unwrap_or_default();
        // The resync request and the credit share the viewer's lane, so the
        // credit follows on the next tick.
        client.tick();
        client.tick();
        credits_sent(&mut client)
    }

    #[test]
    fn a_discarded_credited_update_returns_its_credit() {
        assert_eq!(credit_after_discard(Design::default()), 1);
        let leaky = Design {
            credit_on_discard: false,
            ..Design::default()
        };
        assert_eq!(
            credit_after_discard(leaky),
            0,
            "the leak this rule prevents"
        );
    }
}
