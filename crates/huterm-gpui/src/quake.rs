//! Desktop quake profile policy. Native calls remain in platform adapters.

use std::time::{Duration, Instant};

pub(crate) mod hotkeys;
#[cfg(target_os = "linux")]
#[path = "quake/x11.rs"]
pub(crate) mod native;
#[cfg(target_os = "macos")]
#[path = "quake/macos.rs"]
pub(crate) mod native;

pub(crate) use huterm_config::quake::{Animation, Config, Position, Profile};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

pub(crate) trait ProfileExt {
    fn effects(&self) -> (bool, Option<Edge>);
    fn geometry(&self, display: &Display) -> Rect;
}

impl ProfileExt for Profile {
    fn effects(&self) -> (bool, Option<Edge>) {
        let edge = match self.position {
            Position::Top | Position::Center => Edge::Top,
            Position::Bottom => Edge::Bottom,
            Position::Left => Edge::Left,
            Position::Right => Edge::Right,
        };
        match self.animation {
            Animation::Auto => (
                true,
                (!self.fullscreen && self.position != Position::Center)
                    .then_some(edge),
            ),
            Animation::Slide => (false, Some(edge)),
            Animation::None => (false, None),
            Animation::Fade => (true, None),
            Animation::SlideTop => (false, Some(Edge::Top)),
            Animation::SlideBottom => (false, Some(Edge::Bottom)),
            Animation::SlideLeft => (false, Some(Edge::Left)),
            Animation::SlideRight => (false, Some(Edge::Right)),
            Animation::FadeSlideTop => (true, Some(Edge::Top)),
            Animation::FadeSlideBottom => (true, Some(Edge::Bottom)),
            Animation::FadeSlideLeft => (true, Some(Edge::Left)),
            Animation::FadeSlideRight => (true, Some(Edge::Right)),
        }
    }
    fn geometry(&self, display: &Display) -> Rect {
        if self.fullscreen {
            return display.frame;
        }
        let area = display.work;
        let width = (area.width * self.width).max(280.0).min(area.width);
        let height = (area.height * self.height).max(180.0).min(area.height);
        let mut result = Rect {
            x: area.x + (area.width - width) / 2.0,
            y: area.y + (area.height - height) / 2.0,
            width,
            height,
        };
        match self.position {
            Position::Top => result.y = area.y,
            Position::Bottom => result.y = area.y + area.height - height,
            Position::Left => result.x = area.x,
            Position::Right => result.x = area.x + area.width - width,
            Position::Center => {}
        }
        result
    }
}

/// Native coordinates use a top-left origin, in platform window units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl Rect {
    pub fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x + self.width
            && y < self.y + self.height
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Display {
    pub id: String,
    pub frame: Rect,
    pub work: Rect,
    pub primary: bool,
}

/// Continuous progress survives a reversal without saving animation geometry.
#[derive(Clone, Debug)]
pub(crate) struct Transition {
    progress: f64,
    target: bool,
    sampled: Instant,
}
impl Transition {
    pub fn new(visible: bool, now: Instant) -> Self {
        Self {
            progress: f64::from(visible),
            target: visible,
            sampled: now,
        }
    }
    pub fn retarget(
        &mut self,
        visible: bool,
        now: Instant,
        duration: Duration,
    ) {
        self.sample(now, duration);
        self.target = visible;
    }
    pub fn sample(&mut self, now: Instant, duration: Duration) -> f64 {
        let delta = if duration.is_zero() {
            1.0
        } else {
            now.saturating_duration_since(self.sampled).as_secs_f64()
                / duration.as_secs_f64()
        };
        self.progress = (self.progress
            + if self.target { delta } else { -delta })
        .clamp(0.0, 1.0);
        self.sampled = now;
        self.progress
    }
    pub fn pause(&mut self, now: Instant) {
        self.sampled = now;
    }
    #[expect(
        clippy::float_cmp,
        reason = "sample clamps to the exact binary endpoints zero and one"
    )]
    pub fn finished(&self) -> bool {
        self.progress == f64::from(self.target)
    }
    pub fn visible(&self) -> bool {
        self.target
    }
}

