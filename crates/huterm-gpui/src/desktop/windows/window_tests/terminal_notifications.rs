//! Tier 2 tests for programs' progress and notifications through a real
//! terminal: progress reaches the tab and pane and clears, and a
//! notification appears once, as a notice in front or a system
//! notification behind, marking the tab only when it was not seen.

use huterm_protocol::TerminalProgress;

use huterm_protocol::SplitDirection;

use super::super::notices::Severity;
use super::super::terminal_notifications::SHOW_PANE;
use super::settings::{ScratchConfig, open_settings};
use super::*;
use crate::native_notifications::Availability;

const PROGRAM: &str = "printf READY; while read line; do case \"$line\" in \
     prog) printf '\\033]9;4;1;42\\007PROG';; \
     spin) printf '\\033]9;4;3\\007SPIN';; \
     clear) printf '\\033]9;4;0\\007CLEAR';; \
     note) printf '\\033]9;Build done\\007NOTED';; \
     esac; done";

fn ready(harness: &mut DesktopHarness) -> (AnyWindowHandle, TabId) {
    let tab = harness.open_tab(PROGRAM);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    (window, tab)
}

fn status(cx: &mut App, window: AnyWindowHandle, tab: TabId) -> TabStatus {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().unwrap();
            let view = view.read(cx);
            let shown = view.tabs.iter().find(|shown| shown.id == tab).unwrap();
            shown.label(view.config.tabs, cx).1
        })
        .unwrap()
}

fn counts(cx: &mut App) -> (u64, u64) {
    let notifier = &cx.global::<Desktop>().notifier;
    (notifier.notices, notifier.posted)
}

#[gpui::test]
fn progress_reaches_the_tab_and_pane_and_clears(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab) = ready(&mut harness);
    let _prog = harness.send_line(tab, "prog");
    harness.wait_for("determinate progress", |cx| {
        status(cx, window, tab)
            == TabStatus::Progress(TerminalProgress::Determinate(42), 0)
    });
    let _spin = harness.send_line(tab, "spin");
    harness.wait_for("indeterminate progress", |cx| {
        matches!(
            status(cx, window, tab),
            TabStatus::Progress(TerminalProgress::Indeterminate, _)
        )
    });
    assert!(
        harness.update_view(window, |view, _, _| view.progress_motion),
        "the window steps indeterminate progress"
    );
    let _clear = harness.send_line(tab, "clear");
    harness.wait_for("cleared progress", |cx| {
        terminal(cx, window, tab)
            .unwrap()
            .read(cx)
            .progress
            .is_none()
    });
    assert_eq!(
        harness.cx.update(|cx| status(cx, window, tab)),
        TabStatus::Idle
    );
}

#[gpui::test]
fn a_notification_in_front_is_a_notice_and_marks_nothing(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab) = ready(&mut harness);
    let _note = harness.send_line(tab, "note");
    harness.wait_for("the notice", |cx| counts(cx) == (1, 0));
    harness.update_view(window, |view, _, cx| {
        let notice = view.notices.contents().next().expect("one notice");
        assert_eq!(notice.severity, Severity::Info);
        assert_eq!(notice.message, "Build done");
        assert_eq!(notice.actions, [], "the terminal is in front");
        assert!(!view.tabs[0].focused_view().read(cx).bell.marked());
    });
}

#[gpui::test]
fn a_notification_behind_posts_once_and_marks_the_tab(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().notifier.availability =
            Some(Availability::Available);
    });
    let (window, tab) = ready(&mut harness);
    harness.deactivate(window);
    let _note = harness.send_line(tab, "note");
    harness.wait_for("the system notification", |cx| counts(cx) == (0, 1));
    harness.wait_for("the tab mark", |cx| {
        terminal(cx, window, tab).unwrap().read(cx).bell.marked()
    });
    assert_eq!(
        harness.cx.update(|cx| status(cx, window, tab)),
        TabStatus::Bell
    );
    // Viewing the tab clears the mark.
    harness.activate(window, tab);
    harness.wait_for("the mark to clear", |cx| {
        !terminal(cx, window, tab).unwrap().read(cx).bell.marked()
    });
}

