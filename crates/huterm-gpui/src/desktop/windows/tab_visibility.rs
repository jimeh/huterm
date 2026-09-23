use std::time::{Duration, Instant};

use crate::ui::animation::AnimationSchedule;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::desktop) enum Presentation {
    #[default]
    Hidden,
    Reserved,
    Overlay,
}

impl Presentation {
    pub(super) fn resolve(
        count: usize,
        always: bool,
        fullscreen: bool,
        auto_hide: bool,
    ) -> Self {
        if count == 0 {
            Self::Hidden
        } else if fullscreen && auto_hide {
            Self::Overlay
        } else if count > 1 || always {
            Self::Reserved
        } else {
            Self::Hidden
        }
    }
}

#[derive(Default)]
pub(super) struct Reveal {
    pub(super) progress: f32,
    last: Option<Instant>,
    leave: Option<Instant>,
    activity_until: Option<Instant>,
    hover: bool,
    enabled: bool,
}

impl Reveal {
    pub(super) fn set_input(
        &mut self,
        now: Instant,
        hover: bool,
        enabled: bool,
    ) -> bool {
        if (self.hover, self.enabled) == (hover, enabled)
            && (enabled || self.activity_until.is_none())
        {
            return false;
        }
        if !enabled {
            *self = Self::default();
        }
        if hover || self.activity_until.is_some_and(|until| now < until) {
            self.leave = None;
        }
        self.hover = hover;
        self.enabled = enabled;
        self.last = Some(now);
        true
    }

    pub(super) fn schedule(&self, now: Instant) -> AnimationSchedule {
        if !self.enabled {
            return if self.progress > 0.0 {
                AnimationSchedule::FRAME
            } else {
                AnimationSchedule::IDLE
            };
        }
        let activity = self.activity_until.filter(|at| *at > now);
        if self.hover || activity.is_some() {
            let mut next = if self.progress < 1.0 {
                AnimationSchedule::FRAME
            } else {
                AnimationSchedule::IDLE
            };
            if !self.hover
                && let Some(at) = activity
            {
                next = next.merge(AnimationSchedule::at(at));
            }
            return next;
        }
        if self.progress == 0.0 {
            return AnimationSchedule::IDLE;
        }
        match self.leave {
            None => AnimationSchedule::at(now),
            Some(left) if now < left + Duration::from_millis(300) => {
                let deadline =
                    AnimationSchedule::at(left + Duration::from_millis(300));
                if self.progress < 1.0 {
                    deadline.merge(AnimationSchedule::FRAME)
                } else {
                    deadline
                }
            }
            Some(_) => AnimationSchedule::FRAME,
        }
    }

    pub(super) fn tick(&mut self, now: Instant) -> bool {
        self.advance(now, self.hover, self.enabled)
    }

    pub(super) fn reveal_for_activity(&mut self, now: Instant) {
        self.activity_until = Some(now + Duration::from_secs(1));
        self.leave = None;
        self.last = Some(now);
    }

