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
        TerminalInput::Character { text, meta, .. } => {
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
        TerminalInput::Key { key, modifiers } => {
            encode_key(*key, *modifiers, modes)
        }
        _ => Vec::new(),
    }
}

/// Encodes a terminal focus change, or nothing when the application has not
/// enabled focus reporting.
pub(crate) fn encode_focus(
    focused: bool,
    modes: TerminalModes,
) -> &'static [u8] {
    match (modes.focus_reporting, focused) {
        (false, _) => b"",
        (true, true) => b"\x1b[I",
        (true, false) => b"\x1b[O",
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
        TerminalKey::Insert => b"\x1b[2~".as_slice(),
        TerminalKey::F1 => b"\x1bOP".as_slice(),
        TerminalKey::F2 => b"\x1bOQ".as_slice(),
        TerminalKey::F3 => b"\x1bOR".as_slice(),
        TerminalKey::F4 => b"\x1bOS".as_slice(),
        TerminalKey::F5 => b"\x1b[15~".as_slice(),
        TerminalKey::F6 => b"\x1b[17~".as_slice(),
        TerminalKey::F7 => b"\x1b[18~".as_slice(),
        TerminalKey::F8 => b"\x1b[19~".as_slice(),
        TerminalKey::F9 => b"\x1b[20~".as_slice(),
        TerminalKey::F10 => b"\x1b[21~".as_slice(),
        TerminalKey::F11 => b"\x1b[23~".as_slice(),
        TerminalKey::F12 => b"\x1b[24~".as_slice(),

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

/// Kitty is selected only by the owner engine's live, masked flag stack.
pub(crate) fn encode_kitty_input(
    input: &TerminalInput,
    encoder: &mut huterm_ghostty::KeyEncoder,
) -> huterm_ghostty::Result<Vec<u8>> {
    use huterm_ghostty::{Key, KeyEvent};
    let (key, modifiers, text, unshifted) = match input {
        TerminalInput::Character {
            text,
            key: Some(identity),
            ..
        } => {
            let modifiers = identity.modifiers;
            if !modifiers.control && !modifiers.alt {
                return Ok(text.as_bytes().to_vec());
            }
            (
                physical_key(identity.base_layout),
                modifiers,
                identity.text.as_str(),
                Some(identity.unshifted),
            )
        }
        TerminalInput::Key { key, modifiers } => {
            let key = match key {
                TerminalKey::Enter => Key::Enter,
                TerminalKey::Tab => Key::Tab,
                TerminalKey::Backspace => Key::Backspace,
                TerminalKey::Escape => Key::Escape,
                TerminalKey::Up => Key::ArrowUp,
                TerminalKey::Down => Key::ArrowDown,
                TerminalKey::Left => Key::ArrowLeft,
                TerminalKey::Right => Key::ArrowRight,
                TerminalKey::Home => Key::Home,
                TerminalKey::End => Key::End,
                TerminalKey::PageUp => Key::PageUp,
                TerminalKey::PageDown => Key::PageDown,
                TerminalKey::Delete => Key::Delete,
                TerminalKey::Insert => Key::Insert,
                TerminalKey::F1 => Key::F1,
                TerminalKey::F2 => Key::F2,
                TerminalKey::F3 => Key::F3,
                TerminalKey::F4 => Key::F4,
                TerminalKey::F5 => Key::F5,
                TerminalKey::F6 => Key::F6,
                TerminalKey::F7 => Key::F7,
                TerminalKey::F8 => Key::F8,
                TerminalKey::F9 => Key::F9,
                TerminalKey::F10 => Key::F10,
                TerminalKey::F11 => Key::F11,
                TerminalKey::F12 => Key::F12,
                _ => Key::Unidentified,
            };
            (key, *modifiers, "", None)
        }
        _ => return Ok(Vec::new()),
    };
    encoder.encode(KeyEvent {
        key,
        control: modifiers.control,
        alt: modifiers.alt,
        shift: modifiers.shift,
        text,
        unshifted,
    })
}

/// Maps plain US physical-key characters without exposing engine types to clients.
fn physical_key(character: Option<char>) -> huterm_ghostty::Key {
    use huterm_ghostty::Key;
    match character {
        Some('a') => Key::A,
        Some('b') => Key::B,
        Some('c') => Key::C,
        Some('d') => Key::D,
        Some('e') => Key::E,
        Some('f') => Key::F,
        Some('g') => Key::G,
        Some('h') => Key::H,
        Some('i') => Key::I,
        Some('j') => Key::J,
        Some('k') => Key::K,
        Some('l') => Key::L,
        Some('m') => Key::M,
        Some('n') => Key::N,
        Some('o') => Key::O,
        Some('p') => Key::P,
        Some('q') => Key::Q,
        Some('r') => Key::R,
        Some('s') => Key::S,
        Some('t') => Key::T,
        Some('u') => Key::U,
        Some('v') => Key::V,
        Some('w') => Key::W,
        Some('x') => Key::X,
        Some('y') => Key::Y,
        Some('z') => Key::Z,
        Some('0') => Key::Digit0,
        Some('1') => Key::Digit1,
        Some('2') => Key::Digit2,
        Some('3') => Key::Digit3,
        Some('4') => Key::Digit4,
        Some('5') => Key::Digit5,
        Some('6') => Key::Digit6,
        Some('7') => Key::Digit7,
        Some('8') => Key::Digit8,
        Some('9') => Key::Digit9,
        Some('`') => Key::Backquote,
        Some('\\') => Key::Backslash,
        Some('[') => Key::BracketLeft,
        Some(']') => Key::BracketRight,
        Some(',') => Key::Comma,
        Some('=') => Key::Equal,
        Some('-') => Key::Minus,
        Some('.') => Key::Period,
        Some('\'') => Key::Quote,
        Some(';') => Key::Semicolon,
        Some('/') => Key::Slash,
        Some(' ') => Key::Space,
        _ => Key::Unidentified,
    }
}

#[cfg(test)]
mod tests;
