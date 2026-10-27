//! Tier 2 tests for layout undo: a tab drop undone and redone with every
//! view kept, a refusal's notice, the palette blocking undo, and pruning
//! in a window that does not show the changed tab.

use huterm_core::RestoreDirection;

use super::pane_moves::{active, identity, pane_focused};
use super::*;

/// The label `window` would undo or redo next.
fn next_label(
    cx: &mut App,
    window: AnyWindowHandle,
    direction: RestoreDirection,
) -> Option<&'static str> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx).layout_restore_label(direction, cx)
        })
        .ok()
        .flatten()
}

/// The tabs `window` shows, in order.
fn tab_order(cx: &mut App, window: AnyWindowHandle) -> Vec<TabId> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().unwrap();
            view.read(cx).tabs.iter().map(|tab| tab.id).collect()
        })
        .unwrap_or_default()
}

fn drop_tab(tab: TabId, pane: PaneId) -> CommandInvocation {
    CommandInvocation::new(
        ids::MOVE_TAB_TO_PANE,
        vec![
            CommandArgument::new("tab", CommandValue::Tab(tab)),
            CommandArgument::new("pane", CommandValue::Pane(pane)),
            CommandArgument::new(
                "side",
                CommandValue::Text("right".to_owned()),
            ),
        ],
    )
}

#[gpui::test]
fn undoing_a_tab_drop_reopens_the_tab_with_its_views(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let dropped = harness.open_tab(ECHO);
    let target = harness.open_tab(ECHO);
    harness
        .runtime
        .lock()
        .rename_tab(dropped, Some("dropped"))
        .unwrap();
    let left = harness.first_pane(dropped);
    let right = harness.split(dropped, left, SplitDirection::Right, ECHO);
    let lone = harness.first_pane(target);
    let window = harness.attach_window();
    harness.activate(window, first);
    harness.focus(window, dropped, right);
    harness.wait_for("the dropped tab's panes to show", |cx| {
        [left, right].iter().all(|pane| {
            pane_screen(cx, window, dropped, *pane)
                .is_some_and(|text| text.contains("READY"))
        })
    });
    let views = harness.cx.update(|cx| {
        [left, right].map(|pane| identity(cx, window, dropped, pane).unwrap())
    });
    harness.focus(window, target, lone);
    harness
        .invoke_with(window, &drop_tab(dropped, lone))
        .unwrap();
    harness.wait_for("the tab to nest into the target", |cx| {
        !projected(cx, dropped)
            && panes_shown(cx, window, target)
                .is_some_and(|(panes, _)| panes == [lone, left, right])
    });
    harness.wait_for("the drop to become undoable", |cx| {
        next_label(cx, window, RestoreDirection::Undo)
            == Some("Move Tab into Pane")
    });

    // Undo reopens the tab with its identity, name, and place, and shows
    // and focuses the pane it had focused, through the same views.
    harness.invoke(window, ids::UNDO).unwrap();
    // A second press while the first runs does nothing, without a notice.
    assert!(matches!(
        harness.invoke(window, ids::UNDO),
        Ok(CommandOutcome::Completed)
    ));
    harness.wait_for("the dropped tab to come back", |cx| {
        tab_order(cx, window) == [first, dropped, target]
            && active(cx, window) == Some(dropped)
            && panes_shown(cx, window, dropped)
                .is_some_and(|(panes, _)| panes == [left, right])
            && panes_shown(cx, window, target)
                .is_some_and(|(panes, _)| panes == [lone])
    });
    harness.wait_for("the entry's pane to take keyboard focus", |cx| {
        pane_focused(cx, window, dropped, right)
    });
    assert_eq!(
        harness.cx.update(|cx| {
            [left, right]
                .map(|pane| identity(cx, window, dropped, pane).unwrap())
        }),
        views
    );
    assert_eq!(
        harness.runtime.lock().tab(dropped).unwrap().custom_name(),
        Some("dropped")
    );
    harness.cx.update(|cx| {
        assert_eq!(next_label(cx, window, RestoreDirection::Undo), None);
        assert_eq!(
            next_label(cx, window, RestoreDirection::Redo),
            Some("Move Tab into Pane")
        );
    });
    harness.type_line(window, "undone");
    harness.wait_for("the focused shell to read the line", |cx| {
        pane_screen(cx, window, dropped, right)
            .is_some_and(|text| text.contains("got:undone"))
    });

    // Redo nests it again, and the entry goes back to undo.
    harness.invoke(window, ids::REDO).unwrap();
    harness.wait_for("the tab to nest again", |cx| {
        !projected(cx, dropped)
            && panes_shown(cx, window, target)
                .is_some_and(|(panes, _)| panes == [lone, left, right])
            && next_label(cx, window, RestoreDirection::Undo).is_some()
    });
    harness.cx.update(|cx| {
        assert_eq!(next_label(cx, window, RestoreDirection::Redo), None);
        assert_eq!(notices(cx, window), Vec::<String>::new());
    });
}

