use std::collections::HashSet;
use std::mem::discriminant;

use super::*;
use crate::codec::{Reader, Writer};
use crate::command::{
    DecodedInvocation, decode_invocation, decode_outcome, encode_invocation,
    encode_outcome,
};
use crate::error::{decode_error, encode_error};
use crate::frame::{FrameDecoder, preamble};
use crate::message::{decode_message, encode_message};

#[test]
fn every_message_vector_encodes_to_its_bytes_and_decodes_back() {
    let vectors = minor_0_messages();
    for vector in &vectors {
        let encoded =
            encode_message(&vector.message, scope(), MessageCaps::for_minor(0))
                .unwrap_or_else(|error| panic!("{}: {error}", vector.name));
        assert_eq!(encoded, vector.bytes, "{}", vector.name);
        let decoded = decode_message(vector.bytes, vector.from, context(0))
            .unwrap_or_else(|error| panic!("{}: {error}", vector.name));
        assert_eq!(decoded, vector.message, "{}", vector.name);
    }
    // One vector per event variant, every one of them distinct.
    let events: HashSet<_> = vectors
        .iter()
        .filter_map(|vector| match &vector.message {
            Message::HierarchyEvent(event) => Some(discriminant(&event.event)),
            _ => None,
        })
        .collect();
    assert_eq!(events.len(), 23);
}

#[test]
fn every_record_vector_encodes_to_its_bytes_and_decodes_back() {
    for vector in minor_0_records() {
        let mut w = Writer::new();
        let mut r = Reader::new(vector.bytes);
        let decoded = match &vector.record {
            Record::Invocation(invocation) => {
                encode_invocation(&mut w, scope(), invocation).unwrap();
                match decode_invocation(&mut r, scope()).unwrap() {
                    DecodedInvocation::Known(invocation) => {
                        Record::Invocation(invocation)
                    }
                    unknown @ DecodedInvocation::Unknown { .. } => {
                        panic!("{}: decoded {unknown:?}", vector.name)
                    }
                }
            }
            Record::Outcome(outcome) => {
                encode_outcome(&mut w, *outcome);
                Record::Outcome(decode_outcome(&mut r).unwrap())
            }
            Record::Error(error) => {
                encode_error(&mut w, error).unwrap();
                Record::Error(decode_error(&mut r).unwrap())
            }
        };
        assert_eq!(w.into_bytes(), vector.bytes, "{}", vector.name);
        assert!(r.is_empty(), "{}", vector.name);
        assert_eq!(decoded, vector.record, "{}", vector.name);
    }
}

#[test]
fn frame_and_preamble_vectors_keep_their_bytes() {
    assert_eq!(preamble(), PREAMBLE);
    for vector in frames() {
        assert_eq!(
            vector
                .frame
                .encode(crate::frame::HANDSHAKE_FRAME_LIMIT)
                .unwrap(),
            vector.bytes,
            "{}",
            vector.name
        );
        let mut decoder = FrameDecoder::new();
        decoder.push(vector.bytes);
        assert_eq!(
            decoder.next_frame().unwrap(),
            Some(vector.frame),
            "{}",
            vector.name
        );
        assert_eq!(decoder.buffered(), 0);
    }
}

#[test]
fn the_sample_hierarchy_digest_pins_the_canonical_digest_encoding() {
    assert_eq!(sample_hierarchy().digest(), SAMPLE_HIERARCHY_DIGEST);
}

#[test]
fn the_sample_view_digest_pins_the_canonical_view_digest_encoding() {
    let snapshot = sample_snapshot();
    assert_eq!(
        crate::update::view_digest(
            &crate::update::ViewMetadata::of(&snapshot),
            &snapshot.rows,
            0
        ),
        SAMPLE_VIEW_DIGEST
    );
}

#[test]
fn the_encoder_builds_the_complete_update_vector() {
    let update = crate::update::UpdateEncoder::new(viewer(), 0)
        .next(&sample_snapshot(), false)
        .unwrap();
    assert_eq!(update, sample_update());
}
