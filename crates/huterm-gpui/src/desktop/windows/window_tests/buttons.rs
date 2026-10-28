//! Terminal-window buttons on `huterm-ui`'s `Button`. Dialog buttons take
//! focus inside their dialog, keep their `confirming` commands and focus
//! return, show their ring only after keys, and ignore GPUI's implicit
//! Enter click; title-row and tab-bar buttons are named buttons that leave
//! focus on the terminal.

use gpui::{
    KeyUpEvent, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent,
};
use huterm_ui::focus::ring_drawn;
use serde_json::Value;

use super::menus::a11y_node;
use super::*;

/// The default that closes a pane, and so a one-pane tab.
pub(super) const CLOSE: &str = if cfg!(target_os = "macos") {
    "cmd-w"
} else {
    "ctrl-shift-w"
};

/// Whether `window`'s last frame drew the ring of the dialog button `pick`
/// chooses.
fn ringed(
    cx: &mut App,
    window: AnyWindowHandle,
    pick: impl Fn(&WorkspaceView) -> &FocusHandle,
) -> bool {
    window
        .update(cx, |root, window, cx| {
            let view = root.downcast::<WorkspaceView>().unwrap();
            let handle = pick(view.read(cx)).clone();
            ring_drawn(&handle, window, cx)
        })
        .unwrap()
}

fn primary(view: &WorkspaceView) -> &FocusHandle {
    &view.close_buttons.primary
}

fn cancel(view: &WorkspaceView) -> &FocusHandle {
    &view.close_buttons.cancel
}

fn about(button: AboutButton) -> impl Fn(&WorkspaceView) -> &FocusHandle {
    move |view| view.about_buttons.handle(button)
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

/// The node holding `window`'s focus in its last frame.
fn focused_node(cx: &mut App, window: AnyWindowHandle) -> Value {
    let json = window
        .update(cx, |_, window, _| window.debug_a11y_tree_json())
        .unwrap()
        .expect("test windows report an accessibility tree for every frame");
    let tree: Value = serde_json::from_str(&json).unwrap();
    tree["gpui_focus"]
        .as_str()
        .map_or(Value::Null, |key| tree["nodes"][key].clone())
}

/// `id` as the accessibility dump spells element ids.
fn spelled(id: &str) -> String {
    format!("{:?}", gpui::ElementId::Name(id.to_owned().into()))
}

/// Sends a left `click_count` press at `at` to `window`.
fn press_at(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    at: gpui::Point<Pixels>,
    click_count: usize,
) {
    VisualTestContext::from_window(window, &harness.cx).simulate_event(
        MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: Modifiers::none(),
            click_count,
            first_mouse: false,
        },
    );
}

/// Drags the held left button to `to` and releases it there.
fn drag_and_release(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    to: gpui::Point<Pixels>,
) {
    let mut visual = VisualTestContext::from_window(window, &harness.cx);
    visual.simulate_event(MouseMoveEvent {
        position: to,
        modifiers: Modifiers::none(),
        pressed_button: Some(MouseButton::Left),
    });
    visual.simulate_event(MouseUpEvent {
        button: MouseButton::Left,
        position: to,
        modifiers: Modifiers::none(),
        click_count: 1,
    });
}

