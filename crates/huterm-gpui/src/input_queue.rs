use std::collections::VecDeque;

use huterm_core::{RefusedInput, RuntimeError};
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
    pub(super) fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }

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
        mut send: impl FnMut(TerminalInput) -> Result<(), RefusedInput>,
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
        let input = if self.inputs.is_empty() && !motion {
            match send(input) {
                Ok(()) => return Ok(Admission::Accepted),
                Err(RefusedInput {
                    error: RuntimeError::Busy,
                    input,
                }) => input,
                Err(refused) => {
                    *self = Self::default();
                    return Err(refused.error);
                }
            }
        } else {
            input
        };
        self.inputs.push_back(input);
        self.bytes += bytes;
        self.motion_run = motion;
        Ok(Admission::Accepted)
    }

    pub(super) fn retry(
        &mut self,
        mut send: impl FnMut(TerminalInput) -> Result<(), RefusedInput>,
    ) -> Result<(), RuntimeError> {
        while let Some(input) = self.inputs.pop_front() {
            let bytes = buffered_input_bytes(&input);
            match send(input) {
                Ok(()) => self.bytes -= bytes,
                Err(RefusedInput {
                    error: RuntimeError::Busy,
                    input,
                }) => {
                    self.inputs.push_front(input);
                    break;
                }
                Err(refused) => {
                    *self = Self::default();
                    return Err(refused.error);
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
        TerminalInput::Character { text, meta } => text
            .len()
            .saturating_add(usize::from(*meta && !text.is_empty())),
        _ => std::mem::size_of::<TerminalInput>(),
    }
}

#[cfg(test)]
mod tests;
