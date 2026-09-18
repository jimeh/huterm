//! File-driven observation of the real quake registry and command dispatcher.
use super::*;
use anyhow::Context as _;
use huterm_protocol::{
    CommandArgument, CommandInvocation, CommandValue, lookup,
};
use std::{fmt::Write as _, fs::OpenOptions, io::Write as _, path::Path};

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
                let Ok((state, observations)) = cx.update(|cx| {
                    let state = read_state(cx);
                    let observations =
                        quake_windows::drain_smoke_observations(cx);
                    (state, observations)
                }) else {
                    break;
                };
                append_trace(&directory, &observations);
                publish(
                    &directory,
                    "state",
                    &format!("command_sequence={sequence}\n{state}"),
                );
                if let Ok(command) = std::fs::read_to_string(
                    directory.join(format!("command-{sequence}")),
                ) {
                    let result = execute(&command, cx).await;
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

fn append_trace(
    directory: &Path,
    observations: &[quake_windows::SmokeObservation],
) {
    if observations.is_empty() {
        return;
    }
    let mut batch = String::new();
    for observation in observations {
        writeln!(
            batch,
            "{{\"monotonic_us\":{},\"profile\":\"{}\",\"generation\":{},\"desired\":{},\"progress\":{},\"stage\":\"{}\",\"frame\":[{},{},{},{}],\"target_frame\":[{},{},{},{}],\"display_frame\":[{},{},{},{}],\"opacity\":{},\"scheduler_gap_us\":{}}}",
            observation.monotonic_us,
            json_string(&observation.profile),
            observation.generation,
            observation.desired,
            observation.progress,
            observation.stage,
            observation.frame.x,
            observation.frame.y,
            observation.frame.width,
            observation.frame.height,
            observation.target_frame.x,
            observation.target_frame.y,
            observation.target_frame.width,
            observation.target_frame.height,
            observation.display_frame.x,
            observation.display_frame.y,
            observation.display_frame.width,
            observation.display_frame.height,
            observation.opacity,
            observation.scheduler_gap_us,
        )
        .expect("format quake smoke trace");
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join("trace.jsonl"))
        .expect("open quake smoke trace");
    file.write_all(batch.as_bytes())
        .expect("append quake smoke trace");
}

fn json_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch.is_control() => {
                write!(escaped, "\\u{:04x}", u32::from(ch))
                    .expect("write JSON escape");
            }
            ch => escaped.push(ch),
        }
    }
    escaped
}
fn publish(directory: &Path, name: &str, text: &str) {
    let temporary = directory.join(format!("{name}.tmp"));
    std::fs::write(&temporary, text).expect("write quake smoke observation");
    std::fs::rename(temporary, directory.join(name))
        .expect("publish quake smoke observation");
}
async fn execute(
    command: &str,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<String> {
    if command.split_whitespace().nth(1) == Some("report_dead") {
        return report_dead_for_smoke(cx).await;
    }
    cx.update(|cx| execute_ui(cx, command))?
}

fn execute_ui(cx: &mut App, command: &str) -> anyhow::Result<String> {
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
    if name == "activate_window" {
        handle
            .context("activation host")?
            .update(cx, |_, window, cx| {
                window.activate_window();
                cx.activate(true);
            })?;
        return Ok("activated".into());
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

async fn report_dead_for_smoke(
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<String> {
    let (reporter, fallback) = cx.update(setup_dead_reporter)??;
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !cx.update(|cx| cx.active_window() == Some(fallback))? {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "fallback window {:?} did not become active",
            fallback.window_id()
        );
        cx.background_executor()
            .timer(std::time::Duration::from_millis(10))
            .await;
    }
    cx.update(move |cx| {
        quake_windows::report(cx, "smoke dead reporter", Some(reporter));
    })?;
    Ok(format!("fallback={:?}", fallback.window_id()))
}

fn setup_dead_reporter(
    cx: &mut App,
) -> anyhow::Result<(WeakEntity<WorkspaceView>, AnyWindowHandle)> {
    let existing = cx.windows();
    let fallback = existing
        .first()
        .copied()
        .context("dead reporter fallback window")?;
    open_window_inner(cx, false);
    let temporary = cx
        .windows()
        .into_iter()
        .find(|candidate| !existing.contains(candidate))
        .context("temporary dead reporter window")?;
    let reporter = temporary.update(cx, |root, window, _| {
        let reporter = root
            .downcast::<WorkspaceView>()
            .map_err(|_| anyhow::anyhow!("temporary reporter root"))?
            .downgrade();
        window.remove_window();
        Ok::<_, anyhow::Error>(reporter)
    })??;
    fallback.update(cx, |_, window, cx| {
        window.activate_window();
        cx.activate(true);
    })?;
    Ok((reporter, fallback))
}

fn read_state(cx: &mut App) -> String {
    let mut output = format!(
        "windows={}\nreloading={}\nkeepalive={}\nconfig_error={}\n",
        cx.windows().len(),
        cx.global::<Desktop>().reloading,
        quake_windows::keep_alive(cx),
        cx.global::<Desktop>().config_error.as_deref().unwrap_or("")
    );
    match quake_windows::inspect_return_focus(cx) {
        Ok(focus) => writeln!(output, "{focus}").unwrap(),
        Err(error) => writeln!(
            output,
            "focus_observation_error={}",
            error.to_string().replace('\n', " ")
        )
        .unwrap(),
    }
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
                writeln!(output, "w{index}.tab_presentation={:?}\nw{index}.tab_reveal={}\nw{index}.tab_height={}\nw{index}.terminal_top={}\nw{index}.safe_top={}", terminal.tab_presentation, view.reveal.progress, f32::from(view.tab_strip(window).bounds.size.height), f32::from(bounds.origin.y), f32::from(view.fullscreen_insets.top)).unwrap();
                let text=terminal.snapshot.as_ref().map(|snapshot|snapshot.cells().map(|cell|cell.text.as_str()).collect::<String>()).unwrap_or_default();
                writeln!(output,"w{index}.text={}\nw{index}.grid={},{}\nw{index}.resize_requests={}\nw{index}.terminal_visible={}\nw{index}.focused={}",text.replace('\n'," ").trim(),terminal.last_grid_size.columns,terminal.last_grid_size.rows,terminal.resize_requests,terminal.visible,terminal.focus.is_focused(window)).unwrap();
            }
        });
    }
    output
}

