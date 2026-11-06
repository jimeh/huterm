//! The model loaded into AccessKit's consumer, which answers the same
//! queries the platform adapters answer for assistive technology.

use accesskit_consumer::Tree;
use gpui::accesskit::{self, Point, TreeId, TreeUpdate};
use gpui::{point, px, size};
use huterm_protocol::{
    Cell, CellColor, CellStyle, CellText, GridSize, TerminalId, TerminalModes,
    Viewport,
};
use unicode_segmentation::UnicodeSegmentation;

use super::*;

const CELL_WIDTH: f32 = 8.0;
const CELL_HEIGHT: f32 = 16.0;
const SCALE: f32 = 2.0;
/// Where the grid starts in the window, in points.
const ORIGIN: (f32, f32) = (10.0, 30.0);
const WIDE: [&str; 3] = ["漢", "字", "🙂"];

fn cell(text: &str, style: CellStyle) -> Cell {
    Cell {
        text: CellText::new(text),
        foreground: CellColor::DefaultForeground,
        background: CellColor::DefaultBackground,
        style,
    }
}

/// One cell per grapheme, padded with blanks to `columns`. The graphemes
/// in [`WIDE`] take two cells; `|` at the last column is a spacer head.
fn cells(text: &str, columns: u16) -> Vec<Cell> {
    let mut cells = Vec::new();
    for grapheme in text.graphemes(true) {
        if WIDE.contains(&grapheme) {
            cells.push(cell(
                grapheme,
                CellStyle {
                    wide: true,
                    ..CellStyle::default()
                },
            ));
            cells.push(cell(
                " ",
                CellStyle {
                    wide_spacer: true,
                    ..CellStyle::default()
                },
            ));
        } else if grapheme == "|" {
            cells.push(cell(
                " ",
                CellStyle {
                    wide_spacer: true,
                    ..CellStyle::default()
                },
            ));
        } else {
            cells.push(cell(grapheme, CellStyle::default()));
        }
    }
    assert!(cells.len() <= usize::from(columns), "{text:?} overflows");
    cells.resize_with(usize::from(columns), || cell(" ", CellStyle::default()));
    cells
}

fn row(text: &str, columns: u16, wrapped: bool) -> Arc<TerminalRow> {
    Arc::new(TerminalRow {
        cells: cells(text, columns),
        wrapped,
        ..TerminalRow::default()
    })
}

/// Rows ending in `\` soft-wrap into the next.
fn snapshot(columns: u16, lines: &[&str]) -> TerminalSnapshot {
    let rows = lines
        .iter()
        .map(|line| match line.strip_suffix('\\') {
            Some(line) => row(line, columns, true),
            None => row(line, columns, false),
        })
        .collect::<Vec<_>>();
    snapshot_of(columns, rows)
}

fn snapshot_of(columns: u16, rows: Vec<Arc<TerminalRow>>) -> TerminalSnapshot {
    TerminalSnapshot {
        terminal_id: TerminalId::new(1),
        generation: 1,
        revision: 1,
        geometry_revision: 0,
        size: GridSize::clamped(columns, u16::try_from(rows.len()).unwrap()),
        rows,
        cursor: None,
        modes: TerminalModes::default(),
        viewport: Viewport::default(),
        history_size: 0,
        cursor_color: None,
        default_background: None,
    }
}

fn with_cursor(
    mut snapshot: TerminalSnapshot,
    column: u16,
    row: u16,
    shape: CursorShape,
) -> TerminalSnapshot {
    snapshot.cursor = Some(Cursor { row, column, shape });
    snapshot
}

/// The view paints the whole snapshot.
fn geometry(snapshot: &TerminalSnapshot) -> GridGeometry {
    clipped(
        snapshot,
        f32::from(snapshot.size.columns),
        f32::from(snapshot.size.rows),
    )
}

/// The view paints `columns` by `rows` cells, fractions included.
fn clipped(
    snapshot: &TerminalSnapshot,
    columns: f32,
    rows: f32,
) -> GridGeometry {
    GridGeometry::new(
        Bounds::new(
            point(px(ORIGIN.0), px(ORIGIN.1)),
            size(px(CELL_WIDTH * columns), px(CELL_HEIGHT * rows)),
        ),
        size(px(CELL_WIDTH), px(CELL_HEIGHT)),
        snapshot.size,
        Some(snapshot.size),
    )
}

