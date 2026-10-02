//! Viewers on live PTY terminals: two or more viewers of one terminal unless
//! a test says otherwise.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use huterm_protocol::{
    BufferPoint, BufferRange, CellSize, GridSize, HostEffect, Modifiers,
    MouseAction, MouseButton, MouseInput, MousePosition, ScrollCommand,
    TerminalCommand, TerminalId, TerminalInput, TerminalLifecycle,
    TerminalPresentation, TerminalSnapshot, ViewerCapabilities,
};

use super::{HostEffectViewerOptions, TerminalViewer, ViewerOptions};
use crate::terminal::RuntimeControl;
use crate::{
    DesktopHostEffectClient, HostEffectRecipientOptions, RuntimeError,
    TerminalRuntime,
};

const DEADLINE: Duration = Duration::from_secs(5);

fn cell() -> CellSize {
    CellSize {
        width: 8,
        height: 16,
    }
}

fn spawn(id: u64, script: &str) -> TerminalRuntime {
    TerminalRuntime::spawn(
        TerminalId::new(id),
        &TerminalCommand {
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), script.into()],
            working_directory: std::env::current_dir().unwrap(),
            environment: Vec::new(),
            grid_size: GridSize::clamped(40, 8),
            cell_size: cell(),
            presentation: TerminalPresentation::default(),
        },
    )
    .unwrap()
}

fn viewer(runtime: &TerminalRuntime) -> TerminalViewer {
    runtime
        .subscribe(ViewerOptions::new(ViewerCapabilities::ALL))
        .unwrap()
}

/// A viewer that controls the size from the start: it is focused and
/// reports `columns` x `rows`.
fn sized_viewer(
    runtime: &TerminalRuntime,
    columns: u16,
    rows: u16,
) -> TerminalViewer {
    runtime
        .subscribe(ViewerOptions {
            focused: true,
            geometry: Some((GridSize::clamped(columns, rows), cell())),
            ..ViewerOptions::new(ViewerCapabilities::ALL)
        })
        .unwrap()
}

fn text(snapshot: &TerminalSnapshot) -> String {
    snapshot.cells().map(|cell| cell.text.as_str()).collect()
}

fn wait_for_text(
    viewer: &TerminalViewer,
    needle: &str,
) -> Arc<TerminalSnapshot> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let snapshot = viewer.read_snapshot().unwrap();
        if text(&snapshot).contains(needle) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "missing {needle:?}: {:?}",
            text(&snapshot)
        );
        thread::sleep(Duration::from_millis(10));
    }
}

/// Polls `condition` until it holds, reporting `what` on timeout.
fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + DEADLINE;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// Echoes `name=rows columns.` for each line it reads, so tests read the
/// PTY size the shell actually sees.
const SIZE_SCRIPT: &str = "stty -echo; printf READY; while IFS= read -r l; do printf '%s=%s.' \"$l\" \"$(stty size)\"; done";

/// Reads the PTY size the shell sees, typing through `prober`, which must
/// not be able to take control.
fn pty_size(prober: &TerminalViewer, name: &str) -> String {
    prober
        .send_input(TerminalInput::Text(format!("{name}\n")))
        .unwrap();
    read_size(prober, name)
}

/// Reads the size the shell printed for `name`.
fn read_size(prober: &TerminalViewer, name: &str) -> String {
    let marker = format!("{name}=");
    let mut size = None;
    wait_until(&marker, || {
        let text = text(&prober.read_snapshot().unwrap());
        size = text.find(&marker).and_then(|start| {
            let rest = &text[start + marker.len()..];
            rest.find('.').map(|end| rest[..end].to_owned())
        });
        size.is_some()
    });
    size.unwrap()
}

/// A viewer that types and reads but can never take control.
fn prober(runtime: &TerminalRuntime) -> TerminalViewer {
    runtime
        .subscribe(ViewerOptions::new(ViewerCapabilities {
            input: true,
            ..ViewerCapabilities::NONE
        }))
        .unwrap()
}

/// Returns once the runtime has processed every queued input. Controls run
/// ahead of queued messages, so a control round trip alone is no barrier;
/// once the input bytes are released, the next control round trip follows
/// the turn that processed the last input.
fn drained(runtime: &TerminalRuntime) {
    let client = runtime.client();
    wait_until("queued input", || client.queued_input_bytes() == 0);
    client.job_context().unwrap();
}

/// Holds the runtime owner until the returned sender is dropped or sent on.
fn pause(runtime: &TerminalRuntime) -> mpsc::Sender<()> {
    let (entered, paused) = mpsc::channel();
    let (resume, release) = mpsc::channel();
    runtime
        .client()
        .send_control(RuntimeControl::Pause { entered, release })
        .unwrap();
    paused.recv_timeout(DEADLINE).expect("runtime should pause");
    resume
}