struct HiddenProbe;

/// An external process's exclusive global grab, kept alive with its manager.
struct ExternalGrab {
    chord: &'static str,
    /// A later candidate that registered and was released again, for steps
    /// that need a chord Huterm itself can own.
    free: Option<&'static str>,
    _manager: global_hotkey::GlobalHotKeyManager,
}

/// Candidate chords in Huterm keybinding syntax, least likely to be bound on
/// a developer machine first. Only registration matters: the smoke never
/// presses the chord, it only proves Huterm rejects a binding another process
/// already owns.
const GRAB_CANDIDATES: &[(&str, global_hotkey::hotkey::Code)] = &[
    ("ctrl-alt-shift-f20", global_hotkey::hotkey::Code::F20),
    ("ctrl-alt-shift-f19", global_hotkey::hotkey::Code::F19),
    ("ctrl-alt-shift-f18", global_hotkey::hotkey::Code::F18),
    ("ctrl-alt-shift-f17", global_hotkey::hotkey::Code::F17),
    ("ctrl-alt-shift-f16", global_hotkey::hotkey::Code::F16),
    // Xvfb's default keymap has no keycodes for the high function keys, so
    // X11 needs letter and digit fallbacks, and the probe needs two of them.
    ("ctrl-alt-shift-l", global_hotkey::hotkey::Code::KeyL),
    ("ctrl-alt-shift-k", global_hotkey::hotkey::Code::KeyK),
    ("ctrl-alt-shift-j", global_hotkey::hotkey::Code::KeyJ),
    ("ctrl-alt-shift-9", global_hotkey::hotkey::Code::Digit9),
    ("ctrl-alt-shift-8", global_hotkey::hotkey::Code::Digit8),
];

fn grab_free_chord() -> anyhow::Result<ExternalGrab> {
    let manager = global_hotkey::GlobalHotKeyManager::new()
        .map_err(|error| anyhow::anyhow!("native grab manager: {error}"))?;
    let modifiers = global_hotkey::hotkey::Modifiers::CONTROL
        | global_hotkey::hotkey::Modifiers::ALT
        | global_hotkey::hotkey::Modifiers::SHIFT;
    let hotkey = |code: &global_hotkey::hotkey::Code| {
        global_hotkey::hotkey::HotKey::new(Some(modifiers), *code)
    };
    let mut errors = Vec::new();
    let mut candidates = GRAB_CANDIDATES.iter();
    let owned = candidates.find_map(|(chord, code)| {
        match manager.register(hotkey(code)) {
            Ok(()) => Some(*chord),
            Err(error) => {
                errors.push(format!("{chord}: {error}"));
                None
            }
        }
    });
    let Some(chord) = owned else {
        anyhow::bail!(
            "every external grab candidate is already owned: {}",
            errors.join("; ")
        );
    };
    let free = candidates.find_map(|(chord, code)| {
        manager.register(hotkey(code)).ok()?;
        manager.unregister(hotkey(code)).ok()?;
        Some(*chord)
    });
    Ok(ExternalGrab {
        chord,
        free,
        _manager: manager,
    })
}
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
    crate::assets::application().run(move |cx| {
        // A registration failure must not panic here: GPUI's launch callback
        // cannot unwind, so a panic aborts and leaves a crash dialog behind.
        let grab = if std::env::var_os("HUTERM_QUAKE_GRAB_PROBE").is_some() {
            match grab_free_chord() {
                Ok(grab) => Some(grab),
                Err(error) => {
                    publish(&directory, "failed", &format!("{error:#}"));
                    cx.quit();
                    return;
                }
            }
        } else {
            None
        };
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
        let chord = grab.as_ref().map_or(String::new(), |grab| {
            format!(
                "\nchord={}\nfree={}",
                grab.chord,
                grab.free.unwrap_or_default()
            )
        });
        publish(&directory, "ready", &format!("visible={visible}{chord}"));
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
