//! Views for tabs a window did not spawn: tabs another window opened in a
//! shared workspace, and the tabs of a session the window attached to.
//! Reconcile decides which tabs install (`projection::installs`); the window
//! subscribes their viewers on a worker, waits for the projection to reach
//! the sequence the worker read, and then pushes the views that still apply.
//! A view whose viewer was revoked while its tab stays is replaced the same
//! way (`projection::replacements`), keeping the view and swapping its
//! viewer.

use super::projection::install_applies;
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

/// Whether a subscription installs a new view or replaces the viewer of an
/// existing one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Subscription {
    Install,
    Replace,
}

impl WorkspaceView {
    /// Subscribes viewers for `tabs`, which reconcile has recorded as in
    /// flight, and pushes their views once the projection has caught up.
    /// Closing the window drops the viewers with the completion.
    pub(super) fn install_tabs(
        &mut self,
        tabs: &[TabId],
        cx: &mut Context<'_, Self>,
    ) {
        self.subscribe_views(tabs, Subscription::Install, cx);
    }

    /// Subscribes replacement viewers for the revoked views of `tabs`,
    /// which reconcile has recorded as in flight, each starting with its
    /// view's focus, geometry, and presentation, and swaps them in once the
    /// projection has caught up.
    pub(super) fn replace_tabs(
        &mut self,
        tabs: &[TabId],
        cx: &mut Context<'_, Self>,
    ) {
        self.subscribe_views(tabs, Subscription::Replace, cx);
    }

    fn subscribe_views(
        &mut self,
        tabs: &[TabId],
        kind: Subscription,
        cx: &mut Context<'_, Self>,
    ) {
        let attachment = self.attachment_id(cx);
        let fresh =
            ViewerStart::fresh(terminal_presentation(&self.config.theme));
        let mut targets = Vec::with_capacity(tabs.len());
        let mut gone = Vec::new();
        for &tab in tabs {
            let state = cx.global::<Desktop>().hierarchy.state();
            let start = match kind {
                Subscription::Install => Some(fresh.clone()),
                Subscription::Replace => self
                    .tabs
                    .iter()
                    .find(|view| view.id == tab)
                    .map(|view| view.view.read(cx).viewer_start()),
            };
            match (attachment, state.tab(tab), start) {
                (Some(_), Some(info), Some(start)) => {
                    targets.push((tab, info.terminal_id, start));
                }
                _ => gone.push(tab),
            }
        }
        for tab in gone {
            self.installs.finish(tab, false);
        }
        let Some(attachment) = attachment.filter(|_| !targets.is_empty())
        else {
            return;
        };
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let clipboard_allowed =
            self.config.terminal.clipboard_write.is_allowed();
        let task = cx.background_executor().spawn(async move {
            runtime.subscribe_tabs(attachment, &targets, clipboard_allowed)
        });
        let id = self.window;
        cx.spawn(async move |view, cx| {
            let Committed { result, seq } = task.await;
            let resolution = projected(cx, seq).await;
            let Some(handle) = cx.update(|cx| window_handle(cx, id)) else {
                return;
            };
            let _ = handle.update(cx, |_, window, cx| {
                view.update(cx, |view, cx| match kind {
                    Subscription::Install => {
                        view.finish_install(
                            result, seq, resolution, window, cx,
                        );
                    }
                    Subscription::Replace => {
                        view.finish_replace(result, resolution, window, cx);
                    }
                })
            });
        })
        .detach();
    }

