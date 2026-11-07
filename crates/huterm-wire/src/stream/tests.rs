use super::*;
use crate::caps::{MessageCaps, PASTE_PAYLOAD_LIMIT, reassembly_budget};
use crate::codec::Writer;
use crate::frame::{FrameDecoder, MIN_MAX_FRAME};
use crate::message::{Message, Paste, decode_message, encode_message};
use crate::vectors;

const CAPS: MessageCaps = MessageCaps::for_minor(0);
const SECOND: Duration = Duration::from_secs(1);

fn server_side() -> Reassembler {
    Reassembler::new(CAPS, Direction::ClientToServer, reassembly_budget(0))
}

fn frame(stream: u32, end: FrameEnd, payload: &[u8]) -> Frame {
    Frame {
        stream,
        end,
        payload: payload.to_vec(),
    }
}

fn paste(text: &str) -> Vec<u8> {
    encode_message(
        &Message::Paste(Paste {
            viewer: huterm_protocol::ViewerId::in_runtime(vectors::RUNTIME, 30),
            seq: 1,
            text: text.into(),
        }),
        vectors::scope(),
        CAPS,
    )
    .unwrap()
}

#[test]
fn a_fifth_open_stream_is_rejected_and_a_conforming_sender_waits() {
    let message = paste("abcdef");
    let mut receiver = server_side();
    for stream in 0..4 {
        let start = frame(stream, FrameEnd::More, &message[..3]);
        assert_eq!(receiver.accept(start, Duration::ZERO), Ok(None));
    }
    assert_eq!(
        receiver
            .accept(frame(9, FrameEnd::More, &message[..3]), Duration::ZERO),
        Err(StreamError::TooManyStreams)
    );
    let mut sender = OutboundStreams::new(usize::MAX, None);
    let streams: Vec<u32> = (0..4).map(|_| sender.open(10).unwrap()).collect();
    assert_eq!(streams, [0, 1, 2, 3]);
    assert_eq!(sender.open(10), Err(SendBlocked::NoStream));
    sender.release(2);
    assert_eq!(sender.open(10), Ok(2));
    // The server keeps its last stream for host effects.
    let mut server = OutboundStreams::new(usize::MAX, Some(HOST_EFFECT_STREAM));
    for _ in 0..3 {
        assert_ne!(server.open(10), Ok(HOST_EFFECT_STREAM));
    }
    assert_eq!(server.open(10), Err(SendBlocked::NoStream));
    assert_eq!(server.open_reserved(10), Ok(HOST_EFFECT_STREAM));
    assert_eq!(server.open_reserved(10), Err(SendBlocked::NoStream));
}

#[test]
fn messages_past_their_cap_or_of_the_wrong_kind_are_rejected_from_their_header()
{
    let cap = CAPS.cap(MessageKind::Paste).unwrap();
    // A Paste kind whose record declares more than the cap allows: the
    // receiver refuses on the header's few bytes, before buffering.
    let mut header = Writer::new();
    header.u64(MessageKind::Paste.number());
    header.usize(cap);
    let header = header.into_bytes();
    assert_eq!(
        server_side().accept(frame(0, FrameEnd::More, &header), Duration::ZERO),
        Err(StreamError::OverCap {
            kind: MessageKind::Paste
        })
    );
    // A server-to-client kind sent by a client.
    let welcome = encode_message(
        &Message::Welcome(vectors::welcome()),
        vectors::scope(),
        CAPS,
    )
    .unwrap();
    assert_eq!(
        server_side()
            .accept(frame(0, FrameEnd::Final, &welcome), Duration::ZERO),
        Err(StreamError::UnexpectedKind(MessageKind::Welcome.number()))
    );
    // Frames that carry more than the message declared, or end early.
    let message = paste("abc");
    let mut longer = message.clone();
    longer.push(0);
    assert_eq!(
        server_side()
            .accept(frame(0, FrameEnd::Final, &longer), Duration::ZERO),
        Err(StreamError::LengthMismatch(0))
    );
    assert_eq!(
        server_side().accept(
            frame(0, FrameEnd::Final, &message[..message.len() - 1]),
            Duration::ZERO
        ),
        Err(StreamError::LengthMismatch(0))
    );
    assert_eq!(
        server_side()
            .accept(frame(0, FrameEnd::Final, &message[..1]), Duration::ZERO),
        Err(StreamError::Header(DecodeError::Truncated))
    );
}

