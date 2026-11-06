//! Command records built from OSC 133 output, observed through the engine
//! facade as the runtime sees them after each operation.

use std::fmt::Write as _;
use std::sync::Arc;

use huterm_protocol::{
    COMMAND_RECORD_CAPACITY, CellSize, CommandRecord, CommandState, GridSize,
    TerminalId, TerminalPresentation,
};

use super::TerminalEngine;
use super::records::live;

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

/// An OSC 133 marker with a BEL terminator.
fn mark(body: &str) -> String {
    format!("\x1b]133;{body}\x07")
}

/// One complete command: prompt, input, output, and end.
fn command(prompt: &str, output: &str, end: &str) -> String {
    format!(
        "{}{prompt}{}\r\n{}{output}\r\n{}",
        mark("A"),
        mark("B"),
        mark("C"),
        mark(end)
    )
}

fn process(engine: &mut TerminalEngine, output: &str) {
    engine.process(output.as_bytes()).unwrap();
}

fn states(records: &[CommandRecord]) -> Vec<(u64, CommandState)> {
    records
        .iter()
        .map(|record| (record.sequence, record.state))
        .collect()
}

const fn finished(exit_code: Option<i32>) -> CommandState {
    CommandState::Finished { exit_code }
}

/// The visible rows, as text.
fn screen(engine: &mut TerminalEngine) -> Vec<String> {
    engine
        .snapshot()
        .unwrap()
        .rows
        .iter()
        .map(|row| row.cells.iter().map(|cell| cell.text.displayed()).collect())
        .collect()
}

#[test]
fn commands_finish_with_their_exit_code_or_none() {
    let mut engine = engine(40, 6);
    process(&mut engine, &command("$ ", "ok", "D;0"));
    let records = engine.command_update().expect("a record was created");
    let [record] = &records[..] else {
        panic!("one record expected: {records:?}");
    };
    assert_eq!(record.sequence, 1);
    assert_eq!(record.state, finished(Some(0)));
    assert_eq!(record.text, None, "no command line was sent");
    assert!(record.finished_at.is_some_and(|at| at >= record.started_at));
    assert!(record.duration.is_some());
    assert!(engine.command_update().is_none(), "nothing changed since");

    process(&mut engine, &command("$ ", "no", "D;130"));
    process(&mut engine, &command("$ ", "?", "D"));
    process(&mut engine, &command("$ ", "?", "D;bad"));
    assert_eq!(
        states(&engine.command_update().unwrap()),
        [
            (1, finished(Some(0))),
            (2, finished(Some(130))),
            (3, finished(None)),
            (4, finished(None)),
        ]
    );
}

#[test]
fn a_new_prompt_or_command_abandons_a_running_record() {
    let mut engine = engine(40, 6);
    process(&mut engine, &format!("{}$ {}", mark("A"), mark("C")));
    let running = engine.command_update().unwrap();
    assert_eq!(states(&running), [(1, CommandState::Running)]);
    assert!(running[0].finished_at.is_none() && running[0].duration.is_none());
    // A prompt with no D, as after an inner shell starts.
    process(&mut engine, &mark("A"));
    let abandoned = engine.command_update().unwrap();
    assert_eq!(states(&abandoned), [(1, CommandState::Abandoned)]);
    assert!(abandoned[0].finished_at.is_some());
    assert!(abandoned[0].duration.is_some());
    // The outer D is then ignored.
    process(&mut engine, &mark("D;0"));
    assert!(engine.command_update().is_none());
    // A C without a prompt or D abandons the running record too.
    process(&mut engine, &format!("{}{}", mark("C"), mark("C")));
    assert_eq!(
        states(&engine.command_update().unwrap()),
        [
            (1, CommandState::Abandoned),
            (2, CommandState::Abandoned),
            (3, CommandState::Running),
        ]
    );
}

/// Without OSC 133 `aid`, core cannot tell which shell sent a D. When an
/// inner shell's last command is still running as the inner shell ends, as
/// with `exit` or a dropped `ssh` connection, the outer D finishes that
/// command with the outer command's status.
#[test]
fn an_outer_end_finishes_a_nested_command_that_is_still_running() {
    let mut engine = engine(40, 6);
    process(&mut engine, &format!("{}$ {}", mark("A"), mark("C")));
    // The inner shell's prompt, then `exit`, whose C has no D.
    process(&mut engine, &format!("{}> {}", mark("A"), mark("C")));
    process(&mut engine, &mark("D;255"));
    assert_eq!(
        states(&engine.command_update().unwrap()),
        [(1, CommandState::Abandoned), (2, finished(Some(255)))]
    );
}

