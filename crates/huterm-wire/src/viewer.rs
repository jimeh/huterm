//! A viewer's ordered messages besides its updates: status, input and its
//! acknowledgement, credit, and resynchronization requests.
//!
//! These travel on the viewer's ordering lane. Input is numbered by the
//! client and never resent; a paste travels as its own kind, so only it
//! carries the paste cap.

use huterm_protocol::{
    CharacterKey, InputStamp, Modifiers, MouseAction, MouseButton, MouseInput,
    MousePosition, TerminalInput, TerminalKey, TerminalStatus, ViewerId,
    WheelDirection,
};

use crate::caps::{INPUT_TEXT_PAYLOAD_LIMIT, MAX_CHARACTER_TEXT_BYTES};
use crate::codec::{DecodeError, EncodeError, IdScope, Reader, Writer};
use crate::handshake::MAX_VIEWER_CREDIT;
use crate::status::{decode_status, encode_status};

/// A viewer's whole status, sent whenever its revision changes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewerStatus {
    /// The viewer.
    pub viewer: ViewerId,
    /// The terminal's current status.
    pub status: TerminalStatus,
}

/// One input from a viewer, other than a paste.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Input {
    /// The viewer.
    pub viewer: ViewerId,
    /// The viewer's input sequence number for this input.
    pub seq: u64,
    /// What the client knew when the user acted.
    pub stamp: InputStamp,
    /// The input: text, a character chord, a key, or a mouse report.
    pub input: TerminalInput,
}

/// The highest input sequence of a viewer that reached core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputAck {
    /// The viewer.
    pub viewer: ViewerId,
    /// The highest admitted input sequence.
    pub seq: u64,
}

/// Credit for a viewer's updates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Credit {
    /// The viewer.
    pub viewer: ViewerId,
    /// Credits granted, from 1 to [`MAX_VIEWER_CREDIT`].
    pub grant: u32,
    /// The number of the last update the client applied; zero before the
    /// first.
    pub applied: u64,
}

/// A client's request for a complete snapshot after it discarded a delta.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResyncViewer {
    /// The viewer.
    pub viewer: ViewerId,
}

/// Encodes a viewer status's record.
///
/// # Errors
/// Returns the identity's or the status's encode error.
pub fn encode_viewer_status(
    w: &mut Writer,
    scope: IdScope,
    status: &ViewerStatus,
) -> Result<(), EncodeError> {
    let ViewerStatus { viewer, status } = status;
    w.record(|w| {
        w.id(scope, *viewer)?;
        encode_status(w, status)
    })
}

/// Decodes a viewer status's record.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_viewer_status(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<ViewerStatus, DecodeError> {
    r.record(|r| {
        Ok(ViewerStatus {
            viewer: r.id(scope)?,
            status: decode_status(r)?,
        })
    })
}

/// Encodes an input stamp's record.
///
/// # Errors
/// Never fails; the signature matches the other field encoders.
pub fn encode_stamp(
    w: &mut Writer,
    stamp: InputStamp,
) -> Result<(), EncodeError> {
    let InputStamp { scrolls, geometry } = stamp;
    w.record(|w| {
        w.u64(scrolls);
        w.option(geometry, |w, geometry| {
            w.u64(geometry);
            Ok(())
        })
    })
}

/// Decodes an input stamp's record.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_stamp(r: &mut Reader<'_>) -> Result<InputStamp, DecodeError> {
    r.record(|r| {
        Ok(InputStamp {
            scrolls: r.u64()?,
            geometry: r.option("geometry", Reader::u64)?,
        })
    })
}

const MODIFIER_FLAGS: u64 = 0b111;

fn write_modifiers(w: &mut Writer, modifiers: Modifiers) {
    let Modifiers {
        control,
        alt,
        shift,
    } = modifiers;
    w.u64(u64::from(control) | u64::from(alt) << 1 | u64::from(shift) << 2);
}

fn read_modifiers(r: &mut Reader<'_>) -> Result<Modifiers, DecodeError> {
    let flags = r.u64()?;
    if flags & !MODIFIER_FLAGS != 0 {
        return Err(DecodeError::Invalid("modifiers"));
    }
    Ok(Modifiers {
        control: flags & 1 != 0,
        alt: flags & 2 != 0,
        shift: flags & 4 != 0,
    })
}

fn write_char(w: &mut Writer, character: char) {
    w.u32(u32::from(character));
}

fn read_char(r: &mut Reader<'_>) -> Result<char, DecodeError> {
    char::from_u32(r.u32()?).ok_or(DecodeError::Invalid("character"))
}

