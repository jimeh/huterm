use super::*;
use huterm_protocol::{PaneInfo, RuntimeId, SplitChild, SplitNode, TerminalId};

fn columns(weights: &[u32]) -> PaneLayout {
    PaneLayout::new(split(
        10,
        SplitAxis::Columns,
        weights
            .iter()
            .zip(1..)
            .map(|(weight, value)| (*weight, pane(value)))
            .collect(),
    ))
    .unwrap()
}

fn root_split(layout: &PaneLayout) -> &SplitNode {
    match layout.root() {
        PaneNode::Split(split) => split,
        PaneNode::Pane(_) => panic!("not split"),
    }
}

const RUNTIME: RuntimeId = RuntimeId::new(9);

fn info(value: u64) -> PaneInfo {
    PaneInfo {
        id: id(value),
        terminal_id: TerminalId::new(value),
        fallback_name: "sh".into(),
    }
}

fn pane(value: u64) -> PaneNode {
    PaneNode::Pane(info(value))
}

fn split(id: u64, axis: SplitAxis, children: Vec<(u32, PaneNode)>) -> PaneNode {
    PaneNode::Split(SplitNode {
        id: SplitId::in_runtime(RUNTIME, id),
        axis,
        children: children
            .into_iter()
            .map(|(weight, node)| SplitChild { weight, node })
            .collect(),
    })
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
}

fn id(value: u64) -> PaneId {
    PaneId::in_runtime(RUNTIME, value)
}

#[test]
fn a_lone_pane_fills_the_tab_without_a_title_bar() {
    let content = rect(10.0, 20.0, 400.0, 300.0);
    let panes = layout_panes(&PaneLayout::single(info(1)), content, 2.0, None);
    assert_eq!(
        panes.panes,
        [PaneRect {
            pane: id(1),
            frame: content,
            title_bar: None,
            terminal: content,
        }]
    );
    assert_eq!(panes.dividers, []);
}

#[test]
fn split_panes_share_the_tab_with_dividers_and_title_bars() {
    // Columns: pane 1 takes a quarter; pane 2 over pane 3 take the rest.
    let layout = PaneLayout::new(split(
        10,
        SplitAxis::Columns,
        vec![
            (250_000, pane(1)),
            (
                750_000,
                split(
                    11,
                    SplitAxis::Rows,
                    vec![(500_000, pane(2)), (500_000, pane(3))],
                ),
            ),
        ],
    ))
    .unwrap();
    let content = rect(0.0, 0.0, 401.0, 301.0);
    let panes = layout_panes(&layout, content, 1.0, None);
    let [first, second, third] = panes.panes.as_slice() else {
        panic!("{panes:?}");
    };
    // 400 points remain beside the 1-point divider; a quarter is 100.
    assert_eq!(first.frame, rect(0.0, 0.0, 100.0, 301.0));
    assert_eq!(first.title_bar, Some(rect(0.0, 0.0, 100.0, 24.0)));
    assert_eq!(first.terminal, rect(0.0, 24.0, 100.0, 277.0));
    // The rows split the remaining 300 points of height evenly.
    assert_eq!(second.frame, rect(101.0, 0.0, 300.0, 150.0));
    assert_eq!(third.frame, rect(101.0, 151.0, 300.0, 150.0));
    // The last child ends at the tab's far edge.
    assert_eq!(third.frame.right(), content.right());
    assert_eq!(third.frame.bottom(), content.bottom());
    let lines: Vec<_> = panes
        .dividers
        .iter()
        .map(|divider| {
            (
                divider.split.get(),
                divider.index,
                divider.axis,
                divider.line,
            )
        })
        .collect();
    assert_eq!(
        lines,
        [
            (10, 0, SplitAxis::Columns, rect(100.0, 0.0, 1.0, 301.0)),
            (11, 0, SplitAxis::Rows, rect(101.0, 150.0, 300.0, 1.0)),
        ]
    );
}

