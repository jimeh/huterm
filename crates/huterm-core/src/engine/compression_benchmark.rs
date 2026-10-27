//! Scrollback compression benchmarks. Each holds up to 400 MiB of history,
//! so `mise run bench:compression` runs every test in its own process; a
//! shared process would inflate the peak that `resize-history` gates.

use super::benchmark::{peak_rss_kib, saturating_lines};
use super::*;
use huterm_ghostty::Compression;
use std::fmt::Write as _;
use std::hint::black_box;
use std::time::Instant;

const COLUMNS: u16 = 120;
const ROWS: u16 = 40;

fn cell() -> CellSize {
    CellSize {
        width: 8,
        height: 16,
    }
}

/// Writes one line of a fixture's output.
type Generator = fn(&mut Lcg, usize) -> String;

/// An engine with `generator`'s output saturating the maximum history.
fn saturated(generator: Generator) -> TerminalEngine {
    let mut engine = TerminalEngine::new(
        TerminalId::new(1),
        GridSize::clamped(COLUMNS, ROWS),
        cell(),
        TerminalPresentation::default(),
    )
    .unwrap();
    engine.set_scrollback(ScrollbackLimit::MAX).unwrap();
    let mut rng = Lcg(0x5eed);
    let mut batch = String::new();
    let lines = saturating_lines(ScrollbackLimit::MAX.rows());
    for line in 0..lines {
        batch.push_str(&generator(&mut rng, line));
        if batch.len() > 1 << 20 || line + 1 == lines {
            engine.process(batch.as_bytes()).unwrap();
            batch.clear();
        }
    }
    engine.snapshot().unwrap();
    engine
}

/// A seeded generator, so every run writes the same history.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn word(&mut self) -> &'static str {
        WORDS[usize::try_from(self.below(WORDS.len() as u64)).unwrap()]
    }
}

const WORDS: &[&str] = &[
    "huterm",
    "core",
    "ghostty",
    "render",
    "snapshot",
    "terminal",
    "runtime",
    "viewport",
    "selection",
    "protocol",
    "workspace",
    "session",
    "pane",
    "window",
    "config",
    "theme",
    "scroll",
    "history",
    "engine",
    "native",
    "build",
    "linker",
    "parser",
    "reflow",
    "cursor",
    "palette",
    "glyph",
];

/// Cargo-style output: colored status words, warnings, source excerpts,
/// and test results.
fn build_log(rng: &mut Lcg, line: usize) -> String {
    match rng.below(10) {
        0..=5 => format!(
            "\x1b[1m\x1b[32m   Compiling\x1b[0m {}-{} v0.{}.{} (/home/dev/src/{}/crates/{}-{})\r\n",
            rng.word(),
            rng.word(),
            rng.below(30),
            rng.below(10),
            rng.word(),
            rng.word(),
            line % 97
        ),
        6 => format!(
            "\x1b[1m\x1b[33mwarning\x1b[0m\x1b[1m: unused variable: `{}_{}`\x1b[0m\r\n",
            rng.word(),
            rng.below(1000)
        ),
        7 => format!(
            "\x1b[1m\x1b[34m  --> \x1b[0msrc/{}/{}.rs:{}:{}\r\n",
            rng.word(),
            rng.word(),
            rng.below(2000),
            rng.below(80)
        ),
        8 => format!(
            "\x1b[1m\x1b[34m{:>4} |\x1b[0m     let {} = {}::{}(&mut {}, {})?;\r\n",
            rng.below(2000),
            rng.word(),
            rng.word(),
            rng.word(),
            rng.word(),
            rng.below(4096)
        ),
        _ => format!(
            "test {}::{}::{}_{} ... \x1b[32mok\x1b[0m\r\n",
            rng.word(),
            rng.word(),
            rng.word(),
            rng.below(100)
        ),
    }
}

/// A 256-color foreground and a truecolor background on every word.
fn colored(rng: &mut Lcg, _: usize) -> String {
    let mut line = String::new();
    let mut width = 0;
    loop {
        let word = rng.word();
        if width + word.len() + 1 > 118 {
            break;
        }
        let _ = write!(
            line,
            "\x1b[38;5;{}m\x1b[48;2;{};{};{}m{word}\x1b[0m ",
            rng.below(256),
            rng.below(64),
            rng.below(64),
            rng.below(64)
        );
        width += word.len() + 1;
    }
    line + "\r\n"
}

