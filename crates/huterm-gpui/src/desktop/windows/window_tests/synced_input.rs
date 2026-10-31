//! Tier 2 tests for pane input sync: typing into a syncing tab's focused
//! pane reaches each of its other panes' real shells in order, encoded for
//! each pane's own modes, while Huterm commands still act once and panes
//! that left, exited, or are hidden get nothing. The tab's mark shows in
//! the title row while the tab bar is hidden, and the tab menu toggles its
//! own tab.

use huterm_core::{MoveDestination, PaneRef};
use huterm_ui::components::kbd;
use huterm_ui::listbox::Row as ListRow;
use serde_json::Value;

use super::super::input_sync::syncs_input;
use super::super::tab_bar::TabMark;
use super::pane_bars::focus_marks;
use super::*;

/// A child like [`HEX_ECHO`] that first turns on application cursor keys
/// and bracketed paste, so Up and a paste encode differently from a pane
/// without them.
const MODAL_HEX_ECHO: &str = "printf '\\033[?1h\\033[?2004h'; \
     stty raw -echo; printf READY; \
     while byte=$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\n'); \
     [ -n \"$byte\" ]; do printf '%s' \"$byte\"; done";

/// The bytes a hex-echo child in `pane` reports after `marker`.
fn pane_received(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
    pane: PaneId,
    marker: &str,
) -> Option<String> {
    let text = pane_screen(cx, window, tab, pane)?;
    let (_, echoed) = text.split_once(marker)?;
    Some(echoed.chars().filter(char::is_ascii_hexdigit).collect())
}

/// Whether `pane` shows each of `lines` an [`ECHO`] child answered, in
/// order.
fn answered(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
    pane: PaneId,
    lines: &[&str],
) -> bool {
    let Some(text) = pane_screen(cx, window, tab, pane) else {
        return false;
    };
    let mut rest = text.as_str();
    lines.iter().all(|line| {
        let answer = format!("got:{line} ");
        rest.split_once(&answer)
            .map(|(_, after)| rest = after)
            .is_some()
    })
}

impl DesktopHarness {
    /// Toggles the active tab's input sync in `window` and waits for the
    /// projection to show `tab` at `on`.
    fn toggle_sync(&mut self, window: AnyWindowHandle, tab: TabId, on: bool) {
        self.invoke(window, ids::TOGGLE_PANE_INPUT_SYNC).unwrap();
        self.wait_for("the tab's input sync to change", |cx| {
            syncs_input(tab, cx) == on
        });
    }
}

#[gpui::test]
fn typing_reaches_every_pane_of_a_syncing_tab_in_order(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    harness.type_line(window, "solo");
    harness.wait_for("the focused pane to answer", |cx| {
        answered(cx, window, tab, first, &["solo"])
    });
    harness.toggle_sync(window, tab, true);
    harness.type_line(window, "ab");
    harness.type_line(window, "cd");
    harness.wait_for("both shells to answer both lines in order", |cx| {
        [first, second]
            .into_iter()
            .all(|pane| answered(cx, window, tab, pane, &["ab", "cd"]))
    });
    harness.toggle_sync(window, tab, false);
    harness.type_line(window, "ef");
    harness.focus(window, tab, second);
    harness.type_line(window, "gh");
    // The second shell reads its input in order, so "ef" would have
    // answered before "gh" had it been sent there.
    harness.wait_for("each pane to answer its own line", |cx| {
        answered(cx, window, tab, first, &["ab", "cd", "ef"])
            && answered(cx, window, tab, second, &["ab", "cd", "gh"])
    });
    harness.cx.update(|cx| {
        let first = pane_screen(cx, window, tab, first).unwrap();
        let second = pane_screen(cx, window, tab, second).unwrap();
        assert!(!second.contains("got:solo") && !second.contains("got:ef"));
        assert!(!first.contains("got:gh"));
    });
}

