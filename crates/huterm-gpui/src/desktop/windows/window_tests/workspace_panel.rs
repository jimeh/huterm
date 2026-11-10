//! Tier 2 tests for the sidebar and workspace switcher (#27 PR B): the
//! sidebar button and switcher in the corner, docking that keeps the
//! terminal's process, focus, and mouse mapping (W32), popovers whose
//! anchor goes (W33), rows, drag and drop (W41, W42), and the session
//! switcher through real pointer and keyboard events, a narrow window's
//! sheet, and per-window visibility.

use gpui::{
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
};
use huterm_config::{SessionSwitcherPosition, SidebarPosition, TabPosition};

use super::super::drag::{DropTarget, Gesture, RowKey};
use super::super::workspace_panel::{SwitcherAnchor, SwitcherMode};
use super::menus::a11y_node;
use super::workspaces::shown_workspace;
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

/// The open menu's item ids.
fn menu_ids(cx: &mut App, window: AnyWindowHandle) -> Vec<String> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().unwrap();
            let view = view.read(cx);
            let Some(menu) = view.menu.as_ref() else {
                return Vec::new();
            };
            menu.view
                .read(cx)
                .model()
                .rows
                .iter()
                .filter_map(|row| match row {
                    MenuRow::Item(item) => Some(item.id.to_string()),
                    _ => None,
                })
                .collect()
        })
        .unwrap()
}

/// The chrome `window` lays out now.
fn chrome(cx: &mut App, window: AnyWindowHandle) -> ChromeLayout {
    window
        .update(cx, |root, window, cx| {
            let view = root.downcast::<WorkspaceView>().unwrap();
            view.read(cx).chrome_layout(window)
        })
        .unwrap()
}

/// The terminal bounds `tab`'s view maps the mouse and its grid from.
fn mapped(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
) -> Option<Bounds<Pixels>> {
    terminal(cx, window, tab).and_then(|view| view.read(cx).geometry.bounds)
}

/// The session's workspaces in order.
fn order(cx: &mut App, session: SessionId) -> Vec<WorkspaceId> {
    cx.global::<Desktop>()
        .hierarchy
        .state()
        .session_workspaces(session)
        .unwrap_or_default()
        .to_vec()
}

/// A harness whose one-tab windows draw the sidebar button on both
/// platforms. Without client decorations a Linux window has no title row,
/// and a one-tab window hides its bar, which would leave the corner no
/// host; an always-shown bar hosts it there.
pub(super) fn corner_harness(cx: &mut TestAppContext) -> DesktopHarness {
    DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
    })
}

impl DesktopHarness {
    /// A focused window on one echo tab, waiting for its shell.
    fn workspace_window(&mut self) -> (AnyWindowHandle, TabId) {
        let tab = self.open_tab(ECHO);
        let window = self.attach_window();
        self.activate(window, tab);
        self.wait_text(window, tab, "READY");
        (window, tab)
    }

    /// Waits until `window` lays out chrome that `expected` accepts.
    pub(super) fn wait_chrome(
        &mut self,
        window: AnyWindowHandle,
        what: &str,
        expected: impl Fn(WorkspaceChrome) -> bool,
    ) {
        self.wait_for(what, |cx| expected(chrome(cx, window).workspace_chrome));
    }

    /// Reloads `window` with its configuration changed, as a configuration
    /// reload applies it.
    fn reload_config(
        &mut self,
        window: AnyWindowHandle,
        change: impl FnOnce(&mut Config),
    ) {
        self.update_view(window, |view, window, cx| {
            let mut config = view.config.clone();
            change(&mut config);
            let loaded = (config, view.family.clone(), view.metrics);
            view.apply_reload(Some(&loaded), &[], window, cx);
        });
    }

    /// The painted bounds of `workspace`'s row.
    pub(super) fn row_bounds(
        &mut self,
        window: AnyWindowHandle,
        workspace: WorkspaceId,
    ) -> Bounds<Pixels> {
        self.wait_for("the row to paint", |cx| {
            window
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>().is_ok_and(|view| {
                        view.read(cx)
                            .workspace_row_bounds
                            .borrow()
                            .contains_key(&workspace)
                    })
                })
                .unwrap_or(false)
        });
        self.update_view(window, |view, _, _| {
            view.workspace_row_bounds.borrow()[&workspace]
        })
    }

    /// The painted bounds `read` returns from the view, once it paints.
    pub(super) fn painted(
        &mut self,
        window: AnyWindowHandle,
        what: &str,
        read: impl Fn(&WorkspaceView) -> Option<Bounds<Pixels>>,
    ) -> Bounds<Pixels> {
        self.wait_for(what, |cx| {
            window
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>()
                        .is_ok_and(|view| read(view.read(cx)).is_some())
                })
                .unwrap_or(false)
        });
        self.update_view(window, |view, _, _| read(view).unwrap())
    }

    /// The workspace switcher's painted bounds.
    pub(super) fn switcher_bounds(
        &mut self,
        window: AnyWindowHandle,
    ) -> Bounds<Pixels> {
        self.painted(window, "the switcher to paint", |view| {
            view.workspace_trigger_bounds
                .get()
                .filter(|_| view.workspace_trigger_drawn.get())
        })
    }

    /// Presses `from`, moves past the drag threshold to `to`, and holds.
    pub(super) fn drag_row(
        &mut self,
        window: AnyWindowHandle,
        from: gpui::Point<Pixels>,
        to: gpui::Point<Pixels>,
    ) {
        let mut visual = VisualTestContext::from_window(window, &self.cx);
        visual.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: from,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        });
        visual.simulate_event(MouseMoveEvent {
            position: to,
            modifiers: Modifiers::none(),
            pressed_button: Some(MouseButton::Left),
        });
    }

    /// Releases a held drag at `at`.
    pub(super) fn release(
        &mut self,
        window: AnyWindowHandle,
        at: gpui::Point<Pixels>,
    ) {
        VisualTestContext::from_window(window, &self.cx).simulate_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position: at,
                modifiers: Modifiers::none(),
                click_count: 1,
            },
        );
    }
}

#[gpui::test]
fn the_sidebar_button_shows_alone_until_a_second_workspace(
    cx: &mut TestAppContext,
) {
    let mut harness = corner_harness(cx);
    let (window, _) = harness.workspace_window();
    harness.cx.update(|cx| {
        let layout = chrome(cx, window);
        assert_eq!(layout.workspace_chrome, WorkspaceChrome::NONE);
        assert_eq!(layout.workspace, None);
        let button = &a11y_node(cx, window, "sidebar-button")["aria"];
        assert_eq!(button["role"], "Button");
        assert_eq!(button["label"], "Show Sidebar");
    });
    harness.update_view(window, |view, _, _| {
        assert!(!view.workspace_trigger_drawn.get(), "no switcher yet");
    });
    let (second, _) = harness.add_workspace(1);
    harness.wait_chrome(window, "the switcher", WorkspaceChrome::switcher);
    harness.switcher_bounds(window);
    harness.cx.update(|cx| {
        let switcher = &a11y_node(cx, window, "workspace-switcher")["aria"];
        assert_eq!(switcher["label"], "Workspace: Workspace 1");
        assert_eq!(switcher["expanded"], false);
    });
    // Closing back to one workspace takes the switcher away again.
    harness.runtime.lock().close_workspace(second).unwrap();
    harness.wait_chrome(window, "no switcher", |chrome| !chrome.switcher());
}

