use gpui::{point, size};

use super::*;

const TERMINAL_MARGINS: TrackMargins = TrackMargins {
    start: 2.0,
    end: 8.0,
    padding: 2.0,
};

fn rows(
    length: f32,
    visible: f32,
    history: f32,
    offset: f32,
) -> Option<ScrollbarGeometry> {
    ScrollbarGeometry::new(
        length,
        visible + history,
        visible,
        offset,
        Origin::End,
        TERMINAL_MARGINS,
    )
}

fn options(expand_on_hover: bool, track_press: TrackPress) -> ScrollbarOptions {
    ScrollbarOptions {
        edge: Edge::Right,
        origin: Origin::End,
        expand_on_hover,
        track_press,
        margins: TERMINAL_MARGINS,
        thumb: ThumbSize::Full,
        edge_inset: 0.0,
        hit: HitBand {
            outward: 0.0,
            inward: 4.0,
        },
        reveal_on_hover: false,
        hold: INDICATOR_HOLD,
    }
}

fn vertical(expand_on_hover: bool, track_press: TrackPress) -> Scrollbars {
    Scrollbars::vertical(options(expand_on_hover, track_press))
}

#[test]
fn settled_drag_release_restarts_the_expanded_hold() {
    let now = Instant::now();
    let bounds =
        Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(400.0)));
    let geometry = rows(400.0, 32.0, 100.0, 50.0).unwrap();
    let geometries = ScrollbarGeometries::vertical(Some(geometry));
    let mut scrollbars = Scrollbars::vertical(ScrollbarOptions {
        hold: Duration::from_secs(10),
        ..options(true, TrackPress::Jump)
    });
    scrollbars.show(Axis::Vertical, now);
    assert_eq!(
        scrollbars.press(
            &geometries,
            bounds,
            point(px(95.0), px(geometry.thumb_start + 5.0)),
            now
        ),
        Some((Axis::Vertical, Press::Grabbed))
    );
    scrollbars.advance(now + SCROLLBAR_EXPAND);
    assert_eq!(
        scrollbars.schedule(now + SCROLLBAR_EXPAND),
        AnimationSchedule::IDLE
    );
    // The idle scheduler does not advance a settled off-strip drag.
    let released = now + Duration::from_secs(30);
    assert!(scrollbars.release(released));
    scrollbars.advance(released);
    assert!(
        (scrollbars.vertical.as_ref().unwrap().expansion.progress - 1.0).abs()
            < f32::EPSILON
    );
    let deadline = released + SCROLLBAR_EXPANDED_HOLD;
    assert_eq!(
        scrollbars.schedule(released),
        AnimationSchedule::at(deadline)
    );
    scrollbars.advance(deadline + SCROLLBAR_EXPAND / 2);
    let progress = scrollbars.vertical.as_ref().unwrap().expansion.progress;
    assert!(progress > 0.0 && progress < 1.0);
}

#[test]
fn animation_schedule_preserves_hold_extension_and_finishes_fade() {
    let now = Instant::now();
    let mut indicator = IndicatorVisibility::with_hold(Duration::from_secs(1));
    indicator.activate(now);
    assert!(!indicator.update(now + Duration::from_millis(100), false));
    assert_eq!(
        indicator.schedule(now),
        AnimationSchedule::at(now + Duration::from_secs(1))
    );
    indicator.activate(now + Duration::from_millis(500));
    assert_eq!(
        indicator.schedule(now),
        AnimationSchedule::at(now + Duration::from_millis(1500))
    );
    let fade = now + Duration::from_millis(1600);
    assert!(indicator.update(fade, false));
    assert_eq!(indicator.schedule(fade), AnimationSchedule::FRAME);
    assert!(indicator.update(now + Duration::from_secs(3), false));
    assert_eq!(
        indicator.schedule(now + Duration::from_secs(3)),
        AnimationSchedule::IDLE
    );
}

