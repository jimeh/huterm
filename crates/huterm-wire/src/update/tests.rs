use std::sync::Arc;
use std::time::Duration;

use huterm_protocol::{
    Cell, CellColor, CellStyle, CellText, Cursor, CursorShape, GridSize,
    MAX_CELL_TEXT_BYTES, MAX_GRID_CELLS, MouseTracking, Rgb, TerminalId,
    TerminalRow, TerminalSnapshot, Viewport,
};
use proptest::prelude::*;

use super::*;
use crate::caps::{Direction, MessageCaps, MessageKind};
use crate::codec::{DecodeError, Reader, Writer, varint_len};
use crate::frame::{FrameEnd, MIN_MAX_FRAME};
use crate::message::{Message, decode_message, encode_message};
use crate::stream::{OutboundMessage, Reassembler, StreamError};
use crate::vectors;

const CAPS: MessageCaps = MessageCaps::for_minor(0);

fn terminal() -> TerminalId {
    TerminalId::in_runtime(vectors::RUNTIME, 6)
}

fn text_row(text: &str, columns: usize) -> Arc<TerminalRow> {
    let mut cells: Vec<Cell> = text
        .chars()
        .take(columns)
        .map(|character| Cell {
            text: CellText::from(character),
            foreground: CellColor::DefaultForeground,
            background: CellColor::DefaultBackground,
            style: CellStyle::default(),
        })
        .collect();
    cells.resize(
        columns,
        Cell {
            text: CellText::EMPTY,
            foreground: CellColor::DefaultForeground,
            background: CellColor::DefaultBackground,
            style: CellStyle::default(),
        },
    );
    Arc::new(TerminalRow {
        cells,
        ..TerminalRow::default()
    })
}

/// A snapshot of `lines`, one row each, `columns` wide.
fn screen(lines: &[&str], columns: usize) -> Arc<TerminalSnapshot> {
    let mut snapshot = (*vectors::sample_snapshot()).clone();
    snapshot.size = GridSize {
        columns: u16::try_from(columns).unwrap(),
        rows: u16::try_from(lines.len()).unwrap(),
    };
    snapshot.rows = lines.iter().map(|line| text_row(line, columns)).collect();
    snapshot.cursor = None;
    Arc::new(snapshot)
}

/// A copy of `snapshot` with `edit` applied, as the runtime publishes it.
fn publish(
    snapshot: &Arc<TerminalSnapshot>,
    edit: impl FnOnce(&mut TerminalSnapshot),
) -> Arc<TerminalSnapshot> {
    let mut next = (**snapshot).clone();
    next.revision += 1;
    edit(&mut next);
    Arc::new(next)
}

/// The update as a client receives it: encoded and decoded.
fn wire(update: &TerminalUpdate) -> TerminalUpdate {
    let bytes = encode_message(
        &Message::TerminalUpdate(update.clone()),
        vectors::scope(),
        CAPS,
    )
    .unwrap();
    match decode_message(&bytes, Direction::ServerToClient, vectors::context(0))
    {
        Ok(Message::TerminalUpdate(update)) => update,
        other => panic!("decoded {other:?}"),
    }
}

fn encoder() -> UpdateEncoder {
    UpdateEncoder::new(vectors::viewer(), 0)
}

fn shows(view: &ViewerView, snapshot: &TerminalSnapshot) -> bool {
    view.snapshot(terminal()).as_ref() == Some(snapshot)
}

fn applied(view: &mut ViewerView, update: &TerminalUpdate) {
    assert_eq!(
        view.apply(&wire(update)).map(|disposal| disposal.outcome),
        Ok(UpdateOutcome::Applied)
    );
}

#[derive(Clone, Debug)]
enum Publication {
    Write { row: usize, text: String },
    Scroll,
    MoveCursor { row: u16, column: u16 },
    Modes,
    Rebuild,
    Resize { columns: u16, rows: u16 },
}