#[test]
fn every_viewer_wakes_and_a_stalled_viewer_blocks_no_one() {
    let runtime = spawn(
        200,
        "stty -echo; printf READY; while IFS= read -r l; do printf 'GOT:%s.' \"$l\"; done",
    );
    let active = viewer(&runtime);
    let stalled = viewer(&runtime);
    wait_for_text(&active, "READY");
    stalled.read_snapshot().unwrap();
    while stalled.wake_pending() {}
    assert!(stalled.poll().invalidated.is_none());
    for index in 0..50 {
        active
            .send_input(TerminalInput::Text(format!("{index}\n")))
            .unwrap();
    }
    // The stalled viewer never asks for a snapshot. Parsing and the
    // active viewer continue regardless.
    wait_for_text(&active, "GOT:49.");
    assert!(stalled.wake_pending(), "the stalled viewer was woken");
    assert!(stalled.poll().invalidated.is_some());
    // When it finally asks, its snapshot is complete and current.
    let snapshot = stalled.read_snapshot().unwrap();
    assert!(text(&snapshot).contains("GOT:49."));
    assert_eq!(snapshot.rows.len(), usize::from(snapshot.size.rows));
    runtime.shutdown().unwrap();
}

#[test]
fn a_late_viewer_reads_current_status_and_a_complete_snapshot() {
    let runtime = spawn(201, "printf '\\033]2;late title\\007OUTPUT'; exit 3");
    let first = viewer(&runtime);
    // Exit can be observed before the final output is parsed.
    wait_for_text(&first, "OUTPUT");
    wait_until("exit and title", || {
        let status = runtime.registry().status();
        status.lifecycle != TerminalLifecycle::Running
            && status.title == "late title"
    });
    let late = viewer(&runtime);
    let update = late.poll();
    let status = update.status.unwrap();
    assert_eq!(status.title, "late title");
    assert!(matches!(
        status.lifecycle,
        TerminalLifecycle::Exited(exit) if exit.code == Some(3)
    ));
    assert!(
        update.exited.is_none(),
        "the exit happened before it existed"
    );
    assert!(text(&late.read_snapshot().unwrap()).contains("OUTPUT"));
    drop(first);
    runtime.shutdown().unwrap();
}

#[test]
fn a_spawning_viewer_sees_an_exit_that_beat_its_subscription() {
    let runtime = spawn(202, "exit 4");
    wait_until("exit", || {
        runtime.registry().status().lifecycle != TerminalLifecycle::Running
    });
    let spawning = runtime
        .subscribe(ViewerOptions {
            spawned: true,
            ..ViewerOptions::new(ViewerCapabilities::ALL)
        })
        .unwrap();
    assert_eq!(spawning.poll().exited.and_then(|exit| exit.code), Some(4));
    assert!(spawning.poll().exited.is_none(), "reported once");
    runtime.shutdown().unwrap();
}

#[test]
fn geometry_still_resizes_the_emulator_after_root_exit() {
    let runtime = spawn(203, "printf DONE; exit 0");
    let only = viewer(&runtime);
    wait_until("exit", || {
        runtime.registry().status().lifecycle != TerminalLifecycle::Running
    });
    only.report_geometry(GridSize::clamped(60, 12), cell())
        .unwrap();
    wait_until("emulator resize", || {
        only.read_snapshot().unwrap().size == GridSize::clamped(60, 12)
    });
    runtime.shutdown().unwrap();
}

#[test]
fn a_scroll_from_one_viewer_wakes_and_moves_the_other() {
    let runtime = spawn(
        204,
        "i=0; while [ $i -lt 40 ]; do printf 'row %s\\n' $i; i=$((i+1)); done; printf READY; read l",
    );
    let scroller = viewer(&runtime);
    let watcher = viewer(&runtime);
    let live = wait_for_text(&scroller, "READY");
    watcher.read_snapshot().unwrap();
    assert!(watcher.poll().invalidated.is_none());

    let moved = scroller
        .request_scrolled_snapshot(ScrollCommand::Relative(10))
        .unwrap()
        .recv_blocking()
        .unwrap()
        .snapshot;
    assert_eq!(moved.viewport.bottom_offset, 10);
    assert!(
        scroller.poll().invalidated.is_none(),
        "the scrolling viewer is not woken by its own scroll"
    );
    assert_eq!(watcher.poll().invalidated, Some(live.generation));
    let seen = watcher.read_snapshot().unwrap();
    assert_eq!(seen.viewport.bottom_offset, 10);
    assert_eq!(seen.generation, live.generation);
    assert!(Arc::ptr_eq(&seen, &moved), "both read one snapshot");
    let selection = watcher
        .request_selection(
            live.generation,
            BufferRange {
                start: BufferPoint {
                    rows_from_live_bottom: 10,
                    column: 0,
                },
                end: BufferPoint {
                    rows_from_live_bottom: 10,
                    column: 5,
                },
            },
        )
        .unwrap();
    assert!(selection.recv_blocking().unwrap().is_some());
    runtime.shutdown().unwrap();
}

