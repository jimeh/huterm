//! Message kinds, payload limits, and the byte cap each kind's messages may
//! not exceed.
//!
//! Payload limits bound what a user or terminal supplies, such as paste
//! bytes or clipboard text; core invariants bound server state, such as
//! names and hierarchy items. A kind's cap is its payload limit or the
//! invariants its record carries, plus the worst-case encoding overhead of
//! its kind, plus headroom for fields later minor versions append. So a
//! payload at its limit, or state at every invariant's limit, always fits,
//! and a newer sender's appended fields never push an older receiver past
//! its cap.

use huterm_protocol::{
    COMMAND_RECORD_CAPACITY, GridSize, MAX_CELL_TEXT_BYTES, MAX_GRID_CELLS,
    MAX_HIERARCHY_ITEMS, MAX_NAME_BYTES, MAX_SPLIT_CHILDREN,
    MAX_STATUS_TEXT_BYTES, TERMINAL_FAILURE_CAPACITY,
};

use crate::codec::{MAX_VARINT_BYTES, varint_len};
use crate::command::{
    MAX_ARGUMENT_TEXT_BYTES, MAX_COMMAND_ARGUMENTS, MAX_COMMAND_NAME_BYTES,
};
use crate::error::MAX_ERROR_MESSAGE_BYTES;
use crate::frame::HANDSHAKE_FRAME_LIMIT;
use crate::launch::{
    LAUNCH_PAYLOAD_LIMIT, MAX_LAUNCH_ARGUMENTS, MAX_LAUNCH_ENVIRONMENT,
    MAX_LAUNCH_PATH_BYTES,
};
use crate::request::{
    MAX_CLOSE_REQUEST_TABS, MAX_REQUEST_LAYOUT_ITEMS, MAX_REQUEST_NAME_BYTES,
};
use crate::response::{
    MAX_CLOSE_SUMMARY_PROCESSES, MAX_CLOSE_SUMMARY_TERMINALS,
    MAX_PROCESS_COMMAND_BYTES, MAX_PROCESS_COMMAND_LINE_BYTES,
};

/// The most unacknowledged input bytes one viewer may have outstanding.
pub const VIEWER_INPUT_BUDGET: usize = 1 << 20;

/// The largest paste a client sends as one message: one viewer's whole
/// input budget.
pub const PASTE_PAYLOAD_LIMIT: usize = VIEWER_INPUT_BUDGET;

/// The largest clipboard text one host effect carries, the Kitty
/// protocol's minimum write size.
pub const CLIPBOARD_PAYLOAD_LIMIT: usize = 64 << 20;

/// The longest text one [`Input`](crate::viewer::Input) carries: one
/// viewer's whole input budget, as for a paste.
pub const INPUT_TEXT_PAYLOAD_LIMIT: usize = VIEWER_INPUT_BUDGET;

/// The longest text of one character chord, and of its layout text. A chord
/// produces a few characters; a sender refuses a longer one locally.
pub const MAX_CHARACTER_TEXT_BYTES: usize = 256;

/// The cap of a request or other message that carries no payload.
pub const SMALL_MESSAGE_CAP: usize = 64 << 10;

/// The longest diagnostic text a handshake carries: names, host names,
/// and build versions.
pub const MAX_DIAGNOSTIC_BYTES: usize = 256;

/// Bytes each record reserves for fields later minor versions append.
const APPENDED_HEADROOM: usize = 64;

/// Bytes each hierarchy item reserves for fields later minor versions
/// append.
const ITEM_HEADROOM: usize = 16;

/// Which peer sends a message kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    /// The client sends it; the server receives it.
    ClientToServer,
    /// The server sends it; the client receives it.
    ServerToClient,
}

