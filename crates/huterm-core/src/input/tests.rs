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
        // Larger than the encodings' limits in both dimensions, which
        // MAX_GRID_CELLS rules out for a real grid, so the encoding limit
        // alone decides the coordinates.
        GridSize {
            columns: 4000,
            rows: 4000,
        },
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
                    meta,
                    key: None
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
                meta: true,
                key: None
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
    let reporting = TerminalModes {
        focus_reporting: true,
        ..TerminalModes::default()
    };
    assert_eq!(
        (
            super::encode_focus(true, TerminalModes::default()),
            super::encode_focus(true, reporting),
            super::encode_focus(false, reporting),
        ),
        (&b""[..], &b"\x1b[I"[..], &b"\x1b[O"[..])
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

#[test]
fn legacy_byte_matrix_is_stable() {
    use TerminalKey::*;
    for application_cursor in [false, true] {
        for (key, normal, application) in [
            (Enter, "\r", "\r"),
            (Tab, "\t", "\t"),
            (Backspace, "\x7f", "\x7f"),
            (Escape, "\x1b", "\x1b"),
            (Up, "\x1b[A", "\x1bOA"),
            (Down, "\x1b[B", "\x1bOB"),
            (Right, "\x1b[C", "\x1bOC"),
            (Left, "\x1b[D", "\x1bOD"),
            (Home, "\x1b[H", "\x1bOH"),
            (End, "\x1b[F", "\x1bOF"),
            (PageUp, "\x1b[5~", "\x1b[5~"),
            (PageDown, "\x1b[6~", "\x1b[6~"),
            (Delete, "\x1b[3~", "\x1b[3~"),
        ] {
            for modifiers in [
                Modifiers::default(),
                Modifiers {
                    shift: true,
                    ..Default::default()
                },
                Modifiers {
                    control: true,
                    ..Default::default()
                },
                Modifiers {
                    alt: true,
                    ..Default::default()
                },
            ] {
                let plain = if key == Tab && modifiers.shift {
                    "\x1b[Z"
                } else if application_cursor {
                    application
                } else {
                    normal
                };
                let expected = if modifiers.alt && key != Escape {
                    format!("\x1b{plain}")
                } else {
                    plain.to_owned()
                };
                assert_eq!(
                    encode_input(
                        &TerminalInput::Key { key, modifiers },
                        TerminalModes {
                            application_cursor,
                            ..Default::default()
                        }
                    ),
                    expected.as_bytes(),
                    "{key:?} {modifiers:?} app={application_cursor}"
                );
            }
        }
    }
    for (text, meta, expected) in [
        ("\x01", false, "\x01"),
        ("\0", false, "\0"),
        ("\x1f", true, "\x1b\x1f"),
        ("a", true, "\x1ba"),
        ("A", true, "\x1bA"),
        ("λ", true, "\x1bλ"),
    ] {
        assert_eq!(
            encode_input(
                &TerminalInput::Character {
                    text: text.into(),
                    meta,
                    key: None
                },
                TerminalModes::default()
            ),
            expected.as_bytes()
        );
    }
}

#[test]
fn legacy_function_keys_and_insert_have_xterm_bytes() {
    use TerminalKey::*;
    for (key, expected) in [
        (F1, "\x1bOP"),
        (F2, "\x1bOQ"),
        (F3, "\x1bOR"),
        (F4, "\x1bOS"),
        (F5, "\x1b[15~"),
        (F6, "\x1b[17~"),
        (F7, "\x1b[18~"),
        (F8, "\x1b[19~"),
        (F9, "\x1b[20~"),
        (F10, "\x1b[21~"),
        (F11, "\x1b[23~"),
        (F12, "\x1b[24~"),
        (Insert, "\x1b[2~"),
    ] {
        for modifiers in [
            Modifiers::default(),
            Modifiers {
                control: true,
                shift: true,
                ..Default::default()
            },
            Modifiers {
                alt: true,
                ..Default::default()
            },
        ] {
            let wanted = if modifiers.alt {
                format!("\x1b{expected}")
            } else {
                expected.into()
            };
            assert_eq!(
                encode_input(
                    &TerminalInput::Key { key, modifiers },
                    TerminalModes::default()
                ),
                wanted.as_bytes()
            );
        }
    }
}