#[test]
fn settled_hover_does_not_need_frames_and_leave_restarts_hold() {
    let now = Instant::now();
    let mut bar = AxisScrollbar::new(options(true, TrackPress::Jump));
    bar.hovering = true;
    bar.visibility.activate(now);
    bar.expand(now);
    bar.advance(now + SCROLLBAR_EXPAND);
    assert_eq!(
        bar.schedule(now + SCROLLBAR_EXPAND),
        AnimationSchedule::IDLE
    );
    let mut bars = Scrollbars::vertical(options(true, TrackPress::Jump));
    bars.vertical = Some(bar);
    let left = now + Duration::from_secs(10);
    assert!(bars.pointer_left(left));
    assert_eq!(
        bars.schedule(left).deadline,
        Some(left + bars.vertical.as_ref().unwrap().options.hold)
    );
    assert!(!bars.schedule(left).frame);
}

#[test]
fn unmounted_strip_cancels_hover_drag_and_animation_until_reactivated() {
    let now = Instant::now();
    let config = ScrollbarOptions {
        hold: Duration::from_secs(7),
        reveal_on_hover: true,
        ..options(true, TrackPress::Jump)
    };
    for elapsed in [Duration::ZERO, SCROLLBAR_EXPAND] {
        let mut bars = Scrollbars::vertical(config);
        let bar = bars.vertical.as_mut().unwrap();
        bar.hovering = true;
        bar.drag = Some(5.0);
        bar.visibility.activate(now);
        bar.expand(now);
        bars.advance(now + elapsed);
        assert!(bars.visible(Axis::Vertical));
        assert!(bars.dragging());

        assert!(bars.unmount(Axis::Vertical));
        assert_eq!(bars.schedule(now + elapsed), AnimationSchedule::IDLE);
        assert!(!bars.visible(Axis::Vertical));
        assert!(!bars.dragging());
        assert!(!bars.pointer_left(now + elapsed));
        assert!(!bars.advance(now + Duration::from_secs(30)));
        assert!(!bars.unmount(Axis::Vertical));

        // The strip can be mounted and revealed again with its original hold.
        assert!(bars.wants_strip(Axis::Vertical));
        let remounted = now + Duration::from_secs(60);
        bars.show(Axis::Vertical, remounted);
        assert!(bars.visible(Axis::Vertical));
        assert_eq!(
            bars.schedule(remounted),
            AnimationSchedule::at(remounted + config.hold)
        );
        let bounds =
            Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(400.0)));
        let geometries =
            ScrollbarGeometries::vertical(rows(400.0, 32.0, 100.0, 0.0));
        assert!(bars.pointer_moved(
            &geometries,
            bounds,
            point(px(95.0), px(200.0)),
            remounted,
        ));
        assert!(bars.schedule(remounted).frame);
        let resting_extent = bars.strip_extent(Axis::Vertical);
        bars.advance(remounted + SCROLLBAR_EXPAND);
        assert!(bars.strip_extent(Axis::Vertical) > resting_extent);
    }
}

#[test]
fn unmount_cancels_an_existing_fade_only_on_its_own_axis() {
    let now = Instant::now();
    let mut bars = vertical(true, TrackPress::Jump);
    bars.set_axis(
        Axis::Horizontal,
        Some(ScrollbarOptions {
            edge: Edge::Bottom,
            ..options(true, TrackPress::Jump)
        }),
    );
    bars.show(Axis::Vertical, now);
    let fading = now + INDICATOR_HOLD + INDICATOR_FADE / 2;
    bars.advance(fading);
    assert_eq!(bars.schedule(fading), AnimationSchedule::FRAME);
    bars.show(Axis::Horizontal, fading);

    assert!(bars.unmount(Axis::Vertical));
    assert!(!bars.visible(Axis::Vertical));
    assert!(bars.visible(Axis::Horizontal));
    assert_eq!(
        bars.schedule(fading),
        AnimationSchedule::at(fading + INDICATOR_HOLD)
    );
    assert!(bars.unmount(Axis::Horizontal));
    assert_eq!(bars.schedule(fading), AnimationSchedule::IDLE);
    assert!(!Scrollbars::default().unmount(Axis::Vertical));
}

