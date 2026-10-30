//! Tier 2 tests for `[panes] title_bar` and `title_bar_position`: a reload
//! moves the title bars or hides them, the terminals take the space a bar
//! gives up, and a bottom bar's buttons work.

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
