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
    MAX_HIERARCHY_ITEMS, MAX_NAME_BYTES, MAX_SPLIT_CHILDREN,
};

use crate::codec::{MAX_VARINT_BYTES, varint_len};
use crate::frame::HANDSHAKE_FRAME_LIMIT;

/// The most unacknowledged input bytes one viewer may have outstanding.
pub const VIEWER_INPUT_BUDGET: usize = 1 << 20;

/// The largest paste a client sends as one message: one viewer's whole
/// input budget.
pub const PASTE_PAYLOAD_LIMIT: usize = VIEWER_INPUT_BUDGET;

/// The largest clipboard text one host effect carries, the Kitty
/// protocol's minimum write size.
pub const CLIPBOARD_PAYLOAD_LIMIT: usize = 64 << 20;

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
}

impl MessageKind {
    /// Every kind, in number order.
    pub const ALL: [Self; 8] = [
        Self::Hello,
        Self::Welcome,
        Self::Reject,
        Self::HierarchySnapshot,
        Self::HierarchyEvent,
        Self::HierarchyResync,
        Self::Paste,
        Self::HostEffect,
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
            Self::Hello | Self::Paste => Direction::ClientToServer,
            Self::Welcome
            | Self::Reject
            | Self::HierarchySnapshot
            | Self::HierarchyEvent
            | Self::HierarchyResync
            | Self::HostEffect => Direction::ServerToClient,
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
        })
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

/// A paste record: viewer, input sequence, and the text.
const fn paste_record_cap() -> usize {
    APPENDED_HEADROOM + 2 * MAX_VARINT_BYTES + string(PASTE_PAYLOAD_LIMIT)
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

/// The most bytes of unfinished client messages a server holds: every
/// stream at the largest client message cap. `Welcome` advertises it.
#[must_use]
pub fn reassembly_budget(minor: u16) -> usize {
    crate::stream::MAX_OPEN_STREAMS
        * MessageCaps::for_minor(minor).largest(Direction::ClientToServer)
}

#[cfg(test)]
mod tests;
