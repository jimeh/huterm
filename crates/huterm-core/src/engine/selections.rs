//! Viewers' settled selections, each held by two Ghostty tracked references
//! so it follows its text through output, scrolling, and reflow.
//!
//! Resolution reads only reference metadata, never cells, so it restores no
//! compressed page. A failure to create or resolve a reference drops that
//! viewer's selection, which then reads as gone; it never fails the runtime.

use huterm_ghostty::{
    CellWidth, Mode, Point, PointSpace, Screen, Terminal, TrackedRef,
};
use huterm_protocol::{
    BufferPoint, BufferRange, GridSize, SelectionPlacement, TrackedSelection,
    ViewerId,
};

/// DEC private mode 7, autowrap (DECAWM). Ghostty reflows the primary
/// screen on a width change only while it is set.
const WRAPAROUND: Mode = Mode::dec(7);

/// Both ends of a selection on the screen it was made on.
#[derive(Debug)]
struct Ends {
    screen: Screen,
    start: TrackedRef,
    end: TrackedRef,
}

impl Ends {
    /// Tracks `range` on the active screen, or `None` when it lies outside
    /// the buffer or Ghostty cannot track it.
    fn new<H>(terminal: &mut Terminal<H>, range: BufferRange) -> Option<Self> {
        if BufferRange::ordered(range.start, range.end) != range {
            return None;
        }
        let (columns, _) = terminal.size().ok()?;
        let last = terminal.total_rows().ok()?.checked_sub(1)?;
        let point = |point: BufferPoint| {
            if point.column >= columns {
                return None;
            }
            let row = last.checked_sub(point.rows_from_live_bottom)?;
            Some(Point::screen(point.column, u32::try_from(row).ok()?))
        };
        let (start, end) = (point(range.start)?, point(range.end)?);
        let screen = terminal.screen().ok()?;
        let start = terminal.track(start).ok()?;
        let end = terminal.track(end).ok()?;
        Some(Self { screen, start, end })
    }

    /// The selection's ends as screen points in reading order, or `None`
    /// when its text is gone or another screen is active. A start that lost
    /// its value lost only the selection's oldest rows, since Ghostty
    /// discards cells in reading order, so the oldest retained cell stands
    /// in for it.
    fn points<H>(&self, terminal: &Terminal<H>) -> Option<(Point, Point)> {
        if terminal.screen().ok()? != self.screen {
            return None;
        }
        let end = terminal
            .tracked_point(&self.end, PointSpace::Screen)
            .ok()??;
        let start = terminal
            .tracked_point(&self.start, PointSpace::Screen)
            .ok()?
            .unwrap_or(Point::screen(0, 0));
        // In-place erasure can move both ends onto one replacing row.
        Some(if (start.y, start.x) <= (end.y, end.x) {
            (start, end)
        } else {
            (end, start)
        })
    }

    fn range<H>(&self, terminal: &Terminal<H>) -> Option<BufferRange> {
        let (start, end) = self.points(terminal)?;
        let last = terminal.total_rows().ok()?.checked_sub(1)?;
        let point = |point: Point| {
            Some(BufferPoint {
                rows_from_live_bottom: last
                    .checked_sub(usize::try_from(point.y).ok()?)?,
                column: point.x,
            })
        };
        Some(BufferRange {
            start: point(start)?,
            end: point(end)?,
        })
    }
}

#[derive(Debug)]
struct Entry {
    viewer: ViewerId,
    id: u64,
    /// `None` once the selection is gone. It stays gone, so a selection
    /// whose loss a viewer saw never comes back.
    ends: Option<Ends>,
}

impl Entry {
    /// Resolves the selection, dropping it when it is gone.
    fn points<H>(&mut self, terminal: &Terminal<H>) -> Option<(Point, Point)> {
        let points = self.ends.as_ref()?.points(terminal);
        if points.is_none() {
            self.ends = None;
        }
        points
    }
}

/// What a resize must finish for each selection that survived its start.
#[derive(Debug, Default)]
pub(super) struct PendingResize {
    /// Per tracked selection, whether its start and end were not already on
    /// a spacer head before a column change, so a reflow that leaves one
    /// there moved it off its character.
    normalize: Vec<(ViewerId, [bool; 2])>,
}

/// At most one tracked selection per viewer.
#[derive(Debug, Default)]
pub(super) struct Selections {
    entries: Vec<Entry>,
}

impl Selections {
    /// Replaces `viewer`'s selection, or forgets it for `None`. A selection
    /// made against another generation than `generation` is gone at once.
    pub(super) fn track<H>(
        &mut self,
        terminal: &mut Terminal<H>,
        generation: u64,
        viewer: ViewerId,
        selection: Option<TrackedSelection>,
    ) {
        // Free the old references before taking new ones.
        self.forget(viewer);
        let Some(selection) = selection else {
            return;
        };
        let ends = (selection.generation == generation)
            .then(|| Ends::new(terminal, selection.range))
            .flatten();
        self.entries.push(Entry {
            viewer,
            id: selection.id,
            ends,
        });
    }

