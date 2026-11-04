//! Live transparency reload across retained terminal views.
use super::*;

#[test]
fn workspace_root_and_client_frame_fill_only_opaque_backgrounds() {
    let theme = Config::default().theme;
    let bounds =
        Bounds::new(point(px(0.0), px(0.0)), size(px(800.0), px(600.0)));
    for (alpha, opaque) in
        [(1.0, true), (0.999, false), (0.6, false), (0.0, false)]
    {
        let background = color(theme.background).opacity(alpha);
        for decorated in [false, true] {
            let mut root = workspace_root(decorated, background);
            let mut panel =
                client_frame_panel(bounds, background, tokens(&theme));
            assert_eq!(
                root.style().background.is_some(),
                !decorated && opaque,
                "workspace root fill at alpha {alpha}, decorated={decorated}"
            );
            assert_eq!(
                panel.style().background.is_some(),
                opaque,
                "client frame panel fill at alpha {alpha}"
            );
            for element in [&mut root, &mut panel] {
                if let Some(fill) = &element.style().background {
                    assert_eq!(
                        fill.color().unwrap().as_solid().unwrap(),
                        background
                    );
                }
            }
        }
    }
}

#[gpui::test]
fn reload_changes_surface_and_paint_both_directions_without_replacing_terminals(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    let terminal_before = harness.cx.update(|cx| {
        terminal(cx, window, tab)
            .unwrap()
            .read(cx)
            .viewer
            .terminal_id()
    });
    for background in [0.6, 1.0] {
        harness.update_view(window, |view, window, cx| {
            let mut config = view.config.clone();
            config.window.background_opacity = background;
            config.window.foreground_opacity = 0.8;
            config.window.background_blur = true;
            let loaded = (config, view.family.clone(), view.metrics);
            view.apply_reload(Some(&loaded), &[], window, cx);
            let expected = crate::transparency::appearance(
                Platform::current(),
                view.window_frame().decorated(),
                view.transparency_support,
                background,
                true,
            );
            assert_eq!(view.background_appearance, Some(expected));
            for tab in &view.tabs {
                for pane in &tab.panes {
                    assert_eq!(
                        pane.view.read(cx).renderer.borrow().background(None).a,
                        view.transparency_support.paint(loaded.0.window).0
                    );
                }
            }
        });
    }
    let terminal_after = harness.cx.update(|cx| {
        terminal(cx, window, tab)
            .unwrap()
            .read(cx)
            .viewer
            .terminal_id()
    });
    assert_eq!(terminal_before, terminal_after);
    harness.wait_text(window, tab, "READY");
}

#[gpui::test]
fn chrome_fills_and_dimming_preserve_background_alpha(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.window.background_opacity = 0.6;
        config.panes.inactive_dim_background = 0.3;
    });
    let tab = harness.open_tab(ECHO);
    let window = harness.attach_window();
    harness.wait_text(window, tab, "READY");
    harness.update_view(window, |view, _, cx| {
        // Keep this paint-policy test independent of the host compositor.
        view.transparency_support = crate::transparency::Support::Available;
        let colors = view.tab_colors();
        assert_eq!(colors.bar.a, 0.6);
        assert_eq!(colors.active.a, 0.6);
        assert_eq!(colors.terminal.a, 0.6);
        assert_eq!(colors.foreground.a, 1.0);
        assert_eq!(colors.border.a, 1.0);
        let floating = colors.for_floating(true);
        assert_eq!(
            (floating.bar.a, floating.active.a, floating.terminal.a),
            (1.0, 1.0, 1.0)
        );
        let pane = view.tabs[0].panes[0].pane;
        let terminal = view.tabs[0].panes[0].view.clone();
        view.configure_pane_paint(&terminal, false, 2, cx);
        let dimmed = terminal.read(cx).renderer.borrow().background(None);
        assert_eq!(dimmed.a, 0.6);
        assert_ne!(
            dimmed,
            color(view.config.theme.background).opacity(0.6),
            "inactive_dim_background must tint the unfocused background"
        );
        assert_eq!(
            view.dim_layers(pane, PaneId::new(u64::MAX), 2, colors)
                .count(),
            0
        );
        let mut bar = WorkspaceView::pane_title_bar(
            tab_bar::PaneTitleBar {
                tab,
                pane,
                bounds: Bounds::new(
                    point(px(0.0), px(0.0)),
                    size(px(300.0), px(24.0)),
                ),
                title: "title".into(),
                status: TabStatus::Idle,
                focused: true,
                zoomed: false,
                lone: true,
                input_sync: false,
                sync_dimmed: false,
                tint: Some(gpui::white().opacity(0.3)),
                bottom: false,
            },
            colors,
            tokens(&view.config.theme),
            cx,
        );
        let fill = bar
            .style()
            .background
            .as_ref()
            .unwrap()
            .color()
            .unwrap()
            .as_solid()
            .unwrap();
        assert_eq!(
            fill.a, 0.6,
            "pane title tint must not add a background layer"
        );
        view.config.window.background_opacity = 1.0;
        view.configure_pane_paint(&terminal, false, 2, cx);
        assert_eq!(
            view.dim_layers(pane, PaneId::new(u64::MAX), 2, view.tab_colors())
                .count(),
            2,
            "opaque dimming retains both overlays"
        );
    });
}

/// Read the production scene without a native window or GPU rasterization.
fn painted_at(
    harness: &mut DesktopHarness,
    handle: AnyWindowHandle,
    at: gpui::Point<Pixels>,
    color: gpui::Hsla,
) -> bool {
    harness.cx.update(|cx| {
        handle
            .update(cx, |_, window, cx| {
                let _ = window.draw(cx);
                let at = at.scale(window.scale_factor());
                window.painted_quads().iter().any(|quad| {
                    quad.bounds.contains(&at)
                        && quad.content_mask.bounds.contains(&at)
                        && quad.background.as_solid() == Some(color)
                })
            })
            .unwrap()
    })
}

