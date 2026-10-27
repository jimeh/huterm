//! Each window's layout history: the last pane rearrangement it made, which
//! undo restores, and the last one it undid, which redo reapplies.
//!
//! The history holds one entry each way. An entry is checked against the
//! projection after every batch that touches its tabs, and dropped once the
//! runtime would refuse to restore it, so menus and the palette only offer
//! what can still apply. The runtime checks again under its lock, so a
//! projection that lags never applies a restore to the wrong state.

use huterm_core::RestoreDirection;
use huterm_protocol::{HierarchyState, LayoutChange, PaneId, Touched};

/// One recorded rearrangement and the pane to focus once it is restored.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HistoryEntry {
    /// The operation's name, such as "Move Pane".
    pub(super) label: &'static str,
    /// The pane the window focused after the operation.
    pub(super) focus: Option<PaneId>,
    /// What the operation did, as the runtime returned it.
    pub(super) change: LayoutChange,
    /// The hierarchy sequence the operation, or the restore that filed the
    /// entry last, committed at.
    pub(super) seq: u64,
}

impl HistoryEntry {
    /// Whether the projection still holds the state that restoring in
    /// `direction` leaves, as the runtime's guard checks it.
    pub(super) fn restorable(
        &self,
        direction: RestoreDirection,
        state: &HierarchyState,
    ) -> bool {
        let from = match direction {
            RestoreDirection::Undo => &self.change.after,
            RestoreDirection::Redo => &self.change.before,
        };
        from.iter().all(|snapshot| {
            snapshot.still_matches(
                &state.tab_snapshot(snapshot.tab),
                self.change.placed.contains(&snapshot.tab),
            )
        })
    }

    fn touched_by(&self, touched: &Touched) -> bool {
        self.change
            .before
            .iter()
            .any(|snapshot| touched.contains_tab(snapshot.tab))
    }
}

/// A window's undo entry and redo entry.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct LayoutHistory {
    undo: Option<HistoryEntry>,
    redo: Option<HistoryEntry>,
}

impl LayoutHistory {
    /// Records a new operation as the undo entry, replacing the old one,
    /// and clears the redo entry. An operation that committed before the
    /// newest entry is ignored: operations of different kinds can complete
    /// out of commit order, and the newer entry already covers its tabs'
    /// later state.
    pub(super) fn record(&mut self, entry: HistoryEntry) -> bool {
        let newest = self.undo.iter().chain(&self.redo).map(|entry| entry.seq);
        if newest.max().is_some_and(|newest| entry.seq < newest) {
            return false;
        }
        self.undo = Some(entry);
        self.redo = None;
        true
    }

    /// The entry `direction` would restore next.
    pub(super) fn peek(
        &self,
        direction: RestoreDirection,
    ) -> Option<&HistoryEntry> {
        match direction {
            RestoreDirection::Undo => self.undo.as_ref(),
            RestoreDirection::Redo => self.redo.as_ref(),
        }
    }

    /// Takes the entry `direction` restores, so nothing prunes it while the
    /// restore is in flight.
    pub(super) fn take(
        &mut self,
        direction: RestoreDirection,
    ) -> Option<HistoryEntry> {
        self.slot_mut(direction).take()
    }

    /// Files an entry the restore in `direction` applied at `seq`: an
    /// undone entry becomes the redo entry, and a redone one the undo
    /// entry.
    pub(super) fn finish(
        &mut self,
        direction: RestoreDirection,
        mut entry: HistoryEntry,
        seq: u64,
    ) {
        entry.seq = seq;
        *self.slot_mut(opposite(direction)) = Some(entry);
    }

    /// Drops each entry whose tabs `touched` names and that the projection
    /// would no longer let restore.
    pub(super) fn prune(&mut self, touched: &Touched, state: &HierarchyState) {
        for direction in [RestoreDirection::Undo, RestoreDirection::Redo] {
            let slot = self.slot_mut(direction);
            if slot.as_ref().is_some_and(|entry| {
                entry.touched_by(touched) && !entry.restorable(direction, state)
            }) {
                *slot = None;
            }
        }
    }

    fn slot_mut(
        &mut self,
        direction: RestoreDirection,
    ) -> &mut Option<HistoryEntry> {
        match direction {
            RestoreDirection::Undo => &mut self.undo,
            RestoreDirection::Redo => &mut self.redo,
        }
    }
}

fn opposite(direction: RestoreDirection) -> RestoreDirection {
    match direction {
        RestoreDirection::Undo => RestoreDirection::Redo,
        RestoreDirection::Redo => RestoreDirection::Undo,
    }
}

#[cfg(test)]
mod tests;