/// Keys in tag order.
const KEYS: [TerminalKey; 26] = [
    TerminalKey::Enter,
    TerminalKey::Tab,
    TerminalKey::Backspace,
    TerminalKey::Escape,
    TerminalKey::Up,
    TerminalKey::Down,
    TerminalKey::Left,
    TerminalKey::Right,
    TerminalKey::Home,
    TerminalKey::End,
    TerminalKey::PageUp,
    TerminalKey::PageDown,
    TerminalKey::Delete,
    TerminalKey::Insert,
    TerminalKey::F1,
    TerminalKey::F2,
    TerminalKey::F3,
    TerminalKey::F4,
    TerminalKey::F5,
    TerminalKey::F6,
    TerminalKey::F7,
    TerminalKey::F8,
    TerminalKey::F9,
    TerminalKey::F10,
    TerminalKey::F11,
    TerminalKey::F12,
];

fn button_tag(button: MouseButton) -> u64 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

fn read_button(r: &mut Reader<'_>) -> Result<MouseButton, DecodeError> {
    Ok(match r.tag()? {
        0 => MouseButton::Left,
        1 => MouseButton::Middle,
        2 => MouseButton::Right,
        tag => {
            return Err(DecodeError::UnknownTag {
                field: "mouse button",
                tag,
            });
        }
    })
}

fn write_mouse(w: &mut Writer, mouse: MouseInput) -> Result<(), EncodeError> {
    let MouseInput {
        position: MousePosition { column, row },
        action,
        modifiers,
    } = mouse;
    w.record(|w| {
        w.u32(column);
        w.u32(row);
        match action {
            MouseAction::Press(button) => {
                w.tag(0);
                w.tag(button_tag(button));
            }
            MouseAction::Release(button) => {
                w.tag(1);
                w.tag(button_tag(button));
            }
            MouseAction::Motion(button) => {
                w.tag(2);
                w.option(button, |w, button| {
                    w.tag(button_tag(button));
                    Ok(())
                })?;
            }
            MouseAction::Wheel(direction) => {
                w.tag(3);
                w.tag(match direction {
                    WheelDirection::Up => 0,
                    WheelDirection::Down => 1,
                    WheelDirection::Left => 2,
                    WheelDirection::Right => 3,
                });
            }
        }
        write_modifiers(w, modifiers);
        Ok(())
    })
}

fn read_mouse(r: &mut Reader<'_>) -> Result<MouseInput, DecodeError> {
    r.record(|r| {
        let position = MousePosition {
            column: r.u32()?,
            row: r.u32()?,
        };
        let action = match r.tag()? {
            0 => MouseAction::Press(read_button(r)?),
            1 => MouseAction::Release(read_button(r)?),
            2 => MouseAction::Motion(r.option("mouse button", read_button)?),
            3 => MouseAction::Wheel(match r.tag()? {
                0 => WheelDirection::Up,
                1 => WheelDirection::Down,
                2 => WheelDirection::Left,
                3 => WheelDirection::Right,
                tag => {
                    return Err(DecodeError::UnknownTag {
                        field: "wheel direction",
                        tag,
                    });
                }
            }),
            tag => {
                return Err(DecodeError::UnknownTag {
                    field: "mouse action",
                    tag,
                });
            }
        };
        Ok(MouseInput {
            position,
            action,
            modifiers: read_modifiers(r)?,
        })
    })
}

fn write_character_key(
    w: &mut Writer,
    key: &CharacterKey,
) -> Result<(), EncodeError> {
    let CharacterKey {
        unshifted,
        text,
        base_layout,
        modifiers,
    } = key;
    w.record(|w| {
        write_char(w, *unshifted);
        w.str(text, MAX_CHARACTER_TEXT_BYTES, "layout text")?;
        w.option(*base_layout, |w, character| {
            write_char(w, character);
            Ok(())
        })?;
        write_modifiers(w, *modifiers);
        Ok(())
    })
}

fn read_character_key(r: &mut Reader<'_>) -> Result<CharacterKey, DecodeError> {
    r.record(|r| {
        Ok(CharacterKey {
            unshifted: read_char(r)?,
            text: r.string(MAX_CHARACTER_TEXT_BYTES, "layout text")?,
            base_layout: r.option("base layout", read_char)?,
            modifiers: read_modifiers(r)?,
        })
    })
}

fn write_input(
    w: &mut Writer,
    input: &TerminalInput,
) -> Result<(), EncodeError> {
    match input {
        TerminalInput::Text(text) => {
            w.tag(0);
            w.record(|w| w.str(text, INPUT_TEXT_PAYLOAD_LIMIT, "input text"))
        }
        TerminalInput::Character { text, meta, key } => {
            w.tag(1);
            w.record(|w| {
                w.str(text, MAX_CHARACTER_TEXT_BYTES, "character text")?;
                w.bool(*meta);
                w.option(key.as_deref(), write_character_key)
            })
        }
        TerminalInput::Key { key, modifiers } => {
            let tag = KEYS
                .iter()
                .position(|known| known == key)
                .ok_or(EncodeError::Unsupported)?;
            w.tag(2);
            w.record(|w| {
                w.usize(tag);
                write_modifiers(w, *modifiers);
                Ok(())
            })
        }
        TerminalInput::Mouse(mouse) => {
            w.tag(3);
            write_mouse(w, *mouse)
        }
        // A paste is its own message kind, so it carries the paste cap.
        TerminalInput::Paste(_) => {
            Err(EncodeError::Invalid("paste in an input message"))
        }
        _ => Err(EncodeError::Unsupported),
    }
}

