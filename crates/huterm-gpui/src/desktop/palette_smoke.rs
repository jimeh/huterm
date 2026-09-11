//! File-driven probe around the production palette, window router, and PTY.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use gpui::App;
use huterm_protocol::{
    CommandArgument, CommandInvocation, CommandValue, TabId, WorkspaceId, ids,
};

use super::{Desktop, WorkspaceView, open_window};

pub(crate) fn run() -> anyhow::Result<()> {
    let directory = PathBuf::from(std::env::var("HUTERM_PALETTE_SMOKE")?);
    super::run_with_startup(move |cx| {
        cx.spawn(async move |cx| {
            let mut sequence = 0;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(10))
                    .await;
                let state = cx.update(read_state).expect("palette smoke state");
                publish(
                    &directory,
                    "state",
                    &format!("command_sequence={sequence}\n{state}"),
                );
                let command_path =
                    directory.join(format!("command-{sequence}"));
                let Ok(command) = std::fs::read_to_string(command_path) else {
                    continue;
                };
                let result = execute(&command, cx).await;
                publish(
                    &directory,
                    &format!("result-{sequence}"),
                    &result.unwrap_or_else(|error| format!("error: {error:#}")),
                );
                sequence += 1;
            }
        })
        .detach();
    })
}

async fn execute(
    command: &str,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<String> {
    #[cfg(target_os = "macos")]
    if let Some(event) = command.strip_prefix("native\t") {
        super::input_smoke::post_event(event)?;
        return Ok("posted".to_owned());
    }

    if command == "core-state" {
        let (runtime, workspace, tab) = cx.update(|cx| {
            let (workspace, tab) = window_target(cx, 0)?;
            Ok::<_, anyhow::Error>((
                Arc::clone(&cx.global::<Desktop>().runtime),
                workspace,
                tab,
            ))
        })??;
        return cx
            .background_executor()
            .spawn(async move { core_state(&runtime, workspace, tab) })
            .await;
    }

    if command == "delete-target" {
        let (runtime, workspace, tab) = cx.update(|cx| {
            let (workspace, tab) = window_target(cx, 0)?;
            Ok::<_, anyhow::Error>((
                Arc::clone(&cx.global::<Desktop>().runtime),
                workspace.context("workspace target")?,
                tab.context("tab target")?,
            ))
        })??;
        return cx
            .background_executor()
            .spawn(async move {
                let mut mux = runtime
                    .mux
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                mux.close_tab(workspace, tab)?;
                Ok("target deleted".to_owned())
            })
            .await;
    }

    if command == "remove-shell" {
        let shell = std::env::var_os("SHELL").context("SHELL")?;
        std::fs::remove_file(shell).context("remove palette smoke shell")?;
        return Ok("shell removed".to_owned());
    }

    cx.update(|cx| execute_ui(cx, command))?
}

fn execute_ui(cx: &mut App, command: &str) -> anyhow::Result<String> {
    if command == "quake-state" {
        return quake_state(cx);
    }
    if command == "open-second" {
        open_window(cx);
        return Ok("window requested".to_owned());
    }
    if command == "quit" {
        super::approved_quit(cx);
        return Ok("approved quit requested".to_owned());
    }
    let handle = *cx.windows().first().context("palette smoke window")?;
    let invocation = match command {
        "invoke-new-tab" => Some(CommandInvocation::new(ids::NEW_TAB, vec![])),
        "invoke-rename-tab" => Some(CommandInvocation::new(
            ids::RENAME_TAB,
            vec![CommandArgument::new(
                "name",
                CommandValue::Text("blocked".to_owned()),
            )],
        )),
        "invoke-palette" => {
            Some(CommandInvocation::new(ids::OPEN_COMMAND_PALETTE, vec![]))
        }
        _ => None,
    };
    if let Some(invocation) = invocation {
        return Ok(format!(
            "{:?}",
            Desktop::invoke(cx, &invocation, Some(handle))
        ));
    }
    handle.update(cx, |root, window, cx| -> anyhow::Result<String> {
        let view = root
            .downcast::<WorkspaceView>()
            .map_err(|_| anyhow::anyhow!("workspace root"))?;
        view.update(cx, |view, cx| match command {
            "activate-first" => {
                window.activate_window();
                cx.activate(true);
                Ok("first window activated".to_owned())
            }
            "busy-on" => {
                view.busy = true;
                cx.notify();
                Ok("busy".to_owned())
            }
            "busy-off" => {
                view.busy = false;
                cx.notify();
                Ok("idle".to_owned())
            }
            "clear-status" => {
                view.status = None;
                cx.notify();
                Ok("status cleared".to_owned())
            }
            "focus-terminal" => {
                view.active_view()
                    .context("active terminal")?
                    .read(cx)
                    .focus
                    .focus(window);
                Ok("terminal focused".to_owned())
            }
            "open-explicit" => {
                let tab = view.active.context("active tab")?;
                let invocation = CommandInvocation::new(
                    ids::RENAME_TAB,
                    vec![
                        CommandArgument::new(
                            "name",
                            CommandValue::Text("explicit".to_owned()),
                        ),
                        CommandArgument::new("tab", CommandValue::Tab(tab)),
                    ],
                );
                view.open_palette_request(Some(&invocation), window, cx)?;
                Ok("explicit palette opened".to_owned())
            }
            _ => {
                anyhow::bail!("unknown palette smoke command {command:?}")
            }
        })
    })?
}

fn quake_state(cx: &App) -> anyhow::Result<String> {
    let mut output = String::new();
    for row in super::quake_windows::profile_rows(cx) {
        let state = match row.state {
            super::quake_windows::ProfileState::NotSummoned => "not-summoned",
            super::quake_windows::ProfileState::Hidden { .. } => "hidden",
            super::quake_windows::ProfileState::Visible => "visible",
        };
        write!(output, "{}={state} ", row.name)?;
    }
    Ok(output.trim_end().to_owned())
}

fn workspace_view(
    cx: &mut App,
    index: usize,
) -> anyhow::Result<gpui::Entity<WorkspaceView>> {
    let handle = *cx.windows().get(index).context("smoke window index")?;
    handle.update(cx, |root, _, _| {
        root.downcast::<WorkspaceView>()
            .map_err(|_| anyhow::anyhow!("workspace root"))
    })?
}

fn window_target(
    cx: &mut App,
    index: usize,
) -> anyhow::Result<(Option<WorkspaceId>, Option<TabId>)> {
    let view = workspace_view(cx, index)?;
    let view = view.read(cx);
    Ok((view.workspace, view.active))
}

fn core_state(
    runtime: &super::DesktopRuntime,
    workspace: Option<WorkspaceId>,
    tab: Option<TabId>,
) -> anyhow::Result<String> {
    let hierarchy = runtime
        .mux
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .capture_hierarchy();
    let mut output = format!("workspace={workspace:?} tab={tab:?}");
    for workspace in hierarchy.workspaces {
        for tab in workspace.tabs {
            write!(output, " tab.{:?}.name={:?}", tab.id, tab.custom_name())?;
        }
    }
    Ok(output)
}

fn read_state(cx: &mut App) -> String {
    let mut output = format!("windows={}\n", cx.windows().len());
    for (index, handle) in cx.windows().into_iter().enumerate() {
        let _ = handle.update(cx, |root, window, cx| {
            let Ok(view) = root.downcast::<WorkspaceView>() else {
                return;
            };
            let view = view.read(cx);
            let palette = view.palette.as_ref();
            let palette_focused = palette.is_some_and(|palette| {
                palette.read(cx).focus_handle(cx).is_focused(window)
            });
            let terminal = view.active_view();
            let terminal_focused = terminal.as_ref().is_some_and(|terminal| {
                terminal.read(cx).focus.is_focused(window)
            });
            let terminal_text = terminal
                .as_ref()
                .and_then(|terminal| terminal.read(cx).snapshot.clone())
                .map(|snapshot| {
                    snapshot
                        .cells()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>()
                        .replace('\n', " ")
                })
                .unwrap_or_default();
            let terminal_mouse = terminal
                .as_ref()
                .map_or_else(
                    || "None".to_owned(),
                    |terminal| {
                        terminal.read(cx).snapshot.as_ref().map_or_else(
                            || "None".to_owned(),
                            |snapshot| {
                                format!("{:?}", snapshot.modes.mouse_tracking)
                            },
                        )
                    },
                );
            let active_index = view
                .tabs
                .iter()
                .position(|tab| Some(tab.id) == view.active)
                .map_or_else(|| "none".to_owned(), |index| index.to_string());
            writeln!(
                output,
                "w{index}.palette={} w{index}.palette_focused={palette_focused} w{index}.terminal_focused={terminal_focused} w{index}.tabs={} w{index}.active_index={active_index} w{index}.busy={} w{index}.mouse={terminal_mouse} w{index}.status={:?} w{index}.text={:?}",
                palette.is_some(),
                view.tabs.len(),
                view.busy,
                view.status,
                terminal_text
            )
            .unwrap();
            if let Some(palette) = palette {
                writeln!(
                    output,
                    "w{index}.palette_state={}",
                    palette.read(cx).smoke_state(cx)
                )
                .unwrap();
            }
        });
    }
    output
}

fn publish(directory: &Path, name: &str, text: &str) {
    let temporary = directory.join(format!("{name}.tmp"));
    std::fs::write(&temporary, text).expect("write palette smoke state");
    std::fs::rename(temporary, directory.join(name))
        .expect("publish palette smoke state");
}
