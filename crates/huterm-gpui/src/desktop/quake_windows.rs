//! The desktop owns associations before terminals spawn. Window effects run
//! after GPUI returns its window borrow, on the same foreground executor.
use super::*;
use crate::quake::ProfileExt;
use crate::quake::{self, Display, Profile, Rect, Transition, hotkeys, native};
use std::{
    cell::Cell,
    collections::{BTreeMap, VecDeque},
    rc::Rc,
};

pub(super) struct Registry {
    platform: Option<native::Platform>,
    registrations: Option<hotkeys::Registrations>,
    windows: BTreeMap<String, AnyWindowHandle>,
    return_focus: Option<native::Focus>,
    pump_running: bool,
    pub(super) failed_spawn: Option<(String, String)>,
    smoke_journal: Option<SmokeJournal>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            platform: None,
            registrations: None,
            windows: BTreeMap::new(),
            return_focus: None,
            pump_running: false,
            failed_spawn: None,
            smoke_journal: std::env::var_os("HUTERM_QUAKE_SMOKE")
                .map(|_| SmokeJournal::new()),
        }
    }
}

const SMOKE_JOURNAL_CAPACITY: usize = 4096;

struct SmokeJournal {
    started: Instant,
    observations: VecDeque<SmokeObservation>,
}

impl SmokeJournal {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            observations: VecDeque::new(),
        }
    }

    fn record(&mut self, draft: SmokeObservationDraft) {
        if self.observations.len() == SMOKE_JOURNAL_CAPACITY {
            self.observations.pop_front();
        }
        self.observations.push_back(SmokeObservation {
            monotonic_us: self.started.elapsed().as_micros(),
            profile: draft.profile,
            generation: draft.generation,
            desired: draft.desired,
            progress: draft.progress,
            stage: draft.stage,
            frame: draft.frame,
            target_frame: draft.target_frame,
            display_frame: draft.display_frame,
            opacity: draft.opacity,
            scheduler_gap_us: draft.scheduler_gap.as_micros(),
        });
    }
}

pub(super) struct SmokeObservation {
    pub monotonic_us: u128,
    pub profile: String,
    pub generation: u64,
    pub desired: bool,
    pub progress: f64,
    pub stage: &'static str,
    pub frame: Rect,
    pub target_frame: Rect,
    pub display_frame: Rect,
    pub opacity: f64,
    pub scheduler_gap_us: u128,
}

struct SmokeObservationDraft {
    profile: String,
    generation: u64,
    desired: bool,
    progress: f64,
    stage: &'static str,
    frame: Rect,
    target_frame: Rect,
    display_frame: Rect,
    opacity: f64,
    scheduler_gap: Duration,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Prepare,
    Activate,
    Windowed,
    Animate,
    SettleVisible,
    SettleHidden,
    Idle,
}

#[derive(Default)]
struct Activation {
    seen: bool,
    requested: bool,
    next_retry: Option<Instant>,
}
const ACTIVATION_RETRY_INTERVAL: Duration = Duration::from_millis(100);

impl Activation {
    fn request(&mut self) {
        self.seen = false;
        self.requested = true;
        self.next_retry = None;
    }

    fn cancel(&mut self) {
        self.requested = false;
        self.next_retry = None;
    }

    fn observe(&mut self, active: bool) {
        if active {
            self.seen = true;
            self.cancel();
        } else if self.seen {
            self.cancel();
        }
    }

    fn take_request(&mut self, now: Instant) -> bool {
        if !std::mem::take(&mut self.requested) {
            return false;
        }
        self.next_retry = Some(now + ACTIVATION_RETRY_INTERVAL);
        true
    }

    fn take_retry(&mut self, now: Instant, deadline: Instant) -> bool {
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

    fn refit(
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

#[expect(
    clippy::struct_excessive_bools,
    reason = "independent focus, recovery, and presentation facts accompany the transition state"
)]
pub(super) struct Presentation {
    pub name: String,
    reporter: Option<WeakEntity<WorkspaceView>>,
    generation: Rc<Cell<u64>>,
    pub(super) native: native::Window,
    profile: Profile,
    display: Display,
    regular: bool,
    regular_bounds: Rect,
    target: Rect,
    transition: Transition,
    stage: Stage,
    deadline: Instant,
    activation: Activation,
    suppress_blur: Instant,
    last_display_check: Instant,
    restore_focus: bool,
    fade_supported: bool,
    recovering: bool,
    detach_requested: bool,
    observed_fullscreen: bool,
    smoke_observations: bool,
}
impl Presentation {
    pub fn chrome_hidden(&self) -> bool {
        !self.regular
    }
    pub fn fullscreen_context(&self) -> bool {
        self.observed_fullscreen
    }
    pub fn visible(&self) -> bool {
        self.transition.visible()
    }
    pub(super) fn cleanup(self) {
        self.generation.set(self.generation.get() + 1);
        if let Err(error) = self
            .native
            .set_fullscreen(false)
            .and_then(|()| self.native.opacity(1.0))
        {
            eprintln!("Quake cleanup: {error}");
        }
    }
    fn duration(&self) -> Duration {
        if self.regular || self.profile.animation == quake::Animation::None {
            Duration::ZERO
        } else {
            Duration::from_millis(u64::from(self.profile.animation_ms))
        }
    }
    fn request(&mut self, visible: bool, restore_focus: bool) {
        let now = Instant::now();
        self.deadline = now + Duration::from_secs(3);
        if visible {
            self.activation.request();
        } else {
            self.activation.cancel();
        }
        self.prepare(visible, restore_focus, now);
    }

