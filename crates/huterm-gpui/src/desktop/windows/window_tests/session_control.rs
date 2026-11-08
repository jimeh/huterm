//! The title bar's session control and its menu, through real pointer and
//! keyboard events: the control's name and state, opening and closing the
//! menu, focus returning to the terminal, picks that switch, live updates
//! from other clients, the title row's gestures and drop region beside the
//! control, and the setting that hides it.

use gpui::{Modifiers, MouseDownEvent, MouseUpEvent};

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

/// Whether the open menu's content has painted.
fn painted(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>().is_ok_and(|view| {
                let view = view.read(cx);
                view.menu.is_some() && view.popover.content_bounds().is_some()
            })
        })
        .unwrap_or(false)
}

/// The open menu's selected id and its item labels by id.
fn menu_rows(
    cx: &mut App,
    window: AnyWindowHandle,
) -> (Option<String>, Vec<(String, String)>) {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().unwrap();
            let view = view.read(cx);
            let Some(menu) = view.menu.as_ref() else {
                return (None, Vec::new());
            };
            let menu = menu.view.read(cx);
            let rows = menu
                .model()
                .rows
                .iter()
                .filter_map(|row| match row {
                    MenuRow::Item(item) => {
                        Some((item.id.to_string(), item.label.clone()))
                    }
                    _ => None,
                })
                .collect();
            (menu.selected_id().map(|id| id.to_string()), rows)
        })
        .unwrap()
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

/// A harness whose one-tab windows draw the title-bar session control,
/// which is opt-in, on both platforms. Without client decorations a Linux
/// window has no title row, and a one-tab window hides its bar, which
/// would leave the control no host; an always-shown bar hosts it there,
/// while macOS keeps it in the title strip.
fn control_harness(cx: &mut TestAppContext) -> DesktopHarness {
    DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
        config.window.session_button = true;
    })
}

impl DesktopHarness {
    /// Waits for the session control to paint and returns its bounds.
    fn session_control(&mut self, window: AnyWindowHandle) -> Bounds<Pixels> {
        self.wait_for("the session control to paint", |cx| {
            window
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>().is_ok_and(|view| {
                        view.read(cx).session_button_bounds.get().is_some()
                    })
                })
                .unwrap_or(false)
        });
        self.update_view(window, |view, _, _| view.session_button_bounds.get())
            .unwrap()
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
fn the_control_names_its_session_and_toggles_its_menu_below_it(
    cx: &mut TestAppContext,
) {
    let mut harness = control_harness(cx);
    let (window, tab) = harness.named_window("main");
    let control = harness.session_control(window);
    harness.cx.update(|cx| {
        let aria = &a11y_node(cx, window, "session-control")["aria"];
        assert_eq!(aria["role"], "Button");
        assert_eq!(aria["label"], "Session: main");
        assert_eq!(aria["expanded"], false);
        // A name that fits shows no tooltip.
        assert!(aria["description"].is_null());
    });
    // It sits left of the menu button, which keeps the rightmost slot.
    let menu = harness
        .update_view(window, |view, _, _| view.menu_button_bounds.get())
        .expect("the menu button painted");
    assert!(control.right() <= menu.left(), "{control:?} {menu:?}");
    assert_eq!(control.size.height, CONTROL_SIZE);
    harness.click(window, control.center());
    harness.wait_for("the session menu to paint", |cx| painted(cx, window));
    harness.cx.update(|cx| {
        assert_eq!(open_kind(cx, window), Some("session"));
        assert_eq!(
            a11y_node(cx, window, "session-control")["aria"]["expanded"],
            true
        );
        assert_eq!(
            a11y_node(cx, window, "window-menu")["aria"]["expanded"],
            false,
            "the menu button stays closed"
        );
    });
    // A pointer opens it with no selection, below and right-aligned to the
    // control.
    let (selected, _) = harness.cx.update(|cx| menu_rows(cx, window));
    assert_eq!(selected, None);
    let rect = harness
        .update_view(window, |view, _, _| view.popover.content_bounds())
        .unwrap();
    assert!(rect.top() >= control.bottom(), "{rect:?} {control:?}");
    assert_eq!(rect.right(), control.right());
    // A second press closes it, and focus stays with the terminal.
    harness.click(window, control.center());
    harness.cx.update(|cx| {
        assert_eq!(open_kind(cx, window), None);
        assert!(focused(cx, window, tab), "the control never keeps focus");
    });
    harness.wait_for("a frame without the menu", |cx| !painted(cx, window));
    assert_eq!(harness.cx.update(|cx| open_kind(cx, window)), None);
}