    /// Swaps each replacement viewer into its view while the view is still
    /// revoked and the projection still holds its tab in this window's
    /// workspace, and drops the other viewers. A view whose replacement
    /// failed is removed, and its tab waits for a batch that touches the
    /// workspace before an install tries it again.
    fn finish_replace(
        &mut self,
        viewers: Vec<(TabId, Result<TerminalViewer, MuxError>)>,
        resolution: Resolution,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let current = resolution == Resolution::Ready && !terminating(cx);
        let mut removed = Vec::new();
        for (tab, result) in viewers {
            let revoked = self
                .tabs
                .iter()
                .any(|view| view.id == tab && view.view.read(cx).revoked);
            let applies = current
                && revoked
                && install_applies(
                    cx.global::<Desktop>().hierarchy.state(),
                    self.workspace_id(cx),
                    self.attached_session(cx),
                    &[],
                    tab,
                );
            let failed = match result {
                Ok(viewer) if applies => {
                    self.replace_viewer(tab, viewer, window, cx);
                    false
                }
                Ok(_) => false,
                Err(error) => {
                    if applies {
                        removed.push(tab);
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
                    applies
                }
            };
            self.installs.finish(tab, failed);
        }
        if removed.is_empty() {
            return;
        }
        let active = self.active_tab(cx);
        self.drop_tab_views(&removed, cx);
        self.measure_tab_widths(window, cx);
        if let Some(next) = self.active_tab(cx)
            && Some(next) != active
        {
            let focus = !self.overlay_holds_focus();
            self.select_tab(next, focus, window, cx);
        }
        cx.notify();
    }

    /// Swaps `viewer` into the view of `tab` and restarts the tab's
    /// activity drain on it; the old drain waits on the revoked viewer.
    fn replace_viewer(
        &mut self,
        tab: TabId,
        viewer: TerminalViewer,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(index) = self.tabs.iter().position(|view| view.id == tab)
        else {
            return;
        };
        let activity = viewer.wake_handle();
        let view = self.tabs[index].view.clone();
        let clipboard_allowed =
            self.config.terminal.clipboard_write.is_allowed();
        view.update(cx, |terminal, cx| {
            terminal.replace_viewer(viewer, clipboard_allowed, cx);
        });
        let failure_wakes = view.read(cx).failure_wakes.clone();
        let task =
            Self::tab_activity_task(tab, activity, failure_wakes, window, cx);
        self.tabs[index].restart_activity(task);
    }

    /// Pushes a view for each subscribed tab the projection still holds in
    /// this window's workspace without a view, whose viewer is not revoked,
    /// and drops the other viewers. Selects a tab only when the window has
    /// no active tab, and focuses it unless an overlay holds focus.
    fn finish_install(
        &mut self,
        viewers: Vec<(TabId, Result<TerminalViewer, MuxError>)>,
        committed: u64,
        resolution: Resolution,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let current = resolution == Resolution::Ready && !terminating(cx);
        let mut pushed = false;
        for (tab, result) in viewers {
            let installed: Vec<TabId> =
                self.tabs.iter().map(|tab| tab.id).collect();
            let state = cx.global::<Desktop>().hierarchy.state();
            let terminal = state.tab(tab).map(|info| info.terminal_id);
            let applies = current
                && install_applies(
                    state,
                    self.workspace_id(cx),
                    self.attached_session(cx),
                    &installed,
                    tab,
                );
            let failed = match (result, terminal) {
                (Ok(viewer), Some(terminal)) if applies => {
                    if let Some(view) = self
                        .tab_view(tab, terminal, viewer, committed, window, cx)
                    {
                        self.push_tab_view(view, false, cx);
                        pushed = true;
                    }
                    false
                }
                (Ok(_), _) => false,
                (Err(error), _) => {
                    if applies && !stale_install(&error) {
                        self.notify(
                            NoticeContent::command_failure(
                                "Cannot show tab",
                                format!("Cannot show tab: {error}"),
                            ),
                            cx,
                        );
                    }
                    applies
                }
            };
            self.installs.finish(tab, failed);
        }
        if !pushed {
            return;
        }
        self.measure_tab_widths(window, cx);
        match (self.active_tab(cx), self.tabs.first()) {
            (None, Some(first)) => {
                let first = first.id;
                let focus = !self.overlay_holds_focus();
                self.select_tab(first, focus, window, cx);
            }
            // Hidden views still report their geometry.
            _ => self.sync_tab_layout(window, cx),
        }
        cx.notify();
    }
}
