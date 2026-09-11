#[path = "fullscreen_smoke.rs"]
pub(crate) mod fullscreen_smoke;
#[cfg(target_os = "macos")]
#[path = "input_smoke.rs"]
pub(crate) mod input_smoke;
#[path = "integration_smoke.rs"]
pub(crate) mod integration_smoke;
#[path = "palette_smoke.rs"]
pub(crate) mod palette_smoke;
#[path = "quake_smoke.rs"]
pub(crate) mod quake_smoke;
#[path = "quake_windows.rs"]
mod quake_windows;
#[cfg(all(target_os = "macos", feature = "macos-updater"))]
#[path = "updater_smoke.rs"]
pub(crate) mod updater_smoke;
use super::palette::{
    CommandFrequency, CommandPalette, HistoryView, PaletteEvent, PaletteTarget,
    RecentCommands,
};
use super::*;
use crate::commands::{
    Route, fill_rename_target, fill_target, route, select_tab_slot,
};
use crate::config::TabPosition;
use crate::fullscreen::{Effect, FullscreenController, ToggleIntent};
#[cfg(target_os = "macos")]
use crate::native_quit;
#[cfg(all(target_os = "macos", feature = "macos-updater"))]
use crate::native_updater;
use gpui::{AnyWindowHandle, Entity, Global, WeakEntity};
use huterm_core::{
    CloseAssessment, CloseRequest, HierarchySnapshot, MuxError, OpenedTab,
};
use huterm_protocol::{
    AttachmentId, CommandArgument, CommandScope, SessionId, WorkspaceId,
    catalog, validate, validate_supplied,
};
use std::sync::atomic::{AtomicBool, Ordering};

const TAB_HEIGHT: Pixels = px(32.0);
pub(super) const SIDEBAR_WIDTH: Pixels = px(180.0);
mod tab_strip;
pub(super) mod tab_visibility;
use tab_strip::TabStrip;
use tab_visibility::{Presentation, Reveal};
const CONTROL_SIZE: Pixels = px(28.0);
const TAB_DRAG_THRESHOLD: f64 = 4.0;

#[derive(Default)]
struct DesktopRuntime {
    mux: Mutex<Mux>,
    terminating: AtomicBool,
    restore: Mutex<Option<RestoreSnapshot>>,
}

impl DesktopRuntime {
    fn palette_snapshot(
        &self,
        workspace: Option<WorkspaceId>,
        tab: Option<TabId>,
    ) -> Result<
        Option<(huterm_core::SelectionTarget, HierarchySnapshot)>,
        MuxError,
    > {
        let mux = self
            .mux
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let target = if let Some(tab) = tab {
            mux.select_tab(tab)?
        } else if let Some(workspace) = workspace {
            mux.select_workspace(workspace)?
        } else {
            return Ok(None);
        };
        Ok(Some((target, mux.capture_hierarchy())))
    }

    fn open_tab(
        &self,
        workspace: Option<WorkspaceId>,
        command: &TerminalCommand,
    ) -> Result<
        (SessionId, WorkspaceId, OpenedTab, Option<AttachmentId>),
        MuxError,
    > {
        let mut mux = self
            .mux
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.terminating.load(Ordering::Acquire) {
            return Err(RuntimeError::Stopped.into());
        }
        let id = if let Some(id) = workspace {
            id
        } else {
            let session = mux.create_session(None)?;
            match mux.create_workspace(session, None) {
                Ok(id) => id,
                Err(error) => {
                    let _ = mux.close_session(session);
                    return Err(error);
                }
            }
        };
        match mux.open_tab(id, command) {
            Ok(tab) => {
                let session = mux.select_workspace(id)?.session;
                let attachment = if workspace.is_none() {
                    match mux.attach_session(session) {
                        Ok(attachment) => Some(attachment),
                        Err(error) => {
                            let _ = mux.close_session(session);
                            return Err(error);
                        }
                    }
                } else {
                    None
                };
                Ok((session, id, tab, attachment))
            }
            Err(error) => {
                if workspace.is_none() {
                    let session = mux.select_workspace(id)?.session;
                    let _ = mux.close_session(session);
                }
                Err(error)
            }
        }
    }

    fn cleanup_spawn(
        &self,
        original_session: SessionId,
        workspace: WorkspaceId,
        tab: TabId,
        attachment: Option<AttachmentId>,
    ) {
        let mut mux = self
            .mux
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Never tear down a resource another attachment has adopted, including
        // a tab moved to another session while its UI publication was pending.
        if let Some(attachment) = attachment {
            let _ = mux.detach_session(attachment);
            if mux.session(original_session).is_some() {
                let session = original_session;
                let snapshot = mux.capture_hierarchy();
                if !snapshot.attachments.iter().any(|(_, id)| *id == session) {
                    let _ = mux.close_session(session);
                }
            }
        } else if let Ok(target) = mux.select_tab(tab) {
            let snapshot = mux.capture_hierarchy();
            if target.workspace == Some(workspace)
                && !snapshot
                    .attachments
                    .iter()
                    .any(|(_, id)| *id == target.session)
            {
                let _ = mux.close_tab(workspace, tab);
            }
        }
    }

    fn assess(
        &self,
        request: CloseRequest,
    ) -> Result<CloseAssessment, MuxError> {
        let ticket = self
            .mux
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .prepare_close(request)?;
        Ok(ticket.check_jobs())
    }

    fn commit(
        &self,
        assessment: &CloseAssessment,
        confirmed: bool,
        windows: Option<Vec<WindowRestore>>,
    ) -> Result<(), MuxError> {
        let current = assessment.recheck();
        let mut mux = self
            .mux
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        mux.commit_close_with(assessment, &current, confirmed, |mux| {
            if let Some(windows) = windows {
                self.terminating.store(true, Ordering::Release);
                self.capture(mux, windows);
            }
        })
    }

    fn capture(&self, mux: &Mux, windows: Vec<WindowRestore>) {
        self.restore
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert_with(|| RestoreSnapshot {
                hierarchy: mux.capture_hierarchy(),
                windows,
            });
    }

    /// Runs a runtime-scope command on the structural worker.
    fn execute(
        &self,
        invocation: &CommandInvocation,
    ) -> Result<CommandOutcome, CommandError> {
        let mut mux = self
            .mux
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.terminating.load(Ordering::Acquire) {
            return Err(CommandError::Unavailable(
                "runtime is terminating".to_owned(),
            ));
        }
        let mut invocation = invocation.clone();
        if matches!(
            invocation.id,
            ids::RENAME_SESSION | ids::RESET_SESSION_NAME
        ) {
            // The window knows its workspace; the session owning it is
            // canonical runtime state, so resolve it under the same lock. A
            // workspace that vanished since the window captured it is a
            // stale target, not a missing one.
            if invocation.session("session").is_none() {
                let workspace = invocation.workspace("workspace").ok_or(
                    CommandError::MissingArgument {
                        command: invocation.id,
                        name: "session",
                    },
                )?;
                let session = mux
                    .workspace(workspace)
                    .map(|workspace| workspace.session_id)
                    .ok_or(CommandError::StaleTarget)?;
                invocation.args.push(CommandArgument::new(
                    "session",
                    CommandValue::Session(session),
                ));
            }
            invocation
                .args
                .retain(|argument| argument.name != "workspace");
        }
        huterm_core::execute(&mut mux, &invocation)
    }

    fn reorder_tab(
        &self,
        workspace: WorkspaceId,
        tab: TabId,
        before: Option<TabId>,
    ) -> Result<Vec<TabId>, MuxError> {
        let mut mux = self
            .mux
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.terminating.load(Ordering::Acquire) {
            return Err(RuntimeError::Stopped.into());
        }
        mux.reorder_tab(workspace, tab, before)?;
        Ok(mux
            .workspace(workspace)
            .ok_or(MuxError::UnknownWorkspace(workspace))?
            .tabs
            .iter()
            .map(|tab| tab.id)
            .collect())
    }

    fn terminate(&self) -> Result<(), MuxError> {
        self.terminate_with_windows(Vec::new())
    }

    fn terminate_with_windows(
        &self,
        windows: Vec<WindowRestore>,
    ) -> Result<(), MuxError> {
        // A queued spawn must observe termination after it acquires the same
        // mutex, even when it had not started when the native quit hook ran.
        self.terminating.store(true, Ordering::Release);
        let assessment = self.assess(CloseRequest::Application)?;
        assessment.record_cleanup_groups();
        let mut mux = self
            .mux
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.capture(&mux, windows);
        mux.shutdown()
    }
}

// In-memory input for a future restore writer. No serialization/version contract.
#[derive(Clone, Debug)]
#[expect(
    dead_code,
    reason = "retained restore aggregate awaits the persistence stage"
)]
struct WindowRestore {
    attachment: AttachmentId,
    workspace: Option<WorkspaceId>,
    active: Option<TabId>,
    bounds: WindowBounds,
    tab_position: TabPosition,
    sidebar_width: Pixels,
    tab_scroll: Pixels,
}
#[derive(Debug)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "retained restore aggregate awaits the persistence stage"
    )
)]
struct RestoreSnapshot {
    hierarchy: HierarchySnapshot,
    windows: Vec<WindowRestore>,
}

struct Desktop {
    quake: quake_windows::Registry,
    runtime: Arc<DesktopRuntime>,
    config: Config,
    config_path: PathBuf,
    config_error: Option<String>,
    windows: Vec<WeakEntity<WorkspaceView>>,
    keymap: InstalledKeymap,
    frequency: CommandFrequency,
    reloading: bool,
    quitting: bool,
    pending_spawns: usize,
    quit_pending: bool,
    external_drag_window: Option<gpui::WindowId>,
    #[cfg(all(target_os = "macos", feature = "macos-updater"))]
    updater: native_updater::Updater,
}
impl Global for Desktop {}

impl Desktop {
    fn stop_window_drag(window: &mut Window, cx: &mut App) {
        let desktop = cx.global_mut::<Self>();
        if desktop.external_drag_window
            == Some(window.window_handle().window_id())
        {
            desktop.external_drag_window = None;
            cx.stop_active_drag(window);
        }
    }

    /// Runs a catalog command for a programmatic caller.
    ///
    /// Application commands do not require a window, but use a supplied
    /// workspace as the reporter for deferred failures. Window, runtime, and
    /// terminal commands execute synchronously on `window`'s root view and
    /// report [`CommandError::ClientRequired`] when it is absent or closed.
    ///
    /// # Errors
    /// Reports catalog validation failures before any dispatch, then the
    /// executing view's refusal or failure.
    fn invoke(
        cx: &mut App,
        invocation: &CommandInvocation,
        window: Option<AnyWindowHandle>,
    ) -> Result<CommandOutcome, CommandError> {
        let spec = validate(invocation)?;
        match route(spec.scope, window)? {
            Route::Application => {
                let reporter = window.and_then(|handle| {
                    handle
                        .update(cx, |root, _, _| {
                            root.downcast::<WorkspaceView>()
                                .ok()
                                .map(|view| view.downgrade())
                        })
                        .ok()
                        .flatten()
                });
                run_app_command(cx, invocation, reporter)
            }
            Route::Window(handle) => handle
                .update(cx, |root, window, cx| {
                    let view = root
                        .downcast::<WorkspaceView>()
                        .map_err(|_| CommandError::ClientRequired)?;
                    view.update(cx, |view, cx| {
                        view.run_command(invocation, window, cx)
                    })
                })
                .map_err(|_| CommandError::ClientRequired)?,
            Route::Terminal(handle) => handle
                .update(cx, |root, window, cx| {
                    let view = root
                        .downcast::<WorkspaceView>()
                        .map_err(|_| CommandError::ClientRequired)?;
                    let terminal =
                        view.read(cx).active_view().ok_or_else(|| {
                            CommandError::Unavailable(
                                "window has no active terminal".to_owned(),
                            )
                        })?;
                    terminal.update(cx, |terminal, cx| {
                        terminal.run_command(invocation, window, cx)
                    })
                })
                .map_err(|_| CommandError::ClientRequired)?,
        }
    }
}

/// Shows `message` in the active window's status line, if there is one.
fn show_active_window_status(cx: &mut App, message: String) {
    let Some(window) = cx.active_window() else {
        return;
    };
    let _ = window.update(cx, |root, _, cx| {
        if let Ok(view) = root.downcast::<WorkspaceView>() {
            view.update(cx, |view, cx| {
                view.status = Some(message);
                cx.notify();
            });
        }
    });
}

fn report_deferred_failure(
    cx: &mut App,
    reporter: Option<WeakEntity<WorkspaceView>>,
    message: String,
) {
    report_deferred_failure_inner(cx, reporter, message, false);
}

fn report_deferred_failure_with_global_latch(
    cx: &mut App,
    reporter: Option<WeakEntity<WorkspaceView>>,
    message: String,
) {
    let latch_if_dead = reporter.is_some();
    if reporter.is_none() {
        cx.global_mut::<Desktop>().config_error = Some(message.clone());
    }
    report_deferred_failure_inner(cx, reporter, message, latch_if_dead);
}

fn report_deferred_failure_inner(
    cx: &mut App,
    reporter: Option<WeakEntity<WorkspaceView>>,
    message: String,
    latch_if_dead: bool,
) {
    eprintln!("Huterm {message}");
    cx.defer(move |cx| {
        if let Some(reporter) = reporter.and_then(|reporter| reporter.upgrade())
        {
            reporter.update(cx, |view, cx| {
                view.status = Some(message);
                cx.notify();
            });
            return;
        }
        if latch_if_dead {
            cx.global_mut::<Desktop>().config_error = Some(message.clone());
        }
        show_active_window_status(cx, message);
    });
}

fn run_app_command(
    cx: &mut App,
    invocation: &CommandInvocation,
    reporter: Option<WeakEntity<WorkspaceView>>,
) -> Result<CommandOutcome, CommandError> {
    app_command_availability(cx, invocation.id)?;
    match invocation.id {
        ids::NEW_WINDOW => {
            open_window_with_profile(cx, true, None, reporter);
            Ok(CommandOutcome::Accepted)
        }
        ids::SHOW_QUAKE | ids::HIDE_QUAKE | ids::TOGGLE_QUAKE => {
            quake_windows::invoke(cx, invocation, reporter)
        }
        ids::RELOAD_CONFIG => reload(cx),
        ids::CHECK_FOR_UPDATES => check_for_updates(cx),
        ids::QUIT => {
            // Global actions run while the dispatching window is borrowed.
            // Route quit after that window has returned to App's window map.
            cx.defer(request_quit);
            Ok(CommandOutcome::Accepted)
        }
        ids::HIDE => {
            cx.hide();
            Ok(CommandOutcome::Completed)
        }
        ids::HIDE_OTHERS => {
            cx.hide_other_apps();
            Ok(CommandOutcome::Completed)
        }
        ids::SHOW_ALL => {
            cx.unhide_other_apps();
            Ok(CommandOutcome::Completed)
        }
        other => Err(CommandError::UnknownCommand(other)),
    }
}

fn check_for_updates(cx: &mut App) -> Result<CommandOutcome, CommandError> {
    #[cfg(all(target_os = "macos", feature = "macos-updater"))]
    {
        cx.global::<Desktop>()
            .updater
            .check_for_updates()
            .map_err(CommandError::Unavailable)?;
        Ok(CommandOutcome::Accepted)
    }
    #[cfg(not(all(target_os = "macos", feature = "macos-updater")))]
    {
        let _ = cx;
        Err(unsupported_update_error())
    }
}

#[cfg(not(all(target_os = "macos", feature = "macos-updater")))]
fn unsupported_update_error() -> CommandError {
    CommandError::Unavailable(
        "self-updates are available only in updater-enabled macOS release builds"
            .to_owned(),
    )
}

#[cfg(all(target_os = "macos", feature = "macos-updater"))]
fn apply_update_config(cx: &App, config: &Config) {
    if let Err(error) =
        cx.global::<Desktop>().updater.apply_config(config.updates)
    {
        eprintln!("Updater configuration failed: {error}");
    }
}

fn app_command_availability(
    cx: &App,
    command: huterm_protocol::CommandId,
) -> Result<(), CommandError> {
    let desktop = cx.global::<Desktop>();
    match command {
        ids::NEW_WINDOW if desktop.quitting || desktop.quit_pending => Err(
            CommandError::Unavailable("application is quitting".to_owned()),
        ),
        ids::RELOAD_CONFIG if desktop.reloading => {
            Err(CommandError::Unavailable(
                "configuration reload in progress".to_owned(),
            ))
        }
        ids::QUIT if desktop.quitting => Err(CommandError::Unavailable(
            "application quit is already in progress".to_owned(),
        )),
        ids::NEW_WINDOW
        | ids::SHOW_QUAKE
        | ids::HIDE_QUAKE
        | ids::TOGGLE_QUAKE
        | ids::RELOAD_CONFIG
        | ids::CHECK_FOR_UPDATES
        | ids::QUIT
        | ids::HIDE
        | ids::HIDE_OTHERS
        | ids::SHOW_ALL => Ok(()),
        other => Err(CommandError::UnknownCommand(other)),
    }
}

fn interactive_spec(
    invocation: &CommandInvocation,
) -> Result<(&'static huterm_protocol::CommandSpec, bool), CommandError> {
    let spec = validate_supplied(invocation)?;
    let prompt = spec.scope != CommandScope::Palette
        && !spec.missing_prompted(invocation).is_empty();
    Ok((spec, prompt))
}

fn set_dispatch_error<T>(
    palette: &mut Option<T>,
    status: &mut Option<String>,
    error: &CommandError,
) {
    *palette = None;
    *status = Some(error.to_string());
}

/// Binds the startup keymap and returns its reserved keys with the first
/// diagnostic to show: a config error, a keymap error, or binding conflicts.
///
/// A broken binding never blocks startup: defaults apply and the diagnostic
/// shows like any other non-fatal configuration error.
pub(super) fn install_startup_keymap(
    cx: &mut App,
    loaded: &config::LoadedConfig,
) -> (InstalledKeymap, Option<String>) {
    let (compiled, keymap_error) = compile_keymap(&loaded.config);
    let conflicts =
        (!compiled.conflicts.is_empty()).then(|| compiled.conflicts.join("; "));
    let keymap = bind_keymap(cx, compiled);
    let diagnostic = loaded
        .error
        .clone()
        .or_else(|| {
            keymap_error
                .map(|error| format!("{}: {error}", loaded.path.display()))
        })
        .or(conflicts);
    (keymap, diagnostic)
}

