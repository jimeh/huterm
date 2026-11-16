//! Shell command blocks over a real terminal: navigation, the select
//! commands, the sticky prompt and its click, and the failure marks. The
//! fixture prints OSC 133 marks itself, so the runtime keeps records as it
//! would for a shell with integration.

use huterm_protocol::CommandInvocation;

use super::super::command_status;
use super::*;

/// Three commands of twenty output rows each, exiting 0, 1, and 2, then a
/// prompt and `READY`.
const COMMANDS: &str = "m() { printf '\\033]133;%s\\007' \"$1\"; }; \
    i=1; while [ $i -le 3 ]; do \
      m A; printf '~/src\\n$ '; m B; printf 'cmd %s\\n' $i; m C; \
      j=1; while [ $j -le 20 ]; do printf 'out %s.%s\\n' $i $j; j=$((j+1)); done; \
      m \"D;$((i-1))\"; i=$((i+1)); \
    done; \
    m A; printf '~/src\\n$ '; m B; printf READY; exec sleep 60";

fn harness(cx: &mut TestAppContext) -> ViewHarness {
    let mut harness = ViewHarness::new(cx, COMMANDS);
    harness.wait_for("the commands and their records", |view, _| {
        view.commands.len() == 3
            && view.snapshot.as_ref().is_some_and(|snapshot| {
                screen_text_of(snapshot).contains("READY")
                    && !snapshot.commands.is_empty()
            })
            && view.displayed_grid() == Some(view.requested_grid)
    });
    harness
}

fn screen_text_of(snapshot: &TerminalSnapshot) -> String {
    snapshot.cells().map(|cell| cell.text.displayed()).collect()
}

fn run(harness: &mut ViewHarness, id: huterm_protocol::CommandId) {
    harness.update(|view, window, cx| {
        view.run_command(&CommandInvocation::new(id, vec![]), window, cx)
            .unwrap();
    });
}

