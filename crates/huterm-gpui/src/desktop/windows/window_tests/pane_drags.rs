//! Tier 2 tests for drags: a pane dragged by its title bar onto a pane
//! zone or the tab bar, a tab dragged onto a pane, cancellation, and
//! spring-loaded tabs driven with synthetic instants.

use gpui::EntityId;

use super::super::pane_drag::{DropTarget, SPRING_LOAD_DELAY};
use super::super::pane_geometry::Zone;
use super::super::refresh::Animated;
use super::*;

/// The view entities of `tab`'s panes in `window`, in view order.
fn entities(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
    panes: &[PaneId],
) -> Vec<Option<EntityId>> {
    panes
        .iter()
        .map(|pane| {
            pane_view(cx, window, tab, *pane).map(|view| view.entity_id())
        })
        .collect()
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

/// The pane drag's target and armed spring tab in `window`, if dragging.
fn pane_drag(
    cx: &mut App,
    window: AnyWindowHandle,
) -> Option<(Option<DropTarget>, Option<TabId>)> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            let drag = view.read(cx).pane_drag.as_ref()?;
            drag.dragging.then_some((drag.target, drag.spring.tab()))
        })
        .ok()
        .flatten()
}

fn no_drag(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>().is_ok_and(|view| {
                let view = view.read(cx);
                view.pane_drag.is_none() && view.reorder.is_none() && !view.busy
            })
        })
        .unwrap_or(false)
}

fn target_is(
    target: Option<DropTarget>,
    pane: PaneId,
    zone: Zone,
    refused: bool,
) -> bool {
    matches!(
        target,
        Some(DropTarget::Pane { pane: p, zone: z, refused: r, .. })
            if p == pane && z == zone && r == refused
    )
}

impl DesktopHarness {
    /// The drawn frame of `pane` in `window`'s active tab.
    fn frame(
        &mut self,
        window: AnyWindowHandle,
        pane: PaneId,
    ) -> Bounds<Pixels> {
        self.update_view(window, |view, _, _| {
            view.drawn_panes.borrow().as_ref().and_then(|(_, rects)| {
                rects
                    .iter()
                    .find(|rect| rect.pane == pane)
                    .map(|rect| rect.frame)
            })
        })
        .expect("a drawn pane")
    }

    /// Where a press grabs `pane`'s title bar for a drag.
    fn grab(
        &mut self,
        window: AnyWindowHandle,
        pane: PaneId,
    ) -> gpui::Point<Pixels> {
        self.update_view(window, |view, _, _| {
            view.drawn_panes
                .borrow()
                .as_ref()
                .and_then(|(_, rects)| {
                    rects.iter().find(|rect| rect.pane == pane)?.title_bar
                })
                .map(|bar| gpui::point(bar.origin.x + px(40.0), bar.center().y))
        })
        .expect("a drawn title bar")
    }

    /// The centre of tab `index`'s label in `window`'s tab bar.
    fn label(
        &mut self,
        window: AnyWindowHandle,
        index: usize,
    ) -> gpui::Point<Pixels> {
        self.update_view(window, |view, window, _| {
            view.tab_strip(window).label(index).center()
        })
    }

    fn press_at(&mut self, window: AnyWindowHandle, at: gpui::Point<Pixels>) {
        VisualTestContext::from_window(window, &self.cx).simulate_mouse_down(
            at,
            MouseButton::Left,
            gpui::Modifiers::none(),
        );
    }

    fn move_to(&mut self, window: AnyWindowHandle, at: gpui::Point<Pixels>) {
        VisualTestContext::from_window(window, &self.cx).simulate_mouse_move(
            at,
            Some(MouseButton::Left),
            gpui::Modifiers::none(),
        );
    }

    fn release_at(&mut self, window: AnyWindowHandle, at: gpui::Point<Pixels>) {
        VisualTestContext::from_window(window, &self.cx).simulate_mouse_up(
            at,
            MouseButton::Left,
            gpui::Modifiers::none(),
        );
    }

