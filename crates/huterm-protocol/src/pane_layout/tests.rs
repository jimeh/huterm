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
    // The identity-free check agrees with the transform.
    assert_eq!(
        tiny.check_split(pane_id(1), SplitDirection::Right),
        Err(LayoutError::PaneTooSmall)
    );
    assert_eq!(tiny.check_split(pane_id(1), SplitDirection::Down), Ok(()));
    assert_eq!(
        tiny.check_split(pane_id(9), SplitDirection::Down),
        Err(LayoutError::UnknownPane)
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
    assert_eq!(
        full.check_split(pane_id(1), SplitDirection::Right),
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
    // Removing a direct sibling makes room, so the kept split flattens.
    let flat = kept.without_pane(pane_id(1)).unwrap();
    assert_eq!(flat.split_ids(), [split_id(10)]);
    assert_eq!(flat.pane_count(), MAX_SPLIT_CHILDREN);
}

#[test]
fn a_kept_split_left_alone_replaces_its_parent_and_keeps_its_identity() {
    let PaneNode::Split(full) = full_columns().root().clone() else {
        unreachable!()
    };
    // Columns[pane 900, Columns[256 panes]]: the inner split is kept
    // because flattening it would pass the limit.
    let nested = layout(split(
        20,
        SplitAxis::Columns,
        vec![(HALF, leaf(900)), (HALF, PaneNode::Split(full))],
    ));
    let survivor = nested.without_pane(pane_id(900)).unwrap();
    assert_eq!(survivor.split_ids(), [split_id(10)]);
    assert_eq!(survivor.pane_count(), MAX_SPLIT_CHILDREN);
    let PaneNode::Split(root) = survivor.root() else {
        unreachable!()
    };
    // Its weights are its own, not rescaled into the removed parent.
    assert_eq!(root.children[0].weight, 3906);
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

#[test]
fn inserting_a_subtree_keeps_it_nested_across_the_axis() {
    let columns = layout(split(
        10,
        SplitAxis::Columns,
        vec![(300_000, leaf(1)), (700_000, leaf(2))],
    ));
    let stack = split(
        20,
        SplitAxis::Rows,
        vec![(400_000, leaf(5)), (600_000, leaf(6))],
    );
    // A sibling on the parent's axis keeps its own split and proportions.
    let beside = columns
        .with_inserted(
            pane_id(2),
            SplitDirection::Right,
            stack.clone(),
            split_id(30),
        )
        .unwrap();
    assert_eq!(
        shape(&beside),
        "C[300000:1 350000:2 350000:R[400000:5 600000:6]]"
    );
    assert!(beside.split_node(split_id(20)).is_some());
    assert!(beside.split_node(split_id(30)).is_none());
    // Across the parent's axis, a new split holds the target and the
    // subtree, which keeps its identity because the axes differ.
    let lone = PaneLayout::single(info(1));
    let nested = lone
        .with_inserted(pane_id(1), SplitDirection::Left, stack, split_id(30))
        .unwrap();
    assert_eq!(shape(&nested), "C[500000:R[400000:5 600000:6] 500000:1]");
    assert_eq!(nested.split_ids(), [split_id(30), split_id(20)]);
}

#[test]
fn inserting_a_same_axis_subtree_flattens_it_into_its_new_parent() {
    let columns = layout(split(
        10,
        SplitAxis::Columns,
        vec![(HALF, leaf(1)), (HALF, leaf(2))],
    ));
    let pair = split(
        20,
        SplitAxis::Columns,
        vec![
            (250_000, leaf(5)),
            (
                750_000,
                split(
                    21,
                    SplitAxis::Rows,
                    vec![(HALF, leaf(6)), (HALF, leaf(7))],
                ),
            ),
        ],
    );
    // As a sibling: the subtree's children share pane 2's half.
    let joined = columns
        .with_inserted(
            pane_id(2),
            SplitDirection::Right,
            pair.clone(),
            split_id(30),
        )
        .unwrap();
    assert_eq!(
        shape(&joined),
        "C[500000:1 250000:2 62500:5 187500:R[500000:6 500000:7]]"
    );
    // The subtree's root split is gone; its nested split stays.
    assert_eq!(joined.split_ids(), [split_id(10), split_id(21)]);
    // In a new split across the parent's axis, it flattens into that split.
    let rows = layout(split(
        10,
        SplitAxis::Rows,
        vec![(HALF, leaf(1)), (HALF, leaf(2))],
    ));
    let fresh = rows
        .with_inserted(pane_id(2), SplitDirection::Left, pair, split_id(30))
        .unwrap();
    assert_eq!(
        shape(&fresh),
        "R[500000:1 500000:C[125000:5 375000:R[500000:6 500000:7] 500000:2]]"
    );
    assert_eq!(
        fresh.split_ids(),
        [split_id(10), split_id(30), split_id(21)]
    );
}

#[test]
fn a_flattened_subtree_wider_than_its_slot_reapportions_the_split() {
    let tight = layout(split(
        10,
        SplitAxis::Columns,
        vec![(2, leaf(1)), (SPLIT_TOTAL - 2, leaf(2))],
    ));
    let trio = split(
        20,
        SplitAxis::Columns,
        vec![(300_000, leaf(5)), (300_000, leaf(6)), (400_000, leaf(7))],
    );
    // Pane 1 keeps 1 and the subtree's slot is 1, narrower than its three
    // children; each child takes at least 1 and the largest child absorbs
    // the difference.
    let joined = tight
        .with_inserted(pane_id(1), SplitDirection::Right, trio, split_id(30))
        .unwrap();
    assert_eq!(shape(&joined), "C[1:1 1:5 1:6 1:7 999996:2]");
}

#[test]
fn a_subtree_flatten_past_the_child_limit_keeps_it_nested() {
    let PaneNode::Split(mut root) = full_columns().root().clone() else {
        unreachable!()
    };
    // Make room for exactly one more child.
    let removed = root.children.pop().unwrap();
    root.children[0].weight += removed.weight;
    let crowded = layout(PaneNode::Split(root));
    let pair = split(
        20,
        SplitAxis::Columns,
        vec![(HALF, leaf(300)), (HALF, leaf(301))],
    );
    let kept = crowded
        .with_inserted(pane_id(1), SplitDirection::Right, pair, split_id(30))
        .unwrap();
    let PaneNode::Split(root) = kept.root() else {
        unreachable!()
    };
    assert_eq!(root.children.len(), MAX_SPLIT_CHILDREN);
    assert_eq!(root.children[1].node.id(), NodeId::Split(split_id(20)));
}

#[test]
fn insertion_refuses_unknown_targets_and_repeated_identities() {
    let columns = layout(split(
        10,
        SplitAxis::Columns,
        vec![(HALF, leaf(1)), (HALF, leaf(2))],
    ));
    let right = SplitDirection::Right;
    assert_eq!(
        columns.with_inserted(pane_id(9), right, leaf(5), split_id(30)),
        Err(LayoutError::UnknownPane)
    );
    // A subtree holding a pane the layout already has.
    let reused =
        split(20, SplitAxis::Rows, vec![(HALF, leaf(1)), (HALF, leaf(5))]);
    assert_eq!(
        columns.with_inserted(pane_id(2), right, reused, split_id(30)),
        Err(LayoutError::DuplicateId)
    );
    let tiny = layout(split(
        10,
        SplitAxis::Columns,
        vec![(1, leaf(1)), (SPLIT_TOTAL - 1, leaf(2))],
    ));
    assert_eq!(
        tiny.with_inserted(pane_id(1), right, leaf(5), split_id(30)),
        Err(LayoutError::PaneTooSmall)
    );
}

#[test]
fn swapping_exchanges_panes_and_leaves_each_slot_its_weight() {
    let uneven = left_and_stack()
        .with_weights(
            split_id(10),
            &[NodeId::Pane(pane_id(1)), NodeId::Split(split_id(11))],
            &[300_000, 700_000],
        )
        .unwrap();
    let swapped = uneven.with_swapped(pane_id(1), pane_id(3)).unwrap();
    assert_eq!(shape(&swapped), "C[300000:3 700000:R[500000:2 500000:1]]");
    // Each pane keeps its terminal.
    assert_eq!(
        swapped.pane(pane_id(1)).unwrap().terminal_id,
        TerminalId::new(1)
    );
    assert_eq!(uneven.with_swapped(pane_id(2), pane_id(2)).unwrap(), uneven);
    assert_eq!(
        uneven.with_swapped(pane_id(1), pane_id(9)),
        Err(LayoutError::UnknownPane)
    );
}

#[test]
fn same_shape_ignores_split_identities_and_weights_only() {
    let base = left_and_stack();
    let renamed = layout(split(
        40,
        SplitAxis::Columns,
        vec![
            (100_000, leaf(1)),
            (
                900_000,
                split(
                    41,
                    SplitAxis::Rows,
                    vec![(1, leaf(2)), (SPLIT_TOTAL - 1, leaf(3))],
                ),
            ),
        ],
    ));
    assert!(base.same_shape(&renamed));
    // Panes in other places, or another axis, differ.
    let swapped = base.with_swapped(pane_id(2), pane_id(3)).unwrap();
    assert!(!base.same_shape(&swapped));
    let turned = layout(split(
        10,
        SplitAxis::Rows,
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
    ));
    assert!(!base.same_shape(&turned));
}

#[test]
fn same_structure_compares_split_identities_and_ignores_weights() {
    let base = left_and_stack();
    let reweighted = layout(split(
        10,
        SplitAxis::Columns,
        vec![
            (100_000, leaf(1)),
            (
                900_000,
                split(
                    11,
                    SplitAxis::Rows,
                    vec![(1, leaf(2)), (SPLIT_TOTAL - 1, leaf(3))],
                ),
            ),
        ],
    ));
    assert!(base.same_structure(&reweighted));
    let renamed = layout(split(
        10,
        SplitAxis::Columns,
        vec![
            (HALF, leaf(1)),
            (
                HALF,
                split(
                    41,
                    SplitAxis::Rows,
                    vec![(HALF, leaf(2)), (HALF, leaf(3))],
                ),
            ),
        ],
    ));
    assert!(base.same_shape(&renamed));
    assert!(!base.same_structure(&renamed));
    let swapped = base.with_swapped(pane_id(2), pane_id(3)).unwrap();
    assert!(!base.same_structure(&swapped));
}

#[test]
fn an_insertion_forms_a_split_only_at_the_root_or_across_the_parent_axis() {
    let lone = PaneLayout::single(info(1));
    assert_eq!(
        lone.forms_split(pane_id(1), SplitDirection::Right),
        Ok(true)
    );
    let nested = left_and_stack();
    // Pane 1's parent splits into columns; panes 2 and 3's into rows.
    assert_eq!(
        nested.forms_split(pane_id(1), SplitDirection::Left),
        Ok(false)
    );
    assert_eq!(nested.forms_split(pane_id(1), SplitDirection::Up), Ok(true));
    assert_eq!(
        nested.forms_split(pane_id(3), SplitDirection::Down),
        Ok(false)
    );
    assert_eq!(
        nested.forms_split(pane_id(3), SplitDirection::Right),
        Ok(true)
    );
    assert_eq!(
        nested.forms_split(pane_id(9), SplitDirection::Right),
        Err(LayoutError::UnknownPane)
    );
    // The answer matches whether the insertion used the offered identity.
    for (pane, side) in [
        (1, SplitDirection::Left),
        (1, SplitDirection::Up),
        (3, SplitDirection::Down),
        (3, SplitDirection::Right),
    ] {
        let inserted = nested
            .with_inserted(pane_id(pane), side, leaf(5), split_id(30))
            .unwrap();
        assert_eq!(
            inserted.split_node(split_id(30)).is_some(),
            nested.forms_split(pane_id(pane), side).unwrap(),
            "{pane} {side:?}"
        );
    }
}

/// The layout after moving `source` beside `target` on `side`.
fn moved(
    layout: &PaneLayout,
    source: u64,
    (target, side): (PaneId, SplitDirection),
) -> PaneLayout {
    layout
        .without_pane(pane_id(source))
        .unwrap()
        .with_inserted(target, side, leaf(source), split_id(30))
        .unwrap()
}

#[test]
fn stepping_a_pane_moves_it_one_place_in_traversal_order_with_wrap() {
    let row = layout(split(
        10,
        SplitAxis::Columns,
        vec![(333_334, leaf(1)), (333_333, leaf(2)), (333_333, leaf(3))],
    ));
    // C forward wraps to the front; A back wraps to the end.
    let step = row.step_target(pane_id(3), true).unwrap();
    assert_eq!(step, (pane_id(1), SplitDirection::Left));
    assert_eq!(order(&moved(&row, 3, step)), [3, 1, 2]);
    let step = row.step_target(pane_id(1), false).unwrap();
    assert_eq!(step, (pane_id(3), SplitDirection::Right));
    assert_eq!(order(&moved(&row, 1, step)), [2, 3, 1]);
    // Without a wrap, one place along the row.
    let step = row.step_target(pane_id(1), true).unwrap();
    assert_eq!(step, (pane_id(2), SplitDirection::Right));
    assert_eq!(order(&moved(&row, 1, step)), [2, 1, 3]);
    // A forward from [(A over B) | C]: removing A collapses the stack, so
    // A joins the columns to the right of B.
    let nested = layout(split(
        10,
        SplitAxis::Columns,
        vec![
            (
                HALF,
                split(
                    11,
                    SplitAxis::Rows,
                    vec![(HALF, leaf(1)), (HALF, leaf(2))],
                ),
            ),
            (HALF, leaf(3)),
        ],
    ));
    let step = nested.step_target(pane_id(1), true).unwrap();
    assert_eq!(step, (pane_id(2), SplitDirection::Right));
    assert_eq!(order(&moved(&nested, 1, step)), [2, 1, 3]);
    // A two-pane layout keeps its axis when the target becomes the root.
    let pair = layout(split(
        10,
        SplitAxis::Rows,
        vec![(HALF, leaf(1)), (HALF, leaf(2))],
    ));
    assert_eq!(
        pair.step_target(pane_id(1), true),
        Some((pane_id(2), SplitDirection::Down))
    );
    // Moving 2 forward wraps to before 1, on the leading side.
    assert_eq!(
        pair.step_target(pane_id(2), true),
        Some((pane_id(1), SplitDirection::Up))
    );
    assert_eq!(
        PaneLayout::single(info(1)).step_target(pane_id(1), true),
        None
    );
    assert_eq!(row.step_target(pane_id(9), true), None);
}

/// `[(1 over (2 | 3 | 4)) | 5]` at uneven weights.
fn uneven() -> PaneLayout {
    layout(split(
        10,
        SplitAxis::Columns,
        vec![
            (
                700_000,
                split(
                    11,
                    SplitAxis::Rows,
                    vec![
                        (100_000, leaf(1)),
                        (
                            900_000,
                            split(
                                12,
                                SplitAxis::Columns,
                                vec![
                                    (100_000, leaf(2)),
                                    (200_000, leaf(3)),
                                    (700_000, leaf(4)),
                                ],
                            ),
                        ),
                    ],
                ),
            ),
            (300_000, leaf(5)),
        ],
    ))
}

/// A counter of split identities from 100, recording each one handed out.
fn counter(handed: &mut Vec<u64>) -> impl FnMut() -> SplitId + '_ {
    move || {
        let value = 100 + handed.len() as u64;
        handed.push(value);
        split_id(value)
    }
}

#[test]
fn balancing_gives_equal_weights_at_every_split_and_keeps_the_tree() {
    let uneven = uneven();
    let mut handed = Vec::new();
    let balanced = uneven
        .arranged(&uneven.balanced(), counter(&mut handed))
        .unwrap();
    assert_eq!(
        shape(&balanced),
        "C[500000:R[500000:1 500000:C[333334:2 333333:3 333333:4]] 500000:5]"
    );
    assert_eq!(handed, Vec::<u64>::new());
    assert!(balanced.same_structure(&uneven));
    assert_eq!(balanced.split_ids(), uneven.split_ids());
    // The result depends only on the tree.
    assert_eq!(balanced.balanced(), uneven.balanced());
    assert_eq!(
        balanced.arranged(&balanced.balanced(), counter(&mut handed)),
        Ok(balanced.clone())
    );
    let single = PaneLayout::single(info(1));
    assert_eq!(single.balanced(), Arrangement::Pane(pane_id(1)));
}

#[test]
fn arranging_keeps_split_ids_for_the_same_shape_and_takes_fresh_ones_otherwise()
{
    let uneven = uneven();
    let pane = |value| Arrangement::Pane(pane_id(value));
    // The same shape at new weights keeps every split identity.
    let mut reweighted = uneven.balanced();
    let Arrangement::Split { children, .. } = &mut reweighted else {
        unreachable!()
    };
    children[0].0 = 600_000;
    children[1].0 = 400_000;
    let mut handed = Vec::new();
    let kept = uneven.arranged(&reweighted, counter(&mut handed)).unwrap();
    assert_eq!(handed, Vec::<u64>::new());
    assert_eq!(kept.split_ids(), uneven.split_ids());
    assert_eq!(kept.arrangement(), reweighted);
    // Another shape takes fresh identities in traversal order and keeps
    // each pane's record.
    let rows = |children: Vec<(u32, Arrangement)>| Arrangement::Split {
        axis: SplitAxis::Rows,
        children,
    };
    let columns = |children: Vec<(u32, Arrangement)>| Arrangement::Split {
        axis: SplitAxis::Columns,
        children,
    };
    let other = rows(vec![
        (500_000, columns(vec![(HALF, pane(1)), (HALF, pane(2))])),
        (
            500_000,
            columns(vec![
                (300_000, pane(3)),
                (300_000, pane(4)),
                (400_000, pane(5)),
            ]),
        ),
    ]);
    let arranged = uneven.arranged(&other, counter(&mut handed)).unwrap();
    assert_eq!(handed, [100, 101, 102]);
    assert_eq!(
        arranged.split_ids(),
        [split_id(100), split_id(101), split_id(102)]
    );
    assert_eq!(arranged.arrangement(), other);
    assert_eq!(
        arranged.panes().into_iter().cloned().collect::<Vec<_>>(),
        (1..=5).map(info).collect::<Vec<_>>()
    );
    // A fresh identity already in use is a repeat.
    let mut repeated = || split_id(7);
    assert_eq!(
        uneven.arranged(&other, &mut repeated),
        Err(LayoutError::DuplicateId)
    );
}

#[test]
fn arranging_refuses_to_add_drop_or_repeat_a_pane() {
    let uneven = uneven();
    let pane = |value| Arrangement::Pane(pane_id(value));
    let columns = |panes: &[u64]| Arrangement::Split {
        axis: SplitAxis::Columns,
        children: panes
            .iter()
            .zip(apportion(&vec![1.0; panes.len()]))
            .map(|(value, weight)| (weight, pane(*value)))
            .collect(),
    };
    let mut handed = Vec::new();
    for (arrangement, error) in [
        (columns(&[1, 2, 3, 4, 9]), LayoutError::UnknownPane),
        (columns(&[1, 2, 3, 4, 5, 6]), LayoutError::UnknownPane),
        (columns(&[1, 2, 3, 4, 4]), LayoutError::DuplicateId),
        (columns(&[1, 2, 3, 4]), LayoutError::MissingPane),
        (pane(1), LayoutError::MissingPane),
    ] {
        assert_eq!(
            uneven.arranged(&arrangement, counter(&mut handed)),
            Err(error)
        );
    }
    assert_eq!(handed, Vec::<u64>::new());
    // An arrangement that breaks an invariant fails as the layout would.
    let mut uneven_weights = columns(&[1, 2, 3, 4, 5]);
    let Arrangement::Split { children, .. } = &mut uneven_weights else {
        unreachable!()
    };
    children[0].0 += 1;
    assert_eq!(
        uneven.arranged(&uneven_weights, counter(&mut handed)),
        Err(LayoutError::InvalidWeights)
    );
}