    fn prepare(&mut self, visible: bool, restore_focus: bool, now: Instant) {
        self.generation.set(self.generation.get() + 1);
        self.recovering = false;
        self.transition.retarget(visible, now, self.duration());
        self.stage = Stage::Prepare;
        if visible {
            self.suppress_blur = now + Duration::from_millis(200);
        }
        self.restore_focus = restore_focus;
    }

    fn refit(&mut self) {
        let now = Instant::now();
        self.deadline = self.activation.refit(self.stage, self.deadline, now);
        // Placement changes preserve pending activation decisions and their deadline.
        self.prepare(self.transition.visible(), false, now);
    }
}

enum NativeOp {
    Frame(Rect),
    Quake(bool),
    Fullscreen(bool),
    Opacity(f64),
    Show,
    Hide(Option<(native::Platform, native::Focus)>),
    Failure(anyhow::Error),
}
struct NativeEffect {
    native: native::Window,
    reporter: Option<WeakEntity<WorkspaceView>>,
    gate: Rc<Cell<u64>>,
    generation: u64,
    operations: Vec<NativeOp>,
    observation: Option<SmokeObservationDraft>,
}
struct NativeEffectOutcome {
    warning: Option<String>,
    observation: Option<SmokeObservationDraft>,
}
struct NativeEffectResult {
    outcome: anyhow::Result<NativeEffectOutcome>,
    reporter: Option<WeakEntity<WorkspaceView>>,
}
impl NativeEffect {
    fn for_state(state: &Presentation, operations: Vec<NativeOp>) -> Self {
        Self {
            native: state.native.clone(),
            reporter: state.reporter.clone(),
            gate: Rc::clone(&state.generation),
            generation: state.generation.get(),
            operations,
            observation: None,
        }
    }
    fn for_transition(
        state: &Presentation,
        operations: Vec<NativeOp>,
        progress: f64,
        resulting_stage: Stage,
        frame: Rect,
        opacity: f64,
        scheduler_gap: Duration,
    ) -> Self {
        let mut effect = Self::for_state(state, operations);
        if state.smoke_observations {
            effect.observation = Some(SmokeObservationDraft {
                profile: state.name.clone(),
                generation: state.generation.get(),
                desired: state.transition.visible(),
                progress,
                stage: resulting_stage.name(),
                frame,
                target_frame: state.target,
                display_frame: state.display.frame,
                opacity,
                scheduler_gap,
            });
        }
        effect
    }
    fn run(self) -> NativeEffectResult {
        let reporter = self.reporter.clone();
        let outcome = (|| {
            let mut warning = None;
            for operation in self.operations {
                if self.gate.get() != self.generation {
                    return Ok(NativeEffectOutcome {
                        warning,
                        observation: None,
                    });
                }
                match operation {
                    NativeOp::Frame(frame) => self.native.set_frame(frame)?,
                    NativeOp::Quake(enabled) => {
                        self.native.set_quake(enabled)?;
                    }
                    NativeOp::Fullscreen(enabled) => {
                        self.native.set_fullscreen(enabled)?;
                    }
                    NativeOp::Opacity(value) => self.native.opacity(value)?,
                    NativeOp::Show => self.native.show()?,
                    NativeOp::Hide(target) => {
                        let was_active = self.native.active()?;
                        self.native.hide()?;
                        if was_active
                            && self.gate.get() == self.generation
                            && let Some((platform, target)) = target
                            && let Err(error) = platform.focus(&target)
                        {
                            warning = Some(format!(
                                "focus restoration failed after hiding: {error}"
                            ));
                        }
                    }
                    NativeOp::Failure(error) => return Err(error),
                }
            }
            Ok(NativeEffectOutcome {
                warning,
                observation: self.observation,
            })
        })();
        NativeEffectResult { outcome, reporter }
    }
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Self::Prepare => "Prepare",
            Self::Activate => "Activate",
            Self::Windowed => "Windowed",
            Self::Animate => "Animate",
            Self::SettleVisible => "SettleVisible",
            Self::SettleHidden => "SettleHidden",
            Self::Idle => "Idle",
        }
    }
}

pub(super) fn install(cx: &mut App) {
    let config = cx.global::<Desktop>().config.clone();
    let compiled = keymap::compile(Platform::current(), &config.keybindings);
    let result = compiled
        .map_err(|error| error.to_string())
        .and_then(|compiled| replace_registrations(cx, &config, &compiled));
    if let Err(error) = result {
        report(cx, &error, None);
    }
}

