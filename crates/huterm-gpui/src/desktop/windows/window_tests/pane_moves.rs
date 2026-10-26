//! Tier 2 tests for pane moves: the commands, reconcile adopting a moved
//! pane's view with its new tab's visibility, pane order after a swap, and
//! closes that follow their pane into another tab or let it go.

use gpui::EntityId;
use huterm_core::{Beside, MoveDestination, PaneRef};
use huterm_protocol::ViewerId;

use super::*;

/// The entity and viewer of `window`'s view of `pane` in `tab`.
fn identity(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
    pane: PaneId,
) -> Option<(EntityId, ViewerId)> {
    let view = pane_view(cx, window, tab, pane)?;
    let viewer = view.read(cx).viewer.id();
    Some((view.entity_id(), viewer))
}

/// The tab `window` shows as active.
fn active(cx: &mut App, window: AnyWindowHandle) -> Option<TabId> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx).active_tab(cx)
        })
        .ok()
        .flatten()
}

/// Whether `window`'s view of `pane` in `tab` has keyboard focus.
fn pane_focused(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
    pane: PaneId,
) -> bool {
    pane_view(cx, window, tab, pane).is_some_and(|view| {
        window
            .update(cx, |_, window, cx| view.read(cx).focus.is_focused(window))
            .unwrap_or(false)
    })
}

impl DesktopHarness {
    /// Where `pane` is now, from the runtime.
    fn pane_ref(&self, pane: PaneId) -> PaneRef {
        let target = self.runtime.lock().select_pane(pane).unwrap();
        PaneRef {
            workspace: target.workspace.unwrap(),
            tab: target.tab.unwrap(),
            pane,
        }
    }

    /// Moves `pane` beside `target` through Mux, as another client would.
    fn move_beside(&self, pane: PaneId, target: PaneId, side: SplitDirection) {
        let (from, target) = (self.pane_ref(pane), self.pane_ref(target));
        self.runtime
            .lock()
            .move_pane(from, MoveDestination::Beside(Beside { target, side }))
            .unwrap();
    }

    /// Runs `invocation` in `window` as a binding or the palette would.
    fn invoke_with(
        &mut self,
        window: AnyWindowHandle,
        invocation: &CommandInvocation,
    ) -> Result<CommandOutcome, CommandError> {
        self.update_view(window, |view, window, cx| {
            view.invoke_interactive(invocation, window, cx)
        })
    }

    /// Focuses `pane` of `tab` in `window`, activating the tab.
    fn focus(&mut self, window: AnyWindowHandle, tab: TabId, pane: PaneId) {
        self.update_view(window, |view, window, cx| {
            view.select(tab, window, cx);
            view.focus_pane(tab, pane, window, cx);
        });
        self.wait_for("the pane to take keyboard focus", |cx| {
            pane_focused(cx, window, tab, pane)
        });
    }
}

#[gpui::test]
fn a_pane_moved_between_tabs_keeps_its_view_viewer_and_history(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let second = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.activate(window, first);
    let kept = harness.first_pane(first);
    let moving = harness.split(first, kept, SplitDirection::Right, ECHO);
    let target = harness.first_pane(second);
    // The second tab is hidden, so only its view, not its screen, shows.
    harness.wait_for("every pane to install", |cx| {
        pane_screen(cx, window, first, moving)
            .is_some_and(|text| text.contains("READY"))
            && pane_view(cx, window, second, target).is_some()
    });
    let before = harness
        .cx
        .update(|cx| identity(cx, window, first, moving))
        .unwrap();
    harness.move_beside(moving, target, SplitDirection::Right);
    harness.wait_for("the moved pane's view to join the other tab", |cx| {
        panes_shown(cx, window, second)
            .is_some_and(|(panes, _)| panes == [target, moving])
            && pane_view(cx, window, first, moving).is_none()
    });
    // The same view and viewer: no reinstall, no new subscription.
    assert_eq!(
        harness.cx.update(|cx| identity(cx, window, second, moving)),
        Some(before)
    );
    assert_eq!(
        harness.cx.update(|cx| panes_shown(cx, window, first)),
        Some((vec![kept], kept))
    );
    // Its earlier output is still there, and its shell still answers.
    harness.focus(window, second, moving);
    harness.type_line(window, "moved");
    harness.wait_for("the moved pane to read the line", |cx| {
        pane_screen(cx, window, second, moving).is_some_and(|text| {
            text.contains("READY") && text.contains("got:moved")
        })
    });
}

