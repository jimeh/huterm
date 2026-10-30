//! Terminal-window menus on `huterm-ui`'s popover, driven by real pointer
//! and keyboard events: the modal dismissal and focus contract of
//! `docs/plans/ui-components.md`, with the active terminal as the return
//! target.

use gpui::{Modifiers, MouseDownEvent, MouseUpEvent, PlatformInput};
use serde_json::Value;

use super::*;

/// A child that prints `READY`, turns on SGR mouse reporting once it reads
/// a byte, prints `GO`, and then prints every byte it receives as two hex
/// digits, so a mouse report would show before a later key.
const MOUSE_ECHO: &str = "stty raw -echo; printf READY; dd bs=1 count=1 2>/dev/null >/dev/null; \
     printf '\\033[?1000h\\033[?1006hGO'; \
     while byte=$(dd bs=1 count=1 2>/dev/null | od -An -tx1 | tr -d ' \\n'); \
     [ -n \"$byte\" ]; do printf '%s' \"$byte\"; done";

/// The open menu's kind, as smoke state names it, or `None` while closed.
fn menu_kind(cx: &mut App, window: AnyWindowHandle) -> Option<&'static str> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            Some(match view.read(cx).menu.as_ref()?.kind {
                MenuKind::Window => "window",
                MenuKind::Tab(_) => "tab",
                MenuKind::Terminal { .. } => "terminal",
                MenuKind::Pane { .. } => "pane",
                MenuKind::Field(_) => "field",
            })
        })
        .ok()
        .flatten()
}

/// Whether a menu is open and its content has painted, so a press lands
/// on what the user sees.
fn menu_painted(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>().is_ok_and(|view| {
                let view = view.read(cx);
                view.menu.is_some() && view.popover.content_bounds().is_some()
            })
        })
        .unwrap_or(false)
}

/// Whether `window`'s view of `tab` holds keyboard focus, active or not.
fn terminal_focused(cx: &mut App, window: AnyWindowHandle, tab: TabId) -> bool {
    terminal(cx, window, tab).is_some_and(|view| {
        window
            .update(cx, |_, window, cx| view.read(cx).focus.is_focused(window))
            .unwrap_or(false)
    })
}

/// The accessibility node of the element named `id` in `window`'s last
/// frame. The dump spells element ids in their `Debug` form.
pub(super) fn a11y_node(
    cx: &mut App,
    window: AnyWindowHandle,
    id: &str,
) -> Value {
    let json = window
        .update(cx, |_, window, _| window.debug_a11y_tree_json())
        .unwrap()
        .expect("test windows report an accessibility tree for every frame");
    let tree: Value = serde_json::from_str(&json).unwrap();
    let spelled = format!("{:?}", gpui::ElementId::Name(id.to_owned().into()));
    tree["nodes"]
        .as_object()
        .unwrap()
        .values()
        .find(|node| node["element_id"].as_str() == Some(spelled.as_str()))
        .cloned()
        .unwrap_or_else(|| panic!("no accessibility node for `{id}`"))
}

impl DesktopHarness {
    /// Opens a window on a `script` tab, focused and showing `READY`.
    fn menu_window(&mut self, script: &str) -> (AnyWindowHandle, TabId) {
        let tab = self.open_tab(script);
        let window = self.attach_window();
        self.activate(window, tab);
        self.wait_text(window, tab, "READY");
        (window, tab)
    }

    /// A [`MOUSE_ECHO`] window whose child reports mouse presses.
    fn reporting_window(&mut self) -> (AnyWindowHandle, TabId) {
        let (window, tab) = self.menu_window(MOUSE_ECHO);
        self.press(window, "g");
        self.wait_text(window, tab, "GO");
        (window, tab)
    }

    /// Runs `command` and waits for the menu it opens to paint.
    fn open_painted(
        &mut self,
        window: AnyWindowHandle,
        command: huterm_protocol::CommandId,
    ) {
        self.invoke(window, command).unwrap();
        self.wait_for("the menu to paint", |cx| menu_painted(cx, window));
    }

    /// The centre of the active terminal's drawn pane.
    pub(super) fn terminal_centre(
        &mut self,
        window: AnyWindowHandle,
    ) -> gpui::Point<Pixels> {
        self.update_view(window, |view, _, _| {
            view.drawn_panes
                .borrow()
                .as_ref()
                .and_then(|(_, rects)| rects.first().map(|rect| rect.frame))
        })
        .expect("a drawn pane")
        .center()
    }

    fn click_at(&mut self, window: AnyWindowHandle, at: gpui::Point<Pixels>) {
        VisualTestContext::from_window(window, &self.cx)
            .simulate_click(at, Modifiers::none());
    }

