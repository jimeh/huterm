use super::*;
use crate::caps::MessageCaps;
use crate::frame::DEFAULT_MAX_FRAME;
use crate::message::{Message, decode_message, encode_message};
use crate::vectors;

fn offer(minor: u16, features: &[u32]) -> ServerOffer {
    let welcome = vectors::welcome();
    ServerOffer {
        minor,
        features: features.iter().copied().map(Feature).collect(),
        max_frame: DEFAULT_MAX_FRAME,
        incarnation: welcome.incarnation,
        server_id: welcome.server_id,
        host_name: welcome.host_name,
        socket_name: welcome.socket_name,
        build_version: welcome.build_version,
        process_id: welcome.process_id,
    }
}

fn hello_bytes(hello: &Hello) -> Vec<u8> {
    encode_message(
        &Message::Hello(hello.clone()),
        vectors::scope(),
        MessageCaps::for_minor(0),
    )
    .unwrap()
}

#[test]
fn negotiation_takes_the_smaller_minor_the_shared_features_and_the_smaller_frame()
 {
    let hello = Hello {
        minor: 3,
        features: [Feature(1), Feature(2), Feature(5)].into(),
        max_frame: 256 << 10,
        ..vectors::hello()
    };
    let welcome =
        negotiate(&hello_bytes(&hello), &offer(1, &[2, 5, 9])).unwrap();
    assert_eq!(welcome.minor, 1);
    assert_eq!(welcome.features, [Feature(2), Feature(5)].into());
    assert_eq!(welcome.max_frame, 256 << 10);
    assert_eq!(welcome.profile, TransportProfile::local(1));
    let negotiated = accept_welcome(&hello, &welcome).unwrap();
    assert_eq!(negotiated.minor, 1);
    assert_eq!(negotiated.caps, MessageCaps::for_minor(1));
    assert_eq!(negotiated.incarnation, welcome.incarnation);
    // An older client against a newer server speaks the client's minor.
    let older = Hello {
        minor: 0,
        ..hello.clone()
    };
    assert_eq!(
        negotiate(&hello_bytes(&older), &offer(4, &[]))
            .unwrap()
            .minor,
        0
    );
}

#[test]
fn a_client_of_another_major_reads_a_version_reject_from_frozen_fields_alone() {
    // Major 2's Hello keeps the frozen kind and leading versions, then may
    // use any encoding for the rest.
    let mut w = Writer::new();
    w.u64(MessageKind::Hello.number());
    w.record(|w| {
        w.u16(2);
        w.u16(7);
        w.fixed(&[0xff; 12]);
        Ok(())
    })
    .unwrap();
    let foreign = w.into_bytes();
    assert_eq!(hello_versions(&foreign), Ok((2, 7)));
    let reject = negotiate(&foreign, &offer(0, &[])).unwrap_err();
    assert_eq!(
        reject.reason,
        RejectReason::IncompatibleMajor {
            major: MAJOR,
            minor: 0
        }
    );
    // The client reads it through the frozen Reject encoding.
    let bytes = encode_message(
        &Message::Reject(reject.clone()),
        vectors::scope(),
        MessageCaps::for_minor(0),
    )
    .unwrap();
    // Kind 2, then the record length, then reason tag 0: all frozen.
    assert_eq!((bytes[0], bytes[2]), (2, 0));
    assert_eq!(
        decode_message(&bytes, Direction::ServerToClient, vectors::context(0)),
        Ok(Message::Reject(reject))
    );
}

#[test]
fn malformed_hellos_and_other_first_messages_are_rejected() {
    let small = Hello {
        max_frame: MIN_MAX_FRAME - 1,
        ..vectors::hello()
    };
    let mut garbled = hello_bytes(&vectors::hello());
    let last = garbled.len() - 1;
    garbled[last] = 0xff;
    let mut unordered = Writer::new();
    unordered.u64(MessageKind::Hello.number());
    unordered
        .record(|w| {
            w.u16(MAJOR);
            w.u16(0);
            w.u64(2);
            w.u32(5);
            w.u32(1);
            Ok(())
        })
        .unwrap();
    let welcome = encode_message(
        &Message::Welcome(vectors::welcome()),
        vectors::scope(),
        MessageCaps::for_minor(0),
    )
    .unwrap();
    for bytes in [
        hello_bytes(&small),
        garbled,
        unordered.into_bytes(),
        welcome,
        Vec::new(),
    ] {
        let reject = negotiate(&bytes, &offer(0, &[])).unwrap_err();
        assert_eq!(reject.reason, RejectReason::Malformed, "{bytes:?}");
    }
}

#[test]
fn a_client_refuses_a_welcome_outside_what_it_offered() {
    let hello = vectors::hello();
    let welcome = vectors::welcome();
    assert!(accept_welcome(&hello, &welcome).is_ok());
    for (welcome, error) in [
        (
            Welcome {
                major: MAJOR + 1,
                ..welcome.clone()
            },
            HandshakeError::MajorMismatch(MAJOR + 1),
        ),
        (
            Welcome {
                minor: hello.minor + 1,
                ..welcome.clone()
            },
            HandshakeError::MinorAboveOwn(hello.minor + 1),
        ),
        (
            Welcome {
                features: [Feature(77)].into(),
                ..welcome.clone()
            },
            HandshakeError::UnofferedFeature(Feature(77)),
        ),
        (
            Welcome {
                max_frame: hello.max_frame + 1,
                ..welcome.clone()
            },
            HandshakeError::InvalidMaxFrame(hello.max_frame + 1),
        ),
        (
            Welcome {
                max_frame: MIN_MAX_FRAME - 1,
                ..welcome
            },
            HandshakeError::InvalidMaxFrame(MIN_MAX_FRAME - 1),
        ),
    ] {
        assert_eq!(accept_welcome(&hello, &welcome), Err(error));
    }
}

#[test]
fn reject_messages_past_their_limit_are_cut_at_a_character_boundary() {
    let reject = Reject {
        reason: RejectReason::ServerBusy,
        message: "é".repeat(MAX_REJECT_MESSAGE_BYTES),
    };
    let bytes = encode_message(
        &Message::Reject(reject),
        vectors::scope(),
        MessageCaps::for_minor(0),
    )
    .unwrap();
    let Ok(Message::Reject(decoded)) =
        decode_message(&bytes, Direction::ServerToClient, vectors::context(0))
    else {
        panic!("the reject decodes");
    };
    assert_eq!(decoded.message, "é".repeat(MAX_REJECT_MESSAGE_BYTES / 2));
}