#[gpui::test]
fn move_commands_show_and_focus_the_moved_pane(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.activate(window, first);
    let kept = harness.first_pane(first);
    let moving = harness.split(first, kept, SplitDirection::Right, ECHO);
    harness.wait_for("the second pane to show", |cx| {
        pane_screen(cx, window, first, moving)
            .is_some_and(|text| text.contains("READY"))
    });
    harness.focus(window, first, moving);
    let before = harness
        .cx
        .update(|cx| identity(cx, window, first, moving))
        .unwrap();
    // Extraction opens a tab after the source, shows it, and focuses the
    // pane, whose view and viewer carry over.
    harness.invoke(window, ids::MOVE_PANE_TO_NEW_TAB).unwrap();
    harness.wait_for("the extracted pane to show in its own tab", |cx| {
        active(cx, window).is_some_and(|tab| {
            tab != first
                && panes_shown(cx, window, tab) == Some((vec![moving], moving))
        })
    });
    let extracted = harness.cx.update(|cx| active(cx, window)).unwrap();
    assert_eq!(
        harness
            .cx
            .update(|cx| identity(cx, window, extracted, moving)),
        Some(before)
    );
    harness.wait_for("the extracted pane to take keyboard focus", |cx| {
        pane_focused(cx, window, extracted, moving)
    });
    harness.type_line(window, "out");
    harness.wait_for("the extracted pane to read the line", |cx| {
        pane_screen(cx, window, extracted, moving)
            .is_some_and(|text| text.contains("got:out"))
    });
    assert_eq!(
        harness.update_view(window, |view, _, _| view
            .tabs
            .iter()
            .map(|tab| tab.id)
            .collect::<Vec<_>>()),
        [first, extracted]
    );
    // The lone pane of the first tab moves beside it by explicit pane and
    // side; the emptied tab goes and the window follows the pane.
    harness.focus(window, first, kept);
    harness
        .invoke_with(
            window,
            &CommandInvocation::new(
                ids::MOVE_PANE_TO_PANE,
                vec![
                    CommandArgument::new("pane", CommandValue::Pane(moving)),
                    CommandArgument::new(
                        "side",
                        CommandValue::Text("down".to_owned()),
                    ),
                ],
            ),
        )
        .unwrap();
    harness.wait_for("the first tab to go and its pane to join", |cx| {
        !projected(cx, first)
            && active(cx, window) == Some(extracted)
            && panes_shown(cx, window, extracted)
                == Some((vec![moving, kept], kept))
    });
    harness.wait_for("the moved pane to take keyboard focus", |cx| {
        pane_focused(cx, window, extracted, kept)
    });
    harness.cx.update(|cx| {
        assert_eq!(notices(cx, window), Vec::<String>::new());
    });
}

