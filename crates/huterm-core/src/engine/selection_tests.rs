use super::*;
use huterm_protocol::BufferPoint;

const COLUMNS: u16 = 20;
const ROWS: u16 = 4;

fn engine(output: &[u8]) -> TerminalEngine {
    let mut engine = TerminalEngine::new(
        TerminalId::new(1),
        GridSize::clamped(COLUMNS, ROWS),
        CellSize {
            width: 8,
            height: 16,
        },
        TerminalPresentation::default(),
    )
    .unwrap();
    engine.process(output).unwrap();
    engine
}

/// The point at `column` of screen `row`, counted from the top of a
/// terminal without history.
fn at(row: u16, column: u16) -> BufferPoint {
    BufferPoint {
        rows_from_live_bottom: usize::from(ROWS - 1 - row),
        column,
    }
}

/// The text a multi-click gesture from `anchor` to `head` selects.
fn selected(
    engine: &TerminalEngine,
    anchor: BufferPoint,
    head: BufferPoint,
    unit: SelectionUnit,
) -> Option<String> {
    let generation = engine.generation();
    let range = engine
        .select_units(generation, anchor, head, unit)
        .unwrap()?;
    engine.extract_text(generation, range).unwrap()
}

fn word(engine: &TerminalEngine, row: u16, column: u16) -> Option<String> {
    selected(
        engine,
        at(row, column),
        at(row, column),
        SelectionUnit::Word,
    )
}

#[test]
fn a_word_keeps_paths_and_identifiers_whole_and_stops_at_colons() {
    let engine = engine(b"ls /usr/local/bin\r\nsrc/main_x.rs:12 ok");
    assert_eq!(word(&engine, 0, 8).as_deref(), Some("/usr/local/bin"));
    assert_eq!(word(&engine, 1, 2).as_deref(), Some("src/main_x.rs"));
    assert_eq!(word(&engine, 1, 15).as_deref(), Some("12"));
    assert_eq!(word(&engine, 1, 13).as_deref(), Some(":"));
    assert_eq!(
        engine
            .select_units(
                engine.generation(),
                at(0, 2),
                at(0, 2),
                SelectionUnit::Word
            )
            .unwrap(),
        Some(BufferRange::ordered(at(0, 2), at(0, 2))),
        "a space selects its own run of boundaries"
    );
    assert_eq!(word(&engine, 3, 4), None, "unwritten cells select nothing");
}

#[test]
fn a_word_inside_a_link_selects_the_whole_link() {
    // The plain URL soft-wraps onto the second row.
    let plain = engine(b"see https://ex.com/a?b=c ok");
    assert_eq!(
        word(&plain, 0, 14).as_deref(),
        Some("https://ex.com/a?b=c"),
        "the URL does not split at its colon"
    );
    assert_eq!(word(&plain, 1, 1).as_deref(), Some("https://ex.com/a?b=c"));
    assert_eq!(word(&plain, 1, 6).as_deref(), Some("ok"));
    let explicit =
        engine(b"go \x1b]8;;https://ex.com/\x1b\\the:docs\x1b]8;;\x1b\\ now");
    assert_eq!(word(&explicit, 0, 4).as_deref(), Some("the:docs"));
    assert_eq!(word(&explicit, 0, 13).as_deref(), Some("now"));
}

