//! The session switcher and the session list it opens, through real pointer
//! and keyboard events: the corner switcher's name and state, opening and
//! closing the list, focus returning to the terminal, picks that switch,
//! live updates from other clients, row menus, dragging a session row, the
//! title row's gestures and drop region beside the switcher, and the icon
//! style.

use gpui::{Modifiers, MouseDownEvent, MouseUpEvent};

use super::super::session_control::SessionListAnchor;
use super::menus::a11y_node;
use super::sessions::shown_session;
use super::*;

/// The open menu's kind, as smoke state names it, or `None` while closed.
fn open_kind(cx: &mut App, window: AnyWindowHandle) -> Option<&'static str> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            Some(view.read(cx).menu.as_ref()?.kind.name())
        })
        .ok()
        .flatten()
}

/// Whether the session list is open and its content has painted.
fn list_painted(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>().is_ok_and(|view| {
                let view = view.read(cx);
                view.session_list.is_some()
                    && view.session_list_popover.content_bounds().is_some()
            })
        })
        .unwrap_or(false)
}

/// The session whose list row holds focus.
fn focused_row(cx: &mut App, window: AnyWindowHandle) -> Option<SessionId> {
    window
        .update(cx, |root, window, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx).focused_session_row(window)
        })
        .ok()
        .flatten()
}

/// The runtime's sessions in order.
fn sessions(cx: &mut App) -> Vec<SessionId> {
    cx.global::<Desktop>().hierarchy.state().sessions().to_vec()
}

fn press_at(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    at: gpui::Point<Pixels>,
    clicks: usize,
) {
    VisualTestContext::from_window(window, &harness.cx).simulate_event(
        MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: clicks,
            first_mouse: false,
        },
    );
}

fn release_at(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    at: gpui::Point<Pixels>,
) {
    VisualTestContext::from_window(window, &harness.cx).simulate_event(
        MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count: 1,
        },
    );
}

/// A harness whose one-tab windows show the session switcher beside the
/// sidebar button, which is opt-in. Without client decorations a Linux
/// window has no title row, and a one-tab window hides its bar, which
/// would leave the corner no host; an always-shown bar hosts it there.
fn switcher_harness(cx: &mut TestAppContext) -> DesktopHarness {
    DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
        config.sidebar.session_switcher_in_corner = true;
    })
}

impl DesktopHarness {
    /// Waits for the corner session switcher to paint and returns its
    /// bounds.
    fn session_switcher(&mut self, window: AnyWindowHandle) -> Bounds<Pixels> {
        self.painted(window, "the session switcher to paint", |view| {
            view.session_corner_bounds
                .get()
                .filter(|_| view.session_corner_drawn.get())
        })
    }

    /// The painted bounds of `session`'s list row.
    fn session_row(
        &mut self,
        window: AnyWindowHandle,
        session: SessionId,
    ) -> Bounds<Pixels> {
        self.painted(window, "the session row to paint", |view| {
            view.session_row_bounds.borrow().get(&session).copied()
        })
    }

    /// A focused window on an [`ECHO`] tab in a session named `name`.
    fn named_window(&mut self, name: &str) -> (AnyWindowHandle, TabId) {
        self.runtime
            .lock()
            .rename_session(self.session, Some(name))
            .unwrap();
        let tab = self.open_tab(ECHO);
        let window = self.attach_window();
        self.activate(window, tab);
        self.wait_text(window, tab, "READY");
        (window, tab)
    }

    fn click(&mut self, window: AnyWindowHandle, at: gpui::Point<Pixels>) {
        VisualTestContext::from_window(window, &self.cx)
            .simulate_click(at, Modifiers::none());
    }
}