#[gpui::test]
fn synced_keys_and_paste_encode_for_each_panes_own_modes(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(HEX_ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    let plain = harness.first_pane(tab);
    let modal =
        harness.split(tab, plain, SplitDirection::Right, MODAL_HEX_ECHO);
    harness.wait_for("the modal pane to start", |cx| {
        pane_screen(cx, window, tab, modal)
            .is_some_and(|text| text.contains("READY"))
    });
    harness.toggle_sync(window, tab, true);
    harness.press(window, "up");
    harness
        .cx
        .write_to_clipboard(gpui::ClipboardItem::new_string("x".to_owned()));
    harness.press(
        window,
        match crate::keymap::Platform::current() {
            crate::keymap::Platform::MacOs => "cmd-v",
            crate::keymap::Platform::Linux => "ctrl-shift-v",
        },
    );
    // Up, then the pasted `x`, bracketed only where the child asked.
    let plain_bytes = "1b5b41".to_owned() + "78";
    let modal_bytes =
        "1b4f41".to_owned() + "1b5b3230307e" + "78" + "1b5b3230317e";
    harness.wait_for("both children to report the key and the paste", |cx| {
        pane_received(cx, window, tab, plain, "READY").as_deref()
            == Some(plain_bytes.as_str())
            && pane_received(cx, window, tab, modal, "READY").as_deref()
                == Some(modal_bytes.as_str())
    });
}

#[gpui::test]
fn a_pane_split_in_joins_the_sync_and_a_split_key_acts_once(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    harness.toggle_sync(window, tab, true);
    // A pane split in while the tab syncs receives what the focused pane
    // types once its view installs.
    let third = harness.split(tab, second, SplitDirection::Down, ECHO);
    harness.wait_for("the new pane to start", |cx| {
        pane_screen(cx, window, tab, third)
            .is_some_and(|text| text.contains("READY"))
    });
    harness.type_line(window, "all");
    harness.wait_for("every pane to answer", |cx| {
        [first, second, third]
            .into_iter()
            .all(|pane| answered(cx, window, tab, pane, &["all"]))
    });
    // The split key runs its command once, not once per synced pane. Its
    // new pane runs the login shell, so nothing is typed into it.
    let split = match crate::keymap::Platform::current() {
        crate::keymap::Platform::MacOs => "cmd-d",
        crate::keymap::Platform::Linux => "ctrl-shift-o",
    };
    harness.press(window, split);
    harness.wait_for("one new pane to install and focus", |cx| {
        panes_shown(cx, window, tab).is_some_and(|(panes, focused)| {
            panes.len() == 4 && ![first, second, third].contains(&focused)
        })
    });
    assert_eq!(
        harness
            .runtime
            .lock()
            .tab(tab)
            .unwrap()
            .layout()
            .pane_count(),
        4,
        "the split key split once"
    );
}

#[gpui::test]
fn panes_that_left_or_exited_get_no_synced_input_and_break_nothing(
    cx: &mut TestAppContext,
) {
    // The exited pane stays, so the sync passes over it.
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.terminal.close_on_exit = false;
    });
    let tab = harness.open_tab(ECHO);
    let (window, first, leaving) = harness.split_window(tab);
    let exiting = harness.split(
        tab,
        leaving,
        SplitDirection::Down,
        "printf READY; exit 0",
    );
    harness.wait_for("the third pane to exit", |cx| {
        pane_view(cx, window, tab, exiting)
            .is_some_and(|view| view.read(cx).exited)
    });
    harness.toggle_sync(window, tab, true);
    let from = PaneRef {
        workspace: harness.workspace,
        tab,
        pane: leaving,
    };
    let change = harness
        .runtime
        .lock()
        .move_pane(
            from,
            MoveDestination::NewTab {
                workspace: harness.workspace,
                before: None,
            },
        )
        .unwrap();
    let other = change.after[1].tab;
    harness.wait_for("the moved pane to leave the tab", |cx| {
        panes_shown(cx, window, tab)
            .is_some_and(|(panes, _)| panes == [first, exiting])
            && pane_view(cx, window, other, leaving).is_some()
    });
    harness.cx.update(|cx| {
        assert!(!syncs_input(other, cx), "a moved pane's new tab starts off");
    });
    // The exited pane takes no input, so only the live pane is framed.
    harness.wait_for("only the live pane to draw a sync frame", |cx| {
        focus_marks(cx, window).is_some_and(|marks| marks.synced == [first])
    });
    // Typing in the exited pane goes nowhere, so nothing is framed while it
    // has focus.
    harness.focus(window, tab, exiting);
    harness.wait_for("no sync frame while the exited pane has focus", |cx| {
        focus_marks(cx, window).is_some_and(|marks| marks.synced.is_empty())
    });
    // Nor does typing there sync: the live shell answers "stay" below
    // without having read "dead" first.
    harness.type_line(window, "dead");
    harness.focus(window, tab, first);
    harness.wait_for("the live pane's frame to return", |cx| {
        focus_marks(cx, window).is_some_and(|marks| marks.synced == [first])
    });
    harness.type_line(window, "stay");
    harness.wait_for("the focused pane to answer", |cx| {
        answered(cx, window, tab, first, &["stay"])
    });
    harness.focus(window, other, leaving);
    harness.type_line(window, "own");
    harness.wait_for("the moved pane to answer its own line", |cx| {
        answered(cx, window, other, leaving, &["own"])
    });
    harness.cx.update(|cx| {
        let moved = pane_screen(cx, window, other, leaving).unwrap();
        assert!(!moved.contains("got:stay"), "{moved}");
        let live = pane_screen(cx, window, tab, first).unwrap();
        assert!(!live.contains("got:dead"), "{live}");
        // At most the exit's own notice, which may have expired: no pane
        // refused or failed its copy.
        let notices = notices(cx, window);
        assert!(
            notices.iter().all(|notice| notice
                == "info|terminal:sh|Process exited with status 0"),
            "{notices:?}"
        );
    });
}

