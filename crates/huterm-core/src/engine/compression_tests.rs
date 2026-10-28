//! Compressed history reads exactly as uncompressed history does. Each test
//! drives a compressed engine and an uncompressed twin through the same
//! output and operations, then compares what viewers see.

use super::*;
use huterm_ghostty::{COMPRESSION_SUPPORTED, Compression};
use huterm_protocol::{
    BufferPoint, BufferRange, LinkLookup, MousePosition, TerminalRow,
};
use std::fmt::Write as _;

const COLUMNS: u16 = 120;
const ROWS: u16 = 40;

fn engine() -> TerminalEngine {
    TerminalEngine::new(
        TerminalId::new(1),
        GridSize::clamped(COLUMNS, ROWS),
        CellSize {
            width: 8,
            height: 16,
        },
        TerminalPresentation::default(),
    )
    .unwrap()
}

/// Several pages of history at 120 columns, where a page holds about 400
/// rows: SGR colors, truecolor backgrounds, wide and combining characters,
/// emoji, OSC 8 hyperlinks, and lines that soft-wrap.
fn history(lines: usize) -> String {
    let mut output = String::new();
    for line in 0..lines {
        let _ = write!(
            output,
            "\x1b[1;3{}m{line:05}\x1b[0m \x1b[48;2;{};{};{}m界e\u{301}🙂\x1b[0m ",
            line % 8,
            line % 256,
            line * 7 % 256,
            line * 13 % 256
        );
        if line % 5 == 0 {
            let _ = write!(
                output,
                "\x1b]8;;https://example.test/{line}\x1b\\link {line}\x1b]8;;\x1b\\ "
            );
        }
        if line % 11 == 0 {
            output.push_str(&"wrapped ".repeat(20));
        }
        output.push_str("see https://plain.example/path\r\n");
    }
    output
}

/// A compressed engine and its uncompressed twin.
struct Pair {
    compressed: TerminalEngine,
    twin: TerminalEngine,
}

impl Pair {
    fn new(lines: usize) -> Self {
        let mut pair = Self {
            compressed: engine(),
            twin: engine(),
        };
        pair.process(&history(lines));
        pair.compress();
        pair
    }

    fn apply(&mut self, operation: impl Fn(&mut TerminalEngine)) {
        operation(&mut self.compressed);
        operation(&mut self.twin);
    }

    fn process(&mut self, output: &str) {
        self.apply(|engine| {
            engine.process(output.as_bytes()).unwrap();
        });
    }

    /// Runs a complete pass over the compressed engine and returns how many
    /// pages it holds compressed.
    fn compress(&mut self) -> u64 {
        let mut steps = 0;
        while self.compressed.compress_step().unwrap() == Compression::Pending {
            steps += 1;
            assert!(steps < 100_000, "compression never completed");
        }
        self.compressed
            .memory_usage()
            .unwrap()
            .primary
            .compressed_pages
    }

    /// Compares snapshot rows, the cursor, and link lookups at several scroll
    /// positions, so rendering restores the pages it shows, then the full
    /// text, which restores the rest. Snapshot rows carry no hyperlinks, so
    /// every OSC 8 label on screen is looked up and its destination checked.
    /// Returns how many such labels it checked.
    fn assert_same(&mut self, label: &str) -> usize {
        let mut explicit = 0;
        for offset in [0, 600, 1_500, usize::MAX] {
            self.apply(|engine| {
                engine.scroll(ScrollCommand::Absolute(offset)).unwrap();
            });
            let compressed = self.compressed.snapshot().unwrap();
            let twin = self.twin.snapshot().unwrap();
            assert_eq!(compressed.rows, twin.rows, "{label}: rows at {offset}");
            assert_eq!(
                compressed.cursor, twin.cursor,
                "{label}: cursor at {offset}"
            );
            assert_eq!(
                compressed.history_size, twin.history_size,
                "{label}: history at {offset}"
            );
            for (row, column) in [(0, 2), (0, 30), (17, 40), (39, 70)] {
                let point = MousePosition { row, column };
                assert_eq!(
                    self.compressed.lookup_link(&compressed, point),
                    self.twin.lookup_link(&twin, point),
                    "{label}: link at {offset} {row},{column}"
                );
            }
            for (row, number) in compressed
                .rows
                .iter()
                .enumerate()
                .filter_map(|(row, cells)| Some((row, link_label(cells)?)))
            {
                let point = MousePosition {
                    row: u32::try_from(row).unwrap(),
                    column: u32::from(LINK_COLUMN) + 1,
                };
                let found = self.compressed.lookup_link(&compressed, point);
                assert_eq!(
                    found,
                    self.twin.lookup_link(&twin, point),
                    "{label}: OSC 8 link at {offset} row {row}"
                );
                let LinkLookup::Match(link) = found else {
                    panic!("{label}: no OSC 8 link at {offset} row {row}");
                };
                assert_eq!(
                    link.destination,
                    format!("https://example.test/{number}"),
                    "{label}: OSC 8 destination at {offset} row {row}"
                );
                explicit += 1;
            }
        }
        self.apply(|engine| engine.scroll(ScrollCommand::Live).unwrap());
        assert_eq!(text(&self.compressed), text(&self.twin), "{label}: text");
        explicit
    }
}