#[gpui::test]
fn the_corner_switcher_names_its_session_and_toggles_its_list(
    cx: &mut TestAppContext,
) {
    let mut harness = switcher_harness(cx);
    let (window, tab) = harness.named_window("main");
    let switcher = harness.session_switcher(window);
    harness.cx.update(|cx| {
        let aria = &a11y_node(cx, window, "session-switcher-corner")["aria"];
        assert_eq!(aria["role"], "Button");
        assert_eq!(aria["label"], "Session: main");
        assert_eq!(aria["expanded"], false);
    });
    // It sits beside the sidebar button, on its inner side.
    let button = harness
        .update_view(window, |view, _, _| view.sidebar_button_bounds.get())
        .expect("the sidebar button painted");
    assert!(button.right() <= switcher.left(), "{button:?} {switcher:?}");
    assert_eq!(switcher.size.height, CONTROL_SIZE);
    // A click opens Huterm's list, even with native menus, below it, with
    // the window's session's row focused.
    harness.click(window, switcher.center());
    harness
        .wait_for("the session list to paint", |cx| list_painted(cx, window));
    let session = harness.session;
    harness.cx.update(|cx| {
        assert_eq!(focused_row(cx, window), Some(session));
        assert_eq!(open_kind(cx, window), None, "no menu opened");
        assert_eq!(
            a11y_node(cx, window, "session-switcher-corner")["aria"]
                ["expanded"],
            true
        );
        let row = &a11y_node(cx, window, &format!("session-row-{}", session.get()))
            ["aria"];
        assert_eq!(row["label"], "Session: main, 1 workspace · 1 tab");
        assert_eq!(row["selected"], true);
    });
    let rect = harness
        .update_view(window, |view, _, _| {
            assert_eq!(view.session_list, Some(SessionListAnchor::Corner));
            view.session_list_popover.content_bounds()
        })
        .unwrap();
    assert!(rect.top() >= switcher.bottom(), "{rect:?} {switcher:?}");
    // A second press closes it, and focus returns to the terminal.
    harness.click(window, switcher.center());
    harness.wait_for("the list to close", |cx| {
        !list_painted(cx, window) && focused(cx, window, tab)
    });
}

#[gpui::test]
fn the_keyboard_walks_the_list_and_escape_or_a_pick_returns_focus(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab) = harness.named_window("main");
    let other = harness.other_session("other", 1);
    harness.wait_projection();
    // Without a switcher drawn, `open_session_menu` opens the list at the
    // menu button's place, with the window's session's row focused.
    harness.invoke(window, ids::OPEN_SESSION_MENU).unwrap();
    harness
        .wait_for("the session list to paint", |cx| list_painted(cx, window));
    let main = harness.session;
    let row = |harness: &mut DesktopHarness| {
        harness.cx.update(|cx| focused_row(cx, window))
    };
    assert_eq!(row(&mut harness), Some(main));
    harness.press(window, "down");
    assert_eq!(row(&mut harness), Some(other.session));
    harness.press(window, "home");
    assert_eq!(row(&mut harness), Some(main));
    harness.press(window, "end");
    assert_eq!(row(&mut harness), Some(other.session));
    // A repeated open keeps the list and refocuses the window's session.
    harness.invoke(window, ids::OPEN_SESSION_MENU).unwrap();
    assert_eq!(row(&mut harness), Some(main));
    harness.press(window, "escape");
    harness.wait_for("Escape to close the list", |cx| {
        !list_painted(cx, window) && focused(cx, window, tab)
    });
    // Enter on another session switches the window to it, and focus lands
    // on its terminal.
    harness.invoke(window, ids::OPEN_SESSION_MENU).unwrap();
    harness
        .wait_for("the session list to paint", |cx| list_painted(cx, window));
    harness.press(window, "down enter");
    harness.wait_switched(window, other.session, &other.tabs);
    harness.wait_for("focus on the shown session's terminal", |cx| {
        !list_painted(cx, window) && focused(cx, window, other.tabs[0])
    });
}

#[gpui::test]
fn the_open_list_follows_other_clients(cx: &mut TestAppContext) {
    let mut harness = switcher_harness(cx);
    let (window, tab) = harness.named_window("main");
    let first = harness.other_session("first", 1);
    harness.wait_projection();
    harness.invoke(window, ids::OPEN_SESSION_MENU).unwrap();
    harness
        .wait_for("the session list to paint", |cx| list_painted(cx, window));
    harness.press(window, "down");
    let label = |cx: &mut App, session: SessionId| {
        a11y_node(cx, window, &format!("session-row-{}", session.get()))["aria"]
            ["label"]
            .clone()
    };
    // A rename elsewhere relabels the row and the corner switcher.
    harness
        .runtime
        .lock()
        .rename_session(first.session, Some("renamed"))
        .unwrap();
    harness
        .runtime
        .lock()
        .rename_session(harness.session, Some("main-2"))
        .unwrap();
    harness.wait_for("the rows to relabel", |cx| {
        label(cx, first.session) == "Session: renamed, 1 workspace · 1 tab"
            && a11y_node(cx, window, "session-switcher-corner")["aria"]["label"]
                == "Session: main-2"
    });
    // A session created elsewhere joins the list while it is open.
    let second = harness.other_session("second", 2);
    harness.session_row(window, second.session);
    // The focused row's session closing elsewhere hands focus to the
    // terminal rather than to a handle nothing draws.
    assert_eq!(
        harness.cx.update(|cx| focused_row(cx, window)),
        Some(first.session)
    );
    harness.runtime.lock().close_session(first.session).unwrap();
    harness.wait_for("the row to go and focus to return", |cx| {
        focused(cx, window, tab)
            && window
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>().is_ok_and(|view| {
                        !view
                            .read(cx)
                            .session_row_bounds
                            .borrow()
                            .contains_key(&first.session)
                    })
                })
                .unwrap_or(false)
    });
}