#[gpui::test]
fn a_swap_reorders_the_views_and_focus_falls_back_in_the_new_order(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.activate(window, tab);
    let left = harness.first_pane(tab);
    let middle = harness.split(tab, left, SplitDirection::Right, ECHO);
    let right = harness.split(tab, middle, SplitDirection::Right, ECHO);
    harness.wait_for("three panes to install", |cx| {
        panes_shown(cx, window, tab).is_some_and(|(panes, _)| panes.len() == 3)
    });
    let views: Vec<_> = harness.cx.update(|cx| {
        [left, middle, right]
            .map(|pane| identity(cx, window, tab, pane).unwrap())
            .to_vec()
    });
    harness.focus(window, tab, left);
    harness.invoke(window, ids::SWAP_PANE_NEXT).unwrap();
    harness.wait_for("the views to follow the swapped layout", |cx| {
        panes_shown(cx, window, tab) == Some((vec![middle, left, right], left))
    });
    // The same views, only reordered.
    assert_eq!(
        harness.cx.update(|cx| {
            [left, middle, right]
                .map(|pane| identity(cx, window, tab, pane).unwrap())
                .to_vec()
        }),
        views
    );
    // With no other pane in this window's history, losing the focused pane
    // falls back to the first pane in the new order.
    harness.update_view(window, |view, _, _| {
        if let Some(view) = view.tabs.iter_mut().find(|view| view.id == tab) {
            view.focus_history = vec![left];
        }
    });
    harness
        .runtime
        .lock()
        .close_pane(harness.workspace, tab, left)
        .unwrap();
    harness.wait_for("focus to fall back to the new first pane", |cx| {
        panes_shown(cx, window, tab) == Some((vec![middle, right], middle))
    });
}

#[gpui::test]
fn a_pane_close_taken_before_a_move_closes_the_pane_in_its_new_tab(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let source = harness.open_tab(RUNS_JOB);
    let destination = harness.open_tab(ECHO);
    let window = harness.attach_window();
    let busy_pane = harness.first_pane(source);
    let target = harness.first_pane(destination);
    harness.wait_for("both tabs to install", |cx| {
        pane_screen(cx, window, source, busy_pane)
            .is_some_and(|text| text.contains("READY"))
            && pane_view(cx, window, destination, target).is_some()
    });
    harness.focus(window, source, busy_pane);
    // The close is taken, then held before its assessment while another
    // client moves the pane, the source tab's last, into the other tab.
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Assess);
    harness.invoke(window, ids::CLOSE_PANE).unwrap();
    harness.wait_for("the close to wait before its assessment", |_| {
        runtime.worker_hooks.held(Stage::Assess) > 0
    });
    harness.move_beside(busy_pane, target, SplitDirection::Right);
    runtime.worker_hooks.release(Stage::Assess);
    // The stale assessment finds the source tab gone; the close follows the
    // pane and asks about its job there, with no failure notice.
    harness.wait_for("the close to ask about the moved pane", |cx| {
        confirming(cx, window)
            && panes_shown(cx, window, destination)
                .is_some_and(|(panes, _)| panes.contains(&busy_pane))
    });
    harness.cx.update(|cx| {
        assert_eq!(notices(cx, window), Vec::<String>::new());
    });
    harness.update_view(window, |view, window, cx| {
        let target = view.confirming_target().unwrap();
        view.finish_close(target, window, cx);
    });
    harness.wait_for("the moved pane to close in its new tab", |cx| {
        panes_shown(cx, window, destination) == Some((vec![target], target))
    });
}

