use std::collections::VecDeque;

use huterm_core::RuntimeError;
use huterm_protocol::{MouseAction, TerminalInput};

pub(super) const PENDING_INPUT_CAPACITY: usize = 256;
pub(super) const PENDING_INPUT_BYTE_CAPACITY: usize = 1024 * 1024;

#[derive(Debug, Default)]
pub(super) struct InputQueue {
    closed: bool,
    inputs: VecDeque<TerminalInput>,
    bytes: usize,
    motion_run: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Admission {
    Accepted,
    Closed,
    Full,
}

impl InputQueue {
    pub(super) fn close(&mut self) {
        *self = Self {
            closed: true,
            ..Self::default()
        };
    }

    pub(super) fn boundary(&mut self) {
        self.motion_run = false;
    }

    pub(super) fn cancel_motion(&mut self) {
        if self.motion_run
            && matches!(self.inputs.back(), Some(TerminalInput::Mouse(mouse)) if matches!(mouse.action, MouseAction::Motion(_)))
            && let Some(input) = self.inputs.pop_back()
        {
            self.bytes -= buffered_input_bytes(&input);
        }
        self.boundary();
    }

    pub(super) fn enqueue(
        &mut self,
        input: TerminalInput,
        owned_release: bool,
        mut send: impl FnMut(TerminalInput) -> Result<(), RuntimeError>,
    ) -> Result<Admission, RuntimeError> {
        if self.closed {
            return Ok(Admission::Closed);
        }
        let motion = matches!(&input, TerminalInput::Mouse(mouse) if matches!(mouse.action, MouseAction::Motion(_)));
        if motion
            && self.motion_run
            && let (
                Some(TerminalInput::Mouse(previous)),
                TerminalInput::Mouse(next),
            ) = (self.inputs.back_mut(), &input)
            && previous.action == next.action
            && previous.modifiers == next.modifiers
        {
            *previous = *next;
            return Ok(Admission::Accepted);
        }
        self.boundary();
        let bytes = buffered_input_bytes(&input);
        if !owned_release
            && (self.inputs.len() >= PENDING_INPUT_CAPACITY
                || self.bytes.saturating_add(bytes)
                    > PENDING_INPUT_BYTE_CAPACITY)
        {
            return Ok(Admission::Full);
        }
        if self.inputs.is_empty() && !motion {
            match send(input.clone()) {
                Ok(()) => return Ok(Admission::Accepted),
                Err(RuntimeError::Busy) => {}
                Err(error) => {
                    *self = Self::default();
                    return Err(error);
                }
            }
        }
        self.inputs.push_back(input);
        self.bytes += bytes;
        self.motion_run = motion;
        Ok(Admission::Accepted)
    }