#[gpui::test]
fn a_rows_menu_acts_on_its_own_session(cx: &mut TestAppContext) {
    let mut harness = switcher_harness(cx);
    let (window, _) = harness.named_window("main");
    let other = harness.other_session("other", 1);
    harness.wait_projection();
    let switcher = harness.session_switcher(window);
    harness.click(window, switcher.center());
    harness
        .wait_for("the session list to paint", |cx| list_painted(cx, window));
    // The row's "⋯" button opens the menu a right press opens, for that
    // row's session; the list stays open behind it.
    let row = harness.session_row(window, other.session);
    let button = gpui::point(row.right() - px(16.0), row.center().y);
    harness.click(window, button);
    harness.wait_for("the row menu", |cx| {
        open_kind(cx, window) == Some("session_row")
    });
    harness.update_view(window, |view, _, _| {
        assert_eq!(view.session_list, Some(SessionListAnchor::Corner));
        assert!(matches!(
            view.menu.as_ref().map(|menu| menu.kind.clone()),
            Some(MenuKind::SessionRow(session)) if session == other.session
        ));
    });
    // Open in New Window opens the row's session, not the window's.
    let before = harness.cx.update(|cx| cx.windows().len());
    let pick = format!("session-window:{}", other.session.get());
    harness.update_view(window, |view, window, cx| {
        view.run_session_pick(&pick, window, cx);
    });
    harness.wait_for("a second window on the row's session", |cx| {
        cx.windows().len() == before + 1
            && cx.global::<Desktop>().windows.records().any(|record| {
                record.id != window.window_id()
                    && record.session == Some(other.session)
            })
    });
    assert_eq!(
        harness.cx.update(|cx| shown_session(cx, window)),
        Some(harness.session)
    );
}

#[gpui::test]
fn dragging_a_session_row_reorders_the_sessions(cx: &mut TestAppContext) {
    let mut harness = switcher_harness(cx);
    let (window, _) = harness.named_window("main");
    let first = harness.other_session("first", 1);
    let second = harness.other_session("second", 1);
    harness.wait_projection();
    let main = harness.session;
    assert_eq!(
        harness.cx.update(sessions),
        vec![main, first.session, second.session]
    );
    harness.invoke(window, ids::OPEN_SESSION_MENU).unwrap();
    harness
        .wait_for("the session list to paint", |cx| list_painted(cx, window));
    // Dragging the last row above the first moves its session first, and
    // the list stays open.
    let from = harness.session_row(window, second.session);
    let to = harness.session_row(window, main);
    let above = gpui::point(to.center().x, to.top() + px(2.0));
    harness.drag_row(window, from.center(), above);
    harness.update_view(window, |view, _, _| {
        let drag = view.session_drag.expect("a drag");
        assert!(drag.dragging);
        assert_eq!(drag.before, Some(main));
    });
    harness.release(window, above);
    harness.wait_for("the sessions to reorder", |cx| {
        sessions(cx) == vec![second.session, main, first.session]
    });
    assert!(harness.cx.update(|cx| list_painted(cx, window)));
    // A press that never moves shows its session instead.
    let row = harness.session_row(window, first.session);
    harness.click(window, row.center());
    harness.wait_switched(window, first.session, &first.tabs);
}

/// The title row Huterm draws on both platforms when a window takes
/// client decorations, with tabs in the title bar and one tab, so the
/// strip shows the title, the corner controls, and the menu button.
fn client_frame_window(
    harness: &mut DesktopHarness,
) -> (AnyWindowHandle, TabId) {
    let (window, tab) = harness.named_window("main");
    harness.update_view(window, |view, window, cx| {
        view.decorations_override = Some(gpui::Decorations::Client {
            tiling: gpui::Tiling::default(),
        });
        view.sync_frame(window);
        view.refresh_tab_visibility(window, cx);
        view.sync_tab_layout(window, cx);
        cx.notify();
    });
    (window, tab)
}

