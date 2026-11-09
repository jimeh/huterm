//! Session restore: reading saved state before the event loop, and the
//! bootstrap that reopens it at launch with fresh shells.
//!
//! [`prepare`] runs before `application.run`: it resolves the state root,
//! opens the scope's [`Store`] (taking its lock), loads both files, and sets
//! aside what the plan's Failures table says to. With something to restore,
//! the launch callback calls [`start`] instead of opening a fresh window.
//! The bootstrap spawns every saved terminal on a worker thread, inserts
//! the structure with [`Mux::restore`] under one short lock, installs the
//! populated projection, and opens each saved window attached to its
//! session. Reopen, Quake shortcuts, checkpoints, and `maybe_exit` wait
//! until it settles. Until Mux owns the spawned terminals, the worker does,
//! so termination waits for it through [`Worker`].

use super::layout_history::LayoutHistory;
use super::model::{NavigationMemory, saved_memory};
use super::persistence::{Persistence, State};
use super::*;
use huterm_core::{RestoreReport, SpawnedTerminals};
use huterm_protocol::{RuntimeCapture, TerminalCapture};
use huterm_state::{
    DesktopLayout, DesktopLoad, LockError, OpenError, RuntimeLoad, StateFile,
    Store, WindowMode,
};

/// What [`prepare`] found before the event loop.
pub(super) struct Prepared {
    pub(super) persistence: Persistence,
    /// Saved state to restore, with a layout that matched it.
    pub(super) saved: Option<Saved>,
    /// Startup notices for the first window.
    pub(super) notices: Vec<NoticeContent>,
    /// The run's first write failure, for a notice.
    pub(super) failures: async_channel::Receiver<String>,
}

/// A loaded runtime capture and the layout saved with it.
#[derive(Debug)]
pub(super) struct Saved {
    capture: RuntimeCapture,
    layout: Option<DesktopLayout>,
}

#[cfg(test)]
impl Saved {
    pub(super) fn new(
        capture: RuntimeCapture,
        layout: Option<DesktopLayout>,
    ) -> Self {
        Self { capture, layout }
    }
}

const NOTICE_TITLE: &str = "Sessions";

/// A persistent restore warning.
fn warning(message: impl Into<String>) -> NoticeContent {
    NoticeContent {
        severity: Severity::Warning,
        source: NoticeSource::Sessions,
        title: NOTICE_TITLE.to_owned(),
        message: message.into(),
        location: None,
        actions: Vec::new(),
        lifetime: Lifetime::Persistent,
    }
}

/// An informational restore notice.
fn info(message: impl Into<String>, lifetime: Lifetime) -> NoticeContent {
    NoticeContent {
        severity: Severity::Info,
        lifetime,
        ..warning(message)
    }
}

/// The notice for a failed write.
pub(super) fn write_failure(reason: &str) -> NoticeContent {
    warning(format!(
        "Couldn't save your windows for next launch: {reason}."
    ))
}

fn restore_failure(reason: &str, kept: bool) -> NoticeContent {
    let kept = if kept {
        format!(
            " The saved state was kept as {}.",
            StateFile::Runtime.corrupt_name()
        )
    } else {
        String::new()
    };
    warning(format!(
        "Couldn't restore your previous windows: {reason}.{kept}"
    ))
}

fn layout_unused() -> NoticeContent {
    info(
        "Your sessions reopened in new windows because their saved window layout couldn't be used.",
        Lifetime::Persistent,
    )
}

const LOCK_HELD: &str = "Another Huterm process is saving sessions, so this one won't restore or save its windows.";
const NEWER: &str = "Saved windows came from a newer Huterm and weren't restored. This window's sessions won't be saved.";

/// Opens and loads the scope's saved state under `root` for `socket`.
/// `enabled` is `[general] restore_sessions`; off, saved files are deleted
/// and nothing restores. Runs before the event loop, on the main thread.
pub(super) fn prepare(
    root: Result<PathBuf, huterm_state::PathError>,
    socket: &str,
    enabled: bool,
) -> Prepared {
    let (report, failures) = async_channel::unbounded();
    let failure = move |reason: String| {
        let _ = report.try_send(reason);
    };
    let mut notices = Vec::new();
    let store = root.map_err(|error| error.to_string()).and_then(|root| {
        Store::open(&root, socket).map_err(|error| match error {
            OpenError::Lock(LockError::Held) => String::new(),
            other => other.to_string(),
        })
    });
    let mut store = match store {
        Ok(store) => store,
        Err(reason) => {
            notices.push(if reason.is_empty() {
                info(LOCK_HELD, Lifetime::Persistent)
            } else {
                write_failure(&reason)
            });
            return Prepared {
                persistence: Persistence::new(None, State::Off, true, failure),
                saved: None,
                notices,
                failures,
            };
        }
    };
    let (saved, blocked) = load(&mut store, enabled, &mut notices);
    let state = match (&saved, enabled && !blocked) {
        (_, false) => State::Off,
        (Some(_), true) => State::Bootstrapping,
        (None, true) => State::Running,
    };
    Prepared {
        persistence: Persistence::new(
            Some(Box::new(store)),
            state,
            blocked,
            failure,
        ),
        saved,
        notices,
        failures,
    }
}

