use huterm_protocol::{ScrollCommand, Viewport};

#[derive(Debug, Default)]
pub(super) struct ScrollController {
    desired: usize,
    pending_scroll: Option<ScrollCommand>,
    submitted_scroll: Option<ScrollCommand>,
    displayed: usize,
    history: usize,
    pixel_remainder: f32,
    in_flight: bool,
    failed: bool,
    dirty: bool,
    diagnostics: ScrollDiagnostics,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ScrollDiagnostics {
    pub(super) requests_started: u64,
    pub(super) requests_completed: u64,
    pub(super) requests_coalesced: u64,
    pub(super) maximum_concurrent: usize,
    pub(super) queued_updates: u64,
    pub(super) maximum_queued: usize,
}

impl ScrollController {
    pub(super) fn reset_wheel(&mut self) {
        self.pixel_remainder = 0.0;
    }

    pub(super) fn has_pending_scroll(&self) -> bool {
        self.pending_scroll.is_some()
    }

    pub(super) fn desired(&self) -> usize {
        self.desired
    }

    pub(super) fn displayed(&self) -> usize {
        self.displayed
    }

    pub(super) fn history(&self) -> usize {
        self.history
    }

    pub(super) fn diagnostics(&self) -> ScrollDiagnostics {
        self.diagnostics
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "finite input is consumed only in whole-row increments"
    )]
    pub(super) fn scroll_pixels(
        &mut self,
        pixels: f32,
        row_height: f32,
    ) -> bool {
        if !pixels.is_finite()
            || !row_height.is_finite()
            || row_height <= 0.0
            || pixels == 0.0
        {
            return false;
        }
        if self.pixel_remainder.signum() != pixels.signum() {
            self.pixel_remainder = 0.0;
        }
        self.pixel_remainder += pixels;
        let rows = (self.pixel_remainder / row_height).trunc();
        if rows == 0.0 {
            return false;
        }
        self.pixel_remainder -= rows * row_height;
        self.scroll_rows(rows as i64)
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "finite platform line deltas are rounded to whole rows"
    )]
    pub(super) fn scroll_lines(&mut self, lines: f32) -> bool {
        if !lines.is_finite() || lines == 0.0 {
            return false;
        }
        self.pixel_remainder = 0.0;
        self.scroll_rows(lines.round() as i64)
    }

    pub(super) fn scroll_rows(&mut self, rows: i64) -> bool {
        let previous = self.desired;
        if rows > 0 {
            self.desired = self
                .desired
                .saturating_add(usize::try_from(rows).unwrap_or(usize::MAX))
                .min(self.history);
        } else {
            self.desired = self.desired.saturating_sub(
                usize::try_from(rows.unsigned_abs()).unwrap_or(usize::MAX),
            );
        }
        let changed = self.desired != previous;
        if changed {
            let delta = i64::try_from(self.desired).unwrap_or(i64::MAX)
                - i64::try_from(previous).unwrap_or(i64::MAX);
            self.pending_scroll = Some(match self.pending_scroll {
                Some(ScrollCommand::Absolute(_) | ScrollCommand::Live) => {
                    ScrollCommand::Absolute(self.desired)
                }
                Some(ScrollCommand::Relative(pending)) => {
                    ScrollCommand::Relative(pending.saturating_add(delta))
                }
                None => ScrollCommand::Relative(delta),
            });
        }
        changed
    }

    pub(super) fn page(&mut self, rows: u16, upward: bool) -> bool {
        self.scroll_rows(if upward {
            i64::from(rows)
        } else {
            -i64::from(rows)
        })
    }

    pub(super) fn bottom(&mut self) -> bool {
        let changed = self.desired != 0;
        self.desired = 0;
        if changed || self.in_flight {
            self.pending_scroll = Some(ScrollCommand::Live);
        }
        self.pixel_remainder = 0.0;
        changed
    }

    pub(super) fn set_desired(&mut self, offset: usize) -> bool {
        let offset = offset.min(self.history);
        let changed = self.desired != offset;
        self.desired = offset;
        if changed {
            self.pending_scroll = Some(ScrollCommand::Absolute(offset));
        }
        self.pixel_remainder = 0.0;
        changed
    }

    pub(super) fn invalidate(&mut self) {
        self.dirty = true;
    }

    pub(super) fn begin_request(&mut self) -> Option<Viewport> {
        if self.failed {
            return None;
        }
        if self.in_flight {
            if self.dirty || self.desired != self.displayed {
                self.diagnostics.requests_coalesced =
                    self.diagnostics.requests_coalesced.saturating_add(1);
                self.diagnostics.queued_updates =
                    self.diagnostics.queued_updates.saturating_add(1);
                self.diagnostics.maximum_queued = 1;
            }
            return None;
        }
        if !self.dirty && self.desired == self.displayed {
            return None;
        }
        self.in_flight = true;
        self.submitted_scroll = self.pending_scroll.take();
        self.dirty = false;
        self.diagnostics.requests_started =
            self.diagnostics.requests_started.saturating_add(1);
        self.diagnostics.maximum_concurrent =
            self.diagnostics.maximum_concurrent.max(1);
        Some(Viewport {
            bottom_offset: self.desired,
        })
    }

    pub(super) fn submitted_scroll(&self) -> Option<ScrollCommand> {
        self.submitted_scroll
    }

    pub(super) fn complete(&mut self, viewport: Viewport, history: usize) {
        self.history = history;
        self.displayed = viewport.bottom_offset.min(history);
        self.desired = match self.pending_scroll {
            None => self.displayed,
            Some(ScrollCommand::Live) => 0,
            Some(ScrollCommand::Absolute(offset)) => offset.min(history),
            Some(ScrollCommand::Relative(delta)) if delta >= 0 => self
                .displayed
                .saturating_add(usize::try_from(delta).unwrap_or(usize::MAX))
                .min(history),
            Some(ScrollCommand::Relative(delta)) => {
                self.displayed.saturating_sub(
                    usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX),
                )
            }
        };
        if matches!(self.pending_scroll, Some(ScrollCommand::Relative(_))) {
            let delta = i64::try_from(self.desired).unwrap_or(i64::MAX)
                - i64::try_from(self.displayed).unwrap_or(i64::MAX);
            self.pending_scroll =
                (delta != 0).then_some(ScrollCommand::Relative(delta));
        }
        if self.desired == self.displayed {
            self.pending_scroll = None;
        }
        self.in_flight = false;
        self.submitted_scroll = None;
        self.diagnostics.requests_completed =
            self.diagnostics.requests_completed.saturating_add(1);
    }

    pub(super) fn fail(&mut self) {
        self.failed = true;
        self.pending_scroll = None;
        self.submitted_scroll = None;
        self.dirty = false;
        self.in_flight = false;
    }
}

#[cfg(test)]
mod tests;
