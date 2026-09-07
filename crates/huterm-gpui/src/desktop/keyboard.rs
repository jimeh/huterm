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
mod tests {
    use super::*;
    use crate::keymap;
    use gpui::{KeyContext, Modifiers as GpuiModifiers};
    use huterm_protocol::Modifiers;

    fn prefix(keys: &[&str]) -> Vec<Keystroke> {
        keys.iter()
            .map(|key| Keystroke::parse(key).unwrap())
            .collect()
    }

    fn reload_action() -> Box<dyn gpui::Action> {
        crate::commands::invoke(huterm_protocol::ids::RELOAD_CONFIG)
            .into_boxed()
    }

    fn fallback_bindings(chords: &[&str]) -> Vec<gpui::KeyBinding> {
        use crate::config::KeybindingEntry;
        let entries = chords
            .iter()
            .map(|chord| KeybindingEntry {
                key: (*chord).into(),
                command: "reload_config".into(),
                args: None,
                when: None,
                description: None,
            })
            .collect::<Vec<_>>();
        keymap::compile(Platform::MacOs, &entries).unwrap().bindings
    }

    #[test]
    fn resolution_captures_changed_context_before_actions_and_freezes_the_batch()
     {
        let held = prefix(&["alt-k", "alt-r"]);
        for timeout in [false, true] {
            let mut pending = PendingShortcuts::default();
            // The conditional fallback was inactive when the prefix started.
            pending.update(Some(&held), Vec::new());
            if timeout {
                pending.update(None, Vec::new());
            }
            // Selection now enables the fallback. Capture precedes its handler.
            assert_eq!(pending.resolution_strokes(), Some(held.clone()));
            pending.capture_resolution(fallback_bindings(&["alt-k"]));
            assert!(pending.resolution_strokes().is_none());
            // A handler can reload away that binding before later replay actions.
            pending.capture_resolution(Vec::new());
            pending.observe_action(&held[0], reload_action().as_ref());
            assert!(pending.consume_replay(&held[1]), "timeout={timeout}");
            pending.update(None, Vec::new());
            assert!(!pending.consume_replay(&held[0]));
        }
    }

    #[test]
    fn collapsed_fallback_actions_consume_the_whole_prefix_including_repeated_keys()
     {
        for (held_keys, short, long, final_index) in [
            (
                vec!["alt-j", "alt-f", "alt-d"],
                "alt-j alt-f",
                "alt-j alt-f alt-d alt-s",
                1,
            ),
            (
                vec!["alt-j", "alt-f", "alt-j", "alt-d"],
                "alt-j alt-f alt-j",
                "alt-j alt-f alt-j alt-d alt-s",
                2,
            ),
        ] {
            let held = prefix(&held_keys);
            for timeout in [false, true] {
                let mut pending = PendingShortcuts::default();
                pending.update(
                    Some(&held),
                    fallback_bindings(&["alt-j", short, long]),
                );
                if timeout {
                    pending.update(None, Vec::new());
                }
                pending.observe_action(
                    &held[final_index],
                    reload_action().as_ref(),
                );
                assert!(
                    pending.consume_replay(&held[final_index + 1]),
                    "{short}"
                );
                pending.update(None, Vec::new());
                assert!(!pending.consume_replay(&held[0]));
            }
        }
    }

    #[test]
    fn successive_fallback_actions_consume_only_their_own_prefixes() {
        use crate::config::KeybindingEntry;
        let held = prefix(&["alt-k", "alt-r", "alt-f", "alt-d"]);
        let mut bindings = fallback_bindings(&[
            "alt-k alt-r",
            "alt-k alt-r alt-f alt-d alt-s",
        ]);
        bindings.extend(
            keymap::compile(
                Platform::MacOs,
                &[KeybindingEntry {
                    key: "alt-f".into(),
                    command: "new_tab".into(),
                    args: None,
                    when: None,
                    description: None,
                }],
            )
            .unwrap()
            .bindings,
        );
        let mut pending = PendingShortcuts::default();
        pending.update(Some(&held), Vec::new());
        pending.update(None, Vec::new());
        pending.capture_resolution(bindings);
        pending.observe_action(&held[1], reload_action().as_ref());
        pending.capture_resolution(Vec::new());
        pending.observe_action(
            &held[2],
            crate::commands::invoke(huterm_protocol::ids::NEW_TAB)
                .into_boxed()
                .as_ref(),
        );
        assert!(pending.consume_replay(&held[3]));
        pending.update(None, Vec::new());
        assert!(!pending.consume_replay(&held[0]));
    }

    #[test]
    fn timeout_replays_are_consumed_before_native_text_insertion() {
        let held = prefix(&["alt-k"]);
        let mut pending = PendingShortcuts::default();
        pending.update(Some(&held), fallback_bindings(&["alt-k"]));
        pending.update(None, Vec::new());
        assert!(pending.consume_replay(&held[0]));
        assert!(!pending.consume_replay(&held[0]));
    }

