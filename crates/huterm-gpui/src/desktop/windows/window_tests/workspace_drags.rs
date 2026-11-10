//! Tier 2 tests for workspace drags (D): tabs and panes dropped on and
//! spring-loading sidebar workspace rows, the row drag chip, a workspace
//! dropped on another window, and workspace tear-off, with the lifecycle
//! interleavings of `docs/plans/workspace-drags.md`.

use super::super::drag::{DropTarget, Payload, RowKey, SPRING_LOAD_DELAY};
use super::super::window_locator::Located;
use super::super::worker_hooks::Stage;
use super::pane_drags::{active, no_drag};
use super::workspaces::shown_workspace;
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

/// The tabs of `workspace` in the runtime, in order.
fn tabs_of(h: &DesktopHarness, workspace: WorkspaceId) -> Vec<TabId> {
    h.runtime
        .lock()
        .workspace(workspace)
        .map(|record| record.tabs.iter().map(|tab| tab.id).collect())
        .unwrap_or_default()
}

/// The panes of `tab` in the runtime, in order.
fn panes_of(h: &DesktopHarness, tab: TabId) -> Vec<PaneId> {
    h.runtime.lock().tab(tab).map_or_else(Vec::new, |tab| {
        tab.layout().panes().iter().map(|pane| pane.id).collect()
    })
}

/// The gesture's target in `window`, while one drags.
fn target(cx: &mut App, window: AnyWindowHandle) -> Option<DropTarget> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            let drag = view.read(cx).drag.as_ref()?;
            drag.dragging().then(|| drag.target()).flatten()
        })
        .ok()
        .flatten()
}

fn row(workspace: WorkspaceId, spring: bool) -> DropTarget {
    DropTarget::Workspace { workspace, spring }
}

/// A docked sidebar beside a window on the harness workspace: tab `tab`
/// split `[left | right]` and tab `other`, and a second workspace whose
/// one tab `there` the window holds hidden.
struct Fixture {
    h: DesktopHarness,
    window: AnyWindowHandle,
    tab: TabId,
    left: PaneId,
    right: PaneId,
    other: TabId,
    second: WorkspaceId,
    there: TabId,
}

impl Fixture {
    fn new(cx: &mut TestAppContext) -> Self {
        let mut h = DesktopHarness::with_config(cx, |config| {
            config.sidebar.visible = true;
            config.tabs.always_show = true;
        });
        let tab = h.open_tab(ECHO);
        let other = h.open_tab(ECHO);
        let (second, tabs) = h.add_workspace(1);
        let (window, left, right) = h.split_window(tab);
        h.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
        let there = tabs[0];
        h.wait_for("every view to install", |cx| {
            terminal(cx, window, other).is_some()
                && super::hidden_workspaces::hidden_tabs(cx, window, second)
                    == Some(vec![there])
        });
        Self {
            h,
            window,
            tab,
            left,
            right,
            other,
            second,
            there,
        }
    }

    /// The centre of `workspace`'s row.
    fn row(&mut self, workspace: WorkspaceId) -> gpui::Point<Pixels> {
        self.h.row_bounds(self.window, workspace).center()
    }

    /// Presses `right`'s title bar and drags it to `to`.
    fn drag_pane(&mut self, to: gpui::Point<Pixels>) {
        let grab = self.h.grab(self.window, self.right);
        self.h.press_at(self.window, grab);
        self.h.move_to(self.window, to);
    }

    /// Rests the drag on the armed target until its spring load fires.
    fn spring(&mut self) {
        let (armed, _) = self.h.armed(self.window);
        self.h.advance(self.window, armed + SPRING_LOAD_DELAY);
    }
}

#[gpui::test]
fn a_pane_dropped_on_a_workspace_row_moves_there_and_the_window_stays(
    cx: &mut TestAppContext,
) {
    let mut f = Fixture::new(cx);
    let window = f.window;
    let own = f.h.workspace;
    // Its own workspace's row takes no drop.
    let own_row = f.row(own);
    f.drag_pane(own_row);
    f.h.cx.run_until_parked();
    assert_eq!(f.h.cx.update(|cx| target(cx, window)), None);
    // Another workspace's row does, drawing its drop region (H1).
    let second_row = f.row(f.second);
    f.h.move_to(window, second_row);
    f.h.wait_for("the row to take the drop", |cx| {
        target(cx, window) == Some(row(f.second, true))
    });
    f.h.cx.run_until_parked();
    assert!(
        VisualTestContext::from_window(window, &f.h.cx)
            .debug_bounds("workspace-row-drop")
            .is_some(),
        "the row draws no drop region"
    );
    f.h.release_at(window, second_row);
    let (second, there, right) = (f.second, f.there, f.right);
    f.h.wait_for("the pane to land in a new tab there", |cx| {
        no_drag(cx, window)
            && cx
                .global::<Desktop>()
                .hierarchy
                .state()
                .workspace_tabs(second)
                .is_some_and(|tabs| tabs.len() == 2 && tabs[0] == there)
    });
    let moved = *tabs_of(&f.h, second).last().unwrap();
    assert_eq!(panes_of(&f.h, moved), [right]);
    assert_eq!(panes_of(&f.h, f.tab), [f.left]);
    f.h.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, window), Some(own));
        assert_eq!(active(cx, window), Some(f.tab));
    });
    // The window keeps the moved pane's view, hidden with its workspace.
    f.h.wait_for("the moved view to be held hidden", |cx| {
        super::hidden_workspaces::hidden_tabs(cx, window, second)
            == Some(vec![there, moved])
    });
}

