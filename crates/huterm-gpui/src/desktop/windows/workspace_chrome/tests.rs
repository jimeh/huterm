use gpui::{Bounds, point, size};
use huterm_config::{SidebarPosition, TabPosition, TabsConfig};

use super::super::button_layout::ButtonLayout;
use super::super::{
    ChromeLayout, CornerSlot, LayoutInputs, TITLE_ROW_LEAD,
    TRAFFIC_LIGHT_INSET, WindowFrame, content_inside, shelf_min_width,
};
use super::*;

fn inputs(visible: bool, workspaces: usize) -> Inputs {
    Inputs {
        visible,
        workspaces,
        position: SidebarPosition::Left,
        width: PANEL_WIDTH,
        session_corner: false,
        workspace_corner: true,
    }
}

fn wide(_: WorkspaceChrome) -> Pixels {
    px(800.0)
}

#[test]
fn a_hidden_sidebar_shows_the_switcher_only_with_another_workspace() {
    for (workspaces, switcher) in [(0, false), (1, false), (2, true), (5, true)]
    {
        assert_eq!(
            resolve(inputs(false, workspaces), wide),
            WorkspaceChrome::Hidden {
                edge: Edge::Left,
                switcher,
                session: false,
                narrow: false,
            },
            "{workspaces}"
        );
    }
}

#[test]
fn settings_choose_which_switchers_a_hidden_sidebar_shows() {
    // (session setting, workspace setting, workspaces) -> (session, switcher)
    for (session_corner, workspace_corner, workspaces, session, switcher) in [
        (true, true, 1, true, false),
        (true, true, 2, true, true),
        (true, false, 2, true, false),
        (false, false, 2, false, false),
    ] {
        let chrome = resolve(
            Inputs {
                session_corner,
                workspace_corner,
                ..inputs(false, workspaces)
            },
            wide,
        );
        assert_eq!(
            (chrome.session(), chrome.switcher()),
            (session, switcher),
            "{session_corner} {workspace_corner} {workspaces}"
        );
        assert_eq!(
            chrome.switchers(),
            usize::from(session) + usize::from(switcher)
        );
    }
    // A docked sidebar holds both itself.
    let docked = resolve(
        Inputs {
            session_corner: true,
            ..inputs(true, 2)
        },
        wide,
    );
    assert!(docked.docked() && docked.switchers() == 0);
}

#[test]
fn a_visible_sidebar_docks_on_its_side() {
    for (position, edge) in [
        (SidebarPosition::Left, Edge::Left),
        (SidebarPosition::Right, Edge::Right),
    ] {
        // With one workspace too: the sidebar holds the session switcher.
        for workspaces in [1, 3] {
            let chrome = resolve(
                Inputs {
                    position,
                    ..inputs(true, workspaces)
                },
                wide,
            );
            assert_eq!(
                chrome,
                WorkspaceChrome::Panel {
                    edge,
                    width: PANEL_WIDTH
                }
            );
            assert!(chrome.docked() && chrome.docked_on(edge));
            assert!(!chrome.switcher() && !chrome.narrow());
        }
    }
}

#[test]
fn a_sidebar_that_squeezes_the_terminal_stays_hidden_and_narrow() {
    let squeezed = |_: WorkspaceChrome| MIN_TERMINAL_WIDTH - px(1.0);
    let chrome = resolve(inputs(true, 2), squeezed);
    assert_eq!(
        chrome,
        WorkspaceChrome::Hidden {
            edge: Edge::Left,
            switcher: true,
            session: false,
            narrow: true,
        }
    );
    // Exactly the minimum still docks.
    assert!(resolve(inputs(true, 2), |_| MIN_TERMINAL_WIDTH).docked());
    // A hidden sidebar never measures the terminal.
    assert!(!resolve(inputs(false, 2), squeezed).narrow());
}

#[test]
fn panel_width_clamps_to_its_limits_and_half_the_window() {
    assert_eq!(panel_width(px(100.0), px(1000.0)), PANEL_MIN);
    assert_eq!(panel_width(px(500.0), px(1000.0)), PANEL_MAX);
    assert_eq!(panel_width(px(250.0), px(1000.0)), px(250.0));
    assert_eq!(panel_width(px(250.0), px(300.0)), px(150.0));
    assert_eq!(panel_width(px(250.0), px(-10.0)), px(0.0));
}

