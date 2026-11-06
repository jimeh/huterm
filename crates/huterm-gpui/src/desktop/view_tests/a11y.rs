//! The terminal's accessibility node over a real terminal: the tree GPUI
//! reports, the publication throttle, and agreement with Copy.

use huterm_protocol::BufferPoint;

use super::super::terminal_a11y::tests::{selected, text};
use super::super::terminal_a11y::{Model, PUBLISH_INTERVAL};
use super::*;

/// The window's last accessibility tree dump.
fn a11y_tree(harness: &mut ViewHarness) -> serde_json::Value {
    harness.cx.update(|window, _| {
        let json = window.debug_a11y_tree_json().expect(
            "test windows report an accessibility tree for every frame",
        );
        serde_json::from_str(&json).unwrap()
    })
}

/// The terminal element's own node and its children's nodes.
fn terminal_node(
    tree: &serde_json::Value,
) -> (serde_json::Value, Vec<serde_json::Value>) {
    let nodes = tree["nodes"].as_object().unwrap();
    let terminals: Vec<_> = nodes
        .values()
        .filter(|node| {
            node["aria"]["role"] == "Terminal" && node["synthetic"] != true
        })
        .collect();
    let [terminal] = terminals.as_slice() else {
        panic!("one terminal node: {terminals:#?}");
    };
    let children = terminal["children"]
        .as_array()
        .map(|children| {
            children
                .iter()
                .map(|key| nodes[key.as_str().unwrap()].clone())
                .collect()
        })
        .unwrap_or_default();
    ((*terminal).clone(), children)
}

fn model(harness: &mut ViewHarness) -> std::rc::Rc<Model> {
    harness.read(|view, _| view.a11y.model())
}

fn publications(harness: &mut ViewHarness) -> u64 {
    harness.read(|view, _| view.a11y.publications())
}

#[gpui::test]
fn the_terminal_node_carries_its_states_text_and_caret(
    cx: &mut TestAppContext,
) {
    let mut harness =
        ViewHarness::new(cx, &text_fixture("AppModelExample\\n", ""));
    harness.wait_ready(MouseTracking::Disabled);
    harness.wait_for("the model to show the sentinel", |view, _| {
        !view.a11y.model().runs.is_empty()
            && text(&view.a11y.model()).contains("READY")
    });
    let tree = a11y_tree(&mut harness);
    let (terminal, children) = terminal_node(&tree);
    // The one closure GPUI keeps per element writes the read-only state
    // beside the text runs.
    assert_eq!(terminal["aria"]["read_only"], true, "{terminal}");
    assert_eq!(terminal["aria"]["label"], "Terminal", "{terminal}");
    let runs: String = children
        .iter()
        .filter(|child| child["aria"]["role"] == "TextRun")
        .map(|child| child["aria"]["value"].as_str().unwrap())
        .collect();
    let model = model(&mut harness);
    assert_eq!(runs, text(&model), "the tree carries the published runs");
    assert!(runs.starts_with("AppModelExample\nREADY"), "{runs:?}");
    let carets = children
        .iter()
        .filter(|child| child["aria"]["role"] == "Caret")
        .count();
    assert_eq!(carets, 1, "{children:#?}");
    // The cursor follows READY on the second row: a collapsed selection.
    assert_eq!(selected(&model), Some((21, 21, String::new())));
    // The focused view's node is the focused node.
    let focus = tree["gpui_focus"].as_str().unwrap();
    assert_eq!(tree["nodes"][focus]["aria"]["role"], "Terminal");
}

/// `snapshot` with its first row reading `line`.
fn with_first_row(
    snapshot: &TerminalSnapshot,
    line: &str,
) -> Arc<TerminalSnapshot> {
    let mut snapshot = snapshot.clone();
    let mut row = (*snapshot.rows[0]).clone();
    for (index, cell) in row.cells.iter_mut().enumerate() {
        cell.text = line
            .get(index..=index)
            .map_or(huterm_protocol::CellText::BLANK, Into::into);
    }
    snapshot.rows[0] = Arc::new(row);
    snapshot.revision += 1;
    Arc::new(snapshot)
}

