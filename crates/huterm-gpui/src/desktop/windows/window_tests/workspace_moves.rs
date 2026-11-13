//! Tier 2 tests for the keyboard and palette equivalents of the workspace
//! drags: Move Tab to Workspace, Move Workspace to Session, Move Workspace
//! Up and Down with their sidebar keys, and the pane views a move between
//! workspaces of one window keeps.

use super::super::worker_hooks::Stage;
use super::pane_drags::active;
use super::workspaces::{shown_workspace, workspace_pick};
use super::*;

/// The workspaces of `session` in order, from the projection.
fn order(cx: &mut App, session: SessionId) -> Vec<WorkspaceId> {
    cx.global::<Desktop>()
        .hierarchy
        .state()
        .session_workspaces(session)
        .unwrap_or_default()
        .to_vec()
}

/// The tabs the projection places in `workspace`.
fn tabs_in(cx: &mut App, workspace: WorkspaceId) -> Vec<TabId> {
    cx.global::<Desktop>()
        .hierarchy
        .state()
        .workspace_tabs(workspace)
        .unwrap_or_default()
        .to_vec()
}

/// The panes of `tab` in the runtime, in order.
fn panes_of(h: &DesktopHarness, tab: TabId) -> Vec<PaneId> {
    h.runtime.lock().tab(tab).map_or_else(Vec::new, |tab| {
        tab.layout().panes().iter().map(|pane| pane.id).collect()
    })
}

/// The entity of the view `window` holds for `pane`, shown or hidden.
fn pane_view(
    cx: &mut App,
    window: AnyWindowHandle,
    pane: PaneId,
) -> Option<gpui::EntityId> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx)
                .all_tab_views()
                .find_map(|tab| tab.pane(pane))
                .map(|pane| pane.view.entity_id())
        })
        .ok()
        .flatten()
}

/// The workspace whose views `window` holds `tab` among, shown or hidden.
fn holding_workspace(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
) -> Option<WorkspaceId> {
    let shown = shown_workspace(cx, window);
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            let view = view.read(cx);
            if view.tabs.iter().any(|view| view.id == tab) {
                return shown;
            }
            view.hidden_workspace_of(tab)
        })
        .ok()
        .flatten()
}

fn tab_to(tab: TabId, workspace: WorkspaceId) -> CommandInvocation {
    CommandInvocation::new(
        ids::MOVE_TAB_TO_WORKSPACE,
        vec![
            CommandArgument::new("tab", CommandValue::Tab(tab)),
            CommandArgument::new(
                "workspace",
                CommandValue::Workspace(workspace),
            ),
        ],
    )
}

fn workspace_to(
    workspace: Option<WorkspaceId>,
    session: SessionId,
) -> CommandInvocation {
    let mut args = vec![CommandArgument::new(
        "session",
        CommandValue::Session(session),
    )];
    if let Some(workspace) = workspace {
        args.push(CommandArgument::new(
            "workspace",
            CommandValue::Workspace(workspace),
        ));
    }
    CommandInvocation::new(ids::MOVE_WORKSPACE_TO_SESSION, args)
}

fn unavailable<T: std::fmt::Debug>(result: &Result<T, CommandError>) {
    assert!(
        matches!(result, Err(CommandError::Unavailable(_))),
        "expected unavailable, got {result:?}"
    );
}

fn sidebar_harness(cx: &mut TestAppContext) -> DesktopHarness {
    DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
        config.tabs.always_show = true;
    })
}

