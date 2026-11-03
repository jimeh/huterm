use super::{control_byte, protocol_modifiers};
use crate::config::MacosOptionAsAlt;
use crate::keymap::Platform;
use gpui::{KeyIdentity, Keystroke};
use huterm_protocol::{TerminalInput, TerminalKey};

/// Keystrokes already held by GPUI's matcher, awaiting consumption or replay.
#[cfg(any(target_os = "macos", test))]
#[derive(Default)]
pub(super) struct PendingShortcuts {
    strokes: std::collections::VecDeque<Keystroke>,
    identities: std::collections::VecDeque<Option<KeyIdentity>>,
    intercepted: Option<(Keystroke, Option<KeyIdentity>)>,
    action_observed: bool,
    resolution_captured: bool,
    bindings: Vec<gpui::KeyBinding>,
}

#[cfg(any(target_os = "macos", test))]
impl PendingShortcuts {
    pub(super) fn intercept(
        &mut self,
        stroke: &Keystroke,
        identity: Option<KeyIdentity>,
    ) {
        self.intercepted = Some((stroke.clone(), identity));
    }

    pub(super) fn replay_identity(
        &self,
        stroke: &Keystroke,
    ) -> Option<KeyIdentity> {
        (self.strokes.front() == Some(stroke))
            .then(|| self.identities.front().copied().flatten())
            .flatten()
    }

    pub(super) fn clear(&mut self) {
        self.strokes.clear();
        self.identities.clear();
        self.intercepted = None;
        self.action_observed = false;
        self.bindings.clear();
        self.resolution_captured = false;
    }

    pub(super) fn update(
        &mut self,
        pending: Option<&[Keystroke]>,
        bindings: Vec<gpui::KeyBinding>,
    ) {
        if let Some(pending) = pending {
            let mut identities = std::collections::VecDeque::new();
            for (index, stroke) in pending.iter().enumerate() {
                let identity = if self.strokes.get(index) == Some(stroke) {
                    self.identities.get(index).copied().flatten()
                } else {
                    self.intercepted
                        .as_ref()
                        .filter(|(key, _)| key == stroke)
                        .and_then(|(_, identity)| *identity)
                };
                identities.push_back(identity);
            }
            self.identities = identities;
            self.strokes = pending.iter().cloned().collect();
            self.bindings = bindings;
            self.action_observed = false;
            self.resolution_captured = false;
        } else if self.action_observed {
            self.clear();
        }
        // GPUI announces None before timeout replay, but after mismatch replay.
        // Keep unmatched strokes until on_key_down consumes their replay.
    }

    pub(super) fn resolution_strokes(&self) -> Option<Vec<Keystroke>> {
        (!self.resolution_captured && !self.strokes.is_empty())
            .then(|| self.strokes.iter().cloned().collect())
    }

    pub(super) fn capture_resolution(
        &mut self,
        bindings: Vec<gpui::KeyBinding>,
    ) {
        if !self.resolution_captured {
            self.bindings = bindings;
            self.resolution_captured = true;
        }
    }

    pub(super) fn observe_action(
        &mut self,
        stroke: &Keystroke,
        action: &dyn gpui::Action,
    ) {
        // GPUI collapses the longest matched fallback prefix to its final key.
        // Use its captured bindings and matcher, not the key's position alone:
        // a chord can contain that same key more than once.
        let held = self.strokes.make_contiguous();
        let consumed = self
            .bindings
            .iter()
            .filter(|binding| binding.action().partial_eq(action))
            .filter_map(|binding| {
                let prefix = held.get(..binding.keystrokes().len())?;
                (prefix.last() == Some(stroke)
                    && binding.match_keystrokes(prefix) == Some(false))
                .then_some(prefix.len())
            })
            .max();
        if let Some(consumed) = consumed {
            self.strokes.drain(..consumed);
            self.identities.drain(..consumed);
        } else {
            self.clear();
        }
        self.action_observed = true;
    }

    pub(super) fn consume_replay(&mut self, stroke: &Keystroke) -> bool {
        if self.strokes.front() == Some(stroke) {
            self.strokes.pop_front();
            self.identities.pop_front();
            true
        } else {
            self.clear();
            false
        }
    }
}

/// Which Option key a macOS key-down carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OptionSide {
    Left,
    Right,
}

/// `NX_DEVICELALTKEYMASK` and `NX_DEVICERALTKEYMASK`, the device-dependent
/// bits of an `NSEvent`'s modifier flags.
#[cfg(any(target_os = "macos", test))]
const LEFT_OPTION: usize = 0x20;
#[cfg(any(target_os = "macos", test))]
const RIGHT_OPTION: usize = 0x40;

/// The Option key a key-down's modifier `flags` name, or `None` when they
/// name neither or both. Remapping tools and Screen Sharing can omit the
/// device bits.
#[cfg(any(target_os = "macos", test))]
pub(super) fn option_side(flags: usize) -> Option<OptionSide> {
    match (flags & LEFT_OPTION != 0, flags & RIGHT_OPTION != 0) {
        (true, false) => Some(OptionSide::Left),
        (false, true) => Some(OptionSide::Right),
        _ => None,
    }
}

