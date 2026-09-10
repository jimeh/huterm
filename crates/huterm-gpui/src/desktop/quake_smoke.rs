//! File-driven observation of the real quake registry and command dispatcher.
use super::*;
use anyhow::Context as _;
use huterm_protocol::{
    CommandArgument, CommandInvocation, CommandValue, lookup,
};
use std::{fmt::Write as _, path::Path};

pub(crate) fn run() -> anyhow::Result<()> {
    let directory = PathBuf::from(std::env::var("HUTERM_QUAKE_SMOKE")?);
    if std::env::var_os("HUTERM_QUAKE_HIDDEN_PROBE").is_some() {
        run_hidden_probe(directory);
        return Ok(());
    }
    run_with_startup(move |cx| {
        cx.spawn(async move |cx| {
            let mut sequence = 0;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(10))
                    .await;
                let Ok(state) = cx.update(read_state) else {
                    break;
                };
                publish(
                    &directory,
                    "state",
                    &format!("command_sequence={sequence}\n{state}"),
                );
                if let Ok(command) = std::fs::read_to_string(
                    directory.join(format!("command-{sequence}")),
                ) {
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
fn publish(directory: &Path, name: &str, text: &str) {
    let temporary = directory.join(format!("{name}.tmp"));
    std::fs::write(&temporary, text).expect("write quake smoke observation");
    std::fs::rename(temporary, directory.join(name))
        .expect("publish quake smoke observation");
}
fn execute(cx: &mut App, command: &str) -> anyhow::Result<String> {
    let fields: Vec<_> = command.split_whitespace().collect();
    let target = *fields.first().context("command target")?;
    let name = *fields.get(1).context("command name")?;
    let handle = cx.windows().into_iter().find(|handle| {
        handle
            .update(cx, |root, window, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.read(cx)
                        .quake
                        .as_ref()
                        .map_or("ordinary", |state| state.name.as_str())
                        == target
                        || format!(
                            "id:{:?}",
                            window.window_handle().window_id()
                        ) == target
                })
            })
            .unwrap_or(false)
    });
    #[cfg(target_os = "macos")]
    if name == "native_space" {
        let native = handle
            .context("native Space host")?
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().ok().and_then(|view| {
                    view.read(cx)
                        .quake
                        .as_ref()
                        .map(|state| state.native.clone())
                })
            })?
            .context("quake native host")?;
        cx.foreground_executor()
            .spawn(async move {
                native.toggle_native_for_smoke();
            })
            .detach();
        return Ok("native Space transition queued".into());
    }
    if matches!(name, "confirm_close" | "cancel_close") {
        handle.context("confirmation host")?.update(
            cx,
            |root, window, cx| {
                let view =
                    root.downcast::<WorkspaceView>().ok().context("root")?;
                view.update(cx, |view, cx| {
                    let target =
                        view.close.confirmation.context("no confirmation")?;
                    if name == "confirm_close" {
                        view.finish_close(target, window, cx);
                    } else {
                        view.cancel_close(window, cx);
                    }
                    Ok::<_, anyhow::Error>(())
                })
            },
        )??;
        return Ok("confirmed".into());
    }
    let spec = lookup(name).context("catalog command")?;
    let args = fields
        .get(2)
        .map(|profile| {
            vec![CommandArgument::new(
                "profile",
                CommandValue::Text((*profile).into()),
            )]
        })
        .unwrap_or_default();
    Ok(format!(
        "{:?}",
        Desktop::invoke(cx, &CommandInvocation::new(spec.id, args), handle)
    ))
}
fn read_state(cx: &mut App) -> String {
    let mut output = format!(
        "windows={}\nreloading={}\nkeepalive={}\nconfig_error={}\n",
        cx.windows().len(),
        cx.global::<Desktop>().reloading,
        quake_windows::keep_alive(cx),
        cx.global::<Desktop>().config_error.as_deref().unwrap_or("")
    );
    for (index, handle) in cx.windows().into_iter().enumerate() {
        let _=handle.update(cx,|root,window,cx| {
            let Ok(root)=root.downcast::<WorkspaceView>() else {return;};
            let view=root.read(cx);
            let profile=view.quake.as_ref().map_or("ordinary",|state|state.name.as_str());
            writeln!(output,"w{index}.window_id={:?}\nw{index}.profile={profile}\nw{index}.tabs={}\nw{index}.busy={}\nw{index}.confirming={}\nw{index}.chrome={}\nw{index}.status={}",window.window_handle().window_id(),view.tabs.len(),view.busy,view.close.confirmation.is_some(),view.chrome_hidden(),view.status.as_deref().unwrap_or("")).unwrap();
            let viewport = window.viewport_size();
            writeln!(output, "w{index}.gpui_viewport={},{}\nw{index}.gpui_scale={}", f32::from(viewport.width), f32::from(viewport.height), window.scale_factor()).unwrap();
            if let Some(state)=&view.quake {
                match quake_windows::inspect(state) {
                    Ok(native)=>for line in native.lines() {writeln!(output,"w{index}.{line}").unwrap();},
                    Err(error)=>writeln!(output,"w{index}.native_error={error}").unwrap(),
                }
            }
            if let Some(terminal)=view.active_view() {
                let terminal=terminal.read(cx);
                let bounds = terminal.content_bounds(window);
                writeln!(output, "w{index}.tab_presentation={:?}\nw{index}.tab_reveal={}\nw{index}.terminal_top={}\nw{index}.safe_top={}", terminal.tab_presentation, view.reveal.progress, f32::from(bounds.origin.y), f32::from(view.fullscreen_insets.top)).unwrap();
                let text=terminal.snapshot.as_ref().map(|snapshot|snapshot.cells().map(|cell|cell.text.as_str()).collect::<String>()).unwrap_or_default();
                writeln!(output,"w{index}.text={}\nw{index}.grid={},{}\nw{index}.terminal_visible={}\nw{index}.focused={}",text.replace('\n'," ").trim(),terminal.last_grid_size.columns,terminal.last_grid_size.rows,terminal.visible,terminal.focus.is_focused(window)).unwrap();
            }
        });
    }
    output
}

