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
    tracking: MouseTracking,
    displayed: usize,
    desired: usize,
) -> bool {
    in_grid
        && !scrollbar
        && !shift
        && tracking != MouseTracking::Disabled
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
mod tests {
    use super::*;

    fn state() -> MouseState {
        let mut state = MouseState::default();
        state.observe_modes(TerminalModes {
            mouse_tracking: MouseTracking::AllMotion,
            ..TerminalModes::default()
        });
        state
    }

    #[test]
    fn buttons_only_cleanup_uses_latest_pointer_even_without_motion_reports() {
        let mut state = state();
        state.observe_modes(TerminalModes {
            mouse_tracking: MouseTracking::Buttons,
            ..TerminalModes::default()
        });
        state.down(MouseButton::Left, true);
        state.accepted(MouseButton::Left, MousePosition::default());
        let position = MousePosition { column: 8, row: 4 };
        assert!(state.motion(position, Modifiers::default(), true).is_none());
        assert_eq!(state.cancel()[0].position, position);
    }

    #[test]
    fn routing_requires_live_grid_without_shift_or_scrollbar() {
        assert!(application_route(
            true,
            false,
            false,
            MouseTracking::Buttons,
            0,
            0
        ));
        for route in [
            application_route(
                false,
                false,
                false,
                MouseTracking::Buttons,
                0,
                0,
            ),
            application_route(true, true, false, MouseTracking::Buttons, 0, 0),
            application_route(true, false, true, MouseTracking::Buttons, 0, 0),
            application_route(
                true,
                false,
                false,
                MouseTracking::Disabled,
                0,
                0,
            ),
            application_route(true, false, false, MouseTracking::Buttons, 1, 0),
            application_route(true, false, false, MouseTracking::Buttons, 0, 1),
        ] {
            assert!(!route);
        }
    }

    #[test]
    fn rejected_and_local_presses_never_turn_into_application_drag_or_hover() {
        for application in [false, true] {
            let mut state = state();
            state.down(MouseButton::Left, application);
            assert!(
                state
                    .motion(
                        MousePosition::default(),
                        Modifiers::default(),
                        true
                    )
                    .is_none()
            );
            assert!(
                state
                    .release(
                        MouseButton::Left,
                        MousePosition::default(),
                        Modifiers::default()
                    )
                    .is_none()
            );
            assert!(
                state
                    .motion(
                        MousePosition::default(),
                        Modifiers::default(),
                        true
                    )
                    .is_some()
            );
        }
    }

    #[test]
    fn multibutton_motion_uses_latest_held_and_release_keeps_other_ownership() {
        let mut state = state();
        let position = MousePosition { column: 5, row: 7 };
        for button in
            [MouseButton::Left, MouseButton::Right, MouseButton::Middle]
        {
            assert!(state.down(button, true));
            state.accepted(button, position);
        }
        let shifted = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(
            state.motion(position, shifted, false).unwrap().action,
            MouseAction::Motion(Some(MouseButton::Middle))
        );
        assert!(
            state
                .release(MouseButton::Middle, position, shifted)
                .is_some()
        );
        assert_eq!(
            state.motion(position, shifted, false).unwrap().action,
            MouseAction::Motion(Some(MouseButton::Right))
        );
        let releases = state.cancel();
        assert_eq!(
            releases
                .iter()
                .map(|input| input.action)
                .collect::<Vec<_>>(),
            [
                MouseAction::Release(MouseButton::Left),
                MouseAction::Release(MouseButton::Right)
            ]
        );
        assert!(releases.iter().all(|input| input.position == position));
        assert!(state.cancel().is_empty());
        assert!(state.motion(position, shifted, true).is_none());
        for button in [MouseButton::Left, MouseButton::Right] {
            assert!(state.release(button, position, shifted).is_none());
        }
        assert!(state.motion(position, shifted, true).is_some());
    }

    #[test]
    fn observed_disable_requires_new_press_but_coalesced_transition_keeps_gesture()
     {
        let mut state = state();
        let position = MousePosition::default();
        state.down(MouseButton::Left, true);
        state.accepted(MouseButton::Left, position);
        let modes = state.modes;
        assert!(!state.observe_modes(modes));
        assert!(state.motion(position, Modifiers::default(), true).is_some());
        state.observe_modes(TerminalModes::default());
        state.observe_modes(modes);
        assert!(state.motion(position, Modifiers::default(), true).is_none());
        assert!(
            state
                .release(MouseButton::Left, position, Modifiers::default())
                .is_none()
        );
    }

    #[test]
    fn motion_deduplication_tracks_cells_buttons_and_held_modifiers() {
        let mut state = state();
        let position = MousePosition::default();
        let modifiers = Modifiers::default();
        assert!(state.motion(position, modifiers, true).is_some());
        assert!(
            state
                .motion(
                    position,
                    Modifiers {
                        alt: true,
                        ..modifiers
                    },
                    true
                )
                .is_none()
        );
        state.down(MouseButton::Left, true);
        state.accepted(MouseButton::Left, position);
        assert!(state.motion(position, modifiers, false).is_some());
        assert!(state.motion(position, modifiers, false).is_none());
        assert!(
            state
                .motion(
                    position,
                    Modifiers {
                        shift: true,
                        ..modifiers
                    },
                    false
                )
                .is_some()
        );
    }

    #[test]
    fn wheel_preserves_fraction_reverses_resets_and_caps_both_axes() {
        let mut state = state();
        assert!(state.wheel(0.4, 0.6).is_empty());
        assert_eq!(
            state.wheel(0.7, 0.5),
            [WheelDirection::Up, WheelDirection::Left]
        );
        assert!(state.wheel(0.0, -0.9).is_empty());
        assert_eq!(state.wheel(0.0, -0.2), [WheelDirection::Down]);
        let diagonal = state.wheel(-25.0, 20.0);
        assert_eq!(diagonal.len(), 32);
        assert!(
            diagonal[..20]
                .iter()
                .all(|direction| *direction == WheelDirection::Up)
        );
        assert!(
            diagonal[20..]
                .iter()
                .all(|direction| *direction == WheelDirection::Right)
        );
        assert!(state.wheel(0.0, 0.0).is_empty());
        for delta in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(state.wheel(delta, 1.0).is_empty());
        }
        assert_eq!(state.wheel(f64::MAX, f64::MAX).len(), 32);
        state.wheel(0.0, 0.8);
        state.wheel_route(true);
        assert!(state.wheel(0.0, 0.3).is_empty());
        state.cancel();
        assert!(state.wheel(0.0, 0.8).is_empty());
    }
}
