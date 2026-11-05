//! Session restore: reading saved state before the event loop, and the
//! bootstrap that reopens it at launch with fresh shells.
//!
//! [`prepare`] runs before `application.run`: it resolves the state root,
//! opens the scope's [`Store`] (taking its lock), loads both files, and sets
//! aside what the plan's Failures table says to. With something to restore,
//! the launch callback calls [`start`] instead of opening a fresh window.
//! The bootstrap spawns every saved terminal on a worker, inserts the
//! structure with [`Mux::restore`] under one short lock, installs the
//! populated projection, and opens each saved window attached to its
//! session. Reopen, Quake shortcuts, checkpoints, and `maybe_exit` wait
//! until it settles.

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
    /// The saved restorable bounds, clamped onto a current display at open.
    pub(super) bounds: Option<Bounds<Pixels>>,
    pub(super) mode: WindowMode,
    pub(super) sidebar_width: Pixels,
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

/// A window to open attached to a restored session.
#[derive(Clone, Debug)]
pub(super) struct Reopen {
    pub(super) attachment: AttachmentId,
    pub(super) session: SessionId,
    pub(super) workspace: WorkspaceId,
    pub(super) placement: Option<Placement>,
    pub(super) focus: Focus,
    pub(super) notices: Vec<NoticeContent>,
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
    /// One attachment per restored session that kept a tab, in order.
    attachments: Vec<(SessionId, AttachmentId)>,
    /// Terminals whose saved directories were all unusable.
    fallbacks: HashSet<TerminalId>,
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
    let task = cx
        .background_executor()
        .spawn(async move { run(&runtime, &capture, &base).await });
    cx.spawn(async move |cx| {
        let outcome = task.await;
        cx.update(|cx| finish(cx, &saved, outcome));
    })
    .detach();
}

/// The command a restored terminal starts with: `base` in the first saved
/// directory that can start a process, else `base`'s own.
fn restored_command(
    base: &TerminalCommand,
    record: &TerminalCapture,
) -> (TerminalCommand, bool) {
    let mut command = base.clone();
    match record
        .directories
        .iter()
        .find(|directory| usable_launch_directory(directory))
    {
        Some(directory) => {
            command.working_directory.clone_from(directory);
            (command, false)
        }
        None => (command, true),
    }
}

/// The worker half: spawns outside the lock, then restores and attaches
/// under it. Checks the termination gate between spawns and under the lock.
#[cfg_attr(
    not(all(test, feature = "test-support")),
    expect(
        clippy::unused_async,
        reason = "only test hooks await between the worker's stages"
    )
)]
async fn run(
    runtime: &DesktopRuntime,
    capture: &RuntimeCapture,
    base: &TerminalCommand,
) -> Outcome {
    #[cfg(all(test, feature = "test-support"))]
    if runtime
        .worker_hooks
        .enter(worker_hooks::Stage::Restore)
        .await
    {
        return Outcome::Failed("injected restore failure".into());
    }
    let outcome = restore_blocking(runtime, capture, base);
    #[cfg(all(test, feature = "test-support"))]
    if matches!(outcome, Outcome::Restored(_)) {
        runtime
            .worker_hooks
            .enter(worker_hooks::Stage::Restored)
            .await;
    }
    outcome
}

