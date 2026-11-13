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
            wide: false,
            cells: 0,
        },
    );
    rejected(&empty, "empty run", "run");
    // Run counts whose sum overflows past the row's width.
    let mut overflow = uniform_update(2, "x");
    first_row(&mut overflow).runs = vec![
        Run {
            style: 0,
            wide: false,
            cells: usize::MAX,
        },
        Run {
            style: 0,
            wide: false,
            cells: 3,
        },
    ];
    assert_eq!(
        encode_update(
            &mut Writer::new(),
            vectors::scope(),
            &overflow,
            STYLE_FLAGS
        ),
        Err(EncodeError::Invalid("row width"))
    );
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
    // Swap the kept rows, which travel as one range, for full ones, and the
    // empty table for a full one.
    record -= varint_len(1) + 1 + varint_len(0) + varint_len(rows as u64);
    record += varint_len(rows as u64);
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
        let full = FullRow {
            wrapped: true,
            wrap_continuation: true,
            prompt: huterm_protocol::RowPrompt::Continuation,
            runs: (0..columns)
                .map(|column| Run {
                    style: u32::try_from(row * columns + column).unwrap(),
                    wide: false,
                    cells: 1,
                })
                .collect(),
            texts: vec![CellText::from(text.as_str()); columns],
        };
        write_op(&mut w, &WireOp::Full(&full), columns).unwrap();
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

fn styled_cell(text: &str) -> Cell {
    Cell {
        text: CellText::from(text),
        foreground: CellColor::DefaultForeground,
        background: CellColor::DefaultBackground,
        style: CellStyle::default(),
    }
}

#[test]
fn packed_runs_keep_no_text_apart_from_a_space_and_from_graphemes() {
    // No text, a written space, a 2-byte and a 3-byte scalar pack; a
    // combining sequence and a written U+0000 do not.
    let texts = ["a", "", " ", "é", "界", "e\u{301}", "b", "\0", "c"];
    let mut snapshot = (*screen(&[""], texts.len())).clone();
    snapshot.rows[0] = Arc::new(TerminalRow {
        cells: texts.iter().map(|text| styled_cell(text)).collect(),
        ..TerminalRow::default()
    });
    let snapshot = Arc::new(snapshot);
    let update = encoder().next(&snapshot, false).unwrap();
    let mut update_copy = update.clone();
    let runs: Vec<Vec<&str>> = first_row(&mut update_copy)
        .run_texts()
        .map(|(_, texts)| texts.iter().map(CellText::as_str).collect())
        .collect();
    assert_eq!(
        runs,
        [
            vec!["a", "", " ", "é", "界"],
            vec!["e\u{301}"],
            vec!["b"],
            vec!["\0"],
            vec!["c"],
        ],
        "a run ends where its cells stop packing"
    );
    // Each run's header carries its form: packed runs hold their texts as
    // one string with U+0000 for no text.
    let bytes = update_bytes(&update);
    let packed = [0x01, 0x08, b'a', 0x00, b' ', 0xc3, 0xa9, 0xe7, 0x95, 0x8c];
    let unpacked = [0x00, 0x01, 0x03, b'e', 0xcc, 0x81];
    let row = [
        &packed[..],
        &unpacked,
        &[0x01, 0x01, b'b'],
        &[0x00, 0x01, 0x01, 0x00],
        &[0x01, 0x01, b'c'],
    ]
    .concat();
    assert!(bytes.ends_with(&row), "{bytes:02x?}");
    let mut client: ViewerView = ViewerView::new(0);
    applied(&mut client, &update);
    assert!(shows(&client, &snapshot));
    let shown = client.snapshot(terminal()).unwrap();
    assert!(shown.rows[0].cells[1].text.is_empty());
    assert_eq!(shown.rows[0].cells[2].text, CellText::BLANK);
    assert_eq!(shown.rows[0].cells[7].text, "\0");
}

