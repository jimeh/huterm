//! Reuses retained snapshot rows whose content has not changed.

use std::sync::Arc;

use huterm_protocol::{Cell, TerminalRow};

/// Offset from a current viewport row to the retained row that held the same
/// screen line: positive when output or scrolling moved content upward.
pub(super) fn viewport_shift(
    (old_bottom, old_history): (usize, usize),
    (bottom, history): (usize, usize),
) -> isize {
    let top = |bottom: usize, history: usize| {
        i128::try_from(history).unwrap_or(i128::MAX)
            - i128::try_from(bottom).unwrap_or(i128::MAX)
    };
    isize::try_from(top(bottom, history) - top(old_bottom, old_history))
        .unwrap_or(isize::MAX)
}

/// Finds a retained row with identical cells so unchanged content keeps its
/// `Arc`. Rows are immutable, so any equal row is valid, even at another
/// index. The history-derived shift misses once scrollback reaches its byte
/// budget, so a bounded scan recovers the offset without trusting one anchor.
pub(super) struct RowMatcher<'a> {
    previous: &'a [Arc<TerminalRow>],
    shift: isize,
    last: Option<isize>,
    scan_budget: usize,
}

impl<'a> RowMatcher<'a> {
    pub(super) fn new(previous: &'a [Arc<TerminalRow>], shift: isize) -> Self {
        Self {
            previous,
            shift,
            last: None,
            // Enough for one failed scan, such as a changed first row, plus
            // the scan that finds the shifted rows below it.
            scan_budget: previous.len().saturating_mul(2),
        }
    }

    pub(super) fn find(
        &mut self,
        index: usize,
        cells: &[Cell],
    ) -> Option<&'a Arc<TerminalRow>> {
        let offsets = [Some(0), Some(self.shift), self.last];
        for (position, offset) in offsets.iter().enumerate() {
            let Some(offset) = *offset else {
                continue;
            };
            if offsets[..position].contains(&Some(offset)) {
                continue;
            }
            if let Some(row) = index
                .checked_add_signed(offset)
                .and_then(|row| self.previous.get(row))
                && row.cells == cells
            {
                self.last = Some(offset);
                return Some(row);
            }
        }
        let previous = self.previous;
        for (row_index, row) in previous.iter().enumerate() {
            if self.scan_budget == 0 {
                return None;
            }
            self.scan_budget -= 1;
            if row.cells == cells {
                self.last = isize::try_from(row_index)
                    .ok()
                    .zip(isize::try_from(index).ok())
                    .map(|(old, new)| old - new);
                return Some(row);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use huterm_protocol::{CellColor, CellStyle};

    fn row(text: &str) -> Arc<TerminalRow> {
        Arc::new(TerminalRow {
            cells: text
                .chars()
                .map(|character| Cell {
                    text: character.into(),
                    foreground: CellColor::DefaultForeground,
                    background: CellColor::DefaultBackground,
                    style: CellStyle::default(),
                })
                .collect(),
        })
    }

    /// Matches `current` rows against `previous`, returning retained indices.
    fn matches(
        previous: &[Arc<TerminalRow>],
        shift: isize,
        current: &[&str],
    ) -> Vec<Option<usize>> {
        let mut matcher = RowMatcher::new(previous, shift);
        current
            .iter()
            .enumerate()
            .map(|(index, text)| {
                matcher.find(index, &row(text).cells).map(|found| {
                    previous
                        .iter()
                        .position(|row| Arc::ptr_eq(row, found))
                        .unwrap()
                })
            })
            .collect()
    }

    #[test]
    fn row_matcher_recovers_shifts_the_history_size_cannot_report() {
        let previous = ["    ", "    ", "A   ", "B   "].map(row);
        // At the scrollback budget, history stays constant, so the derived
        // shift is zero even though content moved up one row.
        assert_eq!(
            matches(&previous, 0, &["    ", "A   ", "B   ", "C   "]),
            [Some(0), Some(2), Some(3), None]
        );
        let previous = ["A   ", "B   ", "C   ", "D   "].map(row);
        assert_eq!(
            matches(&previous, 0, &["Z   ", "C   ", "D   ", "E   "]),
            [None, Some(2), Some(3), None]
        );
        // A correct derived shift matches without scanning.
        assert_eq!(
            matches(&previous, 2, &["C   ", "D   ", "E   ", "F   "]),
            [Some(2), Some(3), None, None]
        );
    }
}