#[test]
fn unfinished_messages_past_the_reassembly_budget_are_rejected_and_a_sender_waits()
 {
    let message = paste(&"x".repeat(100));
    let mut receiver = Reassembler::new(CAPS, Direction::ClientToServer, 150);
    receiver
        .accept(frame(0, FrameEnd::More, &message[..80]), Duration::ZERO)
        .unwrap();
    assert_eq!(receiver.buffered(), 80);
    assert_eq!(
        receiver
            .accept(frame(1, FrameEnd::More, &message[..80]), Duration::ZERO),
        Err(StreamError::OverBudget)
    );
    let mut sender = OutboundStreams::new(150, None);
    assert_eq!(sender.open(message.len()), Ok(0));
    assert_eq!(sender.open(message.len()), Err(SendBlocked::OverBudget));
    sender.release(0);
    assert_eq!(sender.open(message.len()), Ok(0));
    // The advertised budget holds every stream at the largest client cap.
    assert_eq!(
        reassembly_budget(0),
        MAX_OPEN_STREAMS * CAPS.cap(MessageKind::Paste).unwrap()
    );
}

#[test]
fn an_idle_stream_is_rejected_unless_the_receiver_paused_reading() {
    let message = paste("abc");
    let mut receiver = server_side();
    assert_eq!(
        receiver.idle_deadline(),
        None,
        "nothing in flight arms nothing"
    );
    receiver
        .accept(frame(0, FrameEnd::More, &message[..2]), Duration::ZERO)
        .unwrap();
    assert_eq!(receiver.idle_deadline(), Some(STREAM_IDLE_DEADLINE));
    assert_eq!(receiver.check_idle(29 * SECOND), Ok(()));
    assert_eq!(
        receiver.check_idle(STREAM_IDLE_DEADLINE),
        Err(StreamError::Idle(0))
    );
    // Forty seconds of the receiver's own pause do not count.
    receiver.pause(10 * SECOND);
    assert_eq!(receiver.idle_deadline(), None);
    assert_eq!(receiver.check_idle(100 * SECOND), Ok(()));
    receiver.resume(50 * SECOND);
    assert_eq!(receiver.idle_deadline(), Some(70 * SECOND));
    assert_eq!(receiver.check_idle(69 * SECOND), Ok(()));
    // A frame read while paused counts from the pause, not twice.
    receiver.pause(60 * SECOND);
    receiver
        .accept(frame(0, FrameEnd::More, &message[2..2]), 65 * SECOND)
        .unwrap();
    receiver.resume(80 * SECOND);
    assert_eq!(receiver.idle_deadline(), Some(110 * SECOND));
    // Another frame restarts the stream's clock.
    receiver
        .accept(frame(0, FrameEnd::More, &message[2..3]), 90 * SECOND)
        .unwrap();
    assert_eq!(receiver.idle_deadline(), Some(120 * SECOND));
}

