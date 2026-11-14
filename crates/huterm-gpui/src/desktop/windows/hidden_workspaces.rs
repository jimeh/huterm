//! A window holds views for every workspace of its attached session, not
//! only the one it shows, so showing another workspace is a visibility
//! change, as showing another tab is.
//!
//! `WorkspaceView::tabs` holds the shown workspace's views, aligned with
//! the window model's entries; `hidden` holds the others' by workspace,
//! with no model entries: a hidden workspace's active tab, history, and
//! focused panes live in its `NavigationMemory`. Hidden views stay
//! invisible, so they take no snapshots, draw nothing, publish no
//! accessibility tree, and never take focus, but their activity drains
//! run as a hidden tab's do: bells mark their tabs, program notifications
//! and failures reach the window, and exits record exit-closes.
//!
//! Reconcile keeps each hidden workspace in line with the projection
//! through the same rules as the shown one: `with_hidden` puts a hidden
//! workspace's views in `tabs` and names it in `scope`, so the view-level
//! operations apply to it while the model writes stay with the shown
//! workspace. A tab that moves between two workspaces the window holds
//! keeps its view (`adopt_moved_tabs`), so a later drop onto a hidden
//! workspace needs no install, and so does a pane (`adopt_moved_panes`).
//!
//! A workspace switch within the session (`show_workspace`) runs in one
//! update: the shown views move to `hidden`, the target's come out, the
//! model takes the target's entries and memory, and the remembered tab
//! shows. Tabs whose views are still installing join as they arrive, and
//! the model keeps their remembered navigation pending until then
//! (`PendingFocus`), so a departure or a capture meanwhile keeps it too. A
//! session switch still replaces every view.

use super::model::NavigationMemory;
use super::{
    App, Arc, Context, Desktop, EmptiedClose, Entity, Ordering, PaneId,
    SessionId, TabEntry, TabId, TabView, TerminalView, Touched, Window,
    WindowReconcile, WorkspaceId, WorkspaceView, mark_titles_changed,
    reconcile_window, terminating,
};

impl WorkspaceView {
    /// The workspace the view-level operations work on: the hidden one
    /// `with_hidden` names, else the shown one.
    pub(super) fn view_workspace(&self, cx: &App) -> Option<WorkspaceId> {
        self.scope.or_else(|| self.workspace_id(cx))
    }

    /// Whether model entries follow the views in `tabs`: false while they
    /// belong to a hidden workspace.
    pub(super) fn tabs_shown(&self) -> bool {
        self.scope.is_none()
    }

    /// Every tab view this window holds, shown and hidden.
    pub(super) fn all_tab_views(&self) -> impl Iterator<Item = &TabView> {
        self.tabs.iter().chain(self.hidden.values().flatten())
    }

    /// The hidden workspace holding `tab`'s view, if a hidden one does.
    pub(super) fn hidden_workspace_of(
        &self,
        tab: TabId,
    ) -> Option<WorkspaceId> {
        self.hidden
            .iter()
            .find(|(_, tabs)| tabs.iter().any(|view| view.id == tab))
            .map(|(workspace, _)| *workspace)
    }

    /// The tab whose view holds `pane`, shown or hidden.
    pub(super) fn tab_holding(&self, pane: PaneId) -> Option<TabId> {
        self.all_tab_views()
            .find(|view| view.pane(pane).is_some())
            .map(|view| view.id)
    }

    /// Runs `f` with `workspace`'s hidden views in `tabs` and `scope`
    /// naming it, so view-level operations, reconcile and installs, apply
    /// to it; the shown views return afterwards. Creates the workspace's
    /// entry when it has none.
    pub(super) fn with_hidden<R>(
        &mut self,
        workspace: WorkspaceId,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        debug_assert!(self.scope.is_none(), "with_hidden does not nest");
        let views = self.hidden.remove(&workspace).unwrap_or_default();
        let shown = std::mem::replace(&mut self.tabs, views);
        self.scope = Some(workspace);
        let result = f(self);
        self.scope = None;
        let views = std::mem::replace(&mut self.tabs, shown);
        self.hidden.insert(workspace, views);
        result
    }

    /// The workspaces of the attached session other than the shown one,
    /// in session order: those whose views the window keeps hidden.
    fn retained_workspaces(&self, cx: &App) -> Vec<WorkspaceId> {
        let shown = self.workspace_id(cx);
        self.attached_session(cx)
            .and_then(|session| {
                cx.global::<Desktop>()
                    .hierarchy
                    .state()
                    .session_workspaces(session)
            })
            .unwrap_or_default()
            .iter()
            .copied()
            .filter(|workspace| Some(*workspace) != shown)
            .collect()
    }

