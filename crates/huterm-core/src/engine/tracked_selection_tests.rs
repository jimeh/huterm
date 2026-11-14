//! Tracked selections follow their text through resizes and output, and
//! read as gone once Ghostty discards it. Each check reads the placement's
//! range back at the current generation and the text by id, so both must
//! still cover the expected text.

use std::fmt::Write as _;

use super::*;
use huterm_protocol::{SelectionPlacement, TrackedSelection, ViewerId};

const VIEWER: ViewerId = ViewerId::new(1);

fn cell() -> CellSize {
    CellSize {
        width: 8,
        height: 16,
    }
}

fn started(columns: u16, rows: u16, output: &[u8]) -> TerminalEngine {
    let mut engine = TerminalEngine::new(
        TerminalId::new(1),
        GridSize::clamped(columns, rows),
        cell(),
        TerminalPresentation::default(),
    )
    .unwrap();
    engine.process(output).unwrap();
    engine
}

fn resize(engine: &mut TerminalEngine, columns: u16, rows: u16) {
    engine
        .resize(GridSize::clamped(columns, rows), cell())
        .unwrap();
}

/// Screen rows of every engine these tests start.
const ROWS: u16 = 4;

/// The point at `column` of screen `row`, counted from the live bottom of
/// a [`ROWS`]-row screen.
fn at(row: u16, column: u16) -> BufferPoint {
    BufferPoint {
        rows_from_live_bottom: usize::from(ROWS - 1 - row),
        column,
    }
}

fn range(start: BufferPoint, end: BufferPoint) -> BufferRange {
    BufferRange { start, end }
}

/// Tracks `range` for `viewer` at the current generation.
fn track(
    engine: &mut TerminalEngine,
    viewer: ViewerId,
    id: u64,
    range: BufferRange,
) {
    let generation = engine.generation();
    engine.track_selection(
        viewer,
        Some(TrackedSelection {
            id,
            generation,
            range,
        }),
    );
}

/// Where `viewer`'s selection `id` lies, after checking that its range
/// and its text by id both read `expected`.
fn kept(
    engine: &mut TerminalEngine,
    viewer: ViewerId,
    id: u64,
    expected: &str,
) -> BufferRange {
    let placement = engine.selection_placement(viewer).unwrap();
    assert_eq!(placement.id, id);
    let range = placement.range.expect("the selection is kept");
    let generation = engine.generation();
    assert_eq!(
        engine.extract_text(generation, range).unwrap().as_deref(),
        Some(expected),
        "the placement's text, at {range:?}"
    );
    assert_eq!(
        engine
            .tracked_selection_text(viewer, id)
            .unwrap()
            .as_deref(),
        Some(expected),
        "the text by id"
    );
    range
}

fn gone(engine: &mut TerminalEngine, viewer: ViewerId, id: u64) {
    assert_eq!(
        engine.selection_placement(viewer),
        Some(SelectionPlacement { id, range: None })
    );
    assert_eq!(engine.tracked_selection_text(viewer, id).unwrap(), None);
    assert_eq!(engine.tracked_selections(), 0, "its references are freed");
}

fn lines(first: usize, count: usize) -> String {
    let mut output = String::new();
    for line in first..first + count {
        let _ = write!(output, "line {line}\r\n");
    }
    output
}

#[test]
fn a_width_change_that_rewraps_keeps_the_same_text() {
    let mut engine = started(20, ROWS, b"top\r\nalpha beta gamma\r\nbottom");
    let selected = range(at(1, 6), at(1, 15));
    track(&mut engine, VIEWER, 1, selected);
    assert_eq!(kept(&mut engine, VIEWER, 1, "beta gamma"), selected);
    resize(&mut engine, 8, ROWS);
    let narrow = kept(&mut engine, VIEWER, 1, "beta gamma");
    assert_ne!(
        narrow.start.rows_from_live_bottom, narrow.end.rows_from_live_bottom,
        "the selection wraps across rows: {narrow:?}"
    );
    resize(&mut engine, 20, ROWS);
    kept(&mut engine, VIEWER, 1, "beta gamma");
}

