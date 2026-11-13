//! Whole messages: a kind varint followed by that kind's record.
//!
//! [`encode_message`] refuses a payload past its limit locally, before any
//! byte is sent, and checks every message against its kind's cap.
//! [`decode_message`] accepts only the kinds the receiving side may get and
//! rejects trailing bytes.

use huterm_protocol::{
    ClipboardDestination, HostEffectKind, InputStamp, TerminalId, ViewerId,
};

use crate::caps::{
    CLIPBOARD_PAYLOAD_LIMIT, Direction, MessageCaps, MessageKind,
    PASTE_PAYLOAD_LIMIT,
};
use crate::codec::{DecodeError, EncodeError, IdScope, Reader, Writer};
use crate::handshake::{
    Hello, Reject, Welcome, decode_hello, decode_reject, decode_welcome,
    encode_hello, encode_reject, encode_welcome,
};
use crate::hierarchy::{
    HierarchyContext, HierarchySnapshot, SequencedEvent, decode_sequenced,
    decode_snapshot, encode_sequenced, encode_snapshot,
};
use crate::request::{
    Cancel, RequestMessage, decode_cancel, decode_request, encode_cancel,
    encode_request,
};
use crate::response::{RequestResult, decode_result, encode_result};
use crate::update::{
    TerminalUpdate, decode_update, encode_update, read_grid, style_flags,
};
use crate::viewer::{
    Credit, Input, InputAck, ResyncViewer, ViewerStatus, decode_credit,
    decode_input, decode_input_ack, decode_resync, decode_stamp,
    decode_viewer_status, encode_credit, encode_input, encode_input_ack,
    encode_resync, encode_stamp, encode_viewer_status,
};

/// A viewer's paste: text the server admits as one bracketed input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Paste {
    /// The viewer that pastes.
    pub viewer: ViewerId,
    /// The viewer's input sequence number for this input.
    pub seq: u64,
    /// What the client knew when the user pasted.
    pub stamp: InputStamp,
    /// The pasted text, at most [`PASTE_PAYLOAD_LIMIT`] bytes.
    pub text: String,
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
}

impl EffectPayload {
    /// The kind whose permission and generation govern this payload.
    #[must_use]
    pub fn kind(&self) -> HostEffectKind {
        match self {
            Self::ClipboardWrite { .. } => HostEffectKind::Clipboard,
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
    /// How long the server waits for an answer, in milliseconds, for an
    /// effect that needs one.
    pub response_deadline_ms: Option<u64>,
}

/// One message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Message {
    /// The client's first message.
    Hello(Hello),
    /// The server's acceptance.
    Welcome(Welcome),
    /// The server's refusal.
    Reject(Reject),
    /// The hierarchy when a subscription starts.
    HierarchySnapshot(HierarchySnapshot),
    /// One sequenced hierarchy event.
    HierarchyEvent(SequencedEvent),
    /// A fresh hierarchy that replaces events the client did not get.
    HierarchyResync(HierarchySnapshot),
    /// A viewer's paste.
    Paste(Paste),
    /// A host effect.
    HostEffect(HostEffect),
    /// A viewer's rows and metadata.
    TerminalUpdate(TerminalUpdate),
    /// A viewer's terminal status.
    ViewerStatus(ViewerStatus),
    /// One ordered input other than a paste.
    Input(Input),
    /// A viewer's highest admitted input.
    InputAck(InputAck),
    /// Credit for a viewer's updates.
    Credit(Credit),
    /// A request for a complete snapshot.
    ResyncViewer(ResyncViewer),
    /// A request.
    Request(RequestMessage),
    /// A request's result.
    Result(RequestResult),
    /// A request to abandon a request that has not started.
    Cancel(Cancel),
}

impl Message {
    /// The message's kind.
    #[must_use]
    pub fn kind(&self) -> MessageKind {
        match self {
            Self::Hello(_) => MessageKind::Hello,
            Self::Welcome(_) => MessageKind::Welcome,
            Self::Reject(_) => MessageKind::Reject,
            Self::HierarchySnapshot(_) => MessageKind::HierarchySnapshot,
            Self::HierarchyEvent(_) => MessageKind::HierarchyEvent,
            Self::HierarchyResync(_) => MessageKind::HierarchyResync,
            Self::Paste(_) => MessageKind::Paste,
            Self::HostEffect(_) => MessageKind::HostEffect,
            Self::TerminalUpdate(_) => MessageKind::TerminalUpdate,
            Self::ViewerStatus(_) => MessageKind::ViewerStatus,
            Self::Input(_) => MessageKind::Input,
            Self::InputAck(_) => MessageKind::InputAck,
            Self::Credit(_) => MessageKind::Credit,
            Self::ResyncViewer(_) => MessageKind::ResyncViewer,
            Self::Request(_) => MessageKind::Request,
            Self::Result(_) => MessageKind::Result,
            Self::Cancel(_) => MessageKind::Cancel,
        }
    }
}

/// What a decoder needs from its connection: the identity scope and
/// hierarchy stream, and the negotiated caps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodeContext {
    /// The connection's hierarchy stream, whose runtime scopes every
    /// identity.
    pub hierarchy: HierarchyContext,
    /// The negotiated caps.
    pub caps: MessageCaps,
}

