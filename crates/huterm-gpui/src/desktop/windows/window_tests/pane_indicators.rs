//! Tier 2 tests for the sync and zoom indicators: what a frame records as
//! the tab syncs, zooms, does both, and changes style by reload; the tab
//! bar's marks; the focus cue keeping clear of the indicator's lines; and
//! the pane area's corners beside a vertical tab bar.

use huterm_config::{IndicatorStyle, LinePattern, PaneTitleBarMode};

use super::super::pane_indicators::{
    IndicatorRecord, IndicatorState, NativeRadii, RadiusSource, TINT_SHARE,
    WindowRadii,
};
use super::super::tab_bar::TabMark;
use super::pane_bars::{drawn, focus_marks};
use super::synced_input::mark_nodes;
use super::*;

/// The indicator `window` last drew.
fn record(cx: &mut App, window: AnyWindowHandle) -> Option<IndicatorRecord> {
    focus_marks(cx, window).map(|marks| marks.indicator)
}

/// The marks `window`'s tab bar shows for `tab`.
fn tab_marks(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
) -> Option<Vec<TabMark>> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            let view = view.read(cx);
            let tab = view.tabs.iter().find(|view| view.id == tab)?;
            Some(view.tab_marks(tab, cx))
        })
        .ok()
        .flatten()
}

/// Whether `window`'s path cache holds the paths its last frame recorded.
fn cache_matches(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            let view = view.read(cx);
            let recorded = view.drawn_focus.borrow().indicator.key.clone();
            Some(view.indicator_paths.borrow().key().cloned() == recorded)
        })
        .ok()
        .flatten()
        .unwrap_or(false)
}

impl DesktopHarness {
    /// Sets `tab`'s input sync as another client would.
    fn set_sync(&self, tab: TabId, on: bool) {
        self.runtime
            .lock()
            .set_input_sync(self.workspace, tab, on)
            .unwrap();
    }
}

#[gpui::test]
fn the_default_title_bar_look_follows_sync_zoom_and_both(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    harness.update_view(window, |view, window, cx| {
        view.focus_pane(tab, first, window, cx);
    });
    harness.wait_for("no look while nothing syncs or zooms", |cx| {
        record(cx, window)
            .is_some_and(|record| record.look.is_none() && record.key.is_none())
            && tab_marks(cx, window, tab) == Some(Vec::new())
    });

    let colors = harness
        .update_view(window, |view, _, _| TabColors::new(&view.config.theme));
    let sync_tint = colors.accent.opacity(0.5 * TINT_SHARE);
    let zoom_tint = colors.zoom.opacity(0.5 * TINT_SHARE);
    assert_ne!(sync_tint, zoom_tint);
    // The default sync look tints both receiving panes' title bars.
    harness.set_sync(tab, true);
    harness.wait_for("the default sync title bar tints", |cx| {
        record(cx, window).is_some_and(|record| {
            record.look
                == Some((IndicatorState::Sync, IndicatorStyle::TitleBar))
                && record.receiving == [first, second]
                && record.tinted == [(first, sync_tint), (second, sync_tint)]
                && record.key.is_none()
        }) && tab_marks(cx, window, tab) == Some(vec![TabMark::InputSync])
    });

    // Zoom wins over sync: only its tint draws, and sync_dimmed makes the
    // title bar's sync mark read as paused, as does the tab's mark.
    harness.invoke(window, ids::TOGGLE_PANE_ZOOM).unwrap();
    harness.wait_for("the default zoom tint over sync", |cx| {
        record(cx, window).is_some_and(|record| {
            record.look
                == Some((IndicatorState::Zoom, IndicatorStyle::TitleBar))
                && record.receiving == [first]
                && record.tinted == [(first, zoom_tint)]
                && record.key.is_none()
        }) && tab_marks(cx, window, tab)
            == Some(vec![TabMark::Zoom, TabMark::InputSync])
            && mark_nodes(cx, window, TabMark::PAUSED_SYNC_NAME) >= 1
            && mark_nodes(cx, window, TabMark::InputSync.name()) == 0
    });

    harness.set_sync(tab, false);
    harness.wait_for("the default zoom tint alone", |cx| {
        record(cx, window).is_some_and(|record| {
            record.look
                == Some((IndicatorState::Zoom, IndicatorStyle::TitleBar))
                && record.receiving.is_empty()
                && record.tinted == [(first, zoom_tint)]
        }) && tab_marks(cx, window, tab) == Some(vec![TabMark::Zoom])
    });
    harness.invoke(window, ids::TOGGLE_PANE_ZOOM).unwrap();
    harness.wait_for("no look once unzoomed", |cx| {
        record(cx, window).is_some_and(|record| record.look.is_none())
            && tab_marks(cx, window, tab) == Some(Vec::new())
    });
}

