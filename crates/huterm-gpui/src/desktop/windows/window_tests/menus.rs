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
fn a11y_node(cx: &mut App, window: AnyWindowHandle, id: &str) -> Value {
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
    fn terminal_centre(
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
