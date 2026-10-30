//! Tier 2 tests for `[panes] drag_modifier`: the held modifiers arm a pane
//! under the pointer from the root's modifier listener or a pointer event,
//! a press with them drags the pane without reaching its terminal, macOS's
//! remapped Control-left press drops on its Right release, presses the
//! grab does not claim reach the terminal, overlays drawn later keep their
//! presses, and the badges name each pane.

use std::time::{Duration, Instant};

use gpui::Modifiers;
use huterm_config::{PaneDragModifiers, PaneTitleBarMode};

use super::super::notices::{NoticeContent, NoticeSource, Severity};
use super::super::pane_drag::DropTarget;
use super::super::pane_geometry::Zone;
use super::super::refresh::Animated;
use super::*;

/// A child that turns on SGR mouse reporting, prints `READY`, and then
/// prints every byte it receives as two hex digits, so a mouse report
/// shows before a later key.
const REPORTING: &str = "stty raw -echo; printf '\\033[?1000h\\033[?1006hREADY'; \
     while byte=$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\n'); \
     [ -n \"$byte\" ]; do printf '%s' \"$byte\"; done";

// Only the macOS remap tests hold Cmd without Control.
#[cfg(target_os = "macos")]
fn cmd() -> Modifiers {
    Modifiers {
        platform: true,
        ..Modifiers::default()
    }
}

fn cmd_ctrl() -> Modifiers {
    Modifiers {
        platform: true,
        control: true,
        ..Modifiers::default()
    }
}

/// A desktop whose windows drag panes with Cmd-Control, which both
/// platforms accept beside their default link chord.
fn grab_harness(cx: &mut TestAppContext) -> DesktopHarness {
    DesktopHarness::with_config(cx, |config| {
        config.panes.drag_modifier = Some(
            PaneDragModifiers::parse("cmd-ctrl", cfg!(target_os = "macos"))
                .unwrap(),
        );
    })
}

/// Whether `window` shows the open hand's armed state, and its badges.
fn grab_state(cx: &mut App, window: AnyWindowHandle) -> (bool, bool) {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>()
                .map_or((false, false), |view| {
                    let grab = &view.read(cx).pane_grab;
                    (grab.armed(), grab.badges())
                })
        })
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn dragging(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>().is_ok_and(|view| {
                view.read(cx).pane_drag.as_ref().is_some_and(|d| d.dragging)
            })
        })
        .unwrap_or(false)
}

fn pressed(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>()
                .is_ok_and(|view| view.read(cx).pane_drag.is_some())
        })
        .unwrap_or(false)
}

/// The bytes a [`REPORTING`] pane has reported receiving.
fn reported(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
    pane: PaneId,
) -> String {
    pane_screen(cx, window, tab, pane)
        .and_then(|text| {
            let (_, echoed) = text.split_once("READY")?;
            Some(echoed.chars().filter(char::is_ascii_hexdigit).collect())
        })
        .unwrap_or_default()
}

impl DesktopHarness {
    /// A window showing `tab` split into a `first` pane running `script`
    /// and an ECHO pane on its right, which samples the pointer from real
    /// events.
    fn grab_window(
        &mut self,
        script: &str,
    ) -> (AnyWindowHandle, TabId, PaneId, PaneId) {
        let tab = self.open_tab(script);
        let (window, first, second) = self.split_window(tab);
        self.update_view(window, |view, _, _| {
            view.pointer_reveal.hovered_in_tests = true;
        });
        (window, tab, first, second)
    }

    /// The drawn terminal rectangle of `pane` in `window`'s active tab.
    fn terminal_rect(
        &mut self,
        window: AnyWindowHandle,
        pane: PaneId,
    ) -> Bounds<Pixels> {
        self.update_view(window, |view, _, _| {
            view.drawn_panes.borrow().as_ref().and_then(|(_, rects)| {
                rects
                    .iter()
                    .find(|rect| rect.pane == pane)
                    .map(|rect| rect.terminal)
            })
        })
        .expect("a drawn pane")
    }

    fn visual(&self, window: AnyWindowHandle) -> VisualTestContext {
        VisualTestContext::from_window(window, &self.cx)
    }

    /// Types `z` into the focused pane and waits for a [`REPORTING`]
    /// `pane` to report it, so every byte a press before it sent has
    /// arrived too. Returns what the pane reported.
    fn probe_pane(
        &mut self,
        window: AnyWindowHandle,
        tab: TabId,
        pane: PaneId,
    ) -> String {
        let before = self.cx.update(|cx| reported(cx, window, tab, pane));
        self.press(window, "z");
        self.wait_for("the probe key to reach the pane", |cx| {
            let now = reported(cx, window, tab, pane);
            now.len() >= before.len() + 2 && now.ends_with("7a")
        });
        self.cx.update(|cx| reported(cx, window, tab, pane))
    }