#[gpui::test]
fn a_tab_dropped_on_a_workspace_row_moves_to_its_end(cx: &mut TestAppContext) {
    let mut f = Fixture::new(cx);
    let window = f.window;
    let label = f.h.label(window, 1);
    f.h.press_at(window, label);
    let second_row = f.row(f.second);
    f.h.move_to(window, second_row);
    f.h.wait_for("the row to take the tab", |cx| {
        target(cx, window) == Some(row(f.second, true))
    });
    f.h.release_at(window, second_row);
    let (second, there, other) = (f.second, f.there, f.other);
    f.h.wait_for("the tab to move there", |cx| {
        no_drag(cx, window)
            && cx
                .global::<Desktop>()
                .hierarchy
                .state()
                .workspace_tabs(second)
                == Some(&[there, other][..])
    });
    assert_eq!(tabs_of(&f.h, f.h.workspace), [f.tab]);
    f.h.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, window), Some(f.h.workspace));
        assert_eq!(active(cx, window), Some(f.tab));
    });
}

#[gpui::test]
fn a_pane_resting_on_a_row_shows_its_workspace_and_drops_into_its_bar(
    cx: &mut TestAppContext,
) {
    let mut f = Fixture::new(cx);
    let window = f.window;
    let second_row = f.row(f.second);
    f.drag_pane(second_row);
    f.h.wait_for("the row to arm", |cx| {
        target(cx, window) == Some(row(f.second, true))
    });
    f.spring();
    let (second, there) = (f.second, f.there);
    f.h.wait_for("the workspace to show under the drag", |cx| {
        shown_workspace(cx, window) == Some(second)
            && active(cx, window) == Some(there)
            && target(cx, window) == Some(row(second, false))
    });
    // The shown row no longer springs; the bar's slot after its tab takes
    // the pane.
    let slot = f.h.update_view(window, |view, window, _| {
        let label = view.tab_strip(window).label(0);
        gpui::point(label.right() - px(2.0), label.center().y)
    });
    f.h.move_to(window, slot);
    f.h.wait_for("the slot to take the pane", |cx| {
        target(cx, window) == Some(DropTarget::Slot(1))
    });
    f.h.release_at(window, slot);
    let right = f.right;
    f.h.wait_for("the pane to land in the shown workspace", |cx| {
        no_drag(cx, window)
            && cx
                .global::<Desktop>()
                .hierarchy
                .state()
                .workspace_tabs(second)
                .is_some_and(|tabs| tabs.len() == 2)
    });
    let moved = tabs_of(&f.h, second)[1];
    assert_eq!(panes_of(&f.h, moved), [right]);
    f.h.wait_for("the moved pane to show and focus", |cx| {
        shown_workspace(cx, window) == Some(second)
            && active(cx, window) == Some(moved)
    });
}

#[gpui::test]
fn a_cancelled_drag_springs_back_across_workspaces(cx: &mut TestAppContext) {
    let mut f = Fixture::new(cx);
    let window = f.window;
    let own = f.h.workspace;
    let seq = f.h.runtime.lock().hierarchy_seq();
    for end in ["escape", "release", "blur"] {
        let second_row = f.row(f.second);
        f.drag_pane(second_row);
        f.h.wait_for("the row to arm", |cx| {
            target(cx, window) == Some(row(f.second, true))
        });
        f.spring();
        let second = f.second;
        f.h.wait_for("the workspace to show", |cx| {
            shown_workspace(cx, window) == Some(second)
        });
        match end {
            "escape" => {
                f.h.press(window, "escape");
                f.h.release_at(window, second_row);
            }
            "release" => {
                // The shown row takes the pane, so release where nothing
                // does: the sidebar's header.
                let header = f.h.update_view(window, |view, window, _| {
                    let sidebar = view.chrome_layout(window).workspace.unwrap();
                    gpui::point(sidebar.center().x, sidebar.bottom() - px(4.0))
                });
                f.h.move_to(window, header);
                f.h.release_at(window, header);
            }
            _ => f.h.update_view(window, |view, window, cx| {
                view.cancel_drag(window, cx);
            }),
        }
        let (tab, right) = (f.tab, f.right);
        f.h.wait_for("the drag to spring back to both", |cx| {
            no_drag(cx, window)
                && shown_workspace(cx, window) == Some(own)
                && active(cx, window) == Some(tab)
                && panes_shown(cx, window, tab)
                    .is_some_and(|(_, focus)| focus == right)
        });
    }
    assert_eq!(f.h.runtime.lock().hierarchy_seq(), seq, "something moved");
}

#[gpui::test]
fn a_tab_springs_a_row_and_lands_in_that_workspaces_bar(
    cx: &mut TestAppContext,
) {
    let mut f = Fixture::new(cx);
    let window = f.window;
    let label = f.h.label(window, 1);
    f.h.press_at(window, label);
    let second_row = f.row(f.second);
    f.h.move_to(window, second_row);
    f.h.wait_for("the row to arm", |cx| {
        target(cx, window) == Some(row(f.second, true))
    });
    f.spring();
    let (second, there, other) = (f.second, f.there, f.other);
    f.h.wait_for("the workspace to show", |cx| {
        shown_workspace(cx, window) == Some(second)
    });
    // The tab left its bar: the drag draws its chip, and the slot before
    // the shown tab moves it there instead of reordering.
    let slot = f.h.update_view(window, |view, window, _| {
        let label = view.tab_strip(window).label(0);
        gpui::point(label.left() + px(2.0), label.center().y)
    });
    f.h.move_to(window, slot);
    f.h.wait_for("the slot to take the tab", |cx| {
        target(cx, window) == Some(DropTarget::Slot(0))
    });
    f.h.cx.run_until_parked();
    assert!(
        VisualTestContext::from_window(window, &f.h.cx)
            .debug_bounds("drag-title-chip")
            .is_some(),
        "the tab drag draws no chip away from its bar"
    );
    f.h.release_at(window, slot);
    f.h.wait_for("the tab to land first there, shown", |cx| {
        no_drag(cx, window)
            && cx
                .global::<Desktop>()
                .hierarchy
                .state()
                .workspace_tabs(second)
                == Some(&[other, there][..])
            && active(cx, window) == Some(other)
    });
}

