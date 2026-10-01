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
//! Not yet covered, for later extension: pane layouts (#28), panel placement
//! (#27), and durable layout records (#42).

use crate::config::TabPosition;
use gpui::{Pixels, WindowBounds, WindowId};
use huterm_protocol::{
    AttachmentId, SessionId, TabId, TerminalId, WorkspaceId,
};

/// One tab as its window shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TabEntry {
    pub(super) id: TabId,
    pub(super) terminal: TerminalId,
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

/// The client's windows, in the order they opened.
#[derive(Debug, Default)]
pub(super) struct WindowModel {
    records: Vec<WindowRecord>,
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

    /// Publishes a quake window's desired visibility.
    pub(super) fn set_quake_visible(&mut self, id: WindowId, visible: bool) {
        if let Some(quake) =
            self.record_mut(id).and_then(|record| record.quake.as_mut())
        {
            quake.visible = visible;
        }
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
