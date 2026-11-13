//! A viewer's ordered messages besides its updates: status, input and its
//! acknowledgement, credit, and resynchronization requests.
//!
//! These travel on the viewer's ordering lane. Input is numbered by the
//! client and never resent; a paste travels as its own kind, so only it
//! carries the paste cap.

use huterm_protocol::{
    BufferPoint, BufferRange, CellSize, CellText, CharacterKey, GridSize,
    InputStamp, LinkCell, LinkLookup, LinkSource, MAX_CELL_TEXT_BYTES,
    Modifiers, MouseAction, MouseButton, MouseInput, MousePosition,
    ScrollCommand, SelectionUnit, TerminalInput, TerminalKey, TerminalLink,
    TerminalPresentation, TerminalStatus, ViewerCapabilities, ViewerId,
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

/// A viewer's focus report, ordered with its input and outside its input
/// budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Focus {
    /// The viewer.
    pub viewer: ViewerId,
    /// Whether the viewer's view holds focus in an active window.
    pub focused: bool,
}

/// The server's notice that a viewer's terminal stopped. It follows the
/// viewer's final status and its complete final update on the viewer's
/// lane, and the viewer sends nothing more.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ViewerStopped {
    /// The viewer.
    pub viewer: ViewerId,
}

/// The server's notice that a viewer's authority was withdrawn, as a
/// detach, a retarget, a session close, or a move of its terminal does. It
/// follows anything already queued on the viewer's lane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ViewerRevoked {
    /// The viewer.
    pub viewer: ViewerId,
}

/// The most bytes of a link destination a snapshot result carries, core's
/// link scan limit.
pub const MAX_LINK_DESTINATION_BYTES: usize = 32 << 10;

/// The most cells of one link a snapshot result carries, core's link scan
/// limit.
pub const MAX_LINK_CELLS: usize = 16 << 10;

/// The most text bytes all cells of one link carry together, core's link
/// scan limit. A server answers a link past it with
/// [`LinkLookup::ScanLimit`].
pub const MAX_LINK_TEXT_BYTES: usize = 32 << 10;

/// How a client asks for a viewer: what it may do and the state it starts
/// with, as core's viewer options carry them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubscribeOptions {
    /// The requests the viewer may make. Host effects arrive with slice 4;
    /// a server refuses them until then.
    pub capabilities: ViewerCapabilities,
    /// Whether the viewer starts focused.
    pub focused: bool,
    /// The grid and cell size the viewer starts requesting.
    pub geometry: Option<(GridSize, CellSize)>,
    /// The presentation the viewer starts submitting.
    pub presentation: Option<Box<TerminalPresentation>>,
    /// Set only by the client that created the terminal, so an exit before
    /// its first status counts as an exit transition.
    pub spawned: bool,
}

/// What one input counts against its viewer's input budget: its text's
/// bytes, a paste's included, and at least one byte. Client and server count
/// the same way, so the client's unacknowledged input and the server's
/// staged input agree.
#[must_use]
pub fn input_cost(input: &TerminalInput) -> usize {
    let bytes = match input {
        TerminalInput::Text(text) | TerminalInput::Paste(text) => text.len(),
        TerminalInput::Character { text, key, .. } => {
            text.len() + key.as_ref().map_or(0, |key| key.text.len())
        }
        _ => 0,
    };
    bytes.max(1)
}

/// Encodes a focus report's record.
///
/// # Errors
/// Returns the identity's encode error.
pub fn encode_focus(
    w: &mut Writer,
    scope: IdScope,
    focus: Focus,
) -> Result<(), EncodeError> {
    let Focus { viewer, focused } = focus;
    w.record(|w| {
        w.id(scope, viewer)?;
        w.bool(focused);
        Ok(())
    })
}

/// Decodes a focus report's record.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_focus(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<Focus, DecodeError> {
    r.record(|r| {
        Ok(Focus {
            viewer: r.id(scope)?,
            focused: r.bool()?,
        })
    })
}

/// Encodes the record of a message that names only a viewer:
/// [`ResyncViewer`], [`ViewerStopped`], or [`ViewerRevoked`].
///
/// # Errors
/// Returns the identity's encode error.
pub fn encode_viewer_only(
    w: &mut Writer,
    scope: IdScope,
    viewer: ViewerId,
) -> Result<(), EncodeError> {
    w.record(|w| w.id(scope, viewer))
}