#[gpui::test]
fn output_republishes_at_most_once_per_interval_and_selection_at_once(
    cx: &mut TestAppContext,
) {
    let mut harness = ViewHarness::new(cx, &text_fixture("burst\\n", ""));
    harness.wait_ready(MouseTracking::Disabled);
    harness.wait_for("the model to show the fixture", |view, _| {
        !view.a11y.model().runs.is_empty()
            && text(&view.a11y.model()).contains("READY")
    });
    // The child is idle from here, so only the frames below change the
    // view's snapshot, and only the executor's clock moves.
    let shown = harness.read(|view, _| view.snapshot.clone().unwrap());
    let show = |harness: &mut ViewHarness, line: &str| {
        let next = with_first_row(&shown, line);
        harness.update(|view, _, cx| {
            view.snapshot = Some(next);
            cx.notify();
        });
        harness.cx.run_until_parked();
    };
    let first_line = |harness: &mut ViewHarness| {
        text(&model(harness)).lines().next().unwrap().to_owned()
    };
    harness.cx.executor().advance_clock(PUBLISH_INTERVAL * 2);
    let base = publications(&mut harness);
    show(&mut harness, "one");
    assert_eq!(publications(&mut harness), base + 1, "the first change");
    assert_eq!(first_line(&mut harness), "one");
    show(&mut harness, "two");
    show(&mut harness, "three");
    assert_eq!(publications(&mut harness), base + 1, "inside the interval");
    assert_eq!(first_line(&mut harness), "one");
    assert!(harness.read(|view, _| view.a11y_deferred.is_some()));
    // The deferred wake publishes the latest content on its own.
    harness.cx.executor().advance_clock(PUBLISH_INTERVAL);
    harness.cx.run_until_parked();
    assert_eq!(publications(&mut harness), base + 2, "the trailing update");
    assert_eq!(first_line(&mut harness), "three");
    assert!(
        harness.read(|view, _| view.a11y_deferred.is_none()),
        "nothing is left to wake for"
    );
    // A new snapshot with the same content publishes nothing.
    harness.cx.executor().advance_clock(PUBLISH_INTERVAL * 2);
    show(&mut harness, "three");
    assert_eq!(
        publications(&mut harness),
        base + 2,
        "an equivalent snapshot"
    );
    // A selection publishes at once, even right after a publication.
    show(&mut harness, "four");
    assert_eq!(publications(&mut harness), base + 3);
    let rows = harness
        .read(|view, _| usize::from(view.snapshot.as_ref().unwrap().size.rows));
    harness.update(|view, _, cx| {
        let generation = view.snapshot.as_ref().unwrap().generation;
        view.selection = Some(Selection::cells(
            generation,
            BufferPoint {
                rows_from_live_bottom: rows - 1,
                column: 0,
            },
            BufferPoint {
                rows_from_live_bottom: rows - 1,
                column: 3,
            },
        ));
        cx.notify();
    });
    harness.cx.run_until_parked();
    assert_eq!(publications(&mut harness), base + 4, "the selection");
    assert_eq!(selected(&model(&mut harness)).unwrap().2, "four");
    // A word selection has no range until the runtime expands it, so the
    // caret stands in for it, and the range publishes at once on arrival.
    let first = BufferPoint {
        rows_from_live_bottom: rows - 1,
        column: 1,
    };
    harness.update(|view, _, cx| {
        let generation = view.snapshot.as_ref().unwrap().generation;
        view.selection = Some(Selection::new(
            generation,
            first,
            Some(huterm_protocol::SelectionUnit::Word),
        ));
        cx.notify();
    });
    harness.cx.run_until_parked();
    let caret = selected(&model(&mut harness)).unwrap();
    assert_eq!(caret.0, caret.1, "a pending word selection: {caret:?}");
    harness.update(|view, _, cx| {
        let selection = view.selection.as_mut().unwrap();
        selection.expand(Some(huterm_protocol::BufferRange::ordered(
            BufferPoint { column: 0, ..first },
            BufferPoint { column: 3, ..first },
        )));
        cx.notify();
    });
    harness.cx.run_until_parked();
    assert_eq!(publications(&mut harness), base + 6, "the expanded word");
    assert_eq!(selected(&model(&mut harness)).unwrap().2, "four");
}