#[gpui::test]
fn a_pane_a_zoom_hides_gets_no_synced_input(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    harness.toggle_sync(window, tab, true);
    harness.invoke(window, ids::TOGGLE_PANE_ZOOM).unwrap();
    harness.wait_for("the focused pane to zoom", |cx| {
        harness_zoom(cx, window, tab) == Some(first)
    });
    harness.type_line(window, "hidden");
    harness.wait_for("the zoomed pane to answer", |cx| {
        answered(cx, window, tab, first, &["hidden"])
    });
    harness.invoke(window, ids::TOGGLE_PANE_ZOOM).unwrap();
    harness.wait_for("the tab to unzoom", |cx| {
        harness_zoom(cx, window, tab).is_none()
    });
    harness.type_line(window, "shown");
    harness.wait_for("both panes to answer the line typed unzoomed", |cx| {
        answered(cx, window, tab, first, &["hidden", "shown"])
            && answered(cx, window, tab, second, &["shown"])
    });
    harness.cx.update(|cx| {
        let hidden = pane_screen(cx, window, tab, second).unwrap();
        assert!(!hidden.contains("got:hidden"), "{hidden}");
    });
}

/// The pane `window` shows across `tab`, if any.
fn harness_zoom(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
) -> Option<PaneId> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            let view = view.read(cx);
            let tab = view.tabs.iter().find(|view| view.id == tab)?;
            view.shown_zoom(tab, cx)
        })
        .ok()
        .flatten()
}

#[gpui::test]
fn a_toggle_elsewhere_redraws_the_frames_of_every_pane(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, first, second) = harness.split_window(tab);
    harness.wait_for("a frame without sync frames", |cx| {
        focus_marks(cx, window).is_some_and(|marks| marks.synced.is_empty())
    });
    // Another client's toggle changes no title, so only the hierarchy
    // change can redraw this window.
    let set = |harness: &DesktopHarness, on| {
        harness
            .runtime
            .lock()
            .set_input_sync(harness.workspace, tab, on)
            .unwrap();
    };
    set(&harness, true);
    harness.wait_for("both panes to draw their sync frames", |cx| {
        focus_marks(cx, window)
            .is_some_and(|marks| marks.synced == [first, second])
    });
    set(&harness, false);
    harness.wait_for("the sync frames to go", |cx| {
        focus_marks(cx, window).is_some_and(|marks| marks.synced.is_empty())
    });
}

/// The label `window`'s open palette shows on the sync toggle's row.
fn palette_sync_label(cx: &mut App, window: AnyWindowHandle) -> Option<String> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            let palette = view.read(cx).palette.clone()?;
            let label = palette
                .read(cx)
                .shown_label(ids::TOGGLE_PANE_INPUT_SYNC, cx)?;
            Some(label.to_string())
        })
        .ok()
        .flatten()
}