#[gpui::test]
fn a_tab_moved_to_a_hidden_workspace_keeps_its_view_and_layout(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let other = h.open_tab(ECHO);
    let (second, there) = h.add_workspace(1);
    let (window, left, right) = h.split_window(tab);
    h.wait_for("every view to install", |cx| {
        super::hidden_workspaces::hidden_tabs(cx, window, second)
            == Some(there.clone())
            && pane_view(cx, window, right).is_some()
    });
    let views = h.cx.update(|cx| {
        [left, right].map(|pane| pane_view(cx, window, pane).unwrap())
    });
    h.invoke_with(window, &tab_to(tab, second)).unwrap();
    let own = h.workspace;
    h.wait_for("the tab to move to the end of the workspace", |cx| {
        tabs_in(cx, second) == [there[0], tab]
            && holding_workspace(cx, window, tab) == Some(second)
            && !busy(cx, window)
    });
    assert_eq!(panes_of(&h, tab), [left, right], "the layout changed");
    h.cx.update(|cx| {
        assert_eq!(
            [left, right].map(|pane| pane_view(cx, window, pane).unwrap()),
            views,
            "the moved tab's views were rebuilt"
        );
        // The window stays on its workspace, on the tab that is left.
        assert_eq!(shown_workspace(cx, window), Some(own));
        assert_eq!(active(cx, window), Some(other));
    });
}

#[gpui::test]
fn a_tab_moved_to_another_sessions_workspace_installs_there(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let moving = h.open_tab(ECHO);
    let source = h.attach_showing(&[tab, moving]);
    let (destination, shown, anchor) = h.private_window();
    h.invoke_with(source, &tab_to(moving, shown)).unwrap();
    h.wait_for("the tab to show in the other window's workspace", |cx| {
        tabs_in(cx, shown) == [anchor, moving]
            && terminal(cx, destination, moving).is_some()
            && holding_workspace(cx, source, moving).is_none()
            && !busy(cx, source)
    });
    h.cx.update(|cx| {
        assert_eq!(active(cx, destination), Some(anchor));
        assert_eq!(active(cx, source), Some(tab));
    });
}

#[gpui::test]
fn a_tab_move_that_empties_the_shown_workspace_follows_unless_pinned(
    cx: &mut TestAppContext,
) {
    for pinned in [false, true] {
        let mut h = sidebar_harness(cx);
        let tab = h.open_tab(ECHO);
        let (second, there) = h.add_workspace(1);
        let window = h.attach_showing(&[tab]);
        let origin = h.workspace;
        if pinned {
            h.runtime.lock().set_workspace_pinned(origin, true).unwrap();
            h.wait_projection();
        }
        h.invoke_with(window, &tab_to(tab, second)).unwrap();
        let session = h.session;
        h.wait_for("the tab to move and the origin to settle", |cx| {
            tabs_in(cx, second) == [there[0], tab]
                && order(cx, session).contains(&origin) == pinned
                && !busy(cx, window)
        });
        let expected = if pinned { origin } else { second };
        h.wait_for("the window to settle", |cx| {
            shown_workspace(cx, window) == Some(expected)
        });
    }
}

#[gpui::test]
fn tab_moves_refuse_their_own_workspace_a_gone_one_and_a_busy_window(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let window = h.attach_showing(&[tab]);
    // With one workspace anywhere there is nowhere to go.
    unavailable(&h.update_view(window, |view, _, cx| {
        view.check_tab_to_workspace_available(None, None, cx)
    }));
    let (second, _) = h.add_workspace(1);
    h.wait_projection();
    let own = h.workspace;
    unavailable(&h.invoke_with(window, &tab_to(tab, own)));
    h.update_view(window, |view, _, _| view.busy = true);
    unavailable(&h.invoke_with(window, &tab_to(tab, second)));
    h.update_view(window, |view, _, _| view.busy = false);
    h.runtime.lock().close_workspace(second).unwrap();
    h.wait_projection();
    assert!(matches!(
        h.invoke_with(window, &tab_to(tab, second)),
        Err(CommandError::StaleTarget)
    ));
    assert_eq!(h.cx.update(|cx| tabs_in(cx, own)), [tab]);
}