    /// Whether reconcile defers removals, adoptions, and installs: the
    /// window's own structural operation is in flight, except a transfer
    /// that installs or a switch that installs.
    pub(super) fn reconcile_busy(&self) -> bool {
        self.busy
            && !self
                .transfer
                .as_ref()
                .is_some_and(|pending| pending.installing)
            && !self.switch_installing()
    }

    /// Brings the hidden workspaces in line with the projection: drops the
    /// views of workspaces that left the attached session, unless the
    /// window is busy, makes room for those that joined, and reconciles
    /// each as the shown one is, which installs their views.
    pub(super) fn reconcile_hidden(
        &mut self,
        touched: &Touched,
        terminating: bool,
        cx: &mut Context<'_, Self>,
    ) {
        if terminating {
            return;
        }
        let retained = self.retained_workspaces(cx);
        if !self.reconcile_busy() {
            self.hidden
                .retain(|workspace, _| retained.contains(workspace));
        }
        for workspace in retained {
            self.with_hidden(workspace, |view| {
                reconcile_window(
                    &mut WindowReconcile { view, cx },
                    touched,
                    terminating,
                );
            });
        }
    }

    /// Moves the views of tabs that moved between two workspaces this
    /// window holds into the workspace that holds them now, once the
    /// projection caught up with each view, keeping each view's viewer,
    /// scroll, and selection. A tab leaving the shown workspace drops its
    /// model entry and hides; one joining it gains an entry, inactive.
    /// Never in a busy window, whose own completion reconciles once it
    /// ends. Returns whether the shown workspace's tabs changed.
    pub(super) fn adopt_moved_tabs(
        &mut self,
        terminating: bool,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if terminating || self.reconcile_busy() {
            return false;
        }
        let Some(shown) = self.workspace_id(cx) else {
            return false;
        };
        let state = cx.global::<Desktop>().hierarchy.state();
        let held = |workspace: WorkspaceId| {
            workspace == shown || self.hidden.contains_key(&workspace)
        };
        let moved: Vec<(TabId, WorkspaceId, WorkspaceId)> = self
            .tabs
            .iter()
            .map(|view| (shown, view))
            .chain(self.hidden.iter().flat_map(|(workspace, tabs)| {
                tabs.iter().map(move |view| (*workspace, view))
            }))
            .filter_map(|(from, view)| {
                let to = state.tab_workspace(view.id)?;
                (view.committed <= state.seq() && to != from && held(to))
                    .then_some((view.id, from, to))
            })
            .collect();
        let mut shown_changed = false;
        for (tab, from, to) in moved {
            let view = if from == shown {
                let Some(index) =
                    self.tabs.iter().position(|view| view.id == tab)
                else {
                    continue;
                };
                let view = self.tabs.remove(index);
                cx.global_mut::<Desktop>()
                    .windows
                    .close_tabs(self.window, &[tab]);
                mark_titles_changed(self.window, cx);
                hide_tab(&view, cx);
                shown_changed = true;
                view
            } else {
                let Some(tabs) = self.hidden.get_mut(&from) else {
                    continue;
                };
                let Some(index) = tabs.iter().position(|view| view.id == tab)
                else {
                    continue;
                };
                tabs.remove(index)
            };
            if to == shown {
                self.push_tab_view(view, false, cx);
                shown_changed = true;
            } else {
                self.hidden.entry(to).or_default().push(view);
            }
        }
        if shown_changed {
            self.debug_assert_tabs_aligned(cx);
            self.sync_tab_order(cx);
            // A shown workspace that had no tab activates the first that
            // arrived, as an install does (`show_pushed_panes`); the
            // reconcile caller sees the new active tab and shows it.
            if self.active_tab(cx).is_none()
                && let Some(first) = self.tabs.first().map(|view| view.id)
            {
                cx.global_mut::<Desktop>()
                    .windows
                    .select(self.window, first);
            }
        }
        shown_changed
    }

    /// Runs `f` on `workspace`'s views: the shown ones in place, a hidden
    /// workspace's through `with_hidden`.
    fn in_workspace<R>(
        &mut self,
        shown: WorkspaceId,
        workspace: WorkspaceId,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        if workspace == shown {
            f(self)
        } else {
            self.with_hidden(workspace, f)
        }
    }

