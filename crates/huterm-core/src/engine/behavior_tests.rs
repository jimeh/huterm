//! Engine behavior observable through the facade: replies, snapshots, row
//! reuse, and metadata effects for byte streams written as a PTY would.

use std::sync::Arc;

use huterm_protocol::{
    CellColor, CellSize, GridSize, Rgb, TerminalId, TerminalPresentation,
    TerminalRow, TerminalSnapshot,
};

use super::{EngineEffect, TerminalEngine};

fn presentation() -> TerminalPresentation {
    let mut palette = TerminalPresentation::default().palette;
    palette[1] = Rgb {
        red: 0xaa,
        green: 0xbb,
        blue: 0xcc,
    };
    TerminalPresentation {
        foreground: Rgb {
            red: 0x11,
            green: 0x22,
            blue: 0x33,
        },
        background: Rgb {
            red: 0x44,
            green: 0x55,
            blue: 0x66,
        },
        cursor: Rgb {
            red: 0x77,
            green: 0x88,
            blue: 0x99,
        },
        palette,
    }
}

fn engine() -> TerminalEngine {
    TerminalEngine::new(
        TerminalId::new(1),
        GridSize::clamped(8, 3),
        CellSize {
            width: 9,
            height: 17,
        },
        presentation(),
    )
    .unwrap()
}

fn replies(effects: Vec<EngineEffect>) -> Vec<Vec<u8>> {
    effects
        .into_iter()
        .filter_map(|effect| match effect {
            EngineEffect::PtyWrite(bytes) => Some(bytes),
            _ => None,
        })
        .collect()
}

#[test]
fn native_queries_report_seeded_colors_size_and_appearance() {
    let mut engine = engine();
    let effects = engine
        .process(
            b"\x1b]4;1;?\x1b\\\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b]12;?\x1b\\\x1b[14t\x1b[16t\x1b[18t\x1b[?996n",
        )
        .unwrap();
    assert_eq!(
        replies(effects),
        vec![
            b"\x1b]4;1;rgb:aaaa/bbbb/cccc\x1b\\".to_vec(),
            b"\x1b]10;rgb:1111/2222/3333\x1b\\".to_vec(),
            b"\x1b]11;rgb:4444/5555/6666\x1b\\".to_vec(),
            b"\x1b]12;rgb:7777/8888/9999\x1b\\".to_vec(),
            b"\x1b[4;51;72t".to_vec(),
            b"\x1b[6;17;9t".to_vec(),
            b"\x1b[8;3;8t".to_vec(),
            b"\x1b[?997;1n".to_vec(),
        ]
    );
}

#[test]
fn size_queries_follow_the_existing_ordered_resize_state() {
    let mut engine = engine();
    engine
        .resize(
            GridSize::clamped(5, 4),
            CellSize {
                width: 11,
                height: 19,
            },
        )
        .unwrap();
    assert_eq!(
        replies(engine.process(b"\x1b[14t\x1b[16t\x1b[18t").unwrap()),
        vec![
            b"\x1b[4;76;55t".to_vec(),
            b"\x1b[6;19;11t".to_vec(),
            b"\x1b[8;4;5t".to_vec(),
        ]
    );
}

#[test]
fn size_queries_are_silent_until_cell_geometry_is_known() {
    let mut engine = TerminalEngine::new(
        TerminalId::new(1),
        GridSize::clamped(8, 3),
        CellSize {
            width: 0,
            height: 0,
        },
        presentation(),
    )
    .unwrap();
    assert!(
        replies(engine.process(b"\x1b[14t\x1b[16t\x1b[18t").unwrap())
            .is_empty()
    );

    engine
        .resize(
            GridSize::clamped(5, 4),
            CellSize {
                width: 11,
                height: 19,
            },
        )
        .unwrap();
    assert_eq!(
        replies(engine.process(b"\x1b[14t\x1b[16t\x1b[18t").unwrap()),
        vec![
            b"\x1b[4;76;55t".to_vec(),
            b"\x1b[6;19;11t".to_vec(),
            b"\x1b[8;4;5t".to_vec(),
        ]
    );
}

