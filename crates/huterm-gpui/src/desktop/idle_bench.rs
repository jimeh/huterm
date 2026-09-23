//! Startup-only fixture. Its task ends before the process counters are sampled.
use std::time::{Duration, Instant};

use anyhow::{Context as _, ensure};
use gpui::App;
use huterm_protocol::{
    CommandArgument, CommandInvocation, CommandValue, lookup,
};

use super::{Desktop, WorkspaceView, quake_windows};

pub(crate) fn run() -> anyhow::Result<()> {
    let tabs: usize = std::env::var("HUTERM_IDLE_TABS")?.parse()?;
    let windows: usize = std::env::var("HUTERM_IDLE_WINDOWS")?.parse()?;
    ensure!((1..=50).contains(&tabs), "expected 1..=50 tabs per window");
    ensure!((1..=2).contains(&windows), "expected one or two windows");
    let quake = std::env::var("HUTERM_IDLE_PRESENTATION").ok().as_deref()
        == Some("quake");
    let hidden = std::env::var("HUTERM_IDLE_VISIBILITY").ok().as_deref()
        == Some("hidden");
    super::run_with_startup(move |cx| {
        cx.spawn(async move |cx| {
            let deadline = Instant::now() + Duration::from_secs(90);
            loop {
                let result = cx.update(|cx| advance(cx, tabs, windows, quake, hidden));
                match result.and_then(std::convert::identity) {
                    Ok(Some(adapters)) => {
                        // No persistent observer, state writer or control loop survives
                        // this acknowledgement. The native runner samples externally.
                        println!("huterm-idle ready windows={windows} tabs_per_window={tabs} adapters={adapters}");
                        break;
                    }
                    Ok(None) if Instant::now() < deadline => {}
                    result => {
                        eprintln!("huterm-idle startup failed: {result:?}");
                        let _ = cx.update(|cx| cx.quit());
                        break;
                    }
                }
                // Startup readiness polling is bounded and ends before sampling.
                cx.background_executor().timer(Duration::from_millis(10)).await;
            }
        }).detach();
    })
}

fn advance(
    cx: &mut App,
    tabs: usize,
    windows: usize,
    quake: bool,
    hidden: bool,
) -> anyhow::Result<Option<usize>> {
    if cx.global::<Desktop>().pending_spawns != 0 {
        return Ok(None);
    }
    let handles = cx.windows();
    if quake && !prepare_quake(cx, &handles)? {
        return Ok(None);
    }
    let forced = std::env::var_os("HUTERM_FULLSCREEN_SMOKE").is_some()
        && std::env::var_os("HUTERM_FULLSCREEN_NO_ADAPTER").is_some();
    let mut adapters = 0;
    for handle in &handles {
        let ready = handle.update(cx, |root, _, cx| {
            let root = root
                .downcast::<WorkspaceView>()
                .ok()
                .context("workspace root")?;
            let view = root.read(cx);
            ensure!(
                view.quake.is_some() == quake,
                "unexpected idle presentation"
            );
            ensure!(
                view.native_fullscreen.is_some() != forced
                    && view.fullscreen_work.fallback == forced,
                "unexpected native fullscreen adapter or fallback state"
            );
            adapters += usize::from(view.native_fullscreen.is_some());
            ensure!(view.status.is_none(), "startup status: {:?}", view.status);
            let ready = view.active_view().is_some_and(|terminal| {
                terminal.read(cx).snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot
                        .cells()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>()
                        .contains("HUTERM_IDLE_READY")
                })
            });
            if let Some(state) = &view.quake {
                let observation = quake_windows::inspect(state)?;
                if !observation.lines().any(|line| line == "stage=Idle") {
                    return Ok((false, view.tabs.len(), None));
                }
                let desired =
                    observation.lines().any(|line| line == "desired=true");
                if ready
                    && !view.busy
                    && view.tabs.len() == tabs
                    && desired == hidden
                {
                    return Ok((
                        true,
                        view.tabs.len(),
                        Some(state.name.clone()),
                    ));
                }
                ensure!(
                    state.native.visible()? == desired,
                    "native Quake visibility disagrees"
                );
            }
            Ok::<_, anyhow::Error>((ready && !view.busy, view.tabs.len(), None))
        })??;
        if let Some(profile) = ready.2 {
            invoke_profile(cx, "hide_quake", &profile)?;
            return Ok(None);
        }
        if !ready.0 {
            return Ok(None);
        }
        if ready.1 < tabs {
            invoke(cx, "new_tab", Some(*handle))?;
            return Ok(None);
        }
        ensure!(ready.1 == tabs, "unexpected tab count");
    }
    if handles.len() < windows {
        if quake {
            invoke_profile(
                cx,
                "show_quake",
                &format!("idle{}", handles.len()),
            )?;
        } else {
            invoke(cx, "new_window", None)?;
        }
        return Ok(None);
    }
    ensure!(handles.len() == windows, "unexpected window count");
    report_windows(cx, &handles)?;
    Ok(Some(adapters))
}

fn report_windows(
    cx: &mut App,
    handles: &[gpui::AnyWindowHandle],
) -> anyhow::Result<()> {
    for (index, handle) in handles.iter().enumerate() {
        handle.update(cx, |root, window, cx| {
            if let Ok(root) = root.downcast::<WorkspaceView>()
                && let Some(state) = &root.read(cx).quake
            {
                println!(
                    "huterm-idle profile={} {}",
                    state.name,
                    quake_windows::inspect(state)
                        .unwrap_or_default()
                        .replace('\n', " ")
                );
            }
            println!(
                "huterm-idle window={index} scale={} bounds={:?}",
                window.scale_factor(),
                window.window_bounds()
            );
        })?;
    }
    Ok(())
}

fn prepare_quake(
    cx: &mut App,
    handles: &[gpui::AnyWindowHandle],
) -> anyhow::Result<bool> {
    let has_quake = handles.iter().any(|handle| {
        handle
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>()
                    .is_ok_and(|view| view.read(cx).quake.is_some())
            })
            .unwrap_or(false)
    });
    if !has_quake {
        invoke_profile(cx, "show_quake", "idle0")?;
        return Ok(false);
    }
    for handle in handles {
        let ordinary = handle.update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>()
                .is_ok_and(|view| view.read(cx).quake.is_none())
        })?;
        if ordinary {
            invoke(cx, "close_window", Some(*handle))?;
            return Ok(false);
        }
    }
    Ok(true)
}

fn invoke(
    cx: &mut App,
    name: &str,
    window: Option<gpui::AnyWindowHandle>,
) -> anyhow::Result<()> {
    let spec = lookup(name).context("benchmark command")?;
    Desktop::invoke(
        cx,
        &CommandInvocation {
            id: spec.id,
            args: Vec::new(),
        },
        window,
    )?;
    Ok(())
}

fn invoke_profile(
    cx: &mut App,
    name: &str,
    profile: &str,
) -> anyhow::Result<()> {
    let spec = lookup(name).context("benchmark Quake command")?;
    Desktop::invoke(
        cx,
        &CommandInvocation::new(
            spec.id,
            vec![CommandArgument::new(
                "profile",
                CommandValue::Text(profile.into()),
            )],
        ),
        None,
    )?;
    Ok(())
}