    /// Selects `tab` in `window` and waits for its panes to draw.
    fn show(&mut self, window: AnyWindowHandle, tab: TabId) {
        self.update_view(window, |view, window, cx| {
            view.select(tab, window, cx);
        });
        self.wait_for("the tab to show", |cx| {
            active(cx, window) == Some(tab)
                && window
                    .update(cx, |root, _, cx| {
                        root.downcast::<WorkspaceView>().is_ok_and(|view| {
                            view.read(cx)
                                .drawn_panes
                                .borrow()
                                .as_ref()
                                .is_some_and(|(drawn, _)| *drawn == tab)
                        })
                    })
                    .unwrap_or(false)
        });
    }

    /// The hierarchy sequence the projection has applied.
    fn seq(&mut self) -> u64 {
        self.cx
            .update(|cx| cx.global::<Desktop>().hierarchy.state().seq())
    }

    /// Runs `window`'s animation step at `now`, as its frame clock would.
    fn advance(&mut self, window: AnyWindowHandle, now: Instant) {
        self.update_view(window, |view, window, cx| {
            view.advance_animation(now, false, window, cx);
        });
    }

    /// When the armed spring load began, with the schedule's deadline.
    fn armed(
        &mut self,
        window: AnyWindowHandle,
    ) -> (Instant, AnimationSchedule) {
        self.update_view(window, |view, _, _| {
            let armed = view
                .pane_drag
                .as_ref()
                .and_then(|drag| drag.spring.armed_at())
                .expect("an armed spring load");
            (armed, view.animation_schedule(armed))
        })
    }
}

/// A window showing a tab split into `[left | right]`, and a second tab of
/// one pane.
fn two_tabs(
    harness: &mut DesktopHarness,
) -> (AnyWindowHandle, TabId, PaneId, PaneId, TabId, PaneId) {
    let tab = harness.open_tab(ECHO);
    let other = harness.open_tab(ECHO);
    let (window, left, right) = harness.split_window(tab);
    let lone = harness.first_pane(other);
    harness.wait_for("the other tab to install", |cx| {
        pane_view(cx, window, other, lone).is_some()
    });
    (window, tab, left, right, other, lone)
}

#[gpui::test]
fn a_pane_dropped_on_an_edge_moves_and_on_a_centre_swaps(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, left, right) = harness.split_window(tab);
    let views = harness
        .cx
        .update(|cx| entities(cx, window, tab, &[left, right]));
    // The left pane onto the right pane's right edge moves it there.
    let grab = harness.grab(window, left);
    let frame = harness.frame(window, right);
    let edge = gpui::point(frame.right() - px(8.0), frame.center().y);
    harness.press_at(window, grab);
    harness.move_to(window, edge);
    harness.wait_for("the drag to target the right edge", |cx| {
        pane_drag(cx, window).is_some_and(|(target, _)| {
            target_is(target, right, Zone::Edge(SplitDirection::Right), false)
        })
    });
    harness.release_at(window, edge);
    harness.wait_for("the pane to move to the right", |cx| {
        no_drag(cx, window)
            && panes_shown(cx, window, tab) == Some((vec![right, left], left))
    });
    // Onto the other pane's centre, the two swap back.
    let grab = harness.grab(window, left);
    let centre = harness.frame(window, right).center();
    harness.press_at(window, grab);
    harness.move_to(window, centre);
    harness.wait_for("the drag to target the centre", |cx| {
        pane_drag(cx, window).is_some_and(|(target, _)| {
            target_is(target, right, Zone::Centre, false)
        })
    });
    harness.release_at(window, centre);
    harness.wait_for("the panes to swap", |cx| {
        no_drag(cx, window)
            && panes_shown(cx, window, tab) == Some((vec![left, right], left))
    });
    // The same views throughout, and no terminal saw the presses.
    harness.cx.update(|cx| {
        assert_eq!(entities(cx, window, tab, &[left, right]), views);
        for pane in [left, right] {
            let view = pane_view(cx, window, tab, pane).unwrap();
            assert!(view.read(cx).selection.is_none());
        }
        assert_eq!(notices(cx, window), Vec::<String>::new());
    });
}

