//! Tier 2 tests for retained workspace views (R1): a window holds hidden
//! views of every workspace of its session, keeps them in line with
//! changes other clients make, and treats a hidden workspace's terminals
//! as it treats a hidden tab's. Interleavings R1-1 to R1-18 of
//! `docs/plans/retained-workspace-views.md`. Ordering comes from worker
//! hooks and sequence waits, never sleeps.

use super::super::workspace_status::Dot;
use super::pane_moves::active;
use super::sessions::{shown_tabs, switching};
use super::workspaces::{empty_state, shown_workspace, workspace_pick};
use super::*;

/// A child that prints `READY`, then rings a bell for each line it reads.
const BELLS: &str =
    "printf READY; while read line; do printf '\\007RANG'; done";

/// A child that posts a notification for each line it reads.
const NOTES: &str = "printf READY; while read line; do printf '\\033]9;Build done\\007NOTED'; done";

impl DesktopHarness {
    /// Waits until `window` holds hidden views of exactly `tabs` for
    /// `workspace`, in order.
    pub(super) fn wait_hidden(
        &mut self,
        window: AnyWindowHandle,
        workspace: WorkspaceId,
        tabs: &[TabId],
    ) {
        self.wait_for("the window to hold the hidden views", |cx| {
            hidden_tabs(cx, window, workspace).as_deref() == Some(tabs)
        });
    }
}

/// The tabs `window` holds hidden views of for `workspace`, in order;
/// `None` when it keeps none for it.
pub(super) fn hidden_tabs(
    cx: &mut App,
    window: AnyWindowHandle,
    workspace: WorkspaceId,
) -> Option<Vec<TabId>> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx)
                .hidden
                .get(&workspace)
                .map(|tabs| tabs.iter().map(|tab| tab.id).collect())
        })
        .ok()
        .flatten()
}

/// The focused view of `tab` that `window` holds hidden.
pub(super) fn hidden_view(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
) -> Option<Entity<TerminalView>> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx)
                .hidden
                .values()
                .flatten()
                .find(|view| view.id == tab)
                .map(|tab| tab.focused_view().clone())
        })
        .ok()
        .flatten()
}

/// The pane views `window` holds hidden for `tab`.
fn hidden_panes(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
) -> Vec<PaneId> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx)
                .hidden
                .values()
                .flatten()
                .find(|view| view.id == tab)
                .map(|tab| tab.panes.iter().map(|pane| pane.pane).collect())
        })
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// The entity of `window`'s view of `tab`, shown or hidden.
fn held_view(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
) -> Option<gpui::EntityId> {
    terminal(cx, window, tab)
        .or_else(|| hidden_view(cx, window, tab))
        .map(|view| view.entity_id())
}

/// Whether any view `window` holds hidden is visible.
fn any_hidden_visible(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().unwrap();
            let view = view.read(cx);
            view.hidden
                .values()
                .flatten()
                .flat_map(TabView::views)
                .any(|terminal| terminal.read(cx).visible)
        })
        .unwrap()
}

