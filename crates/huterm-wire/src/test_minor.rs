//! A test-only minor version, one above [`MINOR`], that exercises the
//! version rules before a real minor version 1 exists.
//!
//! It appends one hierarchy field, a tab's `pinned` flag, and adds three
//! event variants, one for each downgrade path a server takes for an older
//! peer:
//!
//! - [`TestEvent::TabPinned`] changes only the appended field, so an older
//!   peer receives [`HierarchyEvent::Skipped`].
//! - [`TestEvent::TabRetitled`] changes what one older event changes, so
//!   an older peer receives that [`HierarchyEvent::TabRenamed`].
//! - [`TestEvent::TabsExchanged`] changes older state in a way no single
//!   older event expresses, so an older peer receives a resync snapshot
//!   projected onto its version.
//!
//! It also adds a cell style flag, [`CURLY_UNDERLINE`], which a server
//! projects onto an older peer as a plain underline.
//!
//! Its digests cover the appended field and the style flag only for peers at
//! its version, so a projected state and its digest always agree. Shipping
//! clients and servers never enable it.

use std::collections::BTreeSet;
use std::sync::Arc;

use huterm_protocol::{
    ApplyOutcome, HierarchyEnvelope, HierarchyEvent, HierarchyState,
    MAX_NAME_BYTES, StreamId, TabId, TabInfo, TerminalRow,
};

use crate::codec::{DecodeError, EncodeError, IdScope, Reader, Writer};
use crate::hierarchy::{
    HierarchyContext, decode_event, decode_state_with, encode_event,
    encode_state_with,
};
use crate::update::{
    FullRow, STYLE_FLAGS, SourceRow, StyleEntry, UpdateRow, UpdateSource,
    ViewMetadata, ViewRow, build_row, flags_of,
};
use crate::{MINOR, digest_profile};

/// The test-only minor version.
pub const TEST_MINOR: u16 = MINOR + 1;

/// The first event tag after minor version 0's.
const FIRST_TEST_TAG: u64 = 23;

/// A hierarchy at the test-only minor version: minor version 0's state and
/// the tabs the appended field pins.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestHierarchy {
    /// The minor version 0 state.
    pub state: HierarchyState,
    /// The pinned tabs.
    pub pinned: BTreeSet<TabId>,
}

/// A hierarchy event at the test-only minor version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TestEvent {
    /// A minor version 0 event.
    Base(HierarchyEvent),
    /// A tab's pinned flag changed: test-only state alone.
    TabPinned {
        /// The tab.
        tab: TabId,
        /// Whether it is pinned now.
        pinned: bool,
    },
    /// A tab's custom name changed, and it was pinned: one older event
    /// covers the older state's part.
    TabRetitled {
        /// The tab.
        tab: TabId,
        /// The new custom name.
        custom_name: Option<String>,
    },
    /// Two tabs of one workspace exchanged positions, which no single
    /// older event expresses.
    TabsExchanged {
        /// One tab.
        first: TabId,
        /// The other tab.
        second: TabId,
    },
}

/// What a server sends a peer for one test-minor event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Delivery {
    /// The event itself, to a peer at the test-only minor version.
    Native(TestEvent),
    /// One older event: the event's own, `Skipped`, or its one older
    /// equivalent.
    Event(HierarchyEvent),
    /// A snapshot projected onto the peer's version at the event's
    /// sequence.
    Resync(HierarchyState),
}

impl TestHierarchy {
    /// A hierarchy with no pinned tabs.
    #[must_use]
    pub fn new(state: HierarchyState) -> Self {
        Self {
            state,
            pinned: BTreeSet::new(),
        }
    }

    /// The state an older peer holds: minor version 0's fields.
    #[must_use]
    pub fn project(&self) -> HierarchyState {
        self.state.clone()
    }