#[gpui::test]
fn a_style_reload_applies_at_once_and_rebuilds_the_paths(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    harness.reload_panes(window, |panes| {
        panes.sync_indicator.style = IndicatorStyle::Outline;
    });
    harness.set_sync(tab, true);
    harness.wait_for("the sync outline", |cx| {
        record(cx, window).is_some_and(|record| {
            record.look == Some((IndicatorState::Sync, IndicatorStyle::Outline))
                && record.dividers.len() == 1
                && record.key.as_ref().is_some_and(|key| {
                    key.dividers.len() == 1
                        && key.pattern == LinePattern::Dashed
                        && key.width == 2
                })
        }) && cache_matches(cx, window)
    });
    // An edge bar draws no dividers, where a stale cache would keep the
    // outline's.
    harness.reload_panes(window, |panes| {
        panes.sync_indicator.style = IndicatorStyle::Edge;
        panes.sync_indicator.width = 4;
    });
    harness.wait_for("the edge bar after a reload", |cx| {
        record(cx, window).is_some_and(|record| {
            record.look == Some((IndicatorState::Sync, IndicatorStyle::Edge))
                && record.dividers.is_empty()
                && record.key.as_ref().is_some_and(|key| {
                    key.style == IndicatorStyle::Edge && key.width == 4
                })
        }) && cache_matches(cx, window)
    });
    harness.reload_panes(window, |panes| {
        panes.sync_indicator.style = IndicatorStyle::Dividers;
        panes.sync_indicator.pattern = LinePattern::Dotted;
    });
    harness.wait_for("the dotted dividers after a reload", |cx| {
        record(cx, window).is_some_and(|record| {
            record.look
                == Some((IndicatorState::Sync, IndicatorStyle::Dividers))
                && record.dividers.len() == 1
                && record
                    .key
                    .as_ref()
                    .is_some_and(|key| key.pattern == LinePattern::Dotted)
        }) && cache_matches(cx, window)
    });
    // `off` draws nothing, but the marks stay.
    harness.reload_panes(window, |panes| {
        panes.sync_indicator.style = IndicatorStyle::Off;
    });
    harness.wait_for("no shape under off", |cx| {
        record(cx, window).is_some_and(|record| {
            record.look == Some((IndicatorState::Sync, IndicatorStyle::Off))
                && record.key.is_none()
                && record.receiving == [first, second]
        }) && tab_marks(cx, window, tab)
            .is_some_and(|marks| marks.contains(&TabMark::InputSync))
    });
}

#[gpui::test]
fn the_pane_areas_bottom_corners_follow_the_windows_radius(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, _, _) = harness.split_window(tab);
    harness.reload_panes(window, |panes| {
        panes.sync_indicator.style = IndicatorStyle::Outline;
    });
    harness.set_sync(tab, true);
    harness.update_view(window, |view, _, cx| {
        view.native_radii = NativeRadii {
            radii: WindowRadii {
                top: px(16.0),
                bottom: px(16.0),
            },
            source: RadiusSource::AppKit,
        };
        cx.notify();
    });
    // Only macOS rounds a windowed window's bottom corners; GPUI's test
    // window reports server decorations, which Linux draws square.
    let expected = if cfg!(target_os = "macos") {
        px(16.0)
    } else {
        px(0.0)
    };
    harness.wait_for("the bottom corners to take the window's radius", |cx| {
        record(cx, window).is_some_and(|record| {
            record.corners.bottom_left == expected
                && record.corners.bottom_right == expected
                && record.corners.top_left == px(0.0)
                && record.corners.top_right == px(0.0)
                && record.key.is_some_and(|key| key.corners == record.corners)
        })
    });
}

