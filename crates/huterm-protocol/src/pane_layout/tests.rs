use super::*;
use crate::RuntimeId;

const RUNTIME: RuntimeId = RuntimeId::new(3);
const HALF: u32 = SPLIT_TOTAL / 2;

fn pane_id(value: u64) -> PaneId {
    PaneId::in_runtime(RUNTIME, value)
}

fn split_id(value: u64) -> SplitId {
    SplitId::in_runtime(RUNTIME, value)
}

fn info(value: u64) -> PaneInfo {
    PaneInfo {
        id: pane_id(value),
        terminal_id: TerminalId::new(value),
        fallback_name: format!("p{value}"),
    }
}

fn leaf(value: u64) -> PaneNode {
    PaneNode::Pane(info(value))
}

fn split(id: u64, axis: SplitAxis, children: Vec<(u32, PaneNode)>) -> PaneNode {
    PaneNode::Split(SplitNode {
        id: split_id(id),
        axis,
        children: children
            .into_iter()
            .map(|(weight, node)| SplitChild { weight, node })
            .collect(),
    })
}

fn layout(root: PaneNode) -> PaneLayout {
    PaneLayout::new(root).unwrap()
}

/// A compact rendering of a tree: panes by number, splits as `C[...]` or
/// `R[...]` with `weight:child` entries.
fn shape(layout: &PaneLayout) -> String {
    fn render(node: &PaneNode) -> String {
        match node {
            PaneNode::Pane(pane) => pane.id.get().to_string(),
            PaneNode::Split(split) => format!(
                "{}[{}]",
                match split.axis {
                    SplitAxis::Columns => "C",
                    SplitAxis::Rows => "R",
                },
                split
                    .children
                    .iter()
                    .map(|child| format!(
                        "{}:{}",
                        child.weight,
                        render(&child.node)
                    ))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        }
    }
    render(layout.root())
}

fn order(layout: &PaneLayout) -> Vec<u64> {
    layout.panes().iter().map(|pane| pane.id.get()).collect()
}

/// Two columns: pane 1 on the left, and panes 2 over 3 on the right.
fn left_and_stack() -> PaneLayout {
    layout(split(
        10,
        SplitAxis::Columns,
        vec![
            (HALF, leaf(1)),
            (
                HALF,
                split(
                    11,
                    SplitAxis::Rows,
                    vec![(HALF, leaf(2)), (HALF, leaf(3))],
                ),
            ),
        ],
    ))
}

#[test]
fn splitting_a_lone_pane_creates_a_root_split_on_that_side() {
    let single = PaneLayout::single(info(1));
    for (direction, expected) in [
        (SplitDirection::Right, "C[500000:1 500000:2]"),
        (SplitDirection::Left, "C[500000:2 500000:1]"),
        (SplitDirection::Down, "R[500000:1 500000:2]"),
        (SplitDirection::Up, "R[500000:2 500000:1]"),
    ] {
        let split = single
            .with_split(pane_id(1), direction, info(2), split_id(10))
            .unwrap();
        assert_eq!(shape(&split), expected, "{direction:?}");
        assert_eq!(
            split.split_node(split_id(10)).unwrap().axis,
            direction.axis()
        );
    }
    assert_eq!(
        single.with_split(
            pane_id(9),
            SplitDirection::Right,
            info(2),
            split_id(10)
        ),
        Err(LayoutError::UnknownPane)
    );
}

#[test]
fn splitting_along_the_parent_axis_adds_a_sibling_and_halves_the_pane() {
    let columns = layout(split(
        10,
        SplitAxis::Columns,
        vec![(HALF - 1, leaf(1)), (HALF + 1, leaf(2))],
    ));
    // The original keeps the larger half of an odd weight.
    let right = columns
        .with_split(pane_id(2), SplitDirection::Right, info(3), split_id(11))
        .unwrap();
    assert_eq!(shape(&right), "C[499999:1 250001:2 250000:3]");
    let left = columns
        .with_split(pane_id(2), SplitDirection::Left, info(3), split_id(11))
        .unwrap();
    assert_eq!(shape(&left), "C[499999:1 250000:3 250001:2]");
    // The unused split identity is not part of the result.
    assert!(right.split_node(split_id(11)).is_none());
}

#[test]
fn splitting_across_the_parent_axis_nests_a_split_in_the_pane_slot() {
    let columns = layout(split(
        10,
        SplitAxis::Columns,
        vec![(300_000, leaf(1)), (700_000, leaf(2))],
    ));
    let nested = columns
        .with_split(pane_id(2), SplitDirection::Up, info(3), split_id(11))
        .unwrap();
    assert_eq!(shape(&nested), "C[300000:1 700000:R[500000:3 500000:2]]");
    assert_eq!(order(&nested), [1, 3, 2]);
}

#[test]
fn splits_refuse_tiny_panes_full_parents_and_repeated_identities() {
    let tiny = layout(split(
        10,
        SplitAxis::Columns,
        vec![(1, leaf(1)), (SPLIT_TOTAL - 1, leaf(2))],
    ));
    assert_eq!(
        tiny.with_split(
            pane_id(1),
            SplitDirection::Right,
            info(3),
            split_id(11)
        ),
        Err(LayoutError::PaneTooSmall)
    );
    // Across the axis, a weight-1 pane can still split: the new split
    // divides its own space.
    assert!(
        tiny.with_split(
            pane_id(1),
            SplitDirection::Down,
            info(3),
            split_id(11)
        )
        .is_ok()
    );
    assert_eq!(
        tiny.with_split(
            pane_id(2),
            SplitDirection::Right,
            info(1),
            split_id(11)
        ),
        Err(LayoutError::DuplicateId)
    );
    let full = full_columns();
    assert_eq!(
        full.with_split(
            pane_id(1),
            SplitDirection::Right,
            info(999),
            split_id(9)
        ),
        Err(LayoutError::TooManyChildren)
    );
    assert!(
        full.with_split(
            pane_id(1),
            SplitDirection::Down,
            info(999),
            split_id(9)
        )
        .is_ok()
    );
}

/// A columns split at the child limit: panes 1 to 256.
fn full_columns() -> PaneLayout {
    let mut children: Vec<(u32, PaneNode)> = (1..MAX_SPLIT_CHILDREN as u64)
        .map(|v| (3906, leaf(v)))
        .collect();
    children.push((SPLIT_TOTAL - 3906 * 255, leaf(256)));
    layout(split(10, SplitAxis::Columns, children))
}

#[test]
fn removing_a_pane_gives_its_weight_to_the_previous_sibling_or_the_next() {
    let three = layout(split(
        10,
        SplitAxis::Columns,
        vec![(200_000, leaf(1)), (300_000, leaf(2)), (500_000, leaf(3))],
    ));
    assert_eq!(
        shape(&three.without_pane(pane_id(2)).unwrap()),
        "C[500000:1 500000:3]"
    );
    assert_eq!(
        shape(&three.without_pane(pane_id(1)).unwrap()),
        "C[500000:2 500000:3]"
    );
    assert_eq!(
        three.without_pane(pane_id(9)),
        Err(LayoutError::UnknownPane)
    );
    assert_eq!(
        PaneLayout::single(info(1)).without_pane(pane_id(1)),
        Err(LayoutError::LastPane)
    );
    assert_eq!(
        PaneLayout::single(info(1)).without_pane(pane_id(2)),
        Err(LayoutError::UnknownPane)
    );
}

#[test]
fn removing_a_pane_collapses_one_child_splits_at_any_depth() {
    let nested = layout(split(
        10,
        SplitAxis::Columns,
        vec![
            (400_000, leaf(1)),
            (
                600_000,
                split(
                    11,
                    SplitAxis::Rows,
                    vec![(HALF, leaf(2)), (HALF, leaf(3))],
                ),
            ),
        ],
    ));
    assert_eq!(
        shape(&nested.without_pane(pane_id(2)).unwrap()),
        "C[400000:1 600000:3]"
    );
    let pair = nested.without_pane(pane_id(2)).unwrap();
    assert_eq!(shape(&pair.without_pane(pane_id(1)).unwrap()), "3");
}

#[test]
fn a_promoted_same_axis_split_flattens_with_floored_weights_and_a_remainder() {
    let nested = layout(split(
        10,
        SplitAxis::Columns,
        vec![
            (399_999, leaf(1)),
            (
                600_001,
                split(
                    11,
                    SplitAxis::Rows,
                    vec![
                        (300_000, leaf(2)),
                        (
                            700_000,
                            split(
                                12,
                                SplitAxis::Columns,
                                vec![(333_333, leaf(3)), (666_667, leaf(4))],
                            ),
                        ),
                    ],
                ),
            ),
        ],
    ));
    // 333,333 and 666,667 scaled into a 600,001 slot floor to 200,000 and
    // 400,000; the missing 1 goes to the largest child, pane 4.
    let flat = nested.without_pane(pane_id(2)).unwrap();
    assert_eq!(shape(&flat), "C[399999:1 200000:3 400001:4]");
}

#[test]
fn flattening_into_a_weight_one_slot_reapportions_the_whole_split() {
    let nested = layout(split(
        10,
        SplitAxis::Columns,
        vec![
            (
                1,
                split(
                    11,
                    SplitAxis::Rows,
                    vec![
                        (HALF, leaf(2)),
                        (
                            HALF,
                            split(
                                12,
                                SplitAxis::Columns,
                                vec![(HALF, leaf(3)), (HALF, leaf(4))],
                            ),
                        ),
                    ],
                ),
            ),
            (SPLIT_TOTAL - 1, leaf(1)),
        ],
    ));
    // Both promoted panes floor to 0 and rise to 1; pane 1 gives up the
    // extra weight.
    let flat = nested.without_pane(pane_id(2)).unwrap();
    assert_eq!(shape(&flat), "C[1:3 1:4 999998:1]");
}

#[test]
fn a_flatten_past_the_child_limit_keeps_the_nested_split() {
    let PaneNode::Split(mut root) = full_columns().root().clone() else {
        unreachable!()
    };
    root.children[255].node = split(
        11,
        SplitAxis::Rows,
        vec![
            (HALF, leaf(300)),
            (
                HALF,
                split(
                    12,
                    SplitAxis::Columns,
                    vec![(HALF, leaf(301)), (HALF, leaf(302))],
                ),
            ),
        ],
    );
    let crowded = layout(PaneNode::Split(root));
    let kept = crowded.without_pane(pane_id(300)).unwrap();
    let PaneNode::Split(root) = kept.root() else {
        unreachable!()
    };
    assert_eq!(root.children.len(), MAX_SPLIT_CHILDREN);
    let PaneNode::Split(last) = &root.children[255].node else {
        panic!("the same-axis split was flattened past the limit");
    };
    assert_eq!((last.id, last.axis), (split_id(12), SplitAxis::Columns));
}

#[test]
fn weights_change_only_for_the_expected_children_and_valid_sums() {
    let pair = left_and_stack();
    let children = [NodeId::Pane(pane_id(1)), NodeId::Split(split_id(11))];
    let resized = pair
        .with_weights(split_id(10), &children, &[250_000, 750_000])
        .unwrap();
    assert_eq!(shape(&resized), "C[250000:1 750000:R[500000:2 500000:3]]");
    assert_eq!(
        pair.with_weights(split_id(99), &children, &[HALF, HALF]),
        Err(LayoutError::UnknownSplit)
    );
    // Same count, different arrangement.
    let swapped = [NodeId::Split(split_id(11)), NodeId::Pane(pane_id(1))];
    assert_eq!(
        pair.with_weights(split_id(10), &swapped, &[HALF, HALF]),
        Err(LayoutError::ChildrenChanged)
    );
    assert_eq!(
        pair.with_weights(split_id(10), &children[..1], &[SPLIT_TOTAL]),
        Err(LayoutError::ChildrenChanged)
    );
    for weights in [&[HALF][..], &[HALF, HALF + 1], &[0, SPLIT_TOTAL]] {
        assert_eq!(
            pair.with_weights(split_id(10), &children, weights),
            Err(LayoutError::InvalidWeights),
            "{weights:?}"
        );
    }
}

#[test]
fn validation_rejects_each_broken_invariant() {
    let cases = [
        (
            split(10, SplitAxis::Columns, vec![(SPLIT_TOTAL, leaf(1))]),
            LayoutError::TooFewChildren,
        ),
        (
            split(
                10,
                SplitAxis::Columns,
                vec![(HALF, leaf(1)), (HALF - 1, leaf(2))],
            ),
            LayoutError::InvalidWeights,
        ),
        (
            split(
                10,
                SplitAxis::Columns,
                vec![(0, leaf(1)), (SPLIT_TOTAL, leaf(2))],
            ),
            LayoutError::InvalidWeights,
        ),
        (
            split(
                10,
                SplitAxis::Columns,
                vec![
                    (HALF, leaf(1)),
                    (
                        HALF,
                        split(
                            11,
                            SplitAxis::Columns,
                            vec![(HALF, leaf(2)), (HALF, leaf(3))],
                        ),
                    ),
                ],
            ),
            LayoutError::SameAxisChild,
        ),
        (
            split(
                10,
                SplitAxis::Columns,
                vec![(HALF, leaf(1)), (HALF, leaf(1))],
            ),
            LayoutError::DuplicateId,
        ),
        (
            split(
                10,
                SplitAxis::Columns,
                vec![
                    (HALF, leaf(1)),
                    (
                        HALF,
                        PaneNode::Pane(PaneInfo {
                            id: pane_id(2),
                            ..info(1)
                        }),
                    ),
                ],
            ),
            LayoutError::DuplicateId,
        ),
        (
            split(
                10,
                SplitAxis::Columns,
                vec![
                    (HALF, leaf(1)),
                    (
                        HALF,
                        split(
                            10,
                            SplitAxis::Rows,
                            vec![(HALF, leaf(2)), (HALF, leaf(3))],
                        ),
                    ),
                ],
            ),
            LayoutError::DuplicateId,
        ),
        (
            {
                let mut children: Vec<(u32, PaneNode)> =
                    (1..=257).map(|v| (3891, leaf(v))).collect();
                children[0].0 = SPLIT_TOTAL - 3891 * 256;
                split(10, SplitAxis::Columns, children)
            },
            LayoutError::TooManyChildren,
        ),
    ];
    for (root, error) in cases {
        assert_eq!(PaneLayout::new(root), Err(error));
    }
}

#[test]
fn traversal_is_depth_first_and_cycling_wraps() {
    let stack = left_and_stack();
    assert_eq!(order(&stack), [1, 2, 3]);
    assert_eq!(stack.first_pane().id, pane_id(1));
    assert_eq!(stack.pane_count(), 3);
    assert_eq!(stack.cycle(pane_id(1), true), Some(pane_id(2)));
    assert_eq!(stack.cycle(pane_id(3), true), Some(pane_id(1)));
    assert_eq!(stack.cycle(pane_id(1), false), Some(pane_id(3)));
    assert_eq!(stack.cycle(pane_id(9), true), None);
    assert_eq!(PaneLayout::single(info(1)).cycle(pane_id(1), true), None);
}

#[test]
fn normalized_geometry_shares_boundaries_between_neighbours() {
    let uneven = layout(split(
        10,
        SplitAxis::Columns,
        vec![
            (250_000, leaf(1)),
            (
                750_000,
                split(
                    11,
                    SplitAxis::Rows,
                    vec![(HALF, leaf(2)), (HALF, leaf(3))],
                ),
            ),
        ],
    ));
    let extent = NORMALIZED_EXTENT;
    assert_eq!(
        uneven.normalized_rects(),
        [
            (
                pane_id(1),
                NormalizedRect {
                    x0: 0,
                    y0: 0,
                    x1: extent / 4,
                    y1: extent
                }
            ),
            (
                pane_id(2),
                NormalizedRect {
                    x0: extent / 4,
                    y0: 0,
                    x1: extent,
                    y1: extent / 2
                }
            ),
            (
                pane_id(3),
                NormalizedRect {
                    x0: extent / 4,
                    y0: extent / 2,
                    x1: extent,
                    y1: extent
                }
            ),
        ]
    );
}

/// Alternating columns and rows, each giving weight 1 to a deeper split
/// and the rest to one pane, five columns levels deep, so the innermost
/// first pane has no width.
fn needle() -> PaneLayout {
    let mut node = split(
        100,
        SplitAxis::Columns,
        vec![(1, leaf(50)), (SPLIT_TOTAL - 1, leaf(51))],
    );
    for level in 0..8_u64 {
        let axis = if level % 2 == 0 {
            SplitAxis::Rows
        } else {
            SplitAxis::Columns
        };
        node = split(
            101 + level,
            axis,
            vec![(1, node), (SPLIT_TOTAL - 1, leaf(60 + level))],
        );
    }
    layout(node)
}

#[test]
fn zero_extent_panes_are_never_directional_targets() {
    let needle = needle();
    let rects = needle.normalized_rects();
    let zero = rects.iter().find(|(id, _)| *id == pane_id(50)).unwrap().1;
    assert_eq!(zero.x0, zero.x1, "{zero:?}");
    assert_eq!(
        needle.neighbor(pane_id(51), SplitDirection::Left, &[]),
        None
    );
    assert_eq!(
        needle.neighbor(pane_id(50), SplitDirection::Right, &[]),
        Some(pane_id(51))
    );
}

#[test]
fn directional_neighbours_stop_at_edges_and_break_ties_as_documented() {
    let stack = left_and_stack();
    assert_eq!(stack.neighbor(pane_id(1), SplitDirection::Left, &[]), None);
    assert_eq!(stack.neighbor(pane_id(2), SplitDirection::Up, &[]), None);
    assert_eq!(
        stack.neighbor(pane_id(2), SplitDirection::Left, &[]),
        Some(pane_id(1))
    );
    assert_eq!(
        stack.neighbor(pane_id(2), SplitDirection::Down, &[]),
        Some(pane_id(3))
    );
    // Equal distance and overlap: traversal order, unless history says
    // otherwise.
    assert_eq!(
        stack.neighbor(pane_id(1), SplitDirection::Right, &[]),
        Some(pane_id(2))
    );
    assert_eq!(
        stack.neighbor(
            pane_id(1),
            SplitDirection::Right,
            &[pane_id(3), pane_id(2)]
        ),
        Some(pane_id(3))
    );
    assert_eq!(stack.neighbor(pane_id(9), SplitDirection::Right, &[]), None);

    // Top row splits 30/70 and the bottom row 30/70 and 60/40.
    let rows = |bottom: u32| {
        layout(split(
            10,
            SplitAxis::Rows,
            vec![
                (
                    HALF,
                    split(
                        11,
                        SplitAxis::Columns,
                        vec![(300_000, leaf(1)), (700_000, leaf(2))],
                    ),
                ),
                (
                    HALF,
                    split(
                        12,
                        SplitAxis::Columns,
                        vec![
                            (bottom, leaf(3)),
                            (SPLIT_TOTAL - bottom, leaf(4)),
                        ],
                    ),
                ),
            ],
        ))
    };
    // Pane 4 only touches pane 1's corner; pane 3 overlaps it.
    let aligned = rows(300_000);
    assert_eq!(
        aligned.neighbor(pane_id(1), SplitDirection::Down, &[pane_id(4)]),
        Some(pane_id(3))
    );
    // Both overlap pane 2; pane 4's overlap is longer, whatever the history.
    let offset = rows(600_000);
    assert_eq!(
        offset.neighbor(pane_id(2), SplitDirection::Down, &[pane_id(3)]),
        Some(pane_id(4))
    );
}