#[gpui::test]
fn a_pane_dropped_on_a_tab_bar_slot_opens_a_tab_there(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, left, right, other, _) = two_tabs(&mut harness);
    // The slot between the two tabs: the outer quarter of the first label.
    let label = harness
        .update_view(window, |view, window, _| view.tab_strip(window).label(0));
    let slot = gpui::point(label.right() - px(4.0), label.center().y);
    let grab = harness.grab(window, right);
    harness.press_at(window, grab);
    harness.move_to(window, slot);
    harness.wait_for("the drag to target the slot", |cx| {
        pane_drag(cx, window)
            .is_some_and(|(target, _)| target == Some(DropTarget::Slot(1)))
    });
    harness.release_at(window, slot);
    harness.wait_for("the pane to open a tab after its own", |cx| {
        no_drag(cx, window)
            && active(cx, window).is_some_and(|extracted| {
                extracted != tab
                    && extracted != other
                    && panes_shown(cx, window, extracted)
                        == Some((vec![right], right))
            })
    });
    let order = harness.update_view(window, |view, _, _| {
        view.tabs.iter().map(|view| view.id).collect::<Vec<_>>()
    });
    assert_eq!(order.len(), 3);
    assert_eq!((order[0], order[2]), (tab, other));
    harness.cx.update(|cx| {
        assert_eq!(panes_shown(cx, window, tab), Some((vec![left], left)));
    });
}

#[gpui::test]
fn escape_a_release_without_a_target_or_outside_the_window_change_nothing(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, left, right) = harness.split_window(tab);
    let seq = harness.seq();
    let grab = harness.grab(window, left);
    let centre = harness.frame(window, right).center();
    // Escape mid-drag cancels.
    harness.press_at(window, grab);
    harness.move_to(window, centre);
    harness.wait_for("the drag to start", |cx| pane_drag(cx, window).is_some());
    harness
        .cx
        .dispatch_keystroke(window, Keystroke::parse("escape").unwrap());
    harness.wait_for("Escape to cancel the drag", |cx| no_drag(cx, window));
    harness.release_at(window, centre);
    // A release over the dragged pane itself has no target.
    let own = harness.frame(window, left).center();
    harness.press_at(window, grab);
    harness.move_to(window, own);
    harness.wait_for("the drag to start over its own pane", |cx| {
        pane_drag(cx, window).is_some_and(|(target, _)| target.is_none())
    });
    harness.release_at(window, own);
    harness.wait_for("the release to end the drag", |cx| no_drag(cx, window));
    // Nor does a release outside the window.
    let outside = gpui::point(px(-40.0), own.y);
    harness.press_at(window, grab);
    harness.move_to(window, centre);
    harness.wait_for("the drag to start again", |cx| {
        pane_drag(cx, window).is_some()
    });
    harness.move_to(window, outside);
    harness.release_at(window, outside);
    harness.wait_for("the outside release to end the drag", |cx| {
        no_drag(cx, window)
    });
    assert_eq!(harness.seq(), seq);
    harness.cx.update(|cx| {
        assert_eq!(
            panes_shown(cx, window, tab),
            Some((vec![left, right], left))
        );
    });
}

