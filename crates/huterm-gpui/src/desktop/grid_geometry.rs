//! Hit testing for a view whose displayed grid can differ from the grid it
//! requests: another viewer controls the terminal's size, or the snapshot for
//! this view's newest layout has not arrived yet.

use gpui::{Bounds, Pixels, Point, Size, point, px, size};
use huterm_protocol::{GridSize, MousePosition};

/// The requested grid's painted bounds and the grid the view displays in
/// them, in one coordinate space chosen by the caller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct GridGeometry {
    /// Where the requested grid is painted. Painting clips to these bounds.
    bounds: Bounds<Pixels>,
    cell: Size<Pixels>,
    requested: GridSize,
    /// The size of the snapshot the view shows, if it shows one.
    displayed: Option<GridSize>,
}

impl GridGeometry {
    pub(super) fn new(
        bounds: Bounds<Pixels>,
        cell: Size<Pixels>,
        requested: GridSize,
        displayed: Option<GridSize>,
    ) -> Self {
        Self {
            bounds,
            cell,
            requested,
            displayed,
        }
    }

    /// The grid that cell coordinates refer to: the displayed one, or the
    /// requested one before the first snapshot.
    pub(super) fn grid(&self) -> GridSize {
        self.displayed.unwrap_or(self.requested)
    }

    /// The part of the displayed grid the view shows: the grid's extent at
    /// this cell size, cut to the painted bounds. A viewport narrower than a
    /// cell still shows the part of that cell it paints. Empty before the
    /// first snapshot.
    pub(super) fn visible_area(&self) -> Bounds<Pixels> {
        let Some(grid) = self.displayed else {
            return Bounds::new(self.bounds.origin, Size::default());
        };
        let extent = |cell: Pixels, count: u16, bound: Pixels| {
            (cell * f32::from(count)).min(bound).max(px(0.0))
        };
        Bounds::new(
            self.bounds.origin,
            size(
                extent(self.cell.width, grid.columns, self.bounds.size.width),
                extent(self.cell.height, grid.rows, self.bounds.size.height),
            ),
        )
    }

    /// Whether `position` is over the visible area.
    pub(super) fn contains(&self, position: Point<Pixels>) -> bool {
        let area = self.visible_area();
        let relative = position - area.origin;
        relative.x >= px(0.0)
            && relative.y >= px(0.0)
            && relative.x < area.size.width
            && relative.y < area.size.height
    }

    /// The cell under `position`, clamped to the grid's nearest edge.
    pub(super) fn cell(&self, position: Point<Pixels>) -> MousePosition {
        mouse_position(position - self.bounds.origin, self.grid(), self.cell)
    }

    /// Where a cell is drawn, if any part of it is visible.
    pub(super) fn cell_bounds(
        &self,
        column: u16,
        row: u16,
    ) -> Option<Bounds<Pixels>> {
        let grid = self.grid();
        let area = self.visible_area();
        let origin = self.bounds.origin
            + point(
                self.cell.width * f32::from(column),
                self.cell.height * f32::from(row),
            );
        (column < grid.columns
            && row < grid.rows
            && origin.x < area.right()
            && origin.y < area.bottom())
        .then(|| Bounds::new(origin, self.cell))
    }

    /// Which way a selection drag at `position` scrolls: into history above
    /// the visible area, towards live output below it.
    pub(super) fn edge_direction(&self, position: Point<Pixels>) -> i64 {
        let area = self.visible_area();
        if position.y < area.origin.y {
            1
        } else if position.y >= area.bottom() {
            -1
        } else {
            0
        }
    }
}

/// Rows one page scrolls: the displayed rows the view can show. A taller
/// canonical grid is cut to the requested rows, and a page of its full
/// height would skip the rows the view cannot show.
pub(super) fn page_rows(
    requested: GridSize,
    displayed: Option<GridSize>,
) -> u16 {
    displayed.map_or(requested.rows, |displayed| {
        displayed.rows.min(requested.rows)
    })
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite coordinates clamp to the u16 grid before conversion"
)]
pub(super) fn mouse_position(
    position: Point<Pixels>,
    grid: GridSize,
    cell: Size<Pixels>,
) -> MousePosition {
    let coordinate = |value: Pixels, dimension: Pixels, count: u16| {
        let value = f32::from(value);
        let dimension = f32::from(dimension);
        if !value.is_finite() || !dimension.is_finite() || dimension <= 0.0 {
            return 0;
        }
        (value / dimension)
            .floor()
            .clamp(0.0, f32::from(count.saturating_sub(1))) as u32
    };
    MousePosition {
        column: coordinate(position.x, cell.width, grid.columns),
        row: coordinate(position.y, cell.height, grid.rows),
    }
}

#[cfg(test)]
mod tests;