#[test]
fn mutations_queries_and_resets_are_ordered_within_one_write() {
    let mut engine = engine();
    let effects = engine
        .process(
            b"\x1b]4;1;#010203\x1b\\\x1b]4;1;?\x1b\\\x1b]104;1\x1b\\\x1b]4;1;?\x1b\\\x1b]10;#abcdef\x1b\\\x1b]10;?\x1b\\\x1b]110\x1b\\\x1b]10;?\x1b\\\x1b]11;#ffffff\x1b\\\x1b]11;?\x1b\\\x1b[?996n\x1b]111\x1b\\\x1b]11;?\x1b\\\x1b[?996n\x1b]12;#0a0b0c\x1b\\\x1b]12;?\x1b\\\x1b]112\x1b\\\x1b]12;?\x1b\\",
        )
        .unwrap();
    assert_eq!(
        replies(effects),
        vec![
            b"\x1b]4;1;rgb:0101/0202/0303\x1b\\".to_vec(),
            b"\x1b]4;1;rgb:aaaa/bbbb/cccc\x1b\\".to_vec(),
            b"\x1b]10;rgb:abab/cdcd/efef\x1b\\".to_vec(),
            b"\x1b]10;rgb:1111/2222/3333\x1b\\".to_vec(),
            b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\".to_vec(),
            b"\x1b[?997;2n".to_vec(),
            b"\x1b]11;rgb:4444/5555/6666\x1b\\".to_vec(),
            b"\x1b[?997;1n".to_vec(),
            b"\x1b]12;rgb:0a0a/0b0b/0c0c\x1b\\".to_vec(),
            b"\x1b]12;rgb:7777/8888/9999\x1b\\".to_vec(),
        ]
    );
}

#[test]
fn osc_dispatch_at_escape_updates_override_state_before_the_next_byte() {
    let mut engine = engine();
    engine.process(b"A\x1b[31mB").unwrap();
    engine
        .process(
            b"\x1b]10;#010203\x1b]11;#040506\x1b]12;#070809\x1b]4;1;#0a0b0c\x1b",
        )
        .unwrap();
    let overridden = engine.snapshot().unwrap();
    assert_eq!(
        (
            overridden.rows[0].cells[0].foreground,
            overridden.rows[0].cells[0].background,
            overridden.rows[0].cells[1].foreground,
            overridden.cursor_color,
        ),
        (
            CellColor::Rgb(Rgb {
                red: 1,
                green: 2,
                blue: 3,
            }),
            CellColor::Rgb(Rgb {
                red: 4,
                green: 5,
                blue: 6,
            }),
            CellColor::Rgb(Rgb {
                red: 10,
                green: 11,
                blue: 12,
            }),
            Some(Rgb {
                red: 7,
                green: 8,
                blue: 9,
            }),
        )
    );

    engine
        .process(b"]110\x1b]111\x1b]112\x1b]104;1\x1b")
        .unwrap();
    let reset = engine.snapshot().unwrap();
    assert_eq!(
        (
            reset.rows[0].cells[0].foreground,
            reset.rows[0].cells[0].background,
            reset.rows[0].cells[1].foreground,
            reset.cursor_color,
        ),
        (
            CellColor::DefaultForeground,
            CellColor::DefaultBackground,
            CellColor::Indexed(1),
            None,
        )
    );
}

#[test]
fn osc_dispatch_at_can_or_sub_updates_override_state_immediately() {
    for terminator in [0x18, 0x1a] {
        let mut engine = engine();
        engine.process(b"A").unwrap();
        let mut mutation = b"\x1b]10;#010203".to_vec();
        mutation.push(terminator);
        engine.process(&mutation).unwrap();
        assert_eq!(
            engine.snapshot().unwrap().rows[0].cells[0].foreground,
            CellColor::Rgb(Rgb {
                red: 1,
                green: 2,
                blue: 3,
            }),
            "terminator={terminator:#x}"
        );

        let mut reset = b"\x1b]110".to_vec();
        reset.push(terminator);
        engine.process(&reset).unwrap();
        assert_eq!(
            engine.snapshot().unwrap().rows[0].cells[0].foreground,
            CellColor::DefaultForeground,
            "terminator={terminator:#x}"
        );
    }
}

