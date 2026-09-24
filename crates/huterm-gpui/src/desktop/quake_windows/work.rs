//! Each presentation owns its driver, native subscriptions and animation clock.
use super::*;
use crate::quake::observation::{FRAME, Signal};

#[derive(Clone, Copy)]
pub(super) enum WakeSource {
    Intent,
    View,
    Native,
    Continuation,
    Effect,
}

pub(super) struct Work {
    wake: crate::deferred_work::Wake,
    pub signal: Signal,
    pub observer: native::Observer,
    receiver: Option<async_channel::Receiver<()>>,
    task: Option<gpui::Task<()>>,
    display: Option<String>,
    admission: Admission,
    warned_clock: bool,
    area_deadline: Option<Instant>,
    pub failure: Failure,
    timers: Rc<Cell<u64>>,
    window_frames: u64,
    diagnostics: bool,
    passes: Cell<u64>,
    wakes: [Cell<u64>; 5],
}
impl Drop for Work {
    fn drop(&mut self) {
        self.signal.invalidate_frames();
        self.wake.stop();
    }
}
impl Work {
    pub fn new(
        window: &native::Window,
        platform: &native::Platform,
    ) -> anyhow::Result<Self> {
        let diagnostics = std::env::var_os("HUTERM_QUAKE_SMOKE").is_some();
        let timers = Rc::new(Cell::new(0_u64));
        let timer_count = Rc::clone(&timers);
        let callback: Option<Rc<dyn Fn()>> = diagnostics.then(|| {
            Rc::new(move || {
                timer_count.set(timer_count.get() + 1);
            }) as Rc<dyn Fn()>
        });
        let (wake, receiver) = crate::deferred_work::Wake::new(callback);
        let signal = Signal::new(wake.sender());
        let observer = window.observe(signal.clone(), platform)?;
        Ok(Self {
            wake,
            signal,
            observer,
            receiver: Some(receiver),
            task: None,
            display: None,
            admission: Admission::new(),
            warned_clock: false,
            area_deadline: None,
            failure: Failure::default(),
            timers,
            window_frames: 0,
            diagnostics,
            passes: Cell::new(0),
            wakes: std::array::from_fn(|_| Cell::new(0)),
        })
    }
    pub fn signal(&self, source: WakeSource) {
        if self.diagnostics {
            let counter = &self.wakes[source as usize];
            counter.set(counter.get() + 1);
        }
        self.wake.signal();
    }
    pub fn passed(&self) {
        if self.diagnostics {
            self.passes.set(self.passes.get() + 1);
        }
    }
    fn clock_installed(&mut self, running: anyhow::Result<bool>) {
        self.admission.clock_running =
            running.as_ref().copied().unwrap_or(false);
        if let Err(error) = running
            && !self.warned_clock
        {
            self.warned_clock = true;
            eprintln!(
                "Quake target-display clock: {error:#}; using refresh-derived active fallback"
            );
        }
    }
    pub fn invalidate(&mut self) {
        self.admission.invalidate(&self.signal);
        self.display = None;
    }
    pub fn frame_schedule(&self) -> AnimationSchedule {
        if self.admission.wants_frame {
            AnimationSchedule::FRAME
        } else {
            AnimationSchedule::IDLE
        }
    }
    pub fn window_frame(&mut self, now: Instant) {
        if self.diagnostics && self.admission.wants_frame {
            self.window_frames += 1;
        }
        self.admission.window_frame(now, &self.signal);
    }
    pub fn take_frame(&mut self, now: Instant, flags: u8) -> bool {
        self.admission.take_frame(now, flags, &self.signal)
    }
    pub fn inspect(&self) -> String {
        format!(
            "quake_timer={}\nquake_clock={}\nquake_frame_demand={}\nwork_area_fallback={}\nquake_timer_fires={}\nquake_window_frames={}\nquake_native_frames={}\nquake_fact_wakes={}\nquake_raw_native_frames={}\nquake_passes={}\nquake_intent_wakes={}\nquake_view_wakes={}\nquake_native_wakes={}\nquake_continuation_wakes={}\nquake_effect_wakes={}\nquake_pending={}",
            self.wake.armed(),
            self.admission.clock_running,
            self.admission.wants_frame,
            self.area_deadline.is_some(),
            self.timers.get(),
            self.window_frames,
            self.signal.counts().1,
            self.signal.counts().0,
            self.signal.raw_frames(),
            self.passes.get(),
            self.wakes[0].get(),
            self.wakes[1].get(),
            self.wakes[2].get(),
            self.wakes[3].get(),
            self.wakes[4].get(),
            self.wake.pending()
        )
    }
    pub fn display_due(&self, now: Instant) -> bool {
        self.area_deadline.is_some_and(|at| now >= at)
    }
    pub fn sampled(&mut self, now: Instant) {
        self.admission.last_sample = Some(now);
    }
}
#[derive(Default)]
pub(super) struct Failure {
    retry_at: Option<Instant>,
    attempts: u8,
    message: Option<String>,
}
impl Failure {
    pub fn blocked(&self, now: Instant) -> bool {
        self.retry_at.is_some_and(|at| now < at)
    }
    pub fn record(&mut self, now: Instant, message: &str) -> (bool, bool) {
        self.retry_at = Some(now + Duration::from_secs(1));
        let recover = self.attempts < 2;
        self.attempts = self.attempts.saturating_add(1);
        let report = self.message.as_deref() != Some(message);
        self.message = Some(message.to_owned());
        (recover, report)
    }
    pub fn sampled(&mut self, has_effect: bool) {
        if !has_effect {
            self.clear();
        }
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent coalesced frame admission and source-lifetime facts"
)]
struct Admission {
    period: Duration,
    window_source: bool,
    pending_window: bool,
    last_window: Option<Instant>,
    last_sample: Option<Instant>,
    wants_frame: bool,
    clock_running: bool,
}
impl Admission {
    fn new() -> Self {
        Self {
            period: Duration::from_millis(16),
            window_source: false,
            pending_window: false,
            last_window: None,
            last_sample: None,
            wants_frame: false,
            clock_running: false,
        }
    }
    fn set_active(&mut self, active: bool, now: Instant, signal: &Signal) {
        self.wants_frame = active;
        if active {
            self.last_sample.get_or_insert(now);
        } else {
            self.invalidate(signal);
            self.last_sample = None;
        }
    }
    fn invalidate(&mut self, signal: &Signal) {
        signal.invalidate_frames();
        self.pending_window = false;
        self.window_source = false;
        self.last_window = None;
        // Preserve the last sample across display changes so the active-only
        // fallback still advances while replacement clocks have no frames.
    }
    fn window_frame(&mut self, now: Instant, signal: &Signal) {
        if !self.wants_frame {
            return;
        }
        if self.window_source {
            self.pending_window = true;
        } else {
            // A source handoff never adds a second sample beside an auxiliary
            // tick. The next actual window frame admits normal animation work.
            self.window_source = true;
            signal.invalidate_frames();
        }
        self.last_window = Some(now);
        signal.notify(crate::quake::observation::CHANGED);
    }
    fn take_frame(&mut self, now: Instant, flags: u8, signal: &Signal) -> bool {
        if self.window_source
            && self
                .last_window
                .is_some_and(|last| now >= last + self.period * 2)
        {
            self.window_source = false;
            signal.invalidate_frames();
        }
        let window = std::mem::take(&mut self.pending_window);
        let native = !self.window_source && flags & FRAME != 0;
        let overdue = self.wants_frame
            && self.last_sample.is_some_and(|last| {
                now >= last
                    + self.period
                        * if self.clock_running || self.window_source {
                            2
                        } else {
                            1
                        }
            });
        window || native || overdue
    }
}
#[expect(
    clippy::fn_params_excessive_bools,
    reason = "independent platform, presentation, and observer-failure facts"
)]
fn needs_area_fallback(
    macos: bool,
    visible: bool,
    regular: bool,
    fullscreen: bool,
    observer_failed: bool,
) -> bool {
    observer_failed || (macos && visible && !regular && !fullscreen)
}
struct ClockRequest {
    observer: native::Observer,
    display: Option<Display>,
    epoch: u64,
}
fn arm(
    view: &mut WorkspaceView,
    cx: &mut Context<'_, WorkspaceView>,
) -> Option<ClockRequest> {
    let state = view.quake.as_mut()?;
    let now = Instant::now();
    let wants_frame = state.work.failure.retry_at.is_none()
        && state.stage == Stage::Animate
        && !state.native_paused;
    if wants_frame && state.work.display.as_ref() != Some(&state.display.id) {
        let period = state.work.observer.refresh_period(&state.display);
        if period.is_none() {
            eprintln!(
                "Quake: display refresh unavailable; active animation uses unsynchronized 16ms compatibility pacing"
            );
        }
        state.work.admission.period =
            period.unwrap_or(Duration::from_millis(16));
        state.work.display = Some(state.display.id.clone());
    }
    state
        .work
        .admission
        .set_active(wants_frame, now, &state.work.signal);
    // AppKit exposes no dedicated visibleFrame notification. Keep only this
    // visible, non-fullscreen work-area safety sample; hidden owners resample
    // authoritatively on every summon and still observe topology/lease changes.
    if state.work.failure.retry_at.is_none()
        && needs_area_fallback(
            cfg!(target_os = "macos"),
            state.transition.visible(),
            state.regular,
            state.profile.fullscreen,
            state.work.observer.failed(),
        )
    {
        if state.work.area_deadline.is_none_or(|at| now >= at) {
            state.work.area_deadline = Some(now + Duration::from_secs(1));
        }
    } else {
        state.work.area_deadline = None;
    }
    let mut deadline = state
        .work
        .failure
        .retry_at
        .or_else(|| state.model.next_deadline(now));
    if let Some(at) = state.work.area_deadline {
        deadline = Some(deadline.map_or(at, |deadline| deadline.min(at)));
    }
    if wants_frame {
        let at = state.work.admission.last_sample.unwrap_or(now)
            + state.work.admission.period
                * if state.work.admission.clock_running
                    || state.work.admission.window_source
                {
                    2
                } else {
                    1
                };
        deadline = Some(deadline.map_or(at, |deadline| deadline.min(at)));
    }
    state.work.wake.arm(deadline, cx);
    let request = ClockRequest {
        observer: state.work.observer.clone(),
        display: (wants_frame && !state.work.admission.window_source)
            .then(|| state.display.clone()),
        epoch: state.work.signal.epoch(),
    };
    view.frame_clock.animate(
        cx.entity().downgrade(),
        refresh::Animated::animation_schedule(view, now),
        cx,
    );
    Some(request)
}
pub(in crate::desktop) fn start(
    view: &mut WorkspaceView,
    window: &Window,
    cx: &mut Context<'_, WorkspaceView>,
) {
    let Some(state) = view.quake.as_mut() else {
        return;
    };
    let Some(receiver) = state.work.receiver.take() else {
        return;
    };
    let handle = window.window_handle();
    state.work.task = Some(cx.spawn(async move |weak, cx| {
        while receiver.recv().await.is_ok() {
            // Even synchronous native delivery crosses a foreground turn before
            // entering another reconciliation or applying completion effects.
            cx.foreground_executor().spawn(async {}).await;
            if receiver.is_closed() {
                break;
            }
            let result = handle
                .update(cx, |_, window, cx| {
                    weak.update(cx, |view, cx| {
                        let effect = step(view, window, cx);
                        arm(view, cx).map(|clock| (effect, clock))
                    })
                    .ok()
                    .flatten()
                })
                .ok()
                .flatten();
            let Some((effect, clock)) = result else {
                break;
            };
            let running = clock
                .observer
                .set_clock(clock.display.as_ref(), clock.epoch);
            let _ = weak.update(cx, |view, _| {
                if let Some(state) = &mut view.quake {
                    state.work.clock_installed(running);
                }
            });
            if let Some(effect) = effect {
                // Native effects execute outside GPUI window/entity borrows.
                let result = effect.run();
                let recovery =
                    cx.update(|cx| complete(result, handle, cx)).ok().flatten();
                if let Some(effect) = recovery
                    && let Err(error) = effect.run().outcome
                {
                    eprintln!("Quake recovery: {error}");
                }
                let _ = weak.update(cx, |view, _| {
                    if let Some(state) = &view.quake {
                        state.work.signal(WakeSource::Effect);
                    }
                });
            }
        }
    }));
    state.work.signal(WakeSource::Intent);
}

