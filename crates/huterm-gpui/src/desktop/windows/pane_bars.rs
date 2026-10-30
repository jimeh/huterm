//! Hover reveal for pane title bars under `[panes] title_bar = "hover"`:
//! each pane's bar fades in once the pointer has rested in its band for the
//! hover delay, stays while the pointer, a pane drag, or the bar's menu
//! holds it, and fades out after the hide delay. A keyboard focus change
//! reveals the focused pane's bar for a moment. Everything here is pure,
//! driven by the window with the inputs it samples.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use huterm_config::PanesConfig;
use huterm_protocol::PaneId;

use super::tab_visibility::{Reveal, RevealTiming};
use crate::ui::animation::AnimationSchedule;

/// The `[panes] title_bar_*_ms` timings of hover bars.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PaneBarTiming {
    /// How long the pointer rests in a bar's band before the bar reveals,
    /// so a pointer heading for a divider or passing through reveals
    /// nothing.
    pub(super) hover_delay: Duration,
    /// The fade each way, and the hold after the pointer leaves.
    pub(super) reveal: RevealTiming,
}

impl PaneBarTiming {
    pub(super) fn from_config(panes: &PanesConfig) -> Self {
        let ms = |value: u16| Duration::from_millis(u64::from(value));
        Self {
            hover_delay: ms(panes.title_bar_hover_delay_ms),
            reveal: RevealTiming {
                fade: ms(panes.title_bar_fade_ms),
                leave_hold: ms(panes.title_bar_hide_delay_ms),
            },
        }
    }
}

impl Default for PaneBarTiming {
    fn default() -> Self {
        Self::from_config(&PanesConfig::default())
    }
}

/// What the window sampled for one drawn bar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BarInput {
    pub(super) pane: PaneId,
    /// The pointer is in the bar's band, and nothing else claims it.
    pub(super) pointed: bool,
    /// A pane drag or the bar's menu keeps it revealed.
    pub(super) held: bool,
}

struct Bar {
    reveal: Reveal,
    /// When the pointer entered the band, while it stays there.
    entered: Option<Instant>,
    held: bool,
}

impl Bar {
    fn new(timing: PaneBarTiming) -> Self {
        Self {
            reveal: Reveal::with_timing(timing.reveal),
            entered: None,
            held: false,
        }
    }

    /// Whether the bar counts as hovered at `now`: held, or pointed at for
    /// the hover delay. A bar already showing, as after a keyboard reveal,
    /// takes the pointer at once.
    fn hovered(&self, now: Instant, delay: Duration) -> bool {
        self.held
            || self.entered.is_some_and(|at| {
                now >= at + delay || self.reveal.progress > 0.0
            })
    }

    /// When the pointer's rest in the band turns into hover, while it has
    /// not yet.
    fn intent_deadline(
        &self,
        now: Instant,
        delay: Duration,
    ) -> Option<Instant> {
        self.entered
            .map(|at| at + delay)
            .filter(|_| !self.hovered(now, delay))
    }
}

/// Every drawn bar's reveal in one window.
#[derive(Default)]
pub(super) struct PaneBarReveals {
    bars: HashMap<PaneId, Bar>,
    timing: PaneBarTiming,
}

impl PaneBarReveals {
    pub(super) fn with_timing(timing: PaneBarTiming) -> Self {
        Self {
            bars: HashMap::new(),
            timing,
        }
    }

    /// Applies new timings to every bar, as a config reload does.
    pub(super) fn set_timing(&mut self, timing: PaneBarTiming) {
        self.timing = timing;
        for bar in self.bars.values_mut() {
            bar.reveal.set_timing(timing.reveal);
        }
    }