#[test]
fn prompt_redraws_and_repeated_ends_make_no_extra_record() {
    let mut engine = engine(40, 6);
    process(&mut engine, &command("$ ", "out", "D;0"));
    // Readline redraws resend A, and a full redraw resends D.
    process(
        &mut engine,
        &format!(
            "{}$ \r{}$ {}{}",
            mark("A"),
            mark("A"),
            mark("D;1"),
            mark("B")
        ),
    );
    assert_eq!(
        states(&engine.command_update().unwrap()),
        [(1, finished(Some(0)))]
    );
    // Right and continuation prompts never move the anchor or end a record.
    process(
        &mut engine,
        &format!(
            "{}echo \\\r\n{}> {}{}",
            mark("C"),
            mark("A;k=r"),
            mark("A;k=c"),
            mark("A;k=s")
        ),
    );
    assert_eq!(
        states(&engine.command_update().unwrap()),
        [(1, finished(Some(0))), (2, CommandState::Running)]
    );
}

#[test]
fn ctrl_l_at_a_prompt_keeps_anchors_on_their_text() {
    let mut engine = engine(20, 4);
    process(&mut engine, &command("one$ ", "out", "D;0"));
    process(&mut engine, &format!("{}two$ {}", mark("A"), mark("B")));
    assert_eq!(engine.anchored_text(1).as_deref(), Some("one$"));
    // A clear-screen at the prompt, then the shell redraws it.
    process(
        &mut engine,
        &format!("\x1b[H\x1b[2J{}two$ {}", mark("A"), mark("B")),
    );
    assert_eq!(engine.anchored_text(1).as_deref(), Some("one$"));
    assert_eq!(
        states(&engine.command_update().unwrap()),
        [(1, finished(Some(0)))]
    );
}

#[test]
fn the_alternate_screen_ignores_prompts_and_starts_but_accepts_ends() {
    let mut engine = engine(40, 6);
    process(&mut engine, &format!("{}$ {}", mark("A"), mark("C")));
    // A full-screen program that sends markers itself, or crashed before
    // leaving the alternate screen when the shell reports.
    process(
        &mut engine,
        &format!("\x1b[?1049h{}{}{}", mark("A"), mark("C"), mark("D;2")),
    );
    assert_eq!(
        states(&engine.command_update().unwrap()),
        [(1, finished(Some(2)))]
    );
}

#[test]
fn markers_use_the_screen_active_when_they_were_parsed() {
    let mut engine = engine(40, 6);
    // Entering and leaving the alternate screen in the same chunk as the C
    // must not hide that the C was parsed there.
    process(
        &mut engine,
        &format!("\x1b[?1049h{}{}x\x1b[?1049l", mark("A"), mark("C")),
    );
    assert!(engine.command_update().is_none());
    // And a C parsed on the primary screen counts even when the chunk ends
    // on the alternate screen.
    process(
        &mut engine,
        &format!("{}$ {}\x1b[?1049hfull", mark("A"), mark("C")),
    );
    assert_eq!(
        states(&engine.command_update().unwrap()),
        [(1, CommandState::Running)]
    );
}

#[test]
fn anchors_mark_the_prompt_row_the_marker_was_parsed_at() {
    let mut engine = engine(20, 4);
    // Lots of output follows the markers in the same chunk.
    let mut output = command("first$ ", "result", "D;0");
    output.push_str(&command("second$ ", "more", "D;0"));
    for line in 0..10 {
        let _ = write!(output, "line {line}\r\n");
    }
    process(&mut engine, &output);
    assert_eq!(engine.anchored_text(1).as_deref(), Some("first$"));
    assert_eq!(engine.anchored_text(2).as_deref(), Some("second$"));
    // A C without a prompt anchors at the cursor's row.
    process(&mut engine, &format!("run{}", mark("C")));
    assert_eq!(engine.anchored_text(3).as_deref(), Some("run"));
}

#[test]
fn a_reset_between_prompt_and_command_anchors_at_the_command() {
    let mut engine = engine(20, 4);
    // The prompt is below the row the reset leaves the cursor on.
    process(
        &mut engine,
        &format!("x\r\ny\r\n{}old$ \x1bcnew{}", mark("A"), mark("C")),
    );
    assert_eq!(
        states(&engine.command_update().unwrap()),
        [(1, CommandState::Running)]
    );
    assert_eq!(engine.anchored_text(1).as_deref(), Some("new"));
}