#[test]
fn live_scrolling_reuses_shifted_rows_before_and_at_the_scrollback_budget() {
    for budget in [None, Some(0)] {
        let mut engine = TerminalEngine::new(
            TerminalId::new(1),
            GridSize::clamped(8, 4),
            CellSize {
                width: 8,
                height: 16,
            },
            presentation(),
        )
        .unwrap();
        if let Some(bytes) = budget {
            engine.set_scrollback_limit(bytes).unwrap();
        }
        engine.process(b"\r\n\r\nA\r\nB").unwrap();
        let before = engine.snapshot().unwrap();
        engine.process(b"\r\nC").unwrap();
        let after = engine.snapshot().unwrap();
        if budget.is_some() {
            assert_eq!(after.history_size, before.history_size);
        }
        let text = |row: &TerminalRow| {
            row.cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>()
        };
        assert_eq!(text(&after.rows[1]), "A       ", "{budget:?}");
        assert!(Arc::ptr_eq(&after.rows[1], &before.rows[2]), "{budget:?}");
        assert!(Arc::ptr_eq(&after.rows[2], &before.rows[3]), "{budget:?}");
        assert!(
            before.rows[..2]
                .iter()
                .any(|blank| Arc::ptr_eq(blank, &after.rows[0])),
            "{budget:?}"
        );
        assert_eq!(text(&after.rows[3]), "C       ", "{budget:?}");
        assert_eq!(engine.last_snapshot_stats().allocated, 1, "{budget:?}");
    }
}

fn screen_text(snapshot: &TerminalSnapshot) -> String {
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
        .collect::<Vec<_>>()
        .join("|")
}

#[test]
fn clearing_history_keeps_the_screen() {
    let mut engine = engine();
    engine.process(b"1\r\n2\r\n3\r\n4\r\n5").unwrap();
    assert_eq!(engine.snapshot().unwrap().history_size, 2);
    engine.clear_history().unwrap();
    let snapshot = engine.snapshot().unwrap();
    assert_eq!(snapshot.history_size, 0);
    assert_eq!(screen_text(&snapshot), "3|4|5");
}

#[test]
fn clearing_history_waits_for_an_unfinished_sequence_or_codepoint() {
    for (partial, rest, shown) in [
        (&b"\x1b[3"[..], &b"1mX"[..], "X"),
        (&b"\x1b]2;title"[..], &b"\x07X"[..], "X"),
        // The first two bytes of U+2603.
        (&b"\xe2\x98"[..], &b"\x83"[..], "\u{2603}"),
    ] {
        let mut engine = engine();
        engine.process(b"1\r\n2\r\n3\r\n4\r\n").unwrap();
        engine.process(partial).unwrap();
        engine.clear_history().unwrap();
        assert_eq!(engine.snapshot().unwrap().history_size, 2, "{partial:?}");
        engine.process(rest).unwrap();
        let snapshot = engine.snapshot().unwrap();
        assert_eq!(snapshot.history_size, 0, "{partial:?}");
        // The interrupted sequence still completed as sent.
        assert_eq!(screen_text(&snapshot), format!("3|4|{shown}"));
    }
}

#[test]
fn reset_clears_the_screen_history_and_modes() {
    let mut engine = engine();
    engine.process(b"\x1b[?2004h1\r\n2\r\n3\r\n4").unwrap();
    assert!(engine.modes().unwrap().bracketed_paste);
    engine.reset().unwrap();
    let snapshot = engine.snapshot().unwrap();
    assert_eq!(snapshot.history_size, 0);
    assert_eq!(screen_text(&snapshot), "||");
    assert!(!engine.modes().unwrap().bracketed_paste);
}

