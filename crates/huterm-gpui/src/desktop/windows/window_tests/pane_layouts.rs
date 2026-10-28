//! Tier 2 tests for pane layouts: the layout commands from bindings and
//! the palette, fitting and refusals, the ordered layout commit queue and
//! its in-flight gate, races with other changes, undo entries, and two
//! windows converging on one arrangement.

use gpui::MouseButton;
use huterm_core::RestoreDirection;
use huterm_protocol::{HierarchyEvent, PaneNode};

use super::pane_moves::{identity, pane_focused};
use super::*;

/// `layout` in canonical form: each pane by its index in `order`, split
/// identities erased, axes, child order, and weights kept.
fn canonical(layout: &PaneLayout, order: &[PaneId]) -> String {
    fn walk(node: &PaneNode, order: &[PaneId], out: &mut String) {
        match node {
            PaneNode::Pane(pane) => {
                match order.iter().position(|id| *id == pane.id) {
                    Some(index) => write!(out, "{index}").unwrap(),
                    None => out.push('?'),
                }
            }
            PaneNode::Split(split) => {
                out.push(match split.axis {
                    SplitAxis::Columns => 'C',
                    SplitAxis::Rows => 'R',
                });
                out.push('[');
                for (index, child) in split.children.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write!(out, "{}:", child.weight).unwrap();
                    walk(&child.node, order, out);
                }
                out.push(']');
            }
        }
    }
    let mut out = String::new();
    walk(layout.root(), order, &mut out);
    out
}

/// The projected layout of `tab`.
fn layout_of(cx: &App, tab: TabId) -> Option<PaneLayout> {
    Some(
        cx.global::<Desktop>()
            .hierarchy
            .state()
            .tab(tab)?
            .layout
            .clone(),
    )
}

/// The projected layout of `tab` in canonical form.
fn shown(cx: &App, tab: TabId, order: &[PaneId]) -> String {
    layout_of(cx, tab)
        .map_or_else(String::new, |layout| canonical(&layout, order))
}

/// The arrangements `window` has in flight.
fn arranging(cx: &mut App, window: AnyWindowHandle) -> usize {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>()
                .map_or(0, |view| view.read(cx).arrangements)
        })
        .unwrap_or(0)
}

fn dragging(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>()
                .is_ok_and(|view| view.read(cx).divider_drag.is_some())
        })
        .unwrap_or(false)
}

fn undo_label(cx: &mut App, window: AnyWindowHandle) -> Option<&'static str> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx)
                .layout_restore_label(RestoreDirection::Undo, cx)
        })
        .ok()
        .flatten()
}

fn arrange_panes(layout: &str) -> CommandInvocation {
    CommandInvocation::new(
        ids::ARRANGE_PANES,
        vec![CommandArgument::new(
            "layout",
            CommandValue::Text(layout.to_owned()),
        )],
    )
}

/// `[A | (B over C)]` with even weights.
const NESTED: &str = "C[500000:0,500000:R[500000:1,500000:2]]";
const COLUMNS: &str = "C[333334:0,333333:1,333333:2]";
const ROWS: &str = "R[333334:0,333333:1,333333:2]";
const MAIN_LEFT: &str = "C[600000:0,400000:R[500000:1,500000:2]]";
const MAIN_TOP: &str = "R[600000:0,400000:C[500000:1,500000:2]]";
const TILED: &str = "R[500000:C[500000:0,500000:1],500000:2]";

impl DesktopHarness {
    /// Splits a tab of `script` panes into `[A | (B over C)]`, attaches a
    /// window showing it, and focuses A once every shell is ready.
    fn three_panes(
        &mut self,
        script: &str,
    ) -> (AnyWindowHandle, TabId, [PaneId; 3]) {
        let tab = self.open_tab(script);
        let first = self.first_pane(tab);
        let second = self.split(tab, first, SplitDirection::Right, script);
        let third = self.split(tab, second, SplitDirection::Down, script);
        let window = self.attach_window();
        let panes = [first, second, third];
        self.wait_for("every pane's shell", |cx| {
            panes.iter().all(|pane| {
                pane_screen(cx, window, tab, *pane)
                    .is_some_and(|text| text.contains("READY"))
            })
        });
        self.activate(window, tab);
        self.focus(window, tab, first);
        (window, tab, panes)
    }

