//! Host effects routed to a client, and the client's permission changes.
//!
//! The server sends each effect on its reserved host-effect stream as a
//! [`HostEffect`]: a connection-scoped identity, the viewer whose recipient
//! core selected, the permission generation of the payload's kind when core
//! admitted it, the terminal, and the payload. The client owns each kind's
//! permission per viewer and numbers its changes; a
//! [`SetHostEffectPermission`] travels in order with the viewer's input,
//! and the client executes an effect only while that viewer is live and the
//! effect's generation is its current one for the kind.
//!
//! A clipboard write may carry [`CLIPBOARD_PAYLOAD_LIMIT`] bytes, so a
//! sender streams its text from the effect's own buffer after the head
//! [`host_effect_head`] encodes, rather than encoding the whole message.

use huterm_protocol::{
    ClipboardDestination, HostEffectKind, MAX_NOTIFICATION_BODY_BYTES,
    MAX_NOTIFICATION_TITLE_BYTES, TerminalId, ViewerId,
};

use crate::caps::{CLIPBOARD_PAYLOAD_LIMIT, MessageKind};
use crate::codec::{
    DecodeError, EncodeError, IdScope, Reader, Writer, varint_len,
};

/// The generation of every kind's permission when a viewer subscribes.
/// A client numbers its first change of a kind above it.
pub const INITIAL_EFFECT_GENERATION: u64 = 1;

/// The minor version that introduced an effect kind. A recipient is
/// eligible for an effect only when its connection negotiated the kind, so
/// a newer kind never selects a client that would have to drop it.
#[must_use]
pub const fn effect_kind_since(kind: HostEffectKind) -> u16 {
    match kind {
        HostEffectKind::Clipboard | HostEffectKind::Notification => 0,
    }
}

const fn kind_tag(kind: HostEffectKind) -> u64 {
    match kind {
        HostEffectKind::Clipboard => 0,
        HostEffectKind::Notification => 1,
    }
}

fn kind_from_tag(tag: u64, minor: u16) -> Result<HostEffectKind, DecodeError> {
    HostEffectKind::ALL
        .into_iter()
        .find(|kind| {
            kind_tag(*kind) == tag && effect_kind_since(*kind) <= minor
        })
        .ok_or(DecodeError::UnknownTag {
            field: "host effect kind",
            tag,
        })
}

/// A set of host-effect kinds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EffectKinds(u64);

impl EffectKinds {
    /// No kind.
    pub const NONE: Self = Self(0);

    /// Every kind.
    pub const ALL: Self = Self((1 << HostEffectKind::ALL.len()) - 1);

    /// Whether the set holds `kind`.
    #[must_use]
    pub const fn contains(self, kind: HostEffectKind) -> bool {
        self.0 & (1 << kind_tag(kind)) != 0
    }

    /// The set with `kind` added or removed.
    #[must_use]
    pub const fn with(self, kind: HostEffectKind, present: bool) -> Self {
        let bit = 1 << kind_tag(kind);
        Self(if present { self.0 | bit } else { self.0 & !bit })
    }

    /// The kinds a peer at negotiated minor version `minor` knows.
    #[must_use]
    pub fn known_at(minor: u16) -> Self {
        HostEffectKind::ALL
            .into_iter()
            .filter(|kind| effect_kind_since(*kind) <= minor)
            .fold(Self::NONE, |set, kind| set.with(kind, true))
    }

    /// The kinds in both sets.
    #[must_use]
    pub const fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

/// Writes a set of effect kinds as a bit set, one bit per kind tag.
pub(crate) fn write_kinds(w: &mut Writer, kinds: EffectKinds) {
    w.u64(kinds.0);
}

/// Reads a set of effect kinds, refusing a bit no kind known at `minor`
/// has.
pub(crate) fn read_kinds(
    r: &mut Reader<'_>,
    minor: u16,
) -> Result<EffectKinds, DecodeError> {
    let bits = r.u64()?;
    if bits & !EffectKinds::known_at(minor).0 != 0 {
        return Err(DecodeError::Invalid("host effect kinds"));
    }
    Ok(EffectKinds(bits))
}

/// What a host effect asks the client to do.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectPayload {
    /// Replace a clipboard with text, as OSC 52 asks.
    ClipboardWrite {
        /// The clipboard.
        destination: ClipboardDestination,
        /// The text, at most [`CLIPBOARD_PAYLOAD_LIMIT`] bytes.
        text: String,
    },
    /// Present a desktop notification, as OSC 9 and OSC 777 ask, as plain
    /// text.
    Notification {
        /// The program's title; empty when it sent none.
        title: String,
        /// The notification text.
        body: String,
    },
}