impl DesktopHarness {
    /// A window showing a tab with a background job beside an idle one,
    /// the busy tab focused, so closing it asks first.
    pub(super) fn busy_window(&mut self) -> (AnyWindowHandle, TabId) {
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

    /// Whether the title row saw a press, and how many window moves it
    /// started.
    fn title_row(&mut self, window: AnyWindowHandle) -> (bool, u32) {
        self.update_view(window, |view, _, _| {
            (view.title_row_press.get(), view.title_row_moves.get())
        })
    }

    /// Waits for the menu button to paint and returns its centre.
    fn menu_button_centre(
        &mut self,
        window: AnyWindowHandle,
    ) -> gpui::Point<Pixels> {
        self.wait_for("the menu button to paint", |cx| {
            window
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>().is_ok_and(|view| {
                        view.read(cx).menu_button_bounds.get().is_some()
                    })
                })
                .unwrap_or(false)
        });
        self.update_view(window, |view, _, _| view.menu_button_bounds.get())
            .unwrap()
            .center()
    }

    /// The close confirmation's focused button, `primary`, `cancel`, or
    /// `none`, or `None` while no confirmation shows.
    pub(super) fn dialog_focus(
        &mut self,
        window: AnyWindowHandle,
    ) -> Option<&'static str> {
        self.update_view(window, |view, window, _| {
            view.close
                .confirmation
                .is_some()
                .then(|| view.close_buttons.focused(window))
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
        ringed(cx, window, primary)
    });
    assert_eq!(harness.dialog_focus(window), Some("primary"));
    harness.press(window, "tab");
    assert_eq!(harness.dialog_focus(window), Some("cancel"));
    // The shortcut again neither confirms nor resets the dialog.
    harness.press(window, CLOSE);
    harness.cx.run_until_parked();
    assert_eq!(harness.dialog_focus(window), Some("cancel"));
    harness
        .cx
        .update(|cx| assert!(terminal(cx, window, runs).is_some()));
    harness.press(window, "left");
    assert_eq!(harness.dialog_focus(window), Some("primary"));
    harness.press(window, "right");
    assert_eq!(harness.dialog_focus(window), Some("cancel"));
    harness.wait_for("the ring on Cancel", |cx| {
        ringed(cx, window, cancel) && !ringed(cx, window, primary)
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
    assert_eq!(harness.about_focus(window), Some(AboutButton::Copy));
    harness.wait_for("the ring on Copy Details", |cx| {
        ringed(cx, window, about(AboutButton::Copy))
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
        for button in
            [AboutButton::Licenses, AboutButton::Copy, AboutButton::Close]
        {
            assert!(
                !ringed(cx, window, about(button)),
                "{button:?} shows a ring after a click"
            );
        }
    });
    assert_eq!(harness.about_focus(window), Some(AboutButton::Copy));
    harness.press(window, "tab");
    harness.wait_for("the ring on Close after Tab", |cx| {
        ringed(cx, window, about(AboutButton::Close))
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
    assert_eq!(harness.dialog_focus(window), Some("primary"));
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

#[gpui::test]
fn a_click_on_a_dialog_button_focuses_it(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    harness.invoke(window, ids::ABOUT).unwrap();
    assert_eq!(harness.about_focus(window), Some(AboutButton::Close));
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
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
            "cleared".to_owned(),
        ));
    });
    // Enter presses what was clicked, not the Close button Enter started on.
    harness.press(window, "enter");
    harness.cx.update(|cx| {
        let copied = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_ref(), Some(&details), "Enter copied again");
    });
    assert_eq!(harness.about_focus(window), Some(AboutButton::Copy));
}

#[gpui::test]
fn a_click_on_a_licenses_button_focuses_it(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    harness.invoke(window, ids::ABOUT).unwrap();
    harness.press(window, "tab enter");
    harness.wait_for("the notices to load", |cx| {
        matches!(
            licenses_state(cx, window),
            Some(LicensesState::Ready { .. })
        )
    });
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Search));
    // The press focuses the button, and the panel's return of focus to the
    // search leaves a press that focused something alone.
    a11y_click(&mut harness, window, "license-copy");
    assert_eq!(harness.licenses_focus(window), Some(LicensesFocus::Copy));
}