fn metrics() -> GridMetrics {
    GridMetrics::fixed(CELL_WIDTH, CELL_HEIGHT)
}

fn build(snapshot: &TerminalSnapshot) -> Model {
    build_in(
        &mut ModelBuilder::default(),
        snapshot,
        geometry(snapshot),
        None,
    )
}

fn build_in(
    builder: &mut ModelBuilder,
    snapshot: &TerminalSnapshot,
    geometry: GridGeometry,
    selection: Option<BufferRange>,
) -> Model {
    let model = builder.build(&Inputs {
        snapshot,
        geometry,
        selection,
        metrics: metrics(),
        scale: SCALE,
    });
    check_runs(&model);
    model
}

/// The invariants AccessKit relies on, after trimming and splitting.
fn check_runs(model: &Model) {
    for run in &model.runs {
        let node = &run.node;
        let lengths = node.character_lengths();
        let value = node.value().unwrap();
        assert!(!lengths.is_empty(), "an empty run: {run:?}");
        assert!(lengths.len() <= RUN_LIMIT, "an oversized run: {run:?}");
        assert_eq!(
            lengths
                .iter()
                .map(|&length| usize::from(length))
                .sum::<usize>(),
            value.len(),
            "character lengths must cover {value:?}"
        );
        assert_eq!(node.character_positions().unwrap().len(), lengths.len());
        assert_eq!(node.character_widths().unwrap().len(), lengths.len());
        assert_eq!(node.text_direction(), Some(TextDirection::LeftToRight));
        assert!(node.bounds().is_some());
        let starts = node.word_starts();
        assert!(starts.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(
            starts
                .iter()
                .all(|&start| usize::from(start) < lengths.len())
        );
    }
}

const ROOT: accesskit::NodeId = accesskit::NodeId(0);
const TERMINAL: accesskit::NodeId = accesskit::NodeId(1);

fn child_id(key: ChildKey) -> accesskit::NodeId {
    accesskit::NodeId(match key {
        ChildKey::Caret => 2,
        ChildKey::Run(run) => {
            1000 + u64::from(run.row) * 100 + u64::from(run.segment)
        }
    })
}

/// The tree GPUI would send for a window holding just this terminal.
pub(in crate::desktop) fn tree(model: &Model) -> Tree {
    let (children, selection) = model.nodes(child_id);
    let mut terminal = accesskit::Node::new(Role::Terminal);
    terminal
        .set_children(children.iter().map(|(id, _)| *id).collect::<Vec<_>>());
    if let Some(selection) = selection {
        terminal.set_text_selection(selection);
    }
    let mut root = accesskit::Node::new(Role::Window);
    root.set_children(vec![TERMINAL]);
    let mut nodes = vec![(ROOT, root), (TERMINAL, terminal)];
    nodes.extend(children);
    Tree::new(
        TreeUpdate {
            nodes,
            tree: Some(accesskit::Tree::new(ROOT)),
            tree_id: TreeId::ROOT,
            focus: TERMINAL,
        },
        true,
    )
}

pub(in crate::desktop) fn terminal(
    tree: &Tree,
) -> accesskit_consumer::Node<'_> {
    let node = tree.state().root().children().next().unwrap();
    assert_eq!(node.role(), Role::Terminal);
    assert!(node.supports_text_ranges());
    node
}

pub(in crate::desktop) fn text(model: &Model) -> String {
    terminal(&tree(model)).document_range().text()
}

fn lines(model: &Model) -> Vec<String> {
    let tree = tree(model);
    let node = terminal(&tree);
    // AccessKit answers one more, empty, line at the document's end.
    (0..)
        .map_while(|line| node.line_range_from_index(line))
        .map(|range| range.text())
        .take_while(|line| !line.is_empty())
        .collect()
}

/// The window physical rectangle of a cell, clipped like the model.
fn cell_rect(column: u16, row: u16, cells: f32) -> Rect {
    let scale = f64::from(SCALE);
    let x0 = f64::from(ORIGIN.0 + CELL_WIDTH * f32::from(column)) * scale;
    let y0 = f64::from(ORIGIN.1 + CELL_HEIGHT * f32::from(row)) * scale;
    Rect {
        x0,
        y0,
        x1: x0 + f64::from(CELL_WIDTH * cells) * scale,
        y1: y0 + f64::from(CELL_HEIGHT) * scale,
    }
}