/// A message's kind, the varint that opens every message. Numbers are
/// stable; the handshake kinds never change in any major version.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum MessageKind {
    /// The client's first message.
    Hello,
    /// The server's acceptance of a `Hello`.
    Welcome,
    /// The server's refusal of a `Hello`, after which it closes.
    Reject,
    /// The hierarchy's state when a subscription starts.
    HierarchySnapshot,
    /// One sequenced hierarchy event.
    HierarchyEvent,
    /// A fresh hierarchy state that replaces events a lagging connection
    /// lost, or that projects a newer event onto an older peer.
    HierarchyResync,
    /// A viewer's paste, admitted as one input.
    Paste,
    /// A host effect routed to a client, such as an OSC 52 clipboard write.
    HostEffect,
    /// A viewer's rows and metadata, complete or as a delta against an
    /// earlier update. Its cap follows the grid its header declares.
    TerminalUpdate,
    /// A viewer's terminal status, sent whole whenever its revision changes.
    ViewerStatus,
    /// One ordered input from a viewer, other than a paste.
    Input,
    /// The highest input sequence of a viewer that reached core.
    InputAck,
    /// Credit for a viewer's updates, and the last update the client
    /// applied.
    Credit,
    /// A client's request for a complete snapshot after it discarded a
    /// delta.
    ResyncViewer,
    /// A request, numbered by the client.
    Request,
    /// A request's result.
    Result,
    /// A client's request to abandon a request that has not started.
    Cancel,
}

impl MessageKind {
    /// Every kind, in number order.
    pub const ALL: [Self; 17] = [
        Self::Hello,
        Self::Welcome,
        Self::Reject,
        Self::HierarchySnapshot,
        Self::HierarchyEvent,
        Self::HierarchyResync,
        Self::Paste,
        Self::HostEffect,
        Self::TerminalUpdate,
        Self::ViewerStatus,
        Self::Input,
        Self::InputAck,
        Self::Credit,
        Self::ResyncViewer,
        Self::Request,
        Self::Result,
        Self::Cancel,
    ];

    /// The kind's stable number.
    #[must_use]
    pub const fn number(self) -> u64 {
        match self {
            Self::Hello => 0,
            Self::Welcome => 1,
            Self::Reject => 2,
            Self::HierarchySnapshot => 3,
            Self::HierarchyEvent => 4,
            Self::HierarchyResync => 5,
            Self::Paste => 6,
            Self::HostEffect => 7,
            Self::TerminalUpdate => 8,
            Self::ViewerStatus => 9,
            Self::Input => 10,
            Self::InputAck => 11,
            Self::Credit => 12,
            Self::ResyncViewer => 13,
            Self::Request => 14,
            Self::Result => 15,
            Self::Cancel => 16,
        }
    }

    /// The kind with this number, if any.
    #[must_use]
    pub fn from_number(number: u64) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.number() == number)
    }

    /// Which peer sends the kind.
    #[must_use]
    pub const fn direction(self) -> Direction {
        match self {
            Self::Hello
            | Self::Paste
            | Self::Input
            | Self::Credit
            | Self::ResyncViewer
            | Self::Request
            | Self::Cancel => Direction::ClientToServer,
            Self::Welcome
            | Self::Reject
            | Self::HierarchySnapshot
            | Self::HierarchyEvent
            | Self::HierarchyResync
            | Self::HostEffect
            | Self::TerminalUpdate
            | Self::ViewerStatus
            | Self::InputAck
            | Self::Result => Direction::ServerToClient,
        }
    }

    /// The minor version that introduced the kind. A sender withholds a
    /// kind newer than the negotiated minor version.
    #[must_use]
    pub const fn since(self) -> u16 {
        0
    }
}

/// The byte cap of each message kind at one minor version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MessageCaps {
    minor: u16,
}

impl MessageCaps {
    /// The caps that bind both peers at negotiated minor version `minor`.
    #[must_use]
    pub const fn for_minor(minor: u16) -> Self {
        Self { minor }
    }

    /// The negotiated minor version these caps follow.
    #[must_use]
    pub const fn minor(self) -> u16 {
        self.minor
    }