#[test]
fn a_height_only_change_keeps_the_selection() {
    let mut engine = started(20, ROWS, b"one\r\ntwo\r\nthree\r\nfour");
    let selected = range(at(1, 0), at(2, 4));
    track(&mut engine, VIEWER, 1, selected);
    // Shrinking moves the top rows into history; growing brings them back.
    resize(&mut engine, 20, 2);
    kept(&mut engine, VIEWER, 1, "two\nthree");
    resize(&mut engine, 20, 6);
    kept(&mut engine, VIEWER, 1, "two\nthree");
}

#[test]
fn a_cell_size_only_change_keeps_the_range() {
    let mut engine = started(20, ROWS, b"alpha beta");
    let selected = range(at(0, 0), at(0, 4));
    track(&mut engine, VIEWER, 1, selected);
    let generation = engine.generation();
    engine
        .resize(
            engine.size(),
            CellSize {
                width: 10,
                height: 20,
            },
        )
        .unwrap();
    assert_ne!(engine.generation(), generation, "a resize is a change");
    assert_eq!(kept(&mut engine, VIEWER, 1, "alpha"), selected);
}

#[test]
fn a_selection_partly_in_history_follows_reflow() {
    let mut engine = started(20, ROWS, b"");
    engine
        .process(format!("{}one two three four\r\n", lines(0, 3)).as_bytes())
        .unwrap();
    engine.process(lines(3, 3).as_bytes()).unwrap();
    // From `line 2` in history to `line 4` on the screen.
    let selected = range(
        BufferPoint {
            rows_from_live_bottom: 5,
            column: 0,
        },
        BufferPoint {
            rows_from_live_bottom: 2,
            column: 5,
        },
    );
    track(&mut engine, VIEWER, 1, selected);
    let expected = "line 2\none two three four\nline 3\nline 4";
    kept(&mut engine, VIEWER, 1, expected);
    resize(&mut engine, 7, ROWS);
    kept(&mut engine, VIEWER, 1, expected);
    resize(&mut engine, 30, ROWS);
    kept(&mut engine, VIEWER, 1, expected);
}

/// A narrower width pushes `中` from the end of the first row onto the next,
/// leaving a spacer head where its first half was.
#[test]
fn an_end_on_a_wide_character_pushed_across_the_wrap_moves_with_it() {
    let output = "abcdefgh\u{4e2d}x".as_bytes();
    // The end on the character's first half.
    let mut engine = started(10, ROWS, output);
    let selected = range(at(0, 0), at(0, 8));
    track(&mut engine, VIEWER, 1, selected);
    kept(&mut engine, VIEWER, 1, "abcdefgh\u{4e2d}");
    resize(&mut engine, 9, ROWS);
    let moved = kept(&mut engine, VIEWER, 1, "abcdefgh\u{4e2d}");
    assert_eq!(
        moved.end,
        BufferPoint {
            rows_from_live_bottom: moved.start.rows_from_live_bottom - 1,
            column: 1
        },
        "the end is on the character's second column, not the spacer head"
    );
    resize(&mut engine, 10, ROWS);
    kept(&mut engine, VIEWER, 1, "abcdefgh\u{4e2d}");

    // The start on the character's first half.
    let mut engine = started(10, ROWS, output);
    let selected = range(at(0, 8), at(1, 0));
    track(&mut engine, VIEWER, 1, selected);
    kept(&mut engine, VIEWER, 1, "\u{4e2d}x");
    resize(&mut engine, 9, ROWS);
    let moved = kept(&mut engine, VIEWER, 1, "\u{4e2d}x");
    assert_eq!(
        moved.start,
        BufferPoint {
            rows_from_live_bottom: moved.end.rows_from_live_bottom,
            column: 0
        },
        "the start is on the character, not the spacer head"
    );
}

#[test]
fn output_that_scrolls_the_selection_keeps_it_on_its_text() {
    let mut engine = started(20, ROWS, b"alpha beta\r\n");
    let selected = range(at(0, 6), at(0, 9));
    track(&mut engine, VIEWER, 1, selected);
    engine.process(lines(0, 10).as_bytes()).unwrap();
    let scrolled = kept(&mut engine, VIEWER, 1, "beta");
    // The cursor started on the second row, so ten lines scroll eight.
    assert_eq!(
        scrolled.start.rows_from_live_bottom,
        selected.start.rows_from_live_bottom + 8
    );
}

