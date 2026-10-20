//! Tier 1 view tests: one `TerminalView` as the root of a GPUI test window,
//! over a standalone terminal runtime. The harness section of
//! `docs/plans/terminal-viewers.md` describes what this tier can and cannot
//! cover.

use std::time::{Duration, Instant};

use gpui::{
    AnyWindowHandle, Entity, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ScrollWheelEvent, Size, Task, TestAppContext,
    TouchPhase, VisualTestContext,
};
use huterm_core::{TerminalRuntime, TerminalViewer, ViewerOptions};
use huterm_protocol::{
    CellSize, GridSize, MouseTracking, TerminalCommand, TerminalId,
    TerminalPresentation, ViewerCapabilities,
};

use super::*;

/// How long a wait may take before the test fails with the last state it
/// observed. Below GPUI's 15-second limit for a parked test thread.
const WAIT_DEADLINE: Duration = Duration::from_secs(10);

/// How often a wait checks its condition when no task woke it sooner.
const WAIT_POLL: Duration = Duration::from_millis(20);

const CELL_WIDTH: f32 = 8.0;
const CELL_HEIGHT: f32 = 16.0;
/// The physical cell size the view reports at the test window's scale of 2.
const CELL: CellSize = CellSize {
    width: 16,
    height: 32,
};

/// A 400×240 window with 4-point padding requests 49×14 cells.
fn window_size() -> Size<Pixels> {
    gpui::size(px(400.0), px(240.0))
}

const BUTTONS: &str = "\\033[?1000h\\033[?1006h";
const ALL_MOTION: &str = "\\033[?1003h\\033[?1006h";

/// A child that prints `history` lines, enables the mouse `modes`, prints
/// `READY`, and then prints every byte it receives as two hex digits.
fn echo_fixture(history: usize, modes: &str) -> String {
    format!(
        "i=0; while [ $i -lt {history} ]; do echo line $i; i=$((i+1)); done; \
         stty raw -echo; printf '{modes}READY'; \
         while byte=$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\n'); \
         [ -n \"$byte\" ]; do printf '%s' \"$byte\"; done"
    )
}

/// An SGR mouse report: `code` 0 is the left button, 35 is motion with no
/// button held.
fn sgr(code: u8, column: u16, row: u16, press: bool) -> String {
    let end = if press { 'M' } else { 'm' };
    format!("\x1b[<{code};{};{}{end}", column + 1, row + 1)
}

fn click(column: u16, row: u16) -> String {
    sgr(0, column, row, true) + &sgr(0, column, row, false)
}

/// One `TerminalView` as the root of a test window, with the drain the tab
/// activity task runs in production.
struct ViewHarness {
    cx: VisualTestContext,
    view: Entity<TerminalView>,
    _drain: Task<()>,
    // Dropped last: closing the runtime stops the child and joins its thread.
    runtime: TerminalRuntime,
}

