//! Incremental viewport captures for building snapshots.

use crate::error::Result;
use crate::ffi::{
    self,
    keys::{
        render_cell_data as cell_data, render_row_data as row_data,
        render_state_data as state_data,
    },
};
use crate::native::{
    AllocatorRef, CellCursor, NativeRenderState, NativeRowCells,
    NativeRowIterator, RowCursor, style_from_ffi,
};
use crate::terminal::Terminal;
use crate::types::{
    Cell, Cursor, CursorStyle, Dirty, Fill, RenderColors, Rgb, Row, Style,
    palette_from_ffi,
};

/// A reusable capture of a terminal's viewport, with its row iterator and
/// cell container allocated once.
///
/// Rows and cells borrow the render state, so an update cannot invalidate
/// them while they are in use:
///
/// ```compile_fail,E0499
/// # fn demo(state: &mut huterm_ghostty::RenderState, terminal: &mut huterm_ghostty::Terminal<()>) -> huterm_ghostty::Result<()> {
/// let mut rows = state.rows()?;
/// let row = rows.next().unwrap();
/// state.update(terminal)?;
/// row.dirty()?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct RenderState {
    state: NativeRenderState,
    rows: NativeRowIterator,
    cells: NativeRowCells,
}

impl RenderState {
    /// Creates an empty render state.
    ///
    /// # Errors
    ///
    /// Fails when allocation fails.
    pub fn new() -> Result<Self> {
        Self::new_in(None)
    }

    pub(crate) fn new_in(allocator: AllocatorRef) -> Result<Self> {
        Ok(Self {
            state: NativeRenderState::new(allocator)?,
            rows: NativeRowIterator::new(allocator)?,
            cells: NativeRowCells::new(allocator)?,
        })
    }

    /// Captures `terminal`'s viewport and accumulates its damage.
    ///
    /// # Errors
    ///
    /// Fails when the terminal is poisoned or allocation fails.
    pub fn update<H>(&mut self, terminal: &mut Terminal<H>) -> Result<()> {
        self.state.update(terminal.native_parts()?)
    }

    /// Global damage since the last [`RenderState::clean`].
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the query.
    pub fn dirty(&self) -> Result<Dirty> {
        let mut dirty = ffi::GHOSTTY_RENDER_STATE_DIRTY_FALSE;
        self.state.get::<state_data::Dirty>(&mut dirty)?;
        Ok(match dirty {
            ffi::GHOSTTY_RENDER_STATE_DIRTY_FALSE => Dirty::Clean,
            ffi::GHOSTTY_RENDER_STATE_DIRTY_PARTIAL => Dirty::Partial,
            _ => Dirty::Full,
        })
    }

    /// The captured cursor.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the query.
    pub fn cursor(&self) -> Result<Cursor> {
        let mut cursor: ffi::GhosttyRenderStateCursor = ffi::sized();
        self.state.get::<state_data::Cursor>(&mut cursor)?;
        let style = match cursor.visual_style {
            ffi::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BAR => {
                CursorStyle::Bar
            }
            ffi::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_UNDERLINE => {
                CursorStyle::Underline
            }
            ffi::GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK_HOLLOW => {
                CursorStyle::BlockHollow
            }
            _ => CursorStyle::Block,
        };
        Ok(Cursor {
            visible: cursor.visible,
            style,
            position: cursor
                .viewport_has_value
                .then_some((cursor.viewport_x, cursor.viewport_y)),
        })
    }

    /// The captured colors.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the query.
    pub fn colors(&self) -> Result<RenderColors> {
        let mut colors: ffi::GhosttyRenderStateColors = ffi::sized();
        self.state.get::<state_data::Colors>(&mut colors)?;
        Ok(RenderColors {
            background: colors.background.into(),
            foreground: colors.foreground.into(),
            cursor: colors.cursor_has_value.then(|| Rgb::from(colors.cursor)),
            palette: palette_from_ffi(&colors.palette),
        })
    }

    /// Consumes the global and every row's damage. Call it only after a
    /// complete frame is built.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the call.
    pub fn clean(&mut self) -> Result<()> {
        self.state.clean()
    }

    #[cfg(test)]
    pub(crate) const fn native(&self) -> &NativeRenderState {
        &self.state
    }

