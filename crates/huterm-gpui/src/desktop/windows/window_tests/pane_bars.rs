//! Tier 2 tests for `[panes] title_bar` and `title_bar_position`: a reload
//! moves the title bars or hides them, the terminals take the space a bar
//! gives up, a bottom bar's buttons work, and hover bars reveal, hold, and
//! fade.

use std::time::Duration;

use huterm_config::{PaneTitleBar, PaneTitleBarPosition, PanesConfig};

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
        panes.title_bar = PaneTitleBar::Hidden;
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
        panes.title_bar = PaneTitleBar::Hover;
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
