use super::*;
use huterm_protocol::{LinkLookup, LinkSource, MousePosition, TerminalLink};

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
                    format!("\x1b]8;;{destination}\x1b\\label\x1b]8;;\x1b\\")
                        .as_bytes(),
                )
                .unwrap();
            let snapshot = engine.snapshot().unwrap();
            let result = engine
                .lookup_link(&snapshot, MousePosition { row: 0, column: 1 });
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
fn matched(engine: &mut TerminalEngine, row: u32, column: u32) -> TerminalLink {
    let snapshot = engine.snapshot().unwrap();
    let result = engine.lookup_link(&snapshot, MousePosition { row, column });
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
            engine.lookup_link(&before, MousePosition { row: 0, column: 1 }),
            LinkLookup::Match(_)
        ));
        engine.process(b"\x1b[3J").unwrap();
        let cleared = engine.snapshot().unwrap();
        assert_eq!(cleared.history_size, 0);
        assert_eq!(
            engine.lookup_link(&cleared, MousePosition { row: 0, column: 1 }),
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
        assert!(
            link.cells.iter().any(
                |cell| cell.position == MousePosition { row: 1, column: 2 }
            )
        );
    });
}

#[test]
fn link_lookup_hard_breaks_controls_out_of_grid_and_scan_limits_are_not_targets()
 {
    with_engine(|engine| {
        engine.process(b"https://\r\nexample.test").unwrap();
        let snapshot = engine.snapshot().unwrap();
        assert_eq!(
            engine.lookup_link(&snapshot, MousePosition { row: 0, column: 2 }),
            LinkLookup::NoMatch
        );
        assert_eq!(
            engine.lookup_link(&snapshot, MousePosition { row: 0, column: 8 }),
            LinkLookup::NoMatch
        );
        engine
            .process(format!("\x1bchttps://x/{}", "a".repeat(2048)).as_bytes())
            .unwrap();
        let snapshot = engine.snapshot().unwrap();
        assert_eq!(
            engine.lookup_link(&snapshot, MousePosition { row: 2, column: 0 }),
            LinkLookup::ScanLimit
        );
        engine
            .process(b"\x1bc\x1b]8;;file:///tmp/a\x1b\\https://x\x1b]8;;\x1b\\")
            .unwrap();
        let snapshot = engine.snapshot().unwrap();
        assert_eq!(
            engine.lookup_link(&snapshot, MousePosition { row: 0, column: 0 }),
            LinkLookup::NoMatch
        );
    });
}

#[test]
fn link_lookup_limits_apply_to_the_token_not_the_wrapped_line() {
    with_engine(|engine| {
        // 152 soft-wrapped rows, beyond the row limit, ending with a URL
        // on the last two.
        engine
            .process(format!("{}https://a.test/p", "x ".repeat(600)).as_bytes())
            .unwrap();
        let link = matched(engine, 2, 0);
        assert_eq!(link.destination, "https://a.test/p");
        assert_eq!(link.cells.len(), 16);
    });
}

#[test]
fn link_lookup_admits_a_token_at_the_row_limit_between_delimiters() {
    with_engine(|engine| {
        // A URL filling exactly 128 rows, with a delimiter on the rows
        // just before and after it.
        let url = format!("https://a.test/{}", "p".repeat(128 * 8 - 15));
        engine
            .process(format!("xxxxxxx {url} y").as_bytes())
            .unwrap();
        assert_eq!(matched(engine, 1, 0).destination, url);
    });
}

#[test]
fn link_lookup_reads_wide_characters_inside_a_url() {
    with_engine(|engine| {
        engine.process("https://x/界z".as_bytes()).unwrap();
        assert_eq!(matched(engine, 1, 4).destination, "https://x/界z");
        assert_eq!(matched(engine, 1, 3).destination, "https://x/界z");
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
        engine
            .process(
                b"\x1b[H\x1b]8;;https://other.test\x1b\\label\x1b]8;;\x1b\\",
            )
            .unwrap();
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
            engine.lookup_link(&snapshot, MousePosition { row: 0, column: 0 }),
            LinkLookup::NoMatch
        );
        engine.process(b"\x1b[?1049l").unwrap();
        assert_eq!(matched(engine, 0, 0).destination, "https://other.test");
    });
}