    /// Iterates the captured rows from the top of the viewport.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the iterator.
    pub fn rows(&mut self) -> Result<Rows<'_>> {
        let cursor = self.state.rows(&mut self.rows)?;
        Ok(Rows {
            cursor,
            cells: &mut self.cells,
        })
    }
}

/// The captured rows, borrowed from their render state.
#[derive(Debug)]
pub struct Rows<'s> {
    cursor: RowCursor<'s>,
    cells: &'s mut NativeRowCells,
}

impl Rows<'_> {
    /// Moves to the next row, from the top.
    #[expect(
        clippy::should_implement_trait,
        reason = "each row borrows the iterator, which Iterator cannot express"
    )]
    pub fn next(&mut self) -> Option<RenderRow<'_>> {
        self.cursor.next().then_some(RenderRow {
            cursor: &self.cursor,
            cells: &mut *self.cells,
        })
    }
}

/// One captured row.
#[derive(Debug)]
pub struct RenderRow<'r> {
    cursor: &'r RowCursor<'r>,
    cells: &'r mut NativeRowCells,
}

impl RenderRow<'_> {
    /// Whether the row changed since the last clean.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the query.
    pub fn dirty(&self) -> Result<bool> {
        let mut dirty = false;
        self.cursor.get::<row_data::Dirty>(&mut dirty)?;
        Ok(dirty)
    }

    /// The row value, with wrap and content hints.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the query.
    pub fn row(&self) -> Result<Row> {
        let mut row = 0;
        self.cursor.get::<row_data::Raw>(&mut row)?;
        Ok(Row(row))
    }

    #[cfg(test)]
    pub(crate) const fn cursor(&self) -> &RowCursor<'_> {
        self.cursor
    }

    /// Iterates the row's cells from the left.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the container.
    pub fn cells(&mut self) -> Result<RenderCells<'_>> {
        Ok(RenderCells {
            cursor: self.cursor.cells(self.cells)?,
        })
    }
}

/// The cells of one captured row; methods read the current cell.
#[derive(Debug)]
pub struct RenderCells<'c> {
    cursor: CellCursor<'c>,
}

impl RenderCells<'_> {
    #[cfg(test)]
    pub(crate) const fn cursor(&self) -> &CellCursor<'_> {
        &self.cursor
    }

    /// Moves to the next cell; returns false after the last.
    #[expect(
        clippy::should_implement_trait,
        reason = "cells are read in place rather than yielded"
    )]
    pub fn next(&mut self) -> bool {
        self.cursor.next()
    }

    /// The current cell's value.
    ///
    /// # Errors
    ///
    /// Fails before the first [`RenderCells::next`].
    pub fn cell(&self) -> Result<Cell> {
        let mut cell = 0;
        self.cursor.get::<cell_data::Raw>(&mut cell)?;
        Ok(Cell(cell))
    }

    /// The current cell's style; the default for unstyled cells.
    ///
    /// # Errors
    ///
    /// Fails before the first [`RenderCells::next`].
    pub fn style(&self) -> Result<Style> {
        let mut style: ffi::GhosttyStyle = ffi::sized();
        self.cursor.get::<cell_data::Style>(&mut style)?;
        Ok(style_from_ffi(&style))
    }

    /// Replaces `out` with the current cell's grapheme cluster as UTF-8,
    /// empty when the cell has no text. `out`'s allocation is reused.
    ///
    /// # Errors
    ///
    /// Fails before the first [`RenderCells::next`].
    pub fn graphemes_utf8(&self, out: &mut Vec<u8>) -> Result<()> {
        loop {
            let capacity = out.capacity().max(16);
            out.resize(capacity, 0);
            match self.cursor.graphemes_utf8(out) {
                Ok(Fill::Written(len)) => {
                    out.truncate(len);
                    return Ok(());
                }
                Ok(Fill::TooSmall(len)) if len > out.len() => {
                    out.reserve(len - out.len());
                }
                // A required size that already fits contradicts the call.
                Ok(Fill::TooSmall(_)) => {
                    out.clear();
                    return Err(crate::Error::OutOfSpace);
                }
                Err(error) => {
                    out.clear();
                    return Err(error);
                }
            }
        }
    }
}