#[test]
fn boundaries_round_to_physical_pixels_and_never_overlap_in_short_space() {
    let layout = PaneLayout::new(split(
        10,
        SplitAxis::Columns,
        vec![(333_333, pane(1)), (333_333, pane(2)), (333_334, pane(3))],
    ))
    .unwrap();
    let panes = layout_panes(&layout, rect(0.0, 0.0, 100.0, 50.0), 2.0, None);
    for pane in &panes.panes {
        let edges = [pane.frame.origin.x, pane.frame.right()];
        assert!(
            edges
                .iter()
                .all(|edge| (f32::from(*edge) * 2.0).fract() == 0.0),
            "{:?} is not on a physical pixel",
            pane.frame
        );
    }
    // Less room than the dividers need leaves empty panes, not negative
    // or overlapping ones.
    let tiny = layout_panes(&layout, rect(0.0, 0.0, 1.0, 50.0), 1.0, None);
    assert!(
        tiny.panes
            .iter()
            .all(|pane| pane.frame.size.width >= px(0.0))
    );
    assert!(
        tiny.panes
            .windows(2)
            .all(|pair| pair[0].frame.right() <= pair[1].frame.origin.x)
    );
}

#[test]
fn a_zoomed_pane_fills_the_tab_with_its_title_bar_and_no_dividers() {
    let layout = columns(&[500_000, 500_000]);
    let content = rect(0.0, 0.0, 401.0, 300.0);
    let zoomed = layout_panes(&layout, content, 1.0, Some(id(2)));
    assert_eq!(
        zoomed.panes,
        [PaneRect {
            pane: id(2),
            frame: content,
            title_bar: Some(rect(0.0, 0.0, 401.0, 24.0)),
            terminal: rect(0.0, 24.0, 401.0, 276.0),
        }]
    );
    assert_eq!(zoomed.dividers, []);
    // A zoom naming a pane the layout does not hold is ignored, and so is
    // zoom in a tab of one pane.
    let stale = layout_panes(&layout, content, 1.0, Some(id(9)));
    assert_eq!(stale, layout_panes(&layout, content, 1.0, None));
    let lone =
        layout_panes(&PaneLayout::single(info(1)), content, 1.0, Some(id(1)));
    assert_eq!(lone.panes[0].title_bar, None);
}

#[test]
fn a_dividers_grab_zone_straddles_its_line_inside_the_split() {
    let layout = columns(&[500_000, 500_000]);
    let content = rect(0.0, 0.0, 401.0, 300.0);
    let panes = layout_panes(&layout, content, 1.0, None);
    let [divider] = panes.dividers.as_slice() else {
        panic!("{panes:?}");
    };
    assert_eq!(divider.line, rect(200.0, 0.0, 1.0, 300.0));
    assert_eq!(divider.grab, rect(196.5, 0.0, 8.0, 300.0));
    assert_eq!(divider.span, content);
    // Near the split's edge the zone is clipped to the split.
    let skewed = layout_panes(&columns(&[1, 999_999]), content, 1.0, None);
    assert_eq!(skewed.dividers[0].grab.origin.x, px(0.0));
}

#[test]
fn a_drag_moves_one_boundary_and_keeps_both_neighbours() {
    let layout = columns(&[250_000, 250_000, 500_000]);
    let split = root_split(&layout);
    // Two dividers leave 400 points of 402; the second boundary sits at 201.
    let span = rect(0.0, 0.0, 402.0, 300.0);
    let moved = dragged_weights(split, 1, span, px(301.5));
    assert_eq!(moved, [250_000, 500_000, 250_000]);
    assert_eq!(moved.iter().sum::<u32>(), SPLIT_TOTAL);
    let back = boundary_position(
        root_split(
            &layout
                .with_weights(split.id, &node_ids(split), &moved)
                .unwrap(),
        ),
        1,
        span,
    );
    assert_eq!(back, px(301.5));
    // Past either neighbour the boundary stops one weight unit short.
    assert_eq!(
        dragged_weights(split, 1, span, px(-50.0)),
        [250_000, 1, 749_999]
    );
    assert_eq!(
        dragged_weights(split, 1, span, px(900.0)),
        [250_000, 749_999, 1]
    );
}

