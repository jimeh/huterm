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
    displayed_offset: usize,
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
            displayed_offset: terminal.scroll.displayed(),
            callbacks: view.frame_clock.pending_callbacks(),
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

fn scroll_to(cx: &mut App, offset: usize) -> anyhow::Result<()> {
    workspace(cx, |view, _, cx| {
        view.tabs[0].view.update(cx, |terminal, cx| {
            terminal.scroll.set_desired(offset);
            terminal.start_snapshot_if_needed(cx);
        });
    })
}

#[cfg(target_os = "macos")]
async fn pause(cx: &mut AsyncApp) -> anyhow::Result<()> {
    cx.update(|cx| workspace(cx, |_, window, _| window.minimize_window()))??;
    wait(cx, "native window occluded", |cx| {
        let occluded = workspace(cx, |_, window, _| native::occluded(window))??;
        Ok((occluded, format!("occluded={occluded}")))
    })
    .await?;
    Ok(())
}

async fn check_paused_scroll(
    cx: &mut AsyncApp,
    sequence: u64,
) -> anyhow::Result<()> {
    cx.update(|cx| scroll_to(cx, 1))??;
    wait_state(cx, "one viewport snapshot while frames are paused", |s| {
        s.displayed_offset == 1 && s.snapshots == sequence + 1
    })
    .await?;
    // Title delivery proves the event drain revisited snapshot admission after
    // each scroll request. Neither input nor completion replenishes allowance.
    for (offset, marker) in [(2, "SCROLL_SECOND"), (3, "SCROLL_FINAL")] {
        cx.update(|cx| scroll_to(cx, offset))??;
        cx.update(|cx| send(cx, marker))??;
        wait_state(cx, "title progressed with scroll pending", |s| {
            s.title == marker
        })
        .await?;
        let observed = cx.update(state)??;
        ensure!(
            observed.snapshots == sequence + 1
                && observed.displayed_offset == 1
                && observed.callbacks == 1,
            "paused scroll exceeded its allowance: {observed:?}"
        );
    }
    Ok(())
}

async fn check_animations(cx: &mut AsyncApp) -> anyhow::Result<()> {
    let samples = Rc::new(RefCell::new(Vec::<(f32, Instant)>::new()));
    let observed = Rc::clone(&samples);
    let _subscription = cx.update(|cx| {
        workspace(cx, |view, _, cx| {
            view.tabs[0].view.update(cx, |_, cx| {
                cx.observe_self(move |terminal, _| {
                    let opacity = terminal.resize_visibility.opacity;
                    let mut samples = observed.borrow_mut();
                    if opacity > 0.0
                        && opacity < 1.0
                        && samples.last().is_none_or(|(previous, _)| {
                            (opacity - previous).abs() > f32::EPSILON
                        })
                    {
                        samples.push((opacity, Instant::now()));
                    }
                })
            })
        })
    })??;
    cx.update(|cx| {
        workspace(cx, |view, _, cx| {
            view.tabs[0].view.update(cx, |terminal, cx| {
                terminal.resize_visibility =
                    super::IndicatorVisibility::with_hold(
                        Duration::from_millis(100),
                    );
                terminal.resize_visibility.activate(Instant::now());
                terminal.bell.ring(Instant::now(), true, true);
                cx.notify();
            });
        })
    })??;
    wait(cx, "indicator fades on delivered frames", |cx| {
        workspace(cx, |view, _, cx| {
            let opacity = view.tabs[0].view.read(cx).resize_visibility.opacity;
            Ok((opacity > 0.0 && opacity < 1.0, format!("opacity={opacity}")))
        })?
    })
    .await?;
    wait(cx, "animations settle without recurring callbacks", |cx| {
        workspace(cx, |view, _, cx| {
            let terminal = view.tabs[0].view.read(cx);
            let opacity = terminal.resize_visibility.opacity;
            let flash = terminal.bell.flash_until;
            let callbacks = view.frame_clock.pending_callbacks();
            Ok((
                opacity == 0.0 && flash.is_none() && callbacks == 0,
                format!(
                    "opacity={opacity}, flash={flash:?}, callbacks={callbacks}"
                ),
            ))
        })?
    })
    .await?;
    let samples = samples.borrow();
    ensure!(
        samples.len() >= 2,
        "fade skipped intermediate presentation states: {samples:?}"
    );
    let mut intervals: Vec<_> = samples
        .windows(2)
        .map(|pair| pair[1].1.duration_since(pair[0].1).as_micros())
        .collect();
    intervals.sort_unstable();
    let median = intervals[intervals.len() / 2];
    eprintln!(
        "REFRESH_ANIMATION samples={} median_interval_us={median}",
        samples.len()
    );
    if let Ok(budget) = std::env::var("HUTERM_ANIMATION_FRAME_BUDGET_US") {
        let budget: u128 = budget.parse()?;
        ensure!(
            median <= budget,
            "animation median {median} us exceeds {budget} us"
        );
    }
    eprintln!("REFRESH_SMOKE animation_hold_fade_idle passed");
    Ok(())
}