fn center(rect: Rect) -> Point {
    Point::new(rect.x0.midpoint(rect.x1), rect.y0.midpoint(rect.y1))
}

/// The UTF-16 index hit testing reports at `point`.
fn index_at(model: &Model, at: Point) -> usize {
    let tree = tree(model);
    terminal(&tree)
        .text_position_at_point(at)
        .to_global_utf16_index()
}

/// The union of the boxes of the UTF-16 range `start..end`.
fn bounds_of(model: &Model, start: usize, end: usize) -> Rect {
    let tree = tree(model);
    let node = terminal(&tree);
    let mut range = node
        .text_position_from_global_utf16_index(start)
        .unwrap()
        .to_degenerate_range();
    range.set_end(node.text_position_from_global_utf16_index(end).unwrap());
    range
        .bounding_boxes()
        .into_iter()
        .reduce(|left, right| left.union(right))
        .unwrap()
}

fn line_of(model: &Model, index: usize) -> usize {
    let tree = tree(model);
    terminal(&tree)
        .text_position_from_global_utf16_index(index)
        .unwrap()
        .to_line_index()
}

pub(in crate::desktop) fn selected(
    model: &Model,
) -> Option<(usize, usize, String)> {
    let tree = tree(model);
    let node = terminal(&tree);
    node.text_selection().map(|range| {
        (
            range.start().to_global_utf16_index(),
            range.end().to_global_utf16_index(),
            range.text(),
        )
    })
}

fn caret(model: &Model) -> Option<Rect> {
    let tree = tree(model);
    let node = terminal(&tree);
    let caret = node.children().find(|child| child.role() == Role::Caret)?;
    assert!(caret.is_hidden(), "the caret must stay out of navigation");
    caret.bounding_box()
}

/// The painter's cursor rectangle in window physical pixels.
fn painted_cursor(cursor: Cursor) -> Rect {
    let bounds =
        cursor_bounds(point(px(ORIGIN.0), px(ORIGIN.1)), cursor, metrics());
    let scale = f64::from(SCALE);
    Rect {
        x0: f64::from(bounds.origin.x) * scale,
        y0: f64::from(bounds.origin.y) * scale,
        x1: f64::from(bounds.right()) * scale,
        y1: f64::from(bounds.bottom()) * scale,
    }
}

fn point_at(rows_from_live_bottom: usize, column: u16) -> BufferPoint {
    BufferPoint {
        rows_from_live_bottom,
        column,
    }
}

#[test]
fn ascii_rows_are_lines_joined_by_newlines_without_trailing_blanks() {
    let model = build(&snapshot(12, &["$ ls -la", "total 0", "drwxr  x"]));
    assert_eq!(text(&model), "$ ls -la\ntotal 0\ndrwxr  x");
    assert_eq!(lines(&model), ["$ ls -la\n", "total 0\n", "drwxr  x"]);
    assert_eq!(line_of(&model, 0), 0);
    assert_eq!(line_of(&model, 8), 0, "the newline ends its own line");
    assert_eq!(line_of(&model, 9), 1);
    assert_eq!(line_of(&model, 24), 2);
    let tree = tree(&model);
    assert_eq!(
        terminal(&tree).value().as_deref(),
        Some("$ ls -la\ntotal 0\ndrwxr  x"),
        "AXValue reads the displayed rows"
    );
    let words = model.runs[2].node.word_starts().to_vec();
    assert_eq!(words, [0, 7], "an inner blank run separates words");
}

#[test]
fn a_combining_cluster_is_one_character_in_one_cell() {
    let model = build(&snapshot(6, &["e\u{301}x", "ok"]));
    let run = &model.runs[0].node;
    assert_eq!(run.value().unwrap(), "e\u{301}x\n");
    assert_eq!(run.character_lengths(), [3, 1, 1]);
    // UTF-16: e, the mark, x, then the newline.
    assert_eq!(index_at(&model, center(cell_rect(1, 0, 1.0))), 2);
    assert_eq!(bounds_of(&model, 0, 2), cell_rect(0, 0, 1.0));
    assert_eq!(bounds_of(&model, 2, 3), cell_rect(1, 0, 1.0));
    assert_eq!(line_of(&model, 4), 1);
}

