mod ghostty;
mod links;

use crate::host_effects::HostEffectSink;
use crate::terminal::RuntimeError;
use huterm_protocol::{
    BufferRange, CellSize, GridSize, ScrollCommand, TerminalDirectory,
    TerminalId, TerminalModes, TerminalPresentation, TerminalSnapshot,
};

const METADATA_BYTE_LIMIT: usize = 4096;

#[derive(Debug, Eq, PartialEq)]
enum DirectoryUpdate {
    Ignore,
    Clear,
    Set(TerminalDirectory),
}

#[derive(Debug)]
pub(crate) enum EngineEffect {
    PtyWrite(Vec<u8>),
    Title(String),
    Directory(Option<TerminalDirectory>),
    Bell,
}

fn normalize_directory(
    reported: &str,
    server_hostname: Option<&str>,
) -> DirectoryUpdate {
    if reported.len() > METADATA_BYTE_LIMIT || reported.contains('\0') {
        return DirectoryUpdate::Ignore;
    }
    if reported.is_empty() {
        return DirectoryUpdate::Clear;
    }
    if reported.starts_with('/') {
        return DirectoryUpdate::Set(TerminalDirectory::new(
            None,
            reported.to_owned(),
            false,
        ));
    }
    let Ok(uri) = url::Url::parse(reported) else {
        return DirectoryUpdate::Ignore;
    };
    if uri.scheme() != "file"
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.port().is_some()
        || uri.query().is_some()
        || uri.fragment().is_some()
    {
        return DirectoryUpdate::Ignore;
    }
    let Some(path) = percent_decode(uri.path()) else {
        return DirectoryUpdate::Ignore;
    };
    if !path.starts_with('/') || path.contains('\0') {
        return DirectoryUpdate::Ignore;
    }
    let host = uri.host_str().filter(|host| !host.is_empty());
    let local = host.is_none_or(|host| {
        let host = host.strip_suffix('.').unwrap_or(host);
        host.eq_ignore_ascii_case("localhost")
            || server_hostname.is_some_and(|server| {
                host.eq_ignore_ascii_case(
                    server.strip_suffix('.').unwrap_or(server),
                )
            })
    });
    DirectoryUpdate::Set(TerminalDirectory::new(
        host.map(str::to_owned),
        path,
        local,
    ))
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            decoded.push(hex(high)?.checked_mul(16)?.checked_add(hex(low)?)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

const fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Debug)]
pub(crate) struct TerminalEngine {
    inner: Box<ghostty::TerminalEngine>,
}

impl TerminalEngine {
    pub(crate) fn new(
        id: TerminalId,
        size: GridSize,
        cell: CellSize,
        presentation: TerminalPresentation,
    ) -> Result<Self, RuntimeError> {
        ghostty::TerminalEngine::new(id, size, cell, presentation)
            .map(Box::new)
            .map(|inner| Self { inner })
    }
    pub(crate) fn update_presentation(
        &mut self,
        presentation: TerminalPresentation,
    ) -> Result<(), RuntimeError> {
        self.inner.update_presentation(presentation)
    }

    #[cfg(test)]
    pub(crate) fn presentation(&self) -> &TerminalPresentation {
        self.inner.presentation()
    }

    #[cfg(test)]
    pub(crate) fn cell_size(&self) -> CellSize {
        self.inner.cell_size()
    }

    pub(crate) fn set_host_effect_sink(&mut self, sink: HostEffectSink) {
        self.inner.set_host_effect_sink(sink);
    }

    pub(crate) fn requested_viewport(
        &self,
        scroll: Option<ScrollCommand>,
    ) -> Result<huterm_protocol::Viewport, RuntimeError> {
        let (current, history) = self.inner.viewport_state()?;
        let offset = match scroll {
            None => current,
            Some(ScrollCommand::Live) => 0,
            Some(ScrollCommand::Absolute(offset)) => offset,
            Some(ScrollCommand::Relative(delta)) if delta >= 0 => current
                .saturating_add(usize::try_from(delta).unwrap_or(usize::MAX)),
            Some(ScrollCommand::Relative(delta)) => current.saturating_sub(
                usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX),
            ),
        };
        Ok(huterm_protocol::Viewport {
            bottom_offset: offset.min(history),
        })
    }

    pub(crate) fn process(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<EngineEffect>, RuntimeError> {
        self.inner.process(bytes)
    }
    pub(crate) fn resize(
        &mut self,
        size: GridSize,
        cell: CellSize,
    ) -> Result<Vec<EngineEffect>, RuntimeError> {
        self.inner.resize(size, cell)
    }
    pub(crate) fn size(&self) -> GridSize {
        self.inner.size()
    }
    pub(crate) fn modes(&self) -> Result<TerminalModes, RuntimeError> {
        self.inner.modes()
    }
    pub(crate) fn generation(&self) -> u64 {
        self.inner.generation()
    }
    pub(crate) fn scroll(
        &mut self,
        scroll: ScrollCommand,
    ) -> Result<(), RuntimeError> {
        self.inner.scroll(scroll)
    }
    pub(crate) fn snapshot(
        &mut self,
    ) -> Result<TerminalSnapshot, RuntimeError> {
        self.inner.snapshot()
    }

    #[cfg(test)]
    pub(crate) fn last_snapshot_stats(&self) -> ghostty::SnapshotStats {
        self.inner.last_snapshot_stats()
    }
    pub(crate) fn lookup_link(
        &self,
        snapshot: &TerminalSnapshot,
        point: huterm_protocol::MousePosition,
    ) -> huterm_protocol::LinkLookup {
        links::resolve(self.inner.as_ref(), snapshot, point)
    }

    pub(crate) fn extract_text(
        &self,
        generation: u64,
        range: BufferRange,
    ) -> Result<Option<String>, RuntimeError> {
        self.inner.extract_text(generation, range)
    }
}