    /// Runs `window`'s animation step at `now`, as its frame clock would.
    fn animate_grab(&mut self, window: AnyWindowHandle, now: Instant) {
        self.update_view(window, |view, window, cx| {
            view.advance_animation(now, false, window, cx);
        });
    }
}

#[gpui::test]
fn a_modifier_change_alone_arms_the_pane_under_the_pointer(
    cx: &mut TestAppContext,
) {
    let mut harness = grab_harness(cx);
    let (window, _, first, _) = harness.grab_window(ECHO);
    let centre = harness.terminal_rect(window, first).center();
    harness
        .visual(window)
        .simulate_mouse_move(centre, None, Modifiers::none());
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        assert_eq!(grab_state(cx, window), (false, false));
    });
    // GPUI sends modifier changes only along the focus path, which the
    // root div is on; no pointer event follows here.
    harness.visual(window).simulate_modifiers_change(cmd_ctrl());
    harness.wait_for("the combination to arm the pane", |cx| {
        grab_state(cx, window).0
    });
    harness.visual(window).simulate_modifiers_change(Modifiers {
        platform: true,
        ..Modifiers::default()
    });
    harness.wait_for("releasing Control to disarm it", |cx| {
        grab_state(cx, window) == (false, false)
    });
}

#[gpui::test]
fn with_nothing_focused_the_next_pointer_move_records_the_modifiers(
    cx: &mut TestAppContext,
) {
    let mut harness = grab_harness(cx);
    let (window, _, first, _) = harness.grab_window(ECHO);
    let centre = harness.terminal_rect(window, first).center();
    harness
        .visual(window)
        .simulate_mouse_move(centre, None, Modifiers::none());
    harness.update_view(window, |_, window, cx| window.blur(cx));
    // No element is focused, so no listener hears the change.
    harness.visual(window).simulate_modifiers_change(cmd_ctrl());
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        assert_eq!(grab_state(cx, window), (false, false));
    });
    harness.visual(window).simulate_mouse_move(
        centre + gpui::point(px(4.0), px(0.0)),
        None,
        cmd_ctrl(),
    );
    harness.wait_for("the move to record the combination", |cx| {
        grab_state(cx, window).0
    });
}