#[gpui::test]
fn a_spring_loaded_drop_closes_the_emptied_origin_unless_pinned(
    cx: &mut TestAppContext,
) {
    for pinned in [false, true] {
        let mut h = DesktopHarness::with_config(cx, |config| {
            config.sidebar.visible = true;
            config.tabs.always_show = true;
        });
        let tab = h.open_tab(ECHO);
        let (second, tabs) = h.add_workspace(1);
        let window = h.attach_showing(&[tab]);
        h.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
        let origin = h.workspace;
        if pinned {
            h.runtime.lock().set_workspace_pinned(origin, true).unwrap();
            h.wait_projection();
        }
        let label = h.label(window, 0);
        h.press_at(window, label);
        let second_row = h.row_bounds(window, second).center();
        h.move_to(window, second_row);
        h.wait_for("the row to arm", |cx| {
            target(cx, window) == Some(row(second, true))
        });
        let (armed, _) = h.armed(window);
        h.advance(window, armed + SPRING_LOAD_DELAY);
        h.wait_for("the workspace to show", |cx| {
            shown_workspace(cx, window) == Some(second)
        });
        let slot = h.update_view(window, |view, window, _| {
            let label = view.tab_strip(window).label(0);
            gpui::point(label.right() - px(2.0), label.center().y)
        });
        h.move_to(window, slot);
        h.release_at(window, slot);
        let session = h.session;
        h.wait_for("the tab to move and the origin to settle", |cx| {
            no_drag(cx, window)
                && cx
                    .global::<Desktop>()
                    .hierarchy
                    .state()
                    .workspace_tabs(second)
                    == Some(&[tabs[0], tab][..])
                && order(cx, session).contains(&origin) == pinned
        });
        h.cx.update(|cx| {
            assert_eq!(shown_workspace(cx, window), Some(second));
        });
    }
}

#[gpui::test]
fn a_row_drop_that_empties_the_shown_workspace_follows(
    cx: &mut TestAppContext,
) {
    let mut h = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
        config.tabs.always_show = true;
    });
    let tab = h.open_tab(ECHO);
    let (second, tabs) = h.add_workspace(1);
    let window = h.attach_showing(&[tab]);
    h.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    let origin = h.workspace;
    let label = h.label(window, 0);
    h.press_at(window, label);
    let second_row = h.row_bounds(window, second).center();
    h.move_to(window, second_row);
    h.wait_for("the row to take the tab", |cx| {
        target(cx, window) == Some(row(second, true))
    });
    h.release_at(window, second_row);
    let session = h.session;
    h.wait_for("the window to follow its tab", |cx| {
        no_drag(cx, window)
            && order(cx, session) == [second]
            && shown_workspace(cx, window) == Some(second)
    });
    assert_eq!(tabs_of(&h, second), [tabs[0], tab]);
    assert!(!h.workspace_exists(origin));
}

#[gpui::test]
fn a_busy_window_does_not_spring_a_row(cx: &mut TestAppContext) {
    let mut f = Fixture::new(cx);
    let window = f.window;
    let second_row = f.row(f.second);
    f.drag_pane(second_row);
    f.h.wait_for("the row to arm", |cx| {
        target(cx, window) == Some(row(f.second, true))
    });
    let (armed, _) = f.h.armed(window);
    // An arrangement in flight holds switches.
    f.h.update_view(window, |view, _, _| view.arrangements += 1);
    f.h.advance(window, armed + SPRING_LOAD_DELAY);
    f.h.cx.run_until_parked();
    f.h.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, window), Some(f.h.workspace));
        assert!(target(cx, window).is_some(), "the drag ended");
    });
    f.h.update_view(window, |view, _, _| view.arrangements -= 1);
    f.h.update_view(window, |view, window, cx| {
        view.cancel_drag(window, cx);
    });
}

#[gpui::test]
fn a_row_that_goes_before_release_takes_no_drop(cx: &mut TestAppContext) {
    let mut f = Fixture::new(cx);
    let window = f.window;
    let second_row = f.row(f.second);
    f.drag_pane(second_row);
    f.h.wait_for("the row to take the drop", |cx| {
        target(cx, window) == Some(row(f.second, true))
    });
    f.h.runtime.lock().close_workspace(f.second).unwrap();
    f.h.wait_projection();
    f.h.cx.run_until_parked();
    f.h.release_at(window, second_row);
    f.h.cx.run_until_parked();
    let (tab, left, right) = (f.tab, f.left, f.right);
    f.h.wait_for("the drag to end", |cx| no_drag(cx, window));
    assert_eq!(panes_of(&f.h, tab), [left, right]);
    f.h.update_view(window, |view, _, _| {
        assert!(view.notices.is_empty(), "the release reported a failure");
    });
}

