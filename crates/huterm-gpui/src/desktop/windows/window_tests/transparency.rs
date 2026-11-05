//! Live transparency reload across retained terminal views.
use super::*;

#[test]
fn workspace_root_and_client_frame_fill_only_opaque_backgrounds() {
    let theme = Config::default().theme;
    let bounds =
        Bounds::new(point(px(0.0), px(0.0)), size(px(800.0), px(600.0)));
    // Blur mode is translucent with an opaque background, through the
    // root's opacity, so neither underlay may fill then.
    for (alpha, translucent) in [
        (1.0, false),
        (1.0, true),
        (0.999, true),
        (0.6, true),
        (0.0, true),
    ] {
        let opaque = !translucent;
        let background = color(theme.background).opacity(alpha);
        for decorated in [false, true] {
            let mut root = workspace_root(decorated, background, translucent);
            let mut panel = client_frame_panel(
                bounds,
                background,
                translucent,
                tokens(&theme),
            );
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
            config.window.blur = 20;
            let loaded = (config, view.family.clone(), view.metrics);
            view.apply_reload(Some(&loaded), &[], window, cx);
            let expected = crate::transparency::resolve(
                crate::transparency::Values::of(&loaded.0.window),
                view.transparency_support,
                Platform::current(),
                view.window_frame().decorated(),
            );
            assert_eq!(view.transparency, expected);
            assert_eq!(view.background_appearance, Some(expected.appearance));
            for tab in &view.tabs {
                for pane in &tab.panes {
                    assert_eq!(
                        pane.view.read(cx).renderer.borrow().background(None).a,
                        expected.background
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
    harness.update_view(window, |view, window, cx| {
        // Keep this paint-policy test independent of the host compositor.
        view.transparency_support = crate::transparency::Support::Available;
        view.refresh_transparency(window, cx);
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
        view.refresh_transparency(window, cx);
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
pub(super) fn painted_at(
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
        // Keep this paint-policy test independent of the host compositor.
        harness.support_transparency(window);
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
            alpha < 1.0,
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

impl DesktopHarness {
    /// Gives `window` support and resolves its values again, so the test
    /// does not depend on the host compositor. The desktop's cached support
    /// changes too, since a reload copies it into every window.
    pub(super) fn support_transparency(&mut self, window: AnyWindowHandle) {
        self.update_view(window, |view, window, cx| {
            let support = crate::transparency::Support::Available;
            cx.global_mut::<Desktop>().transparency_support = support;
            view.transparency_support = support;
            view.refresh_transparency(window, cx);
        });
    }

    /// Reloads `windows` with `change` applied to the configuration, as a
    /// global reload reaches every window.
    fn reload_transparency(
        &mut self,
        windows: &[AnyWindowHandle],
        change: impl Fn(&mut Config),
    ) {
        for window in windows {
            self.update_view(*window, |view, window, cx| {
                let mut config = view.config.clone();
                change(&mut config);
                cx.global_mut::<Desktop>().config = config.clone();
                let loaded = (config, view.family.clone(), view.metrics);
                view.apply_reload(Some(&loaded), &[], window, cx);
            });
        }
    }

    /// The background alpha each of `window`'s panes paints after a frame.
    pub(super) fn painted_backgrounds(
        &mut self,
        window: AnyWindowHandle,
    ) -> Vec<f32> {
        self.cx.update(|cx| {
            window
                .update(cx, |root, window, cx| {
                    let _ = window.draw(cx);
                    let view = root.downcast::<WorkspaceView>().unwrap();
                    view.read(cx)
                        .tabs
                        .iter()
                        .flat_map(|tab| &tab.panes)
                        .map(|pane| {
                            pane.view
                                .read(cx)
                                .renderer
                                .borrow()
                                .background(None)
                                .a
                        })
                        .collect()
                })
                .unwrap()
        })
    }
}

/// Whether two colors share their red, green, and blue, whatever their
/// alpha.
fn same_rgb(left: gpui::Hsla, right: gpui::Hsla) -> bool {
    let (left, right): (gpui::Rgba, gpui::Rgba) = (left.into(), right.into());
    [left.r - right.r, left.g - right.g, left.b - right.b]
        .iter()
        .all(|delta| delta.abs() < 1e-3)
}

pub(super) fn close(left: f32, right: f32) -> bool {
    (left - right).abs() < 1e-6
}

#[gpui::test]
fn an_override_changes_one_window_and_a_reload_reaches_the_others(
    cx: &mut TestAppContext,
) {
    use crate::transparency::Key;
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.window.background_opacity = 0.6;
    });
    let tab = harness.open_tab(ECHO);
    let first = harness.attach_window();
    let second = harness.attach_window();
    for window in [first, second] {
        harness.wait_text(window, tab, "READY");
        harness.support_transparency(window);
    }
    harness.update_view(first, |view, window, cx| {
        view.set_transparency_override(Key::Background, Some(0.3), window, cx);
        view.set_transparency_override(Key::Opacity, Some(0.5), window, cx);
    });
    let state = |harness: &mut DesktopHarness, window| {
        let painted = harness.painted_backgrounds(window);
        let (background, opacity) =
            harness.update_view(window, |view, _, _| {
                (
                    view.transparency.background,
                    view.transparency_values.opacity,
                )
            });
        assert!(painted.iter().all(|alpha| close(*alpha, background)));
        (background, opacity)
    };
    assert_eq!(state(&mut harness, first), (0.3, 0.5));
    assert_eq!(state(&mut harness, second), (0.6, 1.0), "the other window");
    // A reload's new defaults reach every value a window does not
    // override.
    harness.reload_transparency(&[first, second], |config| {
        config.window.background_opacity = 0.4;
        config.window.opacity = 0.8;
    });
    assert_eq!(state(&mut harness, first), (0.3, 0.5));
    assert_eq!(state(&mut harness, second), (0.4, 0.8));
    // A reset value inherits the defaults again.
    harness.update_view(first, |view, window, cx| {
        view.set_transparency_override(Key::Background, None, window, cx);
    });
    assert_eq!(state(&mut harness, first), (0.4, 0.5));
}

/// The alphas of the quads `handle` paints at `at` whose background
/// `paints` selects, after drawing a frame. A gradient counts as opaque.
fn painted_alphas(
    harness: &mut DesktopHarness,
    handle: AnyWindowHandle,
    at: gpui::Point<Pixels>,
    paints: &dyn Fn(&gpui::Background) -> bool,
) -> Vec<f32> {
    harness.cx.update(|cx| {
        handle
            .update(cx, |_, window, cx| {
                let _ = window.draw(cx);
                let at = at.scale(window.scale_factor());
                window
                    .painted_quads()
                    .iter()
                    .filter(|quad| {
                        quad.bounds.contains(&at)
                            && quad.content_mask.bounds.contains(&at)
                            && paints(&quad.background)
                    })
                    .map(|quad| quad.background.as_solid().map_or(1.0, |c| c.a))
                    .collect()
            })
            .unwrap()
    })
}

#[gpui::test]
fn blur_mode_over_an_opaque_background_paints_one_faded_background(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::with_config(cx, |config| {
        config.window.opacity = 0.6;
        config.window.blur = 20;
        config.tabs.always_show = true;
        config.panes.inactive_dim = 0.3;
    });
    let tab = harness.open_tab(ECHO);
    let _ = harness.open_tab(ECHO);
    let (window, left, _) = harness.split_window(tab);
    harness.support_transparency(window);
    // The default sync look tints each receiving pane's title bar.
    harness
        .runtime
        .lock()
        .set_input_sync(harness.workspace, tab, true)
        .unwrap();
    let blurring = Platform::current() == Platform::MacOs;
    let (terminal_at, bar_at, title_at, background, bar) =
        harness.update_view(window, |view, window, cx| {
            assert_eq!(view.transparency.translucent, blurring);
            // No overlay dims an inactive pane under a faded root; it
            // tints instead, as a translucent background does.
            assert_eq!(
                view.dim_layers(
                    left,
                    PaneId::new(u64::MAX),
                    2,
                    view.tab_colors()
                )
                .count(),
                usize::from(!blurring)
            );
            // The sample lands in the focused pane, which no tint changes.
            view.focus_pane(tab, left, window, cx);
            let layout = view.chrome_layout(window);
            cx.notify();
            // The bottom padding of the pane, below its title bar.
            let frame = view.drawn_frame(left).expect("the pane was drawn");
            let title = view
                .drawn_panes
                .borrow()
                .as_ref()
                .and_then(|(_, rects)| {
                    rects.iter().find(|rect| rect.pane == left)?.title_bar
                })
                .expect("the pane's title bar was drawn");
            (
                point(frame.left() + px(2.0), frame.bottom() - px(1.0)),
                layout.tabs.origin + point(px(2.0), px(2.0)),
                // The bar's leading padding, before its sync mark.
                point(title.left() + px(2.0), title.center().y),
                color(view.config.theme.background),
                view.tab_colors().bar,
            )
        });
    // Linux has no blur mode: the opacity stays native alpha, and the
    // opaque underlays and overlays keep painting.
    if !blurring {
        return;
    }
    harness.wait_for("the sync tint on the title bar", |cx| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx)
                        .drawn_focus
                        .borrow()
                        .indicator
                        .tinted
                        .iter()
                        .any(|(pane, _)| *pane == left)
                })
            })
            .unwrap_or(false)
    });
    // The root's opacity multiplies every primitive, so each region has
    // exactly one background painter, at the opacity.
    for (label, at, fill) in [
        ("terminal padding", terminal_at, background),
        ("tab bar", bar_at, bar),
    ] {
        let found = painted_alphas(&mut harness, window, at, &|paint| {
            paint.as_solid().is_some_and(|solid| same_rgb(solid, fill))
        });
        assert_eq!(found.len(), 1, "{label}: {found:?}");
        assert!(close(found[0], 0.6), "{label}: {found:?}");
    }
    // A tinted title bar blends its tint into its one fill: whatever its
    // colour, a second quad there would stack over the bar.
    let found = painted_alphas(&mut harness, window, title_at, &|paint| {
        !paint.is_transparent()
    });
    assert_eq!(found.len(), 1, "tinted title bar: {found:?}");
    assert!(close(found[0], 0.6), "tinted title bar: {found:?}");
}