    /// Sets the weights of `tab`'s split `split` (by index in traversal
    /// order) through Mux, as another client's divider drag would.
    fn reweigh(&self, tab: TabId, split: usize, weights: &[u32]) {
        fn collect<'a>(node: &'a PaneNode, out: &mut Vec<&'a SplitNode>) {
            if let PaneNode::Split(split) = node {
                out.push(split);
                for child in &split.children {
                    collect(&child.node, out);
                }
            }
        }
        let mut mux = self.runtime.lock();
        let layout = mux.tab(tab).unwrap().layout().clone();
        let mut splits = Vec::new();
        collect(layout.root(), &mut splits);
        let split = splits[split];
        let children: Vec<_> =
            split.children.iter().map(|child| child.node.id()).collect();
        mux.resize_split(self.workspace, tab, split.id, &children, weights)
            .unwrap();
    }

    /// Waits until `window` settles its arrangements and the projection
    /// shows `expected`.
    fn wait_layout(
        &mut self,
        window: AnyWindowHandle,
        tab: TabId,
        panes: &[PaneId],
        expected: &str,
    ) {
        self.wait_for(&format!("the layout {expected}"), |cx| {
            arranging(cx, window) == 0 && shown(cx, tab, panes) == expected
        });
    }

    /// The window's availability of `command`.
    fn available(
        &mut self,
        window: AnyWindowHandle,
        command: huterm_protocol::CommandId,
    ) -> Result<(), CommandError> {
        self.update_view(window, |view, window, cx| {
            let target = PaletteTarget {
                session: None,
                workspace: view.workspace_id(cx),
                tab: view.active_tab(cx),
                terminal: None,
                terminal_view: None,
                contexts: window.context_stack(),
            };
            view.command_availability(command, &target, cx)
        })
    }
}

#[gpui::test]
fn each_layout_command_arranges_the_tab_and_the_picker_matches_bindings(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, panes) = harness.three_panes(ECHO);
    harness
        .cx
        .update(|cx| assert_eq!(shown(cx, tab, &panes), NESTED));
    for (layout, expected) in [
        ("main_top", MAIN_TOP),
        ("columns", COLUMNS),
        ("rows", ROWS),
        ("main_left", MAIN_LEFT),
        ("tiled", TILED),
    ] {
        harness.invoke_with(window, &arrange_panes(layout)).unwrap();
        harness.wait_layout(window, tab, &panes, expected);
    }
    // The cycle wraps from tiled to columns, and back.
    harness.invoke(window, ids::NEXT_PANE_LAYOUT).unwrap();
    harness.wait_layout(window, tab, &panes, COLUMNS);
    harness.invoke(window, ids::PREVIOUS_PANE_LAYOUT).unwrap();
    harness.wait_layout(window, tab, &panes, TILED);
    // Balance keeps the tree and evens every split.
    harness.reweigh(tab, 0, &[700_000, 300_000]);
    harness.reweigh(tab, 1, &[200_000, 800_000]);
    harness.wait_for("the uneven weights", |cx| {
        shown(cx, tab, &panes) == "R[700000:C[200000:0,800000:1],300000:2]"
    });
    harness.invoke(window, ids::BALANCE_PANES).unwrap();
    harness.wait_layout(window, tab, &panes, TILED);
    // Without its argument, the command opens the palette on the layout
    // picker, and picking a row arranges as the explicit argument does.
    harness.invoke(window, ids::ARRANGE_PANES).unwrap();
    harness.wait_for("the layout picker", |cx| {
        palette_state(cx, window).is_some()
    });
    harness.press(window, "m a i n space l e f t enter");
    harness.wait_layout(window, tab, &panes, MAIN_LEFT);
    harness.cx.update(|cx| {
        assert!(palette_state(cx, window).is_none());
        assert_eq!(notices(cx, window), Vec::<String>::new());
        // Focus stays on the pane it was on.
        assert!(pane_focused(cx, window, tab, panes[0]));
    });
}

#[gpui::test]
fn an_unknown_layout_raises_one_notice_and_commits_nothing(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, panes) = harness.three_panes(ECHO);
    let seq = harness.runtime.lock().hierarchy_seq();
    harness.invoke_with(window, &arrange_panes("dev")).unwrap();
    harness.cx.update(|cx| {
        let shown_notices = notices(cx, window);
        assert_eq!(shown_notices.len(), 1, "{shown_notices:?}");
        assert!(
            shown_notices[0].ends_with("|No pane layout named `dev`"),
            "{shown_notices:?}"
        );
        assert_eq!(shown(cx, tab, &panes), NESTED);
        assert_eq!(arranging(cx, window), 0);
    });
    assert_eq!(harness.runtime.lock().hierarchy_seq(), seq);
}