impl ViewHarness {
    fn new(cx: &mut TestAppContext, fixture: &str) -> Self {
        // The runtime wakes the view from its own threads.
        cx.executor().allow_parking();
        let runtime = TerminalRuntime::spawn(
            TerminalId::new(1),
            &TerminalCommand {
                program: "/bin/sh".into(),
                arguments: vec!["-c".into(), fixture.into()],
                working_directory: std::env::current_dir().unwrap(),
                environment: Vec::new(),
                grid_size: GridSize::clamped(INITIAL_COLUMNS, INITIAL_ROWS),
                cell_size: CELL,
                presentation: TerminalPresentation::default(),
            },
        )
        .unwrap();
        let viewer = runtime
            .subscribe(ViewerOptions::new(ViewerCapabilities::ALL))
            .unwrap();
        let wake = viewer.wake_handle();
        let config = Config::default();
        let window = cx.open_window(window_size(), |window, cx| {
            TerminalView::new(
                viewer,
                refresh::FrameClock::new(window),
                &config,
                "Huterm Test".into(),
                GridMetrics::fixed(CELL_WIDTH, CELL_HEIGHT),
                window,
                cx,
            )
        });
        let view = window.root(cx).unwrap();
        let handle: AnyWindowHandle = window.into();
        let mut cx = VisualTestContext::from_window(handle, cx);
        let drain = cx.update(|window, cx| {
            window.activate_window();
            view.update(cx, |view, cx| {
                // As a window root the view paints at the window's origin, so
                // its content must start there too.
                view.chrome_hidden = true;
                assert_eq!(
                    view.content_bounds(window).origin,
                    gpui::Point::default(),
                    "the harness view must have no chrome above it"
                );
                view.visible = true;
                view.focus.focus(window, cx);
                view.start_initial_snapshot(cx);
                cx.notify();
                let failure_wakes = view.failure_wakes.clone();
                cx.spawn_in(window, async move |view, cx| {
                    loop {
                        let stopped = TerminalView::wait_for_activity(
                            &wake,
                            &failure_wakes,
                        )
                        .await
                        .is_err();
                        loop {
                            let more =
                                view.update_in(cx, |view, window, cx| {
                                    view.refresh(window, cx).more
                                });
                            match more {
                                Ok(true) => {
                                    cx.background_executor()
                                        .timer(Duration::from_millis(1))
                                        .await;
                                }
                                Ok(false) => break,
                                Err(_) => return,
                            }
                        }
                        if stopped {
                            return;
                        }
                    }
                })
            })
        });
        let mut harness = Self {
            cx,
            view,
            _drain: drain,
            runtime,
        };
        harness.wait_for("the view to be focused", |view, window| {
            view.focus.is_focused(window) && window.is_window_active()
        });
        harness
    }

    /// Waits until the child is ready and its mouse tracking mode shows.
    fn wait_ready(&mut self, tracking: MouseTracking) {
        self.wait_for("the child to be ready", |view, _| {
            view.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.modes.mouse_tracking == tracking
            }) && received(view).is_some()
                && view.input_queue.is_empty()
        });
    }

    /// Runs the view's tasks, delivering frames, until `done` holds. Fails
    /// with the last observed state after [`WAIT_DEADLINE`].
    fn wait_for(
        &mut self,
        what: &str,
        mut done: impl FnMut(&TerminalView, &Window) -> bool,
    ) {
        let executor = self.cx.executor();
        let foreground = self.cx.foreground_executor().clone();
        let deadline = Instant::now() + WAIT_DEADLINE;
        let view = self.view.clone();
        let cx = &mut self.cx;
        foreground.block_test(async {
            loop {
                let (satisfied, state) = cx.update(|window, cx| {
                    // GPUI's test mode draws dirty windows but never delivers
                    // frames, and only a frame renews the snapshot allowance.
                    window.simulate_next_frame(cx);
                    let view = view.read(cx);
                    (done(view, window), describe(view))
                });
                if satisfied {
                    return;
                }
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for {what}; last state: {state}"
                );
                executor.timer(WAIT_POLL).await;
            }
        });
    }

    /// Waits until the child has received at least as many bytes as
    /// `expected` holds, then checks that they are exactly `expected`.
    fn assert_received(&mut self, expected: &str) {
        let printable = expected.escape_debug().to_string();
        self.wait_for(
            &format!("the child to receive {printable}"),
            |view, _| {
                received(view)
                    .is_some_and(|bytes| bytes.len() >= expected.len())
            },
        );
        let view = self.view.clone();
        let bytes = self.cx.update(|_, cx| received(view.read(cx)));
        assert_eq!(
            String::from_utf8_lossy(&bytes.unwrap_or_default())
                .escape_debug()
                .to_string(),
            printable,
        );
    }

    /// Runs `f` on the view inside one window update, so no snapshot can be
    /// applied between the steps it takes.
    fn update<R>(
        &mut self,
        f: impl FnOnce(
            &mut TerminalView,
            &mut Window,
            &mut Context<'_, TerminalView>,
        ) -> R,
    ) -> R {
        let view = self.view.clone();
        self.cx.update(|window, cx| {
            view.update(cx, |view, cx| f(view, window, cx))
        })
    }

    fn read<R>(&mut self, f: impl FnOnce(&TerminalView, &Window) -> R) -> R {
        let view = self.view.clone();
        self.cx.update(|window, cx| f(view.read(cx), window))
    }

    /// A second viewer that takes control of the terminal's size, as a
    /// second window would, without a second window.
    fn controller(&mut self, grid: GridSize, cell: CellSize) -> TerminalViewer {
        let controller = self
            .runtime
            .subscribe(ViewerOptions::new(ViewerCapabilities::ALL))
            .unwrap();
        controller.report_geometry(grid, cell).unwrap();
        // Sent after the view's own focus report, so it is the latest.
        controller.set_focus(true).unwrap();
        self.wait_for(
            "the view to display the controller's grid",
            |view, _| {
                view.displayed_grid() == Some(grid)
                    && view.input_queue.is_empty()
            },
        );
        controller
    }

    /// The window position of a cell's centre in the requested grid's
    /// layout, from the view's current metrics.
    fn cell_center(&mut self, column: u16, row: u16) -> gpui::Point<Pixels> {
        self.read(|view, window| {
            let layout = view.terminal_layout(window);
            let origin =
                view.content_bounds(window).origin + layout.bounds.origin;
            gpui::point(
                origin.x + view.metrics.cell_width * (f32::from(column) + 0.5),
                origin.y + view.metrics.cell_height * (f32::from(row) + 0.5),
            )
        })
    }

    fn click(&mut self, position: gpui::Point<Pixels>) {
        self.cx.simulate_mouse_down(
            position,
            MouseButton::Left,
            Modifiers::none(),
        );
        self.cx.simulate_mouse_up(
            position,
            MouseButton::Left,
            Modifiers::none(),
        );
    }
}

