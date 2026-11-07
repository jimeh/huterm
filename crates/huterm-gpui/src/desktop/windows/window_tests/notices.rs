//! Tier 2 tests for notices. Timed notices: the progress line moves and the
//! notice expires in a window that receives no input, a toast raised under
//! a resting pointer does not pause, and a pointer moved onto a toast does.
//! Small windows: the column stays inside, older toasts collapse until
//! focused or hovered, and a toast taller than the window scrolls.
//! Accessibility: each toast is an `Alert` that reads its whole notice,
//! even collapsed, and announces once through a live leaf in the first
//! frame that shows it in front.

use std::time::{Duration, Instant};

use gpui::{
    Bounds, Modifiers, Pixels, ScrollDelta, ScrollEvent, TouchPhase, point, px,
};

use huterm_protocol::CommandInvocation;

use super::super::notices::{
    Lifetime, NOTICE_LIFETIME, NoticeContent, NoticeSource, Severity,
};
use super::super::refresh::Animated;
use super::*;

/// A window showing one ready terminal.
fn ready_window(harness: &mut DesktopHarness) -> AnyWindowHandle {
    ready_window_running(harness, ECHO)
}

/// A window showing one terminal running `script`, once it printed READY.
fn ready_window_running(
    harness: &mut DesktopHarness,
    script: &str,
) -> AnyWindowHandle {
    let tab = harness.open_tab(script);
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
    small_window_running(cx, ECHO)
}

/// [`small_window`] with its terminal running `script`.
fn small_window_running(
    cx: &mut TestAppContext,
    script: &str,
) -> (DesktopHarness, AnyWindowHandle) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.font.ui_size = huterm_config::MAX_UI_FONT_SIZE;
    });
    let window = ready_window_running(&mut harness, script);
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
    // A notice that arrives meanwhile leaves the focused one in view.
    raise_persistent(&mut harness, window, "Later", "Another failure.", false);
    let focused = drawn(&mut harness, window, selector).unwrap();
    assert!(
        inside(screen, focused),
        "the focused toast stays in view: {focused:?} in {screen:?}"
    );
    invoke(&mut harness, window, ids::NOTICE_DISMISS);
    assert_eq!(harness.cx.update(|cx| notices(cx, window)).len(), 2);
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