pub(super) fn run() -> anyhow::Result<()> {
    run_with_startup(|_| {})
}

pub(super) fn run_with_startup(
    startup: impl FnOnce(&mut App) + 'static,
) -> anyhow::Result<()> {
    let loaded = config::load();
    if loaded.fatal {
        anyhow::bail!(
            "{}",
            loaded
                .error
                .as_deref()
                .unwrap_or("invalid engine configuration")
        );
    }
    if let Some(error) = &loaded.error {
        eprintln!("Huterm configuration error: {error}");
    }
    let runtime = Arc::new(DesktopRuntime::default());
    let app_runtime = Arc::clone(&runtime);
    let application = Application::new();
    application.on_reopen(|cx| {
        if cx.windows().is_empty() {
            open_window(cx);
        }
        cx.activate(true);
    });
    application.run(move |cx| {
        let (keymap, config_error) = install_startup_keymap(cx, &loaded);
        #[cfg(all(target_os = "macos", feature = "macos-updater"))]
        let updater =
            native_updater::Updater::initialize(loaded.config.updates);
        cx.set_global(Desktop {
            quake: quake_windows::Registry::default(),
            runtime: Arc::clone(&app_runtime),
            config: loaded.config,
            config_error,
            config_path: loaded.path,
            windows: Vec::new(),
            keymap,
            frequency: CommandFrequency::default(),
            reloading: false,
            quitting: false,
            pending_spawns: 0,
            quit_pending: false,
            external_drag_window: None,
            #[cfg(all(target_os = "macos", feature = "macos-updater"))]
            updater,
        });
        install_native_quit(cx);
        quake_windows::install(cx);
        cx.on_app_quit(move |cx| {
            quake_windows::shutdown(cx);
            // AppKit terminate: does not return from Application::run. GPUI
            // allows only 100 ms for quit futures, so this terminal hook must
            // finish synchronous cleanup before returning its empty future.
            if let Err(error) =
                app_runtime.terminate_with_windows(capture_windows(cx))
            {
                eprintln!("Native quit cleanup failed: {error}");
            }
            async {}
        })
        .detach();
        cx.on_action(|action: &InvokeApp, cx| {
            if let Err(error) = Desktop::invoke(cx, &action.0, None) {
                let message =
                    format!("Command `{}` failed: {error}", action.0.id);
                eprintln!("Huterm {message}");
                // Global action callbacks run while the dispatching window
                // is borrowed, so update its status after it is returned.
                cx.defer(move |cx| show_active_window_status(cx, message));
            }
        });
        cx.on_window_closed(|cx| {
            cx.global_mut::<Desktop>()
                .windows
                .retain(|view| view.upgrade().is_some());
            maybe_exit(cx);
        })
        .detach();
        cx.observe_keystrokes(observe_keystroke).detach();
        open_window(cx);
        cx.activate(true);
        startup(cx);
    });
    // Backends whose event loop returns get the same idempotent cleanup.
    runtime.terminate()?;
    Ok(())
}

fn observe_keystroke(
    event: &gpui::KeystrokeEvent,
    window: &mut Window,
    cx: &mut App,
) {
    let reserved = Arc::clone(&cx.global::<Desktop>().keymap.reserved);
    let consumed =
        event.action.is_some() || reserved.is_reserved(&event.keystroke);
    if let Some(root) = window.root::<WorkspaceView>().flatten() {
        root.update(cx, |view, cx| {
            let input_blocked = view.busy
                || view.close.confirmation.is_some()
                || view.reorder.is_some()
                || view.palette.is_some();
            if let Some(tab) = view.active_view() {
                tab.update(cx, |tab, cx| {
                    #[cfg(target_os = "macos")]
                    if let Some(action) = &event.action {
                        tab.pending_shortcuts
                            .observe_action(&event.keystroke, action.as_ref());
                    }
                    if consumed {
                        tab.clear_option_composition();
                        return;
                    }
                    if input_blocked {
                        return;
                    }
                    if tab.handle_keystroke(&event.keystroke, &reserved, window)
                    {
                        cx.stop_propagation();
                        cx.notify();
                    }
                    tab.start_snapshot_if_needed(cx);
                });
            }
        });
    }
}

fn request_quit(cx: &mut App) {
    if cx.global::<Desktop>().pending_spawns > 0 {
        cx.global_mut::<Desktop>().quit_pending = true;
        return;
    }
    if let Some(handle) = cx
        .active_window()
        .or_else(|| cx.windows().into_iter().next())
    {
        let _ = handle.update(cx, |root, window, cx| {
            if let Ok(view) = root.downcast::<WorkspaceView>() {
                window.activate_window();
                cx.activate(true);
                view.update(cx, |view, cx| {
                    view.request_close(CloseTarget::Application, window, cx);
                });
            }
        });
    } else {
        // Give unviewed sessions a confirmation host without creating a shell.
        open_window_inner(cx, false);
        if cx.windows().is_empty() {
            #[cfg(target_os = "macos")]
            native_quit::cancel_request();
        } else {
            cx.defer(request_quit);
        }
    }
}

fn capture_windows(cx: &App) -> Vec<WindowRestore> {
    capture_other_windows(cx, None)
}

fn capture_other_windows(
    cx: &App,
    except: Option<gpui::EntityId>,
) -> Vec<WindowRestore> {
    cx.global::<Desktop>()
        .windows
        .iter()
        .filter_map(WeakEntity::upgrade)
        .filter(|view| Some(view.entity_id()) != except)
        .filter_map(|view| view.read(cx).restore_window())
        .collect()
}

fn maybe_exit(cx: &mut App) {
    if quake_windows::keep_alive(cx) {
        return;
    }
    if !cx.windows().is_empty() || cx.global::<Desktop>().pending_spawns != 0 {
        return;
    }
    let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
    let task = cx.background_executor().spawn(async move {
        runtime
            .mux
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sessions()
            .is_empty()
    });
    cx.spawn(async move |cx| {
        if task.await {
            let _ = cx.update(|cx| {
                if cx.windows().is_empty()
                    && !quake_windows::keep_alive(cx)
                    && cx.global::<Desktop>().pending_spawns == 0
                {
                    approved_quit(cx);
                }
            });
        }
    })
    .detach();
}

fn approved_quit(cx: &mut App) {
    cx.spawn(async move |cx| {
        let mut quake_presentations = Vec::new();
        #[cfg(target_os = "macos")]
        let mut adapters = Vec::new();
        let _ = cx.update(|cx| {
            quake_presentations = quake_windows::take_for_quit(cx);
            quake_windows::shutdown(cx);
            for view in cx.global::<Desktop>().windows.clone() {
                let _ = view.update(cx, |view, _| {
                    view.fullscreen.close();
                    #[cfg(target_os = "macos")]
                    if let Some(adapter) = view.native_fullscreen.take() {
                        adapter.close_gate();
                        adapters.push(adapter);
                    }
                });
            }
        });
        for state in quake_presentations {
            state.cleanup();
        }
        #[cfg(target_os = "macos")]
        for adapter in adapters {
            adapter.close();
        }
        let _ = cx.update(|cx| {
            #[cfg(target_os = "macos")]
            native_quit::allow_termination();
            cx.quit();
        });
    })
    .detach();
}

#[cfg(target_os = "macos")]
fn install_native_quit(cx: &mut App) {
    let requests = native_quit::install()
        .expect("cannot install cancellable native termination hook");
    cx.spawn(async move |cx| {
        while requests.recv().await.is_ok() {
            let _ = cx.update(|cx| cx.defer(request_quit));
        }
    })
    .detach();
}

#[cfg(not(target_os = "macos"))]
fn install_native_quit(_: &mut App) {}

fn initial_window_size(
    config: &Config,
    metrics: GridMetrics,
) -> gpui::Size<Pixels> {
    size(
        metrics.cell_width * f32::from(INITIAL_COLUMNS)
            + px(config.window.padding_x * 2.0)
            + if config.window.always_show_tab_bar
                && config.window.tab_position.vertical()
            {
                SIDEBAR_WIDTH
            } else {
                px(0.0)
            },
        metrics.cell_height * f32::from(INITIAL_ROWS)
            + px(config.window.padding_y * 2.0)
            + titlebar_inset(cfg!(target_os = "macos"), false)
            + if !config.window.always_show_tab_bar
                || config.window.tab_position.vertical()
            {
                px(0.0)
            } else {
                TAB_HEIGHT
            },
    )
}

fn can_open_window(
    launch_shell: bool,
    quitting: bool,
    quit_pending: bool,
) -> bool {
    !launch_shell || (!quitting && !quit_pending)
}

fn open_window(cx: &mut App) {
    open_window_inner(cx, true);
}

fn open_window_inner(cx: &mut App, launch_shell: bool) {
    open_window_with_profile(cx, launch_shell, None, None);
}

#[expect(
    clippy::too_many_lines,
    reason = "native window creation installs lifecycle and event pump"
)]
fn open_window_with_profile(
    cx: &mut App,
    launch_shell: bool,
    profile: Option<(String, crate::quake::Profile, crate::quake::Display)>,
    reporter: Option<WeakEntity<WorkspaceView>>,
) {
    if !can_open_window(
        launch_shell,
        cx.global::<Desktop>().quitting,
        cx.global::<Desktop>().quit_pending,
    ) {
        return;
    }
    let config = cx.global::<Desktop>().config.clone();
    let (family, metrics) = match resolve_metrics(&config, cx) {
        Ok(value) => value,
        Err(error) => {
            report_deferred_failure(
                cx,
                reporter,
                format!("Cannot open window: {error}"),
            );
            maybe_exit(cx);
            return;
        }
    };
    let bounds =
        Bounds::centered(None, initial_window_size(&config, metrics), cx);
    let result = cx.open_window(
        WindowOptions {
            show: profile.is_none(),
            focus: profile.is_none(),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Huterm".into()),
                appears_transparent: cfg!(target_os = "macos"),
                ..TitlebarOptions::default()
            }),
            window_min_size: Some(size(px(280.0), px(180.0))),
            app_id: Some(APP_ID.into()),
            ..WindowOptions::default()
        },
        |window, cx| {
            let scaled_metrics = metrics.at_scale(window.scale_factor());
            if scaled_metrics != metrics {
                window.resize(initial_window_size(&config, scaled_metrics));
            }
            let profile_requested = profile.is_some();
            let quake = profile.and_then(|(name, profile, display)| {
                quake_windows::attach(
                    name,
                    profile,
                    display,
                    reporter.clone(),
                    window,
                    cx,
                )
                .inspect_err(|error| eprintln!("Quake creation: {error}"))
                .ok()
            });
            let view = cx.new(|cx| WorkspaceView {
                quake,
                attachment: None,
                bounds: window.window_bounds(),
                fullscreen_insets: gpui::Edges::default(),
                fullscreen: FullscreenController::new(
                    window.window_bounds(),
                    config.window.macos_fullscreen_mode,
                    cfg!(target_os = "macos"),
                ),
                #[cfg(target_os = "macos")]
                native_fullscreen: crate::native_fullscreen::Adapter::new(
                    window, cx,
                )
                .inspect_err(|error| eprintln!("Fullscreen observer: {error}"))
                .ok(),
                workspace: None,
                tabs: Vec::new(),
                active: None,
                history: Vec::new(),
                tab_scroll: px(0.0),
                scroll_target: None,
                last_scroll: Instant::now(),
                sidebar_width: SIDEBAR_WIDTH,
                resizing_sidebar: false,
                reveal: Reveal::default(),
                reveal_context: None,
                reorder: None,
                config,
                family,
                metrics: scaled_metrics,
                focus: cx.focus_handle(),
                busy: false,
                close: CloseState::default(),
                exited_tabs: ExitQueue::default(),
                status: cx.global::<Desktop>().config_error.clone(),
                palette: None,
                palette_refresh_state: None,
                recent: RecentCommands::default(),
                startup_reporter: reporter.clone(),
            });
            let weak = view.downgrade();
            window.on_window_should_close(cx, move |window, cx| {
                let _ = weak.update(cx, |view, cx| {
                    view.request_close(CloseTarget::Window, window, cx);
                });
                false
            });
            cx.global_mut::<Desktop>().windows.push(view.downgrade());
            view.update(cx, |view, cx| {
                view.focus.focus(window);
                if launch_shell
                    && (!profile_requested || view.quake.is_some())
                    && let Err(error) = view.new_tab(window, cx)
                {
                    view.status = Some(error.to_string());
                }
            });
            if profile_requested && view.read(cx).quake.is_none() {
                let handle = window.window_handle();
                cx.defer(move |cx| {
                    let _ = handle
                        .update(cx, |_, window, _| window.remove_window());
                });
            }
            let pump_view = view.downgrade();
            let pump_window = window.window_handle();
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                    if pump_window
                        .update(cx, |_, window, cx| {
                            let _ = pump_view.update(cx, |view, cx| {
                                view.refresh_fullscreen(window, cx);
                                view.refresh_tab_visibility(window, cx);
                                if view
                                    .advance_tab_scroll(Instant::now(), window)
                                {
                                    cx.notify();
                                }
                                let mut metadata_changed = false;
                                for tab in &view.tabs {
                                    tab.view.update(cx, |terminal, cx| {
                                        let previous = (
                                            terminal.title.clone(),
                                            terminal.exited,
                                        );
                                        terminal.refresh(cx);
                                        view.exited_tabs.observe(
                                            tab.id,
                                            previous.1,
                                            terminal.exited,
                                            view.config.terminal.close_on_exit,
                                        );
                                        metadata_changed |= previous
                                            != (
                                                terminal.title.clone(),
                                                terminal.exited,
                                            );
                                    });
                                }
                                view.resume_close(window, cx);
                                view.refresh_palette(cx);
                                if metadata_changed {
                                    cx.notify();
                                }
                            });
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
            view
        },
    );
    if let Err(error) = result {
        report_deferred_failure(
            cx,
            reporter,
            format!("Cannot open window: {error}"),
        );
        maybe_exit(cx);
    }
}

struct TabView {
    id: TabId,
    record: huterm_core::Tab,
    view: Entity<TerminalView>,
}

impl TabView {
    fn title(&self, cx: &App) -> String {
        let terminal = self.view.read(cx);
        let title = self.record.display_name(&terminal.title).to_owned();
        if terminal.exited {
            format!("{title} · exited")
        } else {
            title
        }
    }
}

struct WorkspaceView {
    quake: Option<quake_windows::Presentation>,
    attachment: Option<AttachmentId>,
    bounds: WindowBounds,
    fullscreen: FullscreenController,
    fullscreen_insets: gpui::Edges<Pixels>,
    #[cfg(target_os = "macos")]
    native_fullscreen: Option<crate::native_fullscreen::Adapter>,
    workspace: Option<WorkspaceId>,
    tabs: Vec<TabView>,
    active: Option<TabId>,
    history: Vec<TabId>,
    tab_scroll: Pixels,
    scroll_target: Option<Pixels>,
    last_scroll: Instant,
    sidebar_width: Pixels,
    resizing_sidebar: bool,
    reveal: Reveal,
    reveal_context: Option<(TabPosition, bool, bool)>,
    reorder: Option<TabReorder>,
    config: Config,
    family: String,
    metrics: GridMetrics,
    focus: FocusHandle,
    busy: bool,
    close: CloseState,
    exited_tabs: ExitQueue,
    status: Option<String>,
    palette: Option<Entity<CommandPalette>>,
    palette_refresh_state: Option<PaletteRefreshState>,
    recent: RecentCommands,
    startup_reporter: Option<WeakEntity<WorkspaceView>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PaletteRefreshState {
    tabs: usize,
    active: Option<TabId>,
    busy: bool,
    confirming: bool,
    reordering: bool,
    quake: Option<(bool, bool)>,
}

impl Drop for WorkspaceView {
    fn drop(&mut self) {
        self.fullscreen.close();
        #[cfg(target_os = "macos")]
        if let Some(adapter) = self.native_fullscreen.take() {
            adapter.schedule_close();
        }
    }
}

#[derive(Default)]
struct ExitQueue(std::collections::VecDeque<TabId>);

impl ExitQueue {
    fn observe(
        &mut self,
        tab: TabId,
        was_exited: bool,
        exited: bool,
        enabled: bool,
    ) {
        if enabled && !was_exited && exited {
            self.0.push_back(tab);
        }
    }

