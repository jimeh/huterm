//! Restoring the tabs a pane rearrangement touched, for undo and redo.
//!
//! A restore applies one side of a recorded [`LayoutChange`] only while the
//! touched tabs still hold the structure the other side recorded. It never
//! starts or stops a terminal: like a move, it changes which tab holds a
//! pane. Every check runs before any change, so a refusal changes nothing,
//! emits nothing, and leaves [`Mux::revision`] alone.

use std::collections::{HashMap, HashSet};

use huterm_protocol::{
    HierarchyEvent, LayoutChange, PaneId, PaneInfo, PaneLayout, RuntimeId,
    SessionId, TabChange, TabId, TabRecord, TabSnapshot, TerminalId,
    WorkspaceId,
};

use super::{Mux, MuxError, Tab, position};

/// Which side of a recorded [`LayoutChange`] a restore returns to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreDirection {
    /// From the change's `after` back to its `before`.
    Undo,
    /// From the change's `before` forward to its `after`.
    Redo,
}

/// A validated restore, ready to apply without further checks.
struct Restore {
    /// The touched tabs, in the change's order.
    tabs: Vec<TabId>,
    /// Tabs to remove, with their current workspace.
    removed: Vec<(WorkspaceId, TabId)>,
    /// Tabs that exist before and after, with their current workspace,
    /// restored layout, and zoom.
    layouts: Vec<(WorkspaceId, TabId, PaneLayout, Option<PaneId>)>,
    /// Tabs to place or reopen, in the order to insert them.
    inserted: Vec<Insertion>,
    /// Terminals whose session changes.
    revoked: Vec<TerminalId>,
    /// Whether a terminal changes tabs or a tab changes position, which
    /// changes what a close covers.
    advances: bool,
}

enum Insertion {
    Placed {
        tab: TabId,
        from: WorkspaceId,
        workspace: WorkspaceId,
        index: u32,
    },
    Opened {
        workspace: WorkspaceId,
        tab: Tab,
        index: u32,
    },
}

impl Insertion {
    fn index(&self) -> u32 {
        match self {
            Self::Placed { index, .. } | Self::Opened { index, .. } => *index,
        }
    }
}

impl Mux {
    /// Returns the tabs `change` touched to its `before` records for
    /// [`RestoreDirection::Undo`], or to its `after` records for
    /// [`RestoreDirection::Redo`], as one `PanesRearranged`, and returns
    /// the change the restore made.
    ///
    /// The restore applies only while every touched tab matches the side
    /// it leaves: an absent tab is still absent, and a present tab has the
    /// same splits, panes, and terminals, ignoring weights, in the same
    /// session, and a tab in `placed` is still in the same workspace. Each
    /// restored tab takes its recorded layout, weights included, and keeps
    /// its current name. Its zoom returns to the recorded one only when the
    /// change set it and nobody changed it since; otherwise its current
    /// zoom stays while its pane remains. Only a tab in `placed` returns to
    /// its recorded workspace and position. A tab the restore reopens keeps
    /// its identity and gets its recorded name and zoom, at its recorded
    /// position clamped to its workspace's current length.
    /// # Errors
    /// Returns [`MuxError::LayoutChanged`] when a touched tab no longer
    /// matches, a recorded workspace is gone or in another session, or a
    /// restored split is in use elsewhere, and
    /// [`MuxError::InvalidRestore`] for a change this runtime could not
    /// have recorded.
    pub fn restore_tabs(
        &mut self,
        change: &LayoutChange,
        direction: RestoreDirection,
    ) -> Result<LayoutChange, MuxError> {
        let (from, to) = match direction {
            RestoreDirection::Undo => (&change.after, &change.before),
            RestoreDirection::Redo => (&change.before, &change.after),
        };
        let restore = self.plan_restore(from, to, &change.placed)?;
        self.apply_restore(restore)
    }