#[cfg(test)]
mod directory_tests {
    use super::*;

    fn directory(
        value: &str,
        hostname: Option<&str>,
    ) -> Option<TerminalDirectory> {
        match normalize_directory(value, hostname) {
            DirectoryUpdate::Set(directory) => Some(directory),
            DirectoryUpdate::Ignore | DirectoryUpdate::Clear => None,
        }
    }

    #[test]
    fn file_directories_decode_and_classify_exact_local_hosts() {
        for host in
            ["", "localhost", "localhost.", "WORKSTATION", "workstation."]
        {
            let value = format!("file://{host}/tmp/hello%20world/%E2%98%83");
            let directory = directory(&value, Some("workstation")).unwrap();
            assert_eq!(directory.path(), "/tmp/hello world/☃");
            assert!(directory.is_local(), "{host:?}");
        }
        for host in ["workstation.local", "short", "workstation.example"] {
            let value = format!("file://{host}/tmp/project");
            let directory = directory(&value, Some("workstation")).unwrap();
            assert_eq!(directory.host(), Some(host));
            assert!(!directory.is_local(), "{host:?}");
        }
    }

    #[test]
    fn bare_paths_are_display_only_and_empty_clears() {
        let bare = directory("/tmp/project", Some("workstation")).unwrap();
        assert_eq!(bare.host(), None);
        assert_eq!(bare.path(), "/tmp/project");
        assert!(!bare.is_local());
        assert_eq!(
            normalize_directory("", Some("workstation")),
            DirectoryUpdate::Clear
        );
    }

    #[test]
    fn malformed_unsafe_and_oversized_reports_are_rejected() {
        for value in [
            "relative/path",
            "https://localhost/tmp",
            "file://user@localhost/tmp",
            "file://localhost:22/tmp",
            "file://localhost/tmp/%gg",
            "file://localhost/tmp?query",
            "file://localhost/tmp#fragment",
            "file://localhost/tmp%00name",
        ] {
            assert_eq!(
                normalize_directory(value, Some("localhost")),
                DirectoryUpdate::Ignore,
                "{value}"
            );
        }
        assert_eq!(
            normalize_directory(&"x".repeat(METADATA_BYTE_LIMIT + 1), None),
            DirectoryUpdate::Ignore
        );
    }
}

#[cfg(test)]
mod clipboard_tests {
    use super::*;
    use huterm_protocol::HostEffect;

    fn engine_with_recipient()
    -> (TerminalEngine, crate::host_effects::HostEffectRecipient) {
        let terminal_id = TerminalId::new(1);
        let (sink, recipient) = crate::host_effects::test_fixture(terminal_id);
        let mut engine = TerminalEngine::new(
            terminal_id,
            GridSize::clamped(8, 3),
            CellSize {
                width: 8,
                height: 16,
            },
            TerminalPresentation::default(),
        )
        .unwrap();
        engine.set_host_effect_sink(sink);
        (engine, recipient)
    }

    fn drain_text(
        recipient: &crate::host_effects::HostEffectRecipient,
    ) -> Vec<String> {
        let mut text = Vec::new();
        while let Some(pending) = recipient.try_next() {
            match pending.effect() {
                HostEffect::ClipboardWrite(write) => {
                    text.push(write.text().to_owned());
                }
                _ => panic!("unexpected host effect"),
            }
        }
        text
    }

    fn with_engine(
        mut test: impl FnMut(
            &mut TerminalEngine,
            &crate::host_effects::HostEffectRecipient,
        ),
    ) {
        let (mut engine, recipient) = engine_with_recipient();
        test(&mut engine, &recipient);
    }