/// R1-4, R1-6, R1-7: closes, splits, and moves other clients make in
/// workspaces a window does not show reach its hidden views, which keep
/// their entities when their tab moves between workspaces the window
/// holds, and never take the shown terminal's focus.
#[gpui::test]
fn hidden_views_follow_closes_splits_and_moves_made_elsewhere(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let shown = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[shown]);
    harness.activate(window, shown);
    let (second, seconds) = harness.add_workspace(2);
    let (third, thirds) = harness.add_workspace(1);
    harness.wait_hidden(window, second, &seconds);
    harness.wait_hidden(window, third, &thirds);
    let (closed, split) = (seconds[0], seconds[1]);
    let moved = thirds[0];
    let kept = |harness: &mut DesktopHarness| {
        harness.cx.update(|cx| {
            assert_eq!(shown_workspace(cx, window), Some(harness.workspace));
            assert!(!switching(cx, window) && !busy(cx, window));
            assert!(
                focused(cx, window, shown) || active(cx, window) != Some(shown)
            );
            assert!(!any_hidden_visible(cx, window));
        });
    };

    // A split elsewhere installs the new pane's view, hidden.
    let pane = harness.first_pane(split);
    let added = harness
        .runtime
        .lock()
        .split_pane(second, split, pane, SplitDirection::Right, &fixture(ECHO))
        .unwrap()
        .pane
        .id;
    harness.wait_for("the hidden split", |cx| {
        hidden_panes(cx, window, split) == [pane, added]
    });
    kept(&mut harness);

    // A tab closed elsewhere drops its hidden view and nothing else.
    harness.runtime.lock().close_tab(second, closed).unwrap();
    harness.wait_hidden(window, second, &[split]);
    kept(&mut harness);
    harness.update_view(window, |view, _, _| {
        assert_eq!(view.close.emptied, EmptiedClose::None);
    });

    // A tab moved between two hidden workspaces keeps its view.
    let view = harness.cx.update(|cx| held_view(cx, window, moved));
    harness
        .runtime
        .lock()
        .move_tab(third, moved, second, None)
        .unwrap();
    harness.wait_hidden(window, second, &[split, moved]);
    harness.wait_hidden(window, third, &[]);
    assert_eq!(harness.cx.update(|cx| held_view(cx, window, moved)), view);

    // Into the shown workspace it joins inactive and stays hidden.
    let view = harness.cx.update(|cx| held_view(cx, window, split));
    harness
        .runtime
        .lock()
        .move_tab(second, split, harness.workspace, None)
        .unwrap();
    harness.wait_for("the moved tab to join the shown workspace", |cx| {
        shown_tabs(cx, window) == [shown, split]
    });
    harness.cx.update(|cx| {
        assert_eq!(held_view(cx, window, split), view);
        assert_eq!(active(cx, window), Some(shown));
        assert!(!terminal(cx, window, split).unwrap().read(cx).visible);
    });
    kept(&mut harness);

    // Out of it, the shown workspace's next tab takes over.
    let view = harness.cx.update(|cx| held_view(cx, window, shown));
    harness
        .runtime
        .lock()
        .move_tab(harness.workspace, shown, second, None)
        .unwrap();
    harness.wait_hidden(window, second, &[moved, shown]);
    harness.wait_for("the remaining tab to show", |cx| {
        active(cx, window) == Some(split)
            && terminal(cx, window, split).unwrap().read(cx).visible
    });
    harness.cx.update(|cx| {
        assert_eq!(held_view(cx, window, shown), view);
        assert!(!any_hidden_visible(cx, window));
    });

    // A workspace closed elsewhere drops its entry.
    harness.runtime.lock().close_workspace(third).unwrap();
    harness.wait_for("the closed workspace's entry to go", |cx| {
        hidden_tabs(cx, window, third).is_none()
    });

    // Showing the workspace that gathered the moves subscribes nothing.
    let subscribed = harness.runtime.worker_hooks.entered(Stage::Subscribe);
    harness.show_workspace(window, second, &[moved, shown]);
    assert_eq!(
        harness.runtime.worker_hooks.entered(Stage::Subscribe),
        subscribed
    );
    harness.answers(window, moved, "moved");
}

/// R1-8: a hidden workspace moved to another session takes its views
/// away, and the window can no longer select it.
#[gpui::test]
fn a_hidden_workspace_moved_to_another_session_drops_its_views(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let shown = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[shown]);
    let (second, seconds) = harness.add_workspace(1);
    harness.wait_hidden(window, second, &seconds);
    let other = harness.other_session("other", 1);
    harness
        .runtime
        .lock()
        .move_workspace(harness.session, second, other.session, None)
        .unwrap();
    harness.wait_for("the moved workspace's views to go", |cx| {
        hidden_tabs(cx, window, second).is_none()
    });
    assert!(matches!(
        harness.invoke_with(
            window,
            &workspace_pick(ids::SELECT_WORKSPACE, second)
        ),
        Err(CommandError::Unavailable(_))
    ));
    harness.answers(window, shown, "stays");
}

/// R1-10: a session switch replaces the left session's views, hidden
/// ones included, with views of every workspace of the target.
#[gpui::test]
fn a_session_switch_replaces_every_workspaces_views(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let shown = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[shown]);
    let (second, seconds) = harness.add_workspace(1);
    harness.wait_hidden(window, second, &seconds);
    let other = harness.other_session("other", 1);
    let (elsewhere, elsewheres) = {
        let mut mux = harness.runtime.lock();
        let workspace = mux.create_workspace(other.session, None).unwrap();
        let tab = mux.open_tab(workspace, &fixture(ECHO)).unwrap().tab.id;
        (workspace, vec![tab])
    };
    harness.switch_to(window, other.session, &other.tabs);
    harness.wait_hidden(window, elsewhere, &elsewheres);
    harness.cx.update(|cx| {
        assert!(hidden_tabs(cx, window, second).is_none());
        assert!(hidden_tabs(cx, window, harness.workspace).is_none());
        assert!(held_view(cx, window, seconds[0]).is_none());
    });
    harness.show_workspace(window, elsewhere, &elsewheres);
    harness.answers(window, elsewheres[0], "there");
}

