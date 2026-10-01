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
mod tests;
