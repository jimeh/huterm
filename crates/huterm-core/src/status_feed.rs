//! A status feed that needs no viewer: one coalescing wake per subscriber
//! whenever any terminal Mux owns publishes a status change a summary of its
//! workspace would show, such as a client's per-workspace status dot.
//!
//! Each terminal's registry signals the feed from
//! [`crate::viewer::StatusPublisher::flush`]'s publication, so a subscriber
//! learns about terminals nothing views. A subscriber reads the statuses
//! themselves through [`crate::Mux::terminal_statuses`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use huterm_protocol::{TerminalId, TerminalLifecycle, TerminalStatus};

/// The feed every terminal of one Mux signals.
#[derive(Debug, Default)]
pub(crate) struct StatusFeed {
    /// Advances once per signalled change, before subscribers wake.
    revision: AtomicU64,
    subscribers: Mutex<Vec<async_channel::Sender<()>>>,
}

impl StatusFeed {
    /// Records a change and wakes every subscriber once; a subscriber whose
    /// wake is still pending stays woken once.
    pub(crate) fn signal(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|wake| {
                !matches!(
                    wake.try_send(()),
                    Err(async_channel::TrySendError::Closed(()))
                )
            });
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub(crate) fn subscribe(&self) -> StatusSubscription {
        let (wake, signal) = async_channel::bounded(1);
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(wake);
        StatusSubscription { wake: signal }
    }
}

/// Whether a status change can change what a workspace summary shows: the
/// root's exit, a new failure, a bell, progress starting or ending, or the
/// foreground program. Titles, directories, and command records do not
/// wake the feed.
pub(crate) fn summary_changed(
    before: &TerminalStatus,
    after: &TerminalStatus,
) -> bool {
    let key = |status: &TerminalStatus| {
        (
            status.lifecycle == TerminalLifecycle::Running,
            status.last_failure(),
            status.bells,
            status.progress.is_some(),
        )
    };
    key(before) != key(after)
        || before.metadata.foreground_process()
            != after.metadata.foreground_process()
}

/// Wakes once after one or more summarized status changes, however many
/// arrived meanwhile.
#[derive(Debug)]
pub struct StatusSubscription {
    wake: async_channel::Receiver<()>,
}

impl StatusSubscription {
    /// Waits for the next change. Returns false only once the feed is
    /// gone: the Mux and every terminal it owned dropped.
    pub async fn changed(&self) -> bool {
        self.wake.recv().await.is_ok()
    }

    /// Consumes a pending change without waiting.
    #[must_use]
    pub fn try_changed(&self) -> bool {
        self.wake.try_recv().is_ok()
    }
}

/// Every live terminal's published status, read without waiting on any
/// terminal thread.
#[derive(Clone, Debug, Default)]
pub struct TerminalStatuses {
    /// The feed's revision when the read started. Every change signalled at
    /// or before it is in the read; a client drops a read older than one it
    /// already applied.
    pub revision: u64,
    /// Each terminal's status.
    pub statuses: Vec<(TerminalId, Arc<TerminalStatus>)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn many_signals_wake_a_subscriber_once_and_advance_the_revision() {
        let feed = StatusFeed::default();
        let first = feed.subscribe();
        let second = feed.subscribe();
        for _ in 0..5 {
            feed.signal();
        }
        assert_eq!(feed.revision(), 5);
        for subscription in [&first, &second] {
            assert!(subscription.try_changed());
            assert!(!subscription.try_changed(), "coalesced into one wake");
        }
        drop(second);
        feed.signal();
        assert_eq!(
            feed.subscribers
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len(),
            1,
            "a dropped subscription is pruned"
        );
        assert!(first.try_changed());
    }

    #[test]
    fn only_a_summarized_change_counts() {
        let before = TerminalStatus::default();
        let titled = TerminalStatus {
            revision: 2,
            title: "vim".into(),
            ..TerminalStatus::default()
        };
        assert!(!summary_changed(&before, &titled));
        let rung = TerminalStatus {
            bells: 1,
            ..TerminalStatus::default()
        };
        assert!(summary_changed(&before, &rung));
    }
}
