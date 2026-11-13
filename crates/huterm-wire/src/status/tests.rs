use std::sync::Arc;
use std::time::{Duration, SystemTime};

use huterm_protocol::{
    COMMAND_RECORD_CAPACITY, CommandRecord, CommandState, ExitStatus,
    MAX_STATUS_TEXT_BYTES, TERMINAL_FAILURE_CAPACITY, TerminalDirectory,
    TerminalFailure, TerminalLifecycle, TerminalMetadata, TerminalProgress,
    TerminalStatus, ViewerId,
};

use super::*;
use crate::caps::{Direction, MessageCaps, MessageKind};
use crate::message::{Message, decode_message, encode_message};
use crate::vectors;
use crate::viewer::ViewerStatus;

const CAPS: MessageCaps = MessageCaps::for_minor(0);

/// Text of exactly `MAX_STATUS_TEXT_BYTES`, in two-byte characters.
fn longest() -> String {
    "é".repeat(MAX_STATUS_TEXT_BYTES / 2)
}

fn directory() -> TerminalDirectory {
    TerminalDirectory::new(Some(longest()), longest(), true)
}

/// Every text at its limit, every log full, every number at its widest.
fn worst_status() -> TerminalStatus {
    // The earliest time a decoder accepts with a fraction: `i64::MIN`
    // seconds and 999,999,999 nanoseconds, each at its widest varint.
    let far = SystemTime::UNIX_EPOCH - Duration::new((1 << 63) - 1, 1);
    TerminalStatus {
        revision: u64::MAX,
        title: longest(),
        metadata: TerminalMetadata::new(Some(directory()), Some(longest()))
            .with_foreground_title(Some(longest()))
            .with_shell_directory(Some(directory())),
        metadata_revision: u64::MAX,
        lifecycle: TerminalLifecycle::Exited(ExitStatus {
            code: Some(u32::MAX),
            success: false,
        }),
        bells: u64::MAX,
        failures: (0..TERMINAL_FAILURE_CAPACITY as u64)
            .map(|index| TerminalFailure {
                sequence: u64::MAX - 8 + index,
                message: longest(),
            })
            .collect(),
        commands: Arc::from(
            (0..COMMAND_RECORD_CAPACITY as u64)
                .map(|index| CommandRecord {
                    sequence: u64::MAX - 256 + index,
                    text: Some(longest()),
                    state: CommandState::Finished {
                        exit_code: Some(i32::MIN),
                    },
                    started_at: far,
                    finished_at: Some(far),
                    duration: Some(Duration::new(u64::MAX, 999_999_999)),
                })
                .collect::<Vec<_>>(),
        ),
        progress: Some(TerminalProgress::Error(Some(100))),
    }
}

fn status_message(status: TerminalStatus) -> Message {
    Message::ViewerStatus(ViewerStatus {
        viewer: ViewerId::in_runtime(vectors::RUNTIME, u64::MAX),
        status,
    })
}

#[test]
fn a_status_at_every_limit_fits_its_cap_and_round_trips() {
    let message = status_message(worst_status());
    let bytes = encode_message(&message, vectors::scope(), CAPS).unwrap();
    assert!(bytes.len() <= CAPS.cap(MessageKind::ViewerStatus).unwrap());
    assert!(bytes.len() > COMMAND_RECORD_CAPACITY * MAX_STATUS_TEXT_BYTES);
    assert_eq!(
        decode_message(&bytes, Direction::ServerToClient, vectors::context(0)),
        Ok(message)
    );
    let mut long = worst_status();
    long.title.push('x');
    assert_eq!(
        encode_message(&status_message(long), vectors::scope(), CAPS),
        Err(EncodeError::LimitExceeded { field: "title" })
    );
}

#[test]
fn a_subscription_result_with_a_status_at_every_limit_fits_the_result_cap() {
    use crate::request::RequestId;
    use crate::response::{Committed, RequestResult, Response, Success};
    for response in [
        Response::Viewer {
            viewer: ViewerId::in_runtime(vectors::RUNTIME, u64::MAX),
            status: Box::new(worst_status()),
        },
        Response::Stopped {
            status: Box::new(worst_status()),
        },
    ] {
        let message = Message::Result(RequestResult {
            request: RequestId::new(u64::MAX).unwrap(),
            outcome: Ok(Success {
                response,
                committed: Some(Committed {
                    seq: u64::MAX,
                    digest: Some(u64::MAX),
                }),
            }),
        });
        let bytes = encode_message(&message, vectors::scope(), CAPS).unwrap();
        let cap = CAPS.cap(MessageKind::Result).unwrap();
        assert!(bytes.len() <= cap, "{} bytes over {cap}", bytes.len());
        assert_eq!(
            decode_message(
                &bytes,
                Direction::ServerToClient,
                vectors::context(0)
            ),
            Ok(message)
        );
    }
}

