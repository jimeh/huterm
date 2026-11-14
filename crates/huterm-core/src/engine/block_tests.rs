//! Command blocks and sticky prompts in snapshots: where each record's
//! block sits for one viewport, checked through the text Copy would take
//! from it.

use std::fmt::Write as _;

use huterm_protocol::{
    BufferPoint, BufferRange, CellColor, CellSize, CommandBlock, GridSize,
    ScrollCommand, ScrollbackLimit, TerminalId, TerminalPresentation,
    TerminalSnapshot,
};

use super::TerminalEngine;

fn engine(columns: u16, rows: u16) -> TerminalEngine {
    TerminalEngine::new(
        TerminalId::new(1),
        GridSize::clamped(columns, rows),
        CellSize {
            width: 9,
            height: 17,
        },
        TerminalPresentation::default(),
    )
    .unwrap()
}

fn mark(body: &str) -> String {
    format!("\x1b]133;{body}\x07")
}

/// A two-line prompt, the first line holding the directory, then the
/// command line, output, and end, as zsh with a two-line theme sends them.
fn command(line: &str, output: &[&str], end: &str) -> String {
    let mut text = format!(
        "{}~/src\r\n\x1b[32m$\x1b[0m {}{line}\r\n{}",
        mark("A"),
        mark("B"),
        mark("C")
    );
    for row in output {
        let _ = write!(text, "{row}\r\n");
    }
    text + &mark(end)
}

/// The prompt a shell shows while it waits for input.
fn prompt() -> String {
    format!("{}~/src\r\n\x1b[32m$\x1b[0m {}", mark("A"), mark("B"))
}

fn process(engine: &mut TerminalEngine, output: &str) {
    engine.process(output.as_bytes()).unwrap();
    engine.command_update();
}

fn point(rows_from_live_bottom: usize, column: u16) -> BufferPoint {
    BufferPoint {
        rows_from_live_bottom,
        column,
    }
}

/// The text Copy takes from `first` to `last`, whole rows.
fn text(
    engine: &TerminalEngine,
    snapshot: &TerminalSnapshot,
    first: usize,
    last: usize,
) -> String {
    engine
        .extract_text(
            snapshot.generation,
            BufferRange::ordered(
                point(first, 0),
                point(last, snapshot.size.columns - 1),
            ),
        )
        .unwrap()
        .unwrap_or_default()
}

/// A block's output and whole-block text, as the select commands take them.
fn block_text(
    engine: &TerminalEngine,
    snapshot: &TerminalSnapshot,
    block: &CommandBlock,
) -> (Option<String>, String) {
    (
        block
            .output
            .map(|output| text(engine, snapshot, output, block.end)),
        text(engine, snapshot, block.prompt, block.end),
    )
}

fn sticky_text(snapshot: &TerminalSnapshot) -> Option<Vec<String>> {
    snapshot.sticky_prompt.as_ref().map(|sticky| {
        sticky
            .rows
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .map(|cell| cell.text.displayed())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    })
}

fn sequences(snapshot: &TerminalSnapshot) -> Vec<u64> {
    snapshot
        .commands
        .iter()
        .map(|block| block.sequence)
        .collect()
}

#[test]
fn blocks_hold_prompt_and_output_and_drop_trailing_blanks_and_empty_prompts() {
    let mut engine = engine(30, 20);
    process(&mut engine, &command("ls", &["a", "b", ""], "D;0"));
    // An empty Enter leaves a prompt with no command between the two.
    process(&mut engine, &format!("{}\r\n", prompt()));
    process(&mut engine, &command("false", &[], "D;1"));
    process(&mut engine, &prompt());
    let snapshot = engine.snapshot().unwrap();
    assert_eq!(sequences(&snapshot), [1, 2]);
    let [ls, failed] = &snapshot.commands[..] else {
        unreachable!()
    };
    assert_eq!(
        block_text(&engine, &snapshot, ls),
        (Some("a\nb".to_owned()), "~/src\n$ ls\na\nb".to_owned())
    );
    assert_eq!(
        block_text(&engine, &snapshot, failed),
        (None, "~/src\n$ false".to_owned()),
        "a command without output has none"
    );
    assert!(snapshot.sticky_prompt.is_none(), "every prompt is visible");
}

