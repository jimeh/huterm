//! Retained history under each scrollback limit, measured through the
//! snapshots and viewport the runtime publishes.

use std::fmt::Write as _;

use huterm_protocol::{
    CellSize, GridSize, ScrollCommand, ScrollbackLimit, TerminalId,
    TerminalPresentation, TerminalSnapshot,
};

use super::TerminalEngine;

/// More rows than one Ghostty page holds at 80 columns or wider, the most
/// that whole-page pruning can fall short of a limit by there.
const PAGE_ROWS_AT_80: usize = 650;

fn engine(columns: u16, limit: ScrollbackLimit) -> TerminalEngine {
    sized_engine(columns, 4, limit)
}

fn sized_engine(
    columns: u16,
    rows: u16,
    limit: ScrollbackLimit,
) -> TerminalEngine {
    let mut engine = TerminalEngine::new(
        TerminalId::new(1),
        GridSize::clamped(columns, rows),
        CellSize {
            width: 8,
            height: 16,
        },
        TerminalPresentation::default(),
    )
    .unwrap();
    engine.set_scrollback(limit).unwrap();
    engine
}

fn limit(rows: u32) -> ScrollbackLimit {
    ScrollbackLimit::new(rows).unwrap()
}

/// Writes lines numbered from `first`, each ending the row it starts.
fn write_lines(engine: &mut TerminalEngine, first: usize, count: usize) {
    let mut output = String::new();
    for line in first..first + count {
        let _ = write!(output, "line {line}\r\n");
    }
    engine.process(output.as_bytes()).unwrap();
}

/// Writes `count` lines of `width` cells each.
fn write_wide_lines(engine: &mut TerminalEngine, width: usize, count: usize) {
    let line = format!("{}\r\n", "w".repeat(width));
    engine.process(line.repeat(count).as_bytes()).unwrap();
}

/// Rows of history, read as the largest offset a scroll may reach.
fn history(engine: &TerminalEngine) -> usize {
    engine
        .requested_viewport(Some(ScrollCommand::Absolute(usize::MAX)))
        .unwrap()
        .bottom_offset
}