    #[test]
    fn mismatch_replays_include_partial_chords_and_nonprinting_strokes() {
        let held = prefix(&["ctrl-k", "alt-f"]);
        assert!(held[0].key_char.is_none());
        let mut pending = PendingShortcuts::default();
        pending.update(Some(&held), fallback_bindings(&["alt-k"]));
        assert!(pending.consume_replay(&held[0]));
        assert!(pending.consume_replay(&held[1]));
        let ordinary = prefix(&["x"]);
        assert!(!pending.consume_replay(&ordinary[0]));
        pending.update(None, Vec::new());
        assert!(!pending.consume_replay(&held[0]));
        pending.update(Some(&held[..1]), fallback_bindings(&["alt-k"]));
        pending.update(None, Vec::new());
        assert!(pending.consume_replay(&held[0]));
    }

    #[test]
    fn accepted_actions_and_focus_cancellation_cannot_swallow_later_typing() {
        let held = prefix(&["alt-k", "alt-r"]);
        let mut pending = PendingShortcuts::default();
        pending.update(Some(&held), fallback_bindings(&["alt-k"]));
        // A full chord may end in the same stroke as its first key.
        pending.observe_action(&held[0], reload_action().as_ref());
        pending.update(None, Vec::new());
        assert!(!pending.consume_replay(&held[1]));
        pending.update(Some(&held), fallback_bindings(&["alt-k"]));
        pending.clear();
        assert!(!pending.consume_replay(&held[0]));
        assert!(!pending.consume_replay(&held[1]));
    }

    #[test]
    fn replayed_fallback_actions_preserve_suppression_of_remaining_strokes() {
        let held = prefix(&["alt-k", "alt-r"]);
        let mut pending = PendingShortcuts::default();
        for timeout in [false, true] {
            pending.update(Some(&held), fallback_bindings(&["alt-k"]));
            if timeout {
                pending.update(None, Vec::new());
            }
            pending.observe_action(&held[0], reload_action().as_ref());
            assert!(pending.consume_replay(&held[1]));
            pending.update(None, Vec::new());
            assert!(!pending.consume_replay(&held[0]));
        }
    }

    #[test]
    fn option_meta_uses_base_key_instead_of_composed_character() {
        let stroke = Keystroke {
            key: "r".into(),
            key_char: Some("®".into()),
            modifiers: GpuiModifiers {
                alt: true,
                ..Default::default()
            },
        };
        assert_eq!(
            translate(&stroke, Platform::MacOs, MacosOptionAsAlt::Both),
            Some(TerminalInput::Character {
                text: "r".into(),
                meta: true
            })
        );
    }
    fn stroke(
        key: &str,
        text: Option<&str>,
        alt: bool,
        shift: bool,
        control: bool,
    ) -> Keystroke {
        Keystroke {
            key: key.into(),
            key_char: text.map(str::to_owned),
            modifiers: GpuiModifiers {
                alt,
                shift,
                control,
                ..Default::default()
            },
        }
    }

    #[test]
    fn meta_characters_preserve_shift_punctuation_digits_space_and_control() {
        for (key, shifted, control, expected) in [
            ("r", false, false, "r"),
            ("r", true, false, "R"),
            ("3", false, false, "3"),
            ("<", false, false, "<"),
            ("?", false, false, "?"),
            ("space", false, false, " "),
            ("r", false, true, "\x12"),
            ("[", false, true, "\x1b"),
            ("space", false, true, "\0"),
            ("e", false, false, "e"),
            ("é", false, false, "é"),
        ] {
            let input =
                stroke(key, Some("option-composed"), true, shifted, control);
            assert_eq!(
                translate(&input, Platform::MacOs, MacosOptionAsAlt::Both),
                Some(TerminalInput::Character {
                    text: expected.into(),
                    meta: true
                }),
                "{key}"
            );
        }
    }

    #[test]
    fn macos_printable_text_is_deferred_to_local_or_native_composition() {
        for option in [MacosOptionAsAlt::Off, MacosOptionAsAlt::Both] {
            assert_eq!(
                translate(
                    &stroke("e", Some("e"), false, false, false),
                    Platform::MacOs,
                    option
                ),
                None
            );
        }
        for (key, text) in [("r", "®"), ("e", "´"), ("space", " ")] {
            assert_eq!(
                translate(
                    &stroke(key, Some(text), true, false, false),
                    Platform::MacOs,
                    MacosOptionAsAlt::Off
                ),
                None
            );
        }
    }

