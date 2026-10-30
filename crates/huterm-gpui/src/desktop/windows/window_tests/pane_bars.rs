//! Tier 2 tests for `[panes] title_bar` and `title_bar_position`: a reload
//! moves the title bars or hides them, the terminals take the space a bar
//! gives up, a bottom bar's buttons work, and hover bars reveal, hold, and
//! fade. Also the focus marks: which panes `focus_accent` and
//! `inactive_dim` mark, and a dim layer leaving pointer input to the pane
//! beneath it.

use std::time::{Duration, Instant};

use huterm_config::{PaneTitleBarMode, PaneTitleBarPosition, PanesConfig};

use super::pane_moves::pane_focused;
use super::*;

/// The rectangles `window` last drew for its active tab's panes.
fn drawn(cx: &mut App, window: AnyWindowHandle) -> Option<Vec<PaneRect>> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            let drawn = view.read(cx).drawn_panes.borrow().clone();
            drawn.map(|(_, rects)| rects)
        })
        .ok()
        .flatten()
}

/// The grid `window`'s view of `pane` requests.
fn grid(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
    pane: PaneId,
) -> Option<GridSize> {
    pane_view(cx, window, tab, pane).map(|view| view.read(cx).requested_grid)
}

impl DesktopHarness {
    /// Reloads `window` with its configuration's `[panes]` changed, as a
    /// configuration reload applies it.
    fn reload_panes(
        &mut self,
        window: AnyWindowHandle,
        change: impl FnOnce(&mut PanesConfig),
    ) {
        self.update_view(window, |view, window, cx| {
            let mut config = view.config.clone();
            change(&mut config.panes);
            let loaded = (config, view.family.clone(), view.metrics);
            view.apply_reload(Some(&loaded), &[], window, cx);
        });
    }
}

#[gpui::test]
fn a_reload_moves_title_bars_to_the_bottom_or_hides_them(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    let top = harness
        .cx
        .update(|cx| grid(cx, window, tab, first))
        .unwrap();

    // At the bottom, each bar runs along its frame's bottom edge, under a
    // terminal of the same height as before.
    harness.reload_panes(window, |panes| {
        panes.title_bar_position = PaneTitleBarPosition::Bottom;
    });
    harness.wait_for("the bars to move to the bottom", |cx| {
        drawn(cx, window).is_some_and(|rects| {
            rects.len() == 2
                && rects.iter().all(|rect| {
                    rect.title_bar.is_some_and(|bar| {
                        bar.bottom() == rect.frame.bottom()
                            && rect.terminal.origin == rect.frame.origin
                    })
                })
        })
    });
    harness.cx.update(|cx| {
        assert_eq!(grid(cx, window, tab, first), Some(top));
    });
    // A bottom bar's zoom button zooms its pane.
    let zoom = harness.update_view(window, |view, _, _| {
        view.drawn_panes
            .borrow()
            .as_ref()
            .and_then(|(_, rects)| {
                rects.iter().find(|rect| rect.pane == first)?.title_bar
            })
            .map(WorkspaceView::pane_zoom_center)
    });
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_click(zoom.unwrap(), gpui::Modifiers::none());
    harness.wait_for("the first pane to zoom", |cx| {
        pane_shown(cx, window, tab, second).is_some_and(|(visible, _)| !visible)
    });
    harness.invoke(window, ids::TOGGLE_PANE_ZOOM).unwrap();
    harness.wait_for("both panes to show again", |cx| {
        pane_shown(cx, window, tab, second).is_some_and(|(visible, _)| visible)
    });

    // Hidden bars give each terminal its whole frame, and its grid grows.
    harness.reload_panes(window, |panes| {
        panes.title_bar = PaneTitleBarMode::Hidden;
    });
    harness.wait_for("the bars to hide and the grids to grow", |cx| {
        drawn(cx, window).is_some_and(|rects| {
            rects.iter().all(|rect| {
                rect.title_bar.is_none() && rect.terminal == rect.frame
            })
        }) && grid(cx, window, tab, first)
            .is_some_and(|grid| grid.rows > top.rows)
    });

    // The defaults put the bars back at the top.
    harness.reload_panes(window, |panes| *panes = PanesConfig::default());
    harness.wait_for("the bars to return to the top", |cx| {
        drawn(cx, window).is_some_and(|rects| {
            rects.iter().all(|rect| {
                rect.title_bar
                    .is_some_and(|bar| bar.origin == rect.frame.origin)
            })
        }) && grid(cx, window, tab, first) == Some(top)
    });

    // A bottom bar's close button closes its pane.
    harness.reload_panes(window, |panes| {
        panes.title_bar_position = PaneTitleBarPosition::Bottom;
    });
    harness.wait_for("the bars to move to the bottom again", |cx| {
        drawn(cx, window).is_some_and(|rects| {
            rects.iter().all(|rect| {
                rect.title_bar
                    .is_some_and(|bar| bar.bottom() == rect.frame.bottom())
            })
        })
    });
    let close = harness.update_view(window, |view, _, _| {
        view.drawn_panes
            .borrow()
            .as_ref()
            .and_then(|(_, rects)| {
                rects.iter().find(|rect| rect.pane == second)?.title_bar
            })
            .map(WorkspaceView::pane_close_center)
    });
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_click(close.unwrap(), gpui::Modifiers::none());
    harness.wait_for("the second pane to close", |cx| {
        panes_shown(cx, window, tab) == Some((vec![first], first))
    });
}