#[gpui::test]
fn a_modifier_press_drags_a_reporting_pane_without_reaching_it(
    cx: &mut TestAppContext,
) {
    let mut harness = grab_harness(cx);
    let (window, tab, first, second) = harness.grab_window(REPORTING);
    let before = harness.probe_pane(window, tab, first);
    let grab = harness.terminal_rect(window, first).center();
    let frame = harness.terminal_rect(window, second);
    let edge = gpui::point(frame.right() - px(8.0), frame.center().y);
    let mut visual = harness.visual(window);
    visual.simulate_modifiers_change(cmd_ctrl());
    visual.simulate_mouse_down(grab, MouseButton::Left, cmd_ctrl());
    harness.cx.run_until_parked();
    // The press shows the closed hand before it moves.
    harness.update_view(window, |view, _, _| {
        assert!(view.pane_drag.is_some());
        assert_eq!(view.drag_cursor(), Some(gpui::CursorStyle::ClosedHand));
    });
    // Letting the modifiers go mid-drag changes nothing.
    let mut visual = harness.visual(window);
    visual.simulate_modifiers_change(Modifiers::none());
    visual.simulate_mouse_move(
        edge,
        Some(MouseButton::Left),
        Modifiers::none(),
    );
    harness.wait_for("the drag to target the right edge", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    matches!(
                        view.read(cx).pane_drag.as_ref().and_then(|d| d.target),
                        Some(DropTarget::Pane {
                            pane,
                            zone: Zone::Edge(SplitDirection::Right),
                            ..
                        }) if pane == second
                    )
                })
            })
            .unwrap_or(false)
    });
    harness.visual(window).simulate_mouse_up(
        edge,
        MouseButton::Left,
        Modifiers::none(),
    );
    harness.wait_for("the pane to move to the right", |cx| {
        !pressed(cx, window)
            && panes_shown(cx, window, tab)
                == Some((vec![second, first], first))
    });
    // The pane saw no press or release report, and selected nothing.
    let after = harness.probe_pane(window, tab, first);
    assert_eq!(after, format!("{before}7a"));
    harness.cx.update(|cx| {
        let view = pane_view(cx, window, tab, first).unwrap();
        assert!(view.read(cx).selection.is_none());
    });
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn a_remapped_control_press_drags_and_its_right_release_drops(
    cx: &mut TestAppContext,
) {
    let mut harness = grab_harness(cx);
    let (window, tab, first, second) = harness.grab_window(REPORTING);
    let before = harness.probe_pane(window, tab, first);
    let grab = harness.terminal_rect(window, first).center();
    let frame = harness.terminal_rect(window, second);
    let edge = gpui::point(frame.right() - px(8.0), frame.center().y);
    let cmd = cmd();
    // As AppKit delivers Cmd-Control-left through GPUI: the flags, a
    // Right press without Control, Left-pressed motion that still carries
    // it, and a Right release without it.
    let mut visual = harness.visual(window);
    visual.simulate_modifiers_change(cmd_ctrl());
    visual.simulate_mouse_down(grab, MouseButton::Right, cmd);
    visual.simulate_mouse_move(edge, Some(MouseButton::Left), cmd_ctrl());
    harness.wait_for("the remapped press to drag", |cx| dragging(cx, window));
    harness
        .visual(window)
        .simulate_mouse_up(edge, MouseButton::Right, cmd);
    harness.wait_for("the Right release to drop the pane", |cx| {
        !pressed(cx, window)
            && panes_shown(cx, window, tab)
                == Some((vec![second, first], first))
    });
    // No menu opened, and the pane saw neither the press nor the release.
    harness.update_view(window, |view, _, _| assert!(view.menu.is_none()));
    let after = harness.probe_pane(window, tab, first);
    assert_eq!(after, format!("{before}7a"));
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn a_remapped_drag_drops_on_a_left_release_once_control_is_let_go(
    cx: &mut TestAppContext,
) {
    let mut harness = grab_harness(cx);
    let (window, tab, first, second) = harness.grab_window(REPORTING);
    let before = harness.probe_pane(window, tab, first);
    let grab = harness.terminal_rect(window, first).center();
    let frame = harness.terminal_rect(window, second);
    let edge = gpui::point(frame.right() - px(8.0), frame.center().y);
    let mut visual = harness.visual(window);
    visual.simulate_modifiers_change(cmd_ctrl());
    visual.simulate_mouse_down(grab, MouseButton::Right, cmd());
    visual.simulate_mouse_move(edge, Some(MouseButton::Left), cmd_ctrl());
    harness.wait_for("the remapped press to drag", |cx| dragging(cx, window));
    // Control goes first, so GPUI leaves the release as Left.
    let mut visual = harness.visual(window);
    visual.simulate_modifiers_change(cmd());
    visual.simulate_mouse_up(edge, MouseButton::Left, cmd());
    harness.wait_for("the Left release to drop the pane", |cx| {
        !pressed(cx, window)
            && panes_shown(cx, window, tab)
                == Some((vec![second, first], first))
    });
    harness.update_view(window, |view, _, _| assert!(view.menu.is_none()));
    let after = harness.probe_pane(window, tab, first);
    assert_eq!(after, format!("{before}7a"));
}

#[gpui::test]
fn a_replayed_drag_move_does_not_rearm_after_the_modifiers_go(
    cx: &mut TestAppContext,
) {
    let mut harness = grab_harness(cx);
    let (window, _, first, _) = harness.grab_window(ECHO);
    let grab = harness.terminal_rect(window, first).center();
    let moved = grab + gpui::point(px(4.0), px(0.0));
    let mut visual = harness.visual(window);
    visual.simulate_modifiers_change(cmd_ctrl());
    visual.simulate_mouse_down(grab, MouseButton::Left, cmd_ctrl());
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), cmd_ctrl());
    visual.simulate_modifiers_change(Modifiers::none());
    // macOS GPUI replays the last move, with its modifiers, while the
    // button stays down.
    visual.simulate_mouse_move(moved, Some(MouseButton::Left), cmd_ctrl());
    harness.cx.run_until_parked();
    // The replay alone leaves the record alone, so a drag that ends on a
    // release the record ignores cannot keep the old modifiers.
    harness.update_view(window, |view, _, _| {
        assert_eq!(view.pane_grab.held(), 0);
    });
    let mut visual = harness.visual(window);
    visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::none());
    harness.cx.run_until_parked();
    harness.update_view(window, |view, window, cx| {
        assert!(view.pane_drag.is_none());
        assert_eq!(view.pane_grab.held(), 0);
        view.refresh_tab_visibility(window, cx);
    });
    harness.animate_grab(window, Instant::now() + Duration::from_secs(1));
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        assert_eq!(grab_state(cx, window), (false, false));
    });
    harness.update_view(window, |view, _, _| {
        assert!(view.drawn_badges.borrow().is_empty());
    });
}