#[test]
fn supplementary_characters_take_two_utf16_units() {
    // U+1D400 is narrow; the emoji is wide.
    let model = build(&snapshot(8, &["\u{1d400}b🙂c"]));
    assert_eq!(text(&model), "\u{1d400}b🙂c");
    assert_eq!(model.runs[0].node.character_lengths(), [4, 1, 4, 1]);
    assert_eq!(index_at(&model, center(cell_rect(1, 0, 1.0))), 2);
    assert_eq!(index_at(&model, center(cell_rect(3, 0, 1.0))), 3);
    assert_eq!(index_at(&model, center(cell_rect(4, 0, 1.0))), 5);
    assert_eq!(bounds_of(&model, 3, 5), cell_rect(2, 0, 2.0));
    let tree = tree(&model);
    assert_eq!(
        terminal(&tree)
            .document_range()
            .end()
            .to_global_utf16_index(),
        6
    );
}

#[test]
fn wide_characters_span_their_spacer_and_spacer_heads_add_nothing() {
    // The second row ends in a spacer head: its wide character wrapped.
    let model = build(&snapshot(5, &["a漢b", "abcd|\\", "字"]));
    assert_eq!(text(&model), "a漢b\nabcd字");
    assert_eq!(lines(&model), ["a漢b\n", "abcd", "字"]);
    let first = &model.runs[0].node;
    assert_eq!(
        first.character_positions().unwrap(),
        [
            0.0,
            CELL_WIDTH * SCALE,
            CELL_WIDTH * SCALE * 3.0,
            CELL_WIDTH * SCALE * 4.0
        ]
    );
    assert_eq!(
        first.character_widths().unwrap(),
        [
            CELL_WIDTH * SCALE,
            CELL_WIDTH * SCALE * 2.0,
            CELL_WIDTH * SCALE,
            CELL_WIDTH * SCALE
        ]
    );
    // Both halves of the wide cell hit the wide character.
    assert_eq!(index_at(&model, center(cell_rect(1, 0, 1.0))), 1);
    assert_eq!(index_at(&model, center(cell_rect(2, 0, 1.0))), 1);
    assert_eq!(bounds_of(&model, 1, 2), cell_rect(1, 0, 2.0));
    // The spacer head has no character: its cell is past the row's text.
    let head = center(cell_rect(4, 1, 1.0));
    assert_eq!(index_at(&model, head), 8, "the end of the wrapped row");
}

#[test]
fn blank_cells_and_rows_keep_inner_blanks_and_never_leave_a_run_empty() {
    let model = build(&snapshot(6, &["a  b", "", "c", ""]));
    assert_eq!(text(&model), "a  b\n\nc\n ");
    assert_eq!(lines(&model), ["a  b\n", "\n", "c\n", " "]);
    // The blank row's newline sits at its start, one cell wide.
    assert_eq!(bounds_of(&model, 5, 6), cell_rect(0, 1, 1.0));
    // An empty last row is one space at its first cell.
    assert_eq!(bounds_of(&model, 8, 9), cell_rect(0, 3, 1.0));
    assert_eq!(model.runs[1].node.word_starts(), [] as [u8; 0]);
}

#[test]
fn a_cursor_past_the_trimmed_text_sits_at_the_line_end() {
    let model = build(&with_cursor(
        snapshot(10, &["$ ", "next"]),
        6,
        0,
        CursorShape::Block,
    ));
    assert_eq!(text(&model), "$\nnext", "no padding to the cursor");
    assert_eq!(selected(&model), Some((1, 1, String::new())));
    let cursor = Cursor {
        row: 0,
        column: 6,
        shape: CursorShape::Block,
    };
    assert_eq!(caret(&model), Some(painted_cursor(cursor)));
    // The cursor in the middle of text sits before its cell's character.
    let model = build(&with_cursor(
        snapshot(10, &["abc", "next"]),
        1,
        1,
        CursorShape::Block,
    ));
    assert_eq!(selected(&model), Some((5, 5, String::new())));
}

