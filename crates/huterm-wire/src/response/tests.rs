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
    assert!(cap <= frame, "result cap {cap} over one frame of {frame}");
    let message = result(Response::CloseAssessment(assessment_at_limits()));
    let bytes = encode_message(&message, vectors::scope(), CAPS).unwrap();
    assert!(bytes.len() <= cap, "{} bytes over {cap}", bytes.len());
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
