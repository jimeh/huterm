//! Terminal accessibility nodes in production windows: focus, names, hidden
//! tabs, and split panes.

use super::*;

/// The window's terminal element nodes, without their synthetic children,
/// each with the text of its runs.
fn terminal_nodes(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> (serde_json::Value, Vec<(serde_json::Value, String)>) {
    let json = harness
        .cx
        .update(|cx| {
            window.update(cx, |_, window, _| window.debug_a11y_tree_json())
        })
        .unwrap()
        .expect("test windows report an accessibility tree for every frame");
    let tree: serde_json::Value = serde_json::from_str(&json).unwrap();
    let nodes = tree["nodes"].as_object().unwrap();
    let terminals = nodes
        .values()
        .filter(|node| {
            node["aria"]["role"] == "Terminal" && node["synthetic"] != true
        })
        .map(|node| {
            let text = node["children"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|key| &nodes[key.as_str().unwrap()])
                .filter(|child| child["aria"]["role"] == "TextRun")
                .map(|child| child["aria"]["value"].as_str().unwrap())
                .collect();
            (node.clone(), text)
        })
        .collect();
    (tree, terminals)
}

/// The text of the node the window reports focused, if it is a terminal.
fn focused_terminal_text(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
) -> Option<String> {
    let (tree, terminals) = terminal_nodes(harness, window);
    let focus = tree["gpui_focus"].as_str()?;
    let id = &tree["nodes"][focus]["accesskit_id"];
    terminals
        .into_iter()
        .find(|(node, _)| node["accesskit_id"] == *id)
        .map(|(_, text)| text)
}

/// The title `window`'s pane title bar would show for `tab`'s focused pane.
fn pane_title(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    tab: TabId,
) -> String {
    harness.update_view(window, |view, _, cx| {
        let tab = view.tabs.iter().find(|view| view.id == tab).unwrap();
        tab.pane_label(tab.focused(), view.config.tabs, cx)
    })
}

#[gpui::test]
fn the_shown_tabs_terminal_is_the_focused_named_node_and_follows_switches(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let first = harness.open_tab("printf FIRST; read line");
    let window = harness.attach_window();
    harness.wait_text(window, first, "FIRST");
    harness.activate(window, first);
    let second = harness.open_tab("printf SECOND; read line");
    harness.wait_for("the second tab to install", |cx| {
        terminal(cx, window, second).is_some()
    });
    harness.wait_for("the first tab's node to be focused", |cx| {
        focused(cx, window, first)
    });
    let shown = |harness: &mut DesktopHarness, text: &str| {
        harness.wait_for(&format!("{text} to be the focused node"), |cx| {
            window
                .update(cx, |_, window, _| window.debug_a11y_tree_json())
                .ok()
                .flatten()
                .is_some_and(|json| json.contains(text))
        });
        let focused = focused_terminal_text(harness, window);
        assert!(
            focused
                .as_deref()
                .is_some_and(|focused| focused.starts_with(text)),
            "the focused node reads {focused:?}"
        );
        let (_, terminals) = terminal_nodes(harness, window);
        assert_eq!(terminals.len(), 1, "the hidden tab has no node");
        terminals.into_iter().next().unwrap().0
    };
    let node = shown(&mut harness, "FIRST");
    assert_eq!(
        node["aria"]["label"],
        pane_title(&mut harness, window, first).as_str()
    );
    assert_eq!(node["aria"]["read_only"], true);
    harness.update_view(window, |view, window, cx| {
        view.select(second, window, cx);
    });
    let node = shown(&mut harness, "SECOND");
    assert_eq!(
        node["aria"]["label"],
        pane_title(&mut harness, window, second).as_str()
    );
    assert!(
        !terminal_nodes(&mut harness, window)
            .1
            .iter()
            .any(|(_, text)| text.contains("FIRST")),
        "the first tab left the tree"
    );
}

#[gpui::test]
fn each_split_pane_has_one_node_whose_runs_move_with_the_pane(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab("printf LEFT; read line");
    let window = harness.attach_window();
    harness.wait_text(window, tab, "LEFT");
    harness.activate(window, tab);
    let first = harness.first_pane(tab);
    let second = harness.split(
        tab,
        first,
        SplitDirection::Right,
        "printf RIGHT; read line",
    );
    harness.wait_for("both panes to show their text", |cx| {
        pane_screen(cx, window, tab, second)
            .is_some_and(|text| text.contains("RIGHT"))
    });
    // Each pane's first run starts at its grid's origin, in physical
    // pixels, once its node reports the text.
    let placed = |harness: &mut DesktopHarness, pane| {
        let mut origin = None;
        harness.wait_for("the pane's runs to sit at its grid", |cx| {
            let Some(view) = pane_view(cx, window, tab, pane) else {
                return false;
            };
            window
                .update(cx, |_, window, cx| {
                    let view = view.read(cx);
                    let model = view.a11y.model();
                    let scale = f64::from(window.scale_factor());
                    let area = view.grid_geometry(window).visible_area();
                    let expected = (
                        f64::from(area.origin.x) * scale,
                        f64::from(area.origin.y) * scale,
                    );
                    let first_run = model
                        .runs
                        .first()
                        .and_then(|run| run.node.bounds())
                        .map(|bounds| (bounds.x0, bounds.y0));
                    origin = Some(expected);
                    first_run == Some(expected)
                })
                .unwrap_or(false)
        });
        origin.unwrap()
    };
    let left = placed(&mut harness, first);
    let right = placed(&mut harness, second);
    assert!(right.0 > left.0, "{left:?} {right:?}");
    let (_, terminals) = terminal_nodes(&mut harness, window);
    let mut texts: Vec<_> = terminals
        .iter()
        .map(|(_, text)| text.lines().next().unwrap_or_default().to_owned())
        .collect();
    texts.sort();
    assert_eq!(texts, ["LEFT", "RIGHT"], "one node per pane");
    // Narrowing the window moves the right pane's grid, and its runs follow.
    let size =
        harness.update_view(window, |_, window, _| window.viewport_size());
    VisualTestContext::from_window(window, &harness.cx)
        .simulate_resize(gpui::size(size.width - px(120.0), size.height));
    let moved = placed(&mut harness, second);
    assert!(moved.0 < right.0, "{right:?} -> {moved:?}");
    assert_eq!(placed(&mut harness, first), left);
}
