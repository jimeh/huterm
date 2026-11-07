//! How a program's progress report (OSC 9;4) is drawn: a ring in the
//! status slot of its tab and pane title, and an optional bar across the
//! top of its pane.
//!
//! Indeterminate progress moves in steps of [`STEP`] on the window's
//! earliest-deadline timer rather than every frame, and stands still while
//! the app asks for reduced motion.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, Hsla, IntoElement, ParentElement, PathBuilder, Pixels,
    Styled, canvas, div, point, px, relative,
};
use huterm_protocol::TerminalProgress;

use crate::config::Theme;
use crate::renderer::rgb_color as color;

/// How long each step of indeterminate motion holds.
pub(super) const STEP: Duration = Duration::from_millis(125);
/// Steps in one turn of the ring or one sweep of the bar.
pub(super) const STEPS: u32 = 12;
/// The bar's height across the top of a pane.
pub(super) const BAR_HEIGHT: Pixels = px(2.0);
/// The ring's side, the status slot's icon size.
const RING_SIZE: Pixels = px(12.0);
const RING_STROKE: f32 = 2.0;
/// The share of a turn or of the width that indeterminate progress fills.
const INDETERMINATE_SPAN: f32 = 0.25;
const TRACK_OPACITY: f32 = 0.25;
/// Line segments approximating a whole circle.
const CIRCLE_SEGMENTS: u32 = 48;

/// The theme colors progress uses: the tab accent while it runs, the ANSI
/// yellow the running dot uses while paused, and the ANSI red for errors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ProgressColors {
    pub(super) normal: Hsla,
    pub(super) paused: Hsla,
    pub(super) error: Hsla,
}

impl ProgressColors {
    pub(super) fn new(theme: &Theme) -> Self {
        Self {
            normal: color(theme.ui().tab_accent),
            paused: color(theme.ansi[3]),
            error: color(theme.ansi[1]),
        }
    }

    fn of(self, progress: TerminalProgress) -> Hsla {
        match progress {
            TerminalProgress::Determinate(_)
            | TerminalProgress::Indeterminate => self.normal,
            TerminalProgress::Paused(_) => self.paused,
            TerminalProgress::Error(_) => self.error,
        }
    }
}

/// The filled part as `(start, length)`, both fractions of a turn for the
/// ring or of the width for the bar. Errors fill completely, and so does a
/// pause that reports no percentage, so either reads at a glance.
pub(super) fn span(progress: TerminalProgress, phase: u32) -> (f32, f32) {
    match progress {
        TerminalProgress::Determinate(percent)
        | TerminalProgress::Paused(Some(percent)) => {
            (0.0, f32::from(percent.min(100)) / 100.0)
        }
        TerminalProgress::Paused(None) | TerminalProgress::Error(_) => {
            (0.0, 1.0)
        }
        TerminalProgress::Indeterminate => {
            #[expect(
                clippy::cast_precision_loss,
                reason = "phases stay below STEPS"
            )]
            let start = (phase % STEPS) as f32 / STEPS as f32;
            (start, INDETERMINATE_SPAN)
        }
    }
}

/// What assistive technology reads after a tab or pane title.
pub(super) fn description(progress: TerminalProgress) -> String {
    match progress {
        TerminalProgress::Determinate(percent) => {
            format!("progress {percent}%")
        }
        TerminalProgress::Indeterminate => "in progress".to_owned(),
        TerminalProgress::Paused(Some(percent)) => {
            format!("paused at {percent}%")
        }
        TerminalProgress::Paused(None) => "paused".to_owned(),
        TerminalProgress::Error(_) => "progress failed".to_owned(),
    }
}

fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// The indeterminate step shown at `now`. Every view shares one epoch, so
/// all indeterminate indicators move together.
pub(super) fn phase(now: Instant) -> u32 {
    let steps =
        now.saturating_duration_since(epoch()).as_millis() / STEP.as_millis();
    u32::try_from(steps % u128::from(STEPS)).unwrap_or(0)
}

/// When the step after the one shown at `now` begins.
pub(super) fn next_step(now: Instant) -> Instant {
    let elapsed = now.saturating_duration_since(epoch());
    let step = STEP.as_nanos();
    let next = (elapsed.as_nanos() / step + 1) * step;
    epoch() + Duration::from_nanos(u64::try_from(next).unwrap_or(u64::MAX))
}