    /// Applies the sampled `inputs`, which name every bar the window last
    /// drew. A bar it no longer draws is dropped, unless a keyboard reveal
    /// still holds it: a focus move out of a zoomed pane reveals a bar the
    /// next frame draws first. Disabled, every bar hides at once. Returns
    /// whether a bar's reveal input changed.
    pub(super) fn update(
        &mut self,
        now: Instant,
        inputs: &[BarInput],
        enabled: bool,
    ) -> bool {
        let before = self.bars.len();
        self.bars.retain(|pane, bar| {
            inputs.iter().any(|input| input.pane == *pane)
                || (enabled && bar.reveal.activity_pending(now))
        });
        let mut changed = before != self.bars.len();
        let timing = self.timing;
        for input in inputs {
            let bar = self
                .bars
                .entry(input.pane)
                .or_insert_with(|| Bar::new(timing));
            bar.held = input.held;
            if input.pointed {
                bar.entered.get_or_insert(now);
            } else {
                bar.entered = None;
            }
            let hovered = bar.hovered(now, timing.hover_delay);
            changed |= bar.reveal.set_input(now, hovered, enabled);
        }
        changed
    }

    /// Reveals `pane`'s bar for a moment, as a keyboard focus change does,
    /// enabling a bar no pointer refresh has seen yet.
    pub(super) fn reveal_for_activity(&mut self, pane: PaneId, now: Instant) {
        let timing = self.timing;
        let bar = self.bars.entry(pane).or_insert_with(|| Bar::new(timing));
        let hovered = bar.hovered(now, timing.hover_delay);
        bar.reveal.set_input(now, hovered, true);
        bar.reveal.reveal_for_activity(now);
    }

    /// How far `pane`'s bar has faded in, from 0 to 1.
    pub(super) fn progress(&self, pane: PaneId) -> f32 {
        self.bars.get(&pane).map_or(0.0, |bar| bar.reveal.progress)
    }

    /// The frames and deadlines the fades and hover intents need.
    pub(super) fn schedule(&self, now: Instant) -> AnimationSchedule {
        self.bars
            .values()
            .fold(AnimationSchedule::IDLE, |next, bar| {
                let next = next.merge(bar.reveal.schedule(now));
                match bar.intent_deadline(now, self.timing.hover_delay) {
                    Some(at) => next.merge(AnimationSchedule::at(at)),
                    None => next,
                }
            })
    }