fn publication() -> impl Strategy<Value = Publication> {
    prop_oneof![
        4 => (0..8_usize, "[a-z ]{0,8}")
            .prop_map(|(row, text)| Publication::Write { row, text }),
        2 => Just(Publication::Scroll),
        2 => (0..8_u16, 0..8_u16)
            .prop_map(|(row, column)| Publication::MoveCursor { row, column }),
        1 => Just(Publication::Modes),
        1 => Just(Publication::Rebuild),
        1 => (1..8_u16, 1..6_u16)
            .prop_map(|(columns, rows)| Publication::Resize { columns, rows }),
    ]
}

fn apply_publication(
    snapshot: &Arc<TerminalSnapshot>,
    publication: &Publication,
) -> Arc<TerminalSnapshot> {
    let columns = usize::from(snapshot.size.columns);
    publish(snapshot, |next| match publication {
        Publication::Write { row, text } => {
            let row = row % next.rows.len();
            next.generation += 1;
            next.rows[row] = text_row(text, columns);
        }
        Publication::Scroll => {
            // Scrolling keeps each row's allocation at a new position.
            next.generation += 1;
            next.rows.rotate_left(1);
            let last = next.rows.len() - 1;
            next.rows[last] = text_row("", columns);
        }
        Publication::MoveCursor { row, column } => {
            next.cursor = Some(Cursor {
                row: *row % next.size.rows,
                column: *column % next.size.columns,
                shape: CursorShape::Block,
            });
        }
        Publication::Modes => {
            next.modes.alternate_screen = !next.modes.alternate_screen;
            next.viewport.bottom_offset += 1;
        }
        Publication::Rebuild => {
            // Equal content in new allocations.
            next.rows = next
                .rows
                .iter()
                .map(|row| Arc::new((**row).clone()))
                .collect();
        }
        Publication::Resize { columns, rows } => {
            next.geometry_revision += 1;
            next.size = GridSize {
                columns: *columns,
                rows: *rows,
            };
            next.rows = (0..*rows)
                .map(|_| text_row("~", usize::from(*columns)))
                .collect();
            next.cursor = None;
        }
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever subset of publications the server encodes, a client that
    /// applies every update it sends, in order, shows exactly the latest
    /// encoded snapshot, and every update survives any split into frames.
    #[test]
    fn a_client_converges_on_the_latest_encoded_snapshot(
        publications in proptest::collection::vec(
            (publication(), any::<bool>(), any::<bool>(), 0.0..1.0_f64),
            1..32,
        ),
    ) {
        let mut snapshot = screen(&["$ ls", "a b", ""], 5);
        let mut server = encoder();
        let mut client: ViewerView = ViewerView::new(0);
        let first = server.next(&snapshot, false).unwrap();
        applied(&mut client, &first);
        for (publication, sent, credited, split) in &publications {
            snapshot = apply_publication(&snapshot, publication);
            if !sent {
                continue;
            }
            let update = server.next(&snapshot, *credited).unwrap();
            let bytes = encode_message(
                &Message::TerminalUpdate(update.clone()),
                vectors::scope(),
                CAPS,
            )
            .unwrap();
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss,
                reason = "a fraction of a short message's length"
            )]
            let offset = 1 + (split * (bytes.len() - 1) as f64) as usize;
            let mut receiver =
                Reassembler::new(CAPS, Direction::ServerToClient, usize::MAX);
            let frame = |end, payload: &[u8]| crate::frame::Frame {
                stream: 0,
                end,
                payload: payload.to_vec(),
            };
            let assembled = match receiver
                .accept(frame(FrameEnd::More, &bytes[..offset]), Duration::ZERO)
                .unwrap()
            {
                Some(assembled) => assembled,
                None => receiver
                    .accept(frame(FrameEnd::Final, &bytes[offset..]), Duration::ZERO)
                    .unwrap()
                    .unwrap(),
            };
            prop_assert_eq!(&assembled.bytes, &bytes);
            let Ok(Message::TerminalUpdate(received)) = decode_message(
                &assembled.bytes,
                Direction::ServerToClient,
                vectors::context(0),
            ) else {
                panic!("an encoded update decodes");
            };
            let disposal = client.apply(&received).unwrap();
            prop_assert_eq!(disposal.outcome, UpdateOutcome::Applied);
            prop_assert_eq!(disposal.return_credit, *credited);
            prop_assert!(shows(&client, &snapshot));
        }
    }
}