#[gpui::test]
fn an_unavailable_system_notification_falls_back_to_a_notice(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().notifier.availability =
            Some(Availability::Unavailable);
    });
    let (window, tab) = ready(&mut harness);
    harness.deactivate(window);
    let _note = harness.send_line(tab, "note");
    harness.wait_for("the fallback notice", |cx| counts(cx) == (1, 0));
    harness.update_view(window, |view, _, _| {
        let notice = view.notices.contents().next().expect("one notice");
        assert_eq!(notice.message, "Build done");
        // The window is in the background, but its tab is the one shown.
        assert!(
            notice
                .actions
                .iter()
                .all(|action| action.command.id != SHOW_PANE)
        );
    });
}

#[gpui::test]
fn denied_notifications_reach_no_window(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.terminal.notifications = huterm_config::NotificationPolicy::Deny;
    });
    let (window, tab) = ready(&mut harness);
    let _note = harness.send_line(tab, "note");
    harness.wait_text(window, tab, "NOTED");
    let _prog = harness.send_line(tab, "prog");
    harness.wait_text(window, tab, "PROG");
    assert_eq!(harness.cx.update(counts), (0, 0));
}

#[gpui::test]
fn a_notification_from_another_tab_offers_show_tab_which_reveals_it(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(PROGRAM);
    let second = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, first, "READY");
    harness.update_view(window, |view, window, cx| {
        view.select_tab(second, true, window, cx);
    });
    harness.activate(window, second);
    let _note = harness.send_line(first, "note");
    harness.wait_for("the notice", |cx| counts(cx) == (1, 0));
    harness.wait_for("the tab mark", |cx| {
        terminal(cx, window, first).unwrap().read(cx).bell.marked()
    });
    harness.update_view(window, |view, window, cx| {
        let id = view.notices.newest().unwrap();
        let notice = &view.notices.get(id).unwrap().content;
        assert_eq!(notice.actions[0].label, "Show Tab");
        assert_eq!(notice.actions[0].command.id, SHOW_PANE);
        view.run_notice_action(id, None, window, cx).unwrap();
    });
    harness.wait_for("the revealed tab", |cx| focused(cx, window, first));
    harness.wait_for("the mark to clear", |cx| {
        !terminal(cx, window, first).unwrap().read(cx).bell.marked()
    });
}

const BELLS: &str =
    "printf READY; while read line; do printf '\\007RANG'; done";

fn attention(harness: &mut DesktopHarness, window: AnyWindowHandle) -> u64 {
    harness.update_view(window, |view, _, _| view.attention_requests)
}

#[gpui::test]
fn a_bell_asks_for_attention_only_from_an_inactive_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(BELLS);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    let _active = harness.send_line(tab, "a");
    harness.wait_text(window, tab, "RANG");
    assert_eq!(attention(&mut harness, window), 0, "the window is active");
    harness.deactivate(window);
    let _inactive = harness.send_line(tab, "b");
    harness.wait_for("the attention request", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>()
                    .unwrap()
                    .read(cx)
                    .attention_requests
                    == 1
            })
            .unwrap()
    });
}

#[gpui::test]
fn bell_attention_follows_its_setting(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.terminal.bell.attention = false;
    });
    let tab = harness.open_tab(BELLS);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    harness.deactivate(window);
    let _bell = harness.send_line(tab, "a");
    // The view takes status before the snapshot that shows the same output.
    harness.wait_text(window, tab, "RANG");
    assert_eq!(attention(&mut harness, window), 0);
}