#[test]
fn output_without_a_final_newline_stays_in_its_block() {
    let mut engine = engine(30, 10);
    process(
        &mut engine,
        &format!(
            "{}$ {}printf x\r\n{}x{}",
            mark("A"),
            mark("B"),
            mark("C"),
            mark("D;0")
        ),
    );
    // A is a fresh-line prompt start, so the prompt starts below the x.
    process(&mut engine, &prompt());
    let snapshot = engine.snapshot().unwrap();
    let block = snapshot.commands[0];
    assert_eq!(
        block_text(&engine, &snapshot, &block).0.as_deref(),
        Some("x")
    );
}

#[test]
fn a_running_command_ends_at_the_cursor() {
    let mut engine = engine(30, 10);
    process(
        &mut engine,
        &format!(
            "{}$ {}make\r\n{}one\r\ntwo",
            mark("A"),
            mark("B"),
            mark("C")
        ),
    );
    let snapshot = engine.snapshot().unwrap();
    let block = snapshot.commands[0];
    assert_eq!(
        block_text(&engine, &snapshot, &block),
        (Some("one\ntwo".to_owned()), "$ make\none\ntwo".to_owned())
    );
}

/// Ten commands of `lines` output each in a 30-by-8 terminal, and a prompt.
fn history(lines: usize) -> TerminalEngine {
    let mut engine = engine(30, 8);
    for command_number in 1..=10 {
        let output: Vec<String> = (1..=lines)
            .map(|line| format!("out {command_number}.{line}"))
            .collect();
        let output: Vec<&str> = output.iter().map(String::as_str).collect();
        let end = if command_number % 2 == 0 {
            "D;1"
        } else {
            "D;0"
        };
        process(
            &mut engine,
            &command(&format!("cmd {command_number}"), &output, end),
        );
    }
    process(&mut engine, &prompt());
    engine
}

#[test]
fn the_sticky_prompt_shows_the_command_whose_output_fills_the_top_row() {
    let mut engine = history(20);
    // Into command 4's output: its prompt is above the viewport.
    let live = engine.snapshot().unwrap();
    let total = live.history_size + usize::from(live.size.rows);
    // Each command takes 2 prompt rows and 20 output rows; command 4's
    // output starts on screen row 3 * 22 + 2. Put screen row 3 * 22 + 10 at
    // the viewport's top.
    let top_row = 3 * 22 + 10;
    let offset = total - usize::from(live.size.rows) - top_row;
    engine.scroll(ScrollCommand::Absolute(offset)).unwrap();
    let snapshot = engine.snapshot().unwrap();
    let sticky = snapshot.sticky_prompt.as_ref().expect("a sticky prompt");
    assert_eq!(sticky.sequence, 4);
    assert_eq!(sticky_text(&snapshot).unwrap(), ["~/src", "$ cmd 4"]);
    assert_eq!(sticky.input, 1, "the command line is the second row");
    assert_eq!(
        sticky.rows[1].cells[0].foreground,
        CellColor::Indexed(2),
        "rows keep their colors"
    );
    assert_eq!(sequences(&snapshot), [4]);
    let (output, _) = block_text(&engine, &snapshot, &snapshot.commands[0]);
    let output = output.unwrap();
    assert!(output.starts_with("out 4.1\n") && output.ends_with("out 4.20"));

    // The same header keeps its rows in the next snapshot.
    engine.scroll(ScrollCommand::Relative(1)).unwrap();
    let next = engine.snapshot().unwrap();
    let rows = |snapshot: &TerminalSnapshot| {
        snapshot.sticky_prompt.as_ref().unwrap().rows.clone()
    };
    assert!(
        rows(&snapshot)
            .iter()
            .zip(rows(&next))
            .all(|(a, b)| std::sync::Arc::ptr_eq(a, &b))
    );

    // At a prompt row there is no header, and the next block starts in
    // view.
    let at_prompt = total - usize::from(live.size.rows) - 4 * 22;
    engine.scroll(ScrollCommand::Absolute(at_prompt)).unwrap();
    let snapshot = engine.snapshot().unwrap();
    assert!(snapshot.sticky_prompt.is_none());
    assert_eq!(sequences(&snapshot), [5]);
}

