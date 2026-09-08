//! File-driven probe around the production window, command router and PTY.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context as _, ensure};
use gpui::{App, Bounds, Pixels, WindowBounds};
use huterm_protocol::{CommandInvocation, lookup};

use super::{Desktop, WorkspaceView};

pub(crate) fn run() -> anyhow::Result<()> {
    let directory =
        std::path::PathBuf::from(std::env::var("HUTERM_FULLSCREEN_SMOKE")?);
    super::run_with_startup(move |cx| {
        let quit_directory = directory.clone();
        cx.on_app_quit(move |cx| {
            let runtime = &cx.global::<Desktop>().runtime;
            if let Some(snapshot) = runtime.restore.lock().unwrap().as_ref() {
                let mut output = String::new();
                for (index, window) in snapshot.windows.iter().enumerate() {
                    writeln!(
                        output,
                        "restore{index}={}",
                        bounds(window.bounds)
                    )
                    .unwrap();
                }
                std::fs::write(quit_directory.join("restore"), output).unwrap();
            }
            async {}
        })
        .detach();
        cx.spawn(async move |cx| {
            let mut sequence = 0;
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(10))
                    .await;
                let state =
                    cx.update(read_state).expect("fullscreen smoke state");
                publish(
                    &directory,
                    "state",
                    &format!("command_sequence={sequence}\n{state}"),
                );
                if let Ok(command) = std::fs::read_to_string(
                    directory.join(format!("command-{sequence}")),
                ) {
                    #[cfg(target_os = "macos")]
                    let native = command.strip_prefix("native\t");
                    #[cfg(target_os = "macos")]
                    let result = if let Some(event) = native {
                        super::input_smoke::post_event(event)
                            .map(|()| "posted".to_owned())
                    } else if command == "probe-display-refit" {
                        cx.update(probe_adapter)
                            .and_then(std::convert::identity)
                            .and_then(|adapter| adapter.probe_display_refit())
                    } else if command == "probe-native-exit" {
                        cx.update(probe_adapter)
                            .and_then(std::convert::identity)
                            .and_then(|adapter| adapter.probe_native_exit())
                    } else {
                        cx.update(|cx| execute(cx, &command))
                            .and_then(std::convert::identity)
                    };
                    #[cfg(not(target_os = "macos"))]
                    let result = cx
                        .update(|cx| execute(cx, &command))
                        .and_then(std::convert::identity);
                    publish(
                        &directory,
                        &format!("result-{sequence}"),
                        &result
                            .unwrap_or_else(|error| format!("error: {error}")),
                    );
                    sequence += 1;
                }
            }
        })
        .detach();
    })
}

#[cfg(target_os = "macos")]
fn probe_adapter(
    cx: &mut App,
) -> anyhow::Result<crate::native_fullscreen::Adapter> {
    let handle = cx.windows().first().copied().context("probe window")?;
    handle.update(cx, |root, _, cx| {
        let view = root
            .downcast::<WorkspaceView>()
            .ok()
            .context("workspace root")?;
        view.read(cx)
            .native_fullscreen
            .clone()
            .context("native fullscreen adapter")
    })?
}

fn publish(directory: &Path, name: &str, text: &str) {
    let temporary = directory.join(format!("{name}.tmp"));
    std::fs::write(&temporary, text).expect("write fullscreen smoke state");
    std::fs::rename(temporary, directory.join(name))
        .expect("publish fullscreen smoke state");
}

fn execute(cx: &mut App, command: &str) -> anyhow::Result<String> {
    let mut fields = command.split_whitespace();
    let index: usize = fields.next().context("window index")?.parse()?;
    let mut outcomes = Vec::new();
    // Multiple commands in one file deliberately share a single GPUI turn.
    for name in fields {
        if matches!(name, "confirm_close" | "cancel_close") {
            let handle = cx
                .windows()
                .get(index)
                .copied()
                .context("confirmation window")?;
            handle.update(cx, |root, window, cx| {
                let view = root
                    .downcast::<WorkspaceView>()
                    .ok()
                    .context("workspace root")?;
                view.update(cx, |view, cx| {
                    let target = view
                        .close
                        .confirmation
                        .context("no close confirmation")?;
                    if name == "confirm_close" {
                        view.finish_close(target, window, cx);
                    } else {
                        view.cancel_close(window, cx);
                    }
                    Ok::<_, anyhow::Error>(())
                })
            })??;
            outcomes.push("confirmation handled".to_owned());
            continue;
        }
        let spec = lookup(name).context("unknown smoke command")?;
        let handle = cx.windows().get(index).copied();
        let outcome = Desktop::invoke(
            cx,
            &CommandInvocation {
                id: spec.id,
                args: Vec::new(),
            },
            handle,
        );
        outcomes.push(format!("{outcome:?}"));
    }
    ensure!(!outcomes.is_empty(), "missing smoke command");
    Ok(outcomes.join(";"))
}