#[gpui::test]
fn the_keyboard_walks_the_menu_and_escape_or_a_pick_returns_focus(
    cx: &mut TestAppContext,
) {
    let mut harness = control_harness(cx);
    let (window, tab) = harness.named_window("main");
    let other = harness.other_session("other", 1);
    harness.wait_projection();
    let main = format!("session:{}", harness.session.get());
    let other_row = format!("session:{}", other.session.get());
    harness.invoke(window, ids::OPEN_SESSION_MENU).unwrap();
    harness.wait_for("the session menu to paint", |cx| painted(cx, window));
    let selected = |harness: &mut DesktopHarness| {
        harness.cx.update(|cx| menu_rows(cx, window)).0
    };
    // `open_session_menu` selects the first row; arrows, Home, and End
    // move as in the window menu.
    assert_eq!(selected(&mut harness), Some(main.clone()));
    harness.press(window, "down");
    assert_eq!(selected(&mut harness), Some(other_row.clone()));
    harness.press(window, "end");
    assert_eq!(selected(&mut harness).as_deref(), Some("close_session"));
    harness.press(window, "home");
    assert_eq!(selected(&mut harness), Some(main.clone()));
    // A repeated open keeps the menu and its selection.
    harness.invoke(window, ids::OPEN_SESSION_MENU).unwrap();
    assert_eq!(selected(&mut harness), Some(main));
    harness.press(window, "escape");
    harness.cx.update(|cx| {
        assert_eq!(open_kind(cx, window), None);
        assert!(focused(cx, window, tab), "Escape returns focus");
    });
    // Enter on another session switches the window to it, and focus lands
    // on its terminal.
    harness.invoke(window, ids::OPEN_SESSION_MENU).unwrap();
    harness.wait_for("the session menu to paint", |cx| painted(cx, window));
    harness.press(window, "down enter");
    harness.wait_switched(window, other.session, &other.tabs);
    harness.wait_for("focus on the shown session's terminal", |cx| {
        open_kind(cx, window).is_none() && focused(cx, window, other.tabs[0])
    });
    // The control names the session the window now shows.
    harness.wait_for("the control to follow the switch", |cx| {
        a11y_node(cx, window, "session-control")["aria"]["label"]
            == "Session: other"
    });
}

#[gpui::test]
fn the_alternate_key_opens_the_selected_session_in_a_new_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, _) = harness.named_window("main");
    let other = harness.other_session("other", 1);
    harness.wait_projection();
    harness.invoke(window, ids::OPEN_SESSION_MENU).unwrap();
    harness.wait_for("the session menu to paint", |cx| painted(cx, window));
    harness.press(window, "down");
    let alternate = match crate::keymap::Platform::current() {
        crate::keymap::Platform::MacOs => "cmd-enter",
        crate::keymap::Platform::Linux => "ctrl-enter",
    };
    let before = harness.cx.update(|cx| cx.windows().len());
    harness.press(window, alternate);
    harness.wait_for("a second window on the session", |cx| {
        cx.windows().len() == before + 1
            && cx.global::<Desktop>().windows.records().any(|record| {
                record.id != window.window_id()
                    && record.session == Some(other.session)
            })
    });
    // This window still shows its own session.
    assert_eq!(
        harness.cx.update(|cx| shown_session(cx, window)),
        Some(harness.session)
    );
}

#[gpui::test]
fn the_open_menu_follows_other_clients_and_keeps_its_place(
    cx: &mut TestAppContext,
) {
    let mut harness = control_harness(cx);
    let (window, _) = harness.named_window("main");
    let first = harness.other_session("first", 1);
    let second = harness.other_session("second", 2);
    harness.wait_projection();
    harness.invoke(window, ids::OPEN_SESSION_MENU).unwrap();
    harness.wait_for("the session menu to paint", |cx| painted(cx, window));
    harness.press(window, "down");
    let first_row = format!("session:{}", first.session.get());
    let second_row = format!("session:{}", second.session.get());
    assert_eq!(
        harness.cx.update(|cx| menu_rows(cx, window)).0,
        Some(first_row.clone())
    );
    // A rename elsewhere relabels the row and the control.
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
        menu_rows(cx, window)
            .1
            .contains(&(first_row.clone(), "renamed".to_owned()))
            && a11y_node(cx, window, "session-control")["aria"]["label"]
                == "Session: main-2"
    });
    // The selected session closing elsewhere hands the selection to the
    // row that took its place.
    harness.runtime.lock().close_session(first.session).unwrap();
    harness.wait_for("the row to go and the selection to move", |cx| {
        let (selected, rows) = menu_rows(cx, window);
        !rows.iter().any(|(id, _)| *id == first_row)
            && selected.as_deref() == Some(second_row.as_str())
    });
    // A session created elsewhere joins the list while it is open.
    let third = harness.other_session("third", 1);
    let third_row = format!("session:{}", third.session.get());
    harness.wait_for("the new session's row", |cx| {
        menu_rows(cx, window)
            .1
            .iter()
            .any(|(id, _)| *id == third_row)
    });
}

