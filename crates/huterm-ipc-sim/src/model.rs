//! The domain both ends share: a sequenced hierarchy of tabs, terminal
//! snapshots with shared rows, and revisioned terminal status.
//!
//! Minor version 1 is test-only. It adds a tab's `pinned` flag, three event
//! variants with one downgrade path each, and a curly-underline style bit.

use std::rc::Rc;

/// Minor version 0, the first release.
pub const MINOR_0: u16 = 0;
/// The test-only minor version 1.
pub const MINOR_1: u16 = 1;

/// Bold.
pub const STYLE_BOLD: u8 = 1;
/// Plain underline.
pub const STYLE_UNDERLINE: u8 = 2;
/// Curly underline, minor version 1 only. Projects to plain underline.
pub const STYLE_CURLY: u8 = 4;

/// One tab: identity, name, and, from minor version 1, its pinned flag.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabInfo {
    /// Stable identity within an incarnation.
    pub id: u64,
    /// Display name.
    pub name: String,
    /// Minor version 1 state; always false at minor version 0.
    pub pinned: bool,
}

/// A structural event, with the sequence the stream assigns it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    /// A tab opened at `index`.
    Opened { tab: TabInfo, index: usize },
    /// A tab was renamed.
    Renamed { tab: u64, name: String },
    /// A tab moved to `index`.
    Moved { tab: u64, index: usize },
    /// A tab closed.
    Closed { tab: u64 },
    /// Advances the sequence and changes nothing (reserved in minor 0).
    Skipped,
    /// Minor 1: a tab was pinned or unpinned. No minor-0 state changes.
    Pinned { tab: u64, pinned: bool },
    /// Minor 1: a rename with a new kind. Downgrades to one `Renamed`.
    Retitled { tab: u64, name: String },
    /// Minor 1: every tab's order reversed. No single minor-0 event
    /// matches, so older peers receive a projected resync.
    Reversed,
}

/// How a newer event reaches a peer on an older minor version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Downgrade {
    /// Send this event.
    Send(Event),
    /// Send a projected snapshot at this sequence instead.
    Resync,
}

impl Event {
    /// The minor version that introduced this variant.
    #[must_use]
    pub fn minor(&self) -> u16 {
        match self {
            Self::Pinned { .. } | Self::Retitled { .. } | Self::Reversed => {
                MINOR_1
            }
            _ => MINOR_0,
        }
    }

    /// This event as a peer on `minor` receives it.
    #[must_use]
    pub fn downgrade(&self, minor: u16) -> Downgrade {
        if self.minor() <= minor {
            return Downgrade::Send(self.clone());
        }
        match self {
            Self::Pinned { .. } => Downgrade::Send(Self::Skipped),
            Self::Retitled { tab, name } => Downgrade::Send(Self::Renamed {
                tab: *tab,
                name: name.clone(),
            }),
            _ => Downgrade::Resync,
        }
    }
}

/// What applying an event did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Applied {
    /// The event applied.
    Applied,
    /// The event was at or below the applied sequence.
    Stale,
    /// A sequence was missing; resynchronize.
    Gap,
    /// The event did not fit the state; resynchronize.
    Inconsistent,
}

/// A sequenced, ordered set of tabs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Hierarchy {
    /// The last applied sequence.
    pub seq: u64,
    /// Tabs in order.
    pub tabs: Vec<TabInfo>,
}

impl Hierarchy {
    /// Applies the event at `seq`.
    pub fn apply(&mut self, seq: u64, event: &Event) -> Applied {
        if seq <= self.seq {
            return Applied::Stale;
        }
        if seq != self.seq + 1 {
            return Applied::Gap;
        }
        if !self.mutate(event) {
            return Applied::Inconsistent;
        }
        self.seq = seq;
        Applied::Applied
    }

    fn position(&self, tab: u64) -> Option<usize> {
        self.tabs.iter().position(|info| info.id == tab)
    }

    /// Applies `event` to the tabs without touching the sequence.
    pub fn mutate(&mut self, event: &Event) -> bool {
        match event {
            Event::Opened { tab, index } => {
                if self.position(tab.id).is_some() || *index > self.tabs.len() {
                    return false;
                }
                self.tabs.insert(*index, tab.clone());
            }
            Event::Renamed { tab, name } | Event::Retitled { tab, name } => {
                let Some(at) = self.position(*tab) else {
                    return false;
                };
                self.tabs[at].name.clone_from(name);
            }
            Event::Moved { tab, index } => {
                let Some(at) = self.position(*tab) else {
                    return false;
                };
                if *index >= self.tabs.len() {
                    return false;
                }
                let info = self.tabs.remove(at);
                self.tabs.insert(*index, info);
            }
            Event::Closed { tab } => {
                let Some(at) = self.position(*tab) else {
                    return false;
                };
                self.tabs.remove(at);
            }
            Event::Pinned { tab, pinned } => {
                let Some(at) = self.position(*tab) else {
                    return false;
                };
                self.tabs[at].pinned = *pinned;
            }
            Event::Reversed => self.tabs.reverse(),
            Event::Skipped => {}
        }
        true
    }

