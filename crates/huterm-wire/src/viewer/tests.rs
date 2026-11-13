use huterm_protocol::{
    InputStamp, Modifiers, TerminalInput, TerminalKey, ViewerId,
};

use super::*;
use crate::caps::{Direction, MessageCaps, MessageKind};
use crate::message::{Message, decode_message, encode_message};
use crate::vectors;

const CAPS: MessageCaps = MessageCaps::for_minor(0);

fn input(input: TerminalInput) -> Message {
    Message::Input(Input {
        viewer: ViewerId::in_runtime(vectors::RUNTIME, u64::MAX),
        seq: u64::MAX,
        stamp: InputStamp {
            scrolls: u64::MAX,
            geometry: Some(u64::MAX),
        },
        input,
    })
}

fn round_trips(message: &Message) {
    let bytes = encode_message(message, vectors::scope(), CAPS).unwrap();
    assert!(bytes.len() <= CAPS.cap(message.kind()).unwrap());
    assert_eq!(
        decode_message(&bytes, Direction::ClientToServer, vectors::context(0))
            .as_ref(),
        Ok(message)
    );
}

#[test]
fn text_input_at_its_limit_fits_and_one_byte_more_is_refused_locally() {
    round_trips(&input(TerminalInput::Text(
        "a".repeat(INPUT_TEXT_PAYLOAD_LIMIT),
    )));
    assert_eq!(
        encode_message(
            &input(TerminalInput::Text(
                "a".repeat(INPUT_TEXT_PAYLOAD_LIMIT + 1)
            )),
            vectors::scope(),
            CAPS
        ),
        Err(EncodeError::LimitExceeded {
            field: "input text"
        })
    );
    assert_eq!(
        encode_message(
            &input(TerminalInput::Character {
                text: "a".repeat(MAX_CHARACTER_TEXT_BYTES + 1),
                meta: false,
                key: None,
            }),
            vectors::scope(),
            CAPS
        ),
        Err(EncodeError::LimitExceeded {
            field: "character text"
        })
    );
}

#[test]
fn a_paste_never_travels_as_an_input() {
    assert_eq!(
        encode_message(
            &input(TerminalInput::Paste("text".into())),
            vectors::scope(),
            CAPS
        ),
        Err(EncodeError::Invalid("paste in an input message"))
    );
}

#[test]
fn every_key_round_trips() {
    for key in KEYS {
        round_trips(&input(TerminalInput::Key {
            key,
            modifiers: Modifiers {
                control: true,
                alt: true,
                shift: true,
            },
        }));
    }
    assert_eq!(KEYS.last(), Some(&TerminalKey::F12));
}

#[test]
fn credit_grants_outside_one_to_the_maximum_are_rejected() {
    let credit = |grant| {
        Message::Credit(Credit {
            viewer: vectors::viewer(),
            grant,
            applied: 0,
        })
    };
    round_trips(&credit(MAX_VIEWER_CREDIT));
    for grant in [0, MAX_VIEWER_CREDIT + 1] {
        assert_eq!(
            encode_message(&credit(grant), vectors::scope(), CAPS),
            Err(EncodeError::Invalid("credit grant"))
        );
        let mut w = Writer::new();
        w.u64(MessageKind::Credit.number());
        w.record(|w| {
            w.u64(30);
            w.u32(grant);
            w.u64(0);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            decode_message(
                &w.into_bytes(),
                Direction::ClientToServer,
                vectors::context(0)
            ),
            Err(DecodeError::Invalid("credit grant"))
        );
    }
}