#[test]
fn malformed_packed_runs_are_rejected() {
    // A run of three ASCII cells ends the message: its header, its byte
    // length, and its text.
    let bytes = update_bytes(&uniform_update(3, "x"));
    assert!(bytes.ends_with(&[0x01, 0x03, b'x', b'x', b'x']));
    let mut invalid = bytes.clone();
    *invalid.last_mut().unwrap() = 0xff;
    assert_eq!(decode(&invalid), Err(DecodeError::InvalidUtf8));
    // More cells than the row has left: the header claims two columns.
    let mut narrow = bytes.clone();
    narrow[2] = 2;
    assert_eq!(decode(&narrow), Err(DecodeError::TooLong { field: "run" }));
    // More bytes than the row's remaining cells could take: two 3-byte
    // scalars where one column is left.
    let mut wide = update_bytes(&uniform_update(2, "界"));
    assert!(wide.ends_with(&[0x01, 0x06, 0xe7, 0x95, 0x8c, 0xe7, 0x95, 0x8c]));
    wide[2] = 1;
    assert_eq!(decode(&wide), Err(DecodeError::TooLong { field: "run" }));
}

#[test]
fn every_single_field_of_damage_changes_the_view_digest() {
    let snapshot = vectors::sample_snapshot();
    let digest = |snapshot: &TerminalSnapshot| {
        view_digest(&ViewMetadata::of(snapshot), &snapshot.rows, 0)
    };
    let cell = |edit: fn(&mut Cell)| {
        publish(&snapshot, |next| {
            edit(&mut Arc::make_mut(&mut next.rows[0]).cells[1]);
        })
    };
    let damaged = [
        publish(&snapshot, |next| next.modes.bracketed_paste ^= true),
        publish(&snapshot, |next| next.generation += 1),
        publish(&snapshot, |next| next.cursor = None),
        publish(&snapshot, |next| next.history_size += 1),
        publish(&snapshot, |next| {
            next.default_background = next.cursor_color;
        }),
        publish(&snapshot, |next| next.rows.swap(0, 1)),
        publish(&snapshot, |next| {
            Arc::make_mut(&mut next.rows[0]).cells.swap(1, 2);
        }),
        publish(&snapshot, |next| {
            Arc::make_mut(&mut next.rows[1]).wrapped = false;
        }),
        publish(&snapshot, |next| {
            Arc::make_mut(&mut next.rows[0]).prompt =
                huterm_protocol::RowPrompt::None;
        }),
        cell(|cell| cell.text = CellText::from("L")),
        cell(|cell| cell.text = CellText::from("ls")),
        // Text past the length word's six bytes, changed in its second word.
        cell(|cell| cell.text = CellText::from("0123456789abcdef!")),
        cell(|cell| cell.text = CellText::from("0123456789abcdef?")),
        cell(|cell| cell.text = CellText::EMPTY),
        cell(|cell| cell.foreground = CellColor::Indexed(0)),
        cell(|cell| cell.background = CellColor::DefaultForeground),
        cell(|cell| cell.style.italic = true),
        publish(&snapshot, |next| {
            let row = Arc::make_mut(&mut next.rows[1]);
            row.cells[2].text = CellText::BLANK;
        }),
    ];
    // `publish` advances the revision, so compare against the original at
    // that revision.
    let original = digest(&publish(&snapshot, |_| {}));
    let mut seen = vec![original];
    for (index, damaged) in damaged.iter().enumerate() {
        let value = digest(damaged);
        assert!(!seen.contains(&value), "damage {index} went unnoticed");
        seen.push(value);
    }
}

fn cell_with(text: &str, foreground: CellColor, style: CellStyle) -> Cell {
    Cell {
        text: CellText::from(text),
        foreground,
        background: CellColor::DefaultBackground,
        style,
    }
}

const WIDE: CellStyle = CellStyle {
    bold: false,
    dim: false,
    italic: false,
    underline: false,
    strikeout: false,
    hidden: false,
    wide: true,
    wide_spacer: false,
};

const SPACER: CellStyle = CellStyle {
    wide: false,
    wide_spacer: true,
    ..WIDE
};

/// A one-row snapshot of `cells`, its first update's runs as each run's
/// form and texts, and the update.
fn runs_of(cells: Vec<Cell>) -> (Arc<TerminalSnapshot>, TerminalUpdate) {
    let mut snapshot = (*screen(&[""], cells.len())).clone();
    snapshot.rows[0] = Arc::new(TerminalRow {
        cells,
        ..TerminalRow::default()
    });
    let snapshot = Arc::new(snapshot);
    let update = encoder().next(&snapshot, false).unwrap();
    (snapshot, update)
}