#[gpui::test]
fn the_palette_row_says_what_the_toggle_will_do_while_open(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, _, _) = harness.split_window(tab);
    harness.invoke(window, ids::OPEN_COMMAND_PALETTE).unwrap();
    harness.press(window, "s y n c");
    harness.wait_for("the palette to offer to sync", |cx| {
        palette_sync_label(cx, window).as_deref() == Some("Sync Pane Input")
    });
    // A toggle that lands while the palette is open, as one still in
    // flight when it opened would, retitles the shown row.
    harness
        .runtime
        .lock()
        .set_input_sync(harness.workspace, tab, true)
        .unwrap();
    harness.wait_for("the open palette to offer to stop", |cx| {
        palette_sync_label(cx, window).as_deref()
            == Some("Stop Syncing Pane Input")
    });
    harness.cx.update(|cx| {
        let state = palette_state(cx, window).unwrap();
        assert!(
            state.contains("selected=toggle_pane_input_sync"),
            "the highlight stays on the row: {state}"
        );
    });
}

/// The marks `window`'s last frame drew in its title row before the active
/// tab's title, or `None` when the row drew no title.
fn title_marks(cx: &mut App, window: AnyWindowHandle) -> Option<Vec<TabMark>> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx).drawn_title_marks.borrow().clone()
        })
        .ok()
        .flatten()
}

/// How many image nodes in `window`'s last accessibility tree read
/// "Syncing pane input".
fn sync_mark_nodes(cx: &mut App, window: AnyWindowHandle) -> usize {
    let json = window
        .update(cx, |_, window, _| window.debug_a11y_tree_json())
        .unwrap()
        .expect("test windows report an accessibility tree for every frame");
    let tree: Value = serde_json::from_str(&json).unwrap();
    tree["nodes"]
        .as_object()
        .unwrap()
        .values()
        .filter(|node| {
            node["aria"]["role"] == "Image"
                && node["aria"]["label"] == TabMark::InputSync.name()
        })
        .count()
}

/// Whether `window`'s title row drew `marks` and no others.
fn title_shows(
    cx: &mut App,
    window: AnyWindowHandle,
    marks: &[TabMark],
) -> bool {
    title_marks(cx, window).is_some_and(|drawn| drawn == marks)
}

/// A harness whose windows have a title row on every host: macOS's strip,
/// or Linux's client-decorated row, which only the title-bar tab position
/// draws and [`DesktopHarness::client_row`] turns on.
fn title_row_harness(
    cx: &mut TestAppContext,
    configure: impl FnOnce(&mut Config),
) -> DesktopHarness {
    DesktopHarness::with_config(cx, |config| {
        if cfg!(not(target_os = "macos")) {
            config.tabs.position = crate::config::TabPosition::Titlebar;
        }
        configure(config);
    })
}

impl DesktopHarness {
    /// Gives `window` the client frame a Linux desktop without a title bar
    /// grants, on any host, as GPUI reports a decoration change; the test
    /// platform itself reports server decorations.
    fn client_row(&mut self, window: AnyWindowHandle) {
        self.update_view(window, |view, window, cx| {
            view.decorations_override = Some(gpui::Decorations::Client {
                tiling: gpui::Tiling::default(),
            });
            view.sync_frame(window);
            view.refresh_tab_visibility(window, cx);
            view.sync_tab_layout(window, cx);
            cx.notify();
        });
    }

    /// A window on a new [`ECHO`] tab with the title row
    /// [`title_row_harness`] promises.
    fn title_row_window(&mut self) -> (AnyWindowHandle, TabId) {
        let tab = self.open_tab(ECHO);
        let window = self.attach_window();
        self.wait_text(window, tab, "READY");
        self.activate(window, tab);
        if cfg!(not(target_os = "macos")) {
            self.client_row(window);
        }
        self.wait_for("the title row to draw the title", |cx| {
            title_shows(cx, window, &[])
        });
        (window, tab)
    }
}

