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
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn satisfied_pending_scroll_is_not_replayed_on_invalidation() {
        for (command, returned, history) in [
            (ScrollCommand::Absolute(3), 3, 10),
            (ScrollCommand::Absolute(10), 5, 5),
            (ScrollCommand::Live, 0, 10),
        ] {
            let mut scroll = ScrollController::default();
            scroll.complete(Viewport { bottom_offset: 1 }, 10);
            scroll.invalidate();
            scroll.begin_request().unwrap();
            match command {
                ScrollCommand::Absolute(offset) => {
                    scroll.set_desired(offset);
                }
                ScrollCommand::Live => {
                    scroll.bottom();
                }
                ScrollCommand::Relative(_) => unreachable!(),
            }
            scroll.complete(
                Viewport {
                    bottom_offset: returned,
                },
                history,
            );
            assert!(scroll.begin_request().is_none());
            scroll.invalidate();
            scroll.begin_request().unwrap();
            assert_eq!(scroll.submitted_scroll(), None, "{command:?}");
        }
        let mut scroll = ScrollController::default();
        scroll.complete(Viewport { bottom_offset: 1 }, 10);
        scroll.invalidate();
        scroll.begin_request().unwrap();
        scroll.set_desired(3);
        scroll.complete(Viewport { bottom_offset: 2 }, 10);
        scroll.begin_request().unwrap();
        assert_eq!(scroll.submitted_scroll(), Some(ScrollCommand::Absolute(3)));
    }

    #[test]
    fn failed_snapshot_stops_resubmission_with_pending_scroll() {
        let mut scroll = ScrollController::default();
        scroll.complete(Viewport { bottom_offset: 2 }, 10);
        scroll.scroll_rows(1);
        scroll.begin_request().unwrap();
        scroll.scroll_rows(1);
        scroll.fail();
        assert!(scroll.begin_request().is_none());
        scroll.invalidate();
        scroll.scroll_rows(-1);
        assert!(scroll.begin_request().is_none());
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "runtime fixture and resize assertions remain together"
    )]
    fn height_resize_round_trips_keep_the_top_visible_row_while_scrolled() {
        use huterm_core::TerminalRuntime;
        use huterm_protocol::{
            CellSize, GridSize, TerminalCommand, TerminalId,
        };

        let cell = CellSize {
            width: 8,
            height: 16,
        };
        let runtime = TerminalRuntime::spawn(
            TerminalId::new(1),
            &TerminalCommand {
                program: "/bin/sh".into(),
                arguments: vec!["-c".into(),
                    "i=0; while [ $i -lt 100 ]; do printf 'ROW-%03d\\n' $i; i=$((i+1)); done; printf READY; read line".into()],
                working_directory: std::env::current_dir().unwrap(),
                environment: Vec::new(),
                grid_size: GridSize::clamped(20, 10),
                cell_size: cell,
                presentation: huterm_protocol::TerminalPresentation::default(),
            },
        ).unwrap();
        let client = runtime.client();
        let resize = |rows| {
            client.resize(GridSize::clamped(20, rows), cell).unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while client.read_snapshot().unwrap().size.rows != rows {
                assert!(Instant::now() < deadline, "resize was not applied");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        let initial = loop {
            let snapshot = client.read_snapshot().unwrap();
            let text: String =
                snapshot.cells().map(|cell| cell.text.as_str()).collect();
            if text.contains("READY") {
                break snapshot;
            }
            assert!(Instant::now() < deadline, "fixture did not become ready");
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut controller = ScrollController::default();
        controller.complete(initial.viewport, initial.history_size);
        controller.set_desired(40);
        let settle = |controller: &mut ScrollController| {
            let mut result = None;
            for _ in 0..4 {
                let Some(_viewport) = controller.begin_request() else {
                    return result.unwrap();
                };
                let snapshot =
                    if let Some(scroll) = controller.submitted_scroll() {
                        client.request_scrolled_snapshot(scroll).unwrap()
                    } else {
                        client.request_snapshot().unwrap()
                    };
                let deadline = Instant::now() + Duration::from_secs(3);
                let snapshot = loop {
                    if let Some(reply) = snapshot.try_recv().unwrap() {
                        break reply.snapshot;
                    }
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                };
                controller.complete(snapshot.viewport, snapshot.history_size);
                result = Some(snapshot);
            }
            panic!("viewport did not settle");
        };
        let top = |snapshot: &huterm_protocol::TerminalSnapshot| -> String {
            snapshot.rows[0]
                .cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect()
        };
        let first = settle(&mut controller);
        let expected = top(&first);
        for rows in [6, 14, 10, 6, 14, 10] {
            resize(rows);
            controller.invalidate();
            let snapshot = settle(&mut controller);
            assert_eq!(
                top(&snapshot),
                expected,
                "top row moved at height {rows}"
            );
        }
        controller.bottom();
        settle(&mut controller);
        for rows in [6, 14, 10] {
            resize(rows);
            controller.invalidate();
            let snapshot = settle(&mut controller);
            assert_eq!(snapshot.viewport.bottom_offset, 0);
            assert!(
                snapshot
                    .cells()
                    .map(|cell| cell.text.as_str())
                    .collect::<String>()
                    .contains("READY")
            );
        }
        runtime.shutdown().unwrap();
    }

    fn controller_with_history(history: usize) -> ScrollController {
        let mut controller = ScrollController::default();
        controller.complete(Viewport::default(), history);
        controller
    }

    #[test]
    fn precise_pixels_accumulate_and_reverse_without_stale_remainder() {
        let mut controller = controller_with_history(100);
        assert!(controller.set_desired(10));
        assert!(!controller.scroll_pixels(4.0, 10.0));
        assert!(!controller.scroll_pixels(-7.0, 10.0));
        assert!(controller.scroll_pixels(-4.0, 10.0));
        assert_eq!(controller.desired(), 9);
        assert!(controller.scroll_pixels(11.0, 10.0));
        assert_eq!(controller.desired(), 10);
    }

    #[test]
    fn line_delta_preserves_magnitude_and_bounds() {
        let mut controller = controller_with_history(4);
        assert!(controller.scroll_lines(3.0));
        assert_eq!(controller.desired(), 3);
        assert!(controller.scroll_lines(8.0));
        assert_eq!(controller.desired(), 4);
        assert!(controller.scroll_lines(-2.0));
        assert_eq!(controller.desired(), 2);
    }

    #[test]
    fn clamped_pending_scroll_does_not_leave_debt_after_reversal() {
        let mut scroll = ScrollController::default();
        scroll.complete(Viewport { bottom_offset: 9 }, 10);
        scroll.invalidate();
        scroll.begin_request().unwrap();
        assert!(scroll.scroll_rows(1));
        // Output advances the authoritative viewport to the history boundary.
        scroll.complete(Viewport { bottom_offset: 10 }, 10);
        assert!(scroll.begin_request().is_none());
        assert!(scroll.scroll_rows(-1));
        assert_eq!(scroll.begin_request().unwrap().bottom_offset, 9);
        assert_eq!(
            scroll.submitted_scroll(),
            Some(ScrollCommand::Relative(-1))
        );
    }

    #[test]
    fn requests_coalesce_while_one_is_in_flight() {
        let mut controller = controller_with_history(100);
        controller.invalidate();
        assert_eq!(controller.begin_request(), Some(Viewport::default()));
        controller.scroll_rows(20);
        assert_eq!(controller.begin_request(), None);
        controller.complete(Viewport::default(), 100);
        assert_eq!(
            controller.begin_request(),
            Some(Viewport { bottom_offset: 20 })
        );
    }

    #[test]
    fn growing_history_keeps_scrolled_content_pinned() {
        let mut controller = controller_with_history(100);
        controller.set_desired(10);
        controller.invalidate();
        let _ = controller.begin_request();
        controller.complete(Viewport { bottom_offset: 15 }, 105);
        assert_eq!(controller.desired(), 15);
        assert_eq!(controller.begin_request(), None);
    }

    #[test]
    fn shrinking_history_clamps_the_anchor_and_respects_return_to_bottom() {
        let mut controller = controller_with_history(100);
        controller.set_desired(3);
        let _ = controller.begin_request();
        controller.complete(Viewport { bottom_offset: 0 }, 95);
        assert_eq!(controller.desired(), 0);
        assert_eq!(controller.begin_request(), None);

        let mut controller = controller_with_history(100);
        controller.set_desired(40);
        let _ = controller.begin_request();
        controller.bottom();
        controller.complete(Viewport { bottom_offset: 40 }, 95);
        assert_eq!(controller.desired(), 0);
        assert_eq!(controller.begin_request(), Some(Viewport::default()));
    }

    #[test]
    fn completion_can_launch_a_replacement_without_a_timer_tick() {
        let mut controller = controller_with_history(100);
        controller.invalidate();
        assert_eq!(controller.begin_request(), Some(Viewport::default()));
        controller.scroll_rows(5);
        controller.complete(Viewport::default(), 100);
        assert_eq!(
            controller.begin_request(),
            Some(Viewport { bottom_offset: 5 })
        );
        assert_eq!(controller.diagnostics().requests_started, 2);
        assert_eq!(controller.diagnostics().maximum_concurrent, 1);
    }

    #[test]
    fn full_history_ring_preserves_the_requested_offset() {
        let mut controller = controller_with_history(10_000);
        controller.set_desired(500);
        controller.invalidate();
        let _ = controller.begin_request();
        controller.complete(Viewport { bottom_offset: 500 }, 10_000);
        assert_eq!(controller.desired(), 500);
    }

    #[test]
    fn return_to_bottom_wins_over_growth_during_an_older_request() {
        let mut controller = controller_with_history(100);
        controller.set_desired(10);
        controller.invalidate();
        let _ = controller.begin_request();
        controller.bottom();
        controller.complete(Viewport { bottom_offset: 10 }, 105);

        assert_eq!(controller.desired(), 0);
        assert_eq!(controller.begin_request(), Some(Viewport::default()));
    }
}