#[gpui::test]
fn dialogs_report_their_role_name_and_modal_state_and_focus_their_buttons(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, _) = harness.busy_window();
    harness.invoke(window, ids::ABOUT).unwrap();
    let focused_on = |harness: &mut DesktopHarness, id: &'static str| {
        harness.wait_for(id, |cx| {
            focused_node(cx, window)["element_id"] == spelled(id)
        });
    };
    let dialog = |harness: &mut DesktopHarness, id: &'static str| {
        harness
            .cx
            .update(|cx| a11y_node(cx, window, id)["aria"].clone())
    };
    // Assistive technology follows the focused button itself.
    focused_on(&mut harness, "about-close");
    assert_eq!(focused_node_role(&mut harness, window), "Button");
    let about = dialog(&mut harness, "about-dialog");
    assert_eq!(about["role"], "Dialog");
    assert_eq!(about["label"], "About Huterm");
    assert_eq!(about["modal"], true);
    harness.press(window, "tab");
    focused_on(&mut harness, "about-licenses");
    harness.press(window, "enter");
    harness.wait_for("the Licenses view", |cx| {
        licenses_state(cx, window).is_some()
    });
    focused_on(&mut harness, "licenses-search");
    let licenses = dialog(&mut harness, "licenses");
    assert_eq!(licenses["role"], "Dialog");
    assert_eq!(licenses["label"], "Open Source Licenses");
    assert_eq!(licenses["modal"], true);
    harness.press(window, "escape");
    harness.press(window, "escape");
    harness.press(window, CLOSE);
    harness.wait_for("the confirmation", |cx| confirming(cx, window));
    let (title, subtitle) = harness.update_view(window, |view, _, cx| {
        let target = view.close.confirmation.as_ref().unwrap();
        let model = build_close_dialog(&view.close_dialog_input(target, cx));
        (model.title, model.subtitle)
    });
    focused_on(&mut harness, "confirm-close");
    let alert = dialog(&mut harness, "close-dialog");
    assert_eq!(alert["role"], "AlertDialog");
    assert_eq!(alert["label"], title.as_str());
    assert_eq!(alert["description"], subtitle.as_str());
    assert_eq!(alert["modal"], true);
    harness.press(window, "tab");
    focused_on(&mut harness, "cancel-close");
}

/// The role of the node holding `window`'s focus in its last frame.
fn focused_node_role(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> String {
    harness.cx.update(|cx| {
        focused_node(cx, window)["aria"]["role"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    })
}

#[gpui::test]
fn dismissing_a_toast_over_a_dialog_returns_focus_to_the_dialog(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    harness.invoke(window, ids::ABOUT).unwrap();
    harness.update_view(window, |view, _, cx| {
        view.notify(
            NoticeContent::diagnostic(
                super::super::notices::Severity::Error,
                NoticeSource::Config,
                "Config has errors",
                "line 3",
            ),
            cx,
        );
    });
    // One update, so no frame lets the terminal hand focus it receives
    // back to the dialog.
    harness.update_view(window, |view, window, cx| {
        let focus = CommandInvocation::new(ids::FOCUS_NOTICES, vec![]);
        view.invoke_interactive(&focus, window, cx).unwrap();
        assert!(view.notice_focus.is_focused(window));
        let dismiss = CommandInvocation::new(ids::NOTICE_DISMISS, vec![]);
        view.invoke_interactive(&dismiss, window, cx).unwrap();
        assert!(view.notices.is_empty());
        assert!(
            view.about_buttons
                .handle(AboutButton::Close)
                .is_focused(window),
            "focus left the dialog"
        );
    });
    harness.cx.run_until_parked();
    assert_eq!(harness.about_focus(window), Some(AboutButton::Close));
}

#[gpui::test]
fn a_press_on_an_inactive_tabs_close_button_neither_selects_nor_drags_it(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let other = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab, other]);
    harness.wait_text(window, tab, "READY");
    harness.update_view(window, |view, window, cx| {
        view.select(tab, window, cx);
    });
    harness.activate(window, tab);
    // GPUI looks debug selectors up by a static name.
    let selector = format!("close-tab-{}", other.get()).leak();
    let close = VisualTestContext::from_window(window, &harness.cx)
        .debug_bounds(selector)
        .expect("the inactive tab draws its close button")
        .center();
    press_at(&mut harness, window, close, 1);
    harness.update_view(window, |view, _, cx| {
        assert!(view.reorder.is_none(), "the press started a tab drag");
        assert_eq!(view.active_tab(cx), Some(tab));
    });
    VisualTestContext::from_window(window, &harness.cx).simulate_event(
        MouseUpEvent {
            button: MouseButton::Left,
            position: close,
            modifiers: Modifiers::none(),
            click_count: 1,
        },
    );
    harness.wait_for("the click to close the inactive tab", |cx| {
        terminal(cx, window, other).is_none()
    });
    assert_eq!(
        harness.update_view(window, |view, _, cx| view.active_tab(cx)),
        Some(tab)
    );
}

