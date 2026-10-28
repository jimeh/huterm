//! Opt-in keystroke driver for `bench:output-latency -- keys` and
//! `bench:output-latency -- compression`.

use std::time::{Duration, Instant};

use gpui::{AnyWindowHandle, AsyncApp, Context, Keystroke, WeakEntity};
use huterm_protocol::GridSize;

use super::TerminalView;

/// How many keystrokes the compression driver times.
const COMPRESSION_CYCLES: u64 = 50;
/// The least history the compression driver waits for. Histories that grow
/// pages reach the size allowance first, so the driver waits for history to
/// stop growing rather than for the row limit.
const COMPRESSION_HISTORY_ROWS: usize = 10_000;

enum Mode {
    Keys,
    /// One key per cycle, typed while the runtime compresses history. A
    /// delay replaces waiting for a pass, for the uncompressed baseline.
    Compression {
        delay: Option<Duration>,
    },
}

fn mode() -> Option<Mode> {
    let value = std::env::var("HUTERM_KEY_BENCH").ok()?;
    if value == "1" || value.eq_ignore_ascii_case("true") {
        return Some(Mode::Keys);
    }
    (value == "compression").then(|| Mode::Compression {
        delay: std::env::var("HUTERM_KEY_BENCH_DELAY_MS")
            .ok()
            .and_then(|delay| delay.parse().ok())
            .map(Duration::from_millis),
    })
}

/// Types into `window` at keystroke pace once the view shows its first
/// snapshot. Keys enter through GPUI's window dispatch, so they take the same
/// observer and input-handler path as platform key events.
pub(super) fn start(
    window: AnyWindowHandle,
    cx: &mut Context<'_, TerminalView>,
) {
    match mode() {
        Some(Mode::Keys) => keys(window, cx),
        Some(Mode::Compression { delay }) => compression(window, delay, cx),
        None => {}
    }
}

fn keys(window: AnyWindowHandle, cx: &mut Context<'_, TerminalView>) {
    cx.spawn(async move |view, cx| {
        let keystroke = Keystroke::parse("a").expect("static keystroke parses");
        let mut paused = false;
        for index in 0_u64.. {
            // An uneven period keeps keys from locking phase with a frame timer.
            cx.background_executor()
                .timer(Duration::from_millis(90 + index * 7 % 23))
                .await;
            let Some(active) = send_key(&view, window, &keystroke, cx) else {
                return;
            };
            let was_paused = paused;
            paused = !active;
            if paused != was_paused {
                eprintln!("HUTERM_BENCH key_bench window_active={active}");
            }
        }
    })
    .detach();
}

/// Sends one key when the view can show its echo, and reports whether the
/// window was active. `None` means the window or view is gone.
fn send_key(
    view: &WeakEntity<TerminalView>,
    window: AnyWindowHandle,
    keystroke: &Keystroke,
    cx: &mut AsyncApp,
) -> Option<bool> {
    window
        .update(cx, |_, window, cx| {
            // macOS delivers printable keys through the platform input
            // handler, which accepts text only in the active window. A key
            // sent while another application is active never echoes, so it
            // is skipped rather than counted.
            let active =
                !cfg!(target_os = "macos") || window.is_window_active();
            let ready = view
                .update(cx, |view, _| {
                    let ready =
                        active && view.visible && view.snapshot.is_some();
                    if ready {
                        view.renderer
                            .borrow_mut()
                            .record_key_sent(Instant::now());
                    }
                    ready
                })
                .ok()?;
            // Dispatch after the view update: the keystroke observer updates
            // this view through the window's root.
            if ready {
                window.dispatch_keystroke(keystroke.clone(), cx);
            }
            Some(active)
        })
        .ok()
        .flatten()
}