#[gpui::test]
fn overflowing_strip_keeps_background_under_end_controls(
    cx: &mut TestAppContext,
) {
    for position in [TabPosition::Top, TabPosition::Left] {
        let mut harness = DesktopHarness::with_config(cx, |config| {
            config.window.background_opacity = 0.6;
            config.tabs.style = TabStyle::Strip;
            config.tabs.position = position;
        });
        for _ in 0..20 {
            harness.open_tab(ECHO);
        }
        let window = harness.attach_window();
        harness.wait_for("the window to show every tab", |cx| {
            window
                .update(cx, |root, _, cx| {
                    root.downcast::<WorkspaceView>()
                        .is_ok_and(|view| view.read(cx).tabs.len() == 20)
                })
                .unwrap_or(false)
        });
        // The first tab that overflows, far enough in to scroll either just
        // inside or just past the strip's end.
        let active = harness.update_view(window, |view, window, cx| {
            let strip = view.tab_strip(window);
            let index = (0..strip.count)
                .find(|&index| {
                    strip.start(index) >= strip.available() + px(8.0)
                })
                .expect("the tabs overflow the strip");
            let active = view.tabs[index].id;
            view.select_tab(active, true, window, cx);
            active
        });
        // The end controls follow the strip: the new-tab button in a
        // horizontal bar, and the new-tab row below a vertical column.
        for (label, past_end) in [("partly", px(-8.0)), ("wholly", px(2.0))] {
            let (at, bar) = harness.update_view(window, |view, window, cx| {
                // Keep this paint-policy test independent of the host
                // compositor.
                view.transparency_support =
                    crate::transparency::Support::Available;
                let index =
                    view.tabs.iter().position(|tab| tab.id == active).unwrap();
                let strip = view.tab_strip(window);
                view.tab_scroll =
                    strip.start(index) - strip.available() - past_end;
                view.scroll_target = None;
                let strip = view.tab_strip(window);
                assert_eq!(
                    strip.start(index) - strip.offset,
                    strip.available() + past_end,
                    "{position:?}: the active tab starts {label} past the end"
                );
                let at = if strip.vertical {
                    point(
                        strip.bounds.left() + px(4.0),
                        strip.bounds.bottom() + px(4.0),
                    )
                } else {
                    point(
                        strip.bounds.right() + px(4.0),
                        strip.bounds.top() + px(4.0),
                    )
                };
                cx.notify();
                (at, view.tab_colors().bar)
            });
            assert!(
                painted_at(&mut harness, window, at, bar),
                "{position:?}: an active tab {label} past the strip's end cut \
                 a hole in the bar under its end controls"
            );
        }
    }
}

#[gpui::test]
fn fullscreen_overlay_keeps_pinned_pane_bars_translucent(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.window.background_opacity = 0.6;
        config.tabs.auto_hide_in_fullscreen = true;
        config.panes.title_bar = huterm_config::PaneTitleBarMode::Visible;
    });
    let tab = harness.open_tab(ECHO);
    let (window, _, _) = harness.split_window(tab);
    harness.update_view(window, |view, window, cx| {
        view.transparency_support = crate::transparency::Support::Available;
        window.toggle_fullscreen();
        view.refresh_fullscreen(window, cx);
        assert_eq!(view.presentation(), Presentation::Overlay);
        assert_eq!(view.reveal.progress, 0.0);
        cx.notify();
    });
    let (at, color) = harness.update_view(window, |view, window, _| {
        let layout = view.chrome_layout(window);
        (
            layout.terminal.origin + point(px(4.0), px(4.0)),
            TabColors::new(&view.config.theme)
                .background_opacity(0.6)
                .active,
        )
    });
    assert!(
        painted_at(&mut harness, window, at, color),
        "pinned pane title bar must keep configured alpha while fullscreen tabs are hidden"
    );
    harness.update_view(window, |view, _, _| {
        let floating = view.tab_colors().for_floating(true);
        assert_eq!(
            (floating.bar.a, floating.active.a, floating.terminal.a),
            (1.0, 1.0, 1.0)
        );
    });
}

#[test]
fn translucent_client_frame_shadows_only_the_ring_outside_its_content() {
    let theme = Config::default().theme;
    let content =
        Bounds::new(point(px(10.0), px(10.0)), size(px(800.0), px(600.0)));
    let panel_shadow = |alpha: f32| {
        let mut panel = client_frame_panel(
            content,
            color(theme.background).opacity(alpha),
            tokens(&theme),
        );
        panel.style().box_shadow.clone().unwrap_or_default()
    };
    assert_eq!(panel_shadow(1.0), tokens(&theme).frame_shadow());
    for alpha in [0.0, 0.6, 0.999] {
        assert!(
            panel_shadow(alpha).is_empty(),
            "alpha {alpha}: the panel's own shadow fills its interior"
        );
    }
    let masks = frame_shadow_masks(content);
    let ring = content.dilate(client_frame::CLIENT_INSET);
    for (x, y) in [
        (5.0, 5.0),
        (400.0, 5.0),
        (815.0, 300.0),
        (400.0, 615.0),
        (5.0, 300.0),
        (12.0, 12.0),
        (400.0, 300.0),
        (809.0, 609.0),
    ] {
        let sample = point(px(x), px(y));
        let inside = content.contains(&sample);
        let masked = masks.iter().filter(|mask| mask.contains(&sample)).count();
        assert_eq!(
            masked,
            usize::from(!inside && ring.contains(&sample)),
            "shadow masks at ({x}, {y})"
        );
    }
}