#[test]
fn slim_halves_the_strip_and_edge_inset_moves_it_inboard() {
    let now = Instant::now();
    let bounds =
        Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(400.0)));
    let geometries =
        ScrollbarGeometries::vertical(rows(400.0, 32.0, 100.0, 0.0));
    let mut slim = Scrollbars::vertical(ScrollbarOptions {
        thumb: ThumbSize::Slim,
        ..options(false, TrackPress::Jump)
    });
    slim.show(Axis::Vertical, now);
    // Resting slim: 1 inset, 3 thumb, 4 inward.
    assert!((slim.strip_extent(Axis::Vertical) - 8.0).abs() < 0.01);
    assert!(
        slim.hit(&geometries, bounds, point(px(91.0), px(200.0)))
            .is_none()
    );
    assert!(
        slim.hit(&geometries, bounds, point(px(95.0), px(200.0)))
            .is_some()
    );
    let mut growing = Scrollbars::vertical(ScrollbarOptions {
        thumb: ThumbSize::Slim,
        ..options(true, TrackPress::Jump)
    });
    growing.show(Axis::Vertical, now);
    assert!(growing.pointer_moved(
        &geometries,
        bounds,
        point(px(97.0), px(200.0)),
        now
    ));
    growing.advance(now + SCROLLBAR_EXPAND);
    assert!((growing.strip_extent(Axis::Vertical) - 18.0).abs() < 0.001);
    assert_eq!(
        growing
            .layers(
                &geometries,
                ScrollbarColors {
                    thumb: Hsla::default(),
                    track: Hsla::default()
                }
            )
            .count(),
        2
    );
}

#[test]
fn exact_thumb_sizes_scale_the_strip_down_to_a_grabbable_minimum() {
    let now = Instant::now();
    let bounds =
        Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(400.0)));
    let geometries =
        ScrollbarGeometries::vertical(rows(400.0, 32.0, 100.0, 0.0));
    let mut hairline = Scrollbars::vertical(ScrollbarOptions {
        thumb: ThumbSize::Points(2.0),
        hit: HitBand::THUMB,
        ..options(false, TrackPress::Jump)
    });
    hairline.show(Axis::Vertical, now);
    // A hairline with a thumb-only band: 2/3 inset plus a 2-point thumb.
    assert!(
        (hairline.strip_extent(Axis::Vertical) - 2.0 - 2.0 / 3.0).abs() < 0.01
    );
    assert!(
        hairline
            .hit(&geometries, bounds, point(px(98.5), px(200.0)))
            .is_some()
    );
    assert!(
        hairline
            .hit(&geometries, bounds, point(px(100.0), px(200.0)))
            .is_none()
    );
    assert!(
        hairline
            .hit(&geometries, bounds, point(px(97.0), px(200.0)))
            .is_none()
    );
    // Reaching outward covers the edge; inward extends the inner side.
    let mut reaching = Scrollbars::vertical(ScrollbarOptions {
        edge_inset: 2.0,
        hit: HitBand {
            outward: 2.0,
            inward: 6.0,
        },
        ..options(false, TrackPress::Jump)
    });
    reaching.show(Axis::Vertical, now);
    assert!(
        reaching
            .hit(&geometries, bounds, point(px(99.5), px(200.0)))
            .is_some()
    );
    assert!(
        reaching
            .hit(&geometries, bounds, point(px(84.5), px(200.0)))
            .is_some()
    );
    assert!(
        reaching
            .hit(&geometries, bounds, point(px(83.0), px(200.0)))
            .is_none()
    );

    let mut inset = Scrollbars::vertical(ScrollbarOptions {
        edge_inset: 6.0,
        ..options(true, TrackPress::Jump)
    });
    inset.show(Axis::Vertical, now);
    assert!((inset.strip_extent(Axis::Vertical) - 18.0).abs() < 0.001);
    // The band nearest the edge belongs to other controls.
    assert!(
        inset
            .hit(&geometries, bounds, point(px(97.0), px(200.0)))
            .is_none()
    );
    assert!(
        inset
            .hit(&geometries, bounds, point(px(90.0), px(200.0)))
            .is_some()
    );
    assert!(
        inset
            .hit(&geometries, bounds, point(px(81.0), px(200.0)))
            .is_none()
    );
}