struct HiddenProbe;
impl gpui::Render for HiddenProbe {
    fn render(
        &mut self,
        _: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> impl gpui::IntoElement {
        gpui::div()
    }
}
fn run_hidden_probe(directory: PathBuf) {
    gpui::Application::new().run(move |cx| {
        let grab = std::env::var_os("HUTERM_QUAKE_GRAB_PROBE").map(|_| {
            let manager = global_hotkey::GlobalHotKeyManager::new()
                .expect("native external grab manager");
            manager
                .register(global_hotkey::hotkey::HotKey::new(
                    Some(
                        global_hotkey::hotkey::Modifiers::CONTROL
                            | global_hotkey::hotkey::Modifiers::ALT,
                    ),
                    global_hotkey::hotkey::Code::KeyL,
                ))
                .expect("external control-alt-L grab");
            manager
        });
        let handle = cx
            .open_window(
                gpui::WindowOptions {
                    show: false,
                    focus: false,
                    ..gpui::WindowOptions::default()
                },
                |_, cx| cx.new(|_| HiddenProbe),
            )
            .expect("create hidden native probe");
        let visible = handle
            .update(cx, |_, window, cx| {
                crate::quake::native::Platform::new(cx)
                    .and_then(|platform| platform.window(window))
                    .and_then(|native| native.visible())
            })
            .expect("read native window")
            .expect("native visibility");
        publish(&directory, "ready", &format!("visible={visible}"));
        cx.spawn(async move |cx| {
            for _ in 0..1000 {
                cx.background_executor()
                    .timer(Duration::from_millis(10))
                    .await;
                if directory.join("finish").exists() {
                    break;
                }
            }
            drop(grab);
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
