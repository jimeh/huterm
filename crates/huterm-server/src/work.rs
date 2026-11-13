//! What a connection hands to a worker, and what the worker hands back.
//!
//! A [`Work`] carries everything its job needs, moved out of the
//! connection's tables, such as a close ticket's consent or a layout
//! history record, so the connection never touches the `Mux` and a job
//! never touches the connection. A [`Completion`] carries back what the job
//! created. Both are plain values, so a test or a fault-injection harness
//! can hold, reorder, or drop them.

use std::sync::Arc;

use huterm_core::{
    CloseAssessment, CloseRequest, RestoreDirection, RuntimeError,
};
use huterm_protocol::{
    AttachmentId, BufferRange, DigestProfile, LayoutChange, SessionId,
    TerminalId, TerminalStatus,
};
use huterm_wire::error::WireError;
use huterm_wire::request::Request;
use huterm_wire::response::Success;

use crate::feed::{FeedEncoding, HierarchyFeed, SnapshotKind};
use crate::server::Live;
use crate::viewer::{SnapshotOutcome, ViewerJob, ViewerPort};

/// A work item's identity on its connection.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorkId(pub(crate) u64);

impl WorkId {
    /// The identity's number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One job a connection hands out, to run on a worker with
/// [`crate::runtime::execute`] and complete with
/// [`crate::Connection::complete`].
#[derive(Debug)]
pub struct Work {
    /// The job's identity, which its completion names.
    pub id: WorkId,
    /// What to run.
    pub job: Job,
}

/// What a worker runs.
#[derive(Debug)]
pub enum Job {
    /// A client's request.
    Request(RequestJob),
    /// A hierarchy replacement: a snapshot and a new feed captured under
    /// one lock.
    Replace(ReplaceJob),
    /// Disconnect cleanup: detach the connection's attachments.
    Detach(Vec<AttachmentId>),
    /// A viewer operation that answers later. It blocks no thread: an
    /// adapter starts it with [`ViewerJob::start`] and awaits the result.
    Viewer(ViewerJob),
}

/// A request to run, with the digest profile its committed sequence's
/// digest uses when the connection holds a hierarchy subscription.
#[derive(Debug)]
pub struct RequestJob {
    /// The operation.
    pub operation: Operation,
    /// The profile of the digest that `Committed` carries, if any.
    pub digest: Option<DigestProfile>,
}

/// A request's operation, with what the connection moved into it.
#[derive(Debug)]
pub enum Operation {
    /// A request that needs nothing from the connection's tables.
    Plain(Request),
    /// Prepare a close under the lock and assess its jobs off it.
    PrepareClose {
        /// The ticket the connection reserved for it.
        ticket: u32,
        /// The close.
        request: CloseRequest,
    },
    /// Recheck a prepared close off the lock and commit it under it.
    CommitClose {
        /// The ticket's consent, returned when the commit needs
        /// confirmation.
        consent: Consent,
        /// Whether the user confirmed.
        confirmed: bool,
        /// The connection's attachments, so the completion reports those
        /// the commit ended.
        attachments: Vec<AttachmentId>,
    },
    /// Attach to a session.
    AttachSession {
        /// The session.
        session: SessionId,
        /// The connection's attachments, so the completion reports those
        /// that no longer exist.
        attachments: Vec<AttachmentId>,
    },
    /// Close a session, which ends its attachments.
    CloseSession {
        /// The session.
        session: SessionId,
        /// The connection's attachments, so the completion reports those
        /// the close ended.
        attachments: Vec<AttachmentId>,
    },
    /// Register a viewer of a terminal in one of the connection's
    /// attachments, and read the terminal's status under the same lock.
    Subscribe {
        /// The attachment.
        attachment: AttachmentId,
        /// The terminal.
        terminal: TerminalId,
        /// The viewer's capabilities and initial state, with its own input
        /// budget.
        options: Box<huterm_core::ViewerOptions>,
    },
    /// Restore a layout history entry.
    Restore {
        /// The entry's record, taken out of its slot while the restore
        /// runs.
        record: HistoryRecord,
        /// Which way.
        direction: RestoreDirection,
    },
}

/// A hierarchy replacement to capture.
#[derive(Clone, Copy, Debug)]
pub struct ReplaceJob {
    /// How the connection encodes hierarchy messages.
    pub encoding: FeedEncoding,
    /// Which message carries the snapshot.
    pub kind: SnapshotKind,
}

/// What a job hands back.
#[derive(Debug)]
pub enum Completion {
    /// A request's outcome, with nothing else to keep.
    Answered(Result<Success, WireError>),
    /// A prepared close: its result and the assessment its ticket keeps.
    Prepared(Result<(Success, CloseAssessment), WireError>),
    /// A close commit: its result, the consent back when the commit needs
    /// confirmation, and the connection's attachments it ended.
    Committed {
        /// The result.
        outcome: Result<Success, WireError>,
        /// The consent, when the ticket stays.
        consent: Option<Consent>,
        /// Attachments the commit ended.
        ended: Vec<AttachmentId>,
    },
    /// A session close: its result and the connection's attachments it
    /// ended.
    Ended {
        /// The result.
        outcome: Result<Success, WireError>,
        /// Attachments the close ended.
        ended: Vec<AttachmentId>,
    },
    /// An attachment the connection now owns, and the connection's
    /// attachments that no longer exist.
    Attached {
        /// The result and the new attachment.
        outcome: Result<(Success, AttachmentId), WireError>,
        /// Attachments another caller ended.
        ended: Vec<AttachmentId>,
    },
    /// A rearrangement and the change its history slot files.
    Rearranged(Result<(Success, LayoutChange), WireError>),
    /// A restore's result and the record it restored, which the slot files
    /// the other way on success.
    Restored {
        /// The result.
        outcome: Result<Success, WireError>,
        /// The record.
        record: HistoryRecord,
    },
    /// A hierarchy replacement.
    Replaced(Result<Replaced, WireError>),
    /// Cleanup finished.
    Cleaned,
    /// A viewer subscription.
    Subscribed(Result<Subscription, WireError>),
    /// A viewer snapshot.
    Snapshot(Result<SnapshotOutcome, RuntimeError>),
    /// Selection text.
    Selection(Result<Option<String>, RuntimeError>),
    /// Selection units.
    SelectUnits(Result<Option<BufferRange>, RuntimeError>),
}

/// What a subscription produced.
pub enum Subscription {
    /// A viewer, and its terminal's status when it registered.
    Viewer {
        /// The viewer.
        port: Arc<dyn ViewerPort>,
        /// The status.
        status: Arc<TerminalStatus>,
    },
    /// The terminal's runtime stopped on its own; its final status.
    Stopped(Arc<TerminalStatus>),
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Viewer { port, status } => f
                .debug_struct("Viewer")
                .field("viewer", &port.id())
                .field("revision", &status.revision)
                .finish(),
            Self::Stopped(status) => {
                f.debug_tuple("Stopped").field(&status.revision).finish()
            }
        }
    }
}