#[gpui::test]
fn sidebar_changes_keep_focus_mouse_mapping_and_processes(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab) = harness.workspace_window();
    let view = harness
        .cx
        .update(|cx| terminal(cx, window, tab))
        .unwrap()
        .entity_id();
    let unchanged = |harness: &mut DesktopHarness, step: &str| {
        harness.wait_for("the view to take the chrome's terminal", |cx| {
            mapped(cx, window, tab) == Some(chrome(cx, window).terminal)
        });
        harness.cx.update(|cx| {
            assert_eq!(
                terminal(cx, window, tab).map(|view| view.entity_id()),
                Some(view),
                "{step}: the view was rebuilt"
            );
            assert!(focused(cx, window, tab), "{step}: focus moved");
            assert!(!exited(cx, window, tab), "{step}: the shell exited");
        });
        // The same shell answers keys after the change.
        harness.type_line(window, step);
        harness.wait_text(window, tab, &format!("got:{step}"));
    };
    let before = harness.cx.update(|cx| mapped(cx, window, tab)).unwrap();
    harness.invoke(window, ids::TOGGLE_SIDEBAR).unwrap();
    harness.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    unchanged(&mut harness, "shown");
    let docked = harness.cx.update(|cx| mapped(cx, window, tab)).unwrap();
    assert!(docked.left() > before.left(), "{docked:?} {before:?}");
    // Hiding gives the width back to the terminal.
    harness.invoke(window, ids::TOGGLE_SIDEBAR).unwrap();
    harness.wait_chrome(window, "no sidebar", |chrome| !chrome.docked());
    unchanged(&mut harness, "hidden");
    // A reload that moves a shown sidebar to the right keeps all of it.
    harness.invoke(window, ids::TOGGLE_SIDEBAR).unwrap();
    harness.reload_config(window, |config| {
        config.sidebar.position = SidebarPosition::Right;
    });
    harness.wait_chrome(window, "the right sidebar", |chrome| {
        chrome.docked_on(workspace_chrome::Edge::Right)
    });
    unchanged(&mut harness, "right");
    // A window too narrow to dock it undocks it, and widening docks it
    // again: the window kept wanting it.
    let height = harness
        .update_view(window, |_, window, _| window.viewport_size().height);
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_resize(gpui::size(px(500.0), height));
    harness.wait_chrome(window, "narrow", WorkspaceChrome::narrow);
    unchanged(&mut harness, "narrow");
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_resize(gpui::size(px(1000.0), height));
    harness.wait_chrome(window, "docked again", WorkspaceChrome::docked);
    unchanged(&mut harness, "wide");
    // A selection survives a change that leaves the grid alone, such as
    // moving the session switcher to the top (settled decision 9).
    let grid = harness.cx.update(|cx| {
        terminal(cx, window, tab).unwrap().read(cx).requested_grid
    });
    let selection = harness.cx.update(|cx| terminal(cx, window, tab)).unwrap();
    harness
        .cx
        .update(|cx| selection.update(cx, TerminalView::select_all));
    harness.reload_config(window, |config| {
        config.sidebar.session_switcher = SessionSwitcherPosition::Top;
    });
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        assert_eq!(
            terminal(cx, window, tab).unwrap().read(cx).requested_grid,
            grid
        );
        assert!(selected(cx, window, tab), "the selection went");
    });
}

#[gpui::test]
fn the_switcher_opens_a_popover_of_the_sidebars_content_under_native_menus(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.window.macos_native_menus = true;
        // A Linux window without client decorations has no title row, so
        // the bar hosts the corner.
        config.tabs.always_show = true;
    });
    let (window, _) = harness.workspace_window();
    let (second, seconds) = harness.add_workspace(1);
    let switcher = harness.switcher_bounds(window);
    harness.click_at(window, switcher.center());
    harness.wait_for("the switcher to report its popover", |cx| {
        a11y_node(cx, window, "workspace-switcher")["aria"]["expanded"] == true
    });
    let popover = harness.painted(window, "the popover to paint", |view| {
        view.switcher_popover.content_bounds()
    });
    harness.update_view(window, |view, _, _| {
        assert_eq!(
            view.switcher_mode,
            Some(SwitcherMode::Popover(SwitcherAnchor::Switcher))
        );
        assert!(view.native_menu.is_none(), "a GPUI popover, never native");
        assert!(popover.top() >= switcher.bottom(), "{popover:?}");
    });
    // It holds the rows, without the sidebar's session row, and each row
    // has a "⋯" button for its menu.
    let row = harness.row_bounds(window, second);
    assert!(popover.contains(&row.center()), "{row:?} {popover:?}");
    harness.update_view(window, |view, _, _| {
        assert!(!view.session_sidebar_drawn.get(), "no session row");
    });
    harness.cx.update(|cx| {
        let more = &a11y_node(
            cx,
            window,
            &format!("workspace-row-more-{}", second.get()),
        )["aria"];
        assert_eq!(more["label"], "Workspace actions: Workspace 2");
    });
    // The shown row takes focus; the keyboard picks the second workspace,
    // which closes the popover and returns focus to its terminal.
    harness.press(window, "down enter");
    harness.wait_workspace(window, second, &seconds);
    harness.cx.update(|cx| {
        assert!(
            focused(cx, window, seconds[0]),
            "focus stayed in the popover"
        );
    });
    harness.update_view(window, |view, _, _| {
        assert_eq!(view.switcher_mode, None);
    });
}

#[gpui::test]
fn a_row_menu_opened_before_the_more_tooltip_keeps_it_hidden(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.window.macos_native_menus = false;
        config.tabs.always_show = true;
    });
    let (window, _) = harness.workspace_window();
    let (second, _) = harness.add_workspace(1);
    harness.click_more_before_its_tooltip(window, second, |cx| {
        open_kind(cx, window).is_some()
    });
}

