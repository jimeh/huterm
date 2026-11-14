use std::time::{Duration, SystemTime};

use huterm_protocol::{
    Cell, CellColor, CellStyle, CellText, CommandBlock, GridSize, StickyPrompt,
    TerminalId, TerminalModes, Viewport,
};

use super::*;

fn row(text: &str) -> Arc<TerminalRow> {
    Arc::new(TerminalRow {
        cells: text
            .chars()
            .map(|character| Cell {
                text: CellText::from(character.to_string().as_str()),
                foreground: CellColor::DefaultForeground,
                background: CellColor::DefaultBackground,
                style: CellStyle::default(),
            })
            .collect(),
        ..TerminalRow::default()
    })
}

/// A 10-row viewport `offset` rows above live output, with `blocks`.
fn snapshot(offset: usize, blocks: &[CommandBlock]) -> TerminalSnapshot {
    TerminalSnapshot {
        terminal_id: TerminalId::new(1),
        generation: 1,
        revision: 1,
        geometry_revision: 0,
        size: GridSize::clamped(20, 10),
        rows: (0..10).map(|_| row("")).collect(),
        cursor: None,
        modes: TerminalModes::default(),
        viewport: Viewport {
            bottom_offset: offset,
        },
        history_size: 100,
        cursor_color: None,
        default_background: None,
        commands: blocks.into(),
        sticky_prompt: None,
    }
}

fn block(
    sequence: u64,
    prompt: usize,
    output: Option<usize>,
    end: usize,
) -> CommandBlock {
    CommandBlock {
        sequence,
        prompt,
        output,
        end,
    }
}

fn record(sequence: u64, state: CommandState) -> CommandRecord {
    CommandRecord {
        sequence,
        text: None,
        state,
        started_at: SystemTime::UNIX_EPOCH,
        finished_at: None,
        duration: Some(Duration::from_millis(4210)),
    }
}

const fn exited(code: Option<i32>) -> CommandState {
    CommandState::Finished { exit_code: code }
}

#[test]
fn only_failures_are_tinted_and_unknown_outcomes_are_never_marked() {
    // At live output the viewport's top row is 9 rows above the bottom.
    let blocks = [
        block(1, 9, Some(8), 7),
        block(2, 6, Some(5), 5),
        block(3, 4, Some(3), 3),
        block(4, 2, None, 2),
        block(5, 1, Some(0), 0),
        block(6, 9, None, 9),
    ];
    let records = [
        record(1, exited(Some(0))),
        record(2, exited(Some(130))),
        record(3, exited(None)),
        record(4, CommandState::Abandoned),
        record(5, CommandState::Running),
    ];
    let snapshot = snapshot(0, &blocks);
    assert_eq!(
        marks(&snapshot, &records, CommandStatusStyle::Tint),
        Marks {
            tints: std::iter::once(3..5).collect(),
            ..Marks::default()
        },
        "exit 130 is a failure; no status, abandoned, running, and a \
         record the status lacks are unknown"
    );
    assert_eq!(
        marks(&snapshot, &records, CommandStatusStyle::TintMarks),
        Marks {
            tints: std::iter::once(3..5).collect(),
            prompts: vec![(0..1, Outcome::Succeeded), (3..4, Outcome::Failed),],
            ..Marks::default()
        }
    );
    assert_eq!(
        marks(&snapshot, &records, CommandStatusStyle::Bar),
        Marks {
            bars: std::iter::once(3..5).collect(),
            ..Marks::default()
        }
    );
    assert!(marks(&snapshot, &records, CommandStatusStyle::Off).is_empty());
}

#[test]
fn marks_are_clipped_to_the_viewport() {
    let records = [record(7, exited(Some(1)))];
    // Scrolled 50 rows up, the viewport shows rows 59 down to 50 above the
    // bottom. The block starts above it and ends inside it.
    let inside = snapshot(50, &[block(7, 70, Some(69), 55)]);
    assert_eq!(
        marks(&inside, &records, CommandStatusStyle::TintMarks),
        Marks {
            tints: std::iter::once(0..5).collect(),
            ..Marks::default()
        },
        "the prompt mark is above the viewport"
    );
    let below = snapshot(50, &[block(7, 58, Some(57), 20)]);
    assert_eq!(
        marks(&below, &records, CommandStatusStyle::Bar).bars,
        std::iter::once(1..10).collect::<Vec<_>>()
    );
}

#[test]
fn the_header_shows_the_command_row_or_the_whole_prompt() {
    // Scrolled 30 rows up, the top row is 39 rows above the bottom, inside
    // command 4's output.
    let mut shown = snapshot(30, &[block(4, 60, Some(58), 20)]);
    shown.sticky_prompt = Some(StickyPrompt {
        sequence: 4,
        rows: vec![row("~/src"), row("$ make")],
        input: 1,
    });
    for style in [StickyPromptStyle::Status, StickyPromptStyle::Command] {
        let header = header(&shown, style).unwrap();
        assert_eq!((header.sequence, header.input), (4, 0));
        assert_eq!(header.rows.len(), 1);
        assert_eq!(row_text(&header.rows[0]), "$ make");
    }
    let whole = header(&shown, StickyPromptStyle::Prompt).unwrap();
    assert_eq!(
        whole
            .rows
            .iter()
            .map(|row| row_text(row))
            .collect::<Vec<_>>(),
        ["~/src", "$ make"]
    );
    assert_eq!(whole.input, 1);
    assert!(header(&shown, StickyPromptStyle::Off).is_none());
    // When the command ends on the top row, two header rows would cover the
    // next command's prompt, so the whole prompt gives way to its command
    // row.
    let mut ending = shown.clone();
    ending.commands = [block(4, 60, Some(58), 39)].into();
    let row_only = header(&ending, StickyPromptStyle::Prompt).unwrap();
    assert_eq!(row_only.rows.len(), 1);
    assert_eq!(row_text(&row_only.rows[0]), "$ make");

    let mut short = shown.clone();
    short.size = GridSize::clamped(20, STICKY_MIN_ROWS - 1);
    assert!(header(&short, StickyPromptStyle::Status).is_none());
    let mut alternate = shown;
    alternate.modes.alternate_screen = true;
    assert!(header(&alternate, StickyPromptStyle::Status).is_none());
}

