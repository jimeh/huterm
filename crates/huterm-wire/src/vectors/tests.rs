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

fn distinct<T: Eq + std::hash::Hash>(values: Vec<T>) -> usize {
    values.into_iter().collect::<HashSet<_>>().len()
}

#[test]
fn request_vectors_cover_every_request_variant() {
    use crate::request::{Request, RequestKind};
    let vectors = minor_0_messages();
    let requests: Vec<&Request> = vectors
        .iter()
        .filter_map(|vector| match &vector.message {
            Message::Request(message) => Some(&message.request),
            _ => None,
        })
        .collect();
    let kinds: HashSet<_> =
        requests.iter().map(|request| request.kind()).collect();
    let minor_0 = RequestKind::ALL.iter().filter(|kind| kind.since() == 0);
    assert_eq!(kinds.len(), minor_0.count());
    assert_eq!(
        distinct(
            requests
                .iter()
                .filter_map(|request| match request {
                    Request::PrepareClose { request } =>
                        Some(discriminant(request)),
                    _ => None,
                })
                .collect()
        ),
        8
    );
    assert_eq!(
        distinct(
            requests
                .iter()
                .filter_map(|request| match request {
                    Request::MovePane { to, .. } => Some(discriminant(to)),
                    _ => None,
                })
                .collect()
        ),
        2
    );
    assert_eq!(
        distinct(
            requests
                .iter()
                .filter_map(|request| match request {
                    Request::ExecuteCommand { invocation } => {
                        Some(discriminant(invocation))
                    }
                    _ => None,
                })
                .collect()
        ),
        2
    );
}

#[test]
fn result_vectors_cover_every_response_variant() {
    use crate::response::{Response, TerminalJobs};
    let vectors = minor_0_messages();
    let responses: Vec<&Response> = vectors
        .iter()
        .filter_map(|vector| match &vector.message {
            Message::Result(result) | Message::LargeResult(result) => {
                result.outcome.as_ref().ok()
            }
            _ => None,
        })
        .map(|success| &success.response)
        .collect();
    assert_eq!(
        distinct(
            responses
                .iter()
                .map(|response| discriminant(*response))
                .collect()
        ),
        17
    );
    let assessments: Vec<_> = responses
        .iter()
        .filter_map(|response| match response {
            Response::CloseAssessment(assessment) => Some(assessment),
            _ => None,
        })
        .collect();
    assert_eq!(
        distinct(
            assessments
                .iter()
                .map(|a| discriminant(&a.effect))
                .collect()
        ),
        8
    );
    assert_eq!(
        distinct(
            assessments
                .iter()
                .flat_map(|a| &a.busy)
                .map(|busy| discriminant(&busy.jobs))
                .collect()
        ),
        2
    );
    assert!(
        assessments
            .iter()
            .flat_map(|a| &a.busy)
            .any(|busy| matches!(
                &busy.jobs,
                TerminalJobs::Running(processes) if processes.len() > 1
            ))
    );
    assert!(vectors.iter().any(|vector| matches!(
        &vector.message,
        Message::Result(result) if result.outcome.is_err()
    )));
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
