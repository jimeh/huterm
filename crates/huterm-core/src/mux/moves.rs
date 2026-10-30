//! Moving and swapping panes within and between tabs, and dropping a whole
//! tab into a pane.
//!
//! No operation here starts or stops a terminal: each changes which tab
//! holds a pane, never the pane or its terminal. Every operation validates
//! its targets, computes every resulting layout, and checks that identities
//! remain before it allocates or mutates anything, so a refusal changes
//! nothing, emits nothing, and leaves [`Mux::revision`] alone. A change
//! that touches one tab emits `PaneLayoutChanged`; a change across tabs
//! emits one `PanesRearranged`.
//!
//! A custom name moves between a pane and its tab rather than being
//! copied: a pane extracted into a new tab gives that tab its name, and the
//! only pane of a tab removed into another tab takes the tab's name when it
//! has one. A multi-pane tab nested into another tab loses its own name.

use huterm_protocol::{
    HierarchyEvent, LayoutChange, LayoutError, PaneId, PaneInfo, PaneLayout,
    PaneNode, SessionId, SplitDirection, SplitId, TabChange, TabId, TabRecord,
    TabSnapshot, WorkspaceId,
};

use super::{Mux, MuxError, Tab, layout_error, position};

/// A pane to move, with the tab and workspace it is expected in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaneRef {
    /// Expected workspace of the pane's tab.
    pub workspace: WorkspaceId,
    /// Expected tab of the pane.
    pub tab: TabId,
    /// The pane.
    pub pane: PaneId,
}

/// Where a moved pane or tab lands beside an existing pane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Beside {
    /// The target pane, with its expected tab and workspace.
    pub target: PaneRef,
    /// The side of the target the moved node takes.
    pub side: SplitDirection,
}

/// Where [`Mux::move_pane`] puts a pane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoveDestination {
    /// Beside a pane in any tab of this runtime.
    Beside(Beside),
    /// Alone in a new tab of a workspace, before an anchor tab or at the
    /// end.
    NewTab {
        /// Destination workspace.
        workspace: WorkspaceId,
        /// The tab the new tab goes before; `None` appends.
        before: Option<TabId>,
    },
}

impl Mux {
    /// Exchanges the places of two panes of one tab. Each slot keeps its
    /// weight and the shared zoom stays on its pane. A swap changes no
    /// close ticket's scope, so pending close assessments stay valid.
    /// Swapping a pane with itself changes nothing.
    /// # Errors
    /// Rejects foreign, missing, or nonmember targets.
    pub fn swap_panes(
        &mut self,
        workspace: WorkspaceId,
        tab: TabId,
        a: PaneId,
        b: PaneId,
    ) -> Result<LayoutChange, MuxError> {
        self.select_workspace_pane(workspace, tab, a)?;
        self.select_workspace_pane(workspace, tab, b)?;
        if a == b {
            return Ok(LayoutChange::default());
        }
        let layout = self
            .tab_record(workspace, tab)?
            .layout
            .with_swapped(a, b)
            .map_err(|error| layout_error(error, a))?;
        let before = vec![self.snapshot(tab)];
        self.tab_record_mut(workspace, tab)?.layout = layout;
        self.emit_layout(workspace, tab);
        Ok(LayoutChange {
            before,
            after: vec![self.snapshot(tab)],
            placed: Vec::new(),
        })
    }

    /// Moves a pane beside another pane, in its own tab or any other tab of
    /// this runtime, or into a new tab. The pane keeps its identity and its
    /// terminal keeps running; a tab whose last pane leaves is removed, and
    /// a pane extracted from a tab of one pane moves that tab instead. A
    /// move into another session revokes the pane's viewers.
    ///
    /// A move whose result has the same shape as the current layout, a
    /// pane moved beside itself, and a lone pane extracted within its
    /// workspace change nothing.
    /// # Errors
    /// Rejects foreign, missing, or nonmember targets, a target too small
    /// or a parent too full to take the pane, and exhausted identities.
    pub fn move_pane(
        &mut self,
        from: PaneRef,
        to: MoveDestination,
    ) -> Result<LayoutChange, MuxError> {
        self.select_workspace_pane(from.workspace, from.tab, from.pane)?;
        match to {
            MoveDestination::Beside(beside) => {
                self.select_workspace_pane(
                    beside.target.workspace,
                    beside.target.tab,
                    beside.target.pane,
                )?;
                if beside.target.tab == from.tab {
                    self.move_within_tab(from, beside)
                } else {
                    self.move_between_tabs(from, beside)
                }
            }
            MoveDestination::NewTab { workspace, before } => {
                self.extract_pane(from, workspace, before)
            }
        }
    }