    /// Types `z` and waits for the child to report it, so every byte a
    /// press before it sent has arrived too. Returns what the child read.
    fn probe(&mut self, window: AnyWindowHandle, tab: TabId) -> String {
        self.press(window, "z");
        self.wait_for("the probe key to reach the child", |cx| {
            received(cx, window, tab, "GO")
                .is_some_and(|bytes| bytes.ends_with("7a"))
        });
        self.cx
            .update(|cx| received(cx, window, tab, "GO"))
            .unwrap()
    }
}

#[gpui::test]
fn an_outside_press_closes_the_menu_and_reaches_no_terminal(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab) = harness.reporting_window();
    let centre = harness.terminal_centre(window);
    harness.open_painted(window, ids::OPEN_MENU);
    harness.click_at(window, centre);
    harness.cx.update(|cx| {
        assert_eq!(menu_kind(cx, window), None);
        assert!(focused(cx, window, tab), "focus returns to the terminal");
    });
    // The press and its release sent no mouse report: the key typed next
    // is the only byte the child reads.
    assert_eq!(harness.probe(window, tab), "7a");
}

#[gpui::test]
fn a_press_before_the_menus_first_frame_reaches_no_terminal(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab) = harness.reporting_window();
    let centre = harness.terminal_centre(window);
    let press = |position| {
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        })
    };
    // Open the menu and press over the terminal in one update, so the
    // press meets the last frame, which drew no menu.
    harness
        .cx
        .update(|cx| {
            window.update(cx, |root, window, cx| {
                let view = root.downcast::<WorkspaceView>().unwrap();
                view.update(cx, |view, cx| view.open_menu(true, window, cx))
                    .unwrap();
                window.dispatch_event(press(centre), cx);
            })
        })
        .unwrap();
    harness.cx.update(|cx| {
        assert_eq!(menu_kind(cx, window), None);
        assert!(terminal_focused(cx, window, tab));
        let terminal = terminal(cx, window, tab).unwrap();
        let terminal = terminal.read(cx);
        assert!(
            !terminal.owns_pointer_gesture(),
            "the terminal took the press"
        );
        assert!(terminal.selection.is_none());
    });
    // The release after a drawn frame acts on nothing either.
    harness
        .wait_for("a frame without the menu", |cx| !menu_painted(cx, window));
    VisualTestContext::from_window(window, &harness.cx).simulate_event(
        MouseUpEvent {
            button: MouseButton::Left,
            position: centre,
            modifiers: Modifiers::none(),
            click_count: 1,
        },
    );
    assert_eq!(harness.probe(window, tab), "7a");
    assert_eq!(harness.cx.update(|cx| menu_kind(cx, window)), None);
}

#[gpui::test]
fn a_press_on_the_menu_button_closes_its_open_menu(cx: &mut TestAppContext) {
    // A shown bar draws the button where no title strip does, as on Linux.
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
    });
    let (window, tab) = harness.menu_window(ECHO);
    harness.wait_for("the menu button to paint", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx).menu_button_bounds.get().is_some()
                })
            })
            .unwrap_or(false)
    });
    let button = harness
        .update_view(window, |view, _, _| view.menu_button_bounds.get())
        .unwrap()
        .center();
    harness.click_at(window, button);
    harness.wait_for("the window menu to paint", |cx| menu_painted(cx, window));
    harness.cx.update(|cx| {
        assert_eq!(menu_kind(cx, window), Some("window"));
        assert_eq!(
            a11y_node(cx, window, "window-menu")["aria"]["expanded"],
            true
        );
    });
    harness.click_at(window, button);
    harness.cx.update(|cx| {
        assert_eq!(menu_kind(cx, window), None, "the press closes the menu");
        assert!(focused(cx, window, tab), "the button never keeps focus");
    });
    // It stays closed: the press did not reach the button to reopen it.
    harness
        .wait_for("a frame without the menu", |cx| !menu_painted(cx, window));
    harness.cx.update(|cx| {
        assert_eq!(menu_kind(cx, window), None);
        assert_eq!(
            a11y_node(cx, window, "window-menu")["aria"]["expanded"],
            false
        );
    });
}

#[gpui::test]
fn escape_tab_shift_tab_and_a_pick_close_the_menu_onto_the_terminal(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab) = harness.menu_window(ECHO);
    for key in ["escape", "tab", "shift-tab"] {
        harness.open_painted(window, ids::OPEN_MENU);
        harness.press(window, key);
        harness.cx.update(|cx| {
            assert_eq!(menu_kind(cx, window), None, "{key}");
            assert!(focused(cx, window, tab), "{key} returns focus");
        });
    }
    // The keyboard-opened terminal menu selects Paste; Select All follows.
    harness.open_painted(window, ids::OPEN_CONTEXT_MENU);
    harness.press(window, "down enter");
    harness.cx.update(|cx| {
        assert_eq!(menu_kind(cx, window), None);
        assert!(focused(cx, window, tab), "a pick returns focus");
        let terminal = terminal(cx, window, tab).unwrap();
        assert!(
            terminal.read(cx).command_availability(ids::COPY).is_ok(),
            "the pick ran Select All"
        );
    });
}

