//! The session's viewers: subscriptions, update application, credit, the
//! input window, scroll numbering, and status baselines that outlive a
//! connection.
//!
//! A viewer's input, focus reports, and the requests that name it leave in
//! the order the caller issued them. Input past the window, which is the
//! server's [`TransportProfile::viewer_input_budget`] of unacknowledged
//! input, waits locally with everything after it, as the desktop's input
//! queue does in process; input in flight when a connection breaks may be
//! lost but is never sent again.
//!
//! A visible viewer keeps the server's credit window full: it grants the
//! whole window when it becomes visible and returns one credit for every
//! credited update it disposes of, applied or discarded. A hidden viewer
//! returns nothing, so the server stops sending it rows while its status
//! keeps arriving.
//!
//! Per server incarnation the session keeps each terminal's status
//! baseline and the final view of each stopped terminal, so a viewer that
//! subscribes again after a reconnect to the same incarnation reports the
//! exit, bells, and failures it missed.
//!
//! [`TransportProfile::viewer_input_budget`]:
//! huterm_wire::handshake::TransportProfile::viewer_input_budget

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use huterm_protocol::{
    AttachmentId, InputStamp, ScrollCommand, StatusBaseline, StatusChanges,
    TerminalId, TerminalInput, TerminalSnapshot, TerminalStatus, ViewerId,
};
use huterm_wire::error::{ErrorCode, WireError};
use huterm_wire::message::{Message, Paste, encode_message};
use huterm_wire::request::Request;
use huterm_wire::response::{Response, Success};
use huterm_wire::update::{
    TerminalUpdate, UpdateOutcome, ViewError, ViewerView,
};
use huterm_wire::viewer::{
    Credit, Focus, Input, InputAck, ResyncViewer, SubscribeOptions,
    ViewerStatus, input_cost,
};

use super::{CallId, ClientEvent, ClientSession, CloseReason};
use crate::outbound::Transmit;

/// Something that happened to a viewer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ViewerEvent {
    /// The view applied an update.
    Updated {
        /// The viewer.
        viewer: ViewerId,
        /// The update's number.
        seq: u64,
    },
    /// The terminal's status changed, with what changed since the
    /// viewer's baseline. A stopped terminal's subscription reports its
    /// final status here with no viewer.
    Status {
        /// The terminal.
        terminal: TerminalId,
        /// The viewer, when one is live.
        viewer: Option<ViewerId>,
        /// The status.
        status: Arc<TerminalStatus>,
        /// Exits, bells, and failures since the baseline.
        changes: StatusChanges,
    },
    /// The server acknowledged input up to `seq`.
    InputAcked {
        /// The viewer.
        viewer: ViewerId,
        /// The highest input sequence core admitted or refused.
        seq: u64,
    },
    /// The terminal stopped; its final view stays readable through
    /// [`ClientSession::final_view`].
    Stopped {
        /// The viewer.
        viewer: ViewerId,
        /// The terminal.
        terminal: TerminalId,
    },
    /// The viewer's authority was withdrawn.
    Revoked {
        /// The viewer.
        viewer: ViewerId,
        /// The terminal.
        terminal: TerminalId,
    },
    /// A complete snapshot disagreed with its own digest, so the session
    /// closed the viewer without asking for another.
    Fault {
        /// The viewer.
        viewer: ViewerId,
    },
}

/// Something a viewer sends, in the order the caller issued it.
#[derive(Debug)]
enum Outgoing {
    Input {
        seq: u64,
        stamp: InputStamp,
        input: TerminalInput,
        cost: usize,
    },
    Focus(bool),
    Call(CallId),
}

/// One live viewer.
#[derive(Debug)]
pub(super) struct ClientViewer {
    terminal: TerminalId,
    view: ViewerView,
    baseline: StatusBaseline,
    status: Option<Arc<TerminalStatus>>,
    visible: bool,
    /// Credits the server holds for this viewer.
    outstanding: u32,
    /// The last input sequence assigned.
    next_input: u64,
    /// Input sent and not yet acknowledged, oldest first.
    unacked: VecDeque<(u64, usize)>,
    unacked_cost: usize,
    pending: VecDeque<Outgoing>,
    /// The last scroll number assigned, which typing stamps carry.
    scrolls: u64,
}

/// What a subscription in flight subscribes to.
#[derive(Debug)]
pub(super) struct Subscribing {
    terminal: TerminalId,
    spawned: bool,
    visible: bool,
}