#[gpui::test]
fn the_title_row_shows_the_tabs_marks_only_while_the_tab_bar_is_hidden(
    cx: &mut TestAppContext,
) {
    let mut harness = title_row_harness(cx, |_| {});
    let (window, tab) = harness.title_row_window();
    harness.toggle_sync(window, tab, true);
    harness.wait_for("the title row to show the sync mark", |cx| {
        title_shows(cx, window, &[TabMark::InputSync])
    });
    harness.cx.update(|cx| {
        assert_eq!(sync_mark_nodes(cx, window), 1, "the mark is an image");
    });
    // The marks sit before the title, inside the row.
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    let marks = visual.debug_bounds("title-marks").expect("the marks drew");
    let title = visual.debug_bounds("strip-title").expect("the title drew");
    assert!(marks.right() <= title.origin.x, "{marks:?} {title:?}");
    assert!(marks.origin.y >= px(0.0) && marks.bottom() <= TAB_HEIGHT);
    // A second tab shows the tab bar, whose tab carries the mark instead.
    let other = harness.open_tab(ECHO);
    harness.wait_for("the tab bar's mark and no title mark", |cx| {
        terminal(cx, window, other).is_some()
            && sync_mark_nodes(cx, window) == 1
            && title_marks(cx, window).unwrap_or_default().is_empty()
    });
    harness.close_tab(other);
    harness.wait_for("the title mark to return with one tab", |cx| {
        title_shows(cx, window, &[TabMark::InputSync])
    });
    harness.toggle_sync(window, tab, false);
    harness.wait_for("the title mark to go", |cx| {
        title_shows(cx, window, &[]) && sync_mark_nodes(cx, window) == 0
    });
}

#[gpui::test]
fn an_always_shown_tab_bar_keeps_the_mark_out_of_the_title_row(
    cx: &mut TestAppContext,
) {
    let mut harness = title_row_harness(cx, |config| {
        config.tabs.always_show = true;
    });
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    if cfg!(not(target_os = "macos")) {
        harness.client_row(window);
    }
    harness.toggle_sync(window, tab, true);
    // The tab bar's mark is the only one.
    harness.wait_for("the tab's mark to draw", |cx| {
        sync_mark_nodes(cx, window) == 1
    });
    harness.cx.update(|cx| {
        let drawn = title_marks(cx, window).unwrap_or_default();
        assert!(drawn.is_empty(), "{drawn:?}");
    });
}

#[gpui::test]
fn the_title_row_keeps_the_mark_while_the_focused_pane_has_exited(
    cx: &mut TestAppContext,
) {
    let mut harness = title_row_harness(cx, |config| {
        config.terminal.close_on_exit = false;
    });
    let tab = harness.open_tab("printf READY; exit 0");
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    if cfg!(not(target_os = "macos")) {
        harness.client_row(window);
    }
    let exited = harness.first_pane(tab);
    harness.split(tab, exited, SplitDirection::Right, ECHO);
    harness.focus(window, tab, exited);
    harness.wait_for("the focused pane to exit", |cx| {
        pane_view(cx, window, tab, exited)
            .is_some_and(|view| view.read(cx).exited)
    });
    harness.toggle_sync(window, tab, true);
    // Typing reaches no pane, so no frame or pane bar mark shows; the
    // title row still says the tab syncs.
    harness.wait_for("the title row's mark alone", |cx| {
        title_shows(cx, window, &[TabMark::InputSync])
            && sync_mark_nodes(cx, window) == 1
            && focus_marks(cx, window)
                .is_some_and(|marks| marks.synced.is_empty())
    });
}

/// macOS keeps its strip under the title-bar tab position while one tab
/// hides the bar; Linux draws that row only with client decorations, which
/// the next test covers on every host.
#[gpui::test]
#[cfg_attr(
    not(target_os = "macos"),
    ignore = "Linux draws the title-bar row only with client decorations"
)]
fn the_title_bar_position_with_one_tab_shows_the_mark_in_the_strip(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.position = crate::config::TabPosition::Titlebar;
    });
    let (window, tab) = harness.title_row_window();
    harness.toggle_sync(window, tab, true);
    harness.wait_for("the strip to show the sync mark", |cx| {
        title_shows(cx, window, &[TabMark::InputSync])
    });
}

#[gpui::test]
fn the_linux_client_row_shows_the_mark_before_the_title(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.position = crate::config::TabPosition::Titlebar;
    });
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    harness.client_row(window);
    harness.toggle_sync(window, tab, true);
    harness.wait_for("the client row to show the sync mark", |cx| {
        title_shows(cx, window, &[TabMark::InputSync])
            && window
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>()
                        .is_ok_and(|view| view.read(cx).window_frame().controls)
                })
                .unwrap_or(false)
    });
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    let marks = visual.debug_bounds("title-marks").expect("the marks drew");
    let title = visual.debug_bounds("strip-title").expect("the title drew");
    assert!(marks.right() <= title.origin.x, "{marks:?} {title:?}");
    assert!(marks.bottom() <= TAB_HEIGHT, "inside the row: {marks:?}");
}

