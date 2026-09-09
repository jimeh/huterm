use std::time::{Duration, Instant};

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
}

impl Reveal {
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
        let elapsed = self.last.replace(now).map_or(0.0, |last| {
            now.saturating_duration_since(last).as_secs_f32()
        });
        let show = if hover {
            self.leave = None;
            true
        } else {
            now.saturating_duration_since(*self.leave.get_or_insert(now))
                < Duration::from_millis(300)
                && self.progress > 0.0
        };
        let delta = elapsed / 0.15;
        self.progress =
            (self.progress + if show { delta } else { -delta }).clamp(0.0, 1.0);
        (before - self.progress).abs() > f32::EPSILON
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        reveal.advance(now + Duration::from_millis(524), false, true);
        assert!((reveal.progress - 0.5).abs() < 0.001);
        reveal.advance(now + Duration::from_millis(599), true, true);
        assert!((reveal.progress - 1.0).abs() < f32::EPSILON);
        reveal.advance(now + Duration::from_millis(600), true, false);
        assert!(reveal.progress.abs() < f32::EPSILON);
    }
}
