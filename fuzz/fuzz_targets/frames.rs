//! Arbitrary bytes through the frame decoder and both directions' stream
//! reassembly, then the message decoder: every path must refuse bad input
//! with a typed error and never panic or allocate on a peer's word.

#![no_main]

use std::time::Duration;

use huterm_wire::caps::{Direction, MessageCaps, reassembly_budget};
use huterm_wire::frame::{DEFAULT_MAX_FRAME, FrameDecoder, MIN_MAX_FRAME};
use huterm_wire::message::decode_message;
use huterm_wire::stream::Reassembler;
use huterm_wire::vectors::context;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&control, bytes)) = data.split_first() else {
        return;
    };
    let caps = MessageCaps::for_minor(0);
    let from = if control & 1 == 0 {
        Direction::ClientToServer
    } else {
        Direction::ServerToClient
    };
    let mut decoder = FrameDecoder::new();
    if control & 2 != 0 {
        decoder.set_limit(if control & 4 == 0 {
            MIN_MAX_FRAME
        } else {
            DEFAULT_MAX_FRAME
        });
    }
    let mut receiver = Reassembler::new(caps, from, reassembly_budget(0));
    // Feed the bytes in chunks the control byte sizes, as reads would.
    let chunk = usize::from(control >> 3) + 1;
    let mut now = Duration::ZERO;
    for piece in bytes.chunks(chunk) {
        decoder.push(piece);
        now += Duration::from_millis(u64::from(piece[0]));
        if receiver.check_idle(now).is_err() {
            return;
        }
        loop {
            match decoder.next_frame() {
                Ok(Some(frame)) => match receiver.accept(frame, now) {
                    Ok(Some(message)) => {
                        let _ =
                            decode_message(&message.bytes, from, context(0));
                    }
                    Ok(None) => {}
                    Err(_) => return,
                },
                Ok(None) => break,
                Err(_) => return,
            }
        }
    }
});