#[test]
fn prompt_jumps_put_each_prompt_at_the_top() {
    let mut engine = history(20);
    engine.scroll(ScrollCommand::Prompt(-1)).unwrap();
    let snapshot = engine.snapshot().unwrap();
    assert!(snapshot.sticky_prompt.is_none());
    assert_eq!(sequences(&snapshot), [10]);
    assert_eq!(
        snapshot.commands[0].prompt,
        snapshot.viewport.bottom_offset + usize::from(snapshot.size.rows) - 1,
        "command 10's prompt is the viewport's top row"
    );
    engine.scroll(ScrollCommand::Prompt(-3)).unwrap();
    assert_eq!(sequences(&engine.snapshot().unwrap()), [7]);
    engine.scroll(ScrollCommand::Prompt(10)).unwrap();
    assert_eq!(engine.snapshot().unwrap().viewport.bottom_offset, 0);
}

#[test]
fn blocks_follow_wrapped_output_through_resizes() {
    let mut engine = engine(40, 12);
    let long = "0123456789".repeat(3);
    process(&mut engine, &command("cat", &[&long, "end"], "D;0"));
    process(&mut engine, &prompt());
    for columns in [12, 7, 40] {
        engine
            .resize(
                GridSize::clamped(columns, 12),
                CellSize {
                    width: 9,
                    height: 17,
                },
            )
            .unwrap();
        engine.command_update();
        let snapshot = engine.snapshot().unwrap();
        let block = snapshot.commands[0];
        assert_eq!(
            block_text(&engine, &snapshot, &block).0,
            Some(format!("{long}\nend")),
            "at {columns} columns"
        );
    }
}

#[test]
fn erased_or_evicted_prompts_leave_no_block() {
    // Deleting the prompt's row in place leaves the record's anchor on the
    // row that replaced it, which is not a prompt.
    let mut engine = engine(30, 8);
    process(&mut engine, &command("ls", &["a"], "D;0"));
    process(&mut engine, "\x1b[1;1H\x1b[M\x1b[8;1H");
    let snapshot = engine.snapshot().unwrap();
    assert_eq!(engine.anchored_text(1).as_deref(), Some("$ ls"));
    assert!(snapshot.commands.is_empty(), "{:?}", snapshot.commands);

    // History that keeps a command's output but not its prompt has no
    // block and no header for that output. Ghostty prunes whole pages, so
    // the output spans several.
    let mut engine = history(500);
    engine.set_history_rows(30).unwrap();
    engine.command_update();
    engine.scroll(ScrollCommand::Absolute(usize::MAX)).unwrap();
    let snapshot = engine.snapshot().unwrap();
    let top = text(
        &engine,
        &snapshot,
        snapshot.history_size + usize::from(snapshot.size.rows) - 1,
        snapshot.history_size + usize::from(snapshot.size.rows) - 1,
    );
    let owner: u64 = top
        .strip_prefix("out ")
        .and_then(|rest| rest.split('.').next())
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("the top row is output: {top:?}"));
    // The top row is output, so its prompt left history.
    assert!(snapshot.sticky_prompt.is_none());
    assert!(!sequences(&snapshot).contains(&owner));
}

#[test]
fn an_older_anchor_moved_onto_a_newer_prompt_gives_way_to_it() {
    let mut engine = engine(30, 8);
    process(
        &mut engine,
        &format!(
            "{}$ {}one\r\n{}a\r\n{}",
            mark("A"),
            mark("B"),
            mark("C"),
            mark("D;1")
        ),
    );
    // A program deletes the top rows in place, then the shell draws its
    // next prompt on the top row, where record 1's anchor stayed.
    process(&mut engine, "\x1b[1;1H\x1b[M\x1b[M\x1b[1;1H");
    process(
        &mut engine,
        &format!(
            "{}$ {}two\r\n{}b\r\n{}",
            mark("A"),
            mark("B"),
            mark("C"),
            mark("D;0")
        ),
    );
    process(&mut engine, &prompt());
    assert_eq!(engine.anchored_text(1), engine.anchored_text(2));
    let snapshot = engine.snapshot().unwrap();
    assert_eq!(
        sequences(&snapshot),
        [2],
        "record 1's failure must not tint it"
    );
}

