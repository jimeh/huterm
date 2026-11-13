use huterm_protocol::RuntimeId;

use super::*;
use crate::caps::{Direction, MessageCaps, MessageKind};
use crate::error::ErrorCode;
use crate::frame::{Frame, MIN_MAX_FRAME};
use crate::message::{Message, decode_message, encode_message};
use crate::request::MAX_CLOSE_REQUEST_TABS;
use crate::stream::MAX_OPEN_STREAMS;
use crate::vectors::{self, RUNTIME};

const CAPS: MessageCaps = MessageCaps::for_minor(0);

fn far<T: crate::codec::WireId>(offset: u64) -> T {
    T::scoped(RUNTIME, u64::MAX - offset)
}

fn process() -> ProcessSummary {
    ProcessSummary {
        pid: u32::MAX,
        foreground: true,
        command: "c".repeat(MAX_PROCESS_COMMAND_BYTES),
        command_line: Some("l".repeat(MAX_PROCESS_COMMAND_LINE_BYTES)),
    }
}

/// A close assessment at every limit: the effect at its most tabs, every
/// busy terminal, and every process at its longest strings, all on the
/// first terminal.
fn assessment_at_limits() -> CloseAssessment {
    let mut busy: Vec<BusyTerminal> = (0..MAX_CLOSE_SUMMARY_TERMINALS)
        .map(|index| BusyTerminal {
            terminal: far(u64::try_from(index).unwrap()),
            jobs: TerminalJobs::Running(Vec::new()),
        })
        .collect();
    busy[0].jobs =
        TerminalJobs::Running(vec![process(); MAX_CLOSE_SUMMARY_PROCESSES]);
    CloseAssessment {
        ticket: u32::MAX,
        effect: CloseEffect::Tabs {
            workspace: far(0),
            tabs: (0..u64::try_from(MAX_CLOSE_REQUEST_TABS).unwrap())
                .map(far)
                .collect(),
        },
        needs_confirmation: true,
        busy,
        omitted_terminals: u32::MAX,
        omitted_processes: u32::MAX,
    }
}

fn result(response: Response) -> Message {
    Message::Result(RequestResult {
        request: RequestId::new(u64::MAX).unwrap(),
        outcome: Ok(Success {
            response,
            committed: Some(Committed {
                seq: u64::MAX,
                digest: Some(u64::MAX),
            }),
        }),
    })
}

#[test]
fn a_close_assessment_at_its_limits_fits_one_frame_and_one_more_is_refused() {
    let cap = CAPS.cap(MessageKind::Result).unwrap();
    let frame = Frame::capacity(
        u32::try_from(MAX_OPEN_STREAMS).unwrap() - 1,
        MIN_MAX_FRAME,
    );
    let message = result(Response::CloseAssessment(assessment_at_limits()));
    let bytes = encode_message(&message, vectors::scope(), CAPS).unwrap();
    assert!(bytes.len() <= cap, "{} bytes over {cap}", bytes.len());
    // An assessment still fits one frame, so a close never waits behind
    // its own result.
    assert!(bytes.len() <= frame, "{} bytes over {frame}", bytes.len());
    assert_eq!(
        decode_message(&bytes, Direction::ServerToClient, vectors::context(0)),
        Ok(message)
    );
    let over = |change: fn(&mut CloseAssessment)| {
        let mut assessment = assessment_at_limits();
        change(&mut assessment);
        encode_message(
            &result(Response::CloseAssessment(assessment)),
            vectors::scope(),
            CAPS,
        )
    };
    // One more process, even on another terminal.
    assert_eq!(
        over(|assessment| {
            assessment.busy[1].jobs = TerminalJobs::Running(vec![process()]);
        }),
        Err(EncodeError::LimitExceeded {
            field: "close processes"
        })
    );
    assert_eq!(
        over(|assessment| assessment.busy.push(assessment.busy[1].clone())),
        Err(EncodeError::LimitExceeded {
            field: "busy terminals"
        })
    );
    assert_eq!(
        over(|assessment| {
            if let TerminalJobs::Running(processes) =
                &mut assessment.busy[0].jobs
            {
                processes[0].command.push('c');
            }
        }),
        Err(EncodeError::LimitExceeded {
            field: "process command"
        })
    );
}

#[test]
fn every_error_at_its_longest_fits_the_result_cap() {
    let message = Message::Result(RequestResult {
        request: RequestId::new(u64::MAX).unwrap(),
        outcome: Err(WireError {
            committed: Some(Committed {
                seq: u64::MAX,
                digest: Some(u64::MAX),
            }),
            ..WireError::new(
                ErrorCode::Internal,
                "e".repeat(crate::error::MAX_ERROR_MESSAGE_BYTES),
            )
        }),
    });
    let bytes = encode_message(&message, vectors::scope(), CAPS).unwrap();
    assert!(bytes.len() <= CAPS.cap(MessageKind::Result).unwrap());
}

#[test]
fn listed_processes_past_the_limit_across_terminals_do_not_decode() {
    // Two terminals of processes that together pass the limit.
    let scope = vectors::scope();
    let mut w = Writer::new();
    w.u32(1);
    encode_close_effect(&mut w, scope, &CloseEffect::Application).unwrap();
    w.bool(true);
    let half = MAX_CLOSE_SUMMARY_PROCESSES / 2 + 1;
    w.seq(0..2_u32, 2, "busy", |w, terminal| {
        w.record(|w| {
            w.u64(u64::from(terminal) + 1);
            w.tag(1);
            w.record(|w| {
                w.seq(0..half, half, "processes", |w, _| {
                    encode_process(
                        w,
                        &ProcessSummary {
                            command_line: None,
                            ..process()
                        },
                    )
                })
            })
        })
    })
    .unwrap();
    w.u32(0);
    w.u32(0);
    let bytes = w.into_bytes();
    assert_eq!(
        decode_assessment(&mut Reader::new(&bytes), scope),
        Err(DecodeError::TooLong {
            field: "close processes"
        })
    );
}

