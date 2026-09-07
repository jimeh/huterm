use huterm_protocol::{
    GridSize, MouseAction, MouseButton, MouseEncoding, MouseInput,
    MouseTracking, WheelDirection,
};
use huterm_protocol::{Modifiers, TerminalInput, TerminalKey, TerminalModes};

pub(crate) fn encode_input(
    input: &TerminalInput,
    modes: TerminalModes,
    size: GridSize,
) -> Vec<u8> {
    match input {
        TerminalInput::Character { text, meta } => {
            let mut bytes = Vec::with_capacity(text.len() + usize::from(*meta));
            if *meta && !text.is_empty() {
                bytes.push(0x1b);
            }
            bytes.extend_from_slice(text.as_bytes());
            bytes
        }
        TerminalInput::Mouse(mouse) => encode_mouse(*mouse, modes, size),
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

fn encode_mouse(
    mouse: MouseInput,
    modes: TerminalModes,
    size: GridSize,
) -> Vec<u8> {
    if modes.mouse_tracking == MouseTracking::Disabled
        || matches!(mouse.action, MouseAction::Motion(_))
            && (modes.mouse_tracking == MouseTracking::Buttons
                || mouse.action == MouseAction::Motion(None)
                    && modes.mouse_tracking != MouseTracking::AllMotion)
    {
        return Vec::new();
    }
    let button_code = |button| match button {
        MouseButton::Left => 0_u8,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let release = matches!(mouse.action, MouseAction::Release(_));
    let mut code = match mouse.action {
        MouseAction::Press(button) => button_code(button),
        MouseAction::Release(button)
            if modes.mouse_encoding == MouseEncoding::Sgr =>
        {
            button_code(button)
        }
        MouseAction::Release(_) => 3,
        MouseAction::Motion(button) => 32 + button.map_or(3, button_code),
        MouseAction::Wheel(direction) => match direction {
            WheelDirection::Up => 64,
            WheelDirection::Down => 65,
            WheelDirection::Left => 66,
            WheelDirection::Right => 67,
        },
    };
    code |= (u8::from(mouse.modifiers.shift) * 4)
        | (u8::from(mouse.modifiers.alt) * 8)
        | (u8::from(mouse.modifiers.control) * 16);
    let limit = match modes.mouse_encoding {
        MouseEncoding::Legacy => 223,
        MouseEncoding::Utf8 => 2015,
        MouseEncoding::Sgr => u32::MAX,
    };
    let coordinate = |value: u32, dimension: u16| {
        value
            .min(u32::from(dimension.saturating_sub(1)))
            .saturating_add(1)
            .min(limit)
    };
    let column = coordinate(mouse.position.column, size.columns);
    let row = coordinate(mouse.position.row, size.rows);
    if modes.mouse_encoding == MouseEncoding::Sgr {
        let terminator = if release { 'm' } else { 'M' };
        return format!("\x1b[<{code};{column};{row}{terminator}").into_bytes();
    }
    let mut bytes = vec![0x1b, b'[', b'M', code + 32];
    for coordinate in [column, row] {
        if modes.mouse_encoding == MouseEncoding::Utf8 {
            if let Some(character) = char::from_u32(coordinate + 32) {
                bytes.extend_from_slice(
                    character.encode_utf8(&mut [0; 4]).as_bytes(),
                );
            }
        } else {
            bytes.push(u8::try_from(coordinate + 32).unwrap_or(u8::MAX));
        }
    }
    bytes
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

    fn encode_input(input: &TerminalInput, modes: TerminalModes) -> Vec<u8> {
        super::encode_input(input, modes, GridSize::clamped(80, 24))
    }

    fn report(
        action: MouseAction,
        encoding: MouseEncoding,
        position: huterm_protocol::MousePosition,
        modifiers: Modifiers,
    ) -> Vec<u8> {
        super::encode_input(
            &TerminalInput::Mouse(MouseInput {
                action,
                position,
                modifiers,
            }),
            TerminalModes {
                mouse_tracking: MouseTracking::AllMotion,
                mouse_encoding: encoding,
                ..TerminalModes::default()
            },
            GridSize::clamped(4000, 4000),
        )
    }

    #[test]
    fn character_meta_adds_exactly_one_prefix_without_changing_text_or_paste() {
        for (text, meta, expected) in [
            ("r", true, "\x1br"),
            ("R", true, "\x1bR"),
            ("\x12", true, "\x1b\x12"),
            ("λ", true, "\x1bλ"),
            ("\x1b", true, "\x1b\x1b"),
            ("é", false, "é"),
            ("", true, ""),
        ] {
            assert_eq!(
                encode_input(
                    &TerminalInput::Character {
                        text: text.into(),
                        meta
                    },
                    TerminalModes::default()
                ),
                expected.as_bytes()
            );
        }
        for input in [
            TerminalInput::Text("®".into()),
            TerminalInput::Paste("®".into()),
        ] {
            assert_eq!(
                encode_input(&input, TerminalModes::default()),
                "®".as_bytes()
            );
        }
        assert_eq!(
            encode_input(
                &TerminalInput::Character {
                    text: "r".into(),
                    meta: true
                },
                TerminalModes {
                    bracketed_paste: true,
                    ..Default::default()
                }
            ),
            b"\x1br"
        );
    }

    #[test]
    fn mouse_formats_encode_all_buttons_motion_wheels_and_modifiers() {
        use huterm_protocol::MousePosition;
        let position = MousePosition { column: 1, row: 2 };
        let modifiers = Modifiers {
            control: true,
            alt: true,
            shift: true,
        };
        for (action, code, release) in [
            (MouseAction::Press(MouseButton::Left), 0, false),
            (MouseAction::Press(MouseButton::Middle), 1, false),
            (MouseAction::Press(MouseButton::Right), 2, false),
            (MouseAction::Release(MouseButton::Left), 0, true),
            (MouseAction::Release(MouseButton::Middle), 1, true),
            (MouseAction::Release(MouseButton::Right), 2, true),
            (MouseAction::Motion(Some(MouseButton::Left)), 32, false),
            (MouseAction::Motion(Some(MouseButton::Middle)), 33, false),
            (MouseAction::Motion(Some(MouseButton::Right)), 34, false),
            (MouseAction::Motion(None), 35, false),
            (MouseAction::Wheel(WheelDirection::Up), 64, false),
            (MouseAction::Wheel(WheelDirection::Down), 65, false),
            (MouseAction::Wheel(WheelDirection::Left), 66, false),
            (MouseAction::Wheel(WheelDirection::Right), 67, false),
        ] {
            let terminator = if release { 'm' } else { 'M' };
            assert_eq!(
                report(action, MouseEncoding::Sgr, position, modifiers),
                format!("\x1b[<{};2;3{terminator}", code + 28).as_bytes()
            );
            for encoding in [MouseEncoding::Legacy, MouseEncoding::Utf8] {
                let button = if release { 3 } else { code };
                assert_eq!(
                    report(action, encoding, position, modifiers),
                    [27, b'[', b'M', button + 28 + 32, 34, 35]
                );
            }
        }
    }

    #[test]
    fn mouse_coordinates_clamp_to_encoding_and_live_size_without_wrapping() {
        use huterm_protocol::MousePosition;
        for (encoding, limit) in
            [(MouseEncoding::Legacy, 223), (MouseEncoding::Utf8, 2015)]
        {
            for coordinate in [0, limit - 1, limit, u32::MAX] {
                let bytes = report(
                    MouseAction::Release(MouseButton::Right),
                    encoding,
                    MousePosition {
                        column: coordinate,
                        row: coordinate,
                    },
                    Modifiers::default(),
                );
                let wire = coordinate.saturating_add(1).min(limit) + 32;
                if encoding == MouseEncoding::Legacy {
                    assert_eq!(&bytes[4..], &[u8::try_from(wire).unwrap(); 2]);
                } else {
                    let character = char::from_u32(wire).unwrap();
                    assert_eq!(
                        &bytes[4..],
                        format!("{character}{character}").as_bytes()
                    );
                }
            }
        }
        for action in [
            MouseAction::Press(MouseButton::Left),
            MouseAction::Release(MouseButton::Left),
            MouseAction::Motion(Some(MouseButton::Left)),
            MouseAction::Wheel(WheelDirection::Up),
        ] {
            let mouse = TerminalInput::Mouse(MouseInput {
                action,
                position: MousePosition {
                    column: u32::MAX,
                    row: u32::MAX,
                },
                modifiers: Modifiers::default(),
            });
            for encoding in [
                MouseEncoding::Legacy,
                MouseEncoding::Utf8,
                MouseEncoding::Sgr,
            ] {
                let bytes = super::encode_input(
                    &mouse,
                    TerminalModes {
                        mouse_tracking: MouseTracking::AllMotion,
                        mouse_encoding: encoding,
                        ..TerminalModes::default()
                    },
                    GridSize::clamped(2, 1),
                );
                if encoding == MouseEncoding::Sgr {
                    assert!(String::from_utf8(bytes).unwrap().contains(";2;1"));
                } else {
                    assert_eq!(&bytes[4..], &[34, 33]);
                }
            }
        }
    }

    #[test]
    fn tracking_filters_motion_and_encoding_alone_never_enables_reports() {
        for tracking in [
            MouseTracking::Disabled,
            MouseTracking::Buttons,
            MouseTracking::ButtonMotion,
            MouseTracking::AllMotion,
        ] {
            for action in [
                MouseAction::Press(MouseButton::Left),
                MouseAction::Release(MouseButton::Left),
                MouseAction::Motion(None),
                MouseAction::Motion(Some(MouseButton::Left)),
                MouseAction::Wheel(WheelDirection::Down),
            ] {
                let encoded = encode_input(
                    &TerminalInput::Mouse(MouseInput {
                        action,
                        position: huterm_protocol::MousePosition::default(),
                        modifiers: Modifiers::default(),
                    }),
                    TerminalModes {
                        mouse_tracking: tracking,
                        mouse_encoding: MouseEncoding::Sgr,
                        ..TerminalModes::default()
                    },
                );
                let expected = tracking != MouseTracking::Disabled
                    && match action {
                        MouseAction::Motion(None) => {
                            tracking == MouseTracking::AllMotion
                        }
                        MouseAction::Motion(Some(_)) => {
                            tracking != MouseTracking::Buttons
                        }
                        _ => true,
                    };
                assert_eq!(
                    !encoded.is_empty(),
                    expected,
                    "{tracking:?} {action:?}"
                );
            }
        }
    }

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