#[test]
fn a_soft_wrapped_row_continues_without_a_newline() {
    let model = build(&snapshot(4, &["abcd\\", "ef", "gh"]));
    assert_eq!(text(&model), "abcdef\ngh");
    assert_eq!(lines(&model), ["abcd", "ef\n", "gh"]);
    assert_eq!(line_of(&model, 3), 0);
    assert_eq!(line_of(&model, 4), 1);
    assert_eq!(index_at(&model, center(cell_rect(0, 1, 1.0))), 4);
}

#[test]
fn rows_over_the_run_limit_split_into_runs_on_one_line() {
    let line = "word ".repeat(60) + "tail";
    let model = build(&snapshot(310, &[&line, "next"]));
    let first: Vec<_> =
        model.runs.iter().filter(|run| run.key.row == 0).collect();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].node.character_lengths().len(), RUN_LIMIT);
    assert!(first[0].continues && !first[0].continued);
    assert!(first[1].continued && !first[1].continues);
    assert_eq!(text(&model), format!("{line}\nnext"));
    assert_eq!(lines(&model), [format!("{line}\n"), "next".to_owned()]);
    // Character 255 is the 'w' of the 52nd word: a word start in the
    // second run, not continued from the first.
    assert_eq!(first[1].node.word_starts()[0], 0);
    // The split does not move characters.
    let split = cell_rect(255, 0, 1.0);
    assert_eq!(index_at(&model, center(split)), 255);
    assert_eq!(bounds_of(&model, 255, 256), split);
    assert_eq!(bounds_of(&model, 254, 256), {
        let mut both = cell_rect(254, 0, 1.0);
        both.x1 = split.x1;
        both
    });
    assert_eq!(line_of(&model, 300), 0);
    // A word cut by the split is not a new word in its second run.
    let cut = "x".repeat(300);
    let model = build(&snapshot(300, &[&cut]));
    assert_eq!(model.runs[1].node.word_starts(), [] as [u8; 0]);
}

#[test]
fn bounds_points_and_lines_agree_at_the_grid_edges() {
    let model = build(&snapshot(6, &["abcdef", "", "uvwxyz"]));
    assert_eq!(text(&model), "abcdef\n\nuvwxyz");
    for (column, row, index) in [(0, 0, 0), (5, 0, 5), (0, 2, 8), (5, 2, 13)] {
        let rect = cell_rect(column, row, 1.0);
        assert_eq!(index_at(&model, center(rect)), index, "({column}, {row})");
        assert_eq!(bounds_of(&model, index, index + 1), rect);
        assert_eq!(line_of(&model, index), usize::from(row));
    }
    // A full row's newline is clipped to nothing at the grid's right edge.
    let newline = bounds_of(&model, 6, 7);
    assert_eq!(newline.x0, cell_rect(6, 0, 1.0).x0);
    assert_eq!(newline.width(), 0.0);
    // AccessKit puts a point past a hard row's text after its newline, at
    // the next line's start; before the grid is the start, and below the
    // last row the end.
    let right = cell_rect(5, 1, 1.0);
    assert_eq!(
        index_at(&model, Point::new(right.x1 + 4.0, right.y0 + 1.0)),
        8
    );
    let last = cell_rect(5, 2, 1.0);
    assert_eq!(
        index_at(&model, Point::new(last.x1 + 4.0, last.y0 + 1.0)),
        14
    );
    assert_eq!(index_at(&model, Point::new(0.0, 0.0)), 0);
    assert_eq!(index_at(&model, Point::new(10_000.0, 10_000.0)), 14);
}