impl DesktopHarness {
    /// Shrinks `window` until the tab holds two panes stacked but not
    /// three.
    fn shorten(&mut self, window: AnyWindowHandle) {
        let (chrome, cell) = self.update_view(window, |view, window, _| {
            let bounds = view.terminal_geometry(window).bounds.unwrap();
            (
                window.viewport_size().height - bounds.size.height,
                view.metrics.at_scale(window.scale_factor()).cell_height,
            )
        });
        // Each stacked pane needs a title bar and two rows; three of them
        // need more than this.
        let content = (pane_geometry::TITLE_BAR_HEIGHT + cell * 2.5) * 2.4;
        VisualTestContext::from_window(window, &self.cx)
            .simulate_resize(gpui::size(px(900.0), chrome + content));
        self.wait_for("the shorter window", |cx| {
            window
                .update(cx, |root, window, cx| {
                    let view = root.downcast::<WorkspaceView>().unwrap();
                    view.read(cx)
                        .terminal_geometry(window)
                        .bounds
                        .is_some_and(|bounds| bounds.size.height == content)
                })
                .unwrap_or(false)
        });
    }
}

#[gpui::test]
fn a_short_window_refuses_a_layout_and_cycling_skips_it(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, panes) = harness.three_panes(ECHO);
    harness.shorten(window);
    let seq = harness.runtime.lock().hierarchy_seq();
    harness.invoke_with(window, &arrange_panes("rows")).unwrap();
    harness.cx.update(|cx| {
        let shown_notices = notices(cx, window);
        assert_eq!(shown_notices.len(), 1, "{shown_notices:?}");
        assert!(
            shown_notices[0].ends_with("|Window too small for the rows layout"),
            "{shown_notices:?}"
        );
        assert_eq!(arranging(cx, window), 0);
    });
    assert_eq!(harness.runtime.lock().hierarchy_seq(), seq);
    // The nested tab matches main left; the step back skips rows, which
    // does not fit, and lands on columns.
    harness.invoke(window, ids::PREVIOUS_PANE_LAYOUT).unwrap();
    harness.wait_layout(window, tab, &panes, COLUMNS);
    harness
        .cx
        .update(|cx| assert_eq!(notices(cx, window).len(), 1));
}

#[gpui::test]
fn balance_lands_after_the_resizes_queued_before_it(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, panes) = harness.three_panes(ECHO);
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Layout);
    harness.invoke(window, ids::RESIZE_PANE_RIGHT).unwrap();
    harness.wait_for("the first resize to reach its worker", |_| {
        runtime.worker_hooks.held(Stage::Layout) == 1
    });
    harness.invoke(window, ids::RESIZE_PANE_RIGHT).unwrap();
    harness.invoke(window, ids::BALANCE_PANES).unwrap();
    harness.cx.update(|cx| assert_eq!(arranging(cx, window), 1));
    harness.runtime.worker_hooks.release(Stage::Layout);
    harness.wait_layout(window, tab, &panes, NESTED);
    assert_eq!(harness.runtime.worker_hooks.entered(Stage::Layout), 3);
    harness.cx.update(|cx| {
        assert_eq!(undo_label(cx, window), Some("Balance Panes"));
    });
}

/// The hierarchy sequence of `window`'s undo entry.
fn undo_seq(cx: &mut App, window: AnyWindowHandle) -> Option<u64> {
    let record = cx.global::<Desktop>().windows.record(window.window_id())?;
    Some(record.layout_history.peek(RestoreDirection::Undo)?.seq)
}

impl DesktopHarness {
    /// Unbalances `tab`'s root split and waits until the projection holds
    /// it, so a balance has something to change.
    fn unbalance(&mut self, tab: TabId, panes: &[PaneId]) {
        self.reweigh(tab, 0, &[700_000, 300_000]);
        self.wait_for("the unbalanced root", |cx| {
            shown(cx, tab, panes) == "C[700000:0,300000:R[500000:1,500000:2]]"
        });
    }

    /// Holds layout queue jobs at `stage` and runs `command`, then waits
    /// until its job waits there.
    fn hold_at(
        &mut self,
        stage: Stage,
        window: AnyWindowHandle,
        command: huterm_protocol::CommandId,
    ) {
        let runtime = Arc::clone(&self.runtime);
        runtime.worker_hooks.hold(stage);
        assert!(matches!(
            self.invoke(window, command),
            Ok(CommandOutcome::Accepted)
        ));
        self.wait_for("the arrangement to wait at its stage", |_| {
            runtime.worker_hooks.held(stage) == 1
        });
    }
}

