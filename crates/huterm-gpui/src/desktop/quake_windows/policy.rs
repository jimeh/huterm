//! Deterministic Quake transitions over authoritative sampled native facts.
use super::{Display, Profile, ProfileExt, Rect, Transition};
use std::time::{Duration, Instant};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::desktop) enum Stage {
    Prepare,
    Activate,
    Windowed,
    Animate,
    SettleVisible,
    SettleHidden,
    Idle,
}

#[derive(Default)]
pub(in crate::desktop) struct Activation {
    pub(in crate::desktop) seen: bool,
    pub(in crate::desktop) requested: bool,
    pub(in crate::desktop) next_retry: Option<Instant>,
}
pub(in crate::desktop) const ACTIVATION_RETRY_INTERVAL: Duration =
    Duration::from_millis(100);

impl Activation {
    pub(in crate::desktop) fn request(&mut self) {
        self.seen = false;
        self.requested = true;
        self.next_retry = None;
    }

    pub(in crate::desktop) fn cancel(&mut self) {
        self.requested = false;
        self.next_retry = None;
    }

    /// Record an activation sample. `settled` means no native fullscreen
    /// transition is unresolved and the observed fullscreen state matches the
    /// target. Samples taken mid-transition are ignored: a window that is
    /// still active while a Space exit begins must not cancel its Show, and
    /// `AppKit` can revert an activation granted before the Space switch ends.
    /// Once settled, a later app switch never re-arms retries.
    pub(in crate::desktop) fn observe(&mut self, active: bool, settled: bool) {
        if !settled {
            return;
        }
        if active {
            self.seen = true;
            self.cancel();
        } else if self.seen {
            self.cancel();
        }
    }

    pub(in crate::desktop) fn take_request(&mut self, now: Instant) -> bool {
        if !std::mem::take(&mut self.requested) {
            return false;
        }
        self.next_retry = Some(now + ACTIVATION_RETRY_INTERVAL);
        true
    }

    pub(in crate::desktop) fn take_retry(
        &mut self,
        now: Instant,
        deadline: Instant,
    ) -> bool {
        if self.seen || now >= deadline {
            return false;
        }
        let Some(next_retry) = self.next_retry else {
            return false;
        };
        if now < next_retry {
            return false;
        }
        self.next_retry = Some(now + ACTIVATION_RETRY_INTERVAL);
        true
    }

    pub(in crate::desktop) fn refit(
        &mut self,
        stage: Stage,
        deadline: Instant,
        now: Instant,
    ) -> Instant {
        if stage == Stage::Idle {
            self.cancel();
            now + Duration::from_secs(3)
        } else {
            deadline
        }
    }
}

pub(in crate::desktop) const REAPPLY_INTERVAL: Duration =
    Duration::from_millis(16);