#[test]
fn rewriting_a_row_with_identical_content_keeps_its_arc() {
    let mut engine = engine();
    engine.process(b"same").unwrap();
    let before = engine.snapshot().unwrap();
    engine.process(b"\rsame").unwrap();
    let after = engine.snapshot().unwrap();
    assert!(Arc::ptr_eq(&before.rows[0], &after.rows[0]));
    let stats = engine.last_snapshot_stats();
    assert_eq!((stats.extracted, stats.allocated), (1, 0));
}

#[test]
fn non_color_osc_and_utf8_output_keep_snapshots_partial() {
    let mut engine = TerminalEngine::new(
        TerminalId::new(1),
        GridSize::clamped(20, 10),
        CellSize {
            width: 9,
            height: 17,
        },
        presentation(),
    )
    .unwrap();
    engine.process(b"first\r\nsecond\r\nthird").unwrap();
    engine.snapshot().unwrap();
    // OSC 8 is absent: Ghostty's own damage still rebuilds most rows.
    for chunk in [
        "\x1b]2;title\x07\x1b[2;1Hx".as_bytes(),
        b"\x1b]133;A\x07\x1b[2;1H$ \x1b]133;B\x07\x1b]7;file:///tmp\x07",
        "\x1b[2;1H\x1b[31m\u{255d}\u{5e1d}\x1b[0m".as_bytes(),
    ] {
        engine.process(chunk).unwrap();
        engine.snapshot().unwrap();
        let extracted = engine.last_snapshot_stats().extracted;
        assert!(
            extracted <= 2,
            "{:?} extracted {extracted} of 10 rows",
            String::from_utf8_lossy(chunk)
        );
    }
    // Shows the fixture detects a full redraw. OSC 4 dirties Ghostty's
    // palette on its own, so this does not test the hint; the full-probe
    // differential test guards against missed color changes.
    engine.process(b"\x1b]4;1;#123456\x07").unwrap();
    engine.snapshot().unwrap();
    assert_eq!(engine.last_snapshot_stats().extracted, 10);
}

#[test]
fn default_and_equal_osc_overrides_survive_theme_updates_and_resets() {
    let mut engine = engine();
    engine.process(b"A\x1b[31mB").unwrap();
    let initial = engine.snapshot().unwrap();
    assert_eq!(
        initial.rows[0].cells[0].foreground,
        CellColor::DefaultForeground
    );
    assert_eq!(initial.rows[0].cells[1].foreground, CellColor::Indexed(1));

    engine
        .process(
            b"\r\x1b[0m\x1b]10;#112233\x1b\\\x1b]11;#ffffff\x1b\\\x1b]12;#778899\x1b\\\x1b]4;1;#aabbcc\x1b\\A",
        )
        .unwrap();
    let overridden = engine.snapshot().unwrap();
    assert_eq!(
        overridden.rows[0].cells[0].foreground,
        CellColor::Rgb(presentation().foreground)
    );
    assert_eq!(
        overridden.rows[0].cells[0].background,
        CellColor::Rgb(Rgb {
            red: 0xff,
            green: 0xff,
            blue: 0xff,
        })
    );
    assert_eq!(
        overridden.rows[0].cells[1].foreground,
        CellColor::Rgb(presentation().palette[1])
    );
    assert_eq!(overridden.cursor_color, Some(presentation().cursor));

    let mut changed = presentation();
    changed.foreground.red = 0xfe;
    changed.background = Rgb {
        red: 0x10,
        green: 0x20,
        blue: 0x30,
    };
    changed.cursor.blue = 0xdc;
    changed.palette[1].red = 0xcb;
    let generation = engine.generation();
    engine.update_presentation(changed.clone()).unwrap();
    let themed = engine.snapshot().unwrap();
    assert_eq!(themed.generation, generation);
    // Every row is read again; rows with unchanged cells may keep their Arc.
    assert_eq!(engine.last_snapshot_stats().extracted, themed.rows.len());
    assert_eq!(
        themed.rows[0].cells[0].foreground,
        CellColor::Rgb(presentation().foreground)
    );
    assert_eq!(
        replies(
            engine
                .process(
                    b"\x1b]4;1;?\x1b\\\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b]12;?\x1b\\\x1b[?996n"
                )
                .unwrap()
        ),
        vec![
            b"\x1b]4;1;rgb:aaaa/bbbb/cccc\x1b\\".to_vec(),
            b"\x1b]10;rgb:1111/2222/3333\x1b\\".to_vec(),
            b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\".to_vec(),
            b"\x1b]12;rgb:7777/8888/9999\x1b\\".to_vec(),
            b"\x1b[?997;2n".to_vec(),
        ]
    );

    assert_eq!(
        replies(
            engine
                .process(
                    b"\x1b]104;1\x1b\\\x1b]110\x1b\\\x1b]111\x1b\\\x1b]112\x1b\\\x1b]4;1;?\x1b\\\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b]12;?\x1b\\\x1b[?996n"
                )
                .unwrap()
        ),
        vec![
            b"\x1b]4;1;rgb:cbcb/bbbb/cccc\x1b\\".to_vec(),
            b"\x1b]10;rgb:fefe/2222/3333\x1b\\".to_vec(),
            b"\x1b]11;rgb:1010/2020/3030\x1b\\".to_vec(),
            b"\x1b]12;rgb:7777/8888/dcdc\x1b\\".to_vec(),
            b"\x1b[?997;1n".to_vec(),
        ]
    );
    let reset = engine.snapshot().unwrap();
    assert_eq!(
        reset.rows[0].cells[0].foreground,
        CellColor::DefaultForeground
    );
    assert_eq!(
        reset.rows[0].cells[0].background,
        CellColor::DefaultBackground
    );
    assert_eq!(reset.rows[0].cells[1].foreground, CellColor::Indexed(1));
    assert_eq!(reset.cursor_color, None);
    assert_eq!(engine.presentation(), &changed);
}