/// Loads both files and applies the Failures table. Returns what to
/// restore and whether persistence must stay off for the run. With the
/// setting off it restores nothing and deletes what a save may replace.
fn load(
    store: &mut Store,
    enabled: bool,
    notices: &mut Vec<NoticeContent>,
) -> (Option<Saved>, bool) {
    let loaded = store.load();
    let crashed = match store.restore_attempt_pending() {
        Ok(pending) => pending,
        Err(error) => {
            notices.push(restore_failure(&error.to_string(), false));
            return (None, true);
        }
    };
    let mut kept = Vec::new();
    let outcome = match loaded.runtime {
        RuntimeLoad::Newer(reason) => {
            eprintln!("Huterm saved state is newer: {reason}");
            kept.push(warning(NEWER));
            (None, true)
        }
        RuntimeLoad::Unreadable(reason) => {
            kept.push(restore_failure(&reason, false));
            (None, true)
        }
        RuntimeLoad::Corrupt(reason) => {
            if set_aside(store, StateFile::Runtime, notices) {
                kept.push(restore_failure(&reason, true));
                (None, false)
            } else {
                (None, true)
            }
        }
        RuntimeLoad::Missing => {
            if matches!(loaded.desktop, DesktopLoad::Corrupt(_)) {
                set_aside(store, StateFile::Desktop, notices);
            }
            (None, false)
        }
        RuntimeLoad::Loaded(_) if crashed && enabled => {
            let keep_layout = matches!(
                loaded.desktop,
                DesktopLoad::Newer(_) | DesktopLoad::Unreadable(_)
            );
            if set_aside(store, StateFile::Runtime, notices)
                && (keep_layout
                    || set_aside(store, StateFile::Desktop, notices))
            {
                kept.push(restore_failure(
                    "the last restore did not finish",
                    true,
                ));
                (None, false)
            } else {
                (None, true)
            }
        }
        RuntimeLoad::Loaded(saved) => {
            let layout = match loaded.desktop {
                DesktopLoad::Loaded(layout) => Some(layout),
                DesktopLoad::Corrupt(reason) => {
                    eprintln!("Huterm saved layout is corrupt: {reason}");
                    set_aside(store, StateFile::Desktop, notices);
                    None
                }
                DesktopLoad::Newer(reason)
                | DesktopLoad::Unreadable(reason) => {
                    eprintln!("Huterm saved layout is not used: {reason}");
                    None
                }
                DesktopLoad::Mismatched | DesktopLoad::Missing => None,
            };
            let saved = (!saved.capture.sessions.is_empty()).then(|| {
                if layout.is_none() {
                    kept.push(layout_unused());
                }
                Saved {
                    capture: saved.capture,
                    layout,
                }
            });
            (saved, false)
        }
    };
    if crashed && outcome.0.is_none() && !outcome.1 {
        let _ = store.clear_restore_attempt();
    }
    if enabled {
        notices.extend(kept);
        outcome
    } else {
        if !outcome.1 {
            let _ = store.remove_saved();
        }
        (None, outcome.1)
    }
}

/// Sets `file` aside as `.corrupt`; returns false, after raising a notice,
/// when the rename failed and the file stays.
fn set_aside(
    store: &mut Store,
    file: StateFile,
    notices: &mut Vec<NoticeContent>,
) -> bool {
    store
        .set_aside(file)
        .map_err(|error| {
            notices.push(warning(format!(
                "Couldn't set aside your saved windows: {error}. This window's sessions won't be saved."
            )));
        })
        .is_ok()
}

/// Where a restored window reopens, in this runtime's scope.
#[derive(Clone, Debug)]
pub(super) struct Placement {
    /// The saved restorable bounds, relative to `display`, clamped onto a
    /// current display at open.
    pub(super) bounds: Option<Bounds<Pixels>>,
    /// The UUID of the display the window was on.
    pub(super) display: Option<String>,
    pub(super) mode: WindowMode,
    pub(super) sidebar_width: Pixels,
    /// The saved sidebar state; `None` from a file saved before it, which
    /// takes `[sidebar] visible`.
    pub(super) workspace_visible: Option<bool>,
    pub(super) workspace_width: Pixels,
    pub(super) appearance: crate::transparency::Layer,
}

/// What a restored window applies once its views have installed.
#[derive(Clone, Debug, Default)]
pub(super) struct Focus {
    pub(super) active: Option<TabId>,
    /// Activated tabs, most recent first.
    pub(super) history: Vec<TabId>,
    pub(super) panes: Vec<(TabId, PaneId)>,
}

/// What [`plan`] does with a restored session.
enum Planned {
    Open(Reopen),
    /// Waits in `quake_stash` for the named Quake profile's first summon.
    Stash(String, Reopen),
}

impl Planned {
    fn reopen_mut(&mut self) -> &mut Reopen {
        match self {
            Self::Open(reopen) | Self::Stash(_, reopen) => reopen,
        }
    }
}

/// A window to open attached to a restored session.
#[derive(Clone, Debug)]
pub(super) struct Reopen {
    pub(super) attachment: AttachmentId,
    pub(super) session: SessionId,
    pub(super) workspace: WorkspaceId,
    pub(super) placement: Option<Placement>,
    pub(super) focus: Focus,
    pub(super) notices: Vec<NoticeContent>,
    /// The saved window layout of a session waiting in `quake_stash`, which
    /// captures write while it waits, so a relaunch stashes it again.
    pub(super) stashed: Option<huterm_state::WindowLayout>,
    /// Where the window was in the other workspaces it showed, scoped into
    /// this runtime and filtered to what came back.
    pub(super) remembered: Vec<NavigationMemory>,
    /// The saved window this reopens, by its index in the saved layout.
    pub(super) saved_index: Option<usize>,
}

impl Reopen {
    /// Moves a Quake stash whose workspace closed or moved to another
    /// session to the workspace its window would follow to, as a session
    /// switch resolves one: the one it remembers in the session, else the
    /// first that holds tabs, else the first, taking that workspace's
    /// remembered navigation. The stash's saved layout takes the same
    /// navigation and remembered list, so a capture before the summon
    /// saves where the window will reopen. Returns whether it moved.
    pub(super) fn follow_workspace(&mut self, state: &HierarchyState) -> bool {
        if state.workspace_session(self.workspace) == Some(self.session) {
            return false;
        }
        self.remembered.retain(|memory| {
            state.workspace_session(memory.workspace) == Some(memory.session)
        });
        let remembered: Vec<WorkspaceId> = self
            .remembered
            .iter()
            .filter(|memory| memory.session == self.session)
            .map(|memory| memory.workspace)
            .collect();
        let Some((workspace, _)) = super::workspace_switch::follow_target(
            state,
            self.session,
            Some(self.workspace),
            remembered,
        ) else {
            return false;
        };
        self.workspace = workspace;
        self.focus = self
            .remembered
            .iter()
            .position(|memory| memory.workspace == workspace)
            .map(|index| {
                let memory = self.remembered.remove(index);
                Focus {
                    active: memory.active,
                    history: memory.history,
                    panes: memory.panes,
                }
            })
            .unwrap_or_default();
        if let Some(layout) = &mut self.stashed {
            let shown = saved_memory(&NavigationMemory {
                session: self.session,
                workspace,
                active: self.focus.active,
                history: self.focus.history.clone(),
                panes: self.focus.panes.clone(),
                layout_history: LayoutHistory::default(),
            });
            layout.workspace = shown.workspace;
            layout.active_tab = shown.active_tab;
            layout.tab_history = shown.tab_history;
            layout.focused_panes = shown.focused_panes;
            layout.remembered =
                self.remembered.iter().map(saved_memory).collect();
        }
        true
    }
}

/// Bootstrap progress, on `Desktop` until it settles.
#[derive(Debug, Default)]
pub(super) struct Bootstrap {
    /// Windows still opening or installing their restored views.
    outstanding: HashSet<gpui::WindowId>,
    /// The worker finished and every window was asked to open.
    opened: bool,
    /// The window to activate last.
    active: Option<gpui::WindowId>,
    /// A reopen request that waited.
    reopen: bool,
    /// Quake summons that waited.
    pub(super) summons: Vec<(
        String,
        huterm_protocol::CommandId,
        Option<WeakEntity<WorkspaceView>>,
    )>,
}