fn focused_title(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> Option<String> {
    harness.update_view(window, |view, window, _| {
        let id = view.focused_notice?;
        view.notice_focus
            .is_focused(window)
            .then(|| view.notices.get(id).unwrap().content.title.clone())
    })
}

#[gpui::test]
fn the_arrow_keys_read_through_a_tall_focused_toast_before_moving_on(
    cx: &mut TestAppContext,
) {
    let (mut harness, window) = small_window(cx);
    raise_persistent(
        &mut harness,
        window,
        RESTORE,
        "Saved state is damaged.",
        false,
    );
    let message = "Another Huterm process is saving sessions. ".repeat(6);
    raise_persistent(&mut harness, window, SAVE, &message, true);
    let screen = area(&mut harness, window);
    let retry = |harness: &mut DesktopHarness| {
        drawn(harness, window, "toast-Can't save sessions-Retry").unwrap()
    };
    invoke(&mut harness, window, ids::FOCUS_NOTICES);
    assert!(!inside(screen, retry(&mut harness)), "it opens at its top");
    let mut presses = 0;
    while !inside(screen, retry(&mut harness)) {
        presses += 1;
        assert!(presses < 20, "Down never reached the toast's actions");
        invoke(&mut harness, window, ids::NOTICE_NEXT);
        assert_eq!(focused_title(&mut harness, window).as_deref(), Some(SAVE));
    }
    assert!(presses > 1, "each press scrolls one step");
    // Up reads back to its top, where Dismiss is, then moves on.
    while !inside(screen, drawn_dismiss(&mut harness, window, SAVE)) {
        invoke(&mut harness, window, ids::NOTICE_PREVIOUS);
        assert_eq!(focused_title(&mut harness, window).as_deref(), Some(SAVE));
    }
    // Moving to an action scrolls it into view.
    invoke(&mut harness, window, ids::NOTICE_FOCUS_NEXT);
    assert!(
        inside(screen, retry(&mut harness)),
        "the current action shows"
    );
    invoke(&mut harness, window, ids::NOTICE_FOCUS_PREVIOUS);
    assert!(
        inside(screen, drawn_dismiss(&mut harness, window, SAVE)),
        "the current Dismiss shows"
    );
    while focused_title(&mut harness, window).as_deref() == Some(SAVE) {
        invoke(&mut harness, window, ids::NOTICE_PREVIOUS);
    }
    assert_eq!(
        focused_title(&mut harness, window).as_deref(),
        Some(RESTORE)
    );
}

#[gpui::test]
fn enter_presses_the_button_the_keyboard_moved_to(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let window = ready_window(&mut harness);
    let two_actions = |title: &str| {
        NoticeContent::diagnostic(
            Severity::Error,
            NoticeSource::Config,
            title,
            "message",
        )
        .action("Reload", CommandInvocation::new(ids::RELOAD_CONFIG, vec![]))
        .action(
            "Clear All",
            CommandInvocation::new(ids::DISMISS_ALL_NOTICES, vec![]),
        )
    };
    raise_persistent(
        &mut harness,
        window,
        RESTORE,
        "Saved state is damaged.",
        false,
    );
    let first = two_actions("First");
    harness.update_view(window, |view, _, cx| view.notify(first, cx));
    harness.cx.run_until_parked();

    // Backwards from no button wraps to Dismiss, which Enter presses.
    invoke(&mut harness, window, ids::FOCUS_NOTICES);
    invoke(&mut harness, window, ids::NOTICE_FOCUS_PREVIOUS);
    invoke(&mut harness, window, ids::NOTICE_RUN_ACTION);
    assert_eq!(harness.cx.update(|cx| notices(cx, window)).len(), 1);
    assert_eq!(
        focused_title(&mut harness, window).as_deref(),
        Some(RESTORE),
        "focus moves to the remaining toast"
    );

    // Forward past the first action, Enter runs the second.
    let second = two_actions("Second");
    harness.update_view(window, |view, _, cx| view.notify(second, cx));
    harness.cx.run_until_parked();
    invoke(&mut harness, window, ids::FOCUS_NOTICES);
    invoke(&mut harness, window, ids::NOTICE_FOCUS_NEXT);
    invoke(&mut harness, window, ids::NOTICE_FOCUS_NEXT);
    invoke(&mut harness, window, ids::NOTICE_RUN_ACTION);
    harness.wait_for("Clear All to run", |cx| notices(cx, window).is_empty());
}

#[gpui::test]
fn the_wheel_scrolls_an_overflowing_stack_and_not_the_terminal_beneath(
    cx: &mut TestAppContext,
) {
    // The terminal holds scrollback for the wheel to scroll.
    let (mut harness, window) =
        small_window_running(cx, &format!("seq 1 200; {ECHO}"));
    let tab = harness.update_view(window, |view, _, _| view.tabs[0].id);
    let terminal_scroll = |harness: &mut DesktopHarness| {
        harness.cx.update(|cx| {
            terminal(cx, window, tab).unwrap().read(cx).scroll.desired()
        })
    };
    let wheel = |harness: &mut DesktopHarness,
                 at: gpui::Point<Pixels>,
                 lines: f32| {
        let mut visual = VisualTestContext::from_window(window, &harness.cx);
        visual.simulate_mouse_move(at, None, Modifiers::none());
        visual.simulate_event(ScrollEvent {
            position: at,
            delta: ScrollDelta::Lines(point(0.0, lines)),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
            ..Default::default()
        });
        harness.cx.run_until_parked();
    };
    let stack_offset = |harness: &mut DesktopHarness| {
        harness.update_view(window, |view, _, _| view.notice_scroll.offset().y)
    };

    // One short toast fits: the wheel over it scrolls the terminal, as
    // before the stack could scroll.
    raise_persistent(
        &mut harness,
        window,
        SAVE,
        "Another process saves.",
        false,
    );
    let toast =
        drawn(&mut harness, window, "notice-Can't save sessions").unwrap();
    wheel(&mut harness, toast.center(), 3.0);
    let after_fit = terminal_scroll(&mut harness);
    assert!(after_fit > 0, "a stack that fits passes the wheel through");

    // A toast taller than the window overflows: the stack takes the wheel.
    let message = "Saved state is damaged. ".repeat(30);
    raise_persistent(&mut harness, window, RESTORE, &message, true);
    let before = stack_offset(&mut harness);
    let toast = drawn(
        &mut harness,
        window,
        "notice-Couldn't restore your previous windows",
    )
    .unwrap();
    let at = point(toast.center().x, area(&mut harness, window).center().y);
    // Down: the tall toast opens at its top, with more below.
    wheel(&mut harness, at, -3.0);
    assert_ne!(stack_offset(&mut harness), before, "the stack scrolled");
    assert_eq!(
        terminal_scroll(&mut harness),
        after_fit,
        "the terminal beneath did not scroll"
    );
}

/// A toast's accessibility node from the window's last frame, and its
/// children.
struct ToastNode {
    node: serde_json::Value,
    children: Vec<serde_json::Value>,
}

impl ToastNode {
    fn label(&self) -> &str {
        self.node["aria"]["label"].as_str().unwrap_or_default()
    }

    /// The live leaf that announces the toast in this frame, if any.
    fn announcement(&self) -> Option<&serde_json::Value> {
        self.children
            .iter()
            .find(|child| child["synthetic"] == true)
    }

    fn button(&self, name: &str) -> &serde_json::Value {
        self.children
            .iter()
            .find(|child| {
                child["aria"]["role"] == "Button"
                    && child["aria"]["label"] == name
            })
            .unwrap_or_else(|| {
                panic!("no {name} button in {:#?}", self.children)
            })
    }
}

/// The toasts' accessibility nodes, by label.
fn toast_nodes(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> Vec<ToastNode> {
    let json = harness
        .cx
        .update(|cx| {
            window.update(cx, |_, window, _| window.debug_a11y_tree_json())
        })
        .unwrap()
        .expect("test windows report an accessibility tree for every frame");
    let tree: serde_json::Value = serde_json::from_str(&json).unwrap();
    let nodes = tree["nodes"].as_object().unwrap();
    // Buttons sit inside the toast's body: gather every descendant.
    let descendants = |node: &serde_json::Value| {
        let mut found = Vec::new();
        let mut pending: Vec<&serde_json::Value> = vec![node];
        while let Some(next) = pending.pop() {
            for key in next["children"].as_array().into_iter().flatten() {
                let child = &nodes[key.as_str().unwrap()];
                found.push(child.clone());
                pending.push(child);
            }
        }
        found
    };
    nodes
        .values()
        .filter(|node| node["aria"]["role"] == "Alert")
        .map(|node| ToastNode {
            node: node.clone(),
            children: descendants(node),
        })
        .collect()
}

fn toast<'a>(toasts: &'a [ToastNode], label: &str) -> &'a ToastNode {
    toasts
        .iter()
        .find(|toast| toast.label() == label)
        .unwrap_or_else(|| {
            let labels: Vec<&str> =
                toasts.iter().map(ToastNode::label).collect();
            panic!("no toast reads {label:?}; toasts read {labels:?}")
        })
}