impl EffectPayload {
    /// The kind whose permission and generation govern this payload.
    #[must_use]
    pub fn kind(&self) -> HostEffectKind {
        self.as_ref().kind()
    }

    /// The payload, borrowed.
    #[must_use]
    pub fn as_ref(&self) -> EffectPayloadRef<'_> {
        match self {
            Self::ClipboardWrite { destination, text } => {
                EffectPayloadRef::ClipboardWrite {
                    destination: *destination,
                    text,
                }
            }
            Self::Notification { title, body } => {
                EffectPayloadRef::Notification { title, body }
            }
        }
    }
}

/// A payload borrowed from where the server holds it, such as core's
/// pending effect, so a large one encodes without a copy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectPayloadRef<'a> {
    /// See [`EffectPayload::ClipboardWrite`].
    ClipboardWrite {
        /// The clipboard.
        destination: ClipboardDestination,
        /// The text.
        text: &'a str,
    },
    /// See [`EffectPayload::Notification`].
    Notification {
        /// The title.
        title: &'a str,
        /// The body.
        body: &'a str,
    },
}

impl<'a> EffectPayloadRef<'a> {
    /// The kind whose permission and generation govern this payload.
    #[must_use]
    pub fn kind(self) -> HostEffectKind {
        match self {
            Self::ClipboardWrite { .. } => HostEffectKind::Clipboard,
            Self::Notification { .. } => HostEffectKind::Notification,
        }
    }

    /// The text a sender streams after the head [`host_effect_head`]
    /// encodes: a clipboard write's text, or nothing for a notification,
    /// which is small enough to encode whole.
    #[must_use]
    pub fn streamed(self) -> &'a str {
        match self {
            Self::ClipboardWrite { text, .. } => text,
            Self::Notification { .. } => "",
        }
    }
}

/// A host effect routed to one client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostEffect {
    /// The effect's identity on this connection.
    pub effect: u64,
    /// The viewer whose recipient core selected.
    pub viewer: ViewerId,
    /// The viewer's permission generation for the payload's effect kind
    /// when the effect was admitted. Permission is per kind, so the
    /// payload's variant names the kind this generation counts.
    pub permission_generation: u64,
    /// The terminal whose content asked for the effect.
    pub terminal: TerminalId,
    /// What to do.
    pub payload: EffectPayload,
}

/// A client's change to one effect kind's permission for one viewer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SetHostEffectPermission {
    /// The viewer.
    pub viewer: ViewerId,
    /// The kind.
    pub kind: HostEffectKind,
    /// The change's number, above every earlier one for this viewer and
    /// kind, starting above [`INITIAL_EFFECT_GENERATION`].
    pub generation: u64,
    /// Whether terminal content may use the kind through this viewer.
    pub allowed: bool,
}

fn destination_tag(destination: ClipboardDestination) -> u64 {
    match destination {
        ClipboardDestination::System => 0,
    }
}

/// Writes the fields that open a host effect's record, up to the payload.
fn write_effect_head(
    w: &mut Writer,
    scope: IdScope,
    effect: u64,
    viewer: ViewerId,
    generation: u64,
    terminal: TerminalId,
) -> Result<(), EncodeError> {
    w.u64(effect);
    w.id(scope, viewer)?;
    w.u64(generation);
    w.id(scope, terminal)?;
    Ok(())
}

/// Writes a payload's tag and record.
fn write_payload(
    w: &mut Writer,
    payload: EffectPayloadRef<'_>,
) -> Result<(), EncodeError> {
    match payload {
        EffectPayloadRef::ClipboardWrite { destination, text } => {
            w.tag(0);
            w.record(|w| {
                w.tag(destination_tag(destination));
                w.str(text, CLIPBOARD_PAYLOAD_LIMIT, "clipboard text")
            })
        }
        EffectPayloadRef::Notification { title, body } => {
            w.tag(1);
            w.record(|w| {
                w.str(
                    title,
                    MAX_NOTIFICATION_TITLE_BYTES,
                    "notification title",
                )?;
                w.str(body, MAX_NOTIFICATION_BODY_BYTES, "notification body")
            })
        }
    }
}

pub(crate) fn encode_host_effect(
    w: &mut Writer,
    scope: IdScope,
    effect: &HostEffect,
) -> Result<(), EncodeError> {
    let HostEffect {
        effect,
        viewer,
        permission_generation,
        terminal,
        payload,
    } = effect;
    w.record(|w| {
        write_effect_head(
            w,
            scope,
            *effect,
            *viewer,
            *permission_generation,
            *terminal,
        )?;
        write_payload(w, payload.as_ref())
    })
}