/// A row with a status dot shows its status text as a tooltip while the
/// pointer rests on it, and a right press before the tooltip's delay opens
/// the row's menu with no tooltip over it.
#[gpui::test]
fn a_row_menu_opened_before_the_status_tooltip_keeps_it_hidden(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.window.macos_native_menus = false;
        config.tabs.always_show = true;
        config.terminal.close_on_exit = false;
    });
    let (window, _) = harness.workspace_window();
    let (second, exits) = harness.add_workspace_running(EXITS, 1);
    // Without `close_on_exit`, its exit leaves it, exited, in place.
    let _viewer = harness.send_line(exits[0], "done");
    harness.wait_for("the row's status", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx).workspace_row_status(second, cx).is_some()
                })
            })
            .unwrap_or(false)
    });
    let switcher = harness.switcher_bounds(window);
    harness.click_at(window, switcher.center());
    let row = harness.row_bounds(window, second);
    let rest = point(row.left() + px(40.0), row.center().y);
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_mouse_move(rest, None, Modifiers::none());
    visual.executor().advance_clock(Duration::from_secs(2));
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("tooltip").is_some(),
        "the status tooltip"
    );
    let away = point(row.left() + px(40.0), row.top() - px(30.0));
    visual.simulate_mouse_move(away, None, Modifiers::none());
    visual.run_until_parked();
    visual.simulate_mouse_move(rest, None, Modifiers::none());
    visual.simulate_event(MouseDownEvent {
        button: MouseButton::Right,
        position: rest,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    harness.wait_for("the row menu", |cx| open_kind(cx, window).is_some());
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.executor().advance_clock(Duration::from_secs(2));
    visual.run_until_parked();
    assert!(visual.debug_bounds("tooltip").is_none(), "over the menu");
}

impl DesktopHarness {
    /// Opens the workspace switcher's popover and checks `workspace`'s row
    /// "⋯" button: resting on it shows its tooltip, and a click before the
    /// tooltip's delay opens the row menu, which `menu_open` reports, with
    /// no tooltip over it while the pointer stays.
    pub(super) fn click_more_before_its_tooltip(
        &mut self,
        window: AnyWindowHandle,
        workspace: WorkspaceId,
        menu_open: impl Fn(&mut App) -> bool,
    ) {
        let switcher = self.switcher_bounds(window);
        self.click_at(window, switcher.center());
        let row = self.row_bounds(window, workspace);
        let id: &'static str =
            format!("workspace-row-more-{}", workspace.get()).leak();
        let mut visual = VisualTestContext::from_window(window, &self.cx);
        let more = visual.debug_bounds(id).expect("the row's ⋯ button");
        let hover = |visual: &mut VisualTestContext, at| {
            visual.simulate_mouse_move(at, None, Modifiers::none());
        };
        hover(&mut visual, more.center());
        visual.executor().advance_clock(Duration::from_secs(2));
        visual.run_until_parked();
        assert!(
            visual.debug_bounds("tooltip").is_some(),
            "the hover tooltip"
        );
        hover(&mut visual, point(row.left() + px(8.0), row.center().y));
        visual.run_until_parked();
        assert!(visual.debug_bounds("tooltip").is_none(), "leaving hides it");
        hover(&mut visual, more.center());
        visual.simulate_click(more.center(), Modifiers::none());
        self.wait_for("the row menu", menu_open);
        let mut visual = VisualTestContext::from_window(window, &self.cx);
        visual.executor().advance_clock(Duration::from_secs(2));
        visual.run_until_parked();
        assert!(visual.debug_bounds("tooltip").is_none(), "over the menu");
        self.cx.update(|cx| {
            assert_eq!(a11y_node(cx, window, id)["aria"]["expanded"], true);
        });
    }
}

#[gpui::test]
fn rows_show_rename_and_open_a_menu_for_their_own_workspace(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
    });
    let (window, _) = harness.workspace_window();
    let (second, seconds) = harness.add_workspace(1);
    harness.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    let row = harness.row_bounds(window, second);
    harness.cx.update(|cx| {
        let id = format!("workspace-row-{}", second.get());
        let node = a11y_node(cx, window, &id);
        assert_eq!(node["aria"]["role"], "Button");
        assert_eq!(node["aria"]["label"], "Workspace: Workspace 2, 1 tab");
        assert_eq!(node["aria"]["selected"], false, "{node}");
        let shown = format!("workspace-row-{}", harness.workspace.get());
        assert_eq!(a11y_node(cx, window, &shown)["aria"]["selected"], true);
    });
    // A click shows it.
    harness.click_at(window, row.center());
    harness.wait_workspace(window, second, &seconds);
    // A right press opens its menu, acting on that row's workspace.
    let first = harness.row_bounds(window, harness.workspace);
    VisualTestContext::from_window(window, &harness.cx).simulate_event(
        MouseDownEvent {
            button: MouseButton::Right,
            position: first.center(),
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        },
    );
    harness.cx.update(|cx| {
        assert_eq!(open_kind(cx, window), Some("workspace_row"));
        let raw = harness.workspace.get();
        assert_eq!(
            menu_ids(cx, window),
            [
                format!("workspace-rename:{raw}"),
                format!("workspace-pin:{raw}"),
                format!("workspace-window:{raw}"),
                format!("workspace-close:{raw}"),
            ]
        );
    });
    harness.press(window, "escape");
    assert_eq!(harness.cx.update(|cx| open_kind(cx, window)), None);
    // A double click renames through the palette prompt.
    harness.click_at(window, row.center());
    VisualTestContext::from_window(window, &harness.cx).simulate_event(
        MouseDownEvent {
            button: MouseButton::Left,
            position: row.center(),
            modifiers: Modifiers::none(),
            click_count: 2,
            first_mouse: false,
        },
    );
    harness.release(window, row.center());
    harness.wait_for("the rename prompt", |cx| {
        palette_state(cx, window).is_some_and(|state| {
            state.contains("slots command=rename_workspace")
        })
    });
}

#[gpui::test]
fn dragging_a_row_reorders_the_sessions_workspaces_in_every_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
    });
    let first = harness.open_tab(ECHO);
    let (second, _) = harness.add_workspace(1);
    let (third, _) = harness.add_workspace(1);
    let one = harness.attach_showing(&[first]);
    let two = harness.attach_showing(&[first]);
    harness.wait_chrome(one, "the sidebar", WorkspaceChrome::docked);
    let session = harness.session;
    let initial = harness.workspace;
    let start = harness.cx.update(|cx| order(cx, session));
    assert_eq!(start, [initial, second, third]);
    // Escape cancels a drag; nothing moves and nothing is shown.
    let from = harness.row_bounds(one, harness.workspace);
    let to = harness.row_bounds(one, third);
    let below = gpui::point(to.center().x, to.bottom() - px(2.0));
    harness.drag_row(one, from.center(), below);
    harness.update_view(one, |view, _, _| {
        let drag = view.drag.as_ref().expect("a drag");
        assert!(drag.dragging());
        assert_eq!(
            drag.target(),
            Some(DropTarget::RowSlot(None)),
            "a drop past the last row ends it"
        );
    });
    harness.press(one, "escape");
    harness.release(one, below);
    harness.cx.run_until_parked();
    assert_eq!(harness.cx.update(|cx| order(cx, session)), start);
    // A drop past the last row moves the first workspace to the end, in
    // the session, so every window on it shows the new order.
    harness.drag_row(one, from.center(), below);
    harness.release(one, below);
    harness.wait_for("the new order", |cx| {
        order(cx, session) == [second, third, initial]
    });
    harness.cx.update(|cx| {
        assert_eq!(shown_workspace(cx, one), Some(harness.workspace));
    });
    let rows = |harness: &mut DesktopHarness, window| {
        let mut rows: Vec<(Pixels, WorkspaceId)> =
            [harness.workspace, second, third]
                .into_iter()
                .map(|workspace| {
                    (harness.row_bounds(window, workspace).top(), workspace)
                })
                .collect();
        rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        rows.into_iter()
            .map(|(_, workspace)| workspace)
            .collect::<Vec<_>>()
    };
    harness.wait_chrome(two, "the other sidebar", WorkspaceChrome::docked);
    harness.cx.run_until_parked();
    for window in [one, two] {
        assert_eq!(
            rows(&mut harness, window),
            [second, third, harness.workspace]
        );
    }
}