#[test]
fn a_scroll_reuses_retained_rows_at_their_new_indices() {
    let rows: Vec<_> = ["one", "two", "three", "four"]
        .iter()
        .map(|line| row(line, 8, false))
        .collect();
    let before = snapshot_of(8, rows[..3].to_vec());
    let after = snapshot_of(8, rows[1..].to_vec());
    let mut builder = ModelBuilder::default();
    let first = build_in(&mut builder, &before, geometry(&before), None);
    assert_eq!(text(&first), "one\ntwo\nthree");
    let second = build_in(&mut builder, &after, geometry(&after), None);
    assert_eq!(text(&second), "two\nthree\nfour");
    assert_eq!(lines(&second), ["two\n", "three\n", "four"]);
    // "three" moved from the last row to the middle: it gained a newline
    // and moved up a row.
    assert_eq!(index_at(&second, center(cell_rect(0, 1, 1.0))), 4);
    assert_eq!(bounds_of(&second, 4, 9).y0, cell_rect(0, 1, 1.0).y0);
    assert_eq!(
        second.runs[0].node.bounds(),
        Some({
            let mut rect = cell_rect(0, 0, 3.0);
            rect.x1 = cell_rect(3, 0, 1.0).x1;
            rect
        })
    );
}

#[test]
fn a_resize_rebuilds_the_rows_at_the_new_width() {
    let mut builder = ModelBuilder::default();
    let wide = snapshot(10, &["abcdefghij", "k"]);
    let model = build_in(&mut builder, &wide, geometry(&wide), None);
    assert_eq!(text(&model), "abcdefghij\nk");
    let narrow = snapshot(4, &["abcd\\", "efgh\\", "ij", "k"]);
    let model = build_in(&mut builder, &narrow, geometry(&narrow), None);
    assert_eq!(text(&model), "abcdefghij\nk");
    assert_eq!(lines(&model), ["abcd", "efgh", "ij\n", "k"]);
    assert_eq!(index_at(&model, center(cell_rect(3, 1, 1.0))), 7);
    assert_eq!(bounds_of(&model, 7, 8), cell_rect(3, 1, 1.0));
}

#[test]
fn a_snapshot_larger_than_the_view_shows_the_painted_part_with_fractional_edges()
 {
    let snapshot = with_cursor(
        snapshot(10, &["0123456789", "abcdefghij", "klmnopqrst", "uvwxyz"]),
        8,
        1,
        CursorShape::Block,
    );
    // Five and a half columns by two and a half rows are painted.
    let geometry = clipped(&snapshot, 5.5, 2.5);
    let model =
        build_in(&mut ModelBuilder::default(), &snapshot, geometry, None);
    assert_eq!(text(&model), "012345\nabcdef\nklmnop");
    // The half-visible column and row clip their characters' boxes.
    let mut edge = cell_rect(5, 2, 1.0);
    edge.x1 = cell_rect(5, 2, 0.5).x1;
    edge.y1 = edge.y0 + f64::from(CELL_HEIGHT * SCALE) / 2.0;
    assert_eq!(bounds_of(&model, 19, 20), edge);
    assert_eq!(index_at(&model, center(edge)), 19);
    // The newline after a clipped row has no width at the clip.
    assert_eq!(bounds_of(&model, 6, 7).width(), 0.0);
    // The cursor's column is not painted, so there is no caret and no
    // selection.
    assert_eq!(caret(&model), None);
    assert_eq!(selected(&model), None);
}

#[test]
fn the_caret_node_follows_the_painted_cursor_for_every_shape() {
    for shape in [
        CursorShape::Block,
        CursorShape::Beam,
        CursorShape::Underline,
    ] {
        let model = build(&with_cursor(snapshot(6, &["ab", "c"]), 1, 0, shape));
        let cursor = Cursor {
            row: 0,
            column: 1,
            shape,
        };
        assert_eq!(caret(&model), Some(painted_cursor(cursor)), "{shape:?}");
        assert_eq!(selected(&model), Some((1, 1, String::new())), "{shape:?}");
    }
    // A hidden cursor's caret is its cell.
    let model = build(&with_cursor(
        snapshot(6, &["ab", "c"]),
        1,
        0,
        CursorShape::Hidden,
    ));
    assert_eq!(caret(&model), Some(cell_rect(1, 0, 1.0)));
    assert_eq!(selected(&model), Some((1, 1, String::new())));
    // On a wide character the painter draws one cell, and so does the
    // caret.
    let model = build(&with_cursor(
        snapshot(6, &["a漢"]),
        1,
        0,
        CursorShape::Block,
    ));
    assert_eq!(caret(&model), Some(cell_rect(1, 0, 1.0)));
    assert_eq!(selected(&model), Some((1, 1, String::new())));
}

