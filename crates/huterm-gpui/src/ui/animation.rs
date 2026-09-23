//! Presentation work can need a frame, a deadline, or both.

use std::time::Instant;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AnimationSchedule {
    pub(crate) frame: bool,
    pub(crate) deadline: Option<Instant>,
}

impl AnimationSchedule {
    pub(crate) const IDLE: Self = Self {
        frame: false,
        deadline: None,
    };
    pub(crate) const FRAME: Self = Self {
        frame: true,
        deadline: None,
    };

    pub(crate) fn at(deadline: Instant) -> Self {
        Self {
            frame: false,
            deadline: Some(deadline),
        }
    }

    pub(crate) fn merge(self, other: Self) -> Self {
        Self {
            frame: self.frame || other.frame,
            deadline: match (self.deadline, other.deadline) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            },
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