#[gpui::test]
fn escape_waits_for_a_row_press_to_drag_and_a_menu_ends_the_drag(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
    });
    let first = harness.open_tab(ECHO);
    let (second, _) = harness.add_workspace(1);
    let window = harness.attach_showing(&[first]);
    harness.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    let session = harness.session;
    let start = harness.cx.update(|cx| order(cx, session));
    let from = harness.row_bounds(window, harness.workspace).center();
    let to = harness.row_bounds(window, second);
    let below = gpui::point(to.center().x, to.bottom() - px(2.0));
    // Before the threshold, Escape leaves the press alone.
    VisualTestContext::from_window(window, &harness.cx).simulate_event(
        MouseDownEvent {
            button: MouseButton::Left,
            position: from,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        },
    );
    harness.press(window, "escape");
    harness.update_view(window, |view, _, _| {
        let drag = view.drag.as_ref().expect("the press survives Escape");
        assert!(!drag.dragging());
    });
    // Once it drags, an opening menu ends it, and the release moves
    // nothing.
    VisualTestContext::from_window(window, &harness.cx).simulate_event(
        MouseMoveEvent {
            position: below,
            modifiers: Modifiers::none(),
            pressed_button: Some(MouseButton::Left),
        },
    );
    harness.update_view(window, |view, window, cx| {
        assert!(view.drag.as_ref().is_some_and(Gesture::dragging));
        view.open_tab_menu(first, below, window, cx);
        assert!(view.drag.is_none(), "the menu ends the row drag");
    });
    harness.release(window, below);
    harness.cx.run_until_parked();
    assert_eq!(harness.cx.update(|cx| order(cx, session)), start);
}

#[gpui::test]
fn a_workspace_drag_cancels_when_its_workspace_goes(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
    });
    let (window, _) = harness.workspace_window();
    let (second, _) = harness.add_workspace(1);
    let (third, _) = harness.add_workspace(1);
    harness.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    let from = harness.row_bounds(window, second);
    let to = harness.row_bounds(window, third);
    let below = gpui::point(to.center().x, to.bottom() - px(2.0));
    harness.drag_row(window, from.center(), below);
    // Another client closes the dragged workspace (W41).
    harness.runtime.lock().close_workspace(second).unwrap();
    harness.wait_for("the drag to end", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>()
                    .is_ok_and(|view| view.read(cx).drag.is_none())
            })
            .unwrap_or(false)
    });
    harness.release(window, below);
    harness.cx.run_until_parked();
    let session = harness.session;
    assert_eq!(
        harness.cx.update(|cx| order(cx, session)),
        [harness.workspace, third]
    );
    harness.update_view(window, |view, _, _| {
        assert!(view.notices.is_empty(), "the release reported a failure");
    });
}

#[gpui::test]
fn a_workspace_drop_whose_anchor_went_lands_by_id(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
    });
    let (window, _) = harness.workspace_window();
    let (second, _) = harness.add_workspace(1);
    let (third, _) = harness.add_workspace(1);
    let (fourth, _) = harness.add_workspace(1);
    harness.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    let first = harness.workspace;
    let from = harness.row_bounds(window, first);
    let anchor = harness.row_bounds(window, third);
    let above = gpui::point(anchor.center().x, anchor.top() + px(2.0));
    harness.drag_row(window, from.center(), above);
    harness.update_view(window, |view, _, _| {
        assert_eq!(
            view.drag.as_ref().unwrap().target(),
            Some(DropTarget::RowSlot(Some(RowKey::Workspace(third))))
        );
    });
    // The anchor closes before the drop (W42); the rows below it move up,
    // and the drop lands before the row now under the pointer, by ID.
    harness.runtime.lock().close_workspace(third).unwrap();
    harness.wait_for("the anchor's row to go", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    !view
                        .read(cx)
                        .workspace_row_bounds
                        .borrow()
                        .contains_key(&third)
                })
            })
            .unwrap_or(false)
    });
    harness.row_bounds(window, fourth);
    harness.release(window, above);
    let session = harness.session;
    harness.wait_for("the drop", |cx| {
        order(cx, session) == [second, first, fourth]
    });
}

#[gpui::test]
fn the_keyboard_walks_the_sidebar_and_returns_to_the_terminal(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
    });
    let (window, tab) = harness.workspace_window();
    let (second, seconds) = harness.add_workspace(1);
    harness.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    harness.row_bounds(window, second);
    let focused_row = |harness: &mut DesktopHarness| {
        harness.update_view(window, |view, window, _| {
            view.focused_workspace_row(window)
        })
    };
    harness.invoke(window, ids::FOCUS_SIDEBAR).unwrap();
    assert_eq!(focused_row(&mut harness), Some(harness.workspace));
    harness.press(window, "down");
    assert_eq!(focused_row(&mut harness), Some(second));
    // The walk reaches the session switcher after the rows and wraps;
    // Home and End jump to its ends.
    harness.press(window, "down");
    assert_eq!(focused_row(&mut harness), None);
    harness.press(window, "down");
    assert_eq!(focused_row(&mut harness), Some(harness.workspace));
    harness.press(window, "end up");
    assert_eq!(focused_row(&mut harness), Some(second));
    harness.press(window, "home");
    assert_eq!(focused_row(&mut harness), Some(harness.workspace));
    harness.press(window, "down enter");
    harness.wait_workspace(window, second, &seconds);
    // Shift-F10 opens the focused row's menu; Escape there and in the
    // sidebar returns focus to the terminal.
    harness.invoke(window, ids::FOCUS_SIDEBAR).unwrap();
    harness.press(window, "shift-f10");
    assert_eq!(
        harness.cx.update(|cx| open_kind(cx, window)),
        Some("workspace_row")
    );
    harness.press(window, "escape");
    harness.invoke(window, ids::FOCUS_SIDEBAR).unwrap();
    harness.press(window, "escape");
    harness.cx.update(|cx| {
        assert!(focused(cx, window, seconds[0]), "focus left the sidebar");
        assert!(!focused(cx, window, tab));
    });
    // A hidden sidebar has nothing to focus.
    harness.invoke(window, ids::TOGGLE_SIDEBAR).unwrap();
    harness.wait_chrome(window, "no sidebar", |chrome| !chrome.docked());
    assert!(matches!(
        harness.invoke(window, ids::FOCUS_SIDEBAR),
        Err(CommandError::Unavailable(_))
    ));
}

#[gpui::test]
fn a_popover_whose_anchor_goes_closes_cleanly(cx: &mut TestAppContext) {
    let mut harness = corner_harness(cx);
    let (window, tab) = harness.workspace_window();
    let (second, _) = harness.add_workspace(1);
    harness.switcher_bounds(window);
    let switcher_open = |harness: &mut DesktopHarness| {
        harness.update_view(window, |view, _, _| view.switcher_mode.is_some())
    };
    // A row menu opened from the popover's "⋯" button leaves it open.
    harness
        .invoke(window, ids::OPEN_WORKSPACE_SWITCHER)
        .unwrap();
    let row = harness.row_bounds(window, second);
    harness
        .click_at(window, gpui::point(row.right() - px(16.0), row.center().y));
    assert_eq!(
        harness.cx.update(|cx| open_kind(cx, window)),
        Some("workspace_row")
    );
    assert!(switcher_open(&mut harness), "the menu closed its popover");
    // The switcher goes with the last other workspace (W33): the popover
    // and the menu opened from it close, and focus returns.
    harness.runtime.lock().close_workspace(second).unwrap();
    harness.wait_for("the popover to close", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>()
                    .is_ok_and(|view| view.read(cx).switcher_mode.is_none())
            })
            .unwrap_or(false)
    });
    harness.cx.update(|cx| {
        assert_eq!(open_kind(cx, window), None);
        assert!(focused(cx, window, tab));
    });
    // Showing the sidebar while the popover is open closes it too.
    harness.add_workspace(1);
    harness.switcher_bounds(window);
    harness
        .invoke(window, ids::OPEN_WORKSPACE_SWITCHER)
        .unwrap();
    assert!(switcher_open(&mut harness));
    harness.invoke(window, ids::TOGGLE_SIDEBAR).unwrap();
    harness.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    assert!(!switcher_open(&mut harness));
    harness.cx.update(|cx| assert!(focused(cx, window, tab)));
}