    #[test]
    fn osc52_writes_preserve_order_unicode_empty_and_nul() {
        with_engine(|engine, recipient| {
            engine
                .process(
                    b"\x1b]52;;dG11eA==\x07\x1b]52;c;\x07\x1b]52;c;YQBi\x1b\\\x1b]52;c;5LiW55WM8J+Zgg==\x07",
                )
                .unwrap();
            assert_eq!(drain_text(recipient), ["tmux", "", "a\0b", "世界🙂"]);
        });
    }

    #[test]
    fn osc52_bel_and_st_complete_at_every_read_split() {
        for terminator in ["\x07", "\x1b\\"] {
            let sequence = format!("\x1b]52;c;5LiW55WM8J+Zgg=={terminator}");
            for split in 0..=sequence.len() {
                with_engine(|engine, recipient| {
                    engine.process(&sequence.as_bytes()[..split]).unwrap();
                    engine.process(&sequence.as_bytes()[split..]).unwrap();
                    assert_eq!(
                        drain_text(recipient),
                        ["世界🙂"],
                        "split={split} terminator={terminator:?}"
                    );
                });
            }
            with_engine(|engine, recipient| {
                for byte in sequence.as_bytes() {
                    engine.process(std::slice::from_ref(byte)).unwrap();
                }
                assert_eq!(
                    drain_text(recipient),
                    ["世界🙂"],
                    "byte-at-a-time terminator={terminator:?}"
                );
            });
        }
    }

    #[test]
    fn osc52_reads_selection_and_invalid_utf8_are_ignored() {
        with_engine(|engine, recipient| {
            let effects = engine
                .process(
                    b"\x1b]52;c;?\x07\x1b]52;p;c2VsZWN0aW9u\x07\x1b]52;c;/w==\x07",
                )
                .unwrap();
            assert!(recipient.try_next().is_none());
            assert!(
                !effects
                    .iter()
                    .any(|effect| matches!(effect, EngineEffect::PtyWrite(_))),
                "Ghostty replied to a clipboard read"
            );
        });
    }

    #[test]
    fn stalled_recipient_keeps_only_the_terminal_effect_budget() {
        with_engine(|engine, recipient| {
            let mut input = Vec::new();
            for _ in 0..100 {
                input.extend_from_slice(b"\x1b]52;c;dmFsdWU=\x07");
            }
            input.extend_from_slice(b"\x1b]2;still-running\x07");
            let effects = engine.process(&input).unwrap();
            assert_eq!(drain_text(recipient).len(), 8);
            assert!(
                effects.iter().any(|effect| {
                    matches!(effect, EngineEffect::Title(title) if title == "still-running")
                }),
                "Ghostty did not process later non-clipboard output"
            );
        });
    }

    #[test]
    fn ghostty_keeps_osc1337_copy_support() {
        let (mut engine, recipient) = engine_with_recipient();
        engine.process(b"\x1b]1337;Copy=:aHV0ZXJt\x07").unwrap();
        assert_eq!(drain_text(&recipient), ["huterm"]);
    }

    #[test]
    fn osc52_preserves_upstream_selector_and_malformed_input_behavior() {
        for (name, sequence, expected) in [
            ("malformed", b"\x1b]52;c;!!!!\x07".as_slice(), None),
            ("extra", b"\x1b]52;c;Zm9v;extra\x07", None),
            ("selector-list", b"\x1b]52;c,p;Zm9v\x07", None),
            ("unknown-selector", b"\x1b]52;x;Zm9v\x07", Some("foo")),
        ] {
            with_engine(|engine, recipient| {
                engine.process(sequence).unwrap();
                assert_eq!(
                    drain_text(recipient),
                    expected.into_iter().collect::<Vec<_>>(),
                    "{name}"
                );
            });
        }
    }

    #[test]
    fn osc52_can_and_sub_finish_the_valid_prefix_once() {
        for cancellation in ['\x18', '\x1a'] {
            with_engine(|engine, recipient| {
                let sequence = format!(
                    "\x1b]52;c;Zm9v{cancellation}\x07\x1b]52;c;YmFy\x07"
                );
                engine.process(sequence.as_bytes()).unwrap();
                assert_eq!(drain_text(recipient), ["foo", "bar"]);
            });
        }
    }
}

