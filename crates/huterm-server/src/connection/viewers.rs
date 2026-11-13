//! A connection's viewers: subscriptions, update pumps with credit, status
//! slots, ordered messages with staging, the input window, and each
//! viewer's end.
//!
//! Each viewer's pump is the only producer of its updates. It asks for one
//! snapshot at a time, and only when the viewer owes an update that spends
//! no credit (its first, a resync's, or a client's snapshot request), or
//! when the terminal published something the viewer has not seen and the
//! viewer has credit. A hidden viewer the client grants no credit therefore
//! stays notified, core does not wake it again, and it costs the wire no
//! rows. Status never waits for credit: each change replaces the viewer's
//! one unsent status, which goes out ahead of any update not yet started.
//!
//! Input, focus, and the viewer requests core orders with them (geometry,
//! presentation, history edits) are admitted in the order they arrived. One
//! that core refuses `Busy` is staged, with everything after it, and offered
//! again every [`STAGING_RETRY`]. Staged input is bounded by the input
//! window: a client keeps at most [`VIEWER_INPUT_BUDGET`] of input the
//! server has not acknowledged, so staging never overflows, and the server
//! acknowledges input once core admitted or refused it.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use huterm_core::RuntimeError;
use huterm_protocol::{
    InputStamp, MousePosition, ScrollCommand, TerminalInput, TerminalSnapshot,
    TerminalStatus, ViewerId,
};
use huterm_wire::caps::VIEWER_INPUT_BUDGET;
use huterm_wire::error::{ErrorCode, WireError};
use huterm_wire::handshake::MAX_VIEWER_CREDIT;
use huterm_wire::request::{Request, RequestId};
use huterm_wire::response::{Response, Success};
use huterm_wire::update::UpdateEncoder;
use huterm_wire::viewer::{Credit, input_cost};

use super::outbound::{Item, Lane};
use super::{CloseReason, Connection, Pending};
use crate::config::{MAX_ENDED_VIEWERS, STAGING_RETRY};
use crate::runtime::runtime_code;
use crate::viewer::{
    Ordered, SnapshotOutcome, ViewerJob, ViewerOp, ViewerPort, WakeSource,
};
use crate::work::{Completion, Job, Subscription};

/// What a viewer's snapshot in flight answers.
#[derive(Debug)]
pub(super) enum Task {
    /// The pump's update: credited, or owed without credit as a viewer's
    /// first update or a resync's complete snapshot.
    Pump {
        /// Whether it spends a credit.
        credited: bool,
    },
    /// A client's snapshot request, whose update spends no credit and
    /// arrives ahead of its result.
    Snapshot(RequestId),
    /// Selection text, a large result.
    Selection(RequestId),
    /// Selection units.
    SelectUnits(RequestId),
}

/// A client's snapshot request waiting for the viewer's snapshot in flight.
#[derive(Debug)]
struct SnapshotRequest {
    id: RequestId,
    scroll: Option<(ScrollCommand, u64)>,
    point: Option<MousePosition>,
}

/// An ordered message waiting for core.
#[derive(Debug)]
struct Staged {
    message: Ordered,
    /// The input sequence of an input, which its acknowledgement names.
    seq: Option<u64>,
    /// What an input counts against the window.
    cost: usize,
    /// The request a viewer request answers once admitted.
    request: Option<RequestId>,
}

/// One live viewer.
#[expect(
    clippy::struct_excessive_bools,
    reason = "the pump's independent conditions"
)]
pub(super) struct Viewer {
    port: Arc<dyn ViewerPort>,
    encoder: UpdateEncoder,
    /// The snapshot last planned, which a stopped terminal's complete final
    /// update shows.
    last: Option<Arc<TerminalSnapshot>>,
    /// Whether the subscription's result is on the lane, so the lane may
    /// carry the viewer's other messages.
    opened: bool,
    /// A wake that arrived before the lane opened.
    woken: bool,
    /// How the viewer ended before its lane opened; it ends once open.
    ending: Option<Ended>,
    /// The status the viewer's last poll read before its lane opened,
    /// which goes out ahead of its end.
    ending_status: Option<Arc<TerminalStatus>>,
    credit: u32,
    /// The terminal published something the viewer has not built.
    notified: bool,
    /// The viewer owes an update that spends no credit and is complete:
    /// its first, or the one that answers `ResyncViewer`.
    owed: bool,
    /// The snapshot in flight.
    building: bool,
    requests: VecDeque<SnapshotRequest>,
    staged: VecDeque<Staged>,
    /// The cost of the staged input, against the input window.
    window: usize,
    /// The last input sequence received.
    received: u64,
    /// The highest input sequence core admitted or refused.
    acked: u64,
}