#[test]
fn the_status_reads_how_the_command_stands() {
    let at = |seconds: u64, millis: u64| {
        SystemTime::UNIX_EPOCH
            + Duration::from_secs(seconds)
            + Duration::from_millis(millis)
    };
    let running = chip(&record(1, CommandState::Running), at(75, 250)).unwrap();
    assert_eq!(running.text, "running \u{b7} 1:15");
    assert_eq!(running.spoken, "running for 1:15");
    assert_eq!(running.outcome, None);
    assert_eq!(running.next_tick, Some(Duration::from_millis(750)));
    let long = chip(&record(1, CommandState::Running), at(3725, 0)).unwrap();
    assert_eq!(long.text, "running \u{b7} 1:02:05");

    let now = at(0, 0);
    let succeeded = chip(&record(1, exited(Some(0))), now).unwrap();
    assert_eq!(succeeded.text, "\u{2713} 4.2s");
    assert_eq!(succeeded.outcome, Some(Outcome::Succeeded));
    let failed = chip(&record(1, exited(Some(101))), now).unwrap();
    assert_eq!(failed.text, "\u{2717} exit 101 \u{b7} 4.2s");
    assert_eq!(failed.spoken, "failed with exit status 101 after 4.2s");
    assert_eq!(failed.next_tick, None);
    let unknown = chip(&record(1, exited(None)), now).unwrap();
    assert_eq!((unknown.text.as_str(), unknown.outcome), ("4.2s", None));
    assert!(chip(&record(1, CommandState::Abandoned), now).is_none());

    let mut slow = record(1, exited(Some(0)));
    slow.duration = Some(Duration::from_secs(185));
    assert_eq!(chip(&slow, now).unwrap().text, "\u{2713} 3:05");
}

#[test]
fn the_label_names_the_action_the_command_and_its_state() {
    let header = Header {
        sequence: 1,
        rows: vec![row("~/src  "), row("$ make  ")],
        input: 1,
    };
    let failed = chip(&record(1, exited(Some(2))), SystemTime::UNIX_EPOCH);
    assert_eq!(
        label(&header, failed.as_ref()),
        "Scroll to prompt: $ make, failed with exit status 2 after 4.2s"
    );
    assert_eq!(label(&header, None), "Scroll to prompt: $ make");
}

/// A row of `columns` cells holding `text` from the left.
fn padded(text: &str, columns: usize) -> TerminalRow {
    let mut row = (*row(text)).clone();
    while row.cells.len() < columns {
        row.cells.push(Cell {
            text: CellText::EMPTY,
            foreground: CellColor::DefaultForeground,
            background: CellColor::DefaultBackground,
            style: CellStyle::default(),
        });
    }
    row
}

fn fitted(text: &str, width: usize, columns: usize) -> Option<String> {
    fit_row(&padded(text, width), columns).map(|row| row_text(&row))
}

#[test]
fn the_command_row_makes_room_for_the_status() {
    // A short command fits, its right prompt is dropped either way.
    assert_eq!(fitted("$ ls", 30, 20), None);
    assert_eq!(
        fitted("$ ls                     12:04:31 ", 34, 30).as_deref(),
        Some("$ ls")
    );
    assert_eq!(
        fitted("$ ls          me@box [12:04:31] ", 32, 30).as_deref(),
        Some("$ ls")
    );
    // A long command line is cut after its last whole word, with an
    // ellipsis, never leaving a fragment.
    assert_eq!(
        fitted("$ make build --release --locked", 40, 20).as_deref(),
        Some("$ make build\u{2026}")
    );
    // One word that fills the room is cut mid-word.
    assert_eq!(
        fitted("$ abcdefghijklmnopqrstuvwxyz", 30, 10).as_deref(),
        Some("$ abcdefg\u{2026}")
    );
    // Text that ends at the edge with no gap before it is the command
    // itself, not a right prompt.
    assert_eq!(
        fitted("$ echo one two three four five", 31, 20).as_deref(),
        Some("$ echo one two\u{2026}")
    );
}

#[test]
fn a_wrapped_command_row_or_a_wide_character_is_not_a_right_prompt() {
    // A command long enough to wrap fills its row; its double space is part
    // of the command, so it is cut with an ellipsis, not dropped.
    let mut wrapped = padded("$ echo a  bbbbbbbbbb", 20);
    wrapped.wrapped = true;
    assert_eq!(
        fit_row(&wrapped, 14).map(|row| row_text(&row)).as_deref(),
        Some("$ echo a\u{2026}")
    );
    // A wide character's spacer and one space are not a two-blank gap.
    let mut wide = padded("$ ls ", 12);
    wide.cells[5].text = CellText::from("\u{754c}");
    wide.cells[5].style.wide = true;
    wide.cells[6].style.wide_spacer = true;
    wide.cells[7].text = CellText::BLANK;
    wide.cells[8].text = CellText::from("x");
    wide.cells[9].text = CellText::from("y");
    wide.cells[10].text = CellText::from("z");
    assert_eq!(fit_row(&wide, 12), None, "it fits as it is");
}