fn output(command: u8) -> String {
    (1..=20)
        .map(|line| format!("out {command}.{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The sequence of the block whose prompt is the viewport's top row.
fn top_block(view: &TerminalView) -> Option<u64> {
    let snapshot = view.snapshot.as_ref()?;
    let top =
        snapshot.viewport.bottom_offset + usize::from(snapshot.size.rows) - 1;
    snapshot
        .commands
        .iter()
        .find(|block| block.prompt == top)
        .map(|block| block.sequence)
}

fn wait_for_selection(harness: &mut ViewHarness, expected: &str) {
    harness
        .wait_for(&format!("the selection to read {expected:?}"), |view, _| {
            view.selected_text.as_deref() == Some(expected)
        });
}

#[gpui::test]
fn the_select_commands_take_the_newest_command_at_live_output(
    cx: &mut TestAppContext,
) {
    let mut harness = harness(cx);
    assert!(harness.read(|view, _| {
        view.command_availability(ids::SELECT_COMMAND_OUTPUT)
            .is_ok()
    }));
    run(&mut harness, ids::SELECT_COMMAND_OUTPUT);
    wait_for_selection(&mut harness, &output(3));
    run(&mut harness, ids::SELECT_COMMAND);
    wait_for_selection(&mut harness, &format!("~/src\n$ cmd 3\n{}", output(3)));
}

#[gpui::test]
fn navigation_moves_between_prompts_and_selection_follows_it(
    cx: &mut TestAppContext,
) {
    let mut harness = harness(cx);
    run(&mut harness, ids::PREVIOUS_COMMAND);
    harness.wait_for("command 3's prompt at the top", |view, _| {
        top_block(view) == Some(3)
    });
    run(&mut harness, ids::PREVIOUS_COMMAND);
    harness.wait_for("command 2's prompt at the top", |view, _| {
        top_block(view) == Some(2)
    });
    run(&mut harness, ids::SELECT_COMMAND_OUTPUT);
    wait_for_selection(&mut harness, &output(2));
    run(&mut harness, ids::NEXT_COMMAND);
    harness.wait_for("command 3's prompt at the top", |view, _| {
        top_block(view) == Some(3)
    });
    // The prompt after command 3 is on the live screen.
    run(&mut harness, ids::NEXT_COMMAND);
    harness.wait_for("live output", |view, _| {
        view.snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.viewport.bottom_offset == 0)
    });
}

#[gpui::test]
fn the_sticky_prompt_shows_inside_output_and_a_click_on_it_jumps_back(
    cx: &mut TestAppContext,
) {
    let mut harness = harness(cx);
    run(&mut harness, ids::PREVIOUS_COMMAND);
    run(&mut harness, ids::PREVIOUS_COMMAND);
    harness.wait_for("command 2's prompt at the top", |view, _| {
        top_block(view) == Some(2)
    });
    // Five rows toward live output, the top row is command 2's output.
    harness.update(|view, _, cx| {
        view.scroll_command(cx, |scroll, _| scroll.scroll_rows(-5));
    });
    harness.wait_for("command 2's sticky prompt", |view, _| {
        view.snapshot.as_ref().is_some_and(|snapshot| {
            command_status::header(snapshot, view.sticky_prompt)
                .is_some_and(|header| header.sequence == 2)
        })
    });
    // The status needs room beside the command row, which the header fits
    // to the columns left of it.
    let fitted = harness.read(|view, _| {
        view.sticky_fit.as_ref().map(|(_, columns, _)| *columns)
    });
    assert!(
        fitted.is_some_and(|columns| (10..49).contains(&columns)),
        "{fitted:?}"
    );
    // Command 2 exited 1: its rows are tinted, the others are not.
    let tinted = harness.read(|view, _| {
        let snapshot = view.snapshot.as_ref().unwrap();
        command_status::marks(snapshot, &view.commands, view.command_status)
    });
    assert!(!tinted.tints.is_empty(), "{tinted:?}");

    // The header is a button that names the command and its state.
    let tree: serde_json::Value = harness.cx.update(|window, _| {
        serde_json::from_str(&window.debug_a11y_tree_json().unwrap()).unwrap()
    });
    let labels: Vec<String> = tree["nodes"]
        .as_object()
        .unwrap()
        .values()
        .filter(|node| node["aria"]["role"] == "Button")
        .filter_map(|node| node["aria"]["label"].as_str().map(str::to_owned))
        .collect();
    assert!(
        labels.iter().any(|label| label.starts_with(
            "Scroll to prompt: $ cmd 2, failed with exit status 1 after"
        )),
        "{labels:?}"
    );

    let header = harness.cell_center(4, 0);
    harness.click(header);
    harness.wait_for("command 2's prompt at the top again", |view, _| {
        top_block(view) == Some(2)
    });
    assert!(
        harness.read(|view, _| view.selection.is_none() && !view.selecting),
        "a press on the header starts no selection"
    );
}

#[gpui::test]
fn an_end_with_no_new_rows_still_marks_the_command(cx: &mut TestAppContext) {
    // The fixture reports command 2's end only after it reads a line, with
    // no output, so only the status changes.
    let mut harness = ViewHarness::new(
        cx,
        "m() { printf '\\033]133;%s\\007' \"$1\"; }; \
         m A; printf '$ '; m B; printf 'make\\n'; m C; printf 'building\\n'; \
         stty -echo; printf READY; read line; m 'D;2'; exec sleep 60",
    );
    harness.wait_for("the running command", |view, _| {
        view.commands.len() == 1
            && view.snapshot.as_ref().is_some_and(|snapshot| {
                screen_text_of(snapshot).contains("READY")
            })
    });
    let marks = |harness: &mut ViewHarness| {
        harness.read(|view, _| {
            command_status::marks(
                view.snapshot.as_ref().unwrap(),
                &view.commands,
                view.command_status,
            )
        })
    };
    assert!(
        marks(&mut harness).is_empty(),
        "a running command is unknown"
    );
    harness.update(|view, _, _| {
        view.enqueue_input(TerminalInput::Text("\r".into()));
    });
    harness.wait_for("the command's end", |view, _| {
        view.commands.first().is_some_and(|record| {
            record.state
                == huterm_protocol::CommandState::Finished {
                    exit_code: Some(2),
                }
        })
    });
    // The prompt row, `building`, and `READY` after it.
    assert_eq!(
        marks(&mut harness).tints,
        std::iter::once(0..3).collect::<Vec<_>>()
    );
}

#[gpui::test]
fn the_select_commands_are_unavailable_without_a_command(
    cx: &mut TestAppContext,
) {
    let mut harness = ViewHarness::new(cx, &text_fixture("plain\\n", ""));
    harness.wait_ready(MouseTracking::Disabled);
    for id in [ids::SELECT_COMMAND_OUTPUT, ids::SELECT_COMMAND] {
        assert_eq!(
            harness.read(|view, _| view.command_availability(id)),
            Err(CommandError::Unavailable("no command here".to_owned()))
        );
    }
}

#[gpui::test]
fn a_running_commands_clock_keeps_its_wake_after_each_tick(
    cx: &mut TestAppContext,
) {
    // A command that printed a screenful and went quiet: nothing but the
    // clock's own wake repaints its sticky prompt.
    let mut harness = ViewHarness::new(
        cx,
        "m() { printf '\\033]133;%s\\007' \"$1\"; }; \
         m A; printf '$ '; m B; printf 'build\\n'; m C; \
         i=1; while [ $i -le 40 ]; do printf 'step %s\\n' $i; i=$((i+1)); done; \
         printf READY; exec sleep 60",
    );
    harness.wait_for("the running command's output", |view, _| {
        view.commands.len() == 1
            && view.snapshot.as_ref().is_some_and(|snapshot| {
                screen_text_of(snapshot).contains("READY")
            })
    });
    harness.update(|view, _, cx| {
        view.scroll_command(cx, |scroll, _| scroll.scroll_rows(10));
    });
    harness.wait_for("the running sticky prompt", |view, _| {
        view.sticky_tick.is_some()
    });
    // The clock's wake fires: the view clears it and repaints, and the
    // repaint must schedule the next one.
    for _ in 0..2 {
        harness.update(|view, window, cx| {
            let now = Instant::now();
            view.sticky_tick = Some(now);
            refresh::Animated::advance_animation(view, now, false, window, cx);
            assert!(view.sticky_tick.is_none());
        });
        harness.wait_for("the next wake", |view, _| view.sticky_tick.is_some());
        let id = harness.view.entity_id();
        let (tick, scheduled) = harness.read(|view, _| {
            (view.sticky_tick, view.frame_clock.deadline_of(id))
        });
        // The frame clock wakes at the earliest of the view's deadlines,
        // which must not be later than the clock's.
        assert!(
            scheduled.zip(tick).is_some_and(|(at, tick)| at <= tick),
            "the frame clock holds the clock's wake: {scheduled:?} {tick:?}"
        );
    }
}