fn hidden(edge: Edge) -> WorkspaceChrome {
    WorkspaceChrome::Hidden {
        edge,
        switcher: true,
        session: false,
        narrow: false,
    }
}

#[test]
fn the_corner_controls_take_the_corner_on_the_sidebars_side() {
    use CornerHost::{Bar, Column, Hidden, Sidebar, TitleRow};
    use TabPosition::{Bottom, Left, Right, Titlebar, Top};
    // (edge, title row, tab position, bar shown) -> host
    let cases = [
        // Left: after the traffic lights, in whatever holds that corner.
        (Edge::Left, true, Titlebar, true, Bar),
        (Edge::Left, true, Titlebar, false, TitleRow),
        (Edge::Left, true, Top, true, TitleRow),
        (Edge::Left, true, Bottom, true, TitleRow),
        (Edge::Left, true, Left, true, Column),
        (Edge::Left, true, Right, true, TitleRow),
        (Edge::Left, false, Top, true, Bar),
        (Edge::Left, false, Bottom, true, Bar),
        (Edge::Left, false, Left, true, Column),
        (Edge::Left, false, Right, true, Column),
        (Edge::Left, false, Top, false, Hidden),
        // Right: wherever the menu button goes.
        (Edge::Right, true, Titlebar, true, Bar),
        (Edge::Right, true, Titlebar, false, TitleRow),
        (Edge::Right, true, Top, true, TitleRow),
        (Edge::Right, true, Right, true, TitleRow),
        (Edge::Right, true, Left, true, TitleRow),
        (Edge::Right, false, Top, true, Bar),
        (Edge::Right, false, Bottom, true, Bar),
        (Edge::Right, false, Left, true, Column),
        (Edge::Right, false, Right, true, Column),
        (Edge::Right, false, Bottom, false, Hidden),
    ];
    for (edge, title_row, position, bar_shown, expected) in cases {
        assert_eq!(
            corner_host(hidden(edge), title_row, position, bar_shown),
            expected,
            "{edge:?} {title_row} {position:?} {bar_shown}"
        );
    }
    // A docked sidebar holds them in its own strip, whatever the tabs do.
    for position in POSITIONS {
        for edge in [Edge::Left, Edge::Right] {
            let docked = WorkspaceChrome::Panel {
                edge,
                width: PANEL_WIDTH,
            };
            assert_eq!(corner_host(docked, false, position, false), Sidebar);
        }
    }
}

const POSITIONS: [TabPosition; 5] = [
    TabPosition::Top,
    TabPosition::Bottom,
    TabPosition::Left,
    TabPosition::Right,
    TabPosition::Titlebar,
];

const EDGES: [Edge; 2] = [Edge::Left, Edge::Right];

/// The layout as it was before the sidebar, frozen as the oracle that a
/// window without a docked sidebar or corner slot must match.
#[derive(Debug, PartialEq)]
struct Legacy {
    terminal: Bounds<Pixels>,
    tabs: Bounds<Pixels>,
    content: Bounds<Pixels>,
    strip_inset: Pixels,
    strip_trailing: Pixels,
    outside_terminal: bool,
}