fn start_pump(cx: &mut App) {
    if cx.global::<Desktop>().quake.pump_running {
        return;
    }
    cx.global_mut::<Desktop>().quake.pump_running = true;
    cx.spawn(async move |cx| {
        loop {
            let keep_running = cx.update(|cx| {
                let registry = &mut cx.global_mut::<Desktop>().quake;
                if registry.windows.is_empty() {
                    registry.pump_running = false;
                    false
                } else {
                    true
                }
            });
            if !matches!(keep_running, Ok(true)) {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            let Ok(effects) = cx.update(tick) else {
                break;
            };
            for (handle, effect) in effects {
                let NativeEffectResult { outcome, reporter } = effect.run();
                match outcome {
                    Ok(outcome) => {
                        let _ = cx.update(|cx| {
                            if let Some(observation) = outcome.observation
                                && let Some(journal) = cx
                                    .global_mut::<Desktop>()
                                    .quake
                                    .smoke_journal
                                    .as_mut()
                            {
                                journal.record(observation);
                            }
                            if let Some(warning) = outcome.warning {
                                report(cx, &warning, reporter);
                            }
                        });
                    }
                    Err(error) => {
                        let message = error.to_string();
                        let recovery = cx
                            .update(|cx| recover(cx, handle, &message))
                            .ok()
                            .flatten();
                        if let Some(effect) = recovery
                            && let Err(error) = effect.run().outcome
                        {
                            eprintln!("Quake recovery: {error}");
                        }
                        let _ = cx.update(|cx| report(cx, &message, reporter));
                    }
                }
            }
        }
    })
    .detach();
}

pub(super) fn replace_registrations(
    cx: &mut App,
    config: &Config,
    compiled: &keymap::CompiledKeymap,
) -> Result<(), String> {
    let bindings =
        hotkeys::compile(&config.global_keybindings, &config.quake, compiled)?;
    if bindings.is_empty()
        && cx.global::<Desktop>().quake.registrations.is_none()
    {
        return Ok(());
    }
    if cx.global::<Desktop>().quake.registrations.is_none() {
        let registrations = hotkeys::Registrations::new()?;
        let wakeups = registrations.wakeups.clone();
        cx.global_mut::<Desktop>().quake.registrations = Some(registrations);
        cx.spawn(async move |cx| {
            while wakeups.recv().await.is_ok() {
                if cx.update(dispatch_hotkeys).is_err() {
                    break;
                }
            }
        })
        .detach();
    }
    cx.global_mut::<Desktop>()
        .quake
        .registrations
        .as_mut()
        .ok_or("global shortcut manager unavailable")?
        .replace(bindings)
}

pub(super) fn keep_alive(cx: &App) -> bool {
    cx.global::<Desktop>()
        .quake
        .registrations
        .as_ref()
        .is_some_and(hotkeys::Registrations::has_bindings)
}
pub(super) fn shutdown(cx: &mut App) {
    cx.global_mut::<Desktop>().quake.registrations.take();
}
fn platform(cx: &mut App) -> Result<native::Platform, String> {
    if cx.global::<Desktop>().quake.platform.is_none() {
        cx.global_mut::<Desktop>().quake.platform =
            Some(native::Platform::new(cx).map_err(|error| error.to_string())?);
    }
    cx.global::<Desktop>()
        .quake
        .platform
        .clone()
        .ok_or_else(|| "native quake platform unavailable".into())
}
fn report(
    cx: &mut App,
    error: &str,
    reporter: Option<WeakEntity<WorkspaceView>>,
) {
    let message = format!("Quake: {error}");
    if reporter.is_none() {
        cx.global_mut::<Desktop>().config_error = Some(message.clone());
    }
    report_deferred_failure(cx, reporter, message);
}

pub(super) fn invoke(
    cx: &mut App,
    invocation: &CommandInvocation,
    reporter: Option<WeakEntity<WorkspaceView>>,
) -> Result<CommandOutcome, CommandError> {
    let name = invocation.text("profile").unwrap_or("default").to_owned();
    if !cx
        .global::<Desktop>()
        .config
        .quake
        .profiles
        .contains_key(&name)
    {
        return Err(CommandError::Unavailable(format!(
            "unknown quake profile {name:?}"
        )));
    }
    if cx.global::<Desktop>().quitting || cx.global::<Desktop>().quit_pending {
        return Err(CommandError::Unavailable(
            "application is quitting".into(),
        ));
    }
    let command = invocation.id;
    cx.defer(move |cx| {
        if let Err(error) = invoke_now(cx, &name, command, reporter.clone()) {
            report(cx, &error, reporter);
        }
    });
    Ok(CommandOutcome::Accepted)
}
fn invoke_now(
    cx: &mut App,
    name: &str,
    command: huterm_protocol::CommandId,
    reporter: Option<WeakEntity<WorkspaceView>>,
) -> Result<(), String> {
    let native = platform(cx)?;
    let config = cx
        .global::<Desktop>()
        .config
        .quake
        .profiles
        .get(name)
        .cloned()
        .ok_or_else(|| format!("unknown quake profile {name:?}"))?;
    let handle = cx.global::<Desktop>().quake.windows.get(name).copied();
    if handle.is_none() && command == ids::HIDE_QUAKE {
        return Ok(());
    }
    if let Some(focus) = native.focused().map_err(|error| error.to_string())?
        && !native.is_ours(&focus).map_err(|error| error.to_string())?
    {
        cx.global_mut::<Desktop>().quake.return_focus = Some(focus);
    }
    if let Some(handle) = handle {
        let result = handle.update(cx, |root, window, cx| {
            let view = root.downcast::<WorkspaceView>()
                .map_err(|_| "quake root unavailable")?;
            view.update(cx, |view, cx| {
                let native_idle = view.native_transition_idle();
                let Some(state) = view.quake.as_mut() else {
                    return Err("quake association unavailable".to_owned());
                };
                if state.detach_requested {
                    return Err("the previous profile window is becoming an ordinary window; retry after its transition".into());
                }
                if native_idle && state.regular
                    && state.native.visible().map_err(|error| error.to_string())?
                    && !state.native.fullscreen().map_err(|error| error.to_string())?
                {
                    state.regular_bounds = state.native.frame()
                        .map_err(|error| error.to_string())?;
                }
                let active = state.native.active().map_err(|error| error.to_string())?;
                let show = command == ids::SHOW_QUAKE
                    || (command == ids::TOGGLE_QUAKE
                        && (!state.transition.visible()
                            || (state.stage == Stage::Idle && !active)));
                if !show && view.close.confirmation.is_some() {
                    return Err("a close confirmation must remain visible".into());
                }
                let unchanged = show && native_idle && !state.regular
                    && state.stage == Stage::Idle && state.transition.visible()
                    && state.profile == config
                    && state.native.visible().map_err(|error| error.to_string())?
                    && state.native.fullscreen().map_err(|error| error.to_string())? == config.fullscreen
                    && near(state.native.frame().map_err(|error| error.to_string())?, config.geometry(&state.display));
                if show {
                    state.profile = config.clone();
                    if !state.regular {
                        state.target = state.profile.geometry(&state.display);
                    }
                }
                state.reporter.clone_from(&reporter);
                state.request(show, !show && active);
                if unchanged {
                    state.activation.observe(active);
                    state.stage = Stage::Activate;
                }
                view.sync_quake_visibility(window, cx);
                Ok(())
            })
        });
        match result {
            Ok(result) => return result,
            Err(_) => {
                cx.global_mut::<Desktop>().quake.windows.remove(name);
            }
        }
    }
    let display = native
        .resolve_display(&config.display)
        .map_err(|error| error.to_string())?;
    open_window_with_profile(
        cx,
        true,
        Some((name.to_owned(), config, display)),
        reporter,
    );
    if !cx.global::<Desktop>().quake.windows.contains_key(name) {
        return Err(format!("could not create quake profile {name:?}"));
    }
    Ok(())
}

pub(super) fn attach(
    name: String,
    profile: Profile,
    display: Display,
    reporter: Option<WeakEntity<WorkspaceView>>,
    window: &Window,
    cx: &mut App,
) -> Result<Presentation, String> {
    let platform = platform(cx)?;
    let native = platform.window(window).map_err(|error| error.to_string())?;
    if native.visible().map_err(|error| error.to_string())? {
        return Err(
            "native quake window was mapped before initialization".into()
        );
    }
    let mut regular_bounds =
        native.frame().map_err(|error| error.to_string())?;
    regular_bounds.x =
        display.work.x + (display.work.width - regular_bounds.width) / 2.0;
    regular_bounds.y =
        display.work.y + (display.work.height - regular_bounds.height) / 2.0;
    let now = Instant::now();
    let fade_supported = platform
        .supports_fade()
        .map_err(|error| error.to_string())?;
    if !fade_supported && profile.effects().0 {
        eprintln!(
            "Quake: no X11 compositor; opacity animation is unavailable, position animation remains enabled"
        );
    }
    let target = profile.geometry(&display);
    let mut transition = Transition::new(false, now);
    transition.retarget(true, now, Duration::ZERO);
    cx.global_mut::<Desktop>()
        .quake
        .windows
        .insert(name.clone(), window.window_handle());
    start_pump(cx);
    Ok(Presentation {
        name,
        reporter,
        generation: Rc::new(Cell::new(0)),
        native,
        profile,
        display,
        regular: false,
        regular_bounds,
        target,
        transition,
        stage: Stage::Prepare,
        deadline: now + Duration::from_secs(3),
        activation: {
            let mut activation = Activation::default();
            activation.request();
            activation
        },
        suppress_blur: now + Duration::from_millis(200),
        last_display_check: now,
        restore_focus: false,
        fade_supported,
        recovering: false,
        detach_requested: false,
        observed_fullscreen: false,
        smoke_observations: std::env::var_os("HUTERM_QUAKE_SMOKE").is_some(),
    })
}

pub(super) fn toggle(
    view: &mut WorkspaceView,
    window: &mut Window,
    cx: &mut Context<'_, WorkspaceView>,
) -> Result<CommandOutcome, CommandError> {
    if view.close.confirmation.is_some() {
        return Err(CommandError::Unavailable(
            "a close confirmation is active".into(),
        ));
    }
    let native_idle = view.native_transition_idle();
    let state = view.quake.as_mut().ok_or(CommandError::ClientRequired)?;
    if state.regular {
        if native_idle
            && !state
                .native
                .fullscreen()
                .map_err(|error| CommandError::Unavailable(error.to_string()))?
        {
            state.regular_bounds = state.native.frame().map_err(|error| {
                CommandError::Unavailable(error.to_string())
            })?;
        }
        let native =
            cx.global::<Desktop>().quake.platform.as_ref().ok_or_else(
                || {
                    CommandError::Unavailable(
                        "native quake platform unavailable".into(),
                    )
                },
            )?;
        let displays = native
            .displays()
            .map_err(|error| CommandError::Unavailable(error.to_string()))?;
        let center = (
            state.regular_bounds.x + state.regular_bounds.width / 2.0,
            state.regular_bounds.y + state.regular_bounds.height / 2.0,
        );
        state.display = displays
            .iter()
            .find(|display| display.frame.contains(center.0, center.1))
            .or_else(|| displays.iter().find(|display| display.primary))
            .or_else(|| displays.first())
            .cloned()
            .ok_or_else(|| {
                CommandError::Unavailable("no display available".into())
            })?;
        state.profile = cx
            .global::<Desktop>()
            .config
            .quake
            .profiles
            .get(&state.name)
            .cloned()
            .unwrap_or_else(|| state.profile.clone());
        state.target = state.profile.geometry(&state.display);
    }
    state.regular = !state.regular;
    state.request(true, false);
    state.suppress_blur = Instant::now() + Duration::from_millis(300);
    view.sync_quake_visibility(window, cx);
    Ok(CommandOutcome::Accepted)
}

fn dispatch_hotkeys(cx: &mut App) {
    let commands = cx
        .global_mut::<Desktop>()
        .quake
        .registrations
        .as_ref()
        .map(hotkeys::Registrations::drain)
        .unwrap_or_default();
    for command in commands {
        if let Err(error) = invoke(cx, &command, None) {
            report(cx, &error.to_string(), None);
        }
    }
}
fn tick(cx: &mut App) -> Vec<(AnyWindowHandle, NativeEffect)> {
    let handles: Vec<_> = cx
        .global::<Desktop>()
        .quake
        .windows
        .values()
        .copied()
        .collect();
    let mut effects = Vec::new();
    for handle in handles {
        let result = handle.update(cx, |root, window, cx| {
            let Ok(view) = root.downcast::<WorkspaceView>() else {
                return None;
            };
            view.update(cx, |view, cx| step(view, window, cx))
        });
        if let Ok(Some(effect)) = result {
            effects.push((handle, effect));
        }
    }
    effects
}
fn recover(
    cx: &mut App,
    handle: AnyWindowHandle,
    message: &str,
) -> Option<NativeEffect> {
    handle
        .update(cx, |root, window, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.update(cx, |view, cx| {
                view.status = Some(format!("Quake: {message}"));
                let state = view.quake.as_mut()?;
                state.generation.set(state.generation.get() + 1);
                let now = Instant::now();
                state.transition = Transition::new(true, now);
                // Recovery uses the same observed native exit gate as ordinary
                // transitions. A second failure stops rather than looping forever.
                state.stage = if state.recovering {
                    Stage::Idle
                } else {
                    Stage::Prepare
                };
                state.recovering = true;
                state.regular = true;
                state.deadline = now + Duration::from_secs(3);
                state.activation.request();
                let effect = NativeEffect::for_state(
                    state,
                    vec![NativeOp::Opacity(1.0)],
                );
                view.sync_quake_visibility(window, cx);
                Some(effect)
            })
        })
        .ok()
        .flatten()
}