/// How far `pane`'s hover bar has revealed in `window`, and the overlay
/// its terminal keeps clear of pointer input.
fn bar_state(
    cx: &mut App,
    window: AnyWindowHandle,
    pane: PaneId,
) -> Option<(f32, Option<Bounds<Pixels>>)> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            let view = view.read(cx);
            let terminal = view.tabs.iter().find_map(|tab| tab.pane(pane))?;
            let overlay = terminal.view.read(cx).overlays.pane_bar;
            Some((view.pane_bars.progress(pane), overlay))
        })
        .ok()
        .flatten()
}

impl DesktopHarness {
    /// Feeds `window`'s hover bars a pointer at `at`, as its pointer
    /// refresh samples one; `None` is a pointer outside every bar.
    fn point_bars(
        &mut self,
        window: AnyWindowHandle,
        at: Option<gpui::Point<Pixels>>,
    ) {
        self.update_view(window, |view, _, cx| {
            view.pane_bar_pointer = at;
            view.refresh_pane_bars(cx);
        });
    }

    /// Runs `window`'s animation step at `now`, as its frame clock would.
    fn animate_bars(&mut self, window: AnyWindowHandle, now: Instant) {
        use super::super::refresh::Animated;
        self.update_view(window, |view, window, cx| {
            view.advance_animation(now, true, window, cx);
        });
    }

    /// Animates `window` in 20 ms frames from `from` through `to`.
    fn animate_through(
        &mut self,
        window: AnyWindowHandle,
        from: Instant,
        to: Instant,
    ) {
        let mut now = from;
        while now < to {
            now += Duration::from_millis(20);
            self.animate_bars(window, now);
        }
    }

    /// `pane`'s drawn bar rectangle in `window`.
    fn bar_of(
        &mut self,
        window: AnyWindowHandle,
        pane: PaneId,
    ) -> Bounds<Pixels> {
        self.update_view(window, |view, _, _| {
            view.drawn_panes.borrow().as_ref().and_then(|(_, rects)| {
                rects.iter().find(|rect| rect.pane == pane)?.title_bar
            })
        })
        .expect("a drawn bar")
    }
}