/// The most ended viewers a session remembers on one connection, so a
/// request naming one answers why it ended instead of `StaleTarget`.
const MAX_ENDED_VIEWERS: usize = 256;

/// Viewers that ended on the current connection, oldest first, with why.
#[derive(Debug, Default)]
pub(super) struct EndedViewers(VecDeque<(ViewerId, ErrorCode)>);

impl EndedViewers {
    fn insert(&mut self, viewer: ViewerId, code: ErrorCode) {
        if self.0.len() >= MAX_ENDED_VIEWERS {
            self.0.pop_front();
        }
        self.0.push_back((viewer, code));
    }

    /// Why `viewer` ended, if it did on this connection.
    pub(super) fn code(&self, viewer: ViewerId) -> Option<ErrorCode> {
        self.0
            .iter()
            .rev()
            .find(|(ended, _)| *ended == viewer)
            .map(|(_, code)| *code)
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }
}

/// What the session keeps per incarnation.
#[derive(Debug, Default)]
pub(super) struct Carried {
    baselines: HashMap<TerminalId, StatusBaseline>,
    final_views: HashMap<TerminalId, TerminalSnapshot>,
}

impl ClientSession {
    /// Subscribes a viewer of `terminal` in `attachment` at `now`. The
    /// result carries the viewer, and its first update follows, which
    /// spends no credit. A `visible` viewer keeps the credit window full
    /// from the start; a hidden one, such as a background tab's, gets no
    /// rows until [`Self::set_visible`] shows it.
    pub fn subscribe_terminal(
        &mut self,
        attachment: AttachmentId,
        terminal: TerminalId,
        options: SubscribeOptions,
        visible: bool,
        now: std::time::Duration,
    ) -> CallId {
        let spawned = options.spawned;
        let call = self.request(
            Request::SubscribeTerminal {
                attachment,
                terminal,
                options,
            },
            now,
        );
        if self.calls.contains_key(&call) {
            self.subscribing.insert(
                call,
                Subscribing {
                    terminal,
                    spawned,
                    visible,
                },
            );
        }
        call
    }

    /// Shows or hides `viewer`: a visible viewer keeps the credit window
    /// full, and a hidden one returns no credit, so its terminal sends it
    /// no rows until it is shown again.
    pub fn set_visible(&mut self, viewer: ViewerId, visible: bool) {
        let window = self.credit_window();
        let Some(entry) = self.viewers.get_mut(&viewer) else {
            return;
        };
        entry.visible = visible;
        if visible && entry.outstanding < window {
            let grant = window - entry.outstanding;
            entry.outstanding = window;
            let applied = entry.view.applied();
            self.send_message(&Message::Credit(Credit {
                viewer,
                grant,
                applied,
            }));
        }
    }

    /// Sends `input` from `viewer`, stamped now with the geometry revision
    /// its coordinates were computed against. It waits locally while the
    /// window is full.
    ///
    /// # Errors
    /// Returns `StaleTarget` for a viewer this session does not hold.
    pub fn send_input(
        &mut self,
        viewer: ViewerId,
        input: TerminalInput,
        geometry: Option<u64>,
    ) -> Result<u64, WireError> {
        let Some(entry) = self.viewers.get_mut(&viewer) else {
            return Err(self.unknown_viewer(viewer));
        };
        entry.next_input += 1;
        let seq = entry.next_input;
        let cost = input_cost(&input);
        entry.pending.push_back(Outgoing::Input {
            seq,
            stamp: InputStamp {
                scrolls: entry.scrolls,
                geometry,
            },
            input,
            cost,
        });
        self.flush_viewer(viewer);
        Ok(seq)
    }

    /// Reports whether `viewer`'s view holds focus, in order with its
    /// input.
    pub fn send_focus(&mut self, viewer: ViewerId, focused: bool) {
        let Some(entry) = self.viewers.get_mut(&viewer) else {
            return;
        };
        entry.pending.push_back(Outgoing::Focus(focused));
        self.flush_viewer(viewer);
    }

    /// Scrolls `viewer`'s shared viewport, numbering the scroll so typing
    /// after it carries the number. The scroll's update arrives before its
    /// result.
    pub fn scroll(
        &mut self,
        viewer: ViewerId,
        command: ScrollCommand,
        now: std::time::Duration,
    ) -> CallId {
        let number = self.viewers.get_mut(&viewer).map_or(0, |entry| {
            entry.scrolls += 1;
            entry.scrolls
        });
        self.request(
            Request::Snapshot {
                viewer,
                scroll: Some((command, number)),
                point: None,
            },
            now,
        )
    }