/// Whether a restore is still in progress.
pub(super) fn bootstrapping(cx: &App) -> bool {
    cx.global::<Desktop>().restoring.is_some()
}

/// Notes a reopen request made while restoring.
pub(super) fn defer_reopen(cx: &mut App) -> bool {
    match cx.global_mut::<Desktop>().restoring.as_mut() {
        Some(bootstrap) => {
            bootstrap.reopen = true;
            true
        }
        None => false,
    }
}

/// What the bootstrap worker did.
enum Outcome {
    /// Quit stopped it; teardown owns what it inserted.
    Cancelled,
    /// The structure could not be restored.
    Failed(String),
    Restored(Box<Restored>),
}

struct Restored {
    report: RestoreReport,
    runtime: huterm_protocol::RuntimeId,
    /// One attachment per saved window and Quake stash of each restored
    /// session, and one for a session no saved window shows, in order.
    attachments: Vec<(SessionId, AttachmentId)>,
    /// Terminals that did not start in their first saved directory.
    fallbacks: HashMap<TerminalId, Fallback>,
    /// Restored terminals and the fresh tabs that replaced emptied
    /// sessions' ones; their first viewer reports an early exit.
    fresh: HashSet<TerminalId>,
    /// Problems for the next window: emptied sessions whose fresh tab
    /// failed, and sessions that could not be attached.
    errors: Vec<String>,
    state: HierarchyState,
    subscription: HierarchySubscription,
}

/// Starts the bootstrap for `saved`. Called from the launch callback
/// instead of opening a fresh window.
pub(super) fn start(cx: &mut App, saved: Saved) {
    cx.global_mut::<Desktop>().restoring = Some(Bootstrap::default());
    let config = cx.global::<Desktop>().config.clone();
    let base = resolve_metrics(&config, cx)
        .map_err(|error| error.to_string())
        .and_then(|(_, metrics, resolved)| {
            note_terminal_font(cx, resolved);
            shell_command(
                metrics,
                &config.theme,
                config.terminal,
                &config.shell_integration,
                config.window,
            )
            .map_err(|error| error.to_string())
        });
    match base {
        Ok(base) => start_with(cx, saved, base),
        Err(reason) => {
            // Nothing restores, and the saved state stays for a later run.
            cx.global::<Desktop>().runtime.persistence.suspend();
            fail(cx, &reason);
        }
    }
}

/// [`start`] with every terminal launched from `base` in its saved
/// directory.
pub(super) fn start_with(cx: &mut App, saved: Saved, base: TerminalCommand) {
    if cx.global::<Desktop>().restoring.is_none() {
        cx.global_mut::<Desktop>().restoring = Some(Bootstrap::default());
    }
    let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
    let capture = saved.capture.clone();
    // Each saved window, and each stash, takes an attachment of its own.
    let shown: Vec<u64> = saved
        .layout
        .as_ref()
        .map(|layout| {
            layout
                .windows
                .iter()
                .map(|window| window.session.get())
                .collect()
        })
        .unwrap_or_default();
    let (sender, outcome) = async_channel::bounded(1);
    // Its own thread: spawning blocks for each terminal, and termination
    // waits for it to stop what it spawned.
    let started = std::thread::Builder::new()
        .name("huterm-restore".into())
        .spawn(move || {
            let _ =
                sender.send_blocking(run(&runtime, &capture, &shown, &base));
        });
    if let Err(error) = started {
        cx.global::<Desktop>().runtime.persistence.suspend();
        fail(cx, &format!("the restore could not start: {error}"));
        return;
    }
    cx.spawn(async move |cx| {
        let outcome = outcome.recv().await.unwrap_or_else(|_| {
            Outcome::Failed("the restore stopped unexpectedly".into())
        });
        cx.update(|cx| finish(cx, &saved, outcome));
    })
    .detach();
}

/// The restore worker's claim on the terminals it spawned before
/// [`Mux::restore`] takes them. Termination sets the gate, then waits here,
/// without the Mux lock, until the worker stopped and joined them or
/// handed them to Mux.
#[derive(Debug, Default)]
pub(super) struct Worker {
    running: Mutex<bool>,
    finished: std::sync::Condvar,
    /// Threads waiting in [`Self::wait`], so tests can see one wait.
    #[cfg(test)]
    waiting: std::sync::atomic::AtomicUsize,
}

/// Held while the worker runs; dropping it releases [`Worker::wait`].
struct Running<'a>(&'a Worker);

impl Worker {
    fn lock(&self) -> std::sync::MutexGuard<'_, bool> {
        self.running
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Marks the worker running, unless termination already began.
    fn begin<'a>(&'a self, terminating: &AtomicBool) -> Option<Running<'a>> {
        let mut running = self.lock();
        // Checked under the lock `wait` takes after setting the gate, so
        // either this sees the gate or `wait` sees the worker.
        if terminating.load(Ordering::Acquire) {
            return None;
        }
        *running = true;
        Some(Running(self))
    }

    /// Waits until no worker runs. Call after setting the termination
    /// gate, and never with the Mux lock held: the worker takes it.
    pub(super) fn wait(&self) {
        #[cfg(test)]
        self.waiting.fetch_add(1, Ordering::AcqRel);
        let mut running = self.lock();
        while *running {
            running = self
                .finished
                .wait(running)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        #[cfg(test)]
        self.waiting.fetch_sub(1, Ordering::AcqRel);
    }

    /// How many threads wait for the worker now.
    #[cfg(test)]
    pub(super) fn waiting(&self) -> usize {
        self.waiting.load(Ordering::Acquire)
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        *self.0.lock() = false;
        self.0.finished.notify_all();
    }
}

/// Where a restored terminal started instead of its first saved directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Fallback {
    /// An earlier saved folder: a later candidate, such as its shell's
    /// launch directory.
    Earlier,
    /// The default launch directory.
    Default,
}

/// The command a restored terminal starts with: `base` in the first saved
/// directory that can start a process, else `base`'s own, and the fallback
/// taken when that is not the first candidate.
fn restored_command(
    base: &TerminalCommand,
    record: &TerminalCapture,
) -> (TerminalCommand, Option<Fallback>) {
    let mut command = base.clone();
    match record
        .directories
        .iter()
        .position(|directory| usable_launch_directory(directory))
    {
        Some(index) => {
            command
                .working_directory
                .clone_from(&record.directories[index]);
            (command, (index > 0).then_some(Fallback::Earlier))
        }
        None => (command, Some(Fallback::Default)),
    }
}