/// Lines whose Copy text tests the model: a multiline selection, trailing
/// blanks, concealed text, and lines that soft-wrap at 49 columns, after a
/// run of `x`, at a space, and inside blanks before more text; then a
/// trailing no-break space. `READY` follows, and the last rows stay empty.
const COPY_FIXTURE: &str = "alpha beta\\n\
    gamma delta   \\n\
    \\033[8mhidden\\033[0m shown\\n\
    xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\\n\
    yyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyyy world\\n\
    c                                                  d\\n\
    nbsp\\302\\240\\n";

#[gpui::test]
fn the_accessible_selection_reads_what_copy_copies(cx: &mut TestAppContext) {
    let mut harness = ViewHarness::new(cx, &text_fixture(COPY_FIXTURE, ""));
    harness.wait_ready(MouseTracking::Disabled);
    let rows = harness.read(|view, _| usize::from(view.requested_grid.rows));
    let at = |column: u16, row: usize| BufferPoint {
        rows_from_live_bottom: rows - 1 - row,
        column,
    };
    let wrapped = "x".repeat(60);
    let at_space = format!("{} world", "y".repeat(48));
    let blanks = format!("c{}d", " ".repeat(50));
    // Copy keeps concealed (SGR 8) text, so the accessible text does too.
    for (what, anchor, head, expected) in [
        ("a multiline selection", at(6, 0), at(4, 1), "beta\ngamma"),
        (
            "a row's trailing blanks",
            at(0, 1),
            at(30, 1),
            "gamma delta",
        ),
        (
            "trailing blanks into the next row",
            at(6, 1),
            at(2, 2),
            "delta\nhid",
        ),
        ("concealed text", at(0, 2), at(11, 2), "hidden shown"),
        ("a soft-wrapped line", at(0, 3), at(10, 4), wrapped.as_str()),
        ("a wrap at a space", at(0, 5), at(4, 6), at_space.as_str()),
        ("a wrap inside blanks", at(0, 7), at(2, 8), blanks.as_str()),
        (
            "a trailing no-break space",
            at(0, 9),
            at(48, 9),
            "nbsp\u{a0}",
        ),
        ("an end on an inner blank", at(0, 0), at(5, 0), "alpha"),
        // Copy copies nothing, so the caret stands in for the selection.
        ("the empty last row", at(0, rows - 1), at(10, rows - 1), ""),
    ] {
        harness.update(|view, _, cx| {
            let generation = view.snapshot.as_ref().unwrap().generation;
            let selection = Selection::cells(generation, anchor, head);
            view.selection = Some(selection);
            view.selected_text = None;
            view.update_renderer_selection();
            view.extract_selection(
                selection,
                selection.range().unwrap(),
                false,
                cx,
            );
            cx.notify();
        });
        harness.wait_for(&format!("Copy's text for {what}"), |view, _| {
            view.selected_text.is_some()
                && selected(&view.a11y.model()).is_some_and(
                    |(start, end, _)| expected.is_empty() || start != end,
                )
        });
        let (copied, accessible) = harness.read(|view, _| {
            (
                view.selected_text.clone().unwrap(),
                selected(&view.a11y.model()).unwrap().2,
            )
        });
        assert_eq!(copied, expected, "Copy's text for {what}");
        assert_eq!(accessible, copied, "{what}");
    }
}