#[test]
fn set_axis_keeps_state_for_equal_options_and_resets_for_new_ones() {
    let now = Instant::now();
    let bounds =
        Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(400.0)));
    let geometries =
        ScrollbarGeometries::vertical(rows(400.0, 32.0, 100.0, 0.0));
    let mut scrollbars = vertical(true, TrackPress::Jump);
    scrollbars.show(Axis::Vertical, now);
    assert!(scrollbars.pointer_moved(
        &geometries,
        bounds,
        point(px(95.0), px(200.0)),
        now
    ));
    // Render re-applies the same options every frame; nothing resets.
    scrollbars.set_axis(Axis::Vertical, Some(options(true, TrackPress::Jump)));
    assert!(scrollbars.visible(Axis::Vertical));
    // Hover survived, so leaving now reports a change.
    assert!(scrollbars.pointer_left(now));
    // Different options rebuild the axis from scratch.
    scrollbars.set_axis(
        Axis::Vertical,
        Some(ScrollbarOptions {
            hold: Duration::from_millis(300),
            ..options(true, TrackPress::Jump)
        }),
    );
    assert!(!scrollbars.visible(Axis::Vertical));
    assert!(!scrollbars.pointer_left(now));
}

#[test]
fn reveal_on_hover_keeps_the_strip_live_after_the_indicator_fades() {
    let now = Instant::now();
    let bounds =
        Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(400.0)));
    let geometries =
        ScrollbarGeometries::vertical(rows(400.0, 32.0, 100.0, 0.0));
    let position = point(px(95.0), px(200.0));
    let mut hidden = vertical(true, TrackPress::Jump);
    assert!(!hidden.wants_strip(Axis::Vertical));
    assert!(hidden.hit(&geometries, bounds, position).is_none());
    let mut revealing = Scrollbars::vertical(ScrollbarOptions {
        reveal_on_hover: true,
        ..options(true, TrackPress::Jump)
    });
    assert!(revealing.wants_strip(Axis::Vertical));
    assert!(!revealing.visible(Axis::Vertical));
    assert!(revealing.hit(&geometries, bounds, position).is_some());
    assert!(revealing.pointer_moved(&geometries, bounds, position, now));
    assert!(revealing.visible(Axis::Vertical));
    hidden.show(Axis::Vertical, now);
    assert!(hidden.wants_strip(Axis::Vertical));
}

#[test]
fn hold_sets_how_long_the_indicator_stays_before_fading() {
    let now = Instant::now();
    let mut quick = IndicatorVisibility::with_hold(Duration::from_millis(600));
    quick.activate(now);
    assert!(!quick.update(now + Duration::from_millis(600), false));
    assert!(quick.update(now + Duration::from_millis(800), false));
    assert!((quick.opacity - 0.5).abs() < f32::EPSILON);
    let mut standard = IndicatorVisibility::default();
    standard.activate(now);
    assert!(!standard.update(now + Duration::from_millis(800), false));
    assert!((standard.opacity - 1.0).abs() < f32::EPSILON);
}