/// A captured hierarchy replacement.
#[derive(Debug)]
pub struct Replaced {
    /// The snapshot's sequence.
    pub seq: u64,
    /// The snapshot's digest.
    pub digest: u64,
    /// The snapshot, encoded as its whole message.
    pub message: Vec<u8>,
    /// The events after the snapshot.
    pub feed: Box<dyn HierarchyFeed>,
}

/// A close ticket's assessment, counted as live until dropped.
#[derive(Debug)]
pub struct Consent {
    assessment: CloseAssessment,
    _live: Live,
}

impl Consent {
    pub(crate) fn new(assessment: CloseAssessment, live: Live) -> Self {
        Self {
            assessment,
            _live: live,
        }
    }

    /// The assessment the user consents to.
    #[must_use]
    pub fn assessment(&self) -> &CloseAssessment {
        &self.assessment
    }
}

/// A layout change a history slot keeps, counted as live until dropped.
#[derive(Debug)]
pub struct HistoryRecord {
    change: LayoutChange,
    _live: Live,
}

impl HistoryRecord {
    pub(crate) fn new(change: LayoutChange, live: Live) -> Self {
        Self {
            change,
            _live: live,
        }
    }

    /// The recorded change.
    #[must_use]
    pub fn change(&self) -> &LayoutChange {
        &self.change
    }
}