#[test]
fn markers_split_across_chunks_byte_by_byte_match_one_chunk() {
    let output = format!(
        "{}{}{}$ {}\x1b]133;C;cmdline_url=ls%20-l\x1b\\",
        command("$ ", "one", "D;0"),
        command("$ ", "two", "D;7"),
        mark("A"),
        mark("B"),
    );
    let mut whole = engine(20, 6);
    process(&mut whole, &output);
    let mut split = engine(20, 6);
    for byte in output.as_bytes() {
        split.process(std::slice::from_ref(byte)).unwrap();
    }
    let (whole_records, split_records) = (
        whole.command_update().unwrap(),
        split.command_update().unwrap(),
    );
    assert_eq!(
        states(&split_records),
        [
            (1, finished(Some(0))),
            (2, finished(Some(7))),
            (3, CommandState::Running)
        ]
    );
    assert_eq!(states(&whole_records), states(&split_records));
    assert_eq!(split_records[2].text.as_deref(), Some("ls -l"));
    for sequence in 1..=3 {
        assert_eq!(
            split.anchored_text(sequence),
            whole.anchored_text(sequence),
            "record {sequence}"
        );
    }
    assert_eq!(screen(&mut split), screen(&mut whole));
}

#[test]
fn command_lines_are_stripped_of_controls_and_never_change_output() {
    // Each A here is at the start of a line, where its fresh line moves
    // nothing.
    let marked = format!(
        "{}a{}b\x1b]133;C;cmdline_url=echo%07%1b%c2%9b%7f%20hi\x07c{}d",
        mark("A"),
        mark("B"),
        mark("D;0"),
    );
    let mut with_marks = engine(20, 4);
    process(&mut with_marks, &marked);
    let records = with_marks.command_update().unwrap();
    assert_eq!(records[0].text.as_deref(), Some("echo hi"));
    // Malformed markers are ignored or rejected without printing.
    process(
        &mut with_marks,
        &format!("e{}f{}g\x1b]133;C;cmdline_url=\x07h", mark("Q"), mark(";")),
    );
    let records = with_marks.command_update().unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].text, None, "an empty command line is no text");
    let mut plain = engine(20, 4);
    process(&mut plain, "abcdefgh");
    assert_eq!(screen(&mut with_marks), screen(&mut plain));
}

#[test]
fn a_command_line_at_the_parser_limit_records_and_beyond_it_does_not() {
    // The pinned parser captures 2,048 bytes of an OSC 133 after its
    // `133;` and rejects a longer one as a whole.
    const LIMIT: usize = 2048;
    let options = "C;cmdline_url=";
    for (captured, recorded) in [(LIMIT, true), (LIMIT + 1, false)] {
        let mut engine = engine(20, 4);
        let text = "x".repeat(captured - options.len());
        process(
            &mut engine,
            &format!("{}\x1b]133;{options}{text}\x07after", mark("A")),
        );
        let records = engine.command_update();
        if recorded {
            let records = records.expect("a record at the limit");
            assert_eq!(records[0].text.as_deref(), Some(text.as_str()));
        } else {
            assert!(records.is_none(), "no record beyond the limit");
            // The D that would end it is then ignored.
            process(&mut engine, &mark("D;0"));
            assert!(engine.command_update().is_none());
        }
        assert!(screen(&mut engine)[0].starts_with("after"), "{captured}");
    }
}

#[test]
fn dense_markers_in_one_chunk_hold_at_most_one_anchor_beyond_the_cap() {
    let mut engine = engine(40, 6);
    live::reset_peak();
    let (before, _) = live::counts();
    let mut output = String::new();
    for _ in 0..2000 {
        output.push_str(&command("$ ", "o", "D;0"));
        output.push_str(&mark("A"));
    }
    process(&mut engine, &output);
    let (live, peak) = live::counts();
    assert!(
        peak - before <= COMMAND_RECORD_CAPACITY + 1,
        "{} live references at once",
        peak - before
    );
    let records = engine.command_update().unwrap();
    assert_eq!(records.len(), COMMAND_RECORD_CAPACITY);
    assert_eq!(records[0].sequence, 2000 - 255);
    assert_eq!(records[255].sequence, 2000);
    // Records the cap dropped freed their references; a pending prompt
    // anchor remains.
    assert!(live - before <= COMMAND_RECORD_CAPACITY + 1);
    drop(engine);
    assert_eq!(live::counts().0, before);
}