    /// The view `viewer` shows, once an update applied.
    #[must_use]
    pub fn view(&self, viewer: ViewerId) -> Option<TerminalSnapshot> {
        let entry = self.viewers.get(&viewer)?;
        entry.view.snapshot(entry.terminal)
    }

    /// The terminal status `viewer` last received.
    #[must_use]
    pub fn viewer_status(&self, viewer: ViewerId) -> Option<&TerminalStatus> {
        self.viewers.get(&viewer)?.status.as_deref()
    }

    /// The final view of a terminal that stopped while this incarnation's
    /// viewer showed it.
    #[must_use]
    pub fn final_view(
        &self,
        terminal: TerminalId,
    ) -> Option<&TerminalSnapshot> {
        self.carried.final_views.get(&terminal)
    }

    /// Live viewers.
    #[must_use]
    pub fn viewer_ids(&self) -> Vec<ViewerId> {
        self.viewers.keys().copied().collect()
    }

    /// Input of `viewer` sent and not yet acknowledged, in bytes.
    #[must_use]
    pub fn unacknowledged_input(&self, viewer: ViewerId) -> usize {
        self.viewers
            .get(&viewer)
            .map_or(0, |entry| entry.unacked_cost)
    }

    fn credit_window(&self) -> u32 {
        self.profile()
            .map_or(huterm_wire::handshake::MAX_VIEWER_CREDIT, |profile| {
                profile.max_viewer_credit
            })
    }

    fn input_budget(&self) -> usize {
        self.profile().map_or(
            huterm_wire::caps::VIEWER_INPUT_BUDGET,
            |profile| {
                usize::try_from(profile.viewer_input_budget)
                    .unwrap_or(usize::MAX)
            },
        )
    }

    /// Encodes and queues a viewer message, or ends the connection when
    /// this session cannot encode its own message.
    fn send_message(&mut self, message: &Message) {
        let Some(connection) = self.connection.as_mut() else {
            return;
        };
        let Some(open) = connection.open.as_ref() else {
            return;
        };
        match encode_message(message, open.scope(), open.negotiated.caps) {
            Ok(bytes) => connection
                .outbound
                .push_back(Transmit::message(bytes, open.negotiated.max_frame)),
            Err(error) => self.close(CloseReason::Violation(format!(
                "the client's {:?} does not encode: {error}",
                message.kind()
            ))),
        }
    }

    /// Whether the open connection has room for another request.
    fn request_room(&self) -> bool {
        self.connection
            .as_ref()
            .and_then(|connection| {
                Some(
                    connection.sent.len()
                        < connection.open.as_ref()?.max_in_flight,
                )
            })
            .unwrap_or(false)
    }

    /// Sends `viewer`'s waiting messages in order while the window and the
    /// in-flight limit allow.
    pub(super) fn flush_viewer(&mut self, viewer: ViewerId) {
        let budget = self.input_budget();
        loop {
            let open = self
                .connection
                .as_ref()
                .is_some_and(|connection| connection.open.is_some());
            if !open {
                return;
            }
            let room = self.request_room();
            let Some(entry) = self.viewers.get_mut(&viewer) else {
                return;
            };
            let message = match entry.pending.front() {
                None => return,
                Some(Outgoing::Input { cost, .. })
                    if entry.unacked_cost + cost > budget
                        && !entry.unacked.is_empty() =>
                {
                    return;
                }
                Some(Outgoing::Call(_)) if !room => return,
                Some(_) => entry.pending.pop_front(),
            };
            match message {
                Some(Outgoing::Input {
                    seq,
                    stamp,
                    input,
                    cost,
                }) => {
                    entry.unacked.push_back((seq, cost));
                    entry.unacked_cost += cost;
                    let message = match input {
                        TerminalInput::Paste(text) => Message::Paste(Paste {
                            viewer,
                            seq,
                            stamp,
                            text,
                        }),
                        input => Message::Input(Input {
                            viewer,
                            seq,
                            stamp,
                            input,
                        }),
                    };
                    self.send_message(&message);
                }
                Some(Outgoing::Focus(focused)) => {
                    self.send_message(&Message::Focus(Focus {
                        viewer,
                        focused,
                    }));
                }
                Some(Outgoing::Call(call)) => {
                    if let Err(error) = self.send(call) {
                        self.finish(call, Err(error));
                    }
                }
                None => return,
            }
        }
    }

