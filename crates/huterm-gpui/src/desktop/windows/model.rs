//! Client-owned facts about this client's windows: what each window attaches
//! to and shows, its tab order and activation history, its quake profile, and
//! its restorable layout. Any context reads these through the `Desktop`
//! global, so no reader needs another window's view entity or handle, which is
//! unsafe while that window is on GPUI's update stack. Views keep entities and
//! ephemeral interaction state, such as focus, hover, selection, drag state,
//! tab-strip scrolling, and animation progress.
//!
//! Names and structure are not stored here: they come from the hierarchy
//! projection on `Desktop`, and reconcile republishes titles and tab order
//! into the model when the runtime changes them, such as after a rename.
//!
//! Each tab entry lists its panes' terminals and titles. The pane layout is
//! runtime state on the projection, and the window's pane focus lives in its
//! view until durable layout records (#42) need it.
//!
//! Not yet covered, for later extension: panel placement (#27) and durable
//! layout records (#42).

use super::layout_history::LayoutHistory;
use crate::config::TabPosition;
use gpui::{Pixels, WindowBounds, WindowId};
use huterm_protocol::{
    AttachmentId, HierarchyState, SessionId, TabId, TerminalId, WorkspaceId,
};

/// One pane of a tab as its window shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PaneTitle {
    pub(super) terminal: TerminalId,
    /// The pane's own title, for close dialog headings in a split tab.
    pub(super) title: String,
}

/// One tab as its window shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TabEntry {
    pub(super) id: TabId,
    /// The tab's panes, in the order the window installed them.
    pub(super) panes: Vec<PaneTitle>,
    /// The display title the tab bar shows, including the exited suffix.
    pub(super) title: String,
}

/// The quake profile a window serves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct QuakeRecord {
    pub(super) profile: String,
    /// The target of the show or hide transition, not whether the window is
    /// on screen yet.
    pub(super) visible: bool,
}

/// The geometry a restored window reopens with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct WindowLayout {
    pub(super) bounds: WindowBounds,
    pub(super) sidebar_width: Pixels,
}

/// Everything the client knows about one window.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct WindowRecord {
    pub(super) id: WindowId,
    pub(super) attachment: Option<AttachmentId>,
    pub(super) session: Option<SessionId>,
    pub(super) workspace: Option<WorkspaceId>,
    /// Tabs in display order.
    pub(super) tabs: Vec<TabEntry>,
    pub(super) active: Option<TabId>,
    /// Activated tabs, most recent first.
    pub(super) history: Vec<TabId>,
    pub(super) quake: Option<QuakeRecord>,
    pub(super) layout: WindowLayout,
    /// The window's own layout changes that undo and redo restore.
    pub(super) layout_history: LayoutHistory,
    /// The window is being removed. Its own view can still read the record,
    /// but profile rows, Quit dialog titles, and Quit capture skip it.
    pub(super) closing: bool,
}

impl WindowRecord {
    fn contains(&self, tab: TabId) -> bool {
        self.tabs.iter().any(|entry| entry.id == tab)
    }
}

/// Which windows' tab titles a close confirmation lists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TitleScope {
    /// One window, including one that is closing.
    Window(WindowId),
    /// Every window that is not closing, for Quit.
    Open,
}

// In-memory input for a future restore writer. No serialization/version contract.
#[derive(Clone, Debug)]
#[cfg_attr(test, derive(PartialEq))]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "retained restore aggregate awaits the persistence stage"
    )
)]
pub(super) struct WindowRestore {
    pub(super) attachment: AttachmentId,
    pub(super) workspace: Option<WorkspaceId>,
    pub(super) active: Option<TabId>,
    pub(super) bounds: WindowBounds,
    pub(super) tab_position: TabPosition,
    pub(super) sidebar_width: Pixels,
    pub(super) quake_profile: Option<String>,
}

/// A quake profile rename Settings is writing or wrote, until the reloads
/// that read the file after it apply it.
#[derive(Debug)]
struct QuakeRename {
    id: QuakeRenameId,
    from: String,
    to: String,
    /// The count of renames noted, this one included, once its save
    /// landed; `None` while it runs.
    noted: Option<u64>,
}

