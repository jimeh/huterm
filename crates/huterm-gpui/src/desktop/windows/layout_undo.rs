//! The `undo` and `redo` commands, and recording the layout changes they
//! restore. Each restore commits one runtime operation on a worker, which
//! the runtime refuses when the touched tabs no longer match the entry.

use huterm_core::RestoreDirection;
use huterm_protocol::LayoutChange;

use super::layout_history::HistoryEntry;
use super::{
    App, Arc, CommandError, CommandOutcome, Committed, Context, Desktop,
    DesktopRuntime, MuxError, Ordering, PaneId, RuntimeError, Window,
    WorkspaceView, ids, projected, terminating,
};

impl DesktopRuntime {
    /// Restores `change` under the Mux lock, refusing during teardown.
    fn commit_layout_restore(
        &self,
        change: &LayoutChange,
        direction: RestoreDirection,
    ) -> Committed<Result<LayoutChange, MuxError>> {
        let mut mux = self.lock();
        let result = if self.terminating.load(Ordering::Acquire) {
            Err(RuntimeError::Stopped.into())
        } else {
            mux.restore_tabs(change, direction)
        };
        Committed {
            result,
            seq: mux.hierarchy_seq(),
        }
    }
}

/// The direction an `undo` or `redo` command restores in.
pub(super) fn restore_direction(
    command: huterm_protocol::CommandId,
) -> Option<RestoreDirection> {
    match command {
        ids::UNDO => Some(RestoreDirection::Undo),
        ids::REDO => Some(RestoreDirection::Redo),
        _ => None,
    }
}

/// The command's name in titles and notices.
fn verb(direction: RestoreDirection) -> &'static str {
    match direction {
        RestoreDirection::Undo => "Undo",
        RestoreDirection::Redo => "Redo",
    }
}

impl WorkspaceView {
    /// Records a layout change this window committed at hierarchy sequence
    /// `seq` as its undo entry, under `label`, with `focus` as the pane to
    /// focus after an undo or redo. Every completion that rearranges panes
    /// calls this once the projection holds its result. An empty change
    /// records nothing, and a change older than the newest entry is
    /// ignored. The history is then checked against the projection.
    pub(super) fn record_layout_change(
        &mut self,
        label: &'static str,
        focus: Option<PaneId>,
        change: LayoutChange,
        seq: u64,
        cx: &mut App,
    ) {
        if change.is_empty() {
            return;
        }
        self.restore_focus = None;
        let Desktop {
            windows, hierarchy, ..
        } = cx.global_mut::<Desktop>();
        let Some(history) = windows.layout_history_mut(self.window) else {
            return;
        };
        let entry = HistoryEntry {
            label,
            focus,
            change,
            seq,
        };
        if history.record(entry) {
            history.prune(hierarchy.state());
        }
    }

    /// The label of the entry `direction` would restore next.
    pub(super) fn layout_restore_label(
        &self,
        direction: RestoreDirection,
        cx: &App,
    ) -> Option<&'static str> {
        let record = cx.global::<Desktop>().windows.record(self.window)?;
        Some(record.layout_history.peek(direction)?.label)
    }

    /// Whether `undo` or `redo` can run now: no structural operation,
    /// confirmation, or quit in progress, and an entry to restore.
    pub(super) fn layout_restore_availability(
        &self,
        direction: RestoreDirection,
        cx: &App,
    ) -> Result<(), CommandError> {
        self.check_new_tab_available(cx)?;
        if self.layout_restore_label(direction, cx).is_none() {
            return Err(CommandError::Unavailable(format!(
                "nothing to {}",
                verb(direction).to_lowercase()
            )));
        }
        Ok(())
    }

    /// Runs `undo` or `redo`. With nothing to restore, or while another
    /// structural operation runs, it does nothing, so a repeated press is
    /// quietly taken. Otherwise the entry leaves the history
    /// while the restore runs; once the projection holds the result, it
    /// moves to the other direction and the window shows and focuses the
    /// entry's pane. A refusal drops the entry and raises a notice.
    pub(super) fn run_layout_restore(
        &mut self,
        direction: RestoreDirection,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        if self.busy {
            return Ok(CommandOutcome::Completed);
        }
        self.check_new_tab_available(cx)?;
        let entry = cx
            .global_mut::<Desktop>()
            .windows
            .layout_history_mut(self.window)
            .and_then(|history| history.take(direction));
        let Some(entry) = entry else {
            return Ok(CommandOutcome::Completed);
        };
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        self.busy = true;
        self.restore_focus = None;
        let change = entry.change.clone();
        let task = cx.background_executor().spawn(async move {
            runtime.commit_layout_restore(&change, direction)
        });
        let app = cx.to_async();
        cx.spawn_in(window, async move |view, cx| {
            let Committed { result, seq } = task.await;
            projected(&app, seq).await;
            let _ = view.update_in(cx, |view, window, cx| {
                view.busy = false;
                // Reconcile first, so the entry's pane has its view in the
                // tab that now holds it.
                view.reconcile_own(window, cx);
                view.finish_layout_restore(
                    direction, entry, result, seq, window, cx,
                );
                view.restore_tab_focus(window, cx);
                view.resume_close(window, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
        Ok(CommandOutcome::Accepted)
    }

    fn finish_layout_restore(
        &mut self,
        direction: RestoreDirection,
        entry: HistoryEntry,
        result: Result<LayoutChange, MuxError>,
        seq: u64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Err(error) = result {
            if !terminating(cx) {
                let reason = match error {
                    MuxError::LayoutChanged | MuxError::InvalidRestore => {
                        "the layout changed".to_owned()
                    }
                    error => error.to_string(),
                };
                let verb = verb(direction);
                let message =
                    format!("Cannot {}: {reason}", verb.to_lowercase());
                self.report_failure(verb, message, cx);
            }
            return;
        }
        let focus = entry.focus;
        let Desktop {
            windows, hierarchy, ..
        } = cx.global_mut::<Desktop>();
        if let Some(history) = windows.layout_history_mut(self.window) {
            history.finish(direction, entry, seq);
            history.prune(hierarchy.state());
        }
        let Some(pane) = focus else {
            return;
        };
        if !self.focus_restored_pane(pane, window, cx)
            && self.projects(pane, cx)
        {
            // The pane came back into this window, and its view is still
            // installing.
            self.restore_focus = Some(pane);
        }
    }

    /// Shows the tab that holds `pane` and focuses it there, if this window
    /// has its view.
    pub(super) fn focus_restored_pane(
        &mut self,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Some(tab) = self.tab_showing(pane) else {
            return false;
        };
        if self.active_tab(cx) != Some(tab) {
            self.select(tab, window, cx);
        }
        self.focus_pane(tab, pane, window, cx);
        true
    }

    /// Whether the projection puts `pane` in this window's workspace.
    fn projects(&self, pane: PaneId, cx: &App) -> bool {
        let state = cx.global::<Desktop>().hierarchy.state();
        let workspace = state
            .pane_tab(pane)
            .and_then(|tab| state.tab_workspace(tab));
        workspace.is_some() && workspace == self.workspace_id(cx)
    }
}
