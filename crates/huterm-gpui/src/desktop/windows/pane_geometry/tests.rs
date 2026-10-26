use super::*;
use huterm_protocol::{PaneInfo, RuntimeId, SplitChild, SplitNode, TerminalId};

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
    let panes = layout_panes(&PaneLayout::single(info(1)), content, 2.0);
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
    let panes = layout_panes(&layout, content, 1.0);
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
    let panes = layout_panes(&layout, rect(0.0, 0.0, 100.0, 50.0), 2.0);
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
    let tiny = layout_panes(&layout, rect(0.0, 0.0, 1.0, 50.0), 1.0);
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