#[gpui::test]
fn a_second_balance_commits_nothing_and_records_nothing(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, panes) = harness.three_panes(ECHO);
    harness.unbalance(tab, &panes);
    harness.invoke(window, ids::BALANCE_PANES).unwrap();
    harness.wait_layout(window, tab, &panes, NESTED);
    let entry = harness.cx.update(|cx| undo_seq(cx, window));
    assert!(entry.is_some());
    let seq = harness.runtime.lock().hierarchy_seq();
    let jobs = harness.runtime.worker_hooks.entered(Stage::Layout);
    // Accepted as done, with nothing sent.
    assert!(matches!(
        harness.invoke(window, ids::BALANCE_PANES),
        Ok(CommandOutcome::Completed)
    ));
    harness.cx.update(|cx| {
        assert_eq!(arranging(cx, window), 0);
        assert_eq!(undo_seq(cx, window), entry);
        assert_eq!(notices(cx, window), Vec::<String>::new());
    });
    assert_eq!(harness.runtime.worker_hooks.entered(Stage::Layout), jobs);
    assert_eq!(harness.runtime.lock().hierarchy_seq(), seq);
}

#[gpui::test]
fn a_split_elsewhere_between_fit_and_commit_refuses_the_arrangement(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, panes) = harness.three_panes(ECHO);
    harness.hold_at(Stage::Layout, window, ids::NEXT_PANE_LAYOUT);
    let added = harness.split(tab, panes[2], SplitDirection::Right, ECHO);
    let order = [panes[0], panes[1], panes[2], added];
    let split =
        canonical(harness.runtime.lock().tab(tab).unwrap().layout(), &order);
    harness.runtime.worker_hooks.release(Stage::Layout);
    harness.wait_for("the refusal", |cx| arranging(cx, window) == 0);
    harness.cx.update(|cx| {
        let shown_notices = notices(cx, window);
        assert_eq!(shown_notices.len(), 1, "{shown_notices:?}");
        assert!(
            shown_notices[0].ends_with("|The panes changed; try again"),
            "{shown_notices:?}"
        );
        assert_eq!(shown(cx, tab, &order), split);
        assert_eq!(undo_label(cx, window), None);
    });
}

#[gpui::test]
fn an_exit_close_that_commits_first_refuses_the_queued_arrangement(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let first = harness.first_pane(tab);
    let second = harness.split(tab, first, SplitDirection::Right, ECHO);
    let exits = harness.split(tab, second, SplitDirection::Down, EXITS);
    let panes = [first, second, exits];
    let window = harness.attach_window();
    harness.wait_for("every pane's shell", |cx| {
        panes.iter().all(|pane| {
            pane_screen(cx, window, tab, *pane)
                .is_some_and(|text| text.contains("READY"))
        })
    });
    harness.activate(window, tab);
    harness.focus(window, tab, exits);
    harness.hold_at(Stage::Layout, window, ids::NEXT_PANE_LAYOUT);
    // Typing still reaches the shell, which exits, and its exit-close
    // commits ahead of the held arrangement: no busy hold defers it.
    harness.type_line(window, "x");
    harness.wait_for("the exited pane to close", |cx| {
        layout_of(cx, tab).is_some_and(|layout| layout.pane(exits).is_none())
    });
    harness.runtime.worker_hooks.release(Stage::Layout);
    harness.wait_for("the refusal", |cx| arranging(cx, window) == 0);
    harness.cx.update(|cx| {
        let shown_notices = notices(cx, window);
        assert_eq!(shown_notices.len(), 1, "{shown_notices:?}");
        assert!(
            shown_notices[0].ends_with("|The panes changed; try again"),
            "{shown_notices:?}"
        );
        assert_eq!(shown(cx, tab, &panes), "C[500000:0,500000:1]");
        assert_eq!(
            panes_shown(cx, window, tab).map(|(panes, _)| panes),
            Some(vec![first, second])
        );
    });
}

#[gpui::test]
fn an_arrangement_in_flight_refuses_structure_but_not_typing(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, panes) = harness.three_panes(ECHO);
    harness.unbalance(tab, &panes);
    harness.invoke(window, ids::BALANCE_PANES).unwrap();
    harness.wait_layout(window, tab, &panes, NESTED);
    harness.hold_at(Stage::Layout, window, ids::NEXT_PANE_LAYOUT);
    for command in [
        ids::UNDO,
        ids::NEXT_PANE_LAYOUT,
        ids::RESIZE_PANE_RIGHT,
        ids::SPLIT_RIGHT,
    ] {
        let refused = harness.available(window, command);
        assert!(
            matches!(
                &refused,
                Err(CommandError::Unavailable(reason))
                    if reason == "pane arrangement in progress"
            ),
            "{command}: {refused:?}"
        );
    }
    // A binding's resize is refused too; a repeated cycle key is taken
    // quietly and queues nothing.
    assert!(harness.invoke(window, ids::RESIZE_PANE_RIGHT).is_err());
    assert!(matches!(
        harness.invoke(window, ids::NEXT_PANE_LAYOUT),
        Ok(CommandOutcome::Completed)
    ));
    // A divider press starts no drag.
    let start = harness.divider_center(window);
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_mouse_down(
        start,
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    visual.simulate_mouse_up(start, MouseButton::Left, gpui::Modifiers::none());
    harness.cx.update(|cx| assert!(!dragging(cx, window)));
    harness.type_line(window, "typed");
    harness.wait_for("the shell to read the typed line", |cx| {
        pane_screen(cx, window, tab, panes[0])
            .is_some_and(|text| text.contains("got:typed"))
    });
    harness.runtime.worker_hooks.release(Stage::Layout);
    harness.wait_layout(window, tab, &panes, MAIN_TOP);
    assert_eq!(harness.runtime.worker_hooks.entered(Stage::Layout), 2);
    harness.cx.update(|cx| {
        assert_eq!(undo_label(cx, window), Some("Arrange Panes: Main Top"));
    });
}