pub(crate) fn decode_host_effect(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<HostEffect, DecodeError> {
    r.record(|r| {
        Ok(HostEffect {
            effect: r.u64()?,
            viewer: r.id(scope)?,
            permission_generation: r.u64()?,
            terminal: r.id(scope)?,
            payload: match r.tag()? {
                0 => r.record(|r| {
                    let destination = match r.tag()? {
                        0 => ClipboardDestination::System,
                        tag => {
                            return Err(DecodeError::UnknownTag {
                                field: "clipboard destination",
                                tag,
                            });
                        }
                    };
                    Ok(EffectPayload::ClipboardWrite {
                        destination,
                        text: r.string(
                            CLIPBOARD_PAYLOAD_LIMIT,
                            "clipboard text",
                        )?,
                    })
                })?,
                1 => r.record(|r| {
                    Ok(EffectPayload::Notification {
                        title: r.string(
                            MAX_NOTIFICATION_TITLE_BYTES,
                            "notification title",
                        )?,
                        body: r.string(
                            MAX_NOTIFICATION_BODY_BYTES,
                            "notification body",
                        )?,
                    })
                })?,
                tag => {
                    return Err(DecodeError::UnknownTag {
                        field: "effect payload",
                        tag,
                    });
                }
            },
        })
    })
}

/// Encodes a whole `HostEffect` message up to the text
/// [`EffectPayloadRef::streamed`] names, which ends every payload's record
/// and so the message. The head and the text together are exactly the
/// message `encode_message` builds, so a sender streams the text from its
/// own buffer instead of copying it.
///
/// # Errors
/// Refuses a payload past its limits with [`EncodeError::LimitExceeded`],
/// and an identity of another scope with [`EncodeError::StaleTarget`].
pub fn host_effect_head(
    scope: IdScope,
    effect: u64,
    viewer: ViewerId,
    generation: u64,
    terminal: TerminalId,
    payload: EffectPayloadRef<'_>,
) -> Result<Vec<u8>, EncodeError> {
    let mut opening = Writer::new();
    write_effect_head(
        &mut opening,
        scope,
        effect,
        viewer,
        generation,
        terminal,
    )?;
    let mut head = Writer::new();
    head.u64(MessageKind::HostEffect.number());
    match payload {
        EffectPayloadRef::ClipboardWrite { destination, text } => {
            let len = text.len();
            if len > CLIPBOARD_PAYLOAD_LIMIT {
                return Err(EncodeError::LimitExceeded {
                    field: "clipboard text",
                });
            }
            let vlen = |value: usize| varint_len(value as u64);
            // The payload record: the destination's tag and the text.
            let payload_record = 1 + vlen(len) + len;
            let record =
                opening.len() + 1 + vlen(payload_record) + payload_record;
            head.usize(record);
            head.fixed(&opening.into_bytes());
            head.tag(0);
            head.usize(payload_record);
            head.tag(destination_tag(destination));
            head.usize(len);
            Ok(head.into_bytes())
        }
        EffectPayloadRef::Notification { .. } => {
            head.record(|w| {
                w.fixed(&opening.into_bytes());
                write_payload(w, payload)
            })?;
            Ok(head.into_bytes())
        }
    }
}

/// Encodes a permission change at negotiated minor version `minor`.
///
/// # Errors
/// Refuses a kind `minor` does not know with [`EncodeError::Unsupported`].
pub(crate) fn encode_set_permission(
    w: &mut Writer,
    scope: IdScope,
    change: SetHostEffectPermission,
    minor: u16,
) -> Result<(), EncodeError> {
    let SetHostEffectPermission {
        viewer,
        kind,
        generation,
        allowed,
    } = change;
    if effect_kind_since(kind) > minor {
        return Err(EncodeError::Unsupported);
    }
    w.record(|w| {
        w.id(scope, viewer)?;
        w.tag(kind_tag(kind));
        w.u64(generation);
        w.bool(allowed);
        Ok(())
    })
}

pub(crate) fn decode_set_permission(
    r: &mut Reader<'_>,
    scope: IdScope,
    minor: u16,
) -> Result<SetHostEffectPermission, DecodeError> {
    r.record(|r| {
        Ok(SetHostEffectPermission {
            viewer: r.id(scope)?,
            kind: kind_from_tag(r.tag()?, minor)?,
            generation: r.u64()?,
            allowed: r.bool()?,
        })
    })
}

#[cfg(test)]
mod tests;