fn complete(
    result: NativeEffectResult,
    handle: AnyWindowHandle,
    cx: &mut App,
) -> Option<NativeEffect> {
    let NativeEffectResult { outcome, reporter } = result;
    match outcome {
        Ok(outcome) => {
            let _ = handle.update(cx, |root, _, cx| {
                if let Ok(root) = root.downcast::<WorkspaceView>() {
                    root.update(cx, |view, _| {
                        if let Some(state) = &mut view.quake {
                            state.work.failure.clear();
                        }
                    });
                }
            });
            if let Some(observation) = outcome.observation
                && let Some(journal) =
                    &mut cx.global_mut::<Desktop>().quake.smoke_journal
            {
                journal.record(observation);
            }
            if let Some(warning) = outcome.warning {
                report(cx, &warning, reporter);
            }
            None
        }
        Err(error) => {
            let message = error.to_string();
            let disposition = handle
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>().ok()?.update(
                        cx,
                        |view, _| {
                            Some(
                                view.quake
                                    .as_mut()?
                                    .work
                                    .failure
                                    .record(Instant::now(), &message),
                            )
                        },
                    )
                })
                .ok()
                .flatten();
            let (retry_recovery, should_report) = disposition?;
            let effect = if retry_recovery {
                recover(cx, handle, &message)
            } else {
                None
            };
            if should_report {
                report(cx, &message, reporter);
            }
            effect
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resume_without_frames_reseeds_the_active_fallback() {
        let (sender, _) = async_channel::bounded(1);
        let signal = Signal::new(sender);
        let now = Instant::now();
        let mut admission = Admission::new();
        admission.set_active(false, now, &signal);
        let resumed = now + Duration::from_secs(1);
        admission.set_active(true, resumed, &signal);
        assert!(!admission.take_frame(resumed, signal.take(), &signal));
        assert!(admission.take_frame(
            resumed + admission.period,
            signal.take(),
            &signal
        ));
    }
    #[test]
    fn persistent_failure_has_bounded_recovery_reports_and_retry_deadline() {
        let now = Instant::now();
        let mut failure = Failure::default();
        assert_eq!(failure.record(now, "no screens"), (true, true));
        assert!(failure.blocked(now));
        assert!(failure.blocked(now + Duration::from_millis(999)));
        let later = now + Duration::from_secs(1);
        assert!(!failure.blocked(later));
        // Successful sampling still needs the native effect to complete.
        failure.sampled(true);
        assert_eq!(failure.record(later, "no screens"), (true, false));
        assert_eq!(
            failure.record(later + Duration::from_secs(1), "no screens"),
            (false, false)
        );
        assert!(failure.blocked(later + Duration::from_secs(1)));
        failure.sampled(false);
        assert!(!failure.blocked(now));
        assert_eq!(failure.record(now, "set_frame failed"), (true, true));
        failure.sampled(true);
        assert_eq!(failure.record(later, "set_frame failed"), (true, false));
        failure.sampled(true);
        assert_eq!(
            failure.record(later + Duration::from_secs(1), "set_frame failed"),
            (false, false)
        );
        failure.clear(); // successful native completion
        assert!(!failure.blocked(now));
    }
    #[test]
    fn display_invalidation_preserves_fallback_progress_without_frames() {
        let (sender, _) = async_channel::bounded(1);
        let signal = Signal::new(sender);
        let now = Instant::now();
        let mut admission = Admission::new();
        admission.wants_frame = true;
        admission.last_sample = Some(now);
        let stale = signal.epoch();
        admission.invalidate(&signal);
        signal.frame(stale);
        assert!(
            admission.take_frame(
                now + admission.period * 2,
                signal.take(),
                &signal
            ),
            "display invalidation must not erase fallback timing"
        );
    }
    #[test]
    fn work_area_safety_sampling_is_visible_macos_only_unless_observer_failed()
    {
        assert!(needs_area_fallback(true, true, false, false, false));
        assert!(!needs_area_fallback(true, false, false, false, false));
        assert!(!needs_area_fallback(true, true, false, true, false));
        assert!(!needs_area_fallback(true, true, true, false, false));
        assert!(!needs_area_fallback(false, true, false, false, false));
        assert!(needs_area_fallback(false, false, false, false, true));
    }
    #[test]
    fn clock_handoff_rejects_stale_callbacks_and_coalesces_window_frames() {
        let (sender, _) = async_channel::bounded(1);
        let signal = Signal::new(sender);
        signal.take();
        let mut admission = Admission::new();
        admission.wants_frame = true;
        let now = Instant::now();
        admission.last_sample = Some(now);
        let native_epoch = signal.epoch();
        signal.frame(native_epoch);
        admission.window_frame(now, &signal);
        signal.frame(native_epoch);
        assert!(!admission.take_frame(now, signal.take(), &signal));
        admission.window_frame(now + admission.period, &signal);
        admission.window_frame(now + admission.period, &signal);
        assert!(admission.take_frame(
            now + admission.period,
            signal.take(),
            &signal
        ));
        admission.last_sample = Some(now + admission.period);
        assert!(!admission.take_frame(
            now + admission.period,
            signal.take(),
            &signal
        ));
    }
    #[test]
    fn stalled_frames_admit_refresh_derived_progress_and_idle_releases_admission()
     {
        let (sender, _) = async_channel::bounded(1);
        let signal = Signal::new(sender);
        signal.take();
        let now = Instant::now();
        let mut admission = Admission::new();
        admission.period = Duration::from_micros(8333);
        admission.wants_frame = true;
        admission.last_sample = Some(now);
        admission.window_frame(now, &signal);
        assert!(!admission.take_frame(
            now + admission.period,
            signal.take(),
            &signal
        ));
        assert!(admission.take_frame(
            now + admission.period * 2,
            signal.take(),
            &signal
        ));
        assert!(!admission.window_source);
        admission.set_active(false, now, &signal);
        admission.window_frame(now + admission.period * 3, &signal);
        assert!(!admission.take_frame(
            now + Duration::from_secs(1),
            signal.take(),
            &signal
        ));
    }
}
