//! File-driven probe for session restore: drives the production desktop
//! through catalog commands and publishes what each window restores.
//!
//! Commands arrive as `command-<n>` files, one per line of tab-separated
//! fields, and each result is written to `result-<n>`:
//!
//! - `invoke <window|-> <command> [<name>=<text>]...` runs a catalog
//!   command in the window at that opening-order index, or without one;
//! - `type <window> <text>` sends text to the window's focused terminal,
//!   with `\r` for Enter.
//!
//! `state` lists the desktop as `windows=<n> restoring=<bool>
//! persistence=<state>`, then one line per window in opening order with
//! its session name, native bounds and mode, active tab index, and per tab
//! its name, split weights, focused pane index, and each pane's name and
//! reported directory.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use gpui::{App, WindowBounds};
use huterm_protocol::{
    CommandArgument, CommandInvocation, CommandValue, PaneNode, TerminalInput,
    lookup,
};

use super::{Desktop, WorkspaceView};

pub(crate) fn run() -> anyhow::Result<()> {
    let directory = PathBuf::from(std::env::var("HUTERM_RESTORE_SMOKE")?);
    super::run_with_startup(move |cx| {
        cx.spawn(async move |cx| {
            let mut sequence = 0;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(20))
                    .await;
                publish(&directory, "state", &cx.update(read_state));
                let Ok(command) = std::fs::read_to_string(
                    directory.join(format!("command-{sequence}")),
                ) else {
                    continue;
                };
                let result = cx.update(|cx| execute(cx, command.trim_end()));
                publish(
                    &directory,
                    &format!("result-{sequence}"),
                    &result.unwrap_or_else(|error| format!("error: {error}")),
                );
                sequence += 1;
            }
        })
        .detach();
    })
}

fn publish(directory: &Path, name: &str, text: &str) {
    let temporary = directory.join(format!("{name}.tmp"));
    std::fs::write(&temporary, text).expect("write restore smoke state");
    std::fs::rename(temporary, directory.join(name))
        .expect("publish restore smoke state");
}

/// The window at `index` in opening order.
fn window(cx: &App, index: &str) -> anyhow::Result<gpui::AnyWindowHandle> {
    let index: usize = index.parse().context("window index")?;
    let id = cx
        .global::<Desktop>()
        .windows
        .records()
        .filter(|record| !record.closing)
        .nth(index)
        .map(|record| record.id)
        .context("no such window")?;
    cx.windows()
        .into_iter()
        .find(|handle| handle.window_id() == id)
        .context("window closed")
}

fn execute(cx: &mut App, command: &str) -> anyhow::Result<String> {
    let mut fields = command.split('\t');
    match fields.next() {
        Some("invoke") => {
            let target = fields.next().context("window")?;
            let id = fields.next().context("command")?;
            let spec = lookup(id).with_context(|| format!("unknown {id}"))?;
            let args = fields
                .map(|field| {
                    let (name, value) =
                        field.split_once('=').context("name=value")?;
                    Ok(CommandArgument::new(
                        name.to_owned(),
                        CommandValue::Text(value.to_owned()),
                    ))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let handle = if target == "-" {
                None
            } else {
                Some(window(cx, target)?)
            };
            let invocation = CommandInvocation::new(spec.id, args);
            super::Desktop::invoke(cx, &invocation, handle)
                .map(|outcome| format!("{outcome:?}"))
                .map_err(|error| anyhow::anyhow!("{error}"))
        }
        Some("type") => {
            let handle = window(cx, fields.next().context("window")?)?;
            let text = fields.next().context("text")?.replace("\\r", "\r");
            handle.update(cx, |root, _, cx| {
                let view =
                    root.downcast::<WorkspaceView>().ok().context("root")?;
                let terminal =
                    view.read(cx).active_view(cx).context("no terminal")?;
                terminal
                    .read(cx)
                    .viewer
                    .send_input(TerminalInput::Text(text))
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                Ok("typed".to_owned())
            })?
        }
        other => bail!("unknown command {other:?}"),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one state dump lists every window, tab, and pane"
)]
fn read_state(cx: &mut App) -> String {
    let desktop = cx.global::<Desktop>();
    let mut output = format!(
        "windows={} restoring={} persistence={:?}\n",
        desktop.windows.records().filter(|r| !r.closing).count(),
        desktop.restoring.is_some(),
        desktop.runtime.persistence.state(),
    );
    let records: Vec<_> = desktop
        .windows
        .records()
        .filter(|record| !record.closing)
        .map(|record| (record.id, record.session))
        .collect();
    for (index, (id, session)) in records.into_iter().enumerate() {
        let Some(handle) = cx
            .windows()
            .into_iter()
            .find(|handle| handle.window_id() == id)
        else {
            continue;
        };
        let _ = handle.update(cx, |root, window, cx| {
            let Ok(view) = root.downcast::<WorkspaceView>() else {
                return;
            };
            let view = view.read(cx);
            let state = cx.global::<Desktop>().hierarchy.state();
            let session = session
                .and_then(|session| state.session(session))
                .map_or_else(String::new, |info| {
                    info.custom_name
                        .clone()
                        .unwrap_or_else(|| info.automatic_name.clone())
                });
            let (mode, bounds) = match window.window_bounds() {
                WindowBounds::Windowed(bounds) => ("windowed", bounds),
                WindowBounds::Maximized(bounds) => ("maximized", bounds),
                WindowBounds::Fullscreen(bounds) => ("fullscreen", bounds),
            };
            let active = view
                .tabs
                .iter()
                .position(|tab| Some(tab.id) == view.active_tab(cx))
                .map_or_else(|| "none".to_owned(), |index| index.to_string());
            write!(
                output,
                "w{index}.session={session:?} w{index}.bounds={},{},{},{} w{index}.mode={mode} w{index}.fullscreen={} w{index}.active={active} w{index}.tabs={}",
                f32::from(bounds.origin.x),
                f32::from(bounds.origin.y),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
                window.is_fullscreen(),
                view.tabs.len(),
            )
            .unwrap();
            for (number, tab) in view.tabs.iter().enumerate() {
                let Some(info) = state.tab(tab.id) else {
                    continue;
                };
                let weights = match info.layout.root() {
                    PaneNode::Split(split) => split
                        .children
                        .iter()
                        .map(|child| child.weight.to_string())
                        .collect::<Vec<_>>()
                        .join("/"),
                    PaneNode::Pane(_) => "-".to_owned(),
                };
                let panes = info.layout.panes();
                let focus = panes
                    .iter()
                    .position(|pane| pane.id == tab.focused().pane)
                    .map_or_else(|| "none".to_owned(), |index| index.to_string());
                let described: Vec<String> = panes
                    .iter()
                    .map(|pane| {
                        let directory = tab
                            .pane(pane.id)
                            .and_then(|view| {
                                view.view
                                    .read(cx)
                                    .metadata
                                    .directory()
                                    .map(|directory| directory.path().to_owned())
                            })
                            .unwrap_or_default();
                        format!(
                            "{}|{directory}",
                            pane.custom_name.as_deref().unwrap_or("")
                        )
                    })
                    .collect();
                write!(
                    output,
                    " w{index}.t{number}.name={:?} w{index}.t{number}.weights={weights} w{index}.t{number}.focus={focus} w{index}.t{number}.panes={:?}",
                    info.custom_name.as_deref().unwrap_or(""),
                    described.join(";"),
                )
                .unwrap();
            }
            output.push('\n');
        });
    }
    output
}