#[test]
fn typing_returns_the_shared_viewport_to_live_without_a_snapshot() {
    let runtime = spawn(
        205,
        "stty -echo; i=0; while [ $i -lt 40 ]; do printf 'row %s\\n' $i; i=$((i+1)); done; printf READY; read l; read l",
    );
    let scroller = viewer(&runtime);
    let typist = viewer(&runtime);
    wait_for_text(&scroller, "READY");
    scroller
        .request_scrolled_snapshot(ScrollCommand::Relative(10))
        .unwrap()
        .recv_blocking()
        .unwrap();
    typist.send_input(TerminalInput::Text("x".into())).unwrap();
    wait_until("return to live", || {
        scroller.poll().invalidated.is_some()
            && scroller.read_snapshot().unwrap().viewport.bottom_offset == 0
    });
    runtime.shutdown().unwrap();
}

#[test]
fn a_key_queued_before_a_later_scroll_leaves_the_scroll_in_place() {
    let runtime = spawn(
        206,
        "stty -echo; i=0; while [ $i -lt 40 ]; do printf 'row %s\\n' $i; i=$((i+1)); done; printf READY; read l; read l",
    );
    let viewer = viewer(&runtime);
    wait_for_text(&viewer, "READY");
    let resume = pause(&runtime);
    // Typed first, but the scroll control overtakes it on the owner thread.
    let stamp = viewer.stamp(None);
    viewer
        .offer_input(TerminalInput::Text("x".into()), stamp)
        .unwrap();
    let scrolled = viewer
        .request_scrolled_snapshot(ScrollCommand::Relative(10))
        .unwrap();
    resume.send(()).unwrap();
    assert_eq!(
        scrolled
            .recv_blocking()
            .unwrap()
            .snapshot
            .viewport
            .bottom_offset,
        10
    );
    drained(&runtime);
    assert_eq!(viewer.read_snapshot().unwrap().viewport.bottom_offset, 10);
    runtime.shutdown().unwrap();
}

#[test]
fn concurrent_input_from_two_viewers_arrives_intact() {
    let runtime = spawn(
        207,
        "stty -echo; printf READY; while IFS= read -r l; do printf '[%s]' \"$l\"; done",
    );
    let first = viewer(&runtime);
    let second = viewer(&runtime);
    wait_for_text(&first, "READY");
    thread::scope(|scope| {
        for (viewer, name) in [(&first, 'a'), (&second, 'b')] {
            scope.spawn(move || {
                for index in 0..5 {
                    viewer
                        .send_input(TerminalInput::Text(format!(
                            "{name}{index}{name}\n"
                        )))
                        .unwrap();
                }
            });
        }
    });
    let mut text = String::new();
    wait_until("both final markers", || {
        text = self::text(&first.read_snapshot().unwrap());
        text.contains("[a4a]") && text.contains("[b4b]")
    });
    for name in ['a', 'b'] {
        for index in 0..5 {
            assert!(text.contains(&format!("[{name}{index}{name}]")), "{text}");
        }
    }
    runtime.shutdown().unwrap();
}

#[test]
fn capabilities_are_enforced_by_the_runtime() {
    let runtime = spawn(208, SIZE_SCRIPT);
    let writer = sized_viewer(&runtime, 40, 8);
    let probe = prober(&runtime);
    let reader = runtime
        .subscribe(ViewerOptions::new(ViewerCapabilities::NONE))
        .unwrap();
    wait_for_text(&writer, "READY");
    assert!(matches!(
        reader.send_input(TerminalInput::Text("x\n".into())),
        Err(RuntimeError::NotPermitted)
    ));
    assert!(matches!(
        reader.clear_history(),
        Err(RuntimeError::NotPermitted)
    ));
    assert!(matches!(
        reader.request_scrolled_snapshot(ScrollCommand::Live),
        Err(RuntimeError::NotPermitted)
    ));
    assert!(matches!(
        reader.report_geometry(GridSize::clamped(90, 30), cell()),
        Err(RuntimeError::NotPermitted)
    ));
    assert!(matches!(
        reader.set_focus(true),
        Err(RuntimeError::NotPermitted)
    ));
    assert!(reader.read_snapshot().is_ok(), "every viewer can read");
    assert_eq!(runtime.client().queued_input_bytes(), 0);
    // A read-only viewer that may size takes control by focus.
    let sizer = runtime
        .subscribe(ViewerOptions {
            geometry: Some((GridSize::clamped(50, 9), cell())),
            ..ViewerOptions::new(ViewerCapabilities {
                size: true,
                ..ViewerCapabilities::NONE
            })
        })
        .unwrap();
    assert_eq!(pty_size(&probe, "before"), "8 40");
    sizer.set_focus(true).unwrap();
    assert_eq!(pty_size(&probe, "after"), "9 50");
    runtime.shutdown().unwrap();
}

