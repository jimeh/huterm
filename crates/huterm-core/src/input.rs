use huterm_protocol::{Modifiers, TerminalInput, TerminalKey, TerminalModes};

pub(crate) fn encode_input(
    input: &TerminalInput,
    modes: TerminalModes,
) -> Vec<u8> {
    match input {
        TerminalInput::Paste(text) if modes.bracketed_paste => {
            let mut bytes = b"\x1b[200~".to_vec();
            bytes.extend(text.replace('\x1b', "").as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
            bytes
        }
        TerminalInput::Paste(text) | TerminalInput::Text(text) => {
            text.as_bytes().to_vec()
        }
        TerminalInput::Focus(focused) if modes.focus_reporting => {
            if *focused {
                b"\x1b[I".to_vec()
            } else {
                b"\x1b[O".to_vec()
            }
        }
        TerminalInput::Key { key, modifiers } => {
            encode_key(*key, *modifiers, modes)
        }
        _ => Vec::new(),
    }
}

fn encode_key(
    key: TerminalKey,
    modifiers: Modifiers,
    modes: TerminalModes,
) -> Vec<u8> {
    let plain = match key {
        TerminalKey::Enter => b"\r".as_slice(),
        TerminalKey::Tab if modifiers.shift => b"\x1b[Z".as_slice(),
        TerminalKey::Tab => b"\t".as_slice(),
        TerminalKey::Backspace => b"\x7f".as_slice(),
        TerminalKey::Escape => b"\x1b".as_slice(),
        TerminalKey::Up if modes.application_cursor => b"\x1bOA".as_slice(),
        TerminalKey::Down if modes.application_cursor => b"\x1bOB".as_slice(),
        TerminalKey::Right if modes.application_cursor => b"\x1bOC".as_slice(),
        TerminalKey::Left if modes.application_cursor => b"\x1bOD".as_slice(),
        TerminalKey::Home if modes.application_cursor => b"\x1bOH".as_slice(),
        TerminalKey::End if modes.application_cursor => b"\x1bOF".as_slice(),
        TerminalKey::Up => b"\x1b[A".as_slice(),
        TerminalKey::Down => b"\x1b[B".as_slice(),
        TerminalKey::Right => b"\x1b[C".as_slice(),
        TerminalKey::Left => b"\x1b[D".as_slice(),
        TerminalKey::Home => b"\x1b[H".as_slice(),
        TerminalKey::End => b"\x1b[F".as_slice(),
        TerminalKey::PageUp => b"\x1b[5~".as_slice(),
        TerminalKey::PageDown => b"\x1b[6~".as_slice(),
        TerminalKey::Delete => b"\x1b[3~".as_slice(),
        _ => return Vec::new(),
    };

    if modifiers.alt && !matches!(key, TerminalKey::Escape) {
        let mut encoded = Vec::with_capacity(plain.len() + 1);
        encoded.push(0x1b);
        encoded.extend_from_slice(plain);
        encoded
    } else {
        plain.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_keys_should_follow_application_cursor_mode() {
        let encode = |key, application_cursor| {
            encode_input(
                &TerminalInput::Key {
                    key,
                    modifiers: Modifiers::default(),
                },
                TerminalModes {
                    application_cursor,
                    ..TerminalModes::default()
                },
            )
        };

        assert_eq!(encode(TerminalKey::Up, false), b"\x1b[A");
        assert_eq!(encode(TerminalKey::Up, true), b"\x1bOA");
        assert_eq!(encode(TerminalKey::Home, false), b"\x1b[H");
        assert_eq!(encode(TerminalKey::Home, true), b"\x1bOH");
        assert_eq!(encode(TerminalKey::End, false), b"\x1b[F");
        assert_eq!(encode(TerminalKey::End, true), b"\x1bOF");
    }

    #[test]
    fn control_text_should_encode_ascii_control_character() {
        let encoded = encode_input(
            &TerminalInput::Key {
                key: TerminalKey::Enter,
                modifiers: Modifiers {
                    control: true,
                    ..Modifiers::default()
                },
            },
            TerminalModes::default(),
        );

        assert_eq!(encoded, b"\r");
    }

    #[test]
    fn focus_should_only_encode_when_reporting_is_enabled() {
        let disabled =
            encode_input(&TerminalInput::Focus(true), TerminalModes::default());
        let focused = encode_input(
            &TerminalInput::Focus(true),
            TerminalModes {
                focus_reporting: true,
                ..TerminalModes::default()
            },
        );
        let blurred = encode_input(
            &TerminalInput::Focus(false),
            TerminalModes {
                focus_reporting: true,
                ..TerminalModes::default()
            },
        );

        assert_eq!(
            (disabled, focused, blurred),
            (vec![], b"\x1b[I".to_vec(), b"\x1b[O".to_vec())
        );
    }

    #[test]
    fn paste_should_use_bracketed_mode_and_strip_nested_escape() {
        let encoded = encode_input(
            &TerminalInput::Paste("hello\x1b[201~world".into()),
            TerminalModes {
                bracketed_paste: true,
                ..TerminalModes::default()
            },
        );

        assert_eq!(encoded, b"\x1b[200~hello[201~world\x1b[201~");
    }
}
