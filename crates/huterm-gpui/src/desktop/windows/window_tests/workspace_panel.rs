//! Tier 2 tests for the workspace panel, bar, and dropdown (#27 PR B):
//! when each shows, panel changes that keep the terminal's process, focus,
//! and mouse mapping (W32), menus whose host goes (W33), rows and the
//! workspace menu through real pointer and keyboard events, live counts,
//! per-window collapse, the session control's sidebar host, and
//! `show = "never"`.

use gpui::{Modifiers, MouseButton, MouseDownEvent};
use huterm_config::{SessionButtonPlacement, WorkspacePosition, WorkspaceShow};

use super::menus::a11y_node;
use super::sessions::shown_tabs;
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

impl DesktopHarness {
    /// A focused window on one echo tab, waiting for its shell.
    fn workspace_window(&mut self) -> (AnyWindowHandle, TabId) {
        let tab = self.open_tab(ECHO);
        let window = self.attach_window();
        self.activate(window, tab);
        self.wait_text(window, tab, "READY");
        (window, tab)
    }

    /// Waits until `window` lays out `expected` chrome, by its kind.
    fn wait_chrome(
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

    /// The painted bounds of `workspace`'s row or chip.
    fn row_bounds(
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
}

#[gpui::test]
fn the_panel_shows_only_with_a_second_workspace_under_auto(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, _) = harness.workspace_window();
    // One workspace: the window is exactly as before the panel existed.
    harness.cx.update(|cx| {
        let layout = chrome(cx, window);
        assert_eq!(layout.workspace_chrome, WorkspaceChrome::None);
        assert_eq!(layout.workspace, None);
    });
    let (second, _) = harness.add_workspace(1);
    harness.wait_chrome(window, "the left panel", |chrome| {
        matches!(chrome, WorkspaceChrome::Panel { .. })
    });
    harness.cx.update(|cx| {
        let layout = chrome(cx, window);
        let band = layout.workspace.unwrap();
        assert_eq!(band.left(), layout.content.left());
        assert_eq!(band.size.width, workspace_chrome::PANEL_WIDTH);
        assert!(layout.terminal.left() >= band.right());
        // Rows name their workspace and tab count, the shown one selected.
        let row = a11y_node(cx, window, "workspace-panel");
        assert_eq!(row["aria"]["label"], "Workspaces");
    });
    // Closing back to one workspace hides it again.
    harness.runtime.lock().close_workspace(second).unwrap();
    harness.wait_chrome(window, "no panel", |chrome| {
        chrome == WorkspaceChrome::None
    });
}

#[gpui::test]
fn panel_changes_keep_focus_mouse_mapping_and_processes(
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
    harness.add_workspace(1);
    harness.wait_chrome(window, "the panel", |chrome| {
        matches!(chrome, WorkspaceChrome::Panel { .. })
    });
    unchanged(&mut harness, "shown");
    let panel = harness.cx.update(|cx| mapped(cx, window, tab)).unwrap();
    assert!(panel.left() > before.left(), "{panel:?} {before:?}");
    // Collapsing gives the width back to the terminal.
    harness.invoke(window, ids::TOGGLE_WORKSPACE_PANEL).unwrap();
    harness.wait_chrome(window, "the dropdown", WorkspaceChrome::is_dropdown);
    unchanged(&mut harness, "collapsed");
    harness.invoke(window, ids::TOGGLE_WORKSPACE_PANEL).unwrap();
    harness.wait_chrome(window, "the panel again", |chrome| {
        matches!(chrome, WorkspaceChrome::Panel { .. })
    });
    unchanged(&mut harness, "expanded");
    // A reload that moves it to every edge keeps all of it too.
    for position in [
        WorkspacePosition::Top,
        WorkspacePosition::Right,
        WorkspacePosition::Bottom,
        WorkspacePosition::Left,
    ] {
        harness.reload_config(window, |config| {
            config.workspaces.position = position;
        });
        let expected = match position {
            WorkspacePosition::Left => workspace_chrome::Edge::Left,
            WorkspacePosition::Top => workspace_chrome::Edge::Top,
            WorkspacePosition::Right => workspace_chrome::Edge::Right,
            WorkspacePosition::Bottom => workspace_chrome::Edge::Bottom,
        };
        harness.wait_chrome(window, "the moved chrome", |chrome| {
            chrome.edge() == Some(expected)
        });
        unchanged(&mut harness, &format!("{position:?}").to_lowercase());
    }
}

#[gpui::test]
fn the_dropdown_opens_the_workspace_menu_which_switches_and_shows_the_panel(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.workspaces.collapsed = true;
        config.tabs.always_show = true;
    });
    let (window, _) = harness.workspace_window();
    let (second, seconds) = harness.add_workspace(1);
    harness.wait_chrome(window, "the dropdown", WorkspaceChrome::is_dropdown);
    let trigger = {
        harness.wait_for("the dropdown to paint", |cx| {
            window
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>().is_ok_and(|view| {
                        let view = view.read(cx);
                        view.workspace_trigger_drawn.get()
                            && view.workspace_trigger_bounds.get().is_some()
                    })
                })
                .unwrap_or(false)
        });
        harness.update_view(window, |view, _, _| {
            view.workspace_trigger_bounds.get().unwrap()
        })
    };
    harness.cx.update(|cx| {
        let aria = &a11y_node(cx, window, "workspace-dropdown")["aria"];
        assert_eq!(aria["role"], "Button");
        assert!(
            aria["label"].as_str().unwrap().starts_with("Workspace: "),
            "{aria}"
        );
        assert_eq!(aria["expanded"], false);
    });
    harness.click_at(window, trigger.center());
    harness.cx.update(|cx| {
        assert_eq!(open_kind(cx, window), Some("workspace"));
        assert_eq!(
            menu_ids(cx, window),
            [
                format!("workspace:{}", harness.workspace.get()),
                format!("workspace:{}", second.get()),
                "new_workspace".to_owned(),
                "rename_workspace".to_owned(),
                "close_workspace".to_owned(),
                "toggle_workspace_panel".to_owned(),
            ]
        );
    });
    // The keyboard picks the second workspace.
    harness.press(window, "down down enter");
    harness.wait_workspace(window, second, &seconds);
    // Show Workspace Panel from the menu expands this window only.
    harness.invoke(window, ids::OPEN_WORKSPACE_MENU).unwrap();
    harness.press(window, "end enter");
    harness.wait_chrome(window, "the panel", |chrome| {
        matches!(chrome, WorkspaceChrome::Panel { .. })
    });
    harness.cx.update(|cx| {
        let record = cx.global::<Desktop>().windows.record(window.window_id());
        assert!(!record.unwrap().layout.workspace_collapsed);
    });
}

