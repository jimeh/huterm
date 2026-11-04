//! The Window Transparency panel: opening and closing, real key dispatch,
//! focus return, outside presses, and its states.
use super::super::transparency_panel::Control;
use super::*;
use crate::transparency::{Key, Layer, Support};
use serde_json::Value;

impl DesktopHarness {
    /// A focused window on an [`ECHO`] tab showing `READY`.
    fn panel_window(&mut self) -> (AnyWindowHandle, TabId) {
        let tab = self.open_tab(ECHO);
        let window = self.attach_window();
        self.activate(window, tab);
        self.wait_text(window, tab, "READY");
        (window, tab)
    }

    /// Runs `window_transparency` and waits for the panel to paint.
    fn open_panel(&mut self, window: AnyWindowHandle) {
        self.invoke(window, ids::WINDOW_TRANSPARENCY).unwrap();
        self.wait_for("the panel to paint", |cx| panel_painted(cx, window));
    }

    /// Whether the panel is open, and its focused control.
    fn panel_state(
        &mut self,
        window: AnyWindowHandle,
    ) -> (bool, Option<Control>) {
        self.update_view(window, |view, window, cx| {
            (view.panel.is_some(), view.panel_focus(window, cx))
        })
    }

    fn overrides(&mut self, window: AnyWindowHandle) -> Layer {
        self.update_view(window, |view, _, cx| view.own_transparency(cx))
    }
}

fn panel_painted(cx: &mut App, window: AnyWindowHandle) -> bool {
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>().is_ok_and(|view| {
                let view = view.read(cx);
                view.panel.is_some()
                    && view.panel_popover.content_bounds().is_some()
            })
        })
        .unwrap_or(false)
}

/// The node `window` last drew with `role` and `label`, if any.
fn labelled(
    cx: &mut App,
    window: AnyWindowHandle,
    role: &str,
    label: &str,
) -> Option<Value> {
    let json = window
        .update(cx, |_, window, _| window.debug_a11y_tree_json())
        .unwrap()
        .expect("test windows report an accessibility tree for every frame");
    let tree: Value = serde_json::from_str(&json).unwrap();
    tree["nodes"]
        .as_object()
        .unwrap()
        .values()
        .find(|node| {
            node["aria"]["role"] == role && node["aria"]["label"] == label
        })
        .cloned()
}

fn bounds_of(
    harness: &DesktopHarness,
    window: AnyWindowHandle,
    selector: &'static str,
) -> Option<Bounds<Pixels>> {
    VisualTestContext::from_window(window, &harness.cx).debug_bounds(selector)
}

#[gpui::test]
fn the_command_opens_the_panel_on_its_first_slider_and_escape_closes_it(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.window.opacity = 0.6;
    });
    let (window, tab) = harness.panel_window();
    harness.open_panel(window);
    assert_eq!(
        harness.panel_state(window),
        (true, Some(Control::Slider(Key::Opacity)))
    );
    harness.cx.update(|cx| {
        assert!(labelled(cx, window, "Group", "Window Transparency").is_some());
        let slider = labelled(cx, window, "Slider", "Opacity").unwrap();
        assert_eq!(slider["aria"]["value"], "60%", "{slider}");
        assert!(labelled(cx, window, "Button", "Close").is_some());
        assert!(
            labelled(cx, window, "Button", "Edit Defaults in Settings\u{2026}")
                .is_some()
        );
        // No override means no Reset.
        assert!(labelled(cx, window, "Button", "Reset Opacity").is_none());
    });
    assert_eq!(
        bounds_of(&harness, window, "transparency-panel-row-blur").is_some(),
        Platform::current() == Platform::MacOs,
        "only macOS shows the Blur row"
    );
    harness.press(window, "escape");
    assert_eq!(harness.panel_state(window), (false, None));
    harness.cx.update(|cx| assert!(focused(cx, window, tab)));
    // The command again closes it too.
    harness.open_panel(window);
    harness.invoke(window, ids::WINDOW_TRANSPARENCY).unwrap();
    assert!(!harness.panel_state(window).0);
    harness.cx.update(|cx| assert!(focused(cx, window, tab)));
}