/// Names a rename whose save `quake_rename_started` saw start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QuakeRenameId(u64);

/// How a reload changed the quake associations.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct QuakeReconciled {
    /// Windows that took a renamed profile's new name.
    pub(super) renamed: Vec<(WindowId, String)>,
    /// Windows whose profile the file no longer has.
    pub(super) removed: Vec<WindowId>,
}

/// The client's windows, in the order they opened.
#[derive(Debug, Default)]
pub(super) struct WindowModel {
    records: Vec<WindowRecord>,
    renames: Vec<QuakeRename>,
    renames_noted: u64,
    renames_started: u64,
}

impl WindowModel {
    fn record_mut(&mut self, id: WindowId) -> Option<&mut WindowRecord> {
        self.records.iter_mut().find(|record| record.id == id)
    }

    fn open_records(&self) -> impl Iterator<Item = &WindowRecord> {
        self.records.iter().filter(|record| !record.closing)
    }

    /// Records a newly opened window, with the profile it serves if any.
    pub(super) fn open(
        &mut self,
        id: WindowId,
        quake: Option<QuakeRecord>,
        layout: WindowLayout,
    ) {
        self.records.retain(|record| record.id != id);
        self.records.push(WindowRecord {
            id,
            attachment: None,
            session: None,
            workspace: None,
            tabs: Vec::new(),
            active: None,
            history: Vec::new(),
            quake,
            layout,
            layout_history: LayoutHistory::default(),
            closing: false,
        });
    }

    /// Records the session and workspace of a window's first spawn, and the
    /// attachment when that spawn created one.
    pub(super) fn attach(
        &mut self,
        id: WindowId,
        attachment: Option<AttachmentId>,
        session: SessionId,
        workspace: WorkspaceId,
    ) {
        let Some(record) = self.record_mut(id) else {
            return;
        };
        if attachment.is_some() {
            record.attachment = attachment;
        }
        record.session = Some(session);
        record.workspace = Some(workspace);
    }

    /// Clears the attachment and session after the runtime detached the
    /// attachment or deleted its session.
    pub(super) fn detach_attachment(&mut self, id: WindowId) {
        if let Some(record) = self.record_mut(id) {
            record.attachment = None;
            record.session = None;
        }
    }

    /// Appends a spawned tab and activates it.
    pub(super) fn open_tab(&mut self, id: WindowId, entry: TabEntry) {
        let Some(record) = self.record_mut(id) else {
            return;
        };
        let tab = entry.id;
        record.tabs.retain(|existing| existing.id != tab);
        record.tabs.push(entry);
        record.active = Some(tab);
        record_tab_activation(&mut record.history, tab);
    }

    /// Appends a tab the window installed without spawning it; the active
    /// tab stays as it was.
    pub(super) fn add_tab(&mut self, id: WindowId, entry: TabEntry) {
        let Some(record) = self.record_mut(id) else {
            return;
        };
        record.tabs.retain(|existing| existing.id != entry.id);
        record.tabs.push(entry);
    }

    /// Activates `tab`; returns false when the window does not show it.
    pub(super) fn select(&mut self, id: WindowId, tab: TabId) -> bool {
        let Some(record) = self.record_mut(id) else {
            return false;
        };
        if !record.contains(tab) {
            return false;
        }
        record.active = Some(tab);
        record_tab_activation(&mut record.history, tab);
        true
    }

    /// Applies canonical tab order; returns false and changes nothing when
    /// `order` is not a permutation of the window's tabs.
    pub(super) fn apply_order(
        &mut self,
        id: WindowId,
        order: &[TabId],
    ) -> bool {
        self.record_mut(id).is_some_and(|record| {
            apply_tab_order(&mut record.tabs, order, |entry| entry.id)
        })
    }

