use huterm_protocol::RuntimeId;

use super::*;
use crate::vectors;

const CAPS: MessageCaps = MessageCaps::for_minor(0);

fn paste(text: String) -> Message {
    Message::Paste(Paste {
        viewer: ViewerId::in_runtime(vectors::RUNTIME, u64::MAX),
        seq: u64::MAX,
        stamp: huterm_protocol::InputStamp::default(),
        text,
    })
}

fn clipboard(text: String) -> Message {
    Message::HostEffect(HostEffect {
        effect: u64::MAX,
        viewer: ViewerId::in_runtime(vectors::RUNTIME, u64::MAX),
        permission_generation: u64::MAX,
        terminal: TerminalId::in_runtime(vectors::RUNTIME, u64::MAX),
        payload: EffectPayload::ClipboardWrite {
            destination: ClipboardDestination::System,
            text,
        },
        response_deadline_ms: Some(u64::MAX),
    })
}

#[test]
fn payloads_at_their_limits_fit_their_caps_and_one_byte_more_is_refused_locally()
 {
    for (message, over, kind, limit, from) in [
        (
            paste as fn(String) -> Message,
            "paste",
            MessageKind::Paste,
            PASTE_PAYLOAD_LIMIT,
            Direction::ClientToServer,
        ),
        (
            clipboard,
            "clipboard text",
            MessageKind::HostEffect,
            CLIPBOARD_PAYLOAD_LIMIT,
            Direction::ServerToClient,
        ),
    ] {
        // Every other field at its widest encoding.
        let at_limit = message("a".repeat(limit));
        let bytes = encode_message(&at_limit, vectors::scope(), CAPS).unwrap();
        assert!(bytes.len() <= CAPS.cap(kind).unwrap(), "{kind:?}");
        assert_eq!(
            decode_message(&bytes, from, vectors::context(0)),
            Ok(at_limit)
        );
        assert_eq!(
            encode_message(
                &message("a".repeat(limit + 1)),
                vectors::scope(),
                CAPS
            ),
            Err(EncodeError::LimitExceeded { field: over })
        );
    }
}

#[test]
fn a_payload_names_the_kind_whose_permission_governs_it() {
    let Message::HostEffect(effect) = clipboard(String::new()) else {
        unreachable!("clipboard builds a host effect");
    };
    assert_eq!(effect.payload.kind(), HostEffectKind::Clipboard);
}

#[test]
fn an_identity_from_another_runtime_refuses_to_encode() {
    let foreign = Message::Paste(Paste {
        viewer: ViewerId::in_runtime(RuntimeId::new(99), 1),
        seq: 1,
        stamp: huterm_protocol::InputStamp::default(),
        text: String::new(),
    });
    assert_eq!(
        encode_message(&foreign, vectors::scope(), CAPS),
        Err(EncodeError::StaleTarget)
    );
}

#[test]
fn every_vector_cut_short_or_extended_fails_with_a_typed_error() {
    for vector in vectors::minor_0_messages() {
        for end in 0..vector.bytes.len() {
            let result = decode_message(
                &vector.bytes[..end],
                vector.from,
                vectors::context(0),
            );
            assert!(result.is_err(), "{} cut at {end}", vector.name);
        }
        let mut extended = vector.bytes.to_vec();
        extended.push(0);
        assert_eq!(
            decode_message(&extended, vector.from, vectors::context(0)),
            Err(DecodeError::TrailingBytes),
            "{}",
            vector.name
        );
    }
}

#[test]
fn kinds_from_the_wrong_side_or_unknown_kinds_are_rejected() {
    let welcome = vectors::minor_0_messages().remove(1);
    assert_eq!(welcome.name, "welcome");
    assert_eq!(
        decode_message(
            welcome.bytes,
            Direction::ClientToServer,
            vectors::context(0)
        ),
        Err(DecodeError::UnknownKind(MessageKind::Welcome.number()))
    );
    assert_eq!(
        decode_message(
            &[0x7f, 0x00],
            Direction::ServerToClient,
            vectors::context(0)
        ),
        Err(DecodeError::UnknownKind(0x7f))
    );
}

#[test]
fn unknown_payload_tags_and_bad_booleans_are_typed_errors() {
    let mut effect =
        encode_message(&clipboard("x".into()), vectors::scope(), CAPS).unwrap();
    // The payload tag follows the effect, viewer, generation, and terminal
    // varints, each ten bytes at u64::MAX, after the kind and record length.
    let tag = 2 + 4 * 10;
    assert_eq!(effect[tag], 0);
    effect[tag] = 5;
    assert_eq!(
        decode_message(&effect, Direction::ServerToClient, vectors::context(0)),
        Err(DecodeError::UnknownTag {
            field: "effect payload",
            tag: 5
        })
    );
    let snapshot = &vectors::minor_0_messages()[5];
    assert_eq!(snapshot.name, "hierarchy snapshot");
    // The single tab ends with no zoom and input sync off; the second
    // session's records follow.
    let mut bad_bool = snapshot.bytes.to_vec();
    let zoom = bad_bool
        .windows(4)
        .position(|window| window == [0x00, 0x00, 0x0e, 0x0c])
        .expect(
            "the single tab's zoom and input sync precede the second session",
        );
    bad_bool[zoom + 1] = 2;
    assert_eq!(
        decode_message(
            &bad_bool,
            Direction::ServerToClient,
            vectors::context(0)
        ),
        Err(DecodeError::InvalidBool(2))
    );
}
