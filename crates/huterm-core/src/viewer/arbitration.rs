//! Owner-thread arbitration between one terminal's viewers: the controlling
//! viewer, aggregated focus reports, and mouse gesture ownership.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use huterm_protocol::{
    CellSize, GridSize, InputStamp, Modifiers, MouseAction, MouseButton,
    MouseInput, MousePosition, TerminalPresentation, ViewerId,
};

use super::{Registry, Slot};

/// Arbitration state one viewer reports, ordered with its input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Report {
    /// The viewer's view holds focus in an active window, or no longer does.
    Focus(bool),
    /// The grid and cell size the viewer would display.
    Geometry(GridSize, CellSize),
}

/// What the runtime must do after an arbitration change, in this order:
/// resize, apply presentation, write mouse releases, then report focus.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Effects {
    pub(crate) resize: Option<(GridSize, CellSize)>,
    pub(crate) presentation: Option<TerminalPresentation>,
    pub(crate) releases: Vec<MouseInput>,
    pub(crate) focus: Option<bool>,
}

impl Effects {
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub(crate) fn merge(&mut self, later: Self) {
        if later.resize.is_some() {
            self.resize = later.resize;
        }
        if later.presentation.is_some() {
            self.presentation = later.presentation;
        }
        self.releases.extend(later.releases);
        // Two focus changes in one batch cancel out or keep the later one.
        self.focus = match (self.focus, later.focus) {
            (Some(earlier), Some(later)) if earlier != later => None,
            (earlier, None) => earlier,
            (_, later) => later,
        };
    }
}

#[derive(Debug)]
struct Entry {
    slot: Arc<Slot>,
    geometry: Option<(GridSize, CellSize)>,
    focused: bool,
    presentation: Option<TerminalPresentation>,
    activity: u64,
}

/// A viewer that may control the terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Candidate {
    pub(crate) id: ViewerId,
    pub(crate) activity: u64,
    pub(crate) order: u64,
}

/// The `latest` policy: the candidate that most recently gained focus or
/// typed, ties going to the latest registration. Whether a viewer is shown
/// does not matter, as with tmux's `latest`.
pub(crate) fn select_controller(
    candidates: impl IntoIterator<Item = Candidate>,
) -> Option<ViewerId> {
    candidates
        .into_iter()
        .max_by_key(|candidate| (candidate.activity, candidate.order))
        .map(|candidate| candidate.id)
}

/// The owner thread's view of every live viewer.
#[derive(Debug)]
pub(crate) struct Arbiter {
    entries: Vec<Entry>,
    next_activity: u64,
    canonical: (GridSize, CellSize),
    geometry_revision: u64,
    presentation: TerminalPresentation,
    /// Focused viewers with the `input` capability.
    focused: usize,
    mouse: MouseArbiter,
}

impl Arbiter {
    pub(crate) fn new(
        grid: GridSize,
        cell: CellSize,
        presentation: TerminalPresentation,
    ) -> Self {
        Self {
            entries: Vec::new(),
            next_activity: 0,
            canonical: (grid, cell),
            geometry_revision: 0,
            presentation,
            focused: 0,
            mouse: MouseArbiter::default(),
        }
    }

    pub(crate) fn geometry_revision(&self) -> u64 {
        self.geometry_revision
    }