#[gpui::test]
fn the_session_row_opens_the_session_list_at_either_end(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
        config.window.macos_native_menus = true;
    });
    let (window, _) = harness.workspace_window();
    harness.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    let read = |view: &WorkspaceView| view.session_sidebar_bounds.get();
    let bottom = harness.painted(window, "the session row", read);
    let band = harness
        .cx
        .update(|cx| chrome(cx, window).workspace.unwrap());
    assert!(bottom.bottom() <= band.bottom() && bottom.top() > band.center().y);
    harness.cx.update(|cx| {
        let aria = &a11y_node(cx, window, "session-switcher")["aria"];
        assert_eq!(aria["role"], "Button");
        // Its caption counts the session's workspaces and tabs.
        assert!(
            aria["label"]
                .as_str()
                .unwrap()
                .ends_with(", 1 workspace · 1 tab"),
            "{aria}"
        );
    });
    harness.click_at(window, bottom.center());
    let list = harness.painted(window, "the session list", |view| {
        view.session_list_popover.content_bounds()
    });
    harness.update_view(window, |view, _, _| {
        assert!(view.native_menu.is_none(), "Huterm's popover, never native");
        assert_eq!(
            view.session_list,
            Some(super::super::session_control::SessionListAnchor::Sidebar)
        );
    });
    // From the bottom row it opens upward, against the row.
    assert!(list.bottom() <= bottom.top(), "{list:?} {bottom:?}");
    harness.cx.update(|cx| {
        assert_eq!(
            a11y_node(cx, window, "session-switcher")["aria"]["expanded"],
            true
        );
    });
    // Escape closes it and returns focus to the row that opened it.
    harness.press(window, "escape");
    harness.update_view(window, |view, window, _| {
        assert!(view.session_list.is_none());
        assert!(view.session_row_focus.is_focused(window));
    });
    // The setting moves it to the top, above the workspace rows.
    harness.reload_config(window, |config| {
        config.sidebar.session_switcher = SessionSwitcherPosition::Top;
    });
    harness.wait_for("the row to move", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx)
                        .session_sidebar_bounds
                        .get()
                        .is_some_and(|row| row.top() < band.center().y)
                })
            })
            .unwrap_or(false)
    });
    let top = harness.painted(window, "the session row", read);
    let first = harness.row_bounds(window, harness.workspace);
    assert!(top.bottom() <= first.top(), "{top:?} {first:?}");
}

#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one sweep of every side, tab position, and corner setting"
)]
fn every_side_draws_with_every_tab_position(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
    });
    let (window, tab) = harness.workspace_window();
    harness.add_workspace(1);
    for position in [SidebarPosition::Left, SidebarPosition::Right] {
        for tabs in [
            TabPosition::Titlebar,
            TabPosition::Top,
            TabPosition::Bottom,
            TabPosition::Left,
            TabPosition::Right,
        ] {
            for session_button in [false, true] {
                // Where the sidebar button painted while hidden.
                let mut hidden_button = None;
                for visible in [false, true] {
                    harness.reload_config(window, |config| {
                        config.sidebar.position = position;
                        config.tabs.position = tabs;
                        config.sidebar.session_switcher_in_corner =
                            session_button;
                    });
                    harness.update_view(window, |view, window, cx| {
                        if view.workspace_visible != visible {
                            view.toggle_sidebar(window, cx);
                        }
                    });
                    let label = format!(
                        "{position:?} {tabs:?} {session_button} {visible}"
                    );
                    harness.wait_for("the frame to draw", |cx| {
                        mapped(cx, window, tab)
                            == Some(chrome(cx, window).terminal)
                    });
                    harness.cx.run_until_parked();
                    // Every control is drawn once: a duplicate would also
                    // panic in the accessibility tree.
                    harness.cx.update(|cx| {
                        let menu = a11y_node(cx, window, "window-menu");
                        assert_eq!(menu["aria"]["role"], "Button", "{label}");
                        let button = a11y_node(cx, window, "sidebar-button");
                        assert_eq!(
                            button["aria"]["label"],
                            if visible {
                                "Hide Sidebar"
                            } else {
                                "Show Sidebar"
                            },
                            "{label}"
                        );
                        let layout = chrome(cx, window);
                        if let Some(band) = layout.workspace {
                            assert!(
                                !band.intersects(&layout.terminal),
                                "{label}: {band:?} {:?}",
                                layout.terminal
                            );
                        }
                        assert_eq!(layout.workspace.is_some(), visible);
                    });
                    harness.update_view(window, |view, window, _| {
                        let button = view.sidebar_button_bounds.get().unwrap();
                        let layout = view.chrome_layout(window);
                        assert!(
                            !button.intersects(&layout.terminal),
                            "{label}: {button:?}"
                        );
                        // Showing the sidebar leaves its button where it
                        // was in a title row, the docked sidebar's strip
                        // standing in for the row's corner.
                        if !visible {
                            hidden_button = Some(button);
                        } else if view.title_row() > px(0.0) {
                            assert_eq!(
                                Some(button),
                                hidden_button,
                                "{label}: the sidebar button moved"
                            );
                        }
                        assert_eq!(
                            view.workspace_trigger_drawn.get(),
                            !visible,
                            "{label}: the switcher shows while hidden"
                        );
                        if let Some(switcher) =
                            view.workspace_trigger_bounds.get()
                        {
                            assert!(
                                !switcher.intersects(&button),
                                "{label}: {switcher:?} {button:?}"
                            );
                        }
                        // The session switcher draws where Settings ask,
                        // and every corner control fits the room its host
                        // reserved: none overlaps another, the menu
                        // button, the first tab, or the terminal.
                        let session = view
                            .session_corner_bounds
                            .get()
                            .filter(|_| view.session_corner_drawn.get());
                        assert_eq!(
                            session.is_some(),
                            session_button && !visible,
                            "{label}: the session switcher"
                        );
                        let workspace = view
                            .workspace_trigger_bounds
                            .get()
                            .filter(|_| view.workspace_trigger_drawn.get());
                        let strip = view.tab_strip(window);
                        let first_tab = if strip.vertical {
                            Bounds::new(
                                strip.bounds.origin
                                    + point(px(0.0), strip.start(0) - strip.offset),
                                size(strip.bounds.size.width, strip.tab_extent(0)),
                            )
                        } else {
                            Bounds::new(
                                strip.bounds.origin
                                    + point(strip.start(0) - strip.offset, px(0.0)),
                                size(strip.tab_extent(0), strip.bounds.size.height),
                            )
                        };
                        let controls: Vec<Bounds<Pixels>> =
                            [Some(button), session, workspace]
                                .into_iter()
                                .flatten()
                                .collect();
                        let menu = view.menu_button_bounds.get();
                        for (index, control) in controls.iter().enumerate() {
                            for other in &controls[index + 1..] {
                                assert!(
                                    !control.intersects(other),
                                    "{label}: {control:?} {other:?}"
                                );
                            }
                            for (what, other) in [
                                ("the menu button", menu),
                                ("the first tab", Some(first_tab)),
                                ("the terminal", Some(layout.terminal)),
                            ] {
                                if let Some(other) = other {
                                    assert!(
                                        !control.intersects(&other),
                                        "{label}: {control:?} overlaps {what} {other:?}"
                                    );
                                }
                            }
                        }
                    });
                }
            }
        }
    }
}