#[gpui::test]
fn a_pending_shortcut_prefix_hides_the_badges(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.panes.drag_modifier = Some(
            PaneDragModifiers::parse("cmd-ctrl", cfg!(target_os = "macos"))
                .unwrap(),
        );
        // A chord whose first stroke uses the drag modifiers.
        config.keybindings.push(huterm_config::KeybindingEntry {
            key: "cmd-ctrl-y cmd-ctrl-u".to_owned(),
            command: "copy".to_owned(),
            args: None,
            when: None,
            description: None,
            disabled: None,
        });
    });
    let (window, _, first, _) = harness.grab_window(ECHO);
    let centre = harness.terminal_rect(window, first).center();
    let mut visual = harness.visual(window);
    visual.simulate_mouse_move(centre, None, Modifiers::none());
    visual.simulate_modifiers_change(cmd_ctrl());
    harness.wait_for("the combination to arm", |cx| grab_state(cx, window).0);
    harness.animate_grab(window, Instant::now() + Duration::from_secs(1));
    harness.wait_for("the badges to show", |cx| {
        grab_state(cx, window) == (true, true)
    });
    harness.press(window, "cmd-ctrl-y");
    harness.update_view(window, |_, window, _| {
        assert!(window.has_pending_keystrokes(), "the prefix is pending");
    });
    harness.wait_for("the prefix to hide the badges", |cx| {
        grab_state(cx, window) == (true, false)
    });
}

#[gpui::test]
fn presses_without_the_combination_reach_a_split_panes_terminal(
    cx: &mut TestAppContext,
) {
    let mut harness = grab_harness(cx);
    let (window, tab, first, _) = harness.grab_window(REPORTING);
    let centre = harness.terminal_rect(window, first).center();
    for modifiers in [
        Modifiers::none(),
        Modifiers {
            control: true,
            alt: true,
            ..Modifiers::default()
        },
    ] {
        let before = harness.probe_pane(window, tab, first);
        harness.visual(window).simulate_modifiers_change(modifiers);
        harness.visual(window).simulate_click(centre, modifiers);
        harness.cx.run_until_parked();
        assert!(!harness.cx.update(|cx| pressed(cx, window)));
        let after = harness.probe_pane(window, tab, first);
        assert!(
            after.len() > before.len() + 2,
            "{modifiers:?} sent no report: {after}"
        );
    }
}

#[gpui::test]
fn a_lone_pane_reports_a_press_with_the_combination(cx: &mut TestAppContext) {
    let mut harness = grab_harness(cx);
    let lone = harness.open_tab(REPORTING);
    let window = harness.attach_window();
    harness.activate(window, lone);
    harness.wait_text(window, lone, "READY");
    let pane = harness.first_pane(lone);
    let before = harness.probe_pane(window, lone, pane);
    let centre = harness.terminal_rect(window, pane).center();
    harness.visual(window).simulate_modifiers_change(cmd_ctrl());
    harness.visual(window).simulate_click(centre, cmd_ctrl());
    harness.cx.run_until_parked();
    assert!(!harness.cx.update(|cx| pressed(cx, window)));
    let after = harness.probe_pane(window, lone, pane);
    assert!(after.len() > before.len() + 2, "no report: {after}");
}

#[gpui::test]
fn an_inactive_windows_pane_takes_a_press_with_the_combination(
    cx: &mut TestAppContext,
) {
    let mut harness = grab_harness(cx);
    // With no program reporting, the press starts a selection.
    let tab = harness.open_tab(ECHO);
    let (split, first, _) = harness.split_window(tab);
    let centre = harness.terminal_rect(split, first).center();
    harness.deactivate(split);
    let mut visual = harness.visual(split);
    visual.simulate_modifiers_change(cmd_ctrl());
    visual.simulate_mouse_down(centre, MouseButton::Left, cmd_ctrl());
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        assert!(!pressed(cx, split));
        let view = pane_view(cx, split, tab, first).unwrap();
        assert!(view.read(cx).selection.is_some(), "the terminal missed it");
    });
    harness.visual(split).simulate_mouse_up(
        centre,
        MouseButton::Left,
        cmd_ctrl(),
    );
}

