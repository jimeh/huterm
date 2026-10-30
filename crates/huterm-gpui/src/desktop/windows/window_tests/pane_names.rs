//! Tier 2 tests for pane names and the pane title bar menu: a right press
//! on a bar focuses its pane and opens the menu, Rename Pane runs through
//! the palette, and pane, tab, and palette labels follow the name.

use super::*;

/// The open menu's kind and the pane it targets, in `window`.
fn pane_menu(cx: &mut App, window: AnyWindowHandle) -> Option<PaneId> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            view.read(cx).menu.as_ref()?.kind.pane()
        })
        .ok()
        .flatten()
}

/// What `window` labels `pane` of `tab` with, and the tab's own label.
fn labels(
    cx: &mut App,
    window: AnyWindowHandle,
    tab: TabId,
    pane: PaneId,
) -> Option<(String, String)> {
    window
        .update(cx, |root, _, cx| {
            let view = root.downcast::<WorkspaceView>().ok()?;
            let view = view.read(cx);
            let tabs = view.config.tabs;
            let tab = view.tabs.iter().find(|view| view.id == tab)?;
            Some((
                tab.pane_label(tab.pane(pane)?, tabs, cx),
                tab.title(tabs, cx),
            ))
        })
        .ok()
        .flatten()
}

impl DesktopHarness {
    /// A point on `pane`'s drawn title bar, clear of its buttons.
    fn title_bar_point(
        &mut self,
        window: AnyWindowHandle,
        pane: PaneId,
    ) -> gpui::Point<Pixels> {
        self.update_view(window, |view, _, _| {
            view.drawn_panes
                .borrow()
                .as_ref()
                .and_then(|(_, rects)| {
                    rects.iter().find(|rect| rect.pane == pane)?.title_bar
                })
                .map(|bar| gpui::point(bar.origin.x + px(40.0), bar.center().y))
        })
        .expect("a drawn title bar")
    }

    fn right_press_at(
        &mut self,
        window: AnyWindowHandle,
        at: gpui::Point<Pixels>,
    ) {
        let mut cx = VisualTestContext::from_window(window, &self.cx);
        cx.simulate_mouse_down(at, MouseButton::Right, gpui::Modifiers::none());
        cx.simulate_mouse_up(at, MouseButton::Right, gpui::Modifiers::none());
    }
}

#[gpui::test]
fn a_right_press_on_a_pane_bar_opens_its_menu_and_renames_the_pane(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, left, right) = harness.split_window(tab);
    harness.update_view(window, |view, window, cx| {
        view.focus_pane(tab, right, window, cx);
    });
    let (default_label, _) = harness
        .cx
        .update(|cx| labels(cx, window, tab, left))
        .unwrap();

    // The press focuses the unfocused pane and opens its menu.
    let bar = harness.title_bar_point(window, left);
    harness.right_press_at(window, bar);
    harness.wait_for("the pane menu to open on the left pane", |cx| {
        pane_menu(cx, window) == Some(left)
            && panes_shown(cx, window, tab)
                .is_some_and(|(_, focused)| focused == left)
    });

    // Rename Pane, the first row, opens the palette on the menu's pane.
    harness.press(window, "down enter");
    harness.wait_for("the rename palette", |cx| {
        palette_state(cx, window).is_some_and(|state| {
            state.contains("slots command=rename_pane")
                && state.contains("active=name")
                && state.contains("pane:explicit")
        })
    });
    harness.press(window, "l o g s enter");
    harness.wait_for("the pane bar and tab to show the name", |cx| {
        palette_state(cx, window).is_none()
            && labels(cx, window, tab, left)
                == Some(("logs".to_owned(), "logs".to_owned()))
    });
    harness.cx.update(|cx| {
        let (label, _) = labels(cx, window, tab, right).unwrap();
        assert_ne!(label, "logs", "only the renamed pane changes");
    });

    // A binding without a pane renames the focused one, and the palette
    // prefills its current name.
    harness.invoke(window, ids::RENAME_PANE).unwrap();
    harness.wait_for("the palette to prefill the name", |cx| {
        palette_state(cx, window).is_some_and(|state| {
            state.contains("slots command=rename_pane")
                && state.contains("pane:prefilled")
                && state.contains("input=\"logs\"")
        })
    });
    harness.press(window, "escape");

    // The tab's custom name wins over its focused pane's; a blank pane
    // name restores the pane's own label.
    harness
        .runtime
        .lock()
        .rename_tab(tab, Some("work"))
        .unwrap();
    harness.wait_for("the tab to show its own name", |cx| {
        labels(cx, window, tab, left)
            == Some(("logs".to_owned(), "work".to_owned()))
    });
    harness.runtime.lock().rename_pane(left, None).unwrap();
    harness.wait_for("the pane to show its own label again", |cx| {
        labels(cx, window, tab, left)
            == Some((default_label.clone(), "work".to_owned()))
    });
}

#[gpui::test]
fn the_pane_menu_closes_when_its_pane_leaves_the_tab(cx: &mut TestAppContext) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, left, right) = harness.split_window(tab);
    let bar = harness.title_bar_point(window, right);
    harness.right_press_at(window, bar);
    harness.wait_for("the pane menu to open", |cx| {
        pane_menu(cx, window) == Some(right)
    });
    harness
        .runtime
        .lock()
        .close_pane(harness.workspace, tab, right)
        .unwrap();
    harness.wait_for("the menu to close with its pane", |cx| {
        pane_menu(cx, window).is_none()
            && panes_shown(cx, window, tab) == Some((vec![left], left))
    });
}