#[expect(
    clippy::too_many_arguments,
    reason = "a verbatim copy of the layout before the workspace band"
)]
fn legacy(
    window: gpui::Size<Pixels>,
    titlebar: Pixels,
    position: TabPosition,
    bar_height: Pixels,
    sidebar_width: Pixels,
    safe_area: gpui::Edges<Pixels>,
    notch_shelf: Option<Bounds<Pixels>>,
    frame: WindowFrame,
) -> Legacy {
    // The layout partitions the content inside the frame; the frame's
    // origin is added back at the end. A notch shelf is already
    // window-relative and never coexists with a frame, so it is
    // brought into content coordinates like everything else.
    let content = content_inside(window, frame);
    let viewport = content.size;
    let notch_shelf = notch_shelf
        .map(|shelf| Bounds::new(shelf.origin - content.origin, shelf.size));
    let left = safe_area.left.max(px(0.0)).min(viewport.width.max(px(0.0)));
    let top = (titlebar + safe_area.top)
        .max(px(0.0))
        .min(viewport.height.max(px(0.0)));
    let available = size(
        (viewport.width - left - safe_area.right.max(px(0.0))).max(px(0.0)),
        (viewport.height - top - safe_area.bottom.max(px(0.0))).max(px(0.0)),
    );
    let mut terminal = Bounds::new(point(left, top), available);
    let mut tabs = terminal;
    let mut strip_inset = px(0.0);
    let mut strip_trailing = px(0.0);
    let mut outside_terminal = false;
    if position == TabPosition::Titlebar {
        // The merged row is the title strip itself: the full window
        // width above the terminal, at the strip's height. Its tabs
        // start after the traffic lights, or after a small lead in the
        // row Huterm draws, whose window controls end the row; the
        // terminal keeps the whole area below the strip.
        let row = titlebar.max(px(0.0)).min(viewport.height.max(px(0.0)));
        tabs = Bounds::new(
            point(px(0.0), px(0.0)),
            size(viewport.width.max(px(0.0)), row),
        );
        if frame.controls {
            strip_inset = if frame.buttons.leading.is_empty() {
                TITLE_ROW_LEAD
            } else {
                frame.buttons.leading.width()
            };
            strip_trailing = frame.buttons.trailing.width();
        } else {
            strip_inset = TRAFFIC_LIGHT_INSET;
        }
        outside_terminal = true;
    } else if position.vertical() {
        tabs.size.width = sidebar_width
            .clamp(px(140.0), px(400.0))
            .min(available.width * 0.5);
        terminal.size.width = (available.width - tabs.size.width).max(px(0.0));
        if position == TabPosition::Left {
            terminal.origin.x += tabs.size.width;
        } else {
            tabs.origin.x += terminal.size.width;
        }
        // The background and divider fill the content height; rows
        // stay below the title row and display safe area.
        strip_inset = top;
        tabs.origin.y = px(0.0);
        tabs.size.height =
            (viewport.height - safe_area.bottom.max(px(0.0))).max(px(0.0));
    } else if let Some(shelf) = notch_shelf.filter(|shelf| {
        // A shelf too short for the bar is not used; the bar then takes
        // its normal place below the safe area at full height.
        position == TabPosition::Top && shelf.size.height >= bar_height
    }) {
        // The bar keeps its height at the bottom of the shelf, so its
        // spacing to the terminal matches a windowed top bar. The
        // terminal keeps the area under the safe area except one point
        // for the border line, so that line never covers a cell.
        tabs = Bounds::new(
            point(shelf.origin.x, shelf.bottom() - bar_height),
            size(shelf.size.width, bar_height),
        );
        let line = px(1.0).min(available.height);
        terminal.origin.y += line;
        terminal.size.height -= line;
        outside_terminal = true;
    } else {
        tabs.size.height = bar_height.min(available.height);
        terminal.size.height =
            (available.height - tabs.size.height).max(px(0.0));
        if position == TabPosition::Top {
            terminal.origin.y += tabs.size.height;
        } else {
            tabs.origin.y += terminal.size.height;
        }
    }
    terminal.origin += content.origin;
    tabs.origin += content.origin;
    let _ = frame;
    Legacy {
        terminal,
        tabs,
        content,
        strip_inset,
        strip_trailing,
        outside_terminal,
    }
}

fn docked(edge: Edge) -> WorkspaceChrome {
    WorkspaceChrome::Panel {
        edge,
        width: PANEL_WIDTH,
    }
}

fn linux_frame(layout: &str) -> WindowFrame {
    WindowFrame {
        inset: gpui::Edges::all(px(10.0)),
        controls: true,
        buttons: ButtonLayout::parse(layout),
        traffic_lights: None,
    }
}

fn layout_inputs(
    window: gpui::Size<Pixels>,
    titlebar: Pixels,
    position: TabPosition,
    workspace: WorkspaceChrome,
) -> LayoutInputs {
    LayoutInputs {
        window,
        titlebar,
        position,
        bar_height: px(32.0),
        sidebar_width: px(180.0),
        safe_area: gpui::Edges::default(),
        notch_shelf: None,
        shelf_min_width: shelf_min_width(TabsConfig::default()),
        frame: WindowFrame::default(),
        workspace,
        corner: None,
    }
}

fn area(bounds: Bounds<Pixels>) -> f32 {
    f32::from(bounds.size.width) * f32::from(bounds.size.height)
}

fn overlap(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
    let i = a.intersect(&b);
    i.size.width > px(0.0) && i.size.height > px(0.0)
}

fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    inner.left() >= outer.left()
        && inner.top() >= outer.top()
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
}

#[test]
fn a_hidden_sidebar_without_a_corner_slot_reproduces_the_layout_before_it() {
    let shelf = Bounds::new(point(px(0.0), px(0.0)), size(px(300.0), px(38.0)));
    let frames = [
        WindowFrame::default(),
        linux_frame(":minimize,maximize,close"),
        linux_frame("close,minimize,maximize:"),
    ];
    let safe_areas = [
        gpui::Edges::default(),
        gpui::Edges {
            top: px(38.0),
            ..gpui::Edges::default()
        },
        gpui::Edges::all(px(7.0)),
    ];
    for window in [
        size(px(800.0), px(600.0)),
        size(px(150.0), px(40.0)),
        size(px(0.0), px(0.0)),
    ] {
        for position in POSITIONS {
            for titlebar in [px(0.0), px(28.0)] {
                for frame in frames {
                    for safe_area in safe_areas {
                        for notch_shelf in [None, Some(shelf)] {
                            for workspace in
                                [WorkspaceChrome::NONE, hidden(Edge::Right)]
                            {
                                let layout =
                                    ChromeLayout::build_with(LayoutInputs {
                                        window,
                                        titlebar,
                                        position,
                                        bar_height: px(32.0),
                                        sidebar_width: px(220.0),
                                        safe_area,
                                        notch_shelf,
                                        shelf_min_width: shelf_min_width(
                                            TabsConfig::default(),
                                        ),
                                        frame,
                                        workspace,
                                        corner: None,
                                    });
                                let expected = legacy(
                                    window,
                                    titlebar,
                                    position,
                                    px(32.0),
                                    px(220.0),
                                    safe_area,
                                    notch_shelf,
                                    frame,
                                );
                                let actual = Legacy {
                                    terminal: layout.terminal,
                                    tabs: layout.tabs,
                                    content: layout.content,
                                    strip_inset: layout.strip_inset,
                                    strip_trailing: layout.strip_trailing,
                                    outside_terminal: layout.outside_terminal,
                                };
                                assert_eq!(
                                    actual, expected,
                                    "{window:?} {position:?} {titlebar:?} {frame:?} {safe_area:?} {notch_shelf:?}"
                                );
                                assert_eq!(layout.workspace, None);
                                assert_eq!(layout.area, layout.content);
                                assert_eq!(layout.corner(), None);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn every_side_with_every_tab_position_partitions_the_window() {
    let window = size(px(1000.0), px(700.0));
    for edge in EDGES {
        for position in POSITIONS {
            for titlebar in [px(0.0), px(28.0)] {
                let layout = ChromeLayout::build_with(layout_inputs(
                    window,
                    titlebar,
                    position,
                    docked(edge),
                ));
                let label = format!("{edge:?} {position:?} {titlebar:?}");
                let band = layout.workspace.expect("a band");
                let content = layout.content;
                for (name, part) in [
                    ("band", band),
                    ("tabs", layout.tabs),
                    ("terminal", layout.terminal),
                ] {
                    assert!(inside(part, content), "{label}: {name} {part:?}");
                }
                assert!(!overlap(band, layout.terminal), "{label}");
                assert!(!overlap(band, layout.tabs), "{label}");
                assert!(!overlap(layout.tabs, layout.terminal), "{label}");
                assert!(!overlap(band, layout.area), "{label}");
                // The sidebar is outermost on its side, full height.
                assert_eq!(band.size.width, PANEL_WIDTH, "{label}");
                assert_eq!(band.size.height, content.size.height, "{label}");
                match edge {
                    Edge::Left => {
                        assert_eq!(band.left(), content.left(), "{label}");
                        assert!(layout.tabs.left() >= band.right(), "{label}");
                        assert!(layout.terminal.left() >= band.right());
                    }
                    Edge::Right => {
                        assert_eq!(band.right(), content.right(), "{label}");
                        assert!(layout.tabs.right() <= band.left(), "{label}");
                        assert!(layout.terminal.right() <= band.left());
                    }
                }
                // Its content starts below its corner strip: the title row,
                // or a row of its own for the sidebar button without one.
                let strip = if titlebar > px(0.0) {
                    titlebar
                } else {
                    CORNER_ROW
                };
                assert_eq!(layout.workspace_inset, strip, "{label}");
                // The band, the title row, the tabs, and the terminal
                // cover the content exactly.
                let title = if position == TabPosition::Titlebar {
                    0.0
                } else if position.vertical() {
                    f32::from(layout.terminal.size.width) * f32::from(titlebar)
                } else {
                    f32::from(layout.area.size.width) * f32::from(titlebar)
                };
                assert!(
                    (area(band)
                        + area(layout.tabs)
                        + area(layout.terminal)
                        + title
                        - area(content))
                    .abs()
                        < 0.5,
                    "{label}"
                );
            }
        }
    }
}

#[test]
fn a_docked_sidebar_owns_its_title_row_corner() {
    let window = size(px(1000.0), px(700.0));
    let left = ChromeLayout::build_with(layout_inputs(
        window,
        px(28.0),
        TabPosition::Titlebar,
        docked(Edge::Left),
    ));
    // The traffic lights and sidebar button sit in the sidebar's strip, so
    // the merged row starts at the sidebar, as a top bar at the window's
    // edge does.
    assert_eq!(left.tabs.left(), PANEL_WIDTH);
    assert_eq!(left.strip_inset, px(0.0));
    let right = ChromeLayout::build_with(layout_inputs(
        window,
        px(28.0),
        TabPosition::Titlebar,
        docked(Edge::Right),
    ));
    assert_eq!(right.strip_inset, TRAFFIC_LIGHT_INSET);
    assert_eq!(right.tabs.right(), px(1000.0) - PANEL_WIDTH);
    // On Linux the sidebar holds the window buttons at its corner.
    for edge in EDGES {
        let frame = linux_frame("close:minimize,maximize,close");
        let layout = ChromeLayout::build_with(LayoutInputs {
            frame,
            ..layout_inputs(
                window,
                px(32.0),
                TabPosition::Titlebar,
                docked(edge),
            )
        });
        let (inset, trailing) = match edge {
            Edge::Left => (px(0.0), frame.buttons.trailing.width()),
            Edge::Right => (frame.buttons.leading.width(), px(0.0)),
        };
        assert_eq!(layout.strip_inset, inset, "{edge:?}");
        assert_eq!(layout.strip_trailing, trailing, "{edge:?}");
    }
}

#[test]
fn a_docked_sidebar_takes_its_part_of_a_notch_shelf() {
    // A 1800 by 1169 point display with a 38-point camera housing between
    // two 790-point shelves, covered by custom fullscreen.
    let window = size(px(1800.0), px(1169.0));
    let safe_area = gpui::Edges {
        top: px(38.0),
        ..gpui::Edges::default()
    };
    let left_shelf =
        Bounds::new(point(px(0.0), px(0.0)), size(px(790.0), px(38.0)));
    let right_shelf =
        Bounds::new(point(px(1010.0), px(0.0)), size(px(790.0), px(38.0)));
    let shelf_inputs = |edge, shelf| LayoutInputs {
        safe_area,
        notch_shelf: Some(shelf),
        bar_height: px(34.0),
        ..layout_inputs(window, px(0.0), TabPosition::Top, docked(edge))
    };
    // The bar on the sidebar's side starts or ends where the sidebar does.
    let left = ChromeLayout::build_with(shelf_inputs(Edge::Left, left_shelf));
    assert_eq!(
        left.tabs,
        Bounds::new(point(PANEL_WIDTH, px(4.0)), size(px(610.0), px(34.0)))
    );
    let right =
        ChromeLayout::build_with(shelf_inputs(Edge::Right, right_shelf));
    assert_eq!(right.tabs.left(), px(1010.0));
    assert_eq!(right.tabs.right(), px(1800.0) - PANEL_WIDTH);
    // On the other side of the notch the shelf is the sidebar's no more.
    let across =
        ChromeLayout::build_with(shelf_inputs(Edge::Left, right_shelf));
    assert_eq!(across.tabs.left(), px(1010.0));
    assert_eq!(across.tabs.size.width, px(790.0));
    for layout in [left, right, across] {
        let band = layout.workspace.unwrap();
        assert!(!overlap(band, layout.tabs), "{layout:?}");
        // The sidebar's corner row is the shelf bar's, so its button sits
        // level with the tabs, and its content starts under the safe area.
        assert_eq!(layout.workspace_row, layout.tabs.top());
        assert_eq!(layout.workspace_inset, px(38.0));
        assert_eq!(layout.shelf_line, px(1.0));
        assert_eq!(layout.terminal.top(), px(39.0));
    }
    // Without a shelf for the bar the sidebar's own row starts under the
    // safe area instead of straddling the notch's edge.
    for (position, shelf) in [
        (TabPosition::Top, None),
        (TabPosition::Left, Some(left_shelf)),
        (TabPosition::Bottom, Some(left_shelf)),
    ] {
        let layout = ChromeLayout::build_with(LayoutInputs {
            safe_area,
            notch_shelf: shelf,
            ..layout_inputs(window, px(0.0), position, docked(Edge::Left))
        });
        assert_eq!(layout.workspace_row, px(38.0), "{position:?}");
        assert_eq!(
            layout.workspace_inset,
            px(38.0) + CORNER_ROW,
            "{position:?}"
        );
        assert_eq!(layout.shelf_line, px(0.0), "{position:?}");
    }
    // A sidebar that leaves less of its shelf than one tab and the new-tab
    // control needs puts the bar back under the safe area.
    let least = shelf_min_width(TabsConfig::default());
    for edge in EDGES {
        for rest in [least, least - px(1.0), px(0.0), px(-40.0)] {
            let width = PANEL_WIDTH + rest;
            let x = match edge {
                Edge::Left => px(0.0),
                Edge::Right => px(1800.0) - width,
            };
            let shelf = Bounds::new(point(x, px(0.0)), size(width, px(38.0)));
            let layout = ChromeLayout::build_with(shelf_inputs(edge, shelf));
            let band = layout.workspace.unwrap();
            assert!(!overlap(band, layout.tabs), "{edge:?} {rest:?}");
            if rest >= least {
                assert_eq!(layout.tabs.size.width, least, "{edge:?}");
                assert_eq!(layout.tabs.top(), px(4.0), "{edge:?}");
                assert_eq!(layout.shelf_line, px(1.0), "{edge:?}");
            } else {
                assert_eq!(layout.tabs.top(), px(38.0), "{edge:?} {rest:?}");
                assert_eq!(
                    layout.tabs.size.width,
                    px(1800.0) - PANEL_WIDTH,
                    "{edge:?} {rest:?}"
                );
                assert_eq!(layout.workspace_row, px(38.0), "{edge:?} {rest:?}");
                assert_eq!(layout.shelf_line, px(0.0), "{edge:?} {rest:?}");
                assert_eq!(
                    layout.terminal.top(),
                    px(38.0 + 34.0),
                    "{edge:?} {rest:?}"
                );
            }
        }
    }
}

#[test]
fn the_corner_slot_leads_a_bar_or_takes_a_row_above_a_column() {
    let window = size(px(1000.0), px(700.0));
    let lead = px(120.0);
    for position in
        [TabPosition::Titlebar, TabPosition::Top, TabPosition::Bottom]
    {
        let titlebar = if position == TabPosition::Titlebar {
            px(28.0)
        } else {
            px(0.0)
        };
        let plain = ChromeLayout::build_with(layout_inputs(
            window,
            titlebar,
            position,
            hidden(Edge::Left),
        ));
        let slotted = ChromeLayout::build_with(LayoutInputs {
            corner: Some(CornerSlot::Lead(lead)),
            ..layout_inputs(window, titlebar, position, hidden(Edge::Left))
        });
        // The controls sit right after the traffic lights, as a docked
        // sidebar's strip draws the button, or a margin in from the bar's
        // start; the tabs start after them, and nothing else moves.
        let start = if plain.strip_inset > px(0.0) {
            plain.strip_inset
        } else {
            CORNER_MARGIN
        };
        assert_eq!(slotted.strip_inset, start + lead, "{position:?}");
        let slot = slotted.corner().expect("a slot");
        assert_eq!(slot.left(), slotted.tabs.left() + start, "{position:?}");
        assert_eq!(slot.size.width, lead, "{position:?}");
        assert!(inside(slot, slotted.tabs), "{position:?}");
        assert_eq!(slotted.terminal, plain.terminal, "{position:?}");
        assert_eq!(slotted.tabs, plain.tabs, "{position:?}");
    }
    for position in [TabPosition::Left, TabPosition::Right] {
        let plain = ChromeLayout::build_with(layout_inputs(
            window,
            px(28.0),
            position,
            hidden(Edge::Left),
        ));
        let slotted = ChromeLayout::build_with(LayoutInputs {
            corner: Some(CornerSlot::Rows(1)),
            ..layout_inputs(window, px(28.0), position, hidden(Edge::Left))
        });
        // A row above the first tab, below the title row.
        assert_eq!(
            slotted.strip_inset,
            plain.strip_inset + CORNER_ROW,
            "{position:?}"
        );
        let slot = slotted.corner().expect("a slot");
        assert!(slot.top() >= slotted.tabs.top() + plain.strip_inset);
        assert!(slot.bottom() <= slotted.tabs.top() + slotted.strip_inset);
        assert!(inside(slot, slotted.tabs), "{position:?}");
        assert_eq!(slotted.terminal, plain.terminal, "{position:?}");
        // Two switchers take a row each, one control and a margin taller.
        let two = ChromeLayout::build_with(LayoutInputs {
            corner: Some(CornerSlot::Rows(2)),
            ..layout_inputs(window, px(28.0), position, hidden(Edge::Left))
        });
        let taller = super::super::CONTROL_SIZE + CORNER_MARGIN;
        assert_eq!(two.strip_inset, slotted.strip_inset + taller);
        let slot = two.corner().expect("a slot");
        assert_eq!(
            slot.size.height,
            super::super::CONTROL_SIZE * 2.0 + CORNER_MARGIN
        );
        assert!(slot.bottom() <= two.tabs.top() + two.strip_inset);
        assert_eq!(two.terminal, plain.terminal, "{position:?}");
    }
}

#[test]
fn small_windows_keep_every_part_inside_and_nonnegative() {
    for window in [
        size(px(0.0), px(0.0)),
        size(px(60.0), px(20.0)),
        size(px(200.0), px(50.0)),
    ] {
        for edge in EDGES {
            for position in POSITIONS {
                for titlebar in [px(0.0), px(28.0)] {
                    for (workspace, corner) in [
                        (docked(edge), None),
                        (hidden(edge), Some(CornerSlot::Lead(px(160.0)))),
                        (hidden(edge), Some(CornerSlot::Rows(1))),
                    ] {
                        let layout = ChromeLayout::build_with(LayoutInputs {
                            corner,
                            ..layout_inputs(
                                window, titlebar, position, workspace,
                            )
                        });
                        let parts = [
                            Some(layout.tabs),
                            Some(layout.terminal),
                            layout.workspace,
                            layout.corner(),
                        ];
                        for part in parts.into_iter().flatten() {
                            assert!(
                                part.size.width >= px(0.0)
                                    && part.size.height >= px(0.0),
                                "{window:?} {edge:?} {position:?} {part:?}"
                            );
                            assert!(
                                inside(part, layout.content),
                                "{window:?} {edge:?} {position:?} {workspace:?} {part:?}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a_sidebar_squeezing_a_small_window_resolves_to_narrow() {
    // The rule reads the terminal width the layout leaves beside the
    // sidebar and the tab column.
    let resolve_in = |width: f32, position: TabPosition| {
        let window = size(px(width), px(500.0));
        resolve(inputs(true, 2), |panel| {
            ChromeLayout::build_with(layout_inputs(
                window,
                px(28.0),
                position,
                panel,
            ))
            .terminal
            .size
            .width
        })
    };
    assert!(resolve_in(800.0, TabPosition::Titlebar).docked());
    assert!(resolve_in(500.0, TabPosition::Titlebar).narrow());
    // A tab column beside the sidebar counts against the terminal.
    assert!(resolve_in(700.0, TabPosition::Left).narrow());
    assert!(resolve_in(700.0, TabPosition::Titlebar).docked());
    assert!(resolve_in(1000.0, TabPosition::Left).docked());
}

#[test]
fn the_frame_inset_offsets_the_band_with_everything_else() {
    let frame = linux_frame(":close");
    for edge in EDGES {
        let layout = ChromeLayout::build_with(LayoutInputs {
            frame,
            ..layout_inputs(
                size(px(900.0), px(600.0)),
                px(32.0),
                TabPosition::Titlebar,
                docked(edge),
            )
        });
        let content = content_inside(size(px(900.0), px(600.0)), frame);
        assert_eq!(layout.content, content);
        assert!(inside(layout.workspace.unwrap(), content), "{edge:?}");
        assert!(inside(layout.area, content), "{edge:?}");
    }
}
