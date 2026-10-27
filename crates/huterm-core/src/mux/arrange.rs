//! Rearranging one tab's existing panes into a new split tree, which named
//! layouts, layout cycling, and balancing share.
//!
//! An arrangement never starts or stops a terminal and never moves a pane
//! to another tab: only the tab's split tree and weights change. Clients
//! choose the arrangement, because fitting it to minimum pane sizes needs
//! their geometry; the runtime owns its validity. Every check runs before
//! any change, so a refusal changes nothing and emits nothing, and no
//! arrangement advances [`Mux::revision`].

use huterm_protocol::{
    Arrangement, LayoutChange, LayoutError, PaneLayout, SplitId, TabId,
    WorkspaceId,
};

use super::{Mux, MuxError};

impl Mux {
    /// Rearranges a tab's panes into `arrangement`, which must hold exactly
    /// the tab's panes, while the tab still has `expected`'s structure: its
    /// shape, split identities, panes, and terminals, ignoring weights.
    ///
    /// An arrangement of the same shape keeps every split identity and
    /// changes only weights; another shape takes a new identity for each
    /// split. A change clears the tab's shared zoom and emits one
    /// `PaneLayoutChanged` carrying it. An arrangement equal to the current
    /// layout, weights included, changes nothing, the shared zoom included,
    /// and returns an empty change. No arrangement changes which terminals a
    /// close covers, so pending close assessments stay valid.
    /// # Errors
    /// Rejects foreign or missing targets, a tab whose structure changed
    /// since `expected` with [`LayoutError::ChildrenChanged`], an
    /// arrangement that adds, drops, or repeats a pane or breaks a layout
    /// invariant, and exhausted identities.
    pub fn arrange_panes(
        &mut self,
        workspace: WorkspaceId,
        tab: TabId,
        expected: &PaneLayout,
        arrangement: &Arrangement,
    ) -> Result<LayoutChange, MuxError> {
        self.select_workspace_tab(workspace, tab)?;
        let current = &self.tab_record(workspace, tab)?.layout;
        if !current.same_structure(expected) {
            return Err(MuxError::InvalidLayout(LayoutError::ChildrenChanged));
        }
        // New splits take the identities the next allocations would return,
        // reserved only once the layout is known to change.
        let (runtime, first) = (self.runtime_id, self.next_id);
        let mut count = 0_u64;
        let layout = current.arranged(arrangement, || {
            count += 1;
            SplitId::in_runtime(runtime, first.wrapping_add(count))
        });
        if self.next_id.checked_add(count).is_none() {
            return Err(MuxError::IdExhausted);
        }
        let layout = layout.map_err(MuxError::InvalidLayout)?;
        if layout == *current {
            return Ok(LayoutChange::default());
        }
        let before = vec![self.snapshot(tab)];
        self.reserve_ids(count)?;
        let record = self.tab_record_mut(workspace, tab)?;
        record.layout = layout;
        record.zoomed = None;
        self.emit_layout(workspace, tab);
        Ok(LayoutChange {
            before,
            after: vec![self.snapshot(tab)],
            placed: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests;