#[test]
fn without_history_anchors_that_share_a_prompt_row_keep_the_newest() {
    let mut engine = engine(30, 6);
    engine.set_scrollback(ScrollbackLimit::NONE).unwrap();
    for number in 1..=8 {
        process(&mut engine, &command(&format!("c{number}"), &["o"], "D;0"));
    }
    process(&mut engine, &prompt());
    let snapshot = engine.snapshot().unwrap();
    // Every block starts on its own prompt row, newest last, and its text
    // is the command it names.
    let mut previous = usize::MAX;
    for block in snapshot.commands.iter() {
        assert!(block.prompt < previous, "{:?}", snapshot.commands);
        previous = block.prompt;
        let (_, whole) = block_text(&engine, &snapshot, block);
        assert!(
            whole.contains(&format!("$ c{}", block.sequence)),
            "block {} reads {whole:?}",
            block.sequence
        );
    }
    assert_eq!(sequences(&snapshot).last(), Some(&8));
}

#[test]
fn the_alternate_screen_has_no_blocks_or_header() {
    let mut engine = history(20);
    engine.scroll(ScrollCommand::Relative(30)).unwrap();
    assert!(engine.snapshot().unwrap().sticky_prompt.is_some());
    process(&mut engine, "\x1b[?1049h");
    let snapshot = engine.snapshot().unwrap();
    assert!(snapshot.commands.is_empty());
    assert!(snapshot.sticky_prompt.is_none());
}

#[test]
fn a_palette_override_recolors_the_sticky_prompt() {
    let mut engine = history(20);
    engine.scroll(ScrollCommand::Relative(30)).unwrap();
    let before = engine.snapshot().unwrap();
    let input = |snapshot: &TerminalSnapshot| {
        let sticky = snapshot.sticky_prompt.as_ref().unwrap();
        sticky.rows[sticky.input].cells[0].foreground
    };
    assert_eq!(input(&before), CellColor::Indexed(2));
    process(&mut engine, "\x1b]4;2;rgb:12/34/56\x07");
    let after = engine.snapshot().unwrap();
    assert!(matches!(input(&after), CellColor::Rgb(_)));
}

#[test]
fn a_header_click_jump_lands_on_its_prompt_after_more_output() {
    let mut engine = history(20);
    engine.scroll(ScrollCommand::Relative(40)).unwrap();
    let shown = engine.snapshot().unwrap();
    let sequence = shown.sticky_prompt.as_ref().unwrap().sequence;
    // Output arrives between the paint and the click, moving every row's
    // distance from the live bottom.
    process(&mut engine, &command("late", &["x", "y", "z"], "D;0"));
    process(&mut engine, &prompt());
    engine.scroll(ScrollCommand::Prompt(-1)).unwrap();
    let snapshot = engine.snapshot().unwrap();
    assert!(snapshot.sticky_prompt.is_none());
    let top =
        snapshot.viewport.bottom_offset + usize::from(snapshot.size.rows) - 1;
    assert_eq!(
        snapshot
            .commands
            .first()
            .map(|block| (block.sequence, block.prompt)),
        Some((sequence, top))
    );
}