fn press(position: gpui::Point<Pixels>) -> MouseDownEvent {
    MouseDownEvent {
        button: MouseButton::Left,
        position,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    }
}

fn release(position: gpui::Point<Pixels>) -> MouseUpEvent {
    MouseUpEvent {
        button: MouseButton::Left,
        position,
        modifiers: Modifiers::none(),
        click_count: 1,
    }
}

fn hover(position: gpui::Point<Pixels>) -> MouseMoveEvent {
    MouseMoveEvent {
        position,
        pressed_button: None,
        modifiers: Modifiers::none(),
    }
}

/// The bytes the echo fixture reports having received, once it is ready.
fn received(view: &TerminalView) -> Option<Vec<u8>> {
    let text = screen_text(view)?;
    let (_, echoed) = text.split_once("READY")?;
    let digits: Vec<u8> =
        echoed.bytes().filter(u8::is_ascii_hexdigit).collect();
    Some(
        digits
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16)
                    .unwrap()
            })
            .collect(),
    )
}

fn screen_text(view: &TerminalView) -> Option<String> {
    let snapshot = view.snapshot.as_ref()?;
    Some(snapshot.cells().map(|cell| cell.text.as_str()).collect())
}

fn describe(view: &TerminalView) -> String {
    let snapshot = view.snapshot.as_ref().map(|snapshot| {
        format!(
            "size={:?} revision={} tracking={:?} text={:?}",
            snapshot.size,
            snapshot.geometry_revision,
            snapshot.modes.mouse_tracking,
            screen_text(view).unwrap_or_default().trim_end(),
        )
    });
    format!(
        "requested={:?} queue_empty={} pending_resize={:?} desired={} hover={:?} snapshot={}",
        view.requested_grid,
        view.input_queue.is_empty(),
        view.pending_resize,
        view.scroll.desired(),
        view.links.hover().map(|link| &link.destination),
        snapshot.unwrap_or_else(|| "none".into()),
    )
}