    /// This state as a peer on `minor` holds it.
    #[must_use]
    pub fn project(&self, minor: u16) -> Self {
        let mut out = self.clone();
        if minor < MINOR_1 {
            for tab in &mut out.tabs {
                tab.pinned = false;
            }
        }
        out
    }

    /// A stable FNV-1a digest over the fields `minor` holds.
    #[must_use]
    pub fn digest(&self, minor: u16) -> u64 {
        let mut hash = Fnv::default();
        for tab in &self.tabs {
            hash.u(tab.id);
            hash.bytes(tab.name.as_bytes());
            if minor >= MINOR_1 {
                hash.u(u64::from(tab.pinned));
            }
        }
        hash.0
    }
}

/// 64-bit FNV-1a.
#[derive(Debug)]
pub struct Fnv(pub u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    /// Hashes bytes.
    pub fn bytes(&mut self, bytes: &[u8]) {
        self.u(bytes.len() as u64);
        for byte in bytes {
            self.byte(*byte);
        }
    }

    /// Hashes one byte.
    pub fn byte(&mut self, byte: u8) {
        self.0 ^= u64::from(byte);
        self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
    }

    /// Hashes a little-endian `u64`.
    pub fn u(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.byte(byte);
        }
    }
}

/// One cell: a character and its style bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cell {
    /// Displayed character.
    pub ch: char,
    /// Style bits.
    pub style: u8,
}

impl Cell {
    /// A blank cell.
    pub const BLANK: Self = Self { ch: ' ', style: 0 };

    /// This cell as a peer on `minor` renders it.
    #[must_use]
    pub fn project(self, minor: u16) -> Self {
        if minor < MINOR_1 && self.style & STYLE_CURLY != 0 {
            Self {
                ch: self.ch,
                style: (self.style & !STYLE_CURLY) | STYLE_UNDERLINE,
            }
        } else {
            self
        }
    }
}

/// A row shared between snapshots while unchanged.
pub type Row = Rc<Vec<Cell>>;

/// A complete immutable viewport, as core publishes it.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Publication revision.
    pub revision: u64,
    /// Advances on every resize.
    pub geometry: u64,
    /// Columns per row.
    pub columns: usize,
    /// Rows, each with `columns` cells.
    pub rows: Vec<Row>,
    /// Cursor row and column.
    pub cursor: (usize, usize),
    /// Mode bits, such as mouse tracking.
    pub modes: u8,
}

/// Rows and metadata as a client holds them, independent of `Rc` identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct View {
    /// Publication revision.
    pub revision: u64,
    /// Geometry revision.
    pub geometry: u64,
    /// Columns per row.
    pub columns: usize,
    /// Cells per row.
    pub rows: Vec<Vec<Cell>>,
    /// Cursor row and column.
    pub cursor: (usize, usize),
    /// Mode bits.
    pub modes: u8,
}

impl Snapshot {
    /// This snapshot as a peer on `minor` should hold it.
    #[must_use]
    pub fn view(&self, minor: u16) -> View {
        View {
            revision: self.revision,
            geometry: self.geometry,
            columns: self.columns,
            rows: self
                .rows
                .iter()
                .map(|row| row.iter().map(|cell| cell.project(minor)).collect())
                .collect(),
            cursor: self.cursor,
            modes: self.modes,
        }
    }
}

impl View {
    /// A stable digest of rows and metadata.
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut hash = Fnv::default();
        hash.u(self.revision);
        hash.u(self.columns as u64);
        hash.u(self.cursor.0 as u64);
        hash.u(self.cursor.1 as u64);
        hash.byte(self.modes);
        for row in &self.rows {
            for cell in row {
                hash.u(u64::from(u32::from(cell.ch)));
                hash.byte(cell.style);
            }
        }
        hash.0
    }
}

/// Revisioned terminal status. Floods cost constant memory.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Status {
    /// Advances whenever another field changes.
    pub revision: u64,
    /// Latest title.
    pub title: String,
    /// Bells since the terminal started.
    pub bells: u64,
    /// The latest failures with their sequence numbers, at most 4.
    pub failures: Vec<(u64, String)>,
    /// The root process's exit code, once it exited.
    pub exited: Option<u8>,
}