    /// Checks a restore from `from` to `to` against the current state and
    /// computes everything it changes.
    fn plan_restore(
        &self,
        from: &[TabSnapshot],
        to: &[TabSnapshot],
        placed: &[TabId],
    ) -> Result<Restore, MuxError> {
        let tabs = self.restore_tabs_checked(from, to, placed)?;
        let current: Vec<TabSnapshot> =
            tabs.iter().map(|tab| self.snapshot(*tab)).collect();
        for (from, now) in from.iter().zip(&current) {
            if !matches(from, now, placed.contains(&from.tab)) {
                return Err(MuxError::LayoutChanged);
            }
        }
        if sorted_panes(&current) != sorted_panes(to) {
            return Err(MuxError::InvalidRestore);
        }
        self.check_restored_splits(&tabs, to)?;
        let was = pane_homes(&current);
        let mut restore = Restore {
            tabs,
            removed: Vec::new(),
            layouts: Vec::new(),
            inserted: Vec::new(),
            revoked: Vec::new(),
            advances: false,
        };
        for ((from, now), to) in from.iter().zip(&current).zip(to) {
            let tab = to.tab;
            let session = match (&now.record, &to.record) {
                (None, None) => continue,
                (Some(now), None) => {
                    restore.removed.push((now.workspace, tab));
                    restore.advances = true;
                    continue;
                }
                (None, Some(to)) => {
                    self.check_destination(to)?;
                    restore.inserted.push(Insertion::Opened {
                        workspace: to.workspace,
                        tab: Tab {
                            id: tab,
                            custom_name: to.custom_name.clone(),
                            layout: to.layout.clone(),
                            zoomed: to.zoomed.filter(|_| valid_zoom(to)),
                        },
                        index: to.index,
                    });
                    restore.advances = true;
                    to.session
                }
                (Some(now), Some(to)) => {
                    let zoomed = restored_zoom(from, now, to);
                    if now.layout != to.layout || now.zoomed != zoomed {
                        restore.layouts.push((
                            now.workspace,
                            tab,
                            to.layout.clone(),
                            zoomed,
                        ));
                    }
                    if placed.contains(&tab) {
                        self.check_destination(to)?;
                        if now.workspace != to.workspace
                            || now.index != to.index
                        {
                            restore.inserted.push(Insertion::Placed {
                                tab,
                                from: now.workspace,
                                workspace: to.workspace,
                                index: to.index,
                            });
                            restore.advances = true;
                        }
                        to.session
                    } else {
                        now.session
                    }
                }
            };
            let Some(to) = &to.record else { continue };
            for pane in to.layout.panes() {
                let Some((old_tab, old_session)) = was.get(&pane.id) else {
                    continue;
                };
                restore.advances |= *old_tab != tab;
                if *old_session != session {
                    restore.revoked.push(pane.terminal_id);
                }
            }
        }
        // Lower positions first, so each clamped insertion lands where the
        // record put it relative to the ones before it.
        restore.inserted.sort_by_key(Insertion::index);
        Ok(restore)
    }

    /// Checks that `from` and `to` describe the same tabs, that every
    /// identity they and `placed` name is one this runtime allocated, and
    /// returns the tabs.
    fn restore_tabs_checked(
        &self,
        from: &[TabSnapshot],
        to: &[TabSnapshot],
        placed: &[TabId],
    ) -> Result<Vec<TabId>, MuxError> {
        if from.is_empty() || from.len() != to.len() {
            return Err(MuxError::InvalidRestore);
        }
        let mut tabs = Vec::with_capacity(to.len());
        for (from, to) in from.iter().zip(to) {
            if from.tab != to.tab || tabs.contains(&to.tab) {
                return Err(MuxError::InvalidRestore);
            }
            self.check_allocated(to.tab.runtime(), to.tab.get())?;
            tabs.push(to.tab);
        }
        if placed.iter().any(|tab| !tabs.contains(tab)) {
            return Err(MuxError::InvalidRestore);
        }
        for record in from.iter().chain(to).filter_map(|s| s.record.as_ref()) {
            for split in record.layout.split_ids() {
                self.check_allocated(split.runtime(), split.get())?;
            }
            for pane in record.layout.panes() {
                self.check_allocated(pane.id.runtime(), pane.id.get())?;
            }
        }
        Ok(tabs)
    }

    /// Refuses an identity of another runtime or one past the allocation
    /// high-water mark.
    fn check_allocated(
        &self,
        runtime: RuntimeId,
        id: u64,
    ) -> Result<(), MuxError> {
        self.validate_scope(runtime)?;
        if id == 0 || id > self.next_id {
            return Err(MuxError::InvalidRestore);
        }
        Ok(())
    }

    /// Refuses restored layouts that share a split with each other or with
    /// a tab the restore does not touch.
    fn check_restored_splits(
        &self,
        tabs: &[TabId],
        to: &[TabSnapshot],
    ) -> Result<(), MuxError> {
        let mut splits = HashSet::new();
        for record in to.iter().filter_map(|snapshot| snapshot.record.as_ref())
        {
            for split in record.layout.split_ids() {
                if !splits.insert(split) {
                    return Err(MuxError::InvalidRestore);
                }
            }
        }
        let in_use = self
            .workspaces
            .values()
            .flat_map(|workspace| &workspace.tabs)
            .filter(|tab| !tabs.contains(&tab.id))
            .flat_map(|tab| tab.layout.split_ids())
            .any(|split| splits.contains(&split));
        if in_use {
            return Err(MuxError::LayoutChanged);
        }
        Ok(())
    }

    /// Refuses a destination workspace that is gone or in another session.
    fn check_destination(&self, to: &TabRecord) -> Result<(), MuxError> {
        self.validate_scope(to.workspace.runtime())?;
        match self.workspaces.get(&to.workspace) {
            Some(workspace) if workspace.session_id == to.session => Ok(()),
            _ => Err(MuxError::LayoutChanged),
        }
    }

