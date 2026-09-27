//! Opt-in keystroke driver for `bench:output-latency -- keys`.

use std::time::{Duration, Instant};

use gpui::{AnyWindowHandle, Context, Keystroke};

use super::TerminalView;

fn enabled() -> bool {
    std::env::var("HUTERM_KEY_BENCH")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Types into `window` at keystroke pace once the view shows its first
/// snapshot. Keys enter through GPUI's window dispatch, so they take the same
/// observer and input-handler path as platform key events.
pub(super) fn start(
    window: AnyWindowHandle,
    cx: &mut Context<'_, TerminalView>,
) {
    if !enabled() {
        return;
    }
    cx.spawn(async move |view, cx| {
        let keystroke = Keystroke::parse("a").expect("static keystroke parses");
        let mut paused = false;
        for index in 0_u64.. {
            // An uneven period keeps keys from locking phase with a frame timer.
            cx.background_executor()
                .timer(Duration::from_millis(90 + index * 7 % 23))
                .await;
            let Ok(Some(active)) = window.update(cx, |_, window, cx| {
                // macOS delivers printable keys through the platform input
                // handler, which accepts text only in the active window. A
                // key sent while another application is active never echoes,
                // so it is skipped rather than counted.
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
                // Dispatch after the view update: the keystroke observer
                // updates this view through the window's root.
                if ready {
                    window.dispatch_keystroke(keystroke.clone(), cx);
                }
                Some(active)
            }) else {
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