#[expect(
    clippy::too_many_lines,
    reason = "one exhaustive native transition reducer keeps effect ordering visible"
)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "native screen coordinates become GPUI logical f32 pixels"
)]
fn step(
    view: &mut WorkspaceView,
    window: &mut Window,
    cx: &mut Context<'_, WorkspaceView>,
) -> Option<NativeEffect> {
    let now = Instant::now();
    let native_idle = view.native_transition_idle();
    let state = view.quake.as_mut()?;
    if state.detach_requested && state.stage == Stage::Idle && !state.recovering
    {
        state.generation.set(state.generation.get() + 1);
        cx.global_mut::<Desktop>().quake.windows.remove(&state.name);
        view.quake.take();
        view.fullscreen = FullscreenController::new(
            window.window_bounds(),
            view.config.window.macos_fullscreen_mode,
            cfg!(target_os = "macos"),
        );
        view.bounds = window.window_bounds();
        view.sync_quake_visibility(window, cx);
        return None;
    }
    let native = state.native.clone();
    let platform = cx.global::<Desktop>().quake.platform.clone()?;
    let fullscreen_before = state.observed_fullscreen;
    state.observed_fullscreen = match native.fullscreen() {
        Ok(value) => value,
        Err(error) => {
            return Some(NativeEffect::for_state(
                state,
                vec![NativeOp::Failure(error)],
            ));
        }
    };
    if fullscreen_before != state.observed_fullscreen {
        cx.notify();
    }
    let active = match native.active() {
        Ok(value) => value,
        Err(error) => {
            return Some(NativeEffect::for_state(
                state,
                vec![NativeOp::Failure(error)],
            ));
        }
    };
    state.activation.observe(active);
    if view.close.confirmation.is_some() && !state.transition.visible() {
        state.request(true, false);
    }
    if !state.regular
        && state.profile.hide_on_focus_loss
        && state.transition.visible()
        && state.activation.seen
        && !active
        && now >= state.suppress_blur
        && view.close.confirmation.is_none()
        && !view.busy
    {
        state.request(false, false);
    }
    let refresh_display = now.duration_since(state.last_display_check)
        >= Duration::from_secs(1)
        || matches!(state.stage, Stage::Windowed | Stage::SettleVisible);
    if refresh_display && !state.regular {
        state.last_display_check = now;
        match platform.displays() {
            Ok(displays) => {
                if let Some(display) = displays
                    .iter()
                    .find(|display| display.id == state.display.id)
                    .or_else(|| displays.iter().find(|display| display.primary))
                    .or_else(|| displays.first())
                    && *display != state.display
                {
                    state.display = display.clone();
                    let target = state.profile.geometry(display);
                    let geometry_changed = target != state.target;
                    state.target = target;
                    // Presentation leases can change the work area mid-transition.
                    // Preserve active progress and its original failure deadline.
                    if geometry_changed
                        && matches!(state.stage, Stage::Idle | Stage::Activate)
                    {
                        state.refit();
                    }
                }
            }
            Err(error) => {
                return Some(NativeEffect::for_state(
                    state,
                    vec![NativeOp::Failure(error)],
                ));
            }
        }
    }
    if state.stage != Stage::Idle && !native_idle && now < state.deadline {
        state.transition.pause(now);
        return None;
    }
    if state.stage != Stage::Idle && now >= state.deadline {
        return Some(NativeEffect::for_state(
            state,
            vec![NativeOp::Failure(anyhow::anyhow!(
                "native quake transition {:?} timed out (frame {:?}, target {:?}, fullscreen {:?}, active {:?})",
                state.stage,
                state.native.frame(),
                state.target,
                state.native.fullscreen(),
                state.native.active()
            ))],
        ));
    }
    let regular = state.regular;
    let showing = state.transition.visible();
    if native_idle
        && state.stage == Stage::Idle
        && state.regular
        && !native.fullscreen().unwrap_or(true)
        && native.visible().unwrap_or(false)
    {
        if let Ok(frame) = native.frame() {
            state.regular_bounds = frame;
        }
        view.bounds = window.window_bounds();
    }
    let effect: Option<NativeEffect> = match state.stage {
        Stage::Activate => {
            state.stage = Stage::SettleVisible;
            state
                .activation
                .take_request(now)
                .then(|| NativeEffect::for_state(state, vec![NativeOp::Show]))
        }
        Stage::Prepare => {
            state.stage = Stage::Windowed;
            state.suppress_blur = now + Duration::from_millis(200);
            state.transition.pause(now);
            Some(NativeEffect::for_state(
                state,
                vec![NativeOp::Fullscreen(false)],
            ))
        }
        Stage::Windowed => {
            state.transition.pause(now);
            match native.fullscreen() {
                Ok(false) => {
                    state.stage = Stage::Animate;
                    if regular {
                        let bounds = state.regular_bounds;
                        Some(NativeEffect::for_state(
                            state,
                            vec![
                                NativeOp::Quake(false),
                                NativeOp::Frame(bounds),
                            ],
                        ))
                    } else {
                        Some(NativeEffect::for_state(
                            state,
                            vec![NativeOp::Quake(true)],
                        ))
                    }
                }
                Ok(true) => None,
                Err(error) => Some(NativeEffect::for_state(
                    state,
                    vec![NativeOp::Failure(error)],
                )),
            }
        }
        Stage::Animate => {
            let scheduler_gap = state.transition.elapsed_since_sample(now);
            let progress = state.transition.sample(now, state.duration());
            let finished = state.transition.finished();
            let (fade, edge) = state.profile.effects();
            let frame = quake::animation_frame(
                state.target,
                state.display.frame,
                edge,
                progress,
            );
            let opacity = if fade && state.fade_supported && !regular {
                progress
            } else {
                1.0
            };
            let focus = state.activation.take_request(now);
            let fullscreen = state.profile.fullscreen && !regular;
            let return_focus =
                if finished && !showing && state.restore_focus && active {
                    cx.global::<Desktop>().quake.return_focus.clone()
                } else {
                    None
                };
            if finished {
                state.stage = if showing {
                    Stage::SettleVisible
                } else {
                    Stage::SettleHidden
                };
            }
            let mut operations = Vec::new();
            if !regular {
                operations.push(NativeOp::Frame(frame));
            }
            operations.push(NativeOp::Opacity(opacity));
            if showing && (focus || !native.visible().unwrap_or(false)) {
                operations.push(NativeOp::Show);
            }
            if finished {
                if showing {
                    operations.push(NativeOp::Fullscreen(fullscreen));
                    operations.push(NativeOp::Opacity(1.0));
                } else {
                    operations.push(NativeOp::Hide(
                        return_focus.map(|focus| (platform, focus)),
                    ));
                    operations.push(NativeOp::Opacity(1.0));
                }
            }
            Some(NativeEffect::for_transition(
                state,
                operations,
                progress,
                state.stage,
                frame,
                opacity,
                scheduler_gap,
            ))
        }
        Stage::SettleVisible => {
            let expected = state.profile.fullscreen && !regular;
            let settled = (|| -> anyhow::Result<bool> {
                Ok(native.visible()?
                    // Initial activation must succeed, but a later app switch
                    // does not invalidate the requested visible geometry.
                    && state.activation.seen
                    && native.fullscreen()? == expected
                    && (regular || near(native.frame()?, state.target)))
            })();
            match settled {
                Ok(true) => {
                    state.stage = Stage::Idle;
                    state.recovering = false;
                    state.reporter = None;
                    if regular {
                        view.bounds = window.window_bounds();
                    } else {
                        let factor = if cfg!(target_os = "linux") {
                            f64::from(window.scale_factor())
                        } else {
                            1.0
                        };
                        view.bounds = WindowBounds::Windowed(Bounds::new(
                            point(
                                px((state.target.x / factor) as f32),
                                px((state.target.y / factor) as f32),
                            ),
                            size(
                                px((state.target.width / factor) as f32),
                                px((state.target.height / factor) as f32),
                            ),
                        ));
                    }
                    None
                }
                Ok(false) => {
                    // A window manager may apply initial placement when mapping,
                    // after the frame set while the window was hidden. AppKit
                    // can also move the frame when presentation options change.
                    // Reassert the endpoint after those native changes; only
                    // X11 delegates fullscreen geometry to the window manager.
                    let reassert_frame = !regular
                        && (!expected || cfg!(target_os = "macos"))
                        && native.visible().unwrap_or(false);
                    let retry_activation =
                        state.activation.take_retry(now, state.deadline);
                    let mut operations = Vec::new();
                    if reassert_frame {
                        operations.push(NativeOp::Frame(state.target));
                    }
                    if retry_activation {
                        operations.push(NativeOp::Show);
                    }
                    if operations.is_empty() {
                        None
                    } else {
                        Some(NativeEffect::for_state(state, operations))
                    }
                }
                Err(error) => Some(NativeEffect::for_state(
                    state,
                    vec![NativeOp::Failure(error)],
                )),
            }
        }
        Stage::SettleHidden => match native.visible() {
            Ok(false) => {
                state.stage = Stage::Idle;
                state.reporter = None;
                None
            }
            Ok(true) => None,
            Err(error) => Some(NativeEffect::for_state(
                state,
                vec![NativeOp::Failure(error)],
            )),
        },
        Stage::Idle => None,
    };
    view.sync_quake_visibility(window, cx);
    effect
}
fn near(left: Rect, right: Rect) -> bool {
    [
        left.x - right.x,
        left.y - right.y,
        left.width - right.width,
        left.height - right.height,
    ]
    .iter()
    .all(|delta| delta.abs() <= 2.0)
}