#[gpui::test]
fn deactivating_the_window_closes_the_menu_onto_the_terminal(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab) = harness.menu_window(ECHO);
    harness.open_painted(window, ids::OPEN_MENU);
    harness.deactivate(window);
    harness.cx.update(|cx| {
        assert_eq!(menu_kind(cx, window), None);
        assert!(terminal_focused(cx, window, tab));
    });
    // Back in the window, keys reach the terminal.
    harness.activate(window, tab);
    harness.type_line(window, "back");
    harness.wait_text(window, tab, "got:back");
}

/// The open palette's field in `window`.
fn palette_field(cx: &mut App, window: AnyWindowHandle) -> Entity<Input> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().unwrap();
            let palette = view.read(cx).palette.clone().expect("open");
            palette.read(cx).field()
        })
        .unwrap()
}

/// A right press on the palette's field opens the field's menu at the
/// pointer; Escape and a pick close it back onto the field, and the
/// palette stays open throughout.
#[gpui::test]
fn a_right_press_in_the_palette_opens_its_fields_menu(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let (window, _) = harness.menu_window(ECHO);
    harness.invoke(window, ids::OPEN_COMMAND_PALETTE).unwrap();
    harness.press(window, "a b c");
    let field = harness.cx.update(|cx| palette_field(cx, window));
    let (_, bounds) = harness
        .cx
        .update(|cx| field.read(cx).painted_caret())
        .expect("the field painted");
    let on_text = gpui::point(bounds.left() + px(1.0), bounds.center().y);
    let right_press = |harness: &mut DesktopHarness| {
        let mut visual = VisualTestContext::from_window(window, &harness.cx);
        visual.simulate_mouse_down(
            on_text,
            MouseButton::Right,
            Modifiers::none(),
        );
        visual.simulate_mouse_up(
            on_text,
            MouseButton::Right,
            Modifiers::none(),
        );
        harness
            .wait_for("the field menu to paint", |cx| menu_painted(cx, window));
    };
    let field_focused = |cx: &mut App| {
        window
            .update(cx, |_, window, cx| {
                field.read(cx).focus_handle(cx).is_focused(window)
            })
            .unwrap()
    };
    right_press(&mut harness);
    harness.cx.update(|cx| {
        assert_eq!(menu_kind(cx, window), Some("field"));
    });
    harness.press(window, "escape");
    harness.cx.update(|cx| {
        assert_eq!(menu_kind(cx, window), None);
        assert!(field_focused(cx), "Escape returns focus to the field");
    });
    // Nothing is selected, so Paste is the first item; the press put the
    // caret before the text.
    harness
        .cx
        .write_to_clipboard(gpui::ClipboardItem::new_string("XY".to_owned()));
    right_press(&mut harness);
    harness.press(window, "down enter");
    harness.cx.update(|cx| {
        assert_eq!(menu_kind(cx, window), None);
        assert!(field_focused(cx), "a pick returns focus to the field");
        assert_eq!(field.read(cx).text(), "XYabc");
        assert!(palette_state(cx, window).is_some(), "the palette stays");
    });
}

/// The Edit menu's Cut is a text field's, so it is disabled over a
/// terminal and enabled in the palette, where Copy acts on the field.
#[gpui::test]
fn the_edit_menus_cut_needs_a_field_and_copy_follows_focus(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab) = harness.menu_window(ECHO);
    let available = |harness: &mut DesktopHarness, id| {
        harness.update_view(window, |_, window, cx| {
            window.is_action_available(
                crate::commands::invoke(id).into_boxed().as_ref(),
                cx,
            )
        })
    };
    assert!(!available(&mut harness, ids::TEXT_CUT));
    assert!(available(&mut harness, ids::COPY));
    // The terminal runs the Edit menu's Select All itself.
    harness.update_view(window, |_, window, cx| {
        window.dispatch_action(
            crate::commands::invoke(ids::SELECT_ALL).into_boxed(),
            cx,
        );
    });
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        let terminal = terminal(cx, window, tab).unwrap();
        assert!(
            terminal.read(cx).command_availability(ids::COPY).is_ok(),
            "Select All selected the terminal"
        );
    });
    harness.invoke(window, ids::OPEN_COMMAND_PALETTE).unwrap();
    harness.press(window, "a b c");
    harness.cx.run_until_parked();
    assert!(available(&mut harness, ids::TEXT_CUT));
    // The terminal's other menu items have nothing to act on in a field.
    assert!(!available(&mut harness, ids::CLEAR_SCROLLBACK));
    for id in [ids::SELECT_ALL, ids::COPY] {
        harness.update_view(window, |_, window, cx| {
            window
                .dispatch_action(crate::commands::invoke(id).into_boxed(), cx);
        });
        harness.cx.run_until_parked();
    }
    let copied = harness
        .cx
        .update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some("abc"));
}