#[test]
fn split_queries_reply_once_after_completion() {
    for (query, dispatch_at, expected) in [
        (
            b"\x1b]10;?\x1b\\".as_slice(),
            b"\x1b]10;?\x1b\\".len() - 1,
            b"\x1b]10;rgb:1111/2222/3333\x1b\\".as_slice(),
        ),
        (
            b"\x1b[14t".as_slice(),
            b"\x1b[14t".len(),
            b"\x1b[4;51;72t".as_slice(),
        ),
        (
            b"\x1b[?996n".as_slice(),
            b"\x1b[?996n".len(),
            b"\x1b[?997;1n".as_slice(),
        ),
    ] {
        for split in 0..=query.len() {
            let mut engine = engine();
            let mut actual = replies(engine.process(&query[..split]).unwrap());
            if split < dispatch_at {
                assert!(actual.is_empty());
            }
            actual.extend(replies(engine.process(&query[split..]).unwrap()));
            assert_eq!(
                actual,
                vec![expected.to_vec()],
                "query={query:?} split={split}"
            );
        }
    }
}

#[test]
fn malformed_and_unsupported_queries_are_silent() {
    let mut engine = engine();
    assert!(
        replies(
            engine
                .process(
                    b"\x1b]4;999;?\x1b\\\x1b]10;bogus\x1b\\\x1b[15t\x1b[?996;1n"
                )
                .unwrap()
        )
        .is_empty()
    );
}

#[test]
fn osc_104_without_indices_resets_the_complete_palette() {
    let mut engine = engine();
    assert_eq!(
        replies(
            engine
                .process(
                    b"\x1b]4;1;#010203;2;#040506\x1b\\\x1b]104\x1b\\\x1b]4;1;?;2;?\x1b\\"
                )
                .unwrap()
        ),
        vec![concat!(
            "\x1b]4;1;rgb:aaaa/bbbb/cccc\x1b\\",
            "\x1b]4;2;rgb:0000/cdcd/0000\x1b\\"
        )
        .as_bytes()
        .to_vec()]
    );
}

