//! Terminal-window buttons on `huterm-ui`'s `Button`. Dialog buttons keep
//! their `confirming` commands and focus return, show their ring only after
//! keys, and ignore GPUI's implicit Enter click; title-row and tab-bar
//! buttons are named buttons that leave focus on the terminal.

use gpui::KeyUpEvent;
use huterm_ui::focus::current_ring_drawn;

use super::super::close_dialog::DialogFocus;
use super::menus::a11y_node;
use super::*;

/// The default that closes a pane, and so a one-pane tab.
const CLOSE: &str = if cfg!(target_os = "macos") {
    "cmd-w"
} else {
    "ctrl-shift-w"
};

/// Whether `window`'s last frame drew the ring of the dialog button named
/// `id`.
fn ringed(cx: &mut App, window: AnyWindowHandle, id: &'static str) -> bool {
    window
        .update(cx, |_, window, cx| current_ring_drawn(id, window, cx))
        .unwrap()
}

/// Clicks the element named `id` as an assistive technology does: GPUI
/// presses and releases the pointer at its center.
fn a11y_click(harness: &mut DesktopHarness, window: AnyWindowHandle, id: &str) {
    harness.cx.update(|cx| {
        let target = a11y_node(cx, window, id)["accesskit_id"]
            .as_str()
            .and_then(|id| id.parse().ok())
            .expect("every node reports its AccessKit id");
        window
            .update(cx, |_, window, cx| {
                window.simulate_a11y_action(
                    gpui::accesskit::ActionRequest {
                        action: gpui::AccessibleAction::Click,
                        target_tree: gpui::accesskit::TreeId::ROOT,
                        target_node: gpui::accesskit::NodeId(target),
                        data: None,
                    },
                    cx,
                );
            })
            .unwrap();
    });
}

impl DesktopHarness {
    /// A window showing a tab with a background job beside an idle one,
    /// the busy tab focused, so closing it asks first.
    fn busy_window(&mut self) -> (AnyWindowHandle, TabId) {
        let runs = self.open_tab(RUNS_JOB);
        let stays = self.open_tab(ECHO);
        let window = self.attach_showing(&[runs, stays]);
        self.wait_text(window, runs, "READY");
        self.update_view(window, |view, window, cx| {
            view.select(runs, window, cx);
        });
        self.activate(window, runs);
        (window, runs)
    }

    /// The close confirmation's keyboard focus, or `None` while none shows.
    fn dialog_focus(&mut self, window: AnyWindowHandle) -> Option<DialogFocus> {
        self.update_view(window, |view, _, _| {
            view.close
                .confirmation
                .is_some()
                .then_some(view.close.dialog_focus)
        })
    }
}

#[gpui::test]
fn the_close_dialogs_keys_outlast_a_repeated_close_and_return_focus(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, runs) = harness.busy_window();
    harness.press(window, CLOSE);
    harness.wait_for("the confirmation", |cx| confirming(cx, window));
    // The shortcut was a key, so the primary button shows its ring.
    harness.wait_for("the ring on the primary button", |cx| {
        ringed(cx, window, "confirm-close")
    });
    assert_eq!(harness.dialog_focus(window), Some(DialogFocus::Primary));
    harness.press(window, "tab");
    assert_eq!(harness.dialog_focus(window), Some(DialogFocus::Cancel));
    // The shortcut again neither confirms nor resets the dialog.
    harness.press(window, CLOSE);
    harness.cx.run_until_parked();
    assert_eq!(harness.dialog_focus(window), Some(DialogFocus::Cancel));
    harness
        .cx
        .update(|cx| assert!(terminal(cx, window, runs).is_some()));
    harness.press(window, "left");
    assert_eq!(harness.dialog_focus(window), Some(DialogFocus::Primary));
    harness.press(window, "right");
    assert_eq!(harness.dialog_focus(window), Some(DialogFocus::Cancel));
    harness.wait_for("the ring on Cancel", |cx| {
        ringed(cx, window, "cancel-close")
            && !ringed(cx, window, "confirm-close")
    });
    // Enter presses the focused button, Cancel.
    harness.press(window, "enter");
    assert_eq!(harness.dialog_focus(window), None);
    harness.wait_for("the terminal to take focus back", |cx| {
        focused(cx, window, runs)
    });
}