#[cfg(test)]
mod benchmark {
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
                    + &(if iteration % 2 == 0 { "x" } else { "y" })
                        .repeat(120 * 40)
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
                format!(
                    "\x1b]2;title {0}\x07\x1b[20;1Htitled {0}",
                    iteration % 2
                )
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
                    assert!(
                        snapshot.cells().any(|cell| cell.text == "e\u{301}")
                    );
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
            .process(
                ("reflow-marker ".to_owned() + &"x".repeat(180)).as_bytes(),
            )
            .unwrap();
        engine.snapshot().unwrap();
        let mut resizing = Vec::new();
        let mut snapshots = Vec::new();
        for iteration in 0..200 {
            let size = GridSize::clamped(
                if iteration % 2 == 0 { 100 } else { 120 },
                40,
            );
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
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use huterm_protocol::{
        BufferPoint, CellColor, CursorShape, MouseEncoding, MouseTracking, Rgb,
    };
    use std::sync::Arc;

    fn with_engine(mut test: impl FnMut(&mut TerminalEngine)) {
        let mut engine = TerminalEngine::new(
            TerminalId::new(1),
            GridSize::clamped(8, 3),
            CellSize {
                width: 8,
                height: 16,
            },
            TerminalPresentation::default(),
        )
        .unwrap();
        test(&mut engine);
    }

    #[test]
    fn ghostty_preserves_style_unicode_defaults_and_dynamic_colors() {
        with_engine(|engine| {
            engine
                .process("A\x1b[31;1;4m界e\u{301}".as_bytes())
                .unwrap();
            let snapshot = engine.snapshot().unwrap();
            let cells = &snapshot.rows[0].cells;
            assert_eq!(cells[0].foreground, CellColor::DefaultForeground);
            assert_eq!(cells[1].text, "界");
            assert!(
                cells[1].style.wide
                    && cells[1].style.bold
                    && cells[1].style.underline
            );
            assert!(cells[2].style.wide_spacer);
            assert_eq!(cells[3].text, "e\u{301}");
            assert_eq!(cells[1].foreground, CellColor::Indexed(1));
            engine.process(b"\x1b]10;#123456\x07").unwrap();
            let changed = engine.snapshot().unwrap();
            assert_eq!(
                changed.rows[0].cells[0].foreground,
                CellColor::Rgb(Rgb {
                    red: 0x12,
                    green: 0x34,
                    blue: 0x56
                })
            );
            assert_eq!(
                snapshot.rows[0].cells[0].foreground,
                CellColor::DefaultForeground
            );
        });
    }

    #[test]
    fn ghostty_preserves_defaults_true_color_indexed_and_reverse_video() {
        with_engine(|engine| {
            engine.process(b"D\x1b[38;2;1;2;3;48;5;4;7mR").unwrap();

            let snapshot = engine.snapshot().unwrap();
            assert_eq!(
                snapshot.rows[0].cells[0].foreground,
                CellColor::DefaultForeground
            );
            assert_eq!(
                snapshot.rows[0].cells[0].background,
                CellColor::DefaultBackground
            );
            assert_eq!(
                snapshot.rows[0].cells[1].foreground,
                CellColor::Indexed(4)
            );
            assert_eq!(
                snapshot.rows[0].cells[1].background,
                CellColor::Rgb(Rgb {
                    red: 1,
                    green: 2,
                    blue: 3,
                })
            );
        });
    }

    #[test]
    fn ghostty_publishes_sparse_rows_metadata_and_skipped_generations() {
        with_engine(|engine| {
            engine.process(b"one\r\ntwo\r\nthree").unwrap();
            let before = engine.snapshot().unwrap();
            engine.process(b"\x1b[2;1HX").unwrap();
            let skipped = engine.snapshot().unwrap();
            engine.process(b"\x1b[2;2HY\x1b[?2004h\x1b[?25l").unwrap();
            let latest = engine.snapshot().unwrap();
            assert!(Arc::ptr_eq(&skipped.rows[0], &latest.rows[0]));
            assert_eq!(before.rows[1].cells[0].text, "t");
            assert_eq!(skipped.rows[1].cells[1].text, "w");
            assert_eq!(latest.rows[1].cells[0].text, "X");
            assert_eq!(latest.rows[1].cells[1].text, "Y");
            assert!(latest.modes.bracketed_paste);
            assert!(
                latest
                    .cursor
                    .is_none_or(|cursor| cursor.shape == CursorShape::Hidden)
            );
        });
    }

    #[test]
    fn ghostty_preserves_shared_scrollback_and_generation_checked_selection() {
        with_engine(|engine| {
            engine
                .process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive")
                .unwrap();
            let generation = engine.generation();
            engine.scroll(ScrollCommand::Absolute(1)).unwrap();
            let before = engine.snapshot().unwrap();
            let visible: Vec<String> = before
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
                .collect();
            assert_eq!(visible, ["two", "three", "four"]);
            assert_eq!(engine.generation(), generation);
            assert!(before.cursor.is_none());
            engine.scroll(ScrollCommand::Absolute(usize::MAX)).unwrap();
            let clamped = engine.snapshot().unwrap();
            assert_eq!(clamped.viewport.bottom_offset, clamped.history_size);
            assert!(clamped.cursor.is_none());
            assert_eq!(engine.generation(), generation);
            engine.scroll(ScrollCommand::Absolute(1)).unwrap();
            let range = BufferRange::ordered(
                BufferPoint {
                    rows_from_live_bottom: 3,
                    column: 0,
                },
                BufferPoint {
                    rows_from_live_bottom: 2,
                    column: 4,
                },
            );
            assert_eq!(
                engine.extract_text(generation, range).unwrap().as_deref(),
                Some("two\nthree")
            );
            assert_eq!(
                engine
                    .extract_text(
                        generation,
                        BufferRange {
                            start: range.end,
                            end: range.start
                        }
                    )
                    .unwrap(),
                None
            );
            assert_eq!(
                engine
                    .extract_text(
                        generation,
                        BufferRange::ordered(
                            BufferPoint {
                                rows_from_live_bottom: usize::MAX,
                                column: 0,
                            },
                            range.end,
                        )
                    )
                    .unwrap(),
                None
            );
            engine.process(b"\r\nsix").unwrap();
            assert_eq!(engine.snapshot().unwrap().rows[0], before.rows[0]);
            assert_eq!(engine.extract_text(generation, range).unwrap(), None);
            engine.scroll(ScrollCommand::Live).unwrap();
            assert_eq!(engine.snapshot().unwrap().viewport.bottom_offset, 0);
            engine.process(b"\x1b[?1049hALT").unwrap();
            assert!(engine.snapshot().unwrap().modes.alternate_screen);
            engine.process(b"\x1b[?1049l").unwrap();
            assert!(!engine.snapshot().unwrap().modes.alternate_screen);
        });
    }

    #[test]
    fn relative_scroll_expectation_uses_runtime_state_before_the_command() {
        with_engine(|engine| {
            engine
                .process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive")
                .unwrap();
            engine.scroll(ScrollCommand::Absolute(1)).unwrap();
            let client_offset =
                engine.snapshot().unwrap().viewport.bottom_offset;
            engine.process(b"\r\nsix").unwrap();
            let command = ScrollCommand::Relative(1);
            let expected = engine.requested_viewport(Some(command)).unwrap();
            assert_eq!(expected.bottom_offset, client_offset + 2);
            engine.scroll(command).unwrap();
            assert_eq!(engine.snapshot().unwrap().viewport, expected);
            let command = ScrollCommand::Absolute(1);
            assert_eq!(
                engine
                    .requested_viewport(Some(command))
                    .unwrap()
                    .bottom_offset,
                1
            );
            engine.scroll(command).unwrap();
            assert_eq!(engine.snapshot().unwrap().viewport.bottom_offset, 1);
        });
    }

    #[test]
    fn ghostty_resizes_reflows_and_preserves_soft_wrapped_selection() {
        with_engine(|engine| {
            engine.process("abcdef界e\u{301}Z".as_bytes()).unwrap();
            let generation = engine.generation();
            let range = BufferRange::ordered(
                BufferPoint {
                    rows_from_live_bottom: 2,
                    column: 0,
                },
                BufferPoint {
                    rows_from_live_bottom: 1,
                    column: 1,
                },
            );
            assert_eq!(
                engine.extract_text(generation, range).unwrap().as_deref(),
                Some("abcdef界e\u{301}Z")
            );
            engine
                .resize(
                    GridSize::clamped(12, 4),
                    CellSize {
                        width: 9,
                        height: 17,
                    },
                )
                .unwrap();
            let snapshot = engine.snapshot().unwrap();
            assert_eq!(snapshot.generation, generation + 1);
            assert_eq!(snapshot.size, GridSize::clamped(12, 4));
            assert_eq!(snapshot.rows.len(), 4);
            assert!(snapshot.rows.iter().all(|row| row.cells.len() == 12));
            assert_eq!(engine.extract_text(generation, range).unwrap(), None);
        });
    }

    #[test]
    fn ghostty_reports_mouse_mode_transitions_without_probe_side_effects() {
        with_engine(|engine| {
            engine
                .resize(
                    GridSize::clamped(1, 1),
                    CellSize {
                        width: 1,
                        height: 1,
                    },
                )
                .unwrap();
            for (sequence, tracking, encoding) in [
                ("\x1b[?1006h", MouseTracking::Disabled, MouseEncoding::Sgr),
                ("\x1b[?1000h", MouseTracking::Buttons, MouseEncoding::Sgr),
                (
                    "\x1b[?1002h",
                    MouseTracking::ButtonMotion,
                    MouseEncoding::Sgr,
                ),
                ("\x1b[?1003h", MouseTracking::AllMotion, MouseEncoding::Sgr),
                ("\x1b[?1000l", MouseTracking::Buttons, MouseEncoding::Sgr),
                ("\x1b[?1002l", MouseTracking::Buttons, MouseEncoding::Sgr),
                ("\x1b[?1005h", MouseTracking::Buttons, MouseEncoding::Utf8),
                ("\x1b[?1006l", MouseTracking::Buttons, MouseEncoding::Legacy),
                ("\x1b[?1005l", MouseTracking::Buttons, MouseEncoding::Legacy),
                (
                    "\x1b[?1003l",
                    MouseTracking::Disabled,
                    MouseEncoding::Legacy,
                ),
                (
                    "\x1b[?1002h\x1b[?1006h\x1bc",
                    MouseTracking::Disabled,
                    MouseEncoding::Legacy,
                ),
            ] {
                let generation = engine.generation();
                engine.process(sequence.as_bytes()).unwrap();
                let modes = engine.modes().unwrap();
                assert_eq!(
                    (modes.mouse_tracking, modes.mouse_encoding),
                    (tracking, encoding),
                    "{sequence:?}"
                );
                assert_eq!(engine.generation(), generation + 1, "{sequence:?}");
                let snapshot = engine.snapshot().unwrap();
                assert_eq!(snapshot.generation, generation + 1, "{sequence:?}");
                assert_eq!(snapshot.modes, modes, "{sequence:?}");
                assert_eq!(engine.generation(), generation + 1, "{sequence:?}");
            }
        });
    }

    #[test]
    fn cached_modes_follow_each_processed_mode_change() {
        with_engine(|engine| {
            assert_eq!(engine.modes().unwrap(), TerminalModes::default());
            for (sequence, application_cursor, alternate_screen, paste) in [
                ("\x1b[?1h", true, false, false),
                ("\x1b[?1049h", true, true, false),
                ("\x1b[?2004h", true, true, true),
                ("\x1b[?1l\x1b[?1049l\x1b[?2004l", false, false, false),
            ] {
                engine.process(sequence.as_bytes()).unwrap();
                let modes = engine.modes().unwrap();
                assert_eq!(
                    (
                        modes.application_cursor,
                        modes.alternate_screen,
                        modes.bracketed_paste,
                    ),
                    (application_cursor, alternate_screen, paste),
                    "{sequence:?}"
                );
                assert_eq!(engine.modes().unwrap(), modes, "{sequence:?}");
            }
        });
    }

    #[test]
    fn ghostty_answers_primary_device_attributes_conservatively() {
        with_engine(|engine| {
            let expected = b"\x1b[?62;22c".as_slice();
            let effects = engine.process(b"\x1b[c").unwrap();
            let replies: Vec<_> = effects
                .iter()
                .filter_map(|effect| match effect {
                    EngineEffect::PtyWrite(bytes) => Some(bytes.as_slice()),
                    _ => None,
                })
                .collect();
            assert_eq!(replies, [expected]);
        });
    }

    #[test]
    fn ghostty_does_not_advertise_unsupported_kitty_keyboard_input() {
        with_engine(|engine| {
            let effects = engine.process(b"\x1b[?u\x1b[>31u\x1b[?u").unwrap();
            assert!(!effects.iter().any(|effect| matches!(effect, EngineEffect::PtyWrite(bytes) if bytes.ends_with(b"u"))));
        });
    }

    #[test]
    fn ghostty_reports_modes_and_owned_effects() {
        with_engine(|engine| {
            let effects = engine.process(b"\x1b]2;contract\x07\x07\x1b[6n\x1b[?1002h\x1b[?1006h\x1b[?1004h").unwrap();
            assert!(
                effects
                    .iter()
                    .any(|effect| matches!(effect, EngineEffect::Bell))
            );
            assert!(effects.iter().any(|effect| matches!(effect, EngineEffect::Title(title) if title == "contract")));
            assert!(effects.iter().any(|effect| matches!(effect, EngineEffect::PtyWrite(bytes) if bytes == b"\x1b[1;1R")));
            let modes = engine.modes().unwrap();
            assert_eq!(modes.mouse_tracking, MouseTracking::ButtonMotion);
            assert_eq!(modes.mouse_encoding, MouseEncoding::Sgr);
            assert!(modes.focus_reporting);
        });
    }
}

#[cfg(test)]
mod link_contract_tests {
    use super::*;
    use huterm_protocol::{
        LinkLookup, LinkSource, MousePosition, TerminalLink,
    };

