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
        assert_eq!(terminal.read(cx).renderer.borrow().background(None).a, 0.6);
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