#[gpui::test]
fn closing_the_sprung_workspace_ends_the_drag(cx: &mut TestAppContext) {
    let mut f = Fixture::new(cx);
    let window = f.window;
    let second_row = f.row(f.second);
    f.drag_pane(second_row);
    f.h.wait_for("the row to arm", |cx| {
        target(cx, window) == Some(row(f.second, true))
    });
    f.spring();
    let second = f.second;
    f.h.wait_for("the workspace to show", |cx| {
        shown_workspace(cx, window) == Some(second)
    });
    f.h.runtime.lock().close_workspace(second).unwrap();
    let own = f.h.workspace;
    let (tab, left, right) = (f.tab, f.left, f.right);
    f.h.wait_for("the drag to end and the window to follow", |cx| {
        no_drag(cx, window) && shown_workspace(cx, window) == Some(own)
    });
    f.h.release_at(window, second_row);
    f.h.cx.run_until_parked();
    assert_eq!(panes_of(&f.h, tab), [left, right]);
}

#[gpui::test]
fn an_exit_of_the_dragged_tab_ends_a_spring_loaded_drag(
    cx: &mut TestAppContext,
) {
    let mut f = Fixture::new(cx);
    let window = f.window;
    let label = f.h.label(window, 1);
    f.h.press_at(window, label);
    let second_row = f.row(f.second);
    f.h.move_to(window, second_row);
    f.h.wait_for("the row to arm", |cx| {
        target(cx, window) == Some(row(f.second, true))
    });
    f.spring();
    let second = f.second;
    f.h.wait_for("the workspace to show", |cx| {
        shown_workspace(cx, window) == Some(second)
    });
    // The dragged tab goes, as when its shell exits and closes it.
    f.h.runtime
        .lock()
        .close_tab(f.h.workspace, f.other)
        .unwrap();
    // The drag ends without a drop, so it springs back, as one whose pane
    // went does.
    let (own, tab) = (f.h.workspace, f.tab);
    f.h.wait_for("the drag to end and spring back", |cx| {
        no_drag(cx, window)
            && shown_workspace(cx, window) == Some(own)
            && active(cx, window) == Some(tab)
    });
    assert_eq!(tabs_of(&f.h, second), [f.there]);
}

#[gpui::test]
fn a_session_switch_ends_a_spring_loaded_drag(cx: &mut TestAppContext) {
    let mut f = Fixture::new(cx);
    let window = f.window;
    let other_session = {
        let mut mux = f.h.runtime.lock();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        mux.open_tab(workspace, &fixture(ECHO)).unwrap();
        session
    };
    f.h.wait_projection();
    let second_row = f.row(f.second);
    f.drag_pane(second_row);
    f.h.wait_for("the row to arm", |cx| {
        target(cx, window) == Some(row(f.second, true))
    });
    f.spring();
    let second = f.second;
    f.h.wait_for("the workspace to show", |cx| {
        shown_workspace(cx, window) == Some(second)
    });
    f.h.update_view(window, |view, window, cx| {
        view.switch_session(
            super::super::session_switch::SwitchTarget::Existing(other_session),
            window,
            cx,
        )
        .unwrap();
    });
    f.h.wait_for("the switch to settle without the drag", |cx| {
        no_drag(cx, window)
            && cx
                .global::<Desktop>()
                .windows
                .record(window.window_id())
                .is_some_and(|record| record.session == Some(other_session))
    });
    let (tab, left, right) = (f.tab, f.left, f.right);
    assert_eq!(panes_of(&f.h, tab), [left, right]);
}

#[gpui::test]
fn the_row_chip_follows_a_dragged_workspace_and_session_row(
    cx: &mut TestAppContext,
) {
    let mut f = Fixture::new(cx);
    let window = f.window;
    let from = f.row(f.second);
    // Above the first row's middle, so the slot is before it.
    let to = f.row(f.h.workspace) - gpui::point(px(0.0), px(6.0));
    f.h.drag_row(window, from, to);
    f.h.cx.run_until_parked();
    let chip = VisualTestContext::from_window(window, &f.h.cx)
        .debug_bounds("drag-title-chip")
        .expect("a row drag draws the chip");
    // The chip sits 12 points past the pointer, as the tab chip does.
    assert_eq!(chip.origin, to + gpui::point(px(12.0), px(12.0)));
    let own = f.h.workspace;
    f.h.update_view(window, |view, _, cx| {
        let drag = view.drag.as_ref().unwrap();
        assert!(matches!(drag.payload, Payload::Workspace { .. }));
        assert_eq!(view.row_title(drag.payload, cx), "Workspace 2");
        assert_eq!(
            drag.target(),
            Some(DropTarget::RowSlot(Some(RowKey::Workspace(own))))
        );
    });
    f.h.press(window, "escape");
    f.h.release(window, to);
    f.h.cx.run_until_parked();
    assert!(
        VisualTestContext::from_window(window, &f.h.cx)
            .debug_bounds("drag-title-chip")
            .is_none(),
        "the chip outlives the drag"
    );
}

/// A harness with docked sidebars and always-shown bars.
fn sidebar_harness(cx: &mut TestAppContext) -> DesktopHarness {
    DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
        config.tabs.always_show = true;
    })
}

/// The window `cx` opened after `before`, once there is one.
fn new_window(
    h: &mut DesktopHarness,
    before: &[AnyWindowHandle],
) -> AnyWindowHandle {
    let mut opened = None;
    h.wait_for("a new window", |cx| {
        opened = cx.windows().into_iter().find(|handle| {
            !before.contains(handle)
                && handle.downcast::<WorkspaceView>().is_some()
        });
        opened.is_some()
    });
    opened.unwrap()
}