#[gpui::test]
fn a_session_rows_detail_is_its_accessible_description(
    cx: &mut TestAppContext,
) {
    let mut harness = control_harness(cx);
    let (window, _) = harness.named_window("main");
    {
        let mut mux = harness.runtime.lock();
        let workspace = mux.create_workspace(harness.session, None).unwrap();
        mux.open_tab(workspace, &fixture(ECHO)).unwrap();
    }
    harness.wait_projection();
    harness.invoke(window, ids::OPEN_SESSION_MENU).unwrap();
    harness.wait_for("the session menu to paint", |cx| painted(cx, window));
    let row = harness.cx.update(|cx| {
        let json = window
            .update(cx, |_, window, _| window.debug_a11y_tree_json())
            .unwrap()
            .expect("test windows report an accessibility tree");
        let tree: serde_json::Value = serde_json::from_str(&json).unwrap();
        tree["nodes"]
            .as_object()
            .unwrap()
            .values()
            .map(|node| node["aria"].clone())
            .find(|aria| {
                aria["role"] == "MenuItemCheckBox" && aria["label"] == "main"
            })
            .expect("the session's row")
    });
    // The row shows the detail beside its name; assistive technology
    // reads it too.
    assert_eq!(row["description"], "2 workspaces · 2 tabs", "{row}");
}

/// The title row Huterm draws on both platforms when a window takes
/// client decorations, with tabs in the title bar and one tab, so the
/// strip shows the title, the control, and the menu button.
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
fn the_title_row_keeps_its_gestures_and_drop_region_beside_the_control(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.position = crate::config::TabPosition::Titlebar;
        config.window.session_button = true;
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
    let control = harness.session_control(window);
    let (drop_region, host) = harness.update_view(window, |view, window, _| {
        let layout = view.chrome_layout(window);
        (
            view.title_row_layout(&layout).drop_region,
            view.session_control_placement(&layout).host,
        )
    });
    assert_eq!(host, SessionControlHost::TitleStrip);
    // A pane dropped on the empty row never lands on the control.
    assert!(
        drop_region.right() <= control.left(),
        "{drop_region:?} {control:?}"
    );
    assert!(drop_region.size.width > px(0.0));
    let title_row = |harness: &mut DesktopHarness| {
        harness.update_view(window, |view, _, _| {
            (view.title_row_press.get(), view.title_row_moves.get())
        })
    };
    // A press on the control is the control's: the row sees nothing.
    press_at(&mut harness, window, control.center(), 1);
    assert_eq!(title_row(&mut harness), (false, 0));
    release_at(&mut harness, window, control.center());
    harness.wait_for("the session menu to open", |cx| painted(cx, window));
    harness.press(window, "escape");
    // The empty strip left of it still starts a move and takes the
    // double-click.
    let empty = gpui::point(drop_region.left() + px(40.0), control.center().y);
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
fn session_button_false_hides_the_control_but_keeps_its_menu(
    cx: &mut TestAppContext,
) {
    // The always-shown bar gives a Linux window a menu button to wait for,
    // as `control_harness` describes.
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
        config.window.session_button = false;
    });
    let (window, tab) = harness.named_window("main");
    harness.wait_for("a frame", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx).menu_button_bounds.get().is_some()
                })
            })
            .unwrap_or(false)
    });
    let (bounds, host) = harness.update_view(window, |view, window, _| {
        (
            view.session_button_bounds.get(),
            view.session_control_placement(&view.chrome_layout(window))
                .host,
        )
    });
    assert_eq!((bounds, host), (None, SessionControlHost::Hidden));
    // `open_session_menu` still opens it, at the window menu's anchor.
    harness.invoke(window, ids::OPEN_SESSION_MENU).unwrap();
    harness.wait_for("the session menu to paint", |cx| painted(cx, window));
    harness.press(window, "escape");
    harness.cx.update(|cx| assert!(focused(cx, window, tab)));
}

#[gpui::test]
fn a_click_on_the_control_at_the_end_of_a_tab_bar_opens_its_menu(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
        config.tabs.position = crate::config::TabPosition::Top;
        config.window.session_button = true;
    });
    let (window, _) = harness.named_window("main");
    // Fullscreen has no title row, so the control ends the top bar. The
    // macOS smoke opens this host's menu by command, because a real click
    // at the screen's top-right corner can land on the auto-hidden menu
    // bar or the notification corner instead.
    harness.update_view(window, |view, window, cx| {
        window.toggle_fullscreen();
        view.refresh_fullscreen(window, cx);
        cx.notify();
    });
    harness.wait_for("the control to end the tab bar", |cx| {
        window
            .update(cx, |root, window, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    let view = view.read(cx);
                    let layout = view.chrome_layout(window);
                    view.fullscreen.chrome_hidden
                        && view.session_control_placement(&layout).host
                            == SessionControlHost::BarEnd
                })
            })
            .unwrap_or(false)
    });
    harness.cx.run_until_parked();
    let control = harness.session_control(window);
    harness.click(window, control.center());
    harness.wait_for("the session menu to paint", |cx| painted(cx, window));
    assert_eq!(
        harness.cx.update(|cx| open_kind(cx, window)),
        Some("session")
    );
}