fn shape(update: &TerminalUpdate) -> Vec<(bool, Vec<String>)> {
    let RowOp::Full(row) = &update.rows[0] else {
        unreachable!("a complete snapshot's rows are full")
    };
    row.run_texts()
        .map(|(run, texts)| {
            (run.wide, texts.iter().map(ToString::to_string).collect())
        })
        .collect()
}

#[test]
fn wide_characters_and_their_spacers_travel_as_one_wide_run() {
    let plain = CellColor::DefaultForeground;
    let red = CellColor::Indexed(1);
    let (snapshot, update) = runs_of(vec![
        cell_with("界", plain, WIDE),
        cell_with("", plain, SPACER),
        cell_with("🙂", plain, WIDE),
        cell_with("", plain, SPACER),
        // A spacer in other colors than its character is not derived from
        // it, so the pair travels as two ordinary cells.
        cell_with("帝", plain, WIDE),
        cell_with("", red, SPACER),
        cell_with("a", plain, CellStyle::default()),
    ]);
    assert_eq!(
        shape(&update),
        [
            (
                true,
                vec!["界".into(), String::new(), "🙂".into(), String::new()]
            ),
            (false, vec!["帝".into()]),
            (false, vec![String::new()]),
            (false, vec!["a".into()]),
        ]
    );
    // The wide style, the red spacer, and the plain cell: a derived spacer
    // takes no entry.
    assert_eq!(update.styles.len(), 3);
    let bytes = update_bytes(&update);
    let wide = [0x03, 0x07, 0xe7, 0x95, 0x8c, 0xf0, 0x9f, 0x99, 0x82];
    assert!(bytes.windows(wide.len()).any(|window| window == wide));
    let mut client: ViewerView = ViewerView::new(0);
    applied(&mut client, &update);
    assert!(shows(&client, &snapshot));
}

#[test]
fn malformed_wide_runs_are_rejected() {
    let plain = CellColor::DefaultForeground;
    let (_, update) = runs_of(vec![
        cell_with("界", plain, WIDE),
        cell_with("", plain, SPACER),
    ]);
    // A wide run whose style lacks the wide flag.
    let mut unflagged = update.clone();
    unflagged.styles[0].flags = 0;
    assert_eq!(
        decode(&update_bytes(&unflagged)),
        Err(DecodeError::Invalid("wide run style"))
    );
    assert_eq!(
        encode_update(
            &mut Writer::new(),
            vectors::scope(),
            &unflagged,
            STYLE_FLAGS
        ),
        Err(EncodeError::Invalid("run"))
    );
    // A spacer that holds text cannot be derived.
    let mut spacer_text = update;
    first_row(&mut spacer_text).texts[1] = CellText::from("x");
    assert_eq!(
        encode_update(
            &mut Writer::new(),
            vectors::scope(),
            &spacer_text,
            STYLE_FLAGS
        ),
        Err(EncodeError::Invalid("run"))
    );
}

#[test]
fn repeats_of_eight_cells_travel_as_fills_and_shorter_ones_stay_packed() {
    let plain = |text: &str| styled_cell(text);
    let mut cells = vec![plain("$"), plain(" ")];
    cells.extend((0..7).map(|_| plain("-")));
    cells.push(plain("x"));
    cells.extend((0..8).map(|_| plain("")));
    let (snapshot, update) = runs_of(cells);
    let texts = |shape: &[(bool, Vec<String>)]| -> Vec<usize> {
        shape.iter().map(|(_, texts)| texts.len()).collect()
    };
    assert_eq!(texts(&shape(&update)), [10, 8], "seven repeats stay packed");
    assert!(update_bytes(&update).ends_with(&[0x02, 0x08, 0x01, 0x00]));
    let mut client: ViewerView = ViewerView::new(0);
    applied(&mut client, &update);
    assert!(shows(&client, &snapshot));
}

