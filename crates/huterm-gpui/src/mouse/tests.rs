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
fn exited_terminal_returns_mouse_and_wheel_to_local_history() {
    assert!(!application_route(true, false, false, None, 0, 0));
    let mut mouse = MouseState::default();
    assert!(!mouse.down(MouseButton::Left, false));
    assert!(mouse.held(MouseButton::Left));
    assert!(
        mouse.cancel().is_empty(),
        "local selection emitted a PTY release"
    );
}

#[test]
fn hidden_tab_cancels_its_press_without_waiting_for_another_tabs_release() {
    let mut hidden = state();
    let mut active = state();
    let button = MouseButton::Left;
    assert!(hidden.down(button, true));
    hidden.accepted(button, MousePosition::default());
    let releases = hidden.cancel();
    hidden.forget_released_buttons();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].action, MouseAction::Release(button));
    assert!(
        active
            .release(button, MousePosition::default(), Modifiers::default())
            .is_none()
    );
    // Returning to the original tab must admit the next physical press.
    assert!(hidden.down(button, true));
    hidden.accepted(button, MousePosition::default());
    assert_eq!(hidden.cancel().len(), 1);
}

#[test]
fn macos_collapsed_right_presses_emit_one_release_in_either_order() {
    for releases in [
        [MouseButton::Left, MouseButton::Right],
        [MouseButton::Right, MouseButton::Left],
        [MouseButton::Right, MouseButton::Right],
    ] {
        let mut state = state();
        assert!(state.down(MouseButton::Right, true));
        state.accepted(MouseButton::Right, MousePosition::default());
        assert!(!state.down(MouseButton::Right, true));
        let mut reports = Vec::new();
        for released in releases {
            if let Some(button) = state.release_button(released, true)
                && let Some(report) = state.release(
                    button,
                    MousePosition::default(),
                    Modifiers::default(),
                )
            {
                reports.push(report.action);
            }
        }
        assert_eq!(reports, [MouseAction::Release(MouseButton::Right)]);
        assert!(state.cancel().is_empty());
        assert!(state.down(MouseButton::Right, true));
    }
    let mut local = state();
    assert!(!local.down(MouseButton::Left, false));
    assert_eq!(
        local.release_button(MouseButton::Right, true),
        Some(MouseButton::Left)
    );
}

#[test]
fn macos_release_pairs_control_click_across_modifier_changes() {
    for (pressed, released) in [
        (MouseButton::Right, MouseButton::Left),
        (MouseButton::Left, MouseButton::Right),
    ] {
        let mut state = state();
        state.down(pressed, true);
        state.accepted(pressed, MousePosition::default());
        assert_eq!(state.release_button(released, false), None);
        let button = state.release_button(released, true).unwrap();
        assert_eq!(button, pressed);
        assert_eq!(
            state
                .release(button, MousePosition::default(), Modifiers::default())
                .unwrap()
                .action,
            MouseAction::Release(pressed)
        );
        assert_eq!(state.release_button(released, true), None);
        assert!(state.cancel().is_empty());
        assert!(state.down(pressed, true));
    }
}

#[test]
fn macos_release_preserves_exact_multibutton_ownership_and_consumes_suppression()
 {
    let mut state = state();
    for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
        state.down(button, true);
        state.accepted(button, MousePosition::default());
    }
    assert_eq!(
        state.release_button(MouseButton::Right, true),
        Some(MouseButton::Right)
    );
    state.release(
        MouseButton::Right,
        MousePosition::default(),
        Modifiers::default(),
    );
    assert!(state.held(MouseButton::Left));
    assert!(state.held(MouseButton::Middle));
    assert_eq!(state.cancel().len(), 2);
    let button = state.release_button(MouseButton::Right, true).unwrap();
    assert_eq!(button, MouseButton::Left);
    assert!(
        state
            .release(button, MousePosition::default(), Modifiers::default())
            .is_none()
    );
    assert_eq!(state.release_button(MouseButton::Right, true), None);
    assert!(state.held(MouseButton::Middle));
    state.release(
        MouseButton::Middle,
        MousePosition::default(),
        Modifiers::default(),
    );
    state.down(MouseButton::Right, true);
    assert_eq!(state.release_button(MouseButton::Middle, true), None);
    assert!(
        state
            .release(
                state.release_button(MouseButton::Left, true).unwrap(),
                MousePosition::default(),
                Modifiers::default()
            )
            .is_none()
    );
    assert!(state.down(MouseButton::Right, true));
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
        Some(MouseTracking::Buttons),
        0,
        0
    ));
    for route in [
        application_route(
            false,
            false,
            false,
            Some(MouseTracking::Buttons),
            0,
            0,
        ),
        application_route(
            true,
            true,
            false,
            Some(MouseTracking::Buttons),
            0,
            0,
        ),
        application_route(
            true,
            false,
            true,
            Some(MouseTracking::Buttons),
            0,
            0,
        ),
        application_route(
            true,
            false,
            false,
            Some(MouseTracking::Disabled),
            0,
            0,
        ),
        application_route(
            true,
            false,
            false,
            Some(MouseTracking::Buttons),
            1,
            0,
        ),
        application_route(
            true,
            false,
            false,
            Some(MouseTracking::Buttons),
            0,
            1,
        ),
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
                .motion(MousePosition::default(), Modifiers::default(), true)
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
                .motion(MousePosition::default(), Modifiers::default(), true)
                .is_some()
        );
    }
}

#[test]
fn multibutton_motion_uses_latest_held_and_release_keeps_other_ownership() {
    let mut state = state();
    let position = MousePosition { column: 5, row: 7 };
    for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
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
