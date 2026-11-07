use huterm_protocol::{CommandError, ids};

use super::*;

#[test]
fn codes_keep_distinct_stable_numbers_and_unknown_codes_fail() {
    let mut numbers: Vec<u64> =
        ErrorCode::ALL.iter().map(|code| code.number()).collect();
    numbers.sort_unstable();
    numbers.dedup();
    assert_eq!(numbers.len(), ErrorCode::ALL.len());
    for code in ErrorCode::ALL {
        assert_eq!(ErrorCode::from_number(code.number()), Some(code));
    }
    let mut w = Writer::new();
    w.record(|w| {
        w.tag(999);
        w.tag(1);
        w.bool(false);
        w.str("", 1, "message")
    })
    .unwrap();
    let bytes = w.into_bytes();
    assert_eq!(
        decode_error(&mut Reader::new(&bytes)),
        Err(DecodeError::UnknownTag {
            field: "error code",
            tag: 999
        })
    );
}

#[test]
fn only_busy_is_safe_to_retry_and_long_messages_are_cut() {
    assert_eq!(WireError::from(&CommandError::Busy).retry, Retry::Safe);
    for (error, code) in [
        (
            CommandError::UnknownCommand(ids::QUIT),
            ErrorCode::UnknownCommand,
        ),
        (CommandError::StaleTarget, ErrorCode::StaleTarget),
        (CommandError::ClientRequired, ErrorCode::ClientRequired),
        (
            CommandError::Unavailable("no".into()),
            ErrorCode::Unavailable,
        ),
        (CommandError::Runtime("io".into()), ErrorCode::Internal),
        (
            CommandError::ArgumentRange {
                command: ids::QUIT,
                name: "count",
                value: 9,
                min: 0,
                max: 1,
            },
            ErrorCode::InvalidArgument,
        ),
    ] {
        let wire = WireError::from(&error);
        assert_eq!((wire.code, wire.retry), (code, Retry::Never), "{error:?}");
        assert_eq!(wire.message, error.to_string());
    }
    let long = WireError::new(
        ErrorCode::Internal,
        "é".repeat(MAX_ERROR_MESSAGE_BYTES),
    );
    let mut w = Writer::new();
    encode_error(&mut w, &long).unwrap();
    let bytes = w.into_bytes();
    let decoded = decode_error(&mut Reader::new(&bytes)).unwrap();
    assert_eq!(decoded.message, "é".repeat(MAX_ERROR_MESSAGE_BYTES / 2));
}
