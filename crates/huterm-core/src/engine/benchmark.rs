use super::*;
use std::fmt::Write as _;
use std::hint::black_box;
use std::time::Instant;

struct Fixture {
    name: &'static str,
    chunk: fn(usize) -> String,
    expected: &'static str,
    fill_scrollback: bool,
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "ascii",
        chunk: |_| "ordinary terminal output\r\n".repeat(100),
        expected: "ordinary",
        fill_scrollback: false,
    },
    Fixture {
        name: "styled",
        chunk: |iteration| {
            let color = if iteration % 2 == 0 { 31 } else { 32 };
            format!("\x1b[{color};1mred\x1b[0m normal\r\n").repeat(100)
        },
        expected: "red",
        fill_scrollback: false,
    },
    Fixture {
        name: "unicode",
        chunk: |_| "界e\u{301}🙂 terminal\r\n".repeat(100),
        expected: "terminal",
        fill_scrollback: false,
    },
    Fixture {
        name: "sparse",
        chunk: |iteration| {
            format!(
                "\x1b[20;1Hsparse {}",
                if iteration % 2 == 0 { 'A' } else { 'B' }
            )
        },
        expected: "sparse",
        fill_scrollback: false,
    },
    Fixture {
        name: "full",
        chunk: |iteration| {
            "\x1b[H".to_owned()
                + &(if iteration % 2 == 0 { "x" } else { "y" }).repeat(120 * 40)
        },
        expected: "",
        fill_scrollback: false,
    },
    // Shell integration: prompt marks and a directory report on one row.
    Fixture {
        name: "prompt",
        chunk: |iteration| {
            format!(
                "\x1b]133;A\x07\x1b[20;1H\x1b[2K$ \x1b]133;B\x07\x1b]7;file:///tmp/{}\x07",
                iteration % 2
            )
        },
        expected: "$",
        fill_scrollback: false,
    },
    Fixture {
        name: "title",
        chunk: |iteration| {
            format!("\x1b]2;title {0}\x07\x1b[20;1Htitled {0}", iteration % 2)
        },
        expected: "titled",
        fill_scrollback: false,
    },
    Fixture {
        name: "osc8",
        chunk: |iteration| {
            format!(
                "\x1b[20;1H\x1b]8;;file:///tmp/{0}\x07entry-{0}\x1b]8;;\x07",
                iteration % 2
            )
        },
        expected: "entry-",
        fill_scrollback: false,
    },
    // Explicit IDs rule out implicit-ID growth as the cause: once cells
    // are re-linked, Ghostty reports a full redraw for both fixtures.
    Fixture {
        name: "osc8-id",
        chunk: |iteration| {
            format!(
                "\x1b[20;1H\x1b]8;id=link-{0};file:///tmp/{0}\x07entry-{0}\x1b]8;;\x07",
                iteration % 2
            )
        },
        expected: "entry-",
        fill_scrollback: false,
    },
    // `╝` and `帝` both encode a 0x9d continuation byte.
    Fixture {
        name: "box-cjk",
        chunk: |iteration| {
            format!("\x1b[20;1H\x1b[3{}m╝帝\x1b[0m", 1 + iteration % 2)
        },
        expected: "╝帝",
        fill_scrollback: false,
    },
    // Control: a real palette change must still rebuild every row.
    Fixture {
        name: "color",
        chunk: |iteration| {
            format!(
                "\x1b]4;1;rgb:{}/00/00\x07\x1b[20;1Hcolor",
                if iteration % 2 == 0 { "ff" } else { "80" }
            )
        },
        expected: "color",
        fill_scrollback: false,
    },
    // A DOOM-fire style frame: truecolor foreground and background SGR
    // for every cell, which dominates the bytes the parser sees.
    Fixture {
        name: "truecolor",
        chunk: truecolor_chunk,
        expected: "\u{2580}",
        fill_scrollback: false,
    },
    Fixture {
        name: "scroll",
        chunk: scroll_chunk,
        expected: "line ",
        fill_scrollback: false,
    },
    Fixture {
        name: "scroll-capped",
        chunk: scroll_chunk,
        expected: "line ",
        fill_scrollback: true,
    },
];

fn truecolor_chunk(iteration: usize) -> String {
    let mut chunk = String::from("\x1b[H");
    for cell in 0..120 * 40 {
        let value = (cell * 13 + iteration * 7) % 256;
        let _ = write!(
            chunk,
            "\x1b[38;2;{value};{};{};48;2;{};{};{}m\u{2580}",
            value / 2,
            value / 4,
            255 - value,
            value / 3,
            value / 5
        );
    }
    chunk
}

/// Three distinct lines per chunk, so most rows shift rather than repeat.
fn scroll_chunk(iteration: usize) -> String {
    let mut chunk = String::new();
    for line in 0..3 {
        let _ = write!(chunk, "line {:08}\r\n", iteration * 3 + line);
    }
    chunk
}

/// Writes history until the byte budget stops `history_size` growing.
fn fill_scrollback(engine: &mut TerminalEngine) {
    let mut previous = 0;
    for batch in 0..1000 {
        let mut lines = String::new();
        for line in 0..2000 {
            let _ = write!(lines, "fill {batch:04} {line:04}\r\n");
        }
        engine.process(lines.as_bytes()).unwrap();
        let history = engine.snapshot().unwrap().history_size;
        if history <= previous {
            return;
        }
        previous = history;
    }
    panic!("scrollback never reached its byte budget");
}