    fn take_next(&mut self, contains: impl Fn(TabId) -> bool) -> Option<TabId> {
        while let Some(tab) = self.0.pop_front() {
            if contains(tab) {
                return Some(tab);
            }
        }
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TabDragSource {
    workspace: WorkspaceId,
    window: gpui::WindowId,
    tab: TabId,
}

struct TabReorder {
    source: TabDragSource,
    origin: gpui::Point<Pixels>,
    pointer: gpui::Point<Pixels>,
    dragging: bool,
    original_scroll: Pixels,
    strip: TabStrip,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CloseTarget {
    Tab(TabId),
    Window,
    Application,
}

fn merge_close(
    pending: Option<CloseTarget>,
    requested: CloseTarget,
) -> CloseTarget {
    match (pending, requested) {
        (Some(CloseTarget::Application), _) | (_, CloseTarget::Application) => {
            CloseTarget::Application
        }
        (Some(CloseTarget::Window), _) | (_, CloseTarget::Window) => {
            CloseTarget::Window
        }
        (_, requested) => requested,
    }
}

#[derive(Default)]
struct CloseState {
    current: Option<CloseTarget>,
    pending: Option<CloseTarget>,
    confirmation: Option<CloseTarget>,
    assessment: Option<CloseAssessment>,
    generation: u64,
}

#[derive(Debug, Eq, PartialEq)]
enum CloseDecision {
    Check(CloseTarget),
    Confirm(CloseTarget),
    Close(CloseTarget),
}

impl CloseState {
    fn next_request(
        &mut self,
        exits: &mut ExitQueue,
        busy: bool,
        contains: impl Fn(TabId) -> bool,
    ) -> Option<CloseTarget> {
        if busy || self.confirmation.is_some() {
            return None;
        }
        self.take_pending(&contains)
            .or_else(|| exits.take_next(contains).map(CloseTarget::Tab))
    }

    fn begin_check(&mut self, target: CloseTarget) -> CloseTarget {
        self.generation += 1;
        self.assessment = None;
        let target = merge_close(self.confirmation.take(), target);
        self.current = Some(target);
        target
    }
    fn queue(&mut self, target: CloseTarget) {
        self.pending =
            Some(merge_close(self.pending, merge_close(self.current, target)));
    }
    fn checked(&mut self, foreground: bool) -> Option<CloseDecision> {
        let target = self.current.take()?;
        let pending = self.pending.take();
        let effective = merge_close(pending, target);
        if effective != target {
            return Some(CloseDecision::Check(effective));
        }
        // A second, different tab close follows the first; wider requests
        // subsume narrower requests and repeated closes of one tab coalesce.
        if matches!((target, pending), (CloseTarget::Tab(first), Some(CloseTarget::Tab(second))) if first != second)
        {
            self.pending = pending;
        }
        if foreground {
            self.confirmation = Some(target);
            Some(CloseDecision::Confirm(target))
        } else {
            Some(CloseDecision::Close(target))
        }
    }
    fn cancel(&mut self) -> Option<CloseTarget> {
        self.generation += 1;
        self.assessment = None;
        let mut target = self.confirmation.take();
        for next in [self.current.take(), self.pending.take()]
            .into_iter()
            .flatten()
        {
            target = Some(merge_close(target, next));
        }
        target
    }
    fn take_pending(
        &mut self,
        contains: impl FnOnce(TabId) -> bool,
    ) -> Option<CloseTarget> {
        self.pending.take().filter(|target| match target {
            CloseTarget::Tab(id) => contains(*id),
            _ => true,
        })
    }
}

fn apply_tab_order<T>(
    tabs: &mut [T],
    order: &[TabId],
    id: impl Fn(&T) -> TabId,
) -> bool {
    if tabs.len() != order.len()
        || order
            .iter()
            .enumerate()
            .any(|(index, tab)| order[..index].contains(tab))
        || tabs.iter().any(|tab| !order.contains(&id(tab)))
    {
        return false;
    }
    tabs.sort_by_key(|tab| {
        order
            .iter()
            .position(|candidate| *candidate == id(tab))
            .unwrap_or(usize::MAX)
    });
    true
}

// Update navigation together with removal, before another close can interrupt
// completion or open a confirmation dialog.
fn remove_tab<T>(
    tabs: &mut Vec<T>,
    active: &mut Option<TabId>,
    closed: TabId,
    id: impl Fn(&T) -> TabId,
) {
    let Some(index) = tabs.iter().position(|tab| id(tab) == closed) else {
        return;
    };
    tabs.remove(index);
    if *active == Some(closed) {
        *active = tabs.get(index.min(tabs.len().saturating_sub(1))).map(id);
    }
}

fn record_tab_activation(history: &mut Vec<TabId>, id: TabId) {
    history.retain(|recorded| *recorded != id);
    history.insert(0, id);
}

fn prune_tab_history(history: &mut Vec<TabId>, id: TabId) {
    history.retain(|recorded| *recorded != id);
}

fn recent_tab(
    history: &[TabId],
    active: Option<TabId>,
    tabs: impl Fn(TabId) -> bool,
) -> Option<TabId> {
    history
        .iter()
        .copied()
        .find(|id| Some(*id) != active && tabs(*id))
}

impl WorkspaceView {
    fn presentation(&self) -> Presentation {
        Presentation::resolve(
            self.tabs.len(),
            self.config.window.always_show_tab_bar,
            self.fullscreen_context(),
            self.config.window.auto_hide_tab_bar_in_fullscreen,
        )
    }

    fn chrome_layout(&self, window: &Window) -> ChromeLayout {
        ChromeLayout::with_safe_area(
            window.viewport_size(),
            terminal_top(self.chrome_hidden()),
            self.config.window.tab_position,
            self.sidebar_width,
            self.fullscreen_insets,
        )
        .present(
            self.presentation(),
            self.config.window.tab_position,
            self.reveal.progress,
        )
    }

    fn sync_tab_layout(&self, window: &Window, cx: &mut Context<'_, Self>) {
        let presentation = self.presentation();
        let chrome_hidden = self.chrome_hidden();
        let overlay = (presentation == Presentation::Overlay
            && self.reveal.progress > 0.0)
            .then(|| {
                let layout = self.chrome_layout(window);
                layout.tabs.intersect(&layout.terminal)
            });
        let mut changed_any = false;
        for tab in &self.tabs {
            tab.view.update(cx, |terminal, cx| {
                let changed = terminal.tab_presentation != presentation
                    || terminal.sidebar_width != self.sidebar_width
                    || terminal.chrome_hidden != chrome_hidden
                    || terminal.fullscreen_insets != self.fullscreen_insets;
                changed_any |= changed;
                terminal.tab_overlay = overlay;
                terminal.tab_presentation = presentation;
                terminal.sidebar_width = self.sidebar_width;
                terminal.chrome_hidden = chrome_hidden;
                terminal.fullscreen_insets = self.fullscreen_insets;
                if changed {
                    terminal.resize_if_needed(window);
                    cx.notify();
                }
            });
        }
        if changed_any {
            cx.notify();
        }
    }

    fn refresh_tab_visibility(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let position = self.config.window.tab_position;
        let context = (
            position,
            self.fullscreen_context(),
            self.config.window.auto_hide_tab_bar_in_fullscreen,
        );
        if self.reveal_context != Some(context) {
            self.reveal = Reveal::default();
            self.reveal_context = Some(context);
            cx.notify();
        }
        let enabled = self.presentation() == Presentation::Overlay
            && self.quake_visible()
            && window.is_window_active()
            && self.close.confirmation.is_none();
        let gesture = self
            .active_view()
            .is_some_and(|tab| tab.read(cx).owns_pointer_gesture());
        let layout = self.chrome_layout(window);
        let pointer = window.mouse_position();
        let bounds = layout.terminal;
        let hovered = window.is_window_hovered();
        #[cfg(target_os = "macos")]
        let hovered = hovered
            && self.native_fullscreen.as_ref().is_none_or(
                crate::native_fullscreen::Adapter::pointer_on_display,
            );
        let inside = hovered && bounds.contains(&pointer);
        let edge = inside
            && match position {
                TabPosition::Top => pointer.y <= bounds.origin.y + px(2.0),
                TabPosition::Bottom => pointer.y >= bounds.bottom() - px(2.0),
                TabPosition::Left => pointer.x <= bounds.origin.x + px(2.0),
                TabPosition::Right => pointer.x >= bounds.right() - px(2.0),
            };
        #[cfg(target_os = "macos")]
        let edge = edge
            || (position == TabPosition::Top
                && self.native_fullscreen.as_ref().is_some_and(
                    crate::native_fullscreen::Adapter::pointer_in_top_edge,
                ));
        let hover = !gesture
            && (edge
                || (self.reveal.progress > 0.0
                    && hovered
                    && layout.tabs.contains(&pointer))
                || self.reorder.is_some()
                || self.resizing_sidebar);
        if self
            .reveal
            .advance(Instant::now(), hover, enabled && !gesture)
        {
            cx.notify();
        }
        self.sync_tab_layout(window, cx);
    }

    fn tab_strip(&self, window: &Window) -> TabStrip {
        let position = self.config.window.tab_position;
        let layout = self.chrome_layout(window);
        TabStrip::new(
            layout.tabs,
            position.vertical(),
            self.tabs.len(),
            self.tab_scroll,
        )
    }

    fn reveal_active(&mut self, window: &Window) {
        self.scroll_target = None;
        if let Some(index) =
            self.tabs.iter().position(|tab| Some(tab.id) == self.active)
        {
            self.tab_scroll = self.tab_strip(window).reveal(index);
        }
    }

    fn scroll_tabs(
        &mut self,
        delta: Pixels,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.scroll_target = None;
        let strip = self.tab_strip(window);
        self.tab_scroll =
            (strip.offset + delta).clamp(px(0.0), strip.max_offset());
        cx.notify();
    }

    fn resize_sidebar(
        &mut self,
        pointer: gpui::Point<Pixels>,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let desired = if self.config.window.tab_position == TabPosition::Left {
            pointer.x
        } else {
            window.viewport_size().width - pointer.x
        };
        self.sidebar_width = desired.clamp(px(140.0), px(400.0));
        for tab in &self.tabs {
            tab.view.update(cx, |view, cx| {
                view.sidebar_width = self.sidebar_width;
                if view.visible {
                    view.resize_if_needed(window);
                }
                cx.notify();
            });
        }
        cx.notify();
    }

    fn can_reorder(
        &self,
        source: TabDragSource,
        window: &Window,
        cx: &App,
    ) -> bool {
        source.window == window.window_handle().window_id()
            && Some(source.workspace) == self.workspace
            && self.tabs.iter().any(|tab| tab.id == source.tab)
            && !self.busy
            && self.close.confirmation.is_none()
            && !cx.global::<Desktop>().quitting
            && !cx.global::<Desktop>().quit_pending
    }

    fn begin_reorder(
        &mut self,
        tab: TabId,
        pointer: gpui::Point<Pixels>,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(workspace) = self.workspace else {
            return;
        };
        let source = TabDragSource {
            workspace,
            window: window.window_handle().window_id(),
            tab,
        };
        if !self.can_reorder(source, window, cx) {
            return;
        }
        self.scroll_target = None;
        self.reorder = Some(TabReorder {
            source,
            origin: pointer,
            pointer,
            dragging: false,
            original_scroll: self.tab_scroll,
            strip: self.tab_strip(window),
        });
        cx.notify();
    }

    fn update_reorder(
        &mut self,
        pointer: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(source) = self.reorder.as_ref().map(|drag| drag.source) else {
            return;
        };
        if !self.can_reorder(source, window, cx) {
            self.cancel_reorder(window, cx);
            self.resizing_sidebar = false;
            return;
        }
        let strip = self.tab_strip(window);
        if let Some(drag) = &mut self.reorder {
            drag.pointer = pointer;
            if !drag.dragging
                && (pointer - drag.origin).magnitude() > TAB_DRAG_THRESHOLD
            {
                drag.dragging = true;
            }
            drag.strip = strip;
        }
        cx.notify();
    }

    fn advance_tab_scroll(&mut self, now: Instant, window: &Window) -> bool {
        let elapsed = now.saturating_duration_since(self.last_scroll);
        self.last_scroll = now;
        let strip = self.tab_strip(window);
        let next = if let Some(drag) = &mut self.reorder {
            drag.strip = strip;
            if !drag.dragging {
                return false;
            }
            drag.strip.autoscroll(drag.pointer, elapsed)
        } else if let Some(target) = self.scroll_target {
            let target = target.clamp(px(0.0), strip.max_offset());
            let next = strip.toward(target, elapsed);
            if next == target {
                self.scroll_target = None;
            }
            next
        } else {
            return false;
        };
        let changed = self.tab_scroll != next;
        self.tab_scroll = next;
        changed
    }

    fn restore_tab_focus(&self, window: &mut Window, cx: &App) {
        if let Some(tab) = self.active_view() {
            tab.read(cx).focus.focus(window);
        }
    }

    fn cancel_reorder(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(drag) = self.reorder.take() {
            self.tab_scroll = drag.original_scroll;
            self.restore_tab_focus(window, cx);
            cx.notify();
        }
    }

    fn finish_reorder(
        &mut self,
        pointer: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.update_reorder(pointer, window, cx);
        let Some(drag) = self.reorder.take() else {
            return;
        };
        if !self.can_reorder(drag.source, window, cx) {
            self.restore_tab_focus(window, cx);
            return;
        }
        if !drag.dragging {
            self.select(drag.source.tab, window, cx);
            return;
        }
        let before = self.tabs.get(drag.strip.slot(pointer)).map(|tab| tab.id);
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        self.busy = true;
        let task = cx.background_executor().spawn(async move {
            runtime.reorder_tab(drag.source.workspace, drag.source.tab, before)
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |view, window, cx| {
                view.busy = false;
                match result {
                    Ok(order) => {
                        if !apply_tab_order(&mut view.tabs, &order, |tab| {
                            tab.id
                        }) {
                            view.status = Some(
                                "Tab order changed before reorder completed"
                                    .into(),
                            );
                        }
                    }
                    Err(error) => {
                        view.status =
                            Some(format!("Cannot reorder tab: {error}"));
                    }
                }
                view.restore_tab_focus(window, cx);
                view.resume_close(window, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn active_view(&self) -> Option<Entity<TerminalView>> {
        self.tabs
            .iter()
            .find(|tab| Some(tab.id) == self.active)
            .map(|tab| tab.view.clone())
    }

    #[expect(
        clippy::too_many_lines,
        reason = "asynchronous tab creation owns publication and orphan cleanup"
    )]
    fn new_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.cancel_reorder(window, cx);
        self.resizing_sidebar = false;
        self.check_new_tab_available(cx)?;
        let command = shell_command(
            self.metrics.at_scale(window.scale_factor()),
            cx.global::<Desktop>().config.engine,
        )
        .map_err(|error| CommandError::Runtime(error.to_string()))?;
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let workspace = self.workspace;
        self.busy = true;
        cx.global_mut::<Desktop>().pending_spawns += 1;
        let task = cx
            .background_executor()
            .spawn(async move { runtime.open_tab(workspace, &command) });
        let cleanup_runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let app = cx.to_async();
        cx.spawn_in(window, async move |view, cx| {
            let result: Result<
                (SessionId, WorkspaceId, OpenedTab, Option<AttachmentId>),
                huterm_core::MuxError,
            > = task.await;
            let _ = app.update(|cx| {
                cx.global_mut::<Desktop>().pending_spawns -= 1;
                if cx.global::<Desktop>().pending_spawns == 0
                    && cx.global::<Desktop>().quit_pending
                {
                    cx.global_mut::<Desktop>().quit_pending = false;
                    cx.defer(|cx| invoke(ids::QUIT).dispatch(cx));
                }
            });
            let mut result = Some(result);
            let update = view.update_in(cx, |view, window, cx| {
                view.busy = false;
                if let Some(result) = result.take() {
                    match result {
                        Ok((_, id, opened, attachment)) => {
                            view.startup_reporter = None;
                            if let Some(attachment) = attachment {
                                view.attachment = Some(attachment);
                            }
                            view.workspace = Some(id);
                            let terminal = cx.new(|cx| {
                                TerminalView::new(
                                    opened.client,
                                    &view.config,
                                    view.family.clone(),
                                    view.metrics
                                        .at_scale(window.scale_factor()),
                                    window,
                                    cx,
                                )
                            });
                            let tab_id = opened.tab.id;
                            view.tabs.push(TabView {
                                id: tab_id,
                                record: opened.tab,
                                view: terminal,
                            });
                            view.select(tab_id, window, cx);
                            view.reveal_tab_activity(window, cx);
                            if let Some(state) = &view.quake {
                                let desktop = cx.global_mut::<Desktop>();
                                if desktop
                                    .quake
                                    .failed_spawn
                                    .as_ref()
                                    .is_some_and(|(name, _)| {
                                        name == &state.name
                                    })
                                    && let Some((_, message)) =
                                        desktop.quake.failed_spawn.take()
                                    && desktop.config_error.as_deref()
                                        == Some(message.as_str())
                                {
                                    desktop.config_error = None;
                                }
                            }
                            view.status =
                                cx.global::<Desktop>().config_error.clone();
                        }
                        Err(error) => {
                            view.status =
                                Some(format!("Cannot open tab: {error}"));
                            if let Some(reporter) = view.startup_reporter.take()
                            {
                                report_deferred_failure(
                                    cx,
                                    Some(reporter),
                                    view.status.clone().unwrap_or_default(),
                                );
                            }
                            if view.quake.is_some() && view.tabs.is_empty() {
                                cx.global_mut::<Desktop>()
                                    .config_error
                                    .clone_from(&view.status);
                                if let Some(state) = &view.quake {
                                    cx.global_mut::<Desktop>()
                                        .quake
                                        .failed_spawn = Some((
                                        state.name.clone(),
                                        view.status.clone().unwrap_or_default(),
                                    ));
                                }
                                eprintln!("Cannot start quake shell: {error}");
                                view.remove_window(window, cx, false);
                                return;
                            }
                        }
                    }
                }
                view.resume_close(window, cx);
                cx.notify();
            });
            if update.is_err()
                && let Some(Ok((session, workspace, opened, attachment))) =
                    result
            {
                cx.background_executor()
                    .spawn(async move {
                        cleanup_runtime.cleanup_spawn(
                            session,
                            workspace,
                            opened.tab.id,
                            attachment,
                        );
                    })
                    .await;
            }
        })
        .detach();
        cx.notify();
        Ok(CommandOutcome::Accepted)
    }

    fn invoke_interactive(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        let (spec, prompt) = interactive_spec(invocation)?;
        if prompt {
            return self.open_palette_request(Some(invocation), window, cx);
        }
        validate(invocation)?;
        match spec.scope {
            CommandScope::Application => {
                let reporter = cx.entity().downgrade();
                run_app_command(cx, invocation, Some(reporter))
            }
            CommandScope::Window | CommandScope::Runtime => {
                self.run_command(invocation, window, cx)
            }
            CommandScope::Palette => self
                .palette
                .clone()
                .ok_or_else(|| {
                    CommandError::Unavailable(
                        "command palette is not open".to_owned(),
                    )
                })?
                .update(cx, |palette, cx| palette.run(invocation, window, cx)),
            CommandScope::Terminal => Err(CommandError::ClientRequired),
        }
    }

    fn invoke_palette(
        &mut self,
        action: &InvokePalette,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        cx.stop_propagation();
        if self.palette.is_none() {
            return;
        }
        if let Err(error) = self.invoke_interactive(&action.0, window, cx)
            && let Some(palette) = self.palette.clone()
        {
            palette.update(cx, |palette, cx| {
                palette.set_error(error.to_string(), cx);
            });
        }
    }

    fn open_palette(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.open_palette_request(None, window, cx)
    }

    fn open_palette_request(
        &mut self,
        request: Option<&CommandInvocation>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        if let Some(palette) = &self.palette {
            palette.read(cx).focus_handle(cx).focus(window);
            cx.notify();
            return Ok(CommandOutcome::Completed);
        }
        self.check_available(true)?;
        if cx.global::<Desktop>().quitting
            || cx.global::<Desktop>().quit_pending
        {
            return Err(CommandError::Unavailable(
                "application is quitting".to_owned(),
            ));
        }

        let terminal = self.active_view();
        if let Some(terminal) = &terminal {
            terminal.update(cx, TerminalView::clear_composition);
        }
        let target = PaletteTarget {
            session: None,
            workspace: self.workspace,
            tab: self.active,
            terminal: self
                .tabs
                .iter()
                .find(|tab| Some(tab.id) == self.active)
                .map(|tab| tab.record.terminal_id),
            terminal_view: terminal.map(|terminal| terminal.downgrade()),
            contexts: window.context_stack(),
        };
        let availability = self.palette_availability(&target, cx);
        let foreground = color(self.config.theme.foreground);
        let background = color(self.config.theme.background);
        let keymap = cx.global::<Desktop>().keymap.clone();
        let palette = cx.new(|cx| {
            CommandPalette::new_with_request(
                target,
                keymap,
                availability,
                foreground,
                background,
                request,
                cx,
            )
        });
        cx.subscribe_in(&palette, window, Self::handle_palette_event)
            .detach();
        self.palette = Some(palette.clone());
        self.palette_refresh_state = Some(self.current_palette_refresh_state());
        palette.read(cx).focus_handle(cx).focus(window);

        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let workspace = self.workspace;
        let tab = self.active;
        let live_titles = self
            .tabs
            .iter()
            .map(|tab| (tab.id, tab.title(cx)))
            .collect();
        let task = cx
            .background_executor()
            .spawn(async move { runtime.palette_snapshot(workspace, tab) });
        cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |view, cx| match result {
                Ok(Some((target, hierarchy))) => {
                    palette.update(cx, |palette, cx| {
                        palette.set_hierarchy(
                            target,
                            &hierarchy,
                            &live_titles,
                            cx,
                        );
                    });
                    view.palette_refresh_state = None;
                    view.refresh_palette(cx);
                }
                Ok(None) => {}
                Err(error) => palette.update(cx, |palette, cx| {
                    palette.set_error(
                        format!("Cannot load command targets: {error}"),
                        cx,
                    );
                }),
            });
        })
        .detach();
        cx.notify();
        Ok(CommandOutcome::Completed)
    }

    fn handle_palette_event(
        &mut self,
        palette: &Entity<CommandPalette>,
        event: &PaletteEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match event {
            PaletteEvent::Cancel => {
                let target = palette.read(cx).target.clone();
                self.palette = None;
                self.palette_refresh_state = None;
                self.restore_palette_focus(&target, window, cx);
                cx.notify();
            }
            PaletteEvent::Execute(invocation) => {
                let target = palette.read(cx).target.clone();
                if let Err(error) = self
                    .command_availability(invocation.id, &target, cx)
                    .and_then(|()| validate(invocation).map(|_| ()))
                {
                    palette.update(cx, |palette, cx| {
                        palette.set_error(error.to_string(), cx);
                    });
                    return;
                }
                self.palette = None;
                self.palette_refresh_state = None;
                self.restore_palette_focus(&target, window, cx);
                let reporter = cx.entity().downgrade();
                let result = match validate(invocation).map(|spec| spec.scope) {
                    Ok(CommandScope::Application) => {
                        run_app_command(cx, invocation, Some(reporter))
                    }
                    Ok(CommandScope::Window | CommandScope::Runtime) => {
                        self.run_command(invocation, window, cx)
                    }
                    Ok(CommandScope::Terminal) => target
                        .terminal_view
                        .as_ref()
                        .and_then(WeakEntity::upgrade)
                        .ok_or(CommandError::StaleTarget)
                        .and_then(|terminal| {
                            terminal.update(cx, |terminal, cx| {
                                terminal.run_command(invocation, window, cx)
                            })
                        }),
                    Ok(CommandScope::Palette) => {
                        Err(CommandError::UnknownCommand(invocation.id))
                    }
                    Err(error) => Err(error),
                };
                match result {
                    Ok(_) => {
                        self.recent.record(invocation.id);
                        cx.global_mut::<Desktop>()
                            .frequency
                            .record(invocation.id);
                    }
                    Err(error) => set_dispatch_error(
                        &mut self.palette,
                        &mut self.status,
                        &error,
                    ),
                }
                cx.notify();
            }
        }
    }

    fn restore_palette_focus(
        &self,
        target: &PaletteTarget,
        window: &mut Window,
        cx: &App,
    ) {
        if target.tab == self.active
            && let Some(terminal) =
                target.terminal_view.as_ref().and_then(WeakEntity::upgrade)
            && terminal.read(cx).visible
        {
            terminal.read(cx).focus.focus(window);
        } else if let Some(terminal) = self.active_view() {
            terminal.read(cx).focus.focus(window);
        } else {
            self.focus.focus(window);
        }
    }

    fn palette_availability(
        &self,
        target: &PaletteTarget,
        cx: &App,
    ) -> std::collections::HashMap<huterm_protocol::CommandId, String> {
        catalog()
            .iter()
            .filter(|spec| spec.id != ids::OPEN_COMMAND_PALETTE)
            .filter_map(|spec| {
                self.command_availability(spec.id, target, cx)
                    .err()
                    .map(|error| (spec.id, error.to_string()))
            })
            .collect()
    }

    fn current_palette_refresh_state(&self) -> PaletteRefreshState {
        PaletteRefreshState {
            tabs: self.tabs.len(),
            active: self.active,
            busy: self.busy,
            confirming: self.close.confirmation.is_some(),
            reordering: self.reorder.is_some(),
            quake: self
                .quake
                .as_ref()
                .map(|state| (state.visible(), state.fullscreen_context())),
        }
    }

    fn refresh_palette(&mut self, cx: &mut Context<'_, Self>) {
        let Some(palette) = self.palette.clone() else {
            self.palette_refresh_state = None;
            return;
        };
        let state = self.current_palette_refresh_state();
        if self.palette_refresh_state == Some(state) {
            return;
        }
        self.palette_refresh_state = Some(state);
        let target = palette.read(cx).target.clone();
        let availability = self.palette_availability(&target, cx);
        let keymap = cx.global::<Desktop>().keymap.clone();
        palette.update(cx, |palette, _| {
            palette.set_availability(availability);
            palette.set_keymap(keymap);
        });
    }

    #[expect(dead_code, reason = "consumed by command palette chunk 6")]
    pub(super) fn history_view<'a>(&'a self, cx: &'a App) -> HistoryView<'a> {
        HistoryView {
            recent: &self.recent,
            frequency: &cx.global::<Desktop>().frequency,
        }
    }

    fn command_availability(
        &self,
        command: huterm_protocol::CommandId,
        target: &PaletteTarget,
        cx: &App,
    ) -> Result<(), CommandError> {
        let spec = huterm_protocol::lookup(command.as_str())
            .ok_or(CommandError::UnknownCommand(command))?;
        match spec.scope {
            CommandScope::Application => app_command_availability(cx, command),
            CommandScope::Window => match command {
                ids::NEW_TAB => self.check_new_tab_available(cx),
                ids::CLOSE_TAB => self.active_tab_id().map(|_| ()),
                ids::NEXT_TAB | ids::PREVIOUS_TAB => {
                    self.check_navigation_available()
                }
                ids::SELECT_TAB => self.check_navigation_available(),
                ids::SELECT_RECENT_TAB => {
                    self.check_navigation_available()?;
                    if self.tabs.len() < 2 {
                        Err(CommandError::Unavailable(
                            "window has one tab".to_owned(),
                        ))
                    } else {
                        Ok(())
                    }
                }
                ids::OPEN_COMMAND_PALETTE => self.check_available(true),
                ids::MINIMIZE | ids::ZOOM => {
                    self.check_presentation_available()
                }
                ids::TOGGLE_FULLSCREEN
                | ids::TOGGLE_NATIVE_FULLSCREEN
                | ids::TOGGLE_NON_NATIVE_FULLSCREEN => {
                    self.check_fullscreen_available(command)
                }
                ids::CLOSE_WINDOW | ids::ABOUT | ids::OPEN_SETTINGS => Ok(()),
                other => Err(CommandError::UnknownCommand(other)),
            },
            CommandScope::Runtime => {
                self.check_runtime_available(cx)?;
                match command {
                    ids::RENAME_TAB if target.tab.is_none() => Err(
                        CommandError::Unavailable("window has no tab".into()),
                    ),
                    ids::RENAME_WORKSPACE if target.workspace.is_none() => {
                        Err(CommandError::Unavailable(
                            "window has no workspace".into(),
                        ))
                    }
                    ids::RENAME_SESSION if target.session.is_none() => {
                        Err(CommandError::Unavailable(
                            "loading command target".into(),
                        ))
                    }
                    ids::RESET_TAB_NAME => self.custom_name_availability(
                        target.tab.map(CommandValue::Tab),
                        "window has no tab",
                        "tab has no custom name",
                        cx,
                    ),
                    ids::RESET_WORKSPACE_NAME => self.custom_name_availability(
                        target.workspace.map(CommandValue::Workspace),
                        "window has no workspace",
                        "workspace has no custom name",
                        cx,
                    ),
                    ids::RESET_SESSION_NAME => self.custom_name_availability(
                        target.session.map(CommandValue::Session),
                        "loading command target",
                        "session has no custom name",
                        cx,
                    ),
                    _ => Ok(()),
                }
            }
            CommandScope::Terminal => {
                let terminal = target
                    .terminal_view
                    .as_ref()
                    .and_then(WeakEntity::upgrade)
                    .ok_or(CommandError::Unavailable(
                        "window has no active terminal".into(),
                    ))?;
                let terminal = terminal.read(cx);
                if Some(terminal.client.terminal_id()) != target.terminal {
                    return Err(CommandError::StaleTarget);
                }
                terminal.command_availability(command)
            }
            CommandScope::Palette => Ok(()),
        }
    }

    fn custom_name_state(
        &self,
        value: &CommandValue,
        cx: &App,
    ) -> Option<bool> {
        self.palette
            .as_ref()
            .and_then(|palette| palette.read(cx).custom_name_state(value))
    }

    fn custom_name_availability(
        &self,
        target: Option<CommandValue>,
        missing_target: &str,
        missing_custom_name: &str,
        cx: &App,
    ) -> Result<(), CommandError> {
        let target = target.ok_or_else(|| {
            CommandError::Unavailable(missing_target.to_owned())
        })?;
        if self.custom_name_state(&target, cx) == Some(false) {
            Err(CommandError::Unavailable(missing_custom_name.to_owned()))
        } else {
            Ok(())
        }
    }

    fn fill_reset_target(
        &self,
        invocation: &CommandInvocation,
    ) -> Result<CommandInvocation, CommandError> {
        match invocation.id {
            ids::RESET_TAB_NAME => fill_target(
                invocation,
                "tab",
                self.active.map(CommandValue::Tab),
            ),
            ids::RESET_WORKSPACE_NAME => fill_target(
                invocation,
                "workspace",
                self.workspace.map(CommandValue::Workspace),
            ),
            ids::RESET_SESSION_NAME
                if invocation.argument("session").is_none() =>
            {
                fill_target(
                    invocation,
                    "workspace",
                    self.workspace.map(CommandValue::Workspace),
                )
            }
            ids::RESET_SESSION_NAME => Ok(invocation.clone()),
            other => Err(CommandError::UnknownCommand(other)),
        }
    }

    fn reload_palette(&mut self, cx: &mut Context<'_, Self>) {
        self.palette_refresh_state = None;
        self.refresh_palette(cx);
        cx.notify();
    }

    /// Key context for binding predicates: `Workspace`, plus `confirming`,
    /// `reordering`, and `fullscreen` while those states hold.
    fn key_context(&self, _window: &Window) -> KeyContext {
        let mut context = KeyContext::default();
        context.add("Workspace");
        if self.close.confirmation.is_some() {
            context.add("confirming");
        }
        if self.reorder.is_some() {
            context.add("reordering");
        }
        if self.fullscreen_context() {
            context.add("fullscreen");
        }
        if self.palette.is_some() {
            context.add("palette");
        }
        context
    }

    /// Refuses commands while structural work, a close confirmation, or (when
    /// `reordering` matters) a tab drag would race with them.
    fn check_available(&self, reordering: bool) -> Result<(), CommandError> {
        let reason = if self.busy {
            "structural operation in progress"
        } else if self.close.confirmation.is_some() {
            "close confirmation pending"
        } else if reordering && self.reorder.is_some() {
            "tab reorder in progress"
        } else {
            return Ok(());
        };
        Err(CommandError::Unavailable(reason.to_owned()))
    }

    fn active_tab_id(&self) -> Result<TabId, CommandError> {
        self.active.ok_or_else(|| {
            CommandError::Unavailable("window has no tab".to_owned())
        })
    }

    fn check_navigation_available(&self) -> Result<(), CommandError> {
        self.check_available(true)?;
        self.active_tab_id().map(|_| ())
    }

    fn check_new_tab_available(&self, cx: &App) -> Result<(), CommandError> {
        self.check_available(false)?;
        if cx.global::<Desktop>().quitting
            || cx.global::<Desktop>().quit_pending
        {
            Err(CommandError::Unavailable(
                "application is quitting".to_owned(),
            ))
        } else {
            Ok(())
        }
    }

    fn check_runtime_available(&self, cx: &App) -> Result<(), CommandError> {
        self.check_available(true)?;
        if cx
            .global::<Desktop>()
            .runtime
            .terminating
            .load(Ordering::Acquire)
        {
            Err(CommandError::Unavailable(
                "runtime is terminating".to_owned(),
            ))
        } else {
            Ok(())
        }
    }

    fn check_fullscreen_available(
        &self,
        command: huterm_protocol::CommandId,
    ) -> Result<(), CommandError> {
        if self.quake.is_some() {
            return if command == ids::TOGGLE_FULLSCREEN {
                if self.close.confirmation.is_some() {
                    Err(CommandError::Unavailable(
                        "a close confirmation is active".into(),
                    ))
                } else {
                    Ok(())
                }
            } else {
                Err(CommandError::Unavailable("quake profile windows use toggle_fullscreen to switch presentation".into()))
            };
        }
        let intent = match command {
            ids::TOGGLE_NATIVE_FULLSCREEN => ToggleIntent::Native,
            ids::TOGGLE_NON_NATIVE_FULLSCREEN => ToggleIntent::NonNative,
            ids::TOGGLE_FULLSCREEN => ToggleIntent::Default,
            other => return Err(CommandError::UnknownCommand(other)),
        };
        self.fullscreen
            .check_toggle(intent, || {
                #[cfg(target_os = "macos")]
                self.native_fullscreen
                    .as_ref()
                    .ok_or_else(|| {
                        "non-native fullscreen adapter is unavailable"
                            .to_owned()
                    })?
                    .preflight()
                    .map_err(|error| error.to_string())?;
                Ok(())
            })
            .map_err(CommandError::Unavailable)
    }

    /// Runs a window- or runtime-scope catalog command against this window.
    ///
    /// Runtime commands take omitted targets from this window's active tab,
    /// workspace, or session, then execute on the structural worker; their
    /// later failure is reported through the window status.
    ///
    /// # Errors
    /// Reports refused commands as [`CommandError::Unavailable`] and commands
    /// this window does not own as [`CommandError::UnknownCommand`].
    fn run_command(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        validate(invocation)?;
        if self.palette.is_some() && invocation.id != ids::OPEN_COMMAND_PALETTE
        {
            return Err(CommandError::Unavailable(
                "command palette is open".to_owned(),
            ));
        }
        if let Some(tab) = self.active_view() {
            tab.update(cx, |tab, _| tab.clear_option_composition());
        }
        match invocation.id {
            ids::NEW_TAB => self.new_tab(window, cx),
            ids::CLOSE_TAB => {
                let id = self.active_tab_id()?;
                self.request_close(CloseTarget::Tab(id), window, cx);
                Ok(CommandOutcome::Accepted)
            }
            ids::CLOSE_WINDOW => {
                self.request_close(CloseTarget::Window, window, cx);
                Ok(CommandOutcome::Accepted)
            }
            ids::NEXT_TAB => self.navigate(true, window, cx),
            ids::PREVIOUS_TAB => self.navigate(false, window, cx),
            ids::SELECT_TAB => self.select_target(invocation, window, cx),
            ids::SELECT_RECENT_TAB => self.select_recent_tab(window, cx),
            ids::TOGGLE_FULLSCREEN
            | ids::TOGGLE_NATIVE_FULLSCREEN
            | ids::TOGGLE_NON_NATIVE_FULLSCREEN => {
                if self.quake.is_some() {
                    self.check_fullscreen_available(invocation.id)?;
                    return quake_windows::toggle(self, window, cx);
                }
                self.observe_fullscreen(window, cx);
                let intent = match invocation.id {
                    ids::TOGGLE_NATIVE_FULLSCREEN => ToggleIntent::Native,
                    ids::TOGGLE_NON_NATIVE_FULLSCREEN => {
                        ToggleIntent::NonNative
                    }
                    _ => ToggleIntent::Default,
                };
                self.check_fullscreen_available(invocation.id)?;
                self.fullscreen
                    .toggle_checked(intent, || Ok(()))
                    .map_err(CommandError::Unavailable)?;
                self.advance_fullscreen(window, cx);
                Ok(CommandOutcome::Accepted)
            }
            ids::MINIMIZE => {
                self.check_presentation_available()?;
                window.minimize_window();
                Ok(CommandOutcome::Completed)
            }
            ids::ZOOM => {
                self.check_presentation_available()?;
                window.zoom_window();
                Ok(CommandOutcome::Completed)
            }
            ids::ABOUT => {
                let detail =
                    format!("Version {}\n{APP_ID}", env!("CARGO_PKG_VERSION"));
                let answer = window.prompt(
                    PromptLevel::Info,
                    "Huterm",
                    Some(&detail),
                    &["OK"],
                    cx,
                );
                cx.spawn(async move |_, _| {
                    let _ = answer.await;
                })
                .detach();
                Ok(CommandOutcome::Completed)
            }
            ids::OPEN_SETTINGS => {
                let config_path = cx.global::<Desktop>().config_path.clone();
                config::create_default(&config_path).map_err(|error| {
                    CommandError::Runtime(format!(
                        "failed to open settings: {error}"
                    ))
                })?;
                cx.open_with_system(&config_path);
                Ok(CommandOutcome::Completed)
            }
            ids::OPEN_COMMAND_PALETTE => self.open_palette(window, cx),
            ids::RENAME_TAB | ids::RENAME_WORKSPACE | ids::RENAME_SESSION => {
                let invocation = fill_rename_target(
                    invocation,
                    self.active,
                    self.workspace,
                )?;
                Ok(run_on_runtime(invocation, cx))
            }
            ids::RESET_TAB_NAME
            | ids::RESET_WORKSPACE_NAME
            | ids::RESET_SESSION_NAME => {
                let invocation = self.fill_reset_target(invocation)?;
                Ok(run_on_runtime(invocation, cx))
            }
            other => Err(CommandError::UnknownCommand(other)),
        }
    }
}

/// Executes a filled runtime command on the structural worker and reports a
/// later failure through the window status.
fn run_on_runtime(
    invocation: CommandInvocation,
    cx: &mut Context<'_, WorkspaceView>,
) -> CommandOutcome {
    let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
    let task = cx
        .background_executor()
        .spawn(async move { runtime.execute(&invocation) });
    cx.spawn(async move |view, cx| {
        if let Err(error) = task.await {
            let _ = view.update(cx, |view, cx| {
                view.status = Some(format!("Command failed: {error}"));
                cx.notify();
            });
        }
    })
    .detach();
    CommandOutcome::Accepted
}

impl WorkspaceView {
    fn check_presentation_available(&self) -> Result<(), CommandError> {
        if self.fullscreen.can_resize_window() {
            Ok(())
        } else {
            Err(CommandError::Unavailable(
                "window owns non-native fullscreen presentation state"
                    .to_owned(),
            ))
        }
    }

    fn refresh_fullscreen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.quake.is_some() {
            #[cfg(target_os = "macos")]
            if let Some(adapter) = &self.native_fullscreen {
                adapter.discard_quake_events();
            }
            return;
        }
        self.observe_fullscreen(window, cx);
        self.advance_fullscreen(window, cx);
    }

    // Commands must reconcile queued native changes before choosing a target.
    // Observation cannot dispatch effects for the preceding desired state.
    fn observe_fullscreen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let previous = (
            self.fullscreen.chrome_hidden,
            self.fullscreen.observed,
            self.fullscreen_insets,
        );
        let now = Instant::now();
        #[cfg(target_os = "macos")]
        if let Some(adapter) = &self.native_fullscreen {
            for event in adapter.drain() {
                use crate::native_fullscreen::Event;
                match event {
                    Event::Native(event) => {
                        self.fullscreen.native_event(event, now);
                    }
                    Event::NativeExitFailed(error) => {
                        self.fullscreen.recover();
                        self.status =
                            Some(format!("Fullscreen failed: {error}"));
                        cx.notify();
                    }
                    Event::State(recovery, chrome) => {
                        self.fullscreen.non_native_state(recovery, chrome);
                    }
                    Event::Complete(generation, recovery) => {
                        self.fullscreen.complete(generation, recovery);
                    }
                    Event::Failed(generation, error) => {
                        if self.fullscreen.fail(generation) {
                            self.status =
                                Some(format!("Fullscreen failed: {error}"));
                            cx.notify();
                        }
                    }
                    Event::Recover => self.fullscreen.recover(),
                }
            }
        } else {
            self.fullscreen.observe_native_flag(window.is_fullscreen());
        }
        self.fullscreen
            .sample(window.is_fullscreen(), window.window_bounds());
        if let Some(generation) = self.fullscreen.expired(now) {
            #[cfg(not(target_os = "macos"))]
            let _ = generation;
            self.status = Some("Fullscreen transition timed out".to_owned());
            eprintln!("Fullscreen transition timed out");
            cx.notify();
            #[cfg(target_os = "macos")]
            if let Some(adapter) = &self.native_fullscreen {
                adapter.cancel(generation);
            }
        }
        #[cfg(target_os = "macos")]
        {
            self.fullscreen_insets = self
                .native_fullscreen
                .as_ref()
                .filter(|_| self.fullscreen.chrome_hidden)
                .map_or_else(
                    gpui::Edges::default,
                    crate::native_fullscreen::Adapter::safe_area,
                );
        }
        if self.quake.is_none() {
            self.bounds = self.fullscreen.restorable_bounds();
        }
        if previous
            != (
                self.fullscreen.chrome_hidden,
                self.fullscreen.observed,
                self.fullscreen_insets,
            )
        {
            for tab in &self.tabs {
                tab.view.update(cx, |terminal, cx| {
                    terminal.chrome_hidden = self.fullscreen.chrome_hidden;
                    terminal.fullscreen_insets = self.fullscreen_insets;
                    terminal.resize_if_needed(window);
                    cx.notify();
                });
            }
            cx.notify();
        }
    }