    fn with_engine(mut test: impl FnMut(&mut TerminalEngine)) {
        let mut engine = TerminalEngine::new(
            TerminalId::new(1),
            GridSize::clamped(8, 3),
            CellSize {
                width: 8,
                height: 16,
            },
            TerminalPresentation::default(),
        )
        .unwrap();
        test(&mut engine);
    }
    #[test]
    fn osc8_native_destination_boundaries_never_return_truncated_targets() {
        with_engine(|engine| {
            for length in [2046, 2047, 8021] {
                engine.process(b"\x1bc").unwrap();
                let destination =
                    format!("https://example.test/{}", "a".repeat(length - 21));
                assert_eq!(destination.len(), length);
                engine
                    .process(
                        format!(
                            "\x1b]8;;{destination}\x1b\\label\x1b]8;;\x1b\\"
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                let snapshot = engine.snapshot().unwrap();
                let result = engine.lookup_link(
                    &snapshot,
                    MousePosition { row: 0, column: 1 },
                );
                if length > 2046 {
                    assert_eq!(result, LinkLookup::NoMatch);
                } else {
                    let LinkLookup::Match(link) = result else {
                        panic!("length {length}: {result:?}")
                    };
                    assert_eq!(link.destination, destination);
                }
            }
        });
    }
    fn matched(
        engine: &mut TerminalEngine,
        row: u32,
        column: u32,
    ) -> TerminalLink {
        let snapshot = engine.snapshot().unwrap();
        let result =
            engine.lookup_link(&snapshot, MousePosition { row, column });
        let LinkLookup::Match(link) = result else {
            panic!("expected link, got {result:?}; snapshot={snapshot:?}")
        };
        link
    }

    #[test]
    fn link_lookup_resolves_offscreen_prefix_and_suffix_without_scrolling() {
        with_engine(|engine| {
            let url = "https://example.test/abcdefghijklmnopqrstuvxyz";
            engine.process(url.as_bytes()).unwrap();
            let snapshot = engine.snapshot().unwrap();
            assert!(snapshot.history_size > 0);
            assert_eq!(matched(engine, 2, 0).destination, url);
            engine.scroll(ScrollCommand::Absolute(3)).unwrap();
            let before = engine.snapshot().unwrap();
            assert_eq!(matched(engine, 0, 0).destination, url);
            let after = engine.snapshot().unwrap();
            assert_eq!(after.viewport, before.viewport);
            assert_eq!(after.generation, before.generation);
            assert!(
                after
                    .rows
                    .iter()
                    .zip(&before.rows)
                    .all(|(a, b)| std::sync::Arc::ptr_eq(a, b))
            );
        });
    }

    #[test]
    fn link_lookup_drops_evicted_prefix_and_preserves_future_damage() {
        with_engine(|engine| {
            engine
                .process(b"https://example.test/abcdefghijklmnopqrstuvwxyz")
                .unwrap();
            let before = engine.snapshot().unwrap();
            assert!(before.history_size > 0);
            assert!(matches!(
                engine
                    .lookup_link(&before, MousePosition { row: 0, column: 1 }),
                LinkLookup::Match(_)
            ));
            engine.process(b"\x1b[3J").unwrap();
            let cleared = engine.snapshot().unwrap();
            assert_eq!(cleared.history_size, 0);
            assert_eq!(
                engine
                    .lookup_link(&cleared, MousePosition { row: 0, column: 1 }),
                LinkLookup::NoMatch
            );
            engine.process(b"\x1b[HZ").unwrap();
            let after = engine.snapshot().unwrap();
            assert_eq!(after.rows[0].cells[0].text, "Z");
            assert_ne!(cleared.rows[0].cells[0].text, "Z");
            assert!(!std::sync::Arc::ptr_eq(&cleared.rows[0], &after.rows[0]));
        });
    }

    #[test]
    fn link_lookup_explicit_target_precedes_label_and_preserves_wide_spacer() {
        with_engine(|engine| {
            engine.process(b"\x1b]8;;https://destination.test/path\x1b\\http://x\x1b]8;;\x1b\\").unwrap();
            let link = matched(engine, 0, 0);
            assert_eq!(link.destination, "https://destination.test/path");
            assert_eq!(link.source, LinkSource::Osc8);
            engine
                .process(
                    "\x1bc\x1b]8;;https://other.test\x1b\\界\x1b]8;;\x1b\\"
                        .as_bytes(),
                )
                .unwrap();
            assert_eq!(matched(engine, 0, 1).destination, "https://other.test");
            engine
                .process("\x1b[2J\x1b[Hhttps://x/界".as_bytes())
                .unwrap();
            let link = matched(engine, 1, 2);
            assert_eq!(link.destination, "https://x/界");
            assert!(link.cells.iter().any(
                |cell| cell.position == MousePosition { row: 1, column: 2 }
            ));
        });
    }

    #[test]
    fn link_lookup_hard_breaks_controls_out_of_grid_and_scan_limits_are_not_targets()
     {
        with_engine(|engine| {
            engine.process(b"https://\r\nexample.test").unwrap();
            let snapshot = engine.snapshot().unwrap();
            assert_eq!(
                engine.lookup_link(
                    &snapshot,
                    MousePosition { row: 0, column: 2 }
                ),
                LinkLookup::NoMatch
            );
            assert_eq!(
                engine.lookup_link(
                    &snapshot,
                    MousePosition { row: 0, column: 8 }
                ),
                LinkLookup::NoMatch
            );
            engine
                .process(
                    format!("\x1bchttps://x/{}", "a".repeat(2048)).as_bytes(),
                )
                .unwrap();
            let snapshot = engine.snapshot().unwrap();
            assert_eq!(
                engine.lookup_link(
                    &snapshot,
                    MousePosition { row: 2, column: 0 }
                ),
                LinkLookup::ScanLimit
            );
            engine
                .process(
                    b"\x1bc\x1b]8;;file:///tmp/a\x1b\\https://x\x1b]8;;\x1b\\",
                )
                .unwrap();
            let snapshot = engine.snapshot().unwrap();
            assert_eq!(
                engine.lookup_link(
                    &snapshot,
                    MousePosition { row: 0, column: 0 }
                ),
                LinkLookup::NoMatch
            );
        });
    }

    #[test]
    fn link_lookup_observes_offscreen_target_mutation_resize_and_alternate_screen()
     {
        with_engine(|engine| {
            engine
                .process(b"\x1b]8;;https://first.test\x1b\\label\x1b]8;;\x1b\\")
                .unwrap();
            let before = engine.snapshot().unwrap();
            let first = matched(engine, 0, 0);
            engine.process(b"\x1b[H\x1b]8;;https://other.test\x1b\\label\x1b]8;;\x1b\\").unwrap();
            assert_ne!(matched(engine, 0, 0).destination, first.destination);
            assert_eq!(before.rows[0].cells[0].text, "l");
            engine
                .resize(
                    GridSize::clamped(12, 4),
                    CellSize {
                        width: 8,
                        height: 16,
                    },
                )
                .unwrap();
            assert_eq!(matched(engine, 0, 0).destination, "https://other.test");
            engine.process(b"\x1b[?1049h").unwrap();
            let snapshot = engine.snapshot().unwrap();
            assert_eq!(
                engine.lookup_link(
                    &snapshot,
                    MousePosition { row: 0, column: 0 }
                ),
                LinkLookup::NoMatch
            );
            engine.process(b"\x1b[?1049l").unwrap();
            assert_eq!(matched(engine, 0, 0).destination, "https://other.test");
        });
    }
}

#[cfg(test)]
mod link_benchmark {
    use super::*;
    use huterm_protocol::{LinkLookup, MousePosition};
    use std::time::Instant;

    #[test]
    #[ignore = "release benchmark; use mise run bench:links"]
    fn bounded_link_lookup_elapsed_time() {
        let long_url = format!("https://example.test/{}", "http".repeat(3000));
        let punctuation = format!("https://example.test/{}", ")".repeat(12000));
        let explicit = format!("https://example.test/{}", "a".repeat(1900));
        let fixtures = [
            (
                "short",
                "https://example.test/path".to_owned(),
                "https://example.test/path".to_owned(),
                false,
            ),
            ("wrapped-prefixes", long_url.clone(), long_url, false),
            (
                "unmatched-punctuation",
                punctuation,
                "https://example.test/".to_owned(),
                false,
            ),
            (
                "long-OSC8-destination",
                format!(
                    "\x1b]8;;{explicit}\x1b\\{}\x1b]8;;\x1b\\",
                    "label".repeat(40)
                ),
                explicit,
                false,
            ),
            (
                "long-OSC8-label",
                format!(
                    "\x1b]8;;https://example.test/\x1b\\{}\x1b]8;;\x1b\\",
                    "label".repeat(2400)
                ),
                "https://example.test/".to_owned(),
                false,
            ),
            (
                "scan-limit",
                format!("https://example.test/{}", "x".repeat(20000)),
                String::new(),
                true,
            ),
        ];
        for (name, output, destination, limited) in fixtures {
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
            engine.process(output.as_bytes()).unwrap();
            engine.scroll(ScrollCommand::Absolute(usize::MAX)).unwrap();
            let snapshot = engine.snapshot().unwrap();
            let mut samples = Vec::new();
            for _ in 0..100 {
                let started = Instant::now();
                let lookup = engine.lookup_link(
                    &snapshot,
                    MousePosition { row: 0, column: 3 },
                );
                samples.push(started.elapsed().as_micros());
                if limited {
                    assert_eq!(lookup, LinkLookup::ScanLimit, "{name}");
                } else {
                    let LinkLookup::Match(link) = lookup else {
                        panic!("{name}: {lookup:?}")
                    };
                    assert_eq!(link.destination, destination, "{name}");
                }
            }
            samples.sort_unstable();
            assert!(
                samples[95] <= 5000,
                "{name} p95 exceeded 5 ms: {} us",
                samples[95]
            );
            println!(
                "link-benchmark engine=ghostty fixture={name} samples=100 p50_us={} p95_us={} max_us={}",
                samples[50], samples[95], samples[99]
            );
        }
    }
}