#[gpui::test]
fn a_narrow_window_opens_the_sidebar_as_a_sheet(cx: &mut TestAppContext) {
    let mut harness = corner_harness(cx);
    let (window, tab) = harness.workspace_window();
    let (second, seconds) = harness.add_workspace(1);
    let height = harness
        .update_view(window, |_, window, _| window.viewport_size().height);
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_resize(gpui::size(px(500.0), height));
    harness.cx.run_until_parked();
    let before = harness.cx.update(|cx| mapped(cx, window, tab)).unwrap();
    // The button opens the sheet, which leaves the saved state alone.
    let button = harness.painted(window, "the sidebar button", |view| {
        view.sidebar_button_bounds.get()
    });
    harness.click_at(window, button.center());
    harness.update_view(window, |view, _, _| {
        assert_eq!(view.switcher_mode, Some(SwitcherMode::Sheet));
        assert!(!view.workspace_visible, "a peek is not a saved choice");
    });
    // The sheet covers the terminal rather than resizing it.
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        assert_eq!(mapped(cx, window, tab), Some(before));
        let aria = &a11y_node(cx, window, "sidebar-button")["aria"];
        assert_eq!(aria["label"], "Hide Sidebar");
    });
    // A second click on the button closes it.
    harness.click_at(window, button.center());
    harness.cx.run_until_parked();
    harness.update_view(window, |view, _, _| {
        assert_eq!(view.switcher_mode, None, "the button closes the sheet");
    });
    harness.click_at(window, button.center());
    let row = harness.row_bounds(window, second);
    harness.click_at(window, row.center());
    harness.wait_workspace(window, second, &seconds);
    harness.update_view(window, |view, _, _| {
        assert_eq!(view.switcher_mode, None, "showing closes the sheet");
    });
    // Widening keeps the sidebar hidden: only the button docks it.
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_resize(gpui::size(px(1000.0), height));
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        assert!(!chrome(cx, window).workspace_chrome.docked());
    });
}

#[gpui::test]
fn widening_a_window_with_its_sheet_open_docks_one_sidebar(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
    });
    let (window, _) = harness.workspace_window();
    let (second, _) = harness.add_workspace(1);
    let height = harness
        .update_view(window, |_, window, _| window.viewport_size().height);
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_resize(gpui::size(px(500.0), height));
    harness.wait_chrome(window, "narrow", WorkspaceChrome::narrow);
    // The window still wants its sidebar; the button peeks with a sheet,
    // whose shown row takes focus.
    harness.invoke(window, ids::TOGGLE_SIDEBAR).unwrap();
    harness.row_bounds(window, second);
    harness.update_view(window, |view, window, _| {
        assert_eq!(view.switcher_mode, Some(SwitcherMode::Sheet));
        assert!(view.focused_workspace_row(window).is_some());
    });
    // A row's menu opened in the sheet holds focus.
    harness.press(window, "shift-f10");
    assert_eq!(
        harness.cx.update(|cx| open_kind(cx, window)),
        Some("workspace_row")
    );
    // Widening docks it: the sheet closes before the docked sidebar draws,
    // so no row or focus handle draws twice in a frame, and the menu goes
    // with it, handing focus back to the terminal.
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_resize(gpui::size(px(1000.0), height));
    harness.wait_chrome(window, "docked", WorkspaceChrome::docked);
    harness.cx.run_until_parked();
    harness.update_view(window, |view, window, cx| {
        assert_eq!(view.switcher_mode, None);
        assert!(view.workspace_visible);
        assert!(view.menu.is_none(), "the sheet's menu stayed");
        let terminal = view.active_view(cx).unwrap();
        assert!(terminal.read(cx).focus.is_focused(window), "focus went");
    });
}

#[gpui::test]
fn each_opener_toggles_only_its_own_popover(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
        config.tabs.position = TabPosition::Titlebar;
    });
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab]);
    let (empty, _) = harness.add_workspace(0);
    harness.activate_window(window);
    harness.show_workspace(window, empty, &[]);
    // An empty workspace has no tab bar to host the corner, so a Linux
    // window takes client decorations for a title row, which it draws with
    // tabs in the title bar, as macOS has one.
    harness.update_view(window, |view, window, cx| {
        view.decorations_override = Some(gpui::Decorations::Client {
            tiling: gpui::Tiling::default(),
        });
        view.sync_frame(window);
        view.refresh_tab_visibility(window, cx);
        view.sync_tab_layout(window, cx);
        cx.notify();
    });
    let switcher = harness.switcher_bounds(window);
    let switch = harness.painted(window, "Switch Workspace…", |view| {
        view.empty_state_switch_bounds()
    });
    let mode = |harness: &mut DesktopHarness| {
        harness.update_view(window, |view, _, _| view.switcher_mode)
    };
    // The empty state's button opens the popover at itself and closes it
    // on a second click; the switcher does not show it open meanwhile.
    harness.click_at(window, switch.center());
    assert_eq!(
        mode(&mut harness),
        Some(SwitcherMode::Popover(SwitcherAnchor::EmptyState))
    );
    harness.cx.update(|cx| {
        let aria = &a11y_node(cx, window, "workspace-switcher")["aria"];
        assert_eq!(aria["expanded"], false);
    });
    harness.click_at(window, switch.center());
    harness.cx.run_until_parked();
    assert_eq!(mode(&mut harness), None, "the button reopened its popover");
    // A click on the switcher while the empty state's popover is open
    // moves it to the switcher in one press.
    harness.click_at(window, switch.center());
    harness.click_at(window, switcher.center());
    harness.cx.run_until_parked();
    assert_eq!(
        mode(&mut harness),
        Some(SwitcherMode::Popover(SwitcherAnchor::Switcher))
    );
    harness.click_at(window, switcher.center());
    harness.cx.run_until_parked();
    assert_eq!(mode(&mut harness), None, "the switcher reopened it");
}

#[gpui::test]
fn the_switcher_closes_for_commands_the_palette_reload_and_deactivation(
    cx: &mut TestAppContext,
) {
    let mut harness = corner_harness(cx);
    let (window, _) = harness.workspace_window();
    harness.add_workspace(1);
    harness.switcher_bounds(window);
    let open = |harness: &mut DesktopHarness| {
        harness
            .invoke(window, ids::OPEN_WORKSPACE_SWITCHER)
            .unwrap();
        assert!(
            harness.update_view(window, |view, _, _| {
                view.switcher_mode.is_some()
            }),
            "the switcher opened"
        );
    };
    let closed = |harness: &mut DesktopHarness, step: &str| {
        harness.cx.run_until_parked();
        assert!(
            harness.update_view(window, |view, _, _| {
                view.switcher_mode.is_none()
            }),
            "{step}: the switcher stayed open"
        );
    };
    // Another command closes it, as it closes an open menu (W33).
    open(&mut harness);
    harness.invoke(window, ids::NEW_TAB).unwrap();
    closed(&mut harness, "a command");
    // So does the palette, which keeps focus.
    open(&mut harness);
    harness.invoke(window, ids::OPEN_COMMAND_PALETTE).unwrap();
    closed(&mut harness, "the palette");
    harness.cx.update(|cx| {
        assert!(palette_state(cx, window).is_some(), "the palette closed");
    });
    harness.press(window, "escape");
    // A reload closes it and returns focus to the terminal.
    open(&mut harness);
    harness.reload_config(window, |_| {});
    closed(&mut harness, "a reload");
    harness.update_view(window, |view, window, cx| {
        assert!(view.focused_workspace_row(window).is_none());
        let terminal = view.active_view(cx).unwrap();
        assert!(terminal.read(cx).focus.is_focused(window), "focus went");
    });
    // Deactivation closes it, as it closes menus.
    open(&mut harness);
    harness.deactivate(window);
    closed(&mut harness, "deactivation");
}

