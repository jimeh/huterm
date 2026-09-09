mod alacritty;
mod ghostty;
mod links;

use crate::terminal::RuntimeError;
use huterm_protocol::{
    BufferRange, CellSize, GridSize, ScrollCommand, TerminalEngineKind,
    TerminalId, TerminalModes, TerminalSnapshot,
};

#[derive(Debug)]
pub(crate) enum EngineEffect {
    PtyWrite(Vec<u8>),
    Title(String),
    Bell,
}

#[derive(Debug)]
pub(crate) enum TerminalEngine {
    Alacritty(Box<alacritty::TerminalEngine>),
    Ghostty(Box<ghostty::TerminalEngine>),
}

impl TerminalEngine {
    pub(crate) fn new(
        id: TerminalId,
        size: GridSize,
        cell: CellSize,
        kind: TerminalEngineKind,
    ) -> Result<Self, RuntimeError> {
        match kind {
            TerminalEngineKind::Alacritty => Ok(Self::Alacritty(Box::new(
                alacritty::TerminalEngine::new(id, size),
            ))),
            TerminalEngineKind::Ghostty => {
                ghostty::TerminalEngine::new(id, size, cell)
                    .map(Box::new)
                    .map(Self::Ghostty)
            }
        }
    }
    pub(crate) fn requested_viewport(
        &self,
        scroll: Option<ScrollCommand>,
    ) -> Result<huterm_protocol::Viewport, RuntimeError> {
        let (current, history) = match self {
            Self::Alacritty(engine) => engine.viewport_state(),
            Self::Ghostty(engine) => engine.viewport_state()?,
        };
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
        match self {
            Self::Alacritty(engine) => Ok(engine.process(bytes)),
            Self::Ghostty(engine) => engine.process(bytes),
        }
    }
    pub(crate) fn resize(
        &mut self,
        size: GridSize,
        cell: CellSize,
    ) -> Result<Vec<EngineEffect>, RuntimeError> {
        match self {
            Self::Alacritty(engine) => {
                let _ = cell;
                engine.resize(size);
                Ok(engine.drain_effects())
            }
            Self::Ghostty(engine) => engine.resize(size, cell),
        }
    }
    pub(crate) fn size(&self) -> GridSize {
        match self {
            Self::Alacritty(engine) => engine.size(),
            Self::Ghostty(engine) => engine.size(),
        }
    }
    pub(crate) fn modes(&self) -> Result<TerminalModes, RuntimeError> {
        match self {
            Self::Alacritty(engine) => Ok(engine.modes()),
            Self::Ghostty(engine) => engine.modes(),
        }
    }
    pub(crate) fn generation(&self) -> u64 {
        match self {
            Self::Alacritty(engine) => engine.generation(),
            Self::Ghostty(engine) => engine.generation(),
        }
    }
    pub(crate) fn scroll(
        &mut self,
        scroll: ScrollCommand,
    ) -> Result<(), RuntimeError> {
        match self {
            Self::Alacritty(engine) => {
                engine.scroll(scroll);
                Ok(())
            }
            Self::Ghostty(engine) => engine.scroll(scroll),
        }
    }
    pub(crate) fn snapshot(
        &mut self,
    ) -> Result<TerminalSnapshot, RuntimeError> {
        match self {
            Self::Alacritty(engine) => Ok(engine.snapshot()),
            Self::Ghostty(engine) => engine.snapshot(),
        }
    }
    pub(crate) fn lookup_link(
        &self,
        snapshot: &TerminalSnapshot,
        point: huterm_protocol::MousePosition,
    ) -> huterm_protocol::LinkLookup {
        match self {
            Self::Alacritty(engine) => {
                links::resolve(engine.as_ref(), snapshot, point)
            }
            Self::Ghostty(engine) => {
                links::resolve(engine.as_ref(), snapshot, point)
            }
        }
    }

    pub(crate) fn extract_text(
        &self,
        generation: u64,
        range: BufferRange,
    ) -> Result<Option<String>, RuntimeError> {
        match self {
            Self::Alacritty(engine) => {
                Ok(engine.extract_text(generation, range))
            }
            Self::Ghostty(engine) => engine.extract_text(generation, range),
        }
    }
}

#[cfg(test)]
mod benchmark {
    use super::*;
    use std::hint::black_box;
    use std::sync::Arc;
    use std::time::Instant;