#[test]
fn a_cursor_outside_the_displayed_grid_has_no_caret_or_selection() {
    let snapshot = snapshot(6, &["ab", "cd", "ef"]);
    let no_cursor = build(&snapshot);
    assert_eq!(caret(&no_cursor), None);
    assert_eq!(selected(&no_cursor), None);
    // Painted rows end after the second.
    let below = with_cursor(snapshot, 0, 2, CursorShape::Block);
    let model = build_in(
        &mut ModelBuilder::default(),
        &below,
        clipped(&below, 6.0, 2.0),
        None,
    );
    assert_eq!(caret(&model), None);
    assert_eq!(selected(&model), None);
}

#[test]
fn a_selection_is_clipped_to_the_displayed_rows() {
    // Three displayed rows, scrolled two rows into history: row 0 is four
    // rows above the live bottom and row 2 two.
    let mut snapshot = with_cursor(
        snapshot(8, &["first", "second", "third"]),
        2,
        2,
        CursorShape::Block,
    );
    snapshot.viewport = Viewport { bottom_offset: 2 };
    let geometry = geometry(&snapshot);
    let select = |range| {
        build_in(
            &mut ModelBuilder::default(),
            &snapshot,
            geometry,
            Some(range),
        )
    };
    // Inside: "cond\nth", both ends inclusive of their cells.
    let model = select(BufferRange::ordered(point_at(3, 2), point_at(2, 1)));
    assert_eq!(selected(&model), Some((8, 15, "cond\nth".into())));
    // From history above the view to below it: every displayed row.
    let model = select(BufferRange::ordered(point_at(9, 4), point_at(0, 0)));
    assert_eq!(selected(&model).unwrap().2, "first\nsecond\nthird");
    // Ends past a row's text clamp to its end, before the newline.
    let model = select(BufferRange::ordered(point_at(4, 7), point_at(3, 7)));
    assert_eq!(selected(&model).unwrap().2, "\nsecond");
    // A wide character's spacer selects the whole character.
    let wide = with_cursor(
        snapshot_of(8, vec![row("a漢b", 8, false)]),
        0,
        0,
        CursorShape::Block,
    );
    let model = build_in(
        &mut ModelBuilder::default(),
        &wide,
        super::tests::geometry(&wide),
        Some(BufferRange::ordered(point_at(0, 2), point_at(0, 3))),
    );
    assert_eq!(selected(&model).unwrap().2, "漢b");
    // Entirely in history, or entirely below: the caret is the selection.
    for range in [
        BufferRange::ordered(point_at(9, 0), point_at(5, 7)),
        BufferRange::ordered(point_at(1, 0), point_at(0, 3)),
    ] {
        assert_eq!(selected(&select(range)), Some((15, 15, String::new())));
    }
    // Clearing the selection restores the caret.
    assert_eq!(
        selected(&build_in(
            &mut ModelBuilder::default(),
            &snapshot,
            geometry,
            None
        )),
        Some((15, 15, String::new()))
    );
}

struct Ignore;

impl accesskit_consumer::TreeChangeHandler for Ignore {
    fn node_added(&mut self, _: &accesskit_consumer::Node<'_>) {}
    fn node_updated(
        &mut self,
        _: &accesskit_consumer::Node<'_>,
        _: &accesskit_consumer::Node<'_>,
    ) {
    }
    fn focus_moved(
        &mut self,
        _: Option<&accesskit_consumer::Node<'_>>,
        _: Option<&accesskit_consumer::Node<'_>>,
    ) {
    }
    fn node_removed(&mut self, _: &accesskit_consumer::Node<'_>) {}
}

/// The whole tree GPUI sends each frame for one terminal.
fn update(model: &Model) -> TreeUpdate {
    let (children, selection) = model.nodes(child_id);
    let mut terminal = accesskit::Node::new(Role::Terminal);
    terminal
        .set_children(children.iter().map(|(id, _)| *id).collect::<Vec<_>>());
    if let Some(selection) = selection {
        terminal.set_text_selection(selection);
    }
    let mut root = accesskit::Node::new(Role::Window);
    root.set_children(vec![TERMINAL]);
    let mut nodes = vec![(ROOT, root), (TERMINAL, terminal)];
    nodes.extend(children);
    TreeUpdate {
        nodes,
        tree: Some(accesskit::Tree::new(ROOT)),
        tree_id: TreeId::ROOT,
        focus: TERMINAL,
    }
}

fn percentiles(mut samples: Vec<std::time::Duration>) -> String {
    samples.sort();
    let at = |fraction: f64| {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "sample counts are small"
        )]
        let index = ((samples.len() - 1) as f64 * fraction).round() as usize;
        samples[index].as_secs_f64() * 1e6
    };
    format!(
        "n={} median={:.1}us p95={:.1}us max={:.1}us",
        samples.len(),
        at(0.5),
        at(0.95),
        at(1.0)
    )
}

