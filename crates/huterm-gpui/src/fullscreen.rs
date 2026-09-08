//! Window presentation policy. Platform effects never run in this reducer.

use std::time::{Duration, Instant};

use gpui::WindowBounds;

use crate::config::MacosFullscreenMode;

#[cfg(any(target_os = "macos", test))]
pub(crate) mod native_policy {
    use super::Operation;
    use gpui::{Bounds, point, size};
    use std::cell::Cell;
    const DOCK: usize = 0b0011;
    const MENU: usize = 0b1100;

    #[derive(Default)]
    pub(crate) struct OperationGate {
        generation: Cell<u64>,
        native_generation: Cell<u64>,
        closing: Cell<bool>,
        operation: Cell<Option<Operation>>,
    }

    impl OperationGate {
        pub fn reserve(&self, operation: Operation) {
            if self.closing.get()
                || operation.generation < self.generation.get()
            {
                return;
            }
            self.generation.set(operation.generation);
            self.operation.set(Some(operation));
        }
        pub fn generation(&self) -> u64 {
            self.generation.get()
        }
        pub fn is_current(&self, generation: u64) -> bool {
            !self.closing.get() && self.generation.get() == generation
        }
        pub fn valid(&self, operation: Operation) -> bool {
            self.is_current(operation.generation)
                && self.operation.get().is_some_and(|current| {
                    current.generation == operation.generation
                })
        }
        pub fn complete(&self, operation: Operation) {
            if self.valid(operation) {
                self.operation.set(None);
            }
        }
        pub fn cancel(&self, generation: u64) {
            self.generation.set(self.generation.get().max(generation));
            self.operation.set(None);
        }
        pub fn native_event(&self, will: bool) {
            self.native_generation.set(self.native_generation.get() + 1);
            if will {
                self.cancel(self.generation.get() + 1);
            }
        }
        pub fn native_generation(&self) -> u64 {
            self.native_generation.get()
        }
        pub fn native_is_current(&self, generation: u64) -> bool {
            !self.closing.get() && self.native_generation.get() == generation
        }
        pub fn close(&self) {
            self.closing.set(true);
            self.native_event(true);
        }
        pub fn closing(&self) -> bool {
            self.closing.get()
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub(crate) struct Display {
        pub id: u32,
        pub frame: Bounds<f64>,
        pub visible: Bounds<f64>,
    }

    pub(crate) fn restore_frame(
        saved: Bounds<f64>,
        original: Display,
        displays: &[Display],
        current: Option<u32>,
    ) -> Option<Bounds<f64>> {
        // `saved` includes the restored window's titlebar and frame borders.
        if let Some(display) =
            displays.iter().find(|display| display.id == original.id)
        {
            if display.frame == original.frame {
                return Some(saved);
            }
            let translated = Bounds::new(
                saved.origin + display.frame.origin - original.frame.origin,
                saved.size,
            );
            Some(clamp_frame(translated, display.visible))
        } else {
            let display = displays
                .iter()
                .find(|display| Some(display.id) == current)
                .or_else(|| displays.first())?;
            let fitted = size(
                saved.size.width.min(display.visible.size.width),
                saved.size.height.min(display.visible.size.height),
            );
            Some(Bounds::new(
                point(
                    display.visible.origin.x
                        + (display.visible.size.width - fitted.width) / 2.0,
                    display.visible.origin.y
                        + (display.visible.size.height - fitted.height) / 2.0,
                ),
                fitted,
            ))
        }
    }

    fn clamp_frame(frame: Bounds<f64>, visible: Bounds<f64>) -> Bounds<f64> {
        let fitted = size(
            frame.size.width.min(visible.size.width),
            frame.size.height.min(visible.size.height),
        );
        Bounds::new(
            point(
                frame.origin.x.clamp(
                    visible.origin.x,
                    visible.origin.x + visible.size.width - fitted.width,
                ),
                frame.origin.y.clamp(
                    visible.origin.y,
                    visible.origin.y + visible.size.height - fitted.height,
                ),
            ),
            fitted,
        )
    }

    pub(crate) fn screen_change_needs_recovery(
        entry_complete: bool,
        original: Display,
        current: Option<Display>,
        frame: Bounds<f64>,
        displays: &[Display],
    ) -> bool {
        entry_complete
            && (!displays.iter().any(|display| display.id == original.id)
                || current.is_none_or(|display| {
                    display.id != original.id || frame != display.frame
                }))
    }

    #[derive(Default)]
    pub(crate) struct Leases {
        owners: usize,
        added: usize,
    }

    #[derive(Default)]
    pub(crate) struct PresentationLease(bool);

    impl PresentationLease {
        pub fn acquire(
            &mut self,
            leases: &mut Leases,
            options: usize,
        ) -> Result<usize, &'static str> {
            if self.0 {
                return Ok(options);
            }
            let next = leases.acquire(options)?;
            self.0 = true;
            Ok(next)
        }
        pub fn release(
            &mut self,
            leases: &mut Leases,
            options: usize,
        ) -> Result<usize, &'static str> {
            if !self.0 {
                return Ok(options);
            }
            let next = leases.release(options)?;
            self.0 = false;
            Ok(next)
        }
        pub fn held(&self) -> bool {
            self.0
        }
    }
    impl Leases {
        pub fn acquire(
            &mut self,
            options: usize,
        ) -> Result<usize, &'static str> {
            let added = if self.owners == 0 {
                usize::from(options & DOCK == 0)
                    | (if options & MENU == 0 { 4 } else { 0 })
            } else {
                self.added
            };
            let next = options | added;
            if !valid_options(next) {
                return Err("invalid fullscreen presentation options");
            }
            self.owners += 1;
            self.added = added;
            Ok(next)
        }
        pub fn release(
            &mut self,
            options: usize,
        ) -> Result<usize, &'static str> {
            if self.owners == 0 {
                return Ok(options);
            }
            if !valid_options(options) {
                return Err("invalid current presentation options");
            }
            let mut next = if self.owners == 1 {
                options & !self.added
            } else {
                options
            };
            // Another application component may have added a menu option that
            // still requires our Dock option. Preserve that dependency.
            if next & MENU != 0 && next & DOCK == 0 {
                next |= options & DOCK;
            }
            if !valid_options(next) {
                return Err("cannot restore application presentation options");
            }
            self.owners -= 1;
            if self.owners == 0 {
                self.added = 0;
            }
            Ok(next)
        }
        #[cfg(test)]
        pub fn owners(&self) -> usize {
            self.owners
        }
    }
    pub(crate) fn valid_options(options: usize) -> bool {
        options & DOCK != DOCK
            && options & MENU != MENU
            && (options & MENU == 0 || options & DOCK != 0)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::fullscreen::Effect;
        #[test]
        fn individual_leases_release_once_across_close_rollback_and_ordinary_windows()
         {
            let mut leases = Leases::default();
            let mut first = PresentationLease::default();
            let mut second = PresentationLease::default();
            let mut ordinary = PresentationLease::default();
            let options = first.acquire(&mut leases, 0).unwrap();
            assert!(first.held());
            assert_eq!(first.acquire(&mut leases, options).unwrap(), options);
            let options = second.acquire(&mut leases, options).unwrap();
            assert_eq!(leases.owners(), 2);
            assert_eq!(
                ordinary.release(&mut leases, options).unwrap(),
                options
            );
            assert_eq!(first.release(&mut leases, options).unwrap(), options);
            assert_eq!(first.release(&mut leases, options).unwrap(), options);
            assert_eq!(leases.owners(), 1);
            assert!(second.release(&mut leases, 12).is_err());
            assert!(second.held());
            assert_eq!(second.release(&mut leases, options).unwrap(), 0);
            assert_eq!(leases.owners(), 0);
        }
        #[test]
        fn deferred_operations_are_canceled_before_native_mutation() {
            let op = Operation {
                generation: 1,
                effect: Effect::EnterNonNative,
                target: super::super::Mode::NonNative,
                deadline: std::time::Instant::now(),
            };
            let gate = OperationGate::default();
            gate.reserve(op);
            assert!(gate.valid(op));
            gate.native_event(true);
            assert!(!gate.valid(op));
            gate.reserve(op);
            assert!(!gate.valid(op));
            let retry = Operation {
                generation: 3,
                ..op
            };
            gate.reserve(retry);
            assert!(gate.valid(retry));
            gate.complete(retry);
            assert!(!gate.valid(retry));
            let exit = Operation {
                generation: 5,
                effect: Effect::ExitNonNative,
                ..op
            };
            gate.reserve(exit);
            assert!(gate.valid(exit));
            gate.complete(op);
            assert!(gate.valid(exit));
            gate.complete(exit);
            assert!(!gate.valid(exit));
            gate.reserve(Operation {
                generation: 6,
                ..op
            });
            gate.close();
            assert!(gate.closing());
            let late = Operation {
                generation: 100,
                ..op
            };
            gate.reserve(late);
            assert!(!gate.valid(late));
        }
        #[test]
        fn native_exit_reconciliation_is_invalidated_by_new_work_and_close() {
            let gate = OperationGate::default();
            gate.native_event(true);
            let exit = gate.generation();
            assert!(gate.is_current(exit));
            gate.native_event(true);
            assert!(!gate.is_current(exit));
            let next_exit = gate.generation();
            gate.cancel(next_exit + 1);
            assert!(!gate.is_current(next_exit));
            let canceled = gate.generation();
            gate.reserve(Operation {
                generation: canceled + 1,
                effect: Effect::EnterNonNative,
                target: super::super::Mode::NonNative,
                deadline: std::time::Instant::now(),
            });
            assert!(!gate.is_current(canceled));
            let closing = gate.generation();
            gate.close();
            assert!(!gate.is_current(closing));
            assert!(!gate.is_current(gate.generation()));
        }

        #[test]
        fn timeout_cancels_frame_mutation_but_preserves_late_native_observation()
         {
            let gate = OperationGate::default();
            gate.native_event(true);
            gate.native_event(false);
            let operation = gate.generation();
            let notification = gate.native_generation();
            gate.cancel(operation + 1);
            assert!(!gate.is_current(operation));
            assert!(gate.native_is_current(notification));
            gate.native_event(true);
            assert!(!gate.native_is_current(notification));
            let next = gate.native_generation();
            gate.native_event(false);
            assert!(!gate.native_is_current(next));
            let closing = gate.native_generation();
            gate.close();
            assert!(!gate.native_is_current(closing));
        }

        fn rect(x: f64, y: f64, w: f64, h: f64) -> Bounds<f64> {
            Bounds::new(point(x, y), size(w, h))
        }
        #[test]
        fn display_restore_preserves_translates_clamps_and_centers() {
            let original = Display {
                id: 1,
                frame: rect(-1000., 0., 1000., 800.),
                visible: rect(-1000., 0., 1000., 780.),
            };
            let saved = rect(-900., 100., 800., 600.);
            assert_eq!(
                restore_frame(saved, original, &[original], Some(1)),
                Some(saved)
            );
            let moved = Display {
                id: 1,
                frame: rect(0., 0., 700., 500.),
                visible: rect(0., 0., 700., 480.),
            };
            assert_eq!(
                restore_frame(saved, original, &[moved], Some(1)),
                Some(moved.visible)
            );
            let other = Display {
                id: 2,
                frame: rect(0., 0., 1400., 1000.),
                visible: rect(0., 0., 1400., 980.),
            };
            assert_eq!(
                restore_frame(saved, original, &[other], Some(2)),
                Some(rect(300., 190., 800., 600.))
            );
            assert!(!screen_change_needs_recovery(
                true,
                original,
                Some(original),
                original.frame,
                &[original]
            ));
            assert!(screen_change_needs_recovery(
                true,
                original,
                Some(moved),
                original.frame,
                &[moved]
            ));
            assert!(screen_change_needs_recovery(
                true,
                original,
                Some(other),
                other.frame,
                &[other]
            ));
        }

        #[test]
        fn screen_change_during_entry_defers_to_completion_validation() {
            let original = Display {
                id: 1,
                frame: rect(0., 0., 1000., 800.),
                visible: rect(0., 0., 1000., 780.),
            };
            let other = Display {
                id: 2,
                frame: rect(1000., 0., 1000., 800.),
                visible: rect(1000., 0., 1000., 780.),
            };
            assert!(!screen_change_needs_recovery(
                false,
                original,
                Some(other),
                rect(100., 100., 800., 600.),
                &[other]
            ));
        }

        #[test]
        fn moved_display_clamps_titled_frame_below_visible_top() {
            let original = Display {
                id: 1,
                frame: rect(-1000., 0., 1000., 800.),
                visible: rect(-1000., 0., 1000., 776.),
            };
            let moved = Display {
                frame: rect(0., 0., 1000., 800.),
                visible: rect(0., 0., 1000., 776.),
                ..original
            };
            // 600 content points plus a 28-point titlebar. Clamping content
            // first would put the expanded frame's top at 804 instead of 776.
            let saved_frame = rect(-900., 176., 800., 628.);
            let restored =
                restore_frame(saved_frame, original, &[moved], Some(1))
                    .unwrap();
            assert_eq!(restored, rect(100., 148., 800., 628.));
            assert_eq!(restored.size - size(0., 28.), size(800., 600.));
            assert_eq!(
                restore_frame(saved_frame, original, &[original], Some(1)),
                Some(saved_frame)
            );
        }

        #[test]
        fn missing_or_shrunk_display_fits_frame_including_titlebar() {
            let original = Display {
                id: 1,
                frame: rect(0., 0., 1000., 800.),
                visible: rect(0., 0., 1000., 776.),
            };
            let smaller = Display {
                id: 1,
                frame: rect(0., 0., 700., 500.),
                visible: rect(0., 0., 700., 476.),
            };
            let saved_frame = rect(100., 100., 800., 628.);
            let shrunk =
                restore_frame(saved_frame, original, &[smaller], Some(1))
                    .unwrap();
            assert_eq!(shrunk, smaller.visible);
            assert_eq!(shrunk.size - size(0., 28.), size(700., 448.));
            let replacement = Display { id: 2, ..smaller };
            assert_eq!(
                restore_frame(saved_frame, original, &[replacement], Some(2)),
                Some(replacement.visible)
            );
        }
        #[test]
        fn leases_preserve_preexisting_bits_until_final_owner() {
            for original in [0, 1, 2, 5, 6, 9, 10, 1 << 10] {
                let mut leases = Leases::default();
                let first = leases.acquire(original).unwrap();
                assert!(valid_options(first));
                let second = leases.acquire(first).unwrap();
                assert_eq!(leases.release(second).unwrap(), first);
                assert_eq!(leases.release(first).unwrap(), original);
                assert_eq!(leases.release(original).unwrap(), original);
                assert_eq!(leases.owners(), 0);
            }
        }
        #[test]
        fn failed_option_update_keeps_retryable_owner() {
            let mut leases = Leases::default();
            assert!(leases.acquire(12).is_err());
            assert_eq!(leases.owners(), 0);
            let options = leases.acquire(0).unwrap();
            assert!(leases.release(12).is_err());
            assert_eq!(leases.owners(), 1);
            assert_eq!(leases.release(options).unwrap(), 0);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Mode {
    Windowed,
    Native,
    NonNative,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum ToggleIntent {
    Default,
    Native,
    NonNative,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::enum_variant_names,
    reason = "effects name the exact platform operation"
)]
pub(crate) enum Effect {
    ToggleNative,
    EnterNonNative,
    ExitNonNative,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Operation {
    pub generation: u64,
    pub effect: Effect,
    target: Mode,
    deadline: Instant,
}

#[derive(Clone, Copy, Debug)]
#[cfg(any(target_os = "macos", test))]
pub(crate) enum NativeEvent {
    WillEnter,
    DidEnter,
    WillExit,
    DidExit,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "platform, chrome, recovery ownership and close gate are independent facts"
)]
pub(crate) struct FullscreenController {
    pub observed: Mode,
    pub chrome_hidden: bool,
    desired: Mode,
    pending: Option<Operation>,
    generation: u64,
    closing: bool,
    // Recovery ownership is independent of native mode, including external
    // native entry over a borderless window.
    pub recovery: bool,
    non_native_chrome: bool,
    recovery_blocked: bool,
    default: MacosFullscreenMode,
    bounds: WindowBounds,
    macos: bool,
    defer_next: bool,
}

impl FullscreenController {
    pub fn new(
        bounds: WindowBounds,
        default: MacosFullscreenMode,
        macos: bool,
    ) -> Self {
        Self {
            observed: Mode::Windowed,
            chrome_hidden: false,
            desired: Mode::Windowed,
            pending: None,
            generation: 0,
            closing: false,
            recovery: false,
            non_native_chrome: false,
            recovery_blocked: false,
            default,
            bounds,
            macos,
            defer_next: false,
        }
    }

    pub fn set_default(&mut self, default: MacosFullscreenMode) {
        self.default = default;
    }

    #[cfg(test)]
    pub fn toggle(&mut self, intent: ToggleIntent) -> Result<(), String> {
        self.toggle_checked(intent, || Ok(()))
    }

    pub fn toggle_checked(
        &mut self,
        intent: ToggleIntent,
        non_native_preflight: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        if self.closing {
            return Err("window is closing".to_owned());
        }
        if matches!(intent, ToggleIntent::NonNative) && !self.macos {
            return Err(
                "non-native fullscreen is only available on macOS".to_owned()
            );
        }
        let desired = if self.recovery_blocked {
            Mode::Windowed
        } else if self.desired == Mode::Windowed {
            match intent {
                ToggleIntent::NonNative => Mode::NonNative,
                ToggleIntent::Default
                    if self.macos
                        && self.default == MacosFullscreenMode::NonNative =>
                {
                    Mode::NonNative
                }
                _ => Mode::Native,
            }
        } else {
            Mode::Windowed
        };
        if self.recovery
            || self.observed == Mode::NonNative
            || desired == Mode::NonNative
            || self
                .pending
                .is_some_and(|op| op.effect != Effect::ToggleNative)
        {
            non_native_preflight()?;
        }
        self.recovery_blocked = false;
        self.desired = desired;
        Ok(())
    }

    pub fn next(&mut self, now: Instant) -> Option<Operation> {
        if self.closing || self.pending.is_some() {
            return None;
        }
        if std::mem::take(&mut self.defer_next) {
            return None;
        }
        let (effect, target) = match self.observed {
            Mode::Native if self.desired != Mode::Native => {
                (Effect::ToggleNative, Mode::Windowed)
            }
            Mode::NonNative if self.desired != Mode::NonNative => {
                (Effect::ExitNonNative, Mode::Windowed)
            }
            Mode::Windowed if self.recovery && !self.recovery_blocked => {
                (Effect::ExitNonNative, Mode::Windowed)
            }
            Mode::Windowed if self.desired == Mode::Native => {
                (Effect::ToggleNative, Mode::Native)
            }
            Mode::Windowed if self.desired == Mode::NonNative => {
                (Effect::EnterNonNative, Mode::NonNative)
            }
            _ => return None,
        };
        self.generation += 1;
        let operation = Operation {
            generation: self.generation,
            effect,
            target,
            deadline: now + Duration::from_secs(5),
        };
        self.pending = Some(operation);
        Some(operation)
    }

    #[cfg(any(target_os = "macos", test))]
    pub fn native_event(&mut self, event: NativeEvent, now: Instant) {
        if self.closing {
            return;
        }
        let target = match event {
            NativeEvent::WillEnter | NativeEvent::DidEnter => Mode::Native,
            _ => Mode::Windowed,
        };
        let will =
            matches!(event, NativeEvent::WillEnter | NativeEvent::WillExit);
        if will {
            if self.recovery {
                self.desired = Mode::Windowed;
            }
            if self.pending.is_none_or(|op| {
                op.effect != Effect::ToggleNative || op.target != target
            }) {
                self.generation += 1;
                if !self.recovery {
                    self.desired = target;
                }
                self.pending = Some(Operation {
                    generation: self.generation,
                    effect: Effect::ToggleNative,
                    target,
                    deadline: now + Duration::from_secs(5),
                });
            }
        } else {
            let expected = self.pending.is_some_and(|op| {
                op.effect == Effect::ToggleNative && op.target == target
            });
            self.observed = target;
            if expected {
                self.pending = None;
            } else if self.pending.is_none() {
                self.desired = if self.recovery {
                    Mode::Windowed
                } else {
                    target
                };
            }
            self.defer_next = true;
        }
    }

    pub fn sample(&mut self, native: bool, bounds: WindowBounds) {
        self.chrome_hidden = native || self.non_native_chrome;
        if !self.macos {
            self.observe_native_flag(native);
        }
        if !self.recovery && self.pending.is_none() {
            match bounds {
                WindowBounds::Windowed(_) | WindowBounds::Maximized(_)
                    if self.observed == Mode::Windowed =>
                {
                    self.bounds = bounds;
                }
                WindowBounds::Fullscreen(frame)
                    if self.observed == Mode::Native =>
                {
                    self.bounds = WindowBounds::Windowed(frame);
                }
                _ => {}
            }
        }
    }

    // X11, or macOS when observer installation failed. A working AppKit
    // observer instead owns transition completion through Will/Did events.
    pub fn observe_native_flag(&mut self, native: bool) {
        let mode = if native { Mode::Native } else { Mode::Windowed };
        if self.pending.is_some_and(|op| op.target == mode) {
            self.observed = mode;
            self.pending = None;
        } else if self.pending.is_none() && self.observed != mode {
            self.observed = mode;
            self.desired = mode;
        }
    }

    #[cfg(any(target_os = "macos", test))]
    pub fn complete(&mut self, generation: u64, recovery: bool) {
        if self.closing
            || self.pending.is_none_or(|op| op.generation != generation)
        {
            return;
        }
        if let Some(op) = self.pending.take() {
            self.observed = op.target;
        }
        self.recovery = recovery;
        self.non_native_chrome = recovery;
        self.chrome_hidden = self.observed != Mode::Windowed || recovery;
    }

    pub fn fail(&mut self, generation: u64) -> bool {
        if self.pending.is_none_or(|op| op.generation != generation) {
            return false;
        }
        self.recovery_blocked = self
            .pending
            .is_some_and(|op| op.effect == Effect::ExitNonNative);
        self.pending = None;
        self.generation += 1;
        self.desired = self.observed;
        true
    }

    pub fn expired(&mut self, now: Instant) -> Option<u64> {
        let op = self.pending.filter(|op| now >= op.deadline)?;
        self.fail(op.generation);
        Some(self.generation)
    }

    #[cfg(any(target_os = "macos", test))]
    pub fn recover(&mut self) {
        self.desired = Mode::Windowed;
    }
    #[cfg(any(target_os = "macos", test))]
    pub fn non_native_state(&mut self, recovery: bool, chrome: bool) {
        self.recovery = recovery;
        self.non_native_chrome = chrome;
    }
    pub fn restorable_bounds(&self) -> WindowBounds {
        self.bounds
    }
    pub fn fullscreen_context(&self) -> bool {
        self.observed != Mode::Windowed
    }
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub fn can_resize_window(&self) -> bool {
        !self.recovery
            && !self
                .pending
                .is_some_and(|op| op.effect != Effect::ToggleNative)
    }
    pub fn close(&mut self) {
        self.closing = true;
        self.generation += 1;
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn controller(macos: bool) -> FullscreenController {
        FullscreenController::new(
            WindowBounds::Windowed(gpui::Bounds::default()),
            MacosFullscreenMode::Native,
            macos,
        )
    }

    #[test]
    fn newer_native_entry_supersedes_an_exit_awaiting_frame_reconciliation() {
        let mut c = controller(true);
        let now = Instant::now();
        c.native_event(NativeEvent::WillEnter, now);
        c.native_event(NativeEvent::DidEnter, now);
        c.toggle(ToggleIntent::Native).unwrap();
        assert!(c.next(now).is_none());
        let exit = c.next(now).unwrap();
        c.native_event(NativeEvent::WillExit, now);
        // The adapter still holds DidExit while foreground reconciliation waits.
        c.native_event(NativeEvent::WillEnter, now);
        c.native_event(NativeEvent::DidEnter, now);
        assert!(!c.is_pending());
        assert_eq!(c.observed, Mode::Native);
        c.complete(exit.generation, false);
        assert_eq!(c.observed, Mode::Native);
        assert!(c.next(now).is_none());
        assert!(c.next(now).is_none());
    }

    #[test]
    fn command_after_queued_native_entry_exits_after_completion() {
        for completed in [false, true] {
            let mut c = controller(true);
            let now = Instant::now();
            // The command observes the inbox before accepting a new intent.
            c.native_event(NativeEvent::WillEnter, now);
            if completed {
                c.native_event(NativeEvent::DidEnter, now);
            }
            c.sample(completed, c.bounds);
            c.toggle(ToggleIntent::Native).unwrap();
            if !completed {
                assert!(c.next(now).is_none());
                c.native_event(NativeEvent::DidEnter, now);
            }
            assert!(c.next(now).is_none());
            let exit =
                c.next(now).expect("accepted command exits external entry");
            assert_eq!(
                (exit.effect, exit.target),
                (Effect::ToggleNative, Mode::Windowed)
            );
        }
    }

    #[test]
    fn command_after_new_linux_fullscreen_flag_exits() {
        let mut c = controller(false);
        c.sample(true, c.bounds);
        c.toggle(ToggleIntent::Native).unwrap();
        let exit = c
            .next(Instant::now())
            .expect("accepted command exits external entry");
        assert_eq!(
            (exit.effect, exit.target),
            (Effect::ToggleNative, Mode::Windowed)
        );
    }

    #[test]
    fn native_toggle_survives_unavailable_non_native_adapter() {
        let mut c = controller(true);
        let now = Instant::now();
        c.toggle_checked(
            ToggleIntent::Default,
            || Err("no adapter".to_owned()),
        )
        .unwrap();
        assert_eq!(c.next(now).unwrap().effect, Effect::ToggleNative);
        // Without an observer, GPUI's native flag still completes the request.
        c.observe_native_flag(true);
        assert_eq!(c.observed, Mode::Native);
        assert!(!c.is_pending());
        c.toggle_checked(ToggleIntent::NonNative, || {
            Err("no screen".to_owned())
        })
        .unwrap();
        assert_eq!(c.next(now).unwrap().target, Mode::Windowed);
        c.observe_native_flag(false);
        assert_eq!(c.observed, Mode::Windowed);
        c.toggle_checked(ToggleIntent::Native, || Err("no screen".to_owned()))
            .unwrap();
        assert_eq!(c.next(now).unwrap().target, Mode::Native);
    }

    #[test]
    fn non_native_preflight_failure_preserves_entry_and_exit_intent() {
        let mut c = controller(true);
        let now = Instant::now();
        c.set_default(MacosFullscreenMode::NonNative);
        for intent in [ToggleIntent::Default, ToggleIntent::NonNative] {
            assert!(
                c.toggle_checked(intent, || Err("no screen".to_owned()))
                    .is_err()
            );
            assert!(c.next(now).is_none());
            assert_eq!(c.desired, Mode::Windowed);
        }
        c.toggle(ToggleIntent::Default).unwrap();
        let entry = c.next(now).unwrap();
        assert!(
            c.toggle_checked(ToggleIntent::Native, || Err(
                "no screen".to_owned()
            ))
            .is_err()
        );
        assert_eq!(c.desired, Mode::NonNative);
        c.complete(entry.generation, true);
        assert!(
            c.toggle_checked(ToggleIntent::Native, || Err(
                "no screen".to_owned()
            ))
            .is_err()
        );
        assert!(c.next(now).is_none());
        c.toggle(ToggleIntent::Native).unwrap();
        let exit = c.next(now).unwrap();
        assert_eq!(exit.effect, Effect::ExitNonNative);
        c.fail(exit.generation);
        assert!(c.recovery_blocked);
        assert!(
            c.toggle_checked(ToggleIntent::Native, || Err(
                "no screen".to_owned()
            ))
            .is_err()
        );
        assert!(
            c.recovery_blocked,
            "refusal must not enable an automatic recovery retry"
        );
        assert!(c.next(now).is_none());
        c.toggle(ToggleIntent::Native).unwrap();
        assert_eq!(c.next(now).unwrap().effect, Effect::ExitNonNative);
    }

    #[test]
    fn fullscreen_context_tracks_completed_modes_through_entry_and_exit() {
        let now = Instant::now();
        let mut native = controller(true);
        assert!(!native.fullscreen_context());
        native.toggle(ToggleIntent::Native).unwrap();
        native.next(now).unwrap();
        native.native_event(NativeEvent::WillEnter, now);
        native.sample(true, native.bounds);
        assert!(!native.fullscreen_context());
        native.native_event(NativeEvent::DidEnter, now);
        assert!(native.fullscreen_context());
        native.toggle(ToggleIntent::Native).unwrap();
        native.next(now);
        native.next(now).unwrap();
        native.native_event(NativeEvent::WillExit, now);
        assert!(native.fullscreen_context());
        native.native_event(NativeEvent::DidExit, now);
        assert!(!native.fullscreen_context());

        let mut non_native = controller(true);
        non_native.toggle(ToggleIntent::NonNative).unwrap();
        let enter = non_native.next(now).unwrap();
        non_native.non_native_state(true, true);
        non_native.sample(false, non_native.bounds);
        assert!(!non_native.fullscreen_context());
        non_native.complete(enter.generation, true);
        assert!(non_native.fullscreen_context());
        non_native.toggle(ToggleIntent::Native).unwrap();
        let exit = non_native.next(now).unwrap();
        non_native.non_native_state(true, false);
        non_native.sample(false, non_native.bounds);
        assert!(non_native.fullscreen_context());
        non_native.complete(exit.generation, false);
        assert!(!non_native.fullscreen_context());
    }

    #[test]
    fn failed_entry_retains_recovery_then_retryable_rollback() {
        let mut c = controller(true);
        let now = Instant::now();
        c.toggle(ToggleIntent::NonNative).unwrap();
        let entry = c.next(now).unwrap();
        c.non_native_state(true, true);
        c.sample(false, c.bounds);
        assert!(c.chrome_hidden);
        assert!(!c.fullscreen_context());
        assert!(c.fail(entry.generation));
        c.recover();
        let rollback = c.next(now).unwrap();
        assert_eq!(rollback.effect, Effect::ExitNonNative);
        c.non_native_state(true, false);
        c.sample(false, c.bounds);
        assert!(!c.chrome_hidden);
        assert!(!c.can_resize_window());
        assert!(c.fail(rollback.generation));
        assert!(
            c.next(now).is_none(),
            "failed rollback must wait for a retry"
        );
        c.toggle(ToggleIntent::Native).unwrap();
        let retry = c.next(now).unwrap();
        c.complete(rollback.generation, false);
        assert!(c.recovery, "stale completion cannot discard recovery");
        c.complete(retry.generation, false);
        assert!(!c.recovery);
        assert!(c.can_resize_window());
    }

    #[test]
    fn two_linux_toggles_finish_entry_before_exit() {
        let mut c = controller(false);
        let now = Instant::now();
        c.toggle(ToggleIntent::Default).unwrap();
        c.sample(false, c.bounds);
        c.next(now).unwrap();
        c.toggle(ToggleIntent::Native).unwrap();
        c.sample(false, c.bounds);
        assert!(c.next(now).is_none());
        c.sample(true, c.bounds);
        c.sample(true, c.bounds);
        assert_eq!(c.next(now).unwrap().effect, Effect::ToggleNative);
        c.sample(false, c.bounds);
        assert_eq!(c.observed, Mode::Windowed);
        assert!(c.next(now).is_none());
    }

    #[test]
    fn maximized_windowed_metadata_survives_fullscreen_and_quit_capture() {
        let mut c = controller(false);
        let maximized = WindowBounds::Maximized(gpui::Bounds::new(
            gpui::point(gpui::px(25.), gpui::px(40.)),
            gpui::size(gpui::px(1200.), gpui::px(760.)),
        ));
        c.sample(false, maximized);
        assert_eq!(c.restorable_bounds(), maximized);
        c.toggle(ToggleIntent::Native).unwrap();
        c.next(Instant::now()).unwrap();
        c.sample(true, WindowBounds::Windowed(gpui::Bounds::default()));
        assert_eq!(c.restorable_bounds(), maximized);
        c.toggle(ToggleIntent::Native).unwrap();
        c.next(Instant::now()).unwrap();
        c.sample(false, maximized);
        assert_eq!(c.restorable_bounds(), maximized);
    }

    #[test]
    fn external_native_will_takes_over_pending_non_native_entry() {
        let mut c = controller(true);
        let now = Instant::now();
        c.toggle(ToggleIntent::NonNative).unwrap();
        let entry = c.next(now).unwrap();
        c.non_native_state(true, true);
        c.native_event(NativeEvent::WillEnter, now);
        assert!(!c.fail(entry.generation));
        assert!(c.next(now).is_none());
        c.native_event(NativeEvent::DidEnter, now);
        assert!(c.next(now).is_none());
        assert_eq!(c.next(now).unwrap().effect, Effect::ToggleNative);
    }

    #[test]
    fn default_reload_does_not_retarget_pending_transition() {
        let mut c = controller(true);
        let now = Instant::now();
        c.toggle(ToggleIntent::Default).unwrap();
        c.next(now).unwrap();
        c.set_default(MacosFullscreenMode::NonNative);
        c.native_event(NativeEvent::DidEnter, now);
        assert_eq!(c.desired, Mode::Native);
        c.toggle(ToggleIntent::Default).unwrap();
        c.next(now);
        c.next(now);
        c.native_event(NativeEvent::DidExit, now);
        c.next(now);
        c.toggle(ToggleIntent::Default).unwrap();
        assert_eq!(c.next(now).unwrap().effect, Effect::EnterNonNative);
    }
    #[test]
    fn all_modes_and_intents_toggle_out_of_fullscreen() {
        for mode in [Mode::Windowed, Mode::Native, Mode::NonNative] {
            for intent in [
                ToggleIntent::Default,
                ToggleIntent::Native,
                ToggleIntent::NonNative,
            ] {
                let mut c = controller(true);
                c.observed = mode;
                c.desired = mode;
                c.toggle(intent).unwrap();
                assert_eq!(
                    c.desired,
                    if mode != Mode::Windowed {
                        Mode::Windowed
                    } else if matches!(intent, ToggleIntent::NonNative) {
                        Mode::NonNative
                    } else {
                        Mode::Native
                    }
                );
            }
        }
    }
    #[test]
    fn rapid_toggles_wait_for_did_then_another_turn() {
        let mut c = controller(true);
        let now = Instant::now();
        c.toggle(ToggleIntent::Native).unwrap();
        c.next(now).unwrap();
        c.native_event(NativeEvent::WillEnter, now);
        c.toggle(ToggleIntent::Native).unwrap();
        c.toggle(ToggleIntent::NonNative).unwrap();
        c.sample(true, c.bounds);
        assert!(!c.fullscreen_context());
        assert!(c.chrome_hidden);
        assert!(c.next(now).is_none());
        c.native_event(NativeEvent::DidEnter, now);
        assert!(c.next(now).is_none());
        assert_eq!(c.next(now).unwrap().effect, Effect::ToggleNative);
        c.native_event(NativeEvent::WillExit, now);
        c.native_event(NativeEvent::DidExit, now);
        assert!(c.next(now).is_none());
        assert_eq!(c.next(now).unwrap().effect, Effect::EnterNonNative);
    }
    #[test]
    fn timeout_reports_once_and_late_completion_is_external() {
        let mut c = controller(false);
        let now = Instant::now();
        c.toggle(ToggleIntent::Default).unwrap();
        c.next(now).unwrap();
        assert!(c.expired(now + Duration::from_secs(5)).is_some());
        assert!(c.expired(now + Duration::from_secs(6)).is_none());
        c.sample(true, c.bounds);
        assert_eq!(c.observed, Mode::Native);
        assert!(c.next(now).is_none());
    }
    #[test]
    fn linux_ignores_portable_default_and_rejects_explicit_non_native() {
        let mut c = controller(false);
        c.set_default(MacosFullscreenMode::NonNative);
        assert!(c.toggle(ToggleIntent::NonNative).is_err());
        c.toggle(ToggleIntent::Default).unwrap();
        c.sample(false, c.bounds);
        assert_eq!(
            c.next(Instant::now()).unwrap().effect,
            Effect::ToggleNative
        );
    }
    #[test]
    fn external_native_keeps_recovery_and_restore_bounds() {
        let mut c = controller(true);
        let original = c.bounds;
        c.recovery = true;
        let now = Instant::now();
        c.native_event(NativeEvent::WillEnter, now);
        c.native_event(NativeEvent::DidEnter, now);
        c.sample(
            true,
            WindowBounds::Fullscreen(gpui::Bounds::new(
                gpui::point(gpui::px(10.), gpui::px(10.)),
                gpui::size(gpui::px(100.), gpui::px(100.)),
            )),
        );
        assert_eq!(c.restorable_bounds(), original);
        assert!(!c.can_resize_window());
        assert!(c.next(now).is_none());
        assert_eq!(c.next(now).unwrap().effect, Effect::ToggleNative);
    }
    #[test]
    fn close_and_failure_reject_stale_completion() {
        let mut c = controller(true);
        c.toggle(ToggleIntent::NonNative).unwrap();
        let op = c.next(Instant::now()).unwrap();
        assert!(c.fail(op.generation));
        c.complete(op.generation, true);
        assert_eq!(c.observed, Mode::Windowed);
        c.close();
        assert!(c.toggle(ToggleIntent::Default).is_err());
        assert!(c.next(Instant::now()).is_none());
    }
}