impl Viewer {
    fn new(port: Arc<dyn ViewerPort>, minor: u16) -> Self {
        let id = port.id();
        Self {
            port,
            encoder: UpdateEncoder::new(id, minor),
            last: None,
            opened: false,
            woken: false,
            ending: None,
            ending_status: None,
            credit: 0,
            notified: false,
            owed: true,
            building: false,
            requests: VecDeque::new(),
            staged: VecDeque::new(),
            window: 0,
            received: 0,
            acked: 0,
        }
    }
}

/// Why a viewer ended, which its later requests answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Ended {
    Stopped,
    Revoked,
    Closed,
}

impl Ended {
    fn error(self) -> WireError {
        match self {
            Self::Stopped => {
                WireError::new(ErrorCode::Stopped, "the terminal stopped")
            }
            Self::Revoked => {
                WireError::new(ErrorCode::Revoked, "the viewer was revoked")
            }
            Self::Closed => {
                WireError::new(ErrorCode::StaleTarget, "the viewer was closed")
            }
        }
    }
}

impl Connection {
    /// The next new viewer whose wake the adapter should forward to
    /// [`Self::viewer_woke`] and, once it ends, [`Self::viewer_ended`].
    pub fn poll_watch(&mut self) -> Option<(ViewerId, Box<dyn WakeSource>)> {
        self.watches.pop_front()
    }

    /// The next viewer whose wake the adapter should stop forwarding.
    pub fn poll_unwatch(&mut self) -> Option<ViewerId> {
        self.unwatches.pop_front()
    }

    /// Live viewers.
    #[must_use]
    pub fn viewers(&self) -> usize {
        self.viewers.len()
    }

    /// Reads what changed for `viewer` after its wake signalled at `now`.
    pub fn viewer_woke(&mut self, viewer: ViewerId, now: Duration) {
        self.clock = now;
        self.poll_viewer(viewer);
        self.progress(now);
    }

    /// Ends `viewer` after its wake closed at `now`: its terminal stopped,
    /// or its revoked viewer was finalized.
    pub fn viewer_ended(&mut self, viewer: ViewerId, now: Duration) {
        self.clock = now;
        self.poll_viewer(viewer);
        if let Some(live) = self.viewers.get_mut(&viewer) {
            let update = live.port.poll();
            // The poll consumed the final status; a viewer whose lane is
            // not open yet sends it once the lane opens.
            if let Some(status) = update.status {
                if live.opened {
                    self.outbound.set_status(viewer, status);
                } else {
                    live.ending_status = Some(status);
                }
            }
            let ended = if update.revoked {
                Ended::Revoked
            } else {
                Ended::Stopped
            };
            self.end_viewer(viewer, ended);
        }
        self.progress(now);
    }

    fn poll_viewer(&mut self, id: ViewerId) {
        if self.closed.is_some() {
            return;
        }
        let Some(viewer) = self.viewers.get_mut(&id) else {
            return;
        };
        if !viewer.opened {
            viewer.woken = true;
            return;
        }
        let update = viewer.port.poll();
        if update.invalidated.is_some() {
            viewer.notified = true;
        }
        if let Some(status) = update.status {
            self.outbound.set_status(id, status);
        }
        if update.revoked {
            self.end_viewer(id, Ended::Revoked);
            return;
        }
        self.pump_viewer(id);
    }

    /// Takes a subscription's completion.
    pub(super) fn subscribed(
        &mut self,
        request: RequestId,
        outcome: Result<Subscription, WireError>,
    ) {
        self.subscribing -= 1;
        if self.closed.is_some() {
            return;
        }
        let Some(negotiated) = self.negotiated else {
            return;
        };
        match outcome {
            Ok(Subscription::Viewer { port, status }) => {
                let id = port.id();
                self.watches.push_back((id, port.wake()));
                self.viewers
                    .insert(id, Viewer::new(port, negotiated.caps.minor()));
                self.outbound.open_viewer(id);
                // The result opens the viewer's lane, ahead of every other
                // message of the viewer's.
                self.requests.remove(&request);
                self.results.push_back(super::Queued {
                    lane: Lane::Viewer(id),
                    opens: Some(id),
                    result: huterm_wire::response::RequestResult {
                        request,
                        outcome: Ok(Success {
                            response: Response::Viewer {
                                viewer: id,
                                status: Box::new((*status).clone()),
                            },
                            committed: None,
                        }),
                    },
                });
            }
            Ok(Subscription::Stopped(status)) => self.answer(
                request,
                Ok(Success {
                    response: Response::Stopped {
                        status: Box::new((*status).clone()),
                    },
                    committed: None,
                }),
            ),
            Err(error) => self.answer(request, Err(error)),
        }
    }