pub(super) fn close(
    view: &mut WorkspaceView,
    cx: &mut Context<'_, WorkspaceView>,
) {
    if let Some(state) = view.quake.take() {
        state.generation.set(state.generation.get() + 1);
        cx.global_mut::<Desktop>().quake.windows.remove(&state.name);
        #[cfg(target_os = "macos")]
        cx.foreground_executor()
            .spawn(async move {
                if let Err(error) = state
                    .native
                    .set_fullscreen(false)
                    .and_then(|()| state.native.opacity(1.0))
                {
                    eprintln!("Quake close cleanup: {error}");
                }
            })
            .detach();
    }
}

pub(super) fn reconcile(cx: &mut App) {
    let removed: Vec<_> = cx
        .global::<Desktop>()
        .quake
        .windows
        .iter()
        .filter(|(name, _)| {
            !cx.global::<Desktop>()
                .config
                .quake
                .profiles
                .contains_key(*name)
        })
        .map(|(_, handle)| *handle)
        .collect();
    for handle in removed {
        cx.defer(move |cx| {
            let _ = handle.update(cx, |root, window, cx| {
                let Ok(view) = root.downcast::<WorkspaceView>() else {
                    return;
                };
                view.update(cx, |view, cx| {
                    let native_idle = view.native_transition_idle();
                    let Some(state) = view.quake.as_mut() else {
                        return;
                    };
                    if native_idle
                        && state.regular
                        && !state.native.fullscreen().unwrap_or(true)
                        && let Ok(frame) = state.native.frame()
                    {
                        state.regular_bounds = frame;
                    }
                    state.detach_requested = true;
                    state.regular = true;
                    state.request(true, false);
                    view.sync_quake_visibility(window, cx);
                });
            });
        });
    }
}