#[test]
fn only_the_controlling_viewer_resizes_the_pty() {
    let runtime = spawn(209, SIZE_SCRIPT);
    let first = sized_viewer(&runtime, 40, 8);
    let second = runtime
        .subscribe(ViewerOptions::new(ViewerCapabilities::ALL))
        .unwrap();
    let probe = prober(&runtime);
    wait_for_text(&first, "READY");
    second
        .report_geometry(GridSize::clamped(70, 14), cell())
        .unwrap();
    assert_eq!(pty_size(&probe, "passive"), "8 40");
    let before = first.read_snapshot().unwrap();
    first.set_focus(false).unwrap();
    second.set_focus(true).unwrap();
    // The resize the handoff caused reaches every viewer's snapshot, with
    // no output to republish it.
    wait_until("the handoff resize in a snapshot", || {
        first.read_snapshot().unwrap().size == GridSize::clamped(70, 14)
    });
    assert!(
        first.read_snapshot().unwrap().geometry_revision
            > before.geometry_revision
    );
    assert_eq!(pty_size(&probe, "handoff"), "14 70");
    // Dropping the controller hands control back.
    drop(second);
    assert_eq!(pty_size(&probe, "dropped"), "8 40");
    runtime.shutdown().unwrap();
}

#[test]
fn a_hidden_single_viewer_still_resizes_the_pty() {
    let runtime = spawn(210, SIZE_SCRIPT);
    let only = sized_viewer(&runtime, 40, 8);
    let probe = prober(&runtime);
    wait_for_text(&only, "READY");
    only.set_focus(false).unwrap();
    only.report_geometry(GridSize::clamped(44, 10), cell())
        .unwrap();
    // The report alone resizes: typing, even through a viewer that cannot
    // control, would recompute the controller.
    wait_until("resize from the report", || {
        only.read_snapshot().unwrap().size == GridSize::clamped(44, 10)
    });
    assert_eq!(pty_size(&probe, "hidden"), "10 44");
    runtime.shutdown().unwrap();
}

#[test]
fn a_revoked_viewer_with_a_queued_snapshot_harms_no_one() {
    let runtime = spawn(211, "printf READY; read l");
    let revoked = viewer(&runtime);
    let survivor = viewer(&runtime);
    wait_for_text(&survivor, "READY");
    let resume = pause(&runtime);
    let queued = revoked.request_snapshot().unwrap();
    runtime
        .registry()
        .revoke_where(|slot| slot.id == revoked.id());
    resume.send(()).unwrap();
    assert!(matches!(queued.recv_blocking(), Err(RuntimeError::Revoked)));
    assert!(revoked.poll().revoked);
    // Once finalized, the revoked viewer's waiter stops instead of hanging.
    wait_until("finalized slot", || {
        runtime
            .registry()
            .slots()
            .iter()
            .all(|slot| slot.id != revoked.id())
    });
    while revoked.wake_pending() {}
    assert!(matches!(ready(revoked.wait()), Err(RuntimeError::Stopped)));
    assert!(matches!(
        revoked.send_input(TerminalInput::Text("x".into())),
        Err(RuntimeError::Revoked)
    ));
    assert!(text(&survivor.read_snapshot().unwrap()).contains("READY"));
    assert!(
        runtime
            .client()
            .job_context()
            .is_some_and(|job| !job.exited),
        "the terminal keeps running"
    );
    runtime.shutdown().unwrap();
}

/// Enables SGR mouse tracking, reads `first` bytes, stops tracking, and
/// resumes it after one more byte; then prints the next press it receives.
fn tracking_script(first: usize) -> String {
    format!(
        "stty raw -echo; printf '\\033[?1000h\\033[?1006hREADY'; dd bs=1 count={first} >/dev/null 2>&1; printf '\\033[?1000lOFF'; dd bs=1 count=1 >/dev/null 2>&1; printf '\\033[?1000hON'; bytes=$(dd bs=1 count=9 2>/dev/null | od -An -tx1 | tr -d ' \\n'); printf 'HEX:%s:END' \"$bytes\""
    )
}

/// Reports focus and SGR mouse input, then prints the hex of the first
/// `count` bytes it receives.
fn reporting_script(count: usize) -> String {
    format!(
        "stty raw -echo; printf '\\033[?1004h\\033[?1000h\\033[?1006hREADY'; bytes=$(dd bs=1 count={count} 2>/dev/null | od -An -tx1 | tr -d ' \\n'); printf 'HEX:%s:END' \"$bytes\""
    )
}

fn hex(bytes: &[u8]) -> String {
    let mut hex = String::new();
    for byte in bytes {
        write!(hex, "{byte:02x}").unwrap();
    }
    hex
}