#[gpui::test]
fn dialog_buttons_show_their_ring_after_keys_and_not_after_a_click(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    harness.invoke(window, ids::ABOUT).unwrap();
    harness.press(window, "tab tab");
    assert_eq!(harness.about_focus(window), Some(AboutFocus::Copy));
    harness.wait_for("the ring on Copy Details", |cx| {
        ringed(cx, window, "about-copy")
    });
    let details = harness
        .update_view(window, |view, _, _| view.about.clone().unwrap().text());
    a11y_click(&mut harness, window, "about-copy");
    harness.wait_for("the click to copy the details", |cx| {
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_ref()
            == Some(&details)
    });
    harness.cx.update(|cx| {
        for id in ["about-licenses", "about-copy", "about-close"] {
            assert!(!ringed(cx, window, id), "{id} shows a ring after a click");
        }
    });
    assert_eq!(harness.about_focus(window), Some(AboutFocus::Copy));
    harness.press(window, "tab");
    harness.wait_for("the ring on Close after Tab", |cx| {
        ringed(cx, window, "about-close")
    });
}

#[gpui::test]
fn a_removed_enter_binding_does_not_press_the_focused_dialog_button(
    cx: &mut TestAppContext,
) {
    let entry = |key: &str, disabled| huterm_config::KeybindingEntry {
        key: key.to_owned(),
        command: ids::DIALOG_CONFIRM.as_str().to_owned(),
        args: None,
        when: None,
        description: None,
        disabled,
    };
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.keybindings =
            vec![entry("enter", Some(true)), entry("f6", None)];
    });
    let (window, runs) = harness.busy_window();
    harness.press(window, CLOSE);
    harness.wait_for("the confirmation", |cx| confirming(cx, window));
    assert_eq!(harness.dialog_focus(window), Some(DialogFocus::Primary));
    // GPUI clicks a focused element on Enter's key-up, so send both.
    harness.press(window, "enter");
    VisualTestContext::from_window(window, &harness.cx).simulate_event(
        KeyUpEvent {
            keystroke: Keystroke::parse("enter").unwrap(),
        },
    );
    harness.cx.run_until_parked();
    harness.cx.update(|cx| {
        assert!(confirming(cx, window), "Enter pressed the primary button");
        assert!(terminal(cx, window, runs).is_some());
    });
    harness.press(window, "f6");
    harness.wait_for("the rebound key to close the tab", |cx| {
        !confirming(cx, window) && terminal(cx, window, runs).is_none()
    });
}

#[gpui::test]
fn title_row_and_tab_bar_buttons_are_named_and_leave_focus_on_the_terminal(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    // The bar shows from the second tab.
    let tab = harness.open_tab(ECHO);
    let other = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab, other]);
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    harness.cx.update(|cx| {
        let aria = &a11y_node(cx, window, "new-tab")["aria"];
        assert_eq!(aria["role"], "Button");
        assert_eq!(aria["label"], "New Tab");
    });
    let before = harness.update_view(window, |view, _, _| view.tabs.len());
    a11y_click(&mut harness, window, "new-tab");
    harness.wait_for("the new tab's terminal to take focus", |cx| {
        window
            .update(cx, |root, window, cx| {
                let view = root.downcast::<WorkspaceView>().unwrap();
                let view = view.read(cx);
                view.tabs.len() == before + 1
                    && view.active_view(cx).is_some_and(|terminal| {
                        terminal.read(cx).focus.is_focused(window)
                    })
            })
            .unwrap()
    });
}