/// A window showing a tab of two panes that it has just swapped.
fn swapped(
    harness: &mut DesktopHarness,
) -> (AnyWindowHandle, TabId, PaneId, PaneId) {
    let tab = harness.open_tab(ECHO);
    let left = harness.first_pane(tab);
    let right = harness.split(tab, left, SplitDirection::Right, ECHO);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_for("both panes to show", |cx| {
        panes_shown(cx, window, tab).is_some_and(|(panes, _)| panes.len() == 2)
    });
    harness.focus(window, tab, left);
    harness.invoke(window, ids::SWAP_PANE_NEXT).unwrap();
    harness.wait_for("the swap to become undoable", |cx| {
        next_label(cx, window, RestoreDirection::Undo) == Some("Swap Panes")
    });
    (window, tab, left, right)
}

#[gpui::test]
fn an_undo_the_runtime_refuses_shows_one_notice_and_drops_the_entry(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, left, right) = swapped(&mut harness);
    let workspace = harness.workspace;
    let runtime = Arc::clone(&harness.runtime);
    // Another client swaps the panes back before this window's projection
    // sees it, so the window still offers the undo and the runtime refuses.
    let outcome = harness.update_view(window, |view, window, cx| {
        runtime
            .lock()
            .swap_panes(workspace, tab, left, right)
            .unwrap();
        view.invoke_interactive(
            &CommandInvocation::new(ids::UNDO, vec![]),
            window,
            cx,
        )
    });
    assert!(matches!(outcome, Ok(CommandOutcome::Accepted)));
    harness.wait_for("the refusal's notice", |cx| {
        !notices(cx, window).is_empty() && !busy(cx, window)
    });
    harness.cx.update(|cx| {
        let shown = notices(cx, window);
        assert_eq!(shown.len(), 1, "{shown:?}");
        assert!(
            shown[0].contains("Cannot undo: the layout changed"),
            "{shown:?}"
        );
        assert_eq!(next_label(cx, window, RestoreDirection::Undo), None);
        assert_eq!(next_label(cx, window, RestoreDirection::Redo), None);
    });
    // Nothing is left to undo, so the key does nothing.
    assert!(matches!(
        harness.invoke(window, ids::UNDO),
        Ok(CommandOutcome::Completed)
    ));
}

/// The platform's default undo key.
const UNDO_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-z"
} else {
    "ctrl-shift-z"
};

/// What `window`'s open palette says about `undo`: its title override and
/// why it is unavailable.
fn palette_undo(
    cx: &mut App,
    window: AnyWindowHandle,
) -> Option<(Option<String>, Option<String>)> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            let palette = view.read(cx).palette.clone()?;
            let palette = palette.read(cx);
            Some((
                palette.titles.get(&ids::UNDO).cloned(),
                palette.availability.get(&ids::UNDO).cloned(),
            ))
        })
        .ok()
        .flatten()
}

