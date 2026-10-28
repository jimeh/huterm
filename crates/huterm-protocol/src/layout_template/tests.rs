use super::*;
use crate::{
    PaneInfo, PaneNode, RuntimeId, SPLIT_TOTAL, SplitChild, SplitId, SplitNode,
    TerminalId,
};

const RUNTIME: RuntimeId = RuntimeId::new(5);

/// Panes 1 to `count`, in traversal order.
fn ids(count: usize) -> Vec<PaneId> {
    (1..=count as u64)
        .map(|value| PaneId::in_runtime(RUNTIME, value))
        .collect()
}

fn template(source: &str) -> LayoutTemplate {
    LayoutTemplate::parse(source).unwrap()
}

fn built_in(name: &str) -> &'static NamedLayout {
    NamedLayout::built_in(name).unwrap()
}

/// A compact canonical rendering: panes by number, splits as `C[...]` or
/// `R[...]` with `weight:child` entries.
fn render(arrangement: &Arrangement) -> String {
    match arrangement {
        Arrangement::Pane(pane) => pane.get().to_string(),
        Arrangement::Split { axis, children } => format!(
            "{}[{}]",
            match axis {
                SplitAxis::Columns => "C",
                SplitAxis::Rows => "R",
            },
            children
                .iter()
                .map(|(weight, child)| format!("{weight}:{}", render(child)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    }
}

fn expand(source: &str, count: usize) -> Result<String, LayoutError> {
    template(source).expand(&ids(count)).map(|a| render(&a))
}

/// A layout of `arrangement`'s panes, with split identities from 1000.
fn layout_of(arrangement: &Arrangement) -> PaneLayout {
    fn node(arrangement: &Arrangement, next: &mut u64) -> PaneNode {
        match arrangement {
            Arrangement::Pane(pane) => PaneNode::Pane(PaneInfo {
                id: *pane,
                terminal_id: TerminalId::new(pane.get()),
                fallback_name: format!("p{}", pane.get()),
            }),
            Arrangement::Split { axis, children } => {
                *next += 1;
                let id = SplitId::in_runtime(RUNTIME, *next);
                PaneNode::Split(SplitNode {
                    id,
                    axis: *axis,
                    children: children
                        .iter()
                        .map(|(weight, child)| SplitChild {
                            weight: *weight,
                            node: node(child, next),
                        })
                        .collect(),
                })
            }
        }
    }
    PaneLayout::new(node(arrangement, &mut 1000)).unwrap()
}

/// `SPLIT_TOTAL` divided among `count` children, the remainder going one
/// unit each to the first: the oracle for equal weights.
fn equal(count: usize) -> Vec<u32> {
    let count = u32::try_from(count).unwrap();
    (0..count)
        .map(|index| {
            SPLIT_TOTAL / count + u32::from(index < SPLIT_TOTAL % count)
        })
        .collect()
}

/// Panes `first ..= last` at equal weights along `axis`, or the pane
/// itself when there is one.
fn equal_split(axis: &str, first: usize, last: usize) -> String {
    if first == last {
        return first.to_string();
    }
    let entries: Vec<String> = equal(last - first + 1)
        .into_iter()
        .zip(first..=last)
        .map(|(weight, pane)| format!("{weight}:{pane}"))
        .collect();
    format!("{axis}[{}]", entries.join(" "))
}

/// The expected tiled tree for panes in rows of `rows[i]` panes.
fn tiled(rows: &[usize]) -> String {
    let mut first = 1;
    let mut rendered = Vec::new();
    for count in rows {
        rendered.push(equal_split("C", first, first + count - 1));
        first += count;
    }
    if rendered.len() == 1 {
        return rendered.remove(0);
    }
    let entries: Vec<String> = equal(rendered.len())
        .into_iter()
        .zip(rendered)
        .map(|(weight, row)| format!("{weight}:{row}"))
        .collect();
    format!("R[{}]", entries.join(" "))
}

/// The tiled table: panes per row for each pane count.
const TILED: [&[usize]; 10] = [
    &[1],
    &[2],
    &[2, 1],
    &[2, 2],
    &[3, 2],
    &[3, 3],
    &[3, 3, 1],
    &[3, 3, 2],
    &[3, 3, 3],
    &[4, 4, 2],
];

#[test]
fn built_in_layouts_give_the_planned_trees_for_one_to_ten_panes() {
    for n in 1..=10 {
        let main = |outer: &str, inner: &str| {
            if n == 1 {
                "1".to_owned()
            } else {
                format!("{outer}[600000:1 400000:{}]", equal_split(inner, 2, n))
            }
        };
        for (name, expected) in [
            ("columns", equal_split("C", 1, n)),
            ("rows", equal_split("R", 1, n)),
            ("main_left", main("C", "R")),
            ("main_top", main("R", "C")),
            ("tiled", tiled(TILED[n - 1])),
        ] {
            let arrangement = built_in(name).template.expand(&ids(n)).unwrap();
            assert_eq!(render(&arrangement), expected, "{name} with {n}");
        }
    }
    // The oracle's equal weights and main share, written out.
    assert_eq!(
        expand("columns(*)", 3).unwrap(),
        "C[333334:1 333333:2 333333:3]"
    );
    assert_eq!(
        expand("columns(pane 60%, rows(*))", 3).unwrap(),
        "C[600000:1 400000:R[500000:2 500000:3]]"
    );
    assert_eq!(
        expand("grid(*)", 7).unwrap(),
        "R[333334:C[333334:1 333333:2 333333:3] \
         333333:C[333334:4 333333:5 333333:6] 333333:7]"
    );
}

#[test]
fn built_in_layouts_past_the_child_limit_are_unavailable() {
    let at = |name: &str, n: usize| built_in(name).template.expand(&ids(n));
    for name in ["columns", "rows"] {
        assert_eq!(at(name, 256).unwrap().panes(), ids(256));
        assert_eq!(at(name, 257), Err(LayoutError::TooManyChildren));
    }
    for (name, outer, inner) in
        [("main_left", "C", "R"), ("main_top", "R", "C")]
    {
        let arrangement = at(name, 257).unwrap();
        assert_eq!(
            render(&arrangement),
            format!("{outer}[600000:1 400000:{}]", equal_split(inner, 2, 257))
        );
        assert_eq!(layout_of(&arrangement).pane_count(), 257);
        assert_eq!(at(name, 258), Err(LayoutError::TooManyChildren));
    }
    // 17 columns: fifteen full rows and a last row of the rest.
    for (n, last) in [(257, 2), (258, 3)] {
        let mut rows = vec![17; 15];
        rows.push(last);
        let arrangement = at("tiled", n).unwrap();
        assert_eq!(render(&arrangement), tiled(&rows));
        assert_eq!(layout_of(&arrangement).panes().len(), n);
    }
}

#[test]
fn built_in_layouts_have_their_names_titles_and_templates() {
    let table: Vec<(&str, &str, &str)> = BUILT_IN_LAYOUTS
        .iter()
        .map(|layout| {
            (
                layout.name.as_str(),
                layout.title.as_str(),
                layout.template.as_str(),
            )
        })
        .collect();
    assert_eq!(
        table,
        [
            ("columns", "Equal Columns", "columns(*)"),
            ("rows", "Equal Rows", "rows(*)"),
            ("main_left", "Main Left", "columns(pane 60%, rows(*))"),
            ("main_top", "Main Top", "rows(pane 60%, columns(*))"),
            ("tiled", "Tiled", "grid(*)"),
        ]
    );
    assert!(NamedLayout::built_in("dev").is_none());
    for name in ["a", "dev", "main_left", "x1_y2", &"a".repeat(32)] {
        assert!(NamedLayout::valid_name(name), "{name}");
    }
    for name in ["", "1a", "_a", "Dev", "a-b", "a b", "é", &"a".repeat(33)] {
        assert!(!NamedLayout::valid_name(name), "{name}");
    }
}

#[test]
fn templates_parse_every_grammar_form_and_skip_whitespace() {
    for (source, n, expected) in [
        ("columns(*)", 2, "C[500000:1 500000:2]"),
        ("rows(*)", 2, "R[500000:1 500000:2]"),
        (
            "grid(*)",
            4,
            "R[500000:C[500000:1 500000:2] 500000:C[500000:3 500000:4]]",
        ),
        ("columns(pane, pane)", 2, "C[500000:1 500000:2]"),
        ("rows(pane 30%, pane 70%)", 2, "R[300000:1 700000:2]"),
        (
            "columns(* 50%, rows(*) 50%)",
            3,
            "C[250000:1 250000:2 500000:3]",
        ),
        ("columns(pane, grid(*) 60%)", 2, "C[400000:1 600000:2]"),
        (
            " \tcolumns ( pane 50 % ,\n rows ( * ) ) \n",
            3,
            "C[500000:1 500000:R[500000:2 500000:3]]",
        ),
        // Each child's first unit comes before the shares, so 5% of the
        // remaining 999,998 units rounds up to 50,001.
        ("columns(pane 05%, *)", 2, "C[50001:1 949999:2]"),
    ] {
        let parsed = template(source);
        assert_eq!(parsed.as_str(), source);
        assert_eq!(parsed.to_string(), source);
        assert_eq!(
            render(&parsed.expand(&ids(n)).unwrap()),
            expected,
            "{source:?}"
        );
    }
}

#[test]
fn template_errors_name_the_rule_and_its_offset() {
    use TemplateErrorKind::*;
    for (source, offset, kind) in [
        ("", 0, ExpectedSplit),
        ("pane", 0, ExpectedSplit),
        ("*", 0, ExpectedSplit),
        ("  col umns(*)", 2, ExpectedSplit),
        ("columns(pa ne, *)", 8, ExpectedNode),
        ("columns(pane,)", 13, ExpectedNode),
        ("columns()", 8, ExpectedNode),
        ("columns *)", 8, ExpectedOpen),
        ("columns(pane 55% rows(*))", 17, ExpectedSeparator),
        ("columns(pane, *", 15, ExpectedSeparator),
        ("columns(pane 2 0%, *)", 15, ExpectedPercent),
        ("columns(pane 20, *)", 15, ExpectedPercent),
        ("columns(pane 0%, *)", 13, ShareOutOfRange),
        ("columns(pane 100%, *)", 13, ShareOutOfRange),
        (
            "columns(pane 99999999999999999999%, *)",
            13,
            ShareOutOfRange,
        ),
        ("columns(pane 60%, pane 40%, *)", 0, SharesTooLarge),
        (
            "rows(pane, columns(pane 50%, pane 50%, *))",
            11,
            SharesTooLarge,
        ),
        ("columns(pane 60%, pane 30%)", 0, SharesNotTotal),
        ("columns(* 50%)", 0, SharesNotTotal),
        ("columns(pane, grid(pane))", 19, ExpectedStar),
        ("grid(*, *)", 6, ExpectedClose),
        ("grid *", 5, ExpectedOpen),
        ("columns(pane)", 0, TooFewChildren),
        ("columns(pane, rows(pane))", 14, TooFewChildren),
        ("columns(*) x", 11, ExpectedEnd),
        ("columns(*) 50%", 11, ExpectedEnd),
        ("columns(*)é", 10, ExpectedEnd),
    ] {
        assert_eq!(
            LayoutTemplate::parse(source),
            Err(TemplateError { offset, kind }),
            "{source:?}"
        );
    }
    assert_eq!(
        LayoutTemplate::parse("columns(pane 55% rows(*))")
            .unwrap_err()
            .to_string(),
        "expected `,` or `)` at 17"
    );
}

#[test]
fn templates_are_limited_in_length_and_depth() {
    let padded = |len: usize| format!("columns(*{})", " ".repeat(len - 10));
    assert_eq!(padded(MAX_TEMPLATE_LEN).len(), MAX_TEMPLATE_LEN);
    assert!(LayoutTemplate::parse(&padded(MAX_TEMPLATE_LEN)).is_ok());
    assert_eq!(
        LayoutTemplate::parse(&padded(MAX_TEMPLATE_LEN + 1)),
        Err(TemplateError {
            offset: MAX_TEMPLATE_LEN,
            kind: TemplateErrorKind::TooLong,
        })
    );
    // A character that crosses the limit is reported where it starts, so
    // the offset can slice the text.
    let crossing =
        format!("{}é", &padded(MAX_TEMPLATE_LEN)[..MAX_TEMPLATE_LEN - 1]);
    let error = LayoutTemplate::parse(&crossing).unwrap_err();
    assert_eq!(error.offset, MAX_TEMPLATE_LEN - 1);
    assert!(crossing.is_char_boundary(error.offset));
    // `depth` alternating splits, each beside a pane, around a grid.
    let nested = |depth: usize| {
        let mut source = String::new();
        let mut starts = Vec::new();
        for level in 1..depth {
            starts.push(source.len());
            source += if level % 2 == 1 {
                "columns(pane, "
            } else {
                "rows(pane, "
            };
        }
        starts.push(source.len());
        source += "grid(*)";
        source += &")".repeat(depth - 1);
        (source, starts)
    };
    let (deepest, _) = nested(MAX_TEMPLATE_DEPTH);
    assert_eq!(
        template(&deepest).expand(&ids(20)).unwrap().panes(),
        ids(20)
    );
    let (too_deep, starts) = nested(MAX_TEMPLATE_DEPTH + 1);
    assert_eq!(
        LayoutTemplate::parse(&too_deep),
        Err(TemplateError {
            offset: starts[MAX_TEMPLATE_DEPTH],
            kind: TemplateErrorKind::TooDeep,
        })
    );
}

#[test]
fn stars_share_the_remaining_panes_and_empty_slots_drop_out() {
    let source = "columns(*, pane 20%, *)";
    for (n, expected) in [
        (1, "1"),
        // The second star is empty: 40% and 20% scale to two thirds and
        // one third.
        (2, "C[666666:1 333334:2]"),
        (3, "C[400000:1 200000:2 400000:3]"),
        (4, "C[200000:1 200000:2 200000:3 400000:4]"),
        (5, "C[200000:1 200000:2 200000:3 200000:4 200000:5]"),
        (
            6,
            "C[133334:1 133333:2 133333:3 200000:4 200000:5 200000:6]",
        ),
    ] {
        assert_eq!(expand(source, n).unwrap(), expected, "{n} panes");
    }
    let sides = "columns(rows(*) 25%, pane, rows(*) 25%)";
    for (n, expected) in [
        (1, "1"),
        (2, "C[333334:1 666666:2]"),
        (
            6,
            "C[250000:R[333334:1 333333:2 333333:3] 500000:4 \
             250000:R[500000:5 500000:6]]",
        ),
    ] {
        assert_eq!(expand(sides, n).unwrap(), expected, "sides, {n} panes");
    }
    // A star counts as one child however many panes it takes, so the
    // pane keeps about half.
    for (n, expected) in [
        (2, "C[500000:1 500000:2]"),
        (3, "C[500000:1 250000:2 250000:3]"),
        (4, "C[499999:1 166667:2 166667:3 166667:4]"),
    ] {
        assert_eq!(expand("columns(pane, *)", n).unwrap(), expected);
    }
}

#[test]
fn templates_without_a_star_hold_at_most_their_pane_slots() {
    let source = "rows(columns(pane, pane) 70%, pane)";
    assert_eq!(expand(source, 1).unwrap(), "1");
    // The empty slot drops out and the lone row stands for the split.
    assert_eq!(expand(source, 2).unwrap(), "C[500000:1 500000:2]");
    assert_eq!(
        expand(source, 3).unwrap(),
        "R[700000:C[500000:1 500000:2] 300000:3]"
    );
    assert_eq!(expand(source, 4), Err(LayoutError::TooManyPanes));
    assert_eq!(
        expand("columns(pane, pane)", 3),
        Err(LayoutError::TooManyPanes)
    );
    assert_eq!(
        template("columns(*)").expand(&[]),
        Err(LayoutError::MissingPane)
    );
}

#[test]
fn user_layouts_from_the_plan_expand_as_written() {
    let dev = "columns(pane 55%, rows(pane 70%, columns(*)))";
    assert_eq!(expand(dev, 1).unwrap(), "1");
    assert_eq!(expand(dev, 2).unwrap(), "C[550000:1 450000:2]");
    assert_eq!(
        expand(dev, 4).unwrap(),
        "C[550000:1 450000:R[700000:2 300000:C[500000:3 500000:4]]]"
    );
    let pair = "rows(columns(pane, pane) 70%, columns(*))";
    assert_eq!(expand(pair, 2).unwrap(), "C[500000:1 500000:2]");
    assert_eq!(
        expand(pair, 5).unwrap(),
        "R[700000:C[500000:1 500000:2] \
         300000:C[333334:3 333333:4 333333:5]]"
    );
}

#[test]
fn nested_splits_collapse_and_flatten_with_their_proportions() {
    // The nested columns flatten: 60% divides into 25% and 75% of it.
    assert_eq!(
        expand("columns(pane 40%, columns(pane 25%, pane))", 3).unwrap(),
        "C[400000:1 150000:2 450000:3]"
    );
    // The rows split's star takes no pane, so its columns child stands
    // for it and flattens.
    assert_eq!(
        expand("columns(*, rows(columns(pane, pane), *))", 3).unwrap(),
        "C[500000:1 250000:2 250000:3]"
    );
    // A grid nested across the axis stays nested; one on the same axis
    // flattens its rows into the parent.
    assert_eq!(
        expand("columns(pane, grid(*))", 4).unwrap(),
        "C[500000:1 500000:R[500000:C[500000:2 500000:3] 500000:4]]"
    );
    assert_eq!(
        expand("rows(pane 60%, grid(*))", 5).unwrap(),
        "R[599999:1 200001:C[500000:2 500000:3] \
         200000:C[500000:4 500000:5]]"
    );
    // A flatten past the child limit is skipped: the nested split stays,
    // which the layout accepts.
    let arrangement = template("columns(pane, columns(*))")
        .expand(&ids(257))
        .unwrap();
    let Arrangement::Split { children, .. } = &arrangement else {
        panic!("expected a split");
    };
    assert_eq!(children.len(), 2);
    assert_eq!(children[0], (500_000, Arrangement::Pane(ids(1)[0])));
    let Arrangement::Split {
        axis,
        children: inner,
    } = &children[1].1
    else {
        panic!("expected a nested split");
    };
    assert_eq!(*axis, SplitAxis::Columns);
    assert_eq!(
        inner.iter().map(|(weight, _)| *weight).collect::<Vec<_>>(),
        equal(256)
    );
    assert_eq!(layout_of(&arrangement).panes().len(), 257);
    // More than the limit in one split is unavailable.
    assert_eq!(
        expand("columns(pane, columns(*))", 258),
        Err(LayoutError::TooManyChildren)
    );
    assert_eq!(
        expand("columns(pane, *)", 257),
        Err(LayoutError::TooManyChildren)
    );
}

#[test]
fn weights_give_every_child_a_unit_and_apportion_the_rest() {
    // Fractions from 0.00000001 to 0.99, every weight positive and the
    // sum exact.
    assert_eq!(
        expand(
            "columns(columns(columns(columns(pane 1%, pane) 1%, pane) 1%, \
             pane) 1%, pane)",
            5
        )
        .unwrap(),
        "C[1:1 2:2 100:3 9901:4 989996:5]"
    );
    assert_eq!(apportion(&[1.0; 3]), [333_334, 333_333, 333_333]);
    assert_eq!(apportion(&[0.6, 0.4]), [600_000, 400_000]);
    for count in 1..=MAX_SPLIT_CHILDREN {
        assert_eq!(apportion(&vec![1.0; count]), equal(count), "{count}");
    }
}

/// Every template the tests use, built-in or not.
fn test_templates() -> Vec<LayoutTemplate> {
    BUILT_IN_LAYOUTS
        .iter()
        .map(|layout| layout.template.clone())
        .chain(
            [
                "columns(*, pane 20%, *)",
                "columns(rows(*) 25%, pane, rows(*) 25%)",
                "columns(pane 55%, rows(pane 70%, columns(*)))",
                "rows(columns(pane, pane) 70%, columns(*))",
                "columns(pane, *)",
                "columns(pane, pane)",
                "rows(columns(pane, pane) 70%, pane)",
                "columns(pane 40%, columns(pane 25%, pane))",
                "columns(*, rows(columns(pane, pane), *))",
                "columns(pane, grid(*))",
                "rows(pane 60%, grid(*))",
                "rows(grid(*), pane, grid(*) 20%)",
            ]
            .map(template),
        )
        .collect()
}

#[test]
fn every_template_gives_a_valid_layout_in_traversal_order_or_is_unavailable() {
    for template in test_templates() {
        let (slots, stars) = template.root.slots();
        for n in 1..=12 {
            let result = template.expand(&ids(n));
            if stars == 0 && n > slots {
                assert_eq!(
                    result,
                    Err(LayoutError::TooManyPanes),
                    "{template} with {n}"
                );
                continue;
            }
            let arrangement = result.unwrap();
            let base = layout_of(&template_free(n));
            let mut next = 0;
            let arranged = base
                .arranged(&arrangement, || {
                    next += 1;
                    SplitId::in_runtime(RUNTIME, next)
                })
                .unwrap_or_else(|error| panic!("{template} with {n}: {error}"));
            assert_eq!(
                arranged.panes().iter().map(|p| p.id).collect::<Vec<_>>(),
                ids(n),
                "{template} with {n}"
            );
        }
    }
}

/// Equal columns of `n` panes, written without a template.
fn template_free(n: usize) -> Arrangement {
    if n == 1 {
        return Arrangement::Pane(ids(1)[0]);
    }
    Arrangement::Split {
        axis: SplitAxis::Columns,
        children: equal(n)
            .into_iter()
            .zip(ids(n))
            .map(|(weight, pane)| (weight, Arrangement::Pane(pane)))
            .collect(),
    }
}

#[test]
fn a_nested_uneven_input_gives_the_same_trees_as_a_flat_one() {
    let pane = |n: usize| Arrangement::Pane(ids(4)[n - 1]);
    // [(1 over (2 | 3)) | 4] with uneven weights.
    let nested = layout_of(&Arrangement::Split {
        axis: SplitAxis::Columns,
        children: vec![
            (
                700_000,
                Arrangement::Split {
                    axis: SplitAxis::Rows,
                    children: vec![
                        (100_000, pane(1)),
                        (
                            900_000,
                            Arrangement::Split {
                                axis: SplitAxis::Columns,
                                children: vec![
                                    (300_000, pane(2)),
                                    (700_000, pane(3)),
                                ],
                            },
                        ),
                    ],
                },
            ),
            (300_000, pane(4)),
        ],
    });
    let flat = layout_of(&template_free(4));
    for template in test_templates() {
        let arrange = |layout: &PaneLayout| {
            let panes: Vec<PaneId> =
                layout.panes().iter().map(|pane| pane.id).collect();
            template.expand(&panes).map(|arrangement| {
                let mut next = 0;
                layout
                    .arranged(&arrangement, || {
                        next += 1;
                        SplitId::in_runtime(RUNTIME, next)
                    })
                    .unwrap()
                    .arrangement()
            })
        };
        assert_eq!(arrange(&nested), arrange(&flat), "{template}");
    }
}

/// The default cycle: the built-ins in table order.
fn default_cycle() -> Vec<NamedLayout> {
    BUILT_IN_LAYOUTS.to_vec()
}

fn named(name: &str, source: &str) -> NamedLayout {
    NamedLayout {
        name: name.to_owned(),
        title: name.to_owned(),
        template: template(source),
    }
}

/// The layout `name` gives `n` panes.
fn in_layout(cycle: &[NamedLayout], name: &str, n: usize) -> PaneLayout {
    let layout = cycle.iter().find(|layout| layout.name == name).unwrap();
    layout_of(&layout.template.expand(&ids(n)).unwrap())
}

#[test]
fn matching_finds_the_nearest_layout_of_the_same_shape() {
    let cycle = default_cycle();
    let resized = |name: &str, n: usize, weights: [u32; 2]| {
        let Arrangement::Split { axis, mut children } =
            in_layout(&cycle, name, n).arrangement()
        else {
            panic!("expected a split");
        };
        children[0].0 = weights[0];
        children[1].0 = weights[1];
        layout_of(&Arrangement::Split { axis, children })
    };
    for (index, layout) in cycle.iter().enumerate() {
        let exact = in_layout(&cycle, &layout.name, 3);
        assert_eq!(
            exact.matching_layout(&cycle),
            Some(index),
            "{}",
            layout.name
        );
    }
    // A main left resized from 60/40 still matches main left.
    assert_eq!(
        resized("main_left", 3, [650_000, 350_000]).matching_layout(&cycle),
        Some(2)
    );
    // Two panes: main left and columns share a shape, and tiled equals
    // columns.
    for (weights, expected) in [
        ([600_000, 400_000], 2),
        ([650_000, 350_000], 2),
        ([550_000, 450_000], 0),
        ([500_000, 500_000], 0),
    ] {
        assert_eq!(
            resized("columns", 2, weights).matching_layout(&cycle),
            Some(expected),
            "{weights:?}"
        );
    }
    // [(1 over 2) | 3] has no built-in's shape.
    let unmatched = layout_of(
        &template("columns(rows(pane, pane), pane)")
            .expand(&ids(3))
            .unwrap(),
    );
    assert_eq!(unmatched.matching_layout(&cycle), None);
    // At 257 panes, columns and rows are unavailable and skipped.
    assert_eq!(
        in_layout(&cycle, "main_left", 257).matching_layout(&cycle),
        Some(2)
    );
    assert_eq!(
        in_layout(&cycle, "tiled", 257).matching_layout(&cycle),
        Some(4)
    );
    assert_eq!(unmatched.matching_layout(&[]), None);
    // Two layouts that differ only in a nested split's weights: the
    // distance counts every split, not only the root.
    let nested = [
        named("low", "columns(pane 50%, rows(pane 30%, pane))"),
        named("high", "columns(pane 50%, rows(pane 70%, pane))"),
    ];
    for (index, layout) in nested.iter().enumerate() {
        let exact = in_layout(&nested, &layout.name, 3);
        assert_eq!(exact.matching_layout(&nested), Some(index));
    }
}

/// A `fit` for a view large enough for every layout.
#[expect(
    clippy::unnecessary_wraps,
    reason = "matches the fit callback, whose `None` means a layout is short"
)]
fn identity(_: &NamedLayout, arrangement: Arrangement) -> Option<Arrangement> {
    Some(arrangement)
}

/// The cycle indices `steps` steps visit from `layout`, applying each.
fn walk(
    cycle: &[NamedLayout],
    mut layout: PaneLayout,
    forward: bool,
    steps: usize,
) -> Vec<usize> {
    let mut visited = Vec::new();
    for _ in 0..steps {
        let Some((index, arrangement)) =
            layout.step_layout(cycle, forward, identity)
        else {
            break;
        };
        assert_ne!(arrangement, layout.arrangement(), "a step changed nothing");
        visited.push(index);
        layout = layout_of(&arrangement);
    }
    visited
}

#[test]
fn stepping_the_default_cycle_visits_every_available_layout() {
    let cycle = default_cycle();
    let unmatched = |n: usize| {
        layout_of(
            &template("columns(rows(pane, pane), *)")
                .expand(&ids(n))
                .unwrap(),
        )
    };
    for n in 3..=6 {
        for (start, name) in cycle.iter().enumerate() {
            let layout = in_layout(&cycle, &name.name, n);
            let forward: Vec<usize> =
                (1..=5).map(|step| (start + step) % 5).collect();
            let back: Vec<usize> =
                (1..=5).map(|step| (start + 5 * 2 - step) % 5).collect();
            assert_eq!(walk(&cycle, layout.clone(), true, 5), forward);
            assert_eq!(walk(&cycle, layout, false, 5), back);
        }
        assert_eq!(walk(&cycle, unmatched(n), true, 6), [0, 1, 2, 3, 4, 0]);
        assert_eq!(walk(&cycle, unmatched(n), false, 6), [4, 3, 2, 1, 0, 4]);
    }
    // Two panes: tiled equals columns, so a cycle has four layouts, and a
    // step back from columns skips tiled.
    for (name, forward, back) in [
        ("columns", [1, 2, 3, 4, 1], [3, 2, 1, 0, 3]),
        ("rows", [2, 3, 4, 1, 2], [0, 3, 2, 1, 0]),
        ("main_left", [3, 4, 1, 2, 3], [1, 0, 3, 2, 1]),
        ("main_top", [4, 1, 2, 3, 4], [2, 1, 0, 3, 2]),
        ("tiled", [1, 2, 3, 4, 1], [3, 2, 1, 0, 3]),
    ] {
        let layout = in_layout(&cycle, name, 2);
        assert_eq!(walk(&cycle, layout.clone(), true, 5), forward, "{name}");
        assert_eq!(walk(&cycle, layout, false, 5), back, "{name}");
    }
    let distinct = |n: usize, steps: usize| {
        let mut layout = in_layout(&cycle, "columns", n);
        let mut seen = vec![layout.arrangement()];
        for _ in 0..steps {
            let (_, next) = layout.step_layout(&cycle, true, identity).unwrap();
            if !seen.contains(&next) {
                seen.push(next.clone());
            }
            layout = layout_of(&next);
        }
        seen.len()
    };
    assert_eq!(distinct(2, 8), 4);
    assert_eq!(distinct(4, 10), 5);
    // 257 panes: columns and rows are unavailable.
    for (name, forward, back) in [
        ("main_left", [3, 4, 2, 3], [4, 3, 2, 4]),
        ("main_top", [4, 2, 3, 4], [2, 4, 3, 2]),
        ("tiled", [2, 3, 4, 2], [3, 2, 4, 3]),
    ] {
        let layout = in_layout(&cycle, name, 257);
        assert_eq!(walk(&cycle, layout.clone(), true, 4), forward, "{name}");
        assert_eq!(walk(&cycle, layout, false, 4), back, "{name}");
    }
    let unmatched_257 = layout_of(
        &template("rows(columns(*), columns(*))")
            .expand(&ids(257))
            .unwrap(),
    );
    assert_eq!(unmatched_257.matching_layout(&cycle), None);
    assert_eq!(walk(&cycle, unmatched_257.clone(), true, 4), [2, 3, 4, 2]);
    assert_eq!(walk(&cycle, unmatched_257, false, 4), [4, 3, 2, 4]);
    // 258 panes: only tiled is available.
    let unmatched = layout_of(
        &template("rows(columns(*), columns(*))")
            .expand(&ids(258))
            .unwrap(),
    );
    for forward in [true, false] {
        assert_eq!(walk(&cycle, unmatched.clone(), forward, 3), [4]);
    }
}

#[test]
fn stepping_skips_layouts_that_do_not_fit_or_change_nothing() {
    let cycle = default_cycle();
    let columns = in_layout(&cycle, "columns", 3);
    let current = columns.arrangement();
    let mut tried = Vec::new();
    let step = columns.step_layout(&cycle, true, |layout, ideal| {
        tried.push(layout.name.clone());
        match layout.name.as_str() {
            // Too small for this view.
            "rows" => None,
            // Fitted to exactly the current layout.
            "main_left" => Some(current.clone()),
            _ => Some(ideal),
        }
    });
    assert_eq!(step.map(|(index, _)| index), Some(3));
    assert_eq!(tried, ["rows", "main_left", "main_top"]);
    // With every candidate skipped, there is no step, and each was tried
    // once, the matched layout last.
    let mut tried = Vec::new();
    let step = columns.step_layout(&cycle, false, |layout, _| {
        tried.push(layout.name.clone());
        None
    });
    assert_eq!(step, None);
    assert_eq!(tried, ["tiled", "main_top", "main_left", "rows", "columns"]);
}

#[test]
fn stepping_follows_a_custom_cycle() {
    let cycle = vec![
        named("dev", "columns(pane 55%, rows(pane 70%, columns(*)))"),
        built_in("rows").clone(),
        built_in("tiled").clone(),
    ];
    let dev = in_layout(&cycle, "dev", 4);
    assert_eq!(walk(&cycle, dev.clone(), true, 4), [1, 2, 0, 1]);
    assert_eq!(walk(&cycle, dev, false, 4), [2, 1, 0, 2]);
    // With only columns and rows, main left on two panes is a columns tree
    // at 60/40: it matches columns and steps to rows. On three panes it
    // matches nothing, and the step starts the cycle.
    let cycle = vec![built_in("columns").clone(), built_in("rows").clone()];
    let defaults = default_cycle();
    let two = in_layout(&defaults, "main_left", 2);
    assert_eq!(two.matching_layout(&cycle), Some(0));
    assert_eq!(walk(&cycle, two, true, 1), [1]);
    let three = in_layout(&defaults, "main_left", 3);
    assert_eq!(three.matching_layout(&cycle), None);
    assert_eq!(walk(&cycle, three, true, 1), [0]);
}

#[test]
fn a_user_layout_equal_to_a_built_in_matches_first_in_cycle_order() {
    let cycle = vec![
        built_in("columns").clone(),
        built_in("rows").clone(),
        named("mine", "columns(*)"),
    ];
    let columns = in_layout(&cycle, "columns", 3);
    assert_eq!(columns.matching_layout(&cycle), Some(0));
    let mine = in_layout(&cycle, "mine", 3);
    assert_eq!(mine.matching_layout(&cycle), Some(0));
    // A step back skips mine, whose result equals the current layout.
    assert_eq!(walk(&cycle, columns, false, 1), [1]);
}

#[test]
fn a_cycle_of_the_current_layout_alone_has_no_step() {
    let cycle = vec![named(
        "dev",
        "columns(pane 55%, rows(pane 70%, columns(*)))",
    )];
    let dev = in_layout(&cycle, "dev", 4);
    assert_eq!(dev.matching_layout(&cycle), Some(0));
    for forward in [true, false] {
        assert_eq!(dev.step_layout(&cycle, forward, identity), None);
    }
    // One pane is every layout of one pane.
    let one = in_layout(&default_cycle(), "tiled", 1);
    assert_eq!(one.step_layout(&default_cycle(), true, identity), None);
}
