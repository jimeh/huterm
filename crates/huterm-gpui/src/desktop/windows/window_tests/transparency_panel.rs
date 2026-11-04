//! The Window Transparency panel: opening and closing, real key dispatch,
//! focus return, outside presses, and its states.
use super::super::transparency_panel::{Control, keys};
use super::settings::{ScratchConfig, settings, settled, with_view};
use super::*;
use crate::transparency::{Key, Layer, Support};
use huterm_config::settings::SectionId;
use huterm_ui::components::scroll_area::REVEAL_CLEARANCE;
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

#[gpui::test]
fn each_slider_describes_where_its_value_comes_from(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.window.opacity = 0.6;
    });
    let (window, _) = harness.panel_window();
    harness.update_view(window, |view, window, cx| {
        view.transparency_support = Support::Available;
        view.refresh_transparency(window, cx);
    });
    harness.open_panel(window);
    let described = |harness: &mut DesktopHarness| {
        harness.cx.update(|cx| {
            let slider = labelled(cx, window, "Slider", "Opacity").unwrap();
            slider["aria"]["description"].clone()
        })
    };
    assert_eq!(described(&mut harness), "Uses the default");
    harness.press(window, "up");
    assert_eq!(described(&mut harness), "Set for this window");
    // A reload keeps the override and its description.
    harness.update_view(window, |view, window, cx| {
        let mut config = view.config.clone();
        config.window.opacity = 0.5;
        let loaded = (config, view.family.clone(), view.metrics);
        view.apply_reload(Some(&loaded), &[], window, cx);
    });
    harness.cx.run_until_parked();
    assert_eq!(described(&mut harness), "Set for this window");
    let reset = match Platform::current() {
        Platform::MacOs => "cmd-backspace",
        Platform::Linux => "ctrl-backspace",
    };
    harness.press(window, reset);
    assert_eq!(described(&mut harness), "Uses the default");
}