#[gpui::test]
fn panel_keys_step_reset_and_move_through_its_controls(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.window.opacity = 0.6;
    });
    let (window, _) = harness.panel_window();
    harness.update_view(window, |view, window, cx| {
        view.transparency_support = Support::Available;
        view.refresh_transparency(window, cx);
    });
    harness.open_panel(window);
    let opacity = |harness: &mut DesktopHarness, keys: &str| {
        harness.press(window, keys);
        harness.overrides(window).opacity
    };
    for (keys, expected) in [
        ("up", 0.65),
        ("right", 0.7),
        ("down", 0.65),
        ("left", 0.6),
        ("pageup", 0.7),
        ("pagedown", 0.6),
        ("end", 1.0),
        ("home", 0.1),
    ] {
        let value = opacity(&mut harness, keys).unwrap();
        assert!((value - expected).abs() < 1e-6, "{keys}: {value}");
    }
    let reset = match Platform::current() {
        Platform::MacOs => "cmd-backspace",
        Platform::Linux => "ctrl-backspace",
    };
    assert_eq!(opacity(&mut harness, reset), None, "Reset clears it");
    // Tab reaches the Reset of an overridden row, and pressing it clears
    // the override and keeps the keyboard on its slider.
    opacity(&mut harness, "up");
    harness.press(window, "tab");
    assert_eq!(
        harness.panel_state(window).1,
        Some(Control::Reset(Key::Opacity))
    );
    harness.press(window, "enter");
    assert_eq!(harness.overrides(window).opacity, None);
    assert_eq!(
        harness.panel_state(window).1,
        Some(Control::Slider(Key::Opacity))
    );
    harness.press(window, "tab");
    assert_eq!(
        harness.panel_state(window).1,
        Some(Control::Slider(Key::Background))
    );
    harness.press(window, "shift-tab shift-tab");
    assert_eq!(harness.panel_state(window).1, Some(Control::Close));
    // Enter and Space press only buttons, and Space presses Close.
    harness.press(window, "space");
    assert_eq!(harness.panel_state(window), (false, None));
}

#[gpui::test]
fn a_user_binding_reaches_the_panel(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.window.opacity = 0.6;
        config.keybindings.push(huterm_config::KeybindingEntry {
            key: "ctrl-u".to_owned(),
            command: "panel_increase".to_owned(),
            args: None,
            when: None,
            description: None,
            disabled: None,
        });
    });
    let (window, _) = harness.panel_window();
    harness.update_view(window, |view, window, cx| {
        view.transparency_support = Support::Available;
        view.refresh_transparency(window, cx);
    });
    harness.open_panel(window);
    harness.press(window, "ctrl-u");
    let value = harness.overrides(window).opacity.unwrap();
    assert!((value - 0.65).abs() < 1e-6, "{value}");
}

#[gpui::test]
fn outside_presses_deactivation_and_other_overlays_close_the_panel(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, tab) = harness.panel_window();
    let outside = harness.update_view(window, |view, window, _| {
        view.chrome_layout(window).terminal.origin + point(px(20.0), px(60.0))
    });
    harness.open_panel(window);
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_click(outside, gpui::Modifiers::none());
    assert!(!harness.panel_state(window).0, "an outside press");
    harness.cx.update(|cx| assert!(focused(cx, window, tab)));
    // A press before the panel's first frame closes it too.
    harness.update_view(window, |view, window, cx| {
        view.toggle_panel(window, cx).unwrap();
    });
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_click(outside, gpui::Modifiers::none());
    assert!(!harness.panel_state(window).0, "a stale-frame press");
    harness.open_panel(window);
    harness.deactivate(window);
    assert!(!harness.panel_state(window).0, "deactivation");
    harness.activate(window, tab);
    for (command, overlay) in [
        (ids::OPEN_MENU, "a menu"),
        (ids::OPEN_COMMAND_PALETTE, "the palette"),
    ] {
        harness.open_panel(window);
        harness.invoke(window, command).unwrap();
        assert!(!harness.panel_state(window).0, "{overlay}");
        harness.press(window, "escape");
        harness.wait_for("the overlay to close", |cx| focused(cx, window, tab));
    }
    // Opening the panel closes an open menu.
    harness.invoke(window, ids::OPEN_MENU).unwrap();
    harness.open_panel(window);
    assert!(harness.update_view(window, |view, _, _| view.menu.is_none()));
}

#[gpui::test]
fn closing_returns_focus_to_the_tabs_focused_pane_after_a_pane_closed(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, left, right) = harness.split_window(tab);
    harness.update_view(window, |view, window, cx| {
        view.focus_pane(tab, right, window, cx);
    });
    harness.open_panel(window);
    harness
        .runtime
        .lock()
        .close_pane(harness.workspace, tab, right)
        .unwrap();
    harness.wait_for("the closed pane's view to go", |cx| {
        pane_view(cx, window, tab, right).is_none()
    });
    assert!(harness.panel_state(window).0);
    harness.press(window, "escape");
    harness.cx.update(|cx| {
        let view = pane_view(cx, window, tab, left).unwrap();
        assert!(
            window
                .update(cx, |_, window, cx| view
                    .read(cx)
                    .focus
                    .is_focused(window))
                .unwrap(),
            "focus returns to the remaining pane"
        );
    });
}

