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
        assert_eq!(snapshot.rows[0].cells[1].foreground, CellColor::Indexed(4));
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
        let client_offset = engine.snapshot().unwrap().viewport.bottom_offset;
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