fn restore_blocking(
    runtime: &DesktopRuntime,
    capture: &RuntimeCapture,
    base: &TerminalCommand,
) -> Outcome {
    let runtime_id = runtime.lock().runtime_id();
    runtime.persistence.mark_restore_attempt();
    let mut fallbacks = HashSet::new();
    let spawned = SpawnedTerminals::spawn(
        runtime_id,
        capture,
        |record| {
            let (command, fallback) = restored_command(base, record);
            if fallback {
                fallbacks.insert(record.id);
            }
            command
        },
        || runtime.terminating.load(Ordering::Acquire),
    );
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
        match mux.attach_session(session) {
            Ok(attachment) => attachments.push((session, attachment)),
            Err(error) => {
                let _ = mux.close_session(session);
                errors.push(format!(
                    "A session couldn't be shown and was closed: {error}."
                ));
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
    let reopens = plan(saved, &report, &attachments, &fallbacks, runtime, cx);
    let active = saved
        .layout
        .as_ref()
        .and_then(|layout| layout.active_window)
        .and_then(|index| saved.layout.as_ref()?.windows.get(index))
        .map(|window| SessionId::in_runtime(runtime, window.session.get()));
    for (profile, reopen) in reopens {
        if let Some(profile) = profile {
            cx.global_mut::<Desktop>()
                .quake_stash
                .insert(profile, reopen);
            continue;
        }
        let session = reopen.session;
        let attachment = reopen.attachment;
        match open_restored(cx, reopen) {
            Some(id) => {
                let bootstrap = cx.global_mut::<Desktop>().restoring.as_mut();
                if let Some(bootstrap) = bootstrap {
                    bootstrap.outstanding.insert(id);
                    if active == Some(session) {
                        bootstrap.active = Some(id);
                    }
                }
            }
            None => close_unopened(cx, session, attachment),
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

/// Opens a restored window; returns its ID, or `None` when it did not
/// open, after latching a warning for the next window.
fn open_restored(cx: &mut App, reopen: Reopen) -> Option<gpui::WindowId> {
    let before = cx.windows();
    open_window_with_profile(cx, false, None, None, Some(reopen));
    let opened = cx
        .windows()
        .into_iter()
        .find(|handle| !before.contains(handle))
        .map(|handle| handle.window_id());
    if opened.is_none() {
        cx.global_mut::<Desktop>().latched.push(warning(
            "A window couldn't open, so its session was closed.",
        ));
    }
    opened
}

/// Closes the session of a window that failed to open, on a worker, so
/// nothing unreachable keeps running.
fn close_unopened(cx: &mut App, session: SessionId, attachment: AttachmentId) {
    let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
    cx.background_executor()
        .spawn(async move {
            let mut mux = runtime.lock();
            let _ = mux.detach_session(attachment);
            let _ = mux.close_session(session);
        })
        .detach();
}

/// The windows to reopen, in saved order and then one per session no
/// saved window shows, each with the Quake profile it waits for, if any.
#[expect(
    clippy::too_many_lines,
    reason = "saved windows map onto the restored hierarchy in one pass"
)]
fn plan(
    saved: &Saved,
    report: &RestoreReport,
    attachments: &[(SessionId, AttachmentId)],
    fallbacks: &HashSet<TerminalId>,
    runtime: huterm_protocol::RuntimeId,
    cx: &App,
) -> Vec<(Option<String>, Reopen)> {
    let desktop = cx.global::<Desktop>();
    let state = desktop.hierarchy.state();
    let scoped_session =
        |id: SessionId| SessionId::in_runtime(runtime, id.get());
    let attachment = |session: SessionId| {
        attachments
            .iter()
            .find(|(id, _)| *id == session)
            .map(|(_, attachment)| *attachment)
    };
    let mut planned: Vec<(Option<String>, Reopen)> = Vec::new();
    let mut shown = HashSet::new();
    let windows = saved
        .layout
        .as_ref()
        .map(|layout| layout.windows.as_slice())
        .unwrap_or_default();
    for window in windows {
        let session = scoped_session(window.session);
        let workspace =
            WorkspaceId::in_runtime(runtime, window.workspace.get());
        let Some(attachment) = attachment(session) else {
            continue;
        };
        if !shown.insert(session) {
            continue;
        }
        let workspace = if state.workspace_session(workspace) == Some(session) {
            workspace
        } else {
            match first_workspace(state, session) {
                Some(workspace) => workspace,
                None => continue,
            }
        };
        let tab = |id: TabId| TabId::in_runtime(runtime, id.get());
        let in_workspace = |tab: &TabId| {
            state
                .workspace_tabs(workspace)
                .is_some_and(|tabs| tabs.contains(tab))
        };
        let mut history: Vec<TabId> = window
            .tab_history
            .iter()
            .rev()
            .map(|id| tab(*id))
            .filter(in_workspace)
            .collect();
        history.dedup();
        let focus = Focus {
            active: window.active_tab.map(tab).filter(in_workspace),
            history,
            panes: window
                .focused_panes
                .iter()
                .map(|focus| {
                    (
                        tab(focus.tab),
                        PaneId::in_runtime(runtime, focus.pane.get()),
                    )
                })
                .filter(|(tab, _)| in_workspace(tab))
                .collect(),
        };
        let profile = window.quake_profile.clone().filter(|name| {
            desktop.config.quake.profiles.contains_key(name)
                && quake_windows::summons(cx, name)
                && !planned
                    .iter()
                    .any(|(other, _)| other.as_deref() == Some(name.as_str()))
        });
        let placement = profile.is_none().then(|| Placement {
            bounds: (window.quake_profile.is_none()).then(|| Bounds {
                origin: point(px(window.bounds.x), px(window.bounds.y)),
                size: size(px(window.bounds.width), px(window.bounds.height)),
            }),
            mode: if window.quake_profile.is_some() {
                WindowMode::Windowed
            } else {
                window.mode
            },
            sidebar_width: px(window.sidebar_width),
            appearance: appearance(window.appearance),
        });
        planned.push((
            profile,
            Reopen {
                attachment,
                session,
                workspace,
                placement,
                focus,
                notices: Vec::new(),
            },
        ));
    }
    for &(session, attachment) in attachments {
        if shown.contains(&session) {
            continue;
        }
        let Some(workspace) = first_workspace(state, session) else {
            continue;
        };
        planned.push((
            None,
            Reopen {
                attachment,
                session,
                workspace,
                placement: None,
                focus: Focus::default(),
                notices: Vec::new(),
            },
        ));
    }
    for (_, reopen) in &mut planned {
        reopen.notices =
            window_notices(&saved.capture, report, fallbacks, reopen.session);
    }
    planned
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

/// The notices for one restored session's window: panes that started in
/// the default directory, and panes or tabs that failed to start.
fn window_notices(
    capture: &RuntimeCapture,
    report: &RestoreReport,
    fallbacks: &HashSet<TerminalId>,
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
    let failed: Vec<_> = report
        .failed
        .iter()
        .filter(|failed| terminals.contains(&failed.terminal))
        .collect();
    let elsewhere = terminals
        .iter()
        .filter(|terminal| {
            fallbacks.contains(*terminal)
                && !failed.iter().any(|failed| failed.terminal == **terminal)
        })
        .count();
    let mut notices = Vec::new();
    if elsewhere > 0 {
        notices.push(info(fallback_message(elsewhere), Lifetime::Expiring));
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

fn fallback_message(count: usize) -> String {
    let folder = if cfg!(target_os = "macos") {
        "your home folder".to_owned()
    } else {
        std::env::current_dir().map_or_else(
            |_| "another folder".to_owned(),
            |directory| directory.display().to_string(),
        )
    };
    let (verb, possessive, exist) = if count == 1 {
        ("started", "its folder", "no longer exists")
    } else {
        ("started", "their folders", "no longer exist")
    };
    format!(
        "{} {verb} in {folder} because {possessive} {exist}.",
        panes(count)
    )
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
    let Some(display) = best.or(primary.as_ref()).copied() else {
        return saved;
    };
    let width = saved.size.width.min(display.size.width);
    let height = saved.size.height.min(display.size.height);
    let origin = if best.is_some() {
        point(
            saved.origin.x.clamp(
                display.origin.x,
                display.origin.x + display.size.width - width,
            ),
            saved.origin.y.clamp(
                display.origin.y,
                display.origin.y + display.size.height - height,
            ),
        )
    } else {
        point(
            display.origin.x + (display.size.width - width) / 2.0,
            display.origin.y + (display.size.height - height) / 2.0,
        )
    };
    Bounds {
        origin,
        size: size(width, height),
    }
}

#[cfg(test)]
mod tests;
