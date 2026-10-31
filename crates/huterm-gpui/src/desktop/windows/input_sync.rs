//! Pane input sync: the toggle for the active tab, and the fan-out that
//! types a syncing tab's focused pane's input into its other panes too.
//!
//! Core holds each tab's flag; this window sends the input through its
//! own viewers, so every terminal keeps its input in order and encodes it
//! against its own modes.

use std::sync::Arc;

use gpui::{App, Context, Entity, Window};
use huterm_protocol::{CommandError, CommandOutcome, TabId, TerminalId};

use super::{
    Desktop, PaneView, SyncedInput, TabView, TerminalView, WorkspaceView,
    terminating,
};

impl WorkspaceView {
    /// Turns the active tab's input sync on or off. The change reaches
    /// this window through the hierarchy, as another window's would.
    pub(super) fn toggle_input_sync(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        let tab = self.active_tab_id(cx)?;
        let workspace = self.workspace_id(cx).ok_or_else(|| {
            CommandError::Unavailable("window has no workspace".to_owned())
        })?;
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let work = cx
            .background_executor()
            .spawn(async move { runtime.toggle_input_sync(workspace, tab) });
        cx.spawn_in(window, async move |view, cx| {
            if let Err(error) = work.await {
                let _ = view.update(cx, |view, cx| {
                    if !terminating(cx) {
                        view.report_failure(
                            "Pane input sync",
                            format!("Cannot change pane input sync: {error}"),
                            cx,
                        );
                    }
                });
            }
        })
        .detach();
        Ok(CommandOutcome::Accepted)
    }

    /// Types input a pane synced into each other pane of its tab that the
    /// projected layout still places and this window shows, through each
    /// pane's own view. A pane a zoom hides gets none, as in tmux, since
    /// nobody can see what it would type. Membership is read now, so a
    /// pane that joined since the input was typed gets it and one that left
    /// does not.
    pub(super) fn handle_synced_input(
        &mut self,
        source: &Entity<TerminalView>,
        event: &SyncedInput,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(tab) = self
            .tabs
            .iter()
            .find(|tab| tab.views().any(|view| view == source))
        else {
            return;
        };
        let Some(info) = cx.global::<Desktop>().hierarchy.state().tab(tab.id)
        else {
            return;
        };
        if !info.input_sync {
            return;
        }
        let targets: Vec<Entity<TerminalView>> = tab
            .panes
            .iter()
            .filter(|pane| {
                &pane.view != source
                    && info.layout.pane(pane.pane).is_some()
                    && self.zoom_shows(tab, pane.pane, cx)
            })
            .map(|pane| pane.view.clone())
            .collect();
        for target in targets {
            target.update(cx, |view, cx| {
                view.receive_synced_input(event.0.clone(), cx);
            });
        }
    }
}

/// Whether the tab showing `terminal` syncs input. False outside a
/// desktop, as in benchmarks that host a bare view.
pub(in crate::desktop) fn terminal_syncs_input(
    terminal: TerminalId,
    cx: &App,
) -> bool {
    let Some(desktop) = cx.try_global::<Desktop>() else {
        return false;
    };
    let state = desktop.hierarchy.state();
    state
        .terminal_tab(terminal)
        .and_then(|tab| state.tab(tab))
        .is_some_and(|tab| tab.input_sync)
}

/// Whether typing now reaches `pane` of `tab` through input sync, so it
/// draws the sync frame and title bar mark: the tab syncs, and neither the
/// pane nor the focused pane, where typing starts, has exited. An exited
/// focused pane takes no input, so it syncs none; the tab keeps its mark.
pub(super) fn receives(tab: &TabView, pane: &PaneView, cx: &App) -> bool {
    syncs_input(tab.id, cx)
        && !pane.view.read(cx).exited
        && !tab.focused().view.read(cx).exited
}

/// Whether `tab` syncs input, from the hierarchy projection.
pub(super) fn syncs_input(tab: TabId, cx: &App) -> bool {
    cx.global::<Desktop>()
        .hierarchy
        .state()
        .tab(tab)
        .is_some_and(|tab| tab.input_sync)
}