/// R1-16: a bell in a hidden workspace marks its tab and its row, flashes
/// nothing, and is seen once the window shows the workspace.
#[gpui::test]
fn a_hidden_workspaces_bell_marks_its_tab_and_row_without_a_flash(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.terminal.bell.visual = true;
    });
    let shown = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[shown]);
    harness.activate(window, shown);
    let (bells, rung) = harness.add_workspace_running(BELLS, 1);
    harness.wait_hidden(window, bells, &rung);
    harness.wait_for("the hidden shell to start", |cx| {
        hidden_view(cx, window, rung[0])
            .is_some_and(|view| view.read(cx).seen_running)
    });
    let _viewer = harness.send_line(rung[0], "a");
    harness.wait_for("the hidden tab's bell mark", |cx| {
        hidden_view(cx, window, rung[0])
            .is_some_and(|view| view.read(cx).bell.marked())
    });
    harness.wait_for("the row's bell", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx)
                        .workspace_row_status(bells, cx)
                        .is_some_and(|status| status.dot == Dot::Bell)
                })
            })
            .unwrap_or(false)
    });
    harness.cx.update(|cx| {
        let view = hidden_view(cx, window, rung[0]).unwrap();
        assert_eq!(view.read(cx).bell_flash_count, 0, "a hidden view flashed");
    });
    harness.show_workspace(window, bells, &rung);
    harness.wait_for("the row's bell to clear", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx).workspace_row_status(bells, cx).is_none()
                })
            })
            .unwrap_or(false)
    });
}

/// R1-17: a notification from a hidden workspace reaches the window as a
/// notice whose Show Tab switches to the workspace and focuses the pane.
#[gpui::test]
fn a_hidden_workspaces_notification_offers_show_tab_which_switches_to_it(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let shown = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[shown]);
    harness.activate(window, shown);
    let (notes, posting) = harness.add_workspace_running(NOTES, 1);
    harness.wait_hidden(window, notes, &posting);
    harness.wait_for("the hidden shell to start", |cx| {
        hidden_view(cx, window, posting[0])
            .is_some_and(|view| view.read(cx).seen_running)
    });
    let _note = harness.send_line(posting[0], "note");
    harness.wait_for("the notice", |cx| {
        cx.global::<Desktop>().notifier.notices == 1
    });
    harness.update_view(window, |view, window, cx| {
        let id = view.notices.newest().unwrap();
        let notice = &view.notices.get(id).unwrap().content;
        assert_eq!(notice.actions[0].label, "Show Tab");
        view.run_notice_action(id, None, window, cx).unwrap();
    });
    harness.wait_for("the revealed tab", |cx| {
        shown_workspace(cx, window) == Some(notes)
            && focused(cx, window, posting[0])
    });
}

/// R1-18: a shell that exits in a hidden workspace under `close_on_exit`
/// closes its tab as a hidden tab's does; a workspace that empties then
/// closes unless pinned, while the window stays where it is.
#[gpui::test]
fn an_exit_in_a_hidden_workspace_closes_its_tab_and_an_emptied_workspace(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let shown = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[shown]);
    let (emptied, emptied_tabs) = harness.add_workspace_running(EXITS, 1);
    let (pinned, pinned_tabs) = harness.add_workspace_running(EXITS, 1);
    let (busy_workspace, busy_tabs) = {
        let mut mux = harness.runtime.lock();
        let workspace = mux.create_workspace(harness.session, None).unwrap();
        let exits = mux.open_tab(workspace, &fixture(EXITS)).unwrap().tab.id;
        let stays = mux.open_tab(workspace, &fixture(ECHO)).unwrap().tab.id;
        (workspace, vec![exits, stays])
    };
    harness
        .runtime
        .lock()
        .set_workspace_pinned(pinned, true)
        .unwrap();
    harness.wait_hidden(window, emptied, &emptied_tabs);
    harness.wait_hidden(window, pinned, &pinned_tabs);
    harness.wait_hidden(window, busy_workspace, &busy_tabs);
    let exiting = [emptied_tabs[0], pinned_tabs[0], busy_tabs[0]];
    harness.wait_for("the hidden shells to start", |cx| {
        exiting.iter().all(|tab| {
            hidden_view(cx, window, *tab)
                .is_some_and(|view| view.read(cx).seen_running)
        })
    });
    let _viewers: Vec<_> = exiting
        .iter()
        .map(|tab| harness.send_line(*tab, "done"))
        .collect();
    harness.wait_for("the emptied workspace to close", |cx| {
        let state = cx.global::<Desktop>().hierarchy.state();
        state.workspace_session(emptied).is_none()
            && state
                .workspace_tabs(pinned)
                .is_some_and(<[TabId]>::is_empty)
            && state.workspace_tabs(busy_workspace)
                == Some([busy_tabs[1]].as_slice())
    });
    harness.wait_hidden(window, pinned, &[]);
    harness.wait_hidden(window, busy_workspace, &[busy_tabs[1]]);
    harness.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, window), Some(harness.workspace));
        assert_eq!(shown_tabs(cx, window), [shown]);
        assert!(!switching(cx, window));
    });
    harness.update_view(window, |view, _, _| {
        assert_eq!(view.close.emptied, EmptiedClose::None);
    });
    harness.answers(window, shown, "still");
}