#[gpui::test]
fn balancing_cancels_this_windows_divider_drag_first(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, panes) = harness.three_panes(ECHO);
    harness.unbalance(tab, &panes);
    let start = harness.divider_center(window);
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_mouse_down(
        start,
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    visual.simulate_mouse_move(
        start + gpui::point(px(-40.0), px(0.0)),
        Some(MouseButton::Left),
        gpui::Modifiers::none(),
    );
    harness.wait_for("the drag to start", |cx| dragging(cx, window));
    harness.invoke(window, ids::BALANCE_PANES).unwrap();
    harness.cx.update(|cx| assert!(!dragging(cx, window)));
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_mouse_up(
        start + gpui::point(px(-40.0), px(0.0)),
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    harness.wait_layout(window, tab, &panes, NESTED);
    // The release committed nothing behind the balance.
    assert_eq!(harness.runtime.worker_hooks.entered(Stage::Layout), 1);
}

#[gpui::test]
fn an_earlier_drop_completing_first_keeps_its_undo_entry(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, panes) = harness.three_panes(ECHO);
    harness.hold_at(Stage::LayoutCommitted, window, ids::NEXT_PANE_LAYOUT);
    // A drop from a drag that started before the arrangement was queued
    // commits through the move path, which no availability check gates.
    let workspace = harness.workspace;
    harness.update_view(window, |view, window, cx| {
        view.commit_pane_move(
            super::super::pane_moves::PaneMove::Swap {
                workspace,
                tab,
                pane: panes[0],
                other: panes[1],
            },
            Some(panes[0]),
            None,
            window,
            cx,
        );
    });
    harness.wait_for("the swap to complete", |cx| {
        !busy(cx, window) && undo_label(cx, window) == Some("Swap Panes")
    });
    harness.runtime.worker_hooks.release(Stage::LayoutCommitted);
    harness.wait_for("the arrangement to complete", |cx| {
        arranging(cx, window) == 0
    });
    harness.cx.update(|cx| {
        assert_eq!(undo_label(cx, window), Some("Swap Panes"));
        // Main top, then the swap of its first two panes.
        assert_eq!(
            shown(cx, tab, &panes),
            "R[600000:1,400000:C[500000:0,500000:2]]"
        );
    });
}

/// What `window`'s view of `tab` keeps through a layout change: each pane
/// view's entity and viewer, the focused pane, the focus history, and the
/// view's own zoom.
fn view_state(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
    panes: &[PaneId],
) -> (
    Vec<Option<(gpui::EntityId, huterm_protocol::ViewerId)>>,
    String,
) {
    let identities = panes
        .iter()
        .map(|pane| identity(cx, window, tab, *pane))
        .collect();
    let rest = window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().unwrap();
            let view = view.read(cx);
            let tab = view.tabs.iter().find(|view| view.id == tab).unwrap();
            format!(
                "focus={:?} history={:?} zoomed={:?}",
                tab.focused().pane,
                tab.focus_history,
                tab.zoomed
            )
        })
        .unwrap();
    (identities, rest)
}

/// The zoom `window` shows for `tab`, from either scope.
fn shown_zoom(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
) -> Option<PaneId> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().unwrap();
            let view = view.read(cx);
            let tab = view.tabs.iter().find(|view| view.id == tab)?;
            view.shown_zoom(tab, cx)
        })
        .ok()
        .flatten()
}

impl DesktopHarness {
    /// A second window on the session, showing `tab` with every pane.
    fn second_window(
        &mut self,
        tab: TabId,
        panes: &[PaneId],
    ) -> AnyWindowHandle {
        let other = self.attach_window();
        self.wait_for("the second window's panes", |cx| {
            panes_shown(cx, other, tab)
                .is_some_and(|(shown, _)| shown.len() == panes.len())
        });
        other
    }

