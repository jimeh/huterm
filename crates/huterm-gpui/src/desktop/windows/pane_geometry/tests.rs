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