/// Whether `window` is busy now.
fn busy_now(h: &mut DesktopHarness, window: AnyWindowHandle) -> bool {
    h.update_view(window, |view, _, _| view.busy)
}

/// The session the projection places `workspace` in.
fn workspace_session(
    h: &mut DesktopHarness,
    workspace: WorkspaceId,
) -> SessionId {
    h.wait_projection();
    h.cx.update(|cx| {
        cx.global::<Desktop>()
            .hierarchy
            .state()
            .workspace_session(workspace)
            .unwrap()
    })
}

/// The session `window` attaches to.
fn session_of(cx: &mut App, window: AnyWindowHandle) -> Option<SessionId> {
    cx.global::<Desktop>()
        .windows
        .record(window.window_id())
        .and_then(|record| record.session)
}

impl DesktopHarness {
    /// Starts dragging `workspace`'s row in `window` past the threshold,
    /// as the capture listeners would, sampling the cross-window locator.
    fn begin_workspace_drag(
        &mut self,
        window: AnyWindowHandle,
        workspace: WorkspaceId,
    ) {
        let at = self.row_bounds(window, workspace).center();
        self.update_view(window, |view, window, cx| {
            view.begin_row_drag(RowKey::Workspace(workspace), at, window, cx);
            view.update_drag(at + gpui::point(px(8.0), px(8.0)), window, cx);
        });
        self.cx.run_until_parked();
    }

    /// Releases `window`'s drag where the locator says it is.
    fn finish_workspace_drag(&mut self, window: AnyWindowHandle) {
        self.update_view(window, |view, window, cx| {
            view.finish_drag(gpui::point(px(-100.0), px(-100.0)), window, cx);
        });
    }
}

#[gpui::test]
fn a_tab_dropped_on_another_windows_row_moves_there_without_switching(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let moving = h.open_tab(ECHO);
    let source = h.attach_showing(&[tab, moving]);
    let (destination, shown, anchor) = h.private_window();
    let target_session = workspace_session(&mut h, shown);
    let (hidden, hidden_tab) = {
        let mut mux = h.runtime.lock();
        let session = target_session;
        let hidden = mux.create_workspace(session, None).unwrap();
        let tab = mux.open_tab(hidden, &fixture(ECHO)).unwrap().tab.id;
        (hidden, tab)
    };
    h.wait_projection();
    h.wait_chrome(destination, "its sidebar", WorkspaceChrome::docked);
    let locator = h.cross_locator();
    let over = h.row_bounds(destination, hidden).center();
    locator
        .0
        .set(Located::Window(destination.window_id(), over));
    h.update_view(source, |view, window, cx| {
        let at = view.tab_strip(window).label(1).center();
        view.begin_reorder(moving, at, window, cx);
        view.update_drag(at + gpui::point(px(8.0), px(8.0)), window, cx);
    });
    h.cx.run_until_parked();
    assert!(h.update_view(destination, |view, _, _| {
        view.incoming.as_ref().is_some_and(|incoming| {
            incoming.hover.target
                == Some(DropTarget::Workspace {
                    workspace: hidden,
                    spring: true,
                })
        })
    }));
    // The destination keeps its reservation until the tab installs there,
    // hidden.
    let runtime = Arc::clone(&h.runtime);
    runtime.worker_hooks.hold(Stage::Subscribe);
    h.finish_workspace_drag(source);
    h.wait_for("the move to commit and the install to wait", |cx| {
        cx.global::<Desktop>()
            .hierarchy
            .state()
            .workspace_tabs(hidden)
            == Some(&[hidden_tab, moving][..])
            && runtime.worker_hooks.held(Stage::Subscribe) >= 1
    });
    h.cx.run_until_parked();
    assert!(busy_now(&mut h, destination), "the reservation ended early");
    runtime.worker_hooks.release(Stage::Subscribe);
    h.wait_for(
        "the tab to install hidden and the windows to settle",
        |cx| {
            super::hidden_workspaces::hidden_tabs(cx, destination, hidden)
                == Some(vec![hidden_tab, moving])
                && !busy(cx, destination)
                && !busy(cx, source)
        },
    );
    h.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, destination), Some(shown));
        assert_eq!(active(cx, destination), Some(anchor));
        assert_eq!(session_of(cx, destination), Some(target_session));
    });
    assert_eq!(tabs_of(&h, h.workspace), [tab]);
}

#[gpui::test]
fn a_workspace_dropped_on_another_window_moves_and_shows_there(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let (second, _) = h.add_workspace(1);
    let source = h.attach_showing(&[tab]);
    let mirror = h.attach_showing(&[tab]);
    let (destination, shown, _) = h.private_window();
    let target_session = workspace_session(&mut h, shown);
    h.wait_chrome(source, "the sidebar", WorkspaceChrome::docked);
    let locator = h.cross_locator();
    let content = h.update_view(destination, |view, window, _| {
        view.chrome_layout(window).terminal.center()
    });
    locator
        .0
        .set(Located::Window(destination.window_id(), content));
    let moved = h.workspace;
    h.begin_workspace_drag(source, moved);
    assert!(h.update_view(destination, |view, _, _| {
        view.incoming.as_ref().is_some_and(|incoming| {
            incoming.hover.target == Some(DropTarget::Window)
        })
    }));
    // The destination draws the drop region over its content, and the
    // chip the floating preview would otherwise show.
    h.cx.run_until_parked();
    let mut visual = VisualTestContext::from_window(destination, &h.cx);
    assert!(visual.debug_bounds("drag-title-chip").is_some());
    h.finish_workspace_drag(source);
    let session = h.session;
    h.wait_for("the workspace to move and show there", |cx| {
        order(cx, target_session) == [shown, moved]
            && order(cx, session) == [second]
            && shown_workspace(cx, destination) == Some(moved)
            && terminal(cx, destination, tab).is_some()
            && !busy(cx, destination)
    });
    h.wait_for("both source windows to follow", |cx| {
        shown_workspace(cx, source) == Some(second)
            && shown_workspace(cx, mirror) == Some(second)
    });
    // The shell keeps running in its new session.
    h.type_line(destination, "moved");
    h.wait_text(destination, tab, "got:moved");
}