    fn toggle_zoom(
        &mut self,
        window: AnyWindowHandle,
        tab: TabId,
        pane: PaneId,
    ) {
        self.update_view(window, |view, window, cx| {
            view.toggle_zoom_of(tab, pane, window, cx);
        });
    }
}

#[gpui::test]
fn another_window_converges_and_keeps_its_views_focus_and_zoom(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, panes) = harness.three_panes(ECHO);
    let other = harness.second_window(tab, &panes);
    harness.update_view(other, |view, window, cx| {
        view.select(tab, window, cx);
        view.focus_pane(tab, panes[1], window, cx);
        view.focus_pane(tab, panes[2], window, cx);
    });
    harness.toggle_zoom(other, tab, panes[2]);
    let before = harness.cx.update(|cx| view_state(cx, other, tab, &panes));
    assert!(before.1.contains("zoomed=Some"), "{before:?}");
    harness
        .invoke_with(window, &arrange_panes("main_top"))
        .unwrap();
    harness.wait_layout(window, tab, &panes, MAIN_TOP);
    harness.cx.update(|cx| {
        assert_eq!(view_state(cx, other, tab, &panes), before);
        assert_eq!(shown_zoom(cx, other, tab), Some(panes[2]));
    });
}

#[gpui::test]
fn the_arranging_window_unzooms_its_view_and_keeps_focus(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, panes) = harness.three_panes(ECHO);
    harness.focus(window, tab, panes[1]);
    harness.toggle_zoom(window, tab, panes[1]);
    harness.cx.update(|cx| {
        assert_eq!(shown_zoom(cx, window, tab), Some(panes[1]));
    });
    harness.invoke(window, ids::NEXT_PANE_LAYOUT).unwrap();
    harness
        .cx
        .update(|cx| assert_eq!(shown_zoom(cx, window, tab), None));
    harness.wait_layout(window, tab, &panes, MAIN_TOP);
    harness.wait_for("focus to stay on the zoomed pane", |cx| {
        pane_focused(cx, window, tab, panes[1])
    });
}

#[gpui::test]
fn a_refused_arrangement_gives_the_view_its_zoom_back(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, panes) = harness.three_panes(ECHO);
    harness.focus(window, tab, panes[1]);
    harness.toggle_zoom(window, tab, panes[1]);
    harness.hold_at(Stage::Layout, window, ids::NEXT_PANE_LAYOUT);
    harness
        .cx
        .update(|cx| assert_eq!(shown_zoom(cx, window, tab), None));
    harness.split(tab, panes[0], SplitDirection::Down, ECHO);
    harness.runtime.worker_hooks.release(Stage::Layout);
    harness.wait_for("the refusal", |cx| arranging(cx, window) == 0);
    harness.cx.update(|cx| {
        assert_eq!(notices(cx, window).len(), 1);
        assert_eq!(shown_zoom(cx, window, tab), Some(panes[1]));
    });
}

#[gpui::test]
fn an_arrangement_another_writer_made_empty_gives_the_zoom_back(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, panes) = harness.three_panes(ECHO);
    harness.unbalance(tab, &panes);
    harness.focus(window, tab, panes[2]);
    harness.toggle_zoom(window, tab, panes[2]);
    harness.hold_at(Stage::Layout, window, ids::BALANCE_PANES);
    harness
        .cx
        .update(|cx| assert_eq!(shown_zoom(cx, window, tab), None));
    // Another window balances first, so this commit changes nothing.
    harness.reweigh(tab, 0, &[500_000, 500_000]);
    harness.runtime.worker_hooks.release(Stage::Layout);
    harness.wait_layout(window, tab, &panes, NESTED);
    harness.cx.update(|cx| {
        assert_eq!(notices(cx, window), Vec::<String>::new());
        assert_eq!(undo_label(cx, window), None);
        assert_eq!(shown_zoom(cx, window, tab), Some(panes[2]));
    });
}