    fn index(&self, slot: &Slot) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.slot.id == slot.id)
    }

    /// Reconciles the table with the registry: adds new viewers with their
    /// initial state and finalizes revoked ones and dropped ones whose queued
    /// messages have run. A dropped viewer leaves control as soon as it is
    /// dropped, whether or not this has run.
    pub(crate) fn sync(&mut self, registry: &Registry) -> Effects {
        let mut effects = Effects::default();
        for slot in registry.slots() {
            if self.index(&slot).is_some() {
                continue;
            }
            if slot.is_revoked() {
                registry.remove(slot.id);
                continue;
            }
            let initial = slot.take_initial();
            let index = self.entries.len();
            self.entries.push(Entry {
                slot,
                geometry: None,
                focused: false,
                presentation: None,
                activity: 0,
            });
            if let Some(initial) = initial {
                let capabilities = self.entries[index].slot.capabilities;
                self.entries[index].geometry = initial.geometry;
                self.entries[index].presentation = initial.presentation;
                // Initial focus needs the same capabilities as a report.
                if initial.focused && (capabilities.input || capabilities.size)
                {
                    effects.merge(self.set_focus(index, true));
                }
            }
        }
        let mut index = 0;
        while index < self.entries.len() {
            let slot = Arc::clone(&self.entries[index].slot);
            if slot.is_revoked() || (slot.is_dropped() && slot.queued() == 0) {
                effects.merge(self.finalize(index, registry));
                continue;
            }
            index += 1;
        }
        effects.merge(self.recompute());
        effects
    }

    /// Removes a viewer: releases its mouse gesture, reports its focus
    /// loss, and frees its registry slot.
    fn finalize(&mut self, index: usize, registry: &Registry) -> Effects {
        let mut effects = self.set_focus(index, false);
        let entry = self.entries.remove(index);
        effects.releases.extend(self.mouse.finalize(entry.slot.id));
        registry.remove(entry.slot.id);
        effects
    }

    /// Finalizes a dropped viewer once its last counted message has run.
    pub(crate) fn settled(
        &mut self,
        slot: &Slot,
        registry: &Registry,
    ) -> Effects {
        let Some(index) = self.index(slot) else {
            return Effects::default();
        };
        if slot.is_dropped() && slot.queued() == 0 {
            let mut effects = self.finalize(index, registry);
            effects.merge(self.recompute());
            effects
        } else {
            Effects::default()
        }
    }

    fn touch(&mut self, index: usize) {
        self.next_activity += 1;
        let entry = &mut self.entries[index];
        entry.activity = self.next_activity;
        entry
            .slot
            .activity
            .store(self.next_activity, Ordering::Release);
    }

    fn set_focus(&mut self, index: usize, focused: bool) -> Effects {
        let mut effects = Effects::default();
        let entry = &mut self.entries[index];
        if entry.focused == focused {
            // A view's queue coalesces a blur and the refocus after it into
            // one gain, so a repeated gain is still new activity.
            if focused {
                self.touch(index);
            }
            return effects;
        }
        entry.focused = focused;
        if entry.slot.capabilities.input {
            if focused {
                self.focused += 1;
                if self.focused == 1 {
                    effects.focus = Some(true);
                }
            } else {
                self.focused -= 1;
                if self.focused == 0 {
                    effects.focus = Some(false);
                }
            }
        }
        if focused {
            self.touch(index);
        }
        effects
    }

    /// Applies one viewer's arbitration report. Reports from revoked or
    /// unknown viewers, and reports the viewer's capabilities do not
    /// permit, are ignored. A dropped viewer's queued reports still apply in
    /// order with its input, but it can no longer control.
    pub(crate) fn report(&mut self, slot: &Slot, report: Report) -> Effects {
        let Some(index) = self.index(slot) else {
            return Effects::default();
        };
        if slot.is_revoked() {
            return Effects::default();
        }
        let capabilities = slot.capabilities;
        let mut effects = match report {
            Report::Focus(focused)
                if capabilities.input || capabilities.size =>
            {
                self.set_focus(index, focused)
            }
            Report::Geometry(grid, cell) if capabilities.size => {
                self.entries[index].geometry = Some((grid, cell));
                Effects::default()
            }
            Report::Focus(_) | Report::Geometry(..) => {
                return Effects::default();
            }
        };
        effects.merge(self.recompute());
        effects
    }

    /// Records the presentation a viewer would apply.
    pub(crate) fn presentation(
        &mut self,
        slot: &Slot,
        presentation: TerminalPresentation,
    ) -> Effects {
        let Some(index) = self.index(slot) else {
            return Effects::default();
        };
        if slot.is_revoked() {
            return Effects::default();
        }
        self.entries[index].presentation = Some(presentation);
        self.recompute()
    }

    /// Records that a viewer typed: keys, characters, text, or paste.
    pub(crate) fn typed(&mut self, slot: &Slot) -> Effects {
        let Some(index) = self.index(slot) else {
            return Effects::default();
        };
        self.touch(index);
        self.recompute()
    }

    /// Whether a mouse report from this viewer reaches the application.
    pub(crate) fn admit_mouse(
        &mut self,
        slot: &Slot,
        mouse: &MouseInput,
        stamp: InputStamp,
    ) -> bool {
        self.mouse
            .admit(slot.id, mouse, stamp.geometry, self.geometry_revision)
    }

    /// Whether a viewer owns a gesture that still blocks other viewers.
    pub(crate) fn gesture_held(&self) -> bool {
        !self.mouse.held.is_empty()
    }

    /// Stops the buttons held now from blocking other viewers once the
    /// application has stopped tracking the mouse. A client that saw
    /// tracking stop never sends their releases, so other viewers' presses
    /// would otherwise stay refused; one that did not still sends them, and
    /// the owner's releases are still written.
    pub(crate) fn tracking_disabled(&mut self) {
        let mouse = &mut self.mouse;
        mouse.untracked.append(&mut mouse.held);
    }

    /// Chooses the controller and returns the resize and presentation
    /// changes its state requires.
    fn recompute(&mut self) -> Effects {
        let mut effects = Effects::default();
        let controller = select_controller(
            self.entries
                .iter()
                .filter(|entry| {
                    entry.slot.capabilities.size
                        && entry.geometry.is_some()
                        && !entry.slot.is_dropped()
                        && !entry.slot.is_revoked()
                })
                .map(|entry| Candidate {
                    id: entry.slot.id,
                    activity: entry.activity,
                    order: entry.slot.order,
                }),
        );
        let Some(entry) = controller.and_then(|id| {
            self.entries.iter().find(|entry| entry.slot.id == id)
        }) else {
            return effects;
        };
        if let Some(geometry) = entry.geometry
            && geometry != self.canonical
        {
            self.canonical = geometry;
            self.geometry_revision += 1;
            effects.resize = Some(geometry);
        }
        if let Some(presentation) = &entry.presentation
            && *presentation != self.presentation
        {
            self.presentation = presentation.clone();
            effects.presentation = Some(presentation.clone());
        }
        effects
    }
}