#[gpui::test]
fn a_closing_window_takes_no_workspace_drop(cx: &mut TestAppContext) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let (second, _) = h.add_workspace(1);
    let source = h.attach_showing(&[tab]);
    let (destination, shown, _) = h.private_window();
    let target_session = workspace_session(&mut h, shown);
    h.wait_chrome(source, "the sidebar", WorkspaceChrome::docked);
    let locator = h.cross_locator();
    locator.0.set(Located::Window(
        destination.window_id(),
        gpui::point(px(200.0), px(200.0)),
    ));
    h.begin_workspace_drag(source, second);
    h.update_view(destination, |view, _, _| view.busy = true);
    h.finish_workspace_drag(source);
    h.cx.run_until_parked();
    let session = h.session;
    h.cx.update(|cx| {
        assert_eq!(order(cx, session), [h.workspace, second]);
        assert_eq!(order(cx, target_session), [shown]);
    });
    h.update_view(destination, |view, _, _| view.busy = false);
}

#[gpui::test]
fn a_workspace_released_outside_tears_off_into_a_window_of_its_own(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let (second, second_tabs) = h.add_workspace(1);
    let (third, third_tabs) = h.add_workspace(1);
    let source = h.attach_showing(&[tab]);
    h.wait_chrome(source, "the sidebar", WorkspaceChrome::docked);
    let _locator = h.cross_locator();
    let moved = h.workspace;
    // Outside every window, the source row draws S1 (W1).
    h.begin_workspace_drag(source, moved);
    h.cx.run_until_parked();
    assert!(
        VisualTestContext::from_window(source, &h.cx)
            .debug_bounds("workspace-tear-off-pill")
            .is_some(),
        "the leaving row draws no tear-off cue"
    );
    let before = h.cx.update(|cx| cx.windows());
    h.finish_workspace_drag(source);
    let torn = new_window(&mut h, &before);
    let session = h.session;
    h.wait_for("the workspace to show in its own session", |cx| {
        let new_session = session_of(cx, torn);
        new_session
            .is_some_and(|new| new != session && order(cx, new) == [moved])
            && shown_workspace(cx, torn) == Some(moved)
            && terminal(cx, torn, tab).is_some()
            && !busy(cx, torn)
            && order(cx, session) == [second, third]
            && shown_workspace(cx, source) == Some(second)
    });
    h.type_line(torn, "torn");
    h.wait_text(torn, tab, "got:torn");
    // The row menu's command shares the path.
    let before = h.cx.update(|cx| cx.windows());
    h.invoke_with(
        source,
        &super::workspaces::workspace_pick(
            ids::MOVE_WORKSPACE_TO_NEW_WINDOW,
            third,
        ),
    )
    .unwrap();
    let by_command = new_window(&mut h, &before);
    h.wait_for("the command's window to show its workspace", |cx| {
        shown_workspace(cx, by_command) == Some(third)
            && terminal(cx, by_command, third_tabs[0]).is_some()
            && order(cx, session) == [second]
    });
    // The session's only workspace stays: no command, no tear-off.
    assert!(matches!(
        h.invoke(source, ids::MOVE_WORKSPACE_TO_NEW_WINDOW),
        Err(CommandError::Unavailable(_))
    ));
    h.begin_workspace_drag(source, second);
    assert!(!h.update_view(source, |view, _, cx| view.tear_off_cue(cx)));
    let before = h.cx.update(|cx| cx.windows());
    h.finish_workspace_drag(source);
    h.cx.run_until_parked();
    assert_eq!(h.cx.update(|cx| cx.windows()), before);
    h.cx.update(|cx| {
        assert_eq!(order(cx, session), [second]);
        assert_eq!(shown_workspace(cx, source), Some(second));
    });
    assert_eq!(tabs_of(&h, second), second_tabs);
}

#[gpui::test]
fn a_failed_workspace_tear_off_puts_the_workspace_back(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let (second, _) = h.add_workspace(1);
    let (third, _) = h.add_workspace(1);
    let source = h.attach_showing(&[tab]);
    let sessions = h.runtime.lock().sessions().len();
    h.cx.update(|cx| cx.global_mut::<Desktop>().cross.fail_window = true);
    h.invoke_with(
        source,
        &super::workspaces::workspace_pick(
            ids::MOVE_WORKSPACE_TO_NEW_WINDOW,
            second,
        ),
    )
    .unwrap();
    let (session, first) = (h.session, h.workspace);
    h.wait_for("the workspace to come back to its slot", |cx| {
        !busy(cx, source)
            && order(cx, session) == [first, second, third]
            && notices(cx, source).iter().any(|notice| {
                notice.contains("Cannot open the destination window")
            })
    });
    assert_eq!(h.runtime.lock().sessions().len(), sessions);
    assert_eq!(h.cx.update(|cx| cx.windows().len()), 1);
    h.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, source), Some(h.workspace));
    });
}