#[gpui::test]
fn rows_show_rename_and_open_a_menu_for_their_own_workspace(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, _) = harness.workspace_window();
    let (second, seconds) = harness.add_workspace(1);
    harness.wait_chrome(window, "the panel", |chrome| {
        matches!(chrome, WorkspaceChrome::Panel { .. })
    });
    let row = harness.row_bounds(window, second);
    harness.cx.update(|cx| {
        let id = format!("workspace-row-{}", second.get());
        let node = a11y_node(cx, window, &id);
        assert_eq!(node["aria"]["role"], "Button");
        assert_eq!(node["aria"]["label"], "Workspace: Workspace 2, 1 tab");
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
                "new_workspace".to_owned(),
                format!("workspace-close:{raw}"),
            ]
        );
    });
    harness.press(window, "escape");
    assert_eq!(harness.cx.update(|cx| open_kind(cx, window)), None);
    // A double click renames through the palette prompt.
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_click(row.center(), Modifiers::none());
    VisualTestContext::from_window(window, &harness.cx).simulate_event(
        MouseDownEvent {
            button: MouseButton::Left,
            position: row.center(),
            modifiers: Modifiers::none(),
            click_count: 2,
            first_mouse: false,
        },
    );
    VisualTestContext::from_window(window, &harness.cx).simulate_event(
        gpui::MouseUpEvent {
            button: MouseButton::Left,
            position: row.center(),
            modifiers: Modifiers::none(),
            click_count: 2,
        },
    );
    harness.wait_for("the rename prompt", |cx| {
        palette_state(cx, window).is_some_and(|state| {
            state.contains("slots command=rename_workspace")
        })
    });
}