#[gpui::test]
fn a_click_reaches_the_child_as_one_press_and_one_release(
    cx: &mut TestAppContext,
) {
    let mut harness = ViewHarness::new(cx, &echo_fixture(0, BUTTONS));
    harness.wait_ready(MouseTracking::Buttons);
    let position = harness.cell_center(2, 1);
    harness.click(position);
    harness.assert_received(&click(2, 1));
}

#[gpui::test]
fn a_press_mapped_against_a_grid_the_runtime_has_left_is_discarded(
    cx: &mut TestAppContext,
) {
    let mut harness = ViewHarness::new(cx, &echo_fixture(0, BUTTONS));
    harness.wait_ready(MouseTracking::Buttons);
    harness.wait_for("the view to settle", |view, _| {
        view.displayed_grid() == Some(view.requested_grid)
    });
    let revision = harness
        .read(|view, _| view.snapshot.as_ref().unwrap().geometry_revision);
    harness.update(|view, window, cx| {
        // A layout change: padding above and below takes two rows.
        view.window_config.padding_y += CELL_HEIGHT;
        let before = view.requested_grid;
        view.resize_to_layout(window, false);
        assert_eq!(view.requested_grid.rows, before.rows - 2);
        assert!(view.pending_resize.is_none(), "the resize was refused");
        // Inside both grids, mapped against the displayed one.
        let cell = view
            .grid_geometry(window)
            .cell_bounds(2, 1)
            .unwrap()
            .center();
        view.mouse_down(&press(cell), window, cx);
        view.mouse_up(&release(cell), window, cx);
    });
    harness.wait_for("the snapshot for the new layout", |view, _| {
        view.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.geometry_revision > revision)
            && view.displayed_grid() == Some(view.requested_grid)
    });
    let position = harness.cell_center(4, 3);
    harness.click(position);
    harness.assert_received(&click(4, 3));
}

#[gpui::test]
fn a_hover_report_discarded_as_stale_does_not_suppress_the_next(
    cx: &mut TestAppContext,
) {
    let mut harness = ViewHarness::new(cx, &echo_fixture(0, ALL_MOTION));
    harness.wait_ready(MouseTracking::AllMotion);
    let grid = harness.read(|view, _| view.requested_grid);
    let controller = harness.controller(grid, CELL);
    let revision = harness
        .read(|view, _| view.snapshot.as_ref().unwrap().geometry_revision);
    let position = harness.cell_center(3, 2);
    harness.update(|view, window, cx| {
        // Only the cell size changes, so the displayed grid keeps its size.
        controller
            .report_geometry(
                grid,
                CellSize {
                    width: 18,
                    height: 36,
                },
            )
            .unwrap();
        view.mouse_move(&hover(position), window, cx);
    });
    harness.wait_for("the snapshot with the new revision", |view, _| {
        view.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.geometry_revision > revision)
            && view.input_queue.is_empty()
    });
    harness.update(|view, window, cx| {
        view.mouse_move(&hover(position), window, cx);
    });
    harness.click(position);
    harness.assert_received(&(sgr(35, 3, 2, true) + &click(3, 2)));
}