    /// The most bytes, kind varint included, one message of `kind` may
    /// take, or `None` when the kind does not exist at this minor version.
    #[must_use]
    pub fn cap(self, kind: MessageKind) -> Option<usize> {
        if kind.since() > self.minor {
            return None;
        }
        Some(match kind {
            MessageKind::Hello | MessageKind::Welcome | MessageKind::Reject => {
                HANDSHAKE_FRAME_LIMIT as usize
            }
            MessageKind::HierarchySnapshot
            | MessageKind::HierarchyEvent
            | MessageKind::HierarchyResync => {
                message_cap(kind, hierarchy_record_cap())
            }
            MessageKind::Paste => message_cap(kind, paste_record_cap()),
            MessageKind::HostEffect => {
                message_cap(kind, host_effect_record_cap())
            }
            // The largest any grid allows; each update's own cap follows
            // the grid its header declares.
            MessageKind::TerminalUpdate => message_cap(
                kind,
                update_record_cap(MAX_GRID_CELLS, u16::MAX as usize),
            ),
            MessageKind::ViewerStatus => message_cap(kind, status_record_cap()),
            MessageKind::Input => message_cap(kind, input_record_cap()),
            MessageKind::Request => message_cap(kind, request_record_cap()),
            MessageKind::Result => message_cap(kind, result_record_cap()),
            MessageKind::InputAck
            | MessageKind::Credit
            | MessageKind::ResyncViewer
            | MessageKind::Cancel => SMALL_MESSAGE_CAP,
        })
    }

    /// The cap of a terminal update whose header declares `size`, or `None`
    /// for a grid with no cells or more than [`MAX_GRID_CELLS`]. A
    /// worst-case update, every cell at [`MAX_CELL_TEXT_BYTES`] in a style of
    /// its own, fits it.
    #[must_use]
    pub fn update_cap(self, size: GridSize) -> Option<usize> {
        let kind = MessageKind::TerminalUpdate;
        if kind.since() > self.minor
            || size.columns == 0
            || size.rows == 0
            || size.cells() > MAX_GRID_CELLS
        {
            return None;
        }
        Some(message_cap(
            kind,
            update_record_cap(size.cells(), usize::from(size.rows)),
        ))
    }

    /// The largest cap of any kind `direction`'s sender may send.
    #[must_use]
    pub fn largest(self, direction: Direction) -> usize {
        MessageKind::ALL
            .into_iter()
            .filter(|kind| kind.direction() == direction)
            .filter_map(|kind| self.cap(kind))
            .max()
            .unwrap_or(0)
    }
}

/// A message's bytes: its kind varint, its record's length prefix, and a
/// record of at most `record` bytes.
const fn message_cap(kind: MessageKind, record: usize) -> usize {
    varint_len(kind.number()) + varint_len(record as u64) + record
}

/// A length-prefixed string of at most `max` bytes.
const fn string(max: usize) -> usize {
    varint_len(max as u64) + max
}

/// An optional string of at most `max` bytes.
const fn optional_string(max: usize) -> usize {
    1 + string(max)
}

/// A record's length prefix for a record of up to `max` bytes.
const fn record_prefix(max: usize) -> usize {
    varint_len(max as u64)
}

/// The most bytes one hierarchy item takes in a snapshot or an event,
/// with its share of record prefixes, counts, and tags.
const fn hierarchy_item_bytes() -> usize {
    let id = MAX_VARINT_BYTES;
    let name = string(MAX_NAME_BYTES);
    let custom = optional_string(MAX_NAME_BYTES);
    // Record prefixes, a sequence count, and an enum tag.
    let framing = 3 * record_prefix(1 << 24) + MAX_VARINT_BYTES + 1;
    // A pane as a split child: weight, node tag, pane and terminal IDs,
    // fallback and custom names.
    let pane = MAX_VARINT_BYTES + 1 + 2 * id + name + custom;
    // A session or workspace: ID, custom and automatic names.
    let named = id + custom + name;
    // A tab: ID, custom name, shared zoom, input sync, and as a change its
    // workspace, position, and a second change's tag.
    let tab = id + custom + 1 + id + 1 + id + MAX_VARINT_BYTES + 1;
    // A split: ID, axis, child count, and its own weight and tag.
    let split =
        id + 1 + varint_len(MAX_SPLIT_CHILDREN as u64) + MAX_VARINT_BYTES + 1;
    let mut largest = pane;
    if named > largest {
        largest = named;
    }
    if tab > largest {
        largest = tab;
    }
    if split > largest {
        largest = split;
    }
    framing + largest + ITEM_HEADROOM
}

/// A hierarchy snapshot, resync, or event record: sequence and digest
/// fields, top-level counts, and every item at its worst case.
const fn hierarchy_record_cap() -> usize {
    APPENDED_HEADROOM
        + 4 * MAX_VARINT_BYTES
        + MAX_HIERARCHY_ITEMS * hierarchy_item_bytes()
}

