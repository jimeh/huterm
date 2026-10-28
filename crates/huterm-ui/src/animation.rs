//! Presentation work can need a frame, a deadline, or both.

use std::time::Instant;

use gpui::{Context, Task, Window};

/// When an animated component next needs attention.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AnimationSchedule {
    /// It needs the next frame, such as during a fade.
    pub frame: bool,
    /// It needs a wake at this instant, such as when a hold ends.
    pub deadline: Option<Instant>,
}

impl AnimationSchedule {
    /// Nothing to do until input arrives.
    pub const IDLE: Self = Self {
        frame: false,
        deadline: None,
    };
    /// The next frame, and no deadline.
    pub const FRAME: Self = Self {
        frame: true,
        deadline: None,
    };

    /// A wake at `deadline`, and no frame before it.
    #[must_use]
    pub fn at(deadline: Instant) -> Self {
        Self {
            frame: false,
            deadline: Some(deadline),
        }
    }

    /// Both schedules' needs: a frame if either wants one, and the earlier
    /// deadline.
    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        Self {
            frame: self.frame || other.frame,
            deadline: match (self.deadline, other.deadline) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            },
        }
    }
}

/// Wakes a view at its animation schedule: the next frame, or a timer at
/// the deadline, such as the end of a scrollbar indicator's hold, so the
/// animation continues without further input.
#[derive(Default)]
pub struct Wake {
    armed: Option<(Instant, Task<()>)>,
}

impl std::fmt::Debug for Wake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wake")
            .field("deadline", &self.armed.as_ref().map(|(at, _)| *at))
            .finish()
    }
}

impl Wake {
    /// Requests `schedule`'s frame, and arms one timer for its deadline
    /// that notifies the view; an unchanged deadline keeps its timer, and
    /// no deadline drops it. Call it while rendering.
    pub fn arm<V: 'static>(
        &mut self,
        schedule: AnimationSchedule,
        window: &mut Window,
        cx: &mut Context<'_, V>,
    ) {
        if schedule.frame {
            window.request_animation_frame();
        }
        match schedule.deadline {
            Some(deadline)
                if self.armed.as_ref().map(|(at, _)| *at) != Some(deadline) =>
            {
                let timer = cx
                    .background_executor()
                    .timer(deadline.saturating_duration_since(Instant::now()));
                let task = cx.spawn(async move |view, cx| {
                    timer.await;
                    let _ = view.update(cx, |_, cx| cx.notify());
                });
                self.armed = Some((deadline, task));
            }
            Some(_) => {}
            None => self.armed = None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn frames_do_not_discard_the_earliest_deadline() {
        let now = Instant::now();
        let schedule = AnimationSchedule::at(now + Duration::from_secs(2))
            .merge(AnimationSchedule::FRAME)
            .merge(AnimationSchedule::at(now + Duration::from_secs(1)));
        assert!(schedule.frame);
        assert_eq!(schedule.deadline, Some(now + Duration::from_secs(1)));
    }
}
