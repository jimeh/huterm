//! What a connection holds for each of its viewers: a [`ViewerPort`].
//!
//! Production ports are core's `TerminalViewer`s. A connection calls a
//! port's non-blocking methods on its own thread: polling what changed and
//! admitting ordered messages such as input, so a keystroke reaches core
//! without a worker hop. Snapshots, selection text, and selection units
//! answer later, so they travel as [`ViewerJob`]s whose futures the adapter
//! awaits on its executor; none of them blocks a thread. A test or the
//! fault-injection harness supplies ports of its own, such as a model
//! terminal driven by a seeded generator.
//!
//! A viewer subscribed with host effects also hands the connection the
//! effects core admitted for its recipient, one [`PendingEffect`] at a
//! time, and takes the client's numbered permission changes.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use huterm_core::{
    PendingHostEffect, RuntimeError, TerminalViewer, ViewerUpdate,
};
use huterm_protocol::{
    BufferPoint, BufferRange, CellSize, GridSize, HostEffect, HostEffectKind,
    InputStamp, LinkLookup, MousePosition, ScrollCommand, ScrollbackLimit,
    SelectionUnit, TerminalId, TerminalInput, TerminalPresentation,
    TerminalSnapshot, ViewerId,
};
use huterm_wire::effect::EffectPayloadRef;

use crate::work::Completion;

/// A future a port's reply resolves through.
pub type Reply<T> =
    Pin<Box<dyn Future<Output = Result<T, RuntimeError>> + Send>>;

/// A future that resolves `true` when a viewer has something to poll, and
/// `false` once its terminal stopped or its revoked viewer was finalized.
pub type Wake = Pin<Box<dyn Future<Output = bool> + Send>>;

/// A viewer's wake signal, awaited by the adapter's forwarding task.
pub trait WakeSource: Send {
    /// Waits for the next signal.
    fn wait(&self) -> Wake;
}

/// An ordered message a viewer sends core: it queues behind the viewer's
/// earlier input and may be refused `Busy` while core's queue is full.
#[derive(Clone, Debug)]
pub enum Ordered {
    /// Input, stamped when the user produced it.
    Input {
        /// The input.
        input: TerminalInput,
        /// What the client knew then.
        stamp: InputStamp,
    },
    /// A focus report.
    Focus(bool),
    /// A geometry report.
    Geometry(GridSize, CellSize),
    /// A presentation the viewer would apply.
    Presentation(Box<TerminalPresentation>),
    /// Erase history.
    ClearHistory,
    /// Reset the emulator.
    Reset,
    /// Set the history limit.
    Scrollback(ScrollbackLimit),
    /// Set idle history compression.
    Compression(bool),
}

/// A snapshot and the link lookup that came with it.
#[derive(Clone, Debug)]
pub struct SnapshotOutcome {
    /// The snapshot.
    pub snapshot: Arc<TerminalSnapshot>,
    /// The link at the requested cell, when one was asked for.
    pub link: Option<LinkLookup>,
}

/// One host effect core admitted for a viewer's recipient. Holding it
/// keeps its reservations; dropping it releases them.
pub trait PendingEffect: Send + fmt::Debug {
    /// Whether the effect still belongs to its recipient's live generation
    /// for its kind, so it may still be sent.
    fn is_current(&self) -> bool;

    /// The permission generation of its kind when core admitted it.
    fn generation(&self) -> u64;

    /// The terminal whose content asked for it.
    fn terminal(&self) -> TerminalId;

    /// What it asks the client to do, borrowed so a large payload streams
    /// from its own buffer, or `None` for a kind with no wire form, which
    /// the connection drops.
    fn payload(&self) -> Option<EffectPayloadRef<'_>>;
}

/// One viewer, as a connection drives it.
pub trait ViewerPort: Send + Sync + fmt::Debug {
    /// The viewer's identity.
    fn id(&self) -> ViewerId;

    /// What changed since the previous poll.
    fn poll(&self) -> ViewerUpdate;

    /// The viewer's wake signal.
    fn wake(&self) -> Box<dyn WakeSource>;