/// The worker half, on the restore thread: spawns outside the lock, then
/// restores and attaches under it. Checks the termination gate between
/// spawns and under the lock.
fn run(
    runtime: &DesktopRuntime,
    capture: &RuntimeCapture,
    shown: &[u64],
    base: &TerminalCommand,
) -> Outcome {
    #[cfg(all(test, feature = "test-support"))]
    if runtime
        .worker_hooks
        .enter_blocking(worker_hooks::Stage::Restore)
    {
        return Outcome::Failed("injected restore failure".into());
    }
    let outcome = restore_blocking(runtime, capture, shown, base);
    #[cfg(all(test, feature = "test-support"))]
    if matches!(outcome, Outcome::Restored(_)) {
        runtime
            .worker_hooks
            .enter_blocking(worker_hooks::Stage::Restored);
    }
    outcome
}

fn restore_blocking(
    runtime: &DesktopRuntime,
    capture: &RuntimeCapture,
    shown: &[u64],
    base: &TerminalCommand,
) -> Outcome {
    // Held until the spawned terminals are inserted or stopped.
    let Some(_running) = runtime.restore_worker.begin(&runtime.terminating)
    else {
        return Outcome::Cancelled;
    };
    let runtime_id = runtime.lock().runtime_id();
    runtime.persistence.mark_restore_attempt();
    let (spawned, fallbacks) = spawn_saved(runtime, runtime_id, capture, base);
    let spawned = match spawned {
        Ok(spawned) => spawned,
        Err(MuxError::RestoreCancelled) => return Outcome::Cancelled,
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let mut fresh: HashSet<TerminalId> = spawned.started().collect();
    let mut mux = runtime.lock();
    if runtime.terminating.load(Ordering::Acquire) {
        drop(mux);
        drop(spawned);
        return Outcome::Cancelled;
    }
    let report = match mux.restore(capture, spawned) {
        Ok(report) => report,
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let mut errors = Vec::new();
    let sessions: Vec<SessionId> =
        mux.sessions().iter().map(|session| session.id).collect();
    drop(mux);
    // A session left without a tab gets one fresh tab, spawned under its
    // own lock as any new tab is.
    for &session in &report.empty_sessions {
        let mut mux = runtime.lock();
        if runtime.terminating.load(Ordering::Acquire) {
            return Outcome::Cancelled;
        }
        let workspace = mux
            .session(session)
            .and_then(|session| session.workspaces.first().copied());
        let opened = workspace
            .ok_or(MuxError::UnknownSession(session))
            .and_then(|workspace| mux.open_tab(workspace, base));
        match opened {
            Ok(opened) => {
                fresh.insert(opened.tab.first_pane().terminal_id);
            }
            Err(error) => {
                let _ = mux.close_session(session);
                errors.push(format!(
                    "A session couldn't start a new tab and was closed: {error}."
                ));
            }
        }
    }
    let mut mux = runtime.lock();
    if runtime.terminating.load(Ordering::Acquire) {
        return Outcome::Cancelled;
    }
    let mut attachments = Vec::new();
    for session in sessions {
        if mux.session(session).is_none() {
            continue;
        }
        // One per saved window that shows the session, so two windows on
        // one session come back independent; at least one, so a session no
        // window showed gets a window of its own.
        let windows = shown
            .iter()
            .filter(|raw| **raw == session.get())
            .count()
            .max(1);
        for _ in 0..windows {
            match mux.attach_session(session) {
                Ok(attachment) => attachments.push((session, attachment)),
                Err(error) => {
                    let _ = mux.close_session(session);
                    attachments.retain(|(attached, _)| *attached != session);
                    errors.push(format!(
                        "A session couldn't be shown and was closed: {error}."
                    ));
                    break;
                }
            }
        }
    }
    let (state, subscription) = mux.subscribe_hierarchy();
    Outcome::Restored(Box::new(Restored {
        report,
        runtime: runtime_id,
        attachments,
        fallbacks,
        fresh,
        errors,
        state,
        subscription,
    }))
}

/// Starts every saved terminal from `base` outside the Mux lock, with the
/// directory each fell back from. Checks the termination gate before each
/// spawn, and once it is set stops and joins what already started.
fn spawn_saved(
    runtime: &DesktopRuntime,
    runtime_id: huterm_protocol::RuntimeId,
    capture: &RuntimeCapture,
    base: &TerminalCommand,
) -> (
    Result<SpawnedTerminals, MuxError>,
    HashMap<TerminalId, Fallback>,
) {
    let mut fallbacks = HashMap::new();
    #[cfg(all(test, feature = "test-support"))]
    let mut checks = 0;
    let spawned = SpawnedTerminals::spawn(
        runtime_id,
        capture,
        |record| {
            let (command, fallback) = restored_command(base, record);
            if let Some(fallback) = fallback {
                fallbacks.insert(record.id, fallback);
            }
            command
        },
        || {
            #[cfg(all(test, feature = "test-support"))]
            {
                checks += 1;
                if checks == 2 {
                    runtime
                        .worker_hooks
                        .enter_blocking(worker_hooks::Stage::RestoreSpawned);
                }
            }
            runtime.terminating.load(Ordering::Acquire)
        },
    );
    (spawned, fallbacks)
}

/// Reports a restore that restored nothing and opens a fresh window.
fn fail(cx: &mut App, reason: &str) {
    eprintln!("Huterm restore failed: {reason}");
    cx.global_mut::<Desktop>()
        .latched
        .push(restore_failure(reason, false));
    let desktop = cx.global_mut::<Desktop>();
    if let Some(bootstrap) = desktop.restoring.as_mut() {
        bootstrap.opened = true;
    }
    open_window(cx);
    settle_if_done(cx);
}

/// The UI half: installs the restored projection and opens the windows.
fn finish(cx: &mut App, saved: &Saved, outcome: Outcome) {
    if terminating(cx) || matches!(outcome, Outcome::Cancelled) {
        // Teardown owns whatever the worker inserted.
        cx.global_mut::<Desktop>().restoring = None;
        return;
    }
    let restored = match outcome {
        Outcome::Cancelled => return,
        Outcome::Failed(reason) => {
            // The saved state stays for a later run.
            cx.global::<Desktop>().runtime.persistence.suspend();
            fail(cx, &reason);
            return;
        }
        Outcome::Restored(restored) => *restored,
    };
    let Restored {
        report,
        runtime,
        attachments,
        fallbacks,
        fresh,
        errors,
        state,
        subscription,
    } = restored;
    if !install_hierarchy(cx, state, subscription) {
        resync_hierarchy(cx);
    }
    let desktop = cx.global_mut::<Desktop>();
    desktop.fresh_terminals.extend(fresh);
    for error in errors {
        desktop.latched.push(warning(error));
    }
    let Plan {
        saved: reopens,
        unshown,
        mut unviewed_notices,
        unused,
    } = plan(saved, &report, &attachments, &fallbacks, runtime, cx);
    if !unused.is_empty() {
        detach_unused(cx, unused);
    }
    let active = saved
        .layout
        .as_ref()
        .and_then(|layout| layout.active_window);
    let mut opens = std::collections::VecDeque::new();
    for planned in reopens {
        match planned {
            Planned::Open(reopen) => opens.push_back(reopen),
            Planned::Stash(profile, reopen) => {
                cx.global_mut::<Desktop>()
                    .quake_stash
                    .insert(profile, reopen);
            }
        }
    }
    let mut opened = false;
    while let Some(mut reopen) = opens.pop_front() {
        // The unviewed sessions' notices ride with the first window that
        // opens.
        let carried = std::mem::take(&mut unviewed_notices);
        reopen.notices.extend(carried.iter().cloned());
        let later = opens.iter().any(|other| other.session == reopen.session);
        if open_planned(cx, reopen, later, active) {
            opened = true;
        } else {
            unviewed_notices = carried;
        }
    }
    if opened {
        // Sessions no saved window showed stay unviewed, reachable from
        // the session menu, as they were before Quit.
        let attachments: Vec<AttachmentId> =
            unshown.iter().map(|reopen| reopen.attachment).collect();
        if !attachments.is_empty() {
            detach_unused(cx, attachments);
        }
    } else {
        // No saved window came back, as after a lost or empty layout or
        // when every one failed to open: each session opens in a window of
        // its own, so the launch never ends with none.
        for reopen in unshown {
            open_planned(cx, reopen, false, active);
        }
    }
    if let Some(bootstrap) = cx.global_mut::<Desktop>().restoring.as_mut() {
        bootstrap.opened = true;
    }
    if cx.windows().is_empty() && !quake_windows::keep_alive(cx) {
        open_window(cx);
    }
    settle_if_done(cx);
}

/// Opens a planned window and counts it toward the bootstrap; one that
/// fails to open is cleaned up. `later` says a window still to open shows
/// its session. Returns whether it opened.
fn open_planned(
    cx: &mut App,
    reopen: Reopen,
    later: bool,
    active: Option<usize>,
) -> bool {
    let (session, attachment) = (reopen.session, reopen.attachment);
    let saved_index = reopen.saved_index;
    let Some(id) = open_restored(cx, reopen) else {
        close_unopened(cx, session, attachment, later);
        return false;
    };
    if let Some(bootstrap) = cx.global_mut::<Desktop>().restoring.as_mut() {
        bootstrap.outstanding.insert(id);
        if saved_index.is_some() && saved_index == active {
            bootstrap.active = Some(id);
        }
    }
    true
}

/// Opens a restored window; returns its ID, or `None` when it did not
/// open.
fn open_restored(cx: &mut App, reopen: Reopen) -> Option<gpui::WindowId> {
    let before = cx.windows();
    open_window_with_profile(cx, false, None, None, Some(reopen));
    cx.windows()
        .into_iter()
        .find(|handle| !before.contains(handle))
        .map(|handle| handle.window_id())
}

/// Cleans up after a restored window that failed to open, on a worker:
/// detaches only its own attachment, and closes its session only when no
/// other restored window or stash holds it, so nothing unreachable keeps
/// running. `later` says a window still to open shows the session. Latches
/// a warning for the next window.
fn close_unopened(
    cx: &mut App,
    session: SessionId,
    attachment: AttachmentId,
    later: bool,
) {
    let desktop = cx.global::<Desktop>();
    let held = later
        || desktop.windows.shown_elsewhere(session, None)
        || desktop
            .quake_stash
            .values()
            .any(|reopen| reopen.session == session);
    cx.global_mut::<Desktop>().latched.push(warning(if held {
        "A window couldn't open."
    } else {
        "A window couldn't open, so its session was closed."
    }));
    let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
    cx.background_executor()
        .spawn(async move {
            let mut mux = runtime.lock();
            let _ = mux.detach_session(attachment);
            // Any other attachment left belongs to a window that failed
            // too, whose own cleanup may not have run yet.
            if !held {
                let _ = mux.close_session(session);
            }
        })
        .detach();
}

/// Detaches attachments no planned window took, such as a saved window's
/// whose session lost its workspaces.
fn detach_unused(cx: &mut App, unused: Vec<AttachmentId>) {
    let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
    cx.background_executor()
        .spawn(async move {
            let mut mux = runtime.lock();
            for attachment in unused {
                let _ = mux.detach_session(attachment);
            }
        })
        .detach();
}

/// What a restore opens.
struct Plan {
    /// The saved windows to reopen, in saved order, and the sessions that
    /// wait for a Quake profile's first summon instead.
    saved: Vec<Planned>,
    /// A window for each session no saved window shows, which opens only
    /// when no saved window does.
    unshown: Vec<Reopen>,
    /// The restore notices of the sessions no saved window shows, named
    /// after each, for the first window that opens.
    unviewed_notices: Vec<NoticeContent>,
    /// The attachments no window takes, to detach.
    unused: Vec<AttachmentId>,
}

/// Plans the restore's windows. Each saved window takes its own attachment
/// of its session.
#[expect(
    clippy::too_many_lines,
    reason = "saved windows map onto the restored hierarchy in one pass"
)]
fn plan(
    saved: &Saved,
    report: &RestoreReport,
    attachments: &[(SessionId, AttachmentId)],
    fallbacks: &HashMap<TerminalId, Fallback>,
    runtime: huterm_protocol::RuntimeId,
    cx: &App,
) -> Plan {
    let desktop = cx.global::<Desktop>();
    let state = desktop.hierarchy.state();
    let scoped_session =
        |id: SessionId| SessionId::in_runtime(runtime, id.get());
    let mut free: Vec<(SessionId, AttachmentId)> = attachments.to_vec();
    let mut take = |session: SessionId| {
        let index = free.iter().position(|(id, _)| *id == session)?;
        Some(free.remove(index).1)
    };
    let mut planned: Vec<Planned> = Vec::new();
    let mut shown = HashSet::new();
    let windows = saved
        .layout
        .as_ref()
        .map(|layout| layout.windows.as_slice())
        .unwrap_or_default();
    for (index, window) in windows.iter().enumerate() {
        let session = scoped_session(window.session);
        let Some((workspace, focus)) = scoped_navigation(
            state,
            runtime,
            session,
            window.workspace,
            window.active_tab,
            &window.tab_history,
            &window.focused_panes,
            Workspace::FirstIfMissing,
        ) else {
            continue;
        };
        let Some(attachment) = take(session) else {
            continue;
        };
        shown.insert(session);
        let remembered = remembered_navigation(
            state,
            runtime,
            workspace,
            &window.remembered,
        );
        let profile = window.quake_profile.clone().filter(|name| {
            desktop.config.quake.profiles.contains_key(name)
                && quake_windows::summons(cx, name)
                && !planned.iter().any(|other| {
                    matches!(other, Planned::Stash(other, _) if other == name)
                })
        });
        let reopen = Reopen {
            attachment,
            session,
            workspace,
            placement: None,
            focus,
            notices: Vec::new(),
            stashed: None,
            remembered,
            saved_index: Some(index),
        };
        planned.push(match profile {
            Some(profile) => Planned::Stash(
                profile,
                Reopen {
                    stashed: Some(huterm_state::WindowLayout {
                        workspace: WorkspaceId::new(workspace.get()),
                        ..window.clone()
                    }),
                    ..reopen
                },
            ),
            None => Planned::Open(Reopen {
                placement: Some(placement(window)),
                ..reopen
            }),
        });
    }
    for planned in &mut planned {
        let reopen = planned.reopen_mut();
        reopen.notices =
            window_notices(&saved.capture, report, fallbacks, reopen.session);
    }
    let mut unused = Vec::new();
    let mut unshown = Vec::new();
    // An unviewed session's restore notices go to the first window that
    // opens, named after the session, since no window of its own shows
    // them; a window that opens for it shows them as its own.
    let mut unviewed_notices = Vec::new();
    for (session, attachment) in free {
        let Some(info) =
            state.session(session).filter(|_| !shown.contains(&session))
        else {
            unused.push(attachment);
            continue;
        };
        shown.insert(session);
        let notices =
            window_notices(&saved.capture, report, fallbacks, session);
        let name = info.display_name().to_owned();
        unviewed_notices.extend(notices.iter().cloned().map(|mut notice| {
            notice.message = format!("In \"{name}\": {}", notice.message);
            notice
        }));
        let Some(workspace) = first_workspace(state, session) else {
            unused.push(attachment);
            continue;
        };
        unshown.push(Reopen {
            attachment,
            session,
            workspace,
            placement: None,
            focus: Focus::default(),
            notices,
            stashed: None,
            remembered: Vec::new(),
            saved_index: None,
        });
    }
    Plan {
        saved: planned,
        unshown,
        unviewed_notices,
        unused,
    }
}

