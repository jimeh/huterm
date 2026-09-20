//! Native frame delivery and silent attachment cancellation regression probe.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, ensure};
use gpui::{App, AsyncApp, Global};
use huterm_protocol::{TabId, TerminalInput};

use super::WorkspaceView;

#[cfg(target_os = "macos")]
#[path = "refresh_smoke/native.rs"]
mod native;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Running,
    Waiting,
    Dropped,
}

#[derive(Default)]
struct Probes(RefCell<HashMap<TabId, Rc<Cell<Stage>>>>);
impl Global for Probes {}

/// Created only by this smoke. Lives inside the same future as its client clone.
pub(super) struct ActivityProbe(Rc<Cell<Stage>>);
impl ActivityProbe {
    pub(super) fn new(id: TabId, cx: &App) -> Option<Self> {
        let probes = cx.try_global::<Probes>()?;
        let stage = Rc::new(Cell::new(Stage::Running));
        probes.0.borrow_mut().insert(id, Rc::clone(&stage));
        Some(Self(stage))
    }

    pub(super) fn waiting(&self, waiting: bool) {
        self.0.set(if waiting {
            Stage::Waiting
        } else {
            Stage::Running
        });
    }
}
impl Drop for ActivityProbe {
    fn drop(&mut self) {
        self.0.set(Stage::Dropped);
    }
}

pub(crate) fn run() -> anyhow::Result<()> {
    ensure!(cfg!(target_os = "macos"), "refresh smoke requires macOS");
    super::run_with_startup(|cx| {
        cx.set_global(Probes::default());
        cx.spawn(async move |cx| {
            match check(cx).await {
                Ok(()) => eprintln!("REFRESH_SMOKE passed"),
                Err(error) => eprintln!("REFRESH_SMOKE failed: {error:#}"),
            }
            // Use normal application cleanup, including the retained runtime.
            cx.update(|cx| cx.quit()).expect("quit refresh smoke");
        })
        .detach();
    })
}

fn workspace<R>(
    cx: &mut App,
    read: impl FnOnce(
        &mut WorkspaceView,
        &mut gpui::Window,
        &mut gpui::Context<'_, WorkspaceView>,
    ) -> R,
) -> anyhow::Result<R> {
    let handle = cx.windows().first().copied().context("no smoke window")?;
    handle.update(cx, |root, window, cx| {
        let view = root
            .downcast::<WorkspaceView>()
            .map_err(|_| anyhow::anyhow!("not a workspace"))?;
        Ok(view.update(cx, |view, cx| read(view, window, cx)))
    })?
}

#[derive(Debug)]
struct State {
    title: String,
    text: String,
    snapshots: u64,
    callbacks: usize,
    stage: Stage,
}
fn state(cx: &mut App) -> anyhow::Result<State> {
    workspace(cx, |view, _, cx| {
        let tab = view.tabs.first().context("no smoke tab")?;
        let terminal = tab.view.read(cx);
        let text =
            terminal
                .snapshot
                .as_ref()
                .map_or_else(String::new, |snapshot| {
                    snapshot.cells().map(|cell| cell.text.as_str()).collect()
                });
        Ok(State {
            title: terminal.title.clone(),
            text: text.trim_end().to_owned(),
            snapshots: terminal.snapshot_sequence,
            // Each deferred registration / native callback owns exactly one weak clock.
            // Count those actual closures, not calls to a test frame dispatcher.
            callbacks: Rc::weak_count(&view.frame_clock),
            stage: cx.global::<Probes>().0.borrow()[&tab.id].get(),
        })
    })?
}

async fn wait(
    cx: &mut AsyncApp,
    label: &str,
    mut condition: impl FnMut(&mut App) -> anyhow::Result<(bool, String)>,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (done, observed) = cx.update(&mut condition)??;
        if done {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "{label}: last observed {observed}"
        );
        // Native frame and PTY delivery are external boundaries. Poll observable
        // progress under a deadline; elapsed time never counts as success.
        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    }
}
async fn wait_state(
    cx: &mut AsyncApp,
    label: &str,
    condition: impl Fn(&State) -> bool,
) -> anyhow::Result<()> {
    wait(cx, label, |cx| match state(cx) {
        Ok(state) => Ok((condition(&state), format!("{state:?}"))),
        Err(error) => Ok((false, error.to_string())),
    })
    .await
}
fn send(cx: &mut App, label: &str) -> anyhow::Result<()> {
    workspace(cx, |view, _, cx| {
        view.tabs[0]
            .view
            .read(cx)
            .client
            .send_input(TerminalInput::Text(format!("{label}\n")))
    })??;
    Ok(())
}
fn show(cx: &mut App) -> anyhow::Result<()> {
    workspace(cx, |_, window, cx| {
        window.activate_window();
        cx.activate(true);
    })
}