#[test]
fn malformed_fills_and_keep_ranges_are_rejected() {
    // Eight cells with no text: the run's header, its count, and its one
    // packed cell end the message.
    let fill = update_bytes(&uniform_update(8, ""));
    assert!(fill.ends_with(&[0x02, 0x08, 0x01, 0x00]));
    let length = fill.len();
    // More cells than the row holds.
    let mut long = fill.clone();
    long[length - 3] = 9;
    assert_eq!(decode(&long), Err(DecodeError::TooLong { field: "run" }));
    // No cells at all.
    let mut empty = fill.clone();
    empty[length - 3] = 0;
    assert_eq!(decode(&empty), Err(DecodeError::Invalid("empty run")));
    // A fill text of no scalars; the byte after it is never read.
    let mut textless = fill.clone();
    textless[length - 2] = 0;
    assert_eq!(decode(&textless), Err(DecodeError::Invalid("fill text")));
    // A fill text of three scalars in the room of one 3-byte scalar.
    let mut three = update_bytes(&uniform_update(8, "界"));
    assert!(three.ends_with(&[0x02, 0x08, 0x03, 0xe7, 0x95, 0x8c]));
    let length = three.len();
    three[length - 3..].copy_from_slice(b"abc");
    assert_eq!(decode(&three), Err(DecodeError::TooLong { field: "run" }));

    // A delta of three rows: two kept from the first, then one in full.
    let snapshot = screen(&["a", "b", "c"], 2);
    let mut server = encoder();
    server.next(&snapshot, false).unwrap();
    let scrolled = publish(&snapshot, |next| {
        next.rows.remove(0);
        next.rows.push(text_row("d", 2));
    });
    let delta = server.next(&scrolled, true).unwrap();
    let bytes = update_bytes(&delta);
    let range = [0x02, 0x00, 0x01, 0x02, 0x01];
    let at = bytes
        .windows(range.len())
        .position(|window| window == range)
        .expect("one range of two kept rows");
    let edited = |edits: &[(usize, u8)]| {
        let mut bytes = bytes.clone();
        for (offset, value) in edits {
            bytes[at + offset] = *value;
        }
        decode(&bytes)
    };
    // The range's start and count follow its tag.
    assert!(edited(&[]).is_ok());
    // No kept rows, rows past the baseline, and more rows than the grid.
    assert_eq!(edited(&[(3, 0)]), Err(DecodeError::Invalid("keep index")));
    assert_eq!(edited(&[(2, 2)]), Err(DecodeError::Invalid("keep index")));
    assert_eq!(
        edited(&[(2, 0), (3, 3)]),
        Err(DecodeError::TooLong { field: "rows" })
    );
    // Two ranges of the whole grid.
    let mut twice = delta.clone();
    twice.rows = [0, 1, 2, 0, 1, 2].map(RowOp::Keep).to_vec();
    assert_eq!(
        decode(&update_bytes(&twice)),
        Err(DecodeError::TooLong { field: "rows" })
    );
}

/// The runs a packed stretch splits into, found by comparing every cell:
/// each maximal repeat of at least [`FILL_CELLS`] cells is a run of its
/// own, and the cells between repeats share one.
fn reference_runs(texts: &[&str]) -> Vec<Vec<String>> {
    let mut runs: Vec<Vec<String>> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    let mut start = 0;
    while start < texts.len() {
        let end = start
            + texts[start..]
                .iter()
                .take_while(|text| **text == texts[start])
                .count();
        let repeat: Vec<String> =
            texts[start..end].iter().map(ToString::to_string).collect();
        if repeat.len() >= FILL_CELLS {
            if !pending.is_empty() {
                runs.push(std::mem::take(&mut pending));
            }
            runs.push(repeat);
        } else {
            pending.extend(repeat);
        }
        start = end;
    }
    if !pending.is_empty() {
        runs.push(pending);
    }
    runs
}

proptest! {
    #[test]
    fn packed_cells_split_into_fills_exactly_where_repeats_reach_eight(
        texts in proptest::collection::vec(
            prop_oneof![3 => Just("a"), 1 => Just("b"), 2 => Just("")],
            0..64,
        ),
    ) {
        let cells: Vec<Cell> = texts.iter().map(|text| styled_cell(text)).collect();
        let mut runs = Vec::new();
        push_runs(&mut runs, 0, Kind::Packed, &cells);
        let mut rest = texts.as_slice();
        let runs: Vec<Vec<String>> = runs
            .iter()
            .map(|run| {
                let (run, after) = rest.split_at(run.cells);
                rest = after;
                run.iter().map(ToString::to_string).collect()
            })
            .collect();
        prop_assert_eq!(runs, reference_runs(&texts));
    }
}

/// One cell of a rich row: its text kind, color, and boldness.
fn rich_cell() -> impl Strategy<Value = (u8, u8, bool)> {
    (0..7_u8, 0..3_u8, any::<bool>())
}