/// A paste record: viewer, input sequence, stamp, and the text.
const fn paste_record_cap() -> usize {
    let stamp = 1 + MAX_VARINT_BYTES + 1 + MAX_VARINT_BYTES + APPENDED_HEADROOM;
    APPENDED_HEADROOM
        + 2 * MAX_VARINT_BYTES
        + stamp
        + string(PASTE_PAYLOAD_LIMIT)
}

/// A host effect record: effect, viewer, generation, and terminal, the
/// payload's tag and record with its destination and text, and the
/// response option.
const fn host_effect_record_cap() -> usize {
    APPENDED_HEADROOM
        + 4 * MAX_VARINT_BYTES
        + 1
        + record_prefix(CLIPBOARD_PAYLOAD_LIMIT + APPENDED_HEADROOM)
        + 1
        + string(CLIPBOARD_PAYLOAD_LIMIT)
        + APPENDED_HEADROOM
        + 1
        + MAX_VARINT_BYTES
}

/// Bytes each style-table entry and each full row reserve for fields later
/// minor versions append.
const UPDATE_ITEM_HEADROOM: usize = 4;

/// An optional RGB color: the tag and three bytes.
const OPTIONAL_RGB: usize = 1 + 3;

/// A cell color: its tag and, at most, three RGB bytes.
const CELL_COLOR: usize = 1 + 3;

/// The fixed fields of an update record: grid, viewer, sequences, credit
/// and digest, revisions, cursor, modes, viewport, colors, and the two
/// sequence counts.
const fn update_header_bytes() -> usize {
    let grid = 2 * varint_len(u16::MAX as u64);
    let sequences = MAX_VARINT_BYTES + 1 + MAX_VARINT_BYTES;
    let credit_and_digest = 1 + 1 + 8;
    let revisions = 3 * MAX_VARINT_BYTES;
    // A cursor record: its option tag, length, position, and shape.
    let cursor =
        1 + 1 + 2 * varint_len(u16::MAX as u64) + 1 + APPENDED_HEADROOM;
    // A modes record: its length, Kitty flags, mode flags, and two tags.
    let modes = 1 + 2 + MAX_VARINT_BYTES + 2 + APPENDED_HEADROOM;
    let viewport = 2 * MAX_VARINT_BYTES;
    let counts = 2 * varint_len(MAX_GRID_CELLS as u64);
    APPENDED_HEADROOM
        + grid
        + MAX_VARINT_BYTES
        + sequences
        + credit_and_digest
        + revisions
        + cursor
        + modes
        + viewport
        + 2 * OPTIONAL_RGB
        + counts
}

/// An update record over `cells` cells in `rows` rows, at its worst case:
/// a style-table entry per cell, every row in full, and a run per cell
/// whose text is at [`MAX_CELL_TEXT_BYTES`]. A run opens with its style
/// index and two form bits in one varint. A packed or wide cell costs at
/// most four bytes and a fill at most a few for any count, far below a
/// length-prefixed text at the limit, and kept rows travel as ranges no
/// larger than a full row's operation, so the worst case is a run of its
/// own per cell with one length-prefixed text.
const fn update_record_cap(cells: usize, rows: usize) -> usize {
    let index = varint_len((MAX_GRID_CELLS as u64) << 2 | 3);
    let count = varint_len(u16::MAX as u64);
    let style = 1 + 2 * CELL_COLOR + MAX_VARINT_BYTES + UPDATE_ITEM_HEADROOM;
    let cell = index + count + string(MAX_CELL_TEXT_BYTES);
    // A full row's length can reach every cell of the grid.
    let row_length = cells * cell + 16 + UPDATE_ITEM_HEADROOM;
    // The row's tag, length, wrap flags, prompt tag, and run count.
    let row = 1
        + varint_len(row_length as u64)
        + 2
        + 1
        + count
        + UPDATE_ITEM_HEADROOM;
    update_header_bytes() + cells * (style + cell) + rows * row
}

