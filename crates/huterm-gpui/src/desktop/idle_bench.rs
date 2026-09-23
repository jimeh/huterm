//! Startup-only fixture. Its task ends before the process counters are sampled.
use std::time::{Duration, Instant};

use anyhow::{Context as _, ensure};
use gpui::App;
use huterm_protocol::{CommandInvocation, lookup};

use super::{Desktop, WorkspaceView};

pub(crate) fn run() -> anyhow::Result<()> {
    let tabs: usize = std::env::var("HUTERM_IDLE_TABS")?.parse()?;
    let windows: usize = std::env::var("HUTERM_IDLE_WINDOWS")?.parse()?;
    ensure!((1..=50).contains(&tabs), "expected 1..=50 tabs per window");
    ensure!((1..=2).contains(&windows), "expected one or two windows");
    super::run_with_startup(move |cx| {
        cx.spawn(async move |cx| {
            let deadline = Instant::now() + Duration::from_secs(90);
            loop {
                let result = cx.update(|cx| advance(cx, tabs, windows));
                match result.and_then(std::convert::identity) {
                    Ok(true) => {
                        // No persistent observer, state writer or control loop survives
                        // this acknowledgement. The native runner samples externally.
                        println!("huterm-idle ready windows={windows} tabs_per_window={tabs} adapters={windows}");
                        break;
                    }
                    Ok(false) if Instant::now() < deadline => {}
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

fn advance(cx: &mut App, tabs: usize, windows: usize) -> anyhow::Result<bool> {
    if cx.global::<Desktop>().pending_spawns != 0 {
        return Ok(false);
    }
    let handles = cx.windows();
    for handle in &handles {
        let ready = handle.update(cx, |root, _, cx| {
            let root = root
                .downcast::<WorkspaceView>()
                .ok()
                .context("workspace root")?;
            let view = root.read(cx);
            ensure!(
                view.quake.is_none(),
                "idle benchmark cannot include Quake"
            );
            ensure!(
                view.native_fullscreen.is_some(),
                "native fullscreen adapter missing"
            );
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
            Ok::<_, anyhow::Error>((ready && !view.busy, view.tabs.len()))
        })??;
        if !ready.0 {
            return Ok(false);
        }
        if ready.1 < tabs {
            invoke(cx, "new_tab", Some(*handle))?;
            return Ok(false);
        }
        ensure!(ready.1 == tabs, "unexpected tab count");
    }
    if handles.len() < windows {
        invoke(cx, "new_window", None)?;
        return Ok(false);
    }
    ensure!(handles.len() == windows, "unexpected window count");
    for (index, handle) in handles.iter().enumerate() {
        handle.update(cx, |_, window, _| {
            println!(
                "huterm-idle window={index} scale={} bounds={:?}",
                window.scale_factor(),
                window.window_bounds()
            );
        })?;
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