#[gpui::test]
fn hover_bars_reveal_after_the_intent_and_stay_while_held(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    harness.reload_panes(window, |panes| {
        panes.title_bar = PaneTitleBarMode::Hover;
    });
    // The bars take no space and nothing shows yet.
    harness.wait_for("hover geometry", |cx| {
        drawn(cx, window).is_some_and(|rects| {
            rects.iter().all(|rect| {
                rect.title_bar.is_some() && rect.terminal == rect.frame
            })
        })
    });
    let bar = harness.bar_of(window, first);
    harness.cx.update(|cx| {
        assert_eq!(bar_state(cx, window, first), Some((0.0, None)));
    });

    // Resting in the band reveals the bar only once the intent passes,
    // and the revealed bar keeps the terminal's pointer input.
    let start = Instant::now();
    harness.point_bars(window, Some(bar.center()));
    harness.animate_through(window, start, start + Duration::from_millis(100));
    harness.cx.update(|cx| {
        assert_eq!(bar_state(cx, window, first), Some((0.0, None)));
    });
    harness.animate_through(
        window,
        start + Duration::from_millis(100),
        start + Duration::from_millis(500),
    );
    harness.cx.update(|cx| {
        assert_eq!(bar_state(cx, window, first), Some((1.0, Some(bar))));
        assert_eq!(bar_state(cx, window, second), Some((0.0, None)));
    });

    // Its menu holds it after the pointer leaves; closing the menu lets it
    // fade after the leave hold.
    harness.update_view(window, |view, window, cx| {
        view.open_pane_menu(tab, first, bar.center(), window, cx);
    });
    harness.point_bars(window, None);
    let held = start + Duration::from_millis(500);
    harness.animate_through(window, held, held + Duration::from_secs(2));
    harness.cx.update(|cx| {
        assert_eq!(
            bar_state(cx, window, first).map(|state| state.0),
            Some(1.0)
        );
    });
    harness.press(window, "escape");
    let closed = held + Duration::from_secs(2);
    harness.animate_through(window, closed, closed + Duration::from_secs(1));
    harness.cx.update(|cx| {
        assert_eq!(bar_state(cx, window, first), Some((0.0, None)));
    });

    // A keyboard focus change reveals the newly focused pane's bar briefly.
    harness.invoke(window, ids::FOCUS_NEXT_PANE).unwrap();
    harness.wait_for("the second pane to take focus", |cx| {
        panes_shown(cx, window, tab).is_some_and(|(_, focus)| focus == second)
    });
    let moved = Instant::now();
    harness.animate_through(window, moved, moved + Duration::from_millis(300));
    harness.cx.update(|cx| {
        assert_eq!(
            bar_state(cx, window, second).map(|state| state.0),
            Some(1.0)
        );
    });
    harness.animate_through(
        window,
        moved + Duration::from_millis(300),
        moved + Duration::from_secs(3),
    );
    harness.cx.update(|cx| {
        assert_eq!(bar_state(cx, window, second), Some((0.0, None)));
    });
}

#[gpui::test]
fn a_keyboard_reveal_out_of_a_zoomed_pane_shows_the_new_bar(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    harness.update_view(window, |view, window, cx| {
        view.focus_pane(tab, first, window, cx);
    });
    harness.reload_panes(window, |panes| {
        panes.title_bar = PaneTitleBarMode::Hover;
    });
    harness.invoke(window, ids::TOGGLE_PANE_ZOOM).unwrap();
    harness.wait_for("the first pane to zoom", |cx| {
        pane_shown(cx, window, tab, second).is_some_and(|(visible, _)| !visible)
            && drawn(cx, window).is_some_and(|rects| rects.len() == 1)
    });
    // Moving focus unzooms and reveals the second pane's bar, which the
    // zoomed frame never drew.
    harness.invoke(window, ids::FOCUS_NEXT_PANE).unwrap();
    let moved = Instant::now();
    harness.wait_for("both panes to draw again", |cx| {
        drawn(cx, window).is_some_and(|rects| rects.len() == 2)
    });
    harness.animate_through(window, moved, moved + Duration::from_millis(300));
    harness.cx.update(|cx| {
        assert_eq!(
            bar_state(cx, window, second).map(|state| state.0),
            Some(1.0)
        );
    });
}

#[gpui::test]
fn a_revealed_bars_overlay_follows_its_pane_through_a_resize(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, _) = harness.split_window(tab);
    harness.update_view(window, |view, window, cx| {
        view.focus_pane(tab, first, window, cx);
    });
    harness.reload_panes(window, |panes| {
        panes.title_bar = PaneTitleBarMode::Hover;
    });
    harness.wait_for("hover geometry", |cx| {
        drawn(cx, window).is_some_and(|rects| {
            rects.iter().all(|rect| rect.terminal == rect.frame)
        })
    });
    let bar = harness.bar_of(window, first);
    let start = Instant::now();
    harness.point_bars(window, Some(bar.center()));
    harness.animate_through(window, start, start + Duration::from_millis(500));
    harness.cx.update(|cx| {
        assert_eq!(bar_state(cx, window, first), Some((1.0, Some(bar))));
    });
    // A resize by command widens the bar while the pointer rests on it;
    // the terminal's overlay follows the bar as drawn.
    harness.invoke(window, ids::RESIZE_PANE_RIGHT).unwrap();
    harness.wait_for("the bar to widen and its overlay to follow", |cx| {
        drawn(cx, window)
            .and_then(|rects| {
                rects.iter().find(|rect| rect.pane == first)?.title_bar
            })
            .is_some_and(|wider| {
                wider.size.width > bar.size.width
                    && bar_state(cx, window, first) == Some((1.0, Some(wider)))
            })
    });
}

/// The focus marks `window` last drew.
fn focus_marks(cx: &mut App, window: AnyWindowHandle) -> Option<FocusMarks> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            Some(view.read(cx).drawn_focus.borrow().clone())
        })
        .ok()
        .flatten()
}