/// Mouse gesture ownership: the viewer that pressed first owns the gesture
/// until its last button is released.
#[derive(Debug, Default)]
struct MouseArbiter {
    owner: Option<ViewerId>,
    /// Buttons the owner holds that block other viewers.
    held: Vec<MouseButton>,
    /// Buttons the owner pressed before the application stopped tracking.
    /// Their releases are still the owner's, but they block no one, and
    /// another viewer's admitted press abandons them.
    untracked: Vec<MouseButton>,
    last: Option<(MousePosition, Modifiers)>,
}

impl MouseArbiter {
    fn admit(
        &mut self,
        viewer: ViewerId,
        mouse: &MouseInput,
        stamp: Option<u64>,
        geometry: u64,
    ) -> bool {
        let stale = stamp.is_some_and(|revision| revision != geometry);
        let owned = self.owner == Some(viewer);
        let foreign = !owned && !self.held.is_empty();
        let admitted = match mouse.action {
            MouseAction::Press(button) => {
                if foreign || stale {
                    return false;
                }
                // Only another viewer's admitted press abandons untracked
                // buttons; the owner's keeps them, so their releases still
                // arrive, and a refused press changes nothing.
                if !owned {
                    *self = Self::default();
                    self.owner = Some(viewer);
                }
                if !self.held.contains(&button) {
                    self.held.push(button);
                }
                true
            }
            // Releases match by button only and are never geometry-checked,
            // so an accepted gesture always ends.
            MouseAction::Release(button) => {
                if !owned
                    || !(remove(&mut self.held, button)
                        || remove(&mut self.untracked, button))
                {
                    return false;
                }
                if self.held.is_empty() && self.untracked.is_empty() {
                    *self = Self::default();
                }
                true
            }
            MouseAction::Motion(_) if owned => true,
            MouseAction::Motion(_) | MouseAction::Wheel(_) => {
                !foreign && !stale
            }
        };
        // Finalization releases held buttons where the owner last reported,
        // so other viewers' reports admitted beside an untracked gesture
        // never move that position.
        if admitted
            && self.owner == Some(viewer)
            && !matches!(mouse.action, MouseAction::Wheel(_))
        {
            self.last = Some((mouse.position, mouse.modifiers));
        }
        admitted
    }

    /// Releases the buttons a finalized viewer held, at its last position.
    fn finalize(&mut self, viewer: ViewerId) -> Vec<MouseInput> {
        if self.owner != Some(viewer) {
            return Vec::new();
        }
        let (position, modifiers) = self.last.unwrap_or_default();
        let ended = std::mem::take(self);
        ended
            .held
            .into_iter()
            .chain(ended.untracked)
            .map(|button| MouseInput {
                position,
                action: MouseAction::Release(button),
                modifiers,
            })
            .collect()
    }
}

/// Removes `button` from `buttons`, returning whether it was there.
fn remove(buttons: &mut Vec<MouseButton>, button: MouseButton) -> bool {
    let Some(index) = buttons.iter().position(|held| *held == button) else {
        return false;
    };
    buttons.remove(index);
    true
}

#[cfg(test)]
mod tests;