#[test]
fn expansion_animates_then_collapses_after_four_seconds_without_hover() {
    let now = Instant::now();
    let mut expansion = ScrollbarExpansion::default();
    expansion.activate(now);
    assert!(expansion.active());
    assert!(expansion.update(now + SCROLLBAR_EXPAND / 2, true, false));
    assert!((expansion.progress - 0.875).abs() < f32::EPSILON);
    expansion.update(now + SCROLLBAR_EXPAND, true, false);
    assert!((expansion.progress - 1.0).abs() < f32::EPSILON);
    assert!(!expansion.update(now + SCROLLBAR_EXPANDED_HOLD, true, false));
    assert!(expansion.active());
    expansion.update(
        now + SCROLLBAR_EXPANDED_HOLD + SCROLLBAR_EXPAND / 2,
        true,
        false,
    );
    assert!((expansion.progress - 0.125).abs() < f32::EPSILON);
    expansion.update(
        now + SCROLLBAR_EXPANDED_HOLD + SCROLLBAR_EXPAND,
        true,
        false,
    );
    assert!(!expansion.active());
    assert!(expansion.progress.abs() < f32::EPSILON);
    assert!(!expansion.update(now + Duration::from_secs(12), true, false));
    assert!(!expansion.active());
}

#[test]
fn expansion_resets_when_indicator_fades_before_collapse() {
    let now = Instant::now();
    let mut visibility = IndicatorVisibility::default();
    let mut expansion = ScrollbarExpansion::default();
    visibility.activate(now);
    expansion.activate(now);
    let fading = now + INDICATOR_HOLD + INDICATOR_FADE / 2;
    visibility.update(fading, false);
    expansion.update(fading, visibility.opacity > 0.0, false);
    assert!(expansion.active());
    assert!((expansion.progress - 1.0).abs() < f32::EPSILON);
    let hidden = now + INDICATOR_HOLD + INDICATOR_FADE;
    visibility.update(hidden, false);
    expansion.update(hidden, visibility.opacity > 0.0, false);
    assert!(!expansion.active());
}

#[test]
fn hovering_or_dragging_holds_expansion_and_reentry_reverses_smoothly() {
    let now = Instant::now();
    let mut expansion = ScrollbarExpansion::default();
    expansion.activate(now);
    let release = now + Duration::from_secs(30);
    expansion.update(release, true, true);
    assert!((expansion.progress - 1.0).abs() < f32::EPSILON);
    let midway = release + SCROLLBAR_EXPANDED_HOLD + SCROLLBAR_EXPAND / 2;
    expansion.update(midway, true, false);
    let progress = expansion.progress;
    expansion.activate(midway);
    assert!((expansion.progress - progress).abs() < f32::EPSILON);
    expansion.update(midway + SCROLLBAR_EXPAND, true, true);
    assert!((expansion.progress - 1.0).abs() < f32::EPSILON);
}

#[test]
fn indicator_holds_then_fades_and_stops_updating() {
    let now = Instant::now();
    let mut visibility = IndicatorVisibility::default();
    assert!(!visibility.update(now, false));
    assert!(visibility.opacity.abs() < f32::EPSILON);
    visibility.activate(now);
    assert!(!visibility.update(now + Duration::from_secs(2), false));
    assert!((visibility.opacity - 1.0).abs() < f32::EPSILON);
    assert!(visibility.update(now + Duration::from_millis(2200), false));
    assert!((visibility.opacity - 0.5).abs() < f32::EPSILON);
    assert!(visibility.update(now + Duration::from_millis(2400), false));
    assert!(visibility.opacity.abs() < f32::EPSILON);
    assert!(!visibility.update(now + Duration::from_secs(3), false));
}

#[test]
fn interaction_and_new_scroll_activity_restart_the_fade_delay() {
    let now = Instant::now();
    let mut visibility = IndicatorVisibility::default();
    assert!(visibility.update(now, true));
    assert!(!visibility.update(now + Duration::from_secs(5), true));
    assert!(!visibility.update(now + Duration::from_secs(6), false));
    assert!(visibility.update(now + Duration::from_millis(7200), false));
    assert!((visibility.opacity - 0.5).abs() < f32::EPSILON);
    visibility.activate(now + Duration::from_millis(7200));
    assert!((visibility.opacity - 1.0).abs() < f32::EPSILON);
    assert!(!visibility.update(now + Duration::from_secs(9), false));
}

