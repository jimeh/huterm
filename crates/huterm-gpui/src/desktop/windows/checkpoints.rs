//! Crash checkpoints: the desktop marks saved state dirty on a committed
//! hierarchy change, a window-model change to a saved fact, and a terminal
//! directory change, and submits one capture after
//! [`CHECKPOINT_DELAY`](super::persistence::CHECKPOINT_DELAY) without
//! further changes. One timer runs only while a change waits, so an idle
//! desktop schedules nothing. A capture due while a cross-window transfer is
//! reserved waits until the transfer settles, and so does one that a
//! transfer reserved under while it ran. A session switch defers captures
//! the same way from its first phase to its last.

use super::persistence::{CHECKPOINT_DELAY, State};
use super::{
    App, Arc, Desktop, Duration, Instant, Ordering, capture_windows,
    shown_sessions,
};

/// The debounce state on `Desktop`.
pub(super) struct Checkpoints {
    /// When the newest change waiting for a checkpoint happened, on the
    /// executor's clock, which tests control.
    changed_at: Option<Instant>,
    /// The one timer, armed while a change waits.
    timer: Option<gpui::Task<()>>,
    /// The window-model revision the last change mark saw.
    seen_revision: u64,
    /// A capture came due while a transfer was reserved or a window was
    /// switching sessions.
    pub(super) waiting_transfer: bool,
    /// Windows switching sessions; a checkpoint waits until none is left.
    pub(super) switching: std::collections::HashSet<gpui::WindowId>,
    /// The quiet time before a capture; tests lengthen it to prove that a
    /// write came from elsewhere.
    pub(super) delay: Duration,
}

impl Default for Checkpoints {
    fn default() -> Self {
        Self {
            changed_at: None,
            timer: None,
            seen_revision: 0,
            waiting_transfer: false,
            switching: std::collections::HashSet::new(),
            delay: CHECKPOINT_DELAY,
        }
    }
}

impl Checkpoints {
    /// Whether no change waits for a checkpoint.
    #[cfg(all(test, feature = "test-support"))]
    pub(super) fn idle(&self) -> bool {
        self.timer.is_none()
    }
}

/// Watches the window model for changes to saved facts.
pub(super) fn observe(cx: &mut App) {
    cx.observe_global::<Desktop>(|cx| {
        let desktop = cx.global::<Desktop>();
        if desktop.windows.revision() != desktop.checkpoints.seen_revision {
            let revision = desktop.windows.revision();
            cx.global_mut::<Desktop>().checkpoints.seen_revision = revision;
            changed(cx);
        }
    })
    .detach();
}

/// Marks saved state dirty: (re)starts the wait for a quiet second. Does
/// nothing unless checkpoints are running.
pub(super) fn changed(cx: &mut App) {
    // A terminal view can run without the desktop, as view tests do.
    if cx.try_global::<Desktop>().is_none_or(|desktop| {
        desktop.runtime.persistence.state() != State::Running
    }) {
        return;
    }
    let now = cx.background_executor().now();
    let checkpoints = &mut cx.global_mut::<Desktop>().checkpoints;
    checkpoints.changed_at = Some(now);
    if checkpoints.timer.is_some() {
        return;
    }
    let task = cx.spawn(async move |cx| {
        loop {
            let wait =
                cx.update(|cx| {
                    let checkpoints = &cx.global::<Desktop>().checkpoints;
                    let due = checkpoints.changed_at? + checkpoints.delay;
                    Some(due.saturating_duration_since(
                        cx.background_executor().now(),
                    ))
                });
            match wait {
                Some(wait) if !wait.is_zero() => {
                    cx.background_executor().timer(wait).await;
                }
                _ => break,
            }
        }
        cx.update(|cx| {
            let checkpoints = &mut cx.global_mut::<Desktop>().checkpoints;
            checkpoints.timer = None;
            if checkpoints.changed_at.take().is_some() {
                take_checkpoint(cx);
            }
        });
    });
    cx.global_mut::<Desktop>().checkpoints.timer = Some(task);
}

/// Marks saved state dirty after a terminal reported a new directory.
pub(in crate::desktop) fn directory_changed(cx: &mut App) {
    changed(cx);
}

/// Captures now and hands the result to the writer, unless a transfer is
/// reserved, in which case the capture waits for it to settle. A capture
/// that a transfer reserved under before it read the runtime may hold the
/// transfer half done; it is dropped and taken again once that transfer
/// settles.
pub(super) fn take_checkpoint(cx: &mut App) {
    let desktop = cx.global_mut::<Desktop>();
    if desktop.runtime.persistence.state() != State::Running {
        return;
    }
    if !desktop.cross.reserved.is_empty()
        || !desktop.checkpoints.switching.is_empty()
    {
        desktop.checkpoints.waiting_transfer = true;
        return;
    }
    desktop.checkpoints.waiting_transfer = false;
    let sequence = desktop.runtime.persistence.begin_capture();
    let transfers = desktop.runtime.transfers.load(Ordering::Acquire);
    let layout = capture_windows(cx);
    let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
    let task = cx.background_executor().spawn(async move {
        #[cfg(all(test, feature = "test-support"))]
        runtime
            .worker_hooks
            .enter(super::worker_hooks::Stage::Checkpoint)
            .await;
        let capture = runtime.lock().capture_runtime(&shown_sessions(&layout));
        if runtime.transfers.load(Ordering::Acquire) != transfers {
            return false;
        }
        runtime.persistence.checkpoint(sequence, capture, layout);
        true
    });
    cx.spawn(async move |cx| {
        if !task.await {
            cx.update(take_checkpoint);
        }
    })
    .detach();
}

/// Notes that `window` reserved, or no longer reserves, a cross-window
/// transfer. A capture that waited on the last one runs now.
pub(super) fn set_transfer_reserved(
    cx: &mut App,
    window: gpui::WindowId,
    reserved: bool,
) {
    let desktop = cx.global_mut::<Desktop>();
    if reserved {
        desktop.runtime.transfers.fetch_add(1, Ordering::AcqRel);
        desktop.cross.reserved.insert(window);
        return;
    }
    if desktop.cross.reserved.remove(&window) {
        resume_waiting(cx);
    }
}

/// Notes that `window` started, or finished, switching sessions. Starting
/// moves the transfer count, so a capture that read the runtime meanwhile
/// is taken again.
pub(super) fn set_switching(
    cx: &mut App,
    window: gpui::WindowId,
    switching: bool,
) {
    let desktop = cx.global_mut::<Desktop>();
    if switching {
        desktop.runtime.transfers.fetch_add(1, Ordering::AcqRel);
        desktop.checkpoints.switching.insert(window);
    } else if desktop.checkpoints.switching.remove(&window) {
        resume_waiting(cx);
    }
}

/// Takes the capture that waited, once no transfer or switch holds it.
fn resume_waiting(cx: &mut App) {
    let desktop = cx.global::<Desktop>();
    if desktop.cross.reserved.is_empty()
        && desktop.checkpoints.switching.is_empty()
        && desktop.checkpoints.waiting_transfer
    {
        take_checkpoint(cx);
    }
}