/// A status record: revisions, the title, six metadata strings, the
/// lifecycle, bells, the failure log, the command records, and progress,
/// every text at [`MAX_STATUS_TEXT_BYTES`].
const fn status_record_cap() -> usize {
    let text = string(MAX_STATUS_TEXT_BYTES);
    let optional_text = optional_string(MAX_STATUS_TEXT_BYTES);
    // A directory: option tag, record length, host, path, and local flag.
    let directory =
        1 + record_prefix(2 * MAX_STATUS_TEXT_BYTES) + optional_text + text + 1;
    let metadata = record_prefix(8 * MAX_STATUS_TEXT_BYTES)
        + 2 * directory
        + 2 * optional_text
        + APPENDED_HEADROOM;
    // The tag and an exit status record: length, code option, success.
    let lifecycle = 1 + 1 + 1 + MAX_VARINT_BYTES + 1 + APPENDED_HEADROOM;
    let failure = record_prefix(MAX_STATUS_TEXT_BYTES)
        + MAX_VARINT_BYTES
        + text
        + APPENDED_HEADROOM;
    // A time: seconds and nanoseconds, behind an option tag at most.
    let time = 1 + 2 * MAX_VARINT_BYTES;
    let command = record_prefix(MAX_STATUS_TEXT_BYTES)
        + MAX_VARINT_BYTES
        + optional_text
        + 1
        + 1
        + MAX_VARINT_BYTES
        + 3 * time
        + APPENDED_HEADROOM;
    let progress = 1 + 1 + 1 + 1;
    APPENDED_HEADROOM
        + MAX_VARINT_BYTES
        + record_prefix(1 << 21)
        + 2 * MAX_VARINT_BYTES
        + text
        + metadata
        + lifecycle
        + MAX_VARINT_BYTES
        + MAX_VARINT_BYTES
        + TERMINAL_FAILURE_CAPACITY * failure
        + MAX_VARINT_BYTES
        + COMMAND_RECORD_CAPACITY * command
        + progress
}

/// An input record: viewer, input sequence, stamp, and the input's tag and
/// record, whose largest is text at [`INPUT_TEXT_PAYLOAD_LIMIT`].
const fn input_record_cap() -> usize {
    let stamp = 1 + MAX_VARINT_BYTES + 1 + MAX_VARINT_BYTES + APPENDED_HEADROOM;
    APPENDED_HEADROOM
        + 2 * MAX_VARINT_BYTES
        + stamp
        + 1
        + record_prefix(INPUT_TEXT_PAYLOAD_LIMIT + APPENDED_HEADROOM)
        + string(INPUT_TEXT_PAYLOAD_LIMIT)
        + APPENDED_HEADROOM
}

/// Bytes each request-sized item record, such as an environment pair or a
/// listed process, reserves for fields later minor versions append.
const REQUEST_ITEM_HEADROOM: usize = 4;