    /// Sends every viewer's waiting messages that may go now.
    pub(super) fn flush_viewers(&mut self) {
        let ids: Vec<ViewerId> = self
            .viewers
            .iter()
            .filter(|(_, entry)| !entry.pending.is_empty())
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.flush_viewer(id);
        }
    }

    /// The error a call naming `viewer`, which the session does not hold,
    /// answers: why it ended on this connection, or `StaleTarget`.
    pub(super) fn unknown_viewer(&self, viewer: ViewerId) -> WireError {
        match self.ended.code(viewer) {
            Some(ErrorCode::Stopped) => {
                WireError::new(ErrorCode::Stopped, "the terminal stopped")
            }
            Some(ErrorCode::Revoked) => {
                WireError::new(ErrorCode::Revoked, "the viewer was revoked")
            }
            _ => WireError::new(ErrorCode::StaleTarget, "no such viewer"),
        }
    }

    /// Queues a call that names `viewer` behind the viewer's waiting
    /// messages, or reports that it names no live viewer.
    pub(super) fn queue_on_viewer(
        &mut self,
        viewer: ViewerId,
        call: CallId,
    ) -> bool {
        let Some(entry) = self.viewers.get_mut(&viewer) else {
            return false;
        };
        entry.pending.push_back(Outgoing::Call(call));
        true
    }

    /// Takes a subscription's success.
    pub(super) fn subscribed(&mut self, call: CallId, success: &Success) {
        let Some(Subscribing {
            terminal,
            spawned,
            visible,
        }) = self.subscribing.remove(&call)
        else {
            return;
        };
        let minor = self.minor;
        match &success.response {
            Response::Viewer { viewer, status } => {
                let baseline =
                    self.carried.baselines.remove(&terminal).unwrap_or_else(
                        || {
                            if spawned {
                                StatusBaseline::SPAWNED
                            } else {
                                StatusBaseline::seen(status)
                            }
                        },
                    );
                let status = Arc::new((**status).clone());
                self.viewers.insert(
                    *viewer,
                    ClientViewer {
                        terminal,
                        view: ViewerView::new(minor),
                        baseline,
                        status: None,
                        visible: false,
                        outstanding: 0,
                        next_input: 0,
                        unacked: VecDeque::new(),
                        unacked_cost: 0,
                        pending: VecDeque::new(),
                        scrolls: 0,
                    },
                );
                self.observe(terminal, Some(*viewer), status);
                self.set_visible(*viewer, visible);
            }
            Response::Stopped { status } => {
                let status = Arc::new((**status).clone());
                let mut baseline =
                    self.carried.baselines.remove(&terminal).unwrap_or_else(
                        || {
                            if spawned {
                                StatusBaseline::SPAWNED
                            } else {
                                StatusBaseline::seen(&status)
                            }
                        },
                    );
                if let Some(changes) = baseline.observe(&status) {
                    self.events.push_back(ClientEvent::Viewer(
                        ViewerEvent::Status {
                            terminal,
                            viewer: None,
                            status,
                            changes,
                        },
                    ));
                }
                self.carried.baselines.insert(terminal, baseline);
            }
            _ => {}
        }
    }

    /// Observes `status` against the viewer's baseline, reporting what
    /// changed.
    fn observe(
        &mut self,
        terminal: TerminalId,
        viewer: Option<ViewerId>,
        status: Arc<TerminalStatus>,
    ) {
        let Some(entry) =
            viewer.and_then(|viewer| self.viewers.get_mut(&viewer))
        else {
            return;
        };
        entry.status = Some(Arc::clone(&status));
        if let Some(changes) = entry.baseline.observe(&status) {
            self.events
                .push_back(ClientEvent::Viewer(ViewerEvent::Status {
                    terminal,
                    viewer,
                    status,
                    changes,
                }));
        }
    }

    /// Takes a viewer message from the server.
    pub(super) fn viewer_message(&mut self, message: Message) {
        match message {
            Message::TerminalUpdate(update) => self.update(&update),
            Message::ViewerStatus(ViewerStatus { viewer, status }) => {
                let Some(terminal) =
                    self.viewers.get(&viewer).map(|entry| entry.terminal)
                else {
                    return;
                };
                self.observe(terminal, Some(viewer), Arc::new(status));
            }
            Message::InputAck(InputAck { viewer, seq }) => {
                let Some(entry) = self.viewers.get_mut(&viewer) else {
                    return;
                };
                if seq > entry.next_input {
                    self.close(CloseReason::Violation(format!(
                        "an acknowledgement of input {seq} not yet sent"
                    )));
                    return;
                }
                while entry
                    .unacked
                    .front()
                    .is_some_and(|(sent, _)| *sent <= seq)
                {
                    if let Some((_, cost)) = entry.unacked.pop_front() {
                        entry.unacked_cost -= cost;
                    }
                }
                self.events.push_back(ClientEvent::Viewer(
                    ViewerEvent::InputAcked { viewer, seq },
                ));
                self.flush_viewer(viewer);
            }
            Message::ViewerStopped(stopped) => {
                let Some(entry) = self.viewers.remove(&stopped.viewer) else {
                    return;
                };
                if let Some(view) = entry.view.snapshot(entry.terminal) {
                    self.carried.final_views.insert(entry.terminal, view);
                }
                self.carried
                    .baselines
                    .insert(entry.terminal, entry.baseline);
                self.end_pending(entry.pending, ErrorCode::Stopped);
                self.ended.insert(stopped.viewer, ErrorCode::Stopped);
                self.events.push_back(ClientEvent::Viewer(
                    ViewerEvent::Stopped {
                        viewer: stopped.viewer,
                        terminal: entry.terminal,
                    },
                ));
            }
            Message::ViewerRevoked(revoked) => {
                let Some(entry) = self.viewers.remove(&revoked.viewer) else {
                    return;
                };
                self.carried
                    .baselines
                    .insert(entry.terminal, entry.baseline);
                self.end_pending(entry.pending, ErrorCode::Revoked);
                self.ended.insert(revoked.viewer, ErrorCode::Revoked);
                self.events.push_back(ClientEvent::Viewer(
                    ViewerEvent::Revoked {
                        viewer: revoked.viewer,
                        terminal: entry.terminal,
                    },
                ));
            }
            _ => {}
        }
    }

    /// Fails a viewer's calls that never left, after the viewer ended.
    fn end_pending(&mut self, pending: VecDeque<Outgoing>, code: ErrorCode) {
        for outgoing in pending {
            if let Outgoing::Call(call) = outgoing {
                self.finish(
                    call,
                    Err(WireError::new(code, "the viewer ended")),
                );
            }
        }
    }

    fn update(&mut self, update: &TerminalUpdate) {
        let viewer = update.viewer;
        let Some(entry) = self.viewers.get_mut(&viewer) else {
            // Updates queued before a viewer ended may still arrive.
            return;
        };
        if update.credited {
            if entry.outstanding == 0 {
                self.close(CloseReason::Violation(
                    "a credited update without credit".into(),
                ));
                return;
            }
            entry.outstanding -= 1;
        }
        match entry.view.apply(update) {
            Ok(disposal) => {
                let applied = entry.view.applied();
                let window = self.credit_window();
                let Some(entry) = self.viewers.get_mut(&viewer) else {
                    return;
                };
                let grant = disposal.return_credit
                    && entry.visible
                    && entry.outstanding < window;
                if grant {
                    entry.outstanding += 1;
                }
                match disposal.outcome {
                    UpdateOutcome::Applied => {
                        self.events.push_back(ClientEvent::Viewer(
                            ViewerEvent::Updated {
                                viewer,
                                seq: update.seq,
                            },
                        ));
                    }
                    UpdateOutcome::Resync => {
                        self.stats.viewer_resyncs += 1;
                        self.send_message(&Message::ResyncViewer(
                            ResyncViewer { viewer },
                        ));
                    }
                    UpdateOutcome::Stale => {}
                }
                if grant {
                    self.send_message(&Message::Credit(Credit {
                        viewer,
                        grant: 1,
                        applied,
                    }));
                }
            }
            Err(ViewError::ContractFault) => {
                self.viewers.remove(&viewer);
                self.ended.insert(viewer, ErrorCode::StaleTarget);
                self.events.push_back(ClientEvent::Viewer(
                    ViewerEvent::Fault { viewer },
                ));
                let call = self.issue(Request::CloseViewer { viewer }, true);
                let _ = call;
                self.flush_queue();
            }
            Err(ViewError::Invalid(field)) => {
                self.close(CloseReason::Violation(format!(
                    "an update that does not apply: {field}"
                )));
            }
        }
    }

    /// Keeps every live viewer's status baseline for a reconnect to the
    /// same incarnation, as the connection ends.
    pub(super) fn carry_viewers(&mut self) {
        self.ended.clear();
        for (_, entry) in std::mem::take(&mut self.viewers) {
            self.carried
                .baselines
                .insert(entry.terminal, entry.baseline);
        }
        self.subscribing.clear();
    }
}