    /// Admits an ordered message without waiting.
    ///
    /// # Errors
    /// Returns the message with the reason core refused it; `Busy` asks
    /// for a retry in order.
    fn offer(&self, message: Ordered) -> Result<(), (RuntimeError, Ordered)>;

    /// Requests a snapshot, after a scroll the client numbered and with a
    /// link lookup at a cell when asked.
    fn snapshot(
        &self,
        scroll: Option<(ScrollCommand, u64)>,
        point: Option<MousePosition>,
    ) -> Reply<SnapshotOutcome>;

    /// Requests text from canonical scrollback.
    fn selection(
        &self,
        generation: u64,
        range: BufferRange,
    ) -> Reply<Option<String>>;

    /// Requests a multi-click selection's whole units.
    fn select_units(
        &self,
        generation: u64,
        anchor: BufferPoint,
        head: BufferPoint,
        unit: SelectionUnit,
    ) -> Reply<Option<BufferRange>>;

    /// Takes the next current effect queued for the viewer's recipient,
    /// without waiting, dropping stale ones on the way. A viewer without a
    /// recipient has none.
    fn next_effect(&self) -> Option<Box<dyn PendingEffect>>;

    /// Applies the client's numbered change to one kind's permission.
    /// Returns false, changing nothing, for a generation not above the
    /// current one or a viewer without a recipient.
    fn set_effect_permission(
        &self,
        kind: HostEffectKind,
        allowed: bool,
        generation: u64,
    ) -> bool;

    /// Withdraws the viewer's recipient for good, as its connection ends,
    /// even while a job still holds the viewer.
    fn revoke_effects(&self);
}

/// A core effect, which a connection sends as it is.
#[derive(Debug)]
struct CoreEffect(PendingHostEffect);

impl PendingEffect for CoreEffect {
    fn is_current(&self) -> bool {
        self.0.is_current()
    }

    fn generation(&self) -> u64 {
        self.0.metadata().recipient_generation
    }

    fn terminal(&self) -> TerminalId {
        self.0.metadata().origin.terminal_id
    }

    fn payload(&self) -> Option<EffectPayloadRef<'_>> {
        match self.0.effect() {
            HostEffect::ClipboardWrite(write) => {
                Some(EffectPayloadRef::ClipboardWrite {
                    destination: write.destination,
                    text: write.text(),
                })
            }
            HostEffect::Notification(notification) => {
                Some(EffectPayloadRef::Notification {
                    title: notification.title(),
                    body: notification.body(),
                })
            }
            _ => None,
        }
    }
}

/// A viewer operation that answers later.
#[derive(Debug)]
pub enum ViewerOp {
    /// A snapshot, with an optional numbered scroll and link lookup.
    Snapshot {
        /// The scroll and its number.
        scroll: Option<(ScrollCommand, u64)>,
        /// The cell to look a link up at.
        point: Option<MousePosition>,
    },
    /// Selection text.
    Selection {
        /// The content generation.
        generation: u64,
        /// The range.
        range: BufferRange,
    },
    /// Selection units.
    SelectUnits {
        /// The content generation.
        generation: u64,
        /// The anchor.
        anchor: BufferPoint,
        /// The head.
        head: BufferPoint,
        /// The unit.
        unit: SelectionUnit,
    },
}

/// A viewer operation and the port it runs on.
pub struct ViewerJob {
    /// The port.
    pub port: Arc<dyn ViewerPort>,
    /// The operation.
    pub op: ViewerOp,
}

impl fmt::Debug for ViewerJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ViewerJob")
            .field("viewer", &self.port.id())
            .field("op", &self.op)
            .finish()
    }
}