    /// The subscription's result reached the lane: the viewer's other
    /// messages may follow it.
    pub(super) fn open_viewer(&mut self, id: ViewerId) {
        let Some(viewer) = self.viewers.get_mut(&id) else {
            return;
        };
        viewer.opened = true;
        // Core signals a new viewer's wake when it registers; the first
        // poll after that reads its status baseline as unchanged.
        viewer.woken = false;
        if let Some(ended) = viewer.ending {
            if let Some(status) = viewer.ending_status.take() {
                self.outbound.set_status(id, status);
            }
            self.end_viewer(id, ended);
            return;
        }
        self.poll_viewer(id);
    }

    /// Starts an update for `id` if its pump owes one and may build it.
    pub(super) fn pump_viewer(&mut self, id: ViewerId) {
        if self.closed.is_some() {
            return;
        }
        if self.at_budget() {
            self.pumps_waiting = true;
            return;
        }
        let Some(viewer) = self.viewers.get_mut(&id) else {
            return;
        };
        if !viewer.opened || viewer.building {
            return;
        }
        let (task, scroll, point) =
            if let Some(request) = viewer.requests.pop_front() {
                (Task::Snapshot(request.id), request.scroll, request.point)
            } else if viewer.owed {
                (Task::Pump { credited: false }, None, None)
            } else if viewer.notified && viewer.credit > 0 {
                (Task::Pump { credited: true }, None, None)
            } else {
                return;
            };
        // Building the snapshot rearms the viewer's notification in core.
        viewer.notified = false;
        viewer.building = true;
        let job = ViewerJob {
            port: Arc::clone(&viewer.port),
            op: ViewerOp::Snapshot { scroll, point },
        };
        let work = self.work(Pending::Viewer(id, task), Job::Viewer(job));
        self.ready.push_back(work);
    }

    /// Pumps every viewer that may build, after the outbound queue drained
    /// below its budget.
    pub(super) fn pump_all(&mut self) {
        if self.at_budget() {
            return;
        }
        let ids: Vec<ViewerId> = self.viewers.keys().copied().collect();
        for id in ids {
            self.pump_viewer(id);
        }
    }

    /// Takes a viewer job's completion.
    pub(super) fn viewer_completed(
        &mut self,
        id: ViewerId,
        task: Task,
        completion: Completion,
    ) {
        self.viewer_works -= 1;
        if self.closed.is_some() {
            return;
        }
        match (task, completion) {
            (Task::Pump { credited }, Completion::Snapshot(outcome)) => {
                self.built(id, credited, None, outcome);
            }
            (Task::Snapshot(request), Completion::Snapshot(outcome)) => {
                self.built(id, false, Some(request), outcome);
            }
            (Task::Selection(request), Completion::Selection(outcome)) => {
                let outcome = outcome
                    .map(|text| Success {
                        response: Response::Text { text },
                        committed: None,
                    })
                    .map_err(|error| self.viewer_error(id, &error));
                self.answer_on(Lane::Large, request, outcome);
            }
            (Task::SelectUnits(request), Completion::SelectUnits(outcome)) => {
                let outcome = outcome
                    .map(|range| Success {
                        response: Response::Range { range },
                        committed: None,
                    })
                    .map_err(|error| self.viewer_error(id, &error));
                self.answer_on(Lane::Viewer(id), request, outcome);
            }
            (task, completion) => {
                let message = format!("{task:?} completed as {completion:?}");
                self.shut(CloseReason::Fault(message));
            }
        }
    }

    /// The error a failed viewer operation answers: why the viewer ended,
    /// when it did, or core's error.
    fn viewer_error(&self, id: ViewerId, error: &RuntimeError) -> WireError {
        if let Some(ended) = self.ended_viewer(id) {
            return ended.error();
        }
        WireError::new(runtime_code(error), error.to_string())
    }