/// A tab whose first pane runs [`PROGRAM`] beside a second pane, zoomed,
/// which hides the first.
fn zoomed_away(
    harness: &mut DesktopHarness,
) -> (AnyWindowHandle, TabId, PaneId, PaneId) {
    let tab = harness.open_tab(PROGRAM);
    let first = harness.first_pane(tab);
    let second = harness.split(tab, first, SplitDirection::Right, ECHO);
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    harness.wait_for("the second pane", |cx| {
        pane_view(cx, window, tab, second).is_some()
    });
    harness.update_view(window, |view, window, cx| {
        view.set_zoom(tab, Some(second), window, cx);
    });
    harness.wait_for("the hidden first pane", |cx| {
        pane_view(cx, window, tab, first)
            .is_some_and(|view| !view.read(cx).visible)
    });
    (window, tab, first, second)
}

#[gpui::test]
fn show_tab_clears_a_zoom_that_hides_the_pane(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, first, _) = zoomed_away(&mut harness);
    let _note = harness.send_line(tab, "note");
    harness.wait_for("the notice", |cx| counts(cx) == (1, 0));
    harness.update_view(window, |view, window, cx| {
        let id = view.notices.newest().unwrap();
        let notice = &view.notices.get(id).unwrap().content;
        assert_eq!(notice.actions[0].command.id, SHOW_PANE);
        view.run_notice_action(id, None, window, cx).unwrap();
    });
    harness.wait_for("the revealed pane", |cx| {
        pane_view(cx, window, tab, first).is_some_and(|view| {
            window
                .update(cx, |_, window, cx| {
                    view.read(cx).visible
                        && view.read(cx).focus.is_focused(window)
                })
                .unwrap_or(false)
        })
    });
}

#[gpui::test]
fn a_pane_hidden_by_zoom_does_not_step_progress(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, first, _) = zoomed_away(&mut harness);
    let _spin = harness.send_line(tab, "spin");
    harness.wait_for("the hidden pane's progress", |cx| {
        pane_view(cx, window, tab, first).is_some_and(|view| {
            view.read(cx).progress == Some(TerminalProgress::Indeterminate)
        })
    });
    harness.update_view(window, |_, window, _| window.refresh());
    harness.wait_for("a frame", |_| true);
    assert!(!harness.update_view(window, |view, _, _| view.progress_motion));
    harness.update_view(window, |view, window, cx| {
        view.set_zoom(tab, None, window, cx);
    });
    harness.wait_for("motion once the pane shows", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>()
                    .unwrap()
                    .read(cx)
                    .progress_motion
            })
            .unwrap()
    });
}

#[gpui::test]
fn a_notification_while_settings_is_in_front_is_a_notice(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("notify-settings", "");
    let mut harness = DesktopHarness::new(cx);
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().notifier.availability =
            Some(Availability::Available);
    });
    let (window, tab) = ready(&mut harness);
    open_settings(&mut harness, &file.path());
    harness.cx.update(|cx| {
        super::settings::settings(cx)
            .unwrap()
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
    });
    harness.wait_for("Settings in front", |cx| {
        !window
            .update(cx, |_, window, _| window.is_window_active())
            .unwrap()
    });
    let _note = harness.send_line(tab, "note");
    harness.wait_for("the notice", |cx| counts(cx) == (1, 0));
    harness.update_view(window, |view, _, _| {
        let notice = view.notices.contents().next().expect("one notice");
        assert_eq!(notice.message, "Build done");
    });
}

#[gpui::test]
fn a_bell_mark_over_the_ring_stops_its_motion(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(PROGRAM);
    let second = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, first, "READY");
    harness.update_view(window, |view, window, cx| {
        view.select_tab(second, true, window, cx);
    });
    harness.activate(window, second);
    let _spin = harness.send_line(first, "spin");
    let motion = |cx: &mut App| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>()
                    .unwrap()
                    .read(cx)
                    .progress_motion
            })
            .unwrap()
    };
    harness.wait_for("the inactive tab's ring to turn", motion);
    // The notification marks the inactive tab, and the mark replaces its
    // ring.
    let _note = harness.send_line(first, "note");
    harness.wait_for("the mark to stop the ring", |cx| {
        status(cx, window, first) == TabStatus::Bell && !motion(cx)
    });
}