    /// The digest for a peer at `minor`: the appended field counts only at
    /// the test-only minor version.
    #[must_use]
    pub fn digest(&self, minor: u16) -> u64 {
        let base = self.state.digest_for(digest_profile(minor));
        if minor < TEST_MINOR {
            return base;
        }
        // FNV-1a over the base digest and each pinned tab, in order.
        let mut hash = base;
        for tab in &self.pinned {
            for byte in tab.get().to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        hash
    }

    /// Applies the next event, returning what a peer at `peer_minor`
    /// receives for it, or `None` when the event does not apply.
    pub fn apply(
        &mut self,
        event: &TestEvent,
        peer_minor: u16,
    ) -> Option<Delivery> {
        let seq = self.state.seq() + 1;
        let older = match event {
            TestEvent::Base(event) => {
                self.advance(seq, event.clone())?;
                Delivery::Event(event.clone())
            }
            TestEvent::TabPinned { tab, pinned } => {
                self.state.tab(*tab)?;
                self.advance(seq, HierarchyEvent::Skipped)?;
                if *pinned {
                    self.pinned.insert(*tab);
                } else {
                    self.pinned.remove(tab);
                }
                Delivery::Event(HierarchyEvent::Skipped)
            }
            TestEvent::TabRetitled { tab, custom_name } => {
                let renamed = HierarchyEvent::TabRenamed {
                    tab: *tab,
                    custom_name: custom_name.clone(),
                };
                self.advance(seq, renamed.clone())?;
                self.pinned.insert(*tab);
                Delivery::Event(renamed)
            }
            TestEvent::TabsExchanged { first, second } => {
                self.state = exchanged(&self.state, seq, *first, *second)?;
                Delivery::Resync(self.project())
            }
        };
        // A tab that closed by any path, its workspace's or session's close
        // included, leaves the appended state with it.
        let state = &self.state;
        self.pinned.retain(|tab| state.tab(*tab).is_some());
        Some(if peer_minor >= TEST_MINOR {
            Delivery::Native(event.clone())
        } else {
            older
        })
    }

    fn advance(&mut self, seq: u64, event: HierarchyEvent) -> Option<()> {
        let envelope = HierarchyEnvelope {
            stream: self.state.stream(),
            seq,
            event,
        };
        matches!(self.state.apply(envelope), ApplyOutcome::Applied(_))
            .then_some(())
    }
}

/// The state at `seq` with two tabs of one workspace in each other's
/// places.
fn exchanged(
    state: &HierarchyState,
    seq: u64,
    first: TabId,
    second: TabId,
) -> Option<HierarchyState> {
    let workspace = state.tab_workspace(first)?;
    if state.tab_workspace(second)? != workspace || first == second {
        return None;
    }
    rebuilt(state, state.stream(), seq, |id| {
        if id == first {
            second
        } else if id == second {
            first
        } else {
            id
        }
    })
}

/// `state` rebuilt at `seq` on `stream`, with each tab position holding the
/// tab `place` maps its tab to.
fn rebuilt(
    state: &HierarchyState,
    stream: StreamId,
    seq: u64,
    place: impl Fn(TabId) -> TabId,
) -> Option<HierarchyState> {
    let sessions: Vec<_> = state
        .sessions()
        .iter()
        .map(|session| {
            let workspaces: Vec<_> = state
                .session_workspaces(*session)
                .unwrap_or_default()
                .iter()
                .map(|workspace| {
                    let tabs: Vec<TabInfo> = state
                        .workspace_tabs(*workspace)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|tab| state.tab(place(*tab)).cloned())
                        .collect();
                    (state.workspace(*workspace).cloned(), tabs)
                })
                .map(|(info, tabs)| info.map(|info| (info, tabs)))
                .collect::<Option<_>>()?;
            Some((state.session(*session)?.clone(), workspaces))
        })
        .collect::<Option<_>>()?;
    let attachments: Vec<_> = state
        .sessions()
        .iter()
        .flat_map(|session| {
            state
                .session_attachments(*session)
                .map(move |attachment| (attachment, *session))
        })
        .collect();
    HierarchyState::from_sessions(stream, seq, sessions, attachments)
}

/// Encodes a snapshot record at the test-only minor version: minor version
/// 0's record with each tab's pinned flag appended.
///
/// # Errors
/// Returns the minor version 0 encoder's error.
pub fn encode_snapshot(
    w: &mut Writer,
    scope: IdScope,
    hierarchy: &TestHierarchy,
) -> Result<(), EncodeError> {
    w.record(|w| {
        w.u64(hierarchy.state.seq());
        w.fixed(&hierarchy.digest(TEST_MINOR).to_le_bytes());
        encode_state_with(w, scope, &hierarchy.state, &|w, tab| {
            w.bool(hierarchy.pinned.contains(&tab.id));
            Ok(())
        })
    })
}

/// Decodes a snapshot record at the test-only minor version; a minor
/// version 0 record decodes with no pinned tabs.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_snapshot(
    r: &mut Reader<'_>,
    context: HierarchyContext,
) -> Result<(u64, TestHierarchy), DecodeError> {
    r.record(|r| {
        let seq = r.u64()?;
        let digest = u64::from_le_bytes(r.fixed()?);
        let mut pinned = BTreeSet::new();
        let state = decode_state_with(r, context, seq, &mut |r, tab| {
            if r.appended(false, Reader::bool)? {
                pinned.insert(tab.id);
            }
            Ok(())
        })?;
        Ok((digest, TestHierarchy { state, pinned }))
    })
}