/// Where `history` writes each OSC 8 label.
const LINK_COLUMN: u16 = 12;

/// The line number of a row that starts a `history` line with an OSC 8
/// label.
fn link_label(row: &TerminalRow) -> Option<usize> {
    let text = |range: std::ops::Range<usize>| -> String {
        row.cells[range]
            .iter()
            .map(|cell| cell.text.as_str())
            .collect()
    };
    let start = usize::from(LINK_COLUMN);
    if row.cells.len() < start + 4 || text(start..start + 4) != "link" {
        return None;
    }
    text(0..5).parse().ok()
}

/// The history rows just above the screen.
fn recent_history(engine: &TerminalEngine) -> String {
    let size = engine.size();
    let rows = usize::from(size.rows);
    let range = BufferRange {
        start: BufferPoint {
            rows_from_live_bottom: rows + 45,
            column: 0,
        },
        end: BufferPoint {
            rows_from_live_bottom: rows,
            column: size.columns - 1,
        },
    };
    engine
        .extract_text(engine.generation(), range)
        .unwrap()
        .unwrap()
}

/// All history and the screen, as a copied selection reads it.
fn text(engine: &TerminalEngine) -> String {
    let size = engine.size();
    let history = engine.inner.viewport_state().unwrap().1;
    let range = BufferRange {
        start: BufferPoint {
            rows_from_live_bottom: history + usize::from(size.rows) - 1,
            column: 0,
        },
        end: BufferPoint {
            rows_from_live_bottom: 0,
            column: size.columns - 1,
        },
    };
    engine
        .extract_text(engine.generation(), range)
        .unwrap()
        .unwrap()
}

#[test]
fn compressed_history_reads_like_uncompressed_history() {
    if !COMPRESSION_SUPPORTED {
        return;
    }
    let mut pair = Pair::new(3_000);
    let compressed = pair.compress();
    assert!(compressed >= 5, "{compressed} compressed pages");
    // Rendering a scrolled viewport restores the pages it shows, so the
    // comparisons below read pages that were compressed.
    pair.compressed
        .scroll(ScrollCommand::Absolute(1_500))
        .unwrap();
    pair.compressed.snapshot().unwrap();
    assert!(
        pair.compressed
            .memory_usage()
            .unwrap()
            .primary
            .compressed_pages
            < compressed,
        "rendering scrolled history restored no page"
    );
    pair.compressed.scroll(ScrollCommand::Live).unwrap();
    assert!(pair.assert_same("compressed") > 0);
}

#[test]
fn reflow_restores_compressed_history_without_changing_it() {
    if !COMPRESSION_SUPPORTED {
        return;
    }
    let mut pair = Pair::new(3_000);
    for (columns, rows) in [(97, 33), (COLUMNS, ROWS), (151, 45)] {
        assert!(pair.compress() > 0);
        let size = GridSize::clamped(columns, rows);
        let cell = CellSize {
            width: 8,
            height: 16,
        };
        pair.apply(|engine| {
            engine.resize(size, cell).unwrap();
        });
        assert!(pair.assert_same(&format!("{columns}x{rows}")) > 0);
    }
    // Pages compressed after a reflow read back like the twin's.
    assert!(pair.compress() > 0);
    assert!(pair.assert_same("compressed after reflow") > 0);
}

