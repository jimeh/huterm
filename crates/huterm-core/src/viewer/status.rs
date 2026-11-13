//! The owner thread's copy of the terminal status, published to viewers in
//! batches and only when a value actually changed.

use std::sync::Arc;
use std::time::{Duration, Instant};

use huterm_protocol::{
    CommandRecord, ExitStatus, TERMINAL_FAILURE_CAPACITY, TerminalFailure,
    TerminalLifecycle, TerminalMetadata, TerminalProgress, TerminalStatus,
    bound_status_text,
};

use super::Registry;

/// How long progress stays without a new report, as Ghostty clears it, so a
/// program that crashed or forgot to remove it leaves no stale indicator.
pub(crate) const PROGRESS_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Default)]
pub(crate) struct StatusPublisher {
    status: TerminalStatus,
    dirty: bool,
    /// When the shown progress expires; `None` while there is none.
    progress_expires: Option<Instant>,
    /// Replaces [`PROGRESS_TIMEOUT`] in tests.
    #[cfg(test)]
    pub(crate) progress_timeout: Option<Duration>,
}

impl StatusPublisher {
    pub(crate) fn title(&mut self, title: String) {
        let title = bound_status_text(title);
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
        let metadata = metadata.clone().bounded();
        if self.status.metadata != metadata {
            self.status.metadata = metadata;
            self.status.metadata_revision = revision;
            self.dirty = true;
        }
    }

    /// Replaces the command records with a changed list.
    pub(crate) fn commands(&mut self, commands: Arc<[CommandRecord]>) {
        if self.status.commands != commands {
            self.status.commands = commands;
            self.dirty = true;
        }
    }

    pub(crate) fn bell(&mut self) {
        self.status.bells = self.status.bells.saturating_add(1);
        self.dirty = true;
    }

    /// Records a progress report at `now`. Every report, even an unchanged
    /// one, extends its expiry. Once the root has exited, output parsed
    /// after the exit can still report progress; nothing would end it, so
    /// only a removal applies.
    pub(crate) fn progress(
        &mut self,
        progress: Option<TerminalProgress>,
        now: Instant,
    ) {
        if progress.is_some()
            && self.status.lifecycle != TerminalLifecycle::Running
        {
            return;
        }
        self.progress_expires = progress.map(|_| now + self.progress_timeout());
        if self.status.progress != progress {
            self.status.progress = progress;
            self.dirty = true;
        }
    }

    #[cfg_attr(
        not(test),
        expect(clippy::unused_self, reason = "only tests replace the timeout")
    )]
    fn progress_timeout(&self) -> Duration {
        #[cfg(test)]
        if let Some(timeout) = self.progress_timeout {
            return timeout;
        }
        PROGRESS_TIMEOUT
    }

    /// When progress expires, for the runtime's waits.
    pub(crate) fn progress_deadline(&self) -> Option<Instant> {
        self.progress_expires
    }

    /// Clears progress whose last report is [`PROGRESS_TIMEOUT`] old.
    /// Returns whether it did.
    pub(crate) fn expire_progress(&mut self, now: Instant) -> bool {
        if self.progress_expires.is_some_and(|expires| now >= expires) {
            self.progress(None, now);
            return true;
        }
        false
    }

    /// Records the root's exit, which also ends any progress it reported.
    pub(crate) fn exit(&mut self, status: ExitStatus) {
        if self.status.lifecycle == TerminalLifecycle::Running {
            self.status.lifecycle = TerminalLifecycle::Exited(status);
            self.dirty = true;
        }
        self.progress(None, Instant::now());
    }

    /// Records a failure. A message identical to the latest one is not
    /// repeated, so a failure that recurs on every turn cannot flood the log.
    pub(crate) fn failure(&mut self, message: String) {
        let message = bound_status_text(message);
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

    #[test]
    fn published_text_is_cut_to_the_status_text_limit() {
        use huterm_protocol::{
            MAX_STATUS_TEXT_BYTES, TerminalDirectory, TerminalMetadata,
        };

        let long = format!("{}\u{e9}", "x".repeat(MAX_STATUS_TEXT_BYTES - 1));
        let cut = "x".repeat(MAX_STATUS_TEXT_BYTES - 1);
        let mut status = StatusPublisher::default();
        status.title(long.clone());
        status.failure(long.clone());
        let metadata = TerminalMetadata::new(
            Some(TerminalDirectory::new(None, long.clone(), true)),
            Some(long.clone()),
        );
        status.metadata(1, &metadata);
        assert_eq!(status.status.title, cut);
        assert_eq!(status.status.failures[0].message, cut);
        assert_eq!(status.status.metadata.directory().unwrap().path(), cut);
        assert_eq!(status.status.metadata.foreground_process(), Some(&*cut));
        // The same over-long text again is no change, so nothing republishes.
        assert!(std::mem::take(&mut status.dirty));
        status.title(long.clone());
        status.failure(long);
        status.metadata(2, &metadata);
        assert!(!status.dirty);
    }

    #[test]
    fn progress_expires_after_its_timeout_and_with_the_root() {
        use std::time::{Duration, Instant};

        use huterm_protocol::{ExitStatus, TerminalProgress};

        use super::PROGRESS_TIMEOUT;

        let start = Instant::now();
        let mut status = StatusPublisher::default();
        assert_eq!(status.progress_deadline(), None);
        status.progress(Some(TerminalProgress::Determinate(10)), start);
        assert!(std::mem::take(&mut status.dirty));
        // An unchanged report publishes nothing but extends the expiry.
        let later = start + Duration::from_secs(10);
        status.progress(Some(TerminalProgress::Determinate(10)), later);
        assert!(!status.dirty);
        assert_eq!(status.progress_deadline(), Some(later + PROGRESS_TIMEOUT));
        assert!(!status.expire_progress(start + PROGRESS_TIMEOUT));
        assert!(status.expire_progress(later + PROGRESS_TIMEOUT));
        assert_eq!(status.status.progress, None);
        assert_eq!(status.progress_deadline(), None);
        assert!(std::mem::take(&mut status.dirty));

        status.progress(Some(TerminalProgress::Indeterminate), later);
        status.exit(ExitStatus {
            code: Some(0),
            success: true,
        });
        assert_eq!(status.status.progress, None, "exit clears progress");
        assert_eq!(status.progress_deadline(), None);
        // Output parsed after the exit cannot bring it back.
        status.progress(Some(TerminalProgress::Determinate(5)), later);
        assert_eq!(status.status.progress, None);
        assert_eq!(status.progress_deadline(), None);
    }
}