fn node_ids(split: &SplitNode) -> Vec<huterm_protocol::NodeId> {
    split.children.iter().map(|child| child.node.id()).collect()
}

#[test]
fn a_resize_moves_the_nearest_boundary_on_its_axis_beside_the_pane() {
    // Columns: pane 1 | (pane 2 over pane 3).
    let layout = PaneLayout::new(split(
        10,
        SplitAxis::Columns,
        vec![
            (500_000, pane(1)),
            (
                500_000,
                split(
                    11,
                    SplitAxis::Rows,
                    vec![(500_000, pane(2)), (500_000, pane(3))],
                ),
            ),
        ],
    ))
    .unwrap();
    let found = |pane, axis, leading| {
        resize_boundary(&layout, id(pane), axis, leading)
            .map(|(split, index)| (split.id.get(), index))
    };
    // Pane 3's left edge is the outer split's only boundary.
    assert_eq!(found(3, SplitAxis::Columns, true), Some((10, 0)));
    assert_eq!(found(3, SplitAxis::Columns, false), None);
    // Its top edge is the inner split's boundary; it has no bottom one.
    assert_eq!(found(3, SplitAxis::Rows, true), Some((11, 0)));
    assert_eq!(found(3, SplitAxis::Rows, false), None);
    assert_eq!(found(1, SplitAxis::Columns, false), Some((10, 0)));
    assert_eq!(found(1, SplitAxis::Rows, true), None);
    assert_eq!(found(9, SplitAxis::Columns, true), None);
}

#[test]
fn a_carried_boundary_keeps_the_other_childrens_weights() {
    // The boundary after child 0 sits at 300,000; child 2 changed elsewhere.
    let layout = columns(&[400_000, 100_000, 500_000]);
    let split = root_split(&layout);
    assert_eq!(
        with_boundary(split, 0, 300_000),
        [300_000, 200_000, 500_000]
    );
    // Past the neighbour the boundary stops one unit short of it.
    assert_eq!(with_boundary(split, 0, 900_000), [499_999, 1, 500_000]);
    assert_eq!(with_boundary(split, 1, 0), [400_000, 1, 599_999]);
}

#[test]
fn drop_zones_split_a_frame_into_edge_quarters_and_a_centre() {
    use SplitDirection::{Down, Left, Right, Up};
    // A 400 by 200 frame whose top title bar is part of it.
    let frame =
        Bounds::new(point(px(100.0), px(50.0)), size(px(400.0), px(200.0)));
    let at =
        |x: f32, y: f32| drop_zone(frame, point(px(100.0 + x), px(50.0 + y)));
    assert_eq!(at(10.0, 100.0), Some(Zone::Edge(Left)));
    assert_eq!(at(390.0, 100.0), Some(Zone::Edge(Right)));
    assert_eq!(
        at(200.0, 5.0),
        Some(Zone::Edge(Up)),
        "the title bar is in the frame"
    );
    assert_eq!(at(200.0, 190.0), Some(Zone::Edge(Down)));
    assert_eq!(at(200.0, 100.0), Some(Zone::Centre));
    assert_eq!(
        at(100.0, 50.0),
        Some(Zone::Centre),
        "a quarter's inner edge is centre"
    );
    // Near a corner the edge nearer as a fraction of its own extent wins.
    assert_eq!(
        at(20.0, 40.0),
        Some(Zone::Edge(Left)),
        "5% across beats 20% down"
    );
    assert_eq!(
        at(80.0, 4.0),
        Some(Zone::Edge(Up)),
        "2% down beats 20% across"
    );
    assert_eq!(
        at(392.0, 196.0),
        Some(Zone::Edge(Down)),
        "2% up beats 2% across: a tie"
    );
    assert_eq!(
        at(380.0, 4.0),
        Some(Zone::Edge(Up)),
        "2% down beats 5% across"
    );
    assert_eq!(
        at(396.0, 180.0),
        Some(Zone::Edge(Right)),
        "1% across beats 10% up"
    );
    // Exact ties go to the top or bottom edge, on both diagonals.
    assert_eq!(at(20.0, 10.0), Some(Zone::Edge(Up)));
    assert_eq!(at(380.0, 190.0), Some(Zone::Edge(Down)));
    // Outside the frame there is no zone.
    assert_eq!(at(-1.0, 100.0), None);
    assert_eq!(at(200.0, 200.0), None);
}

