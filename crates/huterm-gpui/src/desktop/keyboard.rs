use super::{control_byte, protocol_modifiers};
use crate::config::MacosOptionAsAlt;
use crate::keymap::Platform;
use gpui::Keystroke;
use huterm_protocol::{TerminalInput, TerminalKey};

/// Keystrokes already held by GPUI's matcher, awaiting consumption or replay.
#[cfg(any(target_os = "macos", test))]
#[derive(Default)]
pub(super) struct PendingShortcuts {
    strokes: std::collections::VecDeque<Keystroke>,
    action_observed: bool,
    resolution_captured: bool,
    bindings: Vec<gpui::KeyBinding>,
}

#[cfg(any(target_os = "macos", test))]
impl PendingShortcuts {
    pub(super) fn clear(&mut self) {
        self.strokes.clear();
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
        } else {
            self.clear();
        }
        self.action_observed = true;
    }

    pub(super) fn consume_replay(&mut self, stroke: &Keystroke) -> bool {
        if self.strokes.front() == Some(stroke) {
            self.strokes.pop_front();
            true
        } else {
            self.clear();
            false
        }
    }
}

/// None defers printable macOS text to local or native composition.
pub(super) fn translate(
    keystroke: &Keystroke,
    platform: Platform,
    option: MacosOptionAsAlt,
) -> Option<TerminalInput> {
    let modifiers = protocol_modifiers(keystroke.modifiers);
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
        _ => None,
    };
    if let Some(key) = key {
        return Some(TerminalInput::Key { key, modifiers });
    }
    let meta = keystroke.modifiers.alt
        && (platform == Platform::Linux || option == MacosOptionAsAlt::Both);
    let text = if keystroke.modifiers.control {
        String::from(char::from(control_byte(&keystroke.key)?))
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
    Some(TerminalInput::Character { text, meta })
}

#[cfg(test)]
mod tests;