/// Decodes the record of a message that names only a viewer.
///
/// # Errors
/// Returns the first decode failure.
pub fn decode_viewer_only(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<ViewerId, DecodeError> {
    r.record(|r| r.id(scope))
}

/// Writes a scroll command.
pub(crate) fn write_scroll(w: &mut Writer, scroll: ScrollCommand) {
    match scroll {
        ScrollCommand::Relative(rows) => {
            w.tag(0);
            w.i64(rows);
        }
        ScrollCommand::Absolute(offset) => {
            w.tag(1);
            w.usize(offset);
        }
        ScrollCommand::Live => w.tag(2),
    }
}

/// Reads a scroll command.
pub(crate) fn read_scroll(
    r: &mut Reader<'_>,
) -> Result<ScrollCommand, DecodeError> {
    Ok(match r.tag()? {
        0 => ScrollCommand::Relative(r.i64()?),
        1 => ScrollCommand::Absolute(r.usize()?),
        2 => ScrollCommand::Live,
        tag => {
            return Err(DecodeError::UnknownTag {
                field: "scroll command",
                tag,
            });
        }
    })
}

/// Writes a cell position as its column and row.
pub(crate) fn write_position(w: &mut Writer, position: MousePosition) {
    let MousePosition { column, row } = position;
    w.u32(column);
    w.u32(row);
}

/// Reads a cell position.
pub(crate) fn read_position(
    r: &mut Reader<'_>,
) -> Result<MousePosition, DecodeError> {
    Ok(MousePosition {
        column: r.u32()?,
        row: r.u32()?,
    })
}

/// Writes a buffer point's record.
pub(crate) fn write_point(
    w: &mut Writer,
    point: BufferPoint,
) -> Result<(), EncodeError> {
    let BufferPoint {
        rows_from_live_bottom,
        column,
    } = point;
    w.record(|w| {
        w.usize(rows_from_live_bottom);
        w.u16(column);
        Ok(())
    })
}

/// Reads a buffer point's record.
pub(crate) fn read_point(
    r: &mut Reader<'_>,
) -> Result<BufferPoint, DecodeError> {
    r.record(|r| {
        Ok(BufferPoint {
            rows_from_live_bottom: r.usize()?,
            column: r.u16()?,
        })
    })
}

/// Writes a buffer range's record, its endpoints in reading order.
pub(crate) fn write_range(
    w: &mut Writer,
    range: BufferRange,
) -> Result<(), EncodeError> {
    let BufferRange { start, end } = range;
    w.record(|w| {
        write_point(w, start)?;
        write_point(w, end)
    })
}

/// Reads a buffer range's record, ordering its endpoints as core's
/// constructor does.
pub(crate) fn read_range(
    r: &mut Reader<'_>,
) -> Result<BufferRange, DecodeError> {
    r.record(|r| {
        let start = read_point(r)?;
        let end = read_point(r)?;
        let range = BufferRange::ordered(start, end);
        if range.start != start {
            return Err(DecodeError::Invalid("buffer range order"));
        }
        Ok(range)
    })
}

/// Writes a selection unit's tag.
pub(crate) fn write_unit(w: &mut Writer, unit: SelectionUnit) {
    w.tag(match unit {
        SelectionUnit::Word => 0,
        SelectionUnit::Line => 1,
    });
}

/// Reads a selection unit's tag.
pub(crate) fn read_unit(
    r: &mut Reader<'_>,
) -> Result<SelectionUnit, DecodeError> {
    Ok(match r.tag()? {
        0 => SelectionUnit::Word,
        1 => SelectionUnit::Line,
        tag => {
            return Err(DecodeError::UnknownTag {
                field: "selection unit",
                tag,
            });
        }
    })
}

const CAPABILITY_FLAGS: u64 = 0b1111;

/// Writes viewer capabilities as a bit set.
pub(crate) fn write_capabilities(
    w: &mut Writer,
    capabilities: ViewerCapabilities,
) {
    let ViewerCapabilities {
        input,
        viewport,
        size,
        host_effects,
    } = capabilities;
    w.u64(
        u64::from(input)
            | u64::from(viewport) << 1
            | u64::from(size) << 2
            | u64::from(host_effects) << 3,
    );
}

/// Reads viewer capabilities.
pub(crate) fn read_capabilities(
    r: &mut Reader<'_>,
) -> Result<ViewerCapabilities, DecodeError> {
    let flags = r.u64()?;
    if flags & !CAPABILITY_FLAGS != 0 {
        return Err(DecodeError::Invalid("viewer capabilities"));
    }
    Ok(ViewerCapabilities {
        input: flags & 1 != 0,
        viewport: flags & 2 != 0,
        size: flags & 4 != 0,
        host_effects: flags & 8 != 0,
    })
}

/// Encodes subscription options' record.
///
/// # Errors
/// Never fails today; the signature matches the other field encoders.
pub(crate) fn encode_subscribe_options(
    w: &mut Writer,
    options: &SubscribeOptions,
) -> Result<(), EncodeError> {
    let SubscribeOptions {
        capabilities,
        focused,
        geometry,
        presentation,
        spawned,
    } = options;
    w.record(|w| {
        write_capabilities(w, *capabilities);
        w.bool(*focused);
        w.option(*geometry, |w, (grid, cell)| {
            crate::launch::encode_grid(w, grid);
            crate::launch::encode_cell_size(w, cell);
            Ok(())
        })?;
        w.option(presentation.as_deref(), crate::launch::encode_presentation)?;
        w.bool(*spawned);
        Ok(())
    })
}

/// Decodes subscription options' record.
///
/// # Errors
/// Returns the first decode failure.
pub(crate) fn decode_subscribe_options(
    r: &mut Reader<'_>,
) -> Result<SubscribeOptions, DecodeError> {
    r.record(|r| {
        Ok(SubscribeOptions {
            capabilities: read_capabilities(r)?,
            focused: r.bool()?,
            geometry: r.option("geometry", |r| {
                Ok((
                    crate::launch::decode_grid(r)?,
                    crate::launch::decode_cell_size(r)?,
                ))
            })?,
            presentation: r.option("presentation", |r| {
                crate::launch::decode_presentation(r).map(Box::new)
            })?,
            spawned: r.bool()?,
        })
    })
}

/// Writes a link lookup's outcome. A link past the wire's link limits
/// encodes as [`LinkLookup::ScanLimit`], as core reports a link its scan
/// could not bound.
pub(crate) fn write_link(
    w: &mut Writer,
    link: &LinkLookup,
) -> Result<(), EncodeError> {
    match link {
        LinkLookup::Match(link) if link_fits(link) => {
            let TerminalLink {
                destination,
                source,
                cells,
            } = link;
            w.tag(0);
            w.record(|w| {
                w.str(
                    destination,
                    MAX_LINK_DESTINATION_BYTES,
                    "link destination",
                )?;
                w.tag(match source {
                    LinkSource::PlainText => 0,
                    LinkSource::Osc8 => 1,
                });
                w.seq(cells.iter(), MAX_LINK_CELLS, "link cells", |w, cell| {
                    let LinkCell { position, text } = cell;
                    write_position(w, *position);
                    w.str(text, MAX_CELL_TEXT_BYTES, "link cell text")
                })
            })
        }
        LinkLookup::Match(_) | LinkLookup::ScanLimit => {
            w.tag(2);
            Ok(())
        }
        LinkLookup::NoMatch => {
            w.tag(1);
            Ok(())
        }
        LinkLookup::Unavailable => {
            w.tag(3);
            Ok(())
        }
    }
}

fn link_fits(link: &TerminalLink) -> bool {
    link.destination.len() <= MAX_LINK_DESTINATION_BYTES
        && link.cells.len() <= MAX_LINK_CELLS
        && link
            .cells
            .iter()
            .try_fold(0_usize, |total, cell| {
                (cell.text.len() <= MAX_CELL_TEXT_BYTES)
                    .then(|| total + cell.text.len())
            })
            .is_some_and(|total| total <= MAX_LINK_TEXT_BYTES)
}

/// Reads a link lookup's outcome.
pub(crate) fn read_link(r: &mut Reader<'_>) -> Result<LinkLookup, DecodeError> {
    Ok(match r.tag()? {
        0 => r.record(|r| {
            let destination =
                r.string(MAX_LINK_DESTINATION_BYTES, "link destination")?;
            let source = match r.tag()? {
                0 => LinkSource::PlainText,
                1 => LinkSource::Osc8,
                tag => {
                    return Err(DecodeError::UnknownTag {
                        field: "link source",
                        tag,
                    });
                }
            };
            let mut text_bytes = 0_usize;
            let cells = r.seq(MAX_LINK_CELLS, "link cells", |r| {
                let position = read_position(r)?;
                let text = r.str(MAX_CELL_TEXT_BYTES, "link cell text")?;
                text_bytes += text.len();
                if text_bytes > MAX_LINK_TEXT_BYTES {
                    return Err(DecodeError::TooLong {
                        field: "link cell text",
                    });
                }
                Ok(LinkCell {
                    position,
                    text: CellText::from(text),
                })
            })?;
            Ok(LinkLookup::Match(TerminalLink {
                destination,
                source,
                cells,
            }))
        })?,
        1 => LinkLookup::NoMatch,
        2 => LinkLookup::ScanLimit,
        3 => LinkLookup::Unavailable,
        tag => {
            return Err(DecodeError::UnknownTag {
                field: "link lookup",
                tag,
            });
        }
    })
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