    #[test]
    fn linux_alt_uses_native_character_and_ignores_macos_option_setting() {
        for option in [MacosOptionAsAlt::Off, MacosOptionAsAlt::Both] {
            for (key, text, shifted) in [
                ("r", "r", false),
                ("r", "R", true),
                ("<", "<", false),
                ("l", "λ", false),
            ] {
                assert_eq!(
                    translate(
                        &stroke(key, Some(text), true, shifted, false),
                        Platform::Linux,
                        option
                    ),
                    Some(TerminalInput::Character {
                        text: text.into(),
                        meta: true
                    })
                );
            }
            assert_eq!(
                translate(
                    &stroke("r", None, true, false, true),
                    Platform::Linux,
                    option
                ),
                Some(TerminalInput::Character {
                    text: "\x12".into(),
                    meta: true
                })
            );
            assert_eq!(
                translate(
                    &stroke("e", Some("é"), false, false, false),
                    Platform::Linux,
                    option
                ),
                Some(TerminalInput::Character {
                    text: "é".into(),
                    meta: false
                })
            );
        }
    }

    #[test]
    fn special_keys_keep_alt_independent_of_option_policy() {
        for platform in [Platform::MacOs, Platform::Linux] {
            for option in [MacosOptionAsAlt::Off, MacosOptionAsAlt::Both] {
                assert_eq!(
                    translate(
                        &stroke("left", None, true, true, true),
                        platform,
                        option
                    ),
                    Some(TerminalInput::Key {
                        key: TerminalKey::Left,
                        modifiers: Modifiers {
                            alt: true,
                            shift: true,
                            control: true,
                        }
                    })
                );
                assert_eq!(
                    translate(
                        &stroke("f99", None, true, false, false),
                        platform,
                        option
                    ),
                    None
                );
                assert_eq!(
                    translate(
                        &stroke("3", None, true, false, true),
                        platform,
                        option
                    ),
                    None
                );
            }
        }
    }

    #[test]
    fn matcher_consumes_alt_bindings_and_allows_conditional_and_unbound_meta() {
        use crate::config::KeybindingEntry;
        use gpui::Keymap;
        let entry = |when: Option<&str>, command: &str| KeybindingEntry {
            key: "alt-r".into(),
            command: command.into(),
            args: None,
            when: when.map(str::to_owned),
            description: None,
        };
        let key = stroke("r", Some("®"), true, false, false);
        let context = [
            KeyContext::parse("Workspace").unwrap(),
            KeyContext::parse("Terminal").unwrap(),
        ];
        for (entries, consumed, reserved) in [
            (vec![entry(None, "reload_config")], true, true),
            (vec![entry(Some("Terminal"), "reload_config")], true, false),
            (
                vec![entry(Some("selection"), "reload_config")],
                false,
                false,
            ),
            (
                vec![entry(None, "reload_config"), entry(None, "unbind")],
                false,
                false,
            ),
            (vec![], false, false),
            (
                vec![KeybindingEntry {
                    key: "alt-k alt-r".into(),
                    ..entry(None, "reload_config")
                }],
                false,
                false,
            ),
            (
                vec![KeybindingEntry {
                    key: "alt-k alt-r alt-j".into(),
                    ..entry(None, "reload_config")
                }],
                false,
                false,
            ),
        ] {
            let compiled = keymap::compile(Platform::MacOs, &entries).unwrap();
            assert_eq!(compiled.reserved.is_reserved(&key), reserved);
            let matcher = Keymap::new(compiled.bindings);
            let (bindings, pending) = matcher
                .bindings_for_input(std::slice::from_ref(&key), &context);
            assert_eq!(!bindings.is_empty(), consumed);
            assert!(!pending);
            if !consumed {
                assert_eq!(
                    translate(&key, Platform::MacOs, MacosOptionAsAlt::Both),
                    Some(TerminalInput::Character {
                        text: "r".into(),
                        meta: true
                    })
                );
                assert_eq!(
                    translate(&key, Platform::MacOs, MacosOptionAsAlt::Off),
                    None
                );
            }
        }
    }
    #[test]
    fn real_matcher_reports_an_option_chord_prefix_for_composition_cancellation()
     {
        use crate::config::KeybindingEntry;
        use crate::keymap::{self, Platform};
        use gpui::{KeyContext, Keymap, Keystroke};
        let compiled = keymap::compile(
            Platform::MacOs,
            &[KeybindingEntry {
                key: "alt-b alt-r".into(),
                command: "reload_config".into(),
                args: None,
                when: None,
                description: None,
            }],
        )
        .unwrap();
        let matcher = Keymap::new(compiled.bindings);
        let (bindings, pending) = matcher.bindings_for_input(
            &[Keystroke::parse("alt-b").unwrap()],
            &[
                KeyContext::parse("Workspace").unwrap(),
                KeyContext::parse("Terminal").unwrap(),
            ],
        );
        assert!(bindings.is_empty());
        assert!(pending);
    }
}