/// Encodes a test-minor event.
///
/// # Errors
/// Returns the minor version 0 encoder's error.
pub fn encode_test_event(
    w: &mut Writer,
    scope: IdScope,
    event: &TestEvent,
) -> Result<(), EncodeError> {
    match event {
        TestEvent::Base(event) => encode_event(w, scope, event),
        TestEvent::TabPinned { tab, pinned } => {
            w.tag(FIRST_TEST_TAG);
            w.record(|w| {
                w.id(scope, *tab)?;
                w.bool(*pinned);
                Ok(())
            })
        }
        TestEvent::TabRetitled { tab, custom_name } => {
            w.tag(FIRST_TEST_TAG + 1);
            w.record(|w| {
                w.id(scope, *tab)?;
                w.optional_str(
                    custom_name.as_deref(),
                    MAX_NAME_BYTES,
                    "tab name",
                )
            })
        }
        TestEvent::TabsExchanged { first, second } => {
            w.tag(FIRST_TEST_TAG + 2);
            w.record(|w| {
                w.id(scope, *first)?;
                w.id(scope, *second)
            })
        }
    }
}

/// Decodes a test-minor event.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_test_event(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<TestEvent, DecodeError> {
    let mut peek = Reader::new(r.peek());
    match peek.tag()? {
        tag if tag < FIRST_TEST_TAG => {
            decode_event(r, scope).map(TestEvent::Base)
        }
        tag => {
            r.tag()?;
            r.record(|r| match tag - FIRST_TEST_TAG {
                0 => Ok(TestEvent::TabPinned {
                    tab: r.id(scope)?,
                    pinned: r.bool()?,
                }),
                1 => Ok(TestEvent::TabRetitled {
                    tab: r.id(scope)?,
                    custom_name: r
                        .optional_string(MAX_NAME_BYTES, "tab name")?,
                }),
                2 => Ok(TestEvent::TabsExchanged {
                    first: r.id(scope)?,
                    second: r.id(scope)?,
                }),
                _ => Err(DecodeError::UnknownTag {
                    field: "hierarchy event",
                    tag,
                }),
            })
        }
    }
}

/// The cell style flag the test-only minor version adds: a curly
/// underline, which an older peer shows as a plain underline.
pub const CURLY_UNDERLINE: u64 = 1 << 8;

/// Minor version 0's underline flag, which [`CURLY_UNDERLINE`] projects to.
const UNDERLINE: u64 = 1 << 3;

/// The style flags a peer at the test-only minor version understands.
pub const TEST_STYLE_FLAGS: u64 = STYLE_FLAGS | CURLY_UNDERLINE;

/// A row at the test-only minor version: minor version 0's row, and which
/// of its cells have a curly underline.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TestRow {
    /// Minor version 0's row.
    pub row: TerminalRow,
    /// Each cell's curly underline.
    pub curly: Vec<bool>,
}

impl UpdateRow for Arc<TestRow> {
    fn row(&self) -> &TerminalRow {
        &self.row
    }

    fn flags(&self, column: usize, minor: u16) -> u64 {
        let base = self
            .row
            .cells
            .get(column)
            .map_or(0, |cell| flags_of(cell.style));
        if !self.curly.get(column).copied().unwrap_or(false) {
            base
        } else if minor >= TEST_MINOR {
            base | CURLY_UNDERLINE
        } else {
            base | UNDERLINE
        }
    }
}

impl SourceRow for Arc<TestRow> {
    fn identity(&self) -> usize {
        Arc::as_ptr(self).addr()
    }
}

impl ViewRow for Arc<TestRow> {
    fn build(row: &FullRow, styles: &[StyleEntry]) -> Self {
        let curly = row
            .runs
            .iter()
            .flat_map(|run| {
                let curly = styles
                    .get(run.style as usize)
                    .is_some_and(|style| style.flags & CURLY_UNDERLINE != 0);
                std::iter::repeat_n(curly, run.cells)
            })
            .collect();
        Arc::new(TestRow {
            row: build_row(row, styles),
            curly,
        })
    }
}

/// A viewport at the test-only minor version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestSnapshot {
    /// Everything besides rows.
    pub metadata: ViewMetadata,
    /// The rows.
    pub rows: Vec<Arc<TestRow>>,
}

impl UpdateSource for Arc<TestSnapshot> {
    type Row = Arc<TestRow>;

    fn metadata(&self) -> ViewMetadata {
        self.metadata.clone()
    }

    fn rows(&self) -> &[Self::Row] {
        &self.rows
    }
}

#[cfg(test)]
mod tests;
