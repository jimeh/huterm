//! Test-only control over the desktop's workers: counts close assessments,
//! close commits, viewer subscriptions, and layout commits, holds any
//! stage until the test releases it, and injects close failures. Tier 2
//! tests use it to place window events between a worker's start and its
//! completion; production builds compile none of it.

use std::sync::Mutex;

/// A worker stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Stage {
    Assess,
    /// A close assessment that returned, before its result reaches the
    /// window.
    Assessed,
    Commit,
    /// Subscribing viewers for installs and replacements.
    Subscribe,
    /// A new tab spawned, before its view can resize the terminal.
    Spawned,
    /// A layout commit queue job, a resize or an arrangement, before its
    /// runtime call.
    Layout,
    /// A layout commit or cross-window transfer whose runtime call returned,
    /// before its completion waits for the projection.
    LayoutCommitted,
    /// Cross-window transfer before acquiring the Mux lock.
    Transfer,
    /// A restore's worker, before it spawns any terminal.
    Restore,
    /// A restore's worker after its first terminal started, before it
    /// checks for termination and spawns the next.
    RestoreSpawned,
    /// A restore's worker after it inserted the structure, before the
    /// windows open.
    Restored,
    /// A checkpoint's runtime capture, before it takes the Mux lock.
    Checkpoint,
    /// A session switch's worker, before it takes the Mux lock. A failure
    /// fails the shell the switch would spawn, if it spawns one.
    Switch,
    /// A session switch whose commit returned, before its completion waits
    /// for the projection.
    Switched,
    /// Opening a session in a new window, before the worker takes the Mux
    /// lock. A failure fails the window's open.
    OpenSession,
    /// A committed Close Session of the window's own session, before the
    /// worker attaches the window to the session it moves to. A failure
    /// fails that attach.
    CloseFollow,
    /// The session menu's read of exited sessions, before it takes the Mux
    /// lock.
    ExitedRead,
}

#[derive(Default)]
pub(super) struct WorkerHooks(Mutex<[StageHooks; 17]>);

#[derive(Default)]
struct StageHooks {
    /// How many workers entered this stage.
    entered: usize,
    /// How many workers wait at the gate now.
    held: usize,
    /// Workers wait until the sender drops.
    gate: Option<(async_channel::Sender<()>, async_channel::Receiver<()>)>,
    /// The next worker past the gate fails.
    fail: bool,
}

impl WorkerHooks {
    fn with<T>(&self, stage: Stage, f: impl FnOnce(&mut StageHooks) -> T) -> T {
        let mut stages = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut stages[stage as usize])
    }

    /// Makes later workers of `stage` wait until [`Self::release`].
    pub(super) fn hold(&self, stage: Stage) {
        self.with(stage, |hooks| {
            hooks.gate.get_or_insert_with(|| async_channel::bounded(1));
        });
    }

    /// Lets every held worker of `stage` continue, and later ones pass.
    pub(super) fn release(&self, stage: Stage) {
        self.with(stage, |hooks| hooks.gate = None);
    }

    /// Makes the next worker of `stage` to pass the gate fail.
    pub(super) fn fail(&self, stage: Stage) {
        self.with(stage, |hooks| hooks.fail = true);
    }

    pub(super) fn entered(&self, stage: Stage) -> usize {
        self.with(stage, |hooks| hooks.entered)
    }

    pub(super) fn held(&self, stage: Stage) -> usize {
        self.with(stage, |hooks| hooks.held)
    }

    /// Runs at the start of a worker of `stage`: counts it, waits while the
    /// stage is held, and returns whether the worker must fail.
    pub(super) async fn enter(&self, stage: Stage) -> bool {
        if let Some(gate) = self.arrive(stage) {
            // Resolves with an error once `release` drops the sender.
            let _ = gate.recv().await;
            self.with(stage, |hooks| hooks.held -= 1);
        }
        self.with(stage, |hooks| std::mem::take(&mut hooks.fail))
    }

    /// [`Self::enter`] for a worker on its own thread, which blocks it.
    pub(super) fn enter_blocking(&self, stage: Stage) -> bool {
        if let Some(gate) = self.arrive(stage) {
            let _ = gate.recv_blocking();
            self.with(stage, |hooks| hooks.held -= 1);
        }
        self.with(stage, |hooks| std::mem::take(&mut hooks.fail))
    }

    /// Counts a worker entering `stage`; returns the gate it must wait on.
    fn arrive(&self, stage: Stage) -> Option<async_channel::Receiver<()>> {
        self.with(stage, |hooks| {
            hooks.entered += 1;
            let gate = hooks.gate.as_ref().map(|(_, gate)| gate.clone());
            hooks.held += usize::from(gate.is_some());
            gate
        })
    }
}
