//! File-driven presentation query probe around production windows and PTYs.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::Context as _;
use gpui::App;
use huterm_protocol::{CommandInvocation, lookup};

use super::{Desktop, WorkspaceView};

pub(crate) fn run() -> anyhow::Result<()> {
    let directory = std::path::PathBuf::from(std::env::var(
        "HUTERM_PRESENTATION_QUERY_SMOKE",
    )?);
    super::run_with_startup(move |cx| {
        cx.spawn(async move |cx| {
            let mut sequence = 0;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(10))
                    .await;
                let mut state =
                    cx.update(read_state).expect("query smoke state");
                writeln!(state, "commands={sequence}").unwrap();
                publish(&directory, "state", &state);
                if let Ok(command) = std::fs::read_to_string(
                    directory.join(format!("command-{sequence}")),
                ) {
                    let result = cx
                        .update(|cx| execute(cx, command.trim()))
                        .and_then(std::convert::identity);
                    publish(
                        &directory,
                        &format!("result-{sequence}"),
                        &result.map_or_else(
                            |error| format!("error: {error:#}"),
                            |()| "ok".into(),
                        ),
                    );
                    sequence += 1;
                }
            }
        })
        .detach();
    })
}

fn execute(cx: &mut App, command: &str) -> anyhow::Result<()> {
    if command == "shutdown" {
        cx.quit();
        return Ok(());
    }
    let spec = lookup(command).context("unknown smoke command")?;
    let outcome = Desktop::invoke(
        cx,
        &CommandInvocation {
            id: spec.id,
            args: Vec::new(),
        },
        cx.windows().first().copied(),
    );
    anyhow::ensure!(outcome.is_ok(), "command failed: {outcome:?}");
    Ok(())
}

fn publish(directory: &Path, name: &str, text: &str) {
    let temporary = directory.join(format!("{name}.tmp"));
    std::fs::write(&temporary, text).expect("write query smoke state");
    std::fs::rename(temporary, directory.join(name))
        .expect("publish query smoke state");
}

fn read_state(cx: &mut App) -> String {
    let mut output = format!(
        "windows={}\nreloading={}\n",
        cx.windows().len(),
        cx.global::<Desktop>().reloading
    );
    if let Some(handle) = cx.windows().first().copied() {
        let _ = handle.update(cx, |root, window, cx| {
            let Ok(root) = root.downcast::<WorkspaceView>() else {
                return;
            };
            let workspace = root.read(cx);
            writeln!(output, "tabs={}", workspace.tabs.len()).unwrap();
            for (index, tab) in workspace.tabs.iter().enumerate() {
                let terminal = tab.view.read(cx);
                let cell = terminal.physical_cell_size();
                let grid = terminal.last_grid_size;
                let layout = terminal.terminal_layout(window);
                let theme = &terminal.theme;
                writeln!(
                    output,
                    "tab{index}.active={}",
                    Some(tab.id) == workspace.active
                )
                .unwrap();
                writeln!(output, "tab{index}.visible={}", terminal.visible)
                    .unwrap();
                writeln!(
                    output,
                    "tab{index}.grid={},{}",
                    grid.columns, grid.rows
                )
                .unwrap();
                writeln!(
                    output,
                    "tab{index}.layout_grid={},{}",
                    layout.grid.columns, layout.grid.rows
                )
                .unwrap();
                writeln!(
                    output,
                    "tab{index}.cell={},{}",
                    cell.width, cell.height
                )
                .unwrap();
                writeln!(
                    output,
                    "tab{index}.pixels={},{}",
                    u32::from(cell.width) * u32::from(grid.columns),
                    u32::from(cell.height) * u32::from(grid.rows)
                )
                .unwrap();
                writeln!(
                    output,
                    "tab{index}.font={}",
                    f32::from(terminal.font_size)
                )
                .unwrap();
                writeln!(
                    output,
                    "tab{index}.padding={},{}",
                    terminal.window_config.padding_x,
                    terminal.window_config.padding_y
                )
                .unwrap();
                writeln!(
                    output,
                    "tab{index}.foreground={:02x}{:02x}{:02x}",
                    theme.foreground.red,
                    theme.foreground.green,
                    theme.foreground.blue
                )
                .unwrap();
                writeln!(
                    output,
                    "tab{index}.background={:02x}{:02x}{:02x}",
                    theme.background.red,
                    theme.background.green,
                    theme.background.blue
                )
                .unwrap();
            }
        });
    }
    output
}