#[gpui::test]
fn the_title_row_keeps_its_gestures_and_drop_region_beside_the_switcher(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.position = crate::config::TabPosition::Titlebar;
        config.sidebar.position = huterm_config::SidebarPosition::Right;
        config.sidebar.session_switcher_in_corner = true;
    });
    let (window, _) = client_frame_window(&mut harness);
    harness.wait_for("the title row's window buttons", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx).window_button_bounds
                        [WindowButton::Minimize.index()]
                    .get()
                    .is_some()
                })
            })
            .unwrap_or(false)
    });
    let switcher = harness.session_switcher(window);
    let drop_region = harness.update_view(window, |view, window, _| {
        view.title_row_layout(&view.chrome_layout(window))
            .drop_region
    });
    // A pane dropped on the empty row never lands on the switcher.
    assert!(
        drop_region.right() <= switcher.left(),
        "{drop_region:?} {switcher:?}"
    );
    assert!(drop_region.size.width > px(0.0));
    let title_row = |harness: &mut DesktopHarness| {
        harness.update_view(window, |view, _, _| {
            (view.title_row_press.get(), view.title_row_moves.get())
        })
    };
    // A press on the switcher is the switcher's: the row sees nothing.
    press_at(&mut harness, window, switcher.center(), 1);
    assert_eq!(title_row(&mut harness), (false, 0));
    release_at(&mut harness, window, switcher.center());
    harness.wait_for("the session list to open", |cx| list_painted(cx, window));
    harness.press(window, "escape");
    // The empty strip left of it still starts a move and takes the
    // double-click.
    let empty = gpui::point(drop_region.left() + px(40.0), switcher.center().y);
    press_at(&mut harness, window, empty, 1);
    assert_eq!(title_row(&mut harness), (true, 0));
    release_at(&mut harness, window, empty);
    // A Linux double-click zooms the window, which GPUI's test window does
    // not implement.
    if cfg!(target_os = "macos") {
        press_at(&mut harness, window, empty, 2);
        assert_eq!(title_row(&mut harness), (false, 0), "the double-click");
        release_at(&mut harness, window, empty);
    }
}

#[gpui::test]
fn the_icon_style_shows_both_switchers_as_icons(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
        config.sidebar.session_switcher_in_corner = true;
        config.sidebar.switcher_style = huterm_config::SwitcherStyle::Icon;
    });
    let (window, _) = harness.named_window("a long session name");
    harness.add_workspace(1);
    let session = harness.session_switcher(window);
    let workspace = harness.switcher_bounds(window);
    assert_eq!(session.size.width, CONTROL_SIZE);
    assert_eq!(workspace.size.width, CONTROL_SIZE);
    // Each still names what it shows, and the session's name fills its
    // tooltip.
    harness.cx.update(|cx| {
        let aria = &a11y_node(cx, window, "session-switcher-corner")["aria"];
        assert_eq!(aria["label"], "Session: a long session name");
    });
}

#[gpui::test]
fn a_session_drag_cancels_when_its_session_goes(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let (window, _) = harness.named_window("main");
    let first = harness.other_session("first", 1);
    let second = harness.other_session("second", 1);
    harness.wait_projection();
    harness.invoke(window, ids::OPEN_SESSION_MENU).unwrap();
    harness
        .wait_for("the session list to paint", |cx| list_painted(cx, window));
    let from = harness.session_row(window, second.session);
    let to = harness.session_row(window, harness.session);
    let above = gpui::point(to.center().x, to.top() + px(2.0));
    harness.drag_row(window, from.center(), above);
    harness.update_view(window, |view, _, _| {
        assert!(view.session_drag.is_some_and(|drag| drag.dragging));
    });
    // The dragged session closes elsewhere: the drag ends and the release
    // moves nothing.
    harness
        .runtime
        .lock()
        .close_session(second.session)
        .unwrap();
    harness.wait_for("the drag to cancel", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>()
                    .is_ok_and(|view| view.read(cx).session_drag.is_none())
            })
            .unwrap_or(false)
    });
    harness.release(window, above);
    harness.wait_projection();
    assert_eq!(
        harness.cx.update(sessions),
        vec![harness.session, first.session]
    );
}

#[gpui::test]
fn a_session_list_whose_anchor_goes_closes_cleanly(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
    });
    let (window, tab) = harness.named_window("main");
    let row = harness.painted(window, "the sidebar's session row", |view| {
        view.session_sidebar_bounds
            .get()
            .filter(|_| view.session_sidebar_drawn.get())
    });
    harness.click(window, row.center());
    harness
        .wait_for("the session list to paint", |cx| list_painted(cx, window));
    harness.update_view(window, |view, _, _| {
        assert_eq!(view.session_list, Some(SessionListAnchor::Sidebar));
    });
    // Hiding the sidebar takes the row it opened from (W33): the list
    // closes, and focus returns to the terminal.
    harness.invoke(window, ids::TOGGLE_SIDEBAR).unwrap();
    harness.wait_for("the list to close", |cx| {
        !list_painted(cx, window) && focused(cx, window, tab)
    });
    harness.update_view(window, |view, _, _| {
        assert!(view.session_list.is_none());
    });
}