#[test]
fn a_dropped_duplicated_or_reordered_delta_is_never_applied_and_a_resync_restores()
 {
    let mut snapshot = screen(&["one", "two"], 4);
    let mut server = encoder();
    let mut updates = vec![server.next(&snapshot, false).unwrap()];
    for text in ["1", "2", "3"] {
        snapshot = publish(&snapshot, |next| {
            next.rows[1] = text_row(text, 4);
        });
        updates.push(server.next(&snapshot, true).unwrap());
    }
    let [complete, second, third, fourth]: [TerminalUpdate; 4] = updates
        .iter()
        .map(wire)
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();

    // Duplicated: the second copy is older than the view.
    let mut client: ViewerView = ViewerView::new(0);
    client.apply(&complete).unwrap();
    client.apply(&second).unwrap();
    let digest = client.digest();
    let duplicate = client.apply(&second).unwrap();
    assert_eq!(duplicate.outcome, UpdateOutcome::Stale);
    assert!(
        duplicate.return_credit,
        "a discarded update returns its credit"
    );
    assert_eq!(client.digest(), digest);
    // A newer delta that names an older baseline than the view's.
    let mut behind = fourth.clone();
    behind.baseline = Some(1);
    assert_eq!(
        client.apply(&behind).unwrap().outcome,
        UpdateOutcome::Resync
    );
    assert_eq!(client.digest(), digest);

    // Dropped: the third builds on an update the view never applied.
    let mut client: ViewerView = ViewerView::new(0);
    client.apply(&complete).unwrap();
    assert_eq!(client.apply(&third).unwrap().outcome, UpdateOutcome::Resync);
    // Waiting for the snapshot, it asks only once.
    assert_eq!(client.apply(&fourth).unwrap().outcome, UpdateOutcome::Stale);
    // Reordered: the second arrives after the third.
    assert_eq!(client.apply(&second).unwrap().outcome, UpdateOutcome::Stale);
    assert_eq!(client.applied(), 1);
    server.resync();
    let answer = server.next(&snapshot, false).unwrap();
    assert_eq!(answer.baseline, None);
    applied(&mut client, &answer);
    assert!(shows(&client, &snapshot));
    // Deltas apply again after the snapshot.
    snapshot = publish(&snapshot, |next| next.rows[0] = text_row("x", 4));
    applied(&mut client, &server.next(&snapshot, true).unwrap());
    assert!(shows(&client, &snapshot));
}

#[test]
fn a_geometry_change_yields_a_complete_snapshot() {
    let snapshot = screen(&["a", "b"], 3);
    let mut server = encoder();
    let mut client: ViewerView = ViewerView::new(0);
    applied(&mut client, &server.next(&snapshot, false).unwrap());
    let resized = publish(&snapshot, |next| {
        next.geometry_revision += 1;
        next.size.columns = 4;
        next.rows = vec![text_row("a", 4), text_row("b", 4)];
    });
    let update = server.next(&resized, true).unwrap();
    assert_eq!(update.baseline, None);
    assert!(
        update.digest.is_some(),
        "every complete snapshot has a digest"
    );
    assert!(update.rows.iter().all(|op| matches!(op, RowOp::Full(_))));
    applied(&mut client, &update);
    assert!(shows(&client, &resized));
}

#[test]
fn a_metadata_only_change_keeps_every_row_and_changes_cursor_modes_and_viewport()
 {
    let snapshot = screen(&["a", "b", "c"], 2);
    let mut server = encoder();
    let mut client: ViewerView = ViewerView::new(0);
    applied(&mut client, &server.next(&snapshot, false).unwrap());
    let moved = publish(&snapshot, |next| {
        next.cursor = Some(Cursor {
            row: 2,
            column: 1,
            shape: CursorShape::Underline,
        });
        next.modes.mouse_tracking = MouseTracking::AllMotion;
        next.viewport = Viewport { bottom_offset: 7 };
        next.history_size = 9;
    });
    let update = server.next(&moved, true).unwrap();
    assert_eq!(update.baseline, Some(1));
    assert_eq!(update.styles, []);
    assert_eq!(
        update.rows,
        [RowOp::Keep(0), RowOp::Keep(1), RowOp::Keep(2)]
    );
    applied(&mut client, &update);
    let (metadata, _) = client.view().unwrap();
    assert_eq!(metadata.cursor, moved.cursor);
    assert_eq!(metadata.modes.mouse_tracking, MouseTracking::AllMotion);
    assert_eq!(metadata.viewport.bottom_offset, 7);
    assert!(shows(&client, &moved));
}

