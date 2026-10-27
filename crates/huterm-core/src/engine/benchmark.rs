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
    // Alternating explicit IDs kill the old link below a live one, as
    // implicit IDs do. Without Huterm's `ref-counted-set-dead-id-reuse`
    // Ghostty patch, both fixtures redraw every row.
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

/// Writes history until the row limit stops `history_size` growing.
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
    panic!("scrollback never reached its row limit");
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

/// The smallest benchmark history size. Smaller limits evict the output of
/// fixtures that place thousands of rows after it, such as the link
/// benchmark's `scrolled-history` and `scan-limit-mid-history`.
const MIN_HISTORY_ROWS: u32 = 10_000;

/// History sizes, in rows, at which benchmarks saturate scrollback: the
/// comma-separated `HUTERM_BENCH_HISTORY_ROWS`, or the configurable maximum.
pub(super) fn history_sizes() -> Vec<u32> {
    std::env::var("HUTERM_BENCH_HISTORY_ROWS").map_or_else(
        |_| vec![ScrollbackLimit::MAX.rows()],
        |sizes| {
            sizes
                .split(',')
                .map(|size| match size.trim().parse() {
                    Ok(rows) if rows >= MIN_HISTORY_ROWS => rows,
                    _ => panic!(
                        "HUTERM_BENCH_HISTORY_ROWS entry {size:?} must be a \
                         row count of at least {MIN_HISTORY_ROWS}"
                    ),
                })
                .collect()
        },
    )
}

/// Rows of output that saturate a history limit of `rows`. Ghostty prunes
/// whole pages, so history settles less than a page below the limit.
pub(super) const fn saturating_lines(rows: u32) -> usize {
    rows as usize / 5 * 6 + 1000
}

/// The benchmark process's peak resident set size, in KiB.
#[cfg(target_os = "linux")]
fn peak_rss_kib() -> i64 {
    // `getrusage` keeps the peak of the image that ran before `execve`, the
    // forked `cargo`, which doubled the first run after a build in CI. The
    // memory map's own high-water mark starts at `execve`.
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|value| value.trim().strip_suffix(" kB")?.parse().ok())
        .expect("/proc/self/status reports VmHWM in kB")
}

/// The benchmark process's peak resident set size, in KiB.
#[cfg(target_os = "macos")]
fn peak_rss_kib() -> i64 {
    use nix::sys::resource::{UsageWho, getrusage};
    // macOS reports bytes.
    getrusage(UsageWho::RUSAGE_SELF).unwrap().max_rss() / 1024
}

/// Budget for the median resize of `resize-history` at the configurable
/// maximum. Across 76 runs on CI's Ubuntu runners, the median measured 15.9
/// to 24.9 ms on AMD EPYC 7763, 9V45, and 9V74 and Intel Xeon 8573C and
/// 6973P-C hosts, and 29.6 ms once on a host the job did not identify. CPU
/// model set most of the spread: running after the other CI benchmarks
/// matched standalone runs on the same host. The slowest of 20 resizes
/// reached 36.1 ms, so the gate ignores it. Apple silicon measures 6.7 ms.
/// 45 ms leaves 50% headroom over the slowest median and fails a doubling on
/// hosts above 22.5 ms.
const RESIZE_HISTORY_P50_BUDGET_US: u128 = 45_000;

/// Budget for the benchmark process's peak resident size after
/// `resize-history` at the configurable maximum. CI's Ubuntu runners measured
/// 109,716 to 110,124 KiB across 38 runs and four CPU models, and Apple
/// silicon about 111,000 KiB. 140 MiB leaves 30% headroom for allocator and
/// platform differences.
const RESIZE_HISTORY_PEAK_RSS_BUDGET_KIB: i64 = 140 * 1024;

#[test]
#[ignore = "release benchmark; run through mise run bench:engine"]
fn engine_benchmark_resize_history() {
    let cell = CellSize {
        width: 8,
        height: 16,
    };
    // The process peak only grows, so ascending sizes attribute each peak to
    // the largest history so far.
    let mut sizes = history_sizes();
    sizes.sort_unstable();
    for rows in sizes {
        let mut engine = TerminalEngine::new(
            TerminalId::new(1),
            GridSize::clamped(120, 40),
            cell,
            TerminalPresentation::default(),
        )
        .unwrap();
        engine.set_history_rows(rows).unwrap();
        // 149-column lines fill two rows at both widths, so every reflow
        // rewraps them without changing the row count.
        let lines = saturating_lines(rows).div_ceil(2);
        let mut batch = String::new();
        for line in 0..lines {
            let _ = write!(batch, "{line:08} {}\r\n", "x".repeat(140));
            if line % 10_000 == 9_999 || line + 1 == lines {
                engine.process(batch.as_bytes()).unwrap();
                batch.clear();
            }
        }
        let before = engine.snapshot().unwrap().history_size;
        let mut resizing = Vec::new();
        let mut snapshots = Vec::new();
        let mut after = 0;
        for iteration in 0..20 {
            let size = GridSize::clamped(
                if iteration % 2 == 0 { 100 } else { 120 },
                40,
            );
            let started = Instant::now();
            engine.resize(size, cell).unwrap();
            resizing.push(started.elapsed().as_micros());
            let started = Instant::now();
            let snapshot = engine.snapshot().unwrap();
            snapshots.push(started.elapsed().as_micros());
            assert_eq!(snapshot.size, size);
            after = snapshot.history_size;
            black_box(snapshot);
        }
        resizing.sort_unstable();
        snapshots.sort_unstable();
        let peak_rss = peak_rss_kib();
        // Sizes above the configurable maximum are measured, not gated.
        let gated = rows <= ScrollbackLimit::MAX.rows();
        println!(
            "engine=ghostty fixture=resize-history iterations=20 columns=100/120 rows=40 limit_rows={rows} history_rows_before={before} history_rows_after={after} gated={gated} resize_us_p50={} resize_us_max={} snapshot_us_p50={} snapshot_us_max={} peak_rss_kib={peak_rss}",
            resizing[10], resizing[19], snapshots[10], snapshots[19]
        );
        assert!(
            !gated || resizing[10] <= RESIZE_HISTORY_P50_BUDGET_US,
            "resize-history p50 exceeded {RESIZE_HISTORY_P50_BUDGET_US} us \
             at {rows} rows: {} us",
            resizing[10]
        );
        assert!(
            !gated || peak_rss <= RESIZE_HISTORY_PEAK_RSS_BUDGET_KIB,
            "resize-history peak RSS exceeded \
             {RESIZE_HISTORY_PEAK_RSS_BUDGET_KIB} KiB at {rows} rows: \
             {peak_rss} KiB"
        );
    }
}