/// R1-7 into an empty workspace: a tab adopted from a hidden workspace
/// into the shown one that had no tab becomes active and shows, as an
/// installed one does.
#[gpui::test]
fn a_tab_adopted_into_an_empty_shown_workspace_shows(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first]);
    let (second, seconds) = harness.add_workspace(1);
    harness.wait_hidden(window, second, &seconds);
    // The shown workspace stays, empty, once pinned.
    harness
        .runtime
        .lock()
        .set_workspace_pinned(harness.workspace, true)
        .unwrap();
    harness
        .runtime
        .lock()
        .close_tab(harness.workspace, first)
        .unwrap();
    harness.wait_for("the empty state", |cx| empty_state(cx, window).is_some());
    let view = harness.cx.update(|cx| held_view(cx, window, seconds[0]));
    harness
        .runtime
        .lock()
        .move_tab(second, seconds[0], harness.workspace, None)
        .unwrap();
    harness.wait_for("the adopted tab to show", |cx| {
        shown_tabs(cx, window) == seconds
            && active(cx, window) == Some(seconds[0])
            && terminal(cx, window, seconds[0])
                .is_some_and(|view| view.read(cx).visible)
            && empty_state(cx, window).is_none()
    });
    assert_eq!(
        harness.cx.update(|cx| held_view(cx, window, seconds[0])),
        view
    );
    harness.answers(window, seconds[0], "adopted");
}

/// R1-17 with two windows: Show Tab reveals the pane in the window that
/// shows its workspace before switching one that holds it hidden.
#[gpui::test]
fn show_tab_prefers_the_window_that_shows_the_pane(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let holding = harness.attach_showing(&[first]);
    let (second, seconds) = harness.add_workspace(1);
    harness.wait_hidden(holding, second, &seconds);
    let showing = harness.attach_showing(&[first]);
    harness.show_workspace(showing, second, &seconds);
    let pane = harness.first_pane(seconds[0]);
    // Asked first, the holding window would switch but for the showing
    // window's precedence.
    harness.cx.update(|cx| {
        let order: Vec<_> = cx
            .windows()
            .into_iter()
            .filter(|handle| [holding, showing].contains(handle))
            .collect();
        assert_eq!(order, [holding, showing]);
    });
    let revealed = harness.cx.update(|cx| {
        super::super::terminal_notifications::reveal_pane(pane.get(), cx)
    });
    assert!(revealed);
    harness.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, holding), Some(harness.workspace));
        assert_eq!(shown_workspace(cx, showing), Some(second));
    });
}