#[test]
fn an_identity_of_another_runtime_refuses_to_encode_in_a_response() {
    let foreign = result(Response::Session(SessionId::in_runtime(
        RuntimeId::new(99),
        1,
    )));
    assert_eq!(
        encode_message(&foreign, vectors::scope(), CAPS),
        Err(EncodeError::StaleTarget)
    );
}

#[test]
fn an_unknown_response_tag_is_a_typed_error() {
    let bytes = [99, 0];
    assert_eq!(
        decode_response(&mut Reader::new(&bytes), vectors::scope()),
        Err(DecodeError::UnknownTag {
            field: "response",
            tag: 99
        })
    );
}

fn large(response: Response) -> Message {
    let Message::Result(result) = result(response) else {
        unreachable!("`result` builds a result");
    };
    Message::LargeResult(result)
}

#[test]
fn a_link_at_its_limits_fits_the_result_cap_and_a_longer_one_reports_a_scan_limit()
 {
    use huterm_protocol::{
        CellText, LinkCell, LinkLookup, LinkSource, MAX_CELL_TEXT_BYTES,
        MousePosition, TerminalLink,
    };

    use crate::viewer::{
        MAX_LINK_CELLS, MAX_LINK_DESTINATION_BYTES, MAX_LINK_TEXT_BYTES,
    };
    let cell = |index: usize, text: &str| LinkCell {
        position: MousePosition {
            column: u32::MAX - u32::try_from(index).unwrap(),
            row: u32::MAX,
        },
        text: CellText::from(text),
    };
    // Every cell at the widest position, and the shared text limit spent
    // on the first cells at the cell text limit.
    let full = MAX_LINK_TEXT_BYTES / MAX_CELL_TEXT_BYTES;
    let long = "a".repeat(MAX_CELL_TEXT_BYTES);
    let link = |cells: usize| TerminalLink {
        destination: "d".repeat(MAX_LINK_DESTINATION_BYTES),
        source: LinkSource::Osc8,
        cells: (0..cells)
            .map(|index| cell(index, if index < full { &long } else { "" }))
            .collect(),
    };
    let message = result(Response::Snapshot {
        link: Some(LinkLookup::Match(link(MAX_LINK_CELLS))),
    });
    let bytes = encode_message(&message, vectors::scope(), CAPS).unwrap();
    let cap = CAPS.cap(MessageKind::Result).unwrap();
    assert!(bytes.len() <= cap, "{} bytes over {cap}", bytes.len());
    assert_eq!(
        decode_message(&bytes, Direction::ServerToClient, vectors::context(0)),
        Ok(message)
    );
    let past = result(Response::Snapshot {
        link: Some(LinkLookup::Match(link(MAX_LINK_CELLS + 1))),
    });
    let bytes = encode_message(&past, vectors::scope(), CAPS).unwrap();
    assert_eq!(
        decode_message(&bytes, Direction::ServerToClient, vectors::context(0)),
        Ok(result(Response::Snapshot {
            link: Some(LinkLookup::ScanLimit),
        }))
    );
}

#[test]
fn selection_text_at_its_limit_fits_a_large_result_and_one_byte_more_is_refused()
 {
    use crate::caps::SELECTION_PAYLOAD_LIMIT;
    let text = |len| Response::Text {
        text: Some("s".repeat(len)),
    };
    let message = large(text(SELECTION_PAYLOAD_LIMIT));
    let bytes = encode_message(&message, vectors::scope(), CAPS).unwrap();
    let cap = CAPS.cap(MessageKind::LargeResult).unwrap();
    assert!(bytes.len() <= cap, "{} bytes over {cap}", bytes.len());
    assert_eq!(
        decode_message(&bytes, Direction::ServerToClient, vectors::context(0)),
        Ok(message)
    );
    assert_eq!(
        encode_message(
            &large(text(SELECTION_PAYLOAD_LIMIT + 1)),
            vectors::scope(),
            CAPS
        ),
        Err(crate::codec::EncodeError::LimitExceeded {
            field: "selection text"
        })
    );
    // An ordinary result refuses selection text past its own cap.
    let past = CAPS.cap(MessageKind::Result).unwrap();
    assert!(matches!(
        encode_message(&result(text(past)), vectors::scope(), CAPS),
        Err(crate::codec::EncodeError::LimitExceeded { field: "message" })
    ));
}

#[test]
fn a_text_result_split_around_its_text_is_the_large_result_message() {
    use crate::request::RequestId;
    for (request, len) in
        [(1, 0), (7, 1), (200, 127), (u64::MAX, 128), (9, 70_000)]
    {
        let text = "t".repeat(len);
        let request = RequestId::new(request).unwrap();
        let (head, tail) = text_result_parts(request, &text).unwrap();
        let mut bytes = head;
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(&tail);
        let message = Message::LargeResult(RequestResult {
            request,
            outcome: Ok(Success {
                response: Response::Text { text: Some(text) },
                committed: None,
            }),
        });
        assert_eq!(
            bytes,
            encode_message(&message, vectors::scope(), CAPS).unwrap(),
            "{len} bytes"
        );
    }
}
