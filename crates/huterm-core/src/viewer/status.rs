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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use huterm_protocol::{
        RuntimeId, TERMINAL_FAILURE_CAPACITY, TerminalId, ViewerCapabilities,
    };

    use super::StatusPublisher;
    use crate::viewer::{Initial, Registry};

    #[test]
    fn only_changes_publish_a_revision_and_wake_viewers() {
        let registry = Registry::new(
            TerminalId::new(1),
            RuntimeId::new(0),
            Arc::new(crate::wake::Wake::default()),
        );
        let (_slot, wake) = registry
            .register_with_wake(
                None,
                ViewerCapabilities::ALL,
                Initial {
                    focused: false,
                    geometry: None,
                    presentation: None,
                },
            )
            .unwrap();
        while wake.try_recv().is_ok() {}
        let mut status = StatusPublisher::default();
        status.title("title".into());
        status.bell();
        status.bell();
        status.flush(&registry);
        assert!(wake.try_recv().is_ok());
        let published = registry.status();
        assert_eq!((published.revision, published.bells), (1, 2));
        // A program that re-sends its title on every chunk costs nothing.
        for _ in 0..100 {
            status.title("title".into());
            status.flush(&registry);
        }
        assert!(wake.try_recv().is_err(), "an unchanged title wakes no one");
        assert_eq!(registry.status().revision, 1);
    }

    #[test]
    fn failures_keep_a_bounded_numbered_log_without_repeats() {
        let mut status = StatusPublisher::default();
        status.failure("same".into());
        status.failure("same".into());
        assert_eq!(status.status.failures.len(), 1, "a repeat is dropped");
        for index in 0..TERMINAL_FAILURE_CAPACITY {
            status.failure(format!("failure {index}"));
        }
        let failures = &status.status.failures;
        assert_eq!(failures.len(), TERMINAL_FAILURE_CAPACITY);
        assert_eq!(failures[0].sequence, 2, "the oldest was dropped");
        assert_eq!(
            status.status.last_failure(),
            u64::try_from(TERMINAL_FAILURE_CAPACITY).unwrap() + 1
        );
    }
}
