use gpui::{Bounds, point, size};
use huterm_config::TabPosition;

use super::super::button_layout::ButtonLayout;
use super::super::{
    ChromeLayout, LayoutInputs, Presentation, TITLE_ROW_LEAD,
    TRAFFIC_LIGHT_INSET, WindowFrame, content_inside,
};
use super::*;

fn inputs() -> Inputs {
    Inputs {
        show: WorkspaceShow::Auto,
        workspaces: 2,
        collapsed: false,
        position: WorkspacePosition::Left,
        width: PANEL_WIDTH,
        dropdown_lead: px(90.0),
    }
}

fn wide(_: WorkspaceChrome) -> Pixels {
    px(800.0)
}

#[test]
fn show_decides_whether_any_workspace_ui_exists() {
    let cases = [
        (WorkspaceShow::Auto, 1, false),
        (WorkspaceShow::Auto, 2, true),
        (WorkspaceShow::Always, 1, true),
        (WorkspaceShow::Always, 0, false),
        (WorkspaceShow::Never, 5, false),
    ];
    for (show, workspaces, expected) in cases {
        assert_eq!(shown(show, workspaces), expected, "{show:?}");
        let chrome = resolve(
            Inputs {
                show,
                workspaces,
                ..inputs()
            },
            wide,
        );
        assert_eq!(
            chrome != WorkspaceChrome::None,
            expected,
            "{show:?} with {workspaces}"
        );
    }
}

#[test]
fn every_position_resolves_to_its_edge_or_the_dropdown() {
    for (position, expected) in [
        (
            WorkspacePosition::Left,
            WorkspaceChrome::Panel {
                edge: Edge::Left,
                width: PANEL_WIDTH,
            },
        ),
        (
            WorkspacePosition::Right,
            WorkspaceChrome::Panel {
                edge: Edge::Right,
                width: PANEL_WIDTH,
            },
        ),
        (
            WorkspacePosition::Top,
            WorkspaceChrome::Bar { edge: Edge::Top },
        ),
        (
            WorkspacePosition::Bottom,
            WorkspaceChrome::Bar { edge: Edge::Bottom },
        ),
    ] {
        let open = Inputs {
            position,
            ..inputs()
        };
        assert_eq!(resolve(open, wide), expected, "{position:?}");
        let collapsed = Inputs {
            collapsed: true,
            ..open
        };
        assert_eq!(
            resolve(collapsed, wide),
            WorkspaceChrome::Dropdown { lead: px(90.0) },
            "{position:?} collapsed"
        );
    }
}

#[test]
fn a_side_panel_that_squeezes_the_terminal_shows_the_dropdown() {
    let narrow = |chrome: WorkspaceChrome| {
        assert!(matches!(chrome, WorkspaceChrome::Panel { .. }));
        MIN_TERMINAL_WIDTH - px(1.0)
    };
    assert!(resolve(inputs(), narrow).is_dropdown());
    let exact = |_| MIN_TERMINAL_WIDTH;
    assert!(matches!(
        resolve(inputs(), exact),
        WorkspaceChrome::Panel { .. }
    ));
    // A bar never needs the fallback; its chips scroll.
    let bar = Inputs {
        position: WorkspacePosition::Top,
        ..inputs()
    };
    assert_eq!(
        resolve(bar, |_| unreachable!("bars ask no width")),
        WorkspaceChrome::Bar { edge: Edge::Top }
    );
}

#[test]
fn panel_width_clamps_to_its_limits_and_half_the_window() {
    assert_eq!(panel_width(px(100.0), px(1000.0)), PANEL_MIN);
    assert_eq!(panel_width(px(500.0), px(1000.0)), PANEL_MAX);
    assert_eq!(panel_width(px(250.0), px(1000.0)), px(250.0));
    assert_eq!(panel_width(px(250.0), px(300.0)), px(150.0));
    assert_eq!(panel_width(px(250.0), px(-10.0)), px(0.0));
}

const POSITIONS: [TabPosition; 5] = [
    TabPosition::Top,
    TabPosition::Bottom,
    TabPosition::Left,
    TabPosition::Right,
    TabPosition::Titlebar,
];

const EDGES: [Edge; 4] = [Edge::Left, Edge::Top, Edge::Right, Edge::Bottom];

/// The layout as it was before the workspace band, frozen as the oracle
/// that `WorkspaceChrome::None` must match.
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

fn chrome(edge: Edge) -> WorkspaceChrome {
    if edge.vertical() {
        WorkspaceChrome::Panel {
            edge,
            width: PANEL_WIDTH,
        }
    } else {
        WorkspaceChrome::Bar { edge }
    }
}