#[gpui::test]
fn a_tab_dragged_deep_onto_a_pane_edge_nests_and_a_shallow_dip_reorders(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let second = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.activate(window, first);
    let kept = harness.first_pane(first);
    let nested = harness.first_pane(second);
    harness.wait_for("both tabs to install", |cx| {
        pane_view(cx, window, second, nested).is_some()
    });
    harness.show(window, first);
    let bar_bottom = harness.update_view(window, |view, window, _| {
        view.chrome_layout(window).tabs.bottom()
    });
    // A shallow dip below the bar still reorders.
    let label = harness.label(window, 1);
    let dip = gpui::point(label.x - px(200.0), bar_bottom + px(10.0));
    harness.press_at(window, label);
    harness.move_to(window, dip);
    harness.release_at(window, dip);
    harness.wait_for("the shallow dip to reorder", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    let view = view.read(cx);
                    !view.busy
                        && view
                            .tabs
                            .iter()
                            .map(|tab| tab.id)
                            .collect::<Vec<_>>()
                            == [second, first]
                })
            })
            .unwrap_or(false)
    });
    // Deep onto the active tab's pane centre, a release does nothing.
    harness.show(window, first);
    let seq = harness.seq();
    let label = harness.label(window, 0);
    let frame = harness.frame(window, kept);
    harness.press_at(window, label);
    harness.move_to(window, frame.center());
    harness.wait_for("pane mode over the centre", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx)
                        .reorder
                        .as_ref()
                        .is_some_and(|drag| drag.nesting && drag.nest.is_none())
                })
            })
            .unwrap_or(false)
    });
    harness.release_at(window, frame.center());
    harness.wait_for("the drag to end", |cx| no_drag(cx, window));
    assert_eq!(harness.seq(), seq);
    // A menu opening over pane mode cancels the drag, so the release that
    // follows nests nothing.
    let edge = gpui::point(frame.center().x, frame.bottom() - px(8.0));
    harness.press_at(window, label);
    harness.move_to(window, edge);
    harness.wait_for("pane mode over the edge", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx)
                        .reorder
                        .as_ref()
                        .is_some_and(|drag| drag.nest.is_some())
                })
            })
            .unwrap_or(false)
    });
    harness.invoke(window, ids::OPEN_MENU).unwrap();
    harness.wait_for("the menu to cancel the drag", |cx| no_drag(cx, window));
    harness.release_at(window, edge);
    harness
        .cx
        .dispatch_keystroke(window, Keystroke::parse("escape").unwrap());
    assert_eq!(harness.seq(), seq);
    assert!(harness.cx.update(|cx| projected(cx, second)));
    // Deep onto its bottom edge, the tab nests below the pane.
    harness.press_at(window, label);
    harness.move_to(window, edge);
    harness.release_at(window, edge);
    harness.wait_for("the tab to nest below the pane", |cx| {
        no_drag(cx, window)
            && !projected(cx, second)
            && panes_shown(cx, window, first)
                .is_some_and(|(panes, _)| panes == [kept, nested])
    });
}

#[gpui::test]
fn a_pane_resting_on_a_label_spring_loads_its_tab_at_the_deadline(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, left, right, other, lone) = two_tabs(&mut harness);
    let grab = harness.grab(window, right);
    let label = harness.label(window, 1);
    harness.press_at(window, grab);
    harness.move_to(window, label);
    harness.wait_for("the drag to arm the other tab", |cx| {
        pane_drag(cx, window).is_some_and(|(_, armed)| armed == Some(other))
    });
    let (armed, schedule) = harness.armed(window);
    assert_eq!(schedule, AnimationSchedule::at(armed + SPRING_LOAD_DELAY));
    // One millisecond early, nothing activates.
    harness.advance(
        window,
        (armed + SPRING_LOAD_DELAY)
            .checked_sub(Duration::from_millis(1))
            .unwrap(),
    );
    assert_eq!(harness.cx.update(|cx| active(cx, window)), Some(tab));
    harness.advance(window, armed + SPRING_LOAD_DELAY);
    harness.wait_for("the other tab to activate and take focus", |cx| {
        active(cx, window) == Some(other) && focused(cx, window, other)
    });
    // Its pane offers edges, not a centre, and a drop moves the pane there.
    harness.show_drawn(window, other);
    let frame = harness.frame(window, lone);
    harness.move_to(window, frame.center());
    harness.wait_for("no centre in another tab", |cx| {
        pane_drag(cx, window).is_some_and(|(target, _)| target.is_none())
    });
    let edge = gpui::point(frame.right() - px(8.0), frame.center().y);
    harness.move_to(window, edge);
    harness.release_at(window, edge);
    harness.wait_for("the pane to move into the sprung tab", |cx| {
        no_drag(cx, window)
            && active(cx, window) == Some(other)
            && panes_shown(cx, window, other)
                == Some((vec![lone, right], right))
    });
    harness.cx.update(|cx| {
        assert_eq!(panes_shown(cx, window, tab), Some((vec![left], left)));
    });
}