    fn advance_fullscreen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        #[cfg(not(target_os = "macos"))]
        let _ = cx;
        if let Some(operation) = self.fullscreen.next(Instant::now()) {
            match operation.effect {
                Effect::ToggleNative => {
                    #[cfg(target_os = "macos")]
                    if let Some(adapter) = &self.native_fullscreen
                        && let Err(error) = adapter.check_native_transition()
                    {
                        self.fullscreen.fail(operation.generation);
                        self.status =
                            Some(format!("Fullscreen failed: {error}"));
                        cx.notify();
                        return;
                    }
                    window.toggle_fullscreen();
                }
                Effect::EnterNonNative | Effect::ExitNonNative => {
                    #[cfg(target_os = "macos")]
                    if let Some(adapter) = self.native_fullscreen.clone() {
                        adapter.reserve(operation);
                        cx.spawn(async move |_, cx| {
                            adapter.begin(operation);
                            cx.background_executor()
                                .timer(Duration::from_millis(1))
                                .await;
                            adapter.finish(operation);
                        })
                        .detach();
                    } else {
                        self.fullscreen.fail(operation.generation);
                        self.status = Some(
                            "Fullscreen native adapter is unavailable"
                                .to_owned(),
                        );
                    }
                }
            }
        }
    }

    fn remove_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
        quit_after: bool,
    ) {
        quake_windows::close(self, cx);
        self.fullscreen.close();
        #[cfg(target_os = "macos")]
        {
            let adapter = self.native_fullscreen.take();
            if let Some(adapter) = &adapter {
                adapter.close_gate();
            }
            let handle = window.window_handle();
            cx.spawn(async move |_, cx| {
                if let Some(adapter) = adapter {
                    adapter.close();
                }
                let _ = handle.update(cx, |_, window, cx| {
                    Desktop::stop_window_drag(window, cx);
                    window.remove_window();
                    if quit_after {
                        cx.defer(|cx| invoke(ids::QUIT).dispatch(cx));
                    }
                });
            })
            .detach();
        }
        #[cfg(not(target_os = "macos"))]
        {
            Desktop::stop_window_drag(window, cx);
            window.remove_window();
            if quit_after {
                cx.defer(|cx| invoke(ids::QUIT).dispatch(cx));
            }
        }
    }

    fn select(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.tabs.iter().any(|tab| tab.id == id) {
            return;
        }
        self.active = Some(id);
        record_tab_activation(&mut self.history, id);
        self.sync_tab_layout(window, cx);
        self.reveal_active(window);
        let visible = self.quake_visible();
        for tab in &self.tabs {
            tab.view.update(cx, |view, cx| {
                view.visible = visible && tab.id == id;
                if view.visible {
                    view.resize_if_needed(window);
                    view.focus.focus(window);
                    view.start_initial_snapshot(cx);
                } else {
                    view.hide(cx);
                }
            });
        }
        cx.notify();
    }

    fn select_from_command(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let changed = self.active != Some(id);
        self.select(id, window, cx);
        if changed {
            self.reveal_tab_activity(window, cx);
        }
    }

    fn select_recent_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.check_navigation_available()?;
        let next = recent_tab(&self.history, self.active, |id| {
            self.tabs.iter().any(|tab| tab.id == id)
        });
        if let Some(next) = next {
            self.select_from_command(next, window, cx);
        }
        Ok(CommandOutcome::Completed)
    }

    #[expect(dead_code, reason = "consumed by command palette chunk 6")]
    pub(super) fn tab_history(&self) -> &[TabId] {
        &self.history
    }

    fn reveal_tab_activity(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.presentation() == Presentation::Overlay {
            self.refresh_tab_visibility(window, cx);
            self.reveal.reveal_for_activity(Instant::now());
        }
    }

    fn navigate(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.check_navigation_available()?;
        let index = self
            .tabs
            .iter()
            .position(|tab| Some(tab.id) == self.active)
            .unwrap_or(0);
        let next = if forward {
            (index + 1) % self.tabs.len()
        } else {
            (index + self.tabs.len() - 1) % self.tabs.len()
        };
        self.select_from_command(self.tabs[next].id, window, cx);
        Ok(CommandOutcome::Completed)
    }

    fn select_by_id(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.check_available(true)?;
        if !self.tabs.iter().any(|tab| tab.id == id) {
            return Err(CommandError::StaleTarget);
        }
        self.select_from_command(id, window, cx);
        Ok(CommandOutcome::Completed)
    }

    fn select_target(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        match (invocation.tab("tab"), invocation.integer("index")) {
            (Some(tab), _) => self.select_by_id(tab, window, cx),
            (None, Some(_)) => {
                self.select_index(select_tab_slot(invocation)?, window, cx)
            }
            (None, None) => Err(CommandError::MissingArgument {
                command: ids::SELECT_TAB,
                name: "tab",
            }),
        }
    }

    /// Selects the tab at zero-based `index`; slot 8 selects the last tab.
    /// An empty slot completes without selecting anything.
    fn select_index(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.check_available(true)?;
        let tab = if index == 8 {
            self.tabs.last()
        } else {
            self.tabs.get(index)
        };
        // A stray cmd-5 in a three-tab window is routine; other terminals
        // ignore it too, so it is not a refusal worth reporting.
        if let Some(tab) = tab {
            self.select_from_command(tab.id, window, cx);
        }
        Ok(CommandOutcome::Completed)
    }

    fn restore_window(&self) -> Option<WindowRestore> {
        Some(WindowRestore {
            attachment: self.attachment?,
            workspace: self.workspace,
            active: self.active,
            bounds: self.bounds,
            tab_position: self.config.window.tab_position,
            sidebar_width: self.sidebar_width,
            tab_scroll: self.tab_scroll,
        })
    }

    fn request_close(
        &mut self,
        target: CloseTarget,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.cancel_reorder(window, cx);
        self.resizing_sidebar = false;
        if let CloseTarget::Tab(id) = target
            && !self.tabs.iter().any(|tab| tab.id == id)
        {
            return;
        }
        if matches!(target, CloseTarget::Application) {
            cx.global_mut::<Desktop>().quitting = true;
        }
        if self.busy {
            self.close.queue(target);
            return;
        }
        let target = self.close.begin_check(target);
        let request = match target {
            CloseTarget::Application => CloseRequest::Application,
            CloseTarget::Window => {
                let Some(attachment) = self.attachment else {
                    self.remove_window(window, cx, false);
                    return;
                };
                CloseRequest::Window(attachment)
            }
            CloseTarget::Tab(tab) => {
                let Some(workspace) = self.workspace else {
                    return;
                };
                CloseRequest::Tab { workspace, tab }
            }
        };
        let generation = self.close.generation;
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let task = cx
            .background_executor()
            .spawn(async move { runtime.assess(request) });
        self.busy = true;
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |view, window, cx| {
                if view.close.generation != generation {
                    return;
                }
                view.busy = false;
                let assessment = match result {
                    Ok(assessment) => assessment,
                    Err(error) => {
                        view.status =
                            Some(format!("Cannot assess close: {error}"));
                        view.cancel_close(window, cx);
                        return;
                    }
                };
                let foreground = assessment.needs_confirmation();
                view.close.assessment = Some(assessment);
                match view.close.checked(foreground) {
                    Some(CloseDecision::Check(target)) => {
                        view.request_close(target, window, cx);
                    }
                    Some(CloseDecision::Confirm(_)) => {
                        view.focus.focus(window);
                        cx.notify();
                    }
                    Some(CloseDecision::Close(target)) => {
                        view.finish_close(target, window, cx);
                    }
                    None => {}
                }
            });
        })
        .detach();
    }

    fn cancel_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.busy = false;
        if matches!(self.close.cancel(), Some(CloseTarget::Application)) {
            #[cfg(target_os = "macos")]
            native_quit::cancel_request();
            cx.global_mut::<Desktop>().quitting = false;
        }
        if let Some(tab) = self.active_view() {
            tab.read(cx).focus.focus(window);
        }
        cx.notify();
    }

    fn resume_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let target =
            self.close
                .next_request(&mut self.exited_tabs, self.busy, |id| {
                    self.tabs.iter().any(|tab| tab.id == id)
                });
        if let Some(target) = target {
            self.request_close(target, window, cx);
            true
        } else {
            false
        }
    }

    fn finish_close(
        &mut self,
        target: CloseTarget,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let confirmed = self.close.confirmation == Some(target);
        let Some(assessment) = self.close.assessment.take() else {
            self.request_close(target, window, cx);
            return;
        };
        self.close.confirmation = None;
        self.close.current = Some(target);
        self.busy = true;
        let generation = self.close.generation;
        if self.quake.is_none() {
            self.bounds = self.fullscreen.restorable_bounds();
        }
        let windows = if matches!(target, CloseTarget::Application) {
            cx.global_mut::<Desktop>().quitting = true;
            let mut records = capture_other_windows(cx, Some(cx.entity_id()));
            // The dispatching view is borrowed; capture it directly below.
            if let Some(record) = self.restore_window() {
                records.retain(|saved| saved.attachment != record.attachment);
                records.push(record);
            }
            Some(records)
        } else {
            None
        };
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let task = cx.background_executor().spawn(async move {
            runtime.commit(&assessment, confirmed, windows)
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |view, window, cx| {
                if view.close.generation != generation {
                    return;
                }
                view.busy = false;
                view.close.current = None;
                if matches!(
                    result,
                    Err(MuxError::StaleClose | MuxError::ConfirmationRequired)
                ) {
                    view.request_close(target, window, cx);
                    return;
                }
                if let Err(error) = result {
                    view.status = Some(format!("Close failed: {error}"));
                    eprintln!(
                        "{}",
                        view.status.as_deref().unwrap_or("Close failed")
                    );
                }
                match target {
                    CloseTarget::Application => approved_quit(cx),
                    CloseTarget::Window => {
                        let quit_after = matches!(
                            view.close.pending,
                            Some(CloseTarget::Application)
                        );
                        view.remove_window(window, cx, quit_after);
                    }
                    CloseTarget::Tab(id) => {
                        remove_tab(
                            &mut view.tabs,
                            &mut view.active,
                            id,
                            |tab| tab.id,
                        );
                        prune_tab_history(&mut view.history, id);
                        if let Some(active) = view.active {
                            view.select(active, window, cx);
                            view.reveal_tab_activity(window, cx);
                        }
                        if view.resume_close(window, cx) {
                            return;
                        }
                        if view.tabs.is_empty() {
                            view.request_close(CloseTarget::Window, window, cx);
                        }
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }
}

fn reload(cx: &mut App) -> Result<CommandOutcome, CommandError> {
    if cx.global::<Desktop>().reloading {
        return Err(CommandError::Unavailable(
            "configuration reload in progress".to_owned(),
        ));
    }
    cx.global_mut::<Desktop>().reloading = true;
    let path = cx.global::<Desktop>().config_path.clone();
    let task = cx
        .background_executor()
        .spawn(async move { config::reload(&path) });
    cx.spawn(async move |cx| {
        let result = task.await;
        let _ = cx.update(|cx| {
            cx.global_mut::<Desktop>().reloading = false;
            let result = result.and_then(|config| {
                let (family, metrics) = resolve_metrics(&config, cx)
                    .map_err(|error| error.to_string())?;
                let compiled =
                    keymap::compile(Platform::current(), &config.keybindings)
                        .map_err(|error| {
                        format!("{}: {error}", path_for_status(cx))
                    })?;
                quake_windows::replace_registrations(cx, &config, &compiled)?;
                Ok((config, family, metrics, compiled))
            });
            let mut keymap_status = None;
            let result = result.map(|(config, family, metrics, compiled)| {
                #[cfg(all(target_os = "macos", feature = "macos-updater"))]
                apply_update_config(cx, &config);
                cx.global_mut::<Desktop>().config = config.clone();
                cx.global_mut::<Desktop>().config_error = None;
                quake_windows::reconcile(cx);
                if !compiled.conflicts.is_empty() {
                    keymap_status = Some(compiled.conflicts.join("; "));
                }
                let keymap = bind_keymap(cx, compiled);
                cx.global_mut::<Desktop>().keymap = keymap;
                maybe_exit(cx);
                (config, family, metrics)
            });
            report_config_reload_error(&result);
            let windows = cx.global::<Desktop>().windows.clone();
            for window in windows {
                let _ = window.update(cx, |view, cx| {
                    if let Some(tab) = view.active_view() {
                        tab.update(cx, |tab, _| tab.clear_option_composition());
                    }
                    match &result {
                        Ok((config, family, metrics)) => {
                            view.resizing_sidebar = false;
                            view.scroll_target = None;
                            view.config = config.clone();
                            view.fullscreen.set_default(
                                config.window.macos_fullscreen_mode,
                            );
                            view.family.clone_from(family);
                            view.metrics = *metrics;
                            view.status = None;
                            for tab in &view.tabs {
                                tab.view.update(cx, |view, cx| {
                                    let metrics = metrics
                                        .at_scale(view.metrics.scale_factor);
                                    view.renderer.borrow_mut().reconfigure(
                                        family.clone(),
                                        config.theme.clone(),
                                        metrics,
                                    );
                                    view.font_family.clone_from(family);
                                    view.font_size = metrics.font_size;
                                    view.metrics = metrics;
                                    view.window_config = config.window;
                                    view.links.disable();
                                    view.links_enabled = config.terminal.links;
                                    view.link_modifiers =
                                        config.terminal.link_modifiers;
                                    if view.option_as_alt
                                        != config.terminal.macos_option_as_alt
                                    {
                                        view.clear_composition(cx);
                                        view.option_as_alt =
                                            config.terminal.macos_option_as_alt;
                                    }
                                    view.theme = config.theme.clone();
                                    cx.notify();
                                });
                            }
                            view.status.clone_from(&keymap_status);
                        }
                        Err(error) => {
                            view.status =
                                Some(format!("Config reload failed: {error}"));
                        }
                    }
                    view.reload_palette(cx);
                });
            }
        });
    })
    .detach();
    Ok(CommandOutcome::Accepted)
}

fn report_config_reload_error<T>(result: &Result<T, String>) {
    if let Err(error) = result {
        eprintln!("Config reload failed: {error}");
    }
}

fn path_for_status(cx: &App) -> String {
    cx.global::<Desktop>().config_path.display().to_string()
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ChromeLayout {
    pub(super) terminal: Bounds<Pixels>,
    tabs: Bounds<Pixels>,
}
impl ChromeLayout {
    #[cfg(test)]
    pub(super) fn new(
        viewport: gpui::Size<Pixels>,
        titlebar: Pixels,
        position: TabPosition,
    ) -> Self {
        Self::with_sidebar(viewport, titlebar, position, SIDEBAR_WIDTH)
    }
    pub(super) fn present(
        mut self,
        presentation: Presentation,
        position: TabPosition,
        progress: f32,
    ) -> Self {
        if presentation != Presentation::Reserved {
            if position.vertical() {
                self.terminal.size.width += self.tabs.size.width;
                if position == TabPosition::Left {
                    self.terminal.origin.x = self.tabs.origin.x;
                }
            } else {
                self.terminal.size.height += self.tabs.size.height;
                if position == TabPosition::Top {
                    self.terminal.origin.y = self.tabs.origin.y;
                }
            }
        }
        if presentation == Presentation::Overlay {
            let hidden = 1.0 - progress.clamp(0.0, 1.0);
            match position {
                TabPosition::Top => {
                    self.tabs.origin.y -= self.tabs.size.height * hidden;
                }
                TabPosition::Bottom => {
                    self.tabs.origin.y += self.tabs.size.height * hidden;
                }
                TabPosition::Left => {
                    self.tabs.origin.x -= self.tabs.size.width * hidden;
                }
                TabPosition::Right => {
                    self.tabs.origin.x += self.tabs.size.width * hidden;
                }
            }
        }
        self
    }

    fn sidebar_resize_handle(&self, position: TabPosition) -> Bounds<Pixels> {
        let width = px(6.0).min(self.tabs.size.width);
        let x = self.tabs.origin.x
            + if position == TabPosition::Left {
                self.tabs.size.width - width
            } else {
                px(0.0)
            };
        Bounds::new(
            point(x, self.tabs.origin.y),
            size(width, self.tabs.size.height),
        )
    }

    #[cfg(test)]
    pub(super) fn with_sidebar(
        viewport: gpui::Size<Pixels>,
        titlebar: Pixels,
        position: TabPosition,
        sidebar_width: Pixels,
    ) -> Self {
        Self::with_safe_area(
            viewport,
            titlebar,
            position,
            sidebar_width,
            gpui::Edges::default(),
        )
    }

    pub(super) fn with_safe_area(
        viewport: gpui::Size<Pixels>,
        titlebar: Pixels,
        position: TabPosition,
        sidebar_width: Pixels,
        safe_area: gpui::Edges<Pixels>,
    ) -> Self {
        let left = safe_area.left.max(px(0.0)).min(viewport.width.max(px(0.0)));
        let top = (titlebar + safe_area.top)
            .max(px(0.0))
            .min(viewport.height.max(px(0.0)));
        let available = size(
            (viewport.width - left - safe_area.right.max(px(0.0))).max(px(0.0)),
            (viewport.height - top - safe_area.bottom.max(px(0.0)))
                .max(px(0.0)),
        );
        let mut terminal = Bounds::new(point(left, top), available);
        let mut tabs = terminal;
        if position.vertical() {
            tabs.size.width = sidebar_width
                .clamp(px(140.0), px(400.0))
                .min(available.width * 0.5);
            terminal.size.width =
                (available.width - tabs.size.width).max(px(0.0));
            if position == TabPosition::Left {
                terminal.origin.x += tabs.size.width;
            } else {
                tabs.origin.x += terminal.size.width;
            }
        } else {
            tabs.size.height = TAB_HEIGHT.min(available.height);
            terminal.size.height =
                (available.height - tabs.size.height).max(px(0.0));
            if position == TabPosition::Top {
                terminal.origin.y += tabs.size.height;
            } else {
                tabs.origin.y += terminal.size.height;
            }
        }
        Self { terminal, tabs }
    }
}

impl Render for WorkspaceView {
    #[expect(
        clippy::too_many_lines,
        clippy::cast_precision_loss,
        reason = "window chrome composes tab controls and close confirmation"
    )]
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        self.sync_tab_layout(window, cx);
        if self.reveal.progress > 0.0 && self.reveal.progress < 1.0 {
            window.request_animation_frame();
        }
        let position = self.config.window.tab_position;
        let layout = self.chrome_layout(window);
        let foreground = color(self.config.theme.foreground);
        let background = color(self.config.theme.background);
        let mut root = div()
            .size_full()
            .relative()
            .bg(background)
            .text_color(foreground)
            .text_size(px(13.0))
            .key_context(self.key_context(window))
            .track_focus(&self.focus)
            .on_drag_move::<gpui::ExternalPaths>(|_, window, cx| {
                // GPUI owns one app-wide drag. Closing another window must
                // not cancel the external payload delivered to this one.
                cx.global_mut::<Desktop>().external_drag_window =
                    Some(window.window_handle().window_id());
            })
            .on_key_down(cx.listener(
                |view, event: &gpui::KeyDownEvent, window, cx| {
                    if view.reorder.is_some() && event.keystroke.key == "escape"
                    {
                        view.cancel_reorder(window, cx);
                        // GPUI skips raw keystroke observers after propagation
                        // stops, so this Escape cannot also reach the terminal.
                        cx.stop_propagation();
                        return;
                    }
                    if view.close.confirmation.is_some()
                        && event.keystroke.key == "escape"
                    {
                        view.cancel_close(window, cx);
                        cx.stop_propagation();
                    }
                },
            ))
            .on_action(cx.listener(
                |view, action: &InvokeWindow, window, cx| {
                    if let Err(error) =
                        view.invoke_interactive(&action.0, window, cx)
                    {
                        view.status = Some(error.to_string());
                        cx.notify();
                    }
                },
            ))
            .on_action(cx.listener(Self::invoke_palette));
        let move_view = cx.entity().downgrade();
        let release_view = move_view.clone();
        // Register before terminal children so capture consumes drag movement
        // and release even beyond the bar/window, before application mouse input.
        root = root.child(
            canvas(
                |_, _, _| (),
                move |_, (), window, _| {
                    window.on_mouse_event(
                        move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = move_view.update(cx, |view, cx| {
                                    if view.resizing_sidebar {
                                        view.resize_sidebar(
                                            event.position,
                                            window,
                                            cx,
                                        );
                                        cx.stop_propagation();
                                    } else if view.reorder.is_some() {
                                        view.update_reorder(
                                            event.position,
                                            window,
                                            cx,
                                        );
                                        cx.stop_propagation();
                                    } else if view.palette.is_some() {
                                        cx.stop_propagation();
                                    }
                                });
                            }
                        },
                    );
                    window.on_mouse_event(
                        move |event: &MouseUpEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture
                                && event.button == MouseButton::Left
                            {
                                let _ = release_view.update(cx, |view, cx| {
                                    if view.resizing_sidebar {
                                        view.resize_sidebar(
                                            event.position,
                                            window,
                                            cx,
                                        );
                                        view.resizing_sidebar = false;
                                        cx.stop_propagation();
                                    } else if view.reorder.is_some() {
                                        view.finish_reorder(
                                            event.position,
                                            window,
                                            cx,
                                        );
                                        cx.stop_propagation();
                                    }
                                });
                            }
                        },
                    );
                },
            )
            .absolute()
            .inset_0(),
        );
        if terminal_top(self.chrome_hidden()) > px(0.0) {
            root = root.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(terminal_top(self.chrome_hidden()))
                    .pl(px(84.0))
                    .flex()
                    .items_center()
                    .window_control_area(WindowControlArea::Drag)
                    .child("Huterm"),
            );
        }
        if let Some(tab) = self.active_view() {
            root = root.child(
                div()
                    .absolute()
                    .left(layout.terminal.origin.x)
                    .top(layout.terminal.origin.y)
                    .w(layout.terminal.size.width)
                    .h(layout.terminal.size.height)
                    .overflow_hidden()
                    .child(tab),
            );
        }
        let strip = self.tab_strip(window);
        let vertical = strip.vertical;
        self.tab_scroll = strip.offset;
        if let Some(drag) = &mut self.reorder {
            drag.strip = strip.clone();
        }
        if self.presentation() == Presentation::Reserved
            || self.presentation() == Presentation::Overlay
                && self.reveal.progress > 0.0
        {
            let clip = if self.presentation() == Presentation::Overlay {
                layout.terminal
            } else {
                Bounds::new(point(px(0.0), px(0.0)), window.viewport_size())
            };
            let mut chrome = div()
                .absolute()
                .left(clip.origin.x)
                .top(clip.origin.y)
                .w(clip.size.width)
                .h(clip.size.height)
                .overflow_hidden();
            chrome = chrome.child(
                div()
                    .absolute()
                    .left(layout.tabs.origin.x - clip.origin.x)
                    .top(layout.tabs.origin.y - clip.origin.y)
                    .w(layout.tabs.size.width)
                    .h(layout.tabs.size.height)
                    .bg(background)
                    .occlude(),
            );
            let mut bar = div()
                .id("tab-strip")
                .occlude()
                .absolute()
                .left(strip.bounds.origin.x - clip.origin.x)
                .top(strip.bounds.origin.y - clip.origin.y)
                .w(strip.bounds.size.width)
                .h(strip.bounds.size.height)
                .overflow_hidden()
                .bg(background)
                .on_scroll_wheel(cx.listener(
                    move |view, event: &ScrollWheelEvent, window, cx| {
                        let delta = event.delta.pixel_delta(px(32.0));
                        let delta = if vertical {
                            if delta.y == px(0.0) { delta.x } else { delta.y }
                        } else if delta.x != px(0.0) {
                            delta.x
                        } else {
                            delta.y
                        };
                        view.scroll_tabs(-delta, window, cx);
                        cx.stop_propagation();
                    },
                ));
            for index in 0..self.tabs.len() {
                let tab = &self.tabs[index];
                let id = tab.id;
                let title = tab.title(cx);
                bar = bar.child(
                    div()
                        .id(("tab", id.get()))
                        .absolute()
                        .left(if vertical {
                            px(0.0)
                        } else {
                            strip.extent * index as f32 - strip.offset
                        })
                        .top(if vertical {
                            strip.extent * index as f32 - strip.offset
                        } else {
                            px(0.0)
                        })
                        .flex_shrink_0()
                        .w(if vertical {
                            layout.tabs.size.width
                        } else {
                            strip.extent
                        })
                        .h(TAB_HEIGHT)
                        .flex()
                        .items_center()
                        .px_2()
                        .gap_2()
                        .overflow_hidden()
                        .cursor_pointer()
                        .when(Some(id) == self.active, |tab| {
                            tab.bg(foreground.opacity(0.12))
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(
                                move |view,
                                      event: &MouseDownEvent,
                                      window,
                                      cx| {
                                    view.begin_reorder(
                                        id,
                                        event.position,
                                        window,
                                        cx,
                                    );
                                    cx.stop_propagation();
                                },
                            ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis()
                                .child(title),
                        )
                        .child(
                            div()
                                .id(("close-tab", id.get()))
                                .flex_shrink_0()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                })
                                .on_click(cx.listener(
                                    move |view, _, window, cx| {
                                        cx.stop_propagation();
                                        view.request_close(
                                            CloseTarget::Tab(id),
                                            window,
                                            cx,
                                        );
                                    },
                                ))
                                .child("×"),
                        ),
                );
            }
            for forward in [false, true] {
                if (forward && strip.offset < strip.max_offset())
                    || (!forward && strip.offset > px(0.0))
                {
                    let edge = if forward {
                        (strip.available() - CONTROL_SIZE).max(px(0.0))
                    } else {
                        px(0.0)
                    };
                    bar = bar.child(
                        div()
                            .id(if forward {
                                "scroll-tabs-forward"
                            } else {
                                "scroll-tabs-backward"
                            })
                            .absolute()
                            .left(if vertical {
                                (strip.bounds.size.width - CONTROL_SIZE)
                                    .max(px(0.0))
                                    / 2.0
                            } else {
                                edge
                            })
                            .top(if vertical { edge } else { px(2.0) })
                            .w(CONTROL_SIZE)
                            .h(CONTROL_SIZE)
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(background)
                            .rounded_md()
                            .opacity(0.9)
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                cx.stop_propagation();
                            })
                            .on_click(cx.listener(
                                move |view, _, window, cx| {
                                    let amount = view
                                        .tab_strip(window)
                                        .available()
                                        * if forward { 0.75 } else { -0.75 };
                                    let strip = view.tab_strip(window);
                                    view.scroll_target = Some(
                                        (view
                                            .scroll_target
                                            .unwrap_or(strip.offset)
                                            + amount)
                                            .clamp(px(0.0), strip.max_offset()),
                                    );
                                    cx.notify();
                                    cx.stop_propagation();
                                },
                            ))
                            .child(if vertical {
                                if forward { "⌄" } else { "⌃" }
                            } else if forward {
                                "›"
                            } else {
                                "‹"
                            }),
                    );
                }
            }
            chrome = chrome.child(bar).child(
                div()
                    .id("new-tab")
                    .occlude()
                    .bg(background)
                    .absolute()
                    .left(
                        layout.tabs.origin.x - clip.origin.x
                            + if vertical {
                                px(0.0)
                            } else {
                                strip.available()
                            },
                    )
                    .top(
                        layout.tabs.origin.y - clip.origin.y
                            + if vertical {
                                strip.available()
                            } else {
                                px(0.0)
                            },
                    )
                    .w(if vertical {
                        layout.tabs.size.width
                    } else {
                        CONTROL_SIZE
                    })
                    .h(CONTROL_SIZE)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .on_click(cx.listener(|view, _, window, cx| {
                        if let Err(error) = view.new_tab(window, cx) {
                            view.status = Some(error.to_string());
                            cx.notify();
                        }
                    }))
                    .child("+"),
            );
            if vertical {
                let handle = layout.sidebar_resize_handle(position);
                chrome = chrome.child(
                    div()
                        .id("sidebar-resize")
                        .absolute()
                        .left(handle.origin.x - clip.origin.x)
                        .top(handle.origin.y - clip.origin.y)
                        .w(handle.size.width)
                        .h(handle.size.height)
                        .cursor(gpui::CursorStyle::ResizeLeftRight)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|view, _, window, cx| {
                                view.cancel_reorder(window, cx);
                                view.resizing_sidebar = true;
                                cx.stop_propagation();
                            }),
                        ),
                );
            }
            root = root.child(chrome);
        }
        if let Some(drag) = &self.reorder
            && drag.dragging
        {
            let marker = strip.marker(strip.slot(drag.pointer));
            let preview = strip.preview(drag.pointer);
            let title = self
                .tabs
                .iter()
                .find(|tab| tab.id == drag.source.tab)
                .map(|tab| tab.title(cx))
                .unwrap_or_default();
            root = root.child(
                div()
                    .absolute()
                    .left(preview.origin.x)
                    .top(preview.origin.y)
                    .w(preview.size.width)
                    .h(preview.size.height)
                    .overflow_hidden()
                    .px_2()
                    .bg(background)
                    .border_1()
                    .border_color(foreground.opacity(0.5))
                    .opacity(0.8)
                    .child(title),
            );
            root = root.child(
                div()
                    .absolute()
                    .left(marker.origin.x)
                    .top(marker.origin.y)
                    .w(marker.size.width)
                    .h(marker.size.height)
                    .bg(foreground),
            );
        }
        if let Some(status) = &self.status {
            root = root.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .px_2()
                    .py_1()
                    .bg(background)
                    .child(status.clone()),
            );
        }
        if let Some(target) = self.close.confirmation {
            let unknown =
                self.close.assessment.as_ref().is_some_and(|assessment| {
                    assessment.jobs().contains(&huterm_core::JobState::Unknown)
                });
            let tab_title = match target {
                CloseTarget::Tab(id) => self
                    .tabs
                    .iter()
                    .find(|tab| tab.id == id)
                    .map(|tab| tab.title(cx))
                    .unwrap_or_default(),
                _ => String::new(),
            };
            let message = match (target, unknown) {
                (CloseTarget::Application, true) => {
                    "Some process state is unavailable. Quit Huterm and terminate all sessions?".to_owned()
                }
                (CloseTarget::Application, false) => {
                    "Quit Huterm and terminate running jobs in all sessions?".to_owned()
                }
                (CloseTarget::Window, true) => {
                    "Some process state is unavailable. Close this final view and terminate its session?".to_owned()
                }
                (CloseTarget::Window, false) => {
                    "Close this final view and terminate running jobs in its session?".to_owned()
                }
                (CloseTarget::Tab(_), true) => {
                    format!("Process state is unavailable. Close tab \"{tab_title}\" and terminate its terminal?")
                }
                (CloseTarget::Tab(_), false) => {
                    format!("Close tab \"{tab_title}\" and terminate its running jobs?")
                }
            };
            root = root.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(background.opacity(0.9))
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        div()
                            .w((window.viewport_size().width - px(32.0))
                                .clamp(px(0.0), px(460.0)))
                            .flex_none()
                            .p_4()
                            .bg(background)
                            .border_1()
                            .border_color(foreground.opacity(0.3))
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(div().w_full().flex_none().child(message))
                            .child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .justify_end()
                                    .gap_3()
                                    .child(
                                        div()
                                            .id("cancel-close")
                                            .px_3()
                                            .py_1()
                                            .cursor_pointer()
                                            .on_click(cx.listener(
                                                |view, _, window, cx| {
                                                    view.cancel_close(
                                                        window, cx,
                                                    );
                                                },
                                            ))
                                            .child("Cancel"),
                                    )
                                    .child(
                                        div()
                                            .id("confirm-close")
                                            .px_3()
                                            .py_1()
                                            .bg(foreground.opacity(0.15))
                                            .cursor_pointer()
                                            .on_click(cx.listener(
                                                move |view, _, window, cx| {
                                                    view.finish_close(
                                                        target, window, cx,
                                                    );
                                                },
                                            ))
                                            .child(
                                                if target
                                                    == CloseTarget::Application
                                                {
                                                    "Quit"
                                                } else {
                                                    "Close"
                                                },
                                            ),
                                    ),
                            ),
                    ),
            );
        }
        if let Some(palette) = &self.palette {
            root = root.child(palette.clone());
        }
        root
    }
}