#[gpui::test]
fn the_focus_cue_keeps_a_lines_width_clear_of_the_sync_lines(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, _) = harness.split_window(tab);
    harness.update_view(window, |view, window, cx| {
        view.focus_pane(tab, first, window, cx);
    });
    harness.reload_panes(window, |panes| {
        panes.title_bar = PaneTitleBarMode::Hidden;
        panes.focus_accent = 1.0;
        panes.sync_indicator.style = IndicatorStyle::Outline;
    });
    let frame = |cx: &mut App| {
        drawn(cx, window)?
            .into_iter()
            .find(|rect| rect.pane == first)
            .map(|rect| rect.frame)
    };
    let cue = |cx: &mut App| focus_marks(cx, window)?.cue;
    // Without a look, the cue runs along the whole top edge.
    harness.wait_for("the cue along the pane's top edge", |cx| {
        let (Some(frame), Some(cue)) = (frame(cx), cue(cx)) else {
            return false;
        };
        cue.origin == frame.origin && cue.size.width == frame.size.width
    });
    // The 2-point outline sits along the top and left, and the divider's
    // band at the right: the cue keeps 2 points from each.
    harness.set_sync(tab, true);
    harness.wait_for("the cue inset from the outline and divider", |cx| {
        let (Some(frame), Some(cue), Some(record)) =
            (frame(cx), cue(cx), record(cx, window))
        else {
            return false;
        };
        let Some(divider) = record.dividers.first() else {
            return false;
        };
        cue.origin.y == frame.origin.y + px(4.0)
            && cue.origin.x == frame.origin.x + px(4.0)
            && cue.right() == divider.origin.x - px(2.0)
    });
    // A style that draws no lines leaves it where the bar would be.
    harness.reload_panes(window, |panes| {
        panes.sync_indicator.style = IndicatorStyle::TitleBar;
    });
    harness.wait_for("the cue back along the top edge", |cx| {
        let (Some(frame), Some(cue)) = (frame(cx), cue(cx)) else {
            return false;
        };
        cue.origin == frame.origin && cue.size.width == frame.size.width
    });
}

/// A titlebar puts the strip above a vertical tab bar only on macOS.
#[cfg(target_os = "macos")]
#[gpui::test]
fn a_left_tab_bar_keeps_the_pane_areas_inner_corners_square(
    cx: &mut TestAppContext,
) {
    use huterm_config::TabStyle;

    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.position = TabPosition::Left;
        config.tabs.style = TabStyle::Strip;
        config.window.padding_x = 8.0;
        config.window.padding_y = 8.0;
    });
    let first = harness.open_tab(ECHO);
    let second = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, first, "READY");
    harness.update_view(window, |view, window, cx| {
        view.select_tab(second, true, window, cx);
    });
    harness.activate(window, second);
    harness.wait_for(
        "square inner corners beside a non-flush active row",
        |cx| {
            record(cx, window).is_some_and(|record| {
                record.corners.top_left == px(0.0)
                    && record.corners.top_right == px(0.0)
            })
        },
    );
    // The first row is flush with the bar's top, so the corner is square.
    harness.update_view(window, |view, window, cx| {
        view.select_tab(first, true, window, cx);
    });
    harness.wait_for("a square corner beside a flush active row", |cx| {
        record(cx, window)
            .is_some_and(|record| record.corners.top_left == px(0.0))
    });
}