/// A window's saved navigation memory scoped into `runtime`, strictly: an
/// entry whose workspace did not come back in its session is dropped, the
/// workspace the window shows (`shown`) is left out, since its own
/// navigation applies there, and entries are deduplicated by workspace,
/// the most recent winning.
fn remembered_navigation(
    state: &HierarchyState,
    runtime: huterm_protocol::RuntimeId,
    shown: WorkspaceId,
    saved: &[huterm_state::SessionNavigation],
) -> Vec<NavigationMemory> {
    let mut remembered: Vec<NavigationMemory> = Vec::new();
    for memory in saved {
        let session = SessionId::in_runtime(runtime, memory.session.get());
        let Some((workspace, focus)) = scoped_navigation(
            state,
            runtime,
            session,
            memory.workspace,
            memory.active_tab,
            &memory.tab_history,
            &memory.focused_panes,
            Workspace::Strict,
        ) else {
            continue;
        };
        if workspace == shown
            || remembered.iter().any(|kept| kept.workspace == workspace)
        {
            continue;
        }
        remembered.push(NavigationMemory {
            session,
            workspace,
            active: focus.active,
            history: focus.history,
            panes: focus.panes,
            layout_history: LayoutHistory::default(),
        });
    }
    remembered
}

/// What scoping does with a saved workspace that did not come back in its
/// session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Workspace {
    /// The navigation applies to the session's first workspace, as the
    /// workspace a window shows does.
    FirstIfMissing,
    /// The navigation is dropped, as a remembered entry is.
    Strict,
}