#[gpui::test]
fn a_shared_zoom_clears_in_the_arrangements_own_event_and_undo_restores_it(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.panes.zoom = huterm_config::PaneZoomScope::Shared;
    });
    let (window, tab, panes) = harness.three_panes(ECHO);
    let other = harness.second_window(tab, &panes);
    harness
        .runtime
        .lock()
        .set_pane_zoom(harness.workspace, tab, Some(panes[1]))
        .unwrap();
    harness.wait_for("both windows to zoom", |cx| {
        shown_zoom(cx, window, tab) == Some(panes[1])
            && shown_zoom(cx, other, tab) == Some(panes[1])
    });
    let (_, events) = harness.runtime.lock().subscribe_hierarchy();
    harness.invoke(window, ids::NEXT_PANE_LAYOUT).unwrap();
    harness.wait_layout(window, tab, &panes, MAIN_TOP);
    let mut layouts = 0;
    while let Ok(envelope) = events.try_recv() {
        match envelope.event {
            HierarchyEvent::PaneLayoutChanged { zoomed, .. } => {
                assert_eq!(zoomed, None);
                layouts += 1;
            }
            HierarchyEvent::PaneZoomChanged { .. } => {
                panic!("the zoom cleared in its own event")
            }
            _ => {}
        }
    }
    assert_eq!(layouts, 1);
    harness.wait_for("both windows to unzoom", |cx| {
        shown_zoom(cx, window, tab).is_none()
            && shown_zoom(cx, other, tab).is_none()
    });
    harness.invoke(window, ids::UNDO).unwrap();
    harness.wait_layout(window, tab, &panes, NESTED);
    harness.wait_for("undo to restore the shared zoom", |cx| {
        shown_zoom(cx, window, tab) == Some(panes[1])
            && shown_zoom(cx, other, tab) == Some(panes[1])
    });
}

#[gpui::test]
fn another_windows_drag_cancels_under_a_layout_and_continues_under_a_balance(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, panes) = harness.three_panes(ECHO);
    let other = harness.second_window(tab, &panes);
    harness.update_view(other, |view, window, cx| {
        view.select(tab, window, cx);
    });
    let drag = |harness: &mut DesktopHarness| {
        harness.wait_for("the other window to draw its dividers", |cx| {
            other
                .update(cx, |root, _, cx| {
                    let view = root.downcast::<WorkspaceView>().unwrap();
                    !view.read(cx).drawn_dividers.borrow().is_empty()
                })
                .unwrap_or(false)
        });
        let start = harness.divider_center(other);
        let mut visual = VisualTestContext::from_window(other, &harness.cx);
        visual.simulate_mouse_down(
            start,
            MouseButton::Left,
            gpui::Modifiers::none(),
        );
        visual.simulate_mouse_move(
            start + gpui::point(px(-20.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::none(),
        );
        harness.wait_for("the drag to start", |cx| dragging(cx, other));
        start
    };
    drag(&mut harness);
    harness.invoke(window, ids::NEXT_PANE_LAYOUT).unwrap();
    harness.wait_layout(window, tab, &panes, MAIN_TOP);
    harness
        .wait_for("a new shape to cancel the drag", |cx| !dragging(cx, other));
    let start = drag(&mut harness);
    harness.invoke(window, ids::BALANCE_PANES).unwrap();
    harness.wait_layout(
        window,
        tab,
        &panes,
        "R[500000:0,500000:C[500000:1,500000:2]]",
    );
    harness.cx.update(|cx| assert!(dragging(cx, other)));
    let mut visual = VisualTestContext::from_window(other, &harness.cx);
    visual.simulate_mouse_up(
        start + gpui::point(px(-20.0), px(0.0)),
        MouseButton::Left,
        gpui::Modifiers::none(),
    );
    harness.wait_for("the drag to end", |cx| !dragging(cx, other));
}

#[gpui::test]
fn undo_restores_a_layout_or_a_balance_exactly_until_a_split_prunes_it(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, panes) = harness.three_panes(ECHO);
    let current = |harness: &mut DesktopHarness| {
        harness.cx.update(|cx| layout_of(cx, tab).unwrap())
    };
    let nested = current(&mut harness);
    harness
        .invoke_with(window, &arrange_panes("tiled"))
        .unwrap();
    harness.wait_layout(window, tab, &panes, TILED);
    harness.invoke(window, ids::UNDO).unwrap();
    harness.wait_for("undo to restore the nested tree", |cx| {
        !busy(cx, window) && layout_of(cx, tab).as_ref() == Some(&nested)
    });
    harness.unbalance(tab, &panes);
    let unbalanced = current(&mut harness);
    harness.invoke(window, ids::BALANCE_PANES).unwrap();
    harness.wait_layout(window, tab, &panes, NESTED);
    harness.cx.update(|cx| {
        assert_eq!(undo_label(cx, window), Some("Balance Panes"));
    });
    harness.invoke(window, ids::UNDO).unwrap();
    harness.wait_for("undo to restore the weights", |cx| {
        !busy(cx, window) && layout_of(cx, tab).as_ref() == Some(&unbalanced)
    });
    harness.invoke(window, ids::BALANCE_PANES).unwrap();
    harness.wait_layout(window, tab, &panes, NESTED);
    harness.split(tab, panes[0], SplitDirection::Down, ECHO);
    harness.wait_for("the split to prune the entry", |cx| {
        undo_label(cx, window).is_none()
    });
}