#[gpui::test]
fn a_combination_press_on_a_hover_bar_button_or_a_toast_stays_there(
    cx: &mut TestAppContext,
) {
    let mut harness = grab_harness(cx);
    let (window, tab, first, second) = harness.grab_window(ECHO);
    harness.update_view(window, |view, window, cx| {
        let mut config = view.config.clone();
        config.panes.title_bar = PaneTitleBarMode::Hover;
        let loaded = (config, view.family.clone(), view.metrics);
        view.apply_reload(Some(&loaded), &[], window, cx);
    });
    harness.wait_for("hover geometry", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx).drawn_panes.borrow().as_ref().is_some_and(
                        |(_, rects)| {
                            rects.iter().all(|rect| rect.terminal == rect.frame)
                        },
                    )
                })
            })
            .unwrap_or(false)
    });
    // The pointer rests on the first pane's band until its bar reveals.
    let bar = harness.update_view(window, |view, _, _| {
        view.drawn_panes.borrow().as_ref().and_then(|(_, rects)| {
            rects.iter().find(|rect| rect.pane == first)?.title_bar
        })
    });
    let bar = bar.expect("a hover bar");
    harness.visual(window).simulate_mouse_move(
        bar.center(),
        None,
        Modifiers::none(),
    );
    harness.cx.run_until_parked();
    let start = Instant::now();
    let mut now = start;
    while now < start + Duration::from_millis(600) {
        now += Duration::from_millis(20);
        harness.animate_grab(window, now);
    }
    harness.update_view(window, |view, _, _| {
        assert!(view.pane_bars.progress(first) > 0.99, "the bar revealed");
    });
    // Its zoom button takes the combination's click.
    harness.visual(window).simulate_modifiers_change(cmd_ctrl());
    harness
        .visual(window)
        .simulate_click(WorkspaceView::pane_zoom_center(bar), cmd_ctrl());
    harness.wait_for("the first pane to zoom", |cx| {
        pane_shown(cx, window, tab, second).is_some_and(|(shown, _)| !shown)
    });
    assert!(!harness.cx.update(|cx| pressed(cx, window)));
    harness.invoke(window, ids::TOGGLE_PANE_ZOOM).unwrap();
    harness.wait_for("both panes to show again", |cx| {
        pane_shown(cx, window, tab, second).is_some_and(|(shown, _)| shown)
    });

    // A toast's dismiss button takes it too.
    harness.update_view(window, |view, _, cx| {
        view.notify(
            NoticeContent::diagnostic(
                Severity::Error,
                NoticeSource::Config,
                "Config has errors",
                "line 3",
            ),
            cx,
        );
    });
    harness.cx.run_until_parked();
    let dismiss = harness
        .visual(window)
        .debug_bounds("toast-Dismiss Config has errors")
        .expect("the toast drew");
    harness
        .visual(window)
        .simulate_click(dismiss.center(), cmd_ctrl());
    harness
        .wait_for("the toast to dismiss", |cx| notices(cx, window).is_empty());
    assert!(!harness.cx.update(|cx| pressed(cx, window)));
}

#[gpui::test]
fn badges_name_each_pane_after_the_delay_and_a_key_hides_them(
    cx: &mut TestAppContext,
) {
    let mut harness = grab_harness(cx);
    let (window, tab, first, second) = harness.grab_window(ECHO);
    harness
        .runtime
        .lock()
        .rename_pane(second, Some("logs"))
        .unwrap();
    let label = harness.update_view(window, |view, _, cx| {
        let tab = view.tabs.iter().find(|view| view.id == tab).unwrap();
        tab.pane_label(tab.pane(first).unwrap(), view.config.tabs, cx)
    });
    let centre = harness.terminal_rect(window, first).center();
    let mut visual = harness.visual(window);
    visual.simulate_mouse_move(centre, None, Modifiers::none());
    visual.simulate_modifiers_change(cmd_ctrl());
    let held = Instant::now();
    harness.wait_for("the combination to arm", |cx| grab_state(cx, window).0);
    let smoke = |harness: &mut DesktopHarness| {
        harness.update_view(window, |view, window, cx| {
            window.refresh();
            view.pane_smoke_state("", window, cx)
        })
    };
    harness.cx.run_until_parked();
    assert!(smoke(&mut harness).contains("grab_armed=true"));
    // The badges show once the combination has been held for the delay.
    harness.animate_grab(window, held + Duration::from_millis(200));
    harness.wait_for("both badges to draw", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx).drawn_badges.borrow().len() == 2
                })
            })
            .unwrap_or(false)
    });
    let state = smoke(&mut harness);
    assert!(
        state.contains(&format!(
            "grab_badge_titles={:?}",
            vec![label.as_str(), "logs"]
        )),
        "{state}"
    );
    // A key pressed with the combination held, as a pane keybinding's,
    // hides them until the modifiers change.
    harness.press(window, "x");
    harness.wait_for("the key to hide the badges", |cx| {
        grab_state(cx, window) == (true, false)
    });
}