#[test]
fn output_that_rewrites_the_selection_in_place_shows_the_new_text() {
    let mut engine = started(20, ROWS, b"alpha beta\r\nnext");
    let selected = range(at(0, 6), at(0, 9));
    track(&mut engine, VIEWER, 1, selected);
    engine.process(b"\x1b[1;7HBETA").unwrap();
    assert_eq!(kept(&mut engine, VIEWER, 1, "BETA"), selected);
}

/// More rows than one Ghostty page holds at 80 columns.
const PAGES: usize = 3_000;

#[test]
fn eviction_shrinks_a_selection_to_its_retained_rows_or_clears_it() {
    let mut engine = started(80, ROWS, b"");
    engine.process(lines(0, PAGES).as_bytes()).unwrap();
    engine.process(b"last").unwrap();
    // `line 0`, above the other lines and `last`.
    let oldest = PAGES;
    // From the oldest line to the screen, and within the oldest lines.
    let spanning = range(
        BufferPoint {
            rows_from_live_bottom: oldest,
            column: 0,
        },
        at(3, 3),
    );
    let early = range(
        BufferPoint {
            rows_from_live_bottom: oldest,
            column: 0,
        },
        BufferPoint {
            rows_from_live_bottom: oldest - 1,
            column: 5,
        },
    );
    let other = ViewerId::new(2);
    track(&mut engine, VIEWER, 1, spanning);
    track(&mut engine, other, 2, early);
    assert!(
        engine
            .set_scrollback(ScrollbackLimit::new(100).unwrap())
            .unwrap()
    );
    let placement = engine.selection_placement(VIEWER).unwrap();
    let shrunk = placement.range.expect("the newer rows are kept");
    let history = engine
        .requested_viewport(Some(ScrollCommand::Absolute(usize::MAX)))
        .unwrap()
        .bottom_offset;
    assert_eq!(
        shrunk.start,
        BufferPoint {
            rows_from_live_bottom: history + 3,
            column: 0
        },
        "it starts at the oldest retained row"
    );
    assert_eq!(shrunk.end, spanning.end);
    let text = engine.tracked_selection_text(VIEWER, 1).unwrap().unwrap();
    // The screen holds the last three lines and `last`.
    let first = PAGES - 3 - history;
    assert!(
        text.starts_with(&format!("line {first}\n")) && text.ends_with("last"),
        "{text:?}"
    );
    assert_eq!(
        engine.selection_placement(other),
        Some(SelectionPlacement { id: 2, range: None })
    );
}

#[test]
fn a_program_s_history_erasure_shrinks_the_selection_to_the_screen() {
    let mut engine = started(20, ROWS, lines(0, 6).as_bytes());
    engine.process(b"last").unwrap();
    let selected = range(
        BufferPoint {
            rows_from_live_bottom: 5,
            column: 0,
        },
        at(3, 3),
    );
    track(&mut engine, VIEWER, 1, selected);
    engine.process(b"\x1b[3J").unwrap();
    let shrunk = kept(&mut engine, VIEWER, 1, "line 3\nline 4\nline 5\nlast");
    assert_eq!(shrunk.start, at(0, 0));
}

#[test]
fn ris_clears_the_selection() {
    let mut engine = started(20, ROWS, b"alpha beta");
    track(&mut engine, VIEWER, 1, range(at(0, 0), at(0, 4)));
    engine.process(b"\x1bc").unwrap();
    gone(&mut engine, VIEWER, 1);
}

#[test]
fn a_snapshot_on_the_alternate_screen_clears_a_primary_selection() {
    let mut engine = started(20, ROWS, b"alpha beta");
    let selected = range(at(0, 0), at(0, 4));
    track(&mut engine, VIEWER, 1, selected);
    // A round trip between two placements leaves the primary cells intact.
    engine.process(b"\x1b[?1049hfull\x1b[?1049l").unwrap();
    assert_eq!(kept(&mut engine, VIEWER, 1, "alpha"), selected);
    engine.process(b"\x1b[?1049hfull").unwrap();
    gone(&mut engine, VIEWER, 1);
    // It stays gone once a viewer saw it go.
    engine.process(b"\x1b[?1049l").unwrap();
    gone(&mut engine, VIEWER, 1);
}

