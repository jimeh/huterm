//! Tier 2 tests for timed notices: the progress line moves and the notice
//! expires in a window that receives no input, a toast raised under a
//! resting pointer does not pause, and a pointer moved onto a toast does.

use std::time::{Duration, Instant};

use gpui::{Bounds, Modifiers, Pixels, point, px};

use super::super::notices::{NOTICE_LIFETIME, NoticeContent};
use super::super::refresh::Animated;
use super::*;

/// A window showing one ready terminal.
fn ready_window(harness: &mut DesktopHarness) -> AnyWindowHandle {
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    window
}

/// Raises an expiring notice titled `title` outside any window event, as a
/// worker completion or terminal failure does, with `remaining` of its
/// lifetime left.
fn raise_timed(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    title: &str,
    remaining: Duration,
) {
    let content = NoticeContent::command_failure(title, "message");
    harness.update_view(window, |view, _, cx| {
        let created = (Instant::now() + remaining)
            .checked_sub(NOTICE_LIFETIME)
            .expect("the clock is past one notice lifetime");
        view.notices.push(content, created);
        cx.notify();
    });
}

fn drawn(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    selector: &'static str,
) -> Option<Bounds<Pixels>> {
    VisualTestContext::from_window(window, &harness.cx).debug_bounds(selector)
}

fn move_pointer(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    to: gpui::Point<Pixels>,
) {
    VisualTestContext::from_window(window, &harness.cx).simulate_mouse_move(
        to,
        None,
        Modifiers::none(),
    );
}

fn schedule(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> AnimationSchedule {
    harness.update_view(window, |view, _, _| {
        view.animation_schedule(Instant::now())
    })
}

#[gpui::test]
fn a_timed_toast_animates_and_expires_without_input(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let window = ready_window(&mut harness);
    assert!(!schedule(&mut harness, window).frame, "an idle window");
    // Old enough to expire within the wait, young enough to be seen
    // moving first.
    raise_timed(&mut harness, window, "Stopped", Duration::from_millis(600));
    harness.cx.run_until_parked();
    let first = drawn(&mut harness, window, "toast-progress-Stopped")
        .expect("the progress line drew")
        .size
        .width;
    assert!(schedule(&mut harness, window).frame, "the line animates");
    // Frames alone, with no input, move the line.
    let later = Instant::now() + Duration::from_millis(150);
    harness.wait_for("frames to run", |_| Instant::now() >= later);
    let now = drawn(&mut harness, window, "toast-progress-Stopped")
        .expect("the progress line still draws")
        .size
        .width;
    assert!(now < first, "the line stayed at {first:?}, then {now:?}");
    // Without frames, the expiry deadline still removes it.
    harness.wait_without_frames("the notice to expire", |cx| {
        notices(cx, window).is_empty()
    });
    assert!(
        !schedule(&mut harness, window).frame,
        "frames stop with the notice"
    );
}

#[gpui::test]
fn a_toast_raised_under_a_resting_pointer_still_expires(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let window = ready_window(&mut harness);
    // Find where a toast draws, then leave the pointer resting there with
    // no toast under it.
    raise_timed(&mut harness, window, "Probe", NOTICE_LIFETIME);
    harness.cx.run_until_parked();
    let spot = drawn(&mut harness, window, "toast-Dismiss Probe")
        .expect("the toast drew")
        .center();
    harness.update_view(window, |view, window, cx| {
        view.notices.dismiss_all();
        view.reconcile_notices(window, cx);
    });
    harness.cx.run_until_parked();
    move_pointer(&mut harness, window, spot);
    harness.cx.run_until_parked();

    // A toast raised under it is drawn hovered, but nothing moved onto it.
    raise_timed(&mut harness, window, "Stopped", Duration::from_millis(300));
    harness.cx.run_until_parked();
    assert!(
        drawn(&mut harness, window, "toast-Dismiss Stopped")
            .is_some_and(|bounds| bounds.contains(&spot)),
        "the new toast is under the pointer"
    );
    harness.wait_without_frames("the notice to expire", |cx| {
        notices(cx, window).is_empty()
    });
}

#[gpui::test]
fn moving_the_pointer_onto_a_toast_pauses_it_until_it_leaves(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let window = ready_window(&mut harness);
    raise_timed(&mut harness, window, "Stopped", Duration::from_millis(300));
    harness.cx.run_until_parked();
    let toast = drawn(&mut harness, window, "toast-Dismiss Stopped")
        .expect("the toast drew");
    move_pointer(&mut harness, window, toast.center());
    harness.cx.run_until_parked();
    assert_eq!(
        harness.update_view(window, |view, _, _| view.notices.schedule()),
        AnimationSchedule::IDLE,
        "a hovered toast neither animates nor expires"
    );
    // Long past its deadline, it is still there.
    let held = Instant::now() + Duration::from_millis(500);
    harness
        .wait_for("the original deadline to pass", |_| Instant::now() >= held);
    assert_eq!(harness.cx.update(|cx| notices(cx, window)).len(), 1);

    move_pointer(&mut harness, window, point(px(20.0), px(60.0)));
    harness.wait_without_frames("the notice to expire after leaving", |cx| {
        notices(cx, window).is_empty()
    });
}