#[test]
fn a_word_drag_joins_the_words_nearest_each_end() {
    let engine = engine(b"one two three\r\nfour");
    let drag =
        |anchor, head| selected(&engine, anchor, head, SelectionUnit::Word);
    assert_eq!(drag(at(0, 9), at(0, 1)).as_deref(), Some("one two three"));
    assert_eq!(drag(at(0, 1), at(0, 5)).as_deref(), Some("one two"));
    assert_eq!(
        drag(at(0, 1), at(1, 2)).as_deref(),
        Some("one two three\nfour")
    );
    assert_eq!(
        drag(at(0, 9), at(3, 4)).as_deref(),
        Some("three\nfour"),
        "from unwritten cells the head takes the nearest word toward the anchor"
    );
    // A drag that starts on a link keeps the whole link.
    let link = super::TerminalEngine::new(
        TerminalId::new(1),
        GridSize::clamped(40, ROWS),
        CellSize {
            width: 8,
            height: 16,
        },
        TerminalPresentation::default(),
    );
    let mut link = link.unwrap();
    link.process(b"at https://ex.com/a then").unwrap();
    let range = link
        .select_units(
            link.generation(),
            at(0, 10),
            at(0, 23),
            SelectionUnit::Word,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        link.extract_text(link.generation(), range)
            .unwrap()
            .as_deref(),
        Some("https://ex.com/a then")
    );
}

#[test]
fn a_line_spans_soft_wraps_and_a_drag_takes_blank_rows_whole() {
    // The first line soft-wraps onto the second row; the third is blank.
    let engine = engine(b"  first line wraps here  \r\n\r\nlast");
    let line =
        |anchor, head| selected(&engine, anchor, head, SelectionUnit::Line);
    let wrapped = Some("  first line wraps here".to_owned());
    assert_eq!(line(at(0, 0), at(0, 0)), wrapped);
    assert_eq!(line(at(1, 3), at(1, 3)), wrapped);
    assert_eq!(line(at(2, 5), at(2, 5)), None, "a press on a blank line");
    assert_eq!(
        line(at(0, 0), at(3, 1)).as_deref(),
        Some("  first line wraps here\n\nlast")
    );
    let generation = engine.generation();
    let blank = engine
        .select_units(generation, at(3, 0), at(2, 4), SelectionUnit::Line)
        .unwrap();
    assert_eq!(
        blank,
        Some(BufferRange::ordered(at(2, 0), at(3, 3))),
        "a drag onto a blank row takes its whole row"
    );
}

#[test]
fn a_stale_generation_or_outside_point_selects_nothing() {
    let mut engine = engine(b"word");
    let generation = engine.generation();
    let outside = BufferPoint {
        rows_from_live_bottom: 0,
        column: COLUMNS,
    };
    assert_eq!(
        engine
            .select_units(generation, outside, outside, SelectionUnit::Word)
            .unwrap(),
        None
    );
    engine.process(b" more").unwrap();
    assert_eq!(
        engine
            .select_units(generation, at(0, 1), at(0, 1), SelectionUnit::Word)
            .unwrap(),
        None
    );
}

#[test]
fn a_line_keeps_its_indentation_but_not_text_before_it() {
    // Indentation that fills the whole first row of a wrapped line.
    let deep = engine(b"                         deep");
    assert_eq!(
        selected(&deep, at(1, 6), at(1, 6), SelectionUnit::Line).as_deref(),
        Some("                         deep")
    );
    // A prompt that fills a wrapped row bounds the indented input after it.
    let wrapped_prompt =
        engine(b"\x1b]133;A\x07$                   \x1b]133;B\x07  ls");
    assert_eq!(
        selected(&wrapped_prompt, at(1, 2), at(1, 2), SelectionUnit::Line)
            .as_deref(),
        Some("ls")
    );
    // The third row is a shell-integration prompt followed by its input.
    let engine = engine(
        b"    spaced in\r\n\tTabbed in\r\n\x1b]133;A\x07$ \x1b]133;B\x07ls -la",
    );
    let line = |row: u16, column: u16| {
        selected(
            &engine,
            at(row, column),
            at(row, column),
            SelectionUnit::Line,
        )
    };
    assert_eq!(line(0, 9).as_deref(), Some("    spaced in"));
    assert_eq!(line(1, 9).as_deref(), Some("        Tabbed in"));
    assert_eq!(
        line(2, 4).as_deref(),
        Some("ls -la"),
        "the prompt still bounds its input line"
    );
}