#[gpui::test]
fn right_corner_controls_sit_just_inside_the_menu_button(
    cx: &mut TestAppContext,
) {
    for (tabs, width, session_button) in [
        (TabPosition::Titlebar, 1000.0, false),
        (TabPosition::Titlebar, 480.0, false),
        (TabPosition::Top, 1000.0, false),
        (TabPosition::Right, 1000.0, false),
        (TabPosition::Right, 1000.0, true),
    ] {
        let mut harness = DesktopHarness::with_config(cx, |config| {
            config.sidebar.position = SidebarPosition::Right;
            config.tabs.position = tabs;
            config.tabs.always_show = true;
            config.sidebar.session_switcher_in_corner = session_button;
        });
        let (window, _) = harness.workspace_window();
        let height = harness
            .update_view(window, |_, window, _| window.viewport_size().height);
        VisualTestContext::from_window(window, &harness.cx)
            .simulate_resize(gpui::size(px(width), height));
        harness.add_workspace(1);
        let switcher = harness.switcher_bounds(window);
        if session_button {
            harness
                .runtime
                .lock()
                .rename_session(
                    harness.session,
                    Some("a session with a long name"),
                )
                .unwrap();
        }
        let label = format!("{tabs:?} {width} {session_button}");
        harness.cx.run_until_parked();
        harness.update_view(window, |view, window, _| {
            let button = view.sidebar_button_bounds.get().unwrap();
            let menu = view.menu_button_bounds.get().unwrap();
            let layout = view.chrome_layout(window);
            // The button sits just inside the menu button, with no gap
            // left where a wider switcher was reserved.
            assert!(button.right() <= menu.left(), "{label}");
            assert!(
                menu.left() - button.right() <= px(12.0),
                "{label}: {button:?} {menu:?}"
            );
            if tabs == TabPosition::Right {
                // A right column's strip holds the button; the switchers
                // take rows above its tabs, inside the column, the session
                // switcher first.
                assert!(layout.tabs.contains(&switcher.center()), "{label}");
                assert!(button.left() >= layout.tabs.left(), "{label}");
                if let Some(session) = view
                    .session_corner_bounds
                    .get()
                    .filter(|_| view.session_corner_drawn.get())
                {
                    assert!(
                        layout.tabs.contains(&session.center())
                            && session.bottom() <= switcher.top(),
                        "{label}: {session:?} {switcher:?} {:?}",
                        layout.tabs
                    );
                }
            } else {
                assert!(switcher.right() <= button.left(), "{label}");
                assert!(
                    button.left() - switcher.right() <= px(8.0),
                    "{label}: {switcher:?} {button:?}"
                );
            }
            if width < 500.0 {
                assert_eq!(
                    switcher.size.width, CONTROL_SIZE,
                    "{label}: a narrow window shows the switcher's icon"
                );
            }
        });
    }
}

#[gpui::test]
fn the_keyboard_reaches_the_session_switcher(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
    });
    let (window, _) = harness.workspace_window();
    let (second, _) = harness.add_workspace(1);
    harness.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    harness.row_bounds(window, second);
    harness.invoke(window, ids::FOCUS_SIDEBAR).unwrap();
    // Tab walks the rows and then the session switcher at the bottom.
    harness.press(window, "tab tab");
    harness.cx.run_until_parked();
    harness.update_view(window, |view, window, _| {
        assert!(view.session_row_focus.is_focused(window));
    });
    harness.cx.update(|cx| {
        let node = a11y_node(cx, window, "session-switcher");
        assert_eq!(node["aria"]["role"], "Button");
    });
    // Return opens the session list with the session's row focused;
    // Escape returns to the session switcher, and Shift-Tab to the rows.
    harness.press(window, "enter");
    let session = harness.session;
    harness.update_view(window, |view, window, _| {
        assert!(view.session_list.is_some());
        assert_eq!(view.focused_session_row(window), Some(session));
    });
    harness.press(window, "escape");
    harness.update_view(window, |view, window, _| {
        assert!(view.session_list.is_none());
        assert!(view.session_row_focus.is_focused(window));
    });
    harness.press(window, "shift-tab");
    assert_eq!(
        harness.update_view(window, |view, window, _| {
            view.focused_workspace_row(window)
        }),
        Some(second)
    );
}

#[gpui::test]
fn resizing_the_sidebar_keeps_the_terminal_wide_enough_to_dock(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
    });
    let (window, _) = harness.workspace_window();
    let height = harness
        .update_view(window, |_, window, _| window.viewport_size().height);
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_resize(gpui::size(px(700.0), height));
    harness.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    // A drag towards 400 points would leave the terminal under 384.
    harness.update_view(window, |view, window, cx| {
        view.resize_workspace_panel(
            gpui::point(px(400.0), px(10.0)),
            window,
            cx,
        );
    });
    harness.cx.run_until_parked();
    let wide_enough = |harness: &mut DesktopHarness, step: &str| {
        harness.cx.run_until_parked();
        harness.cx.update(|cx| {
            let layout = chrome(cx, window);
            assert!(layout.workspace_chrome.docked(), "{step}: undocked");
            assert!(
                layout.terminal.size.width
                    >= workspace_chrome::MIN_TERMINAL_WIDTH,
                "{step}: {:?}",
                layout.terminal
            );
        });
    };
    wide_enough(&mut harness, "the sidebar");
    // So does widening a tab column beside it, in a window wide enough for
    // both to start with.
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_resize(gpui::size(px(1000.0), height));
    harness.reload_config(window, |config| {
        config.tabs.position = TabPosition::Left;
        config.tabs.always_show = true;
    });
    harness.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    let column = harness.cx.update(|cx| chrome(cx, window).tabs);
    harness.update_view(window, |view, window, cx| {
        view.resize_sidebar(
            gpui::point(column.left() + px(400.0), px(10.0)),
            window,
            cx,
        );
    });
    wide_enough(&mut harness, "the tab column");
}

#[gpui::test]
fn a_row_held_at_the_lists_edge_keeps_scrolling(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
    });
    let (window, _) = harness.workspace_window();
    for _ in 0..40 {
        harness.add_workspace(1);
    }
    harness.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    let first = harness.row_bounds(window, harness.workspace);
    let list = harness
        .update_view(window, |view, _, _| view.workspace_scroll.bounds());
    // Hold the dragged row at the list's bottom edge, then stop moving.
    let edge = gpui::point(first.center().x, list.bottom() - px(4.0));
    harness.drag_row(window, first.center(), edge);
    harness.wait_for("the list to keep scrolling without moves", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx).workspace_scroll.offset().y < px(-60.0)
                })
            })
            .unwrap_or(false)
    });
    harness.press(window, "escape");
    harness.release(window, edge);
}