fn press(button: MouseButton, column: u32) -> TerminalInput {
    TerminalInput::Mouse(MouseInput {
        position: MousePosition { column, row: 0 },
        action: MouseAction::Press(button),
        modifiers: Modifiers::default(),
    })
}

#[test]
fn revoking_an_idle_viewer_releases_its_button_then_reports_focus_out() {
    let expected = b"\x1b[I\x1b[<0;3;1M\x1b[<0;3;1m\x1b[O";
    let runtime = spawn(212, &reporting_script(expected.len()));
    let viewer = viewer(&runtime);
    wait_for_text(&viewer, "READY");
    viewer.set_focus(true).unwrap();
    viewer.send_input(press(MouseButton::Left, 2)).unwrap();
    drained(&runtime);
    runtime.registry().revoke_all();
    let reader = runtime
        .subscribe(ViewerOptions::new(ViewerCapabilities::NONE))
        .unwrap();
    wait_for_text(&reader, &format!("HEX:{}:END", hex(expected)));
    runtime.shutdown().unwrap();
}

#[test]
fn a_dropped_viewer_writes_its_queued_press_before_the_release() {
    let expected = b"\x1b[I\x1b[<0;5;1Mx\x1b[<0;5;1m\x1b[O";
    let runtime = spawn(213, &reporting_script(expected.len()));
    let reader = runtime
        .subscribe(ViewerOptions::new(ViewerCapabilities::NONE))
        .unwrap();
    let dropped = viewer(&runtime);
    wait_for_text(&reader, "READY");
    // Every request is queued before the drop and dequeued after it.
    let resume = pause(&runtime);
    dropped.set_focus(true).unwrap();
    dropped.send_input(press(MouseButton::Left, 4)).unwrap();
    dropped.send_input(TerminalInput::Text("x".into())).unwrap();
    drop(dropped);
    resume.send(()).unwrap();
    wait_for_text(&reader, &format!("HEX:{}:END", hex(expected)));
    runtime.shutdown().unwrap();
}

#[test]
fn a_single_viewer_reports_each_focus_change_once() {
    let expected = b"\x1b[I\x1b[O\x1b[I";
    let runtime = spawn(214, &reporting_script(expected.len()));
    let only = viewer(&runtime);
    wait_for_text(&only, "READY");
    for focused in [true, true, false, false, true] {
        only.set_focus(focused).unwrap();
    }
    wait_for_text(&only, &format!("HEX:{}:END", hex(expected)));
    runtime.shutdown().unwrap();
}

#[test]
fn a_clipboard_write_reaches_exactly_one_recipient() {
    let runtime = spawn(
        215,
        "stty -echo; printf READY; while IFS= read -r l; do printf '\\033]52;c;%s\\007<%s>' \"$(printf '%s' \"$l\" | base64)\" \"$l\"; done",
    );
    let process = DesktopHostEffectClient::new();
    let clipboard = || {
        runtime
            .subscribe(ViewerOptions {
                host_effects: Some(HostEffectViewerOptions {
                    process: process.clone(),
                    options: HostEffectRecipientOptions::local_desktop(true),
                }),
                ..ViewerOptions::new(ViewerCapabilities::ALL)
            })
            .unwrap()
    };
    let first = clipboard();
    let second = clipboard();
    // Typing is activity, so the writes come from a viewer that receives
    // no host effects; focus and registration order pick the recipient.
    let typist = prober(&runtime);
    wait_for_text(&first, "READY");
    let copied = |viewer: &TerminalViewer| {
        viewer.host_effects().unwrap().try_next().map(|pending| {
            match pending.effect() {
                HostEffect::ClipboardWrite(write) => write.text().to_owned(),
                effect => panic!("unexpected effect {effect:?}"),
            }
        })
    };
    // The host-effect unit tests prove the recipient's own wake; output
    // wakes every viewer here, so this checks delivery only.
    let copy = |text: &str, to: &TerminalViewer| {
        typist
            .send_input(TerminalInput::Text(format!("{text}\n")))
            .unwrap();
        wait_until("clipboard write", || {
            copied(to).is_some_and(|copied| copied == text)
        });
    };
    // With no activity, the latest registration receives.
    copy("one", &second);
    assert!(copied(&first).is_none());
    // Focus hands the recipient over.
    first.set_focus(true).unwrap();
    copy("two", &first);
    assert!(copied(&second).is_none());
    // Dropping the recipient never redirects; the next write goes to the
    // remaining viewer.
    drop(first);
    copy("three", &second);
    // Revocation leaves no recipient.
    runtime.registry().revoke_all();
    runtime.host_effect_sink().invalidate_all();
    let typist = prober(&runtime);
    typist
        .send_input(TerminalInput::Text("four\n".into()))
        .unwrap();
    // The marker follows the clipboard write in the same output.
    wait_for_text(&typist, "<four>");
    assert!(copied(&second).is_none());
    runtime.shutdown().unwrap();
}