#[gpui::test]
fn the_undo_key_in_the_palette_does_nothing_and_the_palette_follows_pruning(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, left, right) = swapped(&mut harness);
    let layout = harness.runtime.lock().tab(tab).unwrap().layout().clone();
    harness.invoke(window, ids::OPEN_COMMAND_PALETTE).unwrap();
    harness.wait_for("the palette to offer the undo", |cx| {
        palette_undo(cx, window)
            == Some((Some("Undo Swap Panes".to_owned()), None))
    });
    // The key reaches the window from the palette's search field, which has
    // no undo of its own: nothing changes and no notice appears.
    harness.press(window, UNDO_KEY);
    harness.cx.run_until_parked();
    assert_eq!(*harness.runtime.lock().tab(tab).unwrap().layout(), layout);
    harness.cx.update(|cx| {
        assert_eq!(notices(cx, window), Vec::<String>::new());
        assert_eq!(
            next_label(cx, window, RestoreDirection::Undo),
            Some("Swap Panes")
        );
    });
    // Another client swaps the panes back, which prunes the entry, and the
    // open palette stops offering it.
    let workspace = harness.workspace;
    harness
        .runtime
        .lock()
        .swap_panes(workspace, tab, left, right)
        .unwrap();
    harness.wait_for("the palette to drop the undo", |cx| {
        palette_undo(cx, window)
            == Some((None, Some("nothing to undo".to_owned())))
    });
}

#[gpui::test]
fn undo_focuses_a_pane_whose_view_has_to_install_again(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let kept = harness.first_pane(tab);
    let moving = harness.split(tab, kept, SplitDirection::Right, ECHO);
    let elsewhere = harness
        .runtime
        .lock()
        .create_workspace(harness.session, None)
        .unwrap();
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_for("both panes to show", |cx| {
        panes_shown(cx, window, tab).is_some_and(|(panes, _)| panes.len() == 2)
    });
    harness.focus(window, tab, moving);
    // The pane leaves for a workspace this window does not show, so the
    // window drops its view.
    harness
        .invoke_with(
            window,
            &CommandInvocation::new(
                ids::MOVE_PANE_TO_NEW_TAB,
                vec![CommandArgument::new(
                    "workspace",
                    CommandValue::Workspace(elsewhere),
                )],
            ),
        )
        .unwrap();
    harness.wait_for("the pane to leave and the move to record", |cx| {
        panes_shown(cx, window, tab) == Some((vec![kept], kept))
            && next_label(cx, window, RestoreDirection::Undo).is_some()
    });
    // Undo brings it back through a new view, which takes the focus once
    // it installs.
    harness.invoke(window, ids::UNDO).unwrap();
    harness.wait_for("the returned pane to take keyboard focus", |cx| {
        panes_shown(cx, window, tab)
            .is_some_and(|(panes, _)| panes == [kept, moving])
            && pane_focused(cx, window, tab, moving)
    });
    harness.type_line(window, "back");
    harness.wait_for("the returned pane to read the line", |cx| {
        pane_screen(cx, window, tab, moving)
            .is_some_and(|text| text.contains("got:back"))
    });
}

#[gpui::test]
fn a_change_to_a_tab_the_window_does_not_show_prunes_its_entry(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let kept = harness.first_pane(tab);
    let moving = harness.split(tab, kept, SplitDirection::Right, ECHO);
    let elsewhere = harness
        .runtime
        .lock()
        .create_workspace(harness.session, None)
        .unwrap();
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_for("both panes to show", |cx| {
        panes_shown(cx, window, tab).is_some_and(|(panes, _)| panes.len() == 2)
    });
    harness.focus(window, tab, moving);
    // The pane moves to a new tab in a workspace this window does not show.
    harness
        .invoke_with(
            window,
            &CommandInvocation::new(
                ids::MOVE_PANE_TO_NEW_TAB,
                vec![CommandArgument::new(
                    "workspace",
                    CommandValue::Workspace(elsewhere),
                )],
            ),
        )
        .unwrap();
    harness.wait_for("the move to become undoable", |cx| {
        next_label(cx, window, RestoreDirection::Undo) == Some("Move Pane")
    });
    let moved = harness.runtime.lock().select_pane(moving).unwrap().tab;
    let moved = moved.unwrap();
    harness.cx.update(|cx| {
        assert!(!tab_order(cx, window).contains(&moved));
    });
    // Another client splits that tab, so undoing the move would now be
    // refused, and the entry goes even though this window never shows it.
    harness
        .runtime
        .lock()
        .split_pane(
            elsewhere,
            moved,
            moving,
            SplitDirection::Down,
            &fixture(ECHO),
        )
        .unwrap();
    harness.wait_for("the entry to be pruned", |cx| {
        next_label(cx, window, RestoreDirection::Undo).is_none()
    });
    harness.update_view(window, |view, _, cx| {
        assert_eq!(
            view.window_menu_input(cx).restorable,
            window_menu::Restorable::default()
        );
    });
}
