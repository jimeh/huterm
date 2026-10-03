use std::collections::VecDeque;

use huterm_core::{RefusedInput, RuntimeError};
use huterm_protocol::{InputStamp, MouseAction, TerminalInput};

pub(super) const PENDING_INPUT_CAPACITY: usize = 256;
pub(super) const PENDING_INPUT_BYTE_CAPACITY: usize = 1024 * 1024;

/// One request waiting for the runtime: input stamped when the user produced
/// it, or a focus change that keeps its place among that input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Queued {
    Input(TerminalInput, InputStamp),
    Focus(bool),
}

/// A request the runtime refused, returned unchanged for retry.
#[derive(Debug)]
pub(super) struct Refused {
    pub(super) error: RuntimeError,
    pub(super) queued: Queued,
}

impl Refused {
    /// Wraps refused input with the stamp it was sent with.
    pub(super) fn input(refused: RefusedInput, stamp: InputStamp) -> Self {
        Self {
            error: refused.error,
            queued: Queued::Input(refused.input, stamp),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct InputQueue {
    closed: bool,
    entries: VecDeque<Queued>,
    /// Queued input entries; focus changes do not count toward capacity.
    inputs: usize,
    bytes: usize,
    motion_run: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Admission {
    Accepted,
    Closed,
    Full,
}

fn is_motion(queued: &Queued) -> bool {
    matches!(
        queued,
        Queued::Input(TerminalInput::Mouse(mouse), _)
            if matches!(mouse.action, MouseAction::Motion(_))
    )
}

impl InputQueue {
    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Discards queued input after root exit. Focus changes keep flowing,
    /// including ones waiting for retry: the runtime still uses them to
    /// choose the controlling viewer. Closing again changes nothing.
    pub(super) fn close(&mut self) {
        if self.closed {
            return;
        }
        let mut entries = std::mem::take(&mut self.entries);
        entries.retain(|queued| matches!(queued, Queued::Focus(_)));
        *self = Self {
            closed: true,
            entries,
            ..Self::default()
        };
    }

    pub(super) fn boundary(&mut self) {
        self.motion_run = false;
    }

    pub(super) fn cancel_motion(&mut self) {
        if self.motion_run
            && self.entries.back().is_some_and(is_motion)
            && let Some(Queued::Input(input, _)) = self.entries.pop_back()
        {
            self.inputs -= 1;
            self.bytes -= buffered_input_bytes(&input);
        }
        self.boundary();
    }

    /// Queues a focus change behind earlier input. It replaces a pending
    /// focus change only when no input was queued between them, so the
    /// runtime still sees focus in, the keys typed while focused, and focus
    /// out in order. Focus changes are never refused for capacity or after
    /// exit.
    pub(super) fn enqueue_focus(
        &mut self,
        focused: bool,
        send: impl FnMut(Queued) -> Result<(), Refused>,
    ) -> Result<Admission, RuntimeError> {
        self.boundary();
        if let Some(Queued::Focus(pending)) = self.entries.back_mut() {
            *pending = focused;
            return Ok(Admission::Accepted);
        }
        self.push(Queued::Focus(focused), send)
    }

    pub(super) fn enqueue(
        &mut self,
        input: TerminalInput,
        stamp: InputStamp,
        owned_release: bool,
        send: impl FnMut(Queued) -> Result<(), Refused>,
    ) -> Result<Admission, RuntimeError> {
        if self.closed {
            return Ok(Admission::Closed);
        }
        let queued = Queued::Input(input, stamp);
        let motion = is_motion(&queued);
        if motion
            && self.motion_run
            && let (
                Some(Queued::Input(
                    TerminalInput::Mouse(previous),
                    previous_stamp,
                )),
                Queued::Input(TerminalInput::Mouse(next), next_stamp),
            ) = (self.entries.back_mut(), &queued)
            && previous.action == next.action
            && previous.modifiers == next.modifiers
        {
            // The newer position was computed against the newer grid, so
            // it keeps the newer stamp.
            *previous = *next;
            *previous_stamp = *next_stamp;
            return Ok(Admission::Accepted);
        }
        self.boundary();
        let Queued::Input(input, _) = &queued else {
            unreachable!("input was just queued");
        };
        let bytes = buffered_input_bytes(input);
        if !owned_release
            && (self.inputs >= PENDING_INPUT_CAPACITY
                || self.bytes.saturating_add(bytes)
                    > PENDING_INPUT_BYTE_CAPACITY)
        {
            return Ok(Admission::Full);
        }
        if motion {
            self.push_back(queued);
            self.motion_run = true;
            return Ok(Admission::Accepted);
        }
        self.push(queued, send)
    }

    /// Sends `queued` at once when nothing waits ahead of it, otherwise
    /// queues it behind the waiting requests.
    fn push(
        &mut self,
        queued: Queued,
        mut send: impl FnMut(Queued) -> Result<(), Refused>,
    ) -> Result<Admission, RuntimeError> {
        let queued = if self.entries.is_empty() {
            match send(queued) {
                Ok(()) => return Ok(Admission::Accepted),
                Err(Refused {
                    error: RuntimeError::Busy,
                    queued,
                }) => queued,
                Err(refused) => {
                    self.reset();
                    return Err(refused.error);
                }
            }
        } else {
            queued
        };
        self.push_back(queued);
        Ok(Admission::Accepted)
    }

    fn push_back(&mut self, queued: Queued) {
        if let Queued::Input(input, _) = &queued {
            self.inputs += 1;
            self.bytes += buffered_input_bytes(input);
        }
        self.entries.push_back(queued);
    }

    fn reset(&mut self) {
        *self = Self {
            closed: self.closed,
            ..Self::default()
        };
    }

    /// Sends waiting requests in order, stopping at the first the runtime
    /// cannot accept yet; nothing queued after it may overtake it.
    pub(super) fn retry(
        &mut self,
        mut send: impl FnMut(Queued) -> Result<(), Refused>,
    ) -> Result<(), RuntimeError> {
        while let Some(queued) = self.entries.pop_front() {
            let bytes = match &queued {
                Queued::Input(input, _) => Some(buffered_input_bytes(input)),
                Queued::Focus(_) => None,
            };
            match send(queued) {
                Ok(()) => {
                    if let Some(bytes) = bytes {
                        self.inputs -= 1;
                        self.bytes -= bytes;
                    }
                }
                Err(Refused {
                    error: RuntimeError::Busy,
                    queued,
                }) => {
                    self.entries.push_front(queued);
                    break;
                }
                Err(refused) => {
                    self.reset();
                    return Err(refused.error);
                }
            }
        }
        if self.entries.is_empty() {
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