#[gpui::test]
fn a_spring_loaded_drag_switches_back_when_it_ends_without_a_drop(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, left, right, other, _) = two_tabs(&mut harness);
    let seq = harness.seq();
    for end in ["escape", "release"] {
        harness.show(window, tab);
        let grab = harness.grab(window, right);
        let label = harness.label(window, 1);
        harness.press_at(window, grab);
        harness.move_to(window, label);
        harness.wait_for("the drag to arm the other tab", |cx| {
            pane_drag(cx, window).is_some_and(|(_, armed)| armed == Some(other))
        });
        let (armed, _) = harness.armed(window);
        harness.advance(window, armed + SPRING_LOAD_DELAY);
        harness.wait_for("the other tab to activate", |cx| {
            active(cx, window) == Some(other)
        });
        if end == "escape" {
            harness.cx.dispatch_keystroke(
                window,
                Keystroke::parse("escape").unwrap(),
            );
            harness.release_at(window, label);
        } else {
            // A release on the now-active label's middle has no target.
            harness.release_at(window, label);
        }
        harness.wait_for(
            "the drag to switch back with the pane focused",
            |cx| {
                no_drag(cx, window)
                    && active(cx, window) == Some(tab)
                    && panes_shown(cx, window, tab)
                        .is_some_and(|(_, focus)| focus == right)
                    && focused(cx, window, tab)
            },
        );
    }
    assert_eq!(harness.seq(), seq);
    harness.cx.update(|cx| {
        assert_eq!(
            panes_shown(cx, window, tab).map(|(panes, _)| panes),
            Some(vec![left, right])
        );
    });
}

#[gpui::test]
fn a_tab_switch_the_drag_did_not_make_cancels_it_without_switching_back(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, _, right, other, _) = two_tabs(&mut harness);
    let grab = harness.grab(window, right);
    let centre = harness.frame(window, right).center();
    harness.press_at(window, grab);
    harness.move_to(window, centre);
    harness.wait_for("the drag to start", |cx| pane_drag(cx, window).is_some());
    harness.invoke(window, ids::NEXT_TAB).unwrap();
    harness.wait_for("the switch to cancel the drag", |cx| {
        no_drag(cx, window) && active(cx, window) == Some(other)
    });
    harness.release_at(window, centre);
    harness.cx.update(|cx| {
        assert_eq!(active(cx, window), Some(other));
        assert!(projected(cx, tab));
    });
}

impl DesktopHarness {
    /// Waits for `tab`, already active in `window`, to draw its panes.
    fn show_drawn(&mut self, window: AnyWindowHandle, tab: TabId) {
        self.wait_for("the tab's panes to draw", |cx| {
            window
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>().is_ok_and(|view| {
                        view.read(cx)
                            .drawn_panes
                            .borrow()
                            .as_ref()
                            .is_some_and(|(drawn, _)| *drawn == tab)
                    })
                })
                .unwrap_or(false)
        });
    }
}

#[gpui::test]
fn a_refused_zone_takes_no_drop(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, left, right) = harness.split_window(tab);
    // About six and a half rows: stacking the dragged pane on the other
    // gives both title bars and leaves each half under two rows.
    let (chrome, cell) = harness.update_view(window, |view, window, _| {
        let bounds = view.terminal_geometry(window).bounds.unwrap();
        (
            window.viewport_size().height - bounds.size.height,
            view.metrics.at_scale(window.scale_factor()).cell_height,
        )
    });
    let frame = harness.frame(window, right);
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_resize(gpui::size(px(800.0), chrome + cell * 6.5));
    harness.wait_for("the panes to draw at the short height", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx).drawn_frame(right).is_some_and(|drawn| {
                        drawn.size.height < frame.size.height
                    })
                })
            })
            .unwrap_or(false)
    });
    let seq = harness.seq();
    let frame = harness.frame(window, right);
    let bottom = gpui::point(frame.center().x, frame.bottom() - px(4.0));
    let grab = harness.grab(window, left);
    harness.press_at(window, grab);
    harness.move_to(window, bottom);
    harness.wait_for("the bottom edge to draw refused", |cx| {
        pane_drag(cx, window).is_some_and(|(target, _)| {
            target_is(target, right, Zone::Edge(SplitDirection::Down), true)
        })
    });
    harness.release_at(window, bottom);
    harness.wait_for("the release to end the drag", |cx| no_drag(cx, window));
    assert_eq!(harness.seq(), seq);
    harness.cx.update(|cx| {
        assert_eq!(
            panes_shown(cx, window, tab).map(|(panes, _)| panes),
            Some(vec![left, right])
        );
        assert_eq!(notices(cx, window), Vec::<String>::new());
    });
}