    /// Removes closed tabs in order, moving the active tab as the tab bar
    /// does, and forgets their activations. Returns the new active tab.
    pub(super) fn close_tabs(
        &mut self,
        id: WindowId,
        tabs: &[TabId],
    ) -> Option<TabId> {
        let record = self.record_mut(id)?;
        for &tab in tabs {
            remove_tab(&mut record.tabs, &mut record.active, tab, |entry| {
                entry.id
            });
            prune_tab_history(&mut record.history, tab);
        }
        record.active
    }

    /// Publishes a tab's display title; returns whether it changed.
    pub(super) fn set_tab_title(
        &mut self,
        id: WindowId,
        tab: TabId,
        title: &str,
    ) -> bool {
        let Some(entry) = self.record_mut(id).and_then(|record| {
            record.tabs.iter_mut().find(|entry| entry.id == tab)
        }) else {
            return false;
        };
        if entry.title == title {
            return false;
        }
        title.clone_into(&mut entry.title);
        true
    }

    /// Publishes a tab's pane titles; returns whether they changed.
    pub(super) fn set_pane_titles(
        &mut self,
        id: WindowId,
        tab: TabId,
        panes: Vec<PaneTitle>,
    ) -> bool {
        let Some(entry) = self.record_mut(id).and_then(|record| {
            record.tabs.iter_mut().find(|entry| entry.id == tab)
        }) else {
            return false;
        };
        if entry.panes == panes {
            return false;
        }
        entry.panes = panes;
        true
    }

    /// Publishes a quake window's desired visibility.
    pub(super) fn set_quake_visible(&mut self, id: WindowId, visible: bool) {
        if let Some(quake) =
            self.record_mut(id).and_then(|record| record.quake.as_mut())
        {
            quake.visible = visible;
        }
    }

    /// Notes that Settings started saving a rename of quake profile `from`
    /// to `to`. The window serving `from` takes the new name in the first
    /// reload that reads the file with it (`reconcile_quake`), so a reload
    /// that read the file before the rename does not take the window for a
    /// removed profile's. The save can replace the file before it settles,
    /// so a reload may read the rename while it still runs.
    pub(super) fn quake_rename_started(
        &mut self,
        from: &str,
        to: &str,
    ) -> QuakeRenameId {
        self.renames_started += 1;
        let id = QuakeRenameId(self.renames_started);
        self.renames.push(QuakeRename {
            id,
            from: from.to_owned(),
            to: to.to_owned(),
            noted: None,
        });
        id
    }

    /// Settles rename `id`: noted when its save `landed` in the file,
    /// forgotten when it wrote nothing.
    pub(super) fn quake_rename_settled(
        &mut self,
        id: QuakeRenameId,
        landed: bool,
    ) {
        if landed {
            self.renames_noted += 1;
            let noted = self.renames_noted;
            if let Some(rename) =
                self.renames.iter_mut().find(|rename| rename.id == id)
            {
                rename.noted = Some(noted);
            }
        } else {
            self.renames.retain(|rename| rename.id != id);
        }
    }

    /// How many renames have been noted, which a reload takes before it
    /// reads the file: each rename noted by then was already written.
    pub(super) fn quake_renames_noted(&self) -> u64 {
        self.renames_noted
    }

    /// Whether a window keeps serving profile `name`: one serves it, and no
    /// noted rename moves it on. Settings renames no profile to such a
    /// name, which a removed profile's window keeps until its detach ends.
    pub(super) fn quake_name_held(&self, name: &str) -> bool {
        self.quake_window(name).is_some()
            && !self
                .renames
                .iter()
                .any(|rename| rename.noted.is_some() && rename.from == name)
    }

