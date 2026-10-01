use huterm_protocol::{
    Modifiers, MouseAction, MouseButton, MouseInput, MousePosition,
    MouseTracking, TerminalModes, WheelDirection,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Ownership {
    #[default]
    Up,
    Local,
    Application,
    Suppressed,
}

#[derive(Debug, Default)]
pub(super) struct MouseState {
    buttons: [Ownership; 3],
    order: Vec<MouseButton>,
    last: MousePosition,
    motion: Option<MouseInput>,
    modes: TerminalModes,
    wheel: [f64; 2],
    wheel_application: bool,
}

pub(super) fn application_route(
    in_grid: bool,
    scrollbar: bool,
    shift: bool,
    tracking: Option<MouseTracking>,
    displayed: usize,
    desired: usize,
) -> bool {
    in_grid
        && !scrollbar
        && !shift
        && tracking.is_some_and(|tracking| tracking != MouseTracking::Disabled)
        && displayed == 0
        && desired == 0
}

fn index(button: MouseButton) -> usize {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

impl MouseState {
    pub(super) fn observe_modes(&mut self, modes: TerminalModes) -> bool {
        if (self.modes.mouse_tracking, self.modes.mouse_encoding)
            == (modes.mouse_tracking, modes.mouse_encoding)
        {
            return false;
        }
        self.modes = modes;
        self.boundary();
        if modes.mouse_tracking == MouseTracking::Disabled {
            for ownership in &mut self.buttons {
                if *ownership == Ownership::Application {
                    *ownership = Ownership::Suppressed;
                }
            }
            self.order.clear();
        }
        true
    }

    pub(super) fn boundary(&mut self) {
        self.motion = None;
        self.wheel = [0.0; 2];
    }

    pub(super) fn held(&self, button: MouseButton) -> bool {
        self.buttons[index(button)] != Ownership::Up
    }

    pub(super) fn down(
        &mut self,
        button: MouseButton,
        application: bool,
    ) -> bool {
        self.boundary();
        let ownership = &mut self.buttons[index(button)];
        if *ownership != Ownership::Up {
            return false;
        }
        *ownership = if application {
            Ownership::Suppressed
        } else {
            Ownership::Local
        };
        application
    }

    pub(super) fn accepted(
        &mut self,
        button: MouseButton,
        position: MousePosition,
    ) {
        self.buttons[index(button)] = Ownership::Application;
        self.order.push(button);
        self.last = position;
    }

    pub(super) fn release_button(
        &self,
        button: MouseButton,
        macos: bool,
    ) -> Option<MouseButton> {
        if self.held(button) {
            return Some(button);
        }
        // GPUI remaps macOS Control-left independently on down and up. It
        // exposes no physical-button identity for recovering a changed pair.
        let opposite = match button {
            MouseButton::Left => MouseButton::Right,
            MouseButton::Right => MouseButton::Left,
            MouseButton::Middle => return None,
        };
        (macos && self.held(opposite)).then_some(opposite)
    }

    pub(super) fn release(
        &mut self,
        button: MouseButton,
        position: MousePosition,
        modifiers: Modifiers,
    ) -> Option<MouseInput> {
        self.boundary();
        let ownership = std::mem::take(&mut self.buttons[index(button)]);
        if ownership == Ownership::Application {
            self.last = position;
        }
        self.order.retain(|held| *held != button);
        (ownership == Ownership::Application).then_some(MouseInput {
            position,
            modifiers,
            action: MouseAction::Release(button),
        })
    }

    pub(super) fn cancel(&mut self) -> Vec<MouseInput> {
        self.boundary();
        let releases = self
            .order
            .drain(..)
            .map(|button| MouseInput {
                position: self.last,
                action: MouseAction::Release(button),
                modifiers: Modifiers::default(),
            })
            .collect();
        for ownership in &mut self.buttons {
            if *ownership != Ownership::Up {
                *ownership = Ownership::Suppressed;
            }
        }
        releases
    }

    // Hidden tabs no longer receive the physical releases. Call after cancel
    // has emitted releases for every accepted application button.
    pub(super) fn forget_released_buttons(&mut self) {
        for ownership in &mut self.buttons {
            if *ownership != Ownership::Application {
                *ownership = Ownership::Up;
            }
        }
    }

    pub(super) fn motion(
        &mut self,
        position: MousePosition,
        modifiers: Modifiers,
        hover_allowed: bool,
    ) -> Option<MouseInput> {
        let button = self.order.last().copied();
        if button.is_none()
            && (!hover_allowed
                || self.buttons.iter().any(|held| *held != Ownership::Up))
        {
            self.motion = None;
            return None;
        }
        if button.is_some() {
            self.last = position;
        }
        if self.modes.mouse_tracking == MouseTracking::Disabled
            || self.modes.mouse_tracking == MouseTracking::Buttons
            || button.is_none()
                && self.modes.mouse_tracking != MouseTracking::AllMotion
        {
            return None;
        }
        let sample = MouseInput {
            position,
            modifiers,
            action: MouseAction::Motion(button),
        };
        self.last = position;
        let duplicate = self.motion.is_some_and(|previous| {
            if button.is_none() {
                previous.position == position
                    && previous.action == sample.action
            } else {
                previous == sample
            }
        });
        if duplicate {
            return None;
        }
        self.motion = Some(sample);
        Some(sample)
    }

    pub(super) fn wheel_route(&mut self, application: bool) -> bool {
        let changed = self.wheel_application != application;
        if changed {
            self.boundary();
        }
        self.wheel_application = application;
        changed
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "whole steps are bounded to 32 before conversion"
    )]
    pub(super) fn wheel(&mut self, x: f64, y: f64) -> Vec<WheelDirection> {
        self.motion = None;
        if !x.is_finite() || !y.is_finite() {
            return Vec::new();
        }
        let mut result = Vec::with_capacity(32);
        for (axis, delta, positive, negative) in [
            (0, y, WheelDirection::Up, WheelDirection::Down),
            (1, x, WheelDirection::Left, WheelDirection::Right),
        ] {
            if self.wheel[axis].signum() != delta.signum() && delta != 0.0 {
                self.wheel[axis] = 0.0;
            }
            let total = self.wheel[axis] + delta;
            self.wheel[axis] = total.fract();
            let count =
                total.abs().trunc().min((32 - result.len()) as f64) as usize;
            result.extend(std::iter::repeat_n(
                if total > 0.0 { positive } else { negative },
                count,
            ));
        }
        result
    }
}

#[cfg(test)]
mod tests;
