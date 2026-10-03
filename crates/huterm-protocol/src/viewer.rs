//! Viewer capabilities, input stamps, and revisioned terminal status.

use crate::{ExitStatus, TerminalMetadata};

/// Failure messages a terminal status retains, newest last.
pub const TERMINAL_FAILURE_CAPACITY: usize = 8;

/// What one viewer may do to its terminal. The runtime checks each flag on
/// every request, so a client that lacks a flag cannot act around it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "viewer capabilities are independent permissions"
)]
pub struct ViewerCapabilities {
    /// Keys, characters, text, paste, mouse reports, clear scrollback, and
    /// reset. A focused viewer with this flag counts toward application
    /// focus reports.
    pub input: bool,
    /// Scroll commands on the shared viewport. With `input`, typing also
    /// returns the shared viewport to live output.
    pub viewport: bool,
    /// Geometry reports. Focus, typing, and geometry from a viewer with this
    /// flag can make it the controlling viewer.
    pub size: bool,
    /// Registration as a recipient of terminal host effects.
    pub host_effects: bool,
}

impl ViewerCapabilities {
    /// Every capability, as a writable desktop view holds.
    pub const ALL: Self = Self {
        input: true,
        viewport: true,
        size: true,
        host_effects: true,
    };

    /// No capability: the viewer can only observe the terminal.
    pub const NONE: Self = Self {
        input: false,
        viewport: false,
        size: false,
        host_effects: false,
    };
}

/// Facts a client records when the user acts, attached to each input so the
/// runtime can judge the input against the state the client acted on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InputStamp {
    /// How many scroll commands this viewer had submitted. Typing returns
    /// the shared viewport to live only if none of this viewer's later
    /// scrolls has been applied.
    pub scrolls: u64,
    /// The geometry revision of the snapshot that mouse coordinates were
    /// computed against, if they were. A press, unowned motion, or wheel
    /// report whose revision no longer matches the canonical geometry is
    /// discarded.
    pub geometry: Option<u64>,
}

/// Whether a terminal's root process is still running.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalLifecycle {
    /// The root process is running.
    Running,
    /// The root process exited. The terminal keeps its history.
    Exited(ExitStatus),
}

/// One runtime failure, numbered so a viewer can detect failures it missed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalFailure {
    /// Position among all failures this terminal reported, starting at 1.
    pub sequence: u64,
    /// Human-readable failure.
    pub message: String,
}

/// The current non-cell state of a terminal. Viewers read the latest value
/// when woken instead of receiving a queue of events, so a viewer that wakes
/// late, or starts late, sees the same state as one that saw every change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalStatus {
    /// Advances by one whenever any other field changes, and only then.
    pub revision: u64,
    /// The latest title the terminal reported.
    pub title: String,
    /// Complete current metadata.
    pub metadata: TerminalMetadata,
    /// Monotonic terminal-local metadata revision.
    pub metadata_revision: u64,
    /// Whether the root process is running.
    pub lifecycle: TerminalLifecycle,
    /// Bells rung since the terminal started.
    pub bells: u64,
    /// The most recent failures, oldest first, at most
    /// [`TERMINAL_FAILURE_CAPACITY`].
    pub failures: Vec<TerminalFailure>,
}

impl Default for TerminalStatus {
    fn default() -> Self {
        Self {
            revision: 0,
            title: String::new(),
            metadata: TerminalMetadata::default(),
            metadata_revision: 0,
            lifecycle: TerminalLifecycle::Running,
            bells: 0,
            failures: Vec::new(),
        }
    }
}

impl TerminalStatus {
    /// The sequence number of the latest failure, or zero when none was
    /// reported.
    #[must_use]
    pub fn last_failure(&self) -> u64 {
        self.failures.last().map_or(0, |failure| failure.sequence)
    }

    /// Failures reported after `seen`, and how many of those the bounded log
    /// no longer holds.
    #[must_use]
    pub fn failures_since(&self, seen: u64) -> (&[TerminalFailure], u64) {
        let start = self
            .failures
            .iter()
            .position(|failure| failure.sequence > seen)
            .unwrap_or(self.failures.len());
        let missed = self.failures.get(start).map_or(0, |first| {
            first.sequence.saturating_sub(seen.saturating_add(1))
        });
        (&self.failures[start..], missed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(sequences: &[u64]) -> TerminalStatus {
        TerminalStatus {
            failures: sequences
                .iter()
                .map(|&sequence| TerminalFailure {
                    sequence,
                    message: sequence.to_string(),
                })
                .collect(),
            ..TerminalStatus::default()
        }
    }

    #[test]
    fn failures_since_reports_new_failures_and_the_ones_the_log_dropped() {
        let status = status(&[5, 6, 7]);
        let (new, missed) = status.failures_since(2);
        assert_eq!(
            new.iter().map(|f| f.sequence).collect::<Vec<_>>(),
            [5, 6, 7]
        );
        assert_eq!(missed, 2, "failures 3 and 4 were dropped");

        let (new, missed) = status.failures_since(6);
        assert_eq!(new.iter().map(|f| f.sequence).collect::<Vec<_>>(), [7]);
        assert_eq!(missed, 0);

        let (new, missed) = status.failures_since(7);
        assert!(new.is_empty());
        assert_eq!(missed, 0);
        assert_eq!(status.last_failure(), 7);
        assert_eq!(TerminalStatus::default().last_failure(), 0);
    }
}