/// Saved navigation in one session scoped into `runtime`: the saved
/// workspace when it still belongs to the session, else, as `missing`
/// says, the session's first or nothing, and the tabs and panes the
/// projection holds there. `None` when the session did not come back or
/// has no workspace.
#[expect(
    clippy::too_many_arguments,
    reason = "one saved navigation's fields, scoped together"
)]
fn scoped_navigation(
    state: &HierarchyState,
    runtime: huterm_protocol::RuntimeId,
    session: SessionId,
    workspace: WorkspaceId,
    active_tab: Option<TabId>,
    tab_history: &[TabId],
    focused_panes: &[huterm_state::FocusedPane],
    missing: Workspace,
) -> Option<(WorkspaceId, Focus)> {
    state.session(session)?;
    let workspace = WorkspaceId::in_runtime(runtime, workspace.get());
    let workspace = if state.workspace_session(workspace) == Some(session) {
        workspace
    } else if missing == Workspace::FirstIfMissing {
        first_workspace(state, session)?
    } else {
        return None;
    };
    let tab = |id: TabId| TabId::in_runtime(runtime, id.get());
    let in_workspace = |tab: &TabId| {
        state
            .workspace_tabs(workspace)
            .is_some_and(|tabs| tabs.contains(tab))
    };
    let mut history: Vec<TabId> = tab_history
        .iter()
        .rev()
        .map(|id| tab(*id))
        .filter(in_workspace)
        .collect();
    history.dedup();
    let focus = Focus {
        active: active_tab.map(tab).filter(in_workspace),
        history,
        panes: focused_panes
            .iter()
            .map(|focus| {
                (
                    tab(focus.tab),
                    PaneId::in_runtime(runtime, focus.pane.get()),
                )
            })
            .filter(|(tab, pane)| {
                in_workspace(tab)
                    && state
                        .tab(*tab)
                        .is_some_and(|info| info.layout.pane(*pane).is_some())
            })
            .collect(),
    };
    Some((workspace, focus))
}

/// Where a saved window reopens as a regular window. A Quake window's
/// saved bounds were its profile's, so it opens windowed at the default
/// size instead.
fn placement(window: &huterm_state::WindowLayout) -> Placement {
    let quake = window.quake_profile.is_some();
    Placement {
        bounds: (!quake).then(|| Bounds {
            origin: point(px(window.bounds.x), px(window.bounds.y)),
            size: size(px(window.bounds.width), px(window.bounds.height)),
        }),
        display: window.display.clone(),
        mode: if quake {
            WindowMode::Windowed
        } else {
            window.mode
        },
        sidebar_width: px(window.sidebar_width),
        workspace_visible: window.workspace_panel_visible,
        // A window saved before the sidebar existed takes the default width.
        workspace_width: if window.workspace_panel_width > 0.0 {
            px(window.workspace_panel_width)
        } else {
            workspace_chrome::PANEL_WIDTH
        },
        appearance: appearance(window.appearance),
    }
}

