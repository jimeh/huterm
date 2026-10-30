//! Hover reveal for pane title bars under `[panes] title_bar = "hover"`:
//! each pane's bar fades in once the pointer has rested in its band for the
//! hover intent, stays while the pointer, a pane drag, or the bar's menu
//! holds it, and fades out after the tab bar's leave hold. A keyboard focus
//! change reveals the focused pane's bar for a moment. Everything here is
//! pure, driven by the window with the inputs it samples.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use huterm_protocol::PaneId;

use super::tab_visibility::Reveal;
use crate::ui::animation::AnimationSchedule;

/// How long the pointer rests in a bar's band before the bar reveals, as
/// long as the scrollbar waits, so a pointer heading for a divider or
/// passing through reveals nothing.
pub(super) const HOVER_INTENT: Duration = Duration::from_millis(200);

/// What the window sampled for one drawn bar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BarInput {
    pub(super) pane: PaneId,
    /// The pointer is in the bar's band, and nothing else claims it.
    pub(super) pointed: bool,
    /// A pane drag or the bar's menu keeps it revealed.
    pub(super) held: bool,
}

#[derive(Default)]
struct Bar {
    reveal: Reveal,
    /// When the pointer entered the band, while it stays there.
    entered: Option<Instant>,
    held: bool,
}

impl Bar {
    /// Whether the bar counts as hovered at `now`: held, or pointed at for
    /// the hover intent. A bar already showing, as after a keyboard
    /// reveal, takes the pointer at once.
    fn hovered(&self, now: Instant) -> bool {
        self.held
            || self.entered.is_some_and(|at| {
                now >= at + HOVER_INTENT || self.reveal.progress > 0.0
            })
    }

    /// When the pointer's rest in the band turns into hover, while it has
    /// not yet.
    fn intent_deadline(&self, now: Instant) -> Option<Instant> {
        self.entered
            .map(|at| at + HOVER_INTENT)
            .filter(|_| !self.hovered(now))
    }
}

/// Every drawn bar's reveal in one window.
#[derive(Default)]
pub(super) struct PaneBarReveals {
    bars: HashMap<PaneId, Bar>,
}

impl PaneBarReveals {
    /// Applies the sampled `inputs`, which name every bar the window draws;
    /// bars it no longer draws are dropped. Disabled, every bar hides at
    /// once. Returns whether a bar's reveal input changed.
    pub(super) fn update(
        &mut self,
        now: Instant,
        inputs: &[BarInput],
        enabled: bool,
    ) -> bool {
        let before = self.bars.len();
        self.bars
            .retain(|pane, _| inputs.iter().any(|input| input.pane == *pane));
        let mut changed = before != self.bars.len();
        for input in inputs {
            let bar = self.bars.entry(input.pane).or_default();
            bar.held = input.held;
            if input.pointed {
                bar.entered.get_or_insert(now);
            } else {
                bar.entered = None;
            }
            let hovered = bar.hovered(now);
            changed |= bar.reveal.set_input(now, hovered, enabled);
        }
        changed
    }

    /// Reveals `pane`'s bar for a moment, as a keyboard focus change does,
    /// enabling a bar no pointer refresh has seen yet.
    pub(super) fn reveal_for_activity(&mut self, pane: PaneId, now: Instant) {
        let bar = self.bars.entry(pane).or_default();
        let hovered = bar.hovered(now);
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
                match bar.intent_deadline(now) {
                    Some(at) => next.merge(AnimationSchedule::at(at)),
                    None => next,
                }
            })
    }

    /// Advances every fade to `now`, turning a hover intent that has
    /// elapsed into hover. Returns whether any bar's reveal moved.
    pub(super) fn tick(&mut self, now: Instant, enabled: bool) -> bool {
        let mut moved = false;
        for bar in self.bars.values_mut() {
            let hovered = bar.hovered(now);
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
        let mut bars = PaneBarReveals::default();
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
            Some(now + HOVER_INTENT),
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
            back + HOVER_INTENT + Duration::from_millis(200),
        );
        assert!((bars.progress(pane(1)) - 1.0).abs() < f32::EPSILON);
        assert!(bars.progress(pane(2)).abs() < f32::EPSILON);
    }

    #[test]
    fn a_held_bar_stays_and_a_released_one_fades_after_the_leave_hold() {
        let now = Instant::now();
        let mut bars = PaneBarReveals::default();
        bars.update(now, &[input(1, true, false)], true);
        let shown = now + HOVER_INTENT + Duration::from_millis(200);
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
        let mut bars = PaneBarReveals::default();
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
        let mut quiet = PaneBarReveals::default();
        quiet.update(now, &[input(1, false, false)], true);
        quiet.reveal_for_activity(pane(1), now);
        run(&mut quiet, now, now + Duration::from_secs(2));
        assert!(quiet.progress(pane(1)).abs() < f32::EPSILON);
    }

    #[test]
    fn a_keyboard_reveal_animates_a_bar_no_refresh_has_seen() {
        let now = Instant::now();
        let mut bars = PaneBarReveals::default();
        bars.reveal_for_activity(pane(3), now);
        assert!(bars.schedule(now).frame);
        run(&mut bars, now, now + Duration::from_millis(200));
        assert!((bars.progress(pane(3)) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn disabling_hides_every_bar_and_undrawn_bars_are_dropped() {
        let now = Instant::now();
        let mut bars = PaneBarReveals::default();
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
}
