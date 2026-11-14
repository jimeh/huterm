//! Contracts for resolving grid references across pages of history: the
//! cell addressing [`crate::RowRef`] relies on, and screen points resolving
//! to the cells viewport points do from either end of the scrollback.

use std::fmt::Write as _;

use crate::{Error, Fill, Options, Point, Scroll, Terminal};

const COLUMNS: u16 = 80;
const ROWS: u16 = 4;
/// Several pages of history at 80 columns, where a page holds about 570 rows.
const LINES: usize = 3000;

/// A terminal whose screen rows each begin with their own line number,
/// with some wide, combining, and hyperlinked cells, and history that spans
/// several pages.
fn paged_terminal() -> Terminal<()> {
    let mut terminal = Terminal::new(
        Options {
            columns: COLUMNS,
            rows: ROWS,
            cell_width: 9,
            cell_height: 17,
            device_attributes: None,
            xtversion: None,
        },
        (),
    )
    .unwrap();
    terminal.set_scrollback_bytes(None).unwrap();
    terminal.set_scrollback_lines(None).unwrap();
    let mut output = String::new();
    for line in 0..LINES {
        let _ = write!(output, "{line:05} 界e\u{301} ");
        if line % 7 == 0 {
            output.push_str(
                "\x1b]8;;https://example.test/\x1b\\link\x1b]8;;\x1b\\",
            );
        }
        output.push_str("\r\n");
    }
    terminal.write(output.as_bytes()).unwrap();
    terminal
}

/// Everything a link scan reads from one cell.
fn read(cell: &crate::GridRef<'_>) -> (crate::Cell, Vec<u32>, Vec<u8>) {
    let mut codepoints = [0; 8];
    let Fill::Written(codepoints_len) =
        cell.graphemes(&mut codepoints).unwrap()
    else {
        panic!("grapheme buffer too small");
    };
    let mut uri = [0; 64];
    let Fill::Written(uri_len) = cell.hyperlink_uri(&mut uri).unwrap() else {
        panic!("hyperlink buffer too small");
    };
    (
        cell.cell().unwrap(),
        codepoints[..codepoints_len].to_vec(),
        uri[..uri_len].to_vec(),
    )
}

#[test]
fn a_row_reference_reads_each_cell_as_a_freshly_resolved_reference_does() {
    let terminal = paged_terminal();
    let total = u32::try_from(terminal.total_rows().unwrap()).unwrap();
    assert!(total > 2000, "history spans several pages: {total}");
    let last = COLUMNS - 1;
    for y in (0..total).step_by(37).chain([total - 1]) {
        let row = terminal.row_ref(Point::screen(last, y)).unwrap();
        assert_eq!(row.last_column(), last);
        let fresh_row = terminal.grid_ref(Point::screen(0, y)).unwrap();
        assert_eq!(row.row().unwrap(), fresh_row.row().unwrap(), "row {y}");
        for x in 0..COLUMNS {
            let adjusted = row.cell(x).unwrap();
            let fresh = terminal.grid_ref(Point::screen(x, y)).unwrap();
            assert_eq!(adjusted.position(), fresh.position(), "{x},{y}");
            assert_eq!(read(&adjusted), read(&fresh), "{x},{y}");
        }
    }
    // The line number at the start of each row confirms the rows are
    // distinct, so a stale node or row would not compare equal by chance.
    let row = terminal.row_ref(Point::screen(4, 1234)).unwrap();
    let digits: Vec<u32> = (0..5)
        .map(|x| row.cell(x).unwrap().cell().unwrap().codepoint().unwrap())
        .collect();
    assert_eq!(digits, "01234".chars().map(u32::from).collect::<Vec<_>>());
}

#[test]
fn a_row_reference_refuses_columns_beyond_the_one_it_was_resolved_at() {
    let terminal = paged_terminal();
    let row = terminal.row_ref(Point::screen(3, 10)).unwrap();
    assert!(row.cell(3).is_ok());
    assert_eq!(row.cell(4).unwrap_err(), Error::InvalidValue);
    assert_eq!(row.cell(u16::MAX).unwrap_err(), Error::InvalidValue);
    // Ghostty validates the resolved column, so a row cannot be resolved
    // past the terminal's width.
    assert_eq!(
        terminal.row_ref(Point::screen(COLUMNS, 10)).unwrap_err(),
        Error::InvalidValue
    );
}

#[test]
fn screen_points_resolve_like_viewport_points_at_every_scroll_position() {
    let mut terminal = paged_terminal();
    let total = terminal.total_rows().unwrap();
    // Viewport points resolve from the viewport's own position, a separate
    // path from screen points, which walk from either end of the list.
    for top in (0..=total - usize::from(ROWS)).step_by(3) {
        terminal.scroll(Scroll::Row(top)).unwrap();
        assert_eq!(terminal.scrollbar().unwrap().offset, top as u64);
        for row in 0..u32::from(ROWS) {
            let y = u32::try_from(top).unwrap() + row;
            for x in [0, 6, COLUMNS - 1] {
                let viewport = terminal.grid_ref(Point::viewport(x, row));
                let screen = terminal.grid_ref(Point::screen(x, y));
                assert_eq!(
                    screen.unwrap().position(),
                    viewport.unwrap().position(),
                    "{x},{y}"
                );
            }
        }
    }
    let total = u32::try_from(total).unwrap();
    assert!(terminal.grid_ref(Point::screen(0, total - 1)).is_ok());
    assert_eq!(
        terminal.grid_ref(Point::screen(0, total)).unwrap_err(),
        Error::InvalidValue
    );
    assert_eq!(
        terminal.grid_ref(Point::screen(0, u32::MAX)).unwrap_err(),
        Error::InvalidValue
    );
}