#[gpui::test]
fn opening_the_palette_closes_the_menu(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let (window, _) = harness.menu_window(ECHO);
    harness.open_painted(window, ids::OPEN_MENU);
    harness.invoke(window, ids::OPEN_COMMAND_PALETTE).unwrap();
    harness.cx.update(|cx| {
        assert_eq!(menu_kind(cx, window), None);
        window
            .update(cx, |root, window, cx| {
                let view = root.downcast::<WorkspaceView>().unwrap();
                let palette = view.read(cx).palette.clone().expect("open");
                assert!(palette.read(cx).focus_handle(cx).is_focused(window));
            })
            .unwrap();
    });
}

#[gpui::test]
fn a_menu_supersedes_a_right_click_waiting_for_its_link(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, _) = harness.menu_window(ECHO);
    // A right-click on a cell waits for a snapshot to say whether a link
    // is there. The window menu then opens and closes before it draws, so
    // no terminal press or blur supersedes the lookup.
    harness.update_view(window, |view, window, cx| {
        let terminal = view.active_view(cx).unwrap();
        terminal.update(cx, |terminal, cx| {
            terminal.request_context_menu(
                point(px(20.0), px(20.0)),
                Some(huterm_protocol::MousePosition { column: 1, row: 0 }),
                cx,
            );
            assert!(terminal.context_lookup.is_some(), "the lookup waits");
        });
        view.toggle_menu(window, cx);
        assert!(view.menu.is_some(), "the window menu opens");
        view.toggle_menu(window, cx);
        assert!(view.menu.is_none(), "the window menu closes");
    });
    // Neither the snapshot's answer nor the lookup's timeout opens a menu.
    harness
        .cx
        .executor()
        .advance_clock(crate::desktop::CONTEXT_LINK_TIMEOUT * 2);
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        assert_eq!(menu_kind(cx, window), None, "the lookup opens no menu");
    });
}

impl DesktopHarness {
    /// A window on an [`ECHO`] tab split into two panes, with the left one
    /// focused. Returns the left and right panes.
    fn split_menu_window(
        &mut self,
    ) -> (AnyWindowHandle, TabId, PaneId, PaneId) {
        let (window, tab) = self.menu_window(ECHO);
        let left = self.first_pane(tab);
        let right = self.split(tab, left, SplitDirection::Right, ECHO);
        self.wait_for("the right pane's shell", |cx| {
            pane_screen(cx, window, tab, right)
                .is_some_and(|text| text.contains("READY"))
        });
        self.update_view(window, |view, window, cx| {
            view.focus_pane(tab, left, window, cx);
        });
        (window, tab, left, right)
    }
}

#[gpui::test]
fn a_terminal_menu_closes_when_its_pane_closes_under_it(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, left, _) = harness.split_menu_window();
    harness.open_painted(window, ids::OPEN_CONTEXT_MENU);
    // The pane closes as its shell exiting would; the right pane takes
    // focus, and the menu's picks would act on it.
    harness
        .runtime
        .lock()
        .close_pane(harness.workspace, tab, left)
        .unwrap();
    harness.wait_for("the closed pane's view to go", |cx| {
        pane_view(cx, window, tab, left).is_none()
    });
    harness.cx.update(|cx| {
        assert_eq!(menu_kind(cx, window), None, "the menu outlived its pane");
        assert!(focused(cx, window, tab), "focus returns to the terminal");
    });
}

#[gpui::test]
fn a_terminal_menu_pick_after_focus_leaves_its_pane_runs_nothing(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab, left, right) = harness.split_menu_window();
    harness.open_painted(window, ids::OPEN_CONTEXT_MENU);
    // Focus moves and the pick arrives before a refresh closes the menu.
    harness.update_view(window, |view, window, cx| {
        let menu = view.menu.as_ref().unwrap().view.clone();
        view.focus_pane(tab, right, window, cx);
        view.handle_menu_event(
            &menu,
            &MenuEvent::Picked("select_all".into()),
            window,
            cx,
        );
    });
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        assert_eq!(menu_kind(cx, window), None);
        for pane in [left, right] {
            let view = pane_view(cx, window, tab, pane).unwrap();
            assert!(view.read(cx).selection.is_none(), "{pane:?} selected");
        }
    });
}
