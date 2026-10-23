use gpui::{Bounds, point, px, size};
use huterm_protocol::{GridSize, MousePosition};

use super::super::TerminalLayout;
use super::*;
use crate::config::WindowConfig;

const CELL: Size<Pixels> = size(px(8.0), px(16.0));
/// Where the window's content starts, below a title row.
const CONTENT_TOP: f32 = 32.0;

/// A 105×59 viewport with default padding requests 12×3 cells painted from
/// (4, 4) to (100, 52), below a 32-point title row.
fn requested() -> TerminalLayout {
    TerminalLayout::new(
        size(px(105.0), px(59.0)),
        CELL,
        WindowConfig::default(),
    )
}

fn geometry(
    layout: TerminalLayout,
    displayed: Option<GridSize>,
) -> GridGeometry {
    GridGeometry::new(
        Bounds::new(
            point(px(0.0), px(CONTENT_TOP)) + layout.bounds.origin,
            layout.bounds.size,
        ),
        CELL,
        layout.grid,
        displayed,
    )
}

const EQUAL: (u16, u16) = (12, 3);
const LARGER: (u16, u16) = (20, 6);
const SMALLER: (u16, u16) = (6, 2);

fn grid((columns, rows): (u16, u16)) -> GridSize {
    GridSize::clamped(columns, rows)
}

fn at(x: f32, y: f32) -> Point<Pixels> {
    point(px(x), px(y))
}

#[test]
fn the_inside_test_accepts_exactly_the_visible_area() {
    let equal = geometry(requested(), Some(grid(EQUAL)));
    let larger = geometry(requested(), Some(grid(LARGER)));
    let smaller = geometry(requested(), Some(grid(SMALLER)));
    // The first and last painted pixels of the requested grid.
    for geometry in [equal, larger] {
        assert!(geometry.contains(at(4.0, 36.0)));
        assert!(geometry.contains(at(99.9, 83.9)));
    }
    // The title row, the padding on each side, a larger grid past the clip,
    // and far outside the window.
    for geometry in [equal, larger, smaller] {
        for (x, y) in [
            (4.0, 31.0),
            (3.9, 40.0),
            (10.0, 35.9),
            (100.0, 40.0),
            (10.0, 84.0),
            (-100.0, -100.0),
            (1000.0, 1000.0),
        ] {
            assert!(!geometry.contains(at(x, y)), "{geometry:?} {x} {y}");
        }
    }
    // A smaller grid ends at its own last cell; the background beside and
    // below it is outside.
    assert!(smaller.contains(at(51.9, 67.9)));
    assert!(!smaller.contains(at(52.0, 40.0)));
    assert!(!smaller.contains(at(10.0, 68.0)));
    // Before the first snapshot nothing is displayed.
    assert!(!geometry(requested(), None).contains(at(4.0, 36.0)));
}

#[test]
fn cells_map_into_the_displayed_grid_and_clamp_to_its_nearest_edge() {
    let cases = [
        (EQUAL, MousePosition { column: 11, row: 2 }),
        (LARGER, MousePosition { column: 19, row: 5 }),
        (SMALLER, MousePosition { column: 5, row: 1 }),
    ];
    for (displayed, last) in cases {
        let geometry = geometry(requested(), Some(grid(displayed)));
        assert_eq!(geometry.cell(at(4.0, 36.0)), MousePosition::default());
        assert_eq!(geometry.cell(at(-100.0, -100.0)), MousePosition::default());
        assert_eq!(geometry.cell(at(1000.0, 1000.0)), last, "{displayed:?}");
    }
    // Inside the requested grid a larger grid's cells keep their columns;
    // past the clip they continue beyond the visible area.
    let larger = geometry(requested(), Some(grid(LARGER)));
    assert_eq!(
        larger.cell(at(99.9, 83.9)),
        MousePosition { column: 11, row: 2 }
    );
    assert_eq!(
        larger.cell(at(120.0, 100.0)),
        MousePosition { column: 14, row: 4 }
    );
    for dimension in [0.0, -1.0, f32::INFINITY, f32::NAN] {
        assert_eq!(
            mouse_position(
                at(50.0, 50.0),
                grid(EQUAL),
                size(px(dimension), px(dimension))
            ),
            MousePosition::default()
        );
    }
}

