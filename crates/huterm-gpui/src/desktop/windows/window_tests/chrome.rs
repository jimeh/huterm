//! Window chrome geometry and painted target outlines.

use huterm_config::TabStyle;

use super::*;

fn bounds(
    harness: &DesktopHarness,
    window: AnyWindowHandle,
    selector: &'static str,
) -> Option<Bounds<Pixels>> {
    VisualTestContext::from_window(window, &harness.cx).debug_bounds(selector)
}

#[gpui::test]
fn the_pane_top_border_tracks_a_tabless_decorated_linux_row(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.position = TabPosition::Titlebar;
    });
    let tab = harness.open_tab(ECHO);
    let first = harness.first_pane(tab);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.update_view(window, |view, window, cx| {
        view.decorations_override = Some(gpui::Decorations::Client {
            tiling: gpui::Tiling::default(),
        });
        view.sync_frame(window);
        view.refresh_tab_visibility(window, cx);
        cx.notify();
    });
    harness.cx.run_until_parked();
    assert!(bounds(&harness, window, "pane-area-top-border").is_none());
    harness.split(tab, first, SplitDirection::Right, ECHO);
    harness.wait_for("the split and client frame", |cx| {
        window
            .update(cx, |root, _, cx| {
                let view = root.downcast::<WorkspaceView>().unwrap();
                let view = view.read(cx);
                view.window_frame().decorated()
                    && view
                        .drawn_panes
                        .borrow()
                        .as_ref()
                        .is_some_and(|(_, panes)| panes.len() == 2)
            })
            .unwrap_or(false)
    });
    let layout = harness
        .update_view(window, |view, window, _| view.chrome_layout(window));
    assert!(layout.content.origin.y > px(0.0));
    let border = bounds(&harness, window, "pane-area-top-border")
        .expect("split outline below the client row");
    assert_eq!(
        border,
        Bounds::new(
            point(layout.terminal.origin.x, layout.terminal.origin.y - px(1.0)),
            size(layout.terminal.size.width, px(1.0))
        )
    );
    harness.activate(window, tab);
    harness.invoke(window, ids::TOGGLE_PANE_ZOOM).unwrap();
    harness.cx.run_until_parked();
    let zoomed = harness
        .update_view(window, |view, window, _| view.chrome_layout(window));
    assert_eq!(
        bounds(&harness, window, "pane-area-top-border"),
        Some(zoomed.top_chrome_border(false))
    );
    // Showing the merged tabs replaces this line with the merged row's own.
    harness.update_view(window, |view, _, cx| {
        view.config.tabs.always_show = true;
        cx.notify();
    });
    harness.cx.run_until_parked();
    assert!(bounds(&harness, window, "pane-area-top-border").is_none());
}

#[gpui::test]
#[cfg(target_os = "macos")]
fn pane_top_border_spans_only_panes_or_a_flush_active_strip_row(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let first = harness.first_pane(tab);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.split(tab, first, SplitDirection::Right, ECHO);
    harness.wait_for("two drawn panes", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>()
                    .unwrap()
                    .read(cx)
                    .drawn_panes
                    .borrow()
                    .as_ref()
                    .is_some_and(|(_, panes)| panes.len() == 2)
            })
            .unwrap_or(false)
    });
    for position in [
        TabPosition::Top,
        TabPosition::Bottom,
        TabPosition::Left,
        TabPosition::Right,
        TabPosition::Titlebar,
    ] {
        for style in [TabStyle::Pill, TabStyle::Strip] {
            harness.update_view(window, |view, _, cx| {
                view.config.tabs.position = position;
                view.config.tabs.style = style;
                view.config.tabs.always_show = true;
                cx.notify();
            });
            harness.cx.run_until_parked();
            let layout = harness.update_view(window, |view, window, _| {
                view.chrome_layout(window)
            });
            let border = bounds(&harness, window, "pane-area-top-border");
            if matches!(position, TabPosition::Top | TabPosition::Titlebar) {
                assert!(border.is_none(), "{position:?} {style:?}");
            } else {
                let span = position.vertical() && style == TabStyle::Strip;
                assert_eq!(
                    border,
                    Some(layout.top_chrome_border(span)),
                    "{position:?} {style:?}"
                );
            }
        }
    }
}

#[gpui::test]
fn targeted_pills_outline_the_pill_bounds_in_every_position(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
        config.tabs.style = TabStyle::Pill;
        config.tabs.pill_accent = true;
    });
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    let pill = format!("tab-pill-{}", tab.get()).leak();
    let outline = format!("tab-pill-target-{}", tab.get()).leak();
    for position in [
        TabPosition::Titlebar,
        TabPosition::Top,
        TabPosition::Bottom,
        TabPosition::Left,
        TabPosition::Right,
    ] {
        harness.update_view(window, |view, _, cx| {
            view.config.tabs.position = position;
            // Make Titlebar a real merged row on every test host.
            view.decorations_override = (position == TabPosition::Titlebar)
                .then_some(gpui::Decorations::Client {
                    tiling: gpui::Tiling::default(),
                });
            cx.notify();
        });
        harness.cx.run_until_parked();
        let pill_bounds = bounds(&harness, window, pill).expect("pill painted");
        VisualTestContext::from_window(window, &harness.cx).simulate_event(
            gpui::MouseDownEvent {
                button: MouseButton::Right,
                position: pill_bounds.center(),
                modifiers: gpui::Modifiers::none(),
                click_count: 1,
                first_mouse: false,
            },
        );
        harness.cx.run_until_parked();
        assert_eq!(
            bounds(&harness, window, outline),
            Some(pill_bounds),
            "{position:?}"
        );
        harness.press(window, "escape");
        assert!(bounds(&harness, window, outline).is_none());
    }
}

#[gpui::test]
fn targeted_strip_tabs_keep_their_shell_outline(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.tabs.always_show = true;
        config.tabs.style = TabStyle::Strip;
    });
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.activate(window, tab);
    let shell = format!("tab-shell-{}", tab.get()).leak();
    let outline = format!("tab-strip-target-{}", tab.get()).leak();
    let shell_bounds =
        bounds(&harness, window, shell).expect("strip tab painted");
    VisualTestContext::from_window(window, &harness.cx).simulate_event(
        gpui::MouseDownEvent {
            button: MouseButton::Right,
            position: shell_bounds.center(),
            modifiers: gpui::Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        },
    );
    harness.cx.run_until_parked();
    let inset = px(1.0);
    assert_eq!(
        bounds(&harness, window, outline),
        Some(Bounds::new(
            shell_bounds.origin + point(inset, inset),
            size(
                shell_bounds.size.width - inset * 2.0,
                shell_bounds.size.height - inset * 2.0
            ),
        ))
    );
    // Keyboard navigation keeps the same target until dismissal.
    harness.press(window, "down");
    assert!(bounds(&harness, window, outline).is_some());
    harness.press(window, "escape");
    assert!(bounds(&harness, window, outline).is_none());
}