    pub(super) fn advance(
        &mut self,
        now: Instant,
        hover: bool,
        enabled: bool,
    ) -> bool {
        let before = self.progress;
        if !enabled {
            *self = Self::default();
            return before != 0.0;
        }
        let previous_tick = self.last.replace(now).unwrap_or(now);
        let show =
            if hover || self.activity_until.is_some_and(|until| now < until) {
                self.leave = None;
                true
            } else {
                now.saturating_duration_since(*self.leave.get_or_insert(now))
                    < Duration::from_millis(300)
                    && self.progress > 0.0
            };
        // A deadline skips the static hold. Only time after that hold belongs
        // to the hiding animation, even if no frame arrived in between.
        let animation_start = if show {
            previous_tick
        } else {
            self.leave.map_or(previous_tick, |left| {
                previous_tick.max(left + Duration::from_millis(300))
            })
        };
        let delta =
            now.saturating_duration_since(animation_start).as_secs_f32() / 0.15;
        self.progress =
            (self.progress + if show { delta } else { -delta }).clamp(0.0, 1.0);
        (before - self.progress).abs() > f32::EPSILON
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn returning_to_a_settled_reveal_starts_a_fresh_leave_hold() {
        let now = Instant::now();
        let mut reveal = Reveal::default();
        reveal.set_input(now, true, true);
        reveal.tick(now + Duration::from_millis(150));
        let left = now + Duration::from_secs(1);
        reveal.set_input(left, false, true);
        reveal.tick(left);
        let returned = left + Duration::from_millis(100);
        reveal.set_input(returned, true, true);
        assert_eq!(reveal.schedule(returned), AnimationSchedule::IDLE);
        // No animation tick occurs during this settled hover.
        let left_again = now + Duration::from_secs(2);
        reveal.set_input(left_again, false, true);
        reveal.tick(left_again);
        assert_eq!(
            reveal.schedule(left_again),
            AnimationSchedule::at(left_again + Duration::from_millis(300))
        );
        reveal.reveal_for_activity(left_again + Duration::from_millis(100));
        let expiry = left_again + Duration::from_millis(1100);
        reveal.tick(expiry);
        assert_eq!(
            reveal.schedule(expiry),
            AnimationSchedule::at(expiry + Duration::from_millis(300))
        );
    }

    #[test]
    fn leaving_during_reveal_finishes_opening_during_the_hold() {
        let now = Instant::now();
        let mut reveal = Reveal::default();
        reveal.set_input(now, true, true);
        let left = now + Duration::from_millis(75);
        reveal.tick(left);
        reveal.set_input(left, false, true);
        reveal.tick(left);
        let schedule = reveal.schedule(left);
        assert!(
            schedule.frame,
            "partial reveal must continue opening during its hold"
        );
        assert_eq!(schedule.deadline, Some(left + Duration::from_millis(300)));
        reveal.tick(left + Duration::from_millis(75));
        assert!((reveal.progress - 1.0).abs() < f32::EPSILON);
        assert!(!reveal.schedule(left + Duration::from_millis(75)).frame);
    }

    #[test]
    fn deadline_wake_starts_hiding_without_skipping_the_animation() {
        let now = Instant::now();
        let mut reveal = Reveal::default();
        assert!(reveal.set_input(now, true, true));
        assert_eq!(reveal.schedule(now), AnimationSchedule::FRAME);
        reveal.tick(now + Duration::from_millis(150));
        assert!((reveal.progress - 1.0).abs() < f32::EPSILON);
        assert_eq!(
            reveal.schedule(now + Duration::from_millis(150)),
            AnimationSchedule::IDLE
        );
        let left = now + Duration::from_secs(1);
        reveal.set_input(left, false, true);
        reveal.tick(left);
        let deadline = left + Duration::from_millis(300);
        assert_eq!(reveal.schedule(left), AnimationSchedule::at(deadline));
        assert!(!reveal.tick(deadline));
        assert_eq!(reveal.schedule(deadline), AnimationSchedule::FRAME);
        assert!(reveal.tick(deadline + Duration::from_millis(75)));
        assert!((reveal.progress - 0.5).abs() < 0.001);
        reveal.tick(deadline + Duration::from_secs(1));
        assert!(reveal.progress.abs() < f32::EPSILON);
        assert_eq!(
            reveal.schedule(deadline + Duration::from_secs(1)),
            AnimationSchedule::IDLE
        );
    }

    #[test]
    fn count_and_fullscreen_policy() {
        for (count, always, fullscreen, auto, expected) in [
            (0, false, false, false, Presentation::Hidden),
            (0, true, true, true, Presentation::Hidden),
            (1, false, false, false, Presentation::Hidden),
            (1, true, false, false, Presentation::Reserved),
            (2, false, false, false, Presentation::Reserved),
            (2, false, false, true, Presentation::Reserved),
            (1, false, true, false, Presentation::Hidden),
            (2, false, true, false, Presentation::Reserved),
            (1, false, true, true, Presentation::Overlay),
            (1, true, true, true, Presentation::Overlay),
            (2, true, true, true, Presentation::Overlay),
        ] {
            assert_eq!(
                Presentation::resolve(count, always, fullscreen, auto),
                expected
            );
        }
    }
    #[test]
    fn activity_reveal_restarts_hold_and_respects_cancellation() {
        let now = Instant::now();
        let mut reveal = Reveal::default();
        reveal.reveal_for_activity(now);
        reveal.advance(now + Duration::from_millis(150), false, true);
        assert!((reveal.progress - 1.0).abs() < f32::EPSILON);
        reveal.reveal_for_activity(now + Duration::from_millis(900));
        reveal.advance(now + Duration::from_millis(1500), false, true);
        assert!((reveal.progress - 1.0).abs() < f32::EPSILON);
        reveal.advance(now + Duration::from_millis(1900), false, true);
        reveal.advance(now + Duration::from_millis(2199), false, true);
        assert!((reveal.progress - 1.0).abs() < f32::EPSILON);
        reveal.advance(now + Duration::from_millis(2400), false, true);
        assert!(reveal.progress.abs() < f32::EPSILON);
        reveal.reveal_for_activity(now + Duration::from_millis(2500));
        reveal.advance(now + Duration::from_millis(2650), false, true);
        assert!((reveal.progress - 1.0).abs() < f32::EPSILON);
        reveal.advance(now + Duration::from_millis(2700), false, false);
        reveal.advance(now + Duration::from_millis(2800), false, true);
        assert!(reveal.progress.abs() < f32::EPSILON);
    }

    #[test]
    fn reveal_delays_dismissal_and_reverses_without_jumping() {
        let now = Instant::now();
        let mut reveal = Reveal::default();
        reveal.advance(now, true, true);
        reveal.advance(now + Duration::from_millis(75), true, true);
        assert!((reveal.progress - 0.5).abs() < 0.001);
        reveal.advance(now + Duration::from_millis(150), false, true);
        assert!((reveal.progress - 1.0).abs() < f32::EPSILON);
        reveal.advance(now + Duration::from_millis(449), false, true);
        assert!((reveal.progress - 1.0).abs() < f32::EPSILON);
        reveal.advance(now + Duration::from_millis(525), false, true);
        assert!((reveal.progress - 0.5).abs() < 0.001);
        reveal.advance(now + Duration::from_millis(600), true, true);
        assert!((reveal.progress - 1.0).abs() < f32::EPSILON);
        reveal.advance(now + Duration::from_millis(601), true, false);
        assert!(reveal.progress.abs() < f32::EPSILON);
    }
}