async fn check_render_resize(cx: &mut AsyncApp) -> anyhow::Result<()> {
    let (original, requests) = cx.update(|cx| {
        workspace(cx, |view, window, cx| {
            let original = window.viewport_size();
            let requests = view.tabs[0].view.read(cx).resize_requests;
            window.resize(gpui::size(
                original.width + gpui::px(1.0),
                original.height,
            ));
            (original, requests)
        })
    })??;
    wait(
        cx,
        "one-pixel resize activates the indicator without resizing the grid",
        |cx| {
            workspace(cx, |view, window, cx| {
                let terminal = view.tabs[0].view.read(cx);
                ensure!(
                    terminal.resize_requests == requests,
                    "fixture crossed a grid boundary"
                );
                let opacity = terminal.resize_visibility.opacity;
                Ok((
                    window.viewport_size().width > original.width
                        && opacity > 0.0,
                    format!(
                        "size={:?}, opacity={opacity}",
                        window.viewport_size()
                    ),
                ))
            })?
        },
    )
    .await?;
    wait(
        cx,
        "render-time resize indicator expires without a terminal event",
        |cx| {
            workspace(cx, |view, _, cx| {
                let terminal = view.tabs[0].view.read(cx);
                ensure!(
                    terminal.resize_requests == requests,
                    "fixture resized the grid"
                );
                let opacity = terminal.resize_visibility.opacity;
                Ok((opacity <= f32::EPSILON, format!("opacity={opacity}")))
            })?
        },
    )
    .await?;
    cx.update(|cx| workspace(cx, |_, window, _| window.resize(original)))??;
    wait(cx, "restored viewport and settled indicator", |cx| {
        workspace(cx, |view, window, cx| {
            let opacity = view.tabs[0].view.read(cx).resize_visibility.opacity;
            Ok((
                window.viewport_size() == original
                    && opacity <= f32::EPSILON
                    && view.frame_clock.pending_callbacks() == 0,
                format!("size={:?}, opacity={opacity}", window.viewport_size()),
            ))
        })?
    })
    .await?;
    eprintln!("REFRESH_SMOKE render_time_resize_deadline passed");
    Ok(())
}

async fn check_paused_animation(cx: &mut AsyncApp) -> anyhow::Result<()> {
    cx.update(|cx| {
        workspace(cx, |view, _, cx| {
            view.tabs[0].view.update(cx, |terminal, cx| {
                terminal.bell.ring(Instant::now(), true, true);
                cx.notify();
            });
        })
    })??;
    wait(cx, "bell deadline expires without display frames", |cx| {
        workspace(cx, |view, _, cx| {
            let flash = view.tabs[0].view.read(cx).bell.flash_until;
            Ok((flash.is_none(), format!("flash={flash:?}")))
        })?
    })
    .await?;
    ensure!(
        cx.update(state)??.callbacks == 1,
        "deadline duplicated the paused frame callback"
    );
    eprintln!("REFRESH_SMOKE animation_deadline_while_paused passed");
    Ok(())
}

async fn check_initial_presentation(cx: &mut AsyncApp) -> anyhow::Result<()> {
    wait_state(cx, "initial snapshot and frame", |s| {
        s.text.contains("READY")
            && s.callbacks == 0
            && s.stage == Stage::Waiting
    })
    .await?;
    check_animations(cx).await?;
    check_render_resize(cx).await
}

async fn check(cx: &mut AsyncApp) -> anyhow::Result<()> {
    check_initial_presentation(cx).await?;
    #[cfg(target_os = "macos")]
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
    check_paused_scroll(cx, sequence).await?;
    check_paused_animation(cx).await?;
    cx.update(show)??;
    wait_state(cx, "resume catches up without new producer activity", |s| {
        s.text.contains("SCROLL_FINAL")
            && s.displayed_offset == 3
            && s.callbacks == 0
    })
    .await?;
    eprintln!("REFRESH_SMOKE frame_stop_resume passed");
    eprintln!("REFRESH_SMOKE bounded_scroll_while_paused passed");

    #[cfg(target_os = "macos")]
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