#[gpui::test]
fn blur_cancels_a_spring_loaded_pane_drag_and_a_held_tab_nest(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, left, right, other, _) = two_tabs(&mut harness);
    let seq = harness.seq();
    // A pane drag that spring-loaded switches back when the window blurs.
    let grab = harness.grab(window, right);
    let label = harness.label(window, 1);
    harness.press_at(window, grab);
    harness.move_to(window, label);
    harness.wait_for("the drag to arm the other tab", |cx| {
        pane_drag(cx, window).is_some_and(|(_, armed)| armed == Some(other))
    });
    let (armed, _) = harness.armed(window);
    harness.advance(window, armed + SPRING_LOAD_DELAY);
    harness.wait_for("the other tab to activate", |cx| {
        active(cx, window) == Some(other)
    });
    harness.deactivate(window);
    harness.wait_for("blur to cancel and switch back", |cx| {
        no_drag(cx, window) && active(cx, window) == Some(tab)
    });
    harness.release_at(window, label);
    // A tab held over a pane edge in pane mode nests nothing after a blur.
    harness.activate(window, tab);
    harness.show_drawn(window, tab);
    let frame = harness.frame(window, left);
    let edge = gpui::point(frame.center().x, frame.bottom() - px(8.0));
    harness.press_at(window, label);
    harness.move_to(window, edge);
    harness.wait_for("pane mode over the edge", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx)
                        .reorder
                        .as_ref()
                        .is_some_and(|drag| drag.nest.is_some())
                })
            })
            .unwrap_or(false)
    });
    harness.deactivate(window);
    harness.wait_for("blur to cancel the tab drag", |cx| no_drag(cx, window));
    harness.release_at(window, edge);
    assert_eq!(harness.seq(), seq);
    harness.cx.update(|cx| {
        assert!(projected(cx, other));
        assert_eq!(
            panes_shown(cx, window, tab).map(|(panes, _)| panes),
            Some(vec![left, right])
        );
    });
}

#[gpui::test]
fn a_still_pointer_finds_its_target_once_the_sprung_tab_draws(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, _, _, right, other, lone) = two_tabs(&mut harness);
    // Where the lone pane will be drawn: the active tab's whole content.
    let frame = harness.frame(window, right);
    let content = harness.update_view(window, |view, window, _| {
        view.terminal_geometry(window).bounds.unwrap()
    });
    let grab = harness.grab(window, right);
    let label = harness.label(window, 1);
    harness.press_at(window, grab);
    harness.move_to(window, label);
    harness.wait_for("the drag to arm the other tab", |cx| {
        pane_drag(cx, window).is_some_and(|(_, armed)| armed == Some(other))
    });
    let (armed, _) = harness.armed(window);
    // The pointer reaches the pane before the sprung tab has drawn, so the
    // first resolution finds no drawn pane there; it must not stay stale.
    let edge = gpui::point(content.right() - px(8.0), frame.center().y);
    harness.update_view(window, |view, window, cx| {
        view.advance_animation(armed + SPRING_LOAD_DELAY, false, window, cx);
        view.update_pane_drag(edge, window, cx);
    });
    harness.wait_for("the drawn pane to become the target", |cx| {
        pane_drag(cx, window).is_some_and(|(target, _)| {
            target_is(target, lone, Zone::Edge(SplitDirection::Right), false)
        })
    });
    harness.release_at(window, edge);
    harness.wait_for("the pane to move beside the lone pane", |cx| {
        no_drag(cx, window)
            && panes_shown(cx, window, other)
                .is_some_and(|(panes, _)| panes == [lone, right])
    });
}

#[gpui::test]
fn a_pane_drag_under_way_still_drops_while_an_arrangement_is_in_flight(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, left, right) = harness.split_window(tab);
    // The drag starts first, from the columns layout.
    let grab = harness.grab(window, left);
    let frame = harness.frame(window, right);
    let edge = gpui::point(frame.center().x, frame.bottom() - px(8.0));
    harness.press_at(window, grab);
    harness.move_to(window, edge);
    harness.wait_for("the drag to target the bottom edge", |cx| {
        pane_drag(cx, window).is_some_and(|(target, _)| {
            target_is(target, right, Zone::Edge(SplitDirection::Down), false)
        })
    });
    // An arrangement is then held in flight, before its runtime call.
    let runtime = Arc::clone(&harness.runtime);
    runtime.worker_hooks.hold(Stage::Layout);
    assert!(matches!(
        harness.invoke(window, ids::NEXT_PANE_LAYOUT),
        Ok(CommandOutcome::Accepted)
    ));
    harness.wait_for("the arrangement to wait", |_| {
        runtime.worker_hooks.held(Stage::Layout) == 1
    });
    // The drag goes on and drops: the left pane moves under the right.
    harness.move_to(window, edge);
    harness.release_at(window, edge);
    harness.wait_for("the drop to move the pane", |cx| {
        no_drag(cx, window)
            && panes_shown(cx, window, tab) == Some((vec![right, left], left))
    });
    // The arrangement was fitted against the layout the drop replaced, so
    // the runtime refuses it with one notice.
    runtime.worker_hooks.release(Stage::Layout);
    harness.wait_for("the refused arrangement's notice", |cx| {
        notices(cx, window)
            .iter()
            .any(|notice| notice.ends_with("|The panes changed; try again"))
    });
}