/// Times keys typed while the runtime recompresses all history. Each cycle
/// reports a grid one column wider or narrower than the window's, alternating,
/// which restores every compressed page, waits until the next pass is running,
/// then types one key and prints its delay to the echo snapshot. A key counts
/// as tagged when the runtime handled it while a pass was unfinished.
fn compression(
    window: AnyWindowHandle,
    delay: Option<Duration>,
    cx: &mut Context<'_, TerminalView>,
) {
    cx.spawn(async move |view, cx| {
        let keystroke = Keystroke::parse("a").expect("static keystroke parses");
        let mut last = 0;
        let mut steady = 0;
        poll(cx, Duration::from_millis(100), Duration::MAX, |cx| {
            let Ok(history) = view.update(cx, |view, _| {
                view.snapshot.as_ref().map_or(0, |snapshot| snapshot.history_size)
            }) else {
                return true;
            };
            steady = if history == last { steady + 1 } else { 0 };
            last = history;
            history >= COMPRESSION_HISTORY_ROWS && steady >= 5
        })
        .await;
        for cycle in 0..COMPRESSION_CYCLES {
            let Ok((before, reported)) = view.update(cx, |view, _| {
                let before = view.viewer.compression_stats();
                let current = view.requested_grid;
                let columns = if cycle % 2 == 0 {
                    current.columns + 1
                } else {
                    current.columns - 1
                };
                let size = GridSize::clamped(columns, current.rows);
                let reported =
                    view.viewer.report_geometry(size, view.physical_cell_size());
                (before, reported)
            }) else {
                return;
            };
            if let Err(error) = reported {
                eprintln!(
                    "HUTERM_BENCH compression cycle={cycle} skipped=resize-{error:?}"
                );
                continue;
            }
            let running = match delay {
                Some(delay) => {
                    cx.background_executor().timer(delay).await;
                    true
                }
                None => {
                    poll(cx, Duration::from_millis(1), Duration::from_secs(10), |cx| {
                        view.update(cx, |view, _| {
                            view.viewer.compression_stats().steps > before.steps
                        })
                        .unwrap_or(true)
                    })
                    .await
                }
            };
            if !running {
                eprintln!("HUTERM_BENCH compression cycle={cycle} skipped=no-pass");
                continue;
            }
            match send_key(&view, window, &keystroke, cx) {
                None => return,
                Some(false) => {
                    eprintln!("HUTERM_BENCH compression cycle={cycle} skipped=inactive");
                    continue;
                }
                Some(true) => {}
            }
            let mut applied = None;
            poll(cx, Duration::from_millis(1), Duration::from_secs(5), |cx| {
                applied = view
                    .update(cx, |view, _| {
                        view.renderer.borrow_mut().take_last_key_applied()
                    })
                    .ok()
                    .flatten();
                applied.is_some()
            })
            .await;
            let Some(applied) = applied else {
                eprintln!("HUTERM_BENCH compression cycle={cycle} skipped=no-echo");
                continue;
            };
            let Ok(after) =
                view.update(cx, |view, _| view.viewer.compression_stats())
            else {
                return;
            };
            let steps = after.steps - before.steps;
            let step_time = after.step_time.saturating_sub(before.step_time);
            eprintln!(
                "HUTERM_BENCH compression cycle={cycle} tagged={} applied_us={} steps={steps} step_us_mean={}",
                after.inputs_during_pass > before.inputs_during_pass,
                applied.as_micros(),
                step_time
                    .as_micros()
                    .checked_div(u128::from(steps))
                    .unwrap_or(0),
            );
        }
        eprintln!("HUTERM_BENCH compression done");
    })
    .detach();
}

/// Checks `ready` every `period` until it holds or `timeout` passes, and
/// returns whether it held.
async fn poll(
    cx: &mut AsyncApp,
    period: Duration,
    timeout: Duration,
    mut ready: impl FnMut(&mut AsyncApp) -> bool,
) -> bool {
    let started = Instant::now();
    loop {
        if ready(cx) {
            return true;
        }
        if started.elapsed() >= timeout {
            return false;
        }
        cx.background_executor().timer(period).await;
    }
}