#[gpui::test]
fn a_workspace_moved_to_another_session_leaves_every_window_showing_it(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let (second, _) = h.add_workspace(1);
    let source = h.attach_showing(&[tab]);
    let mirror = h.attach_showing(&[tab]);
    let (destination, shown, anchor) = h.private_window();
    let target = h.cx.update(|cx| {
        cx.global::<Desktop>()
            .hierarchy
            .state()
            .workspace_session(shown)
            .unwrap()
    });
    let moved = h.workspace;
    h.invoke_with(source, &workspace_to(None, target)).unwrap();
    let session = h.session;
    h.wait_for("the workspace to move last into that session", |cx| {
        order(cx, target) == [shown, moved] && order(cx, session) == [second]
    });
    h.wait_for("both windows that showed it to follow", |cx| {
        shown_workspace(cx, source) == Some(second)
            && shown_workspace(cx, mirror) == Some(second)
    });
    // The destination's window holds it hidden and stays where it was.
    h.wait_for("the destination to hold it hidden", |cx| {
        super::hidden_workspaces::hidden_tabs(cx, destination, moved)
            == Some(vec![tab])
    });
    h.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, destination), Some(shown));
        assert_eq!(active(cx, destination), Some(anchor));
    });
    // Shown there, its view installs on the same tab.
    h.invoke_with(destination, &workspace_pick(ids::SELECT_WORKSPACE, moved))
        .unwrap();
    h.wait_for("the destination to show it", |cx| {
        shown_workspace(cx, destination) == Some(moved)
            && terminal(cx, destination, tab).is_some()
    });
}

#[gpui::test]
fn workspace_moves_to_sessions_refuse_their_own_session_and_a_gone_one(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let window = h.attach_showing(&[tab]);
    let own = h.session;
    // No other session to move it to.
    unavailable(&h.update_view(window, |view, _, cx| {
        view.check_workspace_to_session_available(None, None, cx)
    }));
    let other = h.other_session("other", 1);
    let (second, _) = h.add_workspace(1);
    h.wait_projection();
    unavailable(&h.invoke_with(window, &workspace_to(Some(second), own)));
    h.update_view(window, |view, _, _| view.busy = true);
    unavailable(&h.invoke_with(window, &workspace_to(None, other.session)));
    h.update_view(window, |view, _, _| view.busy = false);
    // A workspace of another session is no target for this window.
    assert!(matches!(
        h.invoke_with(
            window,
            &workspace_to(Some(other.workspace), other.session)
        ),
        Err(CommandError::StaleTarget)
    ));
    // A session that went is a stale destination.
    h.runtime.lock().close_session(other.session).unwrap();
    h.wait_projection();
    assert!(matches!(
        h.invoke_with(window, &workspace_to(Some(second), other.session)),
        Err(CommandError::StaleTarget)
    ));
    assert_eq!(h.cx.update(|cx| order(cx, own)), [h.workspace, second]);
}

#[gpui::test]
fn a_sessions_only_workspace_moves_and_its_emptied_window_closes(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let window = h.attach_showing(&[tab]);
    let (destination, shown, _) = h.private_window();
    let target = h.cx.update(|cx| {
        cx.global::<Desktop>()
            .hierarchy
            .state()
            .workspace_session(shown)
            .unwrap()
    });
    let moved = h.workspace;
    h.invoke_with(window, &workspace_to(None, target)).unwrap();
    h.wait_for("the workspace to move and its window to close", |cx| {
        order(cx, target) == [shown, moved] && !cx.windows().contains(&window)
    });
    h.wait_for("the destination to hold it hidden", |cx| {
        super::hidden_workspaces::hidden_tabs(cx, destination, moved)
            == Some(vec![tab])
    });
    assert!(h.runtime.lock().tab(tab).is_some(), "the moved tab went");
}

#[gpui::test]
fn workspaces_move_up_and_down_and_stop_at_the_ends(cx: &mut TestAppContext) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let (second, _) = h.add_workspace(1);
    let (third, _) = h.add_workspace(1);
    let window = h.attach_showing(&[tab]);
    let (session, first) = (h.session, h.workspace);
    // The window's own workspace by default.
    unavailable(&h.invoke(window, ids::MOVE_WORKSPACE_UP));
    h.invoke(window, ids::MOVE_WORKSPACE_DOWN).unwrap();
    h.wait_for("the first to move down", |cx| {
        order(cx, session) == [second, first, third]
    });
    // A named one, as the row menu's items name theirs.
    h.invoke_with(window, &workspace_pick(ids::MOVE_WORKSPACE_UP, third))
        .unwrap();
    h.wait_for("the third to move up", |cx| {
        order(cx, session) == [second, third, first]
    });
    unavailable(&h.invoke(window, ids::MOVE_WORKSPACE_DOWN));
    h.update_view(window, |view, window, cx| {
        let raw = second.get();
        view.run_workspace_pick(&format!("workspace-down:{raw}"), window, cx);
    });
    h.wait_for("the row menu's pick to move it", |cx| {
        order(cx, session) == [third, second, first]
    });
    h.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, window), Some(first));
    });
}