/// The saved layouts of the sessions waiting for a Quake summon, by profile
/// name, for a capture: a Quit or crash before the first summon keeps them.
pub(super) fn stashed_layouts(cx: &App) -> Vec<huterm_state::WindowLayout> {
    let desktop = cx.global::<Desktop>();
    let state = desktop.hierarchy.state();
    let mut stashed: Vec<_> = desktop
        .quake_stash
        .iter()
        .filter(|(_, reopen)| state.session(reopen.session).is_some())
        .filter_map(|(name, reopen)| Some((name, reopen.stashed.as_ref()?)))
        .collect();
    stashed.sort_by_key(|(name, _)| *name);
    stashed
        .into_iter()
        .map(|(name, layout)| huterm_state::WindowLayout {
            quake_profile: Some(name.clone()),
            ..layout.clone()
        })
        .collect()
}

/// Brings the sessions waiting for a Quake summon up to a reload: a
/// renamed profile's session follows the rename, and one whose profile is
/// gone, no longer has a registered shortcut, or is already served opens
/// in a regular window, as a launch does for a missing profile. Runs
/// before the windows reconcile, which drop the renames the reload read.
pub(super) fn reconcile_stash(cx: &mut App) {
    let mut stash: Vec<_> =
        std::mem::take(&mut cx.global_mut::<Desktop>().quake_stash)
            .into_iter()
            .collect();
    stash.sort_by(|(a, _), (b, _)| a.cmp(b));
    let mut kept = HashMap::new();
    let mut orphans = Vec::new();
    let mut renamed = false;
    for (name, mut reopen) in stash {
        let desktop = cx.global::<Desktop>();
        let profiles = &desktop.config.quake.profiles;
        let target = desktop
            .windows
            .quake_rename_target(&name, |name| profiles.contains_key(name))
            .filter(|target| {
                quake_windows::summons(cx, target)
                    && !kept.contains_key(target)
                    && desktop.windows.quake_window(target).is_none()
            });
        match target {
            Some(target) => {
                if let Some(layout) = &mut reopen.stashed {
                    layout.quake_profile = Some(target.clone());
                }
                renamed |= target != name;
                kept.insert(target, reopen);
            }
            None => orphans.push(reopen),
        }
    }
    cx.global_mut::<Desktop>().quake_stash = kept;
    // A renamed stash changes no window record, so nothing else schedules
    // the checkpoint that keeps the new name through a crash.
    if renamed {
        checkpoints::changed(cx);
    }
    for mut reopen in orphans {
        reopen.placement = reopen.stashed.take().as_ref().map(placement);
        let (session, attachment) = (reopen.session, reopen.attachment);
        if open_restored(cx, reopen).is_none() {
            close_unopened(cx, session, attachment, false);
        }
    }
}

fn first_workspace(
    state: &HierarchyState,
    session: SessionId,
) -> Option<WorkspaceId> {
    state.session_workspaces(session)?.first().copied()
}

/// The transparency overrides a saved appearance names, within the
/// ranges the window accepts.
fn appearance(
    saved: Option<huterm_state::WindowAppearance>,
) -> crate::transparency::Layer {
    let Some(saved) = saved else {
        return crate::transparency::Layer::default();
    };
    let opacity = |value: Option<f32>| value.map(|value| value.clamp(0.0, 1.0));
    crate::transparency::Layer {
        opacity: opacity(saved.opacity),
        background: opacity(saved.background_opacity),
        foreground: opacity(saved.foreground_opacity),
        blur: saved.blur.map(|blur| {
            u8::try_from(blur.min(u32::from(huterm_config::MAX_BLUR)))
                .unwrap_or(huterm_config::MAX_BLUR)
        }),
    }
}

/// The notices for one restored session's window: panes that did not
/// start in their last directory, and panes or tabs that failed to start.
fn window_notices(
    capture: &RuntimeCapture,
    report: &RestoreReport,
    fallbacks: &HashMap<TerminalId, Fallback>,
    session: SessionId,
) -> Vec<NoticeContent> {
    let Some(saved) = capture
        .sessions
        .iter()
        .find(|saved| saved.info.id.get() == session.get())
    else {
        return Vec::new();
    };
    let terminals: Vec<TerminalId> = saved
        .workspaces
        .iter()
        .flat_map(|workspace| &workspace.tabs)
        .flat_map(|tab| tab.layout.panes())
        .map(|pane| pane.terminal_id)
        .collect();
    // The report names terminals in the restored runtime's scope; the
    // capture's, like the fallbacks', are unscoped.
    let failed: Vec<_> = report
        .failed
        .iter()
        .filter(|failed| {
            terminals.contains(&TerminalId::new(failed.terminal.get()))
        })
        .collect();
    let elsewhere: Vec<Fallback> = terminals
        .iter()
        .filter(|terminal| {
            !failed
                .iter()
                .any(|failed| failed.terminal.get() == terminal.get())
        })
        .filter_map(|terminal| fallbacks.get(terminal).copied())
        .collect();
    let mut notices = Vec::new();
    if !elsewhere.is_empty() {
        let earlier = elsewhere
            .iter()
            .filter(|fallback| **fallback == Fallback::Earlier)
            .count();
        notices.push(info(
            fallback_message(earlier, elsewhere.len() - earlier),
            Lifetime::Expiring,
        ));
    }
    if let Some(first) = failed.first() {
        notices.push(warning(format!(
            "{} couldn't start and {} left out: {}.",
            panes(failed.len()),
            if failed.len() == 1 { "was" } else { "were" },
            first.error
        )));
    }
    notices
}

fn panes(count: usize) -> String {
    if count == 1 {
        "1 pane".to_owned()
    } else {
        format!("{count} panes")
    }
}

/// The notice for `earlier` panes that started in an earlier saved folder
/// and `default` panes that started in the default directory.
fn fallback_message(earlier: usize, default: usize) -> String {
    let folder = match (earlier, default) {
        (0, _) if cfg!(target_os = "macos") => "your home folder".to_owned(),
        (0, _) => std::env::current_dir().map_or_else(
            |_| "another folder".to_owned(),
            |directory| directory.display().to_string(),
        ),
        (_, 0) => "an earlier folder".to_owned(),
        _ => "a different folder".to_owned(),
    };
    let last = if earlier == 0 { "" } else { "last " };
    let count = earlier + default;
    let reason = if count == 1 {
        format!("its {last}folder no longer exists")
    } else {
        format!("their {last}folders no longer exist")
    };
    format!("{} started in {folder} because {reason}.", panes(count))
}