/// macOS draws the menu button on its title strip, whose empty space moves
/// the window; Linux has a title row only with client-side decorations,
/// which the next test covers.
#[gpui::test]
#[cfg_attr(
    not(target_os = "macos"),
    ignore = "the menu button sits on a title row only on macOS here"
)]
fn a_press_on_the_menu_button_starts_no_title_row_move(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    let button = harness.menu_button_centre(window);
    let away = harness.terminal_centre(window);
    press_at(&mut harness, window, button, 1);
    assert_eq!(harness.title_row(window), (false, 0), "the row saw it");
    drag_and_release(&mut harness, window, away);
    assert_eq!(harness.title_row(window), (false, 0));
    // The strip's empty space beside the button does take the press.
    let empty = gpui::point(button.x - px(60.0), button.y);
    press_at(&mut harness, window, empty, 1);
    assert_eq!(harness.title_row(window), (true, 0));
}

/// Huterm draws window controls only on Linux, under client-side
/// decorations; the test forces both, so it runs on macOS too. The test
/// platform does not implement a window move or Linux's zoom, so on Linux a
/// press that reached the title row would fail there too.
#[gpui::test]
fn a_press_on_a_drawn_window_control_starts_no_title_row_gesture(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.position = crate::config::TabPosition::Titlebar;
    });
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    // The test platform reports server-side decorations; take the client
    // frame a desktop without a title bar would give, as GPUI reports a
    // decoration change.
    harness.update_view(window, |view, window, cx| {
        view.decorations_override = Some(gpui::Decorations::Client {
            tiling: gpui::Tiling::default(),
        });
        assert!(view.sync_frame(window));
        view.refresh_tab_visibility(window, cx);
        view.sync_tab_layout(window, cx);
        cx.notify();
    });
    harness.wait_for("the window controls to paint", |cx| {
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
    let [minimize, maximize, close] = WindowButton::ALL.map(|button| {
        harness
            .update_view(window, |view, _, _| {
                view.window_button_bounds[button.index()].get()
            })
            .unwrap_or_else(|| panic!("{button:?} is not drawn"))
            .center()
    });
    let menu = harness.menu_button_centre(window);
    // Releasing over the terminal clicks nothing, so no control acts.
    let away = harness.terminal_centre(window);
    for at in [minimize, maximize, close, menu] {
        press_at(&mut harness, window, at, 1);
        assert_eq!(harness.title_row(window), (false, 0), "{at:?}");
        drag_and_release(&mut harness, window, away);
        assert_eq!(harness.title_row(window), (false, 0), "{at:?}");
    }
    // A double-click on the row zooms the window.
    press_at(&mut harness, window, minimize, 2);
    drag_and_release(&mut harness, window, away);
    assert_eq!(harness.title_row(window), (false, 0));
}

#[gpui::test]
fn tab_close_and_toast_buttons_are_named_after_what_they_act_on(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let other = harness.open_tab(ECHO);
    let window = harness.attach_showing(&[tab, other]);
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    harness.update_view(window, |view, _, cx| {
        view.notify(
            NoticeContent::diagnostic(
                super::super::notices::Severity::Error,
                NoticeSource::Config,
                "Config has errors",
                "line 3",
            )
            .action(
                "Reload",
                huterm_protocol::CommandInvocation::new(
                    ids::RELOAD_CONFIG,
                    Vec::new(),
                ),
            ),
            cx,
        );
    });
    let title = harness.update_view(window, |view, _, cx| {
        let shown = view.tabs.iter().find(|shown| shown.id == other).unwrap();
        shown.label(view.config.tabs, cx).0
    });
    assert_ne!(title, "");
    let labelled = |harness: &mut DesktopHarness, label: String| {
        harness.wait_for(&label.clone(), |cx| {
            let json = window
                .update(cx, |_, window, _| window.debug_a11y_tree_json())
                .unwrap()
                .unwrap();
            let tree: Value = serde_json::from_str(&json).unwrap();
            tree["nodes"].as_object().unwrap().values().any(|node| {
                node["aria"]["label"].as_str() == Some(label.as_str())
                    && node["aria"]["role"] == "Button"
            })
        });
    };
    labelled(&mut harness, format!("Close {title}"));
    labelled(&mut harness, "Dismiss Config has errors".to_owned());
    // The action looks like a link but runs a command.
    labelled(&mut harness, "Reload".to_owned());
}