#[test]
fn an_abort_frees_the_receivers_stream_and_aborting_a_sent_message_does_nothing()
 {
    let message = paste(&"y".repeat(64));
    let mut receiver = server_side();
    for stream in 0..4 {
        receiver
            .accept(
                frame(stream, FrameEnd::More, &message[..10]),
                Duration::ZERO,
            )
            .unwrap();
    }
    assert_eq!(
        receiver.accept(frame(1, FrameEnd::Abort, &[]), Duration::ZERO),
        Ok(None)
    );
    assert_eq!(receiver.open_streams(), 3);
    assert_eq!(receiver.buffered(), 30);
    assert_eq!(
        receiver.accept(frame(1, FrameEnd::Abort, &[]), Duration::ZERO),
        Err(StreamError::AbortWithoutMessage(1))
    );
    // The freed stream carries a new message.
    let completed = receiver
        .accept(frame(1, FrameEnd::Final, &message), Duration::ZERO)
        .unwrap();
    assert_eq!(
        completed.map(|message| message.bytes),
        Some(message.clone())
    );

    // A sender aborts only a message it started and has not finished.
    let max_frame = u32::try_from(Frame::body_len(0, 40)).unwrap();
    let mut untouched = OutboundMessage::new(0, message.clone());
    assert_eq!(untouched.abort(), None);
    assert_eq!(untouched.next_frame(max_frame), None);
    let mut started = OutboundMessage::new(0, message.clone());
    assert!(started.next_frame(max_frame).is_some());
    assert_eq!(started.abort(), Some(frame(0, FrameEnd::Abort, &[])));
    let mut sent = OutboundMessage::new(0, message.clone());
    while sent.next_frame(max_frame).is_some() {}
    assert!(sent.is_finished());
    assert_eq!(sent.abort(), None, "a fully built message still arrives");
}

#[test]
fn every_vector_split_at_every_byte_offset_decodes_identically() {
    for vector in vectors::minor_0_messages() {
        let bytes = vector.bytes;
        for offset in 1..bytes.len() {
            let mut receiver = Reassembler::new(CAPS, vector.from, usize::MAX);
            let first = frame(5, FrameEnd::More, &bytes[..offset]);
            assert_eq!(receiver.accept(first, Duration::ZERO), Ok(None));
            let rest = frame(5, FrameEnd::Final, &bytes[offset..]);
            let assembled =
                receiver.accept(rest, Duration::ZERO).unwrap().unwrap();
            assert_eq!(assembled.bytes, bytes, "{} at {offset}", vector.name);
            assert_eq!(
                decode_message(
                    &assembled.bytes,
                    vector.from,
                    vectors::context(0)
                ),
                Ok(vector.message.clone()),
                "{} at {offset}",
                vector.name
            );
        }
        // One byte per frame, interleaved with another stream's message.
        let mut receiver = Reassembler::new(CAPS, vector.from, usize::MAX);
        let mut assembled = Vec::new();
        for (index, byte) in bytes.iter().enumerate() {
            let end = if index + 1 == bytes.len() {
                FrameEnd::Final
            } else {
                FrameEnd::More
            };
            assembled.extend(
                receiver
                    .accept(frame(0, end, &[*byte]), Duration::ZERO)
                    .unwrap(),
            );
            assembled.extend(
                receiver
                    .accept(frame(1, end, &[*byte]), Duration::ZERO)
                    .unwrap(),
            );
        }
        assert_eq!(assembled.len(), 2, "{}", vector.name);
        assert!(assembled.iter().all(|message| message.bytes == bytes));
    }
}

#[test]
fn a_paste_at_its_limit_crosses_the_smallest_frames_intact() {
    let message = paste(&"z".repeat(PASTE_PAYLOAD_LIMIT));
    assert!(message.len() <= CAPS.cap(MessageKind::Paste).unwrap());
    let mut outbound = OutboundMessage::new(2, message.clone());
    let mut wire = Vec::new();
    let mut frames = 0;
    while let Some(frame) = outbound.next_frame(MIN_MAX_FRAME) {
        wire.extend(frame.encode(MIN_MAX_FRAME).unwrap());
        frames += 1;
    }
    assert_eq!(
        frames,
        message.len().div_ceil(Frame::capacity(2, MIN_MAX_FRAME))
    );
    let mut decoder = FrameDecoder::new();
    decoder.set_limit(MIN_MAX_FRAME);
    decoder.push(&wire);
    let mut receiver = server_side();
    let mut assembled = None;
    while let Some(frame) = decoder.next_frame().unwrap() {
        if let Some(message) = receiver.accept(frame, Duration::ZERO).unwrap() {
            assembled = Some(message);
        }
    }
    assert_eq!(assembled.map(|message| message.bytes), Some(message));
    assert_eq!(receiver.buffered(), 0);
}