/// An install that completes for a tab another client moved while the
/// window was busy, before the window adopted the tab's view, builds no
/// second view of the tab: the moved view takes the new pane afterwards.
#[gpui::test]
fn an_install_racing_a_deferred_adoption_builds_no_second_view(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first]);
    let (second, seconds) = harness.add_workspace(1);
    harness.wait_hidden(window, second, &seconds);
    let moving = seconds[0];
    let runtime = Arc::clone(&harness.runtime);
    // A split elsewhere starts the hidden pane's install, which waits.
    runtime.worker_hooks.hold(Stage::Subscribe);
    let pane = harness.first_pane(moving);
    let added = runtime
        .lock()
        .split_pane(second, moving, pane, SplitDirection::Right, &fixture(ECHO))
        .unwrap()
        .pane
        .id;
    harness.wait_for("the split's install to wait", |_| {
        runtime.worker_hooks.held(Stage::Subscribe) > 0
    });
    // The window's own New Tab keeps it busy, so it adopts nothing.
    runtime.worker_hooks.hold(Stage::Spawned);
    harness.invoke(window, ids::NEW_TAB).unwrap();
    harness.wait_for("the new tab to wait", |_| {
        runtime.worker_hooks.held(Stage::Spawned) == 1
    });
    runtime
        .lock()
        .move_tab(second, moving, harness.workspace, None)
        .unwrap();
    harness.wait_projection();
    runtime.worker_hooks.release(Stage::Subscribe);
    // The install completes while the window is still busy, before any
    // adoption: its tab is shown now, but the window's view is still hidden.
    harness.wait_for("the split's install to complete", |cx| {
        window
            .update(cx, |root, _, cx| {
                let view = root.downcast::<WorkspaceView>().unwrap();
                view.read(cx).installs.idle_in(Some(second))
            })
            .unwrap()
    });
    harness.cx.update(|cx| {
        assert!(busy(cx, window));
        assert!(!shown_tabs(cx, window).contains(&moving));
        assert_eq!(
            hidden_tabs(cx, window, second).as_deref(),
            Some(&[moving][..])
        );
        assert_eq!(hidden_panes(cx, window, moving), [pane]);
    });
    runtime.worker_hooks.release(Stage::Spawned);
    harness.wait_for("the moved tab to show both panes", |cx| {
        let tabs = shown_tabs(cx, window);
        tabs.iter().filter(|tab| **tab == moving).count() == 1
            && panes_shown(cx, window, moving)
                .is_some_and(|(panes, _)| panes == [pane, added])
            && hidden_tabs(cx, window, second).as_deref() == Some(&[][..])
            && !busy(cx, window)
    });
}

/// R1-2 across a departure and a capture: a switch away before the
/// remembered tabs arrive files their navigation, and a capture meanwhile
/// saves it, so it applies on the next visit.
#[gpui::test]
fn remembered_navigation_survives_leaving_before_its_tabs_arrive(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[first]);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Subscribe);
    let (second, seconds) = harness.add_workspace(2);
    let split = seconds[1];
    let left = harness.first_pane(split);
    let right = runtime
        .lock()
        .split_pane(second, split, left, SplitDirection::Right, &fixture(ECHO))
        .unwrap()
        .pane
        .id;
    harness.wait_for("the hidden install to wait", |_| {
        runtime.worker_hooks.held(Stage::Subscribe) > 0
    });
    let session = harness.session;
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().windows.restore_memory(
            window.window_id(),
            vec![super::super::model::NavigationMemory {
                session,
                workspace: second,
                active: Some(split),
                history: vec![split, seconds[0]],
                panes: vec![(split, right)],
                layout_history:
                    super::super::layout_history::LayoutHistory::default(),
            }],
        );
    });
    harness
        .invoke_with(window, &workspace_pick(ids::SELECT_WORKSPACE, second))
        .unwrap();
    // A capture while the tabs install saves what the window will show.
    let saved = harness
        .cx
        .update(|cx| capture_windows(cx).windows.remove(0));
    assert_eq!(saved.active_tab.map(TabId::get), Some(split.get()));
    // Saved most recent last.
    let history: Vec<u64> =
        saved.tab_history.iter().map(|tab| tab.get()).collect();
    assert_eq!(history, [seconds[0].get(), split.get()]);
    assert!(saved.focused_panes.iter().any(|focused| {
        focused.tab.get() == split.get() && focused.pane.get() == right.get()
    }));
    // Leaving before they arrive files it back into memory.
    harness
        .invoke_with(
            window,
            &workspace_pick(ids::SELECT_WORKSPACE, harness.workspace),
        )
        .unwrap();
    runtime.worker_hooks.release(Stage::Subscribe);
    harness.wait_hidden(window, second, &seconds);
    harness.show_workspace(window, second, &seconds);
    harness.cx.update(|cx| {
        assert_eq!(active(cx, window), Some(split));
        assert_eq!(
            panes_shown(cx, window, split).map(|(_, focused)| focused),
            Some(right)
        );
        let record = cx.global::<Desktop>().windows.record(window.window_id());
        assert_eq!(record.unwrap().history, [split, seconds[0]]);
    });
}