#[test]
fn scrolling_names_the_kept_rows_new_positions() {
    let snapshot = screen(&["a", "b", "c"], 2);
    let mut server = encoder();
    let mut client: ViewerView = ViewerView::new(0);
    applied(&mut client, &server.next(&snapshot, false).unwrap());
    let scrolled = apply_publication(&snapshot, &Publication::Scroll);
    let update = server.next(&scrolled, true).unwrap();
    assert!(matches!(
        update.rows.as_slice(),
        [RowOp::Keep(1), RowOp::Keep(2), RowOp::Full(_)]
    ));
    applied(&mut client, &update);
    assert!(shows(&client, &scrolled));
}

#[test]
fn equal_content_in_distinct_allocations_gives_the_same_client_state() {
    let first = screen(&["same", "rows"], 4);
    let second = screen(&["same", "rows"], 4);
    assert!(!Arc::ptr_eq(&first.rows[0], &second.rows[0]));
    // One client gets the second snapshot as a delta on the first, the
    // other gets it complete.
    let mut server = encoder();
    let mut by_delta: ViewerView = ViewerView::new(0);
    applied(&mut by_delta, &server.next(&first, false).unwrap());
    applied(&mut by_delta, &server.next(&second, true).unwrap());
    let mut complete: ViewerView = ViewerView::new(0);
    applied(&mut complete, &encoder().next(&second, false).unwrap());
    assert_eq!(by_delta.snapshot(terminal()), complete.snapshot(terminal()));
    assert_eq!(by_delta.digest(), complete.digest());
    assert!(shows(&by_delta, &first));
}

/// A complete one-row update of `columns` cells, every cell `text`.
fn uniform_update(columns: u16, text: &str) -> TerminalUpdate {
    let mut snapshot = (*screen(&[""], usize::from(columns))).clone();
    let row = Arc::make_mut(&mut snapshot.rows[0]);
    for cell in &mut row.cells {
        cell.text = CellText::from(text);
    }
    encoder().next(&Arc::new(snapshot), false).unwrap()
}

/// An update's message bytes, written without the encoder's checks.
fn update_bytes(update: &TerminalUpdate) -> Vec<u8> {
    let mut w = Writer::new();
    w.u64(MessageKind::TerminalUpdate.number());
    write_update(&mut w, vectors::scope(), update).unwrap();
    w.into_bytes()
}

fn decode(bytes: &[u8]) -> Result<Message, DecodeError> {
    decode_message(bytes, Direction::ServerToClient, vectors::context(0))
}

/// The decoder rejects `update` as `decoded`, and the encoder refuses it
/// locally as `encoded`, so a sender never sends what its peer rejects.
fn rejected(
    update: &TerminalUpdate,
    decoded: &'static str,
    encoded: &'static str,
) {
    assert_eq!(
        decode(&update_bytes(update)),
        Err(DecodeError::Invalid(decoded))
    );
    assert_eq!(
        encode_update(
            &mut Writer::new(),
            vectors::scope(),
            update,
            STYLE_FLAGS
        ),
        Err(EncodeError::Invalid(encoded))
    );
}

fn first_row(update: &mut TerminalUpdate) -> &mut FullRow {
    match &mut update.rows[0] {
        RowOp::Full(row) => row,
        RowOp::Keep(_) => unreachable!("a complete snapshot's rows are full"),
    }
}