#[gpui::test]
fn the_sidebar_keys_move_the_focused_row_and_keep_its_focus(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let (second, _) = h.add_workspace(1);
    let window = h.attach_showing(&[tab]);
    h.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    h.row_bounds(window, second);
    let (session, first) = (h.session, h.workspace);
    let focused_row = |h: &mut DesktopHarness| {
        h.update_view(window, |view, window, _| {
            view.focused_workspace_row(window)
        })
    };
    h.invoke(window, ids::FOCUS_SIDEBAR).unwrap();
    // At the top, Move Up does nothing and keeps focus.
    h.press(window, "alt-up");
    h.cx.run_until_parked();
    assert_eq!(h.cx.update(|cx| order(cx, session)), [first, second]);
    assert_eq!(focused_row(&mut h), Some(first));
    h.press(window, "alt-down");
    h.wait_for("the focused row to move down", |cx| {
        order(cx, session) == [second, first]
    });
    assert_eq!(focused_row(&mut h), Some(first));
    h.press(window, "up alt-down");
    h.wait_for("the other row to move down", |cx| {
        order(cx, session) == [first, second]
    });
    assert_eq!(focused_row(&mut h), Some(second));
}

#[gpui::test]
fn the_session_list_keys_reorder_sessions(cx: &mut TestAppContext) {
    let mut h = DesktopHarness::new(cx);
    let tab = h.open_tab(ECHO);
    let window = h.attach_showing(&[tab]);
    let other = h.other_session("other", 1);
    h.wait_projection();
    let own = h.session;
    let sessions = |cx: &mut App| {
        cx.global::<Desktop>().hierarchy.state().sessions().to_vec()
    };
    assert_eq!(h.cx.update(sessions), [own, other.session]);
    h.invoke(window, ids::OPEN_SESSION_MENU).unwrap();
    h.wait_for("the window's session row to take focus", |cx| {
        window
            .update(cx, |root, window, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx).focused_session_row(window) == Some(own)
                })
            })
            .unwrap_or(false)
    });
    h.press(window, "alt-down");
    h.wait_for("the session to move down", |cx| {
        sessions(cx) == [other.session, own]
    });
    // The row menu's items move it too, and do nothing at the end.
    h.update_view(window, |view, window, cx| {
        let raw = own.get();
        view.run_session_pick(&format!("session-down:{raw}"), window, cx);
    });
    h.cx.run_until_parked();
    assert_eq!(h.cx.update(sessions), [other.session, own]);
    h.update_view(window, |view, window, cx| {
        let raw = own.get();
        view.run_session_pick(&format!("session-up:{raw}"), window, cx);
    });
    h.wait_for("the row menu's pick to move it back up", |cx| {
        sessions(cx) == [own, other.session]
    });
}