#[test]
fn inset_track_keeps_tiny_windows_bounded() {
    let geometry = rows(12.0, 32.0, 100.0, 50.0).unwrap();
    assert!(geometry.thumb_start > 0.0);
    assert!(geometry.thumb_start + geometry.thumb_size < 12.0);
    assert!(geometry.offset_for_thumb_start(0.0).abs() < f32::EPSILON);
}

#[test]
fn dragging_beyond_the_window_clamps_to_history_endpoints() {
    let geometry = rows(400.0, 32.0, 1000.0, 500.0).unwrap();
    assert!((geometry.offset_for_thumb_start(-200.0) - 1000.0).abs() < 0.01);
    assert!(geometry.offset_for_thumb_start(600.0).abs() < 0.01);
    assert!(
        (geometry.offset_for_thumb_start(geometry.thumb_start) - 500.0).abs()
            < 0.01
    );
}

#[test]
fn end_origin_geometry_tracks_bottom_middle_top_and_minimum_thumb() {
    assert!(rows(400.0, 32.0, 0.0, 0.0).is_none());
    let bottom = rows(400.0, 32.0, 100.0, 0.0).unwrap();
    let middle = rows(400.0, 32.0, 100.0, 50.0).unwrap();
    let top = rows(400.0, 32.0, 100.0, 100.0).unwrap();
    assert!((top.thumb_start - 4.0).abs() < f32::EPSILON);
    assert!((bottom.track_size() - 390.0).abs() < f32::EPSILON);
    assert!((top.thumb_start - top.track_start - 2.0).abs() < f32::EPSILON);
    assert!(
        (bottom.track_start + bottom.track_size()
            - bottom.thumb_start
            - bottom.thumb_size
            - 2.0)
            .abs()
            < 0.0001
    );
    assert!(
        (middle.thumb_start - top.thumb_start.midpoint(bottom.thumb_start))
            .abs()
            < f32::EPSILON
    );
    assert!(
        (bottom.thumb_start - (390.0 - bottom.thumb_size)).abs() < f32::EPSILON
    );
    for (geometry, offset) in [(top, 100.0), (middle, 50.0), (bottom, 0.0)] {
        assert!(
            (geometry.offset_for_thumb_start(geometry.thumb_start) - offset)
                .abs()
                < 0.01
        );
    }
    let deep = rows(400.0, 32.0, 10_000.0, 0.0).unwrap();
    assert!((deep.thumb_size - MIN_THUMB_SIZE).abs() < f32::EPSILON);
    assert!((deep.offset_for_thumb_start(0.0) - 10_000.0).abs() < 0.01);
    assert!(deep.offset_for_thumb_start(400.0 - deep.thumb_size).abs() < 0.01);
}