#[test]
fn a_zone_previews_the_half_an_edge_drop_takes_or_the_whole_frame() {
    let frame =
        Bounds::new(point(px(100.0), px(50.0)), size(px(400.0), px(200.0)));
    assert_eq!(zone_preview(frame, Zone::Centre), frame);
    assert_eq!(
        zone_preview(frame, Zone::Edge(SplitDirection::Right)),
        Bounds::new(point(px(300.0), px(50.0)), size(px(200.0), px(200.0)))
    );
    assert_eq!(
        zone_preview(frame, Zone::Edge(SplitDirection::Up)),
        Bounds::new(point(px(100.0), px(50.0)), size(px(400.0), px(100.0)))
    );
}

/// Cell metrics as a 12-point monospace font measures them, aligned to
/// physical pixels at `scale` as the window aligns them.
fn metrics(scale: f32, padding: (f32, f32)) -> PaneMetrics {
    let grid = crate::renderer::GridMetrics::fixed(7.25, 14.6).at_scale(scale);
    PaneMetrics {
        cell: size(grid.cell_width, grid.cell_height),
        window: WindowConfig {
            padding_x: padding.0,
            padding_y: padding.1,
            ..WindowConfig::default()
        },
    }
}

/// Each pane's grid in `content`, as the window computes it for a split
/// preflight.
fn grids(
    layout: &PaneLayout,
    content: Bounds<Pixels>,
    scale: f32,
    metrics: PaneMetrics,
) -> Vec<(PaneId, huterm_protocol::GridSize)> {
    layout_panes(layout, content, scale, None)
        .panes
        .iter()
        .map(|rect| (rect.pane, metrics.grid(rect.terminal.size)))
        .collect()
}

/// The layout `fit` committed, for asserting through the real grids.
fn placed(layout: &PaneLayout, fitted: &Fit) -> PaneLayout {
    let (Fit::Fits(arrangement) | Fit::Short(arrangement)) = fitted;
    layout.arranged(arrangement, probe_splits()).unwrap()
}

fn weights_of(arrangement: &Arrangement) -> Vec<u32> {
    match arrangement {
        Arrangement::Pane(_) => Vec::new(),
        Arrangement::Split { children, .. } => children
            .iter()
            .flat_map(|(weight, child)| {
                std::iter::once(*weight).chain(weights_of(child))
            })
            .collect(),
    }
}

/// `[p1 | (p2 over p3)]` with the main pane at `main` of the width.
fn main_left(main: u32) -> PaneLayout {
    PaneLayout::new(split(
        10,
        SplitAxis::Columns,
        vec![
            (main, pane(1)),
            (
                SPLIT_TOTAL - main,
                split(
                    11,
                    SplitAxis::Rows,
                    vec![(500_000, pane(2)), (500_000, pane(3))],
                ),
            ),
        ],
    ))
    .unwrap()
}

/// `layout`'s own tree as an arrangement, weights included.
fn arrangement_of(layout: &PaneLayout) -> Arrangement {
    fn of(node: &PaneNode) -> Arrangement {
        match node {
            PaneNode::Pane(pane) => Arrangement::Pane(pane.id),
            PaneNode::Split(split) => Arrangement::Split {
                axis: split.axis,
                children: split
                    .children
                    .iter()
                    .map(|child| (child.weight, of(&child.node)))
                    .collect(),
            },
        }
    }
    of(layout.root())
}

#[test]
fn a_large_window_fits_the_ideal_arrangement_unchanged() {
    let layout = main_left(600_000);
    let ideal = arrangement_of(&layout);
    for scale in [1.0, 2.0] {
        let fitted = fit(
            &layout,
            &ideal,
            rect(0.0, 28.0, 1600.0, 900.0),
            scale,
            metrics(scale, (4.0, 2.0)),
        );
        assert_eq!(fitted, Fit::Fits(ideal.clone()), "at {scale}x");
    }
}