#[gpui::test]
fn a_workspace_tear_off_interrupted_by_quit_saves_the_workspace_once(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let (second, _) = h.add_workspace(1);
    let source = h.attach_showing(&[tab]);
    let runtime = Arc::clone(&h.runtime);
    runtime.worker_hooks.hold(Stage::Transfer);
    h.invoke_with(
        source,
        &super::workspaces::workspace_pick(
            ids::MOVE_WORKSPACE_TO_NEW_WINDOW,
            second,
        ),
    )
    .unwrap();
    h.wait_for("the tear-off worker to start", |_| {
        runtime.worker_hooks.held(Stage::Transfer) == 1
    });
    h.resume_quits();
    h.cx.update(request_quit);
    h.cx.run_until_parked();
    assert!(
        h.recorder.saved().is_empty(),
        "the Quit captured before the tear-off settled"
    );
    runtime.worker_hooks.release(Stage::Transfer);
    h.wait_for("the Quit to commit or ask", |cx| {
        terminating(cx)
            || cx.windows().into_iter().any(|handle| {
                handle
                    .update(cx, |root, _, cx| {
                        root.downcast::<WorkspaceView>().is_ok_and(|view| {
                            view.read(cx).close.confirmation
                                == Some(CloseTarget::Application)
                        })
                    })
                    .unwrap_or(false)
            })
    });
    if !h.cx.update(|cx| terminating(cx)) {
        super::sessions::confirm_quit(&mut h, source);
    }
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
    assert_eq!(
        holding.len(),
        1,
        "the workspace was saved {} times",
        holding.len()
    );
}

#[gpui::test]
fn a_workspace_tear_off_reserves_its_window_until_its_tabs_show(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let (second, tabs) = h.add_workspace(1);
    let source = h.attach_showing(&[tab]);
    let runtime = Arc::clone(&h.runtime);
    // Hold every new install, so the torn-off window waits on its tab.
    runtime.worker_hooks.hold(Stage::Subscribe);
    let before = h.cx.update(|cx| cx.windows());
    h.invoke_with(
        source,
        &super::workspaces::workspace_pick(
            ids::MOVE_WORKSPACE_TO_NEW_WINDOW,
            second,
        ),
    )
    .unwrap();
    let torn = new_window(&mut h, &before);
    h.wait_for("the install to wait", |_| {
        runtime.worker_hooks.held(Stage::Subscribe) >= 1
    });
    // A checkpoint waits while the window holds the transfer reservation.
    h.cx.update(|cx| {
        assert!(
            cx.global::<Desktop>()
                .cross
                .reserved
                .contains(&torn.window_id())
        );
    });
    assert!(h.update_view(torn, |view, _, _| view.busy));
    runtime.worker_hooks.release(Stage::Subscribe);
    h.wait_for("the reservation to end once the tab shows", |cx| {
        terminal(cx, torn, tabs[0]).is_some()
            && !cx
                .global::<Desktop>()
                .cross
                .reserved
                .contains(&torn.window_id())
            && !busy(cx, torn)
    });
}

#[gpui::test]
fn closing_the_window_ends_a_spring_loaded_drag(cx: &mut TestAppContext) {
    let mut f = Fixture::new(cx);
    let window = f.window;
    let second_row = f.row(f.second);
    f.drag_pane(second_row);
    f.h.wait_for("the row to arm", |cx| {
        target(cx, window) == Some(row(f.second, true))
    });
    f.spring();
    let second = f.second;
    f.h.wait_for("the workspace to show", |cx| {
        shown_workspace(cx, window) == Some(second)
    });
    // Another window keeps the session, so the close only detaches.
    let _keeper = f.h.attach_window();
    f.h.update_view(window, |view, window, cx| {
        view.request_close(CloseTarget::Window, window, cx);
    });
    f.h.wait_for("the window to close", |cx| !cx.windows().contains(&window));
    f.h.cx.run_until_parked();
    assert_eq!(panes_of(&f.h, f.tab), [f.left, f.right]);
    assert_eq!(tabs_of(&f.h, second), [f.there]);
}

#[gpui::test]
fn a_workspace_that_goes_before_release_does_not_tear_off(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let (second, _) = h.add_workspace(1);
    let source = h.attach_showing(&[tab]);
    h.wait_chrome(source, "the sidebar", WorkspaceChrome::docked);
    let _locator = h.cross_locator();
    h.begin_workspace_drag(source, second);
    assert!(h.update_view(source, |view, _, cx| view.tear_off_cue(cx)));
    // Its last tab exits and the emptied workspace closes, as R1's
    // exit-close does for a hidden workspace.
    h.runtime.lock().close_workspace(second).unwrap();
    h.wait_for("the drag and its cross-window record to end", |cx| {
        source
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>()
                    .is_ok_and(|view| view.read(cx).drag.is_none())
            })
            .unwrap_or(false)
            && cx.global::<Desktop>().cross.drag.is_none()
    });
    let before = h.cx.update(|cx| cx.windows());
    h.finish_workspace_drag(source);
    h.cx.run_until_parked();
    assert_eq!(h.cx.update(|cx| cx.windows()), before);
    let session = h.session;
    h.cx.update(|cx| assert_eq!(order(cx, session), [h.workspace]));
    let shown = h.cx.update(|cx| notices(cx, source));
    assert!(shown.is_empty(), "{shown:?}");
}