fn status_bytes(status: &TerminalStatus) -> Vec<u8> {
    let mut w = Writer::new();
    encode_status(&mut w, status).unwrap();
    w.into_bytes()
}

/// A status's record bytes with `edit` applied to them, so a decoder sees
/// what the encoder refuses to write.
fn edited_bytes(
    status: &TerminalStatus,
    edit: impl FnOnce(&mut Vec<u8>),
) -> Vec<u8> {
    let mut bytes = status_bytes(status);
    edit(&mut bytes);
    bytes
}

/// The position of the only occurrence of `needle` in `bytes`.
fn find(bytes: &[u8], needle: &[u8]) -> usize {
    let mut found = bytes
        .windows(needle.len())
        .enumerate()
        .filter(|(_, window)| *window == needle)
        .map(|(index, _)| index);
    let index = found.next().expect("the needle occurs");
    assert_eq!(found.next(), None, "the needle occurs once");
    index
}

#[test]
fn out_of_order_logs_and_out_of_range_values_are_rejected() {
    // The encoder refuses each locally.
    let mut unordered = vectors::sample_status();
    unordered.failures.reverse();
    let refused = |status: &TerminalStatus, what| {
        assert_eq!(
            encode_status(&mut Writer::new(), status),
            Err(EncodeError::Invalid(what))
        );
    };
    refused(&unordered, "status sequence order");
    let mut repeated = vectors::sample_status();
    let mut commands = repeated.commands.to_vec();
    commands[1].sequence = commands[0].sequence;
    repeated.commands = Arc::from(commands);
    refused(&repeated, "status sequence order");
    let mut progress = vectors::sample_status();
    progress.progress = Some(TerminalProgress::Determinate(101));
    refused(&progress, "progress percentage");
    progress.progress = Some(TerminalProgress::Paused(Some(101)));
    refused(&progress, "progress percentage");

    // The decoder rejects each in bytes. The sample's failures are numbered
    // 1 ("read") and 3 ("write"), and its progress is paused at 40.
    let sample = vectors::sample_status();
    let swapped = edited_bytes(&sample, |bytes| {
        let second = find(bytes, b"\x03\x05write");
        bytes[second] = 1;
    });
    assert_eq!(
        decode_status(&mut Reader::new(&swapped)),
        Err(DecodeError::Invalid("status sequence order"))
    );
    let over = edited_bytes(&sample, |bytes| {
        let last = bytes.len() - 1;
        assert_eq!(bytes[last], 40);
        bytes[last] = 101;
    });
    assert_eq!(
        decode_status(&mut Reader::new(&over)),
        Err(DecodeError::Invalid("progress percentage"))
    );
    // A nanosecond count of a whole second or more.
    let mut w = Writer::new();
    w.i64(0);
    w.u32(1_000_000_000);
    assert_eq!(
        read_time(&mut Reader::new(&w.into_bytes())),
        Err(DecodeError::Invalid("time"))
    );
}

#[test]
fn times_before_and_after_the_epoch_round_trip() {
    for time in [
        SystemTime::UNIX_EPOCH,
        SystemTime::UNIX_EPOCH + Duration::new(5, 1),
        SystemTime::UNIX_EPOCH - Duration::new(5, 0),
        SystemTime::UNIX_EPOCH - Duration::new(5, 1),
        // The earliest: `i64::MIN` seconds, which negating a converted
        // `i64` cannot produce.
        SystemTime::UNIX_EPOCH - Duration::from_secs(1 << 63),
        SystemTime::UNIX_EPOCH - Duration::new((1 << 63) - 1, 1),
    ] {
        let mut w = Writer::new();
        write_time(&mut w, time).unwrap();
        assert_eq!(read_time(&mut Reader::new(&w.into_bytes())), Ok(time));
    }
}
