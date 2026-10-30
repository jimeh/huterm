//! When the `columns × rows` indicator shows while a terminal resizes. A
//! resize shows it fully; it lingers for `[window]
//! size_indicator_linger_ms` and then fades. While a window edge or a pane
//! divider is still being dragged, the drag holds it up even when no resize
//! arrives, and the linger starts only when the drag ends. Every instant
//! comes from the caller, so tests drive it without sleeping.

use std::time::{Duration, Instant};

use crate::ui::animation::AnimationSchedule;
use crate::ui::scrollbar::INDICATOR_FADE;

#[derive(Debug, Default)]
pub(super) struct SizeIndicatorVisibility {
    /// When the fade starts, while one is due; `None` while hidden or
    /// held.
    fade_start: Option<Instant>,
    /// A drag holds the indicator.
    held: bool,
    /// The indicator's current opacity, from 0 to 1.
    pub(super) opacity: f32,
}

impl SizeIndicatorVisibility {
    /// The grid's size changed at `now`: shows the indicator fully and,
    /// unless a drag holds it, restarts its `linger`.
    pub(super) fn resized(&mut self, now: Instant, linger: Duration) {
        self.opacity = 1.0;
        self.fade_start = (!self.held).then(|| now + linger);
    }

    /// A drag that resizes this terminal starts or ends at `now`. A hold
    /// keeps a showing indicator fully up and never shows a hidden one;
    /// the release starts the `linger`. Returns whether the opacity or its
    /// schedule changed.
    pub(super) fn hold(
        &mut self,
        held: bool,
        now: Instant,
        linger: Duration,
    ) -> bool {
        if self.held == held {
            return false;
        }
        self.held = held;
        if self.opacity <= 0.0 {
            return false;
        }
        if held {
            self.opacity = 1.0;
            self.fade_start = None;
        } else {
            self.fade_start = Some(now + linger);
        }
        true
    }

    /// Hides the indicator at once and forgets any hold, as a view does
    /// when it stops showing.
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    /// When the indicator next needs a frame or a wake. A held indicator
    /// waits for its drag to end and needs neither.
    pub(super) fn schedule(&self, now: Instant) -> AnimationSchedule {
        match self.fade_start {
            Some(at) if now < at => AnimationSchedule::at(at),
            Some(_) if self.opacity > 0.0 => AnimationSchedule::FRAME,
            _ => AnimationSchedule::IDLE,
        }
    }

    /// Advances the fade to `now`; true when the opacity changed.
    pub(super) fn update(&mut self, now: Instant) -> bool {
        let Some(start) = self.fade_start else {
            return false;
        };
        let previous = self.opacity;
        self.opacity = 1.0
            - (now.saturating_duration_since(start).as_secs_f32()
                / INDICATOR_FADE.as_secs_f32())
            .clamp(0.0, 1.0);
        if self.opacity <= 0.0 {
            self.fade_start = None;
        }
        (self.opacity - previous).abs() > f32::EPSILON
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINGER: Duration = Duration::from_millis(500);

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    fn shows(indicator: &SizeIndicatorVisibility) -> bool {
        (indicator.opacity - 1.0).abs() < f32::EPSILON
    }

    #[test]
    fn a_resize_lingers_then_fades() {
        let now = Instant::now();
        let mut indicator = SizeIndicatorVisibility::default();
        indicator.resized(now, LINGER);
        assert!(shows(&indicator));
        assert_eq!(
            indicator.schedule(now),
            AnimationSchedule::at(now + LINGER)
        );
        indicator.update(now + ms(499));
        assert!(shows(&indicator));
        indicator.update(now + LINGER + INDICATOR_FADE / 2);
        assert!((indicator.opacity - 0.5).abs() < 0.01);
        indicator.update(now + LINGER + INDICATOR_FADE);
        assert!(indicator.opacity <= 0.0);
        assert_eq!(
            indicator.schedule(now + LINGER + INDICATOR_FADE),
            AnimationSchedule::IDLE
        );
    }

    #[test]
    fn a_drag_holds_it_past_the_linger_and_its_release_starts_the_linger() {
        let now = Instant::now();
        let mut indicator = SizeIndicatorVisibility::default();
        // The drag starts before its first resize arrives.
        assert!(!indicator.hold(true, now, LINGER));
        indicator.resized(now + ms(10), LINGER);
        assert_eq!(indicator.schedule(now + ms(10)), AnimationSchedule::IDLE);
        // Resting mid-drag well past the linger still shows it.
        let resting = now + ms(10) + LINGER * 4;
        assert!(!indicator.update(resting));
        assert!(shows(&indicator));
        let released = resting + ms(1);
        assert!(indicator.hold(false, released, LINGER));
        assert_eq!(
            indicator.schedule(released),
            AnimationSchedule::at(released + LINGER)
        );
        indicator.update(released + ms(499));
        assert!(shows(&indicator));
        indicator.update(released + LINGER + INDICATOR_FADE);
        assert!(indicator.opacity <= 0.0);
    }

    #[test]
    fn a_hold_brings_back_a_fading_indicator_and_never_shows_a_hidden_one() {
        let now = Instant::now();
        let mut indicator = SizeIndicatorVisibility::default();
        assert!(!indicator.hold(true, now, LINGER));
        assert!(indicator.opacity <= 0.0);
        assert!(!indicator.hold(false, now, LINGER));
        assert_eq!(indicator.schedule(now), AnimationSchedule::IDLE);
        indicator.resized(now, LINGER);
        let fading = now + LINGER + INDICATOR_FADE / 2;
        indicator.update(fading);
        assert!(indicator.hold(true, fading, LINGER));
        assert!(shows(&indicator));
    }

    #[test]
    fn a_zero_linger_waits_for_the_release_without_frames() {
        let now = Instant::now();
        let mut indicator = SizeIndicatorVisibility::default();
        indicator.hold(true, now, Duration::ZERO);
        indicator.resized(now, Duration::ZERO);
        assert_eq!(indicator.schedule(now + ms(100)), AnimationSchedule::IDLE);
        indicator.hold(false, now + ms(100), Duration::ZERO);
        assert_eq!(indicator.schedule(now + ms(100)), AnimationSchedule::FRAME);
        indicator.update(now + ms(100) + INDICATOR_FADE);
        assert!(indicator.opacity <= 0.0);
    }

    #[test]
    fn a_reset_hides_it_and_forgets_the_hold() {
        let now = Instant::now();
        let mut indicator = SizeIndicatorVisibility::default();
        indicator.hold(true, now, LINGER);
        indicator.resized(now, LINGER);
        indicator.reset();
        assert!(indicator.opacity <= 0.0);
        indicator.resized(now, LINGER);
        assert_eq!(
            indicator.schedule(now),
            AnimationSchedule::at(now + LINGER)
        );
    }
}
