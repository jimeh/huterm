//! Views for tabs and panes a window did not create: tabs another window
//! opened in a shared workspace, the tabs of a session the window attached
//! to, and panes another window split. Reconcile decides which tabs need
//! views (`projection::installs`, plus shown tabs with a pane that has
//! none); the window subscribes the missing panes' viewers on a worker,
//! waits for the projection to reach the sequence the worker read, and then
//! pushes the views that still apply. A view whose viewer was revoked while
//! its pane stays is replaced the same way (`projection::replacements`),
//! keeping the view and swapping its viewer. One tab has at most one
//! subscription in flight.

use super::*;

/// Whether an install failed only because its target went away, such as a
/// tab closed or moved to another session after reconcile started it.
fn stale_install(error: &MuxError) -> bool {
    matches!(
        error,
        MuxError::UnknownAttachment(_)
            | MuxError::TerminalNotInAttachment { .. }
            | MuxError::Runtime(RuntimeError::Stopped | RuntimeError::Revoked)
    )
}

/// The open window with `id`.
fn window_handle(cx: &App, id: gpui::WindowId) -> Option<AnyWindowHandle> {
    cx.windows()
        .into_iter()
        .find(|handle| handle.window_id() == id)
}

/// One viewer a subscription asks for: a pane of a tab, its terminal, the
/// state the viewer starts with, and whether it replaces a revoked viewer.
#[derive(Clone)]
pub(super) struct PaneTarget {
    pub(super) tab: TabId,
    pub(super) pane: PaneId,
    pub(super) terminal: TerminalId,
    pub(super) start: ViewerStart,
    replace: bool,
}

impl WorkspaceView {
    /// Subscribes viewers for the panes of `tabs` that have no view, which
    /// reconcile has recorded as in flight, and pushes their views once the
    /// projection has caught up. Closing the window drops the viewers with
    /// the completion.
    pub(super) fn install_tabs(
        &mut self,
        tabs: &[TabId],
        cx: &mut Context<'_, Self>,
    ) {
        self.subscribe_panes(tabs, cx);
    }

    /// Subscribes replacement viewers for the revoked pane views of `tabs`,
    /// which reconcile has recorded as in flight, each starting with its
    /// view's focus, geometry, and presentation, and swaps them in once the
    /// projection has caught up.
    pub(super) fn replace_tabs(
        &mut self,
        tabs: &[TabId],
        cx: &mut Context<'_, Self>,
    ) {
        self.subscribe_panes(tabs, cx);
    }

    /// The viewers `tab` needs now: one for each projected pane without a
    /// view, and one for each pane view whose viewer was revoked.
    fn pane_targets(&self, tab: TabId, cx: &App) -> Vec<PaneTarget> {
        let fresh =
            ViewerStart::fresh(terminal_presentation(&self.config.theme));
        let state = cx.global::<Desktop>().hierarchy.state();
        let Some(info) = state.tab(tab) else {
            return Vec::new();
        };
        let view = self.tabs.iter().find(|view| view.id == tab);
        info.layout
            .panes()
            .into_iter()
            .filter_map(|pane| {
                let target = |start, replace| PaneTarget {
                    tab,
                    pane: pane.id,
                    terminal: pane.terminal_id,
                    start,
                    replace,
                };
                match view.and_then(|view| view.pane(pane.id)) {
                    None => Some(target(fresh.clone(), false)),
                    Some(installed) => {
                        let terminal = installed.view.read(cx);
                        terminal
                            .revoked
                            .then(|| target(terminal.viewer_start(), true))
                    }
                }
            })
            .collect()
    }