#[gpui::test]
fn a_workspace_over_another_windows_list_takes_its_slot(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let (second, _) = h.add_workspace(1);
    let source = h.attach_showing(&[tab]);
    let (destination, shown, _) = h.private_window();
    let target_session = workspace_session(&mut h, shown);
    h.wait_chrome(destination, "its sidebar", WorkspaceChrome::docked);
    h.wait_chrome(source, "the sidebar", WorkspaceChrome::docked);
    let locator = h.cross_locator();
    // Above the destination's only row's middle: the slot before it.
    let above = h.row_bounds(destination, shown).center()
        - gpui::point(px(0.0), px(6.0));
    locator
        .0
        .set(Located::Window(destination.window_id(), above));
    h.begin_workspace_drag(source, second);
    assert!(h.update_view(destination, |view, _, _| {
        view.incoming.as_ref().is_some_and(|incoming| {
            incoming.hover.target
                == Some(DropTarget::RowSlot(Some(RowKey::Workspace(shown))))
        })
    }));
    h.finish_workspace_drag(source);
    h.wait_for("the workspace to land first there and show", |cx| {
        order(cx, target_session) == [second, shown]
            && shown_workspace(cx, destination) == Some(second)
            && !busy(cx, destination)
    });
    h.cx.update(|cx| {
        assert_eq!(order(cx, h.session), [h.workspace]);
        assert_eq!(shown_workspace(cx, source), Some(h.workspace));
    });
}

#[gpui::test]
fn a_tab_sprung_into_another_workspace_still_leaves_for_another_window(
    cx: &mut TestAppContext,
) {
    let mut f = Fixture::new(cx);
    let window = f.window;
    let (destination, shown, anchor) = f.h.private_window();
    let label = f.h.label(window, 1);
    f.h.press_at(window, label);
    let second_row = f.row(f.second);
    f.h.move_to(window, second_row);
    f.h.wait_for("the row to arm", |cx| {
        target(cx, window) == Some(row(f.second, true))
    });
    f.spring();
    let second = f.second;
    f.h.wait_for("the workspace to show", |cx| {
        shown_workspace(cx, window) == Some(second)
    });
    // The tab's view is hidden now; leaving still carries it.
    let locator = f.h.cross_locator();
    let slot = f.h.update_view(destination, |view, window, _| {
        view.tab_strip(window).label(0).origin + gpui::point(px(1.0), px(1.0))
    });
    locator
        .0
        .set(Located::Window(destination.window_id(), slot));
    f.h.move_to(window, second_row + gpui::point(px(4.0), px(4.0)));
    f.h.cx.run_until_parked();
    let dragged = Payload::Tab {
        workspace: f.h.workspace,
        tab: f.other,
    };
    assert!(f.h.update_view(destination, |view, _, _| {
        view.incoming.as_ref().is_some_and(|incoming| {
            incoming.hover.target == Some(DropTarget::Slot(0))
                && incoming.payload.drag == dragged
        })
    }));
    f.h.release_at(window, second_row);
    let other = f.other;
    f.h.wait_for("the tab to land first in the other window", |cx| {
        cx.global::<Desktop>()
            .hierarchy
            .state()
            .workspace_tabs(shown)
            == Some(&[other, anchor][..])
            && !busy(cx, destination)
            && !busy(cx, window)
    });
    assert_eq!(tabs_of(&f.h, f.h.workspace), [f.tab]);
}

#[gpui::test]
fn a_pinned_empty_workspace_tears_off_into_its_empty_state(
    cx: &mut TestAppContext,
) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let empty = {
        let mut mux = h.runtime.lock();
        let empty = mux.create_workspace(h.session, Some("notes")).unwrap();
        mux.set_workspace_pinned(empty, true).unwrap();
        empty
    };
    let source = h.attach_showing(&[tab]);
    let before = h.cx.update(|cx| cx.windows());
    h.invoke_with(
        source,
        &super::workspaces::workspace_pick(
            ids::MOVE_WORKSPACE_TO_NEW_WINDOW,
            empty,
        ),
    )
    .unwrap();
    let torn = new_window(&mut h, &before);
    let session = h.session;
    h.wait_for("the empty workspace to show alone in its window", |cx| {
        shown_workspace(cx, torn) == Some(empty)
            && session_of(cx, torn)
                .is_some_and(|new| new != session && order(cx, new) == [empty])
            && !busy(cx, torn)
            && !busy(cx, source)
            && super::workspaces::empty_state(cx, torn).is_some()
    });
    h.cx.update(|cx| assert_eq!(order(cx, session), [h.workspace]));
}

#[gpui::test]
fn a_tab_held_at_the_workspace_lists_edge_scrolls_it(cx: &mut TestAppContext) {
    let mut h = sidebar_harness(cx);
    let tab = h.open_tab(ECHO);
    let _ = h.open_tab(ECHO);
    for _ in 0..40 {
        h.add_workspace(1);
    }
    let window = h.attach_showing(&[tab]);
    h.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    let list =
        h.update_view(window, |view, _, _| view.workspace_scroll.bounds());
    let label = h.label(window, 1);
    h.press_at(window, label);
    let edge = gpui::point(list.center().x, list.bottom() - px(4.0));
    h.move_to(window, edge);
    h.wait_for("the list to keep scrolling under the tab", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx).workspace_scroll.offset().y < px(-60.0)
                })
            })
            .unwrap_or(false)
    });
    h.press(window, "escape");
    h.release_at(window, edge);
}