#[test]
fn both_viewers_keep_one_viewport_across_the_alternate_screen() {
    let runtime = spawn(
        216,
        "stty -echo; i=0; while [ $i -lt 40 ]; do printf 'row %s\\n' $i; i=$((i+1)); done; printf READY; read l; printf '\\033[?1049hALT'; read l",
    );
    let first = viewer(&runtime);
    let second = viewer(&runtime);
    wait_for_text(&first, "READY");
    first
        .request_scrolled_snapshot(ScrollCommand::Relative(5))
        .unwrap()
        .recv_blocking()
        .unwrap();
    // Reading text input from the second viewer would return to live, so
    // the reader without viewport control sends it.
    let reader = runtime
        .subscribe(ViewerOptions::new(ViewerCapabilities {
            viewport: false,
            ..ViewerCapabilities::ALL
        }))
        .unwrap();
    reader
        .send_input(TerminalInput::Text("go\n".into()))
        .unwrap();
    wait_until("alternate screen", || {
        first.read_snapshot().unwrap().modes.alternate_screen
    });
    let (one, two) = (
        first.read_snapshot().unwrap(),
        second.read_snapshot().unwrap(),
    );
    assert_eq!(one.viewport, two.viewport);
    assert_eq!(one.history_size, two.history_size);
    runtime.shutdown().unwrap();
}

#[test]
fn a_terminal_admits_a_bounded_number_of_viewers() {
    let runtime = spawn(217, "read l");
    let mut viewers: Vec<_> =
        (0..super::VIEWER_LIMIT).map(|_| viewer(&runtime)).collect();
    assert!(matches!(
        runtime.subscribe(ViewerOptions::new(ViewerCapabilities::ALL)),
        Err(RuntimeError::ViewerLimit)
    ));
    drop(viewers.pop());
    wait_until("finalized slot", || {
        runtime
            .subscribe(ViewerOptions::new(ViewerCapabilities::ALL))
            .is_ok()
    });
    runtime.shutdown().unwrap();
}

#[test]
fn closing_with_live_viewers_reaps_the_child_and_closes_their_wakes() {
    let runtime = spawn(218, "printf READY; sleep 30");
    let first = viewer(&runtime);
    let second = viewer(&runtime);
    wait_for_text(&first, "READY");
    let root = runtime.client().job_context().unwrap().shell.unwrap();
    let root = nix::unistd::Pid::from_raw(i32::try_from(root).unwrap());
    runtime.shutdown().unwrap();
    assert!(nix::sys::signal::kill(root, None).is_err(), "child reaped");
    for viewer in [&first, &second] {
        while viewer.wake_pending() {}
        assert!(matches!(ready(viewer.wait()), Err(RuntimeError::Stopped)));
        // The final status stays readable after the wake closes.
        let _ = viewer.poll();
        assert!(viewer.read_snapshot().is_err());
    }
}

#[test]
fn a_viewer_registered_mid_turn_keeps_its_first_reports() {
    let runtime = spawn(219, SIZE_SCRIPT);
    let first = sized_viewer(&runtime, 40, 8);
    let probe = prober(&runtime);
    wait_for_text(&first, "READY");
    // The owner is held inside a turn that has already reconciled its
    // viewers, so the new viewer's reports are dequeued before the next.
    let resume = pause(&runtime);
    let late = viewer(&runtime);
    late.report_geometry(GridSize::clamped(70, 14), cell())
        .unwrap();
    late.set_focus(true).unwrap();
    probe
        .send_input(TerminalInput::Text("late\n".into()))
        .unwrap();
    resume.send(()).unwrap();
    // A snapshot request would reconcile ahead of the queued reports, so
    // wait for the probe's input, queued behind them, without one.
    let client = runtime.client();
    wait_until("queued input", || client.queued_input_bytes() == 0);
    assert_eq!(read_size(&probe, "late"), "14 70");
    runtime.shutdown().unwrap();
}

#[test]
fn viewer_ids_are_unique_across_terminals_in_one_scope() {
    let first = spawn(220, "read l");
    let second = spawn(221, "read l");
    let (one, two) = (viewer(&first), viewer(&second));
    assert_ne!(one.id(), two.id());
    first.shutdown().unwrap();
    second.shutdown().unwrap();
}

#[test]
fn an_initial_geometry_is_clamped_like_a_reported_one() {
    let runtime = spawn(222, "printf READY; read l");
    let only = runtime
        .subscribe(ViewerOptions {
            focused: true,
            geometry: Some((
                GridSize {
                    columns: 0,
                    rows: 0,
                },
                cell(),
            )),
            ..ViewerOptions::new(ViewerCapabilities::ALL)
        })
        .unwrap();
    wait_until("clamped resize", || {
        only.read_snapshot().unwrap().size == GridSize::clamped(1, 1)
    });
    assert!(
        runtime
            .client()
            .job_context()
            .is_some_and(|job| !job.exited),
        "the terminal keeps running"
    );
    runtime.shutdown().unwrap();
}