    /// Brings the quake associations up to a reload whose file has the
    /// profiles `has` names and was read once `read` renames were noted. A
    /// window whose profile the file lacks follows the renames noted or
    /// running for it to the first name the file has, unless another
    /// window serves that name. Renames noted by `read` go, since the file
    /// read shows them, and the windows whose profile the file still lacks
    /// are returned to detach.
    pub(super) fn reconcile_quake(
        &mut self,
        read: u64,
        has: impl Fn(&str) -> bool,
    ) -> QuakeReconciled {
        let moves: Vec<(WindowId, String)> = self
            .quake_windows()
            .filter_map(|(profile, id)| {
                let mut name = profile;
                let mut renames = self.renames.iter();
                while !has(name) {
                    name =
                        renames.find(|rename| rename.from == name)?.to.as_str();
                }
                (name != profile).then(|| (id, name.to_owned()))
            })
            .collect();
        let mut renamed = Vec::new();
        for (id, name) in moves {
            if self.quake_window(&name).is_some() {
                continue;
            }
            if let Some(quake) =
                self.record_mut(id).and_then(|record| record.quake.as_mut())
            {
                name.clone_into(&mut quake.profile);
                renamed.push((id, name));
            }
        }
        self.renames
            .retain(|rename| rename.noted.is_none_or(|noted| noted > read));
        let removed = self
            .quake_windows()
            .filter(|(profile, _)| !has(profile))
            .map(|(_, id)| id)
            .collect();
        QuakeReconciled { renamed, removed }
    }

    /// Ends a window's profile association; the window stays open.
    pub(super) fn detach_quake(&mut self, id: WindowId) {
        if let Some(record) = self.record_mut(id) {
            record.quake = None;
        }
    }

    pub(super) fn set_layout_bounds(
        &mut self,
        id: WindowId,
        bounds: WindowBounds,
    ) {
        if let Some(record) = self.record_mut(id) {
            record.layout.bounds = bounds;
        }
    }

    pub(super) fn set_sidebar_width(&mut self, id: WindowId, width: Pixels) {
        if let Some(record) = self.record_mut(id) {
            record.layout.sidebar_width = width;
        }
    }

    /// Marks a window as being removed and ends its profile association.
    pub(super) fn begin_close(&mut self, id: WindowId) {
        if let Some(record) = self.record_mut(id) {
            record.closing = true;
            record.quake = None;
        }
    }

    /// Forgets a window GPUI has removed.
    pub(super) fn remove(&mut self, id: WindowId) {
        self.records.retain(|record| record.id != id);
    }

    /// A window's layout history.
    pub(super) fn layout_history_mut(
        &mut self,
        id: WindowId,
    ) -> Option<&mut LayoutHistory> {
        self.record_mut(id).map(|record| &mut record.layout_history)
    }

    /// Drops the history entries the projection no longer lets restore,
    /// in every window, including windows that do not show their tabs, and
    /// returns the windows that lost one.
    pub(super) fn prune_layout_histories(
        &mut self,
        state: &HierarchyState,
    ) -> Vec<WindowId> {
        self.records
            .iter_mut()
            .filter_map(|record| {
                record.layout_history.prune(state).then_some(record.id)
            })
            .collect()
    }

    pub(super) fn record(&self, id: WindowId) -> Option<&WindowRecord> {
        self.records.iter().find(|record| record.id == id)
    }

    /// Every window, including closing ones, in opening order.
    pub(super) fn records(&self) -> impl Iterator<Item = &WindowRecord> {
        self.records.iter()
    }

    /// Every window's published tab titles.
    pub(super) fn published_titles(
        &self,
    ) -> impl Iterator<Item = (TabId, &str)> {
        self.records.iter().flat_map(|record| {
            record
                .tabs
                .iter()
                .map(|entry| (entry.id, entry.title.as_str()))
        })
    }

    /// Every pane title an open window published, by terminal.
    pub(super) fn published_pane_titles(
        &self,
    ) -> impl Iterator<Item = (TerminalId, &str)> {
        self.records.iter().flat_map(|record| {
            record.tabs.iter().flat_map(|entry| {
                entry
                    .panes
                    .iter()
                    .map(|pane| (pane.terminal, pane.title.as_str()))
            })
        })
    }

    /// The open window serving quake profile `name`.
    pub(super) fn quake_window(&self, name: &str) -> Option<WindowId> {
        self.open_records()
            .find(|record| {
                record
                    .quake
                    .as_ref()
                    .is_some_and(|quake| quake.profile == name)
            })
            .map(|record| record.id)
    }