/// A distinct truecolor pair on every cell, which grows pages to several
/// times their usual size: the slowest pages to compress.
fn styles_per_cell(rng: &mut Lcg, _: usize) -> String {
    let mut line = String::new();
    for _ in 0..118 {
        let _ = write!(
            line,
            "\x1b[38;2;{};{};{};48;2;{};{};{}mx",
            rng.below(256),
            rng.below(256),
            rng.below(256),
            rng.below(256),
            rng.below(256),
            rng.below(256)
        );
    }
    line + "\x1b[0m\r\n"
}

/// Elapsed time of every step of one pass, in microseconds, sorted.
fn drain(engine: &mut TerminalEngine) -> Vec<u128> {
    let mut steps = Vec::new();
    loop {
        let started = Instant::now();
        let result = engine.compress_step().unwrap();
        steps.push(started.elapsed().as_micros());
        match result {
            Compression::Pending => {}
            Compression::Complete => break,
            Compression::Unsupported => panic!("compression is unsupported"),
        }
        assert!(steps.len() < 1_000_000, "compression never completed");
    }
    steps.sort_unstable();
    steps
}

/// The value at quantile `q` of sorted samples.
fn quantile(sorted: &[u128], q: f64) -> u128 {
    let last = sorted.len() - 1;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        reason = "sample counts are far below 2^52"
    )]
    let index = (last as f64 * q).round() as usize;
    sorted[index]
}

/// Budget for the build-log saving: the fraction of Ghostty's resident page
/// memory a pass releases. Apple silicon and Linux arm64 both measured 89%.
const BUILD_LOG_SAVING_FLOOR_PERCENT: u64 = 70;

/// Budget for the 95th-percentile step on build-log history. Apple silicon
/// measured 214 µs.
const BUILD_LOG_STEP_P95_BUDGET_US: u128 = 2_000;

/// Budget for the 95th-percentile step on styles-per-cell history, whose
/// pages grow to about 2.3 MiB. Apple silicon measured 4.0 ms.
const STYLES_STEP_P95_BUDGET_US: u128 = 25_000;

#[test]
#[ignore = "release benchmark; run through mise run bench:compression"]
fn compression_benchmark_steps() {
    let fixtures: [(&str, Generator, Option<u128>); 3] = [
        ("build-log", build_log, Some(BUILD_LOG_STEP_P95_BUDGET_US)),
        ("colored", colored, None),
        (
            "styles-per-cell",
            styles_per_cell,
            Some(STYLES_STEP_P95_BUDGET_US),
        ),
    ];
    for (name, generator, step_budget) in fixtures {
        let mut engine = saturated(generator);
        let history = engine.snapshot().unwrap().history_size;
        let before = engine.memory_usage().unwrap().primary;
        let started = Instant::now();
        let steps = drain(&mut engine);
        let pass = started.elapsed().as_millis();
        let after = engine.memory_usage().unwrap().primary;
        let saved = 100 - after.resident_bytes * 100 / before.resident_bytes;

        // The first touch of compressed history restores the page shown.
        let mut touches = Vec::new();
        for (point, scroll) in [
            ("back-1000", ScrollCommand::Relative(1_000)),
            ("middle", ScrollCommand::Absolute(history / 2)),
            ("top", ScrollCommand::Absolute(usize::MAX)),
        ] {
            let compressed = engine.memory_usage().unwrap().primary;
            let started = Instant::now();
            engine.scroll(scroll).unwrap();
            black_box(engine.snapshot().unwrap());
            let elapsed = started.elapsed().as_micros();
            let restored = compressed.compressed_pages
                - engine.memory_usage().unwrap().primary.compressed_pages;
            assert!(
                restored > 0,
                "{name}: scrolling to {point} restored nothing"
            );
            touches.push(format!("first_touch_{point}_us={elapsed}"));
        }
        println!(
            "compression-benchmark fixture={name} history_rows={history} pages={} compressed_pages={} resident_before_kib={} resident_after_kib={} saved_percent={saved} steps={} step_us_p50={} step_us_p95={} step_us_max={} pass_ms={pass} {}",
            before.pages,
            after.compressed_pages,
            before.resident_bytes / 1024,
            after.resident_bytes / 1024,
            steps.len(),
            quantile(&steps, 0.5),
            quantile(&steps, 0.95),
            steps.last().unwrap(),
            touches.join(" ")
        );
        if name == "build-log" {
            assert!(
                saved >= BUILD_LOG_SAVING_FLOOR_PERCENT,
                "build-log compression saved {saved}%, below \
                 {BUILD_LOG_SAVING_FLOOR_PERCENT}%"
            );
        }
        if let Some(budget) = step_budget {
            let p95 = quantile(&steps, 0.95);
            assert!(
                p95 <= budget,
                "{name} step p95 exceeded {budget} us: {p95} us"
            );
        }
    }
}