#[gpui::test]
fn the_keyboard_walks_the_panel_and_returns_to_the_terminal(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab) = harness.workspace_window();
    let (second, seconds) = harness.add_workspace(1);
    harness.wait_chrome(window, "the panel", |chrome| {
        matches!(chrome, WorkspaceChrome::Panel { .. })
    });
    let focused_row = |harness: &mut DesktopHarness| {
        harness.update_view(window, |view, window, _| {
            view.focused_workspace_row(window)
        })
    };
    harness.invoke(window, ids::FOCUS_WORKSPACE_PANEL).unwrap();
    assert_eq!(focused_row(&mut harness), Some(harness.workspace));
    harness.press(window, "down");
    assert_eq!(focused_row(&mut harness), Some(second));
    // Down wraps; Home and End jump.
    harness.press(window, "down");
    assert_eq!(focused_row(&mut harness), Some(harness.workspace));
    harness.press(window, "end");
    assert_eq!(focused_row(&mut harness), Some(second));
    harness.press(window, "enter");
    harness.wait_workspace(window, second, &seconds);
    // Shift-F10 opens the focused row's menu; Escape there and in the
    // panel returns focus to the terminal.
    harness.invoke(window, ids::FOCUS_WORKSPACE_PANEL).unwrap();
    harness.press(window, "shift-f10");
    assert_eq!(
        harness.cx.update(|cx| open_kind(cx, window)),
        Some("workspace_row")
    );
    harness.press(window, "escape");
    harness.invoke(window, ids::FOCUS_WORKSPACE_PANEL).unwrap();
    harness.press(window, "escape");
    harness.cx.update(|cx| {
        assert!(focused(cx, window, seconds[0]), "focus left the panel");
        assert!(!focused(cx, window, tab));
    });
}

#[gpui::test]
fn a_menu_whose_host_goes_closes_cleanly(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.workspaces.collapsed = true;
        config.tabs.always_show = true;
        config.window.session_button_placement = SessionButtonPlacement::Both;
    });
    let (window, tab) = harness.workspace_window();
    harness.add_workspace(1);
    harness.wait_chrome(window, "the dropdown", WorkspaceChrome::is_dropdown);
    // A reload closes an open workspace menu and returns focus.
    harness.invoke(window, ids::OPEN_WORKSPACE_MENU).unwrap();
    assert_eq!(
        harness.cx.update(|cx| open_kind(cx, window)),
        Some("workspace")
    );
    harness.reload_config(window, |config| {
        config.workspaces.show = WorkspaceShow::Never;
    });
    harness.wait_chrome(window, "no workspace chrome", |chrome| {
        chrome == WorkspaceChrome::None
    });
    harness.cx.update(|cx| {
        assert_eq!(open_kind(cx, window), None);
        assert!(focused(cx, window, tab));
    });
    // A session menu opened from the panel's host re-anchors to the title
    // bar's when collapsing takes the panel away.
    harness.reload_config(window, |config| {
        config.workspaces.show = WorkspaceShow::Auto;
    });
    harness.invoke(window, ids::TOGGLE_WORKSPACE_PANEL).unwrap();
    harness.wait_chrome(window, "the panel", |chrome| {
        matches!(chrome, WorkspaceChrome::Panel { .. })
    });
    harness.wait_for("the sidebar host to paint", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx).session_sidebar_bounds.get().is_some()
                })
            })
            .unwrap_or(false)
    });
    let sidebar = harness
        .update_view(window, |view, _, _| view.session_sidebar_bounds.get())
        .unwrap();
    harness.click_at(window, sidebar.center());
    assert_eq!(
        harness.cx.update(|cx| open_kind(cx, window)),
        Some("session")
    );
    harness.update_view(window, |view, _, _| {
        assert_eq!(view.session_menu_anchor_for_tests(), Some(sidebar));
    });
    harness.invoke(window, ids::TOGGLE_WORKSPACE_PANEL).unwrap();
    harness.wait_chrome(window, "the dropdown", WorkspaceChrome::is_dropdown);
    harness.wait_for("the sidebar host to go", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx).session_sidebar_bounds.get().is_none()
                })
            })
            .unwrap_or(false)
    });
    harness.update_view(window, |view, _, _| {
        let title = view.session_button_bounds.get();
        assert!(title.is_some(), "the title bar host shows (F1)");
        assert_eq!(view.session_menu_anchor_for_tests(), title);
    });
    harness.press(window, "escape");
    harness.cx.update(|cx| {
        assert_eq!(open_kind(cx, window), None);
        assert!(focused(cx, window, tab));
    });
}