impl ViewerJob {
    /// Sends the operation to its terminal at once and returns the future
    /// of its completion, which holds no reference to the viewer.
    pub fn start(self) -> Pin<Box<dyn Future<Output = Completion> + Send>> {
        let Self { port, op } = self;
        match op {
            ViewerOp::Snapshot { scroll, point } => {
                let reply = port.snapshot(scroll, point);
                Box::pin(async move { Completion::Snapshot(reply.await) })
            }
            ViewerOp::Selection { generation, range } => {
                let reply = port.selection(generation, range);
                Box::pin(async move { Completion::Selection(reply.await) })
            }
            ViewerOp::SelectUnits {
                generation,
                anchor,
                head,
                unit,
            } => {
                let reply = port.select_units(generation, anchor, head, unit);
                Box::pin(async move { Completion::SelectUnits(reply.await) })
            }
        }
    }
}

struct CoreWake(huterm_core::ViewerWake);

impl WakeSource for CoreWake {
    fn wait(&self) -> Wake {
        let wake = self.0.clone();
        Box::pin(async move { wake.wait().await.is_ok() })
    }
}

impl ViewerPort for TerminalViewer {
    fn id(&self) -> ViewerId {
        TerminalViewer::id(self)
    }

    fn poll(&self) -> ViewerUpdate {
        TerminalViewer::poll(self)
    }

    fn wake(&self) -> Box<dyn WakeSource> {
        Box::new(CoreWake(self.wake_handle()))
    }

    fn offer(&self, message: Ordered) -> Result<(), (RuntimeError, Ordered)> {
        let refused = |result: Result<(), RuntimeError>, message| {
            result.map_err(|error| (error, message))
        };
        match message {
            Ordered::Input { input, stamp } => {
                self.offer_input(input, stamp).map_err(|refused| {
                    (
                        refused.error,
                        Ordered::Input {
                            input: refused.input,
                            stamp,
                        },
                    )
                })
            }
            Ordered::Focus(focused) => {
                refused(self.set_focus(focused), Ordered::Focus(focused))
            }
            Ordered::Geometry(grid, cell) => refused(
                self.report_geometry(grid, cell),
                Ordered::Geometry(grid, cell),
            ),
            Ordered::Presentation(presentation) => self
                .update_presentation((*presentation).clone())
                .map_err(|error| (error, Ordered::Presentation(presentation))),
            Ordered::ClearHistory => {
                refused(self.clear_history(), Ordered::ClearHistory)
            }
            Ordered::Reset => refused(self.reset(), Ordered::Reset),
            Ordered::Scrollback(limit) => {
                refused(self.set_scrollback(limit), Ordered::Scrollback(limit))
            }
            Ordered::Compression(enabled) => refused(
                self.set_scrollback_compression(enabled),
                Ordered::Compression(enabled),
            ),
        }
    }

    fn snapshot(
        &self,
        scroll: Option<(ScrollCommand, u64)>,
        point: Option<MousePosition>,
    ) -> Reply<SnapshotOutcome> {
        match self.request_snapshot_numbered(scroll, point) {
            Ok(request) => Box::pin(async move {
                let reply = request.recv().await?;
                Ok(SnapshotOutcome {
                    snapshot: reply.snapshot,
                    link: reply.link,
                })
            }),
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }

    fn selection(
        &self,
        generation: u64,
        range: BufferRange,
    ) -> Reply<Option<String>> {
        match self.request_selection(generation, range) {
            Ok(request) => Box::pin(async move { request.recv().await }),
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }

    fn select_units(
        &self,
        generation: u64,
        anchor: BufferPoint,
        head: BufferPoint,
        unit: SelectionUnit,
    ) -> Reply<Option<BufferRange>> {
        match self.request_select_units(generation, anchor, head, unit) {
            Ok(request) => Box::pin(async move { request.recv().await }),
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }

    fn next_effect(&self) -> Option<Box<dyn PendingEffect>> {
        let recipient = self.host_effects()?;
        let pending = recipient.try_next()?;
        Some(Box::new(CoreEffect(pending)))
    }

    fn set_effect_permission(
        &self,
        kind: HostEffectKind,
        allowed: bool,
        generation: u64,
    ) -> bool {
        self.host_effects().is_some_and(|recipient| {
            recipient.set_permission(kind, allowed, generation)
        })
    }

    fn revoke_effects(&self) {
        if let Some(recipient) = self.host_effects() {
            recipient.revoke();
        }
    }
}