    #[test]
    #[ignore = "release benchmark; run through mise run bench:engine"]
    #[expect(
        clippy::too_many_lines,
        reason = "benchmark fixtures, timing, and output checks remain together"
    )]
    fn engine_benchmark() {
        let kind = match std::env::var("HUTERM_BENCH_ENGINE").as_deref() {
            Ok("ghostty") => TerminalEngineKind::Ghostty,
            Ok("alacritty") | Err(_) => TerminalEngineKind::Alacritty,
            Ok(name) => panic!("unknown benchmark engine {name}"),
        };
        for (name, first, second, expected) in [
            (
                "ascii",
                "ordinary terminal output\r\n".repeat(100),
                "ordinary terminal output\r\n".repeat(100),
                "ordinary",
            ),
            (
                "styled",
                "\x1b[31;1mred\x1b[0m normal\r\n".repeat(100),
                "\x1b[32;1mred\x1b[0m normal\r\n".repeat(100),
                "red",
            ),
            (
                "unicode",
                "界e\u{301}🙂 terminal\r\n".repeat(100),
                "界e\u{301}🙂 terminal\r\n".repeat(100),
                "terminal",
            ),
            (
                "sparse",
                "\x1b[20;1Hsparse A".to_owned(),
                "\x1b[20;1Hsparse B".to_owned(),
                "sparse",
            ),
            (
                "full",
                "\x1b[H".to_owned() + &"x".repeat(120 * 40),
                "\x1b[H".to_owned() + &"y".repeat(120 * 40),
                "",
            ),
        ] {
            let mut engine = TerminalEngine::new(
                TerminalId::new(1),
                GridSize::clamped(120, 40),
                CellSize {
                    width: 8,
                    height: 16,
                },
                kind,
            )
            .unwrap();
            engine.process(b"warmup").unwrap();
            let mut previous = engine.snapshot().unwrap();
            let mut processing = Vec::new();
            let mut snapshots = Vec::new();
            let mut combined = Vec::new();
            let mut reused = 0;
            for iteration in 0..200 {
                let bytes = if iteration % 2 == 0 { &first } else { &second };
                let started = Instant::now();
                black_box(engine.process(black_box(bytes.as_bytes())).unwrap());
                processing.push(started.elapsed().as_nanos());
                let started = Instant::now();
                let snapshot = engine.snapshot().unwrap();
                snapshots.push(started.elapsed().as_nanos());
                combined.push(processing[iteration] + snapshots[iteration]);
                assert_eq!(snapshot.size, GridSize::clamped(120, 40));
                let text: String =
                    snapshot.cells().map(|cell| cell.text.as_str()).collect();
                assert!(text.contains(expected), "{name}: {text:?}");
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
                reused += snapshot
                    .rows
                    .iter()
                    .zip(&previous.rows)
                    .filter(|(after, before)| Arc::ptr_eq(after, before))
                    .count();
                previous = black_box(snapshot);
            }
            let processing_total: u128 = processing.iter().sum();
            combined.sort_unstable();
            processing.sort_unstable();
            snapshots.sort_unstable();
            println!(
                "engine={} revision={} fixture={name} bytes={} iterations=200 process_ns_p50={} process_ns_p95={} snapshot_ns_p50={} snapshot_ns_p95={} rebuilt_rows={} reused_rows={} history={} combined_ns_p50={} combined_ns_p95={} process_bytes_per_second={}",
                kind.name(),
                if kind == TerminalEngineKind::Alacritty {
                    "0.26.0"
                } else {
                    crate::GHOSTTY_REVISION
                },
                first.len(),
                processing[100],
                processing[190],
                snapshots[100],
                snapshots[190],
                8000 - reused,
                reused,
                previous.history_size,
                combined[100],
                combined[190],
                first.len() as u128 * 200 * 1_000_000_000 / processing_total
            );
        }
    }

    #[test]
    #[ignore = "release benchmark; run through mise run bench:engine"]
    fn engine_benchmark_resize() {
        let kind = if std::env::var("HUTERM_BENCH_ENGINE").as_deref()
            == Ok("ghostty")
        {
            TerminalEngineKind::Ghostty
        } else {
            TerminalEngineKind::Alacritty
        };
        let cell = CellSize {
            width: 8,
            height: 16,
        };
        let mut engine = TerminalEngine::new(
            TerminalId::new(1),
            GridSize::clamped(120, 40),
            cell,
            kind,
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
            "engine={} fixture=resize-reflow iterations=200 columns=100/120 rows=40 resize_ns_p50={} resize_ns_p95={} snapshot_ns_p50={} snapshot_ns_p95={}",
            kind.name(),
            resizing[100],
            resizing[190],
            snapshots[100],
            snapshots[190]
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

    fn each_engine(mut test: impl FnMut(&mut TerminalEngine)) {
        for kind in [TerminalEngineKind::Alacritty, TerminalEngineKind::Ghostty]
        {
            let mut engine = TerminalEngine::new(
                TerminalId::new(1),
                GridSize::clamped(8, 3),
                CellSize {
                    width: 8,
                    height: 16,
                },
                kind,
            )
            .unwrap();
            test(&mut engine);
        }
    }

    #[test]
    fn engines_preserve_style_unicode_defaults_and_dynamic_colors() {
        each_engine(|engine| {
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
    fn engines_publish_sparse_rows_metadata_and_skipped_generations() {
        each_engine(|engine| {
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
    fn engines_preserve_shared_scrollback_and_generation_checked_selection() {
        each_engine(|engine| {
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
        each_engine(|engine| {
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
    fn engines_resize_reflow_and_preserve_soft_wrapped_selection() {
        each_engine(|engine| {
            engine.process("abcdef界e\u{301}Z".as_bytes()).unwrap();
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
                engine
                    .extract_text(engine.generation(), range)
                    .unwrap()
                    .as_deref(),
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
            assert_eq!(snapshot.size, GridSize::clamped(12, 4));
            assert_eq!(snapshot.rows.len(), 4);
            assert!(snapshot.rows.iter().all(|row| row.cells.len() == 12));
        });
    }

    #[test]
    fn engines_use_the_last_enabled_mouse_encoding() {
        each_engine(|engine| {
            let inactive_reset =
                if matches!(engine, TerminalEngine::Alacritty(_)) {
                    MouseEncoding::Utf8
                } else {
                    MouseEncoding::Legacy
                };
            engine
                .resize(
                    GridSize::clamped(1, 1),
                    CellSize {
                        width: 1,
                        height: 1,
                    },
                )
                .unwrap();
            for (sequence, encoding) in [
                ("\x1b[?1002h\x1b[?1006h", MouseEncoding::Sgr),
                ("\x1b[?1005h", MouseEncoding::Utf8),
                ("\x1b[?1006l", inactive_reset),
                ("\x1b[?1005l", MouseEncoding::Legacy),
                ("\x1b[?1006h\x1bc", MouseEncoding::Legacy),
            ] {
                engine.process(sequence.as_bytes()).unwrap();
                assert_eq!(
                    engine.modes().unwrap().mouse_encoding,
                    encoding,
                    "{sequence:?}"
                );
            }
        });
    }

    #[test]
    fn engines_answer_primary_device_attributes_conservatively() {
        each_engine(|engine| {
            let expected = if matches!(engine, TerminalEngine::Alacritty(_)) {
                b"\x1b[?6c".as_slice()
            } else {
                b"\x1b[?62;22c".as_slice()
            };
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
    fn engines_do_not_advertise_unsupported_kitty_keyboard_input() {
        each_engine(|engine| {
            let effects = engine.process(b"\x1b[?u\x1b[>31u\x1b[?u").unwrap();
            assert!(!effects.iter().any(|effect| matches!(effect, EngineEffect::PtyWrite(bytes) if bytes.ends_with(b"u"))));
        });
    }

    #[test]
    fn engines_report_modes_and_owned_effects() {
        each_engine(|engine| {
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

    fn each_engine(mut test: impl FnMut(&mut TerminalEngine)) {
        for kind in [TerminalEngineKind::Alacritty, TerminalEngineKind::Ghostty]
        {
            let mut engine = TerminalEngine::new(
                TerminalId::new(1),
                GridSize::clamped(8, 3),
                CellSize {
                    width: 8,
                    height: 16,
                },
                kind,
            )
            .unwrap();
            test(&mut engine);
        }
    }
    #[test]
    fn osc8_native_destination_boundaries_never_return_truncated_targets() {
        each_engine(|engine| {
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
                if matches!(engine, TerminalEngine::Ghostty(_)) && length > 2046
                {
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
        each_engine(|engine| {
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
        each_engine(|engine| {
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
        each_engine(|engine| {
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
        each_engine(|engine| {
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
        each_engine(|engine| {
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
        for kind in [TerminalEngineKind::Alacritty, TerminalEngineKind::Ghostty]
        {
            let long_url =
                format!("https://example.test/{}", "http".repeat(3000));
            let punctuation =
                format!("https://example.test/{}", ")".repeat(12000));
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
                    kind,
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
                            panic!("{kind:?} {name}: {lookup:?}")
                        };
                        assert_eq!(link.destination, destination, "{name}");
                    }
                }
                samples.sort_unstable();
                assert!(
                    samples[95] <= 5000,
                    "{kind:?} {name} p95 exceeded 5 ms: {} us",
                    samples[95]
                );
                println!(
                    "link-benchmark engine={} fixture={name} samples=100 p50_us={} p95_us={} max_us={}",
                    kind.name(),
                    samples[50],
                    samples[95],
                    samples[99]
                );
            }
        }
    }
}