#[gpui::test]
fn a_move_that_would_leave_a_pane_under_the_minimum_is_refused(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let second = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.activate(window, first);
    let moving = harness.first_pane(first);
    let target = harness.first_pane(second);
    harness.wait_for("both panes to show", |cx| {
        pane_view(cx, window, first, moving).is_some()
            && pane_view(cx, window, second, target).is_some()
    });
    // Six rows: stacking a pane on the target gives both title bars and
    // leaves each half under two rows.
    let (chrome, cell) = harness.update_view(window, |view, window, _| {
        let bounds = view.terminal_geometry(window).bounds.unwrap();
        (
            window.viewport_size().height - bounds.size.height,
            view.metrics.at_scale(window.scale_factor()).cell_height,
        )
    });
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_resize(gpui::size(px(800.0), chrome + cell * 6.5));
    harness.wait_for("the pane to request six rows", |cx| {
        pane_view(cx, window, first, moving)
            .is_some_and(|view| view.read(cx).requested_grid.rows == 6)
    });
    let seq = harness
        .cx
        .update(|cx| cx.global::<Desktop>().hierarchy.state().seq());
    let refused = harness.invoke_with(
        window,
        &CommandInvocation::new(
            ids::MOVE_PANE_TO_PANE,
            vec![
                CommandArgument::new("pane", CommandValue::Pane(target)),
                CommandArgument::new("side", CommandValue::Text("down".into())),
            ],
        ),
    );
    assert!(
        matches!(
            &refused,
            Err(CommandError::Unavailable(reason)) if reason.contains("too small")
        ),
        "{refused:?}"
    );
    // Beside it fits, so the same move to the right runs.
    harness
        .invoke_with(
            window,
            &CommandInvocation::new(
                ids::MOVE_PANE_TO_PANE,
                vec![
                    CommandArgument::new("pane", CommandValue::Pane(target)),
                    CommandArgument::new(
                        "side",
                        CommandValue::Text("right".into()),
                    ),
                ],
            ),
        )
        .unwrap();
    harness.wait_for("the pane to join the other tab", |cx| {
        cx.global::<Desktop>().hierarchy.state().seq() > seq
            && panes_shown(cx, window, second)
                .is_some_and(|(panes, _)| panes == [target, moving])
    });
}

#[gpui::test]
fn an_adopted_pane_takes_its_new_tab_s_visibility(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let shown = harness.open_tab(ECHO);
    let hidden = harness.open_tab(ECHO);
    let other = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.activate(window, shown);
    let kept = harness.first_pane(shown);
    let arriving = harness.first_pane(hidden);
    let target = harness.first_pane(other);
    harness.wait_for("every tab to install", |cx| {
        pane_view(cx, window, hidden, arriving).is_some()
            && pane_view(cx, window, other, target).is_some()
    });
    harness.focus(window, shown, kept);
    assert_eq!(
        harness
            .cx
            .update(|cx| pane_shown(cx, window, hidden, arriving))
            .map(|(visible, _)| visible),
        Some(false)
    );
    // Another client moves a hidden pane into the active tab. The active
    // tab and its focused pane stay, yet the arriving view must show.
    harness.move_beside(arriving, kept, SplitDirection::Right);
    harness.wait_for("the arriving pane to show and draw", |cx| {
        pane_shown(cx, window, shown, arriving)
            .is_some_and(|(visible, _)| visible)
            && pane_screen(cx, window, shown, arriving)
                .is_some_and(|text| text.contains("READY"))
    });
    // Moved on into a hidden tab, the same view hides again.
    harness.move_beside(arriving, target, SplitDirection::Right);
    harness.wait_for("the departed pane to hide", |cx| {
        pane_shown(cx, window, other, arriving)
            .is_some_and(|(visible, _)| !visible)
    });
    harness.cx.update(|cx| {
        assert_eq!(active(cx, window), Some(shown));
        assert!(pane_focused(cx, window, shown, kept));
    });
}

#[gpui::test]
fn a_swap_and_a_close_in_one_batch_fall_back_in_the_new_order(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.activate(window, tab);
    let left = harness.first_pane(tab);
    let middle = harness.split(tab, left, SplitDirection::Right, ECHO);
    let right = harness.split(tab, middle, SplitDirection::Right, ECHO);
    harness.wait_for("three panes to install", |cx| {
        panes_shown(cx, window, tab).is_some_and(|(panes, _)| panes.len() == 3)
    });
    harness.update_view(window, |view, _, _| {
        if let Some(view) = view.tabs.iter_mut().find(|view| view.id == tab) {
            view.focus_history = vec![left];
        }
    });
    // Both land in one drain: the focused pane closes as the other two
    // trade places, so focus falls back to the first pane after the swap.
    {
        let mut mux = harness.runtime.lock();
        mux.swap_panes(harness.workspace, tab, middle, right)
            .unwrap();
        mux.close_pane(harness.workspace, tab, left).unwrap();
    }
    harness.wait_for("focus to fall back to the new first pane", |cx| {
        panes_shown(cx, window, tab) == Some((vec![right, middle], right))
    });
}