/// Draws one more frame of `window`.
fn redraw(harness: &mut DesktopHarness, window: AnyWindowHandle) {
    harness.update_view(window, |_, window, _| window.refresh());
    harness.cx.run_until_parked();
}

/// Runs `raise` while `window` is behind, then brings it forward, so the
/// frame that announces is the last one drawn. A notice raised in front
/// announces in its first frame, but scrolling to it draws another before
/// a test can read the tree.
fn raise_behind(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    raise: impl FnOnce(&mut DesktopHarness),
) {
    harness.deactivate(window);
    raise(harness);
    harness.cx.run_until_parked();
    harness
        .cx
        .update(|cx| window.update(cx, |_, window, _| window.activate_window()))
        .unwrap();
    harness.cx.run_until_parked();
}

const DAMAGED: &str = "Saved state is damaged.";

#[gpui::test]
fn each_toast_reads_its_whole_notice_and_announces_it_once(
    cx: &mut TestAppContext,
) {
    let (mut harness, window) = small_window(cx);
    let restore = format!(
        "Error: {RESTORE}. {DAMAGED} ~/.local/state/huterm/sessions.json"
    );
    raise_behind(&mut harness, window, |harness| {
        raise_persistent(harness, window, RESTORE, DAMAGED, true);
    });
    let toasts = toast_nodes(&mut harness, window);
    let first = toast(&toasts, &restore);
    let announcement = first.announcement().expect("a new toast announces");
    assert_eq!(announcement["aria"]["role"], "Label");
    assert_eq!(announcement["aria"]["value"], restore.as_str());
    assert_eq!(
        announcement["aria"]["live"], "Polite",
        "a restore problem waits for speech"
    );
    for name in ["Retry", &format!("Dismiss {RESTORE}")] {
        let button = first.button(name);
        assert!(button["aria"]["live"].is_null(), "{name} stays quiet");
    }
    assert!(
        first.node["aria"]["live"].is_null(),
        "only the leaf is live"
    );

    redraw(&mut harness, window);
    let toasts = toast_nodes(&mut harness, window);
    assert!(
        toast(&toasts, &restore).announcement().is_none(),
        "the leaf lasts one frame"
    );

    // A newer failure interrupts, and the older toast, collapsed in this
    // small window, still reads its message and stays quiet.
    raise_behind(&mut harness, window, |harness| {
        harness.update_view(window, |view, _, cx| {
            view.notify(
                NoticeContent::command_failure(
                    "Copy failed",
                    "Nothing is selected",
                ),
                cx,
            );
        });
    });
    let toasts = toast_nodes(&mut harness, window);
    let failure = toast(&toasts, "Error: Copy failed. Nothing is selected");
    assert_eq!(
        failure.announcement().expect("it announces")["aria"]["live"],
        "Assertive"
    );
    assert!(toast(&toasts, &restore).announcement().is_none());
    let older = drawn(
        &mut harness,
        window,
        "notice-Couldn't restore your previous windows",
    )
    .expect("the older toast drew");
    let newer = drawn(&mut harness, window, "notice-Copy failed")
        .expect("the newer toast drew");
    assert!(
        older.size.height < newer.size.height,
        "the older toast collapsed: {older:?} vs {newer:?}"
    );
}

#[gpui::test]
fn a_toast_raised_behind_announces_when_its_window_comes_forward(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    harness.deactivate(window);
    raise_persistent(
        &mut harness,
        window,
        SAVE,
        "Another process saves.",
        false,
    );
    let label = format!("Error: {SAVE}. Another process saves.");
    let toasts = toast_nodes(&mut harness, window);
    assert!(
        toast(&toasts, &label).announcement().is_none(),
        "a window behind announces nothing"
    );
    harness.activate(window, tab);
    harness.cx.run_until_parked();
    let toasts = toast_nodes(&mut harness, window);
    assert!(
        toast(&toasts, &label).announcement().is_some(),
        "activation redraws and announces"
    );
    redraw(&mut harness, window);
    harness.deactivate(window);
    harness.activate(window, tab);
    harness.cx.run_until_parked();
    let toasts = toast_nodes(&mut harness, window);
    assert!(
        toast(&toasts, &label).announcement().is_none(),
        "announced once"
    );
}
