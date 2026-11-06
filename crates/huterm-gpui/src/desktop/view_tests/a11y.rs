//! The terminal's accessibility node over a real terminal: the tree GPUI
//! reports, the publication throttle, and agreement with Copy.

use huterm_protocol::{BufferPoint, BufferRange};

use super::super::terminal_a11y::tests::{selected, text};
use super::super::terminal_a11y::{
    Inputs, Model, ModelBuilder, PUBLISH_INTERVAL,
};
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
            .map_or(huterm_protocol::CellText::EMPTY, Into::into);
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
                    |(start, end, _)| (start == end) == expected.is_empty(),
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

/// A deterministic xorshift generator, so a failing selection reproduces.
struct Xorshift(u64);

impl Xorshift {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % u64::try_from(bound).unwrap()).unwrap()
    }
}

/// Screens for the 49 by 14 grid the harness requests whose Copy text
/// exercises Ghostty's whitespace rules: written spaces and empty cells
/// (left by CUF, EL, ECH, and a background erase), soft wraps at a space,
/// at spaces, and at empty cells, blank rows inside and at the end, wide
/// characters wrapping at the last column, combining marks, NBSP, U+3000,
/// and rows that line deletion and insertion leave after a soft-wrapped
/// one. Each puts `READY` on row 12, so rows after it stay empty.
fn parity_fixtures() -> [(&'static str, String); 5] {
    let run = |character: char, count: usize| {
        std::iter::repeat_n(character, count).collect::<String>()
    };
    let ready = "\\033[13;1H";
    [
        (
            "blanks",
            format!(
                "a\\n    \\nb\\033[3Cc   \\n\\n\\n  d  e\\n\\033[5Cf\\n\
                 g\\302\\240\\nh\\343\\200\\200\\ne\\314\\201 \\314\\201x  \\n\
                 {ready}"
            ),
        ),
        (
            "wraps",
            format!(
                "{} yy\\n{}    z\\n{}\\033[8Cvu\\n{}\\n{}{}q\\n{}\\033[1Cp{ready}",
                run('x', 48),
                run('x', 45),
                run('w', 40),
                run('t', 49),
                run('s', 49),
                run('r', 49),
                run('o', 47),
            ),
        ),
        (
            "wide",
            format!(
                "a\\033[47C漢\\n\\033[48C漢\\nb\\033[46C漢\\n字字 字\\n🙂c\\n\
                 c{}\\033[1C漢\\n\\033[47C字字{ready}",
                run(' ', 46),
            ),
        ),
        (
            "erase",
            format!(
                "abcdefgh\\033[3D\\033[K\\nijklmnop\\033[4D\\033[2X\\n\
                 \\033[41mqr\\033[3X\\033[0m\\033[3Cst\\n\
                 \\033[42m\\033[K\\033[0muv\\n{h} world\\n\
                 \\033[6;1H\\033[Mnext\\n{g} tail\\n\\033[8;1H\\033[L\
                 \\033[10;1H\\033[1;31m\\033[7mz\\033[0m  \\033[1C{ready}",
                h = run('h', 48),
                g = run('g', 48),
            ),
        ),
        ("rows", format!("$ ls\\n\\nfile one  \\n\\n\\n$ \\n{ready}")),
    ]
}

/// Whether Copy's text of the selection from cell `start` through cell
/// `end`, both (`column`, `row`), passes a row part with no text on a
/// soft-wrapped row before its last row, after Ghostty moves a start on a
/// spacer to its wide character, skips the row of a start on a spacer head,
/// and moves an end on a spacer head to the next row. Ghostty writes such a
/// part as a line break of its own, and blanks before it after that break,
/// which no range of a document holding the grid's Copy text reads; see the
/// terminal accessibility section of `docs/agents/desktop-client.md`.
fn passes_a_wrapped_part_without_text(
    snapshot: &TerminalSnapshot,
    start: (usize, usize),
    end: (usize, usize),
) -> bool {
    let cells = |row: usize| &snapshot.rows[row].cells;
    let head = |(column, row): (usize, usize)| {
        let cells = cells(row);
        column > 0
            && cells[column].style.wide_spacer
            && !cells[column - 1].style.wide
    };
    let last = if head(end) && end.1 + 1 < snapshot.rows.len() {
        end.1 + 1
    } else {
        end.1
    };
    (start.1..last).any(|row| {
        let from = if row == start.1 {
            if head(start) {
                return false;
            }
            let (column, cells) = (start.0, cells(row));
            if cells[column].style.wide_spacer {
                column - 1
            } else {
                column
            }
        } else {
            0
        };
        snapshot.rows[row].wrapped
            && cells(row)[from..].iter().all(|cell| cell.text.is_empty())
    })
}

/// What the parity check found.
#[derive(Default)]
struct Parity {
    compared: usize,
    /// Selections that pass a soft-wrapped row part with no text.
    exempt: usize,
    exempt_differing: usize,
    mismatches: Vec<String>,
}

/// Selections of the screen `harness` shows, starting with the whole grid:
/// `count` more with random ends.
fn parity_ranges(
    snapshot: &TerminalSnapshot,
    random: &mut Xorshift,
    count: usize,
) -> Vec<BufferRange> {
    let rows = usize::from(snapshot.size.rows);
    let columns = usize::from(snapshot.size.columns);
    let at = |column: usize, row: usize| BufferPoint {
        rows_from_live_bottom: rows - 1 - row,
        column: u16::try_from(column).unwrap(),
    };
    // Columns near both edges are where the fixtures put their blanks,
    // wraps, and spacer heads.
    let column = |random: &mut Xorshift| {
        if random.below(2) == 0 {
            random.below(columns)
        } else {
            let edge = random.below(8);
            if random.below(2) == 0 {
                edge
            } else {
                columns - 1 - edge
            }
        }
    };
    let mut ranges =
        vec![BufferRange::ordered(at(0, 0), at(columns - 1, rows - 1))];
    for _ in 0..count {
        let start = at(column(random), random.below(rows));
        let end = at(column(random), random.below(rows));
        ranges.push(BufferRange::ordered(start, end));
    }
    ranges
}

/// Compares the accessible text of `ranges` on the screen `harness` shows
/// with the runtime's Copy text of each.
fn check_parity(
    harness: &mut ViewHarness,
    name: &str,
    random: &mut Xorshift,
    parity: &mut Parity,
) {
    const SELECTIONS: usize = 160;
    let (snapshot, geometry, metrics, scale) = harness.read(|view, window| {
        (
            view.snapshot.clone().unwrap(),
            view.grid_geometry(window),
            view.metrics,
            window.scale_factor(),
        )
    });
    assert_eq!(snapshot.size, GridSize::clamped(49, 14), "{name}");
    let ranges = parity_ranges(&snapshot, random, SELECTIONS);
    let requests: Vec<_> = harness.update(|view, _, _| {
        ranges
            .iter()
            .map(|range| {
                view.viewer
                    .request_selection(snapshot.generation, *range)
                    .unwrap()
            })
            .collect()
    });
    let foreground = harness.cx.foreground_executor().clone();
    let rows = usize::from(snapshot.size.rows);
    let cell = |point: BufferPoint| {
        (
            usize::from(point.column),
            rows - 1 - point.rows_from_live_bottom,
        )
    };
    let mut builder = ModelBuilder::default();
    for (index, (range, request)) in ranges.iter().zip(requests).enumerate() {
        let copied = foreground
            .block_test(request.recv())
            .unwrap()
            .expect("the snapshot's generation is current");
        let model = builder.build(&Inputs {
            snapshot: &snapshot,
            geometry,
            selection: Some(*range),
            metrics,
            scale,
        });
        let accessible = if index == 0 {
            text(&model)
        } else {
            selected(&model)
                .map(|(_, _, text)| text)
                .unwrap_or_default()
        };
        parity.compared += 1;
        if index > 0
            && passes_a_wrapped_part_without_text(
                &snapshot,
                cell(range.start),
                cell(range.end),
            )
        {
            parity.exempt += 1;
            parity.exempt_differing += usize::from(accessible != copied);
        } else if accessible != copied {
            parity.mismatches.push(format!(
                "{name} {:?}..={:?}: copy {copied:?}, accessible \
                 {accessible:?}",
                cell(range.start),
                cell(range.end),
            ));
        }
    }
}

/// The convergence check for the accessible text: over real terminals, the
/// document reads what Copy copies from the whole displayed grid, and
/// every selection inside it reads what Copy copies from that selection,
/// except one that passes a soft-wrapped row part with no text, which the
/// document cannot hold (see [`passes_a_wrapped_part_without_text`]).
#[gpui::test]
fn the_accessible_text_matches_copy_for_random_selections(
    cx: &mut TestAppContext,
) {
    let mut random = Xorshift(0x5eed_0a11_u64);
    let mut parity = Parity::default();
    for (name, fixture) in parity_fixtures() {
        // The view starts the terminal unsized. Printed before its geometry
        // applies, the fixture's rows move when the grid resizes.
        let script = format!(
            "until [ \"$(stty size)\" = '14 49' ]; do sleep 0.01; done; {}",
            text_fixture(&fixture, "")
        );
        let mut harness = ViewHarness::new(cx, &script);
        harness.wait_ready(MouseTracking::Disabled);
        harness.wait_for("READY on row 12", |view, _| {
            view.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.rows[12].cells[..5]
                    .iter()
                    .map(|cell| cell.text.as_str())
                    .eq(["R", "E", "A", "D", "Y"])
            })
        });
        check_parity(&mut harness, name, &mut random, &mut parity);
    }
    println!(
        "A11Y_PARITY compared={} exempt={} exempt_differing={} \
         mismatches={}",
        parity.compared,
        parity.exempt,
        parity.exempt_differing,
        parity.mismatches.len()
    );
    assert!(
        parity.mismatches.is_empty(),
        "{} of {} differ:\n{}",
        parity.mismatches.len(),
        parity.compared,
        parity.mismatches.join("\n")
    );
}