#[test]
fn start_origin_geometry_maps_offset_from_top_and_needs_overflow() {
    let even = TrackMargins::EVEN;
    let pixels = |offset| {
        ScrollbarGeometry::new(336.0, 672.0, 336.0, offset, Origin::Start, even)
    };
    assert!(
        ScrollbarGeometry::new(336.0, 336.0, 336.0, 0.0, Origin::Start, even)
            .is_none()
    );
    let top = pixels(0.0).unwrap();
    let bottom = pixels(336.0).unwrap();
    let middle = pixels(168.0).unwrap();
    assert!((top.track_start - 2.0).abs() < f32::EPSILON);
    assert!(
        (bottom.track_start + bottom.track_size() - 334.0).abs() < f32::EPSILON
    );
    assert!(
        (middle.offset_for_thumb_start(middle.thumb_start) - 168.0).abs()
            < 0.01
    );
    assert!((top.thumb_start - top.track_start - 2.0).abs() < f32::EPSILON);
    assert!(
        (bottom.track_start + bottom.track_size()
            - bottom.thumb_start
            - bottom.thumb_size
            - 2.0)
            .abs()
            < 0.0001
    );
    assert!(
        (middle.thumb_start - top.thumb_start.midpoint(bottom.thumb_start))
            .abs()
            < 0.0001
    );
    assert!(
        (top.thumb_size * 2.0 - (top.travel + top.thumb_size)).abs() < 0.0001
    );
    // Flush margins let the thumb touch both ends of the track.
    let flush = ScrollbarGeometry::new(
        336.0,
        672.0,
        336.0,
        336.0,
        Origin::Start,
        TrackMargins {
            start: 0.0,
            end: 0.0,
            padding: 0.0,
        },
    )
    .unwrap();
    assert!(flush.track_start.abs() < f32::EPSILON);
    assert!((flush.thumb_start + flush.thumb_size - 336.0).abs() < 0.0001);
}

#[test]
fn hover_target_expands_only_while_visible() {
    let now = Instant::now();
    let bounds =
        Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(400.0)));
    let geometries =
        ScrollbarGeometries::vertical(rows(400.0, 32.0, 100.0, 0.0));
    let position = point(px(85.0), px(200.0));
    let mut scrollbars = vertical(true, TrackPress::Page);
    assert!(scrollbars.hit(&geometries, bounds, position).is_none());
    scrollbars.show(Axis::Vertical, now);
    assert!(scrollbars.hit(&geometries, bounds, position).is_none());
    assert_eq!(
        scrollbars.hit(&geometries, bounds, point(px(95.0), px(200.0))),
        Some((Axis::Vertical, 200.0))
    );
    assert!(scrollbars.pointer_moved(
        &geometries,
        bounds,
        point(px(95.0), px(200.0)),
        now
    ));
    // Expanding widens the target: 4 inset, 10 thumb, 4 inward.
    scrollbars.advance(now + SCROLLBAR_EXPAND);
    assert!((scrollbars.strip_extent(Axis::Vertical) - 18.0).abs() < 0.01);
    assert_eq!(
        scrollbars.hit(&geometries, bounds, position),
        Some((Axis::Vertical, 200.0))
    );
    assert!(scrollbars.pointer_left(now));
    assert!(!scrollbars.pointer_left(now));
}

#[test]
fn hit_testing_excludes_insets_and_outside_bounds() {
    let now = Instant::now();
    let bounds =
        Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(400.0)));
    let geometries =
        ScrollbarGeometries::vertical(rows(400.0, 32.0, 100.0, 0.0));
    let mut scrollbars = vertical(true, TrackPress::Page);
    scrollbars.show(Axis::Vertical, now);
    for position in [
        point(px(99.0), px(1.0)),
        point(px(99.0), px(393.0)),
        point(px(100.0), px(200.0)),
        point(px(-1.0), px(200.0)),
    ] {
        assert!(
            scrollbars.hit(&geometries, bounds, position).is_none(),
            "{position:?}"
        );
    }
    assert!(
        scrollbars
            .hit(&geometries, bounds, point(px(99.0), px(392.0)))
            .is_some()
    );
}