#[test]
fn screen_points_track_pruned_history() {
    let mut terminal = paged_terminal();
    let before = terminal.total_rows().unwrap();
    terminal.set_scrollback_lines(Some(1000)).unwrap();
    let total = terminal.total_rows().unwrap();
    assert!(total < before, "pruning removed pages: {before} -> {total}");
    terminal.scroll(Scroll::Top).unwrap();
    // The first remaining row is now at the top of the screen.
    let top = terminal.grid_ref(Point::viewport(0, 0)).unwrap();
    assert_eq!(
        terminal.grid_ref(Point::screen(0, 0)).unwrap().position(),
        top.position()
    );
    terminal.scroll(Scroll::Bottom).unwrap();
    let total = u32::try_from(total).unwrap();
    let bottom = terminal
        .grid_ref(Point::viewport(0, u32::from(ROWS) - 1))
        .unwrap();
    assert_eq!(
        terminal
            .grid_ref(Point::screen(0, total - 1))
            .unwrap()
            .position(),
        bottom.position()
    );
    assert_eq!(
        terminal.grid_ref(Point::screen(0, total)).unwrap_err(),
        Error::InvalidValue
    );
}

#[test]
fn history_cells_read_their_osc_133_role_and_style() {
    let mut terminal = paged_terminal();
    terminal
        .write(b"\x1b]133;A\x07\x1b[32m> \x1b[0m\x1b]133;B\x07ls\r\n\x1b]133;C\x07out\r\n")
        .unwrap();
    write_rows(&mut terminal, usize::from(ROWS) * 2);
    let total = terminal.total_rows().unwrap();
    let prompt = u32::try_from(total - usize::from(ROWS) * 2 - 3).unwrap();
    let row = terminal.row_ref(Point::screen(4, prompt)).unwrap();
    let cell = |column| row.cell(column).unwrap();
    assert_eq!(
        cell(0).cell().unwrap().semantic().unwrap(),
        crate::CellSemantic::Prompt
    );
    assert_eq!(
        cell(0).style().unwrap().foreground,
        crate::StyleColor::Palette(2)
    );
    assert_eq!(
        cell(2).cell().unwrap().semantic().unwrap(),
        crate::CellSemantic::Input
    );
    assert_eq!(cell(2).style().unwrap(), crate::Style::default());
    let output = terminal.row_ref(Point::screen(0, prompt + 1)).unwrap();
    assert_eq!(
        output.cell(0).unwrap().cell().unwrap().semantic().unwrap(),
        crate::CellSemantic::Output
    );
}

#[test]
fn prompt_scrolls_put_the_prompt_at_the_top_and_stop_at_live() {
    let mut terminal = paged_terminal();
    for command in 0..3 {
        terminal
            .write(
                format!("\x1b]133;A\x07$ cmd{command}\r\n\x1b]133;C\x07")
                    .as_bytes(),
            )
            .unwrap();
        write_rows(&mut terminal, usize::from(ROWS) * 3);
    }
    terminal.write(b"\x1b]133;A\x07$ ").unwrap();
    let top_text = |terminal: &Terminal<()>| {
        let row = terminal.row_ref(Point::viewport(COLUMNS - 1, 0)).unwrap();
        let mut text = String::new();
        for column in 0..6 {
            let codepoint = row
                .cell(column)
                .unwrap()
                .cell()
                .unwrap()
                .codepoint()
                .unwrap();
            text.push(char::from_u32(codepoint).unwrap_or(' '));
        }
        text.trim_end_matches('\0').to_owned()
    };
    // The live screen holds the newest prompt, so the first jump back
    // reaches the prompt above it.
    terminal.scroll(Scroll::Prompt(-1)).unwrap();
    assert_eq!(top_text(&terminal), "$ cmd2");
    terminal.scroll(Scroll::Prompt(-2)).unwrap();
    assert_eq!(top_text(&terminal), "$ cmd0");
    let at_first = terminal.scrollbar().unwrap();
    terminal.scroll(Scroll::Prompt(-1)).unwrap();
    assert_eq!(terminal.scrollbar().unwrap(), at_first, "no prompt above");
    terminal.scroll(Scroll::Prompt(1)).unwrap();
    assert_eq!(top_text(&terminal), "$ cmd1");
    // The newest prompt is in the active area, which returns to live.
    terminal.scroll(Scroll::Prompt(2)).unwrap();
    let live = terminal.scrollbar().unwrap();
    assert_eq!(live.offset + live.len, live.total);
}

/// Writes `count` plain rows.
fn write_rows(terminal: &mut Terminal<()>, count: usize) {
    for _ in 0..count {
        terminal.write(b"x\r\n").unwrap();
    }
}
