//! Contracts for scrollback compression: what a step compresses, what
//! restores a page, and what changes the activity token.

use std::fmt::Write as _;

use crate::{
    COMPRESSION_SUPPORTED, Compression, Fill, Options, Point, RenderState,
    Scroll, Terminal,
};

const COLUMNS: u16 = 80;
const ROWS: u16 = 4;
/// Several pages of history at 80 columns, where a page holds about 570 rows.
const LINES: usize = 3000;

/// A terminal whose history spans several pages, with styled, wide,
/// combining, and hyperlinked cells.
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
    terminal.write(&history(0..LINES)).unwrap();
    terminal
}

fn history(lines: std::ops::Range<usize>) -> Vec<u8> {
    let mut output = String::new();
    for line in lines {
        let _ =
            write!(output, "\x1b[3{}m{line:05}\x1b[0m 界e\u{301} ", line % 8);
        if line % 7 == 0 {
            output.push_str(
                "\x1b]8;;https://example.test/\x1b\\link\x1b]8;;\x1b\\",
            );
        }
        output.push_str("\r\n");
    }
    output.into_bytes()
}

/// Steps until the pass completes, with a bound so a regression that never
/// completes fails instead of hanging.
fn drain<H>(terminal: &mut Terminal<H>) -> Compression {
    for _ in 0..100_000 {
        match terminal.compress_step().unwrap() {
            Compression::Pending => {}
            done => return done,
        }
    }
    panic!("compression never completed");
}

/// Everything a link scan reads from one cell.
fn read(
    terminal: &Terminal<()>,
    point: Point,
) -> (crate::Cell, crate::Row, Vec<u32>, Vec<u8>) {
    let cell = terminal.grid_ref(point).unwrap();
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
        cell.row().unwrap(),
        codepoints[..codepoints_len].to_vec(),
        uri[..uri_len].to_vec(),
    )
}

#[test]
fn a_pass_compresses_cold_history_and_lowers_resident_memory() {
    let mut terminal = paged_terminal();
    let before = terminal.memory_usage().unwrap();
    assert_eq!(before.compression_supported, COMPRESSION_SUPPORTED);
    assert!(before.primary.pages > 3, "{before:?}");
    assert_eq!(before.primary.compressed_pages, 0);
    if !COMPRESSION_SUPPORTED {
        assert_eq!(drain(&mut terminal), Compression::Unsupported);
        return;
    }
    assert_eq!(drain(&mut terminal), Compression::Complete);
    let after = terminal.memory_usage().unwrap();
    // Every page but the one holding the active area and viewport.
    assert_eq!(after.primary.compressed_pages, before.primary.pages - 1);
    assert!(after.primary.compressed_bytes > 0);
    assert!(
        after.primary.resident_bytes < before.primary.resident_bytes / 2,
        "{before:?} {after:?}"
    );
    // Compression keeps each page's address range for its restore.
    assert_eq!(after.primary.virtual_bytes, before.primary.virtual_bytes);
    assert_eq!(after.alternate, crate::ScreenMemory::default());
    // Stepping again starts a new pass, which finds nothing left; callers
    // use the activity token to avoid it.
    assert_eq!(drain(&mut terminal), Compression::Complete);
    assert_eq!(terminal.memory_usage().unwrap(), after);
}

#[test]
fn reading_a_compressed_cell_restores_exactly_its_page() {
    if !COMPRESSION_SUPPORTED {
        return;
    }
    let mut terminal = paged_terminal();
    let points = [7, 1234, 2001].map(|y| Point::screen(6, y));
    let expected = points.map(|point| read(&terminal, point));
    drain(&mut terminal);
    let compressed = terminal.memory_usage().unwrap().primary.compressed_pages;
    // Rows 1234 and 2001 sit on different pages, more than one page apart.
    for (restored, (point, expected)) in
        points.iter().zip(&expected).enumerate().skip(1)
    {
        assert_eq!(&read(&terminal, *point), expected, "{point:?}");
        assert_eq!(
            terminal.memory_usage().unwrap().primary.compressed_pages,
            compressed - restored as u64,
            "{point:?}"
        );
    }
    // Reading the first page again restores it and only it.
    assert_eq!(read(&terminal, points[0]), expected[0]);
    assert_eq!(
        terminal.memory_usage().unwrap().primary.compressed_pages,
        compressed - 3
    );
}

#[test]
fn the_activity_token_follows_history_changes_and_ignores_reads() {
    if !COMPRESSION_SUPPORTED {
        return;
    }
    let mut terminal = paged_terminal();
    drain(&mut terminal);
    let token = terminal.compression_activity().unwrap();

    // Reads, including ones that restore a page, leave it alone.
    let mut render = RenderState::new().unwrap();
    render.update(&mut terminal).unwrap();
    terminal.memory_usage().unwrap();
    read(&terminal, Point::screen(0, 100));
    terminal.write(b"\x1b[2;1Hredraw").unwrap();
    assert_eq!(terminal.compression_activity().unwrap(), token);

    // Output that allocates pages changes it.
    terminal.write(&history(LINES..LINES + 1000)).unwrap();
    let grown = terminal.compression_activity().unwrap();
    assert_ne!(grown, token);

    // So does moving the viewport, which leaves different pages cold.
    terminal.scroll(Scroll::Top).unwrap();
    assert_ne!(terminal.compression_activity().unwrap(), grown);
}