    /// Advances every fade to `now`, turning a hover intent that has
    /// elapsed into hover. Returns whether any bar's reveal moved.
    pub(super) fn tick(&mut self, now: Instant, enabled: bool) -> bool {
        let mut moved = false;
        let delay = self.timing.hover_delay;
        for bar in self.bars.values_mut() {
            let hovered = bar.hovered(now, delay);
            bar.reveal.set_input(now, hovered, enabled);
            moved |= bar.reveal.tick(now);
        }
        moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use huterm_protocol::RuntimeId;

    /// Round timings the cases below count in.
    const TIMING: PaneBarTiming = PaneBarTiming {
        hover_delay: Duration::from_millis(200),
        reveal: RevealTiming {
            fade: Duration::from_millis(150),
            leave_hold: Duration::from_millis(300),
        },
    };

    fn pane(value: u64) -> PaneId {
        PaneId::in_runtime(RuntimeId::new(1), value)
    }

    fn input(value: u64, pointed: bool, held: bool) -> BarInput {
        BarInput {
            pane: pane(value),
            pointed,
            held,
        }
    }

    /// Runs the fades from `from` to `to` in 10 ms frames.
    fn run(bars: &mut PaneBarReveals, from: Instant, to: Instant) {
        let mut now = from;
        while now < to {
            now += Duration::from_millis(10);
            bars.tick(now, true);
        }
    }

    #[test]
    fn a_bar_reveals_only_after_the_pointer_rests_for_the_intent() {
        let now = Instant::now();
        let mut bars = PaneBarReveals::with_timing(TIMING);
        bars.update(
            now,
            &[input(1, true, false), input(2, false, false)],
            true,
        );
        // Passing through within the intent reveals nothing.
        run(&mut bars, now, now + Duration::from_millis(150));
        assert!(bars.progress(pane(1)).abs() < f32::EPSILON);
        assert_eq!(
            bars.schedule(now).deadline,
            Some(now + TIMING.hover_delay),
            "the intent wakes the window"
        );
        let left = now + Duration::from_millis(150);
        bars.update(
            left,
            &[input(1, false, false), input(2, false, false)],
            true,
        );
        run(&mut bars, left, left + Duration::from_millis(400));
        assert!(bars.progress(pane(1)).abs() < f32::EPSILON);
        // Resting past it fades the bar in; its neighbour stays hidden.
        let back = left + Duration::from_millis(400);
        bars.update(
            back,
            &[input(1, true, false), input(2, false, false)],
            true,
        );
        run(
            &mut bars,
            back,
            back + TIMING.hover_delay + Duration::from_millis(200),
        );
        assert!((bars.progress(pane(1)) - 1.0).abs() < f32::EPSILON);
        assert!(bars.progress(pane(2)).abs() < f32::EPSILON);
    }

    #[test]
    fn a_held_bar_stays_and_a_released_one_fades_after_the_leave_hold() {
        let now = Instant::now();
        let mut bars = PaneBarReveals::with_timing(TIMING);
        bars.update(now, &[input(1, true, false)], true);
        let shown = now + TIMING.hover_delay + Duration::from_millis(200);
        run(&mut bars, now, shown);
        // The pointer leaves for the bar's menu, which holds it.
        bars.update(shown, &[input(1, false, true)], true);
        run(&mut bars, shown, shown + Duration::from_secs(2));
        assert!((bars.progress(pane(1)) - 1.0).abs() < f32::EPSILON);
        let closed = shown + Duration::from_secs(2);
        bars.update(closed, &[input(1, false, false)], true);
        run(&mut bars, closed, closed + Duration::from_millis(250));
        assert!(
            (bars.progress(pane(1)) - 1.0).abs() < f32::EPSILON,
            "the leave hold keeps it a moment"
        );
        run(
            &mut bars,
            closed + Duration::from_millis(250),
            closed + Duration::from_secs(1),
        );
        assert!(bars.progress(pane(1)).abs() < f32::EPSILON);
    }

    #[test]
    fn a_keyboard_reveal_shows_a_bar_briefly_and_takes_the_pointer_at_once() {
        let now = Instant::now();
        let mut bars = PaneBarReveals::with_timing(TIMING);
        bars.update(now, &[input(1, false, false)], true);
        bars.reveal_for_activity(pane(1), now);
        run(&mut bars, now, now + Duration::from_millis(200));
        assert!((bars.progress(pane(1)) - 1.0).abs() < f32::EPSILON);
        // The pointer reaching a showing bar keeps it without waiting.
        let at = now + Duration::from_millis(500);
        bars.update(at, &[input(1, true, false)], true);
        run(&mut bars, at, now + Duration::from_secs(3));
        assert!((bars.progress(pane(1)) - 1.0).abs() < f32::EPSILON);
        // Without the pointer it fades after its second.
        let mut quiet = PaneBarReveals::with_timing(TIMING);
        quiet.update(now, &[input(1, false, false)], true);
        quiet.reveal_for_activity(pane(1), now);
        run(&mut quiet, now, now + Duration::from_secs(2));
        assert!(quiet.progress(pane(1)).abs() < f32::EPSILON);
    }

    #[test]
    fn a_keyboard_reveal_animates_a_bar_no_refresh_has_seen() {
        let now = Instant::now();
        let mut bars = PaneBarReveals::with_timing(TIMING);
        bars.reveal_for_activity(pane(3), now);
        assert!(bars.schedule(now).frame);
        run(&mut bars, now, now + Duration::from_millis(200));
        assert!((bars.progress(pane(3)) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_keyboard_reveal_survives_a_refresh_from_the_frame_before_its_bar() {
        let now = Instant::now();
        let mut bars = PaneBarReveals::with_timing(TIMING);
        // A zoomed frame drew only pane 1; a focus move reveals pane 2.
        bars.update(now, &[input(1, false, false)], true);
        bars.reveal_for_activity(pane(2), now);
        bars.update(now, &[input(1, false, false)], true);
        run(&mut bars, now, now + Duration::from_millis(200));
        assert!((bars.progress(pane(2)) - 1.0).abs() < f32::EPSILON);
        // Once its second passes, an undrawn bar goes.
        let later = now + Duration::from_secs(3);
        bars.update(later, &[input(1, false, false)], true);
        assert!(bars.progress(pane(2)).abs() < f32::EPSILON);
    }

    #[test]
    fn disabling_hides_every_bar_and_undrawn_bars_are_dropped() {
        let now = Instant::now();
        let mut bars = PaneBarReveals::with_timing(TIMING);
        bars.update(now, &[input(1, false, true), input(2, false, true)], true);
        run(&mut bars, now, now + Duration::from_millis(200));
        assert!((bars.progress(pane(2)) - 1.0).abs() < f32::EPSILON);
        let later = now + Duration::from_millis(200);
        assert!(bars.update(later, &[input(1, false, true)], true));
        assert!(bars.progress(pane(2)).abs() < f32::EPSILON);
        bars.update(later, &[input(1, false, true)], false);
        assert!(bars.progress(pane(1)).abs() < f32::EPSILON);
        assert_eq!(bars.schedule(later).deadline, None);
    }
    #[test]
    fn configured_timings_set_the_delay_fade_and_hide_delay() {
        let timing = PaneBarTiming::from_config(&PanesConfig {
            title_bar_hover_delay_ms: 75,
            title_bar_fade_ms: 100,
            title_bar_hide_delay_ms: 400,
            ..PanesConfig::default()
        });
        let now = Instant::now();
        let mut bars = PaneBarReveals::with_timing(timing);
        bars.update(now, &[input(1, true, false)], true);
        assert_eq!(
            bars.schedule(now).deadline,
            Some(now + Duration::from_millis(75))
        );
        run(&mut bars, now, now + Duration::from_millis(70));
        assert!(bars.progress(pane(1)).abs() < f32::EPSILON);
        // Half the 100 ms fade after the 75 ms delay.
        run(
            &mut bars,
            now + Duration::from_millis(70),
            now + Duration::from_millis(130),
        );
        let half = bars.progress(pane(1));
        assert!((0.3..0.7).contains(&half), "{half}");
        let shown = now + Duration::from_millis(200);
        run(&mut bars, now + Duration::from_millis(130), shown);
        assert!((bars.progress(pane(1)) - 1.0).abs() < f32::EPSILON);
        bars.update(shown, &[input(1, false, false)], true);
        run(&mut bars, shown, shown + Duration::from_millis(390));
        assert!(
            (bars.progress(pane(1)) - 1.0).abs() < f32::EPSILON,
            "the hide delay holds it"
        );
        run(
            &mut bars,
            shown + Duration::from_millis(390),
            shown + Duration::from_millis(510),
        );
        assert!(bars.progress(pane(1)).abs() < f32::EPSILON);
    }

    #[test]
    fn a_zero_fade_shows_and_hides_at_once_and_a_reload_retimes_bars() {
        let now = Instant::now();
        let mut bars = PaneBarReveals::with_timing(TIMING);
        bars.update(now, &[input(1, true, false)], true);
        bars.set_timing(PaneBarTiming {
            hover_delay: Duration::ZERO,
            reveal: RevealTiming {
                fade: Duration::ZERO,
                leave_hold: Duration::ZERO,
            },
        });
        let at = now + Duration::from_millis(1);
        bars.tick(at, true);
        assert!((bars.progress(pane(1)) - 1.0).abs() < f32::EPSILON);
        bars.update(at, &[input(1, false, false)], true);
        bars.tick(at + Duration::from_millis(1), true);
        assert!(bars.progress(pane(1)).abs() < f32::EPSILON);
        assert!(!bars.schedule(at + Duration::from_millis(1)).frame);
    }
}
