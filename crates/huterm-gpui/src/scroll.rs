use std::time::{Duration, Instant};

use huterm_protocol::Viewport;

const MIN_THUMB_SIZE: f32 = 24.0;
const TRACK_TOP_MARGIN: f32 = 2.0;
const TRACK_BOTTOM_MARGIN: f32 = 8.0;
const TRACK_PADDING: f32 = 2.0;
const INDICATOR_HOLD: Duration = Duration::from_secs(2);
const INDICATOR_FADE: Duration = Duration::from_millis(400);
const SCROLLBAR_EXPAND: Duration = Duration::from_millis(180);
const SCROLLBAR_EXPANDED_HOLD: Duration = Duration::from_secs(4);

#[derive(Debug, Default)]
pub(super) struct ScrollbarExpansion {
    started: Option<Instant>,
    collapse_at: Option<Instant>,
    from: f32,
    target: f32,
    pub(super) progress: f32,
}

impl ScrollbarExpansion {
    pub(super) fn activate(&mut self, now: Instant) {
        self.sample(now);
        if self.target < 1.0 {
            self.from = self.progress;
            self.target = 1.0;
            self.started = Some(now);
        }
        self.collapse_at = Some(now + SCROLLBAR_EXPANDED_HOLD);
    }

    pub(super) fn active(&self) -> bool {
        self.target > 0.0 || self.progress > 0.0
    }

    pub(super) fn update(
        &mut self,
        now: Instant,
        visible: bool,
        interacting: bool,
    ) -> bool {
        let previous = self.progress;
        if visible {
            if interacting {
                self.activate(now);
            } else if let Some(deadline) = self.collapse_at
                && now >= deadline
            {
                self.sample(deadline);
                self.from = self.progress;
                self.target = 0.0;
                self.started = Some(deadline);
                self.collapse_at = None;
            }
            self.sample(now);
        } else {
            *self = Self::default();
        }
        (previous - self.progress).abs() > f32::EPSILON
    }

    fn sample(&mut self, now: Instant) {
        if let Some(started) = self.started {
            let elapsed =
                (now.saturating_duration_since(started).as_secs_f32()
                    / SCROLLBAR_EXPAND.as_secs_f32())
                .clamp(0.0, 1.0);
            let eased = 1.0 - (1.0 - elapsed).powi(3);
            self.progress = self.from + (self.target - self.from) * eased;
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct IndicatorVisibility {
    fade_start: Option<Instant>,
    pub(super) opacity: f32,
}

impl IndicatorVisibility {
    pub(super) fn activate(&mut self, now: Instant) {
        self.fade_start = Some(now + INDICATOR_HOLD);
        self.opacity = 1.0;
    }

    pub(super) fn update(&mut self, now: Instant, interacting: bool) -> bool {
        let previous = self.opacity;
        if interacting {
            self.activate(now);
        } else {
            self.opacity = self.fade_start.map_or(0.0, |start| {
                1.0 - (now.saturating_duration_since(start).as_secs_f32()
                    / INDICATOR_FADE.as_secs_f32())
                .clamp(0.0, 1.0)
            });
        }
        (self.opacity - previous).abs() > f32::EPSILON
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ScrollbarGeometry {
    pub(super) thumb_start: f32,
    pub(super) thumb_size: f32,
    pub(super) track_start: f32,
    track_padding: f32,
    travel: f32,
    history: usize,
}

impl ScrollbarGeometry {
    pub(super) fn new(
        height: f32,
        visible_rows: u16,
        history: usize,
        displayed_offset: usize,
    ) -> Option<Self> {
        if !height.is_finite() || height <= 0.0 || history == 0 {
            return None;
        }
        let margin_scale = (height
            / (TRACK_TOP_MARGIN
                + TRACK_BOTTOM_MARGIN
                + TRACK_PADDING * 2.0
                + MIN_THUMB_SIZE))
            .min(1.0);
        let track_start = TRACK_TOP_MARGIN * margin_scale;
        let track_height =
            height - track_start - TRACK_BOTTOM_MARGIN * margin_scale;
        let track_padding = TRACK_PADDING * margin_scale;
        let inner_height = (track_height - track_padding * 2.0).max(0.0);
        let visible = f32::from(visible_rows.max(1));
        #[expect(
            clippy::cast_precision_loss,
            reason = "terminal scrollback is capped at 10000 rows"
        )]
        let total = visible + history as f32;
        let thumb_size = (inner_height * visible / total)
            .max(MIN_THUMB_SIZE)
            .min(inner_height);
        let travel = (inner_height - thumb_size).max(0.0);
        #[expect(
            clippy::cast_precision_loss,
            reason = "terminal scrollback is capped at 10000 rows"
        )]
        let bottom_ratio =
            displayed_offset.min(history) as f32 / history as f32;
        Some(Self {
            thumb_start: track_start
                + track_padding
                + travel * (1.0 - bottom_ratio),
            thumb_size,
            track_start,
            track_padding,
            travel,
            history,
        })
    }

    pub(super) fn contains(self, position: f32) -> bool {
        (self.thumb_start..=self.thumb_start + self.thumb_size)
            .contains(&position)
    }

    pub(super) fn track_size(self) -> f32 {
        self.travel + self.thumb_size + self.track_padding * 2.0
    }

    pub(super) fn track_contains(self, position: f32) -> bool {
        (self.track_start..=self.track_start + self.track_size())
            .contains(&position)
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "a bounded scrollbar ratio maps to at most 10000 rows"
    )]
    pub(super) fn offset_for_thumb_start(self, position: f32) -> usize {
        if self.travel <= 0.0 {
            return 0;
        }
        let ratio = ((position - self.track_start - self.track_padding)
            / self.travel)
            .clamp(0.0, 1.0);
        ((1.0 - ratio) * self.history as f32).round() as usize
    }
}

