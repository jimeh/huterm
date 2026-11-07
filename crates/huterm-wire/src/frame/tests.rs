use super::*;

fn frame(stream: u32, end: FrameEnd, payload: &[u8]) -> Frame {
    Frame {
        stream,
        end,
        payload: payload.to_vec(),
    }
}

#[test]
fn preambles_reject_foreign_magic_and_other_framing_versions() {
    assert_eq!(check_preamble(preamble()), Ok(()));
    assert_eq!(
        check_preamble(*b"SSH-2.0-"),
        Err(FrameError::ForeignPreamble)
    );
    let mut future = preamble();
    future[6] = 2;
    assert_eq!(
        check_preamble(future),
        Err(FrameError::UnsupportedFraming(2))
    );
}

#[test]
fn headers_are_checked_before_their_frame_is_buffered() {
    // A length above the limit fails from its four bytes alone.
    let mut decoder = FrameDecoder::new();
    decoder.push(&(HANDSHAKE_FRAME_LIMIT + 1).to_le_bytes());
    assert_eq!(
        decoder.next_frame(),
        Err(FrameError::OverLimit {
            length: HANDSHAKE_FRAME_LIMIT + 1,
            limit: HANDSHAKE_FRAME_LIMIT,
        })
    );
    // The connection cannot continue after a violation.
    decoder.push(
        &frame(0, FrameEnd::Final, b"x")
            .encode(HANDSHAKE_FRAME_LIMIT)
            .unwrap(),
    );
    assert!(decoder.next_frame().is_err());
    for (bytes, error) in [
        (&[0, 0, 0, 0][..], FrameError::EmptyFrame),
        (&[2, 0, 0, 0, 0b100, 0], FrameError::InvalidFlags(0b100)),
        (&[2, 0, 0, 0, 0b11, 0], FrameError::InvalidFlags(0b11)),
        (&[3, 0, 0, 0, 0b10, 0, 9], FrameError::AbortWithPayload),
        (
            &[3, 0, 0, 0, 0, 0x80, 0x00],
            FrameError::InvalidStream(DecodeError::OverlongVarint),
        ),
    ] {
        let mut decoder = FrameDecoder::new();
        decoder.push(bytes);
        assert_eq!(decoder.next_frame(), Err(error), "{bytes:?}");
    }
}

#[test]
fn the_negotiated_limit_replaces_the_handshake_limit() {
    let large = frame(1, FrameEnd::More, &vec![7; 20_000]);
    assert!(large.encode(HANDSHAKE_FRAME_LIMIT).is_err());
    let bytes = large.encode(MIN_MAX_FRAME).unwrap();
    let mut decoder = FrameDecoder::new();
    decoder.push(&bytes);
    assert!(decoder.next_frame().is_err());
    let mut decoder = FrameDecoder::new();
    decoder.set_limit(MIN_MAX_FRAME);
    decoder.push(&bytes);
    assert_eq!(decoder.next_frame(), Ok(Some(large)));
}

#[test]
fn frames_split_at_any_byte_decode_in_order() {
    let frames = [
        frame(0, FrameEnd::More, b"hello"),
        frame(300, FrameEnd::Final, b""),
        frame(2, FrameEnd::Abort, b""),
    ];
    let bytes: Vec<u8> = frames
        .iter()
        .flat_map(|frame| frame.encode(HANDSHAKE_FRAME_LIMIT).unwrap())
        .collect();
    for chunk in 1..=bytes.len() {
        let mut decoder = FrameDecoder::new();
        let mut received = Vec::new();
        for piece in bytes.chunks(chunk) {
            decoder.push(piece);
            while let Some(frame) = decoder.next_frame().unwrap() {
                received.push(frame);
            }
        }
        assert_eq!(received, frames, "chunks of {chunk}");
        assert_eq!(decoder.buffered(), 0);
    }
}
