//! Views for tabs a window did not spawn: tabs another window opened in a
//! shared workspace, and the tabs of a session the window attached to.
//! Reconcile decides which tabs install (`projection::installs`); the window
//! subscribes their viewers on a worker, waits for the projection to reach
//! the sequence the worker read, and then pushes the views that still apply.

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

impl WorkspaceView {
    /// Subscribes viewers for `tabs`, which reconcile has recorded as in
    /// flight, and pushes their views once the projection has caught up.
    /// Closing the window drops the viewers with the completion.
    pub(super) fn install_tabs(
        &mut self,
        tabs: &[TabId],
        cx: &mut Context<'_, Self>,
    ) {
        let attachment = self.attachment_id(cx);
        let state = cx.global::<Desktop>().hierarchy.state();
        let mut targets = Vec::with_capacity(tabs.len());
        let mut gone = Vec::new();
        for &tab in tabs {
            match (attachment, state.tab(tab)) {
                (Some(_), Some(info)) => targets.push((tab, info.terminal_id)),
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
        let presentation = terminal_presentation(&self.config.theme);
        let task = cx.background_executor().spawn(async move {
            runtime.subscribe_tabs(
                attachment,
                &targets,
                clipboard_allowed,
                &presentation,
            )
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
                    view.finish_install(result, seq, resolution, window, cx);
                })
            });
        })
        .detach();
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