#[test]
fn any_grid_change_clears_an_alternate_screen_selection() {
    for (columns, rows) in [(20, 6), (30, 4), (10, 4)] {
        let mut engine = started(20, ROWS, b"\x1b[?1049h\x1b[2;1Hfull screen");
        track(&mut engine, VIEWER, 1, range(at(1, 0), at(1, 3)));
        kept(&mut engine, VIEWER, 1, "full");
        resize(&mut engine, columns, rows);
        gone(&mut engine, VIEWER, 1);
    }
}

#[test]
fn a_column_decrease_with_wraparound_off_clears_the_selection() {
    let mut engine = started(20, ROWS, b"\x1b[?7lalpha beta gamma");
    track(&mut engine, VIEWER, 1, range(at(0, 0), at(0, 4)));
    resize(&mut engine, 30, ROWS);
    kept(&mut engine, VIEWER, 1, "alpha");
    resize(&mut engine, 10, ROWS);
    gone(&mut engine, VIEWER, 1);
}

#[test]
fn a_selection_made_at_a_stale_generation_is_gone() {
    let mut engine = started(20, ROWS, b"alpha beta");
    let generation = engine.generation();
    engine.process(b" gamma").unwrap();
    engine.track_selection(
        VIEWER,
        Some(TrackedSelection {
            id: 1,
            generation,
            range: range(at(0, 0), at(0, 4)),
        }),
    );
    gone(&mut engine, VIEWER, 1);
}

#[test]
fn a_new_selection_replaces_the_old_and_none_forgets_it() {
    let mut engine = started(20, ROWS, b"alpha beta");
    track(&mut engine, VIEWER, 1, range(at(0, 0), at(0, 4)));
    track(&mut engine, VIEWER, 2, range(at(0, 6), at(0, 9)));
    assert_eq!(engine.tracked_selections(), 1);
    kept(&mut engine, VIEWER, 2, "beta");
    assert_eq!(
        engine.tracked_selection_text(VIEWER, 1).unwrap(),
        None,
        "the replaced selection has no text"
    );
    engine.track_selection(VIEWER, None);
    assert_eq!(engine.selection_placement(VIEWER), None);
    assert_eq!(engine.tracked_selections(), 0);
}

#[test]
fn two_viewers_keep_separate_selections_through_one_resize() {
    let mut engine = started(20, ROWS, b"alpha beta gamma");
    let other = ViewerId::new(2);
    track(&mut engine, VIEWER, 1, range(at(0, 0), at(0, 4)));
    track(&mut engine, other, 7, range(at(0, 11), at(0, 15)));
    resize(&mut engine, 8, ROWS);
    kept(&mut engine, VIEWER, 1, "alpha");
    kept(&mut engine, other, 7, "gamma");
    engine.forget_selection(VIEWER);
    assert_eq!(engine.selection_placement(VIEWER), None);
    kept(&mut engine, other, 7, "gamma");
}

#[test]
fn placements_over_compressed_history_restore_no_page() {
    if !huterm_ghostty::COMPRESSION_SUPPORTED {
        return;
    }
    let mut engine = started(120, 40, b"");
    let mut output = String::new();
    for line in 0..4_000 {
        let _ = write!(output, "\x1b[3{}m{line:05}\x1b[0m text\r\n", line % 8);
    }
    engine.process(output.as_bytes()).unwrap();
    let oldest = 4_000;
    track(
        &mut engine,
        VIEWER,
        1,
        range(
            BufferPoint {
                rows_from_live_bottom: oldest,
                column: 0,
            },
            BufferPoint {
                rows_from_live_bottom: oldest - 1_000,
                column: 4,
            },
        ),
    );
    while engine.compress_step().unwrap()
        == huterm_ghostty::Compression::Pending
    {}
    let before = engine.memory_usage().unwrap().primary.compressed_pages;
    assert!(before > 0, "history was compressed");
    for _ in 0..3 {
        engine.snapshot().unwrap();
        let placement = engine.selection_placement(VIEWER).unwrap();
        assert!(placement.range.is_some());
    }
    assert_eq!(
        engine.memory_usage().unwrap().primary.compressed_pages,
        before
    );
}