    /// Moves the views of panes that moved into a tab of another workspace
    /// this window holds, once the projection caught up with each view,
    /// keeping each view's viewer, scroll, and selection, as pane adoption
    /// within a workspace does. A tab left without a pane view goes; the
    /// tab the pane joins gets a view when the window had none. A pane
    /// leaving the shown workspace hides. Never in a busy window, whose own
    /// completion reconciles once it ends. Returns whether the shown
    /// workspace's tabs changed.
    pub(super) fn adopt_moved_panes(
        &mut self,
        terminating: bool,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if terminating || self.reconcile_busy() {
            return false;
        }
        let Some(shown) = self.workspace_id(cx) else {
            return false;
        };
        let state = cx.global::<Desktop>().hierarchy.state();
        let held = |workspace: WorkspaceId| {
            workspace == shown || self.hidden.contains_key(&workspace)
        };
        let moved: Vec<(PaneId, WorkspaceId, TabId, WorkspaceId, TabId)> = self
            .tabs
            .iter()
            .map(|view| (shown, view))
            .chain(self.hidden.iter().flat_map(|(workspace, tabs)| {
                tabs.iter().map(move |view| (*workspace, view))
            }))
            .flat_map(|(from, view)| {
                view.panes.iter().map(move |pane| (from, view.id, pane))
            })
            .filter_map(|(from, tab, pane)| {
                let into = state.pane_tab(pane.pane)?;
                let to = state.tab_workspace(into)?;
                (pane.committed <= state.seq() && to != from && held(to))
                    .then_some((pane.pane, from, tab, to, into))
            })
            .collect();
        let mut shown_changed = false;
        for (pane, from, tab, to, into) in moved {
            let Some(view) = self.in_workspace(shown, from, |view| {
                // The tab's panes take the projected order first, as
                // reconcile orders them before adoption, so focus falls
                // back along the current layout.
                if let Some(source) =
                    view.tabs.iter_mut().find(|view| view.id == tab)
                {
                    super::view_install::in_layout_order(
                        &mut source.panes,
                        tab,
                        cx,
                    );
                }
                let (moved, emptied) = view.take_pane_view(pane, tab, cx)?;
                if emptied {
                    view.drop_tab_views(&[tab], cx);
                }
                Some(moved)
            }) else {
                continue;
            };
            if from == shown {
                view.view.update(cx, |view, cx| {
                    if view.visible {
                        view.hide(cx);
                        cx.notify();
                    }
                });
            }
            shown_changed |= from == shown || to == shown;
            self.in_workspace(shown, to, |target| {
                target.put_pane_view(view, into, cx);
            });
        }
        if shown_changed {
            self.debug_assert_tabs_aligned(cx);
            self.sync_tab_order(cx);
            if self.active_tab(cx).is_none()
                && let Some(first) = self.tabs.first().map(|view| view.id)
            {
                cx.global_mut::<Desktop>()
                    .windows
                    .select(self.window, first);
            }
        }
        shown_changed
    }

    /// Moves the shown workspace's views into `hidden`, under `left`, and
    /// `target`'s out of it, hiding the views that leave, and gives the
    /// model the target's entries with `memory`'s activation and pane
    /// focus. Views of tabs still installing are not there yet; the switch
    /// applies their memory as they arrive.
    pub(super) fn swap_shown(
        &mut self,
        left: Option<WorkspaceId>,
        target: WorkspaceId,
        memory: Option<&NavigationMemory>,
        cx: &mut Context<'_, Self>,
    ) {
        let views = self.hidden.remove(&target).unwrap_or_default();
        let leaving = std::mem::replace(&mut self.tabs, views);
        for tab in &leaving {
            hide_tab(tab, cx);
        }
        if let Some(left) = left.filter(|left| *left != target) {
            self.hidden.insert(left, leaving);
        }
        if let Some(memory) = memory {
            for (tab, pane) in &memory.panes {
                if let Some(view) =
                    self.tabs.iter_mut().find(|view| view.id == *tab)
                {
                    view.focus_pane(*pane);
                }
            }
        }
        let entries = self
            .tabs
            .iter()
            .map(|tab| TabEntry {
                id: tab.id,
                panes: tab.pane_titles(self.config.tabs, cx),
                title: tab.title(self.config.tabs, cx),
                focused: Some(tab.focused().pane),
            })
            .collect();
        cx.global_mut::<Desktop>().windows.show_tabs(
            self.window,
            entries,
            memory.and_then(|memory| memory.active),
            memory.map_or(&[], |memory| memory.history.as_slice()),
            memory.map_or(&[], |memory| memory.panes.as_slice()),
        );
        mark_titles_changed(self.window, cx);
        self.debug_assert_tabs_aligned(cx);
        self.sync_tab_order(cx);
    }

    /// Shows `workspace` of the attached session `session` in one update:
    /// marks the left workspace's bells seen, files its navigation memory,
    /// swaps the views, and selects the remembered tab, or shows the empty
    /// state. `recovers` says the target was chosen because it held tabs,
    /// so one that holds none any more owes the emptied close.
    pub(super) fn show_workspace(
        &mut self,
        session: SessionId,
        workspace: WorkspaceId,
        recovers: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let focus = !self.overlay_holds_focus();
        self.show_workspace_focused(
            session, workspace, recovers, focus, window, cx,
        );
    }

