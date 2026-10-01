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
fn height_resize_round_trips_keep_the_top_visible_row_while_scrolled() {
    use huterm_core::TerminalRuntime;
    use huterm_protocol::{CellSize, GridSize, TerminalCommand, TerminalId};

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
            let snapshot = if let Some(scroll) = controller.submitted_scroll() {
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
        assert_eq!(top(&snapshot), expected, "top row moved at height {rows}");
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
    assert_eq!(scroll.submitted_scroll(), Some(ScrollCommand::Relative(-1)));
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
fn input_at_live_output_requests_no_snapshot_but_leaves_history() {
    // Typed input relies on this: its echo requests the next snapshot.
    let mut controller = controller_with_history(100);
    assert!(!controller.bottom());
    assert_eq!(controller.begin_request(), None);

    controller.set_desired(7);
    let _ = controller.begin_request();
    controller.complete(Viewport { bottom_offset: 7 }, 100);
    assert!(controller.bottom());
    assert_eq!(controller.begin_request(), Some(Viewport::default()));
    assert_eq!(controller.submitted_scroll(), Some(ScrollCommand::Live));
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