#[derive(Clone, Copy)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "authoritative independent native and close-state facts"
)]
pub(in crate::desktop) struct Facts {
    pub active: bool,
    pub blurred: bool,
    pub visible: bool,
    pub fullscreen: bool,
    pub native_idle: bool,
    pub frame: Rect,
    pub confirming: bool,
    pub busy: bool,
}
#[derive(Debug, PartialEq)]
pub(in crate::desktop) enum Action {
    Frame(Rect),
    Quake(bool),
    Fullscreen(bool),
    Opacity(f64),
    Show,
    Hide(bool),
}
pub(in crate::desktop) struct Sample {
    pub progress: f64,
    pub frame: Rect,
    pub opacity: f64,
    pub gap: Duration,
}
#[derive(Default)]
pub(in crate::desktop) struct Decision {
    pub actions: Vec<Action>,
    pub sample: Option<Sample>,
    pub settled: bool,
    pub continuation: bool,
}
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent native presentation policies"
)]
pub(in crate::desktop) struct Model {
    pub profile: Profile,
    pub display: Display,
    pub regular: bool,
    pub regular_bounds: Rect,
    pub target: Rect,
    pub transition: Transition,
    pub stage: Stage,
    pub deadline: Instant,
    pub activation: Activation,
    pub suppress_blur: Instant,
    pub restore_focus: bool,
    pub fade_supported: bool,
    pub recovering: bool,
    pub detach_requested: bool,
    pub observed_fullscreen: bool,
    pub revision: u64,
    pub next_reapply: Instant,
    pub animation_started: bool,
    pub native_paused: bool,
}
impl Model {
    pub fn duration(&self) -> Duration {
        if self.regular
            || self.profile.animation == crate::quake::Animation::None
        {
            Duration::ZERO
        } else {
            Duration::from_millis(u64::from(self.profile.animation_ms))
        }
    }
    pub fn request(
        &mut self,
        visible: bool,
        restore_focus: bool,
        now: Instant,
    ) {
        self.deadline = now + Duration::from_secs(3);
        if visible {
            self.activation.request();
        } else {
            self.activation.cancel();
        }
        self.prepare(visible, restore_focus, now);
    }
    fn prepare(&mut self, visible: bool, restore_focus: bool, now: Instant) {
        self.revision = self.revision.wrapping_add(1);
        self.recovering = false;
        if self.native_paused {
            self.transition.pause(now);
        }
        self.transition.retarget(visible, now, self.duration());
        self.stage = Stage::Prepare;
        self.animation_started = false;
        self.next_reapply = now;
        if visible {
            self.suppress_blur = now + Duration::from_millis(200);
        }
        self.restore_focus = restore_focus;
    }
    pub fn refit(&mut self, now: Instant, preserve_presentation: bool) {
        self.deadline = self.activation.refit(self.stage, self.deadline, now);
        if preserve_presentation
            && self.stage == Stage::Idle
            && self.transition.visible()
        {
            // A settled window only needs new bounds. Releasing fullscreen
            // here changes the Dock/menu work area and delays its content resize.
            self.revision = self.revision.wrapping_add(1);
            self.stage = Stage::SettleVisible;
            self.next_reapply = now;
            self.suppress_blur = now + Duration::from_millis(200);
            return;
        }
        self.prepare(self.transition.visible(), false, now);
    }
    pub fn animation_end(&self) -> Instant {
        self.transition.end_at(self.duration())
    }
    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        let mut next = (self.stage != Stage::Idle).then_some(self.deadline);
        let mut include = |at: Instant| {
            next = Some(next.map_or(at, |next| next.min(at)));
        };
        if self.profile.hide_on_focus_loss
            && self.transition.visible()
            && now < self.suppress_blur
        {
            include(self.suppress_blur);
        }
        if self.stage == Stage::Animate && !self.native_paused {
            include(self.animation_end());
        }
        if self.stage == Stage::SettleVisible && !self.native_paused {
            include(self.next_reapply);
        }
        if let Some(at) = self.activation.next_retry
            && self.stage == Stage::SettleVisible
            && !self.native_paused
        {
            include(at);
        }
        next
    }
    #[expect(
        clippy::too_many_lines,
        reason = "exhaustive transition policy preserves native operation ordering"
    )]
    pub fn decide(
        &mut self,
        facts: Facts,
        now: Instant,
        frame: bool,
    ) -> Result<Decision, String> {
        if self.native_paused && facts.native_idle {
            self.transition.pause(now);
        }
        self.native_paused = !facts.native_idle;
        self.observed_fullscreen = facts.fullscreen;
        self.activation.observe(
            facts.active,
            facts.native_idle
                && facts.fullscreen
                    == (self.profile.fullscreen && !self.regular),
        );
        if facts.confirming && !self.transition.visible() {
            self.request(true, false, now);
        }
        if !self.regular
            && self.profile.hide_on_focus_loss
            && self.transition.visible()
            && self.activation.seen
            && facts.blurred
            && now >= self.suppress_blur
            && !facts.confirming
            && !facts.busy
        {
            self.request(false, false, now);
        }
        if self.stage != Stage::Idle && now >= self.deadline {
            return Err(format!(
                "native quake transition {:?} timed out (frame {:?}, target {:?}, fullscreen {}, active {})",
                self.stage,
                facts.frame,
                self.target,
                facts.fullscreen,
                facts.active
            ));
        }
        if self.stage != Stage::Idle && !facts.native_idle {
            self.transition.pause(now);
            return Ok(Decision::default());
        }
        let mut result = Decision::default();
        match self.stage {
            Stage::Prepare => {
                self.stage = Stage::Windowed;
                self.suppress_blur = now + Duration::from_millis(200);
                self.transition.pause(now);
                result.actions.push(Action::Fullscreen(false));
            }
            Stage::Windowed if !facts.fullscreen => {
                self.transition.pause(now);
                self.stage = Stage::Animate;
                self.animation_started = false;
                result.actions.push(Action::Quake(!self.regular));
                if self.regular {
                    result.actions.push(Action::Frame(self.regular_bounds));
                }
            }
            Stage::Animate => {
                if self.animation_started
                    && !frame
                    && now < self.animation_end()
                    && !self.duration().is_zero()
                {
                    return Ok(result);
                }
                self.animation_started = true;
                let gap = self.transition.elapsed_since_sample(now);
                let progress = self.transition.sample(now, self.duration());
                let finished = self.transition.finished();
                let showing = self.transition.visible();
                let (fade, edge) = self.profile.effects();
                let geometry = crate::quake::animation_frame(
                    self.target,
                    self.display.frame,
                    edge,
                    progress,
                );
                let opacity = if fade && self.fade_supported && !self.regular {
                    progress
                } else {
                    1.0
                };
                let focus = self.activation.take_request(now);
                if !self.regular {
                    result.actions.push(Action::Frame(geometry));
                }
                result.actions.push(Action::Opacity(opacity));
                if showing && (focus || !facts.visible) {
                    result.actions.push(Action::Show);
                }
                if finished {
                    self.next_reapply = now + REAPPLY_INTERVAL;
                    self.stage = if showing {
                        Stage::SettleVisible
                    } else {
                        Stage::SettleHidden
                    };
                    result.actions.push(if showing {
                        Action::Fullscreen(
                            self.profile.fullscreen && !self.regular,
                        )
                    } else {
                        Action::Hide(self.restore_focus && facts.active)
                    });
                    result.actions.push(Action::Opacity(1.0));
                }
                result.sample = Some(Sample {
                    progress,
                    frame: geometry,
                    opacity,
                    gap,
                });
            }
            Stage::Activate => {
                self.stage = Stage::SettleVisible;
                self.next_reapply = now + REAPPLY_INTERVAL;
                if self.activation.take_request(now) {
                    result.actions.push(Action::Show);
                }
            }
            Stage::SettleVisible => {
                let expected = self.profile.fullscreen && !self.regular;
                if facts.visible
                    && self.activation.seen
                    && facts.fullscreen == expected
                    && (self.regular || super::near(facts.frame, self.target))
                {
                    self.stage = Stage::Idle;
                    self.recovering = false;
                    result.settled = true;
                } else {
                    if now >= self.next_reapply {
                        self.next_reapply = now + REAPPLY_INTERVAL;
                        if !self.regular
                            && (!expected || cfg!(target_os = "macos"))
                            && facts.visible
                        {
                            result.actions.push(Action::Frame(self.target));
                        }
                    }
                    if self.activation.take_retry(now, self.deadline) {
                        result.actions.push(Action::Show);
                    }
                }
            }
            Stage::SettleHidden if !facts.visible => {
                self.stage = Stage::Idle;
                result.settled = true;
            }
            Stage::Idle
                if self.regular && !facts.fullscreen && facts.visible =>
            {
                self.regular_bounds = facts.frame;
            }
            Stage::Windowed | Stage::SettleHidden | Stage::Idle => {}
        }
        result.continuation = self.detach_requested
            && self.stage == Stage::Idle
            && !self.recovering;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn visible(now: Instant) -> (Model, Facts) {
        let target = Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 400.0,
        };
        let model = Model {
            profile: Profile {
                hide_on_focus_loss: false,
                ..Profile::default()
            },
            display: Display {
                id: "test".into(),
                frame: target,
                work: target,
                primary: true,
            },
            regular: false,
            regular_bounds: target,
            target,
            transition: Transition::new(true, now),
            stage: Stage::Idle,
            deadline: now + Duration::from_secs(3),
            activation: Activation {
                seen: true,
                ..Activation::default()
            },
            suppress_blur: now,
            restore_focus: false,
            fade_supported: true,
            recovering: false,
            detach_requested: false,
            observed_fullscreen: false,
            revision: 0,
            next_reapply: now,
            animation_started: false,
            native_paused: false,
        };
        let facts = Facts {
            active: true,
            blurred: false,
            visible: true,
            fullscreen: false,
            native_idle: true,
            frame: target,
            confirming: false,
            busy: false,
        };
        (model, facts)
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn settled_refit_preserves_presentation_and_resizes_without_a_frame() {
        for fullscreen in [false, true] {
            let now = Instant::now();
            let (mut model, mut facts) = visible(now);
            model.profile.fullscreen = fullscreen;
            facts.fullscreen = fullscreen;
            facts.active = false;
            model.target.width += 200.0;
            model.refit(now, true);
            let result = model.decide(facts, now, false).unwrap();
            assert_eq!(result.actions, vec![Action::Frame(model.target)]);
            facts.frame = model.target;
            assert!(model.decide(facts, now, false).unwrap().settled);
        }
    }
    #[test]
    fn refit_without_native_permission_keeps_transition_preparation() {
        let now = Instant::now();
        let (mut model, facts) = visible(now);
        model.refit(now, false);
        assert_eq!(
            model.decide(facts, now, false).unwrap().actions,
            vec![Action::Fullscreen(false)]
        );
    }
    #[test]
    fn refit_during_activation_keeps_show_request_and_original_deadline() {
        let now = Instant::now();
        let (mut model, mut facts) = visible(now);
        model.stage = Stage::Activate;
        model.activation.request();
        facts.active = false;
        let deadline = model.deadline;
        let refit_at = now + Duration::from_secs(1);
        model.refit(refit_at, true);
        assert_eq!(model.deadline, deadline);
        assert_eq!(
            model.decide(facts, refit_at, false).unwrap().actions,
            vec![Action::Fullscreen(false)]
        );
        model.decide(facts, refit_at, false).unwrap();
        let result = model.decide(facts, refit_at, true).unwrap();
        assert!(result.actions.contains(&Action::Show));
    }
    #[test]
    fn reversal_requested_during_native_pause_preserves_progress() {
        let now = Instant::now();
        let (mut model, mut facts) = visible(now);
        model.request(false, false, now);
        model.decide(facts, now, false).unwrap();
        model.decide(facts, now, false).unwrap();
        model.decide(facts, now, false).unwrap();
        let sampled = now + Duration::from_millis(30);
        let progress = model
            .decide(facts, sampled, true)
            .unwrap()
            .sample
            .unwrap()
            .progress;
        facts.native_idle = false;
        model.decide(facts, sampled, false).unwrap();
        let requested = sampled + Duration::from_secs(1);
        model.request(true, false, requested);
        assert_eq!(model.deadline, requested + Duration::from_secs(3));
        facts.native_idle = true;
        let resumed = requested + Duration::from_millis(100);
        model.decide(facts, resumed, false).unwrap();
        model.decide(facts, resumed, false).unwrap();
        let result = model.decide(facts, resumed, true).unwrap();
        assert!(
            (result.sample.unwrap().progress - progress).abs() < f64::EPSILON,
            "reversal sampled paused wall time"
        );
        assert_eq!(model.deadline, requested + Duration::from_secs(3));
    }
    #[test]
    fn quiet_settlement_requests_detach_continuation_without_native_effect() {
        let now = Instant::now();
        let (mut model, facts) = visible(now);
        model.regular = true;
        model.detach_requested = true;
        model.stage = Stage::SettleVisible;
        let result = model.decide(facts, now, false).unwrap();
        assert!(result.actions.is_empty());
        assert_eq!(model.next_deadline(now), None);
        assert!(
            result.continuation,
            "quiet settlement must schedule the detach pass"
        );
    }
    #[test]
    fn native_pause_resume_preserves_progress_and_failure_deadline() {
        let now = Instant::now();
        let (mut model, mut facts) = visible(now);
        model.request(false, false, now);
        model.decide(facts, now, false).unwrap();
        model.decide(facts, now, false).unwrap();
        model.decide(facts, now, false).unwrap();
        let sampled = now + Duration::from_millis(30);
        let progress = model
            .decide(facts, sampled, true)
            .unwrap()
            .sample
            .unwrap()
            .progress;
        let deadline = model.deadline;
        facts.native_idle = false;
        model.decide(facts, sampled, false).unwrap();
        facts.native_idle = true;
        let resumed = model
            .decide(facts, sampled + Duration::from_secs(1), true)
            .unwrap()
            .sample
            .unwrap()
            .progress;
        assert!(
            (resumed - progress).abs() < f64::EPSILON,
            "native pause advanced animation: {progress} -> {resumed}"
        );
        assert_eq!(model.deadline, deadline);
    }
    #[test]
    fn settled_visible_and_hidden_have_no_policy_deadline() {
        let now = Instant::now();
        let (mut model, mut facts) = visible(now);
        assert!(model.decide(facts, now, false).unwrap().actions.is_empty());
        assert_eq!(model.next_deadline(now), None);
        model.transition = Transition::new(false, now);
        facts.visible = false;
        assert!(model.decide(facts, now, false).unwrap().actions.is_empty());
        assert_eq!(model.next_deadline(now), None);
    }
    #[test]
    fn geometry_events_cannot_bypass_retry_floor_or_extend_failure_deadline() {
        let now = Instant::now();
        let (mut model, mut facts) = visible(now);
        model.stage = Stage::SettleVisible;
        model.next_reapply = now + REAPPLY_INTERVAL;
        facts.frame.x = 50.0;
        let failure = model.deadline;
        for millis in 0..16 {
            assert!(
                model
                    .decide(facts, now + Duration::from_millis(millis), false)
                    .unwrap()
                    .actions
                    .is_empty()
            );
        }
        assert_eq!(
            model
                .decide(facts, now + REAPPLY_INTERVAL, false)
                .unwrap()
                .actions,
            vec![Action::Frame(model.target)]
        );
        assert_eq!(model.deadline, failure);
        assert!(model.decide(facts, failure, false).is_err());
    }
    #[test]
    fn activation_deadline_advances_independently_of_geometry_retry() {
        let now = Instant::now();
        let (mut model, mut facts) = visible(now);
        model.stage = Stage::SettleVisible;
        model.activation.seen = false;
        model.activation.next_retry = Some(now + Duration::from_millis(100));
        model.next_reapply = now + Duration::from_millis(112);
        facts.active = false;
        let retry = now + Duration::from_millis(100);
        assert_eq!(model.next_deadline(now), Some(retry));
        assert_eq!(
            model.decide(facts, retry, false).unwrap().actions,
            vec![Action::Show]
        );
        assert_eq!(model.next_deadline(retry), Some(model.next_reapply));
        assert!(model.next_deadline(retry).unwrap() > retry);
    }
    #[test]
    fn notifications_do_not_sample_animation_but_endpoint_does_not_require_frames()
     {
        let now = Instant::now();
        let (mut model, facts) = visible(now);
        model.request(false, false, now);
        model.decide(facts, now, false).unwrap();
        model.decide(facts, now, false).unwrap();
        assert!(model.decide(facts, now, false).unwrap().sample.is_some());
        let later = now + Duration::from_millis(50);
        assert!(model.decide(facts, later, false).unwrap().sample.is_none());
        assert!(model.decide(facts, later, true).unwrap().sample.is_some());
        let result = model.decide(facts, model.animation_end(), false).unwrap();
        assert!(result.actions.contains(&Action::Hide(false)));
        assert_eq!(model.stage, Stage::SettleHidden);
    }
    #[test]
    fn paused_native_transition_keeps_only_finite_failure_deadline() {
        let now = Instant::now();
        let (mut model, mut facts) = visible(now);
        model.stage = Stage::Animate;
        facts.native_idle = false;
        model.decide(facts, now, true).unwrap();
        assert_eq!(model.next_deadline(now), Some(model.deadline));
    }
    #[test]
    fn blur_waits_for_suppression_and_cleared_close_blocker() {
        let now = Instant::now();
        let (mut model, mut facts) = visible(now);
        model.profile.hide_on_focus_loss = true;
        model.suppress_blur = now + Duration::from_millis(200);
        facts.active = false;
        facts.blurred = true;
        model.decide(facts, now, false).unwrap();
        assert!(model.transition.visible());
        assert_eq!(model.next_deadline(now), Some(model.suppress_blur));
        facts.busy = true;
        model.decide(facts, model.suppress_blur, false).unwrap();
        assert!(model.transition.visible());
        facts.busy = false;
        model.decide(facts, model.suppress_blur, false).unwrap();
        assert!(!model.transition.visible());
    }
    #[test]
    fn nonactivating_panel_does_not_hide_and_reused_earlier_wake_does_not_finish_reversal()
     {
        let now = Instant::now();
        let (mut model, mut facts) = visible(now);
        model.profile.hide_on_focus_loss = true;
        facts.active = false;
        model.decide(facts, now, false).unwrap();
        assert!(model.transition.visible());
        model.request(false, false, now);
        model.decide(facts, now, false).unwrap();
        model.decide(facts, now, false).unwrap();
        model
            .decide(facts, now + Duration::from_millis(100), true)
            .unwrap();
        let old_end = model.animation_end();
        model.request(true, false, now + Duration::from_millis(100));
        model
            .decide(facts, now + Duration::from_millis(100), false)
            .unwrap();
        model
            .decide(facts, now + Duration::from_millis(100), false)
            .unwrap();
        model
            .decide(facts, now + Duration::from_millis(100), false)
            .unwrap();
        assert!(model.animation_end() > old_end);
        assert!(
            model
                .decide(facts, old_end, false)
                .unwrap()
                .sample
                .is_none()
        );
        assert_eq!(model.stage, Stage::Animate);
    }
}