fn rows_text(snapshot: &TerminalSnapshot) -> Vec<String> {
    snapshot
        .rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// The rows the snapshot's viewport must show after `lines` numbered
/// lines: the live bottom row is blank, and the row `k` rows above it
/// holds line `lines - k`.
fn expected_rows(snapshot: &TerminalSnapshot, lines: usize) -> Vec<String> {
    let rows = usize::from(snapshot.size.rows);
    (0..rows)
        .map(|row| {
            let above = snapshot.viewport.bottom_offset + rows - 1 - row;
            if above == 0 {
                String::new()
            } else {
                format!("line {}", lines - above)
            }
        })
        .collect()
}

#[test]
fn zero_keeps_no_history_until_a_later_limit_restores_it() {
    let mut engine = engine(80, ScrollbackLimit::NONE);
    write_lines(&mut engine, 0, 50);
    assert_eq!(history(&engine), 0);

    engine.set_scrollback(ScrollbackLimit::DEFAULT).unwrap();
    write_lines(&mut engine, 50, 50);
    assert_eq!(history(&engine), 50);

    // Zero erases history rather than pruning pages; it still counts.
    assert!(engine.set_scrollback(ScrollbackLimit::NONE).unwrap());
    assert_eq!(history(&engine), 0);
    let snapshot = engine.snapshot().unwrap();
    assert_eq!(snapshot.history_size, 0);
    assert_eq!(rows_text(&snapshot), expected_rows(&snapshot, 100));
}

#[test]
fn a_limit_below_one_page_keeps_up_to_one_page() {
    let mut engine = engine(80, limit(1));
    let mut peak = 0;
    for line in 0..2000 {
        write_lines(&mut engine, line, 1);
        peak = peak.max(history(&engine));
    }
    assert!(
        (400..=PAGE_ROWS_AT_80).contains(&peak),
        "a one-row limit kept up to {peak} rows"
    );
}

#[test]
fn plain_output_keeps_up_to_the_row_limit_and_less_than_a_page_fewer() {
    // The configuration reference promises this up to about 400 columns.
    for (columns, width) in [(80, 8), (400, 400)] {
        let mut engine = engine(columns, ScrollbackLimit::DEFAULT);
        write_wide_lines(&mut engine, width, 15_000);
        let kept = history(&engine);
        assert!(
            kept <= 10_000 && kept > 10_000 - PAGE_ROWS_AT_80,
            "{columns} columns kept {kept} rows"
        );
    }

    // The largest limit keeps more than the former 16 MiB budget did,
    // about 23,000 rows at 80 columns.
    let mut engine = engine(80, ScrollbackLimit::MAX);
    write_lines(&mut engine, 0, 30_000);
    assert_eq!(history(&engine), 30_000 - 3);
}

/// Writes 400-column lines and returns the most and, once pruning began,
/// the fewest rows of history kept.
fn wide_history_range(engine: &mut TerminalEngine) -> (usize, usize) {
    let line = format!("{}\r\n", "w".repeat(400));
    let (mut peak, mut floor, mut previous) = (0, usize::MAX, 0);
    for _ in 0..3000 {
        engine.process(line.as_bytes()).unwrap();
        let kept = history(engine);
        peak = peak.max(kept);
        if kept < previous || floor != usize::MAX {
            floor = floor.min(kept);
        }
        previous = kept;
    }
    (peak, floor)
}

#[test]
fn small_limits_on_wide_tall_screens_keep_the_row_guarantee() {
    // A 400-column page holds about 119 rows, and the screen's own pages
    // count toward the byte cap too.
    for screen_rows in [60, 150, 1500] {
        let mut engine = sized_engine(400, screen_rows, limit(350));
        let (peak, floor) = wide_history_range(&mut engine);
        assert_eq!(peak, 350, "{screen_rows} screen rows");
        assert!(floor > 350 - 120, "{screen_rows} screen rows kept {floor}");
    }

    // Growing the screen raises the allowance with it.
    let mut engine = sized_engine(400, 60, limit(350));
    wide_history_range(&mut engine);
    engine
        .resize(
            GridSize::clamped(400, 1500),
            CellSize {
                width: 8,
                height: 16,
            },
        )
        .unwrap();
    let (peak, floor) = wide_history_range(&mut engine);
    assert_eq!(peak, 350, "after growing the screen");
    assert!(floor > 350 - 120, "kept {floor} after growing the screen");
}

#[test]
fn rows_too_wide_for_the_byte_cap_keep_fewer_rows_than_the_limit() {
    // A 1,000-column row needs about 8 KiB, twice the cap's allowance.
    let mut engine = engine(1000, ScrollbackLimit::DEFAULT);
    write_wide_lines(&mut engine, 1000, 12_000);
    let kept = history(&engine);
    assert!(kept > 1000 && kept < 7000, "kept {kept} rows");
}

#[test]
fn shrinking_keeps_a_scrolled_viewport_on_its_rows_or_moves_it_to_the_top() {
    let mut engine = engine(80, ScrollbackLimit::DEFAULT);
    write_lines(&mut engine, 0, 5000);
    engine.scroll(ScrollCommand::Absolute(1000)).unwrap();
    let before = engine.snapshot().unwrap();
    assert_eq!(before.viewport.bottom_offset, 1000);

    // Resending the current limit or raising it evicts nothing, so neither
    // is a content change.
    assert!(!engine.set_scrollback(ScrollbackLimit::DEFAULT).unwrap());
    assert!(!engine.set_scrollback(ScrollbackLimit::MAX).unwrap());
    assert_eq!(engine.generation(), before.generation);
    assert_eq!(history(&engine), before.history_size);

    // The viewport's rows survive: it stays on them, and the shrink counts
    // as a content change so selections and cached rows are dropped.
    assert!(engine.set_scrollback(limit(2000)).unwrap());
    let kept = engine.snapshot().unwrap();
    assert!(kept.generation > before.generation);
    assert!(
        kept.history_size <= 2000 && kept.history_size > 1000,
        "kept {} rows",
        kept.history_size
    );
    assert_eq!(kept.viewport.bottom_offset, 1000);
    assert_eq!(rows_text(&kept), rows_text(&before));
    assert_eq!(rows_text(&kept), expected_rows(&kept, 5000));

    // At the top, the viewport's rows are evicted and it moves to the new
    // top, showing the oldest rows that remain.
    engine.scroll(ScrollCommand::Absolute(usize::MAX)).unwrap();
    engine.snapshot().unwrap();
    engine.set_scrollback(limit(1)).unwrap();
    let top = engine.snapshot().unwrap();
    assert!(top.history_size <= PAGE_ROWS_AT_80);
    assert_eq!(top.viewport.bottom_offset, top.history_size);
    assert_eq!(rows_text(&top), expected_rows(&top, 5000));

    // Output continues within the smaller limit.
    write_lines(&mut engine, 5000, 3000);
    engine.scroll(ScrollCommand::Live).unwrap();
    let live = engine.snapshot().unwrap();
    assert!(live.history_size <= PAGE_ROWS_AT_80);
    assert_eq!(rows_text(&live), expected_rows(&live, 8000));
}

#[test]
fn shrinking_on_the_alternate_screen_bounds_history_after_it_exits() {
    let mut engine = engine(80, ScrollbackLimit::DEFAULT);
    write_lines(&mut engine, 0, 5000);
    engine.process(b"\x1b[?1049h").unwrap();
    let alternate = engine.snapshot().unwrap();
    assert_eq!(alternate.history_size, 0);

    // The alternate screen shows no history, so pruning the primary one
    // changes nothing on screen; leaving it is output, which advances the
    // generation.
    assert!(!engine.set_scrollback(limit(1000)).unwrap());
    assert_eq!(engine.generation(), alternate.generation);
    engine.process(b"\x1b[?1049l").unwrap();
    assert!(engine.generation() > alternate.generation);
    let primary = engine.snapshot().unwrap();
    assert!(
        primary.history_size <= 1000 && primary.history_size > 0,
        "kept {} rows",
        primary.history_size
    );
    assert_eq!(rows_text(&primary), expected_rows(&primary, 5000));
}

#[test]
fn narrowing_evicts_rows_that_rewrapping_pushes_past_the_limit() {
    let mut engine = engine(80, limit(2000));
    write_wide_lines(&mut engine, 60, 2000);
    assert!(history(&engine) > 1500);

    // Each line now fills two rows, which would be about 4,000 rows.
    engine
        .resize(
            GridSize::clamped(40, 4),
            CellSize {
                width: 8,
                height: 16,
            },
        )
        .unwrap();
    let kept = history(&engine);
    assert!(kept > 0 && kept <= 2000, "kept {kept} rows");
}

fn with_bytes(rows: u32, bytes: u64) -> ScrollbackLimit {
    limit(rows).with_bytes(bytes).unwrap()
}

/// Writes `count` lines of `width` cells one at a time and returns the most
/// history kept at any point.
fn peak_history(
    engine: &mut TerminalEngine,
    width: usize,
    count: usize,
) -> usize {
    let line = format!("{}\r\n", "w".repeat(width));
    let mut peak = 0;
    for _ in 0..count {
        engine.process(line.as_bytes()).unwrap();
        peak = peak.max(history(engine));
    }
    peak
}

#[test]
fn whichever_of_the_row_and_byte_limits_is_reached_first_applies() {
    // Two MiB is about five 80-column pages, far fewer rows than the limit.
    let mut by_bytes = engine(80, with_bytes(50_000, 2 << 20));
    let peak = peak_history(&mut by_bytes, 8, 10_000);
    assert!(peak > 1000 && peak < 4000, "bytes kept {peak} rows");

    // A byte limit that is never reached leaves the rows to bind.
    let mut by_rows = engine(80, with_bytes(1000, 1 << 30));
    assert_eq!(peak_history(&mut by_rows, 8, 5000), 1000);
}

#[test]
fn either_zero_limit_keeps_no_history() {
    for limit in [with_bytes(0, 1 << 30), with_bytes(10_000, 0)] {
        let mut engine = engine(80, limit);
        write_lines(&mut engine, 0, 50);
        assert_eq!(history(&engine), 0, "{limit:?}");
    }
}

#[test]
fn a_byte_limit_below_the_screen_and_a_page_keeps_at_most_two_pages() {
    // Ghostty raises it to the screen's pages plus one more, and a short
    // screen shares its page with history.
    let mut tiny = engine(80, with_bytes(10_000, 1));
    let peak = peak_history(&mut tiny, 8, 3000);
    assert!(peak <= 2 * PAGE_ROWS_AT_80, "kept {peak} rows");
}

#[test]
fn only_a_change_that_evicts_counts_as_a_content_change() {
    let mut engine = engine(80, ScrollbackLimit::DEFAULT);
    write_lines(&mut engine, 0, 5000);
    let generation = engine.generation();
    let kept = history(&engine);

    // Raising either limit, or switching to a larger explicit size, evicts
    // nothing.
    assert!(!engine.set_scrollback(with_bytes(10_000, 1 << 30)).unwrap());
    assert!(!engine.set_scrollback(with_bytes(50_000, 1 << 30)).unwrap());
    assert!(!engine.set_scrollback(ScrollbackLimit::MAX).unwrap());
    assert_eq!(engine.generation(), generation);
    assert_eq!(history(&engine), kept);

    // Lowering only the bytes evicts.
    assert!(engine.set_scrollback(with_bytes(50_000, 1 << 20)).unwrap());
    assert!(engine.generation() > generation);
    assert!(history(&engine) < kept, "kept {} rows", history(&engine));
}

#[test]
fn raising_a_byte_limit_that_grown_pages_exceed_still_reports_eviction() {
    let mut engine = engine(80, with_bytes(50_000, 2 << 20));
    write_lines(&mut engine, 0, 2500);
    // Combining marks grow the page that holds them past what the byte
    // limit allows; Ghostty prunes for bytes only when it allocates.
    let grapheme = format!("x{}", "\u{301}".repeat(64));
    engine.process(grapheme.repeat(79).as_bytes()).unwrap();
    let before = engine.snapshot().unwrap();

    assert!(
        engine
            .set_scrollback(with_bytes(50_000, (2 << 20) + 1))
            .unwrap()
    );
    let after = engine.snapshot().unwrap();
    assert!(after.generation > before.generation);
    assert!(
        after.history_size < before.history_size,
        "kept {} of {} rows",
        after.history_size,
        before.history_size
    );
}

#[test]
fn explicit_bytes_stay_fixed_and_enforced_across_resizes() {
    let cell = CellSize {
        width: 8,
        height: 16,
    };
    // A taller screen gets no more bytes, so history stays within them.
    let mut taller = engine(80, with_bytes(50_000, 4 << 20));
    let before = peak_history(&mut taller, 8, 20_000);
    taller.resize(GridSize::clamped(80, 400), cell).unwrap();
    let after = peak_history(&mut taller, 8, 20_000);
    assert!(after <= before, "kept {after} rows, {before} before");

    // Widening rewraps 300-cell lines from four rows onto one, which costs
    // more bytes per row. Ghostty checks bytes only when they are set or a
    // page is allocated, not after a reflow, so the limit is applied again
    // afterward; without that, history stays above what the bytes hold.
    let mut steady = sized_engine(400, 4, with_bytes(50_000, 2 << 20));
    let most = peak_history(&mut steady, 300, 5000);
    let mut widened = engine(80, with_bytes(50_000, 2 << 20));
    peak_history(&mut widened, 300, 5000);
    widened.resize(GridSize::clamped(400, 4), cell).unwrap();
    let kept = history(&widened);
    assert!(
        kept <= most,
        "kept {kept} rows after widening; at most {most}"
    );
}