/// Whether a macOS keystroke with Alt `alt` held, from the Option key on
/// `side`, sends Meta under `policy`. `left` and `right` need the matching
/// side, so the other Option key still composes, and an unknown side sends
/// no Meta.
pub(super) fn option_sends_meta(
    policy: MacosOptionAsAlt,
    side: Option<OptionSide>,
    alt: bool,
) -> bool {
    alt && match policy {
        MacosOptionAsAlt::Off => false,
        MacosOptionAsAlt::Both => true,
        MacosOptionAsAlt::Left => side == Some(OptionSide::Left),
        MacosOptionAsAlt::Right => side == Some(OptionSide::Right),
    }
}

/// None defers printable macOS text to local or native composition.
/// `option_meta` is [`option_sends_meta`]'s decision for this keystroke;
/// Linux always sends Alt chords as Meta.
pub(super) fn translate(
    keystroke: &Keystroke,
    platform: Platform,
    option_meta: bool,
    native: Option<KeyIdentity>,
    kitty: bool,
) -> Option<TerminalInput> {
    if keystroke.modifiers.platform {
        return None;
    }
    let mut modifiers = protocol_modifiers(keystroke.modifiers);
    if let Some(identity) = native {
        modifiers.shift = identity.shift;
    }
    let key = match keystroke.key.as_str() {
        "enter" => Some(TerminalKey::Enter),
        "tab" => Some(TerminalKey::Tab),
        "backspace" => Some(TerminalKey::Backspace),
        "escape" => Some(TerminalKey::Escape),
        "up" => Some(TerminalKey::Up),
        "down" => Some(TerminalKey::Down),
        "left" => Some(TerminalKey::Left),
        "right" => Some(TerminalKey::Right),
        "home" => Some(TerminalKey::Home),
        "end" => Some(TerminalKey::End),
        "pageup" => Some(TerminalKey::PageUp),
        "pagedown" => Some(TerminalKey::PageDown),
        "delete" => Some(TerminalKey::Delete),
        "insert" => Some(TerminalKey::Insert),
        "f1" => Some(TerminalKey::F1),
        "f2" => Some(TerminalKey::F2),
        "f3" => Some(TerminalKey::F3),
        "f4" => Some(TerminalKey::F4),
        "f5" => Some(TerminalKey::F5),
        "f6" => Some(TerminalKey::F6),
        "f7" => Some(TerminalKey::F7),
        "f8" => Some(TerminalKey::F8),
        "f9" => Some(TerminalKey::F9),
        "f10" => Some(TerminalKey::F10),
        "f11" => Some(TerminalKey::F11),
        "f12" => Some(TerminalKey::F12),

        _ => None,
    };
    if let Some(key) = key {
        return Some(TerminalInput::Key { key, modifiers });
    }
    let meta =
        keystroke.modifiers.alt && (platform == Platform::Linux || option_meta);
    let identity = native.unwrap_or_else(|| fallback_identity(keystroke));
    let layout_text = identity.unshifted.map(|unshifted| {
        if identity.shift {
            identity.shifted.unwrap_or(unshifted)
        } else {
            unshifted
        }
        .to_string()
    });
    let text = if keystroke.modifiers.control {
        match control_byte(&keystroke.key) {
            Some(byte) => String::from(char::from(byte)),
            None if kitty => layout_text.clone()?,
            None => return None,
        }
    } else if meta && platform == Platform::MacOs {
        match keystroke.key.as_str() {
            "space" => " ".into(),
            key if key.chars().count() == 1 => {
                if keystroke.modifiers.shift {
                    key.to_ascii_uppercase()
                } else {
                    key.to_owned()
                }
            }
            _ => return None,
        }
    } else if platform == Platform::MacOs {
        return None;
    } else {
        keystroke.key_char.clone()?
    };
    modifiers.alt = meta;
    Some(TerminalInput::Character {
        text,
        meta,
        key: identity.unshifted.map(|unshifted| {
            Box::new(huterm_protocol::CharacterKey {
                unshifted,
                text: layout_text.unwrap_or_default(),
                base_layout: identity.base_layout,
                modifiers,
            })
        }),
    })
}

/// Synthetic and unpatched paths cannot recover shifted punctuation's base key.
fn fallback_identity(stroke: &Keystroke) -> KeyIdentity {
    let character = if stroke.key == "space" {
        Some(' ')
    } else {
        let mut chars = stroke.key.chars();
        let first = chars.next();
        if chars.next().is_none() { first } else { None }
    };
    KeyIdentity {
        unshifted: character,
        shifted: character.map(|ch| {
            if ch.is_ascii_lowercase() {
                ch.to_ascii_uppercase()
            } else {
                ch
            }
        }),
        shift: stroke.modifiers.shift,
        base_layout: character.filter(char::is_ascii),
    }
}

#[cfg(test)]
mod tests;
