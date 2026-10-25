//! Test-only control over the desktop's workers: counts close assessments,
//! close commits, and viewer subscriptions, holds any stage until the test
//! releases it, and injects close failures. Tier 2 tests use it to place
//! window events between a worker's start and its completion; production
//! builds compile none of it.

use std::sync::Mutex;

/// A worker stage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Stage {
    Assess,
    Commit,
    /// Subscribing viewers for installs and replacements.
    Subscribe,
}

#[derive(Default)]
pub(super) struct WorkerHooks(Mutex<[StageHooks; 3]>);

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
        let gate = self.with(stage, |hooks| {
            hooks.entered += 1;
            let gate = hooks.gate.as_ref().map(|(_, gate)| gate.clone());
            hooks.held += usize::from(gate.is_some());
            gate
        });
        if let Some(gate) = gate {
            // Resolves with an error once `release` drops the sender.
            let _ = gate.recv().await;
            self.with(stage, |hooks| hooks.held -= 1);
        }
        self.with(stage, |hooks| std::mem::take(&mut hooks.fail))
    }
}