    /// Applies a validated restore and emits its event.
    fn apply_restore(
        &mut self,
        restore: Restore,
    ) -> Result<LayoutChange, MuxError> {
        let before: Vec<TabSnapshot> =
            restore.tabs.iter().map(|tab| self.snapshot(*tab)).collect();
        let mut changes = Vec::new();
        let mut placed = Vec::new();
        for terminal in restore.revoked {
            self.invalidate_terminal_authority(terminal);
        }
        if restore.advances {
            self.changed();
        }
        for (workspace, tab) in restore.removed {
            self.remove_tab_record(workspace, tab)?;
            changes.push(TabChange::Removed { tab });
        }
        for (workspace, tab, layout, zoomed) in restore.layouts {
            let record = self.tab_record_mut(workspace, tab)?;
            record.layout = layout.clone();
            record.zoomed = zoomed;
            changes.push(TabChange::Layout {
                tab,
                layout,
                zoomed,
            });
        }
        for insertion in restore.inserted {
            match insertion {
                Insertion::Placed {
                    tab,
                    from,
                    workspace,
                    index,
                } => {
                    let record = self.remove_tab_record(from, tab)?;
                    let index = self.insert_tab(workspace, record, index)?;
                    placed.push(tab);
                    changes.push(TabChange::Placed {
                        tab,
                        workspace,
                        index,
                    });
                }
                Insertion::Opened {
                    workspace,
                    tab,
                    index,
                } => {
                    let info = tab.info();
                    let index = self.insert_tab(workspace, tab, index)?;
                    changes.push(TabChange::Opened {
                        workspace,
                        tab: info,
                        index,
                    });
                }
            }
        }
        if changes.is_empty() {
            return Ok(LayoutChange::default());
        }
        self.emit_hierarchy(|_| HierarchyEvent::PanesRearranged { changes });
        let after =
            restore.tabs.iter().map(|tab| self.snapshot(*tab)).collect();
        Ok(LayoutChange {
            before,
            after,
            placed,
        })
    }

    /// Inserts a tab at `index`, clamped to the workspace's length, and
    /// returns where it landed.
    fn insert_tab(
        &mut self,
        workspace: WorkspaceId,
        tab: Tab,
        index: u32,
    ) -> Result<u32, MuxError> {
        let tabs = &mut self
            .workspaces
            .get_mut(&workspace)
            .ok_or(MuxError::UnknownWorkspace(workspace))?
            .tabs;
        let index =
            usize::try_from(index).unwrap_or(usize::MAX).min(tabs.len());
        tabs.insert(index, tab);
        Ok(position(index))
    }
}

/// Every pane the snapshots hold, in identity order.
fn sorted_panes(snapshots: &[TabSnapshot]) -> Vec<PaneInfo> {
    let mut panes: Vec<PaneInfo> = snapshots
        .iter()
        .filter_map(|snapshot| snapshot.record.as_ref())
        .flat_map(|record| record.layout.panes())
        .cloned()
        .collect();
    panes.sort_by_key(|pane| pane.id);
    panes
}

/// Where each pane of the snapshots is: its tab and session.
fn pane_homes(
    snapshots: &[TabSnapshot],
) -> HashMap<PaneId, (TabId, SessionId)> {
    let mut homes = HashMap::new();
    for snapshot in snapshots {
        if let Some(record) = &snapshot.record {
            for pane in record.layout.panes() {
                homes.insert(pane.id, (snapshot.tab, record.session));
            }
        }
    }
    homes
}

/// Whether the current snapshot `now` still matches the recorded `from`.
/// Weights, zoom, position, and name never block a restore; a placed
/// tab's workspace does.
fn matches(from: &TabSnapshot, now: &TabSnapshot, placed: bool) -> bool {
    match (&from.record, &now.record) {
        (None, None) => true,
        (Some(from), Some(now)) => {
            from.session == now.session
                && from.layout.same_structure(&now.layout)
                && (!placed || from.workspace == now.workspace)
        }
        _ => false,
    }
}

/// The zoom a tab that exists before and after a restore ends with: the
/// recorded `to` zoom when the change set the zoom and nobody changed it
/// since, and otherwise the current zoom while its pane remains. A layout
/// of one pane has no zoom.
fn restored_zoom(
    from: &TabSnapshot,
    now: &TabRecord,
    to: &TabRecord,
) -> Option<PaneId> {
    let recorded = from.record.as_ref().map(|from| from.zoomed);
    let zoomed = if recorded
        .is_some_and(|recorded| recorded != to.zoomed && recorded == now.zoomed)
    {
        to.zoomed
    } else {
        now.zoomed
    };
    zoomed.filter(|pane| {
        to.layout.pane_count() > 1 && to.layout.pane(*pane).is_some()
    })
}

/// Whether a record's zoom is valid over its layout.
fn valid_zoom(record: &TabRecord) -> bool {
    record.zoomed.is_none_or(|pane| {
        record.layout.pane_count() > 1 && record.layout.pane(pane).is_some()
    })
}

#[cfg(test)]
mod tests;
