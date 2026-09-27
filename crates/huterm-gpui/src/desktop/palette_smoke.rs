//! File-driven probe around the production palette, window router, and PTY.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use gpui::App;
use huterm_protocol::{
    CommandArgument, CommandInvocation, CommandValue, TabId, TerminalInput,
    WorkspaceId, ids,
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
    #[cfg(target_os = "macos")]
    if command == "native-title" {
        return super::input_smoke::key_window_title();
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
    if let Some(id) = command.strip_prefix("invoke\t") {
        return invoke_catalog(cx, handle, id);
    }
    if command == "clipboard" {
        return Ok(cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default());
    }
    if let Some(size) = command.strip_prefix("resize\t") {
        return resize_window(cx, handle, size);
    }
    if let Some(message) = command.strip_prefix("terminal-failure\t") {
        return report_terminal_failure(cx, handle, message);
    }
    if let Some(invocation) = fixed_invocation(command) {
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
            "dismiss-notices" => {
                view.notices.dismiss_all();
                cx.notify();
                Ok("notices dismissed".to_owned())
            }
            "focus-terminal" => {
                view.active_view()
                    .context("active terminal")?
                    .read(cx)
                    .focus
                    .focus(window);
                Ok("terminal focused".to_owned())
            }
            "input-barrier" => {
                view.active_view()
                    .context("active terminal")?
                    .read(cx)
                    .client
                    .send_input(TerminalInput::Text("\x1f".into()))?;
                Ok("input barrier queued".to_owned())
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

/// Resizes the first window's content to `size`, a tab-separated width
/// and height in points.
fn resize_window(
    cx: &mut App,
    handle: gpui::AnyWindowHandle,
    size: &str,
) -> anyhow::Result<String> {
    let mut values = size.split('\t').map(str::parse::<f32>);
    let (Some(Ok(width)), Some(Ok(height)), None) =
        (values.next(), values.next(), values.next())
    else {
        anyhow::bail!("resize takes a width and a height");
    };
    handle.update(cx, |_, window, _| {
        window.resize(gpui::size(gpui::px(width), gpui::px(height)));
    })?;
    Ok("resize requested".to_owned())
}

/// The `invoke-*` commands that run a fixed invocation through
/// [`Desktop::invoke`].
fn fixed_invocation(command: &str) -> Option<CommandInvocation> {
    match command {
        "invoke-new-tab" => Some(CommandInvocation::new(ids::NEW_TAB, vec![])),
        "invoke-reload" => {
            Some(CommandInvocation::new(ids::RELOAD_CONFIG, vec![]))
        }
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
    }
}

/// `terminal-failure\t<message>` queues a failure on the active terminal as
/// its runtime would, so it reaches the window through the tab's activity
/// drain and replaces the tab's earlier toast. Repeated messages are dropped
/// by the queue, so each call needs a distinct one.
fn report_terminal_failure(
    cx: &mut App,
    handle: gpui::AnyWindowHandle,
    message: &str,
) -> anyhow::Result<String> {
    let message = message.to_owned();
    handle.update(cx, |root, _, cx| -> anyhow::Result<String> {
        let view = root
            .downcast::<WorkspaceView>()
            .map_err(|_| anyhow::anyhow!("workspace root"))?;
        let terminal =
            view.read(cx).active_view().context("active terminal")?;
        let queued = terminal.update(cx, |terminal, _| {
            terminal.report_failure(
                super::notices::Severity::Error,
                "Terminal error",
                message,
            )
        });
        Ok(format!("terminal failure queued={queued}"))
    })?
}

/// `invoke\t<command id>` runs any argument-free catalog command through
/// the production router, as a binding or menu pick would.
fn invoke_catalog(
    cx: &mut App,
    handle: gpui::AnyWindowHandle,
    id: &str,
) -> anyhow::Result<String> {
    let spec = huterm_protocol::catalog()
        .iter()
        .find(|spec| spec.id.as_str() == id)
        .with_context(|| format!("unknown catalog command {id:?}"))?;
    let invocation = CommandInvocation::new(spec.id, vec![]);
    Ok(format!(
        "{:?}",
        Desktop::invoke(cx, &invocation, Some(handle))
    ))
}

fn quake_state(cx: &App) -> anyhow::Result<String> {
    let mut output = String::new();
    // Smoke commands run from an App update with no window on the stack.
    let viewpoint = super::quake_windows::Viewpoint::Outside;
    for row in super::quake_windows::profile_rows(cx, &viewpoint) {
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

/// Space-separated notice fields for the single-line palette state.
fn smoke_notices<'a>(
    prefix: &str,
    notices: impl Iterator<Item = &'a super::notices::NoticeContent>,
) -> String {
    let lines: Vec<String> = notices
        .map(super::notices::NoticeContent::smoke_line)
        .collect();
    let mut output = format!("{prefix}notices={}", lines.len());
    for (index, line) in lines.iter().enumerate() {
        write!(output, " {prefix}notice{index}={line:?}").unwrap();
    }
    output
}

fn read_state(cx: &mut App) -> String {
    // Desktop notices new windows raise (diagnostics, then latched
    // failures) as `desktop.notices=<n>` and Debug-quoted
    // `desktop.notice<i>="<severity>|<source>|<message>"`; windows list
    // theirs as `w<i>.notices=` and `w<i>.notice<j>=` on their line.
    let desktop = cx.global::<Desktop>();
    let mut output = format!(
        "windows={} config.warning={:?} {}\n",
        cx.windows().len(),
        desktop.config.warning,
        smoke_notices(
            "desktop.",
            desktop.diagnostics.iter().chain(&desktop.latched)
        )
    );
    for (index, handle) in cx.windows().into_iter().enumerate() {
        let _ = handle.update(cx, |root, window, cx| {
            let Ok(entity) = root.downcast::<WorkspaceView>() else {
                return;
            };
            let ui = ui_state(&entity, index, window, cx);
            let view = entity.read(cx);
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
                "w{index}.palette={} w{index}.palette_focused={palette_focused} w{index}.terminal_focused={terminal_focused} w{index}.active={} w{index}.tabs={} w{index}.active_index={active_index} w{index}.busy={} w{index}.mouse={terminal_mouse} {} w{index}.text={:?}",
                palette.is_some(),
                window.is_window_active(),
                view.tabs.len(),
                view.busy,
                smoke_notices(&format!("w{index}."), view.notices.contents()),
                terminal_text
            )
            .unwrap();
            writeln!(output, "{ui}").unwrap();
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

/// Transient UI, tab geometry, and the active terminal's scroll state for
/// pointer fixtures, as `w<index>.` fields: `scrolled` is the displayed
/// offset (distinct from the palette's `scroll_offset`), `scroll_pill`
/// whether the pill is drawn, `grid` the last PTY size as `columns,rows`,
/// `grid_bounds` the painted grid in window points, `cell` its cell size,
/// `selection` whether text is selected, `selecting` whether a selection
/// drag is still in progress, and `history` the scrollback rows.
fn ui_state(
    entity: &gpui::Entity<WorkspaceView>,
    index: usize,
    window: &mut gpui::Window,
    cx: &mut App,
) -> String {
    let prefix = format!("w{index}.");
    entity.update(cx, |view, cx| {
        let terminal_line = view.active_view().map_or_else(
            || {
                format!(
                    "{prefix}terminal_bounds=none {prefix}scrolled=0 {prefix}scroll_pill=false {prefix}grid=0,0 {prefix}grid_bounds=none {prefix}cell=0,0 {prefix}selection=false {prefix}selecting=false {prefix}history=0"
                )
            },
            |terminal| {
                let terminal = terminal.read(cx);
                let content = terminal.content_bounds(window);
                let grid = terminal.terminal_layout(window).bounds;
                format!(
                    "{prefix}terminal_bounds={} {prefix}scrolled={} {prefix}scroll_pill={} {prefix}grid={},{} {prefix}grid_bounds={} {prefix}cell={},{} {prefix}selection={} {prefix}selecting={} {prefix}history={}",
                    rect(content),
                    terminal.scroll.displayed(),
                    terminal.scroll_pill_visible(),
                    terminal.last_grid_size.columns,
                    terminal.last_grid_size.rows,
                    rect(gpui::Bounds::new(content.origin + grid.origin, grid.size)),
                    f32::from(terminal.metrics.cell_width),
                    f32::from(terminal.metrics.cell_height),
                    terminal.command_availability(ids::COPY).is_ok(),
                    terminal.selecting,
                    terminal
                        .snapshot
                        .as_ref()
                        .map_or(0, |snapshot| snapshot.history_size),
                )
            },
        );
        format!(
            "{} {prefix}tabs_rects={} {terminal_line}",
            view.ui_smoke_state(&prefix, window, cx),
            view.tab_smoke_rects(window)
        )
    })
}

fn rect(value: gpui::Bounds<gpui::Pixels>) -> String {
    format!(
        "{},{},{},{}",
        f32::from(value.origin.x),
        f32::from(value.origin.y),
        f32::from(value.size.width),
        f32::from(value.size.height)
    )
}

fn publish(directory: &Path, name: &str, text: &str) {
    let temporary = directory.join(format!("{name}.tmp"));
    std::fs::write(&temporary, text).expect("write palette smoke state");
    std::fs::rename(temporary, directory.join(name))
        .expect("publish palette smoke state");
}