#[gpui::test]
fn a_larger_canonical_grid_is_clipped_and_owned_gestures_survive_it(
    cx: &mut TestAppContext,
) {
    let mut harness = ViewHarness::new(cx, &echo_fixture(100, BUTTONS));
    harness.wait_ready(MouseTracking::Buttons);
    let requested = harness.read(|view, _| view.requested_grid);
    assert_eq!(requested, GridSize::clamped(49, 14));
    let a = harness.cell_center(2, 1);
    harness
        .cx
        .simulate_mouse_down(a, MouseButton::Left, Modifiers::none());
    harness.assert_received(&sgr(0, 2, 1, true));
    let larger = GridSize::clamped(80, 30);
    let controller = harness.controller(larger, CELL);
    assert_eq!(harness.read(|view, _| view.requested_grid), requested);
    // Far outside the window: the owned release still ends the gesture,
    // clamped to the displayed grid's last cell.
    harness.cx.simulate_mouse_up(
        gpui::point(px(2000.0), px(2000.0)),
        MouseButton::Left,
        Modifiers::none(),
    );
    let mut expected = sgr(0, 2, 1, true) + &sgr(0, 79, 29, false);
    harness.assert_received(&expected);
    // The gesture ended, so another viewer's press is accepted.
    for action in [
        MouseAction::Press(ProtocolMouseButton::Left),
        MouseAction::Release(ProtocolMouseButton::Left),
    ] {
        controller
            .send_input(TerminalInput::Mouse(MouseInput {
                position: MousePosition { column: 5, row: 5 },
                action,
                modifiers: huterm_protocol::Modifiers::default(),
            }))
            .unwrap();
    }
    expected += &click(5, 5);
    harness.assert_received(&expected);
    let b = harness.cell_center(4, 2);
    harness.click(b);
    expected += &click(4, 2);
    harness.assert_received(&expected);
    let rows = harness.update(|view, _, cx| {
        view.scroll_command(cx, |scroll, rows| scroll.page(rows, true));
        view.scroll.desired()
    });
    assert_eq!(rows, usize::from(requested.rows));
    drop(controller);
}

#[gpui::test]
fn the_background_beside_a_smaller_canonical_grid_reports_nothing(
    cx: &mut TestAppContext,
) {
    let mut harness = ViewHarness::new(cx, &echo_fixture(100, ALL_MOTION));
    harness.wait_ready(MouseTracking::AllMotion);
    let controller = harness.controller(GridSize::clamped(20, 5), CELL);
    let background = harness.cell_center(30, 2);
    harness
        .cx
        .simulate_mouse_move(background, None, Modifiers::none());
    harness.click(background);
    let inside = harness.cell_center(2, 1);
    harness.click(inside);
    harness.assert_received(&click(2, 1));
    harness.cx.simulate_event(ScrollWheelEvent {
        position: background,
        delta: ScrollDelta::Lines(gpui::point(0.0, 3.0)),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    let desired = harness.read(|view, _| view.scroll.desired());
    assert_eq!(desired, 3, "the wheel did not scroll the viewport locally");
    drop(controller);
}

#[gpui::test]
fn a_hovered_link_is_found_again_after_a_local_resize_without_control(
    cx: &mut TestAppContext,
) {
    let mut harness = ViewHarness::new(
        cx,
        "printf 'https://example.test/link\\n'; printf READY; read line",
    );
    harness.wait_ready(MouseTracking::Disabled);
    let grid = harness.read(|view, _| view.requested_grid);
    let controller = harness.controller(grid, CELL);
    let macos = cfg!(target_os = "macos");
    harness.update(|view, _, _| {
        view.link_modifiers = config::LinkModifiers::parse(
            if macos { "cmd" } else { "ctrl" },
            macos,
        )
        .unwrap();
    });
    let modifiers = Modifiers {
        platform: macos,
        control: !macos,
        ..Modifiers::default()
    };
    let link = harness.cell_center(3, 0);
    harness.cx.simulate_modifiers_change(modifiers);
    harness.cx.simulate_mouse_move(link, None, modifiers);
    harness.wait_for("the link under the pointer", |view, _| {
        view.links.hover().is_some()
    });
    // A tab bar below the terminal changes the requested grid but neither
    // the origin nor the cell under the pointer, and the terminal's size
    // stays the controller's. (A simulated window resize would also move
    // GPUI's test pointer to the window's corner.)
    harness.update(|view, window, cx| {
        view.tabs_config.position = config::TabPosition::Bottom;
        view.tab_presentation = windows::tab_visibility::Presentation::Reserved;
        assert_eq!(view.content_bounds(window).origin, gpui::Point::default());
        view.resize_to_layout(window, true);
        cx.notify();
    });
    harness.wait_for("the view to request its new grid", |view, _| {
        view.requested_grid != grid && view.displayed_grid() == Some(grid)
    });
    harness.wait_for("the link under the pointer again", |view, _| {
        view.links.hover().is_some()
    });
    drop(controller);
}