#[test]
fn a_newer_prompt_drawn_above_an_older_one_keeps_its_own_rows() {
    // Cursor addressing draws command 2 low on the screen and then command
    // 3 above it, as a program that repositions the shell's output would.
    let mut engine = engine(30, 12);
    process(&mut engine, &format!("{}\r\n", "x\r\n".repeat(20)));
    process(&mut engine, &command("one", &["a"], "D;1"));
    process(
        &mut engine,
        &format!(
            "\x1b[9;1H{}$ {}two\r\n{}b\r\n{}",
            mark("A"),
            mark("B"),
            mark("C"),
            mark("D;1")
        ),
    );
    process(
        &mut engine,
        &format!(
            "\x1b[3;1H{}$ {}three\r\n{}c\r\n{}\x1b[12;1H",
            mark("A"),
            mark("B"),
            mark("C"),
            mark("D;0")
        ),
    );
    for offset in [0, 2] {
        engine.scroll(ScrollCommand::Absolute(offset)).unwrap();
        let snapshot = engine.snapshot().unwrap();
        let named: Vec<(u64, String)> = snapshot
            .commands
            .iter()
            .map(|block| {
                (block.sequence, block_text(&engine, &snapshot, block).1)
            })
            .collect();
        // Each block starts at its own prompt; the rows the fixture left
        // between them belong to the block above.
        let three = named.iter().find(|(sequence, _)| *sequence == 3);
        assert!(
            three.is_some_and(|(_, text)| text.starts_with("$ three\nc\n")),
            "scrolled {offset}: {named:?}"
        );
        let two = named.iter().find(|(sequence, _)| *sequence == 2);
        assert!(
            two.is_some_and(|(_, text)| text.starts_with("$ two\nb")),
            "scrolled {offset}: {named:?}"
        );
        // Rows run top to bottom, so command 3 comes before command 2.
        assert_eq!(
            snapshot
                .commands
                .iter()
                .map(|block| block.sequence)
                .filter(|sequence| *sequence != 1)
                .collect::<Vec<_>>(),
            [3, 2],
            "scrolled {offset}"
        );
    }
}

#[test]
fn a_sticky_prompt_never_holds_more_rows_than_its_grid() {
    use huterm_wire::update::UpdateEncoder;

    for rows in [1_u16, 2, 3] {
        let mut engine = engine(30, rows);
        // A three-row prompt, then output the viewport can scroll into.
        let output: Vec<String> =
            (1..=30).map(|line| format!("out {line}")).collect();
        let output: Vec<&str> = output.iter().map(String::as_str).collect();
        process(
            &mut engine,
            &format!(
                "{}~/src\r\nmain\r\n$ {}make\r\n{}{}\r\n{}",
                mark("A"),
                mark("B"),
                mark("C"),
                output.join("\r\n"),
                mark("D;0")
            ),
        );
        process(&mut engine, &prompt());
        engine.scroll(ScrollCommand::Relative(10)).unwrap();
        let snapshot = std::sync::Arc::new(engine.snapshot().unwrap());
        let sticky = snapshot.sticky_prompt.as_ref().expect("a sticky prompt");
        assert!(sticky.rows.len() <= usize::from(rows), "{rows} rows");
        assert_eq!(
            sticky_text(&snapshot).unwrap().last().map(String::as_str),
            Some("$ make"),
            "the command line row stays"
        );
        UpdateEncoder::new(huterm_protocol::ViewerId::new(1), 0)
            .next(&snapshot, false)
            .unwrap_or_else(|error| panic!("{rows} rows: {error}"));
    }
}

#[test]
fn the_sticky_prompt_belongs_to_the_nearest_prompt_above_whatever_its_age() {
    // Command 1 is drawn low on the screen, and five newer commands above
    // it; scrolled into command 1's output, the header is command 1's.
    let mut engine = engine(30, 30);
    let at = |row: usize, name: &str, end: &str| {
        format!(
            "\x1b[{row};1H{}$ {}{name}\r\n{}{name} out\r\n{}",
            mark("A"),
            mark("B"),
            mark("C"),
            mark(end)
        )
    };
    process(&mut engine, &at(21, "one", "D;1"));
    for (row, name) in [(3, "two"), (6, "three"), (9, "four"), (12, "five")] {
        process(&mut engine, &at(row, name, "D;0"));
    }
    process(&mut engine, &at(15, "six", "D;0"));
    // Output below command 1 pushes every prompt into history.
    process(&mut engine, "\x1b[30;1H");
    for line in 0..45 {
        process(&mut engine, &format!("\r\nfill {line}"));
    }
    let snapshot = engine.snapshot().unwrap();
    let total = snapshot.history_size + usize::from(snapshot.size.rows);
    // Put the row after command 1's output line at the viewport's top.
    let one_output = 21; // the screen row below "one out" (1-based 22)
    engine
        .scroll(ScrollCommand::Absolute(total - 30 - one_output - 1))
        .unwrap();
    let snapshot = engine.snapshot().unwrap();
    let sticky = snapshot.sticky_prompt.as_ref().expect("a sticky prompt");
    assert_eq!(
        sticky.sequence,
        1,
        "{:?} {:?}",
        sticky_text(&snapshot),
        sequences(&snapshot)
    );
}