#[cfg(target_os = "macos")]
pub(super) fn terminal_input_allowed(window: &Window, cx: &App) -> bool {
    window
        .root::<WorkspaceView>()
        .flatten()
        .is_some_and(|root| {
            let view = root.read(cx);
            !view.busy
                && view.close.confirmation.is_none()
                && view.reorder.is_none()
                && view.palette.is_none()
        })
}

#[cfg(target_os = "macos")]
pub(super) fn active_composition(window: &Window, cx: &App) -> bool {
    window
        .root::<WorkspaceView>()
        .flatten()
        .is_some_and(|root| {
            root.read(cx)
                .active_view()
                .is_some_and(|view| !view.read(cx).composition.is_empty())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(all(target_os = "macos", feature = "macos-updater")))]
    #[test]
    fn check_for_updates_reports_updater_build_requirement() {
        assert_eq!(
            unsupported_update_error(),
            CommandError::Unavailable(
                "self-updates are available only in updater-enabled macOS release builds"
                    .to_owned()
            )
        );
    }

    #[test]
    fn interactive_invocation_without_required_arguments_opens_the_palette() {
        let select_tab = CommandInvocation::new(ids::SELECT_TAB, Vec::new());
        assert!(interactive_spec(&select_tab).unwrap().1);

        let toggle_quake =
            CommandInvocation::new(ids::TOGGLE_QUAKE, Vec::new());
        assert!(!interactive_spec(&toggle_quake).unwrap().1);
    }

    #[test]
    fn programmatic_invocation_without_required_arguments_fails() {
        let invocation = CommandInvocation::new(ids::SELECT_TAB, Vec::new());
        assert_eq!(
            validate(&invocation),
            Err(CommandError::MissingArgument {
                command: ids::SELECT_TAB,
                name: "tab",
            })
        );
    }

    #[test]
    fn synchronous_dispatch_failure_lands_in_status_not_palette() {
        let mut status = None;
        let mut palette = Some(());
        set_dispatch_error(
            &mut palette,
            &mut status,
            &CommandError::StaleTarget,
        );
        assert!(palette.is_none());
        assert_eq!(status.as_deref(), Some("command target no longer exists"));
    }

    #[test]
    fn copy_is_unavailable_without_selection() {
        assert_eq!(
            copy_availability(None),
            Err(CommandError::Unavailable("no selection".to_owned()))
        );
        assert_eq!(
            copy_availability(Some(Selection {
                generation: 1,
                anchor: BufferPoint {
                    rows_from_live_bottom: 0,
                    column: 0,
                },
                head: BufferPoint {
                    rows_from_live_bottom: 0,
                    column: 1,
                },
            })),
            Ok(())
        );
    }

    #[test]
    fn hidden_and_every_overlay_frame_preserve_full_terminal_bounds() {
        for position in [
            TabPosition::Top,
            TabPosition::Bottom,
            TabPosition::Left,
            TabPosition::Right,
        ] {
            for viewport in [size(px(800.0), px(600.0)), size(px(3.0), px(2.0))]
            {
                let safe = gpui::Edges {
                    top: px(40.0),
                    left: px(5.0),
                    right: px(7.0),
                    bottom: px(3.0),
                };
                let hidden = ChromeLayout::with_safe_area(
                    viewport,
                    px(0.0),
                    position,
                    px(180.0),
                    safe,
                )
                .present(Presentation::Hidden, position, 0.0);
                let expected = if viewport.width == px(800.0) {
                    Bounds::new(
                        point(px(5.0), px(40.0)),
                        size(px(788.0), px(557.0)),
                    )
                } else {
                    Bounds::new(point(px(3.0), px(2.0)), size(px(0.0), px(0.0)))
                };
                assert_eq!(hidden.terminal, expected);
                for progress in [0.0, 0.1, 0.5, 0.9, 1.0] {
                    let overlay = ChromeLayout::with_safe_area(
                        viewport,
                        px(0.0),
                        position,
                        px(180.0),
                        safe,
                    )
                    .present(
                        Presentation::Overlay,
                        position,
                        progress,
                    );
                    assert_eq!(
                        overlay.terminal, expected,
                        "{position:?} at {progress}"
                    );
                }
            }
        }
    }

    #[test]
    fn exit_queue_retains_inactive_siblings_while_busy_and_cancel_consumes_only_current()
     {
        let first = TabId::new(1);
        let second = TabId::new(2);
        let mut queue = ExitQueue::default();
        let mut close = CloseState::default();
        close.begin_check(CloseTarget::Tab(TabId::new(3)));
        queue.observe(first, false, true, true);
        queue.observe(second, false, true, true);
        // Busy structural work leaves both requests queued, independent of active tab.
        assert_eq!(close.next_request(&mut queue, true, |_| true), None);
        close.cancel();
        let target = close.next_request(&mut queue, false, |_| true).unwrap();
        assert_eq!(target, CloseTarget::Tab(first));
        close.begin_check(target);
        assert_eq!(close.checked(true), Some(CloseDecision::Confirm(target)));
        assert_eq!(close.next_request(&mut queue, false, |_| true), None);
        assert_eq!(close.cancel(), Some(target));
        queue.observe(first, true, true, true);
        queue.observe(second, true, true, true);
        assert_eq!(queue.take_next(|_| true), Some(second));
        assert_eq!(queue.take_next(|_| true), None);
    }

    #[test]
    fn exit_queue_reload_affects_future_transitions_and_discards_removed_tabs()
    {
        let first = TabId::new(1);
        let second = TabId::new(2);
        let third = TabId::new(3);
        let mut queue = ExitQueue::default();
        queue.observe(first, false, true, false);
        queue.observe(first, true, true, true);
        queue.observe(second, false, true, true);
        queue.observe(third, false, true, true);
        assert_eq!(queue.take_next(|id| id != second), Some(third));
        assert_eq!(queue.take_next(|_| true), None);
    }

    #[test]
    fn reorder_projection_and_preview_stay_in_bar_for_all_four_placements() {
        for position in [
            TabPosition::Top,
            TabPosition::Bottom,
            TabPosition::Left,
            TabPosition::Right,
        ] {
            let layout = ChromeLayout::new(
                size(px(600.0), px(240.0)),
                px(32.0),
                position,
            );
            let strip =
                TabStrip::new(layout.tabs, position.vertical(), 8, px(0.0));
            let pointer = if strip.vertical {
                strip.bounds.origin + point(px(20.0), strip.extent * 1.1)
            } else {
                strip.bounds.origin + point(strip.extent * 1.1, px(10.0))
            };
            assert_eq!(strip.slot(pointer), 1, "{position:?}");
            for excursion in [-10_000.0, 10_000.0] {
                let perpendicular = pointer
                    + if strip.vertical {
                        point(px(excursion), px(0.0))
                    } else {
                        point(px(0.0), px(excursion))
                    };
                assert_eq!(strip.slot(perpendicular), 1, "{position:?}");
                let beyond = pointer
                    + if strip.vertical {
                        point(px(0.0), px(excursion))
                    } else {
                        point(px(excursion), px(0.0))
                    };
                let slot = strip.slot(beyond);
                assert_eq!(
                    slot,
                    if excursion < 0.0 {
                        0
                    } else {
                        strip.slot(
                            strip.bounds.origin
                                + point(
                                    strip.bounds.size.width,
                                    strip.bounds.size.height,
                                ),
                        )
                    }
                );
                for bounds in [
                    strip.preview(beyond),
                    strip.preview(perpendicular),
                    strip.marker(slot),
                ] {
                    assert!(
                        bounds.origin.x >= strip.bounds.origin.x
                            && bounds.origin.y >= strip.bounds.origin.y
                    );
                    assert!(
                        bounds.origin.x + bounds.size.width
                            <= strip.bounds.origin.x + strip.bounds.size.width
                    );
                    assert!(
                        bounds.origin.y + bounds.size.height
                            <= strip.bounds.origin.y + strip.bounds.size.height
                    );
                }
            }
        }
    }

    #[test]
    fn safe_area_keeps_tabs_and_terminal_below_notch_for_every_placement() {
        for position in [
            TabPosition::Top,
            TabPosition::Bottom,
            TabPosition::Left,
            TabPosition::Right,
        ] {
            let layout = ChromeLayout::with_safe_area(
                size(px(800.0), px(600.0)),
                px(0.0),
                position,
                SIDEBAR_WIDTH,
                gpui::Edges {
                    top: px(48.5),
                    ..Default::default()
                },
            );
            for bounds in [layout.tabs, layout.terminal] {
                assert!(
                    bounds.origin.y >= px(48.5),
                    "{position:?}: {bounds:?}"
                );
                assert!(bounds.bottom() <= px(600.0));
            }
            let area = f32::from(layout.tabs.size.width)
                * f32::from(layout.tabs.size.height)
                + f32::from(layout.terminal.size.width)
                    * f32::from(layout.terminal.size.height);
            assert!((area - 800.0 * 551.5).abs() < f32::EPSILON);
        }
    }

    #[test]
    fn safe_area_bounds_include_side_insets_and_clamp_small_windows() {
        let insets = gpui::Edges {
            top: px(48.5),
            right: px(6.0),
            bottom: px(8.0),
            left: px(4.0),
        };
        for position in [
            TabPosition::Top,
            TabPosition::Bottom,
            TabPosition::Left,
            TabPosition::Right,
        ] {
            for viewport in [size(px(800.0), px(600.0)), size(px(3.0), px(2.0))]
            {
                let layout = ChromeLayout::with_safe_area(
                    viewport,
                    px(0.0),
                    position,
                    SIDEBAR_WIDTH,
                    insets,
                );
                for bounds in [layout.tabs, layout.terminal] {
                    assert!(bounds.origin.x >= px(4.0).min(viewport.width));
                    assert!(bounds.origin.y >= px(48.5).min(viewport.height));
                    assert!(
                        bounds.size.width >= px(0.0)
                            && bounds.size.height >= px(0.0)
                    );
                    assert!(
                        bounds.right()
                            <= (viewport.width - px(6.0)).max(bounds.origin.x)
                    );
                    assert!(
                        bounds.bottom()
                            <= (viewport.height - px(8.0)).max(bounds.origin.y)
                    );
                }
            }
        }
    }

    #[test]
    fn sidebar_resize_handle_never_overlaps_terminal_input() {
        for position in [TabPosition::Left, TabPosition::Right] {
            for (width, preferred) in
                [(1000.0, 180.0), (300.0, 400.0), (3.0, 140.0)]
            {
                let layout = ChromeLayout::with_sidebar(
                    size(px(width), px(600.0)),
                    px(32.0),
                    position,
                    px(preferred),
                );
                let handle = layout.sidebar_resize_handle(position);
                let handle_end = handle.origin.x + handle.size.width;
                let terminal_end =
                    layout.terminal.origin.x + layout.terminal.size.width;
                assert!(
                    handle_end <= layout.terminal.origin.x
                        || handle.origin.x >= terminal_end,
                    "resize handle overlaps terminal: {position:?}, width {width}",
                );
                assert!(handle.origin.x >= layout.tabs.origin.x);
                assert!(
                    handle_end <= layout.tabs.origin.x + layout.tabs.size.width
                );
                assert_eq!(handle.origin.y, layout.tabs.origin.y);
                assert_eq!(handle.size.height, layout.tabs.size.height);
                assert_eq!(
                    handle.size.width,
                    px(6.0).min(layout.tabs.size.width)
                );
            }
        }
    }

    #[test]
    fn sidebar_width_clamps_to_window_without_losing_preference() {
        for position in [TabPosition::Left, TabPosition::Right] {
            let preferred = px(350.0);
            let wide = ChromeLayout::with_sidebar(
                size(px(1000.0), px(600.0)),
                px(32.0),
                position,
                preferred,
            );
            let narrow = ChromeLayout::with_sidebar(
                size(px(300.0), px(600.0)),
                px(32.0),
                position,
                preferred,
            );
            let restored = ChromeLayout::with_sidebar(
                size(px(1000.0), px(600.0)),
                px(32.0),
                position,
                preferred,
            );
            assert_eq!(wide.tabs.size.width, px(350.0));
            assert_eq!(narrow.tabs.size.width, px(150.0));
            assert_eq!(restored.tabs.size.width, px(350.0));
            let minimum = ChromeLayout::with_sidebar(
                size(px(1000.0), px(600.0)),
                px(32.0),
                position,
                px(20.0),
            );
            let maximum = ChromeLayout::with_sidebar(
                size(px(1000.0), px(600.0)),
                px(32.0),
                position,
                px(900.0),
            );
            assert_eq!(minimum.tabs.size.width, px(140.0));
            assert_eq!(maximum.tabs.size.width, px(400.0));
            assert_eq!(
                wide.terminal.size.width + wide.tabs.size.width,
                px(1000.0)
            );
        }
    }

    #[test]
    fn applying_canonical_order_retains_view_state_and_rejects_stale_reply() {
        let a = TabId::new(1);
        let b = TabId::new(2);
        let c = TabId::new(3);
        let active = b;
        let mut views = vec![
            (a, "selection-a", 123),
            (b, "selection-b", 456),
            (c, "selection-c", 789),
        ];
        assert!(apply_tab_order(&mut views, &[c, a, b], |view| view.0));
        assert_eq!(
            views,
            [
                (c, "selection-c", 789),
                (a, "selection-a", 123),
                (b, "selection-b", 456)
            ]
        );
        assert_eq!(views.iter().find(|view| view.0 == active).unwrap().2, 456);
        let before = views.clone();
        assert!(!apply_tab_order(&mut views, &[a, a, c], |view| view.0));
        assert!(!apply_tab_order(
            &mut views,
            &[a, b, TabId::new(4)],
            |view| view.0
        ));
        assert_eq!(views, before);
    }

    #[test]
    fn application_mouse_coordinates_follow_all_tab_placements() {
        use crate::config::TabPosition;
        let cell = size(px(8.0), px(16.0));
        for inset in [px(0.0), px(48.5)] {
            for placement in [
                TabPosition::Top,
                TabPosition::Bottom,
                TabPosition::Left,
                TabPosition::Right,
            ] {
                let chrome = ChromeLayout::with_safe_area(
                    size(px(800.0), px(600.0)),
                    px(32.0),
                    placement,
                    SIDEBAR_WIDTH,
                    gpui::Edges {
                        top: inset,
                        ..Default::default()
                    },
                );
                let layout = TerminalLayout::new(
                    chrome.terminal.size,
                    cell,
                    WindowConfig::default(),
                );
                let start = chrome.terminal.origin + layout.bounds.origin;
                assert_eq!(
                    application_mouse_geometry(
                        start,
                        chrome.terminal.origin,
                        layout,
                        cell
                    ),
                    (true, MousePosition::default()),
                    "{placement:?}"
                );
                assert_eq!(
                    application_mouse_geometry(
                        start + point(px(8.0), px(16.0)),
                        chrome.terminal.origin,
                        layout,
                        cell
                    ),
                    (true, MousePosition { column: 1, row: 1 }),
                    "{placement:?}"
                );
                let tab_center = chrome.tabs.origin
                    + point(
                        chrome.tabs.size.width / 2.0,
                        chrome.tabs.size.height / 2.0,
                    );
                assert!(
                    !application_mouse_geometry(
                        tab_center,
                        chrome.terminal.origin,
                        layout,
                        cell
                    )
                    .0,
                    "{placement:?}"
                );
            }
        }
    }

    fn lifecycle_command() -> TerminalCommand {
        TerminalCommand {
            engine: huterm_protocol::TerminalEngineKind::default(),
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), "printf READY; read value".into()],
            working_directory: std::env::current_dir().unwrap(),
            environment: Vec::new(),
            grid_size: GridSize::clamped(40, 8),
            cell_size: CellSize {
                width: 8,
                height: 16,
            },
        }
    }

    #[test]
    fn attachment_allocation_failure_rolls_back_published_pty() {
        let runtime = DesktopRuntime::default();
        runtime.mux.lock().unwrap().reserve_through(u64::MAX - 5);
        assert!(matches!(
            runtime.open_tab(None, &lifecycle_command()),
            Err(MuxError::IdExhausted)
        ));
        let mux = runtime.mux.lock().unwrap();
        assert!(mux.sessions().is_empty());
        assert_eq!(mux.terminal_count(), 0);
    }

    #[test]
    fn orphaned_publication_preserves_another_attachment_and_transferred_tab() {
        let runtime = DesktopRuntime::default();
        let (session, workspace, opened, attachment) =
            runtime.open_tab(None, &lifecycle_command()).unwrap();
        let second =
            runtime.mux.lock().unwrap().attach_session(session).unwrap();
        runtime.cleanup_spawn(session, workspace, opened.tab.id, attachment);
        assert_eq!(
            runtime
                .mux
                .lock()
                .unwrap()
                .attachment_session(second)
                .unwrap(),
            session
        );
        assert!(runtime.mux.lock().unwrap().tab(opened.tab.id).is_some());
        let (source, source_workspace, transferred, initial) =
            runtime.open_tab(None, &lifecycle_command()).unwrap();
        runtime
            .mux
            .lock()
            .unwrap()
            .move_tab(source_workspace, transferred.tab.id, workspace, None)
            .unwrap();
        runtime.cleanup_spawn(
            source,
            source_workspace,
            transferred.tab.id,
            initial,
        );
        let mux = runtime.mux.lock().unwrap();
        assert!(mux.session(source).is_none());
        assert_eq!(
            mux.select_tab(transferred.tab.id).unwrap().session,
            session
        );
        drop(mux);
        runtime.terminate().unwrap();
    }

    #[test]
    fn orphaned_publication_never_terminates_a_retargeted_destination() {
        let runtime = DesktopRuntime::default();
        let (source, workspace, opened, attachment) =
            runtime.open_tab(None, &lifecycle_command()).unwrap();
        let destination = runtime
            .mux
            .lock()
            .unwrap()
            .create_session(Some("survivor"))
            .unwrap();
        runtime
            .mux
            .lock()
            .unwrap()
            .retarget_attachment(attachment.unwrap(), destination)
            .unwrap();
        runtime.cleanup_spawn(source, workspace, opened.tab.id, attachment);
        let mux = runtime.mux.lock().unwrap();
        assert!(mux.session(source).is_none());
        assert!(mux.session(destination).is_some());
        assert!(mux.capture_hierarchy().attachments.is_empty());
    }

    #[test]
    fn quit_captures_zero_view_hierarchy_and_window_navigation_once() {
        let runtime = DesktopRuntime::default();
        let mut mux = runtime.mux.lock().unwrap();
        let visible = mux.create_session(Some("visible")).unwrap();
        let workspace = mux.create_workspace(visible, None).unwrap();
        let attachment = mux.attach_session(visible).unwrap();
        let unviewed = mux.create_session(Some("unviewed")).unwrap();
        mux.create_workspace(unviewed, Some("retained")).unwrap();
        drop(mux);
        let bounds = WindowBounds::Windowed(Bounds {
            origin: point(px(5.0), px(10.0)),
            size: size(px(800.0), px(600.0)),
        });
        let windows = vec![WindowRestore {
            attachment,
            workspace: Some(workspace),
            active: None,
            bounds,
            tab_position: TabPosition::Left,
            sidebar_width: px(220.0),
            tab_scroll: px(24.0),
        }];
        let assessment = runtime.assess(CloseRequest::Application).unwrap();
        runtime.commit(&assessment, false, Some(windows)).unwrap();
        runtime.terminate().unwrap();
        let restore = runtime.restore.lock().unwrap();
        let restore = restore.as_ref().unwrap();
        assert_eq!(
            restore
                .hierarchy
                .sessions
                .iter()
                .map(|s| s.id)
                .collect::<Vec<_>>(),
            vec![visible, unviewed]
        );
        assert_eq!(restore.hierarchy.workspaces.len(), 2);
        assert_eq!(restore.windows.len(), 1);
        assert_eq!(restore.windows[0].workspace, Some(workspace));
        assert_eq!(restore.windows[0].sidebar_width, px(220.0));
        assert_eq!(restore.windows[0].tab_scroll, px(24.0));
        assert!(runtime.mux.lock().unwrap().sessions().is_empty());
    }

    #[test]
    fn rejected_quit_never_sets_termination_gate_or_capture() {
        let runtime = DesktopRuntime::default();
        runtime.mux.lock().unwrap().create_session(None).unwrap();
        let assessment = runtime.assess(CloseRequest::Application).unwrap();
        runtime
            .mux
            .lock()
            .unwrap()
            .create_session(Some("new survivor"))
            .unwrap();
        assert!(matches!(
            runtime.commit(&assessment, false, Some(Vec::new())),
            Err(MuxError::StaleClose)
        ));
        assert!(!runtime.terminating.load(Ordering::Acquire));
        assert!(runtime.restore.lock().unwrap().is_none());
        assert_eq!(runtime.mux.lock().unwrap().sessions().len(), 2);
        let assessment = runtime.assess(CloseRequest::Application).unwrap();
        runtime
            .commit(&assessment, false, Some(Vec::new()))
            .unwrap();
        assert!(runtime.terminating.load(Ordering::Acquire));
        assert_eq!(
            runtime
                .restore
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .hierarchy
                .sessions
                .len(),
            2
        );
        assert!(runtime.mux.lock().unwrap().sessions().is_empty());
    }

    #[test]
    fn cancellation_invalidates_assessment_generation_and_application_intent() {
        let mut state = CloseState::default();
        state.begin_check(CloseTarget::Application);
        let generation = state.generation;
        assert_eq!(state.cancel(), Some(CloseTarget::Application));
        assert_ne!(state.generation, generation);
        assert_eq!(state.checked(true), None);
    }

    #[test]
    fn private_session_spawn_failure_rolls_back_and_cleanup_keeps_siblings() {
        let runtime = DesktopRuntime::default();
        let mut command = TerminalCommand {
            engine: huterm_protocol::TerminalEngineKind::Alacritty,
            program: "/huterm-nonexistent-shell".into(),
            arguments: vec!["-c".into(), "printf READY; read value".into()],
            working_directory: std::env::current_dir().unwrap(),
            environment: Vec::new(),
            grid_size: GridSize::clamped(40, 8),
            cell_size: CellSize {
                width: 8,
                height: 16,
            },
        };
        assert!(runtime.open_tab(None, &command).is_err());
        assert!(runtime.mux.lock().unwrap().sessions().is_empty());
        assert_eq!(runtime.mux.lock().unwrap().terminal_count(), 0);
        command.program = "/bin/sh".into();
        let (session, workspace, first, _) =
            runtime.open_tab(None, &command).unwrap();
        let (sibling, _, second, _) = runtime.open_tab(None, &command).unwrap();
        assert_ne!(session, sibling);
        command.program = "/huterm-nonexistent-shell".into();
        assert!(runtime.open_tab(None, &command).is_err());
        assert!(runtime.open_tab(Some(workspace), &command).is_err());
        assert_eq!(runtime.mux.lock().unwrap().sessions().len(), 2);
        assert_eq!(
            runtime
                .mux
                .lock()
                .unwrap()
                .workspace(workspace)
                .unwrap()
                .tabs,
            [first.tab]
        );
        runtime.mux.lock().unwrap().close_session(session).unwrap();
        assert!(runtime.mux.lock().unwrap().workspace(workspace).is_none());
        assert_eq!(runtime.mux.lock().unwrap().sessions().len(), 1);
        assert!(matches!(
            first.client.read_snapshot(),
            Err(RuntimeError::Stopped)
        ));
        assert!(second.client.read_snapshot().is_ok());
        runtime.mux.lock().unwrap().close_session(sibling).unwrap();
        assert!(runtime.mux.lock().unwrap().sessions().is_empty());
        assert_eq!(runtime.mux.lock().unwrap().terminal_count(), 0);
        runtime.mux.lock().unwrap().reserve_through(u64::MAX - 1);
        assert!(matches!(
            runtime.open_tab(None, &command),
            Err(MuxError::IdExhausted)
        ));
        assert!(runtime.mux.lock().unwrap().sessions().is_empty());
    }

    #[test]
    fn native_termination_drains_existing_terminals_and_rejects_queued_spawns()
    {
        let runtime = Arc::new(DesktopRuntime::default());
        let command = TerminalCommand {
            engine: huterm_protocol::TerminalEngineKind::Alacritty,
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), "printf READY; read value".into()],
            working_directory: std::env::current_dir().unwrap(),
            environment: Vec::new(),
            grid_size: GridSize::clamped(40, 8),
            cell_size: CellSize {
                width: 8,
                height: 16,
            },
        };
        let (_, _, opened, _) = runtime.open_tab(None, &command).unwrap();
        // Hold the structural lock as an already-running spawn would, then
        // queue another spawn and invoke the exact native-hook cleanup method.
        let guard = runtime.mux.lock().unwrap();
        let spawn_runtime = Arc::clone(&runtime);
        let spawn =
            std::thread::spawn(move || spawn_runtime.open_tab(None, &command));
        let quit_runtime = Arc::clone(&runtime);
        let (finished, completion) = std::sync::mpsc::channel();
        let quit = std::thread::spawn(move || {
            quit_runtime.terminate().unwrap();
            finished.send(()).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while !runtime.terminating.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "quit did not start");
            std::thread::yield_now();
        }
        assert!(
            completion.try_recv().is_err(),
            "quit must await the structural owner"
        );
        drop(guard);
        assert!(matches!(
            spawn.join().unwrap(),
            Err(MuxError::Runtime(RuntimeError::Stopped))
        ));
        quit.join().unwrap();
        assert_eq!(runtime.mux.lock().unwrap().terminal_count(), 0);
        assert!(matches!(
            opened.client.read_snapshot(),
            Err(RuntimeError::Stopped)
        ));
        runtime.terminate().unwrap();
    }

    #[test]
    fn queued_quit_after_final_window_close_admits_a_confirmation_host() {
        let runtime = DesktopRuntime::default();
        let mut mux = runtime.mux.lock().unwrap();
        let visible = mux.create_session(None).unwrap();
        let attachment = mux.attach_session(visible).unwrap();
        let unviewed = mux.create_session(Some("keep alive")).unwrap();
        drop(mux);
        let assessment =
            runtime.assess(CloseRequest::Window(attachment)).unwrap();
        let mut close = CloseState::default();
        close.begin_check(CloseTarget::Window);
        close.current = Some(CloseTarget::Window);
        close.queue(CloseTarget::Application);
        runtime.commit(&assessment, false, None).unwrap();
        assert_eq!(
            close.take_pending(|_| false),
            Some(CloseTarget::Application)
        );
        assert_eq!(
            runtime
                .mux
                .lock()
                .unwrap()
                .sessions()
                .iter()
                .map(|session| session.id)
                .collect::<Vec<_>>(),
            vec![unviewed]
        );
        // Production retains quitting=true while removing the final window.
        assert!(
            can_open_window(false, true, false),
            "ongoing Quit lost its confirmation host"
        );
        assert!(
            !can_open_window(true, true, false),
            "Quit admitted a new shell"
        );
    }

    #[test]
    fn widening_a_tab_check_rechecks_all_targets_before_confirmation() {
        let mut close = CloseState::default();
        close.begin_check(CloseTarget::Tab(TabId::new(1)));
        close.queue(CloseTarget::Application);
        assert_eq!(
            close.checked(false),
            Some(CloseDecision::Check(CloseTarget::Application))
        );
        close.begin_check(CloseTarget::Application);
        assert_eq!(
            close.checked(true),
            Some(CloseDecision::Confirm(CloseTarget::Application))
        );
        assert_eq!(close.cancel(), Some(CloseTarget::Application));
    }

    #[test]
    fn close_check_completion_preserves_application_and_window_scope() {
        for (current, later) in [
            (CloseTarget::Application, CloseTarget::Window),
            (CloseTarget::Application, CloseTarget::Tab(TabId::new(1))),
            (CloseTarget::Window, CloseTarget::Tab(TabId::new(1))),
        ] {
            let mut close = CloseState::default();
            close.begin_check(current);
            close.queue(later);
            assert_eq!(
                close.checked(true),
                Some(CloseDecision::Confirm(current))
            );
            assert_eq!(
                close.cancel(),
                Some(current),
                "cancel must retain application scope so it clears quitting"
            );
            assert!(close.pending.is_none());
        }
    }

    #[test]
    fn tab_removal_precedes_queued_window_confirmation_and_cancel() {
        let (first, second) = (TabId::new(1), TabId::new(2));
        let mut tabs = vec![first, second];
        let mut active = Some(first);
        let mut close = CloseState::default();
        close.begin_check(CloseTarget::Tab(first));
        assert_eq!(
            close.checked(false),
            Some(CloseDecision::Close(CloseTarget::Tab(first)))
        );
        close.queue(CloseTarget::Window);
        remove_tab(&mut tabs, &mut active, first, |id| *id);
        let next = close.take_pending(|id| tabs.contains(&id)).unwrap();
        close.begin_check(next);
        assert_eq!(
            close.checked(true),
            Some(CloseDecision::Confirm(CloseTarget::Window))
        );
        close.cancel();
        assert_eq!(active, Some(second));
        assert!(tabs.contains(&active.unwrap()));
    }

    #[test]
    fn activation_history_tracks_selection_and_prunes_closed_tabs() {
        let (first, second, third) =
            (TabId::new(1), TabId::new(2), TabId::new(3));
        let mut history = Vec::new();

        record_tab_activation(&mut history, first);
        record_tab_activation(&mut history, second);
        record_tab_activation(&mut history, third);
        record_tab_activation(&mut history, first);
        assert_eq!(history, [first, third, second]);

        prune_tab_history(&mut history, third);
        assert_eq!(history, [first, second]);
    }

    #[test]
    fn select_recent_tab_toggles_between_two_tabs_and_is_a_no_op_alone() {
        let (first, second) = (TabId::new(1), TabId::new(2));
        let tabs = [first, second];
        let mut active = Some(second);
        let mut history = vec![second, first];

        let selected = recent_tab(&history, active, |id| tabs.contains(&id));
        assert_eq!(selected, Some(first));
        active = selected;
        record_tab_activation(&mut history, first);

        let selected = recent_tab(&history, active, |id| tabs.contains(&id));
        assert_eq!(selected, Some(second));
        active = selected;
        record_tab_activation(&mut history, second);

        assert_eq!(active, Some(second));
        assert_eq!(history, [second, first]);
        assert_eq!(recent_tab(&[first], Some(first), |id| id == first), None);
    }

    #[test]
    fn repeated_tab_close_is_discarded_after_removal() {
        let (first, second) = (TabId::new(1), TabId::new(2));
        let mut tabs = vec![first, second];
        let mut active = Some(first);
        let mut close = CloseState::default();
        close.queue(CloseTarget::Tab(first));
        remove_tab(&mut tabs, &mut active, first, |id| *id);
        assert_eq!(close.take_pending(|id| tabs.contains(&id)), None);
        assert_eq!(active, Some(second));
        remove_tab(&mut tabs, &mut active, first, |id| *id);
        assert_eq!(tabs, vec![second]);
        assert_eq!(active, Some(second));
    }

    #[test]
    fn queued_close_preserves_the_widest_requested_scope() {
        let tab = CloseTarget::Tab(TabId::new(1));
        assert_eq!(merge_close(None, tab), tab);
        assert_eq!(
            merge_close(Some(tab), CloseTarget::Window),
            CloseTarget::Window
        );
        assert_eq!(
            merge_close(Some(CloseTarget::Window), tab),
            CloseTarget::Window
        );
        assert_eq!(
            merge_close(Some(CloseTarget::Application), CloseTarget::Window),
            CloseTarget::Application
        );
        assert_eq!(
            merge_close(Some(tab), CloseTarget::Application),
            CloseTarget::Application
        );
    }

    #[test]
    fn all_placements_share_nonoverlapping_terminal_and_tab_bounds() {
        for position in [
            TabPosition::Top,
            TabPosition::Bottom,
            TabPosition::Left,
            TabPosition::Right,
        ] {
            for titlebar in [px(0.0), px(32.0)] {
                let layout = ChromeLayout::new(
                    size(px(800.0), px(600.0)),
                    titlebar,
                    position,
                );
                assert_eq!(
                    layout.terminal.size.width
                        * f32::from(layout.terminal.size.height)
                        + layout.tabs.size.width
                            * f32::from(layout.tabs.size.height),
                    px(800.0) * f32::from(px(600.0) - titlebar)
                );
                match position {
                    TabPosition::Top => {
                        assert_eq!(layout.tabs.bottom(), layout.terminal.top());
                    }
                    TabPosition::Bottom => {
                        assert_eq!(layout.terminal.bottom(), layout.tabs.top());
                    }
                    TabPosition::Left => {
                        assert_eq!(layout.tabs.right(), layout.terminal.left());
                    }
                    TabPosition::Right => {
                        assert_eq!(layout.terminal.right(), layout.tabs.left());
                    }
                }
                assert!(layout.terminal.top() >= titlebar);
            }
        }
    }

    #[test]
    fn tiny_windows_keep_nonnegative_bounds_and_a_bounded_sidebar() {
        for position in [
            TabPosition::Top,
            TabPosition::Bottom,
            TabPosition::Left,
            TabPosition::Right,
        ] {
            let layout =
                ChromeLayout::new(size(px(3.0), px(2.0)), px(32.0), position);
            assert!(layout.terminal.size.width >= px(0.0));
            assert!(layout.terminal.size.height >= px(0.0));
            assert!(layout.terminal.bottom() <= px(2.0));
            assert!(layout.terminal.right() <= px(3.0));
            if position.vertical() {
                assert!(layout.tabs.size.width <= px(1.5));
            }
        }
    }

    #[test]
    fn native_tab_shortcuts_match_actions_and_are_reserved_from_terminal_input()
    {
        for platform in [Platform::MacOs, Platform::Linux] {
            let macos = platform == Platform::MacOs;
            let compiled = keymap::compile(platform, &[]).unwrap();
            let prefix = if macos { "cmd" } else { "ctrl-shift" };
            for (chord, command) in [
                (format!("{prefix}-n"), ids::NEW_WINDOW),
                (format!("{prefix}-t"), ids::NEW_TAB),
                (format!("{prefix}-w"), ids::CLOSE_TAB),
                ("ctrl-tab".into(), ids::NEXT_TAB),
                ("ctrl-shift-tab".into(), ids::PREVIOUS_TAB),
                (
                    format!("{}-9", if macos { "cmd" } else { "alt" }),
                    ids::SELECT_TAB,
                ),
            ] {
                let key = Keystroke::parse(&chord).unwrap();
                let matched = compiled
                    .bindings
                    .iter()
                    .filter(|binding| {
                        binding.match_keystrokes(std::slice::from_ref(&key))
                            == Some(false)
                    })
                    .map(keymap::bound_command)
                    .collect::<Vec<_>>();
                assert_eq!(matched, vec![Some(command)], "{chord}");
                assert!(compiled.reserved.is_reserved(&key), "{chord}");
            }
        }
    }

    #[test]
    fn hidden_tabs_coalesce_invalidations_without_requesting_snapshots() {
        let mut scroll = ScrollController::default();
        for _ in 0..100 {
            scroll.invalidate();
            assert!(begin_visible_snapshot(&mut scroll, false).is_none());
        }
        assert_eq!(scroll.diagnostics().requests_started, 0);
        assert!(begin_visible_snapshot(&mut scroll, true).is_some());
        assert_eq!(scroll.diagnostics().requests_started, 1);
    }
}