/// The ring drawn in a status slot.
pub(super) fn ring(
    progress: TerminalProgress,
    phase: u32,
    colors: ProgressColors,
) -> AnyElement {
    let tint = colors.of(progress);
    let (start, length) = span(progress, phase);
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            if let Some(track) = arc(bounds, 0.0, 1.0) {
                window.paint_path(track, tint.opacity(TRACK_OPACITY));
            }
            if length > 0.0
                && let Some(filled) = arc(bounds, start, length)
            {
                window.paint_path(filled, tint);
            }
        },
    )
    .flex_shrink_0()
    .w(RING_SIZE)
    .h(RING_SIZE)
    .into_any_element()
}

/// A stroked arc centred in `bounds`, from `start` for `length` turns,
/// clockwise from twelve o'clock.
fn arc(
    bounds: Bounds<Pixels>,
    start: f32,
    length: f32,
) -> Option<gpui::Path<Pixels>> {
    let center = bounds.center();
    let radius = (f32::from(bounds.size.width.min(bounds.size.height))
        - RING_STROKE)
        / 2.0;
    let length = length.clamp(0.0, 1.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "a positive count no larger than CIRCLE_SEGMENTS"
    )]
    let segments = ((CIRCLE_SEGMENTS as f32 * length).ceil() as u32).max(1);
    let at = |turn: f32| {
        let angle = (turn - 0.25) * std::f32::consts::TAU;
        point(
            center.x + px(radius * angle.cos()),
            center.y + px(radius * angle.sin()),
        )
    };
    let mut path = PathBuilder::stroke(px(RING_STROKE));
    path.move_to(at(start));
    for segment in 1..=segments {
        #[expect(
            clippy::cast_precision_loss,
            reason = "segment counts are small"
        )]
        let turn = start + length * segment as f32 / segments as f32;
        path.line_to(at(turn));
    }
    path.build().ok()
}

/// The bar across the top of a pane, filling its parent's width.
pub(super) fn bar(
    progress: TerminalProgress,
    phase: u32,
    colors: ProgressColors,
) -> AnyElement {
    let tint = colors.of(progress);
    let (start, length) = span(progress, phase);
    // Indeterminate motion sweeps in from the left edge and out past the
    // right one.
    let start = if progress == TerminalProgress::Indeterminate {
        start * (1.0 + INDETERMINATE_SPAN) - INDETERMINATE_SPAN
    } else {
        start
    };
    div()
        .absolute()
        .top_0()
        .left_0()
        .w_full()
        .h(BAR_HEIGHT)
        .overflow_hidden()
        .bg(tint.opacity(TRACK_OPACITY))
        .child(
            div()
                .absolute()
                .top_0()
                .h_full()
                .left(relative(start))
                .w(relative(length))
                .bg(tint),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_follow_the_reported_state() {
        use TerminalProgress::{Determinate, Error, Indeterminate, Paused};
        assert_eq!(span(Determinate(0), 0), (0.0, 0.0));
        assert_eq!(span(Determinate(42), 5), (0.0, 0.42));
        assert_eq!(span(Paused(Some(60)), 0), (0.0, 0.6));
        assert_eq!(span(Paused(None), 0), (0.0, 1.0));
        assert_eq!(span(Error(Some(10)), 0), (0.0, 1.0));
        assert_eq!(span(Indeterminate, 3), (0.25, INDETERMINATE_SPAN));
        assert_eq!(span(Indeterminate, STEPS + 3), (0.25, INDETERMINATE_SPAN));
    }

    #[test]
    fn indeterminate_steps_advance_on_shared_boundaries() {
        let start = epoch();
        assert_eq!(phase(start), 0);
        assert_eq!(next_step(start), start + STEP);
        let inside = start + STEP * 3 + STEP / 2;
        assert_eq!(phase(inside), 3);
        assert_eq!(next_step(inside), start + STEP * 4);
        assert_eq!(phase(start + STEP * STEPS), 0, "a turn wraps");
    }

    #[test]
    fn descriptions_name_the_state() {
        use TerminalProgress::{Determinate, Error, Indeterminate, Paused};
        assert_eq!(description(Determinate(42)), "progress 42%");
        assert_eq!(description(Indeterminate), "in progress");
        assert_eq!(description(Paused(Some(5))), "paused at 5%");
        assert_eq!(description(Paused(None)), "paused");
        assert_eq!(description(Error(Some(5))), "progress failed");
    }
}