    /// Drops a whole tab into a pane of another tab: the tab's layout nests
    /// beside the target with its proportions and identities, a root split
    /// on the axis of the split that takes it flattening into that split,
    /// and the emptied tab is removed. A drop into another session revokes
    /// the moved panes' viewers.
    /// # Errors
    /// Rejects foreign, missing, or nonmember targets, a target inside the
    /// moved tab, a target too small or a parent too full to take the
    /// layout, and exhausted identities.
    pub fn move_tab_to_pane(
        &mut self,
        workspace: WorkspaceId,
        tab: TabId,
        to: Beside,
    ) -> Result<LayoutChange, MuxError> {
        self.select_workspace_tab(workspace, tab)?;
        let target = to.target;
        self.select_workspace_pane(target.workspace, target.tab, target.pane)?;
        if target.tab == tab {
            return Err(MuxError::InvalidLayout(LayoutError::SelfTarget));
        }
        let source = self.tab_record(workspace, tab)?.clone();
        let mut node = source.layout.root().clone();
        if let PaneNode::Pane(pane) = &mut node {
            take_tab_name(pane, &source);
        }
        let (layout, allocates) = self.inserted(target, to.side, node)?;
        self.preflight(allocates)?;
        let terminals = source.terminals();
        self.revoke_if_session_changes(workspace, target.workspace, &terminals);
        let before = vec![self.snapshot(tab), self.snapshot(target.tab)];
        self.allocate_if(allocates)?;
        self.changed();
        self.remove_tab_record(workspace, tab)?;
        let record = self.tab_record_mut(target.workspace, target.tab)?;
        record.layout = layout.clone();
        record.zoomed = None;
        self.emit_hierarchy(|_| HierarchyEvent::PanesRearranged {
            changes: vec![
                TabChange::Removed { tab },
                TabChange::Layout {
                    tab: target.tab,
                    layout,
                    zoomed: None,
                },
            ],
        });
        Ok(LayoutChange {
            before,
            after: vec![self.snapshot(tab), self.snapshot(target.tab)],
            placed: Vec::new(),
        })
    }

    fn move_within_tab(
        &mut self,
        from: PaneRef,
        to: Beside,
    ) -> Result<LayoutChange, MuxError> {
        if to.target.pane == from.pane {
            return Ok(LayoutChange::default());
        }
        let current = self.tab_record(from.workspace, from.tab)?.layout.clone();
        let pane = moved_pane(&current, from.pane)?;
        // The target is another pane of the tab, so the source is not its
        // last pane.
        let remaining = current
            .without_pane(from.pane)
            .map_err(|error| layout_error(error, from.pane))?;
        let (layout, allocates) = self.insert_into(
            &remaining,
            to.target.pane,
            to.side,
            PaneNode::Pane(pane),
        )?;
        if layout.same_shape(&current) {
            return Ok(LayoutChange::default());
        }
        self.preflight(allocates)?;
        let before = vec![self.snapshot(from.tab)];
        self.allocate_if(allocates)?;
        self.changed();
        let record = self.tab_record_mut(from.workspace, from.tab)?;
        record.layout = layout;
        record.zoomed = None;
        self.emit_layout(from.workspace, from.tab);
        Ok(LayoutChange {
            before,
            after: vec![self.snapshot(from.tab)],
            placed: Vec::new(),
        })
    }

    fn move_between_tabs(
        &mut self,
        from: PaneRef,
        to: Beside,
    ) -> Result<LayoutChange, MuxError> {
        let source = self.tab_record(from.workspace, from.tab)?.clone();
        let mut pane = moved_pane(source.layout(), from.pane)?;
        let remaining = match source.layout.without_pane(from.pane) {
            Ok(layout) => Some(layout),
            Err(LayoutError::LastPane) => None,
            Err(error) => return Err(layout_error(error, from.pane)),
        };
        if remaining.is_none() {
            take_tab_name(&mut pane, &source);
        }
        let terminal = pane.terminal_id;
        let (layout, allocates) =
            self.inserted(to.target, to.side, PaneNode::Pane(pane))?;
        self.preflight(allocates)?;
        self.revoke_if_session_changes(
            from.workspace,
            to.target.workspace,
            &[terminal],
        );
        let before =
            vec![self.snapshot(from.tab), self.snapshot(to.target.tab)];
        self.allocate_if(allocates)?;
        self.changed();
        let mut changes = Vec::with_capacity(2);
        if let Some(remaining) = remaining {
            let zoomed = kept_zoom(&remaining, source.zoomed);
            let record = self.tab_record_mut(from.workspace, from.tab)?;
            record.layout = remaining.clone();
            record.zoomed = zoomed;
            changes.push(TabChange::Layout {
                tab: from.tab,
                layout: remaining,
                zoomed,
            });
        } else {
            self.remove_tab_record(from.workspace, from.tab)?;
            changes.push(TabChange::Removed { tab: from.tab });
        }
        let record = self.tab_record_mut(to.target.workspace, to.target.tab)?;
        record.layout = layout.clone();
        record.zoomed = None;
        changes.push(TabChange::Layout {
            tab: to.target.tab,
            layout,
            zoomed: None,
        });
        self.emit_hierarchy(|_| HierarchyEvent::PanesRearranged { changes });
        Ok(LayoutChange {
            before,
            after: vec![self.snapshot(from.tab), self.snapshot(to.target.tab)],
            placed: Vec::new(),
        })
    }