/// Prints `READY`, reads a count without echoing it, prints a line of that
/// many `w`s and `END`, then idles.
const PRINTS_A_LINE: &str = "stty -echo; printf 'READY\\n'; read n; i=0; s=; \
     while [ \"$i\" -lt \"$n\" ]; do s=\"${s}w\"; i=$((i+1)); done; \
     printf '%s\\nEND\\n' \"$s\"; while read x; do :; done";

/// The lengths of the runs of `w` that start each row of `snapshot`.
fn runs(snapshot: &TerminalSnapshot) -> Vec<usize> {
    snapshot
        .rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .take_while(|cell| cell.text.as_str() == "w")
                .count()
        })
        .filter(|run| *run > 0)
        .collect()
}

fn view_of(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
    pane: PaneId,
) -> (
    GridSize,
    Option<GridSize>,
    u64,
    Option<Arc<TerminalSnapshot>>,
) {
    let view = pane_view(cx, window, tab, pane).unwrap();
    let view = view.read(cx);
    (
        view.requested_grid,
        view.displayed_grid(),
        view.resize_requests,
        view.snapshot.clone(),
    )
}

#[gpui::test]
fn a_layout_reflows_a_shrunk_pane_and_leaves_an_unchanged_one_alone(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    // `[(A | B) over C]`, A at 70 % of its row.
    let tab = harness.open_tab(PRINTS_A_LINE);
    let printing = harness.first_pane(tab);
    let idle = harness.split(tab, printing, SplitDirection::Down, ECHO);
    let beside = harness.split(tab, printing, SplitDirection::Right, ECHO);
    harness.reweigh(tab, 1, &[700_000, 300_000]);
    let panes = [printing, beside, idle];
    let window = harness.attach_window();
    harness.wait_for("every pane at its own grid", |cx| {
        shown(cx, tab, &panes) == "R[500000:C[700000:0,300000:1],500000:2]"
            && panes.iter().all(|pane| {
                let (requested, displayed, _, _) =
                    view_of(cx, window, tab, *pane);
                pane_screen(cx, window, tab, *pane)
                    .is_some_and(|text| text.contains("READY"))
                    && displayed == Some(requested)
            })
    });
    harness.activate(window, tab);
    harness.focus(window, tab, printing);
    let wide = harness.cx.update(|cx| view_of(cx, window, tab, printing).0);
    let length = usize::from(wide.columns) - 1;
    harness.type_line(window, &length.to_string());
    harness.wait_for("the line", |cx| {
        view_of(cx, window, tab, printing)
            .3
            .is_some_and(|snapshot| runs(&snapshot) == [length])
    });
    let (kept_grid, _, kept_resizes, kept) =
        harness.cx.update(|cx| view_of(cx, window, tab, idle));
    let (_, _, resizes, _) =
        harness.cx.update(|cx| view_of(cx, window, tab, printing));

    // Balancing narrows A to half its row and moves nothing else in C.
    harness.invoke(window, ids::BALANCE_PANES).unwrap();
    harness.wait_without_frames("the shrunk pane's new grid", |cx| {
        view_of(cx, window, tab, printing).0.columns < wide.columns
    });
    let (narrow, _, after, _) =
        harness.cx.update(|cx| view_of(cx, window, tab, printing));
    assert_eq!(after, resizes + 1, "one new grid");
    // The runtime may answer before any frame, so the clipped old snapshot
    // is `a_larger_canonical_grid_is_clipped_and_owned_gestures_survive_it`'s
    // to check; here the rows must come from the new size.
    harness.wait_for("the reflowed snapshot", |cx| {
        let (requested, displayed, _, snapshot) =
            view_of(cx, window, tab, printing);
        displayed == Some(requested)
            && requested == narrow
            && snapshot.is_some_and(|snapshot| {
                runs(&snapshot)
                    == [
                        usize::from(narrow.columns),
                        length - usize::from(narrow.columns),
                    ]
            })
    });
    harness.cx.update(|cx| {
        let (grid, _, resizes, snapshot) = view_of(cx, window, tab, idle);
        assert_eq!(grid, kept_grid);
        assert_eq!(resizes, kept_resizes, "the unchanged pane resized");
        assert!(
            snapshot
                .zip(kept.clone())
                .is_some_and(|(now, before)| { Arc::ptr_eq(&now, &before) }),
            "the unchanged pane took a new snapshot"
        );
    });

    // Main top gives A the whole width again, and the line unwraps.
    harness
        .invoke_with(window, &arrange_panes("main_top"))
        .unwrap();
    harness.wait_for("the unwrapped line", |cx| {
        let (requested, displayed, _, snapshot) =
            view_of(cx, window, tab, printing);
        displayed == Some(requested)
            && requested.columns > wide.columns
            && snapshot.is_some_and(|snapshot| runs(&snapshot) == [length])
    });
}