    pub(super) fn forget(&mut self, viewer: ViewerId) {
        self.entries.retain(|entry| entry.viewer != viewer);
    }

    /// Where `viewer`'s selection lies now; `None` when it tracks none.
    pub(super) fn placement<H>(
        &mut self,
        terminal: &Terminal<H>,
        viewer: ViewerId,
    ) -> Option<SelectionPlacement> {
        let entry = self.entry(viewer)?;
        let range = entry.ends.as_ref().and_then(|ends| ends.range(terminal));
        if range.is_none() {
            entry.ends = None;
        }
        Some(SelectionPlacement {
            id: entry.id,
            range,
        })
    }

    /// The screen points of `viewer`'s selection `id`, for reading its
    /// text; `None` when it tracks another selection or this one is gone.
    pub(super) fn points<H>(
        &mut self,
        terminal: &Terminal<H>,
        viewer: ViewerId,
        id: u64,
    ) -> Option<(Point, Point)> {
        self.entry(viewer)
            .filter(|entry| entry.id == id)?
            .points(terminal)
    }

    /// Drops the selections a resize from `old` to `new` cannot keep, and
    /// notes what [`Self::finish_resize`] must check after it. Call it
    /// before resizing.
    pub(super) fn prepare_resize<H>(
        &mut self,
        terminal: &Terminal<H>,
        old: GridSize,
        new: GridSize,
    ) -> PendingResize {
        let mut pending = PendingResize::default();
        if old.columns == new.columns && old.rows == new.rows {
            return pending;
        }
        // Ghostty clips the alternate screen without reflow, and clips the
        // primary one too when a narrower width meets wraparound off.
        let clipped = new.columns < old.columns
            && !terminal.mode(WRAPAROUND).unwrap_or(false);
        let columns_changed = old.columns != new.columns;
        for entry in &mut self.entries {
            let Some(ends) = &entry.ends else {
                continue;
            };
            if ends.screen == Screen::Alternate || clipped {
                entry.ends = None;
                continue;
            }
            if !columns_changed {
                continue;
            }
            // Cells are read from the active screen. A selection on the
            // other one resolves as gone anyway.
            if terminal.screen().ok() != Some(ends.screen) {
                continue;
            }
            let [start, end] = [&ends.start, &ends.end].map(|reference| {
                terminal
                    .tracked_point(reference, PointSpace::Screen)
                    .ok()
                    .flatten()
                    .is_some_and(|point| {
                        spacer_head(terminal, point) == Some(false)
                    })
            });
            pending.normalize.push((entry.viewer, [start, end]));
        }
        pending
    }

    /// Moves an end that a reflow left on a spacer head onto the wide
    /// character the wrap pushed to the next row: the start onto its first
    /// column and the end onto its second.
    pub(super) fn finish_resize<H>(
        &mut self,
        terminal: &mut Terminal<H>,
        pending: PendingResize,
    ) {
        for (viewer, [check_start, check_end]) in pending.normalize {
            let Some(entry) = self.entry(viewer) else {
                continue;
            };
            let Some(ends) = &mut entry.ends else {
                continue;
            };
            let mut normalize = |reference: &mut TrackedRef, column: u16| {
                let Some(point) = terminal
                    .tracked_point(reference, PointSpace::Screen)
                    .ok()
                    .flatten()
                else {
                    // A reference without a value resolves as before.
                    return true;
                };
                if spacer_head(terminal, point) != Some(true) {
                    return true;
                }
                let row = point.y.saturating_add(1);
                let wide = terminal
                    .grid_ref(Point::screen(0, row))
                    .and_then(|cell| cell.cell())
                    .and_then(huterm_ghostty::Cell::width)
                    .is_ok_and(|width| width == CellWidth::Wide);
                if !wide {
                    return true;
                }
                match terminal.track(Point::screen(column, row)) {
                    Ok(moved) => {
                        *reference = moved;
                        true
                    }
                    Err(_) => false,
                }
            };
            let kept = (!check_start || normalize(&mut ends.start, 0))
                && (!check_end || normalize(&mut ends.end, 1));
            if !kept {
                entry.ends = None;
            }
        }
    }

    /// Selections that still hold references, for tests that bound them.
    #[cfg(test)]
    pub(super) fn tracked(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.ends.is_some())
            .count()
    }

    fn entry(&mut self, viewer: ViewerId) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|entry| entry.viewer == viewer)
    }
}

/// Whether the cell at the screen `point` is a spacer head; `None` when it
/// cannot be read.
fn spacer_head<H>(terminal: &Terminal<H>, point: Point) -> Option<bool> {
    let width = terminal.grid_ref(point).ok()?.cell().ok()?.width().ok()?;
    Some(width == CellWidth::SpacerHead)
}