    fn extract_pane(
        &mut self,
        from: PaneRef,
        workspace: WorkspaceId,
        before: Option<TabId>,
    ) -> Result<LayoutChange, MuxError> {
        self.select_workspace(workspace)?;
        if let Some(anchor) = before {
            self.validate_scope(anchor.runtime())?;
            self.tab_record(workspace, anchor)?;
        }
        let source = self.tab_record(from.workspace, from.tab)?.clone();
        if source.layout.pane_count() == 1 {
            return self.place_lone_pane(from, workspace, before);
        }
        let mut pane = moved_pane(source.layout(), from.pane)?;
        let remaining = source
            .layout
            .without_pane(from.pane)
            .map_err(|error| layout_error(error, from.pane))?;
        self.preflight(true)?;
        self.revoke_if_session_changes(
            from.workspace,
            workspace,
            &[pane.terminal_id],
        );
        let id = TabId::in_runtime(self.runtime_id, self.allocate()?);
        let snapshot_before = vec![
            self.snapshot(from.tab),
            TabSnapshot {
                tab: id,
                record: None,
            },
        ];
        let zoomed = kept_zoom(&remaining, source.zoomed);
        let record = self.tab_record_mut(from.workspace, from.tab)?;
        record.layout = remaining.clone();
        record.zoomed = zoomed;
        // The pane's name moves up to the tab it becomes.
        let opened = Tab {
            id,
            custom_name: pane.custom_name.take(),
            layout: PaneLayout::single(pane),
            zoomed: None,
        };
        let tabs = &mut self
            .workspaces
            .get_mut(&workspace)
            .ok_or(MuxError::UnknownWorkspace(workspace))?
            .tabs;
        let index = before
            .and_then(|anchor| tabs.iter().position(|tab| tab.id == anchor))
            .unwrap_or(tabs.len());
        tabs.insert(index, opened.clone());
        self.emit_hierarchy(|_| HierarchyEvent::PanesRearranged {
            changes: vec![
                TabChange::Opened {
                    workspace,
                    tab: opened.info(),
                    index: position(index),
                },
                TabChange::Layout {
                    tab: from.tab,
                    layout: remaining,
                    zoomed,
                },
            ],
        });
        Ok(LayoutChange {
            before: snapshot_before,
            after: vec![self.snapshot(from.tab), self.snapshot(id)],
            placed: Vec::new(),
        })
    }

    /// Extracting a tab's only pane: a no-op within its workspace, and a
    /// move of the whole tab into another one.
    fn place_lone_pane(
        &mut self,
        from: PaneRef,
        workspace: WorkspaceId,
        before: Option<TabId>,
    ) -> Result<LayoutChange, MuxError> {
        if workspace == from.workspace {
            return Ok(LayoutChange::default());
        }
        let terminals = self.tab_record(from.workspace, from.tab)?.terminals();
        self.revoke_if_session_changes(from.workspace, workspace, &terminals);
        let snapshot_before = vec![self.snapshot(from.tab)];
        self.changed();
        let record = self.remove_tab_record(from.workspace, from.tab)?;
        let tabs = &mut self
            .workspaces
            .get_mut(&workspace)
            .ok_or(MuxError::UnknownWorkspace(workspace))?
            .tabs;
        let index = before
            .and_then(|anchor| tabs.iter().position(|tab| tab.id == anchor))
            .unwrap_or(tabs.len());
        tabs.insert(index, record);
        self.emit_hierarchy(|_| HierarchyEvent::PanesRearranged {
            changes: vec![TabChange::Placed {
                tab: from.tab,
                workspace,
                index: position(index),
            }],
        });
        Ok(LayoutChange {
            before: snapshot_before,
            after: vec![self.snapshot(from.tab)],
            placed: vec![from.tab],
        })
    }