#[gpui::test]
fn fullscreen_draws_no_title_row_and_so_no_mark(cx: &mut TestAppContext) {
    let mut harness = title_row_harness(cx, |_| {});
    let (window, tab) = harness.title_row_window();
    harness.toggle_sync(window, tab, true);
    harness.wait_for("the title row to show the sync mark", |cx| {
        title_shows(cx, window, &[TabMark::InputSync])
    });
    harness.update_view(window, |view, window, cx| {
        window.toggle_fullscreen();
        view.refresh_fullscreen(window, cx);
        cx.notify();
    });
    // With one tab and the fullscreen bar not auto-hiding, the bar stays
    // hidden too, so nothing shows the mark.
    harness.wait_for("fullscreen to drop the title row", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>()
                    .is_ok_and(|view| view.read(cx).fullscreen.chrome_hidden)
            })
            .unwrap_or(false)
            && title_marks(cx, window).is_none()
    });
    harness.cx.update(|cx| {
        assert_eq!(sync_mark_nodes(cx, window), 0);
    });
}

/// The label and shortcut text of the open menu's sync row, if it has one.
fn menu_sync_row(
    cx: &mut App,
    window: AnyWindowHandle,
) -> Option<(String, String)> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            let model = view.read(cx).current_menu_model(cx)?;
            model.rows.iter().find_map(|row| match row {
                ListRow::Item(item)
                    if item.id.as_ref()
                        == ids::TOGGLE_PANE_INPUT_SYNC.as_str() =>
                {
                    Some((item.label.clone(), kbd::text(&item.shortcut)))
                }
                _ => None,
            })
        })
        .ok()
        .flatten()
}

#[gpui::test]
fn the_tab_menu_toggles_its_own_tab_and_not_the_active_one(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let active = harness.open_tab(ECHO);
    let other = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[active, other]);
    harness.wait_text(window, active, "READY");
    harness.update_view(window, |view, window, cx| {
        view.select(active, window, cx);
    });
    harness.activate(window, active);
    let first = harness.first_pane(other);
    harness.split(other, first, SplitDirection::Right, ECHO);
    harness.wait_for("the inactive tab to hold two panes", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx)
                        .tabs
                        .iter()
                        .any(|tab| tab.id == other && tab.panes.len() == 2)
                })
            })
            .unwrap_or(false)
    });
    let open = |harness: &mut DesktopHarness| {
        harness.update_view(window, |view, window, cx| {
            view.open_tab_menu(other, point(px(40.0), px(10.0)), window, cx);
        });
    };
    let pick = |harness: &mut DesktopHarness| {
        harness.update_view(window, |view, window, cx| {
            let menu = view.menu.as_ref().unwrap().view.clone();
            view.handle_menu_event(
                &menu,
                &MenuEvent::Picked(ids::TOGGLE_PANE_INPUT_SYNC.as_str().into()),
                window,
                cx,
            );
        });
    };
    open(&mut harness);
    // The shortcut would toggle the active tab, so the row shows none.
    assert_eq!(
        harness.cx.update(|cx| menu_sync_row(cx, window)),
        Some(("Sync Pane Input".to_owned(), String::new()))
    );
    pick(&mut harness);
    harness.wait_for("the menu's tab to sync", |cx| syncs_input(other, cx));
    harness.cx.update(|cx| {
        assert!(!syncs_input(active, cx), "the active tab synced");
    });
    assert_eq!(
        harness.update_view(window, |view, _, cx| view.active_tab(cx)),
        Some(active),
        "the pick changed the active tab"
    );
    // Without a tab, the command still toggles the active one.
    harness.toggle_sync(window, active, true);
    harness.cx.update(|cx| assert!(syncs_input(other, cx)));
    open(&mut harness);
    assert_eq!(
        harness.cx.update(|cx| menu_sync_row(cx, window)),
        Some(("Stop Syncing Pane Input".to_owned(), String::new()))
    );
    pick(&mut harness);
    harness.wait_for("the menu's tab to stop", |cx| !syncs_input(other, cx));
    harness.cx.update(|cx| assert!(syncs_input(active, cx)));
}