pub(crate) fn animation_frame(
    target: Rect,
    display: Rect,
    edge: Option<Edge>,
    progress: f64,
) -> Rect {
    let mut frame = target;
    let hidden = 1.0 - progress;
    match edge {
        Some(Edge::Top) => {
            frame.y -= (target.y + target.height - display.y) * hidden;
        }
        Some(Edge::Bottom) => {
            frame.y += (display.y + display.height - target.y) * hidden;
        }
        Some(Edge::Left) => {
            frame.x -= (target.x + target.width - display.x) * hidden;
        }
        Some(Edge::Right) => {
            frame.x += (display.x + display.width - target.x) * hidden;
        }
        None => {}
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_effects_follow_fullscreen_and_anchor() {
        let mut profile = Profile::default();
        assert_eq!(profile.effects(), (true, Some(Edge::Top)));
        profile.position = Position::Right;
        assert_eq!(profile.effects(), (true, Some(Edge::Right)));
        profile.fullscreen = true;
        assert_eq!(profile.effects(), (true, None));
        profile.animation = Animation::FadeSlideBottom;
        assert_eq!(profile.effects(), (true, Some(Edge::Bottom)));
    }
    #[test]
    fn center_auto_fades_and_slide_uses_position_with_top_center_fallback() {
        let mut profile = Profile {
            position: Position::Center,
            ..Profile::default()
        };
        assert_eq!(profile.effects(), (true, None));
        profile.animation = Animation::Slide;
        for (position, edge) in [
            (Position::Center, Edge::Top),
            (Position::Top, Edge::Top),
            (Position::Bottom, Edge::Bottom),
            (Position::Left, Edge::Left),
            (Position::Right, Edge::Right),
        ] {
            profile.position = position;
            for fullscreen in [false, true] {
                profile.fullscreen = fullscreen;
                assert_eq!(profile.effects(), (false, Some(edge)));
            }
        }
        profile.position = Position::Center;
        profile.animation = Animation::SlideRight;
        assert_eq!(profile.effects(), (false, Some(Edge::Right)));
    }

    #[test]
    fn reversing_keeps_position_and_completes_without_a_jump() {
        let now = Instant::now();
        let duration = Duration::from_millis(100);
        let mut transition = Transition::new(false, now);
        transition.retarget(true, now, duration);
        let midpoint = now + Duration::from_millis(40);
        assert!(
            (transition.sample(midpoint, duration) - 0.4).abs() < f64::EPSILON
        );
        transition.retarget(false, midpoint, duration);
        assert!(
            (transition.sample(midpoint, duration) - 0.4).abs() < f64::EPSILON
        );
        assert!(
            transition.sample(midpoint + duration, duration).abs()
                < f64::EPSILON
        );
        assert!(transition.finished());
    }
    #[test]
    fn placement_honors_work_area_and_minimum_without_exceeding_display() {
        let display = Display {
            id: "test".into(),
            primary: true,
            frame: Rect {
                x: -800.0,
                y: 0.0,
                width: 800.0,
                height: 600.0,
            },
            work: Rect {
                x: -800.0,
                y: 30.0,
                width: 800.0,
                height: 570.0,
            },
        };
        let mut profile = Profile {
            width: 0.01,
            height: 0.01,
            position: Position::Bottom,
            ..Profile::default()
        };
        assert_eq!(
            profile.geometry(&display),
            Rect {
                x: -540.0,
                y: 420.0,
                width: 280.0,
                height: 180.0
            }
        );
        profile.position = Position::Center;
        assert_eq!(
            profile.geometry(&display),
            Rect {
                x: -540.0,
                y: 225.0,
                width: 280.0,
                height: 180.0
            }
        );
        profile.fullscreen = true;
        assert_eq!(profile.geometry(&display), display.frame);
    }
}