    /// [`Self::show_workspace`], moving keyboard focus to the shown tab
    /// only when `focus`, as another window's drag spring-loading a row
    /// must not.
    pub(super) fn show_workspace_focused(
        &mut self,
        session: SessionId,
        workspace: WorkspaceId,
        recovers: bool,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let left = self.workspace_id(cx);
        let desktop = cx.global_mut::<Desktop>();
        // Bells the left workspace rang while shown were seen there.
        desktop.windows.see_shown_bells(
            self.window,
            desktop.hierarchy.state(),
            &desktop.statuses,
        );
        let memory = desktop.windows.switch_to(self.window, session, workspace);
        desktop
            .windows
            .follow_bells(desktop.hierarchy.state(), &mut desktop.statuses);
        desktop.windows.prune_sessions(desktop.hierarchy.state());
        self.close.emptied = EmptiedClose::None;
        desktop.windows.set_emptied_close(self.window, false);
        self.restore_focus = None;
        self.swap_shown(left, workspace, memory.as_ref(), cx);
        self.layout_pending = true;
        self.reconcile_own(window, cx);
        self.measure_tab_widths(window, cx);
        if let Some(tab) = self.active_tab(cx) {
            self.select_tab(tab, focus, window, cx);
            self.reveal_tab_activity(window, cx);
        } else if self.attached_session_gone(cx)
            || self.shown_workspace_gone(cx)
            || (recovers && self.has_nothing_to_install(cx))
        {
            self.owe_emptied_close(cx);
        } else if focus && self.has_nothing_to_install(cx) {
            self.focus_empty_state(window, cx);
        }
        self.publish_pane_focus(cx);
        cx.notify();
    }

    /// Applies the remembered navigation of tabs that were still
    /// installing when a switch showed their workspace, as their views
    /// arrive: each one's focused pane, and the remembered active tab
    /// unless the user chose another meanwhile. What never arrives goes
    /// once the workspace's installs are idle.
    pub(super) fn apply_arriving(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let settled = self.installs.idle_in(self.workspace_id(cx));
        let tabs = &self.tabs;
        let (active, panes) = cx.global_mut::<Desktop>().windows.take_pending(
            self.window,
            |tab| tabs.iter().any(|view| view.id == tab),
            settled,
        );
        if active.is_none() && panes.is_empty() {
            return;
        }
        for (tab, pane) in panes {
            if let Some(view) = self.tabs.iter_mut().find(|view| view.id == tab)
            {
                view.focus_pane(pane);
            }
        }
        if let Some(active) = active {
            let focus = !self.overlay_holds_focus();
            self.measure_tab_widths(window, cx);
            self.select_tab(active, focus, window, cx);
        }
        self.publish_pane_focus(cx);
    }

    /// Ends a committed exit-close of a pane in hidden `workspace`: its
    /// views go, and a workspace the close emptied follows the rule an
    /// emptied shown workspace does, closing unless pinned or the
    /// session's last, decided under the Mux lock
    /// (`Mux::close_emptied_workspace`). Queued closes run next.
    pub(super) fn finish_hidden_close(
        &mut self,
        workspace: WorkspaceId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.reconcile_own(window, cx);
        self.close_emptied_hidden(workspace, cx);
        if !self.resume_close(window, cx) {
            cx.notify();
        }
    }

    /// Closes hidden `workspace` when it holds no tab, unless pinned or its
    /// session's last, decided under the Mux lock
    /// (`Mux::close_emptied_workspace`): an exit-close or a drag emptied it.
    pub(super) fn close_emptied_hidden(
        &self,
        workspace: WorkspaceId,
        cx: &mut Context<'_, Self>,
    ) {
        let emptied = cx
            .global::<Desktop>()
            .hierarchy
            .state()
            .workspace_tabs(workspace)
            .is_some_and(<[TabId]>::is_empty);
        // Scoped to the session that held the workspace when the move or
        // exit-close finished, so a later session switch does not change
        // the decision.
        if emptied
            && !terminating(cx)
            && let Some(session) = self.attached_session(cx)
        {
            let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
            cx.background_executor()
                .spawn(async move {
                    let mut mux = runtime.lock();
                    if runtime.terminating.load(Ordering::Acquire) {
                        return;
                    }
                    if let Err(error) =
                        mux.close_emptied_workspace(session, workspace)
                    {
                        eprintln!("Cannot close emptied workspace: {error}");
                    }
                })
                .detach();
        }
    }

    /// Every hidden view, for settings that reach views whether or not
    /// they show, such as a configuration reload.
    pub(super) fn hidden_views(&self) -> Vec<Entity<TerminalView>> {
        self.hidden
            .values()
            .flatten()
            .flat_map(|tab| tab.views().cloned())
            .collect()
    }
}

/// Hides every pane view of `tab` that shows.
fn hide_tab(tab: &TabView, cx: &mut App) {
    for pane in &tab.panes {
        pane.view.update(cx, |view, cx| {
            if view.visible {
                view.hide(cx);
                cx.notify();
            }
        });
    }
}