#[gpui::test]
fn the_visible_state_is_per_window_and_saved(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[first]);
    let two = harness.attach_showing(&[first]);
    harness.invoke(one, ids::TOGGLE_SIDEBAR).unwrap();
    harness.wait_chrome(one, "the sidebar", WorkspaceChrome::docked);
    harness.cx.update(|cx| {
        assert!(!chrome(cx, two).workspace_chrome.docked());
        let saved = |window: AnyWindowHandle| {
            cx.global::<Desktop>()
                .windows
                .record(window.window_id())
                .unwrap()
                .layout
                .workspace_visible
        };
        assert!(saved(one));
        assert!(!saved(two));
    });
}

#[gpui::test]
fn counts_follow_other_workspaces(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
    });
    let (window, _) = harness.workspace_window();
    let (second, _) = harness.add_workspace(1);
    harness.row_bounds(window, second);
    let id = format!("workspace-row-{}", second.get());
    harness
        .runtime
        .lock()
        .open_tab(second, &fixture(ECHO))
        .unwrap();
    harness.wait_for("the count to follow", |cx| {
        a11y_node(cx, window, &id)["aria"]["label"]
            == "Workspace: Workspace 2, 2 tabs"
    });
}

#[gpui::test]
fn closing_the_focused_rows_workspace_returns_focus_to_the_terminal(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
    });
    let (window, tab) = harness.workspace_window();
    let (second, _) = harness.add_workspace(1);
    harness.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    harness.row_bounds(window, second);
    harness.invoke(window, ids::FOCUS_SIDEBAR).unwrap();
    harness.press(window, "down");
    harness.press(window, "shift-f10");
    assert_eq!(
        harness.cx.update(|cx| open_kind(cx, window)),
        Some("workspace_row")
    );
    // Close Workspace… is the row menu's last item; the idle workspace
    // closes without asking.
    harness.press(window, "end enter");
    harness.wait_for("the workspace to close", |cx| {
        let state = cx.global::<Desktop>().hierarchy.state();
        state.workspace(second).is_none()
    });
    harness
        .wait_for("focus back on the terminal", |cx| focused(cx, window, tab));
    harness.type_line(window, "after");
    harness.wait_text(window, tab, "got:after");
}

#[gpui::test]
fn a_focused_row_whose_workspace_goes_hands_focus_to_the_terminal(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
    });
    let (window, tab) = harness.workspace_window();
    let (second, _) = harness.add_workspace(1);
    let (third, _) = harness.add_workspace(1);
    harness.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    harness.row_bounds(window, third);
    harness.invoke(window, ids::FOCUS_SIDEBAR).unwrap();
    harness.press(window, "down");
    assert_eq!(
        harness.update_view(window, |view, window, _| {
            view.focused_workspace_row(window)
        }),
        Some(second)
    );
    // Another client closes it, as a native menu's pick does after it
    // handed focus back to the row.
    harness.runtime.lock().close_workspace(second).unwrap();
    harness
        .wait_for("focus back on the terminal", |cx| focused(cx, window, tab));
    harness.type_line(window, "after");
    harness.wait_text(window, tab, "got:after");
}

#[gpui::test]
fn showing_a_workspace_scrolls_its_row_into_view(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.sidebar.visible = true;
    });
    let (window, _) = harness.workspace_window();
    let mut last = None;
    for _ in 0..40 {
        last = Some(harness.add_workspace(1));
    }
    let (last, tabs) = last.unwrap();
    harness.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
    let band = harness
        .cx
        .update(|cx| chrome(cx, window).workspace.unwrap());
    harness.show_workspace(window, last, &tabs);
    harness.wait_for("the shown row inside the sidebar", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx)
                        .workspace_row_bounds
                        .borrow()
                        .get(&last)
                        .is_some_and(|row| {
                            row.top() >= band.top()
                                && row.bottom() <= band.bottom()
                        })
                })
            })
            .unwrap_or(false)
    });
}

#[gpui::test]
fn a_tab_column_beside_the_sidebar_resizes_from_its_own_edge(
    cx: &mut TestAppContext,
) {
    for (position, tabs) in [
        (SidebarPosition::Left, TabPosition::Left),
        (SidebarPosition::Right, TabPosition::Right),
    ] {
        let mut harness = DesktopHarness::with_config(cx, |config| {
            config.sidebar.visible = true;
            config.sidebar.position = position;
            config.tabs.position = tabs;
            config.tabs.always_show = true;
        });
        let (window, _) = harness.workspace_window();
        harness.wait_chrome(window, "the sidebar", WorkspaceChrome::docked);
        let column = harness.cx.update(|cx| chrome(cx, window).tabs);
        // The pointer drags the column's inner edge to 220 points from its
        // outer edge, which the sidebar moved in from the window's.
        let pointer = if tabs == TabPosition::Left {
            gpui::point(column.left() + px(220.0), px(10.0))
        } else {
            gpui::point(column.right() - px(220.0), px(10.0))
        };
        harness.update_view(window, |view, window, cx| {
            view.resize_sidebar(pointer, window, cx);
        });
        harness.cx.update(|cx| {
            let layout = chrome(cx, window);
            assert_eq!(layout.tabs.size.width, px(220.0), "{tabs:?}");
            let band = layout.workspace.unwrap();
            assert!(!band.intersects(&layout.tabs), "{tabs:?}");
        });
    }
}

/// The sidebar button follows the traffic lights a window measured in every
/// host that puts it beside them, and a merged row's tabs move with it. Only
/// macOS has the title strip and its traffic lights.
#[cfg(target_os = "macos")]
#[gpui::test]
fn the_sidebar_button_follows_the_measured_traffic_lights(
    cx: &mut TestAppContext,
) {
    let mut harness = corner_harness(cx);
    let (window, tab) = harness.workspace_window();
    harness.add_workspace(1);
    // About where macOS 26's buttons put it: 16 points before the fallback.
    let lights = px(68.0);
    for tabs in [TabPosition::Titlebar, TabPosition::Top, TabPosition::Left] {
        for visible in [false, true] {
            let mut fallback_strip = None;
            for measured in [None, Some(lights)] {
                harness.reload_config(window, |config| {
                    config.tabs.position = tabs;
                });
                harness.update_view(window, |view, window, cx| {
                    view.traffic_lights = measured;
                    if view.workspace_visible != visible {
                        view.toggle_sidebar(window, cx);
                    }
                    cx.notify();
                });
                harness.wait_for("the frame to draw", |cx| {
                    mapped(cx, window, tab) == Some(chrome(cx, window).terminal)
                });
                harness.cx.run_until_parked();
                let label = format!("{tabs:?} {visible} {measured:?}");
                harness.update_view(window, |view, window, _| {
                    let button = view.sidebar_button_bounds.get().unwrap();
                    assert_eq!(
                        button.left(),
                        measured.unwrap_or(TRAFFIC_LIGHT_INSET),
                        "{label}"
                    );
                    let strip = view
                        .chrome_layout(window)
                        .strip_bounds(view.layout_tabs())
                        .left();
                    match fallback_strip {
                        None => fallback_strip = Some(strip),
                        // Beside a hidden sidebar the merged row starts
                        // after the button, so it moves with the lights.
                        Some(fallback) => assert_eq!(
                            fallback - strip,
                            if tabs == TabPosition::Titlebar && !visible {
                                TRAFFIC_LIGHT_INSET - lights
                            } else {
                                px(0.0)
                            },
                            "{label}"
                        ),
                    }
                });
            }
        }
    }
}