#[gpui::test]
fn a_pane_moved_to_another_workspace_of_the_window_keeps_its_view(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let (second, there) = h.add_workspace(1);
    let (window, left, right) = h.split_window(tab);
    h.wait_for("every view to install", |cx| {
        super::hidden_workspaces::hidden_tabs(cx, window, second)
            == Some(there.clone())
            && pane_view(cx, window, right).is_some()
    });
    let view = h.cx.update(|cx| pane_view(cx, window, right).unwrap());
    h.update_view(window, |view, window, cx| {
        view.focus_pane(tab, right, window, cx);
    });
    h.invoke_with(window, &workspace_pick(ids::MOVE_PANE_TO_NEW_TAB, second))
        .unwrap();
    h.wait_for("the pane to land in a new tab there", |cx| {
        tabs_in(cx, second).len() == 2 && !busy(cx, window)
    });
    let moved = h.cx.update(|cx| tabs_in(cx, second)[1]);
    assert_eq!(panes_of(&h, moved), [right]);
    assert_eq!(panes_of(&h, tab), [left]);
    h.cx.update(|cx| {
        assert_eq!(holding_workspace(cx, window, moved), Some(second));
        assert_eq!(
            pane_view(cx, window, right),
            Some(view),
            "the moved pane's view was rebuilt"
        );
    });
    // Back into the shown workspace, beside the pane it left.
    h.invoke_with(window, &workspace_pick(ids::SELECT_WORKSPACE, second))
        .unwrap();
    h.wait_for("the window to show the pane's workspace", |cx| {
        shown_workspace(cx, window) == Some(second)
    });
    h.update_view(window, |view, window, cx| view.select(moved, window, cx));
    h.wait_for("the moved pane's tab to show", |cx| {
        active(cx, window) == Some(moved)
    });
    h.invoke_with(
        window,
        &CommandInvocation::new(
            ids::MOVE_PANE_TO_PANE,
            vec![
                CommandArgument::new("pane", CommandValue::Pane(left)),
                CommandArgument::new(
                    "side",
                    CommandValue::Text("right".to_owned()),
                ),
            ],
        ),
    )
    .unwrap();
    h.wait_for("the pane to return beside the other", |cx| {
        tabs_in(cx, second) == there && !busy(cx, window)
    });
    assert_eq!(panes_of(&h, tab), [left, right]);
    h.cx.update(|cx| {
        assert_eq!(pane_view(cx, window, right), Some(view));
        assert_eq!(holding_workspace(cx, window, tab), Some(h.workspace));
    });
}

#[gpui::test]
fn a_destination_that_goes_while_a_tab_move_waits_refuses_it(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let other = h.open_tab(ECHO);
    let (second, _) = h.add_workspace(1);
    let window = h.attach_showing(&[tab, other]);
    let runtime = Arc::clone(&h.runtime);
    runtime.worker_hooks.hold(Stage::Move);
    h.invoke_with(window, &tab_to(other, second)).unwrap();
    h.wait_for("the move's worker to start", |_| {
        runtime.worker_hooks.held(Stage::Move) == 1
    });
    // The window is busy, so a second move waits its turn.
    unavailable(&h.invoke_with(window, &tab_to(tab, second)));
    runtime.lock().close_workspace(second).unwrap();
    runtime.worker_hooks.release(Stage::Move);
    let own = h.workspace;
    h.wait_for("the refused move to report and settle", |cx| {
        !busy(cx, window)
            && notices(cx, window)
                .iter()
                .any(|notice| notice.contains("Cannot move tab"))
    });
    h.cx.update(|cx| {
        assert_eq!(tabs_in(cx, own), [tab, other]);
        assert_eq!(shown_workspace(cx, window), Some(own));
    });
}

#[gpui::test]
fn quit_waits_for_a_workspace_move_and_saves_the_workspace_once(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let (second, _) = h.add_workspace(1);
    let window = h.attach_showing(&[tab]);
    let other = h.other_session("other", 1);
    h.wait_projection();
    let runtime = Arc::clone(&h.runtime);
    runtime.worker_hooks.hold(Stage::Move);
    h.invoke_with(window, &workspace_to(Some(second), other.session))
        .unwrap();
    h.wait_for("the move's worker to start", |_| {
        runtime.worker_hooks.held(Stage::Move) == 1
    });
    h.resume_quits();
    h.cx.update(request_quit);
    h.cx.run_until_parked();
    assert!(
        h.recorder.saved().is_empty(),
        "the Quit captured before the move settled"
    );
    runtime.worker_hooks.release(Stage::Move);
    h.wait_quit(h.session);
    let (captured, _) = h.recorder.saved().remove(0);
    let holding: Vec<_> = captured
        .sessions
        .iter()
        .filter(|session| {
            session
                .workspaces
                .iter()
                .any(|workspace| workspace.info.id.get() == second.get())
        })
        .collect();
    assert_eq!(holding.len(), 1, "saved {} times", holding.len());
    assert_eq!(
        holding[0].info.id.get(),
        other.session.get(),
        "saved outside the session it moved to"
    );
}