/// Clears the process's peak resident size, so a later read covers only
/// what follows. Linux only; other platforms report the process peak.
#[cfg(target_os = "linux")]
fn reset_peak_rss() {
    std::fs::write("/proc/self/clear_refs", "5")
        .expect("/proc/self/clear_refs accepts 5");
}

#[cfg(not(target_os = "linux"))]
const fn reset_peak_rss() {}

/// Budget for the median resize of build-log history at the maximum, with a
/// pass drained before each. Apple silicon measured 31.9 ms.
const RESIZE_COMPRESSED_P50_BUDGET_US: u128 = 250_000;

/// Budget for the peak resident size during the first resize, measured on
/// Linux from a `VmHWM` reset just before it, so the fill does not count.
/// Linux arm64 measured 89.8 MiB for plain history.
const RESIZE_COMPRESSED_PEAK_RSS_BUDGET_KIB: i64 = 200 * 1024;

#[test]
#[ignore = "release benchmark; run through mise run bench:compression"]
fn compression_benchmark_resize() {
    let mut engine = saturated(build_log);
    let history = engine.snapshot().unwrap().history_size;
    drain(&mut engine);
    reset_peak_rss();
    let started = Instant::now();
    engine.resize(GridSize::clamped(100, ROWS), cell()).unwrap();
    let first = started.elapsed().as_micros();
    let peak = peak_rss_kib();
    let restored = engine.memory_usage().unwrap().primary;
    assert_eq!(restored.compressed_pages, 0, "a width change restores all");
    let mut resizing = Vec::new();
    let mut passes = Vec::new();
    for iteration in 0..20 {
        let started = Instant::now();
        drain(&mut engine);
        passes.push(started.elapsed().as_micros());
        let size = GridSize::clamped(
            if iteration % 2 == 0 { COLUMNS } else { 100 },
            ROWS,
        );
        let started = Instant::now();
        engine.resize(size, cell()).unwrap();
        resizing.push(started.elapsed().as_micros());
        black_box(engine.snapshot().unwrap());
    }
    resizing.sort_unstable();
    passes.sort_unstable();
    let after = engine.snapshot().unwrap().history_size;
    let p50 = quantile(&resizing, 0.5);
    let gated = cfg!(target_os = "linux");
    println!(
        "compression-benchmark fixture=resize-build-log history_rows={history} after_rows={after} first_resize_us={first} resize_us_p50={p50} resize_us_max={} pass_ms_p50={} peak_rss_kib={peak} peak_since_reset={gated}",
        resizing.last().unwrap(),
        quantile(&passes, 0.5) / 1000
    );
    assert!(
        p50 <= RESIZE_COMPRESSED_P50_BUDGET_US,
        "compressed resize p50 exceeded {RESIZE_COMPRESSED_P50_BUDGET_US} \
         us: {p50} us"
    );
    // macOS cannot reset its peak, so there it includes the fill.
    assert!(
        !gated || peak <= RESIZE_COMPRESSED_PEAK_RSS_BUDGET_KIB,
        "peak resident size during a compressed resize exceeded \
         {RESIZE_COMPRESSED_PEAK_RSS_BUDGET_KIB} KiB: {peak} KiB"
    );
}
