use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};

use huterm_protocol::{AttachmentId, TerminalId, TerminalPresentation};

use crate::engine::TerminalEngine;
use crate::{RuntimeClient, RuntimeError};

#[derive(Debug)]
pub(crate) struct PresentationRegistration {
    generation: u64,
    attachment: AttachmentId,
    active: Mutex<bool>,
}

impl PresentationRegistration {
    pub(crate) fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
            && *self
                .active
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn revoke(&self) {
        *self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = false;
    }

    fn apply_if_current<T>(
        &self,
        generation: u64,
        apply: impl FnOnce() -> T,
    ) -> Option<T> {
        let active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (self.generation == generation && *active).then(apply)
    }
}

#[derive(Debug)]
pub(crate) struct PresentationUpdate {
    registration: Arc<PresentationRegistration>,
    generation: u64,
    presentation: TerminalPresentation,
}

impl PresentationUpdate {
    fn new(
        registration: Arc<PresentationRegistration>,
        generation: u64,
        presentation: TerminalPresentation,
    ) -> Self {
        Self {
            registration,
            generation,
            presentation,
        }
    }

    pub(crate) fn apply(
        self,
        engine: &mut TerminalEngine,
    ) -> Result<bool, RuntimeError> {
        self.registration
            .apply_if_current(self.generation, || {
                engine.update_presentation(self.presentation)
            })
            .transpose()
            .map(|applied| applied.is_some())
    }
}

/// Exclusive attachment authority for publishing one terminal's presentation.
///
/// The runtime validates this controller again when it applies each update.
/// Dropping, detaching, retargeting, or superseding the controller revokes
/// already queued updates while retaining the last state the runtime accepted.
#[derive(Debug)]
pub struct PresentationController {
    registration: Arc<PresentationRegistration>,
    client: RuntimeClient,
}

impl PresentationController {
    /// Publishes a coherent replacement for the retained presentation state.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Busy`] when the bounded runtime queue is full, or
    /// [`RuntimeError::Stopped`] after the terminal or this controller stops.
    pub fn update(
        &self,
        presentation: TerminalPresentation,
    ) -> Result<(), RuntimeError> {
        if !self.registration.is_current(self.registration.generation) {
            return Err(RuntimeError::Stopped);
        }
        self.client.update_presentation(PresentationUpdate::new(
            Arc::clone(&self.registration),
            self.registration.generation,
            presentation,
        ))
    }
}

#[cfg(test)]
mod tests {
    use huterm_protocol::{CellSize, GridSize};

    use super::*;

    #[test]
    fn queued_update_is_rejected_when_controller_is_revoked_before_apply() {
        let initial = TerminalPresentation::default();
        let mut engine = TerminalEngine::new(
            TerminalId::new(1),
            GridSize::clamped(80, 24),
            CellSize {
                width: 8,
                height: 16,
            },
            initial.clone(),
        )
        .unwrap();
        let registration = Arc::new(PresentationRegistration {
            generation: 1,
            attachment: AttachmentId::new(1),
            active: Mutex::new(true),
        });
        let mut changed = initial.clone();
        changed.foreground.red = 42;
        let queued =
            PresentationUpdate::new(Arc::clone(&registration), 1, changed);
        registration.revoke();

        assert!(!queued.apply(&mut engine).unwrap());
        assert_eq!(engine.presentation(), &initial);
    }
}

impl Drop for PresentationController {
    fn drop(&mut self) {
        self.registration.revoke();
    }
}

#[derive(Debug, Default)]
pub(crate) struct PresentationAuthority {
    next_generation: u64,
    registrations: BTreeMap<TerminalId, Weak<PresentationRegistration>>,
}

impl PresentationAuthority {
    pub(crate) fn authorize(
        &mut self,
        attachment: AttachmentId,
        terminal: TerminalId,
        client: RuntimeClient,
    ) -> Option<PresentationController> {
        self.next_generation = self.next_generation.checked_add(1)?;
        if let Some(previous) =
            self.registrations.get(&terminal).and_then(Weak::upgrade)
        {
            previous.revoke();
        }
        let registration = Arc::new(PresentationRegistration {
            generation: self.next_generation,
            attachment,
            active: Mutex::new(true),
        });
        self.registrations
            .insert(terminal, Arc::downgrade(&registration));
        Some(PresentationController {
            registration,
            client,
        })
    }

    pub(crate) fn invalidate_attachment(&mut self, attachment: AttachmentId) {
        self.invalidate_where(|registration| {
            registration.attachment == attachment
        });
    }

    pub(crate) fn invalidate_terminal(&mut self, terminal: TerminalId) {
        if let Some(registration) = self
            .registrations
            .remove(&terminal)
            .and_then(|weak| weak.upgrade())
        {
            registration.revoke();
        }
    }

    pub(crate) fn invalidate_all(&mut self) {
        self.invalidate_where(|_| true);
    }

    fn invalidate_where(
        &mut self,
        matches: impl Fn(&PresentationRegistration) -> bool,
    ) {
        self.registrations.retain(|_, weak| {
            let Some(registration) = weak.upgrade() else {
                return false;
            };
            if matches(&registration) {
                registration.revoke();
                false
            } else {
                true
            }
        });
    }
}