    /// Every open quake window with the profile it serves.
    pub(super) fn quake_windows(
        &self,
    ) -> impl Iterator<Item = (&str, WindowId)> + '_ {
        self.open_records().filter_map(|record| {
            record
                .quake
                .as_ref()
                .map(|quake| (quake.profile.as_str(), record.id))
        })
    }

    /// Profile `name`'s desired visibility and tab count, when a window
    /// serves it.
    pub(super) fn quake_state(&self, name: &str) -> Option<(bool, usize)> {
        let record = self.record(self.quake_window(name)?)?;
        let quake = record.quake.as_ref()?;
        Some((quake.visible, record.tabs.len()))
    }

    /// Tabs in most-recently-used order with the active tab last, so a
    /// picker's first row is the tab the user most likely wants next.
    pub(super) fn palette_tab_order(&self, id: WindowId) -> Vec<TabId> {
        let Some(record) = self.record(id) else {
            return Vec::new();
        };
        let mut order: Vec<TabId> = record
            .history
            .iter()
            .copied()
            .filter(|tab| Some(*tab) != record.active)
            .collect();
        for entry in &record.tabs {
            if !order.contains(&entry.id) && Some(entry.id) != record.active {
                order.push(entry.id);
            }
        }
        order.extend(record.active);
        order
    }

    /// The most recently activated tab other than the active one.
    pub(super) fn recent_tab(&self, id: WindowId) -> Option<TabId> {
        let record = self.record(id)?;
        record
            .history
            .iter()
            .copied()
            .find(|tab| Some(*tab) != record.active && record.contains(*tab))
    }

    /// Tab entries a close confirmation maps busy terminals to.
    pub(super) fn tab_titles(&self, scope: TitleScope) -> Vec<&TabEntry> {
        match scope {
            TitleScope::Window(id) => self
                .record(id)
                .map(|record| record.tabs.iter().collect())
                .unwrap_or_default(),
            TitleScope::Open => self
                .open_records()
                .flat_map(|record| record.tabs.iter())
                .collect(),
        }
    }

    /// The Quit capture: every open, attached window in opening order.
    pub(super) fn restore_windows(
        &self,
        tab_position: TabPosition,
    ) -> Vec<WindowRestore> {
        self.open_records()
            .filter_map(|record| {
                Some(WindowRestore {
                    attachment: record.attachment?,
                    workspace: record.workspace,
                    active: record.active,
                    bounds: record.layout.bounds,
                    tab_position,
                    sidebar_width: record.layout.sidebar_width,
                    quake_profile: record
                        .quake
                        .as_ref()
                        .map(|quake| quake.profile.clone()),
                })
            })
            .collect()
    }
}

/// Sorts `tabs` into canonical `order`; returns false and leaves `tabs`
/// unchanged when `order` is not a permutation of them.
pub(super) fn apply_tab_order<T>(
    tabs: &mut [T],
    order: &[TabId],
    id: impl Fn(&T) -> TabId,
) -> bool {
    if tabs.len() != order.len()
        || order
            .iter()
            .enumerate()
            .any(|(index, tab)| order[..index].contains(tab))
        || tabs.iter().any(|tab| !order.contains(&id(tab)))
    {
        return false;
    }
    tabs.sort_by_key(|tab| {
        order
            .iter()
            .position(|candidate| *candidate == id(tab))
            .unwrap_or(usize::MAX)
    });
    true
}

// Update navigation together with removal, before another close can interrupt
// completion or open a confirmation dialog.
pub(super) fn remove_tab<T>(
    tabs: &mut Vec<T>,
    active: &mut Option<TabId>,
    closed: TabId,
    id: impl Fn(&T) -> TabId,
) {
    let Some(index) = tabs.iter().position(|tab| id(tab) == closed) else {
        return;
    };
    tabs.remove(index);
    if *active == Some(closed) {
        *active = tabs.get(index.min(tabs.len().saturating_sub(1))).map(id);
    }
}

pub(super) fn record_tab_activation(history: &mut Vec<TabId>, id: TabId) {
    history.retain(|recorded| *recorded != id);
    history.insert(0, id);
}

pub(super) fn prune_tab_history(history: &mut Vec<TabId>, id: TabId) {
    history.retain(|recorded| *recorded != id);
}

#[cfg(test)]
mod tests;