fn destination_tag(destination: ClipboardDestination) -> u64 {
    match destination {
        ClipboardDestination::System => 0,
    }
}

fn encode_paste(
    w: &mut Writer,
    scope: IdScope,
    paste: &Paste,
) -> Result<(), EncodeError> {
    let Paste {
        viewer,
        seq,
        stamp,
        text,
    } = paste;
    w.record(|w| {
        w.id(scope, *viewer)?;
        w.u64(*seq);
        encode_stamp(w, *stamp)?;
        w.str(text, PASTE_PAYLOAD_LIMIT, "paste")
    })
}

fn decode_paste(
    r: &mut Reader<'_>,
    scope: IdScope,
) -> Result<Paste, DecodeError> {
    r.record(|r| {
        Ok(Paste {
            viewer: r.id(scope)?,
            seq: r.u64()?,
            stamp: decode_stamp(r)?,
            text: r.string(PASTE_PAYLOAD_LIMIT, "paste")?,
        })
    })
}

fn encode_host_effect(
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
        response_deadline_ms,
    } = effect;
    w.record(|w| {
        w.u64(*effect);
        w.id(scope, *viewer)?;
        w.u64(*permission_generation);
        w.id(scope, *terminal)?;
        match payload {
            EffectPayload::ClipboardWrite { destination, text } => {
                w.tag(0);
                w.record(|w| {
                    w.tag(destination_tag(*destination));
                    w.str(text, CLIPBOARD_PAYLOAD_LIMIT, "clipboard text")
                })?;
            }
        }
        w.option(*response_deadline_ms, |w, deadline| {
            w.u64(deadline);
            Ok(())
        })
    })
}

fn decode_host_effect(
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
                tag => {
                    return Err(DecodeError::UnknownTag {
                        field: "effect payload",
                        tag,
                    });
                }
            },
            response_deadline_ms: r.option("response deadline", Reader::u64)?,
        })
    })
}

/// Encodes a whole message in `scope`, checking it against its kind's cap
/// under `caps`.
///
/// # Errors
/// Refuses a payload past its limit with [`EncodeError::LimitExceeded`], an
/// identity of another scope with [`EncodeError::StaleTarget`], and a kind
/// the negotiated version lacks with [`EncodeError::Unsupported`]. Nothing
/// is sent for a refused message.
pub fn encode_message(
    message: &Message,
    scope: IdScope,
    caps: MessageCaps,
) -> Result<Vec<u8>, EncodeError> {
    let kind = message.kind();
    let cap = caps.cap(kind).ok_or(EncodeError::Unsupported)?;
    let mut w = Writer::new();
    w.u64(kind.number());
    match message {
        Message::Hello(hello) => encode_hello(&mut w, hello)?,
        Message::Welcome(welcome) => encode_welcome(&mut w, welcome)?,
        Message::Reject(reject) => encode_reject(&mut w, reject)?,
        Message::HierarchySnapshot(snapshot)
        | Message::HierarchyResync(snapshot) => {
            encode_snapshot(&mut w, scope, snapshot)?;
        }
        Message::HierarchyEvent(event) => {
            encode_sequenced(&mut w, scope, event)?;
        }
        Message::Paste(paste) => encode_paste(&mut w, scope, paste)?,
        Message::HostEffect(effect) => {
            encode_host_effect(&mut w, scope, effect)?;
        }
        Message::TerminalUpdate(update) => {
            encode_update(&mut w, scope, update, style_flags(caps.minor()))?;
        }
        Message::ViewerStatus(status) => {
            encode_viewer_status(&mut w, scope, status)?;
        }
        Message::Input(input) => encode_input(&mut w, scope, input)?,
        Message::InputAck(ack) => encode_input_ack(&mut w, scope, *ack)?,
        Message::Credit(credit) => encode_credit(&mut w, scope, *credit)?,
        Message::ResyncViewer(resync) => {
            encode_resync(&mut w, scope, *resync)?;
        }
        Message::Request(request) => {
            encode_request(&mut w, scope, request, caps.minor())?;
        }
        Message::Result(result) => encode_result(&mut w, scope, result)?,
        Message::Cancel(cancel) => encode_cancel(&mut w, *cancel)?,
    }
    // An update's cap follows its grid, which the encoder already checked.
    let cap = match message {
        Message::TerminalUpdate(update) => {
            caps.update_cap(update.metadata.size).unwrap_or(cap)
        }
        _ => cap,
    };
    if w.len() > cap {
        return Err(EncodeError::LimitExceeded { field: "message" });
    }
    Ok(w.into_bytes())
}

