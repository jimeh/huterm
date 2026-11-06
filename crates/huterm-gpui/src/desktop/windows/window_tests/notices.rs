//! Tier 2 tests for notices. Timed notices: the progress line moves and the
//! notice expires in a window that receives no input, a toast raised under
//! a resting pointer does not pause, and a pointer moved onto a toast does.
//! Small windows: the column stays inside, older toasts collapse until
//! focused or hovered, and a toast taller than the window scrolls.

use std::time::{Duration, Instant};

use gpui::{Bounds, Modifiers, Pixels, point, px};

use huterm_protocol::CommandInvocation;

use super::super::notices::{
    Lifetime, NOTICE_LIFETIME, NoticeContent, NoticeSource, Severity,
};
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

/// A ready window at the largest interface font, resized to the small
/// window #408 reported.
fn small_window(cx: &mut TestAppContext) -> (DesktopHarness, AnyWindowHandle) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.font.ui_size = huterm_config::MAX_UI_FONT_SIZE;
    });
    let window = ready_window(&mut harness);
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_resize(gpui::size(px(344.0), px(233.0)));
    harness.cx.run_until_parked();
    (harness, window)
}

/// Raises a persistent session notice titled `title`. A `detailed` one
/// also has a file location and a Retry action.
fn raise_persistent(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    title: &str,
    message: &str,
    detailed: bool,
) {
    let mut content = NoticeContent {
        severity: Severity::Error,
        source: NoticeSource::Sessions,
        title: title.to_owned(),
        message: message.to_owned(),
        location: None,
        actions: Vec::new(),
        lifetime: Lifetime::Persistent,
    };
    if detailed {
        content = content
            .location("~/.local/state/huterm/sessions.json")
            .action(
                "Retry",
                CommandInvocation::new(ids::RELOAD_CONFIG, vec![]),
            );
    }
    harness.update_view(window, |view, _, cx| view.notify(content, cx));
    harness.cx.run_until_parked();
}

