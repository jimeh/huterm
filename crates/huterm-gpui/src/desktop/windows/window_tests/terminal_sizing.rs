//! Tier 2 tests for terminal sizing: Set Terminal Size fixes a terminal's
//! grid until Clear Terminal Size, and a reload sends the sizing policy to
//! an open terminal another client also sizes.

use huterm_core::ViewerOptions;
use huterm_protocol::{
    CommandArgument, CommandInvocation, CommandValue, ViewerCapabilities,
};

use super::*;

/// A window showing `tab` once its child printed READY, and the grid the
/// window gave the terminal.
fn ready(
    harness: &mut DesktopHarness,
    tab: TabId,
) -> (AnyWindowHandle, GridSize) {
    let window = harness.attach_window();
    harness.activate(window, tab);
    harness.wait_text(window, tab, "READY");
    let size = harness.canonical(tab).size;
    (window, size)
}

fn invoke_sizing(
    harness: &mut DesktopHarness,
    window: AnyWindowHandle,
    command: huterm_protocol::CommandId,
    arguments: &[(&str, i64)],
) {
    let invocation = CommandInvocation::new(
        command,
        arguments
            .iter()
            .map(|(name, value)| {
                CommandArgument::new(*name, CommandValue::Integer(*value))
            })
            .collect(),
    );
    harness
        .cx
        .update(|cx| Desktop::invoke(cx, &invocation, Some(window)))
        .unwrap();
}

#[gpui::test]
fn set_terminal_size_fixes_the_grid_until_it_is_cleared(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, own) = ready(&mut harness, tab);
    invoke_sizing(
        &mut harness,
        window,
        ids::SET_TERMINAL_SIZE,
        &[("columns", 33), ("rows", 7)],
    );
    harness.wait_canonical(tab, "the set size", |snapshot| {
        snapshot.size == GridSize::clamped(33, 7)
    });
    // An omitted dimension keeps the size the view shows.
    invoke_sizing(
        &mut harness,
        window,
        ids::SET_TERMINAL_SIZE,
        &[("columns", 40)],
    );
    harness.wait_canonical(tab, "the view's rows", |snapshot| {
        snapshot.size == GridSize::clamped(40, own.rows)
    });
    invoke_sizing(&mut harness, window, ids::CLEAR_TERMINAL_SIZE, &[]);
    harness.wait_canonical(tab, "the view's own size", |snapshot| {
        snapshot.size == own
    });
}

#[gpui::test]
fn a_reload_sends_the_sizing_policy_to_an_open_terminal(
    cx: &mut TestAppContext,
) {
    let mut harness = DesktopHarness::new(cx);
    let tab = harness.open_tab(ECHO);
    let (window, own) = ready(&mut harness, tab);
    // Another client's larger view takes control under `latest`.
    let larger = GridSize::clamped(own.columns + 20, own.rows + 5);
    let terminal_id = harness.cx.update(|cx| {
        cx.global::<Desktop>()
            .hierarchy
            .state()
            .tab(tab)
            .unwrap()
            .layout
            .first_pane()
            .terminal_id
    });
    let _other = {
        let mut mux = harness.runtime.lock();
        let attachment = mux.attach_session(harness.session).unwrap();
        mux.subscribe_terminal(
            attachment,
            terminal_id,
            ViewerOptions {
                focused: true,
                geometry: Some((
                    larger,
                    CellSize {
                        width: 1,
                        height: 1,
                    },
                )),
                ..ViewerOptions::new(ViewerCapabilities::ALL)
            },
        )
        .unwrap()
    };
    harness.wait_canonical(tab, "the other client's size", |snapshot| {
        snapshot.size == larger
    });
    let reloaded = config::TerminalConfig {
        sizing: huterm_config::TerminalSizing::Smallest,
        ..config::TerminalConfig::default()
    };
    harness.cx.update(|cx| {
        terminal(cx, window, tab)
            .unwrap()
            .update(cx, |view, cx| view.reload_terminal_config(reloaded, cx));
    });
    harness.wait_canonical(tab, "the smaller view's size", |snapshot| {
        snapshot.size == own
    });
}
