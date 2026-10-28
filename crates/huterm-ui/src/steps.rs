//! The range and step grid a number field or slider moves along.
//!
//! Values on the grid are `min + k * step`, clamped to `max`, so `max` is
//! reachable even when it is not on the grid. Stepping from a value off the
//! grid, such as one read from a file, first lands on the next grid value in
//! that direction, as React Aria's number field and the WAI-ARIA slider do.
//! Every result is clamped to the range: a value below the minimum steps to
//! the minimum in either direction, because that is the nearest valid
//! value. An exclusive bound is modelled as the nearest grid value inside
//! it, as the Quake width's 5% floor is for its exclusive 0.

/// A closed range and the step grid inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Steps {
    min: f64,
    max: f64,
    step: f64,
    page: u32,
}

/// Grid values are rounded to this many decimal places, so repeated steps
/// such as `0.05 * 3` do not drift off the grid.
const PRECISION: f64 = 1e9;

impl Steps {
    /// The range `min..=max` in steps of `step`, paging by ten steps.
    ///
    /// # Panics
    /// In debug builds, when `min` exceeds `max` or `step` is not positive.
    #[must_use]
    pub fn new(min: f64, max: f64, step: f64) -> Self {
        debug_assert!(min <= max, "an empty range: {min} > {max}");
        debug_assert!(step > 0.0, "a step must be positive: {step}");
        Self {
            min,
            max,
            step,
            page: 10,
        }
    }

    /// Pages by `steps` steps, as Page Up and Page Down do.
    #[must_use]
    pub fn page(mut self, steps: u32) -> Self {
        self.page = steps.max(1);
        self
    }

    /// The lowest value.
    #[must_use]
    pub fn min(self) -> f64 {
        self.min
    }

    /// The highest value.
    #[must_use]
    pub fn max(self) -> f64 {
        self.max
    }

    /// The distance between grid values.
    #[must_use]
    pub fn step(self) -> f64 {
        self.step
    }

    /// `value` limited to the range.
    #[must_use]
    pub fn clamp(self, value: f64) -> f64 {
        value.clamp(self.min, self.max)
    }

    /// The grid value nearest to `value`, inside the range.
    #[must_use]
    pub fn snap(self, value: f64) -> f64 {
        let index = ((self.clamp(value) - self.min) / self.step).round();
        self.at_index(index)
    }

    /// `value` moved `steps` grid steps, up for a positive count. A value
    /// off the grid counts landing on the next grid value as the first
    /// step.
    #[must_use]
    pub fn step_by(self, value: f64, steps: i32) -> f64 {
        if steps == 0 {
            return self.clamp(value);
        }
        let position = (value - self.min) / self.step;
        let nearest = position.round();
        let on_grid = (position - nearest).abs() < 1e-6;
        let index = match (on_grid, steps > 0) {
            (true, _) => nearest + f64::from(steps),
            (false, true) => position.ceil() + f64::from(steps - 1),
            (false, false) => position.floor() + f64::from(steps + 1),
        };
        self.at_index(index)
    }

    /// One step up.
    #[must_use]
    pub fn increment(self, value: f64) -> f64 {
        self.step_by(value, 1)
    }

    /// One step down.
    #[must_use]
    pub fn decrement(self, value: f64) -> f64 {
        self.step_by(value, -1)
    }

    /// One page up.
    #[must_use]
    pub fn page_up(self, value: f64) -> f64 {
        self.step_by(value, self.page_steps())
    }

    /// One page down.
    #[must_use]
    pub fn page_down(self, value: f64) -> f64 {
        self.step_by(value, -self.page_steps())
    }

    /// The grid value at `fraction` of the way from the minimum to the
    /// maximum, as a pointer along a slider's track picks.
    #[must_use]
    pub fn at_fraction(self, fraction: f64) -> f64 {
        let fraction = fraction.clamp(0.0, 1.0);
        self.snap(self.min + fraction * (self.max - self.min))
    }

    /// How far `value` lies from the minimum to the maximum, from 0 to 1.
    #[must_use]
    pub fn fraction(self, value: f64) -> f64 {
        let span = self.max - self.min;
        if span <= 0.0 {
            return 0.0;
        }
        ((self.clamp(value) - self.min) / span).clamp(0.0, 1.0)
    }

    fn page_steps(self) -> i32 {
        i32::try_from(self.page).unwrap_or(i32::MAX)
    }

    fn at_index(self, index: f64) -> f64 {
        let value = self.min + index * self.step;
        self.clamp((value * PRECISION).round() / PRECISION)
    }
}

#[cfg(test)]
mod tests {
    use super::Steps;

    /// The Quake width and height: greater than 0 and at most 1, in 5%
    /// steps, so the floor is one step.
    fn quake() -> Steps {
        Steps::new(0.05, 1.0, 0.05)
    }

    #[test]
    fn steps_stay_on_the_grid_without_drift() {
        // In floating point, 0.1 * 3 is 0.30000000000000004.
        let steps = Steps::new(0.0, 1.0, 0.1);
        let mut value = 0.0;
        for _ in 0..3 {
            value = steps.increment(value);
        }
        assert_eq!(value, 0.3);
        assert_eq!(steps.decrement(value), 0.2);
    }

    #[test]
    fn every_step_clamps_to_the_range() {
        // D7: stepping a Quake width or height must never leave (0, 1].
        let steps = quake();
        assert_eq!(steps.increment(1.0), 1.0);
        assert_eq!(steps.page_up(0.9), 1.0);
        assert_eq!(steps.decrement(0.05), 0.05);
        assert_eq!(steps.page_down(0.3), 0.05);
        assert_eq!(steps.increment(4.0), 1.0);
    }

    #[test]
    fn off_grid_values_step_to_the_next_grid_value() {
        let steps = quake();
        assert_eq!(steps.increment(0.33), 0.35);
        assert_eq!(steps.decrement(0.33), 0.3);
        // The next grid value, not the nearest one.
        assert_eq!(steps.increment(0.31), 0.35);
        assert_eq!(steps.decrement(0.34), 0.3);
        // Below the floor, the floor is the nearest valid value either way.
        assert_eq!(steps.increment(0.02), 0.05);
        assert_eq!(steps.decrement(0.02), 0.05);
    }

    #[test]
    fn a_maximum_off_the_grid_is_still_reached() {
        let steps = Steps::new(0.0, 1.0, 0.3);
        assert_eq!(steps.increment(0.9), 1.0);
        assert_eq!(steps.decrement(1.0), 0.9);
    }

    #[test]
    fn pages_move_by_their_step_count() {
        let steps = Steps::new(0.0, 100.0, 1.0).page(10);
        assert_eq!(steps.page_up(42.0), 52.0);
        assert_eq!(steps.page_down(42.0), 32.0);
        assert_eq!(steps.page_up(95.0), 100.0);
    }

    #[test]
    fn fractions_map_to_grid_values_inside_the_range() {
        let steps = quake();
        assert_eq!(steps.at_fraction(-0.5), 0.05);
        assert_eq!(steps.at_fraction(1.5), 1.0);
        assert_eq!(steps.at_fraction(0.25), 0.3);
        assert_eq!(steps.fraction(0.05), 0.0);
        assert_eq!(steps.fraction(1.0), 1.0);
        assert_eq!(steps.fraction(0.0), 0.0);
        assert_eq!(steps.fraction(2.0), 1.0);
        assert_eq!(steps.snap(0.33), 0.35);
    }
}