async fn pause(cx: &mut AsyncApp) -> anyhow::Result<()> {
    cx.update(|cx| workspace(cx, |_, window, _| window.minimize_window()))??;
    #[cfg(target_os = "macos")]
    wait(cx, "native window occluded", |cx| {
        let occluded = workspace(cx, |_, window, _| native::occluded(window))??;
        Ok((occluded, format!("occluded={occluded}")))
    })
    .await?;
    Ok(())
}

async fn check(cx: &mut AsyncApp) -> anyhow::Result<()> {
    wait_state(cx, "initial snapshot and frame", |s| {
        s.text.contains("READY")
            && s.callbacks == 0
            && s.stage == Stage::Waiting
    })
    .await?;
    pause(cx).await?;
    cx.update(|cx| send(cx, "PAUSED_FIRST"))??;
    wait_state(cx, "pending native callback while occluded", |s| {
        s.text.contains("PAUSED_FIRST")
            && s.title == "PAUSED_FIRST"
            && s.callbacks == 1
            && s.stage == Stage::Waiting
    })
    .await?;
    let sequence = cx.update(state)??.snapshots;
    // Title delivery is an ordering witness that the producer and event drain
    // ran while snapshots were blocked. No sleep stands in for that witness.
    for marker in ["PAUSED_SECOND", "PAUSED_FINAL"] {
        cx.update(|cx| send(cx, marker))??;
        wait_state(cx, "title progressed without a frame", |s| {
            s.title == marker
        })
        .await?;
        let observed = cx.update(state)??;
        ensure!(
            observed.snapshots == sequence && observed.callbacks == 1,
            "paused frames admitted a snapshot or duplicate callback: {observed:?}"
        );
    }
    cx.update(show)??;
    wait_state(cx, "resume catches up without new producer activity", |s| {
        s.text.contains("PAUSED_FINAL") && s.callbacks == 0
    })
    .await?;
    eprintln!("REFRESH_SMOKE frame_stop_resume passed");

    pause(cx).await?;
    cx.update(|cx| send(cx, "PENDING_DROP"))??;
    wait_state(
        cx,
        "silent waiter and pending callback before detach",
        |s| {
            s.text.contains("PENDING_DROP")
                && s.title == "PENDING_DROP"
                && s.callbacks == 1
                && s.stage == Stage::Waiting
        },
    )
    .await?;
    let (id, weak, retained_client) = cx.update(|cx| {
        workspace(cx, |view, _, cx| {
            let tab = &view.tabs[0];
            let id = tab.id;
            let weak = tab.view.downgrade();
            let client = tab.view.read(cx).client.clone();
            // Drop the production TabView, including its owned activity task. Keep
            // the core tab alive, so shutdown or a later event cannot rescue a leak.
            super::remove_tab(&mut view.tabs, &mut view.active, id, |tab| {
                tab.id
            });
            (id, weak, client)
        })
    })??;
    wait(cx, "idle task canceled without a runtime wake", |cx| {
        let stage = cx.global::<Probes>().0.borrow()[&id].get();
        Ok((
            stage == Stage::Dropped,
            format!("stage={stage:?}, view_alive={}", weak.upgrade().is_some()),
        ))
    })
    .await?;
    // A successful owner-thread request proves detaching did not shut core down.
    retained_client.request_snapshot()?.recv().await?;
    eprintln!("REFRESH_SMOKE silent_task_cancellation passed");

    cx.update(|cx| workspace(cx, WorkspaceView::new_tab))???;
    wait_state(cx, "replacement activity while frames paused", |s| {
        s.title == "READY"
    })
    .await?;
    ensure!(
        cx.update(state)??.callbacks == 1,
        "replacement registered a duplicate callback"
    );
    cx.update(show)??;
    wait_state(cx, "stale callback releases after replacement", |s| {
        s.callbacks == 0 && s.title == "READY" && s.text.contains("READY")
    })
    .await?;
    // The old rendered scene may keep the entity alive until a native repaint.
    wait(cx, "detached view released after repaint", |_| {
        let alive = weak.upgrade().is_some();
        Ok((!alive, format!("view_alive={alive}")))
    })
    .await?;
    eprintln!("REFRESH_SMOKE stale_callback_after_detach passed");
    Ok(())
}