#[test]
fn press_grabs_jumps_or_pages_by_option_and_drag_follows_the_grab() {
    let now = Instant::now();
    let bounds =
        Bounds::new(point(px(10.0), px(20.0)), size(px(100.0), px(400.0)));
    let geometry = rows(400.0, 32.0, 100.0, 50.0).unwrap();
    let geometries = ScrollbarGeometries::vertical(Some(geometry));
    let on_thumb = point(px(105.0), px(20.0 + geometry.thumb_start + 5.0));
    let above = point(px(105.0), px(20.0 + geometry.track_start + 1.0));

    let mut paging = vertical(true, TrackPress::Page);
    paging.show(Axis::Vertical, now);
    assert_eq!(
        paging.press(&geometries, bounds, above, now),
        Some((Axis::Vertical, Press::Page { backward: true }))
    );
    assert!(!paging.dragging());
    assert_eq!(
        paging.press(&geometries, bounds, on_thumb, now),
        Some((Axis::Vertical, Press::Grabbed))
    );
    assert!(paging.dragging());
    assert_eq!(
        paging.drag_to(bounds, point(px(0.0), px(20.0 + 100.0)), now),
        Some((Axis::Vertical, 100.0 - 5.0))
    );
    assert!(paging.release(now));
    assert!(!paging.release(now));

    let mut jumping = vertical(false, TrackPress::Jump);
    jumping.show(Axis::Vertical, now);
    assert_eq!(
        jumping.press(&geometries, bounds, above, now),
        Some((
            Axis::Vertical,
            Press::Jump(geometry.track_start + 1.0 - geometry.thumb_size / 2.0)
        ))
    );
    assert!(jumping.dragging());
    // Neither the press nor a later pump tick may open the expansion.
    jumping.advance(now + SCROLLBAR_EXPAND);
    assert!((jumping.strip_extent(Axis::Vertical) - 12.0).abs() < f32::EPSILON);
    assert_eq!(
        jumping
            .layers(
                &geometries,
                ScrollbarColors {
                    thumb: Hsla::default(),
                    track: Hsla::default()
                }
            )
            .count(),
        1
    );
}

#[test]
fn horizontal_and_vertical_share_bounds_with_vertical_winning_corners() {
    let now = Instant::now();
    let bounds =
        Bounds::new(point(px(0.0), px(0.0)), size(px(400.0), px(400.0)));
    let mut scrollbars = vertical(true, TrackPress::Jump);
    scrollbars.set_axis(
        Axis::Horizontal,
        Some(ScrollbarOptions {
            edge: Edge::Bottom,
            origin: Origin::Start,
            expand_on_hover: false,
            track_press: TrackPress::Jump,
            margins: TrackMargins::EVEN,
            thumb: ThumbSize::Full,
            edge_inset: 0.0,
            hit: HitBand {
                outward: 0.0,
                inward: 4.0,
            },
            reveal_on_hover: false,
            hold: INDICATOR_HOLD,
        }),
    );
    let geometries = ScrollbarGeometries {
        vertical: rows(400.0, 32.0, 100.0, 0.0),
        horizontal: ScrollbarGeometry::new(
            400.0,
            800.0,
            400.0,
            0.0,
            Origin::Start,
            TrackMargins::EVEN,
        ),
    };
    scrollbars.show(Axis::Vertical, now);
    scrollbars.show(Axis::Horizontal, now);
    assert_eq!(
        scrollbars
            .hit(&geometries, bounds, point(px(200.0), px(395.0)))
            .map(|(axis, _)| axis),
        Some(Axis::Horizontal)
    );
    assert_eq!(
        scrollbars
            .hit(&geometries, bounds, point(px(395.0), px(200.0)))
            .map(|(axis, _)| axis),
        Some(Axis::Vertical)
    );
    // The vertical track ends 8 points up, so only horizontal owns the
    // very corner.
    assert_eq!(
        scrollbars
            .hit(&geometries, bounds, point(px(395.0), px(395.0)))
            .map(|(axis, _)| axis),
        Some(Axis::Horizontal)
    );
    assert_eq!(
        scrollbars
            .layers(
                &geometries,
                ScrollbarColors {
                    thumb: Hsla::default(),
                    track: Hsla::default()
                }
            )
            .count(),
        2
    );
    scrollbars.set_axis(Axis::Horizontal, None);
    assert!(
        scrollbars
            .hit(&geometries, bounds, point(px(200.0), px(395.0)))
            .is_none()
    );
}
