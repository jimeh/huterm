//! File-driven native input probe. The desktop and PTY are production instances.

#[path = "input_smoke/native.rs"]
mod native;
pub(super) fn post_event(command: &str) -> anyhow::Result<()> {
    native::post(command)
}

pub(super) fn key_window_title() -> anyhow::Result<String> {
    native::key_window_title()
}

pub(super) fn perform_close(view: *mut std::ffi::c_void) -> anyhow::Result<()> {
    native::perform_close(view)
}

pub(crate) fn run() -> anyhow::Result<()> {
    if let Some(mode) = std::env::args().nth(1) {
        return native::clipboard_file(
            &mode,
            &std::env::args().nth(2).ok_or_else(|| {
                anyhow::anyhow!("clipboard helper requires a file")
            })?,
        );
    }
    native::validate_layout()?;
    let directory =
        std::path::PathBuf::from(std::env::var("HUTERM_INPUT_SMOKE")?);
    super::run_with_startup(move |cx| {
        cx.spawn(async move |cx| {
            let mut sequence = 0;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(10))
                    .await;
                let state = cx.update(read_state);
                // `fs::write` truncates first, so a reader could see an empty
                // file; a rename replaces the whole state at once.
                let temporary = directory.join("state.tmp");
                std::fs::write(&temporary, state).expect("write smoke state");
                std::fs::rename(temporary, directory.join("state"))
                    .expect("publish smoke state");
                let file = directory.join(format!("event-{sequence}"));
                if let Ok(command) = std::fs::read_to_string(&file) {
                    let result = if command == "shutdown" {
                        cx.update(|cx| cx.quit());
                        Ok(())
                    } else if let Some(id) = command.strip_prefix("invoke\t") {
                        cx.update(|cx| {
                            let spec = huterm_protocol::lookup(id).ok_or_else(
                                || {
                                    anyhow::anyhow!(
                                        "unknown smoke command {id}"
                                    )
                                },
                            )?;
                            super::Desktop::invoke(
                                cx,
                                &huterm_protocol::CommandInvocation::new(
                                    spec.id,
                                    Vec::new(),
                                ),
                                cx.active_window(),
                            )
                            .map(|_| ())
                            .map_err(anyhow::Error::from)
                        })
                    } else if let Some(text) =
                        command.strip_prefix("clipboard\t")
                    {
                        cx.update(|cx| {
                            cx.write_to_clipboard(
                                gpui::ClipboardItem::new_string(text.into()),
                            );
                        });
                        Ok(())
                    } else {
                        native::post(&command)
                    };
                    if let Err(error) = result {
                        eprintln!(
                            "NATIVE_INPUT_SMOKE event {sequence}: {error:#}"
                        );
                        cx.update(|cx| cx.quit());
                        return;
                    }
                    std::fs::write(
                        directory.join(format!("posted-{sequence}")),
                        "posted",
                    )
                    .expect("write native event acknowledgement");
                    sequence += 1;
                }
            }
        })
        .detach();
    })
}

fn read_state(cx: &mut gpui::App) -> String {
    let desktop = cx.global::<super::Desktop>();
    let policy = format!("{:?}", desktop.config.terminal.macos_option_as_alt);
    let reloading = desktop.reloads.reloading();
    let watching = if desktop.watch_status.watching {
        "on"
    } else {
        "off"
    };
    let reconciliations = desktop.watch_status.reconciliations;
    let bindings = desktop.config.keybindings.len();
    // Smoke state is read from an App update with no window on the stack.
    let view = cx
        .windows()
        .first()
        .and_then(gpui::AnyWindowHandle::downcast::<super::WorkspaceView>)
        .and_then(|handle| handle.read(cx).ok());
    let (tabs, selection, columns, exited, ready, active) =
        view.map_or((0, false, None, false, false, 0), |view| {
            let terminal = view.active_view(cx);
            let terminal = terminal.as_ref().map(|tab| tab.read(cx));
            let range = terminal.and_then(|tab| {
                tab.selection.and_then(super::super::Selection::range)
            });
            let selected = range.is_some();
            // The first and last selected columns tell a word from a line.
            let columns = range.map(|range| {
                format!("{}..{}", range.start.column, range.end.column)
            });
            let exited = terminal.is_some_and(|tab| tab.exited);
            let ready = terminal.is_some_and(|tab| {
                tab.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot
                        .cells()
                        .map(|cell| cell.text.displayed())
                        .collect::<String>()
                        .contains("READY")
                })
            });
            let active = view
                .tabs
                .iter()
                .position(|tab| Some(tab.id) == view.active_tab(cx))
                .unwrap_or(0);
            (view.tabs.len(), selected, columns, exited, ready, active)
        });
    let pending = cx.windows().first().copied().is_some_and(|window| {
        window
            .update(cx, |_, window, _| window.has_pending_keystrokes())
            .unwrap_or(false)
    });
    let panes = cx
        .windows()
        .first()
        .copied()
        .and_then(|window| window.update(cx, pane_state).ok())
        .unwrap_or_default();
    let windows = super::cross_window::smoke_state(cx);
    format!(
        "{windows}policy={policy} reloading={reloading} watching={watching} reconciliations={reconciliations} bindings={bindings} tabs={tabs} selection={selection} selection_columns={} pending={pending} exited={exited} ready={ready} active={active} {panes}",
        columns.as_deref().unwrap_or("none")
    )
}

/// The active tab's panes, the focused pane's grid (its painted origin in
/// window points, its cell size, and its mouse tracking mode), and whether
/// a close is running or asking for confirmation. `title_row_moves` counts
/// platform window moves started by a press and drag on the title row.
fn pane_state(
    root: gpui::AnyView,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> String {
    let Ok(view) = root.downcast::<super::WorkspaceView>() else {
        return String::new();
    };
    let view = view.read(cx);
    let grid = view.active_view(cx).map_or_else(
        || "grid_origin=none cell=0,0 tracking=None".to_owned(),
        |terminal| {
            let terminal = terminal.read(cx);
            let content = terminal.content_bounds(window);
            let grid = terminal.terminal_layout(window).bounds;
            let tracking = terminal
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.modes.mouse_tracking);
            format!(
                "grid_origin={},{} cell={},{} tracking={tracking:?} kitty={}",
                f32::from(content.origin.x + grid.origin.x),
                f32::from(content.origin.y + grid.origin.y),
                f32::from(terminal.metrics.cell_width),
                f32::from(terminal.metrics.cell_height),
                terminal
                    .snapshot
                    .as_ref()
                    .map_or(0, |snapshot| snapshot.modes.kitty_keyboard.bits()),
            )
        },
    );
    format!(
        "{} {grid} busy={} confirming={} title_row_moves={}",
        view.pane_smoke_state("", window, cx),
        view.busy,
        view.close.confirmation.is_some(),
        view.title_row_move_count(),
    )
}