/// Records that `window` finished opening its restored views, or went
/// away, and settles the bootstrap once none is left.
pub(super) fn window_settled(cx: &mut App, window: gpui::WindowId) {
    let Some(bootstrap) = cx.global_mut::<Desktop>().restoring.as_mut() else {
        return;
    };
    if bootstrap.outstanding.remove(&window) {
        settle_if_done(cx);
    }
}

/// Settles once the worker finished and every restored window opened or
/// failed: removes the marker, starts checkpoints with one capture,
/// releases the gates, and activates the saved active window.
fn settle_if_done(cx: &mut App) {
    let desktop = cx.global::<Desktop>();
    if !desktop.restoring.as_ref().is_some_and(|bootstrap| {
        bootstrap.opened && bootstrap.outstanding.is_empty()
    }) {
        return;
    }
    let Some(bootstrap) = cx.global_mut::<Desktop>().restoring.take() else {
        return;
    };
    if cx.global::<Desktop>().runtime.persistence.settle() {
        take_checkpoint(cx);
    }
    if let Some(active) = bootstrap.active
        && let Some(handle) = cx
            .windows()
            .into_iter()
            .find(|handle| handle.window_id() == active)
    {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
    }
    for (name, command, reporter) in bootstrap.summons {
        quake_windows::summon_later(cx, name, command, reporter);
    }
    if bootstrap.reopen && cx.windows().is_empty() {
        open_window(cx);
    }
    maybe_exit(cx);
}

/// Applies `[general] restore_sessions` after a reload: off stops saving
/// and deletes the saved files, on starts checkpointing with a capture now.
pub(super) fn apply_setting(cx: &mut App, enabled: bool) {
    if cx
        .global::<Desktop>()
        .runtime
        .persistence
        .set_enabled(enabled)
    {
        take_checkpoint(cx);
    }
}

/// Raises each write failure the writer reports, in the active window or
/// the next one to open or become active.
pub(super) fn report_write_failures(
    cx: &mut App,
    failures: async_channel::Receiver<String>,
) {
    cx.spawn(async move |cx| {
        while let Ok(reason) = failures.recv().await {
            cx.update(|cx| raise(cx, write_failure(&reason)));
        }
    })
    .detach();
}

/// Raises `content` in the active window when it can show notices, else
/// latches it for the next window to open or become active.
fn raise(cx: &mut App, content: NoticeContent) {
    let shown = cx.active_window().is_some_and(|handle| {
        handle
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.update(cx, |view, cx| {
                        let shown = view.can_show_notices(cx);
                        if shown {
                            view.notify(content.clone(), cx);
                        }
                        shown
                    })
                })
            })
            .unwrap_or(false)
    });
    if !shown {
        cx.global_mut::<Desktop>().latched.push(content);
    }
}

/// The UUID that names `display` in a saved layout, in hyphenated form.
/// X11 reports the nil UUID for every screen, which names none.
pub(super) fn display_uuid(
    display: &dyn gpui::PlatformDisplay,
) -> Option<String> {
    display
        .uuid()
        .ok()
        .filter(|uuid| !uuid.is_nil())
        .map(|uuid| uuid.to_string())
}

/// A current display, as restored placement sees it.
#[derive(Clone, Debug)]
pub(super) struct Screen {
    pub(super) id: gpui::DisplayId,
    /// See [`display_uuid`].
    pub(super) uuid: Option<String>,
    /// The visible area, in the coordinates window bounds on this display
    /// use.
    pub(super) visible: Bounds<Pixels>,
    pub(super) primary: bool,
}

/// Where a restored window with `saved` bounds on the display named
/// `display` opens: on that display when one of `screens` has its UUID, at
/// the same position within it, moved onto its visible area; otherwise on
/// the primary display through [`place_on_displays`]. `global` says the
/// platform reports every display in one coordinate space, so saved bounds
/// may land on any of them; macOS reports each display from its own origin.
pub(super) fn place_restored(
    saved: Bounds<Pixels>,
    display: Option<&str>,
    screens: &[Screen],
    global: bool,
) -> (Option<gpui::DisplayId>, Bounds<Pixels>) {
    let named = display.and_then(|uuid| {
        screens.iter().find(|screen| {
            screen
                .uuid
                .as_deref()
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(uuid))
        })
    });
    if let Some(screen) = named {
        return (Some(screen.id), clamp_onto(saved, screen.visible));
    }
    let primary = screens
        .iter()
        .find(|screen| screen.primary)
        .map(|screen| screen.visible);
    let displays: Vec<_> = if global {
        screens.iter().map(|screen| screen.visible).collect()
    } else {
        primary.into_iter().collect()
    };
    (None, place_on_displays(saved, &displays, primary))
}

/// `saved` shrunk to fit `display` and moved fully inside it.
fn clamp_onto(
    saved: Bounds<Pixels>,
    display: Bounds<Pixels>,
) -> Bounds<Pixels> {
    let width = saved.size.width.min(display.size.width);
    let height = saved.size.height.min(display.size.height);
    Bounds {
        origin: point(
            saved.origin.x.clamp(
                display.origin.x,
                display.origin.x + display.size.width - width,
            ),
            saved.origin.y.clamp(
                display.origin.y,
                display.origin.y + display.size.height - height,
            ),
        ),
        size: size(width, height),
    }
}

/// Places saved `bounds` on a current display: kept where it meets one,
/// shrunk to fit and moved fully inside the display it overlaps most, and
/// centred on `primary` when it meets none.
pub(super) fn place_on_displays(
    saved: Bounds<Pixels>,
    displays: &[Bounds<Pixels>],
    primary: Option<Bounds<Pixels>>,
) -> Bounds<Pixels> {
    let overlap = |display: &Bounds<Pixels>| {
        let area = saved.intersect(display);
        f32::from(area.size.width).max(0.0)
            * f32::from(area.size.height).max(0.0)
    };
    let best = displays
        .iter()
        .filter(|display| overlap(display) > 0.0)
        .max_by(|a, b| overlap(a).total_cmp(&overlap(b)));
    if let Some(display) = best {
        return clamp_onto(saved, *display);
    }
    let Some(display) = primary else {
        return saved;
    };
    let width = saved.size.width.min(display.size.width);
    let height = saved.size.height.min(display.size.height);
    Bounds {
        origin: point(
            display.origin.x + (display.size.width - width) / 2.0,
            display.origin.y + (display.size.height - height) / 2.0,
        ),
        size: size(width, height),
    }
}

#[cfg(test)]
mod tests;