    pub(super) fn retry(
        &mut self,
        mut send: impl FnMut(TerminalInput) -> Result<(), RuntimeError>,
    ) -> Result<(), RuntimeError> {
        while let Some(input) = self.inputs.front().cloned() {
            match send(input) {
                Ok(()) => {
                    if let Some(sent) = self.inputs.pop_front() {
                        self.bytes -= buffered_input_bytes(&sent);
                    }
                }
                Err(RuntimeError::Busy) => break,
                Err(error) => {
                    *self = Self::default();
                    return Err(error);
                }
            }
        }
        if self.inputs.is_empty() {
            self.boundary();
        }
        Ok(())
    }
}

pub(super) fn buffered_input_bytes(input: &TerminalInput) -> usize {
    match input {
        TerminalInput::Text(text) | TerminalInput::Paste(text) => text.len(),
        _ => std::mem::size_of::<TerminalInput>(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mouse::MouseState;
    use huterm_protocol::{Modifiers, MouseButton, MouseInput, MousePosition};

    fn mouse(action: MouseAction, column: u32) -> TerminalInput {
        TerminalInput::Mouse(MouseInput {
            action,
            position: MousePosition { column, row: 0 },
            modifiers: Modifiers::default(),
        })
    }

    #[test]
    fn exit_discards_pending_input_and_rejects_all_later_terminal_input() {
        let mut queue = InputQueue::default();
        queue
            .enqueue(TerminalInput::Text("queued".into()), false, |_| {
                Err(RuntimeError::Busy)
            })
            .unwrap();
        queue.close();
        queue
            .retry(|_| panic!("exited input reached runtime"))
            .unwrap();
        for input in [
            TerminalInput::Text("key".into()),
            TerminalInput::Paste("paste".into()),
            TerminalInput::Focus(true),
            TerminalInput::Focus(false),
            mouse(MouseAction::Press(MouseButton::Left), 0),
            mouse(MouseAction::Release(MouseButton::Left), 0),
        ] {
            assert_eq!(
                queue
                    .enqueue(input, false, |_| panic!(
                        "exited input reached runtime"
                    ))
                    .unwrap(),
                Admission::Closed
            );
        }
        assert_eq!(queue.bytes, 0);
        assert!(queue.inputs.is_empty());
    }

    #[test]
    fn cancellation_progresses_without_paint_and_releases_precede_focus_out() {
        let mut queue = InputQueue::default();
        let mut state = MouseState::default();
        state.down(MouseButton::Left, true);
        let press = mouse(MouseAction::Press(MouseButton::Left), 0);
        queue
            .enqueue(press.clone(), false, |_| Err(RuntimeError::Busy))
            .unwrap();
        state.accepted(MouseButton::Left, MousePosition::default());
        queue
            .enqueue(
                mouse(MouseAction::Motion(Some(MouseButton::Left)), 1),
                false,
                |_| unreachable!(),
            )
            .unwrap();
        queue.cancel_motion();
        for release in state.cancel() {
            queue
                .enqueue(
                    TerminalInput::Mouse(release),
                    true,
                    |_| unreachable!(),
                )
                .unwrap();
        }
        assert!(state.cancel().is_empty());
        queue
            .enqueue(TerminalInput::Focus(false), false, |_| unreachable!())
            .unwrap();
        let mut sent = Vec::new();
        queue
            .retry(|input| {
                sent.push(input);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            sent,
            [
                press,
                mouse(MouseAction::Release(MouseButton::Left), 0),
                TerminalInput::Focus(false)
            ]
        );
    }

    #[test]
    fn wheel_saturation_discards_unadmitted_steps_without_reordering() {
        let mut queue = InputQueue::default();
        for _ in 0..PENDING_INPUT_CAPACITY - 2 {
            queue
                .enqueue(TerminalInput::Focus(true), false, |_| {
                    Err(RuntimeError::Busy)
                })
                .unwrap();
        }
        let mut state = MouseState::default();
        let wheel = state.wheel(20.0, 20.0);
        assert_eq!(wheel.len(), 32);
        let mut accepted = 0;
        for direction in wheel {
            accepted += usize::from(
                queue
                    .enqueue(
                        mouse(MouseAction::Wheel(direction), 0),
                        false,
                        |_| unreachable!(),
                    )
                    .unwrap()
                    == Admission::Accepted,
            );
        }
        assert_eq!(accepted, 2);
        assert_eq!(queue.inputs.len(), PENDING_INPUT_CAPACITY);
        assert_eq!(
            queue.inputs.back(),
            Some(&mouse(
                MouseAction::Wheel(huterm_protocol::WheelDirection::Up),
                0
            ))
        );
        assert!(state.wheel(0.0, 0.0).is_empty());
    }

    #[test]
    fn motion_coalesces_only_within_compatible_fifo_run() {
        let mut queue = InputQueue::default();
        let motion = |column| mouse(MouseAction::Motion(None), column);
        for column in 0..10_000 {
            assert_eq!(
                queue
                    .enqueue(motion(column), false, |_| panic!(
                        "motion must wait for refresh"
                    ))
                    .unwrap(),
                Admission::Accepted
            );
        }
        assert_eq!(queue.inputs.len(), 1);
        let keyboard = TerminalInput::Text("K".into());
        queue
            .enqueue(keyboard.clone(), false, |_| Err(RuntimeError::Busy))
            .unwrap();
        queue.enqueue(motion(4), false, |_| unreachable!()).unwrap();
        queue
            .enqueue(
                mouse(MouseAction::Press(MouseButton::Left), 4),
                false,
                |_| unreachable!(),
            )
            .unwrap();
        queue
            .enqueue(
                mouse(MouseAction::Motion(Some(MouseButton::Left)), 5),
                false,
                |_| unreachable!(),
            )
            .unwrap();
        let expected = [
            motion(9999),
            keyboard,
            motion(4),
            mouse(MouseAction::Press(MouseButton::Left), 4),
            mouse(MouseAction::Motion(Some(MouseButton::Left)), 5),
        ];
        let mut sent = Vec::new();
        queue
            .retry(|input| {
                sent.push(input);
                if sent.len() == 2 {
                    Err(RuntimeError::Busy)
                } else {
                    Ok(())
                }
            })
            .unwrap();
        assert_eq!(sent, expected[..2]);
        let mut retried = Vec::new();
        queue
            .retry(|input| {
                retried.push(input);
                Ok(())
            })
            .unwrap();
        assert_eq!(retried, expected[1..]);
        assert_eq!(queue.bytes, 0);
    }

    #[test]
    fn three_owned_releases_survive_event_saturation_without_allowing_new_cycles()
     {
        let mut queue = InputQueue::default();
        let mut state = MouseState::default();
        for button in
            [MouseButton::Left, MouseButton::Middle, MouseButton::Right]
        {
            state.down(button, true);
            assert_eq!(
                queue
                    .enqueue(
                        mouse(MouseAction::Press(button), 0),
                        false,
                        |_| Err(RuntimeError::Busy)
                    )
                    .unwrap(),
                Admission::Accepted
            );
            state.accepted(button, MousePosition::default());
        }
        while queue.inputs.len() < PENDING_INPUT_CAPACITY {
            queue
                .enqueue(TerminalInput::Focus(true), false, |_| unreachable!())
                .unwrap();
        }
        for release in state.cancel() {
            queue
                .enqueue(
                    TerminalInput::Mouse(release),
                    true,
                    |_| unreachable!(),
                )
                .unwrap();
        }
        assert_eq!(queue.inputs.len(), PENDING_INPUT_CAPACITY + 3);
        assert!(state.cancel().is_empty());
        for _ in 0..100 {
            state.release(
                MouseButton::Left,
                MousePosition::default(),
                Modifiers::default(),
            );
            state.down(MouseButton::Left, true);
            assert_eq!(
                queue
                    .enqueue(
                        mouse(MouseAction::Press(MouseButton::Left), 0),
                        false,
                        |_| unreachable!()
                    )
                    .unwrap(),
                Admission::Full
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
        }
        assert_eq!(queue.inputs.len(), PENDING_INPUT_CAPACITY + 3);
        let mut sent = Vec::new();
        queue
            .retry(|input| {
                sent.push(input);
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            sent[PENDING_INPUT_CAPACITY],
            TerminalInput::Mouse(MouseInput {
                action: MouseAction::Release(MouseButton::Left),
                ..
            })
        ));
    }

    #[test]
    fn byte_saturation_counts_overflow_and_disconnect_clears_everything() {
        let mut queue = InputQueue::default();
        let press = mouse(MouseAction::Press(MouseButton::Left), 0);
        let size = buffered_input_bytes(&press);
        queue
            .enqueue(press, false, |_| Err(RuntimeError::Busy))
            .unwrap();
        queue
            .enqueue(
                TerminalInput::Paste(
                    "p".repeat(PENDING_INPUT_BYTE_CAPACITY - size),
                ),
                false,
                |_| unreachable!(),
            )
            .unwrap();
        assert_eq!(queue.bytes, PENDING_INPUT_BYTE_CAPACITY);
        assert_eq!(
            queue
                .enqueue(
                    TerminalInput::Text("x".into()),
                    false,
                    |_| unreachable!()
                )
                .unwrap(),
            Admission::Full
        );
        queue
            .enqueue(
                mouse(MouseAction::Release(MouseButton::Left), 0),
                true,
                |_| unreachable!(),
            )
            .unwrap();
        assert_eq!(queue.bytes, PENDING_INPUT_BYTE_CAPACITY + size);
        assert_eq!(
            queue
                .enqueue(
                    TerminalInput::Text("x".into()),
                    false,
                    |_| unreachable!()
                )
                .unwrap(),
            Admission::Full
        );
        let error = queue.retry(|_| Err(RuntimeError::Busy));
        assert!(error.is_ok());
        assert_eq!(queue.inputs.len(), 3);
        assert!(matches!(
            queue.retry(|_| Err(RuntimeError::Stopped)),
            Err(RuntimeError::Stopped)
        ));
        assert!(queue.inputs.is_empty());
        assert_eq!(queue.bytes, 0);
        assert!(!queue.motion_run);
    }

    #[test]
    fn immediate_press_and_buffered_release_keep_acceptance_distinct() {
        let mut queue = InputQueue::default();
        assert_eq!(
            queue
                .enqueue(
                    mouse(MouseAction::Press(MouseButton::Left), 0),
                    false,
                    |_| Ok(())
                )
                .unwrap(),
            Admission::Accepted
        );
        assert!(queue.inputs.is_empty());
        assert_eq!(
            queue
                .enqueue(
                    mouse(MouseAction::Release(MouseButton::Left), 0),
                    true,
                    |_| Err(RuntimeError::Busy)
                )
                .unwrap(),
            Admission::Accepted
        );
        assert_eq!(queue.inputs.len(), 1);
    }
}