#[test]
fn linked_native_memset_preserves_rust_byte_fills() {
    // Ghostty's exported memset once treated C's int fill as a u8.
    // Rust may pass -1 for 0xff, corrupting hash-table control bytes.
    for size in [4, 8, 16, 17, 31, 32, 64, 256] {
        let bytes = vec![u8::MAX; std::hint::black_box(size)];
        assert!(
            bytes.iter().all(|byte| *byte == u8::MAX),
            "native memset corrupted a {size}-byte fill: {bytes:?}"
        );
    }
}

#[test]
fn split_osc_palette_override_equal_to_default_stays_explicit_until_reset() {
    // The engine seeds its default palette from the presentation.
    let original = TerminalPresentation::default().palette[1];
    let sequence = format!(
        "\x1b]4;1;#{:02x}{:02x}{:02x}\x1b\\",
        original.red, original.green, original.blue
    );
    for split in 0..=sequence.len() {
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
        engine.process(b"\x1b[31mA").unwrap();
        assert_eq!(
            engine.snapshot().unwrap().rows[0].cells[0].foreground,
            CellColor::Indexed(1)
        );
        engine.process(&sequence.as_bytes()[..split]).unwrap();
        let _ = engine.snapshot().unwrap();
        engine.process(&sequence.as_bytes()[split..]).unwrap();
        assert_eq!(
            engine.snapshot().unwrap().rows[0].cells[0].foreground,
            CellColor::Rgb(original),
            "split={split}"
        );
        engine.process(b"\x1b]104;1\x07").unwrap();
        assert_eq!(
            engine.snapshot().unwrap().rows[0].cells[0].foreground,
            CellColor::Indexed(1)
        );
    }
}

#[test]
fn osc7_directory_callbacks_preserve_framing_and_chunk_boundaries() {
    for terminator in ["\x07", "\x1b\\"] {
        let sequence =
            format!("\x1b]7;file://localhost/tmp/hello%20world{terminator}");
        for split in 0..=sequence.len() {
            let mut engine = engine();
            let mut effects =
                engine.process(&sequence.as_bytes()[..split]).unwrap();
            effects
                .extend(engine.process(&sequence.as_bytes()[split..]).unwrap());
            assert!(effects.iter().any(|effect| matches!(
                effect,
                EngineEffect::Directory(Some(directory))
                    if directory.path() == "/tmp/hello world" && directory.is_local()
            )), "split={split} terminator={terminator:?}");
        }
        let mut engine = engine();
        let mut effects = Vec::new();
        for byte in sequence.bytes() {
            effects.extend(engine.process(&[byte]).unwrap());
        }
        assert!(effects.iter().any(|effect| matches!(
            effect,
            EngineEffect::Directory(Some(directory)) if directory.path() == "/tmp/hello world"
        )));
    }
}

#[test]
fn empty_osc7_clears_directory_metadata() {
    let mut engine = engine();
    let effects = engine
        .process(b"\x1b]7;file://localhost/tmp\x07\x1b]7;\x07")
        .unwrap();
    assert!(matches!(
        effects.as_slice(),
        [
            EngineEffect::Directory(Some(_)),
            EngineEffect::Directory(None)
        ]
    ));
}

#[test]
fn iterm_current_directory_reports_bare_display_only_paths() {
    let mut engine = engine();
    let effects = engine
        .process(b"\x1b]1337;CurrentDir=/tmp/bare\x07")
        .unwrap();
    assert!(matches!(
        effects.as_slice(),
        [EngineEffect::Directory(Some(directory))]
            if directory.path() == "/tmp/bare" && !directory.is_local()
    ));
}

#[test]
fn malformed_utf8_directory_reports_are_nonfatal_and_ignored() {
    let mut engine = engine();
    let effects = engine
        .process(b"\x1b]7;file://localhost/tmp/\xff\x07")
        .unwrap();
    assert!(
        !effects
            .iter()
            .any(|effect| matches!(effect, EngineEffect::Directory(_)))
    );
}