/// The terminal area the toast column is drawn over.
fn area(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> Bounds<Pixels> {
    harness
        .update_view(window, |view, window, _| {
            view.terminal_geometry(window).bounds
        })
        .expect("the window shows a terminal")
}

fn inside(outer: Bounds<Pixels>, inner: Bounds<Pixels>) -> bool {
    outer.contains(&inner.origin) && outer.contains(&inner.bottom_right())
}

fn invoke(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    command: huterm_protocol::CommandId,
) {
    harness.invoke(window, command).unwrap();
    harness.cx.run_until_parked();
}

const RESTORE: &str = "Couldn't restore your previous windows";
const SAVE: &str = "Can't save sessions";

#[gpui::test]
fn a_small_window_keeps_both_toasts_and_their_dismiss_buttons_inside(
    cx: &mut TestAppContext,
) {
    let (mut harness, window) = small_window(cx);
    raise_persistent(
        &mut harness,
        window,
        RESTORE,
        "Saved state is damaged. The saved state was kept as \
         sessions-2026-11-05-with-a-long-unbroken-name.json.",
        true,
    );
    raise_persistent(
        &mut harness,
        window,
        SAVE,
        "Another Huterm process is saving sessions.",
        false,
    );
    let screen = area(&mut harness, window);
    let older = drawn(
        &mut harness,
        window,
        "notice-Couldn't restore your previous windows",
    )
    .expect("the older toast drew");
    let newer = drawn(&mut harness, window, "notice-Can't save sessions")
        .expect("the newer toast drew");
    for toast in [older, newer] {
        assert!(inside(screen, toast), "{toast:?} leaves {screen:?}");
    }
    assert!(
        older.size.height < newer.size.height,
        "the older toast collapses to one line: {older:?} vs {newer:?}"
    );
    for title in [RESTORE, SAVE] {
        let dismiss = drawn_dismiss(&mut harness, window, title);
        assert!(inside(screen, dismiss), "{title}'s Dismiss is off screen");
    }

    // The older toast's Dismiss takes a click.
    let dismiss = drawn_dismiss(&mut harness, window, RESTORE);
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_click(dismiss.center(), Modifiers::none());
    harness
        .wait_for("the older toast to go", |cx| notices(cx, window).len() == 1);
    assert_eq!(
        harness.update_view(window, |view, _, _| {
            view.notices.contents().next().unwrap().title.clone()
        }),
        SAVE
    );
}

fn drawn_dismiss(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    title: &str,
) -> Bounds<Pixels> {
    let selector: &'static str =
        Box::leak(format!("toast-Dismiss {title}").into_boxed_str());
    drawn(harness, window, selector)
        .unwrap_or_else(|| panic!("{title}'s Dismiss drew"))
}

#[gpui::test]
fn focusing_a_toast_scrolled_out_of_view_expands_it_in_view_and_the_keyboard_dismisses_it(
    cx: &mut TestAppContext,
) {
    let (mut harness, window) = small_window(cx);
    let selector = "notice-Couldn't restore your previous windows";
    raise_persistent(
        &mut harness,
        window,
        RESTORE,
        "Saved state is damaged.",
        true,
    );
    let alone = drawn(&mut harness, window, selector).unwrap();
    // A newer toast this tall leaves no room: the column opens on it, with
    // the older one collapsed and scrolled out above.
    raise_persistent(
        &mut harness,
        window,
        SAVE,
        "Another Huterm process is saving sessions, so this one won't save.",
        true,
    );
    let screen = area(&mut harness, window);
    assert!(
        !inside(screen, drawn_dismiss(&mut harness, window, RESTORE)),
        "the older toast starts out of view"
    );
    assert!(inside(screen, drawn_dismiss(&mut harness, window, SAVE)));
    invoke(&mut harness, window, ids::FOCUS_NOTICES);
    invoke(&mut harness, window, ids::NOTICE_PREVIOUS);
    let focused = drawn(&mut harness, window, selector).unwrap();
    assert_eq!(focused.size, alone.size, "focus expands it");
    assert!(
        inside(screen, focused),
        "the focused toast scrolled into view: {focused:?} in {screen:?}"
    );
    invoke(&mut harness, window, ids::NOTICE_DISMISS);
    assert_eq!(harness.cx.update(|cx| notices(cx, window)).len(), 1);
    assert!(
        harness.update_view(window, |view, window, _| {
            view.notice_focus.is_focused(window)
        }),
        "focus moves to the remaining toast"
    );
}

#[gpui::test]
fn only_a_pointer_move_onto_a_collapsed_toast_expands_it(
    cx: &mut TestAppContext,
) {
    let (mut harness, window) = small_window(cx);
    raise_persistent(
        &mut harness,
        window,
        RESTORE,
        "Saved state is damaged.",
        true,
    );
    raise_persistent(
        &mut harness,
        window,
        SAVE,
        "Another process saves.",
        false,
    );
    let selector = "notice-Couldn't restore your previous windows";
    let collapsed = drawn(&mut harness, window, selector).unwrap();
    // A pointer left resting where the toast draws, with no move onto it,
    // does not expand it: the stack is raised again under it.
    harness.update_view(window, |view, window, cx| {
        view.notices.dismiss_all();
        view.reconcile_notices(window, cx);
    });
    harness.cx.run_until_parked();
    move_pointer(&mut harness, window, collapsed.center());
    harness.cx.run_until_parked();
    raise_persistent(
        &mut harness,
        window,
        RESTORE,
        "Saved state is damaged.",
        true,
    );
    raise_persistent(
        &mut harness,
        window,
        SAVE,
        "Another process saves.",
        false,
    );
    let resting = drawn(&mut harness, window, selector).unwrap();
    assert_eq!(
        resting.size, collapsed.size,
        "a stale pointer expands nothing"
    );
    // A move onto it does, and a move off collapses it again.
    move_pointer(
        &mut harness,
        window,
        resting.center() + point(px(1.0), px(0.0)),
    );
    harness.cx.run_until_parked();
    let hovered = drawn(&mut harness, window, selector).unwrap();
    assert!(hovered.size.height > collapsed.size.height, "{hovered:?}");
    let screen = area(&mut harness, window);
    move_pointer(
        &mut harness,
        window,
        screen.origin + point(px(4.0), px(4.0)),
    );
    harness.cx.run_until_parked();
    let left = drawn(&mut harness, window, selector).unwrap();
    assert_eq!(left.size, collapsed.size);
}

#[gpui::test]
fn a_toast_taller_than_the_window_scrolls_and_opens_at_its_dismiss(
    cx: &mut TestAppContext,
) {
    let (mut harness, window) = small_window(cx);
    let message = "Saved state is damaged. ".repeat(30);
    raise_persistent(&mut harness, window, RESTORE, &message, true);
    let screen = area(&mut harness, window);
    let toast = drawn(
        &mut harness,
        window,
        "notice-Couldn't restore your previous windows",
    )
    .unwrap();
    assert!(toast.size.height > screen.size.height, "{toast:?}");
    assert!(
        inside(screen, drawn_dismiss(&mut harness, window, RESTORE)),
        "it opens at its top, where Dismiss is"
    );
}