/// Per-frame accessibility cost on a 200x50 grid under output that scrolls
/// one full row per frame, as `cat` of a large file does. Run in release:
/// `cargo test --release -p huterm-gpui --features test-support --lib
/// terminal_a11y::tests::frame_cost -- --ignored --nocapture`.
#[test]
#[ignore = "a timing harness, not a check"]
fn frame_cost_under_output_flood() {
    use std::time::{Duration, Instant};
    const COLUMNS: u16 = 200;
    const ROWS: usize = 50;
    const FRAMES: usize = 600;
    // Frames at 60 Hz: the view publishes at most every 100 ms, so one
    // frame in six rebuilds and every frame pushes the published model.
    const FRAMES_PER_PUBLICATION: usize = 6;
    let line = |index: usize| {
        let text: String = format!("{index:06} ")
            .chars()
            .chain(
                "the quick brown fox jumps over the lazy dog "
                    .chars()
                    .cycle(),
            )
            .take(usize::from(COLUMNS))
            .collect();
        row(&text, COLUMNS, false)
    };
    let mut rows: std::collections::VecDeque<_> = (0..ROWS).map(line).collect();
    let mut builder = ModelBuilder::default();
    let snapshot = |rows: &std::collections::VecDeque<Arc<TerminalRow>>| {
        with_cursor(
            snapshot_of(COLUMNS, rows.iter().cloned().collect()),
            0,
            u16::try_from(ROWS - 1).unwrap(),
            CursorShape::Block,
        )
    };
    let first = snapshot(&rows);
    let mut model = build_in(&mut builder, &first, geometry(&first), None);
    let mut tree = tree(&model);
    let (mut builds, mut pushes, mut diffs, mut changed_diffs) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for frame in 0..FRAMES {
        rows.pop_front();
        rows.push_back(line(ROWS + frame));
        let rebuilt = frame % FRAMES_PER_PUBLICATION == 0;
        if rebuilt {
            let next = snapshot(&rows);
            let started = Instant::now();
            model = builder.build(&Inputs {
                snapshot: &next,
                geometry: geometry(&next),
                selection: None,
                metrics: metrics(),
                scale: SCALE,
            });
            builds.push(started.elapsed());
        }
        let started = Instant::now();
        let sent = update(&model);
        pushes.push(started.elapsed());
        let started = Instant::now();
        tree.update_and_process_changes(sent, &mut Ignore);
        let elapsed = started.elapsed();
        if rebuilt {
            changed_diffs.push(elapsed);
        } else {
            diffs.push(elapsed);
        }
    }
    let total: Duration = builds.iter().sum::<Duration>()
        + pushes.iter().sum::<Duration>()
        + diffs.iter().sum::<Duration>()
        + changed_diffs.iter().sum::<Duration>();
    #[expect(clippy::cast_precision_loss, reason = "frame counts are small")]
    let per_frame = total.as_secs_f64() * 1e6 / FRAMES as f64;
    println!("A11Y_FRAME_COST grid={COLUMNS}x{ROWS} frames={FRAMES}");
    println!(
        "A11Y_FRAME_COST build (publishing frames): {}",
        percentiles(builds)
    );
    println!(
        "A11Y_FRAME_COST push (every frame): {}",
        percentiles(pushes)
    );
    println!(
        "A11Y_FRAME_COST adapter diff, unchanged model: {}",
        percentiles(diffs)
    );
    println!(
        "A11Y_FRAME_COST adapter diff, new model: {}",
        percentiles(changed_diffs)
    );
    println!("A11Y_FRAME_COST mean per frame: {per_frame:.1}us");
}