/// Retained failure messages.
pub const FAILURE_CAPACITY: usize = 4;

/// What changed since a viewer's baseline.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Derived {
    /// The exit, the first time it is seen.
    pub exit: Option<u8>,
    /// New bells.
    pub bells: u64,
    /// New failure sequence numbers.
    pub failures: Vec<u64>,
    /// Failures the bounded log dropped.
    pub missed: u64,
}

/// A client's memory of the last status it read for one terminal. Kept
/// across a reconnect to the same incarnation, so it reports what happened
/// while the client was away.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StatusBaseline {
    revision: Option<u64>,
    exited: bool,
    bells: u64,
    failure: u64,
}

impl StatusBaseline {
    /// Compares `status` with the baseline and advances it.
    pub fn observe(&mut self, status: &Status) -> Derived {
        let mut out = Derived::default();
        if self.revision == Some(status.revision) {
            return out;
        }
        self.revision = Some(status.revision);
        if let Some(code) = status.exited
            && !self.exited
        {
            self.exited = true;
            out.exit = Some(code);
        }
        out.bells = status.bells.saturating_sub(self.bells);
        self.bells = self.bells.max(status.bells);
        for (sequence, _) in &status.failures {
            if *sequence > self.failure {
                out.failures.push(*sequence);
            }
        }
        if let Some(first) = out.failures.first() {
            out.missed = first.saturating_sub(self.failure + 1);
        } else if let Some((last, _)) = status.failures.last()
            && *last > self.failure
        {
            out.missed = last - self.failure;
        }
        self.failure =
            self.failure.max(status.failures.last().map_or(0, |f| f.0));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: u64, name: &str) -> TabInfo {
        TabInfo {
            id,
            name: name.into(),
            pinned: false,
        }
    }

    #[test]
    fn reducer_applies_in_order_and_detects_gaps_and_stale_events() {
        let mut state = Hierarchy::default();
        assert_eq!(
            state.apply(
                1,
                &Event::Opened {
                    tab: tab(1, "a"),
                    index: 0
                }
            ),
            Applied::Applied
        );
        assert_eq!(state.apply(1, &Event::Skipped), Applied::Stale);
        assert_eq!(state.apply(3, &Event::Skipped), Applied::Gap);
        assert_eq!(
            state.apply(2, &Event::Closed { tab: 9 }),
            Applied::Inconsistent
        );
        assert_eq!(state.apply(2, &Event::Skipped), Applied::Applied);
        assert_eq!(state.seq, 2);
    }

    #[test]
    fn downgrades_use_one_of_three_paths() {
        let pinned = Event::Pinned {
            tab: 1,
            pinned: true,
        };
        assert_eq!(pinned.downgrade(MINOR_0), Downgrade::Send(Event::Skipped));
        let retitled = Event::Retitled {
            tab: 1,
            name: "b".into(),
        };
        assert_eq!(
            retitled.downgrade(MINOR_0),
            Downgrade::Send(Event::Renamed {
                tab: 1,
                name: "b".into()
            })
        );
        assert_eq!(Event::Reversed.downgrade(MINOR_0), Downgrade::Resync);
        assert_eq!(
            Event::Reversed.downgrade(MINOR_1),
            Downgrade::Send(Event::Reversed)
        );
    }

    #[test]
    fn digests_cover_only_the_profile_fields() {
        let mut state = Hierarchy::default();
        state.mutate(&Event::Opened {
            tab: tab(1, "a"),
            index: 0,
        });
        let before = state.digest(MINOR_0);
        state.mutate(&Event::Pinned {
            tab: 1,
            pinned: true,
        });
        assert_eq!(state.digest(MINOR_0), before);
        assert_ne!(
            state.digest(MINOR_1),
            state.project(MINOR_0).digest(MINOR_1)
        );
        assert_eq!(state.project(MINOR_0).digest(MINOR_0), before);
    }

    #[test]
    fn status_baseline_reports_what_happened_while_away() {
        let mut baseline = StatusBaseline::default();
        let mut status = Status {
            revision: 1,
            ..Status::default()
        };
        assert_eq!(baseline.observe(&status), Derived::default());
        status.revision = 9;
        status.bells = 3;
        status.exited = Some(2);
        status.failures = vec![(5, "x".into()), (6, "y".into())];
        let derived = baseline.observe(&status);
        assert_eq!(derived.exit, Some(2));
        assert_eq!(derived.bells, 3);
        assert_eq!(derived.failures, vec![5, 6]);
        assert_eq!(derived.missed, 4);
        assert_eq!(baseline.observe(&status), Derived::default());
    }
}