/// A row whose cells cover every run form: no text, spaces, ASCII,
/// multi-byte scalars, graphemes that do not pack, long text, repeats that
/// fill, and wide characters with their spacers, in a few styles.
fn rich_row(spec: &[(u8, u8, bool)], columns: usize) -> Arc<TerminalRow> {
    let mut cells = Vec::with_capacity(columns);
    let mut index = 0;
    while cells.len() < columns {
        let (kind, color, bold) = spec[index % spec.len()];
        index += 1;
        let foreground = match color {
            0 => CellColor::DefaultForeground,
            1 => CellColor::Indexed(3),
            _ => CellColor::Rgb(Rgb {
                red: 1,
                green: 2,
                blue: 3,
            }),
        };
        let style = CellStyle {
            bold,
            ..CellStyle::default()
        };
        let text = match kind {
            0 => "",
            1 => " ",
            2 => "a",
            3 => "é",
            4 => "e\u{301}",
            5 => "x",
            _ => "界",
        };
        if kind == 6 && cells.len() + 1 < columns {
            cells.push(cell_with(text, foreground, CellStyle { bold, ..WIDE }));
            cells.push(cell_with("", foreground, CellStyle { bold, ..SPACER }));
        } else if kind == 5 {
            // A repeat long enough to fill.
            for _ in 0..9 {
                if cells.len() < columns {
                    cells.push(cell_with(text, foreground, style));
                }
            }
        } else if kind != 6 {
            cells.push(cell_with(text, foreground, style));
        }
    }
    Arc::new(TerminalRow {
        cells,
        ..TerminalRow::default()
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// A streamed update, cut into frames of any size, is byte for byte the
    /// message `encode_message` builds from the same update, complete or
    /// delta, and the encoder's numbering advances the same way.
    #[test]
    fn a_streamed_update_matches_the_encoded_message_at_any_frame_size(
        rows in proptest::collection::vec(
            proptest::collection::vec(rich_cell(), 1..12),
            1..6,
        ),
        edits in proptest::collection::vec(
            (any::<prop::sample::Index>(), proptest::collection::vec(rich_cell(), 1..12)),
            0..6,
        ),
        frame in 1..64_usize,
    ) {
        let columns = 24;
        let mut snapshot = (*screen(&vec![""; rows.len()], columns)).clone();
        snapshot.rows = rows.iter().map(|spec| rich_row(spec, columns)).collect();
        let mut snapshot = Arc::new(snapshot);
        let mut values = encoder();
        let mut streams = encoder();
        for step in 0..=edits.len() {
            if step > 0 {
                let (row, spec) = &edits[step - 1];
                snapshot = publish(&snapshot, |next| {
                    next.generation += 1;
                    let row = row.index(next.rows.len());
                    next.rows[row] = rich_row(spec, columns);
                    next.rows.rotate_left(1);
                });
            }
            let credited = step % 2 == 1;
            let expected = encode_message(
                &Message::TerminalUpdate(values.next(&snapshot, credited).unwrap()),
                vectors::scope(),
                CAPS,
            )
            .unwrap();
            let planned = streams.plan(&snapshot, credited).unwrap();
            prop_assert_eq!(planned.seq(), values.seq());
            let mut frames = planned.frames(vectors::scope()).unwrap();
            prop_assert_eq!(frames.len(), expected.len());
            let mut streamed = Vec::new();
            while frames.remaining() > 0 {
                let before = streamed.len();
                let appended = frames.fill(&mut streamed, frame).unwrap();
                prop_assert_eq!(appended, streamed.len() - before);
                prop_assert!(appended > 0 && appended <= frame);
            }
            prop_assert_eq!(&streamed, &expected);
        }
    }
}

#[test]
fn a_streamed_update_keeps_only_the_rest_of_one_row_between_frames() {
    let snapshot = screen(&["abcdefgh"; 64], 400);
    let mut frames = encoder()
        .plan(&snapshot, false)
        .unwrap()
        .frames(vectors::scope())
        .unwrap();
    let mut out = Vec::new();
    let mut held = 0;
    while frames.remaining() > 0 {
        out.clear();
        frames.fill(&mut out, 100).unwrap();
        held = held.max(frames.pending.capacity());
    }
    // A 400-column row of a few letters and a fill encodes in a few dozen
    // bytes; the whole message is far larger.
    assert!(held < 64, "held {held} bytes between frames");
    assert!(frames.len() > 64 * 10);
}