impl WorkspaceView {
    #[cfg_attr(
        not(target_os = "macos"),
        expect(
            clippy::unused_self,
            reason = "macOS checks the window-owned native Space observer"
        )
    )]
    fn native_transition_idle(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.native_fullscreen
                .as_ref()
                .is_none_or(|adapter| adapter.check_native_transition().is_ok())
        }
        #[cfg(not(target_os = "macos"))]
        {
            true
        }
    }
    pub(super) fn chrome_hidden(&self) -> bool {
        self.quake
            .as_ref()
            .map_or(self.fullscreen.chrome_hidden, Presentation::chrome_hidden)
    }
    pub(super) fn fullscreen_context(&self) -> bool {
        self.quake.as_ref().map_or_else(
            || self.fullscreen.fullscreen_context(),
            Presentation::fullscreen_context,
        )
    }
    pub(super) fn quake_visible(&self) -> bool {
        self.quake.as_ref().is_none_or(Presentation::visible)
    }
    fn sync_quake_visibility(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let visible = self.quake_visible();
        #[cfg(target_os = "macos")]
        {
            self.fullscreen_insets = self
                .quake
                .as_ref()
                .filter(|state| !state.regular && state.profile.fullscreen)
                .map_or_else(gpui::Edges::default, |state| {
                    state.native.safe_area()
                });
        }
        self.sync_tab_layout(window, cx);
        let mut changed_any = false;
        for tab in &self.tabs {
            let active = visible && Some(tab.id) == self.active;
            tab.view.update(cx, |terminal, cx| {
                let changed = terminal.visible != active;
                changed_any |= changed;
                if terminal.visible && !active {
                    terminal.hide(cx);
                }
                if !terminal.visible && active {
                    terminal.visible = true;
                    terminal.scroll.invalidate();
                    terminal.start_initial_snapshot(cx);
                }
                if changed {
                    terminal.resize_if_needed(window);
                    cx.notify();
                }
            });
        }
        if changed_any {
            cx.notify();
        }
    }
}

