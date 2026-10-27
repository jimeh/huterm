use super::*;
use huterm_protocol::{LinkLookup, MousePosition};
use std::time::Instant;

struct Fixture {
    name: &'static str,
    output: String,
    destination: Option<String>,
    scroll: ScrollCommand,
    point: MousePosition,
    /// Runs again after saturating history at each benchmark history size.
    saturate: bool,
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
            saturate: false,
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
            saturate: true,
        },
        Fixture {
            name: "prefix-above-viewport",
            output: bottom(url_rows(60)),
            destination: Some(url_rows(60)),
            scroll: ScrollCommand::Live,
            point: MousePosition { row: 0, column: 3 },
            saturate: true,
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
            saturate: true,
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
            saturate: true,
        },
        Fixture {
            name: "scan-limit-above-viewport",
            // 200 rows without a delimiter, 160 of them above the viewport.
            output: bottom("x".repeat(200 * 120)),
            destination: None,
            scroll: ScrollCommand::Live,
            point: MousePosition { row: 0, column: 3 },
            saturate: true,
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
            saturate: true,
        },
    ]
}

#[test]
#[ignore = "release benchmark; use mise run bench:links"]
fn bounded_link_lookup_elapsed_time() {
    let sizes = benchmark::history_sizes();
    for fixture in fixtures() {
        // Empty history first, then history saturated at each size.
        let runs = std::iter::once((ScrollbackLimit::MAX.rows(), 0)).chain(
            sizes
                .iter()
                .filter(|_| fixture.saturate)
                .map(|&rows| (rows, benchmark::saturating_lines(rows))),
        );
        for (limit, lines) in runs {
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
            engine.set_history_rows(limit).unwrap();
            engine.process("line\r\n".repeat(lines).as_bytes()).unwrap();
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
            // Sizes above the configurable maximum are measured, not gated.
            let gated = limit <= ScrollbackLimit::MAX.rows();
            assert!(
                !gated || samples[95] <= 5000,
                "{name} p95 exceeded 5 ms: {} us",
                samples[95]
            );
            println!(
                "link-benchmark engine=ghostty fixture={name} history_rows={} gated={gated} samples=100 p50_us={} p95_us={} max_us={}",
                snapshot.history_size, samples[50], samples[95], samples[99]
            );
        }
    }
}
