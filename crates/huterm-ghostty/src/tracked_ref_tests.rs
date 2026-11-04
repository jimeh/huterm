//! Contracts for tracked grid references: they follow their cell through
//! scrolling and reflow, lose their value when Ghostty discards the cell,
//! and stay safe to drop after their terminal.

use crate::{Options, Point, PointSpace, Terminal};

fn terminal(columns: u16) -> Terminal<()> {
    let mut terminal = Terminal::new(
        Options {
            columns,
            rows: 4,
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
    terminal
}

/// The text of the screen row `reference` is on, from its column on.
fn text_at(terminal: &Terminal<()>, reference: &crate::TrackedRef) -> String {
    let point = terminal
        .tracked_point(reference, PointSpace::Screen)
        .unwrap()
        .expect("reference has a value");
    let (columns, _) = terminal.size().unwrap();
    let end = Point::screen(columns - 1, point.y);
    let text = terminal.format_plain(point, end).unwrap();
    String::from_utf8(text.as_bytes().to_vec()).unwrap()
}

#[test]
fn a_tracked_reference_follows_its_cell_through_scrolling_and_reflow() {
    let mut terminal = terminal(20);
    terminal.write(b"one\r\n").unwrap();
    let mark = terminal.track(Point::active(0, 1)).unwrap();
    terminal.write(b"marked line text\r\n").unwrap();
    for line in 0..10 {
        terminal
            .write(format!("filler {line}\r\n").as_bytes())
            .unwrap();
    }
    assert!(terminal.has_value(&mark));
    assert_eq!(text_at(&terminal, &mark), "marked line text");
    terminal.resize(6, 4, 9, 17).unwrap();
    assert_eq!(text_at(&terminal, &mark), "marked");
    terminal.resize(40, 4, 9, 17).unwrap();
    assert_eq!(text_at(&terminal, &mark), "marked line text");
    assert!(
        terminal
            .tracked_point(&mark, PointSpace::Active)
            .unwrap()
            .is_none(),
        "a history row has no active-area point"
    );
}

#[test]
fn ris_and_history_erasure_discard_tracked_references() {
    type Discard = fn(&mut Terminal<()>);
    let ris: Discard = |terminal| terminal.write(b"\x1bc").unwrap();
    let reset: Discard = |terminal| terminal.reset().unwrap();
    let ed3: Discard = |terminal| terminal.write(b"\x1b[3J").unwrap();
    // Whether each discards the screen's cells as well as history's.
    for (discard, screen_too) in [(ris, true), (reset, true), (ed3, false)] {
        let mut terminal = terminal(20);
        let history = terminal.track(Point::active(0, 0)).unwrap();
        terminal.write(b"a\r\nb\r\nc\r\nd\r\ne\r\nf").unwrap();
        let screen = terminal.track(Point::active(0, 3)).unwrap();
        discard(&mut terminal);
        assert!(!terminal.has_value(&history));
        assert_eq!(
            terminal
                .tracked_point(&history, PointSpace::Screen)
                .unwrap(),
            None
        );
        assert_eq!(terminal.has_value(&screen), !screen_too);
    }
}

/// Pruning history past its limit, by output or by lowering the limit,
/// discards references in the pruned pages and keeps newer ones. A pin
/// bump that pruned through plain page erasure would instead leave them on
/// the remapped top row, and command records would never leave history.
#[test]
fn pruning_history_discards_tracked_references_in_pruned_pages() {
    const LINES: usize = 200_000;
    let write_lines = |terminal: &mut Terminal<()>, count: usize| {
        for line in 0..count {
            terminal
                .write(format!("line {line}\r\n").as_bytes())
                .unwrap();
        }
    };

    // By output, under a fixed limit. Ghostty prunes whole pages, so write
    // until the anchored page goes, within a generous bound.
    let mut limited = terminal(20);
    limited.set_scrollback_lines(Some(100)).unwrap();
    let pruned = limited.track(Point::active(0, 0)).unwrap();
    let mut written = 0;
    while limited.has_value(&pruned) && written < LINES {
        write_lines(&mut limited, 1_000);
        written += 1_000;
    }
    assert!(
        !limited.has_value(&pruned),
        "output pruned no page within {LINES} lines"
    );
    let recent = limited.track(Point::active(0, 0)).unwrap();
    write_lines(&mut limited, 2);
    assert!(limited.has_value(&recent));

    // By lowering the limit after the output.
    let mut unlimited = terminal(20);
    let pruned = unlimited.track(Point::active(0, 0)).unwrap();
    write_lines(&mut unlimited, written);
    let recent = unlimited.track(Point::active(0, 0)).unwrap();
    assert!(unlimited.has_value(&pruned));
    unlimited.set_scrollback_lines(Some(100)).unwrap();
    assert!(!unlimited.has_value(&pruned));
    assert!(unlimited.has_value(&recent));
}

#[test]
fn in_place_erasure_keeps_a_reference_on_its_row() {
    let mut terminal = terminal(20);
    terminal.write(b"one\r\ntwo").unwrap();
    let mark = terminal.track(Point::active(0, 1)).unwrap();
    // ED 2 and delete line erase rows without discarding their cells.
    terminal
        .write(b"\x1b[2J\x1b[2;1Hnew\x1b[1;1H\x1b[M")
        .unwrap();
    assert!(terminal.has_value(&mark));
}

#[test]
fn references_are_read_only_through_their_terminal_and_outlive_it() {
    let mut first = terminal(20);
    let second = terminal(20);
    let mark = first.track(Point::active(0, 0)).unwrap();
    assert!(first.has_value(&mark));
    assert!(!second.has_value(&mark));
    assert!(matches!(
        second.tracked_point(&mark, PointSpace::Screen),
        Err(crate::Error::InvalidValue)
    ));
    assert!(matches!(
        first.track(Point::active(20, 0)),
        Err(crate::Error::InvalidValue)
    ));
    drop(first);
    // grid_ref_tracked.h: a reference may be freed after its terminal.
    drop(mark);
}
