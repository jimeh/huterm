use huterm_protocol::{
    ClipboardDestination, HostEffectKind, MAX_NOTIFICATION_BODY_BYTES,
    MAX_NOTIFICATION_TITLE_BYTES, TerminalId, ViewerId,
};

use super::*;
use crate::caps::{Direction, MessageCaps};
use crate::message::{Message, decode_message, encode_message};
use crate::vectors;

const CAPS: MessageCaps = MessageCaps::for_minor(0);

fn effect(payload: EffectPayload) -> HostEffect {
    HostEffect {
        effect: u64::MAX,
        viewer: ViewerId::in_runtime(vectors::RUNTIME, u64::MAX),
        permission_generation: u64::MAX,
        terminal: TerminalId::in_runtime(vectors::RUNTIME, u64::MAX),
        payload,
    }
}

fn notification(title: &str, body: &str) -> EffectPayload {
    EffectPayload::Notification {
        title: title.into(),
        body: body.into(),
    }
}

#[test]
fn a_streamed_head_and_its_text_are_the_whole_message() {
    // Texts whose lengths take one, two, and three varint bytes, and a
    // notification, which streams nothing.
    let payloads = [0, 1, 200, 20_000]
        .map(|len| EffectPayload::ClipboardWrite {
            destination: ClipboardDestination::System,
            text: "é".repeat(len / 2) + &"a".repeat(len % 2),
        })
        .into_iter()
        .chain([notification("title", "body"), notification("", "")]);
    for payload in payloads {
        let effect = effect(payload);
        let whole = encode_message(
            &Message::HostEffect(effect.clone()),
            vectors::scope(),
            CAPS,
        )
        .unwrap();
        let payload = effect.payload.as_ref();
        let mut streamed = host_effect_head(
            vectors::scope(),
            effect.effect,
            effect.viewer,
            effect.permission_generation,
            effect.terminal,
            payload,
        )
        .unwrap();
        streamed.extend_from_slice(payload.streamed().as_bytes());
        assert_eq!(streamed, whole, "{:?}", effect.payload.kind());
    }
}

#[test]
fn a_notification_at_its_limits_round_trips_and_one_byte_more_is_refused() {
    let title = "t".repeat(MAX_NOTIFICATION_TITLE_BYTES);
    let body = "b".repeat(MAX_NOTIFICATION_BODY_BYTES);
    let at_limit = Message::HostEffect(effect(notification(&title, &body)));
    let bytes = encode_message(&at_limit, vectors::scope(), CAPS).unwrap();
    assert!(bytes.len() <= CAPS.cap(MessageKind::HostEffect).unwrap());
    assert_eq!(
        decode_message(&bytes, Direction::ServerToClient, vectors::context(0)),
        Ok(at_limit)
    );
    for (over, field) in [
        (
            notification(&format!("{title}t"), &body),
            "notification title",
        ),
        (
            notification(&title, &format!("{body}b")),
            "notification body",
        ),
    ] {
        assert_eq!(
            encode_message(
                &Message::HostEffect(effect(over)),
                vectors::scope(),
                CAPS
            ),
            Err(EncodeError::LimitExceeded { field })
        );
    }
}

#[test]
fn a_payload_names_the_kind_whose_permission_governs_it() {
    assert_eq!(
        EffectPayload::ClipboardWrite {
            destination: ClipboardDestination::System,
            text: String::new(),
        }
        .kind(),
        HostEffectKind::Clipboard
    );
    assert_eq!(notification("", "").kind(), HostEffectKind::Notification);
}

#[test]
fn a_permission_change_round_trips_and_an_unknown_kind_is_refused() {
    for kind in HostEffectKind::ALL {
        let change =
            Message::SetHostEffectPermission(SetHostEffectPermission {
                viewer: ViewerId::in_runtime(vectors::RUNTIME, 7),
                kind,
                generation: u64::MAX,
                allowed: true,
            });
        let bytes = encode_message(&change, vectors::scope(), CAPS).unwrap();
        assert_eq!(
            decode_message(
                &bytes,
                Direction::ClientToServer,
                vectors::context(0)
            ),
            Ok(change)
        );
    }
    let mut w = Writer::new();
    w.u64(MessageKind::SetHostEffectPermission.number());
    w.record(|w| {
        w.u64(7);
        w.tag(9);
        w.u64(2);
        w.bool(false);
        Ok(())
    })
    .unwrap();
    assert_eq!(
        decode_message(
            &w.into_bytes(),
            Direction::ClientToServer,
            vectors::context(0)
        ),
        Err(DecodeError::UnknownTag {
            field: "host effect kind",
            tag: 9,
        })
    );
}

#[test]
fn every_kind_is_known_at_minor_zero_and_a_kind_set_refuses_unknown_bits() {
    assert_eq!(EffectKinds::known_at(0), EffectKinds::ALL);
    let only_clipboard =
        EffectKinds::NONE.with(HostEffectKind::Clipboard, true);
    assert!(only_clipboard.contains(HostEffectKind::Clipboard));
    assert!(!only_clipboard.contains(HostEffectKind::Notification));
    assert_eq!(EffectKinds::ALL.intersect(only_clipboard), only_clipboard);
    let mut w = Writer::new();
    w.u64(0b100);
    assert_eq!(
        read_kinds(&mut Reader::new(&w.into_bytes())),
        Err(DecodeError::Invalid("host effect kinds"))
    );
}