pub(super) fn inspect_return_focus(cx: &App) -> anyhow::Result<String> {
    let registry = &cx.global::<Desktop>().quake;
    let Some(platform) = &registry.platform else {
        return Ok(
            "current_focus_id=0\nreturn_focus_id=0\nreturn_focus_gone=false"
                .into(),
        );
    };
    let current = platform
        .focused()?
        .as_ref()
        .map(|focus| platform.inspect_focus(focus))
        .transpose()?
        .map_or(0, |(id, _)| id);
    let (id, gone) = registry
        .return_focus
        .as_ref()
        .map(|focus| platform.inspect_focus(focus))
        .transpose()?
        .unwrap_or((0, false));
    Ok(format!(
        "current_focus_id={current}\nreturn_focus_id={id}\nreturn_focus_gone={gone}"
    ))
}

pub(super) fn drain_smoke_observations(cx: &mut App) -> Vec<SmokeObservation> {
    cx.global_mut::<Desktop>()
        .quake
        .smoke_journal
        .as_mut()
        .map(|journal| journal.observations.drain(..).collect())
        .unwrap_or_default()
}

pub(super) fn inspect(state: &Presentation) -> anyhow::Result<String> {
    let frame = state.native.frame()?;
    Ok(format!(
        "stage={:?}\nregular={}\ndesired={}\nvisible={}\nactive={}\nactivation_seen={}\nfullscreen={}\nfullscreen_context={}\nframe={},{},{},{}\nwork_area={},{},{},{}\ndisplay={}\n{}",
        state.stage,
        state.regular,
        state.transition.visible(),
        state.native.visible()?,
        state.native.active()?,
        state.activation.seen,
        state.native.fullscreen()?,
        state.fullscreen_context(),
        frame.x,
        frame.y,
        frame.width,
        frame.height,
        state.display.work.x,
        state.display.work.y,
        state.display.work.width,
        state.display.work.height,
        state.display.id,
        state.native.inspect()?
    ))
}

pub(super) fn take_for_quit(cx: &mut App) -> Vec<Presentation> {
    cx.global_mut::<Desktop>().quake.windows.clear();
    let mut states = Vec::new();
    for view in cx.global::<Desktop>().windows.clone() {
        if let Ok(Some(state)) = view.update(cx, |view, _| view.quake.take()) {
            state.generation.set(state.generation.get() + 1);
            states.push(state);
        }
    }
    states
}