#[gpui::test]
fn focus_marks_follow_focus_zoom_and_the_bar_mode(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    harness.invoke(window, ids::FOCUS_PANE_RIGHT).unwrap();
    harness.reload_panes(window, |panes| {
        panes.title_bar = PaneTitleBarMode::Hidden;
        panes.focus_accent = 0.5;
        panes.inactive_dim = 0.4;
    });
    let marks =
        |dimmed: Vec<(PaneId, f32)>, accent| FocusMarks { dimmed, accent };
    // The focused pane carries the accent; the other dims.
    let expected = marks(vec![(first, 0.4)], Some((second, 0.5)));
    harness.wait_for("the right pane to carry the accent", |cx| {
        focus_marks(cx, window).as_ref() == Some(&expected)
    });
    harness.invoke(window, ids::FOCUS_PANE_LEFT).unwrap();
    let expected = marks(vec![(second, 0.4)], Some((first, 0.5)));
    harness.wait_for("the marks to follow focus left", |cx| {
        focus_marks(cx, window).as_ref() == Some(&expected)
    });
    // A zoomed pane is the only one on screen, so nothing is marked.
    harness.invoke(window, ids::TOGGLE_PANE_ZOOM).unwrap();
    harness.wait_for("zoom to clear the marks", |cx| {
        pane_shown(cx, window, tab, second).is_some_and(|(shown, _)| !shown)
            && focus_marks(cx, window) == Some(FocusMarks::default())
    });
    harness.invoke(window, ids::TOGGLE_PANE_ZOOM).unwrap();
    // Visible bars show focus themselves, so only the dimming stays.
    harness.reload_panes(window, |panes| {
        panes.title_bar = PaneTitleBarMode::Visible;
    });
    let expected = marks(vec![(second, 0.4)], None);
    harness.wait_for("visible bars to drop the accent", |cx| {
        pane_shown(cx, window, tab, second).is_some_and(|(shown, _)| shown)
            && focus_marks(cx, window).as_ref() == Some(&expected)
    });
}

#[gpui::test]
fn a_dimmed_pane_takes_the_wheel_and_a_selection_through_its_dim_layer(
    cx: &mut TestAppContext,
) {
    use gpui::{Modifiers, ScrollDelta, ScrollEvent, TouchPhase, point};
    let mut harness = DesktopHarness::new(cx);
    // The first pane holds scrollback for the wheel to scroll.
    let tab = harness.open_tab(&format!("seq 1 200; {ECHO}"));
    let (window, first, second) = harness.split_window(tab);
    harness.invoke(window, ids::FOCUS_PANE_RIGHT).unwrap();
    harness.reload_panes(window, |panes| panes.inactive_dim = 0.5);
    harness.wait_for("the second pane to take focus", |cx| {
        pane_focused(cx, window, tab, second)
    });
    let centre = harness.cx.update(|cx| {
        drawn(cx, window)
            .and_then(|rects| rects.into_iter().find(|rect| rect.pane == first))
            .map(|rect| rect.terminal.center())
            .unwrap()
    });
    let scrolled = |cx: &mut App| {
        pane_view(cx, window, tab, first)
            .map_or(0, |view| view.read(cx).scroll.desired())
    };
    // The wheel scrolls what the pointer is over, as of a frame drawn after
    // the pointer arrived, and leaves focus where it was.
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_mouse_move(centre, None, Modifiers::none());
    visual.update(|window, _| window.refresh());
    visual.run_until_parked();
    visual.simulate_event(ScrollEvent {
        position: centre,
        delta: ScrollDelta::Lines(point(0.0, 3.0)),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
        ..Default::default()
    });
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        assert_eq!(scrolled(cx), 3, "the wheel did not reach the dimmed pane");
        assert!(pane_focused(cx, window, tab, second));
    });
    // A drag selects in the dimmed pane and focuses it.
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_mouse_down(centre, MouseButton::Left, Modifiers::none());
    visual.simulate_mouse_move(
        centre + point(px(48.0), px(0.0)),
        Some(MouseButton::Left),
        Modifiers::none(),
    );
    visual.simulate_mouse_up(
        centre + point(px(48.0), px(0.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    harness.wait_for("the dimmed pane to focus and select", |cx| {
        pane_focused(cx, window, tab, first)
            && pane_view(cx, window, tab, first)
                .is_some_and(|view| view.read(cx).selection.is_some())
    });
}
