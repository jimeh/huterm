use super::*;
use huterm_protocol::{LinkLookup, MousePosition};
use std::time::Instant;

// Enough output lines to saturate the 16 MiB scrollback budget.
const FULL_HISTORY_LINES: usize = 100_000;

struct Fixture {
    name: &'static str,
    output: String,
    destination: Option<String>,
    scroll: ScrollCommand,
    point: MousePosition,
    /// Filler lines written before the fixture output, one run each.
    history: &'static [usize],
}

impl Fixture {
    /// Starts at the top of a scrollback containing only this output.
    fn top(name: &'static str, output: String, destination: &str) -> Self {
        Self {
            name,
            output,
            destination: (!destination.is_empty())
                .then(|| destination.to_owned()),
            scroll: ScrollCommand::Absolute(usize::MAX),
            point: MousePosition { row: 0, column: 3 },
            history: &[0],
        }
    }
}

fn fixtures() -> Vec<Fixture> {
    let long_url = format!("https://example.test/{}", "http".repeat(3000));
    let punctuation = format!("https://example.test/{}", ")".repeat(12000));
    let explicit = format!("https://example.test/{}", "a".repeat(1900));
    // Fills `rows` complete 120-column rows.
    let url_rows = |rows: usize| {
        format!("https://example.test/{}", "a".repeat(rows * 120 - 21))
    };
    let bottom = |output: String| format!("{}{output}", "\r\n".repeat(39));
    vec![
        Fixture::top(
            "short",
            "https://example.test/path".to_owned(),
            "https://example.test/path",
        ),
        Fixture::top("wrapped-prefixes", long_url.clone(), &long_url),
        Fixture::top(
            "unmatched-punctuation",
            punctuation,
            "https://example.test/",
        ),
        Fixture::top(
            "long-OSC8-destination",
            format!(
                "\x1b]8;;{explicit}\x1b\\{}\x1b]8;;\x1b\\",
                "label".repeat(40)
            ),
            &explicit,
        ),
        Fixture::top(
            "long-OSC8-label",
            format!(
                "\x1b]8;;https://example.test/\x1b\\{}\x1b]8;;\x1b\\",
                "label".repeat(2400)
            ),
            "https://example.test/",
        ),
        Fixture::top(
            "scan-limit",
            format!("https://example.test/{}", "x".repeat(20000)),
            "",
        ),
        // The rest start on the bottom row, whatever the history, and run
        // with empty and full scrollback so their costs can be compared.
        Fixture {
            name: "viewport-url",
            output: bottom(url_rows(20)),
            destination: Some(url_rows(20)),
            scroll: ScrollCommand::Live,
            point: MousePosition { row: 39, column: 3 },
            history: &[0, FULL_HISTORY_LINES],
        },
        Fixture {
            name: "prefix-above-viewport",
            output: bottom(url_rows(60)),
            destination: Some(url_rows(60)),
            scroll: ScrollCommand::Live,
            point: MousePosition { row: 0, column: 3 },
            history: &[0, FULL_HISTORY_LINES],
        },
        Fixture {
            name: "long-line-short-url",
            // 99 rows of words, 60 of them above the viewport.
            output: bottom(format!(
                "{}https://example.test/path",
                "word ".repeat(2376)
            )),
            destination: Some("https://example.test/path".to_owned()),
            scroll: ScrollCommand::Live,
            point: MousePosition { row: 39, column: 3 },
            history: &[0, FULL_HISTORY_LINES],
        },
        Fixture {
            name: "top-row-url",
            // Checking the unwrapped row above the viewport reads history.
            output: bottom(format!(
                "https://example.test/path{}",
                "\r\nline".repeat(39)
            )),
            destination: Some("https://example.test/path".to_owned()),
            scroll: ScrollCommand::Live,
            point: MousePosition { row: 0, column: 3 },
            history: &[0, FULL_HISTORY_LINES],
        },
        Fixture {
            name: "scan-limit-above-viewport",
            // 200 rows without a delimiter, 160 of them above the viewport.
            output: bottom("x".repeat(200 * 120)),
            destination: None,
            scroll: ScrollCommand::Live,
            point: MousePosition { row: 0, column: 3 },
            history: &[0, FULL_HISTORY_LINES],
        },
        Fixture {
            name: "scrolled-history",
            output: bottom(format!(
                "{}{}",
                url_rows(20),
                "\r\nline".repeat(2000)
            )),
            destination: Some(url_rows(20)),
            scroll: ScrollCommand::Absolute(2000),
            point: MousePosition { row: 39, column: 3 },
            history: &[0, FULL_HISTORY_LINES],
        },
    ]
}

#[test]
#[ignore = "release benchmark; use mise run bench:links"]
fn bounded_link_lookup_elapsed_time() {
    for fixture in fixtures() {
        for &history in fixture.history {
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
            engine
                .process("line\r\n".repeat(history).as_bytes())
                .unwrap();
            engine.process(fixture.output.as_bytes()).unwrap();
            engine.scroll(fixture.scroll).unwrap();
            let snapshot = engine.snapshot().unwrap();
            let mut samples = Vec::new();
            for _ in 0..100 {
                let started = Instant::now();
                let lookup = engine.lookup_link(&snapshot, fixture.point);
                samples.push(started.elapsed().as_micros());
                if let Some(destination) = &fixture.destination {
                    let LinkLookup::Match(link) = lookup else {
                        panic!("{name}: {lookup:?}")
                    };
                    assert_eq!(&link.destination, destination, "{name}");
                } else {
                    assert_eq!(lookup, LinkLookup::ScanLimit, "{name}");
                }
            }
            samples.sort_unstable();
            assert!(
                samples[95] <= 5000,
                "{name} p95 exceeded 5 ms: {} us",
                samples[95]
            );
            println!(
                "link-benchmark engine=ghostty fixture={name} history_rows={} samples=100 p50_us={} p95_us={} max_us={}",
                snapshot.history_size, samples[50], samples[95], samples[99]
            );
        }
    }
}