    fn subscribe_panes(&mut self, tabs: &[TabId], cx: &mut Context<'_, Self>) {
        let attachment = self.attachment_id(cx);
        let mut targets = Vec::new();
        for &tab in tabs {
            let needed = self.pane_targets(tab, cx);
            if attachment.is_none() || needed.is_empty() {
                self.installs.finish(tab, false);
            } else {
                targets.extend(needed);
            }
        }
        let Some(attachment) = attachment.filter(|_| !targets.is_empty())
        else {
            return;
        };
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let clipboard_allowed =
            self.config.terminal.clipboard_write.is_allowed();
        let requested = targets.clone();
        let task = cx.background_executor().spawn(async move {
            #[cfg(all(test, feature = "test-support"))]
            runtime
                .worker_hooks
                .enter(worker_hooks::Stage::Subscribe)
                .await;
            runtime.subscribe_panes(attachment, &requested, clipboard_allowed)
        });
        let id = self.window;
        cx.spawn(async move |view, cx| {
            let Committed { result, seq } = task.await;
            let resolution = projected(cx, seq).await;
            let Some(handle) = cx.update(|cx| window_handle(cx, id)) else {
                return;
            };
            let _ = handle.update(cx, |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.finish_subscribe(
                        &targets, result, seq, resolution, window, cx,
                    );
                })
            });
        })
        .detach();
    }

    /// Applies each subscribed viewer while the projection still holds its
    /// tab in this window's workspace: it builds the tab's view, adds a
    /// pane's view, or swaps into a revoked view, and drops the viewers
    /// that no longer apply. A failed replacement removes its pane view.
    /// Selects a tab only when the window has no active tab, or when an
    /// undo or redo waits to focus a pane this install covers, and focuses
    /// it unless an overlay holds focus.
    fn finish_subscribe(
        &mut self,
        targets: &[PaneTarget],
        viewers: Vec<Result<TerminalViewer, MuxError>>,
        committed: u64,
        resolution: Resolution,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let current = resolution == Resolution::Ready && !terminating(cx);
        // Before any removal: a failed replacement may move the active tab
        // or its focused pane, which must then be shown and focused.
        let active = self.active_pane(cx);
        let mut pushed = false;
        let mut removed_any = false;
        let mut by_tab: Vec<(TabId, Vec<_>)> = Vec::new();
        for (target, result) in targets.iter().zip(viewers) {
            match by_tab.iter_mut().find(|(tab, _)| *tab == target.tab) {
                Some((_, entries)) => entries.push((target, result)),
                None => by_tab.push((target.tab, vec![(target, result)])),
            }
        }
        for (tab, entries) in by_tab {
            let held = current && self.shows_projected_tab(tab, cx);
            let (added, removed) = self
                .apply_tab_viewers(tab, entries, held, committed, window, cx);
            pushed |= added;
            removed_any |= removed;
        }
        if removed_any {
            self.measure_tab_widths(window, cx);
            cx.notify();
        }
        if let Some(next) = reactivation(active, self.active_pane(cx)) {
            let focus = !self.overlay_holds_focus();
            self.select_tab(next, focus, window, cx);
        }
        if pushed {
            self.show_pushed_panes(window, cx);
        } else if self.emptied_close_may_retry(cx) {
            self.owe_emptied_close(cx);
        }
        if let Some(pane) = self.restore_focus
            && targets.iter().any(|target| target.pane == pane)
        {
            self.restore_focus = None;
            self.focus_restored_pane(pane, window, cx);
        }
        // A pane split into a tab while its install was in flight was
        // excluded then; installs that failed stay excluded.
        if current {
            self.reconcile_own(window, cx);
        }
    }

    /// Selects a tab for a window that had none once views arrived, or
    /// shows the panes the active tab gained.
    fn show_pushed_panes(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.measure_tab_widths(window, cx);
        match (self.active_tab(cx), self.tabs.first()) {
            (None, Some(first)) => {
                let first = first.id;
                let focus = !self.overlay_holds_focus();
                self.select_tab(first, focus, window, cx);
            }
            // Hidden views still report their geometry, and a shown tab
            // that gained a pane shows it.
            (Some(active), _) => {
                self.sync_tab_layout(window, cx);
                let focus = !self.overlay_holds_focus();
                self.select_tab(active, focus, window, cx);
            }
            (None, None) => {}
        }
        cx.notify();
    }

    /// Applies one tab's subscribed viewers, as `finish_subscribe`
    /// describes. Reports whether it pushed a pane view, and whether a
    /// failed replacement removed the tab's last pane view.
    fn apply_tab_viewers(
        &mut self,
        tab: TabId,
        entries: Vec<(&PaneTarget, Result<TerminalViewer, MuxError>)>,
        held: bool,
        committed: u64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> (bool, bool) {
        let mut failed = false;
        let mut new_panes = Vec::new();
        let mut gone_panes = Vec::new();
        for (target, result) in entries {
            let applies = held && self.pane_still_applies(target, cx);
            match result {
                Ok(viewer) if applies && target.replace => {
                    self.replace_viewer(
                        tab,
                        target.pane,
                        viewer,
                        &target.start,
                        window,
                        cx,
                    );
                }
                Ok(viewer) if applies => {
                    let key = PaneKey {
                        tab,
                        pane: target.pane,
                        terminal: target.terminal,
                    };
                    if let Some(pane) =
                        self.pane_view(key, viewer, committed, window, cx)
                    {
                        new_panes.push(pane);
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    if applies {
                        failed = true;
                        if target.replace {
                            gone_panes.push(target.pane);
                        }
                        if !stale_install(&error) {
                            self.notify(
                                NoticeContent::command_failure(
                                    "Cannot show tab",
                                    format!("Cannot show tab: {error}"),
                                ),
                                cx,
                            );
                        }
                    }
                }
            }
        }
        let removed = !gone_panes.is_empty()
            && self.remove_pane_views(tab, &gone_panes, cx)
            && !self.tabs.iter().any(|view| view.id == tab);
        let pushed = !new_panes.is_empty()
            && self.add_pane_views(tab, new_panes, committed, cx);
        self.installs.finish(tab, failed);
        (pushed, removed)
    }

    /// Whether the projection holds `tab` in this window's workspace and
    /// attached session.
    fn shows_projected_tab(&self, tab: TabId, cx: &App) -> bool {
        projection::installs(
            cx.global::<Desktop>().hierarchy.state(),
            self.workspace_id(cx),
            self.attached_session(cx),
            &[],
            &[],
            false,
            false,
        )
        .contains(&tab)
    }

    /// Whether a subscribed viewer still has a place: its pane is in the
    /// tab's projected layout, and it replaces a view still revoked or adds
    /// a view no other subscription added meanwhile.
    fn pane_still_applies(&self, target: &PaneTarget, cx: &App) -> bool {
        let state = cx.global::<Desktop>().hierarchy.state();
        let in_layout = state
            .tab(target.tab)
            .is_some_and(|info| info.layout.pane(target.pane).is_some());
        let installed = self
            .tabs
            .iter()
            .find(|view| view.id == target.tab)
            .and_then(|view| view.pane(target.pane));
        in_layout
            && match installed {
                Some(pane) => target.replace && pane.view.read(cx).revoked,
                None => !target.replace,
            }
    }

    /// Adds new pane views to `tab`, building the tab's view when the
    /// window had none. Returns whether anything was pushed.
    pub(super) fn add_pane_views(
        &mut self,
        tab: TabId,
        panes: Vec<PaneView>,
        committed: u64,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if let Some(view) = self.tabs.iter_mut().find(|view| view.id == tab) {
            view.panes.extend(panes);
            in_layout_order(&mut view.panes, tab, cx);
            let titles = view.pane_titles(self.config.tabs, cx);
            if cx.global_mut::<Desktop>().windows.set_pane_titles(
                self.window,
                tab,
                titles,
            ) {
                mark_titles_changed(self.window, cx);
            }
            return true;
        }
        let names = cx
            .global::<Desktop>()
            .hierarchy
            .state()
            .tab(tab)
            .map(TabNames::from_info)
            .unwrap_or_default();
        let mut panes = panes;
        // The tab's first pane in traversal order takes focus.
        in_layout_order(&mut panes, tab, cx);
        self.push_tab_view(
            TabView::new(tab, committed, names, panes),
            false,
            cx,
        );
        true
    }

    /// Swaps `viewer` into the view of `pane` in `tab` and restarts the
    /// pane's activity drain on it; the old drain waits on the revoked
    /// viewer.
    fn replace_viewer(
        &mut self,
        tab: TabId,
        pane: PaneId,
        viewer: TerminalViewer,
        start: &ViewerStart,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(view) = self
            .tabs
            .iter()
            .find(|view| view.id == tab)
            .and_then(|view| view.pane(pane))
            .map(|pane| pane.view.clone())
        else {
            return;
        };
        let activity = viewer.wake_handle();
        let clipboard_allowed =
            self.config.terminal.clipboard_write.is_allowed();
        view.update(cx, |terminal, cx| {
            terminal.replace_viewer(viewer, start, clipboard_allowed, cx);
        });
        let failure_wakes = view.read(cx).failure_wakes.clone();
        let task = Self::pane_activity_task(
            tab,
            pane,
            activity,
            failure_wakes,
            window,
            cx,
        );
        if let Some(installed) = self
            .tabs
            .iter_mut()
            .find(|view| view.id == tab)
            .and_then(|view| view.pane_mut(pane))
        {
            installed.restart_activity(task);
        }
    }
}

/// Orders `panes` as `tab`'s projected layout traverses them, so focus falls
/// back to the first pane in traversal order and later panes paint above
/// earlier ones. Panes the layout no longer holds go last.
pub(super) fn in_layout_order(panes: &mut [PaneView], tab: TabId, cx: &App) {
    let Some(info) = cx.global::<Desktop>().hierarchy.state().tab(tab) else {
        return;
    };
    let order: Vec<PaneId> = info
        .layout
        .panes()
        .into_iter()
        .map(|pane| pane.id)
        .collect();
    panes.sort_by_key(|pane| {
        order
            .iter()
            .position(|id| *id == pane.pane)
            .unwrap_or(usize::MAX)
    });
}
