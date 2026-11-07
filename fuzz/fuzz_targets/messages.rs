//! Arbitrary message bytes through every decoder. A message that decodes
//! must encode again and decode to the same value, so encoders and
//! decoders agree on every value a peer can produce.

#![no_main]

use huterm_wire::caps::Direction;
use huterm_wire::codec::Reader;
use huterm_wire::message::{decode_message, encode_message};
use huterm_wire::test_minor::{decode_snapshot, decode_test_event};
use huterm_wire::vectors::{context, scope};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let context = context(0);
    for from in [Direction::ClientToServer, Direction::ServerToClient] {
        if let Ok(message) = decode_message(data, from, context) {
            let encoded = encode_message(&message, scope(), context.caps)
                .expect("a decoded message encodes");
            assert_eq!(
                decode_message(&encoded, from, context).as_ref(),
                Ok(&message),
                "a re-encoded message decodes to the same value"
            );
        }
    }
    // The test-only minor version's records share the same decoders.
    let _ = decode_snapshot(&mut Reader::new(data), context.hierarchy);
    let _ = decode_test_event(&mut Reader::new(data), scope());
});
