use super::{control_byte, protocol_modifiers};
use crate::config::MacosOptionAsAlt;
use crate::keymap::Platform;
use gpui::Keystroke;
use huterm_protocol::{TerminalInput, TerminalKey};

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
