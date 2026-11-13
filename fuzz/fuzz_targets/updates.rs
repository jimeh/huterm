//! Arbitrary sequences of terminal updates applied to one client view.
//! The input is a series of messages, each behind a two-byte little-endian
//! length. Every update that decodes is applied in order: applying must
//! never panic, and the view must always hold exactly its grid. The same
//! records also go through the test-only minor version's decoder and view,
//! which must not panic either.

#![no_main]

use std::sync::Arc;

use huterm_protocol::TerminalRow;
use huterm_wire::caps::Direction;
use huterm_wire::codec::Reader;
use huterm_wire::message::{Message, decode_message};
use huterm_wire::test_minor::{TEST_MINOR, TEST_STYLE_FLAGS, TestRow};
use huterm_wire::update::{ViewerView, decode_update};
use huterm_wire::vectors::{context, scope};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut view: ViewerView<Arc<TerminalRow>> = ViewerView::new(0);
    let mut test_view: ViewerView<Arc<TestRow>> = ViewerView::new(TEST_MINOR);
    let mut rest = data;
    while let [low, high, tail @ ..] = rest {
        let len =
            usize::from(u16::from_le_bytes([*low, *high])).min(tail.len());
        let (message, tail) = tail.split_at(len);
        rest = tail;
        // A fuzzed complete snapshot rarely matches its digest, but a
        // contract fault still installs the view, so check its shape
        // whatever the outcome.
        if let Ok(Message::TerminalUpdate(update)) =
            decode_message(message, Direction::ServerToClient, context(0))
        {
            let _ = view.apply(&update);
        }
        if let Some((metadata, rows)) = view.view() {
            assert_eq!(rows.len(), usize::from(metadata.size.rows));
            assert!(rows.iter().all(|row| {
                row.cells.len() == usize::from(metadata.size.columns)
            }));
        }
        // The same record at the test-only minor version, past the kind.
        let mut r = Reader::new(message);
        if r.u64().is_ok()
            && let Ok(update) = decode_update(&mut r, scope(), TEST_STYLE_FLAGS)
        {
            let _ = test_view.apply(&update);
        }
    }
});