#[test]
fn reflow_keeps_anchors_on_the_same_text() {
    let mut engine = engine(30, 4);
    process(&mut engine, &command("first prompt$ ", "out", "D;0"));
    process(&mut engine, &command("second prompt$ ", "out", "D;0"));
    for line in 0..20 {
        process(&mut engine, &format!("filler {line}\r\n"));
    }
    let cell = CellSize {
        width: 9,
        height: 17,
    };
    engine.resize(GridSize::clamped(8, 4), cell).unwrap();
    assert_eq!(engine.anchored_text(1).as_deref(), Some("first pr"));
    assert_eq!(engine.anchored_text(2).as_deref(), Some("second p"));
    engine.resize(GridSize::clamped(60, 4), cell).unwrap();
    assert_eq!(engine.anchored_text(1).as_deref(), Some("first prompt$"));
    assert_eq!(engine.anchored_text(2).as_deref(), Some("second prompt$"));
    assert!(
        engine
            .command_update()
            .is_some_and(|records| records.len() == 2)
    );
}

#[test]
fn eviction_removes_records_whose_rows_left_history_anywhere_in_the_list() {
    let mut engine = engine(20, 4);
    // Cursor addressing puts record 1 on the bottom row and record 2 on
    // the top one, so record 2 scrolls into history first.
    process(
        &mut engine,
        &format!(
            "\x1b[4;1H{}one{}\x1b[1;1H{}two{}",
            mark("A"),
            mark("C"),
            mark("A"),
            mark("C")
        ),
    );
    process(&mut engine, &format!("\x1b[4;1H\r\n{}", mark("D;0")));
    process(&mut engine, &command("three$ ", "", "D;0"));
    assert_eq!(engine.command_update().unwrap().len(), 3);
    engine.clear_history().unwrap();
    assert_eq!(
        states(&engine.command_update().unwrap()),
        [(1, CommandState::Abandoned), (3, finished(Some(0)))]
    );
    assert_eq!(engine.anchored_text(1).as_deref(), Some("one"));
}

#[test]
fn eviction_keeps_the_newest_record_and_shows_a_final_state_once() {
    let mut engine = engine(20, 4);
    process(&mut engine, &format!("{}one$ {}", mark("A"), mark("C")));
    for line in 0..6 {
        process(&mut engine, &format!("out {line}\r\n"));
    }
    // The chunk that supersedes record 1 also erases its rows.
    process(&mut engine, &format!("two$ {}\x1b[3J", mark("C")));
    assert_eq!(
        states(&engine.command_update().unwrap()),
        [(1, CommandState::Abandoned), (2, CommandState::Running)],
        "a record that just left Running is shown once"
    );
    process(&mut engine, "x");
    assert_eq!(
        states(&engine.command_update().unwrap()),
        [(2, CommandState::Running)]
    );
    // The newest record stays after its rows leave history.
    for line in 0..6 {
        process(&mut engine, &format!("more {line}\r\n"));
    }
    process(&mut engine, &mark("D;4"));
    // The runtime publishes after every operation.
    assert!(engine.command_update().is_some());
    engine.clear_history().unwrap();
    assert_eq!(engine.anchored_text(2), None, "its rows are gone");
    assert!(engine.command_update().is_none(), "the newest record stays");
    assert_eq!(states(&engine.commands()), [(2, finished(Some(4)))]);
    // So does it with no history at all.
    engine
        .set_scrollback(huterm_protocol::ScrollbackLimit::NONE)
        .unwrap();
    process(&mut engine, &command("three$ ", "", "D;5"));
    for line in 0..6 {
        process(&mut engine, &format!("gone {line}\r\n"));
    }
    assert_eq!(
        engine
            .commands()
            .last()
            .map(|record| (record.sequence, record.state)),
        Some((3, finished(Some(5))))
    );
}

#[test]
fn the_cap_keeps_the_newest_records_and_their_order() {
    let mut engine = engine(20, 4);
    for _ in 0..COMMAND_RECORD_CAPACITY + 44 {
        process(&mut engine, &command("$ ", "", "D;0"));
    }
    let records = engine.command_update().unwrap();
    assert_eq!(records.len(), COMMAND_RECORD_CAPACITY);
    assert!(
        records.iter().map(|record| record.sequence).eq(45..=300),
        "the newest {COMMAND_RECORD_CAPACITY} remain in order"
    );
    let same: Arc<[CommandRecord]> = engine.commands();
    assert_eq!(same, records);
}