#[test]
#[ignore = "release benchmark; run through mise run bench:engine"]
fn engine_benchmark() {
    for fixture in FIXTURES {
        let name = fixture.name;
        let mut engine = TerminalEngine::new(
            TerminalId::new(1),
            GridSize::clamped(120, 40),
            CellSize {
                width: 8,
                height: 16,
            },
            TerminalPresentation::default(),
        )
        .unwrap();
        engine.process(b"warmup").unwrap();
        if fixture.fill_scrollback {
            fill_scrollback(&mut engine);
        }
        let mut previous = engine.snapshot().unwrap();
        let mut processing = Vec::new();
        let mut snapshots = Vec::new();
        let mut combined = Vec::new();
        let mut extracted = 0;
        let mut allocated = 0;
        let mut reused = 0;
        let mut bytes = 0;
        for iteration in 0..200 {
            let chunk = (fixture.chunk)(iteration);
            bytes += chunk.len();
            let started = Instant::now();
            black_box(engine.process(black_box(chunk.as_bytes())).unwrap());
            processing.push(started.elapsed().as_nanos());
            let started = Instant::now();
            let snapshot = engine.snapshot().unwrap();
            snapshots.push(started.elapsed().as_nanos());
            combined.push(processing[iteration] + snapshots[iteration]);
            let stats = engine.last_snapshot_stats();
            extracted += stats.extracted;
            allocated += stats.allocated;
            reused += stats.reused;
            assert_eq!(snapshot.size, GridSize::clamped(120, 40));
            let text: String =
                snapshot.cells().map(|cell| cell.text.as_str()).collect();
            assert!(text.contains(fixture.expected), "{name}: {text:?}");
            if name == "styled" {
                let cell =
                    snapshot.cells().find(|cell| cell.text == "r").unwrap();
                assert!(cell.style.bold);
                assert_eq!(
                    cell.foreground,
                    huterm_protocol::CellColor::Indexed(
                        if iteration % 2 == 0 { 1 } else { 2 }
                    )
                );
            }
            if name == "unicode" {
                assert!(snapshot.cells().any(|cell| cell.text == "e\u{301}"));
            }
            if name == "sparse" {
                assert_eq!(
                    snapshot.rows[19].cells[7].text,
                    if iteration % 2 == 0 { "A" } else { "B" }
                );
            }
            if name == "full" {
                assert!(snapshot.cells().all(|cell| cell.text
                    == if iteration % 2 == 0 { "x" } else { "y" }));
            }
            previous = black_box(snapshot);
        }
        let processing_total: u128 = processing.iter().sum();
        combined.sort_unstable();
        processing.sort_unstable();
        snapshots.sort_unstable();
        println!(
            "engine=ghostty revision={} fixture={name} bytes={} iterations=200 process_ns_p50={} process_ns_p95={} snapshot_ns_p50={} snapshot_ns_p95={} extracted_rows={extracted} allocated_rows={allocated} reused_rows={reused} history={} combined_ns_p50={} combined_ns_p95={} process_bytes_per_second={}",
            crate::GHOSTTY_REVISION,
            bytes / 200,
            processing[100],
            processing[190],
            snapshots[100],
            snapshots[190],
            previous.history_size,
            combined[100],
            combined[190],
            bytes as u128 * 1_000_000_000 / processing_total
        );
    }
}

#[test]
#[ignore = "release benchmark; run through mise run bench:engine"]
fn engine_benchmark_resize() {
    let cell = CellSize {
        width: 8,
        height: 16,
    };
    let mut engine = TerminalEngine::new(
        TerminalId::new(1),
        GridSize::clamped(120, 40),
        cell,
        TerminalPresentation::default(),
    )
    .unwrap();
    engine
        .process(("reflow-marker ".to_owned() + &"x".repeat(180)).as_bytes())
        .unwrap();
    engine.snapshot().unwrap();
    let mut resizing = Vec::new();
    let mut snapshots = Vec::new();
    for iteration in 0..200 {
        let size =
            GridSize::clamped(if iteration % 2 == 0 { 100 } else { 120 }, 40);
        let started = Instant::now();
        engine.resize(size, cell).unwrap();
        resizing.push(started.elapsed().as_nanos());
        let started = Instant::now();
        let snapshot = engine.snapshot().unwrap();
        snapshots.push(started.elapsed().as_nanos());
        assert_eq!(snapshot.size, size);
        let text: String =
            snapshot.cells().map(|cell| cell.text.as_str()).collect();
        assert!(text.contains("reflow-marker"));
        assert_eq!(text.matches('x').count(), 180);
        black_box(snapshot);
    }
    resizing.sort_unstable();
    snapshots.sort_unstable();
    println!(
        "engine=ghostty fixture=resize-reflow iterations=200 columns=100/120 rows=40 resize_ns_p50={} resize_ns_p95={} snapshot_ns_p50={} snapshot_ns_p95={}",
        resizing[100], resizing[190], snapshots[100], snapshots[190]
    );
}
