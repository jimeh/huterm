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
                std::fs::write(directory.join("state"), state)
                    .expect("write smoke state");
                let file = directory.join(format!("event-{sequence}"));
                if let Ok(command) = std::fs::read_to_string(&file) {
                    let result = if command == "shutdown" {
                        cx.update(|cx| cx.quit());
                        Ok(())
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
    let reloading = desktop.reloading;
    let bindings = desktop.config.keybindings.len();
    // Smoke state is read from an App update with no window on the stack.
    let view = cx
        .windows()
        .first()
        .and_then(gpui::AnyWindowHandle::downcast::<super::WorkspaceView>)
        .and_then(|handle| handle.read(cx).ok());
    let (tabs, selection, exited, ready, active) =
        view.map_or((0, false, false, false, 0), |view| {
            let terminal = view.active_view(cx);
            let terminal = terminal.as_ref().map(|tab| tab.read(cx));
            let selected = terminal.is_some_and(|tab| {
                tab.selection
                    .and_then(super::super::Selection::range)
                    .is_some()
            });
            let exited = terminal.is_some_and(|tab| tab.exited);
            let ready = terminal.is_some_and(|tab| {
                tab.snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot
                        .cells()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>()
                        .contains("READY")
                })
            });
            let active = view
                .tabs
                .iter()
                .position(|tab| Some(tab.id) == view.active_tab(cx))
                .unwrap_or(0);
            (view.tabs.len(), selected, exited, ready, active)
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
    format!(
        "policy={policy} reloading={reloading} bindings={bindings} tabs={tabs} selection={selection} pending={pending} exited={exited} ready={ready} active={active} {panes}"
    )
}

/// The active tab's panes, the focused pane's grid (its painted origin in
/// window points, its cell size, and its mouse tracking mode), and whether
/// a close is running or asking for confirmation.
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
                "grid_origin={},{} cell={},{} tracking={tracking:?}",
                f32::from(content.origin.x + grid.origin.x),
                f32::from(content.origin.y + grid.origin.y),
                f32::from(terminal.metrics.cell_width),
                f32::from(terminal.metrics.cell_height),
            )
        },
    );
    format!(
        "{} {grid} busy={} confirming={}",
        view.pane_smoke_state("", window, cx),
        view.busy,
        view.close.confirmation.is_some(),
    )
}
