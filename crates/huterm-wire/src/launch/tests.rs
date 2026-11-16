use super::*;
use crate::vectors::sample_command;

fn round_trip(command: &TerminalCommand) -> TerminalCommand {
    let mut w = Writer::new();
    encode_command(&mut w, command).unwrap();
    let bytes = w.into_bytes();
    let mut r = Reader::new(&bytes);
    let decoded = decode_command(&mut r).unwrap();
    assert!(r.is_empty());
    decoded
}

#[cfg(unix)]
#[test]
fn paths_that_are_not_utf8_round_trip_as_bytes_on_unix() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let command = TerminalCommand {
        program: PathBuf::from(OsStr::from_bytes(b"/bin/\xff\xfe")),
        working_directory: PathBuf::from(OsStr::from_bytes(b"/tmp/\x80")),
        ..sample_command()
    };
    assert_eq!(round_trip(&command), command);
}

/// A launch record whose one argument is `bytes` long, written by hand so
/// it can pass the payload limit.
fn launch_with_argument(bytes: usize) -> Vec<u8> {
    let mut w = Writer::new();
    w.record(|w| {
        w.bytes(b"/bin/sh", MAX_LAUNCH_PATH_BYTES, "program")?;
        w.usize(1);
        w.str(&"a".repeat(bytes), usize::MAX, "argument")
    })
    .unwrap();
    w.into_bytes()
}

#[test]
fn a_launch_past_its_payload_limit_does_not_decode() {
    // The record ends after the argument, so the payload check is what
    // stops the decoder before the next field.
    assert_eq!(
        decode_command(&mut Reader::new(&launch_with_argument(
            LAUNCH_PAYLOAD_LIMIT
        ))),
        Err(DecodeError::Truncated)
    );
    assert_eq!(
        decode_command(&mut Reader::new(&launch_with_argument(
            LAUNCH_PAYLOAD_LIMIT + 1
        ))),
        Err(DecodeError::TooLong { field: "argument" })
    );
    // Arguments and environment share the limit.
    let mut command = sample_command();
    command.arguments = vec!["a".repeat(LAUNCH_PAYLOAD_LIMIT / 2)];
    command.environment =
        vec![(String::new(), "v".repeat(LAUNCH_PAYLOAD_LIMIT / 2))];
    assert_eq!(round_trip(&command), command);
    command.environment[0].0.push('n');
    assert_eq!(
        encode_command(&mut Writer::new(), &command),
        Err(EncodeError::LimitExceeded {
            field: "launch payload"
        })
    );
    let mut w = Writer::new();
    w.record(|w| {
        w.bytes(b"/bin/sh", MAX_LAUNCH_PATH_BYTES, "program")?;
        w.usize(1);
        w.str(
            &"a".repeat(LAUNCH_PAYLOAD_LIMIT / 2),
            usize::MAX,
            "argument",
        )?;
        w.bytes(b"/", MAX_LAUNCH_PATH_BYTES, "working directory")?;
        w.usize(1);
        w.record(|w| {
            w.str("n", usize::MAX, "name")?;
            w.str(&"v".repeat(LAUNCH_PAYLOAD_LIMIT / 2), usize::MAX, "value")
        })
    })
    .unwrap();
    assert_eq!(
        decode_command(&mut Reader::new(&w.into_bytes())),
        Err(DecodeError::TooLong {
            field: "launch payload"
        })
    );
}

#[test]
fn unknown_shell_features_and_history_past_its_maximum_are_rejected() {
    let shell = |bits: u64| {
        let mut w = Writer::new();
        w.record(|w| {
            w.tag(0);
            w.u64(bits);
            Ok(())
        })
        .unwrap();
        w.into_bytes()
    };
    for (bits, features) in [
        (1, &[ShellFeature::Path][..]),
        (2, &[ShellFeature::SshEnv]),
        (4, &[ShellFeature::SshTerminfo]),
        (8, &[ShellFeature::Sudo]),
        (
            15,
            &[
                ShellFeature::Path,
                ShellFeature::SshEnv,
                ShellFeature::SshTerminfo,
                ShellFeature::Sudo,
            ],
        ),
    ] {
        let decoded =
            decode_shell_integration(&mut Reader::new(&shell(bits))).unwrap();
        assert_eq!(decoded.features.iter().collect::<Vec<_>>(), features);
        let mut w = Writer::new();
        encode_shell_integration(&mut w, &decoded).unwrap();
        assert_eq!(w.into_bytes(), shell(bits), "{bits}");
    }
    for bits in [16, 1 << 63] {
        assert_eq!(
            decode_shell_integration(&mut Reader::new(&shell(bits))),
            Err(DecodeError::Invalid("shell features"))
        );
    }
    let history = |rows: u32, bytes: Option<u64>| {
        let mut w = Writer::new();
        w.record(|w| {
            w.u32(rows);
            w.option(bytes, |w, bytes| {
                w.u64(bytes);
                Ok(())
            })
        })
        .unwrap();
        w.into_bytes()
    };
    let max = ScrollbackLimit::MAX;
    assert_eq!(
        decode_scrollback(&mut Reader::new(&history(
            max.rows(),
            Some(ScrollbackLimit::MAX_BYTES)
        ))),
        Ok(max.with_bytes(ScrollbackLimit::MAX_BYTES).unwrap())
    );
    assert_eq!(
        decode_scrollback(&mut Reader::new(&history(max.rows() + 1, None))),
        Err(DecodeError::Invalid("scrollback rows"))
    );
    assert_eq!(
        decode_scrollback(&mut Reader::new(&history(
            0,
            Some(ScrollbackLimit::MAX_BYTES + 1)
        ))),
        Err(DecodeError::Invalid("scrollback bytes"))
    );
}