/// The larger of two sizes.
const fn max(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

/// A terminal launch record: two paths, every argument and environment
/// string's length prefix, the shared payload, and the fixed fields.
const fn launch_record_cap() -> usize {
    let prefix = varint_len(LAUNCH_PAYLOAD_LIMIT as u64);
    let strings = MAX_LAUNCH_ARGUMENTS + 2 * MAX_LAUNCH_ENVIRONMENT;
    let pairs = MAX_LAUNCH_ENVIRONMENT
        * (record_prefix(2 * LAUNCH_PAYLOAD_LIMIT) + REQUEST_ITEM_HEADROOM);
    let shell = 1 + 1 + MAX_VARINT_BYTES + APPENDED_HEADROOM;
    let sizes = 4 * varint_len(u16::MAX as u64);
    // Three colors and the 256-colour palette, three bytes each.
    let presentation = record_prefix(1 << 10) + 3 * 259 + APPENDED_HEADROOM;
    let scrollback = 1 + 5 + 1 + MAX_VARINT_BYTES + APPENDED_HEADROOM;
    record_prefix(1 << 20)
        + APPENDED_HEADROOM
        + 2 * string(MAX_LAUNCH_PATH_BYTES)
        + 2 * MAX_VARINT_BYTES
        + strings * prefix
        + LAUNCH_PAYLOAD_LIMIT
        + pairs
        + shell
        + sizes
        + presentation
        + scrollback
        + 1
}

/// A catalog invocation record: its identifier and every argument with
/// its name and a text value at the limit.
const fn invocation_record_cap() -> usize {
    let argument = record_prefix(1 << 13)
        + string(MAX_COMMAND_NAME_BYTES)
        + 1
        + record_prefix(1 << 13)
        + string(MAX_ARGUMENT_TEXT_BYTES)
        + REQUEST_ITEM_HEADROOM;
    record_prefix(1 << 20)
        + APPENDED_HEADROOM
        + string(MAX_COMMAND_NAME_BYTES)
        + MAX_VARINT_BYTES
        + MAX_COMMAND_ARGUMENTS * argument
}

/// An arrangement node: its tag and record, a pane's identity or a split's
/// axis and child count, and its share of its parent's child record and
/// weight.
const fn arrangement_node_bytes() -> usize {
    let child = record_prefix(1 << 20) + 5;
    1 + record_prefix(1 << 20)
        + MAX_VARINT_BYTES
        + 1
        + child
        + REQUEST_ITEM_HEADROOM
}

/// A request record: its identity, tag, and fields record, at the largest
/// of its kinds' worst cases: an arrangement with its expected layout, a
/// launch with its targets and name, an invocation, and a close of many
/// tabs. Every other kind is a few identities.
const fn request_record_cap() -> usize {
    let ids = 4 * MAX_VARINT_BYTES;
    let arrange = 5
        + ids
        + MAX_REQUEST_LAYOUT_ITEMS * hierarchy_item_bytes()
        + MAX_REQUEST_LAYOUT_ITEMS * arrangement_node_bytes();
    let launch =
        ids + 1 + optional_string(MAX_REQUEST_NAME_BYTES) + launch_record_cap();
    let close = 1
        + record_prefix(1 << 20)
        + 2 * MAX_VARINT_BYTES
        + MAX_CLOSE_REQUEST_TABS * MAX_VARINT_BYTES;
    let fields = max(max(arrange, launch), max(invocation_record_cap(), close));
    APPENDED_HEADROOM
        + MAX_VARINT_BYTES
        + MAX_VARINT_BYTES
        + record_prefix(1 << 20)
        + fields
}

/// A close assessment's fields: the ticket, the effect at its most tabs,
/// the confirmation flag, every busy terminal, every listed process at its
/// longest strings, and the omitted counts.
const fn assessment_bytes() -> usize {
    let effect = 1
        + record_prefix(1 << 16)
        + 2 * MAX_VARINT_BYTES
        + MAX_CLOSE_REQUEST_TABS * MAX_VARINT_BYTES;
    let terminal = record_prefix(1 << 16)
        + MAX_VARINT_BYTES
        + 1
        + record_prefix(1 << 16)
        + 1
        + REQUEST_ITEM_HEADROOM;
    let process = record_prefix(1 << 11)
        + 5
        + 1
        + string(MAX_PROCESS_COMMAND_BYTES)
        + optional_string(MAX_PROCESS_COMMAND_LINE_BYTES)
        + REQUEST_ITEM_HEADROOM;
    5 + effect
        + 1
        + 1
        + MAX_CLOSE_SUMMARY_TERMINALS * terminal
        + MAX_CLOSE_SUMMARY_PROCESSES * process
        + 2 * 5
}

/// A committed sequence: its option tag and record, the sequence, and the
/// optional digest.
const COMMITTED_BYTES: usize =
    1 + 1 + MAX_VARINT_BYTES + 1 + 8 + APPENDED_HEADROOM;

/// A result record: the request identity and outcome tag, then a success
/// at its largest response, a close assessment, or an error at its
/// longest message.
const fn result_record_cap() -> usize {
    let success = record_prefix(1 << 16)
        + 1
        + record_prefix(1 << 16)
        + assessment_bytes()
        + APPENDED_HEADROOM
        + COMMITTED_BYTES
        + APPENDED_HEADROOM;
    let error = record_prefix(1 << 11)
        + 2
        + COMMITTED_BYTES
        + string(MAX_ERROR_MESSAGE_BYTES)
        + APPENDED_HEADROOM;
    APPENDED_HEADROOM + MAX_VARINT_BYTES + 1 + max(success, error)
}

/// The most bytes of unfinished client messages a server holds: every
/// stream at the largest client message cap. `Welcome` advertises it.
#[must_use]
pub fn reassembly_budget(minor: u16) -> usize {
    crate::stream::MAX_OPEN_STREAMS
        * MessageCaps::for_minor(minor).largest(Direction::ClientToServer)
}

#[cfg(test)]
mod tests;