#[gpui::test]
fn without_support_every_slider_disables_and_the_note_shows(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, _) = harness.panel_window();
    harness.update_view(window, |view, window, cx| {
        view.transparency_support = Support::NoCompositor;
        view.refresh_transparency(window, cx);
    });
    harness.open_panel(window);
    assert_eq!(harness.panel_state(window).1, Some(Control::Close));
    assert!(bounds_of(&harness, window, "transparency-panel-note").is_some());
    harness.cx.update(|cx| {
        let slider = labelled(cx, window, "Slider", "Opacity").unwrap();
        assert_eq!(
            slider["aria"]["description"],
            Support::NoCompositor.note().unwrap(),
            "{slider}"
        );
        assert!(slider.to_string().contains("disabled"), "{slider}");
    });
    // Nothing but Close and the Settings link takes focus.
    harness.press(window, "tab");
    assert_eq!(harness.panel_state(window).1, Some(Control::Settings));
    harness.press(window, "tab");
    assert_eq!(harness.panel_state(window).1, Some(Control::Close));
}

#[gpui::test]
fn reset_all_enables_with_an_override_and_clears_every_value(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.window.opacity = 0.6;
        config.window.background_opacity = 0.6;
    });
    let (window, _) = harness.panel_window();
    harness.update_view(window, |view, window, cx| {
        view.transparency_support = Support::Available;
        view.refresh_transparency(window, cx);
    });
    harness.open_panel(window);
    let reset_all = |harness: &mut DesktopHarness| {
        harness
            .cx
            .update(|cx| labelled(cx, window, "Button", "Reset All").unwrap())
    };
    assert!(reset_all(&mut harness).to_string().contains("disabled"));
    harness.press(window, "up tab tab up");
    assert!(harness.overrides(window).opacity.is_some());
    assert!(harness.overrides(window).background.is_some());
    harness.cx.update(|cx| {
        assert!(labelled(cx, window, "Button", "Reset Opacity").is_some());
    });
    let button = reset_all(&mut harness);
    assert!(!button.to_string().contains("disabled"), "{button}");
    let at = bounds_of(&harness, window, "transparency-panel-reset-all")
        .unwrap()
        .center();
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_click(at, gpui::Modifiers::none());
    assert!(harness.overrides(window).is_empty());
    assert_eq!(
        harness.panel_state(window),
        (true, Some(Control::Slider(Key::Opacity)))
    );
}

#[gpui::test]
fn a_reload_keeps_the_panel_open_and_refreshes_its_rows(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, _) = harness.panel_window();
    harness.open_panel(window);
    harness.update_view(window, |view, window, cx| {
        let mut config = view.config.clone();
        config.window.opacity = 0.5;
        let loaded = (config, view.family.clone(), view.metrics);
        view.apply_reload(Some(&loaded), &[], window, cx);
        let opacity = view
            .panel_rows(cx)
            .into_iter()
            .find(|row| row.key == Key::Opacity)
            .unwrap();
        assert!((opacity.value - 0.5).abs() < 1e-6);
    });
    harness.cx.run_until_parked();
    assert_eq!(
        harness.panel_state(window),
        (true, Some(Control::Slider(Key::Opacity)))
    );
    harness.cx.update(|cx| {
        let slider = labelled(cx, window, "Slider", "Opacity").unwrap();
        assert_eq!(slider["aria"]["value"], "50%", "{slider}");
    });
}

#[gpui::test]
fn a_short_window_scrolls_the_rows_and_keeps_the_footer(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, _) = harness.panel_window();
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_resize(size(px(480.0), px(180.0)));
    harness.open_panel(window);
    let viewport =
        harness.update_view(window, |_, window, _| window.viewport_size());
    let panel = bounds_of(&harness, window, "transparency-panel").unwrap();
    let footer =
        bounds_of(&harness, window, "transparency-panel-settings").unwrap();
    assert!(
        panel.bottom() <= viewport.height,
        "{panel:?} in {viewport:?}"
    );
    assert!(
        footer.bottom() <= panel.bottom() && footer.top() >= panel.top(),
        "the footer {footer:?} stays inside the panel {panel:?}"
    );
}