#[gpui::test]
fn counts_and_badges_follow_other_workspaces_and_windows(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.workspaces.collapsed = true;
    });
    let first = harness.open_tab(ECHO);
    let (second, seconds) = harness.add_workspace(1);
    let one = harness.attach_showing(&[first]);
    let two = harness.attach_showing(&[first]);
    harness.wait_chrome(two, "the dropdown", WorkspaceChrome::is_dropdown);
    // Window two's open menu shows the second workspace's count live, and
    // a badge once window one shows it.
    harness.invoke(two, ids::OPEN_WORKSPACE_MENU).unwrap();
    let detail = |harness: &mut DesktopHarness| {
        harness.update_view(two, |view, _, cx| {
            let menu = view.menu.as_ref().unwrap().view.read(cx);
            menu.model()
                .rows
                .iter()
                .find_map(|row| match row {
                    MenuRow::Item(item)
                        if item.id.as_ref()
                            == format!("workspace:{}", second.get()) =>
                    {
                        Some((item.detail.clone(), item.tags.len()))
                    }
                    _ => None,
                })
                .unwrap()
        })
    };
    assert_eq!(detail(&mut harness), (Some("1 tab".to_owned()), 0));
    harness
        .runtime
        .lock()
        .open_tab(second, &fixture(ECHO))
        .unwrap();
    harness.wait_for("the count to follow", |cx| {
        window_detail(cx, two, second) == Some("2 tabs".to_owned())
    });
    assert_eq!(seconds.len(), 1);
    harness.update_view(one, |view, window, cx| {
        view.invoke_interactive(
            &CommandInvocation::new(
                ids::SELECT_WORKSPACE,
                vec![CommandArgument::new(
                    "workspace",
                    CommandValue::Workspace(second),
                )],
            ),
            window,
            cx,
        )
        .unwrap();
    });
    harness.wait_for("window one to show the second workspace", |cx| {
        shown_workspace(cx, one) == Some(second)
            && shown_tabs(cx, one).len() == 2
    });
    harness.wait_for("the badge in window two", |cx| {
        window_tags(cx, two, second) == 1
    });
}

/// The detail of `workspace`'s row in `window`'s open menu.
fn window_detail(
    cx: &mut App,
    window: AnyWindowHandle,
    workspace: WorkspaceId,
) -> Option<String> {
    menu_item(cx, window, workspace).and_then(|(detail, _)| detail)
}

/// How many tags `workspace`'s row in `window`'s open menu carries.
fn window_tags(
    cx: &mut App,
    window: AnyWindowHandle,
    workspace: WorkspaceId,
) -> usize {
    menu_item(cx, window, workspace).map_or(0, |(_, tags)| tags)
}

fn menu_item(
    cx: &mut App,
    window: AnyWindowHandle,
    workspace: WorkspaceId,
) -> Option<(Option<String>, usize)> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            let view = view.read(cx);
            let menu = view.menu.as_ref()?.view.read(cx);
            let id = format!("workspace:{}", workspace.get());
            menu.model().rows.iter().find_map(|row| match row {
                MenuRow::Item(item) if item.id.as_ref() == id => {
                    Some((item.detail.clone(), item.tags.len()))
                }
                _ => None,
            })
        })
        .ok()
        .flatten()
}

#[gpui::test]
fn never_hides_the_workspace_ui_but_keeps_the_commands(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.workspaces.show = WorkspaceShow::Never;
    });
    let (window, _) = harness.workspace_window();
    let (second, seconds) = harness.add_workspace(1);
    harness.wait_projection();
    harness.cx.update(|cx| {
        assert_eq!(chrome(cx, window).workspace_chrome, WorkspaceChrome::None);
    });
    for command in [ids::TOGGLE_WORKSPACE_PANEL, ids::FOCUS_WORKSPACE_PANEL] {
        let result = harness.invoke(window, command);
        assert!(
            matches!(result, Err(CommandError::Unavailable(_))),
            "{command:?}: {result:?}"
        );
    }
    // The session menu drops New Workspace; the palette's commands work.
    harness.invoke(window, ids::OPEN_SESSION_MENU).unwrap();
    assert!(
        !harness
            .cx
            .update(|cx| menu_ids(cx, window))
            .contains(&"new_workspace".to_owned())
    );
    harness.press(window, "escape");
    harness.show_workspace(window, second, &seconds);
}

#[gpui::test]
fn always_shows_the_panel_with_one_workspace_and_collapse_is_per_window(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.workspaces.show = WorkspaceShow::Always;
    });
    let first = harness.open_tab(ECHO);
    let one = harness.attach_showing(&[first]);
    let two = harness.attach_showing(&[first]);
    for window in [one, two] {
        harness.wait_chrome(window, "the panel", |chrome| {
            matches!(chrome, WorkspaceChrome::Panel { .. })
        });
    }
    harness.invoke(one, ids::TOGGLE_WORKSPACE_PANEL).unwrap();
    harness.wait_chrome(one, "the dropdown", WorkspaceChrome::is_dropdown);
    harness.cx.update(|cx| {
        assert!(matches!(
            chrome(cx, two).workspace_chrome,
            WorkspaceChrome::Panel { .. }
        ));
        let saved = |window: AnyWindowHandle| {
            cx.global::<Desktop>()
                .windows
                .record(window.window_id())
                .unwrap()
                .layout
                .workspace_collapsed
        };
        assert!(saved(one));
        assert!(!saved(two));
    });
}