#[test]
fn a_gesture_ends_when_the_application_stops_tracking_the_mouse() {
    let runtime = spawn(223, &tracking_script(9));
    let first = viewer(&runtime);
    let second = viewer(&runtime);
    wait_for_text(&first, "READY");
    // The application quits tracking while the first viewer holds Left,
    // so that viewer never sends its release.
    first.send_input(press(MouseButton::Left, 2)).unwrap();
    wait_for_text(&first, "OFF");
    second.send_input(TerminalInput::Text("y".into())).unwrap();
    wait_for_text(&first, "ON");
    second.send_input(press(MouseButton::Left, 4)).unwrap();
    wait_for_text(&first, &format!("HEX:{}:END", hex(b"\x1b[<0;5;1M")));
    runtime.shutdown().unwrap();
}

#[test]
fn a_press_dequeued_after_tracking_stops_takes_no_gesture() {
    let runtime = spawn(226, &tracking_script(1));
    let first = viewer(&runtime);
    let second = viewer(&runtime);
    wait_for_text(&first, "READY");
    second.send_input(TerminalInput::Text("y".into())).unwrap();
    wait_for_text(&first, "OFF");
    // Computed against a display that still showed tracking, so its view
    // never sends the release.
    first.send_input(press(MouseButton::Left, 2)).unwrap();
    second.send_input(TerminalInput::Text("z".into())).unwrap();
    wait_for_text(&first, "ON");
    second.send_input(press(MouseButton::Left, 4)).unwrap();
    wait_for_text(&first, &format!("HEX:{}:END", hex(b"\x1b[<0;5;1M")));
    runtime.shutdown().unwrap();
}

#[test]
fn typing_supersedes_an_earlier_scroll_that_is_applied_late() {
    let runtime = spawn(
        227,
        "stty -echo; i=0; while [ $i -lt 40 ]; do printf 'row %s\\n' $i; i=$((i+1)); done; printf READY; read l; read l",
    );
    let viewer = viewer(&runtime);
    wait_for_text(&viewer, "READY");
    let resume = pause(&runtime);
    // Filling the paused turn's control budget leaves the scroll for the
    // next turn, after the typing the user produced once it was sent.
    let client = runtime.client();
    for _ in 1..crate::terminal::MESSAGE_CAPACITY {
        client.send_control(RuntimeControl::Wake).unwrap();
    }
    let scrolled = viewer
        .request_scrolled_snapshot(ScrollCommand::Relative(10))
        .unwrap();
    viewer.send_input(TerminalInput::Text("x".into())).unwrap();
    resume.send(()).unwrap();
    assert_eq!(
        scrolled
            .recv_blocking()
            .unwrap()
            .snapshot
            .viewport
            .bottom_offset,
        0
    );
    drained(&runtime);
    assert_eq!(viewer.read_snapshot().unwrap().viewport.bottom_offset, 0);
    runtime.shutdown().unwrap();
}

#[test]
fn reconciliation_waits_while_writes_are_backed_up() {
    let runtime =
        spawn(228, "stty raw -echo; printf '\\033[?1004hREADY'; sleep 30");
    let typist = prober(&runtime);
    wait_for_text(&typist, "READY");
    let client = runtime.client();
    let pending = || {
        let (reply, receiver) = mpsc::channel();
        client
            .send_control(RuntimeControl::PendingWrites(reply))
            .unwrap();
        receiver.recv_timeout(DEADLINE).unwrap()
    };
    // The application never reads, so writes back up behind the PTY.
    let chunk = "x".repeat(4096);
    wait_until("backed-up writes", || {
        match typist.send_input(TerminalInput::Text(chunk.clone())) {
            Ok(()) | Err(RuntimeError::Busy) => {}
            Err(error) => panic!("input failed: {error}"),
        }
        pending() > 0
    });
    let backlog = pending();
    // Reconciling and then finalizing a focused viewer writes a focus
    // report each time; churn must not grow the backlog.
    let mut refused = false;
    for _ in 0..2 * super::VIEWER_LIMIT {
        match runtime.subscribe(ViewerOptions {
            focused: true,
            ..ViewerOptions::new(ViewerCapabilities::ALL)
        }) {
            Ok(churned) => {
                churned.read_snapshot().unwrap();
            }
            Err(RuntimeError::ViewerLimit) => {
                refused = true;
                break;
            }
            Err(error) => panic!("subscription failed: {error}"),
        }
    }
    assert!(
        refused,
        "dropped viewers stay registered during the backlog"
    );
    assert_eq!(pending(), backlog);
    runtime.shutdown().unwrap();
}