fn read_input(r: &mut Reader<'_>) -> Result<TerminalInput, DecodeError> {
    Ok(match r.tag()? {
        0 => TerminalInput::Text(
            r.record(|r| r.string(INPUT_TEXT_PAYLOAD_LIMIT, "input text"))?,
        ),
        1 => r.record(|r| {
            Ok(TerminalInput::Character {
                text: r.string(MAX_CHARACTER_TEXT_BYTES, "character text")?,
                meta: r.bool()?,
                key: r.option("character key", |r| {
                    read_character_key(r).map(Box::new)
                })?,
            })
        })?,
        2 => r.record(|r| {
            let key = KEYS
                .get(r.usize()?)
                .copied()
                .ok_or(DecodeError::Invalid("terminal key"))?;
            Ok(TerminalInput::Key {
                key,
                modifiers: read_modifiers(r)?,
            })
        })?,
        3 => TerminalInput::Mouse(read_mouse(r)?),
        tag => {
            return Err(DecodeError::UnknownTag {
                field: "terminal input",
                tag,
            });
        }
    })
}

/// Encodes an input's record.
///
/// # Errors
/// Refuses text past its limit with [`EncodeError::LimitExceeded`], a
/// paste with [`EncodeError::Invalid`], and an input variant with no
/// encoding yet with [`EncodeError::Unsupported`].
pub fn encode_input(
    w: &mut Writer,
    scope: IdScope,
    input: &Input,
) -> Result<(), EncodeError> {
    let Input {
        viewer,
        seq,
        stamp,
        input,
    } = input;
    w.record(|w| {
        w.id(scope, *viewer)?;
        w.u64(*seq);
        encode_stamp(w, *stamp)?;
        write_input(w, input)
    })
}

/// Decodes an input's record.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_input(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<Input, DecodeError> {
    r.record(|r| {
        Ok(Input {
            viewer: r.id(scope)?,
            seq: r.u64()?,
            stamp: decode_stamp(r)?,
            input: read_input(r)?,
        })
    })
}

/// Encodes an input acknowledgement's record.
///
/// # Errors
/// Returns the identity's encode error.
pub fn encode_input_ack(
    w: &mut Writer,
    scope: IdScope,
    ack: InputAck,
) -> Result<(), EncodeError> {
    let InputAck { viewer, seq } = ack;
    w.record(|w| {
        w.id(scope, viewer)?;
        w.u64(seq);
        Ok(())
    })
}

/// Decodes an input acknowledgement's record.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_input_ack(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<InputAck, DecodeError> {
    r.record(|r| {
        Ok(InputAck {
            viewer: r.id(scope)?,
            seq: r.u64()?,
        })
    })
}

/// Encodes a credit grant's record.
///
/// # Errors
/// Refuses a grant of zero or more than [`MAX_VIEWER_CREDIT`] with
/// [`EncodeError::Invalid`].
pub fn encode_credit(
    w: &mut Writer,
    scope: IdScope,
    credit: Credit,
) -> Result<(), EncodeError> {
    let Credit {
        viewer,
        grant,
        applied,
    } = credit;
    if grant == 0 || grant > MAX_VIEWER_CREDIT {
        return Err(EncodeError::Invalid("credit grant"));
    }
    w.record(|w| {
        w.id(scope, viewer)?;
        w.u32(grant);
        w.u64(applied);
        Ok(())
    })
}

/// Decodes a credit grant's record.
///
/// # Errors
/// Returns the first decode failure, or [`DecodeError::Invalid`] for a
/// grant of zero or more than [`MAX_VIEWER_CREDIT`].
pub fn decode_credit(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<Credit, DecodeError> {
    r.record(|r| {
        let credit = Credit {
            viewer: r.id(scope)?,
            grant: r.u32()?,
            applied: r.u64()?,
        };
        if credit.grant == 0 || credit.grant > MAX_VIEWER_CREDIT {
            return Err(DecodeError::Invalid("credit grant"));
        }
        Ok(credit)
    })
}

/// Encodes a resynchronization request's record.
///
/// # Errors
/// Returns the identity's encode error.
pub fn encode_resync(
    w: &mut Writer,
    scope: IdScope,
    resync: ResyncViewer,
) -> Result<(), EncodeError> {
    let ResyncViewer { viewer } = resync;
    w.record(|w| w.id(scope, viewer))
}

/// Decodes a resynchronization request's record.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_resync(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<ResyncViewer, DecodeError> {
    r.record(|r| {
        Ok(ResyncViewer {
            viewer: r.id(scope)?,
        })
    })
}

#[cfg(test)]
mod tests;