#[derive(Debug, Default)]
pub(super) struct ScrollController {
    desired: usize,
    displayed: usize,
    history: usize,
    pixel_remainder: f32,
    in_flight: bool,
    dirty: bool,
    diagnostics: ScrollDiagnostics,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ScrollDiagnostics {
    pub(super) requests_started: u64,
    pub(super) requests_completed: u64,
    pub(super) requests_coalesced: u64,
    pub(super) maximum_concurrent: usize,
    pub(super) queued_updates: u64,
    pub(super) maximum_queued: usize,
}

impl ScrollController {
    pub(super) fn desired(&self) -> usize {
        self.desired
    }

    pub(super) fn displayed(&self) -> usize {
        self.displayed
    }

    pub(super) fn history(&self) -> usize {
        self.history
    }

    pub(super) fn diagnostics(&self) -> ScrollDiagnostics {
        self.diagnostics
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "finite input is consumed only in whole-row increments"
    )]
    pub(super) fn scroll_pixels(
        &mut self,
        pixels: f32,
        row_height: f32,
    ) -> bool {
        if !pixels.is_finite()
            || !row_height.is_finite()
            || row_height <= 0.0
            || pixels == 0.0
        {
            return false;
        }
        if self.pixel_remainder.signum() != pixels.signum() {
            self.pixel_remainder = 0.0;
        }
        self.pixel_remainder += pixels;
        let rows = (self.pixel_remainder / row_height).trunc();
        if rows == 0.0 {
            return false;
        }
        self.pixel_remainder -= rows * row_height;
        self.scroll_rows(rows as i64)
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "finite platform line deltas are rounded to whole rows"
    )]
    pub(super) fn scroll_lines(&mut self, lines: f32) -> bool {
        if !lines.is_finite() || lines == 0.0 {
            return false;
        }
        self.pixel_remainder = 0.0;
        self.scroll_rows(lines.round() as i64)
    }

    pub(super) fn scroll_rows(&mut self, rows: i64) -> bool {
        let previous = self.desired;
        if rows > 0 {
            self.desired = self
                .desired
                .saturating_add(usize::try_from(rows).unwrap_or(usize::MAX))
                .min(self.history);
        } else {
            self.desired = self.desired.saturating_sub(
                usize::try_from(rows.unsigned_abs()).unwrap_or(usize::MAX),
            );
        }
        self.desired != previous
    }

    pub(super) fn page(&mut self, rows: u16, upward: bool) -> bool {
        self.scroll_rows(if upward {
            i64::from(rows)
        } else {
            -i64::from(rows)
        })
    }

    pub(super) fn bottom(&mut self) -> bool {
        let changed = self.desired != 0;
        self.desired = 0;
        self.pixel_remainder = 0.0;
        changed
    }

    pub(super) fn set_desired(&mut self, offset: usize) -> bool {
        let offset = offset.min(self.history);
        let changed = self.desired != offset;
        self.desired = offset;
        self.pixel_remainder = 0.0;
        changed
    }

    pub(super) fn invalidate(&mut self) {
        self.dirty = true;
    }

    pub(super) fn begin_request(&mut self) -> Option<Viewport> {
        if self.in_flight {
            if self.dirty || self.desired != self.displayed {
                self.diagnostics.requests_coalesced =
                    self.diagnostics.requests_coalesced.saturating_add(1);
                self.diagnostics.queued_updates =
                    self.diagnostics.queued_updates.saturating_add(1);
                self.diagnostics.maximum_queued = 1;
            }
            return None;
        }
        if !self.dirty && self.desired == self.displayed {
            return None;
        }
        self.in_flight = true;
        self.dirty = false;
        self.diagnostics.requests_started =
            self.diagnostics.requests_started.saturating_add(1);
        self.diagnostics.maximum_concurrent =
            self.diagnostics.maximum_concurrent.max(1);
        Some(Viewport {
            bottom_offset: self.desired,
        })
    }

    pub(super) fn complete(&mut self, viewport: Viewport, history: usize) {
        let pinned = self.desired > 0;
        if pinned && history > self.history {
            self.desired = self.desired.saturating_add(history - self.history);
        }
        self.history = history;
        self.desired = self.desired.min(history);
        self.displayed = viewport.bottom_offset.min(history);
        self.in_flight = false;
        self.diagnostics.requests_completed =
            self.diagnostics.requests_completed.saturating_add(1);
    }

    pub(super) fn fail(&mut self) {
        self.in_flight = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let geometry = ScrollbarGeometry::new(12.0, 32, 100, 50).unwrap();
        assert!(geometry.thumb_start > 0.0);
        assert!(geometry.thumb_start + geometry.thumb_size < 12.0);
        assert_eq!(geometry.offset_for_thumb_start(0.0), 0);
    }

    #[test]
    fn dragging_beyond_the_window_clamps_to_history_endpoints() {
        let geometry = ScrollbarGeometry::new(400.0, 32, 1000, 500).unwrap();
        assert_eq!(geometry.offset_for_thumb_start(-200.0), 1000);
        assert_eq!(geometry.offset_for_thumb_start(600.0), 0);
        assert_eq!(geometry.offset_for_thumb_start(geometry.thumb_start), 500);
    }

    fn controller_with_history(history: usize) -> ScrollController {
        let mut controller = ScrollController::default();
        controller.complete(Viewport::default(), history);
        controller
    }

    #[test]
    fn precise_pixels_accumulate_and_reverse_without_stale_remainder() {
        let mut controller = controller_with_history(100);
        assert!(controller.set_desired(10));
        assert!(!controller.scroll_pixels(4.0, 10.0));
        assert!(!controller.scroll_pixels(-7.0, 10.0));
        assert!(controller.scroll_pixels(-4.0, 10.0));
        assert_eq!(controller.desired(), 9);
        assert!(controller.scroll_pixels(11.0, 10.0));
        assert_eq!(controller.desired(), 10);
    }

    #[test]
    fn line_delta_preserves_magnitude_and_bounds() {
        let mut controller = controller_with_history(4);
        assert!(controller.scroll_lines(3.0));
        assert_eq!(controller.desired(), 3);
        assert!(controller.scroll_lines(8.0));
        assert_eq!(controller.desired(), 4);
        assert!(controller.scroll_lines(-2.0));
        assert_eq!(controller.desired(), 2);
    }

    #[test]
    fn requests_coalesce_while_one_is_in_flight() {
        let mut controller = controller_with_history(100);
        controller.invalidate();
        assert_eq!(controller.begin_request(), Some(Viewport::default()));
        controller.scroll_rows(20);
        assert_eq!(controller.begin_request(), None);
        controller.complete(Viewport::default(), 100);
        assert_eq!(
            controller.begin_request(),
            Some(Viewport { bottom_offset: 20 })
        );
    }

    #[test]
    fn growing_history_keeps_scrolled_content_pinned() {
        let mut controller = controller_with_history(100);
        controller.set_desired(10);
        controller.invalidate();
        let _ = controller.begin_request();
        controller.complete(Viewport { bottom_offset: 10 }, 105);
        assert_eq!(controller.desired(), 15);
        assert_eq!(
            controller.begin_request(),
            Some(Viewport { bottom_offset: 15 })
        );
    }

    #[test]
    fn completion_can_launch_a_replacement_without_a_timer_tick() {
        let mut controller = controller_with_history(100);
        controller.invalidate();
        assert_eq!(controller.begin_request(), Some(Viewport::default()));
        controller.scroll_rows(5);
        controller.complete(Viewport::default(), 100);
        assert_eq!(
            controller.begin_request(),
            Some(Viewport { bottom_offset: 5 })
        );
        assert_eq!(controller.diagnostics().requests_started, 2);
        assert_eq!(controller.diagnostics().maximum_concurrent, 1);
    }

    #[test]
    fn full_history_ring_preserves_the_requested_offset() {
        let mut controller = controller_with_history(10_000);
        controller.set_desired(500);
        controller.invalidate();
        let _ = controller.begin_request();
        controller.complete(Viewport { bottom_offset: 500 }, 10_000);
        assert_eq!(controller.desired(), 500);
    }

    #[test]
    fn return_to_bottom_wins_over_growth_during_an_older_request() {
        let mut controller = controller_with_history(100);
        controller.set_desired(10);
        controller.invalidate();
        let _ = controller.begin_request();
        controller.bottom();
        controller.complete(Viewport { bottom_offset: 10 }, 105);

        assert_eq!(controller.desired(), 0);
        assert_eq!(controller.begin_request(), Some(Viewport::default()));
    }

    #[test]
    fn scrollbar_geometry_tracks_bottom_middle_top_and_minimum_thumb() {
        assert!(ScrollbarGeometry::new(400.0, 32, 0, 0).is_none());
        let bottom = ScrollbarGeometry::new(400.0, 32, 100, 0)
            .expect("history should produce an indicator");
        let middle = ScrollbarGeometry::new(400.0, 32, 100, 50)
            .expect("history should produce an indicator");
        let top = ScrollbarGeometry::new(400.0, 32, 100, 100)
            .expect("history should produce an indicator");
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
            (bottom.thumb_start - (390.0 - bottom.thumb_size)).abs()
                < f32::EPSILON
        );
        assert_eq!(top.offset_for_thumb_start(top.thumb_start), 100);
        assert_eq!(middle.offset_for_thumb_start(middle.thumb_start), 50);
        assert_eq!(bottom.offset_for_thumb_start(bottom.thumb_start), 0);

        let deep = ScrollbarGeometry::new(400.0, 32, 10_000, 0)
            .expect("deep history should produce an indicator");
        assert!((deep.thumb_size - MIN_THUMB_SIZE).abs() < f32::EPSILON);
        assert_eq!(deep.offset_for_thumb_start(0.0), 10_000);
        assert_eq!(deep.offset_for_thumb_start(400.0 - deep.thumb_size), 0);
    }
}