/// The row of the panel's focused control, if it sits in one, and whether
/// that row shows whole in the panel's body.
fn focused_row_shown(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> Option<(Key, bool)> {
    let key = match harness.panel_state(window).1? {
        Control::Slider(key) | Control::Reset(key) => key,
        Control::Close | Control::ResetAll | Control::Settings => return None,
    };
    let viewport = harness
        .update_view(window, |view, _, _| view.panel_viewport())
        .unwrap();
    let selector = match key {
        Key::Opacity => "transparency-panel-row-opacity",
        Key::Background => "transparency-panel-row-background_opacity",
        Key::Foreground => "transparency-panel-row-foreground_opacity",
        Key::Blur => "transparency-panel-row-blur",
    };
    let row = bounds_of(harness, window, selector).unwrap();
    Some((
        key,
        row.top() >= viewport.top() && row.bottom() <= viewport.bottom(),
    ))
}

#[gpui::test]
fn keyboard_focus_scrolls_a_short_panel_to_show_the_focused_row(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let (window, _) = harness.panel_window();
    harness.update_view(window, |view, window, cx| {
        view.transparency_support = Support::Available;
        view.refresh_transparency(window, cx);
    });
    // The reviewer's window, which shows only the first two rows.
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_resize(size(px(408.0), px(233.0)));
    harness.open_panel(window);
    let last = *keys(Platform::current()).last().unwrap();
    assert_eq!(
        focused_row_shown(&mut harness, window),
        Some((Key::Opacity, true))
    );
    let mut steps = 0;
    while harness.panel_state(window).1 != Some(Control::Slider(last)) {
        harness.press(window, "tab");
        if let Some((key, shown)) = focused_row_shown(&mut harness, window) {
            assert!(shown, "Tab left the {key:?} row out of view");
        }
        steps += 1;
        assert!(steps < 10, "Tab never reached the {last:?} slider");
    }
    let scrolled = |harness: &mut DesktopHarness| {
        let viewport = harness
            .update_view(window, |view, _, _| view.panel_viewport())
            .unwrap();
        bounds_of(harness, window, "transparency-panel-row-opacity")
            .unwrap()
            .top()
            < viewport.top()
    };
    assert!(scrolled(&mut harness), "the body never scrolled");
    // Shift-Tab back up reveals each row above the fold.
    while harness.panel_state(window).1 != Some(Control::Slider(Key::Opacity)) {
        harness.press(window, "shift-tab");
        if let Some((key, shown)) = focused_row_shown(&mut harness, window) {
            assert!(shown, "Shift-Tab left the {key:?} row out of view");
        }
    }
    assert!(!scrolled(&mut harness), "the Opacity row shows");
    // Reset All, from the bottom row, returns focus to the top row and
    // shows it.
    harness.press(window, "shift-tab shift-tab");
    assert_eq!(harness.panel_state(window).1, Some(Control::Settings));
    harness.press(window, "shift-tab");
    assert_eq!(harness.panel_state(window).1, Some(Control::Slider(last)));
    // Off the bound the default sits at: Blur's 0, or Linux Foreground's 1.
    harness.press(window, if last == Key::Blur { "up" } else { "down" });
    harness.press(window, "tab");
    assert_eq!(harness.panel_state(window).1, Some(Control::Reset(last)));
    harness.press(window, "tab");
    assert_eq!(harness.panel_state(window).1, Some(Control::ResetAll));
    harness.press(window, "enter");
    assert_eq!(
        focused_row_shown(&mut harness, window),
        Some((Key::Opacity, true))
    );
}

/// Presses Edit Defaults in Settings from the keyboard and waits for
/// Settings to read its file and finish revealing the group.
fn edit_defaults(harness: &mut DesktopHarness, window: AnyWindowHandle) {
    harness.open_panel(window);
    // Shift-Tab from the first slider reaches Close, then wraps to the link.
    harness.press(window, "shift-tab shift-tab");
    assert_eq!(harness.panel_state(window).1, Some(Control::Settings));
    harness.press(window, "enter");
    assert!(!harness.panel_state(window).0, "the link closes the panel");
    harness.wait_for("Settings to reveal Transparency", |cx| {
        settled(cx)
            && settings(cx).is_some_and(|handle| {
                handle
                    .read_with(cx, |view, _| {
                        view.shown_section() == SectionId::Appearance
                            && !view.group_reveal_pending()
                    })
                    .unwrap_or(false)
            })
    });
}

/// Asserts that Appearance's Transparency card starts at the top of the
/// Settings pane, the reveal's clearance below it, or that the pane
/// scrolled as far as it can toward that.
fn assert_transparency_at_top(harness: &mut DesktopHarness, case: &str) {
    let handle = harness.cx.update(|cx| settings(cx).unwrap());
    let card = VisualTestContext::from_window(handle.into(), &harness.cx)
        .debug_bounds("settings-group-Transparency")
        .unwrap_or_else(|| panic!("{case}: the card painted"));
    let (viewport, scrolled, limit) = with_view(harness, |view, _| {
        (
            view.pane_viewport(),
            view.pane_scrolled(),
            view.pane_scroll_limit(),
        )
    });
    let gap = card.top() - viewport.top();
    assert!(
        scrolled > px(0.0),
        "{case}: the pane never scrolled; the card sits {gap:?} down"
    );
    assert!(
        (gap - REVEAL_CLEARANCE).abs() < px(0.5) || scrolled == limit,
        "{case}: the card sits {gap:?} below the pane's top, scrolled \
         {scrolled:?} of {limit:?}"
    );
}

#[gpui::test]
fn edit_defaults_shows_transparency_at_the_top_of_a_new_or_open_settings(
    cx: &mut TestAppContext,
) {
    let file = ScratchConfig::new("panel-edit-defaults", "");
    let mut harness = DesktopHarness::new(cx);
    harness.cx.update(|cx| {
        cx.global_mut::<Desktop>().config_path = file.path();
    });
    let (window, tab) = harness.panel_window();
    // A new Settings window reads its file and scans its themes after it
    // opens, which grows the theme gallery above the card.
    edit_defaults(&mut harness, window);
    assert_transparency_at_top(&mut harness, "a new window");
    // An open one, showing another section scrolled elsewhere.
    with_view(&mut harness, |view, cx| {
        view.show_section(SectionId::Terminal, cx);
    });
    harness.cx.run_until_parked();
    with_view(&mut harness, |view, _| view.scroll_pane_to(px(120.0)));
    harness.activate(window, tab);
    edit_defaults(&mut harness, window);
    assert_transparency_at_top(&mut harness, "an open window");
}