#[test]
fn a_pass_on_the_alternate_screen_compresses_primary_history() {
    if !COMPRESSION_SUPPORTED {
        return;
    }
    let mut pair = Pair::new(3_000);
    assert!(pair.assert_same("before") > 0);
    pair.process("\x1b[?1049h\x1b[Halternate screen\r\n");
    let compressed = pair.compress();
    assert!(compressed > 0);
    let usage = pair.compressed.memory_usage().unwrap();
    assert_eq!(usage.alternate.compressed_pages, 0, "{usage:?}");
    pair.process("\x1b[?1049l");
    assert!(pair.assert_same("after the alternate screen") > 0);
}

#[test]
fn erasing_compressed_history_matches_the_twin() {
    if !COMPRESSION_SUPPORTED {
        return;
    }
    let mut pair = Pair::new(3_000);
    let limit = ScrollbackLimit::new(1_000).unwrap();
    pair.apply(|engine| {
        engine.set_scrollback(limit).unwrap();
    });
    assert!(pair.assert_same("lowered limit") > 0);

    pair.process(&history(2_000));
    assert!(pair.compress() > 0);
    pair.apply(|engine| {
        engine.clear_history().unwrap();
    });
    assert!(pair.assert_same("cleared") > 0);

    pair.process(&history(2_000));
    assert!(pair.compress() > 0);
    pair.apply(|engine| {
        engine.reset().unwrap();
    });
    pair.assert_same("reset");
}

/// ED 22, and ED 2 at a shell-integration prompt, move the screen into
/// history row by row, which recycles the oldest page at the limit and
/// moves the cursor's style and hyperlink to another page.
#[test]
fn scroll_clear_over_compressed_history_matches_the_twin() {
    if !COMPRESSION_SUPPORTED {
        return;
    }
    for (label, clear) in [
        ("ED 22", "\x1b[22J"),
        (
            "ED 2 at a prompt",
            "\x1b]133;A\x07$ \x1b]133;B\x07\x1b[H\x1b[2J",
        ),
    ] {
        let mut pair = Pair::new(0);
        let limit = ScrollbackLimit::new(2_000).unwrap();
        pair.apply(|engine| {
            engine.set_scrollback(limit).unwrap();
        });
        pair.process(&history(3_000));
        pair.process(&"screen line\r\n".repeat(usize::from(ROWS) - 1));
        pair.process("\x1b[1;31m\x1b]8;;https://example.test/cursor\x1b\\");
        for round in 0..3 {
            assert!(pair.compress() > 0);
            pair.process(clear);
            if round == 0 {
                // Scrolling the screen into history keeps its rows; erasing
                // the screen in place would drop them.
                let above = recent_history(&pair.compressed);
                assert!(above.contains("screen line"), "{label}: {above:?}");
            }
            pair.process("styled after clear\r\n");
            // The text after the clear carries the cursor's hyperlink, which
            // scrollClear moved with the cursor.
            let snapshot = pair.compressed.snapshot().unwrap();
            let row = snapshot
                .rows
                .iter()
                .position(|row| {
                    row.cells[..7]
                        .iter()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>()
                        == "styled "
                })
                .expect("the text written after the clear is on screen");
            let point = MousePosition {
                row: u32::try_from(row).unwrap(),
                column: 2,
            };
            let twin = pair.twin.snapshot().unwrap();
            let found = pair.compressed.lookup_link(&snapshot, point);
            assert_eq!(found, pair.twin.lookup_link(&twin, point), "{label}");
            let LinkLookup::Match(link) = found else {
                panic!("{label}: the cursor lost its hyperlink");
            };
            assert_eq!(link.destination, "https://example.test/cursor");
            pair.assert_same(&format!("{label}, round {round}"));
        }
    }
}