#[test]
fn malformed_rows_and_references_are_rejected() {
    let complete = uniform_update(3, "x");
    // A kept row in a complete snapshot, or past the grid in a delta.
    let mut keep = complete.clone();
    keep.rows[0] = RowOp::Keep(0);
    rejected(&keep, "keep index", "keep index");
    keep.seq = 2;
    keep.baseline = Some(1);
    keep.rows[0] = RowOp::Keep(1);
    rejected(&keep, "keep index", "keep index");
    // A style index past the table.
    let mut style = complete.clone();
    first_row(&mut style).runs[0].style = 1;
    rejected(&style, "style index", "run");
    // Runs that do not fill the row: the header claims four columns.
    let mut short = update_bytes(&complete);
    assert_eq!(short[2], 3, "the record opens with the column count");
    short[2] = 4;
    assert_eq!(decode(&short), Err(DecodeError::Invalid("row width")));
    // An empty run, which would let a peer send endless runs.
    let mut empty = complete.clone();
    first_row(&mut empty).runs.insert(
        0,
        Run {
            style: 0,
            texts: Vec::new(),
        },
    );
    rejected(&empty, "empty run", "run");
    // A complete snapshot always carries its digest, so a client can always
    // tell a contract fault.
    let mut digestless = complete;
    digestless.digest = None;
    rejected(
        &digestless,
        "complete snapshot digest",
        "complete snapshot digest",
    );
    assert_eq!(
        ViewerView::<Arc<TerminalRow>>::new(0).apply(&digestless),
        Err(ViewError::Invalid("complete snapshot digest"))
    );
}

#[test]
fn a_delta_that_changes_the_grid_is_rejected() {
    let snapshot = screen(&["a", "b"], 2);
    let mut server = encoder();
    let mut client: ViewerView = ViewerView::new(0);
    applied(&mut client, &server.next(&snapshot, false).unwrap());
    let mut delta = wire(&server.next(&snapshot, true).unwrap());
    // A wider grid with the same row count, which only a complete snapshot
    // may introduce.
    delta.metadata.size.columns = 3;
    delta.rows = vec![RowOp::Keep(0), RowOp::Keep(1)];
    assert_eq!(
        client.apply(&delta),
        Err(ViewError::Invalid("delta grid size"))
    );
}

/// Counts the cells digests read, to show when a client computes one.
#[derive(Clone)]
struct Counted(Arc<TerminalRow>);