/// Decodes a whole message that `from`'s side sent.
///
/// # Errors
/// Returns [`DecodeError::UnknownKind`] for a kind the receiver does not
/// accept from that side or at the negotiated version, or the first decode
/// failure.
pub fn decode_message(
    bytes: &[u8],
    from: Direction,
    context: DecodeContext,
) -> Result<Message, DecodeError> {
    let mut r = Reader::new(bytes);
    let number = r.u64()?;
    let kind = MessageKind::from_number(number)
        .filter(|kind| {
            kind.direction() == from && context.caps.cap(*kind).is_some()
        })
        .ok_or(DecodeError::UnknownKind(number))?;
    let scope = context.hierarchy.scope();
    let message = match kind {
        MessageKind::Hello => Message::Hello(decode_hello(&mut r)?),
        MessageKind::Welcome => Message::Welcome(decode_welcome(&mut r)?),
        MessageKind::Reject => Message::Reject(decode_reject(&mut r)?),
        MessageKind::HierarchySnapshot => Message::HierarchySnapshot(
            decode_snapshot(&mut r, context.hierarchy)?,
        ),
        MessageKind::HierarchyEvent => {
            Message::HierarchyEvent(decode_sequenced(&mut r, scope)?)
        }
        MessageKind::HierarchyResync => Message::HierarchyResync(
            decode_snapshot(&mut r, context.hierarchy)?,
        ),
        MessageKind::Paste => Message::Paste(decode_paste(&mut r, scope)?),
        MessageKind::HostEffect => {
            Message::HostEffect(decode_host_effect(&mut r, scope)?)
        }
        MessageKind::TerminalUpdate => {
            // Bytes past the cap the update's own grid allows are refused
            // before the rest decodes.
            let cap = grid_cap(r.peek(), context.caps)?;
            if bytes.len() > cap {
                return Err(DecodeError::TooLong { field: "update" });
            }
            Message::TerminalUpdate(decode_update(
                &mut r,
                scope,
                style_flags(context.caps.minor()),
            )?)
        }
        MessageKind::ViewerStatus => {
            Message::ViewerStatus(decode_viewer_status(&mut r, scope)?)
        }
        MessageKind::Input => Message::Input(decode_input(&mut r, scope)?),
        MessageKind::InputAck => {
            Message::InputAck(decode_input_ack(&mut r, scope)?)
        }
        MessageKind::Credit => Message::Credit(decode_credit(&mut r, scope)?),
        MessageKind::ResyncViewer => {
            Message::ResyncViewer(decode_resync(&mut r, scope)?)
        }
        MessageKind::Request => Message::Request(decode_request(
            &mut r,
            scope,
            context.caps.minor(),
        )?),
        MessageKind::Result => Message::Result(decode_result(&mut r, scope)?),
        MessageKind::Cancel => Message::Cancel(decode_cancel(&mut r)?),
    };
    r.finish()?;
    Ok(message)
}

/// The cap of the update whose record starts `record`: its length prefix,
/// then its grid.
fn grid_cap(record: &[u8], caps: MessageCaps) -> Result<usize, DecodeError> {
    let mut r = Reader::new(record);
    r.usize()?;
    caps.update_cap(read_grid(&mut r)?)
        .ok_or(DecodeError::Invalid("grid size"))
}

#[cfg(test)]
mod tests;