#[cfg(test)]
mod tests {
    use super::{ACTIVATION_RETRY_INTERVAL, Activation, Stage};
    use std::time::{Duration, Instant};

    #[test]
    fn observed_app_switch_cancels_activation_still_waiting_to_run() {
        let now = Instant::now();
        let mut activation = Activation::default();
        activation.request();
        assert!(activation.take_request(now));
        activation.observe(true);
        activation.observe(false);
        assert!(!activation.take_request(now));
        assert!(!activation.take_retry(
            now + ACTIVATION_RETRY_INTERVAL,
            now + Duration::from_secs(3)
        ));
        assert!(activation.seen, "initial activation remains successful");
    }

    #[test]
    fn unfocused_observation_before_initial_activation_keeps_the_request() {
        let now = Instant::now();
        let mut activation = Activation::default();
        activation.request();
        activation.observe(false);
        assert!(activation.take_request(now));
        assert!(!activation.seen, "initial activation is still required");
        activation.observe(true);
        activation.request();
        assert!(!activation.seen, "a new summon needs fresh activation");
    }

    #[test]
    fn delayed_initial_show_anchors_retry_cadence_at_consumption() {
        let requested = Instant::now();
        let consumed = requested + Duration::from_millis(500);
        let deadline = requested + Duration::from_secs(3);
        let mut activation = Activation::default();
        activation.request();
        assert!(activation.take_request(consumed));
        assert!(
            !activation
                .take_retry(consumed + Duration::from_millis(16), deadline)
        );
        assert!(
            activation
                .take_retry(consumed + ACTIVATION_RETRY_INTERVAL, deadline)
        );
    }

    #[test]
    fn active_observation_before_consumption_cancels_initial_show_and_retry() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(3);
        let mut activation = Activation::default();
        activation.request();
        activation.observe(true);
        assert!(!activation.take_request(now));
        assert!(
            !activation.take_retry(now + ACTIVATION_RETRY_INTERVAL, deadline)
        );
    }

    #[test]
    fn initial_activation_retries_on_cadence_until_the_original_deadline() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(3);
        let mut activation = Activation::default();
        activation.request();
        assert!(activation.take_request(now), "initial Show must run");
        assert!(
            !activation.take_retry(now + Duration::from_millis(99), deadline)
        );
        assert!(
            activation.take_retry(now + ACTIVATION_RETRY_INTERVAL, deadline)
        );
        assert!(
            activation
                .take_retry(now + ACTIVATION_RETRY_INTERVAL * 2, deadline)
        );
        assert!(!activation.take_retry(deadline, deadline));
    }

    #[test]
    fn activation_retry_cadence_prevents_per_frame_show_requests() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(3);
        let mut activation = Activation::default();
        activation.request();
        assert!(activation.take_request(now));
        assert!(
            !activation.take_retry(now + Duration::from_millis(16), deadline)
        );
        assert!(
            activation.take_retry(now + ACTIVATION_RETRY_INTERVAL, deadline)
        );
        assert!(!activation.take_retry(
            now + ACTIVATION_RETRY_INTERVAL + Duration::from_millis(16),
            deadline
        ));
    }

    #[test]
    fn a_new_summon_resets_activation_retry_eligibility() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(3);
        let mut activation = Activation::default();
        activation.request();
        assert!(activation.take_request(now));
        activation.observe(true);
        activation.observe(false);
        assert!(
            !activation.take_retry(now + ACTIVATION_RETRY_INTERVAL, deadline)
        );

        let next = now + Duration::from_secs(1);
        activation.request();
        assert!(activation.take_request(next));
        assert!(
            activation.take_retry(next + ACTIVATION_RETRY_INTERVAL, deadline)
        );
    }

    #[test]
    fn geometry_refit_does_not_rearm_canceled_activation_or_extend_its_budget()
    {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(3);
        let mut activation = Activation::default();
        activation.request();
        activation.observe(true);
        activation.observe(false);
        let deadline = activation.refit(
            Stage::Activate,
            deadline,
            now + Duration::from_secs(2),
        );
        assert!(
            !activation.take_request(now),
            "refitting must not steal focus back after an observed app switch"
        );
        assert!(
            activation.seen,
            "successful initial activation still permits inactive settlement"
        );
        let expired = now + Duration::from_secs(4);
        assert!(
            activation.refit(Stage::Activate, deadline, expired) <= expired,
            "repeated placement changes must not postpone transition failure"
        );
    }

    #[test]
    fn geometry_refit_keeps_an_activation_that_has_not_yet_run() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(3);
        let mut activation = Activation::default();
        activation.request();
        activation.observe(false);
        let refit_deadline = activation.refit(
            Stage::Activate,
            deadline,
            now + Duration::from_secs(2),
        );
        assert!(
            activation.take_request(now),
            "the requested initial activation must still run"
        );
        assert!(
            !activation.take_request(now),
            "refitting must not duplicate activation"
        );
        assert!(!activation.seen);
        assert_eq!(refit_deadline, deadline);
    }

    #[test]
    fn idle_geometry_refit_is_passive_with_a_fresh_placement_budget() {
        let now = Instant::now();
        let mut activation = Activation::default();
        activation.request();
        assert!(activation.take_request(now));
        activation.observe(true);
        let deadline = activation.refit(Stage::Idle, now, now);
        assert!(
            !activation.take_request(now),
            "passive refit must not activate a window"
        );
        assert!(activation.seen);
        assert!(
            deadline > now,
            "a new passive refit needs time to place the window"
        );
    }
}