thread_local! {
    static READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl UpdateRow for Counted {
    fn row(&self) -> &TerminalRow {
        &self.0
    }

    fn flags(&self, column: usize, minor: u16) -> u64 {
        READS.set(READS.get() + 1);
        self.0.flags(column, minor)
    }
}

impl ViewRow for Counted {
    fn build(row: &FullRow, styles: &[StyleEntry]) -> Self {
        Self(Arc::new(build_row(row, styles)))
    }
}

#[test]
fn a_client_computes_a_digest_only_for_an_update_that_carries_one() {
    let snapshot = screen(&["ab", "cd"], 2);
    let mut server = encoder();
    let mut client: ViewerView<Counted> = ViewerView::new(0);
    client
        .apply(&wire(&server.next(&snapshot, false).unwrap()))
        .unwrap();
    assert_eq!(READS.get(), 4, "the complete snapshot's digest");
    let moved = publish(&snapshot, |next| next.history_size += 1);
    let delta = wire(&server.next(&moved, true).unwrap());
    assert_eq!(delta.digest, None);
    client.apply(&delta).unwrap();
    assert_eq!(READS.get(), 4, "a delta without a digest reads no cell");
}

/// An update message whose header declares `columns` by `rows` and whose
/// record holds `padding` more bytes, which a decoder would skip as
/// appended fields if the cap allowed them.
fn padded_update(update: &TerminalUpdate, padding: usize) -> Vec<u8> {
    let mut record = Writer::new();
    encode_update(&mut record, vectors::scope(), update, STYLE_FLAGS).unwrap();
    let record = record.into_bytes();
    let mut r = Reader::new(&record);
    let mut fields = r.record(|r| Ok(r.peek().to_vec())).unwrap();
    fields.resize(fields.len() + padding, 0);
    let mut w = Writer::new();
    w.u64(MessageKind::TerminalUpdate.number());
    w.usize(fields.len());
    w.fixed(&fields);
    w.into_bytes()
}

#[test]
fn a_grid_past_the_cell_cap_or_bytes_past_the_grids_cap_are_rejected() {
    // 2048 by 1024 cells is twice the cap.
    let mut w = Writer::new();
    w.u64(MessageKind::TerminalUpdate.number());
    w.usize(6);
    w.u16(2048);
    w.u16(1024);
    w.fixed(&[0, 0]);
    let oversized = w.into_bytes();
    assert_eq!(decode(&oversized), Err(DecodeError::Invalid("grid size")));
    let mut receiver =
        Reassembler::new(CAPS, Direction::ServerToClient, usize::MAX);
    let frame = |payload: &[u8]| crate::frame::Frame {
        stream: 0,
        end: FrameEnd::Final,
        payload: payload.to_vec(),
    };
    assert_eq!(
        receiver.accept(frame(&oversized), Duration::ZERO),
        Err(StreamError::Header(DecodeError::Invalid("grid size")))
    );

    let update = uniform_update(2, "x");
    let size = update.metadata.size;
    let cap = CAPS.update_cap(size).unwrap();
    let fits = padded_update(&update, 0).len();
    // The smallest padding past the cap, and the largest within it.
    let padding = (cap.saturating_sub(fits + 4)..=cap)
        .find(|padding| padded_update(&update, *padding).len() > cap)
        .unwrap();
    let over = padded_update(&update, padding);
    let at_cap = padded_update(&update, padding - 1);
    assert!(at_cap.len() <= cap);
    assert_eq!(decode(&over), Err(DecodeError::TooLong { field: "update" }));
    // The reassembler refuses the whole first frame before buffering any of
    // it, and likewise when the header arrives split across frames.
    for split in [over.len(), 3] {
        let mut receiver =
            Reassembler::new(CAPS, Direction::ServerToClient, usize::MAX);
        let mut frames = [&over[..split], &over[split..]].into_iter();
        let first = frames.next().unwrap();
        let mut result = receiver.accept(
            crate::frame::Frame {
                stream: 0,
                end: FrameEnd::More,
                payload: first.to_vec(),
            },
            Duration::ZERO,
        );
        if split < over.len() {
            assert_eq!(result, Ok(None));
            assert_eq!(receiver.buffered(), split);
            result = receiver.accept(
                crate::frame::Frame {
                    stream: 0,
                    end: FrameEnd::More,
                    payload: frames.next().unwrap().to_vec(),
                },
                Duration::ZERO,
            );
        }
        assert_eq!(
            result,
            Err(StreamError::OverCap {
                kind: MessageKind::TerminalUpdate
            })
        );
        // Only the split header's first bytes were ever buffered.
        let header = if split < over.len() { split } else { 0 };
        assert_eq!(receiver.buffered(), header, "{split}");
    }
    // One byte less fits, and the decoder skips the padding.
    assert!(decode(&at_cap).is_ok());
}

/// A cell of `bytes` bytes of text in a color of its own.
fn worst_cell(index: usize, bytes: usize) -> Cell {
    let [_, red, green, blue] = u32::try_from(index).unwrap().to_be_bytes();
    Cell {
        text: CellText::from("x".repeat(bytes)),
        foreground: CellColor::Rgb(Rgb { red, green, blue }),
        background: CellColor::Rgb(Rgb { red, green, blue }),
        style: CellStyle {
            bold: true,
            wide_spacer: true,
            ..CellStyle::default()
        },
    }
}

/// Every non-row field at its widest encoding.
fn widest_metadata(size: GridSize) -> ViewMetadata {
    ViewMetadata {
        generation: u64::MAX,
        revision: u64::MAX,
        geometry_revision: u64::MAX,
        size,
        cursor: Some(Cursor {
            row: u16::MAX,
            column: u16::MAX,
            shape: CursorShape::Hidden,
        }),
        modes: huterm_protocol::TerminalModes {
            kitty_keyboard: huterm_protocol::KittyKeyboardFlags::from_bits(5),
            application_cursor: true,
            alternate_screen: true,
            bracketed_paste: true,
            focus_reporting: true,
            mouse_tracking: MouseTracking::AllMotion,
            mouse_encoding: huterm_protocol::MouseEncoding::Sgr,
        },
        viewport: Viewport {
            bottom_offset: usize::MAX,
        },
        history_size: usize::MAX,
        cursor_color: Some(Rgb {
            red: 1,
            green: 2,
            blue: 3,
        }),
        default_background: Some(Rgb {
            red: 1,
            green: 2,
            blue: 3,
        }),
    }
}

#[test]
fn a_worst_case_update_at_a_mid_size_grid_fits_its_cap() {
    let size = GridSize {
        columns: 80,
        rows: 24,
    };
    let rows = (0..24)
        .map(|row| {
            Arc::new(TerminalRow {
                cells: (0..80)
                    .map(|column| {
                        worst_cell(row * 80 + column, MAX_CELL_TEXT_BYTES)
                    })
                    .collect(),
                wrapped: true,
                wrap_continuation: true,
                prompt: huterm_protocol::RowPrompt::Continuation,
            })
        })
        .collect();
    let snapshot = Arc::new(widest_metadata(size).snapshot(terminal(), rows));
    let mut update = encoder().next(&snapshot, true).unwrap();
    assert_eq!(update.styles.len(), size.cells(), "a style per cell");
    update.seq = u64::MAX;
    update.baseline = Some(u64::MAX - 1);
    let message = Message::TerminalUpdate(update);
    let bytes = encode_message(&message, vectors::scope(), CAPS).unwrap();
    assert!(bytes.len() <= CAPS.update_cap(size).unwrap());
    assert!(bytes.len() > size.cells() * MAX_CELL_TEXT_BYTES);
    assert_eq!(decode(&bytes), Ok(message));
}

/// The exact message length of a worst-case update over `size`, every cell
/// at [`MAX_CELL_TEXT_BYTES`] in a style of its own, measured one row at a
/// time so the whole message never exists.
fn worst_case_length(size: GridSize) -> usize {
    let columns = usize::from(size.columns);
    let rows = usize::from(size.rows);
    // Every row kept, with no styles: the header and the row sequence.
    let kept = TerminalUpdate {
        viewer: ViewerId::in_runtime(vectors::RUNTIME, u64::MAX),
        seq: u64::MAX,
        baseline: Some(u64::MAX - 1),
        credited: true,
        digest: Some(u64::MAX),
        metadata: widest_metadata(size),
        styles: Vec::new(),
        rows: (0..size.rows).map(RowOp::Keep).collect(),
    };
    let mut w = Writer::new();
    encode_update(&mut w, vectors::scope(), &kept, STYLE_FLAGS).unwrap();
    let encoded = w.into_bytes();
    let mut record = Reader::new(&encoded).usize().unwrap();
    // Swap the kept rows for full ones and the empty table for a full one.
    record -= (0..size.rows)
        .map(|index| 1 + varint_len(index.into()))
        .sum::<usize>();
    record -= varint_len(0);
    record += varint_len(size.cells() as u64);
    let text = "x".repeat(MAX_CELL_TEXT_BYTES);
    for row in 0..rows {
        let mut w = Writer::new();
        for column in 0..columns {
            let index = row * columns + column;
            let cell = worst_cell(index, 0);
            write_style(
                &mut w,
                &StyleEntry {
                    foreground: cell.foreground,
                    background: cell.background,
                    flags: STYLE_FLAGS,
                },
            )
            .unwrap();
        }
        let full = RowOp::Full(FullRow {
            wrapped: true,
            wrap_continuation: true,
            prompt: huterm_protocol::RowPrompt::Continuation,
            runs: (0..columns)
                .map(|column| Run {
                    style: u32::try_from(row * columns + column).unwrap(),
                    texts: vec![CellText::from(text.as_str())],
                })
                .collect(),
        });
        write_op(&mut w, &full, columns).unwrap();
        record += w.len();
    }
    1 + varint_len(record as u64) + record
}

#[test]
fn a_worst_case_update_at_the_cell_cap_fits_its_cap() {
    // The widest grid at the cap, and the tallest, whose per-row overhead
    // is largest.
    for size in [
        GridSize {
            columns: 1024,
            rows: 1024,
        },
        GridSize {
            columns: 16,
            rows: u16::MAX,
        },
    ] {
        assert!(size.cells() <= MAX_GRID_CELLS);
        let length = worst_case_length(size);
        let cap = CAPS.update_cap(size).unwrap();
        assert!(length <= cap, "{size:?}: {length} > {cap}");
        assert!(cap <= CAPS.cap(MessageKind::TerminalUpdate).unwrap());
    }
}

#[test]
fn a_row_larger_than_the_smallest_frame_crosses_frames_intact() {
    // 128 cells of near-limit graphemes: a base and combining marks.
    let grapheme =
        format!("e{}", "\u{301}".repeat((MAX_CELL_TEXT_BYTES - 1) / 2));
    let update = uniform_update(128, &grapheme);
    let bytes = encode_message(
        &Message::TerminalUpdate(update.clone()),
        vectors::scope(),
        CAPS,
    )
    .unwrap();
    assert!(bytes.len() > MIN_MAX_FRAME as usize);
    let mut outbound = OutboundMessage::new(1, bytes.clone());
    let mut receiver =
        Reassembler::new(CAPS, Direction::ServerToClient, usize::MAX);
    let mut frames = 0;
    let mut assembled = None;
    while let Some(frame) = outbound.next_frame(MIN_MAX_FRAME) {
        frames += 1;
        assembled = receiver.accept(frame, Duration::ZERO).unwrap();
    }
    assert!(frames > 1);
    let assembled = assembled.expect("the last frame completes the message");
    assert_eq!(assembled.bytes, bytes);
    let mut client: ViewerView = ViewerView::new(0);
    let Ok(Message::TerminalUpdate(update)) = decode(&assembled.bytes) else {
        panic!("the update decodes");
    };
    client.apply(&update).unwrap();
    let (_, rows) = client.view().unwrap();
    assert!(
        rows[0]
            .cells
            .iter()
            .all(|cell| cell.text == grapheme.as_str())
    );
}

#[test]
fn a_digest_mismatch_faults_a_complete_snapshot_and_resyncs_a_delta_once() {
    let snapshot = screen(&["a", "b"], 2);
    let mut server = encoder();
    let mut complete = wire(&server.next(&snapshot, false).unwrap());
    complete.digest = complete.digest.map(|digest| digest ^ 1);
    let mut client: ViewerView = ViewerView::new(0);
    assert_eq!(client.apply(&complete), Err(ViewError::ContractFault));

    // A delta's mismatch asks for one resync and waits for the snapshot.
    let mut server = encoder();
    let mut client: ViewerView = ViewerView::new(0);
    applied(&mut client, &server.next(&snapshot, false).unwrap());
    let mut current = snapshot;
    let mut deltas = Vec::new();
    for _ in 0..=UPDATE_DIGEST_INTERVAL {
        current = publish(&current, |next| next.history_size += 1);
        deltas.push(wire(&server.next(&current, true).unwrap()));
    }
    let digested = usize::try_from(UPDATE_DIGEST_INTERVAL - 1).unwrap();
    assert!(
        deltas[..digested]
            .iter()
            .all(|delta| delta.digest.is_none())
    );
    assert!(deltas[digested].digest.is_some(), "every 64th delta");
    deltas[digested].digest = deltas[digested].digest.map(|digest| !digest);
    for delta in &deltas[..digested] {
        assert_eq!(
            client.apply(delta).unwrap().outcome,
            UpdateOutcome::Applied
        );
    }
    let mismatch = client.apply(&deltas[digested]).unwrap();
    assert_eq!(mismatch.outcome, UpdateOutcome::Resync);
    assert!(mismatch.return_credit);
    assert_eq!(
        client.apply(&deltas[digested + 1]).unwrap().outcome,
        UpdateOutcome::Stale,
        "no second resync"
    );
    server.resync();
    applied(&mut client, &server.next(&current, false).unwrap());
    assert!(shows(&client, &current));
}

#[test]
fn malformed_snapshots_refuse_to_encode_and_nothing_advances() {
    let snapshot = screen(&["a", "b"], 2);
    let mut server = encoder();
    let narrow = publish(&snapshot, |next| next.rows[1] = text_row("", 1));
    assert_eq!(
        server.next(&narrow, false),
        Err(EncodeError::Invalid("row width"))
    );
    let short = publish(&snapshot, |next| {
        next.rows.pop();
    });
    assert_eq!(
        server.next(&short, false),
        Err(EncodeError::Invalid("row count"))
    );
    assert_eq!(server.seq(), 0);
    assert_eq!(server.next(&snapshot, false).unwrap().seq, 1);
}