#[test]
fn a_viewport_smaller_than_one_cell_accepts_only_what_it_paints() {
    // 13×24 with 4-point padding leaves 5×16: one requested cell, painted
    // five points wide.
    let partial = TerminalLayout::new(
        size(px(13.0), px(24.0)),
        CELL,
        WindowConfig::default(),
    );
    assert_eq!(partial.grid, GridSize::clamped(1, 1));
    let geometry = geometry(partial, Some(grid(LARGER)));
    assert!(geometry.contains(at(8.9, 51.9)));
    assert!(!geometry.contains(at(9.0, 40.0)));
    assert!(geometry.cell_bounds(0, 0).is_some());
    assert_eq!(page_rows(partial.grid, Some(grid(LARGER))), 1);
    // Padding that consumes the viewport leaves nothing to hit.
    for padding_balance in [false, true] {
        let empty = TerminalLayout::new(
            size(px(3.0), px(2.0)),
            CELL,
            WindowConfig {
                padding_balance,
                ..WindowConfig::default()
            },
        );
        assert_eq!(empty.bounds.size, Size::default());
        let geometry = super::GridGeometry::new(
            empty.bounds,
            CELL,
            empty.grid,
            Some(grid(LARGER)),
        );
        assert!(!geometry.contains(empty.bounds.origin));
        assert!(geometry.cell_bounds(0, 0).is_none());
        assert_eq!(page_rows(empty.grid, Some(grid(LARGER))), 1);
    }
}

#[test]
fn pages_scroll_the_rows_the_view_shows_and_the_scrollbar_counts_displayed_rows()
 {
    let requested = requested().grid;
    assert_eq!(page_rows(requested, Some(grid(LARGER))), 3);
    assert_eq!(page_rows(requested, Some(grid(SMALLER))), 2);
    assert_eq!(page_rows(requested, Some(grid(EQUAL))), 3);
    assert_eq!(page_rows(requested, None), 3);
    assert_eq!(
        geometry(self::requested(), Some(grid(LARGER))).grid(),
        grid(LARGER)
    );
    assert_eq!(geometry(self::requested(), None).grid(), requested);
}

#[test]
fn selection_drags_scroll_past_the_edges_of_the_visible_area() {
    let smaller = geometry(requested(), Some(grid(SMALLER)));
    assert_eq!(smaller.edge_direction(at(10.0, 35.9)), 1);
    assert_eq!(smaller.edge_direction(at(10.0, 67.9)), 0);
    // Below a shorter grid, still inside the requested bounds.
    assert_eq!(smaller.edge_direction(at(10.0, 68.0)), -1);
    let larger = geometry(requested(), Some(grid(LARGER)));
    assert_eq!(larger.edge_direction(at(10.0, 83.9)), 0);
    // At the clip, though the grid continues below it.
    assert_eq!(larger.edge_direction(at(10.0, 84.0)), -1);
}

#[test]
fn a_smaller_grid_starts_at_the_requested_origin_with_balanced_padding() {
    let balanced = TerminalLayout::new(
        size(px(111.0), px(59.0)),
        CELL,
        WindowConfig {
            padding_balance: true,
            ..WindowConfig::default()
        },
    );
    assert_eq!(balanced.bounds.origin, point(px(7.5), px(4.0)));
    let geometry = super::GridGeometry::new(
        balanced.bounds,
        CELL,
        balanced.grid,
        Some(grid(SMALLER)),
    );
    assert_eq!(
        geometry.cell_bounds(0, 0).map(|bounds| bounds.origin),
        Some(balanced.bounds.origin)
    );
    assert_eq!(
        geometry.cell(at(15.5, 20.0)),
        MousePosition { column: 1, row: 1 }
    );
}

#[test]
fn a_cell_beyond_the_clip_has_no_visible_bounds() {
    let larger = geometry(requested(), Some(grid(LARGER)));
    assert_eq!(
        larger.cell_bounds(11, 2),
        Some(Bounds::new(at(92.0, 68.0), CELL))
    );
    assert_eq!(larger.cell_bounds(12, 0), None);
    assert_eq!(larger.cell_bounds(0, 3), None);
    // Outside the grid itself.
    assert_eq!(
        geometry(requested(), Some(grid(SMALLER))).cell_bounds(6, 0),
        None
    );
}