#[gpui::test]
fn a_lone_bar_drag_skips_own_pane_and_slots_but_nests_after_spring_loading(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.panes.title_bar_always_show = true;
    });
    let tab = harness.open_tab(ECHO);
    let other = harness.open_tab(ECHO);
    let pane = harness.first_pane(tab);
    let target = harness.first_pane(other);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.show_drawn(window, tab);
    let grab = harness.grab(window, pane);
    harness.press_at(window, grab);
    let frame = harness.frame(window, pane);
    harness.move_to(window, frame.center());
    harness.wait_for("the lone drag without an own-pane target", |cx| {
        pane_drag(cx, window).is_some_and(|(target, _)| target.is_none())
    });
    let slot = harness.update_view(window, |view, window, _| {
        let strip = view.tab_strip(window);
        let bar = view.chrome_layout(window).strip_bounds(view.layout_tabs());
        gpui::point(bar.right() - px(2.0), strip.label(0).center().y)
    });
    harness.move_to(window, slot);
    harness
        .cx
        .update(|cx| assert_eq!(pane_drag(cx, window).unwrap().0, None));
    let label = harness.label(window, 1);
    harness.move_to(window, label);
    harness.wait_for("the other tab to arm", |cx| {
        pane_drag(cx, window).is_some_and(|(_, armed)| armed == Some(other))
    });
    let (armed, _) = harness.armed(window);
    harness.advance(window, armed + SPRING_LOAD_DELAY);
    harness.wait_for("the spring-loaded destination to draw", |cx| {
        active(cx, window) == Some(other)
            && super::pane_bars::drawn(cx, window)
                .is_some_and(|rects| rects[0].pane == target)
    });
    harness.move_to(window, slot);
    harness
        .cx
        .update(|cx| assert_eq!(pane_drag(cx, window).unwrap().0, None));
    let frame = harness.frame(window, target);
    let edge = gpui::point(frame.right() - px(8.0), frame.center().y);
    harness.move_to(window, edge);
    harness.wait_for("a permitted edge target in the other tab", |cx| {
        pane_drag(cx, window).is_some_and(|(drop, _)| {
            target_is(drop, target, Zone::Edge(SplitDirection::Right), false)
        })
    });
    harness.release_at(window, edge);
    harness.wait_for("the lone pane to nest and focus", |cx| {
        panes_shown(cx, window, other) == Some((vec![target, pane], pane))
    });
}

#[gpui::test]
fn a_lone_bar_drag_has_no_targets_with_a_hidden_tab_bar(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.panes.title_bar_always_show = true;
        config.tabs.always_show = false;
    });
    let tab = harness.open_tab(ECHO);
    let pane = harness.first_pane(tab);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.show_drawn(window, tab);
    let grab = harness.grab(window, pane);
    harness.press_at(window, grab);
    let frame = harness.frame(window, pane);
    harness.move_to(window, frame.center());
    harness.wait_for("the lone drag to start without a target", |cx| {
        pane_drag(cx, window).is_some_and(|(target, _)| target.is_none())
    });
    harness.update_view(window, |view, _, _| assert!(!view.bar_drawn()));
    harness.release_at(window, frame.center());
    harness.wait_for("the untargeted release to end without a move", |cx| {
        no_drag(cx, window)
    });
    harness.cx.update(|cx| {
        assert_eq!(panes_shown(cx, window, tab), Some((vec![pane], pane)));
    });
}