fn linux_frame(layout: &str) -> WindowFrame {
    WindowFrame {
        inset: gpui::Edges::all(px(10.0)),
        controls: true,
        buttons: ButtonLayout::parse(layout),
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
        frame: WindowFrame::default(),
        workspace,
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
fn no_workspace_chrome_reproduces_the_layout_before_it() {
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
                            for bar_height in [px(32.0), px(34.0)] {
                                let layout =
                                    ChromeLayout::build_with(LayoutInputs {
                                        window,
                                        titlebar,
                                        position,
                                        bar_height,
                                        sidebar_width: px(220.0),
                                        safe_area,
                                        notch_shelf,
                                        frame,
                                        workspace: WorkspaceChrome::None,
                                    });
                                let expected = legacy(
                                    window,
                                    titlebar,
                                    position,
                                    bar_height,
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
                                assert_eq!(layout.dropdown(), None);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn every_edge_with_every_tab_position_partitions_the_window() {
    let window = size(px(1000.0), px(700.0));
    for edge in EDGES {
        for position in POSITIONS {
            for titlebar in [px(0.0), px(28.0)] {
                let layout = ChromeLayout::build_with(layout_inputs(
                    window,
                    titlebar,
                    position,
                    chrome(edge),
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
                // The band is outermost on its edge.
                match edge {
                    Edge::Left => {
                        assert_eq!(band.left(), content.left(), "{label}");
                        assert_eq!(band.size.width, PANEL_WIDTH, "{label}");
                        assert!(layout.tabs.left() >= band.right(), "{label}");
                        assert!(layout.terminal.left() >= band.right());
                    }
                    Edge::Right => {
                        assert_eq!(band.right(), content.right(), "{label}");
                        assert_eq!(band.size.width, PANEL_WIDTH, "{label}");
                        assert!(layout.tabs.right() <= band.left(), "{label}");
                        assert!(layout.terminal.right() <= band.left());
                    }
                    Edge::Top => {
                        assert_eq!(band.top(), content.top(), "{label}");
                        assert!(layout.tabs.top() >= band.bottom(), "{label}");
                        assert!(layout.terminal.top() >= band.bottom());
                        // A title row becomes the bar.
                        let height = if titlebar > px(0.0) {
                            titlebar
                        } else {
                            BAR_HEIGHT
                        };
                        assert_eq!(band.size.height, height, "{label}");
                    }
                    Edge::Bottom => {
                        assert_eq!(band.bottom(), content.bottom(), "{label}");
                        assert!(layout.tabs.bottom() <= band.top(), "{label}");
                        assert!(layout.terminal.bottom() <= band.top());
                    }
                }
                if edge.vertical() {
                    assert_eq!(band.size.height, content.size.height);
                    // Rows start below the title row.
                    assert_eq!(layout.workspace_inset, titlebar, "{label}");
                } else {
                    assert_eq!(band.size.width, content.size.width);
                }
                // The band, the title row, the tabs, and the terminal
                // cover the content exactly.
                let title = if edge == Edge::Top
                    || position == TabPosition::Titlebar
                {
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
                // Titlebar tabs drop below a top bar that took the title row.
                if edge == Edge::Top && position == TabPosition::Titlebar {
                    assert_eq!(layout.tabs.top(), band.bottom(), "{label}");
                    assert!(!layout.outside_terminal, "{label}");
                    assert_eq!(layout.strip_inset, px(0.0), "{label}");
                }
            }
        }
    }
}

#[test]
fn a_side_panel_owns_its_title_row_corner() {
    let window = size(px(1000.0), px(700.0));
    let left = ChromeLayout::build_with(layout_inputs(
        window,
        px(28.0),
        TabPosition::Titlebar,
        chrome(Edge::Left),
    ));
    // The traffic lights sit in the panel's strip, so the merged row
    // starts at the panel with a small lead.
    assert_eq!(left.tabs.left(), PANEL_WIDTH);
    assert_eq!(left.strip_inset, TITLE_ROW_LEAD);
    let right = ChromeLayout::build_with(layout_inputs(
        window,
        px(28.0),
        TabPosition::Titlebar,
        chrome(Edge::Right),
    ));
    assert_eq!(right.strip_inset, TRAFFIC_LIGHT_INSET);
    assert_eq!(right.tabs.right(), px(1000.0) - PANEL_WIDTH);
    // On Linux the panel holds the window buttons at its corner.
    for (edge, inset, trailing) in [
        (Edge::Left, TITLE_ROW_LEAD, true),
        (Edge::Right, px(0.0), false),
    ] {
        let frame = linux_frame("close:minimize,maximize,close");
        let layout = ChromeLayout::build_with(LayoutInputs {
            frame,
            ..layout_inputs(
                window,
                px(32.0),
                TabPosition::Titlebar,
                chrome(edge),
            )
        });
        if edge == Edge::Left {
            assert_eq!(layout.strip_inset, inset);
        } else {
            assert_eq!(layout.strip_inset, frame.buttons.leading.width());
        }
        assert_eq!(
            layout.strip_trailing,
            if trailing {
                frame.buttons.trailing.width()
            } else {
                px(0.0)
            },
            "{edge:?}"
        );
    }
}

#[test]
fn a_top_bar_in_the_drawn_title_row_avoids_its_window_buttons() {
    let frame = linux_frame("close:minimize,maximize");
    let layout = ChromeLayout::build_with(LayoutInputs {
        frame,
        ..layout_inputs(
            size(px(900.0), px(600.0)),
            px(32.0),
            TabPosition::Titlebar,
            chrome(Edge::Top),
        )
    });
    let band = layout.workspace.unwrap();
    assert_eq!(band.origin, layout.content.origin);
    assert_eq!(layout.workspace_inset, frame.buttons.leading.width());
    assert_eq!(layout.workspace_trailing, frame.buttons.trailing.width());
    // Without leading buttons the chips start after a small lead; on
    // macOS after the traffic lights.
    let lead = ChromeLayout::build_with(LayoutInputs {
        frame: linux_frame(":close"),
        ..layout_inputs(
            size(px(900.0), px(600.0)),
            px(32.0),
            TabPosition::Top,
            chrome(Edge::Top),
        )
    });
    assert_eq!(lead.workspace_inset, TITLE_ROW_LEAD);
    let mac = ChromeLayout::build_with(layout_inputs(
        size(px(900.0), px(600.0)),
        px(28.0),
        TabPosition::Top,
        chrome(Edge::Top),
    ));
    assert_eq!(mac.workspace_inset, TRAFFIC_LIGHT_INSET);
    assert_eq!(mac.workspace_trailing, px(0.0));
}

#[test]
fn a_top_bar_takes_the_notch_shelf_and_the_tabs_never_do() {
    let shelf = Bounds::new(point(px(0.0), px(0.0)), size(px(300.0), px(38.0)));
    let safe_area = gpui::Edges {
        top: px(38.0),
        ..gpui::Edges::default()
    };
    let layout = ChromeLayout::build_with(LayoutInputs {
        safe_area,
        notch_shelf: Some(shelf),
        ..layout_inputs(
            size(px(1000.0), px(700.0)),
            px(0.0),
            TabPosition::Top,
            chrome(Edge::Top),
        )
    });
    let band = layout.workspace.unwrap();
    assert_eq!(band.bottom(), shelf.bottom());
    assert_eq!(band.size.height, BAR_HEIGHT);
    assert!(
        !layout.outside_terminal,
        "the tab bar sits below the safe area"
    );
    assert!(layout.tabs.top() > safe_area.top);
    assert!(layout.terminal.top() >= layout.tabs.bottom());
    // A shelf too short for the bar leaves it below the safe area.
    let short = Bounds::new(point(px(0.0), px(0.0)), size(px(300.0), px(20.0)));
    let below = ChromeLayout::build_with(LayoutInputs {
        safe_area,
        notch_shelf: Some(short),
        ..layout_inputs(
            size(px(1000.0), px(700.0)),
            px(0.0),
            TabPosition::Left,
            chrome(Edge::Top),
        )
    });
    assert_eq!(below.workspace.unwrap().top(), safe_area.top);
}

#[test]
fn the_dropdown_leads_every_strip() {
    let lead = px(110.0);
    let window = size(px(1000.0), px(700.0));
    for position in POSITIONS {
        let plain = ChromeLayout::build_with(layout_inputs(
            window,
            px(28.0),
            position,
            WorkspaceChrome::None,
        ));
        let layout = ChromeLayout::build_with(layout_inputs(
            window,
            px(28.0),
            position,
            WorkspaceChrome::Dropdown { lead },
        ));
        // The terminal and tabs keep their places; only the strip moves.
        assert_eq!(layout.terminal, plain.terminal, "{position:?}");
        assert_eq!(layout.tabs, plain.tabs, "{position:?}");
        assert_eq!(layout.workspace, None);
        let slot = layout.dropdown().expect("a slot");
        assert!(inside(slot, layout.tabs), "{position:?} {slot:?}");
        if position.vertical() {
            assert_eq!(layout.strip_inset, plain.strip_inset + DROPDOWN_ROW);
            assert!(slot.bottom() <= layout.tabs.top() + layout.strip_inset);
        } else {
            assert_eq!(layout.strip_inset, plain.strip_inset + lead);
            assert_eq!(
                slot.left(),
                layout.tabs.left() + plain.strip_inset + DROPDOWN_MARGIN
            );
            assert_eq!(
                slot.size.width,
                lead - DROPDOWN_MARGIN - DROPDOWN_DIVIDER
            );
        }
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
                    for workspace in [
                        chrome(edge),
                        WorkspaceChrome::Dropdown { lead: px(120.0) },
                    ] {
                        let layout = ChromeLayout::build_with(layout_inputs(
                            window, titlebar, position, workspace,
                        ));
                        let parts = [
                            Some(layout.tabs),
                            Some(layout.terminal),
                            layout.workspace,
                            layout.dropdown(),
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
fn a_side_panel_squeezing_a_small_window_resolves_to_the_dropdown() {
    // SW1 reads the terminal width the layout leaves beside the panel
    // and the tab column.
    let resolve_in = |width: f32, position: TabPosition| {
        let window = size(px(width), px(500.0));
        resolve(
            Inputs {
                show: WorkspaceShow::Auto,
                workspaces: 2,
                collapsed: false,
                position: WorkspacePosition::Left,
                width: PANEL_WIDTH,
                dropdown_lead: px(100.0),
            },
            |panel| {
                ChromeLayout::build_with(layout_inputs(
                    window,
                    px(28.0),
                    position,
                    panel,
                ))
                .terminal
                .size
                .width
            },
        )
    };
    assert!(matches!(
        resolve_in(800.0, TabPosition::Titlebar),
        WorkspaceChrome::Panel { .. }
    ));
    assert!(resolve_in(500.0, TabPosition::Titlebar).is_dropdown());
    // A tab column beside the panel counts against the terminal.
    assert!(matches!(
        resolve_in(700.0, TabPosition::Left),
        WorkspaceChrome::Dropdown { .. }
    ));
    assert!(matches!(
        resolve_in(700.0, TabPosition::Titlebar),
        WorkspaceChrome::Panel { .. }
    ));
    assert!(matches!(
        resolve_in(1000.0, TabPosition::Left),
        WorkspaceChrome::Panel { .. }
    ));
}

#[test]
fn an_auto_hidden_bar_hides_a_workspace_bar_only_on_its_edge() {
    let window = size(px(1000.0), px(700.0));
    for (edge, position, joined) in [
        (Edge::Top, TabPosition::Top, true),
        (Edge::Bottom, TabPosition::Bottom, true),
        (Edge::Top, TabPosition::Bottom, false),
        (Edge::Bottom, TabPosition::Top, false),
        (Edge::Left, TabPosition::Left, false),
        (Edge::Left, TabPosition::Top, false),
    ] {
        let built = ChromeLayout::build_with(layout_inputs(
            window,
            px(0.0),
            position,
            chrome(edge),
        ));
        let band = built.workspace.unwrap();
        assert_eq!(built.bar_shares_edge(position), joined, "{edge:?}");
        let hidden = built.present(Presentation::Overlay, position, 0.0);
        let shown = built.present(Presentation::Overlay, position, 1.0);
        let reserved = built.present(Presentation::Reserved, position, 1.0);
        assert_eq!(reserved.terminal, built.terminal);
        // The terminal takes the space of whatever hides.
        let gained = if joined { band.size.height } else { px(0.0) };
        if position.vertical() {
            assert_eq!(
                hidden.terminal.size.width,
                built.terminal.size.width + built.tabs.size.width
            );
        } else {
            assert_eq!(
                hidden.terminal.size.height,
                built.terminal.size.height + built.tabs.size.height + gained,
                "{edge:?} {position:?}"
            );
        }
        // The terminal keeps one rectangle through the reveal, so mouse
        // mapping never moves under the pointer.
        let halfway = built.present(Presentation::Overlay, position, 0.5);
        assert_eq!(shown.terminal, hidden.terminal, "{edge:?} {position:?}");
        assert_eq!(halfway.terminal, hidden.terminal, "{edge:?} {position:?}");
        // Hidden, a joined band slides out with the tabs; revealed, both
        // sit where the reserved layout puts them.
        if joined {
            assert!(!overlap(hidden.workspace.unwrap(), hidden.content));
            assert_eq!(shown.workspace, Some(band));
            assert_eq!(shown.tabs, built.tabs);
            assert_eq!(shown.reveal_region(position), built.tabs.union(&band));
        } else {
            assert_eq!(hidden.workspace, Some(band), "{edge:?} {position:?}");
            assert_eq!(shown.reveal_region(position), built.tabs);
        }
    }
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
                chrome(edge),
            )
        });
        let content = content_inside(size(px(900.0), px(600.0)), frame);
        assert_eq!(layout.content, content);
        assert!(inside(layout.workspace.unwrap(), content), "{edge:?}");
        assert!(inside(layout.area, content), "{edge:?}");
    }
}
