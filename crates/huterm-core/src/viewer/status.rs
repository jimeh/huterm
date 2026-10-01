//! The owner thread's copy of the terminal status, published to viewers in
//! batches and only when a value actually changed.

use std::sync::Arc;

use huterm_protocol::{
    ExitStatus, TERMINAL_FAILURE_CAPACITY, TerminalFailure, TerminalLifecycle,
    TerminalMetadata, TerminalStatus,
};

use super::Registry;

#[derive(Debug, Default)]
pub(crate) struct StatusPublisher {
    status: TerminalStatus,
    dirty: bool,
}

impl StatusPublisher {
    pub(crate) fn title(&mut self, title: String) {
        if self.status.title != title {
            self.status.title = title;
            self.dirty = true;
        }
    }

    pub(crate) fn metadata(
        &mut self,
        revision: u64,
        metadata: &TerminalMetadata,
    ) {
        if self.status.metadata != *metadata {
            self.status.metadata = metadata.clone();
            self.status.metadata_revision = revision;
            self.dirty = true;
        }
    }

    pub(crate) fn bell(&mut self) {
        self.status.bells = self.status.bells.saturating_add(1);
        self.dirty = true;
    }

    pub(crate) fn exit(&mut self, status: ExitStatus) {
        if self.status.lifecycle == TerminalLifecycle::Running {
            self.status.lifecycle = TerminalLifecycle::Exited(status);
            self.dirty = true;
        }
    }

    /// Records a failure. A message identical to the latest one is not
    /// repeated, so a failure that recurs on every turn cannot flood the log.
    pub(crate) fn failure(&mut self, message: String) {
        if self
            .status
            .failures
            .last()
            .is_some_and(|last| last.message == message)
        {
            return;
        }
        let sequence = self.status.last_failure().saturating_add(1);
        if self.status.failures.len() == TERMINAL_FAILURE_CAPACITY {
            self.status.failures.remove(0);
        }
        self.status
            .failures
            .push(TerminalFailure { sequence, message });
        self.dirty = true;
    }

    /// Publishes the changes since the previous flush as one revision.
    pub(crate) fn flush(&mut self, registry: &Registry) {
        if !std::mem::take(&mut self.dirty) {
            return;
        }
        self.status.revision = self.status.revision.saturating_add(1);
        registry.publish_status(Arc::new(self.status.clone()));
    }
}