    /// The target tab's layout with `node` inserted beside the target pane,
    /// and whether that needed a new split identity.
    fn inserted(
        &self,
        target: PaneRef,
        side: SplitDirection,
        node: PaneNode,
    ) -> Result<(PaneLayout, bool), MuxError> {
        let layout = &self.tab_record(target.workspace, target.tab)?.layout;
        self.insert_into(layout, target.pane, side, node)
    }

    /// `layout` with `node` inserted beside `target`, and whether that
    /// needed a new split identity. A new split takes the identity the next
    /// allocation returns, so a caller that commits allocates it and the
    /// layout is already correct. An insertion that forms no split never
    /// reads the identity, so it works even with none left.
    fn insert_into(
        &self,
        layout: &PaneLayout,
        target: PaneId,
        side: SplitDirection,
        node: PaneNode,
    ) -> Result<(PaneLayout, bool), MuxError> {
        let allocates = layout
            .forms_split(target, side)
            .map_err(|error| layout_error(error, target))?;
        let next = if allocates {
            self.next_id.checked_add(1).ok_or(MuxError::IdExhausted)?
        } else {
            self.next_id
        };
        let split = SplitId::in_runtime(self.runtime_id, next);
        let layout = layout
            .with_inserted(target, side, node, split)
            .map_err(|error| layout_error(error, target))?;
        Ok((layout, allocates))
    }

    /// Refuses before any change when a needed identity cannot be
    /// allocated. `allocate` advances the revision before it detects
    /// exhaustion, so a refused move must not reach it.
    fn preflight(&self, allocates: bool) -> Result<(), MuxError> {
        if allocates && self.next_id.checked_add(1).is_none() {
            return Err(MuxError::IdExhausted);
        }
        Ok(())
    }

    fn allocate_if(&mut self, allocates: bool) -> Result<(), MuxError> {
        if allocates {
            self.allocate()?;
        }
        Ok(())
    }

    /// Revokes the viewers of `terminals` when they move from one session
    /// to another, as a cross-session tab move does.
    fn revoke_if_session_changes(
        &mut self,
        source: WorkspaceId,
        destination: WorkspaceId,
        terminals: &[huterm_protocol::TerminalId],
    ) {
        if self.workspace_session(source) != self.workspace_session(destination)
        {
            for terminal in terminals {
                self.invalidate_terminal_authority(*terminal);
            }
        }
    }

    fn workspace_session(&self, workspace: WorkspaceId) -> Option<SessionId> {
        self.workspaces.get(&workspace).map(|w| w.session_id)
    }

    /// Removes a tab from its workspace without closing its terminals.
    pub(super) fn remove_tab_record(
        &mut self,
        workspace: WorkspaceId,
        tab: TabId,
    ) -> Result<Tab, MuxError> {
        let tabs = &mut self
            .workspaces
            .get_mut(&workspace)
            .ok_or(MuxError::UnknownWorkspace(workspace))?
            .tabs;
        let index = tabs
            .iter()
            .position(|t| t.id == tab)
            .ok_or(MuxError::UnknownTab(tab))?;
        Ok(tabs.remove(index))
    }

    /// The tab's current record, or its absence.
    pub(super) fn snapshot(&self, tab: TabId) -> TabSnapshot {
        let record = self.workspaces.values().find_map(|workspace| {
            let index = workspace.tabs.iter().position(|t| t.id == tab)?;
            let record = &workspace.tabs[index];
            Some(TabRecord {
                session: workspace.session_id,
                workspace: workspace.id,
                index: position(index),
                custom_name: record.custom_name.clone(),
                layout: record.layout.clone(),
                zoomed: record.zoomed,
            })
        });
        TabSnapshot { tab, record }
    }
}

/// The moving pane's record.
fn moved_pane(layout: &PaneLayout, pane: PaneId) -> Result<PaneInfo, MuxError> {
    layout
        .pane(pane)
        .cloned()
        .ok_or(MuxError::UnknownPane(pane))
}

/// Gives the only pane of a tab that is about to be removed the tab's
/// custom name; without one, the pane keeps its own.
fn take_tab_name(pane: &mut PaneInfo, tab: &Tab) {
    if let Some(name) = tab.custom_name() {
        pane.custom_name = Some(name.to_owned());
    }
}

/// A source tab's shared zoom once a pane left it: kept while its pane
/// remains and the tab still has more than one pane.
fn kept_zoom(layout: &PaneLayout, zoomed: Option<PaneId>) -> Option<PaneId> {
    zoomed
        .filter(|pane| layout.pane_count() > 1 && layout.pane(*pane).is_some())
}

#[cfg(test)]
mod tests;