#[test]
fn a_dropped_viewer_s_focus_reports_run_before_it_is_finalized() {
    let expected = b"\x1b[Ix";
    let runtime = spawn(229, &reporting_script(expected.len()));
    let reader = runtime
        .subscribe(ViewerOptions::new(ViewerCapabilities::NONE))
        .unwrap();
    wait_for_text(&reader, "READY");
    let leaving = runtime
        .subscribe(ViewerOptions {
            focused: true,
            ..ViewerOptions::new(ViewerCapabilities::ALL)
        })
        .unwrap();
    let arriving = viewer(&runtime);
    // Reconciles both viewers, writing the first focus in.
    leaving.read_snapshot().unwrap();
    let resume = pause(&runtime);
    // Focus moves to the other window, then the first window closes; the
    // application stays focused throughout.
    arriving.set_focus(true).unwrap();
    leaving.set_focus(false).unwrap();
    drop(leaving);
    arriving
        .send_input(TerminalInput::Text("x".into()))
        .unwrap();
    resume.send(()).unwrap();
    wait_for_text(&reader, &format!("HEX:{}:END", hex(expected)));
    runtime.shutdown().unwrap();
}

#[test]
fn a_closed_host_effect_sink_refuses_a_viewer_without_registering_it() {
    let runtime = spawn(230, "read l");
    let _first = viewer(&runtime);
    runtime.host_effect_sink().close();
    let refused = runtime.subscribe(ViewerOptions {
        host_effects: Some(HostEffectViewerOptions {
            process: DesktopHostEffectClient::new(),
            options: HostEffectRecipientOptions::local_desktop(true),
        }),
        focused: true,
        ..ViewerOptions::new(ViewerCapabilities::ALL)
    });
    assert!(matches!(refused, Err(RuntimeError::Stopped)));
    assert_eq!(runtime.registry().slots().len(), 1);
    runtime.shutdown().unwrap();
}

#[test]
fn refused_requests_leave_nothing_queued() {
    let runtime = spawn(231, "read l");
    let only = viewer(&runtime);
    let slot = only.slot();
    runtime.shutdown().unwrap();
    assert!(only.send_input(TerminalInput::Text("x".into())).is_err());
    assert!(only.clear_history().is_err());
    assert!(only.set_focus(true).is_err());
    assert!(
        only.report_geometry(GridSize::clamped(10, 4), cell())
            .is_err()
    );
    assert!(
        only.update_presentation(TerminalPresentation::default())
            .is_err()
    );
    assert_eq!(slot.queued(), 0);
}

#[test]
fn the_owner_rechecks_capabilities_at_dequeue() {
    let runtime = spawn(
        224,
        "stty -echo; printf READY; while IFS= read -r l; do printf '[%s]' \"$l\"; done",
    );
    let reader = runtime
        .subscribe(ViewerOptions::new(ViewerCapabilities::NONE))
        .unwrap();
    let typist = prober(&runtime);
    wait_for_text(&typist, "READY");
    // Forged requests that skip the handle's checks.
    let slot = reader.slot();
    let client = runtime.client();
    slot.queue();
    client
        .offer_input(
            Arc::clone(&slot),
            TerminalInput::Text("forged\n".into()),
            reader.stamp(None),
        )
        .unwrap();
    let (reply, scrolled) = async_channel::bounded(1);
    client
        .send_control(RuntimeControl::Snapshot {
            slot,
            scroll: Some((ScrollCommand::Relative(1), 1)),
            point: None,
            reply,
            fail_lookup: false,
        })
        .unwrap();
    assert!(matches!(
        scrolled.recv_blocking().unwrap(),
        Err(RuntimeError::NotPermitted)
    ));
    typist
        .send_input(TerminalInput::Text("allowed\n".into()))
        .unwrap();
    let text = text(&wait_for_text(&typist, "[allowed]"));
    assert!(!text.contains("forged"), "{text}");
    assert!(
        runtime
            .client()
            .job_context()
            .is_some_and(|job| !job.exited),
        "a refused request is not fatal"
    );
    runtime.shutdown().unwrap();
}

#[test]
fn a_runtime_panic_closes_every_wake() {
    let runtime = spawn(225, "printf READY; read l");
    let only = viewer(&runtime);
    wait_for_text(&only, "READY");
    runtime
        .client()
        .send_control(RuntimeControl::Panic)
        .unwrap();
    // Shutting down first would close the runtime before the panic.
    wait_until("closed wake", || {
        while only.wake_pending() {}
        only.wake.is_closed()
    });
    assert!(matches!(ready(only.wait()), Err(RuntimeError::Stopped)));
    assert!(matches!(runtime.shutdown(), Err(RuntimeError::ThreadPanic)));
}

/// Polls a future that must already be ready.
fn ready<T>(future: impl std::future::Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    match future.as_mut().poll(&mut context) {
        std::task::Poll::Ready(value) => value,
        std::task::Poll::Pending => panic!("future was not ready"),
    }
}