fn bounds(value: WindowBounds) -> String {
    match value {
        WindowBounds::Windowed(value)
        | WindowBounds::Fullscreen(value)
        | WindowBounds::Maximized(value) => rect(value),
    }
}

fn rect(value: Bounds<Pixels>) -> String {
    format!(
        "{},{},{},{}",
        f32::from(value.origin.x),
        f32::from(value.origin.y),
        f32::from(value.size.width),
        f32::from(value.size.height)
    )
}

fn read_state(cx: &mut App) -> String {
    let mut output = format!(
        "windows={}\nreloading={}\n",
        cx.windows().len(),
        cx.global::<Desktop>().reloading
    );
    for (index, handle) in cx.windows().into_iter().enumerate() {
        let _ = handle.update(cx, |root, window, cx| {
            let Ok(root) = root.downcast::<WorkspaceView>() else { return; };
            let view = root.read(cx);
            writeln!(output, "w{index}.default={:?}", view.config.window.macos_fullscreen_mode).unwrap();
            writeln!(output, "w{index}.window_bounds={}", bounds(window.window_bounds())).unwrap();
            writeln!(output, "w{index}.mode={:?}\nw{index}.pending={}\nw{index}.chrome={}\nw{index}.restore={}\nw{index}.viewport={},{}\nw{index}.tabs={}\nw{index}.status={}\nw{index}.confirming={}",
                view.fullscreen.observed, view.fullscreen.is_pending(), view.fullscreen.chrome_hidden,
                bounds(view.fullscreen.restorable_bounds()), f32::from(window.viewport_size().width), f32::from(window.viewport_size().height),
                view.tabs.len(), view.status.as_deref().unwrap_or(""), view.close.confirmation.is_some()).unwrap();
            let insets = view.fullscreen_insets;
            writeln!(output, "w{index}.insets={},{},{},{}", f32::from(insets.top), f32::from(insets.right), f32::from(insets.bottom), f32::from(insets.left)).unwrap();
            writeln!(output, "w{index}.tab_bounds={}", rect(view.tab_strip(window).bounds)).unwrap();
            let mut consistent = true;
            for tab in &view.tabs {
                let terminal = tab.view.read(cx);
                consistent &= terminal.chrome_hidden == view.fullscreen.chrome_hidden
                    && terminal.fullscreen_insets == view.fullscreen_insets;
            }
            writeln!(output, "w{index}.retained={consistent}").unwrap();
            if let Some(terminal) = view.active_view() {
                let terminal = terminal.read(cx);
                let text = terminal.snapshot.as_ref().map(|snapshot| snapshot.cells().map(|cell| cell.text.as_str()).collect::<String>()).unwrap_or_default();
                writeln!(output, "w{index}.ready={}\nw{index}.focused={}\nw{index}.grid={},{}\nw{index}.terminal={}\nw{index}.text={}", text.contains("READY"), terminal.focus.is_focused(window), terminal.last_grid_size.columns, terminal.last_grid_size.rows, rect(terminal.content_bounds(window)), text.replace('\n', " ")).unwrap();
            }
            #[cfg(target_os = "macos")]
            if let Some(adapter) = &view.native_fullscreen {
                match adapter.inspect() {
                    Ok(native) => for line in native.lines() { writeln!(output, "w{index}.{line}").unwrap(); },
                    Err(error) => { writeln!(output, "w{index}.native_error={error}").unwrap(); }
                }
            }
        });
    }
    output
}