#[test]
fn a_main_share_that_starves_the_stack_shrinks_main_just_enough() {
    let scale = 2.0;
    let metrics = metrics(scale, (4.0, 2.0));
    let layout = main_left(600_000);
    // Wide enough for both minimums, too narrow for the stack's 40 %.
    let content = rect(0.0, 28.0, 90.0, 600.0);
    let ideal = arrangement_of(&layout);
    let before = grids(&layout, content, scale, metrics);
    assert!(!fits_minimum(&before), "the ideal already fits: {before:?}");
    let fitted = fit(&layout, &ideal, content, scale, metrics);
    let Fit::Fits(arrangement) = &fitted else {
        panic!("the minimums fit, yet {fitted:?}");
    };
    let after = grids(&placed(&layout, &fitted), content, scale, metrics);
    assert!(fits_minimum(&after), "{after:?}");
    // The stack takes exactly its minimum, and main keeps the rest.
    let columns = |pane: u64| {
        after
            .iter()
            .find(|(placed, _)| *placed == id(pane))
            .unwrap()
            .1
            .columns
    };
    assert_eq!(
        (columns(2), columns(3)),
        (MIN_PANE_COLUMNS, MIN_PANE_COLUMNS)
    );
    assert!(columns(1) > MIN_PANE_COLUMNS, "{after:?}");
    // The stack's own rows keep their ideal halves.
    assert_eq!(weights_of(arrangement)[2..], [500_000, 500_000]);
}

#[test]
fn two_columns_fit_exactly_where_a_split_preflight_accepts_them() {
    for scale in [1.0_f32, 2.0] {
        for padding in [(0.0, 0.0), (6.0, 4.0)] {
            let metrics = metrics(scale, padding);
            let single = PaneLayout::single(info(1));
            let split = single
                .with_split(
                    id(1),
                    SplitDirection::Right,
                    info(2),
                    SplitId::in_runtime(RUNTIME, 20),
                )
                .unwrap();
            let content = |width: f32| rect(0.0, 28.0, width, 400.0);
            let pixel = 1.0 / scale;
            // The smallest width the preflight accepts, in physical pixels.
            let mut width = pixel;
            while !fits_minimum(&grids(&split, content(width), scale, metrics))
            {
                width += pixel;
            }
            let ideal = split.balanced();
            let at = |width: f32| {
                fit(&split, &ideal, content(width), scale, metrics)
            };
            let label = format!("{scale}x, padding {padding:?}, {width}");
            assert_eq!(at(width), Fit::Fits(ideal.clone()), "{label}");
            assert!(matches!(at(width - pixel), Fit::Short(_)), "{label}");
        }
    }
}

#[test]
fn a_tree_that_cannot_fit_shares_space_in_proportion_to_its_minimums() {
    let scale = 1.0;
    let metrics = metrics(scale, (0.0, 0.0));
    let layout = main_left(600_000);
    // Narrower than one pane's minimum beside the stack's.
    let content = rect(0.0, 28.0, 40.0, 600.0);
    let fitted =
        fit(&layout, &arrangement_of(&layout), content, scale, metrics);
    let Fit::Short(arrangement) = &fitted else {
        panic!("a window this narrow fit: {fitted:?}");
    };
    let after = grids(&placed(&layout, &fitted), content, scale, metrics);
    assert!(!fits_minimum(&after));
    // Main and the stack need the same width, so they share it equally.
    assert_eq!(weights_of(arrangement)[..2], [500_000, 500_000]);
}

#[test]
fn balancing_twice_at_one_size_fits_the_same_weights() {
    let scale = 2.0;
    let metrics = metrics(scale, (4.0, 2.0));
    let content = rect(0.0, 28.0, 140.0, 600.0);
    let layout = main_left(830_000);
    let once = fit(&layout, &layout.balanced(), content, scale, metrics);
    let balanced = placed(&layout, &once);
    let twice = fit(&balanced, &balanced.balanced(), content, scale, metrics);
    assert_eq!(once, twice);
}