    fn built(
        &mut self,
        id: ViewerId,
        credited: bool,
        request: Option<RequestId>,
        outcome: Result<SnapshotOutcome, RuntimeError>,
    ) {
        let Some(viewer) = self.viewers.get_mut(&id) else {
            if let Some(request) = request {
                let error =
                    self.ended_viewer(id).unwrap_or(Ended::Closed).error();
                self.answer(request, Err(error));
            }
            return;
        };
        viewer.building = false;
        let SnapshotOutcome { snapshot, link } = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                // No snapshot was built, so core still holds the viewer
                // notified and will not wake it again: keep owing it.
                viewer.notified = true;
                if let Some(request) = request {
                    let error = self.viewer_error(id, &error);
                    self.answer(request, Err(error));
                    self.pump_viewer(id);
                }
                // A pump fails only when its viewer is ending, which its
                // wake reports; trying again now would spin until then.
                return;
            }
        };
        let owed = viewer.owed;
        if owed {
            viewer.encoder.resync();
            viewer.owed = false;
        }
        let credited = credited && !owed;
        if credited {
            viewer.credit = viewer.credit.saturating_sub(1);
        }
        match viewer.encoder.plan(&snapshot, credited) {
            Ok(planned) => {
                viewer.last = Some(snapshot);
                self.outbound.push(Lane::Viewer(id), Item::Update(planned));
            }
            Err(error) => {
                self.shut(CloseReason::Fault(error.to_string()));
                return;
            }
        }
        if let Some(request) = request {
            self.answer_on(
                Lane::Viewer(id),
                request,
                Ok(Success {
                    response: Response::Snapshot { link },
                    committed: None,
                }),
            );
        }
        self.pump_viewer(id);
    }

    /// Ends a live viewer: its notice follows everything already on its
    /// lane, and its requests still waiting answer why it ended.
    pub(super) fn end_viewer(&mut self, id: ViewerId, ended: Ended) {
        // The notice must follow the subscription's result on the lane, so
        // a viewer whose result still waits for room ends once it is out.
        if let Some(viewer) = self.viewers.get_mut(&id)
            && !viewer.opened
        {
            viewer.ending.get_or_insert(ended);
            return;
        }
        let Some(mut viewer) = self.viewers.remove(&id) else {
            return;
        };
        self.unwatches.push_back(id);
        let lane = Lane::Viewer(id);
        match ended {
            Ended::Stopped => {
                // A complete final view, so a client recovering from a
                // damaged delta does not keep a stale one.
                if let Some(last) = viewer.last.take() {
                    viewer.encoder.resync();
                    if let Ok(planned) = viewer.encoder.plan(&last, false) {
                        self.outbound.push(lane, Item::Update(planned));
                    }
                }
                self.push_notice(
                    lane,
                    &huterm_wire::message::Message::ViewerStopped(
                        huterm_wire::viewer::ViewerStopped { viewer: id },
                    ),
                );
            }
            Ended::Revoked => self.push_notice(
                lane,
                &huterm_wire::message::Message::ViewerRevoked(
                    huterm_wire::viewer::ViewerRevoked { viewer: id },
                ),
            ),
            Ended::Closed => {}
        }
        self.outbound.close_viewer(id, ended != Ended::Closed);
        let error = ended.error();
        for staged in viewer.staged.drain(..) {
            if let Some(request) = staged.request {
                self.answer(request, Err(error.clone()));
            }
        }
        for request in viewer.requests.drain(..) {
            self.answer(request.id, Err(error.clone()));
        }
        if self.ended.len() >= MAX_ENDED_VIEWERS {
            self.ended.pop_front();
        }
        self.ended.push_back((id, ended));
        self.update_retry();
    }

    fn push_notice(
        &mut self,
        lane: Lane,
        message: &huterm_wire::message::Message,
    ) {
        let Some(negotiated) = self.negotiated else {
            return;
        };
        match huterm_wire::message::encode_message(
            message,
            self.scope,
            negotiated.caps,
        ) {
            Ok(bytes) => self.outbound.push_encoded(lane, bytes, None),
            Err(error) => self.shut(CloseReason::Fault(error.to_string())),
        }
    }

    pub(super) fn ended_viewer(&self, id: ViewerId) -> Option<Ended> {
        self.ended
            .iter()
            .rev()
            .find(|(ended, _)| *ended == id)
            .map(|(_, why)| *why)
    }

    /// The error a message naming an unknown viewer answers.
    fn unknown_viewer(&self, id: ViewerId) -> WireError {
        self.ended_viewer(id).map_or_else(
            || WireError::new(ErrorCode::StaleTarget, "no such viewer"),
            Ended::error,
        )
    }

    /// A request that names a viewer, handled as it arrives so it stays in
    /// order with the viewer's input.
    pub(super) fn viewer_request(&mut self, id: RequestId, request: Request) {
        let Some(viewer_id) = request.viewer() else {
            return;
        };
        if !self.viewers.contains_key(&viewer_id) {
            let error = self.unknown_viewer(viewer_id);
            self.answer(id, Err(error));
            return;
        }
        let ordered = match request {
            Request::CloseViewer { .. } => {
                self.end_viewer(viewer_id, Ended::Closed);
                self.answer(
                    id,
                    Ok(Success {
                        response: Response::Unit,
                        committed: None,
                    }),
                );
                return;
            }
            Request::Snapshot { scroll, point, .. } => {
                self.queue_snapshot(id, viewer_id, scroll, point);
                return;
            }
            Request::Selection {
                generation, range, ..
            } => {
                self.viewer_job(
                    viewer_id,
                    Task::Selection(id),
                    ViewerOp::Selection { generation, range },
                );
                return;
            }
            Request::SelectUnits {
                generation,
                anchor,
                head,
                unit,
                ..
            } => {
                self.viewer_job(
                    viewer_id,
                    Task::SelectUnits(id),
                    ViewerOp::SelectUnits {
                        generation,
                        anchor,
                        head,
                        unit,
                    },
                );
                return;
            }
            Request::ReportGeometry { grid, cell, .. } => {
                Ordered::Geometry(grid, cell)
            }
            Request::UpdatePresentation { presentation, .. } => {
                Ordered::Presentation(presentation)
            }
            Request::SetScrollback { limit, .. } => Ordered::Scrollback(limit),
            Request::ClearHistory { .. } => Ordered::ClearHistory,
            Request::Reset { .. } => Ordered::Reset,
            Request::SetScrollbackCompression { enabled, .. } => {
                Ordered::Compression(enabled)
            }
            _ => {
                self.answer(
                    id,
                    Err(WireError::new(
                        ErrorCode::Unsupported,
                        "this server does not serve that request",
                    )),
                );
                return;
            }
        };
        self.stage(
            viewer_id,
            Staged {
                message: ordered,
                seq: None,
                cost: 0,
                request: Some(id),
            },
        );
    }

    /// Queues a client's snapshot request behind the viewer's snapshot in
    /// flight, so its update follows every earlier one.
    fn queue_snapshot(
        &mut self,
        id: RequestId,
        viewer: ViewerId,
        scroll: Option<(ScrollCommand, u64)>,
        point: Option<MousePosition>,
    ) {
        // Clients number scrolls from 1; core takes 0 as a scroll already
        // superseded, which would never move the viewport.
        if scroll.is_some_and(|(_, number)| number == 0) {
            self.answer(
                id,
                Err(WireError::new(
                    ErrorCode::InvalidArgument,
                    "scroll numbers start at 1",
                )),
            );
            return;
        }
        if let Some(live) = self.viewers.get_mut(&viewer) {
            live.requests
                .push_back(SnapshotRequest { id, scroll, point });
        }
        self.pump_viewer(viewer);
    }

    fn viewer_job(&mut self, id: ViewerId, task: Task, op: ViewerOp) {
        let Some(viewer) = self.viewers.get(&id) else {
            return;
        };
        let job = ViewerJob {
            port: Arc::clone(&viewer.port),
            op,
        };
        let work = self.work(Pending::Viewer(id, task), Job::Viewer(job));
        self.ready.push_back(work);
    }

    /// One input or paste: numbered by the client, inside the window.
    pub(super) fn viewer_input(
        &mut self,
        id: ViewerId,
        seq: u64,
        stamp: InputStamp,
        input: TerminalInput,
    ) {
        let Some(viewer) = self.viewers.get_mut(&id) else {
            // Input for a viewer that ended is dropped, as for any viewer
            // whose connection ended.
            return;
        };
        if seq != viewer.received + 1 {
            let received = viewer.received;
            self.shut(CloseReason::Violation(format!(
                "input {seq} after {received}"
            )));
            return;
        }
        let cost = input_cost(&input);
        if viewer.window + cost > VIEWER_INPUT_BUDGET {
            self.shut(CloseReason::Violation(
                "input past the viewer's window".into(),
            ));
            return;
        }
        viewer.received = seq;
        viewer.window += cost;
        self.stage(
            id,
            Staged {
                message: Ordered::Input { input, stamp },
                seq: Some(seq),
                cost,
                request: None,
            },
        );
    }

    /// A focus report, ordered with input and outside the window.
    pub(super) fn viewer_focus(&mut self, id: ViewerId, focused: bool) {
        let Some(viewer) = self.viewers.get_mut(&id) else {
            return;
        };
        // Focus is state: a report staged behind another with nothing
        // between them replaces it, so staged focus reports never outnumber
        // the input and requests around them.
        if let Some(Staged {
            message: Ordered::Focus(staged),
            ..
        }) = viewer.staged.back_mut()
        {
            *staged = focused;
            return;
        }
        self.stage(
            id,
            Staged {
                message: Ordered::Focus(focused),
                seq: None,
                cost: 0,
                request: None,
            },
        );
    }

    /// A credit grant.
    pub(super) fn viewer_credit(&mut self, credit: Credit) {
        let Some(viewer) = self.viewers.get_mut(&credit.viewer) else {
            return;
        };
        let granted = viewer.credit.saturating_add(credit.grant);
        if granted > MAX_VIEWER_CREDIT {
            self.shut(CloseReason::Violation(
                "credit past the viewer's limit".into(),
            ));
            return;
        }
        viewer.credit = granted;
        self.pump_viewer(credit.viewer);
    }

    /// A client's request for a complete snapshot.
    pub(super) fn viewer_resync(&mut self, id: ViewerId) {
        let Some(viewer) = self.viewers.get_mut(&id) else {
            return;
        };
        viewer.owed = true;
        self.pump_viewer(id);
    }

    fn stage(&mut self, id: ViewerId, staged: Staged) {
        let Some(viewer) = self.viewers.get_mut(&id) else {
            return;
        };
        viewer.staged.push_back(staged);
        if viewer.staged.len() == 1 {
            self.admit(id);
        }
        self.update_retry();
    }

    /// Offers `id`'s staged messages to core in order until one is refused
    /// `Busy`.
    fn admit(&mut self, id: ViewerId) {
        let mut answers = Vec::new();
        let Some(viewer) = self.viewers.get_mut(&id) else {
            return;
        };
        let acked = viewer.acked;
        while let Some(staged) = viewer.staged.pop_front() {
            let Staged {
                message,
                seq,
                cost,
                request,
            } = staged;
            let outcome = match viewer.port.offer(message) {
                Ok(()) => Ok(()),
                Err((RuntimeError::Busy, message)) => {
                    viewer.staged.push_front(Staged {
                        message,
                        seq,
                        cost,
                        request,
                    });
                    break;
                }
                Err((error, _)) => Err(error),
            };
            // Input core admitted or refused for good leaves the window.
            if let Some(seq) = seq {
                viewer.acked = viewer.acked.max(seq);
                viewer.window -= cost;
            }
            if let Some(request) = request {
                answers.push((request, outcome));
            }
        }
        let new_ack = (viewer.acked > acked).then_some(viewer.acked);
        if let Some(seq) = new_ack {
            self.outbound.set_ack(id, seq);
        }
        for (request, outcome) in answers {
            let outcome = outcome
                .map(|()| Success {
                    response: Response::Unit,
                    committed: None,
                })
                .map_err(|error| {
                    WireError::new(runtime_code(&error), error.to_string())
                });
            self.answer_on(Lane::Viewer(id), request, outcome);
        }
    }

    /// Offers every viewer's staged messages again.
    pub(super) fn retry_staged(&mut self) {
        let ids: Vec<ViewerId> = self
            .viewers
            .iter()
            .filter(|(_, viewer)| !viewer.staged.is_empty())
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.admit(id);
        }
        self.update_retry();
    }

    /// Arms the staging retry while something is staged, and only then.
    fn update_retry(&mut self) {
        let staged = self
            .viewers
            .values()
            .any(|viewer| !viewer.staged.is_empty());
        self.retry_at = match (staged, self.retry_at) {
            (false, _) => None,
            (true, Some(at)) => Some(at),
            (true, None) => Some(self.clock + STAGING_RETRY),
        };
    }

    /// Drops every viewer as a connection ends: admitted input still runs,
    /// and staged input is discarded.
    pub(super) fn drop_viewers(&mut self) {
        for (id, _) in std::mem::take(&mut self.viewers) {
            self.unwatches.push_back(id);
        }
        self.watches.clear();
        self.retry_at = None;
    }
}