#[gpui::test]
fn a_pane_close_whose_pane_leaves_the_window_ends_without_a_notice(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let source = harness.open_tab(RUNS_JOB);
    let staying = harness.open_tab(ECHO);
    let window = harness.attach_window();
    let leaving = harness.first_pane(source);
    let stays = harness.first_pane(staying);
    harness.wait_for("both tabs to install", |cx| {
        pane_screen(cx, window, source, leaving)
            .is_some_and(|text| text.contains("READY"))
            && pane_view(cx, window, staying, stays).is_some()
    });
    let elsewhere = harness
        .runtime
        .lock()
        .create_workspace(harness.session, None)
        .unwrap();
    harness.focus(window, source, leaving);
    // The close is held before its assessment while another client moves
    // the pane into a workspace this window does not show.
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Assess);
    harness.invoke(window, ids::CLOSE_PANE).unwrap();
    harness.wait_for("the close to wait before its assessment", |_| {
        runtime.worker_hooks.held(Stage::Assess) > 0
    });
    let from = harness.pane_ref(leaving);
    harness
        .runtime
        .lock()
        .move_pane(
            from,
            MoveDestination::NewTab {
                workspace: elsewhere,
                before: None,
            },
        )
        .unwrap();
    runtime.worker_hooks.release(Stage::Assess);
    // The close never reached Mux; it ends quietly and the pane lives on.
    // A tab's only pane takes the tab with it, so the window drops the tab.
    harness.wait_for("the close to end and the source tab to go", |cx| {
        panes_shown(cx, window, source).is_none()
            && active(cx, window) == Some(staying)
            && window
                .update(cx, |root, _, cx| {
                    let view = root.downcast::<WorkspaceView>().unwrap();
                    let view = view.read(cx);
                    !view.busy && view.close.current.is_none()
                })
                .unwrap()
    });
    harness.cx.update(|cx| {
        assert!(!confirming(cx, window));
        assert_eq!(notices(cx, window), Vec::<String>::new());
        let state = cx.global::<Desktop>().hierarchy.state();
        assert!(state.pane_tab(leaving).is_some());
    });
}

#[gpui::test]
fn a_tab_closed_after_its_pane_moved_out_keeps_the_moved_view(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let closing = harness.open_tab(ECHO);
    let receiving = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.activate(window, closing);
    let gone = harness.first_pane(closing);
    let moving = harness.split(closing, gone, SplitDirection::Right, ECHO);
    let target = harness.first_pane(receiving);
    harness.wait_for("every pane to install", |cx| {
        pane_screen(cx, window, closing, moving)
            .is_some_and(|text| text.contains("READY"))
            && pane_view(cx, window, receiving, target).is_some()
    });
    let before = harness
        .cx
        .update(|cx| identity(cx, window, closing, moving))
        .unwrap();
    // The tab close is held before its assessment, so the window stays
    // busy and defers reconcile, while another client moves a pane out.
    // The assessment then covers only the pane left behind, and the window
    // adopts the moved view before the close commits.
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Assess);
    harness.invoke(window, ids::CLOSE_TAB).unwrap();
    harness.wait_for("the close to wait before its assessment", |_| {
        runtime.worker_hooks.held(Stage::Assess) > 0
    });
    harness.move_beside(moving, target, SplitDirection::Right);
    runtime.worker_hooks.release(Stage::Assess);
    harness.wait_for("the tab to close and the moved pane to join", |cx| {
        !projected(cx, closing)
            && panes_shown(cx, window, receiving)
                .is_some_and(|(panes, _)| panes == [target, moving])
    });
    assert_eq!(
        harness
            .cx
            .update(|cx| identity(cx, window, receiving, moving)),
        Some(before)
    );
}
