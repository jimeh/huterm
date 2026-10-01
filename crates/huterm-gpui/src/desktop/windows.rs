#[cfg(target_os = "macos")]
#[path = "clipboard_smoke.rs"]
pub(crate) mod clipboard_smoke;
#[path = "fullscreen_smoke.rs"]
pub(crate) mod fullscreen_smoke;
#[cfg(target_os = "macos")]
#[path = "idle_bench.rs"]
pub(crate) mod idle_bench;
#[cfg(target_os = "macos")]
#[path = "input_smoke.rs"]
pub(crate) mod input_smoke;
#[path = "integration_smoke.rs"]
pub(crate) mod integration_smoke;
#[path = "palette_smoke.rs"]
pub(crate) mod palette_smoke;
#[path = "presentation_query_smoke.rs"]
pub(crate) mod presentation_query_smoke;
#[path = "quake_smoke.rs"]
pub(crate) mod quake_smoke;
#[path = "quake_windows.rs"]
mod quake_windows;
#[path = "refresh_smoke.rs"]
pub(crate) mod refresh_smoke;
#[cfg(all(target_os = "macos", feature = "macos-updater"))]
#[path = "updater_smoke.rs"]
pub(crate) mod updater_smoke;
use super::about::{AboutDetails, BuildFacts, about_details, render_about};
use super::close_dialog::{
    CloseDialogInput, CloseDialogTarget, DialogFocus, ProcessGroup,
    ProcessGroupState, ProcessRow, build_close_dialog, render_close_dialog,
};
use super::menu::{
    MENU_MIN_WIDTH, Menu as MenuView, MenuAnchor, MenuEvent, MenuModel,
    MenuRow, place_menu,
};
use super::notices::{
    Lifetime, NoticeContent, NoticeId, NoticeSource, NoticeStack,
    ToastHandlers, render_notice_stack,
};
use super::overlay::{OverlayColors, Swatch, TextTooltip};
use super::palette::{
    CommandFrequency, CommandPalette, HistoryView, OwnedHistory, PaletteEvent,
    PaletteHierarchy, PaletteOpen, PaletteScope, PaletteTarget,
    QuakeProfileRow, RecentCommands,
};
use super::*;
use crate::commands::{Route, fill_rename_target, route, select_tab_slot};
use crate::config::TabPosition;
use crate::fullscreen::{Effect, FullscreenController, ToggleIntent};
#[cfg(target_os = "macos")]
use crate::native_quit;
#[cfg(all(target_os = "macos", feature = "macos-updater"))]
use crate::native_updater;
use gpui::{AnyWindowHandle, Entity, Global, WeakEntity};
use huterm_config::{TabStyle, TabWidth, TabsConfig};
use huterm_core::{
    CloseAssessment, CloseRequest, DesktopHostEffectClient, HierarchySnapshot,
    HierarchySubscription, HostEffectRecipientOptions, MuxError, OpenedTab,
};
use huterm_protocol::{
    AttachmentId, CommandArgument, CommandScope, HierarchyState, SessionId,
    TerminalId, Touched, WorkspaceId, catalog, resolve_tab_name, validate,
    validate_supplied,
};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const TAB_HEIGHT: Pixels = px(32.0);
pub(super) const SIDEBAR_WIDTH: Pixels = px(180.0);
mod button_layout;
mod client_frame;
mod model;
mod projection;
mod tab_bar;
mod tab_menu;
mod tab_position;
mod tab_strip;
pub(super) mod tab_visibility;
mod terminal_menu;
mod views;
mod window_menu;
use crate::assets::Icon;
use crate::ui::scrollbar::{
    Axis, Edge, HitBand, Origin, Press, ScrollbarGeometries, ScrollbarGeometry,
    ScrollbarOptions, Scrollbars, ThumbSize, TrackMargins, TrackPress,
};
use button_layout::{
    ButtonGroup, ButtonLayout, WINDOW_CONTROL_GAP, WINDOW_CONTROL_SIZE,
    WINDOW_CONTROLS_PADDING_INNER, WINDOW_CONTROLS_PADDING_OUTER, WindowButton,
};
use client_frame::{
    ClientFrame, FRAME_RADIUS, FrameState, requested_decorations, resize_cursor,
};
use model::{
    QuakeRecord, TabEntry, TitleScope, WindowLayout, WindowModel, WindowRecord,
    WindowRestore, apply_tab_order,
};
use projection::{
    Drained, Install, Projection, ReconcileTarget, Resolution, TabNames,
    TitleConsumers, Wait, WindowChange, installed_order, reactivation,
    reconcile_window, tab_names, windows_to_visit,
};
use tab_bar::{
    Activity, PILL_HEIGHT, PILL_INSET, PILL_MARGIN_LEFT, PILL_MARGIN_RIGHT,
    TabColors, TabItem, TabStatus, VERTICAL_ROW_MARGIN_X,
    VERTICAL_ROW_MARGIN_Y, icon_element, tab_bar_height, top_chrome_uses_bar,
};
use tab_menu::{TabMenuInput, tab_menu_model};
use tab_position::{TabHost, resolve_tab_position};
use tab_strip::{TabExtents, TabStrip};
use tab_visibility::{Presentation, Reveal};
use terminal_menu::{DirectoryState, TerminalMenuInput, terminal_menu_model};
use views::{Views, broadcast, update_windows};
use window_menu::{
    MenuButtonPlacement, WindowMenuInput, menu_button_placement,
    split_new_tab_row, window_menu_model,
};
/// Space the tab strip reserves for the new-tab control on its axis.
const CONTROL_SLOT: Pixels = TAB_HEIGHT;
/// Visible size of the new-tab and scroll controls. Along the strip they sit
/// centered in their slot; across it they center in the bar, so a Pill bar
/// gives them the pill's inset.
const CONTROL_SIZE: Pixels = PILL_HEIGHT;
const CONTROL_INSET: Pixels = px(3.0);
/// Width the macOS traffic lights take at the start of the title strip; the
/// strip's title and a merged tab row start after it.
const TRAFFIC_LIGHT_INSET: Pixels = px(84.0);
/// Where the tabs start in the title row Huterm draws on Linux, which has
/// no traffic lights.
const TITLE_ROW_LEAD: Pixels = px(8.0);
/// Space kept below a vertical column's new-tab button when tabs overflow,
/// matching the rows' horizontal inset.
const VERTICAL_END_MARGIN: Pixels = px(5.0);
/// Width of the grab zone along a vertical tab bar's terminal edge. The
/// column scrollbar sits just inboard of it, so it stays narrow.
const SIDEBAR_HANDLE_WIDTH: f32 = 4.0;
/// Tab bar indicators fade sooner than the terminal's: the bar is small and
/// the indicator would otherwise linger over its tabs.
const TAB_SCROLLBAR_HOLD: Duration = Duration::from_millis(700);
/// The overlay scrollbar on a vertical tab column: pixel offsets from the
/// top, a jump-to-pointer track, and hover expansion like the palette list.
/// It sits inboard of the resize handle. On the left, its target stops at
/// the handle; on the right, the handle is on the far side, so the target
/// reaches the window edge.
fn tab_column_scrollbar(position: TabPosition) -> ScrollbarOptions {
    ScrollbarOptions {
        edge: Edge::Right,
        origin: Origin::Start,
        expand_on_hover: true,
        track_press: TrackPress::Jump,
        margins: TrackMargins::EVEN,
        thumb: ThumbSize::Slim,
        edge_inset: SIDEBAR_HANDLE_WIDTH,
        // No inward reach: the rows' close buttons sit just inside.
        hit: HitBand {
            outward: if position == TabPosition::Left {
                0.0
            } else {
                SIDEBAR_HANDLE_WIDTH
            },
            inward: 0.0,
        },
        // Hover reveal here made close-button hovers flash the scrollbar.
        reveal_on_hover: false,
        hold: TAB_SCROLLBAR_HOLD,
    }
}
/// The hairline position indicator along a horizontal tab bar's bottom
/// edge: it never expands or shows a track, but its thumb still drags and
/// the track jumps. Its ends align with the tabs: a point in from the bar's
/// edge for Strip, and on the pills' visible edges for Pill.
fn tab_row_scrollbar(style: TabStyle) -> ScrollbarOptions {
    ScrollbarOptions {
        edge: Edge::Bottom,
        origin: Origin::Start,
        expand_on_hover: false,
        track_press: TrackPress::Jump,
        margins: match style {
            // One point clear of the window's own edge outline.
            TabStyle::Strip => TrackMargins {
                start: 1.0,
                end: 0.0,
                padding: 0.0,
            },
            TabStyle::Pill => TrackMargins {
                start: f32::from(PILL_MARGIN_LEFT),
                end: f32::from(PILL_MARGIN_RIGHT),
                padding: 0.0,
            },
        },
        thumb: ThumbSize::Points(2.0),
        edge_inset: 0.0,
        // Only the line itself; the tabs and chevrons above it keep their
        // presses.
        hit: HitBand::THUMB,
        reveal_on_hover: false,
        hold: TAB_SCROLLBAR_HOLD,
    }
}
const TAB_DRAG_THRESHOLD: f64 = 4.0;

#[derive(Default)]
struct DesktopRuntime {
    mux: Mutex<Mux>,
    host_effects: DesktopHostEffectClient,
    terminating: AtomicBool,
    restore: Mutex<Option<RestoreSnapshot>>,
}

/// A structural result with the hierarchy sequence read under the same
/// lock before releasing it, whether the operation succeeded or failed after
/// committing. Completions wait for the projection to reach `seq`.
struct Committed<T> {
    result: T,
    seq: u64,
}

/// What `DesktopRuntime::open_tab` publishes to the spawning window.
type Spawned = (
    SessionId,
    WorkspaceId,
    OpenedTab,
    Option<AttachmentId>,
    TerminalViewAuthority,
);

impl DesktopRuntime {
    fn lock(&self) -> std::sync::MutexGuard<'_, Mux> {
        self.mux
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Captures the hierarchy and subscribes to every later event.
    fn subscribe_hierarchy(&self) -> (HierarchyState, HierarchySubscription) {
        self.lock().subscribe_hierarchy()
    }

    fn open_tab(
        &self,
        workspace: Option<WorkspaceId>,
        attachment: Option<AttachmentId>,
        command: &TerminalCommand,
        clipboard_allowed: bool,
    ) -> Committed<Result<Spawned, MuxError>> {
        let mut mux = self.lock();
        let result = self.open_tab_locked(
            &mut mux,
            workspace,
            attachment,
            command,
            clipboard_allowed,
        );
        Committed {
            result,
            seq: mux.hierarchy_seq(),
        }
    }

    fn open_tab_locked(
        &self,
        mux: &mut Mux,
        workspace: Option<WorkspaceId>,
        attachment: Option<AttachmentId>,
        command: &TerminalCommand,
        clipboard_allowed: bool,
    ) -> Result<Spawned, MuxError> {
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
                let created_attachment = if workspace.is_none() {
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
                let recipient_attachment = created_attachment.or(attachment);
                let Some(recipient_attachment) = recipient_attachment else {
                    let _ = mux.close_tab(id, tab.tab.id);
                    return Err(MuxError::HostEffectRegistrationUnavailable);
                };
                let recipient = match mux.register_host_effect_recipient(
                    recipient_attachment,
                    tab.tab.terminal_id,
                    &self.host_effects,
                    HostEffectRecipientOptions::local_desktop(
                        clipboard_allowed,
                    ),
                ) {
                    Ok(recipient) => recipient,
                    Err(error) => {
                        if workspace.is_none() {
                            let _ = mux.close_session(session);
                        } else {
                            let _ = mux.close_tab(id, tab.tab.id);
                        }
                        return Err(error);
                    }
                };
                let presentation = match mux.register_presentation_controller(
                    recipient_attachment,
                    tab.tab.terminal_id,
                ) {
                    Ok(presentation) => presentation,
                    Err(error) => {
                        if workspace.is_none() {
                            let _ = mux.close_session(session);
                        } else {
                            let _ = mux.close_tab(id, tab.tab.id);
                        }
                        return Err(error);
                    }
                };
                Ok((
                    session,
                    id,
                    tab,
                    created_attachment,
                    TerminalViewAuthority {
                        presentation,
                        host_effects: recipient,
                    },
                ))
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

    /// Removes an unpublished spawn's resources; returns the sequence the
    /// cleanup committed at.
    fn cleanup_spawn(
        &self,
        original_session: SessionId,
        workspace: WorkspaceId,
        tab: TabId,
        attachment: Option<AttachmentId>,
    ) -> u64 {
        let mut mux = self.lock();
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
        mux.hierarchy_seq()
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
    ) -> Committed<Result<(), MuxError>> {
        let current = assessment.recheck();
        let mut mux = self.lock();
        let result =
            mux.commit_close_with(assessment, &current, confirmed, |mux| {
                if let Some(windows) = windows {
                    self.terminating.store(true, Ordering::Release);
                    self.capture(mux, windows);
                }
            });
        Committed {
            result,
            seq: mux.hierarchy_seq(),
        }
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
    ) -> Committed<Result<CommandOutcome, CommandError>> {
        let mut mux = self.lock();
        let result = Self::execute_locked(
            &mut mux,
            invocation,
            self.terminating.load(Ordering::Acquire),
        );
        Committed {
            result,
            seq: mux.hierarchy_seq(),
        }
    }

    fn execute_locked(
        mux: &mut Mux,
        invocation: &CommandInvocation,
        terminating: bool,
    ) -> Result<CommandOutcome, CommandError> {
        if terminating {
            return Err(CommandError::Unavailable(
                "runtime is terminating".to_owned(),
            ));
        }
        let mut invocation = invocation.clone();
        if invocation.id == ids::RENAME_SESSION {
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
        huterm_core::execute(mux, &invocation)
    }

    fn reorder_tab(
        &self,
        workspace: WorkspaceId,
        tab: TabId,
        before: Option<TabId>,
    ) -> Committed<Result<(), MuxError>> {
        let mut mux = self.lock();
        let result = if self.terminating.load(Ordering::Acquire) {
            Err(RuntimeError::Stopped.into())
        } else {
            mux.reorder_tab(workspace, tab, before)
        };
        Committed {
            result,
            seq: mux.hierarchy_seq(),
        }
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
    /// Persistent Config and Keymap notices for the active configuration.
    /// New windows raise them; every reload replaces them.
    diagnostics: Vec<NoticeContent>,
    /// Failures reported while no window could show them, such as a startup
    /// hotkey conflict. New windows raise them until a reload clears them.
    latched: Vec<NoticeContent>,
    /// Client-owned facts about every window; read window facts here.
    windows: WindowModel,
    /// The runtime structure every window and palette reads names, tab
    /// membership, and order from. The UI thread never locks Mux for it.
    hierarchy: Projection,
    /// Open palettes and close confirmations, which show titles they do not
    /// own, and the windows whose titles changed since their last refresh.
    title_consumers: TitleConsumers,
    /// Open windows' views, for updates that reach every window.
    views: Views,
    keymap: InstalledKeymap,
    frequency: CommandFrequency,
    reloading: bool,
    quitting: bool,
    pending_spawns: usize,
    quit_pending: bool,
    external_drag_window: Option<gpui::WindowId>,
    /// The window buttons the desktop asks title rows to draw.
    button_layout: ButtonLayout,
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
                        view.read(cx).active_view(cx).ok_or_else(|| {
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

/// Raises `message` as a command failure in the active window, if there is
/// one.
fn show_active_window_failure(cx: &mut App, message: String) {
    let Some(window) = cx.active_window() else {
        return;
    };
    let _ = window.update(cx, |root, _, cx| {
        if let Ok(view) = root.downcast::<WorkspaceView>() {
            view.update(cx, |view, cx| {
                view.report_failure("Command failed", message, cx);
            });
        }
    });
}

/// Records a failure for windows that do not exist yet. Repeats of a waiting
/// message collapse.
fn latch_failure(cx: &mut App, message: &str) {
    let latched = &mut cx.global_mut::<Desktop>().latched;
    if !latched.iter().any(|content| content.message == message) {
        latched.push(NoticeContent::command_failure("Command failed", message));
    }
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
        latch_failure(cx, &message);
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
                view.report_failure("Command failed", message, cx);
            });
            return;
        }
        if latch_if_dead {
            latch_failure(cx, &message);
        }
        show_active_window_failure(cx, message);
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

/// Profile rows for the palette's quake profile picker, read from the window
/// model, so opening the palette inside a quake window reads no view.
fn quake_profile_rows(cx: &App) -> Vec<QuakeProfileRow> {
    quake_windows::profile_rows(cx)
        .into_iter()
        .map(|row| QuakeProfileRow {
            detail: format!(
                "{} · {}",
                row.geometry,
                match row.state {
                    quake_windows::ProfileState::NotSummoned => {
                        "not summoned yet".to_owned()
                    }
                    quake_windows::ProfileState::Hidden { tabs } => {
                        format!(
                            "hidden · {tabs} tab{}",
                            if tabs == 1 { "" } else { "s" }
                        )
                    }
                    quake_windows::ProfileState::Visible =>
                        "visible".to_owned(),
                }
            ),
            name: row.name,
        })
        .collect()
}

/// Binds the startup keymap and returns its reserved keys with the
/// persistent diagnostics in precedence order: config error, keymap error,
/// conflicts, then warning.
///
/// A broken binding never blocks startup: defaults apply and the diagnostic
/// shows like any other non-fatal configuration error.
pub(super) fn install_startup_keymap(
    cx: &mut App,
    loaded: &config::LoadedConfig,
) -> (InstalledKeymap, Vec<NoticeContent>) {
    let (compiled, keymap_error) = compile_keymap(&loaded.config);
    let diagnostics = config_diagnostics(
        &loaded.path,
        loaded.error.as_deref(),
        keymap_error.as_deref(),
        &compiled.conflicts,
        loaded.warning.as_deref(),
    );
    let keymap = bind_keymap(cx, compiled);
    (keymap, diagnostics)
}

/// A notice action that runs `id` without arguments.
fn bare(id: huterm_protocol::CommandId) -> CommandInvocation {
    CommandInvocation::new(id, Vec::new())
}

/// The persistent notices for a loaded configuration, one per diagnostic,
/// in precedence order. Each offers Open Settings; the config error also
/// offers Reload.
fn config_diagnostics(
    path: &Path,
    error: Option<&str>,
    keymap_error: Option<&str>,
    conflicts: &[String],
    warning: Option<&str>,
) -> Vec<NoticeContent> {
    let location = path.display().to_string();
    let open_settings = || bare(ids::OPEN_SETTINGS);
    let mut diagnostics = Vec::new();
    if let Some(error) = error {
        diagnostics.push(
            NoticeContent::diagnostic(
                Severity::Error,
                NoticeSource::Config,
                "Configuration error",
                error,
            )
            .location(location.clone())
            .action("Open Settings", open_settings())
            .action("Reload", bare(ids::RELOAD_CONFIG)),
        );
    }
    if let Some(error) = keymap_error {
        diagnostics.push(
            NoticeContent::diagnostic(
                Severity::Error,
                NoticeSource::Keymap,
                "Keybinding error",
                error,
            )
            .location(location.clone())
            .action("Open Settings", open_settings()),
        );
    }
    if !conflicts.is_empty() {
        diagnostics.push(
            NoticeContent::diagnostic(
                Severity::Warning,
                NoticeSource::Keymap,
                "Keybinding conflicts",
                conflicts.join("; "),
            )
            .location(location.clone())
            .action("Open Settings", open_settings()),
        );
    }
    if let Some(warning) = warning {
        diagnostics.push(
            NoticeContent::diagnostic(
                Severity::Warning,
                NoticeSource::Config,
                "Configuration warning",
                warning,
            )
            .location(location)
            .action("Open Settings", open_settings()),
        );
    }
    diagnostics
}

/// The persistent notices after a successful reload: binding conflicts and
/// the config warning. A reload that failed never reaches this.
fn reload_diagnostics(
    path: &Path,
    config: &Config,
    compiled: &CompiledKeymap,
) -> Vec<NoticeContent> {
    if let Some(warning) = &config.warning {
        eprintln!("Huterm configuration warning: {warning}");
    }
    config_diagnostics(
        path,
        None,
        None,
        &compiled.conflicts,
        config.warning.as_deref(),
    )
}

/// The configuration notices after a failed reload: the failure, then the
/// active configuration's own diagnostics, which still apply.
fn failed_reload_diagnostics(
    error: &str,
    retained: &[NoticeContent],
) -> Vec<NoticeContent> {
    let mut diagnostics = vec![
        NoticeContent::diagnostic(
            Severity::Error,
            NoticeSource::Config,
            "Configuration reload failed",
            format!("Config reload failed: {error}"),
        )
        .action("Open Settings", bare(ids::OPEN_SETTINGS))
        .action("Reload", bare(ids::RELOAD_CONFIG)),
    ];
    diagnostics.extend(retained.iter().cloned());
    diagnostics
}

pub(super) fn run() -> anyhow::Result<()> {
    run_with_startup(|_| {})
}

pub(super) fn run_with_startup(
    startup: impl FnOnce(&mut App) + 'static,
) -> anyhow::Result<()> {
    // Production and every terminal-hosting smoke start here, except the
    // native quit smoke, which spawns its terminal directly and raises the
    // limit itself.
    huterm_core::raise_open_file_limit();
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
    if let Some(warning) = &loaded.warning {
        eprintln!("Huterm configuration warning: {warning}");
    }
    let runtime = Arc::new(DesktopRuntime::default());
    // Subscribe before any window or worker exists, so this is the only
    // time the UI thread takes the Mux lock for structure.
    let (hierarchy, subscription) = runtime.subscribe_hierarchy();
    let hierarchy = Projection::new(hierarchy, subscription);
    let app_runtime = Arc::clone(&runtime);
    let application = crate::assets::application();
    application.on_reopen(|cx| {
        if cx.windows().is_empty() {
            open_window(cx);
        }
        cx.activate(true);
    });
    application.run(move |cx| {
        let (keymap, diagnostics) = install_startup_keymap(cx, &loaded);
        #[cfg(all(target_os = "macos", feature = "macos-updater"))]
        let updater =
            native_updater::Updater::initialize(loaded.config.updates);
        cx.set_global(Desktop {
            quake: quake_windows::Registry::default(),
            runtime: Arc::clone(&app_runtime),
            config: loaded.config,
            diagnostics,
            latched: Vec::new(),
            config_path: loaded.path,
            windows: WindowModel::default(),
            hierarchy,
            title_consumers: TitleConsumers::default(),
            views: Views::default(),
            keymap,
            frequency: CommandFrequency::default(),
            reloading: false,
            quitting: false,
            pending_spawns: 0,
            quit_pending: false,
            external_drag_window: None,
            button_layout: ButtonLayout::standard(),
            #[cfg(all(target_os = "macos", feature = "macos-updater"))]
            updater,
        });
        start_hierarchy_drain(cx);
        #[cfg(target_os = "linux")]
        follow_button_layout(cx);
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
            cx.global_mut::<Desktop>().hierarchy.settle_waiters(true);
            async {}
        })
        .detach();
        cx.on_action(|action: &InvokeApp, cx| {
            if let Err(error) = Desktop::invoke(cx, &action.0, None) {
                let message =
                    format!("Command `{}` failed: {error}", action.0.id);
                eprintln!("Huterm {message}");
                // Global action callbacks run while the dispatching window
                // is borrowed, so raise the notice after it is returned.
                cx.defer(move |cx| show_active_window_failure(cx, message));
            }
        });
        cx.on_window_closed(|cx, window| {
            let desktop = cx.global_mut::<Desktop>();
            desktop.windows.remove(window);
            desktop.title_consumers.set_host(window, false);
            desktop.views.prune();
            // The closed window's titles left the model; Quit dialogs and
            // palettes elsewhere still list them until they refresh.
            mark_titles_changed(window, cx);
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

/// Follows the desktop's window-button layout, relaying each change to
/// every window.
#[cfg(target_os = "linux")]
fn follow_button_layout(cx: &mut App) {
    let (sender, receiver) = async_channel::bounded(1);
    button_layout::watch(sender);
    cx.spawn(async move |cx| {
        while let Ok(layout) = receiver.recv().await {
            cx.update(|cx| {
                cx.global_mut::<Desktop>().button_layout = layout;
                broadcast(cx, |view, cx| {
                    view.button_layout = layout;
                    cx.notify();
                });
            });
        }
    })
    .detach();
}

/// Starts the application task the current hierarchy subscription wakes.
/// Each wake drains every queued event in one batch on the UI thread, then
/// reconciles; replacing the subscription replaces, and so cancels, the
/// previous task.
fn start_hierarchy_drain(cx: &mut App) {
    let subscription = cx.global::<Desktop>().hierarchy.subscription();
    let task = cx.spawn(async move |cx| {
        while subscription.wait_for_activity().await.is_ok() {
            cx.update(sync_hierarchy);
        }
        // The stream closed: drain once more so the projection freezes and
        // cancels any waiter still parked on it.
        cx.update(sync_hierarchy);
    });
    cx.global_mut::<Desktop>().hierarchy.set_drain_task(task);
}

/// Applies queued hierarchy events, reconciles the windows they touched,
/// and releases the sequence waiters the projection now satisfies. Lost
/// events start a resubscription instead; nothing reconciles against a
/// state that missed events.
/// After teardown commits, nothing more applies: the projection keeps its
/// last state and every waiter is cancelled.
fn sync_hierarchy(cx: &mut App) {
    let terminating = terminating(cx);
    let projection = &mut cx.global_mut::<Desktop>().hierarchy;
    if projection.resyncing() && !terminating {
        return;
    }
    match projection.sync(terminating) {
        Drained::Current(touched) => {
            reconcile(cx, &touched);
            settle_waiters(cx);
        }
        Drained::Resync => resync_hierarchy(cx),
        // Events applied before the freeze still reach views; reconcile
        // skips membership once teardown has committed.
        Drained::Frozen(touched) => reconcile(cx, &touched),
    }
}

/// Whether application teardown has committed.
fn terminating(cx: &App) -> bool {
    cx.global::<Desktop>()
        .runtime
        .terminating
        .load(Ordering::Acquire)
}

/// Releases satisfied sequence waiters, or cancels every waiter once
/// application teardown has committed.
fn settle_waiters(cx: &mut App) {
    let desktop = cx.global_mut::<Desktop>();
    let terminating = desktop.runtime.terminating.load(Ordering::Acquire);
    desktop.hierarchy.settle_waiters(terminating);
}

/// Replaces the projection from a fresh snapshot taken on a worker. The
/// worker takes the Mux lock, which recovers from poisoning, so in process
/// resubscription cannot fail; it retries while the new subscription lags
/// again before installation finishes.
fn resync_hierarchy(cx: &mut App) {
    if !cx.global_mut::<Desktop>().hierarchy.begin_resync() {
        return;
    }
    let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
    cx.spawn(async move |cx| {
        loop {
            let worker = Arc::clone(&runtime);
            let (state, subscription) = cx
                .background_executor()
                .spawn(async move { worker.subscribe_hierarchy() })
                .await;
            if cx.update(|cx| install_hierarchy(cx, state, subscription)) {
                break;
            }
        }
    })
    .detach();
}

/// Installs a resync snapshot: replaces the state and subscription
/// together, drains the new subscription, reconciles every window, then
/// releases waiters. Returns false, reconciling nothing, when the new
/// subscription lagged again. After teardown it installs nothing and ends
/// the resubscription.
fn install_hierarchy(
    cx: &mut App,
    state: HierarchyState,
    subscription: HierarchySubscription,
) -> bool {
    let terminating = terminating(cx);
    match cx.global_mut::<Desktop>().hierarchy.install(
        state,
        subscription,
        terminating,
    ) {
        Install::Current => {}
        Install::Lagged => return false,
        Install::Frozen(touched) => {
            reconcile(cx, &touched);
            return true;
        }
    }
    start_hierarchy_drain(cx);
    reconcile(cx, &Touched::everything());
    settle_waiters(cx);
    true
}

/// Brings the windows showing touched workspaces or tabs in line with the
/// projection, notifying each only when something it renders changed, then
/// refreshes title consumers. Never installs views.
fn reconcile(cx: &mut App, touched: &Touched) {
    if touched.is_empty() {
        return;
    }
    let desktop = cx.global::<Desktop>();
    let terminating = desktop.runtime.terminating.load(Ordering::Acquire);
    let windows = windows_to_visit(&desktop.windows, touched);
    let mut retitled = HashSet::new();
    let mut activate = Vec::new();
    update_windows(cx, &windows, |view, cx| {
        let before = view.active_tab(cx);
        let change = view.reconcile(touched, terminating, cx);
        if let Some(tab) = reactivation(before, view.active_tab(cx)) {
            activate.push((view.window, tab));
        }
        if change.titles {
            retitled.insert(view.window);
        }
        if change.changed {
            cx.notify();
        }
    });
    if !activate.is_empty() {
        // Selecting needs the window, which reconcile does not have here;
        // a deferred update runs once no window is on GPUI's update stack.
        cx.defer(move |cx| activate_reconciled_tabs(cx, &activate));
    }
    refresh_title_consumers(cx, true, &retitled);
}

/// Shows and focuses the tabs reconcile made active by removing the active
/// tab, as `reconcile_own` does for a window's own completion. A window
/// whose active tab changed again meanwhile is left alone.
fn activate_reconciled_tabs(cx: &mut App, tabs: &[(gpui::WindowId, TabId)]) {
    for &(id, tab) in tabs {
        let Some(handle) = cx
            .windows()
            .into_iter()
            .find(|handle| handle.window_id() == id)
        else {
            continue;
        };
        let _ = handle.update(cx, |root, window, cx| {
            let Ok(view) = root.downcast::<WorkspaceView>() else {
                return;
            };
            view.update(cx, |view, cx| {
                if view.active_tab(cx) == Some(tab) {
                    view.measure_tab_widths(window, cx);
                    view.select(tab, window, cx);
                }
            });
        });
    }
}

/// Refreshes the views that show titles they do not own. A structural batch
/// or any title change rebuilds open palettes, which compare their rows
/// before notifying; a confirmation redraws when a title in its scope
/// changed. It never changes a confirmation's assessment or focus.
fn refresh_title_consumers(
    cx: &mut App,
    structural: bool,
    retitled: &HashSet<gpui::WindowId>,
) {
    let hosts = cx.global::<Desktop>().title_consumers.hosts();
    if hosts.is_empty() || (!structural && retitled.is_empty()) {
        return;
    }
    update_windows(cx, &hosts, |view, cx| {
        if view.palette.is_some() {
            view.refresh_palette_hierarchy(cx);
        }
        let in_scope = match &view.close.confirmation {
            Some(CloseTarget::Application) => !retitled.is_empty(),
            Some(_) => retitled.contains(&view.window),
            None => false,
        };
        if in_scope {
            cx.notify();
        }
    });
}

/// Notes a terminal-driven title change in `window`. With a title consumer
/// open, defers one application-level refresh per turn; otherwise it does
/// nothing. Callers may be inside a view update, which a direct refresh
/// would re-lease.
fn mark_titles_changed(window: gpui::WindowId, cx: &mut App) {
    if cx.global_mut::<Desktop>().title_consumers.mark(window) {
        cx.defer(|cx| {
            let retitled =
                cx.global_mut::<Desktop>().title_consumers.take_dirty();
            refresh_title_consumers(cx, false, &retitled);
        });
    }
}

/// Waits until the projection has applied `seq`, the sequence a structural
/// result committed at. It drains in application context first, which in
/// process always suffices, so the completion continues on the same turn;
/// otherwise it parks on a one-shot waiter that resolves exactly once.
async fn projected(cx: &gpui::AsyncApp, seq: u64) -> Resolution {
    let wait = cx.update(|cx| {
        sync_hierarchy(cx);
        let desktop = cx.global_mut::<Desktop>();
        let terminating = desktop.runtime.terminating.load(Ordering::Acquire);
        desktop.hierarchy.wait_for(seq, terminating)
    });
    match wait {
        Wait::Ready => Resolution::Ready,
        Wait::Cancelled => Resolution::Cancelled,
        Wait::Pending(receiver) => {
            receiver.recv().await.unwrap_or(Resolution::Cancelled)
        }
    }
}

/// What a spawn completion does after its sequence wait.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SpawnDisposition {
    /// Publish the result as before: install the view or report failure.
    Publish,
    /// The wait was cancelled: publish nothing. Clean up a successful spawn
    /// unless teardown, which owns every terminal, has committed, and report
    /// a failure only while the application is not terminating.
    Abandon { cleanup: bool, notify: bool },
}

fn spawn_disposition(
    resolution: Resolution,
    terminating: bool,
) -> SpawnDisposition {
    match resolution {
        Resolution::Ready => SpawnDisposition::Publish,
        Resolution::Cancelled => SpawnDisposition::Abandon {
            cleanup: !terminating,
            notify: !terminating,
        },
    }
}

/// Settles one spawn: Quit deferred behind pending spawns resumes when the
/// last one settles. Returns whether it resumes.
fn settle_spawn(pending_spawns: &mut usize, quit_pending: &mut bool) -> bool {
    *pending_spawns = pending_spawns.saturating_sub(1);
    *pending_spawns == 0 && std::mem::take(quit_pending)
}

/// Whether Quit may proceed now; otherwise it waits for pending spawns to
/// publish or fail.
fn quit_now(pending_spawns: usize, quit_pending: &mut bool) -> bool {
    if pending_spawns > 0 {
        *quit_pending = true;
        return false;
    }
    true
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
                || view.dialog_showing()
                || view.reorder.is_some()
                || view.palette.is_some()
                || view.menu.is_some();
            if let Some(tab) = view.active_view(cx) {
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
                    if tab.handle_keystroke(
                        &event.keystroke,
                        &reserved,
                        window,
                        cx,
                    ) {
                        cx.stop_propagation();
                    }
                    tab.start_snapshot_if_needed(cx);
                });
            }
        });
    }
}

fn request_quit(cx: &mut App) {
    let desktop = cx.global_mut::<Desktop>();
    if !quit_now(desktop.pending_spawns, &mut desktop.quit_pending) {
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
    let desktop = cx.global::<Desktop>();
    desktop
        .windows
        .restore_windows(desktop.config.tabs.position)
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
            cx.update(|cx| {
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
        cx.update(|cx| {
            quake_presentations = quake_windows::take_for_quit(cx);
            quake_windows::shutdown(cx);
            broadcast(cx, |view, _| {
                view.fullscreen_work.wake.stop();
                view.fullscreen.close();
                #[cfg(target_os = "macos")]
                if let Some(adapter) = view.native_fullscreen.take() {
                    adapter.close_gate();
                    adapters.push(adapter);
                }
            });
        });
        for state in quake_presentations {
            state.cleanup();
        }
        #[cfg(target_os = "macos")]
        for adapter in adapters {
            adapter.close();
        }
        cx.update(|cx| {
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
            cx.update(|cx| cx.defer(request_quit));
        }
    })
    .detach();
}

#[cfg(not(target_os = "macos"))]
fn install_native_quit(_: &mut App) {}

/// The window's tab configuration with `titlebar` resolved for `host`.
fn layout_tabs(tabs: TabsConfig, host: TabHost) -> TabsConfig {
    TabsConfig {
        position: resolve_tab_position(tabs.position, host),
        ..tabs
    }
}

/// The window size that gives the initial grid its columns and rows
/// beside the chrome: the tab bar, the title row above the terminal, and
/// the frame's border on every side.
fn initial_window_size(
    config: &Config,
    metrics: GridMetrics,
    host: TabHost,
    frame: WindowFrame,
) -> gpui::Size<Pixels> {
    let reserved = if config.tabs.always_show {
        ChromeLayout::bar_reservation(
            layout_tabs(config.tabs, host),
            SIDEBAR_WIDTH,
        )
    } else {
        size(px(0.0), px(0.0))
    };
    size(
        metrics.cell_width * f32::from(INITIAL_COLUMNS)
            + px(config.window.padding_x * 2.0)
            + reserved.width
            + frame.inset.left
            + frame.inset.right,
        metrics.cell_height * f32::from(INITIAL_ROWS)
            + px(config.window.padding_y * 2.0)
            + title_row_height(false, frame)
            + reserved.height
            + frame.inset.top
            + frame.inset.bottom,
    )
}

/// The decoration facts GPUI reports for `window`, sampled together.
fn frame_state(window: &Window) -> FrameState {
    FrameState {
        decorations: window.window_decorations(),
        maximized: window.is_maximized(),
        fullscreen: window.is_fullscreen(),
    }
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
    let display_id = match crate::benchmark_display::selected(cx) {
        Ok(display) => display,
        Err(error) => {
            report_deferred_failure(cx, reporter, error.to_string());
            maybe_exit(cx);
            return;
        }
    };
    // A new window is never fullscreen; its host is fixed by the profile.
    // Whether it draws its own title bar is known once GPUI has created
    // it, so the centred bounds assume the window manager's.
    let host = TabHost {
        platform: Platform::current(),
        fullscreen: false,
        quake: profile.is_some(),
        client_decorations: false,
    };
    let bounds = Bounds::centered(
        display_id,
        initial_window_size(&config, metrics, host, WindowFrame::default()),
        cx,
    );
    let result = cx.open_window(
        WindowOptions {
            display_id,
            show: profile.is_none(),
            focus: profile.is_none(),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_decorations: Some(requested_decorations(
                config.tabs.position,
                host.platform,
                host.quake,
            )),
            // A macOS title strip holds tabs, `+`, and the menu button. When AppKit
            // owns the strip, the window server takes a press there as a
            // window drag, so Huterm moves the window from empty strip
            // space itself (`title_row_gestures`).
            app_owns_titlebar_drag: cfg!(target_os = "macos"),
            // GPUI caps unfocused windows at 30 frames per second by default.
            // A terminal left unfocused still shows live output, and Huterm's
            // own frame admission already stops idle redraws.
            inactive_frame_interval: None,
            titlebar: Some(TitlebarOptions {
                title: Some(window_title(None).into()),
                appears_transparent: cfg!(target_os = "macos"),
                // A merged tab row centres the buttons once the window
                // exists, from AppKit's native placement; see
                // `merged_traffic_lights`.
                traffic_light_position: None,
            }),
            window_min_size: Some(size(px(280.0), px(180.0))),
            app_id: Some(APP_ID.into()),
            ..WindowOptions::default()
        },
        |window, cx| {
            if let Some(display) = display_id {
                crate::benchmark_display::observe(window, cx, display);
            }
            let scaled_metrics = metrics.at_scale(window.scale_factor());
            // AppKit centres the buttons in its 28-point title bar, two points
            // above the 32-point strip's centre. A merged tab row makes that
            // visible, so it centres them in the strip, keeping AppKit's left
            // inset. Other positions keep the native placement until the
            // same alignment is checked for them. Borderless Quake windows
            // have no buttons.
            #[cfg(target_os = "macos")]
            if config.tabs.position == TabPosition::Titlebar && !host.quake {
                match crate::native_titlebar::close_button_frame(window) {
                    Ok(native) => window.set_traffic_light_position(
                        merged_traffic_lights(native),
                    ),
                    Err(error) => eprintln!("Traffic lights: {error}"),
                }
            }
            // GPUI has applied the requested decorations, or fallen back
            // without a compositor: the window now knows whether it draws
            // its title row and border, which take their own room.
            let state = frame_state(window);
            let host = TabHost {
                client_decorations: state.client_decorations(),
                ..host
            };
            let button_layout = cx.global::<Desktop>().button_layout;
            let frame = WindowFrame::resolve(
                resolve_tab_position(config.tabs.position, host),
                state,
                button_layout,
            );
            if scaled_metrics != metrics || frame != WindowFrame::default() {
                window.resize(initial_window_size(
                    &config,
                    scaled_metrics,
                    host,
                    frame,
                ));
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
            let frame_clock = refresh::FrameClock::new(window);
            #[cfg(target_os = "macos")]
            let native_fullscreen =
                if std::env::var_os("HUTERM_FULLSCREEN_SMOKE").is_some()
                    && std::env::var_os("HUTERM_FULLSCREEN_NO_ADAPTER")
                        .is_some()
                {
                    None
                } else {
                    crate::native_fullscreen::Adapter::new(window, cx)
                        .inspect_err(|error| {
                            eprintln!("Fullscreen observer: {error}");
                        })
                        .ok()
                };
            #[cfg(target_os = "macos")]
            let fallback = native_fullscreen.is_none();
            #[cfg(not(target_os = "macos"))]
            let fallback = false;
            let fullscreen_work = crate::fullscreen_work::Work::new(fallback);
            #[cfg(target_os = "macos")]
            if let Some(adapter) = &native_fullscreen {
                adapter.set_wake(fullscreen_work.wake.clone());
                if quake.is_some() {
                    adapter.discard_quake_events();
                }
            }
            let window_id = window.window_handle().window_id();
            cx.global_mut::<Desktop>().windows.open(
                window_id,
                quake.as_ref().map(|state| QuakeRecord {
                    profile: state.name.clone(),
                    visible: state.visible(),
                }),
                WindowLayout {
                    bounds: window.window_bounds(),
                    sidebar_width: SIDEBAR_WIDTH,
                },
            );
            let view = cx.new(|cx| WorkspaceView {
                frame_clock,
                quake,
                window: window_id,
                fullscreen_insets: gpui::Edges::default(),
                notch_shelves: None,
                fullscreen: FullscreenController::new(
                    window.window_bounds(),
                    config.window.macos_fullscreen_mode,
                    cfg!(target_os = "macos"),
                ),
                #[cfg(target_os = "macos")]
                native_fullscreen,
                fullscreen_work,
                frame_state: state,
                applied_frame: None,
                button_layout,
                tabs: Vec::new(),
                tab_scroll: px(0.0),
                tab_widths: Vec::new(),
                title_widths: HashMap::new(),
                scroll_target: None,
                last_scroll: Instant::now(),
                tab_scrollbars: Scrollbars::default(),
                sidebar_width: SIDEBAR_WIDTH,
                resizing_sidebar: false,
                reveal: Reveal::default(),
                reveal_context: None,
                pointer_reveal: PointerReveal::default(),
                layout_pending: false,
                reorder: None,
                config,
                family,
                metrics: scaled_metrics,
                focus: cx.focus_handle(),
                busy: false,
                close: CloseState::default(),
                exited_tabs: ExitQueue::default(),
                notices: NoticeStack::default(),
                notice_focus: cx.focus_handle(),
                focused_notice: None,
                hovered_notices: HashSet::new(),
                palette: None,
                palette_refresh_state: None,
                retained_query: None,
                recent: RecentCommands::default(),
                startup_reporter: reporter.clone(),
                menu: None,
                menu_button_bounds: Rc::new(Cell::new(None)),
                menu_bounds: Rc::new(Cell::new(None)),
                window_button_bounds: Rc::default(),
                title_row_press: Rc::new(Cell::new(false)),
                title_row_moves: Rc::new(Cell::new(0)),
                about: None,
                // The window opened with this title. Writing it again from
                // the first render costs GPUI's X11 backend a blocking round
                // trip that can queue the window's MapNotify where the event
                // loop never sees it, so no later frame is ever requested.
                window_title: window_title(None),
                drawn_dialog_groups: None,
            });
            view.update(cx, |view, cx| {
                view.frame_clock.observe(cx);
                view.raise_desktop_notices(cx);
                let notice_focus = view.notice_focus.clone();
                cx.on_focus_out(
                    &notice_focus,
                    window,
                    |view, _, window, cx| {
                        view.focused_notice = None;
                        view.reconcile_notices(window, cx);
                    },
                )
                .detach();
                quake_windows::start(view, window, cx);
                cx.observe_in(&cx.entity(), window, |view, _, window, cx| {
                    view.refresh_tab_visibility(window, cx);
                    if let Some(state) = &view.quake {
                        state.wake();
                    }
                    if view.layout_pending {
                        view.sync_tab_layout(window, cx);
                    }
                    view.resume_close(window, cx);
                    view.refresh_palette(cx);
                    view.refresh_menu(window, cx);
                    view.sync_title_consumer(cx);
                })
                .detach();
                cx.observe_window_activation(window, |view, window, cx| {
                    if let Some(state) = &view.quake {
                        state.native_wake();
                    }
                    view.refresh_tab_visibility(window, cx);
                })
                .detach();
                cx.observe_window_bounds(window, |view, window, cx| {
                    view.fullscreen_work.wake.signal();
                    if let Some(state) = &view.quake {
                        state.native_wake();
                    }
                    // Tiling, maximizing, and fullscreen arrive with new
                    // bounds; the frame follows them before the layout.
                    view.sync_frame(window);
                    view.layout_pending = true;
                    view.refresh_tab_visibility(window, cx);
                    view.sync_tab_layout(window, cx);
                })
                .detach();
                // GPUI reports a decoration change, including the
                // no-compositor fallback, as an appearance change.
                cx.observe_window_appearance(window, |view, window, cx| {
                    if view.sync_frame(window) {
                        view.refresh_tab_visibility(window, cx);
                        view.sync_tab_layout(window, cx);
                        cx.notify();
                    }
                })
                .detach();
            });
            let weak = view.downgrade();
            window.on_window_should_close(cx, move |window, cx| {
                let _ = weak.update(cx, |view, cx| {
                    view.request_close(CloseTarget::Window, window, cx);
                });
                false
            });
            cx.global_mut::<Desktop>()
                .views
                .push(window_id, view.downgrade());
            view.update(cx, |view, cx| {
                view.focus.focus(window, cx);
                if launch_shell
                    && (!profile_requested || view.quake.is_some())
                    && let Err(error) = view.new_tab(window, cx)
                {
                    view.report_failure(
                        "Cannot open tab",
                        error.to_string(),
                        cx,
                    );
                }
            });
            if profile_requested && view.read(cx).quake.is_none() {
                let handle = window.window_handle();
                cx.defer(move |cx| {
                    let _ = handle
                        .update(cx, |_, window, _| window.remove_window());
                });
            }
            view.update(cx, |view, cx| {
                let Some(wakes) = view.fullscreen_work.receiver.take() else {
                    return;
                };
                let handle = window.window_handle();
                view.fullscreen_work.task =
                    Some(cx.spawn(async move |weak, cx| {
                        let foreground = cx.foreground_executor().clone();
                        crate::fullscreen_work::run(
                            wakes,
                            || {
                                handle
                                    .update(cx, |_, window, cx| {
                                        weak.update(cx, |view, cx| {
                                            if view.fullscreen.is_closed() {
                                                return None;
                                            }
                                            view.fullscreen_work.wake.passed();
                                            let continuation = view
                                                .refresh_fullscreen(window, cx);
                                            view.arm_fullscreen(cx);
                                            Some(continuation)
                                        })
                                        .ok()
                                        .flatten()
                                    })
                                    .ok()
                                    .flatten()
                            },
                            || foreground.spawn(async {}),
                        )
                        .await;
                    }));
                view.fullscreen_work.wake.signal();
            });
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

impl refresh::Animated for WorkspaceView {
    fn animation_schedule(&self, now: Instant) -> AnimationSchedule {
        let mut next = self
            .tab_scrollbars
            .schedule(now)
            .merge(self.reveal.schedule(now));
        if let Some(at) = self.pointer_reveal.probe_at {
            next = next.merge(AnimationSchedule::at(at));
        }
        if self.scroll_target.is_some()
            || self.reorder.as_ref().is_some_and(|drag| {
                drag.dragging
                    && drag
                        .strip
                        .autoscroll(drag.pointer, Duration::from_millis(16))
                        != self.tab_scroll
            })
        {
            next = next.merge(AnimationSchedule::FRAME);
        }
        if let Some(state) = &self.quake {
            next = next.merge(state.frame_schedule());
        }
        next.merge(self.notices.schedule())
    }

    fn advance_animation(
        &mut self,
        now: Instant,
        frame: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if frame && let Some(state) = &mut self.quake {
            state.window_frame(now);
        }
        if self.notices.expire(now) {
            self.reconcile_notices(window, cx);
        }
        if self.pointer_reveal.probe_at.is_some_and(|at| now >= at) {
            self.refresh_tab_visibility(window, cx);
        }
        if self.advance_tab_scroll(now, window)
            | self.tab_scrollbars.advance(now)
            | self.reveal.tick(now)
        {
            self.sync_tab_layout(window, cx);
            cx.notify();
        }
        self.update_pointer_probe(now, window);
    }
}

impl WorkspaceView {
    /// Raises an expiring command failure notice.
    fn report_failure(
        &mut self,
        title: &str,
        message: impl Into<String>,
        cx: &mut Context<'_, Self>,
    ) {
        self.notify(NoticeContent::command_failure(title, message), cx);
    }

    /// Raises `content` and repaints.
    fn notify(&mut self, content: NoticeContent, cx: &mut Context<'_, Self>) {
        self.notices.push(content, Instant::now());
        cx.notify();
    }

    /// Raises the desktop's configuration diagnostics and any failures
    /// latched while no window could show them.
    fn raise_desktop_notices(&mut self, cx: &mut Context<'_, Self>) {
        let desktop = cx.global::<Desktop>();
        let diagnostics = desktop.diagnostics.clone();
        let latched = desktop.latched.clone();
        let now = Instant::now();
        self.notices.replace_diagnostics(&diagnostics, now);
        for content in latched {
            self.notices.push(content, now);
        }
        cx.notify();
    }

    /// Replaces the configuration notices after a reload attempt.
    fn replace_diagnostics(
        &mut self,
        diagnostics: &[NoticeContent],
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.notices
            .replace_diagnostics(diagnostics, Instant::now());
        self.reconcile_notices(window, cx);
    }

    /// Drops focus and hover state for notices that no longer exist, returns
    /// focus to the terminal when the focused toast went away, and repaints.
    /// Every change to the stack or to focus and hover state ends here.
    fn reconcile_notices(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.heal_notice_state(window, cx);
        cx.notify();
    }

    /// The bookkeeping behind [`Self::reconcile_notices`] without a repaint.
    /// Render runs it too: a stack change that skipped reconciling would
    /// otherwise keep keyboard focus on a dead toast, where every `notice_*`
    /// key fails and the paused stack never expires.
    fn heal_notice_state(&mut self, window: &mut Window, cx: &mut App) {
        if reconcile_notice_state(
            &mut self.notices,
            &mut self.focused_notice,
            &mut self.hovered_notices,
            Instant::now(),
        ) {
            self.restore_tab_focus(window, cx);
        }
    }

    fn focus_notice(
        &mut self,
        id: NoticeId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.focused_notice = Some(id);
        self.notice_focus.focus(window, cx);
        self.reconcile_notices(window, cx);
    }

    /// The focused toast, for the `notice_*` commands.
    fn focused_notice(&self) -> Result<NoticeId, CommandError> {
        self.focused_notice
            .filter(|id| self.notices.get(*id).is_some())
            .ok_or_else(|| {
                CommandError::Unavailable("no notice is focused".to_owned())
            })
    }

    fn notice_availability(
        &self,
        command: huterm_protocol::CommandId,
    ) -> Result<(), CommandError> {
        match command {
            ids::FOCUS_NOTICES | ids::DISMISS_ALL_NOTICES => {
                if self.notices.is_empty() {
                    Err(CommandError::Unavailable("no notices".to_owned()))
                } else {
                    Ok(())
                }
            }
            _ => self.focused_notice().map(|_| ()),
        }
    }

    fn run_notice_command(
        &mut self,
        command: huterm_protocol::CommandId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        match command {
            ids::FOCUS_NOTICES => {
                let newest = self.notices.newest().ok_or_else(|| {
                    CommandError::Unavailable("no notices".to_owned())
                })?;
                self.focus_notice(newest, window, cx);
            }
            ids::DISMISS_ALL_NOTICES => {
                if !self.notices.dismiss_all() {
                    return Err(CommandError::Unavailable(
                        "no notices".to_owned(),
                    ));
                }
                self.reconcile_notices(window, cx);
            }
            ids::NOTICE_NEXT | ids::NOTICE_PREVIOUS => {
                let id = self.focused_notice()?;
                let forward = command == ids::NOTICE_NEXT;
                if let Some(neighbour) = self.notices.neighbour(id, forward) {
                    self.focus_notice(neighbour, window, cx);
                }
            }
            ids::NOTICE_RUN_ACTION => {
                let id = self.focused_notice()?;
                self.run_notice_action(id, None, window, cx)?;
            }
            ids::NOTICE_DISMISS => {
                let id = self.focused_notice()?;
                self.dismiss_notice(id, window, cx);
            }
            other => return Err(CommandError::UnknownCommand(other)),
        }
        Ok(CommandOutcome::Completed)
    }

    /// Dismisses `id`, moving keyboard focus to the toast that takes its
    /// slot or back to the terminal when none remain.
    fn dismiss_notice(
        &mut self,
        id: NoticeId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let focused = self.focused_notice == Some(id);
        let next = self.notices.dismiss_focused(id);
        match (focused, next) {
            (true, Some(next)) => self.focus_notice(next, window, cx),
            (true, None) => {
                self.focused_notice = None;
                self.restore_tab_focus(window, cx);
            }
            (false, _) => {}
        }
        self.reconcile_notices(window, cx);
    }

    /// Runs `command` from notice `id`, or its first action when `None`.
    /// An expiring notice is dismissed first; a persistent one stays until
    /// its source replaces it. Enter on a toast without actions dismisses
    /// it, as most terminal failure and exit toasts have none.
    fn run_notice_action(
        &mut self,
        id: NoticeId,
        command: Option<CommandInvocation>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        let notice = self.notices.get(id).ok_or(CommandError::StaleTarget)?;
        let lifetime = notice.content.lifetime;
        let first_action = notice
            .content
            .actions
            .first()
            .map(|action| action.command.clone());
        let Some(command) = command.or(first_action) else {
            self.dismiss_notice(id, window, cx);
            return Ok(CommandOutcome::Completed);
        };
        if lifetime == Lifetime::Expiring {
            self.dismiss_notice(id, window, cx);
        } else if self.focused_notice == Some(id) {
            self.focused_notice = None;
            self.restore_tab_focus(window, cx);
            self.reconcile_notices(window, cx);
        }
        self.invoke_interactive(&command, window, cx)
    }

    /// Whether the active terminal draws its scroll pill, which the toast
    /// column must clear.
    fn scroll_pill_visible(&self, cx: &App) -> bool {
        self.active_view(cx)
            .is_some_and(|view| view.read(cx).scroll_pill_visible())
    }

    fn render_notices(
        &self,
        terminal: Bounds<Pixels>,
        cx: &mut Context<'_, Self>,
    ) -> Option<gpui::Div> {
        if self.notices.is_empty() {
            return None;
        }
        let bottom = if self.scroll_pill_visible(cx) {
            px(52.0)
        } else {
            px(12.0)
        };
        let dismiss = cx.entity().downgrade();
        let run_action = cx.entity().downgrade();
        let hover = cx.entity().downgrade();
        let handlers = ToastHandlers {
            dismiss: Rc::new(move |id, window, cx| {
                let _ = dismiss.update(cx, |view, cx| {
                    view.dismiss_notice(id, window, cx);
                });
            }),
            run_action: Rc::new(move |id, command, window, cx| {
                let _ = run_action.update(cx, |view, cx| {
                    if let Err(error) =
                        view.run_notice_action(id, Some(command), window, cx)
                    {
                        view.report_failure(
                            "Command failed",
                            error.to_string(),
                            cx,
                        );
                    }
                });
            }),
            hover: Rc::new(move |id, hovering, window, cx| {
                let _ = hover.update(cx, |view, cx| {
                    if hovering {
                        view.hovered_notices.insert(id);
                    } else {
                        view.hovered_notices.remove(&id);
                    }
                    view.reconcile_notices(window, cx);
                });
            }),
        };
        Some(
            div()
                .absolute()
                .left(terminal.origin.x)
                .top(terminal.origin.y)
                .w(terminal.size.width)
                .h(terminal.size.height)
                .child(render_notice_stack(
                    &self.notices,
                    Instant::now(),
                    self.focused_notice,
                    &self.notice_focus,
                    bottom,
                    Swatch::from_theme(&self.config.theme),
                    &handlers,
                )),
        )
    }
}

struct TabView {
    id: TabId,
    terminal: TerminalId,
    /// The hierarchy sequence the tab's spawn committed at. Reconcile drops
    /// the view only once the projection has applied it and no longer holds
    /// the tab.
    committed: u64,
    /// The names the projection last held for this tab, used while it no
    /// longer does, such as during a close commit.
    names: TabNames,
    view: Entity<TerminalView>,
    _activity_task: Task<()>,
}

impl TabView {
    /// The tab's label with status text, for previews, dialogs, and the
    /// window title.
    fn title(&self, tabs: huterm_config::TabsConfig, cx: &App) -> String {
        let (title, status) = self.label(tabs, cx);
        with_status_suffix(title, status)
    }

    /// Returns the display name without status text, and the status its
    /// indicator reports. Names come from the hierarchy projection by
    /// identity; a tab the projection no longer holds keeps the names it
    /// last held.
    fn label(
        &self,
        tabs: huterm_config::TabsConfig,
        cx: &App,
    ) -> (String, TabStatus) {
        let terminal = self.view.read(cx);
        let (custom, fallback) = tab_names(
            cx.global::<Desktop>().hierarchy.state(),
            self.id,
            &self.names,
        );
        let fallback = resolve_tab_name(custom, &terminal.title, fallback);
        let label = if custom.is_some() {
            fallback.to_owned()
        } else {
            resolve_tab_label(
                tabs.label,
                tabs.directory,
                fallback,
                &terminal.metadata,
                home_paths(),
            )
        };
        (
            label,
            TabStatus::new(
                terminal.exited,
                terminal.failed,
                terminal.bell.unseen,
                terminal.metadata.foreground_process(),
            ),
        )
    }
}

/// A tab label with the status text previews, dialogs, and the window model
/// show.
fn with_status_suffix(label: String, status: TabStatus) -> String {
    if status.exited() {
        format!("{label} · exited")
    } else {
        label
    }
}

/// The native window title: the active tab's title before the application
/// name, or the name alone without a tab.
fn window_title(active_tab: Option<&str>) -> String {
    match active_tab {
        Some(title) => format!("{title} — Huterm"),
        None => "Huterm".to_owned(),
    }
}

/// The directory `copy_tab_directory` copies: the tab's latest reported
/// path, local or remote, when it has one.
fn tab_directory_path(
    metadata: &huterm_protocol::TerminalMetadata,
) -> Option<String> {
    metadata
        .directory()
        .map(|directory| directory.path().to_owned())
        .filter(|path| !path.is_empty())
}

/// The display backend named in the About panel's platform row.
#[cfg(target_os = "linux")]
fn display_backend(window: &Window) -> Option<&'static str> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Xcb(_) | RawWindowHandle::Xlib(_) => Some("X11"),
        RawWindowHandle::Wayland(_) => Some("Wayland"),
        _ => None,
    }
}

#[cfg(not(target_os = "linux"))]
fn display_backend(_: &Window) -> Option<&'static str> {
    None
}

/// `$HOME` as written and resolved, read once. Shells report the logical
/// path, while process directories are resolved.
fn home_paths() -> &'static [String] {
    static HOME: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return Vec::new();
        };
        let resolved = home.canonicalize().ok();
        [Some(home), resolved]
            .into_iter()
            .flatten()
            .filter_map(|path| path.into_os_string().into_string().ok())
            .map(|path| path.trim_end_matches('/').to_owned())
            .filter(|path| !path.is_empty())
            .collect()
    })
}

fn resolve_tab_label(
    mode: huterm_config::TabLabel,
    style: huterm_config::TabDirectory,
    title: &str,
    metadata: &huterm_protocol::TerminalMetadata,
    home: &[String],
) -> String {
    let process = metadata
        .foreground_process()
        .filter(|value| !value.is_empty());
    let directory = metadata
        .directory()
        .and_then(|directory| directory_label(directory, style, home));
    match mode {
        // A process name is published only while a program holds the
        // foreground; at an idle shell the directory says more.
        huterm_config::TabLabel::Smart => match process {
            Some(process) => {
                metadata.foreground_title().unwrap_or(process).to_owned()
            }
            None => directory.unwrap_or_else(|| title.to_owned()),
        },
        huterm_config::TabLabel::Title => title.to_owned(),
        huterm_config::TabLabel::Process => process.unwrap_or(title).to_owned(),
        huterm_config::TabLabel::Directory => {
            directory.unwrap_or_else(|| title.to_owned())
        }
        huterm_config::TabLabel::ProcessAndDirectory => {
            match (process, directory) {
                (Some(process), Some(directory)) => {
                    format!("{process} · {directory}")
                }
                (Some(process), None) => process.to_owned(),
                (None, Some(directory)) => directory,
                (None, None) => title.to_owned(),
            }
        }
    }
}

/// Formats a directory for a tab label. Only a local directory can be under
/// this machine's home, so remote paths stay absolute.
fn directory_label(
    directory: &huterm_protocol::TerminalDirectory,
    style: huterm_config::TabDirectory,
    home: &[String],
) -> Option<String> {
    let path = directory.path().trim_end_matches('/');
    if path.is_empty() {
        return directory.path().starts_with('/').then(|| "/".to_owned());
    }
    let under_home = directory
        .is_local()
        .then(|| {
            home.iter().find_map(|home| {
                if path == home {
                    Some(String::new())
                } else {
                    path.strip_prefix(home.as_str())?
                        .strip_prefix('/')
                        .map(str::to_owned)
                }
            })
        })
        .flatten();
    let display = match under_home {
        Some(rest) if rest.is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{rest}"),
        None => path.to_owned(),
    };
    match style {
        huterm_config::TabDirectory::Name => display
            .rsplit('/')
            .find(|part| !part.is_empty())
            .map(str::to_owned),
        huterm_config::TabDirectory::Path => Some(display),
        huterm_config::TabDirectory::Short => Some(shorten_path(&display)),
    }
}

/// Shortens every component but the last to its first character, keeping
/// a hidden directory's dot: `~/.config/huterm` becomes `~/.c/huterm`.
fn shorten_path(path: &str) -> String {
    let mut parts: Vec<String> = path.split('/').map(str::to_owned).collect();
    let last = parts.len().saturating_sub(1);
    for part in &mut parts[..last] {
        if part == "~" {
            continue;
        }
        let keep = if part.starts_with('.') { 2 } else { 1 };
        *part = part.chars().take(keep).collect();
    }
    parts.join("/")
}

#[derive(Default)]
struct PointerReveal {
    refresh_pending: bool,
    outside: bool,
    probe_at: Option<Instant>,
}

struct WorkspaceView {
    frame_clock: Rc<refresh::FrameClock>,
    quake: Option<quake_windows::Presentation>,
    /// This window's key in the window model, which owns its attachment,
    /// workspace, active tab, activation history, and restorable layout.
    window: gpui::WindowId,
    fullscreen: FullscreenController,
    fullscreen_work: crate::fullscreen_work::Work,
    /// The decorations, maximized, and fullscreen facts GPUI last
    /// reported; `sync_frame` samples them.
    frame_state: FrameState,
    /// The frame last pushed to GPUI as the client inset and background
    /// appearance; `None` before the first sample.
    applied_frame: Option<WindowFrame>,
    /// The window buttons the desktop last published.
    button_layout: ButtonLayout,
    fullscreen_insets: gpui::Edges<Pixels>,
    /// Areas beside a display notch while custom fullscreen covers it.
    notch_shelves: Option<crate::fullscreen::NotchShelves>,
    #[cfg(target_os = "macos")]
    native_fullscreen: Option<crate::native_fullscreen::Adapter>,
    /// Tab views in display order, aligned with the window model's entries.
    tabs: Vec<TabView>,
    tab_scroll: Pixels,
    scroll_target: Option<Pixels>,
    last_scroll: Instant,
    /// Overlay scrollbar for the tab strip; axes follow the tab placement.
    tab_scrollbars: Scrollbars,
    sidebar_width: Pixels,
    resizing_sidebar: bool,
    reveal: Reveal,
    reveal_context: Option<(TabPosition, bool, bool)>,
    pointer_reveal: PointerReveal,
    layout_pending: bool,
    reorder: Option<TabReorder>,
    /// Fit tab widths measured during the last render.
    tab_widths: Vec<Pixels>,
    /// Title text widths by title, cleared on config reload.
    title_widths: HashMap<String, Pixels>,
    config: Config,
    family: String,
    metrics: GridMetrics,
    focus: FocusHandle,
    busy: bool,
    close: CloseState,
    exited_tabs: ExitQueue,
    notices: NoticeStack,
    /// Focus for the toast column; `notices` bindings match while it holds
    /// focus.
    notice_focus: FocusHandle,
    focused_notice: Option<NoticeId>,
    /// Toasts under the pointer; ids of dismissed toasts are ignored.
    hovered_notices: HashSet<NoticeId>,
    palette: Option<Entity<CommandPalette>>,
    palette_refresh_state: Option<PaletteRefreshState>,
    /// A cancelled search query and when it was cancelled.
    retained_query: Option<(String, Instant)>,
    recent: RecentCommands,
    startup_reporter: Option<WeakEntity<WorkspaceView>>,
    /// The open window, tab, or terminal menu.
    menu: Option<OpenMenu>,
    /// Where the menu button was last painted, for anchoring the menu and
    /// letting a press on it toggle rather than dismiss; `None` while the
    /// button is not drawn.
    menu_button_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The window controls' painted bounds in Huterm's own title row, for
    /// smoke pointer fixtures; `None` while the row is not drawn.
    /// The painted window buttons, by [`WindowButton::index`].
    window_button_bounds: Rc<[Cell<Option<Bounds<Pixels>>>; 3]>,
    /// A primary press on the drawn title row's empty space that has not
    /// yet moved: the first drag motion turns it into a window move. Any
    /// press elsewhere clears it.
    title_row_press: Rc<Cell<bool>>,
    /// How many window moves the title row has started, for smokes.
    title_row_moves: Rc<Cell<u32>>,
    /// Where the open menu was last painted, for outside-press dismissal.
    menu_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The About panel's details while it is showing.
    about: Option<AboutDetails>,
    /// The native window title last set, so unchanged titles are not reset.
    window_title: String,
    /// The close dialog's group headings as the last render drew them, for
    /// smoke state; `None` while no dialog was drawn.
    drawn_dialog_groups: Option<String>,
}

/// Which menu is open, and what its picks act on.
#[derive(Clone, Debug, Eq, PartialEq)]
enum MenuKind {
    /// The window menu, from its button, `open_menu`, or empty bar space.
    Window,
    /// A tab's context menu.
    Tab(TabId),
    /// The context menu of the terminal in `tab`, with the destination of
    /// the link under the pointer when it opened.
    Terminal { tab: TabId, link: Option<String> },
}

impl MenuKind {
    /// The tab the menu's tab-targeted picks act on.
    fn tab(&self) -> Option<TabId> {
        match self {
            Self::Window => None,
            Self::Tab(tab) | Self::Terminal { tab, .. } => Some(*tab),
        }
    }
}

struct OpenMenu {
    view: Entity<MenuView>,
    kind: MenuKind,
    /// Where a context menu opened, in window coordinates; `None` anchors
    /// the menu to its button.
    pointer: Option<gpui::Point<Pixels>>,
    /// The key contexts captured when the menu opened, for shortcut text.
    contexts: Vec<KeyContext>,
}

/// Where focus goes when the window menu closes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MenuFocusReturn {
    /// The active terminal, after Escape, a pick, an outside press, or a
    /// command. Returning to the menu button would leave terminal bindings
    /// inactive until the user clicked back into the terminal.
    Terminal,
    /// Whatever takes focus next, such as a close confirmation.
    Keep,
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
        self.fullscreen_work.wake.stop();
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

#[derive(Clone, Debug, Eq, PartialEq)]
enum CloseTarget {
    Tab(TabId),
    /// Several tabs of this window, distinct and in window order. Always
    /// two or more: [`tabs_target`] normalizes shorter lists.
    Tabs(Vec<TabId>),
    Window,
    Application,
}

impl CloseTarget {
    fn tabs(&self) -> Option<Vec<TabId>> {
        match self {
            Self::Tab(id) => Some(vec![*id]),
            Self::Tabs(ids) => Some(ids.clone()),
            Self::Window | Self::Application => None,
        }
    }
}

/// The owner-side bookkeeping after the notice stack changed: focus and
/// hover state for notices that no longer exist are dropped, and expiry
/// pauses only while a live toast is hovered or focused. Returns whether the
/// focused toast went away, so the caller returns focus to the terminal.
fn reconcile_notice_state(
    notices: &mut NoticeStack,
    focused: &mut Option<NoticeId>,
    hovered: &mut HashSet<NoticeId>,
    now: Instant,
) -> bool {
    let focus_lost = focused.take_if(|id| notices.get(*id).is_none()).is_some();
    hovered.retain(|id| notices.get(*id).is_some());
    if focused.is_some() || !hovered.is_empty() {
        notices.pause(now);
    } else {
        notices.resume(now);
    }
    focus_lost
}

/// The tab target for `tabs`: none when empty, a single tab for one, and a
/// set otherwise.
fn tabs_target(tabs: Vec<TabId>) -> Option<CloseTarget> {
    match tabs.as_slice() {
        [] => None,
        [tab] => Some(CloseTarget::Tab(*tab)),
        _ => Some(CloseTarget::Tabs(tabs)),
    }
}

/// Window or application scope wins over tabs. A tab set unions with any
/// other tab target; a single tab replaces a single tab, so repeated closes
/// of one tab coalesce and [`CloseState::checked`] can requeue a second.
/// Whether a close commit was refused and must be assessed again. Any other
/// result, including a terminal teardown error, means the runtime accepted
/// the close and already removed its structure.
fn close_commit_retries(result: &Result<(), MuxError>) -> bool {
    matches!(
        result,
        Err(MuxError::StaleClose | MuxError::ConfirmationRequired)
    )
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
        (Some(CloseTarget::Tab(_)), requested @ CloseTarget::Tab(_))
        | (None, requested) => requested,
        (Some(pending), requested) => {
            let mut union = pending.tabs().unwrap_or_default();
            for tab in requested.tabs().unwrap_or_default() {
                if !union.contains(&tab) {
                    union.push(tab);
                }
            }
            tabs_target(union).unwrap_or(requested)
        }
    }
}

/// Tabs in `order` other than `tab`.
fn other_tabs(order: &[TabId], tab: TabId) -> Vec<TabId> {
    order.iter().copied().filter(|id| *id != tab).collect()
}

/// Tabs in `order` after `tab`; empty when `tab` is last or absent.
fn tabs_after(order: &[TabId], tab: TabId) -> Vec<TabId> {
    order
        .iter()
        .position(|id| *id == tab)
        .map(|index| order[index + 1..].to_vec())
        .unwrap_or_default()
}

#[derive(Default)]
struct CloseState {
    current: Option<CloseTarget>,
    pending: Option<CloseTarget>,
    confirmation: Option<CloseTarget>,
    assessment: Option<CloseAssessment>,
    /// The confirmation button holding keyboard focus; reset to the primary
    /// button whenever a confirmation opens.
    dialog_focus: DialogFocus,
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
        self.current = Some(target.clone());
        target
    }
    fn queue(&mut self, target: CloseTarget) {
        self.pending = Some(merge_close(
            self.pending.take(),
            merge_close(self.current.clone(), target),
        ));
    }
    fn checked(&mut self, foreground: bool) -> Option<CloseDecision> {
        let target = self.current.take()?;
        let pending = self.pending.take();
        let effective = merge_close(pending.clone(), target.clone());
        if effective != target {
            return Some(CloseDecision::Check(effective));
        }
        // A second, different tab close follows the first; wider requests
        // subsume narrower requests and repeated closes of one tab coalesce.
        if matches!((&target, &pending), (CloseTarget::Tab(first), Some(CloseTarget::Tab(second))) if first != second)
        {
            self.pending = pending;
        }
        if foreground {
            self.confirmation = Some(target.clone());
            self.dialog_focus = DialogFocus::Primary;
            Some(CloseDecision::Confirm(target))
        } else {
            Some(CloseDecision::Close(target))
        }
    }
    /// Starts committing `target`: takes the assessment and ends the
    /// confirmation, so a later cancel cannot reach the commit. Returns the
    /// assessment and whether the user confirmed this target, or `None`
    /// when there is no assessment to commit.
    fn begin_commit(
        &mut self,
        target: &CloseTarget,
    ) -> Option<(CloseAssessment, bool)> {
        let confirmed = self.confirmation.as_ref() == Some(target);
        let assessment = self.assessment.take()?;
        self.confirmation = None;
        self.current = Some(target.clone());
        Some((assessment, confirmed))
    }
    /// Whether a confirmation is showing for its Cancel button or Escape to
    /// cancel; false once its commit has begun.
    fn can_cancel_confirmation(&self) -> bool {
        self.confirmation.is_some()
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
    /// Refuses tab closes while a confirmation is showing, so a repeated
    /// close shortcut can neither confirm nor replace the dialog.
    fn check_tab_close_available(&self) -> Result<(), CommandError> {
        if self.confirmation.is_some() {
            Err(Self::confirmation_pending())
        } else {
            Ok(())
        }
    }
    /// Refuses `target` while the showing confirmation already covers it:
    /// a repeated close-window shortcut would otherwise re-assess the same
    /// target and reset the dialog's focused button. A wider target, such
    /// as closing the window while a tab confirmation shows, still merges.
    fn check_close_available(
        &self,
        target: &CloseTarget,
    ) -> Result<(), CommandError> {
        match &self.confirmation {
            Some(showing)
                if merge_close(Some(showing.clone()), target.clone())
                    == *showing =>
            {
                Err(Self::confirmation_pending())
            }
            _ => Ok(()),
        }
    }
    fn confirmation_pending() -> CommandError {
        CommandError::Unavailable("close confirmation pending".to_owned())
    }
    /// Takes the queued request, dropping tabs that no longer exist.
    fn take_pending(
        &mut self,
        contains: impl Fn(TabId) -> bool,
    ) -> Option<CloseTarget> {
        match self.pending.take()? {
            CloseTarget::Tab(id) => {
                contains(id).then_some(CloseTarget::Tab(id))
            }
            CloseTarget::Tabs(ids) => tabs_target(
                ids.into_iter().filter(|id| contains(*id)).collect(),
            ),
            target => Some(target),
        }
    }
}

impl WorkspaceView {
    /// This window's record in the window model. `None` only after GPUI
    /// removed the window while a queued update of this view still runs.
    fn record<'a>(&self, cx: &'a App) -> Option<&'a WindowRecord> {
        cx.global::<Desktop>().windows.record(self.window)
    }

    fn active_tab(&self, cx: &App) -> Option<TabId> {
        self.record(cx).and_then(|record| record.active)
    }

    fn workspace_id(&self, cx: &App) -> Option<WorkspaceId> {
        self.record(cx).and_then(|record| record.workspace)
    }

    fn attachment_id(&self, cx: &App) -> Option<AttachmentId> {
        self.record(cx).and_then(|record| record.attachment)
    }

    /// Appends a spawned tab's view and model entry, activates it, then
    /// applies the installed order, so a tab the projection placed earlier
    /// lands in its position.
    fn push_tab_view(&mut self, tab: TabView, cx: &mut App) {
        let entry = TabEntry {
            id: tab.id,
            terminal: tab.terminal,
            title: tab.title(self.config.tabs, cx),
        };
        self.tabs.push(tab);
        cx.global_mut::<Desktop>()
            .windows
            .open_tab(self.window, entry);
        mark_titles_changed(self.window, cx);
        self.debug_assert_tabs_aligned(cx);
        self.sync_tab_order(cx);
    }

    /// Applies the installed order (see [`installed_order`]) to the views
    /// and model entries when it differs from the current order; returns
    /// whether anything moved.
    fn sync_tab_order(&mut self, cx: &mut App) -> bool {
        let current: Vec<TabId> = self.tabs.iter().map(|tab| tab.id).collect();
        let order = {
            let state = cx.global::<Desktop>().hierarchy.state();
            let projected = self
                .workspace_id(cx)
                .and_then(|workspace| state.workspace_tabs(workspace))
                .unwrap_or(&[]);
            installed_order(projected, &current)
        };
        order != current && self.apply_tab_view_order(&order, cx)
    }

    /// Reconciles this window once its own structural completion has
    /// cleared `busy`, applying removals reconcile deferred meanwhile. A
    /// removed active tab hands selection and focus to the new active tab in
    /// the same update, so no keystroke lands without a focused terminal.
    fn reconcile_own(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let active = self.active_tab(cx);
        let change = self.reconcile(&Touched::default(), terminating(cx), cx);
        if !change.changed {
            return;
        }
        if let Some(next) = self.active_tab(cx)
            && Some(next) != active
        {
            self.measure_tab_widths(window, cx);
            self.select(next, window, cx);
        }
        cx.notify();
    }

    /// Brings this window in line with the projection for `touched`.
    fn reconcile(
        &mut self,
        touched: &Touched,
        terminating: bool,
        cx: &mut Context<'_, Self>,
    ) -> WindowChange {
        reconcile_window(
            &mut WindowReconcile { view: self, cx },
            touched,
            terminating,
        )
    }

    /// Applies canonical order to the model entries and tab views; returns
    /// false and changes neither when `order` does not match the tabs.
    fn apply_tab_view_order(&mut self, order: &[TabId], cx: &mut App) -> bool {
        // The model decides, so a rejected order can never leave the tab bar
        // reordered while palette order and Quit capture keep the old one.
        if !cx
            .global_mut::<Desktop>()
            .windows
            .apply_order(self.window, order)
        {
            return false;
        }
        let applied = apply_tab_order(&mut self.tabs, order, |tab| tab.id);
        debug_assert!(applied, "tab views rejected an order the model took");
        self.debug_assert_tabs_aligned(cx);
        true
    }

    /// Drops tab views and their model entries without closing anything; the
    /// model moves the active tab as closing would. The installed order is
    /// applied afterwards, so a removal that follows a missed reorder still
    /// converges.
    fn drop_tab_views(&mut self, ids: &[TabId], cx: &mut App) {
        if !self.tabs.iter().any(|tab| ids.contains(&tab.id)) {
            return;
        }
        self.tabs.retain(|tab| !ids.contains(&tab.id));
        cx.global_mut::<Desktop>()
            .windows
            .close_tabs(self.window, ids);
        mark_titles_changed(self.window, cx);
        self.debug_assert_tabs_aligned(cx);
        self.sync_tab_order(cx);
    }

    /// Records the bounds this window reopens with, unless quake owns its
    /// geometry.
    fn publish_restorable_bounds(&self, cx: &mut App) {
        if self.quake.is_none() {
            cx.global_mut::<Desktop>().windows.set_layout_bounds(
                self.window,
                self.fullscreen.restorable_bounds(),
            );
        }
    }

    /// Republishes every tab's title, for changes outside the tab's own
    /// activity drain, such as label settings.
    fn publish_tab_titles(&self, cx: &mut App) {
        let titles: Vec<_> = self
            .tabs
            .iter()
            .map(|tab| (tab.id, tab.title(self.config.tabs, cx)))
            .collect();
        let windows = &mut cx.global_mut::<Desktop>().windows;
        let mut changed = false;
        for (tab, title) in titles {
            changed |= windows.set_tab_title(self.window, tab, &title);
        }
        if changed {
            mark_titles_changed(self.window, cx);
        }
    }

    /// Records whether this window shows a title consumer: an open palette
    /// or a close confirmation.
    fn sync_title_consumer(&self, cx: &mut App) {
        let host = self.palette.is_some() || self.close.confirmation.is_some();
        cx.global_mut::<Desktop>()
            .title_consumers
            .set_host(self.window, host);
    }

    fn debug_assert_tabs_aligned(&self, cx: &App) {
        debug_assert!(
            self.record(cx).is_none_or(|record| {
                record
                    .tabs
                    .iter()
                    .map(|entry| entry.id)
                    .eq(self.tabs.iter().map(|tab| tab.id))
            }),
            "tab views and window model entries diverged"
        );
    }
}

/// A window's views and model entries as [`reconcile_window`] changes them.
struct WindowReconcile<'a, 'b> {
    view: &'a mut WorkspaceView,
    cx: &'a mut Context<'b, WorkspaceView>,
}

impl ReconcileTarget for WindowReconcile<'_, '_> {
    fn projection(&self) -> &HierarchyState {
        self.cx.global::<Desktop>().hierarchy.state()
    }

    fn workspace(&self) -> Option<WorkspaceId> {
        self.view.workspace_id(self.cx)
    }

    fn installed(&self) -> Vec<(TabId, u64)> {
        self.view
            .tabs
            .iter()
            .map(|tab| (tab.id, tab.committed))
            .collect()
    }

    fn busy(&self) -> bool {
        self.view.busy
    }

    fn drop_tabs(&mut self, tabs: &[TabId]) {
        self.view.drop_tab_views(tabs, self.cx);
    }

    fn apply_order(&mut self, order: &[TabId]) -> bool {
        self.view.apply_tab_view_order(order, self.cx)
    }

    fn remember_names(&mut self, tab: TabId) {
        let Some(info) = self.cx.global::<Desktop>().hierarchy.state().tab(tab)
        else {
            return;
        };
        if let Some(view) =
            self.view.tabs.iter_mut().find(|view| view.id == tab)
        {
            view.names.remember(info);
        }
    }

    fn title(&self, tab: TabId) -> String {
        self.view
            .tabs
            .iter()
            .find(|view| view.id == tab)
            .map(|view| view.title(self.view.config.tabs, self.cx))
            .unwrap_or_default()
    }

    fn publish_title(&mut self, tab: TabId, title: &str) -> bool {
        self.cx.global_mut::<Desktop>().windows.set_tab_title(
            self.view.window,
            tab,
            title,
        )
    }
}

impl WorkspaceView {
    /// The facts that decide whether this window has a title-bar row.
    fn tab_host(&self) -> TabHost {
        TabHost {
            platform: Platform::current(),
            fullscreen: self.fullscreen.chrome_hidden,
            quake: self.quake.is_some(),
            client_decorations: self.frame_state.client_decorations(),
        }
    }

    /// The frame this window's chrome sits in, for its resolved position
    /// and the decorations GPUI last reported.
    fn window_frame(&self) -> WindowFrame {
        WindowFrame::resolve(
            self.layout_tabs().position,
            self.frame_state,
            self.button_layout,
        )
    }

    /// The title row above the terminal, if the window has one.
    fn title_row(&self) -> Pixels {
        title_row_height(self.chrome_hidden(), self.window_frame())
    }

    /// Samples the decorations GPUI reports and pushes the frame they
    /// imply: the client inset the compositor keeps clear for resizing
    /// and the shadow, and a transparent background while that inset is
    /// drawn around. Returns whether the sample or the frame changed, so
    /// the caller relays out.
    fn sync_frame(&mut self, window: &mut Window) -> bool {
        let state = frame_state(window);
        let state_changed = self.frame_state != state;
        self.frame_state = state;
        let frame = self.window_frame();
        if self.applied_frame == Some(frame) {
            return state_changed;
        }
        if self.applied_frame.is_some_and(WindowFrame::decorated)
            != frame.decorated()
        {
            window.set_background_appearance(if frame.decorated() {
                gpui::WindowBackgroundAppearance::Transparent
            } else {
                gpui::WindowBackgroundAppearance::Opaque
            });
        }
        // GPUI writes `_GTK_FRAME_EXTENTS` only when the inset changes,
        // so a zero inset on a server-decorated window sets no property.
        window.set_client_inset(frame.inset.top.max(px(0.0)));
        self.applied_frame = Some(frame);
        self.layout_pending = true;
        true
    }

    /// The tab configuration with its position resolved for this window.
    /// Layout, visibility, placement, and the terminals all read this;
    /// `self.config.tabs.position` is only the configured value.
    fn layout_tabs(&self) -> TabsConfig {
        layout_tabs(self.config.tabs, self.tab_host())
    }

    fn presentation(&self) -> Presentation {
        Presentation::resolve(
            self.tabs.len(),
            self.config.tabs.always_show,
            self.tab_fullscreen_context(),
            // A bar on the notch shelf has nowhere to hide.
            self.config.tabs.auto_hide_in_fullscreen
                && self.notch_shelf().is_none(),
        )
    }

    /// The shelf beside the notch a top bar should occupy, when configured
    /// and available.
    fn notch_shelf(&self) -> Option<Bounds<Pixels>> {
        select_notch_shelf(self.layout_tabs(), self.notch_shelves)
    }

    fn chrome_layout(&self, window: &Window) -> ChromeLayout {
        let tabs = self.layout_tabs();
        ChromeLayout::for_tabs(
            window.viewport_size(),
            self.title_row(),
            tabs,
            self.sidebar_width,
            self.fullscreen_insets,
            self.notch_shelf(),
            self.window_frame(),
        )
        .present(
            self.presentation(),
            tabs.position,
            self.reveal.progress,
        )
    }

    fn sync_tab_layout(&mut self, window: &Window, cx: &mut Context<'_, Self>) {
        // Config and bounds changes must reach hidden or frame-blocked PTYs.
        // Consume this once; pointer events do not require scanning every tab.
        let geometry_changed = std::mem::take(&mut self.layout_pending);
        let presentation = self.presentation();
        let chrome_hidden = self.chrome_hidden();
        let notch_shelf = self.notch_shelf();
        let tabs_config = self.layout_tabs();
        let window_frame = self.window_frame();
        let quake_presenting = self.quake_presenting();
        let overlay = (presentation == Presentation::Overlay
            && self.reveal.progress > 0.0)
            .then(|| {
                let layout = self.chrome_layout(window);
                layout.tabs.intersect(&layout.terminal)
            });
        let mut changed_any = false;
        for tab in &self.tabs {
            tab.view.update(cx, |terminal, cx| {
                let scale_changed =
                    terminal.metrics.at_scale(window.scale_factor())
                        != terminal.metrics;
                let cell_changed = terminal.last_cell_size
                    != Some(terminal.physical_cell_size());
                // The tab bar appearing or hiding is not a resize to report.
                let bar_toggled = terminal.tab_presentation != presentation;
                let changed = bar_toggled
                    || terminal.tabs_config != tabs_config
                    || terminal.sidebar_width != self.sidebar_width
                    || terminal.chrome_hidden != chrome_hidden
                    || terminal.fullscreen_insets != self.fullscreen_insets
                    || terminal.notch_shelf != notch_shelf
                    || terminal.window_frame != window_frame;
                changed_any |= geometry_changed
                    || changed
                    || scale_changed
                    || cell_changed;
                terminal.tab_overlay = overlay;
                terminal.tab_presentation = presentation;
                terminal.tabs_config = tabs_config;
                terminal.sidebar_width = self.sidebar_width;
                terminal.chrome_hidden = chrome_hidden;
                terminal.fullscreen_insets = self.fullscreen_insets;
                terminal.notch_shelf = notch_shelf;
                terminal.window_frame = window_frame;
                terminal.quiet_resize = quake_presenting;
                if geometry_changed || changed || scale_changed || cell_changed
                {
                    terminal.resize_to_layout(window, bar_toggled);
                    cx.notify();
                }
            });
        }
        if changed_any {
            cx.notify();
        }
    }

    /// A Quake window is still showing, settling, or hiding; its layout
    /// changes are presentation, not resizes to report.
    fn quake_presenting(&self) -> bool {
        self.quake
            .as_ref()
            .is_some_and(quake_windows::Presentation::presenting)
    }

    fn defer_pointer_refresh(
        &mut self,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.pointer_reveal.refresh_pending {
            return;
        }
        self.pointer_reveal.refresh_pending = true;
        // Capture runs before terminal gesture handlers. Reconcile after they
        // have acquired or released ownership, even if they stop propagation.
        cx.defer_in(window, |view, window, cx| {
            view.pointer_reveal.refresh_pending = false;
            view.refresh_tab_visibility(window, cx);
        });
    }

    fn update_pointer_probe(&mut self, now: Instant, window: &Window) {
        let needed = cfg!(target_os = "macos")
            && self.presentation() == Presentation::Overlay
            && self.quake_visible()
            && window.is_window_active()
            && self.close.confirmation.is_none()
            && (self.reveal.progress > 0.0
                // AppKit's native fullscreen top edge can be outside the
                // content view and never deliver a window mouse event.
                || (self.layout_tabs().position == TabPosition::Top
                    && window.is_fullscreen()));
        self.pointer_reveal.probe_at = tab_visibility::pointer_probe_deadline(
            self.pointer_reveal.probe_at,
            now,
            needed,
        );
    }

    fn refresh_tab_visibility(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let position = self.layout_tabs().position;
        let context = (
            position,
            self.tab_fullscreen_context(),
            self.config.tabs.auto_hide_in_fullscreen,
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
            .active_view(cx)
            .is_some_and(|tab| tab.read(cx).owns_pointer_gesture());
        let layout = self.chrome_layout(window);
        let pointer = window.mouse_position();
        let bounds = layout.terminal;
        let hovered =
            window.is_window_hovered() && !self.pointer_reveal.outside;
        #[cfg(target_os = "macos")]
        let hovered = hovered
            && self.native_fullscreen.as_ref().is_none_or(
                crate::native_fullscreen::Adapter::pointer_on_display,
            );
        let inside = hovered && bounds.contains(&pointer);
        let edge = inside
            && match position {
                TabPosition::Top | TabPosition::Titlebar => {
                    pointer.y <= bounds.origin.y + px(2.0)
                }
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
        // An open window menu holds the bar that anchors its button.
        let hover = !gesture
            && (edge
                || (self.reveal.progress > 0.0
                    && hovered
                    && layout.tabs.contains(&pointer))
                || self.reorder.is_some()
                || self.resizing_sidebar
                || self.menu.is_some());
        if self
            .reveal
            .set_input(Instant::now(), hover, enabled && !gesture)
        {
            cx.notify();
        }
        self.update_pointer_probe(Instant::now(), window);
        self.frame_clock.animate(
            cx.entity().downgrade(),
            refresh::Animated::animation_schedule(self, Instant::now()),
            cx,
        );
    }

    fn tab_strip(&self, window: &Window) -> TabStrip {
        let tabs = self.layout_tabs();
        let layout = self.chrome_layout(window);
        let extents = if tabs.width == TabWidth::Fit {
            // Render measures titles; tabs added since then use the minimum.
            TabExtents::Fit(
                (0..self.tabs.len())
                    .map(|index| {
                        self.tab_widths
                            .get(index)
                            .copied()
                            .unwrap_or(px(tabs.min_width))
                    })
                    .collect(),
            )
        } else {
            TabExtents::Uniform(self.tabs.len())
        };
        TabStrip::new(
            layout.strip_bounds(tabs),
            tabs.position.vertical(),
            extents,
            self.tab_scroll,
            self.bar_menu_placement() == MenuButtonPlacement::BarEnd,
        )
    }

    fn reveal_active(&mut self, window: &Window, cx: &App) {
        self.scroll_target = None;
        let active = self.active_tab(cx);
        if let Some(index) =
            self.tabs.iter().position(|tab| Some(tab.id) == active)
        {
            let revealed = self.tab_strip(window).reveal(index);
            // Only an actual move shows the indicator; switching to a tab
            // that is already in view leaves it hidden.
            if revealed != self.tab_scroll {
                self.tab_scroll = revealed;
                self.show_tab_scrollbar();
            }
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
        self.show_tab_scrollbar();
        cx.notify();
    }

    /// Reveals the strip's scrollbar; the window pump fades it out.
    fn show_tab_scrollbar(&mut self) {
        let now = Instant::now();
        self.tab_scrollbars.show(Axis::Vertical, now);
        self.tab_scrollbars.show(Axis::Horizontal, now);
    }

    /// Enables the scrollbar axis matching the tab placement. Idempotent, so
    /// render calls it each frame and reload needs no extra hook.
    fn sync_tab_scrollbars(&mut self) {
        let tabs = self.layout_tabs();
        let vertical = tabs.position.vertical();
        self.tab_scrollbars.set_axis(
            Axis::Vertical,
            vertical.then(|| tab_column_scrollbar(tabs.position)),
        );
        self.tab_scrollbars.set_axis(
            Axis::Horizontal,
            (!vertical).then(|| tab_row_scrollbar(self.config.tabs.style)),
        );
    }

    fn tab_scrollbar_axis(strip: &TabStrip) -> Axis {
        if strip.vertical {
            Axis::Vertical
        } else {
            Axis::Horizontal
        }
    }

    /// The scrolled part of the strip: its bounds minus the new-tab slot.
    fn tab_scrollbar_bounds(strip: &TabStrip) -> Bounds<Pixels> {
        let mut bounds = strip.bounds;
        if strip.vertical {
            bounds.size.height = strip.available();
        } else {
            bounds.size.width = strip.available();
        }
        bounds
    }

    fn tab_scrollbar_geometries(
        strip: &TabStrip,
        tabs: TabsConfig,
    ) -> ScrollbarGeometries {
        let options = if strip.vertical {
            tab_column_scrollbar(tabs.position)
        } else {
            tab_row_scrollbar(tabs.style)
        };
        let available = f32::from(strip.available());
        let geometry = ScrollbarGeometry::new(
            available,
            available + f32::from(strip.max_offset()),
            available,
            f32::from(strip.offset),
            options.origin,
            options.margins,
        );
        if strip.vertical {
            ScrollbarGeometries::vertical(geometry)
        } else {
            ScrollbarGeometries {
                vertical: None,
                horizontal: geometry,
            }
        }
    }

    fn tab_scrollbar_pointer_moved(
        &mut self,
        position: gpui::Point<Pixels>,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let strip = self.tab_strip(window);
        if self.tab_scrollbars.pointer_moved(
            &Self::tab_scrollbar_geometries(&strip, self.layout_tabs()),
            Self::tab_scrollbar_bounds(&strip),
            position,
            Instant::now(),
        ) {
            cx.notify();
        }
    }

    /// Mouse down on the strip's scrollbar; true when it took the press.
    fn tab_scrollbar_press(
        &mut self,
        position: gpui::Point<Pixels>,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let strip = self.tab_strip(window);
        let geometries =
            Self::tab_scrollbar_geometries(&strip, self.layout_tabs());
        let Some((axis, press)) = self.tab_scrollbars.press(
            &geometries,
            Self::tab_scrollbar_bounds(&strip),
            position,
            Instant::now(),
        ) else {
            return false;
        };
        if let Press::Jump(thumb_start) = press {
            self.tab_scrollbar_seek(axis, &geometries, thumb_start, cx);
        }
        cx.notify();
        true
    }

    fn tab_scrollbar_drag_to(
        &mut self,
        position: gpui::Point<Pixels>,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let strip = self.tab_strip(window);
        let geometries =
            Self::tab_scrollbar_geometries(&strip, self.layout_tabs());
        if let Some((axis, thumb_start)) = self.tab_scrollbars.drag_to(
            Self::tab_scrollbar_bounds(&strip),
            position,
            Instant::now(),
        ) {
            self.tab_scrollbar_seek(axis, &geometries, thumb_start, cx);
        }
    }

    fn tab_scrollbar_release(&mut self, cx: &mut Context<'_, Self>) {
        if self.tab_scrollbars.release(Instant::now()) {
            cx.notify();
        }
    }

    fn tab_scrollbar_seek(
        &mut self,
        axis: Axis,
        geometries: &ScrollbarGeometries,
        thumb_start: f32,
        cx: &mut Context<'_, Self>,
    ) {
        let geometry = match axis {
            Axis::Vertical => geometries.vertical,
            Axis::Horizontal => geometries.horizontal,
        };
        if let Some(geometry) = geometry {
            self.scroll_target = None;
            self.tab_scroll = px(geometry.offset_for_thumb_start(thumb_start));
            cx.notify();
        }
    }

    fn resize_sidebar(
        &mut self,
        pointer: gpui::Point<Pixels>,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let desired = if self.layout_tabs().position == TabPosition::Left {
            pointer.x
        } else {
            window.viewport_size().width - pointer.x
        };
        self.sidebar_width = desired.clamp(px(140.0), px(400.0));
        cx.global_mut::<Desktop>()
            .windows
            .set_sidebar_width(self.window, self.sidebar_width);
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
            && Some(source.workspace) == self.workspace_id(cx)
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
        let Some(workspace) = self.workspace_id(cx) else {
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
            let was_scrolling = drag.dragging
                && drag
                    .strip
                    .autoscroll(drag.pointer, Duration::from_millis(16))
                    != self.tab_scroll;
            drag.pointer = pointer;
            if !drag.dragging
                && (pointer - drag.origin).magnitude() > TAB_DRAG_THRESHOLD
            {
                drag.dragging = true;
            }
            drag.strip = strip;
            if !was_scrolling {
                self.last_scroll = Instant::now();
            }
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
        if changed {
            self.show_tab_scrollbar();
        }
        changed
    }

    fn restore_tab_focus(&self, window: &mut Window, cx: &mut App) {
        if let Some(tab) = self.active_view(cx) {
            let focus = tab.read(cx).focus.clone();
            focus.focus(window, cx);
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
        let app = cx.to_async();
        cx.spawn_in(window, async move |view, cx| {
            let Committed { result, seq } = task.await;
            // Reconcile applies the committed order while draining; this
            // completion only restores focus and reports failure.
            projected(&app, seq).await;
            let _ = view.update_in(cx, |view, window, cx| {
                view.busy = false;
                if let Err(error) = result
                    && !terminating(cx)
                {
                    view.report_failure(
                        "Reorder tab",
                        format!("Cannot reorder tab: {error}"),
                        cx,
                    );
                }
                view.reconcile_own(window, cx);
                view.restore_tab_focus(window, cx);
                view.resume_close(window, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn active_view(&self, cx: &App) -> Option<Entity<TerminalView>> {
        let active = self.active_tab(cx)?;
        self.tabs
            .iter()
            .find(|tab| tab.id == active)
            .map(|tab| tab.view.clone())
    }

    fn refresh_tab(
        &mut self,
        tab_id: TabId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<bool> {
        let tab = self.tabs.iter().find(|tab| tab.id == tab_id)?;
        let result = tab
            .view
            .update(cx, |terminal, cx| terminal.refresh(window, cx));
        let (title, status) = tab.label(self.config.tabs, cx);
        // Title, metadata, and exit changes all arrive through this drain;
        // the model only records a title that actually changed.
        if cx.global_mut::<Desktop>().windows.set_tab_title(
            self.window,
            tab_id,
            &with_status_suffix(title.clone(), status),
        ) {
            mark_titles_changed(self.window, cx);
        }
        if !result.failures.is_empty() {
            // A tab is one keyed source: its latest failures replace the
            // earlier ones, so a flooding tab cannot fill the stack. The
            // replaced toast may have been the focused one.
            let source = NoticeSource::Terminal {
                tab: tab_id,
                title: title.clone(),
            };
            let replacements = result
                .failures
                .into_iter()
                .map(|failure| terminal_notice(tab_id, &title, failure))
                .collect();
            self.notices
                .replace_source(&source, replacements, Instant::now());
            self.reconcile_notices(window, cx);
        }
        if result.exited && !self.config.terminal.close_on_exit {
            self.notices.push(
                exit_notice(tab_id, &title, result.exit_code),
                Instant::now(),
            );
            cx.notify();
        }
        if result.exited {
            self.exited_tabs.observe(
                tab_id,
                false,
                true,
                self.config.terminal.close_on_exit,
            );
        }
        if result.changed {
            cx.notify();
        }
        self.resume_close(window, cx);
        self.refresh_palette(cx);
        Some(result.more)
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
        let mut command = shell_command(
            self.metrics.at_scale(window.scale_factor()),
            &self.config.theme,
            self.config.terminal.term,
        )
        .map_err(|error| CommandError::Runtime(error.to_string()))?;
        let inherited_directory = self.active_view(cx).and_then(|view| {
            inherited_directory(
                self.config.terminal.new_tab_directory,
                &view.read(cx).metadata,
            )
        });
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let workspace = self.workspace_id(cx);
        let attachment = self.attachment_id(cx);
        let clipboard_allowed =
            self.config.terminal.clipboard_write.is_allowed();
        self.busy = true;
        cx.global_mut::<Desktop>().pending_spawns += 1;
        let task = cx.background_executor().spawn(async move {
            if let Some(directory) = inherited_directory
                && usable_launch_directory(&directory)
            {
                command.working_directory = directory;
            }
            runtime.open_tab(workspace, attachment, &command, clipboard_allowed)
        });
        let cleanup_runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let app = cx.to_async();
        cx.spawn_in(window, async move |view, cx| {
            let Committed { result, seq } = task.await;
            let resolution = projected(&app, seq).await;
            // A spawn the window never published; cleaned up exactly once
            // below, whichever path left it here.
            let mut unpublished = None;
            let mut result = Some(result);
            let update = view.update_in(cx, |view, window, cx| {
                view.busy = false;
                let terminating = cx
                    .global::<Desktop>()
                    .runtime
                    .terminating
                    .load(Ordering::Acquire);
                if let Some(result) = result.take() {
                    match (spawn_disposition(resolution, terminating), result) {
                        (SpawnDisposition::Publish, Ok(spawned)) => {
                            view.publish_spawn(spawned, seq, window, cx);
                        }
                        (SpawnDisposition::Publish, Err(error)) => {
                            if view.spawn_failed(&error, window, cx) {
                                return;
                            }
                        }
                        (
                            SpawnDisposition::Abandon { cleanup, notify },
                            result,
                        ) => {
                            if notify {
                                let reason = result.as_ref().map_or_else(
                                    ToString::to_string,
                                    |_| {
                                        "runtime structure is unavailable"
                                            .to_owned()
                                    },
                                );
                                view.notify(
                                    NoticeContent::command_failure(
                                        "Cannot open tab",
                                        format!("Cannot open tab: {reason}"),
                                    ),
                                    cx,
                                );
                            }
                            if cleanup {
                                unpublished = result.ok();
                            }
                        }
                    }
                }
                view.reconcile_own(window, cx);
                view.resume_close(window, cx);
                cx.notify();
            });
            // With the window gone, clean up a successful spawn unless
            // teardown has committed and owns every terminal, as
            // `spawn_disposition` decides for a cancelled wait.
            if update.is_err() && !app.update(|cx| terminating(cx)) {
                unpublished = result.take().and_then(Result::ok);
            }
            if let Some((session, workspace, opened, attachment, authority)) =
                unpublished
            {
                drop(authority);
                cx.background_executor()
                    .spawn(async move {
                        cleanup_runtime.cleanup_spawn(
                            session,
                            workspace,
                            opened.tab.id,
                            attachment,
                        )
                    })
                    .await;
            }
            // The spawn stays pending through its wait and publication, so
            // Quit requested meanwhile resumes only after it settles.
            app.update(|cx| {
                let desktop = cx.global_mut::<Desktop>();
                if settle_spawn(
                    &mut desktop.pending_spawns,
                    &mut desktop.quit_pending,
                ) {
                    cx.defer(|cx| invoke(ids::QUIT).dispatch(cx));
                } else if desktop.pending_spawns == 0 {
                    maybe_exit(cx);
                }
            });
        })
        .detach();
        cx.notify();
        Ok(CommandOutcome::Accepted)
    }

    /// Installs a published spawn's terminal view, activates it, and records
    /// the sequence its tab committed at.
    fn publish_spawn(
        &mut self,
        spawned: Spawned,
        committed: u64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let (session, id, opened, attachment, authority) = spawned;
        self.startup_reporter = None;
        cx.global_mut::<Desktop>().windows.attach(
            self.window,
            attachment,
            session,
            id,
        );
        authority
            .host_effects
            .set_allowed(self.config.terminal.clipboard_write.is_allowed());
        let activity_client = opened.client.clone();
        let scroll_key = scroll_to_bottom_key(&cx.global::<Desktop>().keymap);
        let terminal = cx.new(|cx| {
            let mut terminal = TerminalView::new(
                opened.client,
                authority,
                Rc::clone(&self.frame_clock),
                &self.config,
                self.family.clone(),
                self.metrics.at_scale(window.scale_factor()),
                window,
                cx,
            );
            terminal.set_scroll_to_bottom_key(scroll_key, cx);
            terminal.tabs_config = self.layout_tabs();
            terminal.window_frame = self.window_frame();
            terminal
        });
        cx.subscribe_in(&terminal, window, Self::handle_context_menu_request)
            .detach();
        let tab_id = opened.tab.id;
        // The completion drained first, so the projection holds the tab.
        let names = cx
            .global::<Desktop>()
            .hierarchy
            .state()
            .tab(tab_id)
            .map(TabNames::from_info)
            .unwrap_or_default();
        let failure_wakes = terminal.read(cx).failure_wakes.clone();
        let activity_task = Self::tab_activity_task(
            tab_id,
            activity_client,
            failure_wakes,
            window,
            cx,
        );
        self.push_tab_view(
            TabView {
                id: tab_id,
                terminal: opened.tab.terminal_id,
                committed,
                names,
                view: terminal,
                _activity_task: activity_task,
            },
            cx,
        );
        self.select(tab_id, window, cx);
        self.reveal_tab_activity(window, cx);
        if let Some(state) = &self.quake {
            // This profile's earlier failed spawn no
            // longer applies; drop its latched notice.
            let desktop = cx.global_mut::<Desktop>();
            if desktop
                .quake
                .failed_spawn
                .as_ref()
                .is_some_and(|(name, _)| name == &state.name)
                && let Some((_, message)) = desktop.quake.failed_spawn.take()
            {
                desktop.latched.retain(|content| content.message != message);
                if self
                    .notices
                    .dismiss_where(|content| content.message == message)
                {
                    self.reconcile_notices(window, cx);
                }
            }
        }
    }

    /// The tab's activity drain: waits for terminal events or failures and
    /// refreshes the tab in bounded batches.
    fn tab_activity_task(
        tab_id: TabId,
        activity_client: RuntimeClient,
        failure_wakes: async_channel::Receiver<()>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Task<()> {
        let activity_probe = refresh_smoke::ActivityProbe::new(tab_id, cx);
        cx.spawn_in(window, async move |view, cx| {
            loop {
                if let Some(probe) = &activity_probe {
                    probe.waiting(true);
                }
                let stopped = TerminalView::wait_for_activity(
                    &activity_client,
                    &failure_wakes,
                )
                .await
                .is_err();
                if let Some(probe) = &activity_probe {
                    probe.waiting(false);
                }
                loop {
                    let more = view.update_in(cx, |view, window, cx| {
                        view.refresh_tab(tab_id, window, cx)
                    });
                    match more {
                        Ok(Some(true)) => {
                            cx.background_executor()
                                .timer(Duration::from_millis(1))
                                .await;
                        }
                        Ok(Some(false)) => break,
                        _ => return,
                    }
                }
                if stopped {
                    return;
                }
                // Bound metadata floods independently of frame delivery.
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
        })
    }

    /// Reports a failed spawn. Returns true when the failure removed the
    /// window, a quake window whose shell never started.
    fn spawn_failed(
        &mut self,
        error: &MuxError,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let message = format!("Cannot open tab: {error}");
        self.notify(
            NoticeContent::command_failure("Cannot open tab", message.clone())
                .action("Try Again", bare(ids::NEW_TAB)),
            cx,
        );
        if let Some(reporter) = self.startup_reporter.take() {
            report_deferred_failure(cx, Some(reporter), message.clone());
        }
        if self.quake.is_some() && self.tabs.is_empty() {
            latch_failure(cx, &message);
            if let Some(state) = &self.quake {
                cx.global_mut::<Desktop>().quake.failed_spawn =
                    Some((state.name.clone(), message));
            }
            eprintln!("Cannot start quake shell: {error}");
            self.remove_window(window, cx, false);
            return true;
        }
        false
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
            palette.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
            return Ok(CommandOutcome::Completed);
        }
        // A request that already carries every required argument has
        // nothing to prompt for; run it like any interactive invocation.
        if let Some(request) = request
            && interactive_spec(request).is_ok_and(|(_, prompt)| !prompt)
        {
            return self.invoke_interactive(request, window, cx);
        }
        self.check_available(true)?;
        if cx.global::<Desktop>().quitting
            || cx.global::<Desktop>().quit_pending
        {
            return Err(CommandError::Unavailable(
                "application is quitting".to_owned(),
            ));
        }

        let terminal = self.active_view(cx);
        if let Some(terminal) = &terminal {
            terminal.update(cx, TerminalView::clear_composition);
        }
        let active = self.active_tab(cx);
        let workspace = self.workspace_id(cx);
        let target = PaletteTarget {
            session: workspace.and_then(|workspace| {
                cx.global::<Desktop>()
                    .hierarchy
                    .state()
                    .workspace_session(workspace)
            }),
            workspace,
            tab: active,
            terminal: self
                .tabs
                .iter()
                .find(|tab| Some(tab.id) == active)
                .map(|tab| tab.terminal),
            terminal_view: terminal.map(|terminal| terminal.downgrade()),
            contexts: window.context_stack(),
        };
        let availability = self.palette_availability(&target, cx);
        let keymap = cx.global::<Desktop>().keymap.clone();
        let history = OwnedHistory::capture(&self.history_view(cx));
        let retained_query = if request.is_some() {
            None
        } else {
            self.take_retained_query()
        };
        let (hierarchy, hierarchy_seq) = self.palette_hierarchy(cx);
        let open = PaletteOpen {
            target,
            keymap,
            availability,
            colors: self.palette_colors(),
            placement: self.config.palette.placement,
            history,
            profiles: quake_profile_rows(cx),
            request,
            retained_query,
            hierarchy,
            hierarchy_seq,
        };
        let palette = cx.new(|cx| {
            self.frame_clock.observe(cx);
            CommandPalette::open(open, cx)
        });
        cx.subscribe_in(&palette, window, Self::handle_palette_event)
            .detach();
        self.palette = Some(palette.clone());
        self.palette_refresh_state =
            Some(self.current_palette_refresh_state(cx));
        palette.read(cx).focus_handle(cx).focus(window, cx);
        // Its rows copy titles, so register before any title can change.
        self.sync_title_consumer(cx);
        cx.notify();
        Ok(CommandOutcome::Completed)
    }

    /// The palette's identity rows from the projection, with each tab
    /// labelled by its custom name, the title its window published, or its
    /// fallback name, and this window's tabs most recently used first.
    fn palette_hierarchy(&self, cx: &App) -> (PaletteHierarchy, u64) {
        let desktop = cx.global::<Desktop>();
        let titles: HashMap<TabId, &str> =
            desktop.windows.published_titles().collect();
        let state = desktop.hierarchy.state();
        let hierarchy = PaletteHierarchy::from_projection(
            state,
            |tab| titles.get(&tab).copied(),
            &desktop.windows.palette_tab_order(self.window),
        );
        (hierarchy, state.seq())
    }

    /// Rebuilds an open palette's identity rows from the projection; the
    /// palette notifies only when its rows changed.
    fn refresh_palette_hierarchy(&mut self, cx: &mut Context<'_, Self>) {
        let Some(palette) = self.palette.clone() else {
            return;
        };
        let (hierarchy, seq) = self.palette_hierarchy(cx);
        let workspace = self.workspace_id(cx);
        let active = self.active_tab(cx);
        let scope = PaletteScope {
            session: workspace.and_then(|workspace| {
                cx.global::<Desktop>()
                    .hierarchy
                    .state()
                    .workspace_session(workspace)
            }),
            workspace,
            tab: active,
            terminal: self
                .tabs
                .iter()
                .find(|tab| Some(tab.id) == active)
                .map(|tab| tab.terminal),
            terminal_view: self.active_view(cx).map(|view| view.downgrade()),
        };
        palette.update(cx, |palette, cx| {
            palette.set_hierarchy(hierarchy, scope, seq, cx);
        });
    }

    fn handle_palette_event(
        &mut self,
        palette: &Entity<CommandPalette>,
        event: &PaletteEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self
            .palette
            .as_ref()
            .is_none_or(|current| current.entity_id() != palette.entity_id())
        {
            return;
        }
        match event {
            PaletteEvent::Cancel { query } => {
                let target = palette.read(cx).target.clone();
                self.palette = None;
                self.palette_refresh_state = None;
                self.retain_palette_query(query.clone());
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
                    Err(error) => {
                        self.palette = None;
                        self.report_failure(
                            "Command failed",
                            error.to_string(),
                            cx,
                        );
                    }
                }
                cx.notify();
            }
        }
    }

    fn restore_palette_focus(
        &self,
        target: &PaletteTarget,
        window: &mut Window,
        cx: &mut App,
    ) {
        if target.tab == self.active_tab(cx)
            && let Some(terminal) =
                target.terminal_view.as_ref().and_then(WeakEntity::upgrade)
            && terminal.read(cx).visible
        {
            let focus = terminal.read(cx).focus.clone();
            focus.focus(window, cx);
        } else if let Some(terminal) = self.active_view(cx) {
            let focus = terminal.read(cx).focus.clone();
            focus.focus(window, cx);
        } else {
            self.focus.focus(window, cx);
        }
    }

    fn retain_palette_query(&mut self, query: Option<String>) {
        self.retained_query = query
            .filter(|_| self.config.palette.retain_query)
            .map(|query| (query, Instant::now()));
    }

    fn dismiss_palette_for_confirmation(&mut self, cx: &mut Context<'_, Self>) {
        let Some(palette) = self.palette.take() else {
            return;
        };
        self.palette_refresh_state = None;
        self.retain_palette_query(palette.read(cx).cancellation_query());
        cx.notify();
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
                self.command_availability(spec.id, target, cx).err().map(
                    |error| {
                        let reason = match error {
                            CommandError::Unavailable(reason) => reason,
                            other => other.to_string(),
                        };
                        (spec.id, reason)
                    },
                )
            })
            .collect()
    }

    fn current_palette_refresh_state(&self, cx: &App) -> PaletteRefreshState {
        PaletteRefreshState {
            tabs: self.tabs.len(),
            active: self.active_tab(cx),
            busy: self.busy,
            confirming: self.dialog_showing(),
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
        let state = self.current_palette_refresh_state(cx);
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

    fn history_view<'a>(&'a self, cx: &'a App) -> HistoryView<'a> {
        HistoryView {
            recent: &self.recent,
            frequency: &cx.global::<Desktop>().frequency,
        }
    }

    /// The retained query when it is still within the configured window.
    fn take_retained_query(&mut self) -> Option<String> {
        let (query, cancelled) = self.retained_query.take()?;
        let limit = Duration::from_secs(u64::from(
            self.config.palette.retain_query_seconds,
        ));
        (self.config.palette.retain_query && cancelled.elapsed() <= limit)
            .then_some(query)
    }

    fn palette_colors(&self) -> OverlayColors {
        OverlayColors::from_theme(&self.config.theme)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one match routes the availability of every command scope"
    )]
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
                ids::CLOSE_TAB
                | ids::CLOSE_OTHER_TABS
                | ids::CLOSE_TABS_AFTER
                | ids::COPY_TAB_DIRECTORY
                | ids::OPEN_TAB_DIRECTORY => {
                    self.tab_command_availability(command, target.tab, cx)
                }
                ids::OPEN_CONTEXT_MENU => {
                    self.check_available(true)?;
                    self.active_view(cx).map(|_| ()).ok_or_else(|| {
                        CommandError::Unavailable(
                            "window has no active terminal".to_owned(),
                        )
                    })
                }
                ids::DIALOG_CONFIRM
                | ids::DIALOG_CANCEL
                | ids::DIALOG_FOCUS_NEXT
                | ids::DIALOG_FOCUS_PREVIOUS => {
                    if self.about.is_some() {
                        Ok(())
                    } else {
                        self.confirming_target().map(|_| ())
                    }
                }
                ids::FOCUS_NOTICES
                | ids::DISMISS_ALL_NOTICES
                | ids::NOTICE_NEXT
                | ids::NOTICE_PREVIOUS
                | ids::NOTICE_RUN_ACTION
                | ids::NOTICE_DISMISS => self.notice_availability(command),
                ids::OPEN_MENU => self.check_available(false),
                ids::MENU_SELECT_NEXT
                | ids::MENU_SELECT_PREVIOUS
                | ids::MENU_SELECT_FIRST
                | ids::MENU_SELECT_LAST
                | ids::MENU_SELECT_RIGHT
                | ids::MENU_SELECT_LEFT
                | ids::MENU_CONFIRM
                | ids::MENU_CLOSE => self.open_menu_entity().map(|_| ()),
                ids::NEXT_TAB | ids::PREVIOUS_TAB => {
                    self.check_navigation_available(cx)
                }
                ids::SELECT_TAB => self.check_navigation_available(cx),
                ids::SELECT_RECENT_TAB => {
                    self.check_navigation_available(cx)?;
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
                ids::CLOSE_WINDOW => self.check_close_window_available(),
                ids::ABOUT | ids::OPEN_SETTINGS => Ok(()),
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
                            "window has no session".into(),
                        ))
                    }
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

    /// Availability of the commands that take an optional `tab` target:
    /// the named tab must exist, or the window must have an active one.
    fn tab_command_availability(
        &self,
        command: huterm_protocol::CommandId,
        tab: Option<TabId>,
        cx: &App,
    ) -> Result<(), CommandError> {
        if command != ids::COPY_TAB_DIRECTORY
            && command != ids::OPEN_TAB_DIRECTORY
        {
            self.check_tab_close_available()?;
        }
        let tab = tab.map_or_else(
            || self.active_tab_id(cx),
            |tab| self.existing_tab(tab),
        )?;
        match command {
            ids::CLOSE_TAB => Ok(()),
            ids::COPY_TAB_DIRECTORY => self.tab_directory(tab, cx).map(|_| ()),
            ids::OPEN_TAB_DIRECTORY => {
                self.local_tab_directory(tab, cx).map(|_| ())
            }
            other => self.tab_set_target(other, tab).map(|_| ()),
        }
    }

    fn reload_palette(&mut self, cx: &mut Context<'_, Self>) {
        self.palette_refresh_state = None;
        self.refresh_palette(cx);
        if let Some(palette) = self.palette.clone() {
            let colors = self.palette_colors();
            let profiles = quake_profile_rows(cx);
            palette.update(cx, |palette, cx| {
                palette.set_presentation(
                    colors,
                    self.config.palette.placement,
                    profiles,
                    cx,
                );
            });
        }
        cx.notify();
    }

    /// Key context for binding predicates: `Workspace`, plus `confirming`,
    /// `reordering`, and `fullscreen` while those states hold. The About
    /// panel sets `confirming` too, so the `dialog_*` bindings close it.
    fn key_context(&self, _window: &Window) -> KeyContext {
        let mut context = KeyContext::default();
        context.add("Workspace");
        if self.dialog_showing() {
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
        } else if self.about.is_some() {
            "About panel is showing"
        } else if reordering && self.reorder.is_some() {
            "tab reorder in progress"
        } else {
            return Ok(());
        };
        Err(CommandError::Unavailable(reason.to_owned()))
    }

    /// A modal panel that takes the `confirming` context and blocks
    /// terminal input: a close confirmation or the About panel.
    fn dialog_showing(&self) -> bool {
        self.close.confirmation.is_some() || self.about.is_some()
    }

    fn active_tab_id(&self, cx: &App) -> Result<TabId, CommandError> {
        self.active_tab(cx).ok_or_else(|| {
            CommandError::Unavailable("window has no tab".to_owned())
        })
    }

    /// The directory `copy_tab_directory` copies for `tab`, refused while
    /// the tab has reported none.
    fn tab_directory(
        &self,
        tab: TabId,
        cx: &App,
    ) -> Result<String, CommandError> {
        let tab = self
            .tabs
            .iter()
            .find(|record| record.id == tab)
            .ok_or(CommandError::StaleTarget)?;
        tab_directory_path(&tab.view.read(cx).metadata).ok_or_else(|| {
            CommandError::Unavailable("directory unknown".to_owned())
        })
    }

    /// The tab's working directory when it is on this machine, so the file
    /// manager can open it.
    fn local_tab_directory(
        &self,
        tab: TabId,
        cx: &App,
    ) -> Result<PathBuf, CommandError> {
        let tab = self
            .tabs
            .iter()
            .find(|record| record.id == tab)
            .ok_or(CommandError::StaleTarget)?;
        match tab.view.read(cx).metadata.directory() {
            Some(directory) if directory.is_local() => {
                Ok(PathBuf::from(directory.path()))
            }
            Some(_) => Err(CommandError::Unavailable(
                "directory is on another host".to_owned(),
            )),
            None => {
                Err(CommandError::Unavailable("directory unknown".to_owned()))
            }
        }
    }

    /// Opens the named or active tab's working directory in the file
    /// manager.
    fn open_tab_directory(
        &self,
        invocation: &CommandInvocation,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        let tab = self.target_tab(invocation, cx)?;
        let path = self.local_tab_directory(tab, cx)?;
        cx.open_with_system(&path);
        Ok(CommandOutcome::Completed)
    }

    /// Opens the active terminal's context menu at its cursor.
    fn open_context_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.check_available(true)?;
        let terminal = self.active_view(cx).ok_or_else(|| {
            CommandError::Unavailable(
                "window has no active terminal".to_owned(),
            )
        })?;
        let anchor = terminal.read(cx).context_menu_anchor(window);
        self.open_terminal_menu(anchor, None, window, cx);
        if let Some(menu) = &self.menu {
            menu.view.update(cx, MenuView::select_first);
        }
        Ok(CommandOutcome::Completed)
    }

    /// Copies the named or active tab's working directory through the
    /// application clipboard write path.
    fn copy_tab_directory(
        &self,
        invocation: &CommandInvocation,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        let tab = self.target_tab(invocation, cx)?;
        let path = self.tab_directory(tab, cx)?;
        cx.write_to_clipboard(ClipboardItem::new_string(path));
        Ok(CommandOutcome::Completed)
    }

    fn existing_tab(&self, tab: TabId) -> Result<TabId, CommandError> {
        if self.tabs.iter().any(|record| record.id == tab) {
            Ok(tab)
        } else {
            Err(CommandError::StaleTarget)
        }
    }

    /// The tab a tab-targeted command names, or the active tab.
    fn target_tab(
        &self,
        invocation: &CommandInvocation,
        cx: &App,
    ) -> Result<TabId, CommandError> {
        match invocation.tab("tab") {
            Some(tab) => self.existing_tab(tab),
            None => self.active_tab_id(cx),
        }
    }

    /// The tabs `close_other_tabs` or `close_tabs_after` closes relative to
    /// `tab`, refused when there are none.
    fn tab_set_target(
        &self,
        command: huterm_protocol::CommandId,
        tab: TabId,
    ) -> Result<CloseTarget, CommandError> {
        let order: Vec<TabId> = self.tabs.iter().map(|tab| tab.id).collect();
        let (tabs, reason) = match command {
            ids::CLOSE_OTHER_TABS => {
                (other_tabs(&order, tab), "window has one tab")
            }
            ids::CLOSE_TABS_AFTER => {
                (tabs_after(&order, tab), "no tabs after this one")
            }
            other => return Err(CommandError::UnknownCommand(other)),
        };
        tabs_target(tabs)
            .ok_or_else(|| CommandError::Unavailable(reason.to_owned()))
    }

    /// Tab closes are refused while a confirmation or the About panel is
    /// showing. Unlike [`Self::check_available`], a structural operation in
    /// progress does not refuse them: the close queues behind it.
    fn check_tab_close_available(&self) -> Result<(), CommandError> {
        tab_close_availability(&self.close, self.about.is_some())
    }

    /// `close_window` is refused only while its own confirmation shows; it
    /// widens a tab confirmation and queues behind structural work.
    fn check_close_window_available(&self) -> Result<(), CommandError> {
        self.close.check_close_available(&CloseTarget::Window)
    }

    /// Closes the named or active tab, or the tabs around it, unless a
    /// confirmation or the About panel is already showing.
    fn run_tab_close(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.check_tab_close_available()?;
        let tab = self.target_tab(invocation, cx)?;
        let target = if invocation.id == ids::CLOSE_TAB {
            CloseTarget::Tab(tab)
        } else {
            self.tab_set_target(invocation.id, tab)?
        };
        self.request_close(target, window, cx);
        Ok(CommandOutcome::Accepted)
    }

    /// Runs a `dialog_*` command against the showing confirmation. Confirm
    /// presses the focused button, so Enter on Cancel cancels. The About
    /// panel shares these bindings: Enter and Escape close it, and its
    /// buttons are not keyboard-focusable.
    fn run_dialog_command(
        &mut self,
        command: huterm_protocol::CommandId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        if self.about.is_some() {
            match command {
                ids::DIALOG_CONFIRM | ids::DIALOG_CANCEL => {
                    self.close_about(window, cx);
                }
                ids::DIALOG_FOCUS_NEXT | ids::DIALOG_FOCUS_PREVIOUS => {}
                other => return Err(CommandError::UnknownCommand(other)),
            }
            return Ok(CommandOutcome::Completed);
        }
        let target = self.confirming_target()?;
        match (command, self.close.dialog_focus) {
            (ids::DIALOG_CONFIRM, DialogFocus::Primary) => {
                self.finish_close(target, window, cx);
            }
            (ids::DIALOG_CONFIRM | ids::DIALOG_CANCEL, _) => {
                self.cancel_confirmation(window, cx);
            }
            (ids::DIALOG_FOCUS_NEXT | ids::DIALOG_FOCUS_PREVIOUS, focus) => {
                self.close.dialog_focus = focus.toggled();
                cx.notify();
            }
            (other, _) => return Err(CommandError::UnknownCommand(other)),
        }
        Ok(CommandOutcome::Completed)
    }

    fn confirming_target(&self) -> Result<CloseTarget, CommandError> {
        self.close.confirmation.clone().ok_or_else(|| {
            CommandError::Unavailable(
                "no close confirmation is showing".to_owned(),
            )
        })
    }

    fn check_navigation_available(&self, cx: &App) -> Result<(), CommandError> {
        self.check_available(true)?;
        self.active_tab_id(cx).map(|_| ())
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
    fn run_fullscreen_toggle(
        &mut self,
        command: huterm_protocol::CommandId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        if self.quake.is_some() {
            self.check_fullscreen_available(command)?;
            return quake_windows::toggle(self, window, cx);
        }
        self.observe_fullscreen(window, cx, true);
        self.fullscreen_work.wake.signal();
        let intent = match command {
            ids::TOGGLE_NATIVE_FULLSCREEN => ToggleIntent::Native,
            ids::TOGGLE_NON_NATIVE_FULLSCREEN => ToggleIntent::NonNative,
            _ => ToggleIntent::Default,
        };
        self.check_fullscreen_available(command)?;
        self.fullscreen
            .toggle_checked(intent, || Ok(()))
            .map_err(CommandError::Unavailable)?;
        if self.advance_fullscreen(window, cx) {
            self.fullscreen_work.wake.signal();
        }
        self.arm_fullscreen(cx);
        Ok(CommandOutcome::Accepted)
    }

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
        if let Some(tab) = self.active_view(cx) {
            tab.update(cx, |tab, _| tab.clear_option_composition());
        }
        // A shortcut pressed while the menu is open runs its command in
        // place of the menu.
        if !is_menu_command(invocation.id) {
            self.close_menu(MenuFocusReturn::Terminal, window, cx);
        }
        match invocation.id {
            ids::NEW_TAB => self.new_tab(window, cx),
            ids::OPEN_MENU => self.open_menu(true, window, cx),
            ids::MENU_SELECT_NEXT
            | ids::MENU_SELECT_PREVIOUS
            | ids::MENU_SELECT_FIRST
            | ids::MENU_SELECT_LAST
            | ids::MENU_SELECT_RIGHT
            | ids::MENU_SELECT_LEFT
            | ids::MENU_CONFIRM
            | ids::MENU_CLOSE => self.run_menu_command(invocation.id, cx),
            ids::CLOSE_TAB | ids::CLOSE_OTHER_TABS | ids::CLOSE_TABS_AFTER => {
                self.run_tab_close(invocation, window, cx)
            }
            ids::COPY_TAB_DIRECTORY => self.copy_tab_directory(invocation, cx),
            ids::OPEN_TAB_DIRECTORY => self.open_tab_directory(invocation, cx),
            ids::OPEN_CONTEXT_MENU => self.open_context_menu(window, cx),
            ids::DIALOG_CONFIRM
            | ids::DIALOG_CANCEL
            | ids::DIALOG_FOCUS_NEXT
            | ids::DIALOG_FOCUS_PREVIOUS => {
                self.run_dialog_command(invocation.id, window, cx)
            }
            ids::FOCUS_NOTICES
            | ids::DISMISS_ALL_NOTICES
            | ids::NOTICE_NEXT
            | ids::NOTICE_PREVIOUS
            | ids::NOTICE_RUN_ACTION
            | ids::NOTICE_DISMISS => {
                self.run_notice_command(invocation.id, window, cx)
            }
            ids::CLOSE_WINDOW => {
                self.check_close_window_available()?;
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
                self.run_fullscreen_toggle(invocation.id, window, cx)
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
            ids::ABOUT => self.show_about(window, cx),
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
                    self.active_tab(cx),
                    self.workspace_id(cx),
                )?;
                Ok(run_on_runtime(invocation, cx))
            }
            other => Err(CommandError::UnknownCommand(other)),
        }
    }
}

impl WorkspaceView {
    /// The showing close dialog's quoted title and its busy groups' tab
    /// headings, for smoke state.
    fn dialog_smoke_fields(&self, cx: &App) -> (String, String) {
        let Some(target) = &self.close.confirmation else {
            return ("none".to_owned(), String::new());
        };
        let input = self.close_dialog_input(target, cx);
        (
            format!("{:?}", build_close_dialog(&input).title),
            dialog_group_titles(&input),
        )
    }

    /// The busy terminals of a pending confirmation, mapped to the tab titles
    /// the window model publishes. Quit covers every open window's tabs.
    fn close_dialog_input(
        &self,
        target: &CloseTarget,
        cx: &App,
    ) -> CloseDialogInput {
        let scope = if matches!(target, CloseTarget::Application) {
            TitleScope::Open
        } else {
            TitleScope::Window(self.window)
        };
        let titles: Vec<TabTitle> = cx
            .global::<Desktop>()
            .windows
            .tab_titles(scope)
            .into_iter()
            .map(|entry| TabTitle {
                tab: entry.id,
                terminal: entry.terminal,
                title: entry.title.clone(),
            })
            .collect();
        let jobs = self
            .close
            .assessment
            .iter()
            .flat_map(CloseAssessment::terminal_jobs);
        close_dialog_input(target, jobs, &titles)
    }
}

/// The busy groups' tab headings, `;`-separated, for smoke state.
fn dialog_group_titles(input: &CloseDialogInput) -> String {
    input
        .groups
        .iter()
        .filter_map(|group| group.tab_title.clone())
        .collect::<Vec<_>>()
        .join(";")
}

/// A tab's display title with the identities the close evidence uses.
#[derive(Clone, Debug)]
struct TabTitle {
    tab: TabId,
    terminal: TerminalId,
    title: String,
}

/// Tab closes are refused while a confirmation is showing, or while the
/// About panel is up and would otherwise stay over a closing tab.
fn tab_close_availability(
    close: &CloseState,
    about_showing: bool,
) -> Result<(), CommandError> {
    close.check_tab_close_available()?;
    if about_showing {
        return Err(CommandError::Unavailable(
            "About panel is showing".to_owned(),
        ));
    }
    Ok(())
}

/// Maps assessed job evidence onto the dialog's busy groups. `titles` lists
/// the tabs the dialog may cover, in window order; idle terminals are
/// omitted, and headings are set when the dialog covers more than one tab:
/// a tab set, the application, or a window with more than one tab.
fn close_dialog_input<'a>(
    target: &CloseTarget,
    jobs: impl IntoIterator<Item = (TerminalId, &'a huterm_core::JobState)>,
    titles: &[TabTitle],
) -> CloseDialogInput {
    let dialog_target = match target {
        CloseTarget::Tab(id) => CloseDialogTarget::Tab {
            title: titles
                .iter()
                .find(|entry| entry.tab == *id)
                .map(|entry| entry.title.clone())
                .unwrap_or_default(),
        },
        CloseTarget::Tabs(ids) => CloseDialogTarget::Tabs { count: ids.len() },
        CloseTarget::Window => CloseDialogTarget::Window,
        CloseTarget::Application => CloseDialogTarget::Application,
    };
    let headings = match dialog_target {
        CloseDialogTarget::Tab { .. } => false,
        CloseDialogTarget::Window => titles.len() > 1,
        CloseDialogTarget::Tabs { .. } | CloseDialogTarget::Application => true,
    };
    let groups = jobs
        .into_iter()
        .filter_map(|(terminal, state)| {
            let state = match state {
                huterm_core::JobState::Idle => return None,
                huterm_core::JobState::Unknown => ProcessGroupState::Unknown,
                huterm_core::JobState::Running(processes) => {
                    ProcessGroupState::Known(
                        processes
                            .iter()
                            .map(|process| ProcessRow {
                                command: process.command.clone(),
                                pid: process.pid,
                                foreground: process.foreground,
                                command_line: process.command_line.clone(),
                            })
                            .collect(),
                    )
                }
            };
            let tab_title = headings.then(|| {
                titles
                    .iter()
                    .find(|entry| entry.terminal == terminal)
                    .map_or_else(
                        || "Detached terminal".to_owned(),
                        |entry| entry.title.clone(),
                    )
            });
            Some(ProcessGroup { tab_title, state })
        })
        .collect();
    CloseDialogInput {
        target: dialog_target,
        groups,
    }
}

impl WorkspaceView {
    /// Shows the About panel, or refocuses it when it is already showing.
    /// It takes the root focus so the `confirming` bindings close it, and
    /// blocks terminal input while it is up.
    fn show_about(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        if self.about.is_some() {
            self.focus.focus(window, cx);
            cx.notify();
            return Ok(CommandOutcome::Completed);
        }
        self.check_available(false)?;
        self.cancel_reorder(window, cx);
        self.resizing_sidebar = false;
        if let Some(terminal) = self.active_view(cx) {
            terminal.update(cx, TerminalView::clear_composition);
        }
        let facts = BuildFacts::current(display_backend(window));
        self.about = Some(about_details(&facts));
        self.focus.focus(window, cx);
        cx.notify();
        Ok(CommandOutcome::Completed)
    }

    /// Closes the About panel and returns focus to the terminal.
    fn close_about(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.about.take().is_none() {
            return;
        }
        self.focus_terminal(window, cx);
        cx.notify();
    }

    /// Copies the About panel's details as plain text.
    fn copy_about_details(&self, cx: &mut Context<'_, Self>) {
        if let Some(details) = &self.about {
            cx.write_to_clipboard(ClipboardItem::new_string(details.text()));
        }
    }

    /// Sets the native window title from the active tab, only when it
    /// changes, so tab switches and title changes reach window managers and
    /// switchers without resetting an unchanged title every frame.
    fn sync_window_title(&mut self, window: &mut Window, cx: &App) {
        let active_tab = self.active_tab(cx);
        let active = self
            .tabs
            .iter()
            .find(|tab| Some(tab.id) == active_tab)
            .map(|tab| tab.title(self.config.tabs, cx));
        let title = window_title(active.as_deref());
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title;
        }
    }
}

fn usable_launch_directory(directory: &Path) -> bool {
    std::fs::metadata(directory).is_ok_and(|metadata| metadata.is_dir())
        && nix::unistd::access(directory, nix::unistd::AccessFlags::X_OK)
            .is_ok()
}

fn inherited_directory(
    policy: huterm_config::NewTabDirectory,
    metadata: &huterm_protocol::TerminalMetadata,
) -> Option<PathBuf> {
    if policy != huterm_config::NewTabDirectory::Inherit {
        return None;
    }
    metadata
        .directory()
        .filter(|directory| directory.is_local())
        .map(|directory| PathBuf::from(directory.path()))
}

/// An expiring notice for a terminal failure, keyed by its tab and naming
/// the tab in its location line.
fn terminal_notice(
    tab: TabId,
    title: &str,
    failure: TerminalFailure,
) -> NoticeContent {
    NoticeContent {
        severity: failure.severity,
        source: NoticeSource::Terminal {
            tab,
            title: title.to_owned(),
        },
        title: failure.title.to_owned(),
        message: failure.message,
        location: Some(title.to_owned()),
        actions: Vec::new(),
        lifetime: Lifetime::Expiring,
    }
}

/// Announces a kept tab's root exit. Exit is not a failure, so it is
/// informational and expires like other terminal notices.
fn exit_notice(tab: TabId, title: &str, code: Option<u32>) -> NoticeContent {
    NoticeContent {
        severity: Severity::Info,
        source: NoticeSource::Terminal {
            tab,
            title: title.to_owned(),
        },
        title: "Process exited".to_owned(),
        message: code.map_or_else(
            || "Process exited".into(),
            |code| format!("Process exited with status {code}"),
        ),
        location: Some(title.to_owned()),
        actions: Vec::new(),
        lifetime: Lifetime::Expiring,
    }
}

/// Executes a filled runtime command on the structural worker and reports a
/// later failure as a notice offering to run the same invocation again.
fn run_on_runtime(
    invocation: CommandInvocation,
    cx: &mut Context<'_, WorkspaceView>,
) -> CommandOutcome {
    let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
    let retry = invocation.clone();
    let task = cx
        .background_executor()
        .spawn(async move { runtime.execute(&invocation) });
    cx.spawn(async move |view, cx| {
        let Committed { result, seq } = task.await;
        // Reconcile publishes renamed titles while draining; the completion
        // reports failure only after the projection holds its result.
        let resolution = projected(cx, seq).await;
        if let Err(error) = result
            && (resolution == Resolution::Ready
                || !cx.update(|cx| terminating(cx)))
        {
            let _ = view.update(cx, |view, cx| {
                view.notify(
                    NoticeContent::command_failure(
                        "Command failed",
                        format!("Command failed: {error}"),
                    )
                    .action("Try Again", retry),
                    cx,
                );
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

    fn arm_fullscreen(&self, cx: &App) {
        if self.fullscreen.is_closed() {
            self.fullscreen_work.wake.stop();
            return;
        }
        let mut deadline = self.fullscreen.deadline();
        #[cfg(target_os = "macos")]
        if let Some(adapter) = &self.native_fullscreen {
            deadline = deadline.into_iter().chain(adapter.deadline()).min();
        }
        if self.quake.is_some() {
            deadline = None;
        } else if self.fullscreen_work.fallback {
            deadline = deadline
                .into_iter()
                .chain(Some(Instant::now() + Duration::from_millis(16)))
                .min();
        }
        self.fullscreen_work.wake.arm(deadline, cx);
    }

    fn refresh_fullscreen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if self.quake.is_some() {
            #[cfg(target_os = "macos")]
            if let Some(adapter) = &self.native_fullscreen {
                adapter.discard_quake_events();
            }
            return false;
        }
        self.observe_fullscreen(window, cx, false);
        #[cfg(target_os = "macos")]
        if self
            .native_fullscreen
            .as_ref()
            .is_some_and(crate::native_fullscreen::Adapter::has_events)
        {
            return true;
        }
        self.advance_fullscreen(window, cx)
    }

    // Commands must reconcile queued native changes before choosing a target.
    // Observation cannot dispatch effects for the preceding desired state.
    fn observe_fullscreen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
        command: bool,
    ) {
        #[cfg(not(target_os = "macos"))]
        let _ = command;
        let previous = (
            self.fullscreen.chrome_hidden,
            self.fullscreen.observed,
            self.fullscreen_insets,
            self.notch_shelves,
        );
        let now = Instant::now();
        #[cfg(target_os = "macos")]
        if let Some(adapter) = &self.native_fullscreen {
            for event in adapter.drain(command) {
                use crate::native_fullscreen::Event;
                match event {
                    Event::Native(event) => {
                        self.fullscreen.native_event(event, now);
                    }
                    Event::NativeExitFailed(error) => {
                        self.fullscreen.recover();
                        self.fullscreen_failed(&error, cx);
                    }
                    Event::State(recovery, chrome) => {
                        self.fullscreen.non_native_state(recovery, chrome);
                    }
                    Event::Complete(generation, recovery) => {
                        self.fullscreen.complete(generation, recovery);
                    }
                    Event::Failed(generation, error) => {
                        if self.fullscreen.fail(generation) {
                            self.fullscreen_failed(&error, cx);
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
            self.report_failure(
                "Fullscreen",
                "Fullscreen transition timed out",
                cx,
            );
            eprintln!("Fullscreen transition timed out");
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
            self.notch_shelves = self
                .native_fullscreen
                .as_ref()
                .filter(|_| self.fullscreen.chrome_hidden)
                .and_then(crate::native_fullscreen::Adapter::notch_shelves);
        }
        self.publish_restorable_bounds(cx);
        if previous
            != (
                self.fullscreen.chrome_hidden,
                self.fullscreen.observed,
                self.fullscreen_insets,
                self.notch_shelves,
            )
        {
            // Leaving fullscreen can restore a Linux client frame. Apply it
            // before the terminals resize, so no PTY is sized for the
            // windowed chrome inside the fullscreen frame.
            self.sync_frame(window);
            let window_frame = self.window_frame();
            let notch_shelf = self.notch_shelf();
            let quake_presenting = self.quake_presenting();
            for tab in &self.tabs {
                tab.view.update(cx, |terminal, cx| {
                    terminal.quiet_resize = quake_presenting;
                    terminal.chrome_hidden = self.fullscreen.chrome_hidden;
                    terminal.fullscreen_insets = self.fullscreen_insets;
                    terminal.notch_shelf = notch_shelf;
                    terminal.window_frame = window_frame;
                    terminal.resize_if_needed(window);
                    cx.notify();
                });
            }
            cx.notify();
        }
    }

    #[cfg(target_os = "macos")]
    fn fullscreen_failed(
        &mut self,
        error: &dyn std::fmt::Display,
        cx: &mut Context<'_, Self>,
    ) {
        self.report_failure(
            "Fullscreen",
            format!("Fullscreen failed: {error}"),
            cx,
        );
    }

    fn advance_fullscreen(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        #[cfg(not(target_os = "macos"))]
        let _ = cx;
        let operation = match self.fullscreen.schedule(Instant::now()) {
            crate::fullscreen::Next::Operation(operation) => operation,
            crate::fullscreen::Next::ContinueLater => return true,
            crate::fullscreen::Next::Idle => return false,
        };
        {
            match operation.effect {
                Effect::ToggleNative => {
                    #[cfg(target_os = "macos")]
                    if let Some(adapter) = &self.native_fullscreen
                        && let Err(error) = adapter.check_native_transition()
                    {
                        self.fullscreen.fail(operation.generation);
                        self.fullscreen_failed(&error, cx);
                        self.fullscreen_work.wake.signal();
                        return false;
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
                        self.report_failure(
                            "Fullscreen",
                            "Fullscreen native adapter is unavailable",
                            cx,
                        );
                    }
                }
            }
        }
        false
    }

    fn remove_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
        quit_after: bool,
    ) {
        quake_windows::close(self, cx);
        cx.global_mut::<Desktop>().windows.begin_close(self.window);
        self.fullscreen_work.wake.stop();
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
        if !cx.global_mut::<Desktop>().windows.select(self.window, id) {
            return;
        }
        self.sync_tab_layout(window, cx);
        self.reveal_active(window, cx);
        let visible = self.quake_visible();
        for tab in &self.tabs {
            tab.view.update(cx, |view, cx| {
                let was_visible = view.visible;
                view.visible = visible && tab.id == id;
                if view.visible {
                    view.bell.viewed(window.is_window_active());
                    view.resize_if_needed(window);
                    view.focus.focus(window, cx);
                    view.start_initial_snapshot(cx);
                } else {
                    view.hide(cx);
                }
                if was_visible != view.visible {
                    cx.notify();
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
        let changed = self.active_tab(cx) != Some(id);
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
        self.check_navigation_available(cx)?;
        let next = cx.global::<Desktop>().windows.recent_tab(self.window);
        if let Some(next) = next {
            self.select_from_command(next, window, cx);
        }
        Ok(CommandOutcome::Completed)
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
        self.check_navigation_available(cx)?;
        let active = self.active_tab(cx);
        let index = self
            .tabs
            .iter()
            .position(|tab| Some(tab.id) == active)
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

    fn request_close(
        &mut self,
        target: CloseTarget,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.cancel_reorder(window, cx);
        self.resizing_sidebar = false;
        let Some(target) = self.current_close_target(target) else {
            return;
        };
        if matches!(target, CloseTarget::Application) {
            cx.global_mut::<Desktop>().quitting = true;
        }
        if self.busy {
            self.close.queue(target);
            return;
        }
        let target = self.close.begin_check(target);
        let request = match &target {
            CloseTarget::Application => CloseRequest::Application,
            CloseTarget::Window => {
                let Some(attachment) = self.attachment_id(cx) else {
                    self.remove_window(window, cx, false);
                    return;
                };
                CloseRequest::Window(attachment)
            }
            CloseTarget::Tab(tab) => {
                let Some(workspace) = self.workspace_id(cx) else {
                    return;
                };
                CloseRequest::Tab {
                    workspace,
                    tab: *tab,
                }
            }
            CloseTarget::Tabs(tabs) => {
                let Some(workspace) = self.workspace_id(cx) else {
                    return;
                };
                CloseRequest::Tabs {
                    workspace,
                    tabs: tabs.clone(),
                }
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
                view.reconcile_own(window, cx);
                let assessment = match result {
                    Ok(assessment) => assessment,
                    Err(error) => {
                        view.report_failure(
                            "Close",
                            format!("Cannot assess close: {error}"),
                            cx,
                        );
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
                        view.dismiss_palette_for_confirmation(cx);
                        view.close_menu(MenuFocusReturn::Keep, window, cx);
                        view.about = None;
                        view.focus.focus(window, cx);
                        cx.notify();
                    }
                    Some(CloseDecision::Close(target)) => {
                        view.finish_close(target, window, cx);
                    }
                    None => {}
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Restricts tab targets to current tabs, in window order; `None` when
    /// nothing remains to close.
    fn current_close_target(&self, target: CloseTarget) -> Option<CloseTarget> {
        match target {
            CloseTarget::Tab(id) => self
                .tabs
                .iter()
                .any(|tab| tab.id == id)
                .then_some(CloseTarget::Tab(id)),
            CloseTarget::Tabs(ids) => tabs_target(
                self.tabs
                    .iter()
                    .map(|tab| tab.id)
                    .filter(|id| ids.contains(id))
                    .collect(),
            ),
            target => Some(target),
        }
    }

    /// Cancels the showing confirmation from its Cancel button or Escape.
    /// A stale press, dispatched against a frame drawn before Confirm began
    /// the commit, finds no confirmation and must not cancel the commit.
    fn cancel_confirmation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.close.can_cancel_confirmation() {
            self.cancel_close(window, cx);
        }
    }

    fn cancel_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.busy = false;
        self.reconcile_own(window, cx);
        if matches!(self.close.cancel(), Some(CloseTarget::Application)) {
            #[cfg(target_os = "macos")]
            native_quit::cancel_request();
            cx.global_mut::<Desktop>().quitting = false;
        }
        if let Some(tab) = self.active_view(cx) {
            let focus = tab.read(cx).focus.clone();
            focus.focus(window, cx);
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
        let Some((assessment, confirmed)) = self.close.begin_commit(&target)
        else {
            self.request_close(target, window, cx);
            return;
        };
        self.busy = true;
        let generation = self.close.generation;
        self.publish_restorable_bounds(cx);
        let windows = if matches!(target, CloseTarget::Application) {
            cx.global_mut::<Desktop>().quitting = true;
            Some(capture_windows(cx))
        } else {
            None
        };
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let task = cx.background_executor().spawn(async move {
            runtime.commit(&assessment, confirmed, windows)
        });
        let app = cx.to_async();
        cx.spawn_in(window, async move |view, cx| {
            let Committed { result, seq } = task.await;
            // Application close commits teardown itself, which cancels every
            // waiter, so it keeps today's flow without waiting.
            let resolution = if matches!(target, CloseTarget::Application) {
                Resolution::Ready
            } else {
                projected(&app, seq).await
            };
            let _ = view.update_in(cx, |view, window, cx| {
                if view.close.generation != generation {
                    return;
                }
                view.busy = false;
                view.close.current = None;
                if close_commit_retries(&result) {
                    view.reconcile_own(window, cx);
                    view.request_close(target, window, cx);
                    return;
                }
                if let Err(error) = result {
                    let message = format!("Close failed: {error}");
                    eprintln!("{message}");
                    view.report_failure("Close", message, cx);
                }
                if resolution == Resolution::Cancelled {
                    // Teardown owns removal; publish nothing.
                    view.reconcile_own(window, cx);
                    view.resume_close(window, cx);
                    cx.notify();
                    return;
                }
                match target {
                    CloseTarget::Application => {
                        settle_waiters(cx);
                        approved_quit(cx);
                    }
                    CloseTarget::Window => {
                        let quit_after = matches!(
                            view.close.pending,
                            Some(CloseTarget::Application)
                        );
                        // The runtime detached the attachment or deleted its
                        // session, even when terminal teardown then failed.
                        cx.global_mut::<Desktop>()
                            .windows
                            .detach_attachment(view.window);
                        view.remove_window(window, cx, quit_after);
                    }
                    CloseTarget::Tab(id) => {
                        view.remove_closed_tabs(&[id], window, cx);
                    }
                    CloseTarget::Tabs(ids) => {
                        view.remove_closed_tabs(&ids, window, cx);
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Drops the records of committed tab closes, then resumes queued
    /// closes or closes the emptied window.
    fn remove_closed_tabs(
        &mut self,
        ids: &[TabId],
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.drop_tab_views(ids, cx);
        // Removals deferred while this window was busy apply now, before
        // the new active tab takes focus below.
        self.reconcile(&Touched::default(), terminating(cx), cx);
        // Fit widths are index-based; refresh them before the reveal below
        // reads them.
        self.measure_tab_widths(window, cx);
        if let Some(active) = self.active_tab(cx) {
            self.select(active, window, cx);
            self.reveal_tab_activity(window, cx);
        }
        if self.resume_close(window, cx) {
            return;
        }
        if self.tabs.is_empty() {
            self.request_close(CloseTarget::Window, window, cx);
        }
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
    let reload_path = path.clone();
    let task = cx
        .background_executor()
        .spawn(async move { config::reload(&reload_path) });
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
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
            let result = result.map(|(config, family, metrics, compiled)| {
                #[cfg(all(target_os = "macos", feature = "macos-updater"))]
                apply_update_config(cx, &config);
                let desktop = cx.global_mut::<Desktop>();
                desktop.config = config.clone();
                desktop.diagnostics =
                    reload_diagnostics(&path, &config, &compiled);
                desktop.latched.clear();
                quake_windows::reconcile(cx);
                let keymap = bind_keymap(cx, compiled);
                cx.global_mut::<Desktop>().keymap = keymap;
                maybe_exit(cx);
                (config, family, metrics)
            });
            report_config_reload_error(&result);
            let diagnostics = match &result {
                Ok(_) => cx.global::<Desktop>().diagnostics.clone(),
                Err(error) => failed_reload_diagnostics(
                    error,
                    &cx.global::<Desktop>().diagnostics,
                ),
            };
            for handle in cx.windows() {
                let _ = handle.update(cx, |root, window, cx| {
                    if let Ok(view) = root.downcast::<WorkspaceView>() {
                        view.update(cx, |view, cx| {
                            view.apply_reload(
                                result.as_ref().ok(),
                                &diagnostics,
                                window,
                                cx,
                            );
                        });
                    }
                });
            }
            palette_smoke::record_reload_titles(cx);
        });
    })
    .detach();
    Ok(CommandOutcome::Accepted)
}

impl WorkspaceView {
    /// Applies a reload's outcome: a new configuration when it loaded, and
    /// the replacement configuration notices either way.
    fn apply_reload(
        &mut self,
        loaded: Option<&(Config, String, GridMetrics)>,
        diagnostics: &[NoticeContent],
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(tab) = self.active_view(cx) {
            tab.update(cx, |tab, _| tab.clear_option_composition());
        }
        let scroll_key = scroll_to_bottom_key(&cx.global::<Desktop>().keymap);
        if let Some((config, family, metrics)) = loaded {
            self.close_menu(MenuFocusReturn::Terminal, window, cx);
            self.resizing_sidebar = false;
            self.scroll_target = None;
            // A position change to or from `titlebar` changes who draws
            // the Linux title bar. GPUI applies the request at once, and
            // its appearance callback cannot reach this view while it is
            // updating, so the frame is sampled here.
            let quake = self.quake.is_some();
            let previous = requested_decorations(
                self.config.tabs.position,
                Platform::current(),
                quake,
            );
            let requested = requested_decorations(
                config.tabs.position,
                Platform::current(),
                quake,
            );
            if previous != requested {
                window.request_decorations(requested);
            }
            self.config = config.clone();
            self.sync_frame(window);
            self.layout_pending = true;
            self.title_widths.clear();
            self.fullscreen_work.wake.signal();
            self.fullscreen
                .set_default(config.window.macos_fullscreen_mode);
            self.family.clone_from(family);
            self.metrics = *metrics;
            let tabs_config = self.layout_tabs();
            let window_frame = self.window_frame();
            for tab in &self.tabs {
                tab.view.update(cx, |view, cx| {
                    let metrics = metrics.at_scale(view.metrics.scale_factor);
                    view.renderer.borrow_mut().reconfigure(
                        family.clone(),
                        config.theme.clone(),
                        metrics,
                    );
                    view.font_family.clone_from(family);
                    view.font_size = metrics.font_size;
                    view.metrics = metrics;
                    view.window_config = config.window;
                    view.tabs_config = tabs_config;
                    view.window_frame = window_frame;
                    view.reload_terminal_config(config.terminal, cx);
                    view.publish_presentation(&config.theme);
                    view.theme = config.theme.clone();
                    view.set_scroll_to_bottom_key(scroll_key.clone(), cx);
                    cx.notify();
                });
            }
            self.publish_tab_titles(cx);
        }
        self.replace_diagnostics(diagnostics, window, cx);
        self.reload_palette(cx);
    }
}

fn report_config_reload_error<T>(result: &Result<T, String>) {
    if let Err(error) = result {
        eprintln!("Config reload failed: {error}");
    }
}

fn path_for_status(cx: &App) -> String {
    cx.global::<Desktop>().config_path.display().to_string()
}

/// The shelf a top bar occupies for `tabs`, if the config asks for one and
/// the display offers one tall enough for the bar. Only top bars use
/// shelves; a shorter shelf is not a shelf at all, so presentation, painting,
/// and layout agree on the bar's normal place below the safe area.
fn select_notch_shelf(
    tabs: TabsConfig,
    shelves: Option<crate::fullscreen::NotchShelves>,
) -> Option<Bounds<Pixels>> {
    if tabs.position != TabPosition::Top {
        return None;
    }
    let shelves = shelves?;
    let shelf = match tabs.notch {
        huterm_config::TabNotch::Off => return None,
        huterm_config::TabNotch::Left => shelves.left,
        huterm_config::TabNotch::Right => shelves.right,
    };
    (shelf.size.height >= tab_bar_height(tabs)).then_some(shelf)
}

/// Largest corner radius the rounded terminal corner may use, so wide padding
/// does not produce an oversized arc.
const TERMINAL_CORNER_RADIUS_LIMIT: Pixels = px(12.0);

/// The rounded terminal corner stays inside the window padding, so the arc
/// never covers a cell.
pub(super) fn terminal_corner_radius(
    window: huterm_config::WindowConfig,
) -> Pixels {
    px(window.padding_x.min(window.padding_y).max(0.0).floor())
        .min(TERMINAL_CORNER_RADIUS_LIMIT)
}

/// The strip's bounds inside the tab bar. Horizontal Pill bars start with a
/// leading margin so the first pill's visible edge matches the vertical
/// inset; every other placement and style fills the bar.
/// The strip also starts after `inset` along its axis: the display safe
/// area a vertical column's background spans but its rows avoid, or the
/// traffic lights at the start of the title-bar row. A horizontal strip
/// also ends `trailing` before the bar's end: the window controls at the
/// end of the title row Huterm draws.
fn strip_bounds(
    tabs: Bounds<Pixels>,
    inset: Pixels,
    trailing: Pixels,
    config: huterm_config::TabsConfig,
) -> Bounds<Pixels> {
    if config.position.vertical() {
        let inset = inset.min(tabs.size.height);
        return Bounds::new(
            point(tabs.origin.x, tabs.origin.y + inset),
            size(tabs.size.width, tabs.size.height - inset),
        );
    }
    let mut lead = inset.min(tabs.size.width);
    if config.style == TabStyle::Pill {
        lead = (lead + PILL_INSET - PILL_MARGIN_LEFT).min(tabs.size.width);
    }
    let trailing = trailing.max(px(0.0)).min(tabs.size.width - lead);
    Bounds::new(
        point(tabs.origin.x + lead, tabs.origin.y),
        size(tabs.size.width - lead - trailing, tabs.size.height),
    )
}

/// The window frame the chrome sits in, from Linux client-side
/// decorations: the border Huterm owns on each side of the content,
/// whether the title row is Huterm's own, and the window buttons it draws
/// at the row's ends. They are zero, false, and none on macOS, in
/// fullscreen, in Quake windows, and wherever the window manager draws the
/// title bar.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct WindowFrame {
    pub(super) inset: gpui::Edges<Pixels>,
    pub(super) controls: bool,
    pub(super) buttons: ButtonLayout,
}

impl WindowFrame {
    /// The frame for a window whose tabs resolved to `position`. Only a
    /// Linux window drawing the merged row owns a border; its inset follows
    /// the tiling, maximized, and fullscreen state GPUI reports, and its
    /// row draws the desktop's `buttons`.
    fn resolve(
        position: TabPosition,
        state: FrameState,
        buttons: ButtonLayout,
    ) -> Self {
        let controls = Platform::current() == Platform::Linux
            && position == TabPosition::Titlebar;
        if !controls {
            return Self::default();
        }
        Self {
            inset: state.frame().edges(),
            controls,
            buttons,
        }
    }

    /// The shadow and rounded corners are drawn only around a border.
    pub(super) fn decorated(self) -> bool {
        self.client_frame().decorated()
    }

    fn client_frame(self) -> ClientFrame {
        ClientFrame {
            inset: self.inset.top.max(px(0.0)),
        }
    }
}

/// The window's content inside `frame`: the border is clamped to the window
/// so a tiny window keeps nonnegative content.
fn content_inside(
    window: gpui::Size<Pixels>,
    frame: WindowFrame,
) -> Bounds<Pixels> {
    let window = size(window.width.max(px(0.0)), window.height.max(px(0.0)));
    let left = frame.inset.left.max(px(0.0)).min(window.width);
    let top = frame.inset.top.max(px(0.0)).min(window.height);
    let right = frame.inset.right.max(px(0.0)).min(window.width - left);
    let bottom = frame.inset.bottom.max(px(0.0)).min(window.height - top);
    Bounds::new(
        point(left, top),
        size(window.width - left - right, window.height - top - bottom),
    )
}

/// The height of the title row above the terminal: `AppKit`'s strip on
/// macOS, or the row Huterm draws inside its own frame on Linux, which is
/// the tab bar's height.
/// Where a merged tab row puts the macOS traffic lights: centred in the title
/// strip, at `AppKit`'s own left inset. Their size depends on the macOS release
/// and window style, so this starts from the native close button's frame.
#[cfg(any(target_os = "macos", test))]
fn merged_traffic_lights(native: Bounds<f64>) -> gpui::Point<Pixels> {
    #[expect(clippy::cast_possible_truncation, reason = "point-sized geometry")]
    let (left, height) = (native.origin.x as f32, native.size.height as f32);
    point(
        px(left),
        ((super::TITLEBAR_HEIGHT - px(height)) / 2.0).max(px(0.0)),
    )
}

pub(super) fn title_row_height(
    chrome_hidden: bool,
    frame: WindowFrame,
) -> Pixels {
    if frame.controls {
        TAB_HEIGHT
    } else {
        terminal_top(chrome_hidden)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ChromeLayout {
    pub(super) terminal: Bounds<Pixels>,
    tabs: Bounds<Pixels>,
    /// The window inside its frame: the whole viewport without one.
    pub(super) content: Bounds<Pixels>,
    pub(super) frame: WindowFrame,
    /// Bar length the strip avoids at its start: the safe-area height a
    /// vertical column spans above its rows, or the traffic lights or
    /// window buttons at the start of the title-bar row.
    strip_inset: Pixels,
    /// Bar length the strip leaves free at its end: the window buttons at
    /// the end of the title row Huterm draws.
    strip_trailing: Pixels,
    /// The bar sits on chrome that exists anyway, a notch shelf or the
    /// title bar, so hiding it frees no terminal space.
    outside_terminal: bool,
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
        if presentation != Presentation::Reserved && !self.outside_terminal {
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
                TabPosition::Top | TabPosition::Titlebar => {
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

    /// The one-point line where the tab bar meets the terminal.
    fn tab_border(&self, position: TabPosition) -> Bounds<Pixels> {
        let tabs = self.tabs;
        let line = px(1.0);
        match position {
            TabPosition::Top | TabPosition::Titlebar => Bounds::new(
                point(tabs.origin.x, tabs.bottom() - line),
                size(tabs.size.width, line),
            ),
            TabPosition::Bottom => {
                Bounds::new(tabs.origin, size(tabs.size.width, line))
            }
            TabPosition::Left => Bounds::new(
                point(tabs.right() - line, tabs.origin.y),
                size(line, tabs.size.height),
            ),
            TabPosition::Right => {
                Bounds::new(tabs.origin, size(line, tabs.size.height))
            }
        }
    }

    /// The one-point line where the terminal meets the top chrome (the macOS
    /// titlebar or the safe area above a notch) beside a vertical tab bar. It
    /// joins the bar's terminal edge so the titlebar and bar read as one
    /// surface around the terminal.
    /// With `span_bar`, the line also runs across the tab bar, for a flush
    /// active Strip row whose top edge continues the terminal's.
    fn top_chrome_border(
        &self,
        position: TabPosition,
        top_chrome: Pixels,
        span_bar: bool,
    ) -> Option<Bounds<Pixels>> {
        if !position.vertical() || top_chrome <= px(0.0) {
            return None;
        }
        let terminal = self.terminal;
        let (x, width) = if span_bar {
            (
                terminal.origin.x.min(self.tabs.origin.x),
                terminal.size.width + self.tabs.size.width,
            )
        } else {
            (terminal.origin.x, terminal.size.width)
        };
        Some(Bounds::new(
            point(x, top_chrome - px(1.0)),
            size(width, px(1.0)),
        ))
    }

    /// The square patch that rounds the terminal's top corner beside a
    /// vertical tab bar. It extends one point into the top chrome and bar so
    /// its border continues the straight lines around the terminal.
    fn terminal_corner(
        &self,
        position: TabPosition,
        top_chrome: Pixels,
        radius: Pixels,
    ) -> Option<Bounds<Pixels>> {
        if !position.vertical() || top_chrome <= px(0.0) || radius <= px(0.0) {
            return None;
        }
        let terminal = self.terminal;
        let extent = radius + px(1.0);
        let x = if position == TabPosition::Left {
            terminal.origin.x - px(1.0)
        } else {
            terminal.right() - radius
        };
        Some(Bounds::new(
            point(x, top_chrome - px(1.0)),
            size(extent, extent),
        ))
    }

    fn sidebar_resize_handle(&self, position: TabPosition) -> Bounds<Pixels> {
        let width = px(SIDEBAR_HANDLE_WIDTH).min(self.tabs.size.width);
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

    /// Lays out with a 32-point horizontal bar; production uses `for_tabs`
    /// so the Pill style can take its taller bar.
    #[cfg(test)]
    pub(super) fn with_safe_area(
        viewport: gpui::Size<Pixels>,
        titlebar: Pixels,
        position: TabPosition,
        sidebar_width: Pixels,
        safe_area: gpui::Edges<Pixels>,
    ) -> Self {
        Self::build(
            viewport,
            titlebar,
            position,
            TAB_HEIGHT,
            sidebar_width,
            safe_area,
            None,
            WindowFrame::default(),
        )
    }

    /// Lays out with a 32-point bar inside `frame`, with the title row
    /// Huterm draws when the frame carries its controls.
    #[cfg(test)]
    pub(super) fn with_frame(
        viewport: gpui::Size<Pixels>,
        position: TabPosition,
        frame: WindowFrame,
    ) -> Self {
        Self::build(
            viewport,
            title_row_height(false, frame),
            position,
            TAB_HEIGHT,
            SIDEBAR_WIDTH,
            gpui::Edges::default(),
            None,
            frame,
        )
    }

    /// The size a Reserved bar takes from the terminal in a window without
    /// a safe area: a vertical column's width, a horizontal bar's height,
    /// and nothing for the merged title-bar row, which shares the titlebar
    /// inset.
    fn bar_reservation(
        tabs: huterm_config::TabsConfig,
        sidebar_width: Pixels,
    ) -> gpui::Size<Pixels> {
        match tabs.position {
            TabPosition::Left | TabPosition::Right => {
                size(sidebar_width, px(0.0))
            }
            TabPosition::Top | TabPosition::Bottom => {
                size(px(0.0), tab_bar_height(tabs))
            }
            TabPosition::Titlebar => size(px(0.0), px(0.0)),
        }
    }

    /// `notch_shelf` places a top bar in that window-relative area beside a
    /// display notch instead of below the safe area. Everything lies
    /// inside `frame`, whose inset is the resize border Huterm owns.
    pub(super) fn for_tabs(
        viewport: gpui::Size<Pixels>,
        titlebar: Pixels,
        tabs: huterm_config::TabsConfig,
        sidebar_width: Pixels,
        safe_area: gpui::Edges<Pixels>,
        notch_shelf: Option<Bounds<Pixels>>,
        frame: WindowFrame,
    ) -> Self {
        Self::build(
            viewport,
            titlebar,
            tabs.position,
            tab_bar_height(tabs),
            sidebar_width,
            safe_area,
            notch_shelf,
            frame,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "every input is one fact about the window the layout partitions"
    )]
    fn build(
        window: gpui::Size<Pixels>,
        titlebar: Pixels,
        position: TabPosition,
        bar_height: Pixels,
        sidebar_width: Pixels,
        safe_area: gpui::Edges<Pixels>,
        notch_shelf: Option<Bounds<Pixels>>,
        frame: WindowFrame,
    ) -> Self {
        // The layout partitions the content inside the frame; the frame's
        // origin is added back at the end. A notch shelf is already
        // window-relative and never coexists with a frame, so it is
        // brought into content coordinates like everything else.
        let content = content_inside(window, frame);
        let viewport = content.size;
        let notch_shelf = notch_shelf.map(|shelf| {
            Bounds::new(shelf.origin - content.origin, shelf.size)
        });
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
        let mut strip_inset = px(0.0);
        let mut strip_trailing = px(0.0);
        let mut outside_terminal = false;
        if position == TabPosition::Titlebar {
            // The merged row is the title strip itself: the full window
            // width above the terminal, at the strip's height. Its tabs
            // start after the traffic lights, or after a small lead in the
            // row Huterm draws, whose window controls end the row; the
            // terminal keeps the whole area below the strip.
            let row = titlebar.max(px(0.0)).min(viewport.height.max(px(0.0)));
            tabs = Bounds::new(
                point(px(0.0), px(0.0)),
                size(viewport.width.max(px(0.0)), row),
            );
            if frame.controls {
                strip_inset = if frame.buttons.leading.is_empty() {
                    TITLE_ROW_LEAD
                } else {
                    frame.buttons.leading.width()
                };
                strip_trailing = frame.buttons.trailing.width();
            } else {
                strip_inset = TRAFFIC_LIGHT_INSET;
            }
            outside_terminal = true;
        } else if position.vertical() {
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
            // The column runs through the display safe area to the
            // titlebar, so its edge spans the whole screen height beside a
            // notch; only its rows stay below the safe area.
            let column_top =
                titlebar.max(px(0.0)).min(viewport.height.max(px(0.0)));
            strip_inset = top - column_top;
            tabs.origin.y = column_top;
            tabs.size.height =
                (viewport.height - column_top - safe_area.bottom.max(px(0.0)))
                    .max(px(0.0));
        } else if let Some(shelf) = notch_shelf.filter(|shelf| {
            // A shelf too short for the bar is not used; the bar then takes
            // its normal place below the safe area at full height.
            position == TabPosition::Top && shelf.size.height >= bar_height
        }) {
            // The bar keeps its height at the bottom of the shelf, so its
            // spacing to the terminal matches a windowed top bar. The
            // terminal keeps the area under the safe area except one point
            // for the border line, so that line never covers a cell.
            tabs = Bounds::new(
                point(shelf.origin.x, shelf.bottom() - bar_height),
                size(shelf.size.width, bar_height),
            );
            let line = px(1.0).min(available.height);
            terminal.origin.y += line;
            terminal.size.height -= line;
            outside_terminal = true;
        } else {
            tabs.size.height = bar_height.min(available.height);
            terminal.size.height =
                (available.height - tabs.size.height).max(px(0.0));
            if position == TabPosition::Top {
                terminal.origin.y += tabs.size.height;
            } else {
                tabs.origin.y += terminal.size.height;
            }
        }
        terminal.origin += content.origin;
        tabs.origin += content.origin;
        Self {
            terminal,
            tabs,
            content,
            frame,
            strip_inset,
            strip_trailing,
            outside_terminal,
        }
    }

    /// The strip's bounds inside the tab bar.
    pub(super) fn strip_bounds(
        &self,
        config: huterm_config::TabsConfig,
    ) -> Bounds<Pixels> {
        strip_bounds(self.tabs, self.strip_inset, self.strip_trailing, config)
    }

    /// The one-point line under the title row Huterm draws, across the
    /// whole content width so it also runs under the window controls.
    fn title_row_border(&self) -> Bounds<Pixels> {
        let line = px(1.0);
        Bounds::new(
            point(self.content.origin.x, self.tabs.bottom() - line),
            size(self.content.size.width, line),
        )
    }
}

impl Render for WorkspaceView {
    #[expect(
        clippy::too_many_lines,
        reason = "window chrome composes tab controls, the window menu, and close confirmation"
    )]
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let active_tab = self.active_tab(cx);
        self.measure_tab_widths(window, cx);
        self.sync_frame(window);
        self.refresh_tab_visibility(window, cx);
        self.sync_tab_layout(window, cx);
        self.sync_window_title(window, cx);
        self.heal_notice_state(window, cx);
        let tabs = self.layout_tabs();
        let position = tabs.position;
        let layout = self.chrome_layout(window);
        let frame = layout.frame;
        let content = layout.content;
        // Inside a drawn border the window is transparent around a rounded,
        // shadowed panel; otherwise the root fills the window.
        let decorated = frame.decorated();
        let foreground = color(self.config.theme.foreground);
        let background = color(self.config.theme.background);
        let colors = TabColors::new(&self.config.theme);
        let swatch = Swatch::from_theme(&self.config.theme);
        let menu_placement = self.menu_button_placement();
        if menu_placement == MenuButtonPlacement::Hidden {
            // A hidden button anchors nothing; `open_menu` falls back.
            self.menu_button_bounds.set(None);
        }
        for button in WindowButton::ALL {
            if !frame.buttons.contains(button) {
                self.window_button_bounds[button.index()].set(None);
            }
        }
        let mut root = div()
            .size_full()
            .relative()
            .when(!decorated, |root| root.bg(background))
            .text_color(foreground)
            .text_size(tab_bar::TAB_TEXT_SIZE)
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
                    }
                },
            ))
            .on_action(cx.listener(
                |view, action: &InvokeWindow, window, cx| {
                    if let Err(error) =
                        view.invoke_interactive(&action.0, window, cx)
                    {
                        view.report_failure(
                            "Command failed",
                            error.to_string(),
                            cx,
                        );
                    }
                },
            ))
            .on_action(cx.listener(Self::invoke_palette));
        if decorated {
            root = root.child(
                div()
                    .absolute()
                    .left(content.origin.x)
                    .top(content.origin.y)
                    .w(content.size.width)
                    .h(content.size.height)
                    .bg(background)
                    .rounded_tl(FRAME_RADIUS)
                    .rounded_tr(FRAME_RADIUS)
                    .shadow(swatch.frame_shadow()),
            );
        }
        let move_view = cx.entity().downgrade();
        let release_view = move_view.clone();
        let press_view = move_view.clone();
        let exit_view = move_view.clone();
        let modifiers_view = move_view.clone();
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
                                    view.pointer_reveal.outside = false;
                                    view.defer_pointer_refresh(window, cx);
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
                                    } else if view.tab_scrollbars.dragging() {
                                        view.tab_scrollbar_drag_to(
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
                    window.on_mouse_event(
                        move |event: &gpui::MouseDownEvent,
                              phase,
                              window,
                              cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = press_view.update(cx, |view, cx| {
                                    view.pointer_reveal.outside = false;
                                    // Only a press the title row itself
                                    // accepts (in its bubble handler) may
                                    // become a window move.
                                    view.title_row_press.set(false);
                                    view.defer_pointer_refresh(window, cx);
                                    // A press outside the menu closes it and
                                    // then proceeds; the button toggles it.
                                    let inside = [
                                        view.menu_bounds.get(),
                                        view.menu_button_bounds.get(),
                                    ]
                                    .into_iter()
                                    .flatten()
                                    .any(|bounds| {
                                        bounds.contains(&event.position)
                                    });
                                    if view.menu.is_some() && !inside {
                                        view.close_menu(
                                            MenuFocusReturn::Terminal,
                                            window,
                                            cx,
                                        );
                                    }
                                });
                            }
                        },
                    );
                    window.on_mouse_event(
                        move |_: &gpui::MouseExitEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = exit_view.update(cx, |view, cx| {
                                    view.pointer_reveal.outside = true;
                                    view.defer_pointer_refresh(window, cx);
                                });
                            }
                        },
                    );
                    window.on_modifiers_changed(move |_, window, cx| {
                        let _ = modifiers_view.update(cx, |view, cx| {
                            view.defer_pointer_refresh(window, cx);
                        });
                    });
                    window.on_mouse_event(
                        move |event: &MouseUpEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = release_view.update(cx, |view, cx| {
                                    view.defer_pointer_refresh(window, cx);
                                    if event.button != MouseButton::Left {
                                        return;
                                    }
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
                                    } else if view.tab_scrollbars.dragging() {
                                        view.tab_scrollbar_release(cx);
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
        let titlebar = self.title_row();
        let top_chrome = titlebar + self.fullscreen_insets.top.max(px(0.0));
        let bar_drawn = self.presentation() == Presentation::Reserved
            || self.presentation() == Presentation::Overlay
                && self.reveal.progress > 0.0;
        // The tab bar and the title strip are one row: the tabs cover the
        // strip, whose trailing space keeps the title bar's gestures.
        let merged_row = position == TabPosition::Titlebar && bar_drawn;
        // A top bar on the notch shelf, currently shown.
        let shelf_bar = self.notch_shelf().is_some()
            && self.presentation() == Presentation::Reserved;
        let title_menu_button = (menu_placement
            == MenuButtonPlacement::TitleStrip)
            .then(|| self.menu_button_element(colors, cx).mr(CONTROL_INSET));
        // The window buttons that start the row Huterm draws.
        let title_leading = (!frame.buttons.leading.is_empty()).then(|| {
            self.window_controls_element(
                frame.buttons.leading,
                true,
                colors,
                window,
                cx,
            )
        });
        // The strip's trailing group: the menu button when the strip holds
        // it, then the window buttons that end the row Huterm draws.
        let title_trailing = div()
            .ml_auto()
            .flex()
            .items_center()
            .children(title_menu_button)
            .when(!frame.buttons.trailing.is_empty(), |trailing| {
                trailing.child(self.window_controls_element(
                    frame.buttons.trailing,
                    false,
                    colors,
                    window,
                    cx,
                ))
            });
        // The title centres between the space its ends reserve.
        let title_padding =
            layout.strip_inset.max(layout.strip_trailing + CONTROL_SLOT);
        if top_chrome > px(0.0) {
            root = root.child(
                div()
                    .absolute()
                    .top(content.origin.y)
                    .left(content.origin.x)
                    .w(content.size.width)
                    .h(top_chrome)
                    .when(decorated, |bar| {
                        bar.rounded_tl(FRAME_RADIUS).rounded_tr(FRAME_RADIUS)
                    })
                    .bg(
                        // A shelf bar fills the whole safe-area strip, so
                        // the bar reads as one band across the notch.
                        if shelf_bar
                            || top_chrome_uses_bar(
                                position,
                                titlebar > px(0.0),
                                self.presentation(),
                                self.reveal.progress,
                            )
                        {
                            colors.bar
                        } else {
                            background
                        },
                    )
                    .when(titlebar > px(0.0), |bar| {
                        // The strip stays draggable and keeps the menu
                        // button at its right end.
                        bar.flex()
                            .items_center()
                            .window_control_area(WindowControlArea::Drag)
                            .children(title_leading)
                            .child(title_trailing)
                    })
                    .when(titlebar > px(0.0) || frame.controls, |bar| {
                        // The macOS strip or Huterm's own Linux title row:
                        // its empty space moves the window, a double-click
                        // runs the title-bar action, and a secondary press
                        // opens the window manager's menu.
                        self.title_row_gestures(bar)
                    })
                    .when(titlebar > px(0.0) && !merged_row, |bar| {
                        // The active tab's title, centred across the strip
                        // and kept clear of the traffic lights and the
                        // menu button. A merged row shows the tabs instead.
                        let strip_title = self
                            .tabs
                            .iter()
                            .find(|tab| Some(tab.id) == active_tab)
                            .map_or_else(
                                || "Huterm".to_owned(),
                                |tab| tab.title(self.config.tabs, cx),
                            );
                        bar.child(
                            div()
                                .absolute()
                                .inset_0()
                                .px(title_padding)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(div().truncate().child(strip_title)),
                        )
                    }),
            );
        }
        if let Some(tab) = self.active_view(cx) {
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
        self.sync_tab_scrollbars();
        let strip = self.tab_strip(window);
        let vertical = strip.vertical;
        self.tab_scroll = strip.offset;
        if let Some(drag) = &mut self.reorder {
            let was_scrolling = drag.dragging
                && drag
                    .strip
                    .autoscroll(drag.pointer, Duration::from_millis(16))
                    != self.tab_scroll;
            drag.strip = strip.clone();
            if !was_scrolling {
                self.last_scroll = Instant::now();
            }
        }
        if bar_drawn {
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
            // The bar occludes the title strip beneath it, so it leaves out
            // the ends where the strip draws the window buttons.
            let bar_lead = frame.buttons.leading.width();
            chrome = chrome.child(
                div()
                    .id("tab-bar")
                    .absolute()
                    .left(layout.tabs.origin.x + bar_lead - clip.origin.x)
                    .top(layout.tabs.origin.y - clip.origin.y)
                    .w(layout.tabs.size.width
                        - bar_lead
                        - layout.strip_trailing)
                    .h(layout.tabs.size.height)
                    .bg(colors.bar)
                    .occlude()
                    .when(merged_row && decorated, |row| {
                        // Where the bar reaches a top corner of the frame,
                        // it takes the frame's rounding.
                        row.when(bar_lead == px(0.0), |row| {
                            row.rounded_tl(FRAME_RADIUS)
                        })
                        .when(layout.strip_trailing == px(0.0), |row| {
                            row.rounded_tr(FRAME_RADIUS)
                        })
                    })
                    .when(merged_row, |row| {
                        // The space after the tabs is still the title bar:
                        // it moves the window and takes the title-bar
                        // double-click. Tabs, `+`, and the menu button sit above this
                        // background and keep their presses.
                        self.title_row_gestures(
                            row.window_control_area(WindowControlArea::Drag),
                        )
                    })
                    .when(!(merged_row && frame.controls), |bar| {
                        // A right press on empty bar space opens the window
                        // menu there. Huterm's Linux title row keeps the
                        // window manager's menu instead.
                        bar.on_mouse_down(
                            MouseButton::Right,
                            cx.listener(
                                |view, event: &MouseDownEvent, window, cx| {
                                    view.open_menu_at(
                                        event.position,
                                        window,
                                        cx,
                                    );
                                    cx.stop_propagation();
                                },
                            ),
                        )
                    }),
            );
            if shelf_bar {
                // One point below the safe area so the notch never hides
                // it, across the whole window, in the point the layout kept
                // clear above the terminal.
                chrome = chrome.child(
                    div()
                        .absolute()
                        .left(px(0.0) - clip.origin.x)
                        .top(layout.terminal.origin.y - px(1.0) - clip.origin.y)
                        .w(window.viewport_size().width)
                        .h(px(1.0))
                        .bg(colors.border),
                );
            } else {
                // Huterm's own row also runs its line under the window
                // controls, which lie beyond the tab bar.
                let edge = if merged_row && frame.controls {
                    layout.title_row_border()
                } else {
                    layout.tab_border(position)
                };
                chrome = chrome.child(
                    div()
                        .absolute()
                        .left(edge.origin.x - clip.origin.x)
                        .top(edge.origin.y - clip.origin.y)
                        .w(edge.size.width)
                        .h(edge.size.height)
                        .bg(colors.border),
                );
            }
            let mut bar = div()
                .id("tab-strip")
                .occlude()
                .absolute()
                .left(strip.bounds.origin.x - clip.origin.x)
                .top(strip.bounds.origin.y - clip.origin.y)
                .w(strip.bounds.size.width)
                .h(strip.bounds.size.height)
                .overflow_hidden()
                // The strip occludes the row beneath it, so its own empty
                // space after the last tab carries the row's gestures too;
                // tabs stop their presses before they reach it.
                .when(merged_row && frame.controls, |bar| {
                    self.title_row_gestures(bar)
                })
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
            // Centers 26-point controls across a horizontal bar.
            let bar_inset =
                ((layout.tabs.size.height - CONTROL_SIZE) / 2.0).max(px(0.0));
            for index in 0..self.tabs.len() {
                let tab = &self.tabs[index];
                let (title, status) = tab.label(self.config.tabs, cx);
                let offset = strip.start(index) - strip.offset;
                let bounds = if vertical {
                    Bounds::new(
                        point(px(0.0), offset),
                        size(layout.tabs.size.width, TAB_HEIGHT),
                    )
                } else {
                    Bounds::new(
                        point(offset, px(0.0)),
                        size(strip.tab_extent(index), layout.tabs.size.height),
                    )
                };
                let item = TabItem {
                    id: tab.id,
                    index,
                    title,
                    status,
                    activity: if Some(tab.id) == active_tab {
                        Activity::Active
                    } else if index > 0
                        && Some(self.tabs[index - 1].id) == active_tab
                    {
                        Activity::FollowsActive
                    } else {
                        Activity::Inactive
                    },
                    flush_start: index == 0 && strip.offset == px(0.0),
                    targeted: self
                        .menu
                        .as_ref()
                        .is_some_and(|menu| menu.kind == MenuKind::Tab(tab.id)),
                };
                bar = bar
                    .child(Self::tab_element(&item, bounds, tabs, colors, cx));
            }
            for forward in [false, true] {
                if !vertical
                    && ((forward && strip.offset < strip.max_offset())
                        || (!forward && strip.offset > px(0.0)))
                {
                    let edge = if forward {
                        (strip.available() - CONTROL_SLOT).max(px(0.0))
                    } else {
                        px(0.0)
                    } + CONTROL_INSET;
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
                            .top(if vertical { edge } else { bar_inset })
                            .w(CONTROL_SIZE)
                            .h(CONTROL_SIZE)
                            .flex()
                            .items_center()
                            .justify_center()
                            .group("scroll-tabs")
                            .bg(colors.bar)
                            .hover(|style| style.bg(colors.control_hover))
                            .active(|style| style.bg(colors.control_pressed))
                            .rounded_md()
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
                                    if view.scroll_target.is_none() {
                                        view.last_scroll = Instant::now();
                                    }
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
                            .child(
                                icon_element(
                                    if forward {
                                        Icon::ChevronRight
                                    } else {
                                        Icon::ChevronLeft
                                    },
                                    colors.inactive,
                                )
                                .group_hover("scroll-tabs", |style| {
                                    style.text_color(colors.foreground)
                                }),
                            ),
                    );
                }
            }
            // A vertical column shares the new-tab row with the menu control.
            let row = split_new_tab_row(
                (layout.tabs.size.width - VERTICAL_ROW_MARGIN_X * 2.0)
                    .max(px(0.0)),
                menu_placement == MenuButtonPlacement::SplitRow,
            );
            chrome = chrome.child(bar).child(
                div()
                    .id("new-tab")
                    .group("new-tab")
                    .occlude()
                    .hover(|style| style.bg(colors.control_hover))
                    .active(|style| style.bg(colors.control_pressed))
                    .rounded(px(7.0))
                    .absolute()
                    .left(
                        strip.bounds.origin.x - clip.origin.x
                            + if vertical {
                                VERTICAL_ROW_MARGIN_X
                            } else {
                                strip.available() + CONTROL_INSET
                            },
                    )
                    .top(
                        strip.bounds.origin.y - clip.origin.y
                            + if vertical {
                                strip.available() + VERTICAL_ROW_MARGIN_Y
                            } else {
                                bar_inset
                            },
                    )
                    .w(if vertical {
                        row.plus_width
                    } else {
                        CONTROL_SIZE
                    })
                    .h(if vertical {
                        CONTROL_SLOT - VERTICAL_ROW_MARGIN_Y * 2.0
                    } else {
                        CONTROL_SIZE
                    })
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_click(cx.listener(|view, _, window, cx| {
                        if let Err(error) = view.new_tab(window, cx) {
                            view.report_failure(
                                "Cannot open tab",
                                error.to_string(),
                                cx,
                            );
                        }
                    }))
                    .child(
                        icon_element(Icon::Plus, colors.inactive)
                            .group_hover("new-tab", |style| {
                                style.text_color(colors.foreground)
                            }),
                    ),
            );
            let bar_menu_button = match menu_placement {
                // The strip area ends where the bar does, or before the
                // window controls in Huterm's own title row. `TabStrip`
                // shrinks its own bounds to Fit tabs' total width, so the
                // layout's strip area anchors the button, not `strip`.
                MenuButtonPlacement::BarEnd => Some(point(
                    layout.strip_bounds(tabs).right()
                        - clip.origin.x
                        - CONTROL_SLOT
                        + CONTROL_INSET,
                    strip.bounds.origin.y - clip.origin.y + bar_inset,
                )),
                MenuButtonPlacement::SplitRow => row.menu_x.map(|menu_x| {
                    let row_height = CONTROL_SLOT - VERTICAL_ROW_MARGIN_Y * 2.0;
                    point(
                        strip.bounds.origin.x - clip.origin.x
                            + VERTICAL_ROW_MARGIN_X
                            + menu_x,
                        strip.bounds.origin.y - clip.origin.y
                            + strip.available()
                            + VERTICAL_ROW_MARGIN_Y
                            + (row_height - CONTROL_SIZE) / 2.0,
                    )
                }),
                MenuButtonPlacement::TitleStrip
                | MenuButtonPlacement::Hidden => None,
            };
            if let Some(origin) = bar_menu_button {
                chrome = chrome.child(
                    self.menu_button_element(colors, cx)
                        .absolute()
                        .left(origin.x)
                        .top(origin.y),
                );
            }
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
            // Above the resize handle so a press on the thumb scrolls instead
            // of resizing; misses fall through to the handle and rows.
            {
                let axis = Self::tab_scrollbar_axis(&strip);
                let geometries =
                    Self::tab_scrollbar_geometries(&strip, self.layout_tabs());
                // Covers the strip and its edge inset so the layers inside
                // line up with the hit test; misses fall through.
                let extent = px(self.tab_scrollbars.strip_extent(axis));
                let geometry = match axis {
                    Axis::Vertical => geometries.vertical,
                    Axis::Horizontal => geometries.horizontal,
                };
                let placement = if vertical {
                    Bounds::new(
                        point(
                            strip.bounds.right() - extent,
                            strip.bounds.origin.y,
                        ),
                        size(extent, strip.available()),
                    )
                } else {
                    Bounds::new(
                        point(
                            strip.bounds.origin.x,
                            strip.bounds.bottom() - extent,
                        ),
                        size(strip.available(), extent),
                    )
                };
                let show_strip =
                    self.tab_scrollbars.wants_strip(axis) && geometry.is_some();
                if !show_strip {
                    // No strip remains to paint its leave animation.
                    self.tab_scrollbars.unmount(axis);
                }
                if show_strip {
                    chrome = chrome.child(
                        div()
                            .id("tab-scrollbar")
                            .absolute()
                            .left(placement.origin.x - clip.origin.x)
                            .top(placement.origin.y - clip.origin.y)
                            .w(placement.size.width)
                            .h(placement.size.height)
                            .on_mouse_move(cx.listener(
                                |view, event: &MouseMoveEvent, window, cx| {
                                    view.tab_scrollbar_pointer_moved(
                                        event.position,
                                        window,
                                        cx,
                                    );
                                },
                            ))
                            .on_hover(cx.listener(
                                |view, hovering: &bool, _, cx| {
                                    if !hovering
                                        && view
                                            .tab_scrollbars
                                            .pointer_left(Instant::now())
                                    {
                                        cx.notify();
                                    }
                                },
                            ))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(
                                    |view,
                                     event: &MouseDownEvent,
                                     window,
                                     cx| {
                                        if view.tab_scrollbar_press(
                                            event.position,
                                            window,
                                            cx,
                                        ) {
                                            cx.stop_propagation();
                                        }
                                    },
                                ),
                            )
                            .children(
                                self.tab_scrollbars
                                    .layers(&geometries, colors.scrollbar)
                                    .collect::<Vec<_>>(),
                            ),
                    );
                }
            }
            root = root.child(chrome);
            // A flush active Strip row continues the terminal's top border
            // across the bar, so the corner stays square.
            let flush_active = vertical
                && tabs.style == TabStyle::Strip
                && strip.offset == px(0.0)
                && self
                    .tabs
                    .first()
                    .is_some_and(|tab| Some(tab.id) == active_tab);
            // Only a titlebar joins the column through a line and corner; a
            // fullscreen safe area lets the column run to the screen top.
            if self.presentation() == Presentation::Reserved
                && let Some(edge) =
                    layout.top_chrome_border(position, titlebar, flush_active)
            {
                root = root.child(
                    div()
                        .absolute()
                        .left(edge.origin.x)
                        .top(edge.origin.y)
                        .w(edge.size.width)
                        .h(edge.size.height)
                        .bg(colors.border),
                );
                let radius = terminal_corner_radius(self.config.window);
                if !flush_active
                    && let Some(corner) =
                        layout.terminal_corner(position, titlebar, radius)
                {
                    let left = position == TabPosition::Left;
                    let outer = radius + px(1.0);
                    root = root.child(
                        div()
                            .absolute()
                            .left(corner.origin.x)
                            .top(corner.origin.y)
                            .w(corner.size.width)
                            .h(corner.size.height)
                            .bg(colors.bar)
                            .child(
                                div()
                                    .size_full()
                                    .bg(background)
                                    .border_color(colors.border)
                                    .border_t(px(1.0))
                                    .when(left, |patch| {
                                        patch
                                            .border_l(px(1.0))
                                            .rounded_tl(outer)
                                    })
                                    .when(!left, |patch| {
                                        patch
                                            .border_r(px(1.0))
                                            .rounded_tr(outer)
                                    }),
                            ),
                    );
                }
            }
        }
        if let Some(drag) = &self.reorder
            && drag.dragging
        {
            let marker = strip.marker(strip.slot(drag.pointer));
            let source = self
                .tabs
                .iter()
                .position(|tab| tab.id == drag.source.tab)
                .unwrap_or_default();
            let preview = strip.preview(drag.pointer, source);
            let title = self
                .tabs
                .iter()
                .find(|tab| tab.id == drag.source.tab)
                .map(|tab| tab.title(self.config.tabs, cx))
                .unwrap_or_default();
            root = root.child(
                div()
                    .absolute()
                    .left(preview.origin.x)
                    .top(preview.origin.y)
                    .w(preview.size.width)
                    .h(preview.size.height)
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .px(px(12.0))
                    .rounded(px(7.0))
                    .bg(colors.active)
                    .border_1()
                    .border_color(colors.border)
                    .text_color(colors.foreground)
                    .opacity(0.9)
                    .child(title),
            );
            root = root.child(
                div()
                    .absolute()
                    .left(marker.origin.x)
                    .top(marker.origin.y)
                    .w(marker.size.width)
                    .h(marker.size.height)
                    .bg(colors.accent),
            );
        }
        if let Some(notices) = self.render_notices(layout.terminal, cx) {
            root = root.child(notices);
        }
        if let Some(menu) = self.render_menu(&layout, window, cx) {
            root = root.child(menu);
        }
        if let Some(details) = &self.about {
            let about = render_about(
                details,
                content.size,
                Swatch::from_theme(&self.config.theme),
                self.config.window.shortcut_hints,
                cx.listener(|view, _, _, cx| {
                    view.copy_about_details(cx);
                }),
                cx.listener(|view, _, window, cx| {
                    view.close_about(window, cx);
                }),
            );
            root = root.child(modal_layer(about, &layout));
        }
        self.drawn_dialog_groups = None;
        if let Some(target) = self.close.confirmation.clone() {
            let input = self.close_dialog_input(&target, cx);
            self.drawn_dialog_groups = Some(dialog_group_titles(&input));
            let model = build_close_dialog(&input);
            let dialog = render_close_dialog(
                &model,
                self.close.dialog_focus,
                content.size,
                Swatch::from_theme(&self.config.theme),
                self.config.window.shortcut_hints,
                cx.listener(|view, _, window, cx| {
                    view.cancel_confirmation(window, cx);
                }),
                cx.listener(move |view, _, window, cx| {
                    view.finish_close(target.clone(), window, cx);
                }),
            );
            root = root.child(modal_layer(dialog, &layout));
        }
        if let Some(palette) = &self.palette {
            root = root.child(palette.clone());
        }
        if decorated {
            // The border around the content: a press on it asks the window
            // manager to resize from that edge or corner.
            for (edge, zone) in
                frame.client_frame().resize_zones(window.viewport_size())
            {
                root = root.child(
                    div()
                        .absolute()
                        .left(zone.origin.x)
                        .top(zone.origin.y)
                        .w(zone.size.width)
                        .h(zone.size.height)
                        .cursor(resize_cursor(edge))
                        .on_mouse_down(
                            MouseButton::Left,
                            move |_, window, cx| {
                                window.start_window_resize(edge);
                                cx.stop_propagation();
                            },
                        ),
                );
            }
        }
        // Layout may change drag geometry or clear hover without an input event.
        self.frame_clock.animate(
            cx.entity().downgrade(),
            refresh::Animated::animation_schedule(self, Instant::now()),
            cx,
        );
        root
    }
}

/// Hosts a modal `overlay` that fills its parent: the whole window, or the
/// content inside a drawn client frame, so the transparent border around
/// the frame's shadow stays untinted and its rounded top corners hold.
fn modal_layer(overlay: gpui::Div, layout: &ChromeLayout) -> gpui::Div {
    if !layout.frame.decorated() {
        return overlay;
    }
    let content = layout.content;
    div()
        .absolute()
        .left(content.origin.x)
        .top(content.origin.y)
        .w(content.size.width)
        .h(content.size.height)
        .child(overlay.rounded_tl(FRAME_RADIUS).rounded_tr(FRAME_RADIUS))
}

/// The `menu_*` commands act on the open menu instead of replacing it.
fn is_menu_command(command: huterm_protocol::CommandId) -> bool {
    matches!(
        command,
        ids::OPEN_MENU
            | ids::MENU_SELECT_NEXT
            | ids::MENU_SELECT_PREVIOUS
            | ids::MENU_SELECT_FIRST
            | ids::MENU_SELECT_LAST
            | ids::MENU_SELECT_RIGHT
            | ids::MENU_SELECT_LEFT
            | ids::MENU_CONFIRM
            | ids::MENU_CLOSE
    )
}

/// The compiled keymap's first `scroll_to_bottom` binding as a terminal
/// sees it, for the scroll pill's key cap.
fn scroll_to_bottom_key(keymap: &InstalledKeymap) -> Option<String> {
    let contexts = [
        KeyContext::parse("Workspace").unwrap_or_default(),
        KeyContext::parse("Terminal").unwrap_or_default(),
    ];
    keymap
        .shortcuts(ids::SCROLL_TO_BOTTOM, &contexts, None)
        .first()
        .map(|binding| binding.key.clone())
}

impl WorkspaceView {
    /// The button's placement when the tab bar is drawn. Bar geometry
    /// reserves the slot whether or not an overlay bar is revealed, so
    /// scrolling and drop mapping do not shift with the reveal.
    fn bar_menu_placement(&self) -> MenuButtonPlacement {
        menu_button_placement(
            self.config.window.menu_button,
            self.title_row() > px(0.0),
            self.layout_tabs().position,
            true,
        )
    }

    /// Where the menu button is drawn this frame.
    fn menu_button_placement(&self) -> MenuButtonPlacement {
        let presentation = self.presentation();
        let bar_shown = presentation == Presentation::Reserved
            || presentation == Presentation::Overlay
                && self.reveal.progress > 0.0;
        menu_button_placement(
            self.config.window.menu_button,
            self.title_row() > px(0.0),
            self.layout_tabs().position,
            bar_shown,
        )
    }

    fn open_menu_entity(&self) -> Result<Entity<MenuView>, CommandError> {
        self.menu
            .as_ref()
            .map(|menu| menu.view.clone())
            .ok_or_else(|| {
                CommandError::Unavailable("no menu is open".to_owned())
            })
    }

    fn window_menu_input(&self, cx: &App) -> WindowMenuInput {
        let selection = self.active_view(cx).is_some_and(|terminal| {
            terminal.read(cx).command_availability(ids::COPY).is_ok()
        });
        WindowMenuInput::for_build(selection, self.notices.contents().len())
    }

    /// Opens the window menu, or focuses it when it is already open; an
    /// open tab menu is replaced. `from_keyboard` selects the first item,
    /// as `open_menu` does; the button opens with no selection.
    fn open_menu(
        &mut self,
        from_keyboard: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.check_available(false)?;
        if let Some(menu) = &self.menu
            && menu.kind == MenuKind::Window
        {
            menu.view.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
            return Ok(CommandOutcome::Completed);
        }
        if self.palette.is_some() {
            return Err(CommandError::Unavailable(
                "command palette is open".to_owned(),
            ));
        }
        self.close_menu(MenuFocusReturn::Keep, window, cx);
        if let Some(terminal) = self.active_view(cx) {
            terminal.update(cx, TerminalView::clear_composition);
        }
        let contexts = window.context_stack();
        let model = window_menu_model(
            &self.window_menu_input(cx),
            &cx.global::<Desktop>().keymap,
            &contexts,
        );
        self.mount_menu(
            model,
            MenuKind::Window,
            None,
            contexts,
            from_keyboard,
            window,
            cx,
        );
        Ok(CommandOutcome::Completed)
    }

    /// Opens the window menu at `pointer`, as a right press on empty tab-bar
    /// space does, replacing any open menu. It refuses where the tab menu
    /// does.
    fn open_menu_at(
        &mut self,
        pointer: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.check_available(true).is_err() || self.palette.is_some() {
            return;
        }
        self.close_menu(MenuFocusReturn::Keep, window, cx);
        if let Err(error) = self.open_menu(false, window, cx) {
            self.report_failure("Open Menu", error.to_string(), cx);
            return;
        }
        if let Some(menu) = &mut self.menu {
            menu.pointer = Some(pointer);
        }
    }

    /// Opens the context menu for `tab` at `pointer`, replacing any open
    /// menu, without activating the tab. It refuses while structural work,
    /// a close confirmation, the About panel, the palette, or a tab drag
    /// is in progress, so a right press never disturbs reorder capture.
    fn open_tab_menu(
        &mut self,
        tab: TabId,
        pointer: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.check_available(true).is_err()
            || self.palette.is_some()
            || !self.tabs.iter().any(|record| record.id == tab)
        {
            return;
        }
        self.close_menu(MenuFocusReturn::Keep, window, cx);
        if let Some(terminal) = self.active_view(cx) {
            terminal.update(cx, TerminalView::clear_composition);
        }
        let contexts = window.context_stack();
        let Some(model) = self.tab_menu_model(tab, &contexts, cx) else {
            return;
        };
        self.mount_menu(
            model,
            MenuKind::Tab(tab),
            Some(pointer),
            contexts,
            false,
            window,
            cx,
        );
    }

    /// Opens the active terminal's context menu at `pointer`, replacing any
    /// open menu, with rows for the link under the pointer when there is
    /// one. It refuses where the tab menu does.
    fn open_terminal_menu(
        &mut self,
        pointer: gpui::Point<Pixels>,
        link: Option<String>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(tab) = self.active_tab(cx) else {
            return;
        };
        if self.check_available(true).is_err() || self.palette.is_some() {
            return;
        }
        self.close_menu(MenuFocusReturn::Keep, window, cx);
        if let Some(terminal) = self.active_view(cx) {
            terminal.update(cx, TerminalView::clear_composition);
        }
        let contexts = window.context_stack();
        let kind = MenuKind::Terminal { tab, link };
        let Some(model) = self.terminal_menu_model(&kind, &contexts, cx) else {
            return;
        };
        self.mount_menu(
            model,
            kind,
            Some(pointer),
            contexts,
            false,
            window,
            cx,
        );
    }

    /// A terminal's right-click. Only the active terminal's menu opens: a
    /// lookup can finish after its tab was switched away.
    fn handle_context_menu_request(
        &mut self,
        terminal: &Entity<TerminalView>,
        request: &ContextMenuRequest,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        // A right-click closes any open menu in the capture phase before
        // the terminal sees it, so an open menu here was opened after the
        // click; a lookup that finished late must not replace it.
        if self.active_view(cx).as_ref() == Some(terminal)
            && self.menu.is_none()
        {
            self.open_terminal_menu(
                request.position,
                request.link.clone(),
                window,
                cx,
            );
        }
    }

    /// The terminal menu's rows, or `None` once its tab is no longer the
    /// active one.
    fn terminal_menu_model(
        &self,
        kind: &MenuKind,
        contexts: &[KeyContext],
        cx: &App,
    ) -> Option<MenuModel> {
        let MenuKind::Terminal { tab, link } = kind else {
            return None;
        };
        if self.active_tab(cx) != Some(*tab) {
            return None;
        }
        let terminal = self.active_view(cx)?.read(cx);
        let directory = match terminal.metadata.directory() {
            None => DirectoryState::Unknown,
            Some(directory) if directory.is_local() => DirectoryState::Local,
            Some(_) => DirectoryState::Remote,
        };
        let input = TerminalMenuInput {
            platform: Platform::current(),
            link: link.is_some(),
            selection: terminal.command_availability(ids::COPY).is_ok(),
            exited: terminal.command_availability(ids::PASTE).is_err(),
            scrolled_back: terminal.scrolled_back(),
            directory,
        };
        Some(terminal_menu_model(
            input,
            &cx.global::<Desktop>().keymap,
            contexts,
        ))
    }

    /// The tab menu's rows for `tab`, or `None` once the tab is gone.
    fn tab_menu_model(
        &self,
        tab: TabId,
        contexts: &[KeyContext],
        cx: &App,
    ) -> Option<MenuModel> {
        let index = self.tabs.iter().position(|record| record.id == tab)?;
        let directory_known =
            tab_directory_path(&self.tabs[index].view.read(cx).metadata)
                .is_some();
        let input = TabMenuInput {
            platform: Platform::current(),
            index,
            count: self.tabs.len(),
            active: self.active_tab(cx) == Some(tab),
            vertical: self.layout_tabs().position.vertical(),
            directory_known,
        };
        Some(tab_menu_model(
            &input,
            &cx.global::<Desktop>().keymap,
            contexts,
        ))
    }

    /// The rows for whichever menu is open, or `None` once its target is
    /// gone.
    fn current_menu_model(&self, cx: &App) -> Option<MenuModel> {
        let menu = self.menu.as_ref()?;
        match &menu.kind {
            MenuKind::Window => Some(window_menu_model(
                &self.window_menu_input(cx),
                &cx.global::<Desktop>().keymap,
                &menu.contexts,
            )),
            MenuKind::Tab(tab) => self.tab_menu_model(*tab, &menu.contexts, cx),
            kind @ MenuKind::Terminal { .. } => {
                self.terminal_menu_model(kind, &menu.contexts, cx)
            }
        }
    }

    /// Creates the menu entity over `model`, subscribes to its events, and
    /// focuses it.
    #[expect(
        clippy::too_many_arguments,
        reason = "every menu kind supplies its own anchor and context stack"
    )]
    fn mount_menu(
        &mut self,
        model: MenuModel,
        kind: MenuKind,
        pointer: Option<gpui::Point<Pixels>>,
        contexts: Vec<KeyContext>,
        from_keyboard: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let swatch = Swatch::from_theme(&self.config.theme);
        let clock = Rc::clone(&self.frame_clock);
        let view = cx.new(|cx| {
            clock.observe(cx);
            MenuView::new(model, swatch, from_keyboard, cx)
        });
        cx.subscribe_in(&view, window, Self::handle_menu_event)
            .detach();
        view.read(cx).focus_handle(cx).focus(window, cx);
        self.menu = Some(OpenMenu {
            view,
            kind,
            pointer,
            contexts,
        });
        self.menu_bounds.set(None);
        cx.notify();
    }

    /// The menu button: closes an open menu, otherwise opens one with no
    /// selection.
    fn toggle_menu(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.close_menu(MenuFocusReturn::Terminal, window, cx) {
            return;
        }
        if let Err(error) = self.open_menu(false, window, cx) {
            self.report_failure("Open Menu", error.to_string(), cx);
        }
    }

    /// Closes the menu and moves focus; `false` when none was open.
    fn close_menu(
        &mut self,
        focus: MenuFocusReturn,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if self.menu.take().is_none() {
            return false;
        }
        self.menu_bounds.set(None);
        match focus {
            MenuFocusReturn::Terminal => self.focus_terminal(window, cx),
            MenuFocusReturn::Keep => {}
        }
        cx.notify();
        true
    }

    fn focus_terminal(&self, window: &mut Window, cx: &mut App) {
        if let Some(terminal) = self.active_view(cx) {
            let focus = terminal.read(cx).focus.clone();
            focus.focus(window, cx);
        } else {
            self.focus.focus(window, cx);
        }
    }

    /// Rebuilds the open menu's rows so Show Notices, Copy, and a tab
    /// menu's enablement follow the window's state; an unchanged model
    /// leaves the menu alone, and a tab menu closes with its tab.
    fn refresh_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(menu) = self.menu.as_ref().map(|menu| menu.view.clone())
        else {
            return;
        };
        match self.current_menu_model(cx) {
            Some(model) => {
                menu.update(cx, |menu, cx| menu.set_model(model, cx));
            }
            None => {
                self.close_menu(MenuFocusReturn::Terminal, window, cx);
            }
        }
    }

    fn handle_menu_event(
        &mut self,
        menu: &Entity<MenuView>,
        event: &MenuEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(kind) = self
            .menu
            .as_ref()
            .filter(|current| current.view.entity_id() == menu.entity_id())
            .map(|current| current.kind.clone())
        else {
            return;
        };
        match event {
            MenuEvent::Dismissed => {
                self.close_menu(MenuFocusReturn::Terminal, window, cx);
            }
            MenuEvent::Picked(id) => {
                self.close_menu(MenuFocusReturn::Terminal, window, cx);
                if let MenuKind::Terminal {
                    link: Some(link), ..
                } = &kind
                    && self.run_link_pick(id, link, cx)
                {
                    return;
                }
                let Some(spec) = huterm_protocol::lookup(id) else {
                    self.report_failure(
                        "Command failed",
                        format!("unknown command `{id}`"),
                        cx,
                    );
                    return;
                };
                // Context menu picks that take a tab act on the menu's tab.
                let args = kind
                    .tab()
                    .filter(|_| spec.args.iter().any(|arg| arg.name == "tab"))
                    .map(|tab| {
                        vec![CommandArgument::new(
                            "tab",
                            CommandValue::Tab(tab),
                        )]
                    })
                    .unwrap_or_default();
                let invocation = CommandInvocation::new(spec.id, args);
                let result = match spec.scope {
                    CommandScope::Terminal => self
                        .active_view(cx)
                        .ok_or_else(|| {
                            CommandError::Unavailable(
                                "window has no active terminal".to_owned(),
                            )
                        })
                        .and_then(|terminal| {
                            terminal.update(cx, |terminal, cx| {
                                terminal.run_command(&invocation, window, cx)
                            })
                        }),
                    _ => self.invoke_interactive(&invocation, window, cx),
                };
                match result {
                    Ok(_) => {
                        self.recent.record(spec.id);
                        cx.global_mut::<Desktop>().frequency.record(spec.id);
                    }
                    Err(error) => {
                        self.report_failure(
                            "Command failed",
                            error.to_string(),
                            cx,
                        );
                    }
                }
                cx.notify();
            }
        }
    }

    /// Opens or copies the link a terminal menu opened over; `false` when
    /// `id` is not a link row.
    fn run_link_pick(
        &mut self,
        id: &str,
        link: &str,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        match id {
            terminal_menu::OPEN_LINK => {
                if let Some(terminal) = self.active_view(cx) {
                    terminal.update(cx, |terminal, cx| {
                        (terminal.open_link)(link, cx);
                    });
                }
            }
            terminal_menu::COPY_LINK => {
                cx.write_to_clipboard(ClipboardItem::new_string(
                    link.to_owned(),
                ));
            }
            _ => return false,
        }
        true
    }

    /// Runs a `menu_*` command against the open menu. Confirm and close
    /// report through [`MenuEvent`], so the keyboard and pointer share one
    /// path.
    fn run_menu_command(
        &mut self,
        command: huterm_protocol::CommandId,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        let menu = self.open_menu_entity()?;
        menu.update(cx, |menu, cx| {
            match command {
                ids::MENU_SELECT_NEXT => menu.select_next(cx),
                ids::MENU_SELECT_PREVIOUS => menu.select_previous(cx),
                ids::MENU_SELECT_FIRST => menu.select_first(cx),
                ids::MENU_SELECT_LAST => menu.select_last(cx),
                ids::MENU_SELECT_RIGHT => menu.select_right(cx),
                ids::MENU_SELECT_LEFT => menu.select_left(cx),
                ids::MENU_CONFIRM => menu.confirm(cx),
                ids::MENU_CLOSE => MenuView::dismiss(cx),
                other => return Err(CommandError::UnknownCommand(other)),
            }
            Ok(())
        })?;
        Ok(CommandOutcome::Completed)
    }

    /// The menu's anchor: the painted menu button, or the terminal's top
    /// right corner when the button is hidden and `open_menu` ran.
    fn menu_anchor(&self, layout: &ChromeLayout) -> Bounds<Pixels> {
        self.menu_button_bounds.get().unwrap_or_else(|| {
            Bounds::new(
                point(
                    layout.terminal.right() - CONTROL_SLOT + CONTROL_INSET,
                    layout.terminal.origin.y + CONTROL_INSET,
                ),
                size(CONTROL_SIZE, CONTROL_SIZE),
            )
        })
    }

    /// Whether the menu the button anchors is open. Context menus opened at
    /// the pointer, including the window menu from empty bar space, leave
    /// the button alone.
    fn menu_button_open(&self) -> bool {
        self.menu.as_ref().is_some_and(|menu| {
            menu.kind == MenuKind::Window && menu.pointer.is_none()
        })
    }

    /// The menu control: a 14-point icon in the 26-point control with the
    /// tab-bar hover style, a "Menu" tooltip, and a yellow dot while notices
    /// wait. It never holds keyboard focus: Escape returns focus to the
    /// terminal. The caller positions it. It records its painted bounds for
    /// the menu's anchor.
    fn menu_button_element(
        &self,
        colors: TabColors,
        cx: &mut Context<'_, Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let bounds_cell = Rc::clone(&self.menu_button_bounds);
        let open = self.menu_button_open();
        let dot = !open && !self.notices.is_empty();
        let swatch = Swatch::from_theme(&self.config.theme);
        div()
            .id("window-menu")
            .group("window-menu")
            .occlude()
            .w(CONTROL_SIZE)
            .h(CONTROL_SIZE)
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .hover(|style| style.bg(colors.control_hover))
            .when(open, |button| button.bg(colors.control_hover))
            .active(|style| style.bg(colors.control_pressed))
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click(cx.listener(|view, _, window, cx| {
                view.toggle_menu(window, cx);
                cx.stop_propagation();
            }))
            .child(
                icon_element(Icon::Menu, colors.inactive)
                    .w(px(14.0))
                    .h(px(14.0))
                    .group_hover("window-menu", |style| {
                        style.text_color(colors.foreground)
                    }),
            )
            .when(dot, |button| {
                button.child(
                    div()
                        .absolute()
                        .top(px(2.0))
                        .right(px(2.0))
                        .w(px(9.0))
                        .h(px(9.0))
                        .rounded_full()
                        .bg(swatch.warning)
                        .border_1()
                        .border_color(colors.bar),
                )
            })
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), _, _| bounds_cell.set(Some(bounds)),
                )
                .absolute()
                .inset_0(),
            )
    }

    /// The gestures of a title row Huterm owns, on its empty space: a
    /// primary drag starts the platform's window move, a double-click
    /// runs the title-bar action, and a secondary press opens the window
    /// manager's menu on Linux.
    ///
    /// The move starts on the first drag motion after the press, not on
    /// the press itself: `_NET_WM_MOVERESIZE` and `AppKit`'s window drag
    /// take over the pointer, which would swallow the release and keep a
    /// second press from counting as the double-click.
    fn title_row_gestures<E: InteractiveElement>(&self, row: E) -> E {
        let pressed = Rc::clone(&self.title_row_press);
        let dragged = Rc::clone(&self.title_row_press);
        let released = Rc::clone(&self.title_row_press);
        let moves = Rc::clone(&self.title_row_moves);
        let row = row
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                if event.click_count == 2 {
                    pressed.set(false);
                    // macOS follows the user's double-click preference.
                    if cfg!(target_os = "macos") {
                        window.titlebar_double_click();
                    } else {
                        window.zoom_window();
                    }
                } else {
                    pressed.set(true);
                }
                cx.stop_propagation();
            })
            .on_mouse_move(move |event, window, _| {
                if event.dragging() && dragged.replace(false) {
                    moves.set(moves.get() + 1);
                    window.start_window_move();
                }
            })
            .on_mouse_up(MouseButton::Left, move |_, _, _| released.set(false));
        // macOS has no window menu to show; a right press on its title row
        // reaches the tab bar's own menu instead.
        if cfg!(target_os = "macos") {
            return row;
        }
        row.on_mouse_down(MouseButton::Right, |event, window, cx| {
            window.show_window_menu(event.position);
            cx.stop_propagation();
        })
    }

    /// One side's window buttons in the title row Huterm draws: round
    /// 22-point buttons with a subtle fill, in the desktop's order, padded
    /// more at the window edge. Close takes the assessed window-close path,
    /// like the `close_window` command, so live jobs still get their
    /// confirmation.
    fn window_controls_element(
        &self,
        group: ButtonGroup,
        leading: bool,
        colors: TabColors,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> gpui::Div {
        let maximized = window.is_maximized();
        let swatch = Swatch::from_theme(&self.config.theme);
        let (outer, inner) =
            (WINDOW_CONTROLS_PADDING_OUTER, WINDOW_CONTROLS_PADDING_INNER);
        let mut controls = div()
            .flex()
            .items_center()
            .gap(WINDOW_CONTROL_GAP)
            .pl(if leading { outer } else { inner })
            .pr(if leading { inner } else { outer });
        for button in group.iter() {
            let (id, icon, label) = match button {
                WindowButton::Minimize => {
                    ("window-minimize", Icon::Minus, "Minimize")
                }
                WindowButton::Maximize if maximized => {
                    ("window-maximize", Icon::Copy, "Restore")
                }
                WindowButton::Maximize => {
                    ("window-maximize", Icon::Square, "Maximize")
                }
                WindowButton::Close => {
                    ("window-close", Icon::X, "Close window")
                }
            };
            let bounds = Rc::clone(&self.window_button_bounds);
            let element = div()
                .id(id)
                .group(id)
                .occlude()
                .relative()
                .w(WINDOW_CONTROL_SIZE)
                .h(WINDOW_CONTROL_SIZE)
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(colors.hover)
                .hover(|style| style.bg(colors.control_hover))
                .active(|style| style.bg(colors.control_pressed))
                .tooltip(move |_, cx| TextTooltip::view(label, swatch, cx))
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .child(
                    icon_element(icon, colors.inactive)
                        .group_hover(id, |style| {
                            style.text_color(colors.foreground)
                        }),
                )
                .child(
                    canvas(
                        |_, _, _| (),
                        move |painted, (), _, _| {
                            bounds[button.index()].set(Some(painted));
                        },
                    )
                    .absolute()
                    .inset_0(),
                );
            controls = controls.child(match button {
                WindowButton::Minimize => element.on_click(|_, window, cx| {
                    window.minimize_window();
                    cx.stop_propagation();
                }),
                WindowButton::Maximize => element.on_click(|_, window, cx| {
                    window.zoom_window();
                    cx.stop_propagation();
                }),
                WindowButton::Close => {
                    element.on_click(cx.listener(|view, _, window, cx| {
                        view.request_close(CloseTarget::Window, window, cx);
                        cx.stop_propagation();
                    }))
                }
            });
        }
        controls
    }

    /// The open menu mounted at its placement: below the anchor, or above
    /// it from the lower half; right-aligned from the right half; scrolling
    /// within the space left. Edges facing the anchor stay flush even if
    /// the panel measures wider or shorter than expected.
    fn render_menu(
        &self,
        layout: &ChromeLayout,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<gpui::Div> {
        let open = self.menu.as_ref()?;
        let menu = open.view.clone();
        let viewport = window.viewport_size();
        let height = menu.read(cx).model().height();
        let anchor = match open.pointer {
            Some(pointer) => MenuAnchor::Pointer(pointer),
            None => MenuAnchor::Button(self.menu_anchor(layout)),
        };
        let placement =
            place_menu(anchor, size(MENU_MIN_WIDTH, height), viewport);
        menu.update(cx, |menu, cx| {
            menu.set_max_height(Some(placement.max_height), cx);
        });
        let bounds_cell = Rc::clone(&self.menu_bounds);
        let shown_height = height.min(placement.max_height);
        let wrapper = div()
            .absolute()
            .when(placement.align_right, |wrapper| {
                wrapper.right(
                    viewport.width - (placement.origin.x + MENU_MIN_WIDTH),
                )
            })
            .when(!placement.align_right, |wrapper| {
                wrapper.left(placement.origin.x)
            })
            .when(placement.opens_up, |wrapper| {
                wrapper.bottom(
                    viewport.height - (placement.origin.y + shown_height),
                )
            })
            .when(!placement.opens_up, |wrapper| {
                wrapper.top(placement.origin.y)
            })
            .child(menu)
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), _, _| bounds_cell.set(Some(bounds)),
                )
                .absolute()
                .inset_0(),
            );
        Some(wrapper)
    }

    /// The open menu's smoke fields: kind, focus, selection, items with
    /// disabled ones marked, overflow and indicator, whether the button
    /// shows it open, and its painted bounds.
    fn menu_smoke_state(
        &self,
        prefix: &str,
        window: &Window,
        cx: &Context<'_, Self>,
    ) -> String {
        let menu = match &self.menu {
            Some(open) => {
                let menu = open.view.read(cx);
                let selection = menu
                    .selection()
                    .and_then(|selection| menu.model().id_at(selection))
                    .unwrap_or("none");
                let kind = match open.kind {
                    MenuKind::Window => "window",
                    MenuKind::Tab(_) => "tab",
                    MenuKind::Terminal { .. } => "terminal",
                };
                let (overflow, indicator) = menu.scroll_state();
                format!(
                    "{prefix}menu=true {prefix}menu_kind={kind} {prefix}menu_focused={} {prefix}menu_selection={selection} {prefix}menu_items={} {prefix}menu_overflow={overflow} {prefix}menu_indicator={indicator}",
                    menu.focus_handle(cx).is_focused(window),
                    menu_items_state(menu.model()),
                )
            }
            None => format!(
                "{prefix}menu=false {prefix}menu_kind=none {prefix}menu_focused=false {prefix}menu_selection=none {prefix}menu_items=none {prefix}menu_overflow=false {prefix}menu_indicator=false"
            ),
        };
        format!(
            "{menu} {prefix}menu_button_open={} {prefix}menu_rect={}",
            self.menu_button_open(),
            self.menu_bounds.get().map_or_else(
                || "none".to_owned(),
                |bounds| format!(
                    "{},{},{},{}",
                    f32::from(bounds.origin.x),
                    f32::from(bounds.origin.y),
                    f32::from(bounds.size.width),
                    f32::from(bounds.size.height)
                )
            )
        )
    }

    /// Smoke output for the transient UI, each field named after `prefix`:
    /// the menu (open, focused, selected item, targeted tab index), the
    /// About panel, the close confirmation (showing, focused button,
    /// Debug-quoted title), notice focus, the Debug-quoted native window
    /// title, the painted menu button and window-controls bounds in logical
    /// points, the window scale, the sampled frame (granted client decorations, applied inset,
    /// maximized, observed fullscreen mode), and the content bounds inside
    /// the frame. Quoted values may contain spaces.
    pub(super) fn ui_smoke_state(
        &self,
        prefix: &str,
        window: &Window,
        cx: &Context<'_, Self>,
    ) -> String {
        let target = self
            .menu
            .as_ref()
            .and_then(|menu| match menu.kind {
                MenuKind::Tab(tab) => Some(tab),
                MenuKind::Window | MenuKind::Terminal { .. } => None,
            })
            .and_then(|tab| {
                self.tabs.iter().position(|record| record.id == tab)
            })
            .map_or_else(|| "none".to_owned(), |index| index.to_string());
        let about = self.about.is_some();
        let menu = self.menu_smoke_state(prefix, window, cx);
        let dialog_focus = match self.close.dialog_focus {
            DialogFocus::Primary => "primary",
            DialogFocus::Cancel => "cancel",
        };
        let (dialog_title, dialog_groups) = self.dialog_smoke_fields(cx);
        let painted = |cell: &Cell<Option<Bounds<Pixels>>>| {
            cell.get().map_or_else(
                || "none".to_owned(),
                |bounds| {
                    format!(
                        "{},{},{},{}",
                        f32::from(bounds.origin.x),
                        f32::from(bounds.origin.y),
                        f32::from(bounds.size.width),
                        f32::from(bounds.size.height)
                    )
                },
            )
        };
        let button = painted(&self.menu_button_bounds);
        let window_buttons = self
            .window_frame()
            .buttons
            .buttons()
            .map(|button| (button, &self.window_button_bounds[button.index()]))
            .filter(|(_, bounds)| bounds.get().is_some())
            .map(|(button, bounds)| {
                format!("{}@{}", button.name(), painted(bounds))
            })
            .collect::<Vec<_>>();
        let controls = if window_buttons.is_empty() {
            "none".to_owned()
        } else {
            window_buttons.join(";")
        };
        let content = self.chrome_layout(window).content;
        #[cfg(target_os = "macos")]
        let traffic_lights = crate::native_titlebar::buttons_in_window(window)
            .map_or_else(
                |_| "none".to_owned(),
                |lights| {
                    format!(
                        "{},{},{},{}",
                        lights.origin.x,
                        lights.origin.y,
                        lights.size.width,
                        lights.size.height
                    )
                },
            );
        #[cfg(not(target_os = "macos"))]
        let traffic_lights = "none";
        format!(
            "{menu} {prefix}menu_target={target} {prefix}about={about} {prefix}confirming={} {prefix}dialog_focus={dialog_focus} {prefix}dialog_title={dialog_title} {prefix}dialog_groups={dialog_groups:?} {prefix}dialog_drawn_groups={:?} {prefix}notice_focus={} {prefix}window_title={:?} {prefix}menu_button={button} {prefix}window_buttons={controls} {prefix}traffic_lights={traffic_lights} {prefix}title_row_moves={} {prefix}scale={} {prefix}client_decorations={} {prefix}frame_inset={} {prefix}maximized={} {prefix}fullscreen={:?} {prefix}content={},{},{},{}",
            self.close.confirmation.is_some(),
            self.drawn_dialog_groups.as_deref().unwrap_or("none"),
            self.notice_focus.is_focused(window),
            self.window_title,
            self.title_row_moves.get(),
            window.scale_factor(),
            self.frame_state.client_decorations(),
            f32::from(self.window_frame().inset.top),
            self.frame_state.maximized,
            self.fullscreen.observed,
            f32::from(content.origin.x),
            f32::from(content.origin.y),
            f32::from(content.size.width),
            f32::from(content.size.height),
        )
    }

    /// Smoke output for tab geometry: each tab's painted bounds in logical
    /// points as `x,y,w,h`, joined by `;`, from the strip's current layout.
    pub(super) fn tab_smoke_rects(&self, window: &Window) -> String {
        let strip = self.tab_strip(window);
        let vertical = self.layout_tabs().position.vertical();
        (0..self.tabs.len())
            .map(|index| {
                let offset = strip.start(index) - strip.offset;
                let extent = strip.tab_extent(index);
                let bounds = if vertical {
                    Bounds::new(
                        strip.bounds.origin + point(px(0.0), offset),
                        size(strip.bounds.size.width, extent),
                    )
                } else {
                    Bounds::new(
                        strip.bounds.origin + point(offset, px(0.0)),
                        size(extent, strip.bounds.size.height),
                    )
                };
                format!(
                    "{},{},{},{}",
                    f32::from(bounds.origin.x),
                    f32::from(bounds.origin.y),
                    f32::from(bounds.size.width),
                    f32::from(bounds.size.height)
                )
            })
            .collect::<Vec<_>>()
            .join(";")
    }
}

/// Whether the window's root shows a close confirmation or the About panel.
/// The scrim occludes the terminal beneath; this keeps pointer input out
/// even if a modal is ever mounted without one.
/// The model's item and button ids in order for smoke state, a trailing
/// `!` marking disabled ones.
fn menu_items_state(model: &MenuModel) -> String {
    let mut ids = Vec::new();
    for row in &model.rows {
        match row {
            MenuRow::Item(item) => {
                ids.push(format!(
                    "{}{}",
                    item.id,
                    if item.enabled { "" } else { "!" }
                ));
            }
            MenuRow::Buttons { buttons, .. } => {
                ids.extend(buttons.iter().map(|button| {
                    format!(
                        "{}{}",
                        button.id,
                        if button.enabled { "" } else { "!" }
                    )
                }));
            }
            MenuRow::Separator => {}
        }
    }
    ids.join(",")
}

/// The focus a showing dialog or About panel holds, for handing it back
/// when something else takes it; `None` while neither shows.
pub(super) fn modal_focus(window: &Window, cx: &App) -> Option<FocusHandle> {
    let root = window.root::<WorkspaceView>().flatten()?;
    let view = root.read(cx);
    view.dialog_showing().then(|| view.focus.clone())
}

pub(super) fn modal_showing(window: &Window, cx: &App) -> bool {
    window
        .root::<WorkspaceView>()
        .flatten()
        .is_some_and(|root| root.read(cx).dialog_showing())
}

#[cfg(target_os = "macos")]
pub(super) fn terminal_input_allowed(window: &Window, cx: &App) -> bool {
    window
        .root::<WorkspaceView>()
        .flatten()
        .is_some_and(|root| {
            let view = root.read(cx);
            !view.busy
                && !view.dialog_showing()
                && view.reorder.is_none()
                && view.palette.is_none()
                && view.menu.is_none()
        })
}

#[cfg(target_os = "macos")]
pub(super) fn active_composition(window: &Window, cx: &App) -> bool {
    window
        .root::<WorkspaceView>()
        .flatten()
        .is_some_and(|root| {
            root.read(cx)
                .active_view(cx)
                .is_some_and(|view| !view.read(cx).composition.is_empty())
        })
}

#[cfg(test)]
mod tests {
    use super::model::remove_tab;
    use super::*;

    #[test]
    fn a_cancelled_spawn_publishes_nothing_and_cleans_up_only_before_teardown()
    {
        assert_eq!(
            spawn_disposition(Resolution::Ready, false),
            SpawnDisposition::Publish
        );
        assert_eq!(
            spawn_disposition(Resolution::Cancelled, false),
            SpawnDisposition::Abandon {
                cleanup: true,
                notify: true
            }
        );
        // Teardown owns every terminal and the application is leaving.
        assert_eq!(
            spawn_disposition(Resolution::Cancelled, true),
            SpawnDisposition::Abandon {
                cleanup: false,
                notify: false
            }
        );
    }

    #[test]
    fn quit_from_a_sibling_waits_for_a_spawn_still_waiting_on_the_projection() {
        let mut pending_spawns = 0;
        let mut quit_pending = false;
        assert!(quit_now(pending_spawns, &mut quit_pending));
        assert!(!quit_pending);
        // A spawn is pending from its start through its sequence wait and
        // publication.
        pending_spawns += 1;
        assert!(!quit_now(pending_spawns, &mut quit_pending));
        assert!(quit_pending);
        assert!(!can_open_window(true, false, quit_pending));
        assert!(settle_spawn(&mut pending_spawns, &mut quit_pending));
        assert_eq!((pending_spawns, quit_pending), (0, false));

        // With two spawns, Quit resumes only after the last settles.
        pending_spawns = 2;
        assert!(!quit_now(pending_spawns, &mut quit_pending));
        assert!(!settle_spawn(&mut pending_spawns, &mut quit_pending));
        assert!(quit_pending);
        assert!(settle_spawn(&mut pending_spawns, &mut quit_pending));
        assert!(!settle_spawn(&mut pending_spawns, &mut quit_pending));
    }

    #[test]
    fn a_stale_cancel_after_confirm_cannot_reach_the_commit() {
        let runtime = DesktopRuntime::default();
        let session = runtime.lock().create_session(None).unwrap();
        let mut close = CloseState::default();
        let target = close.begin_check(CloseTarget::Window);
        close.assessment =
            Some(runtime.assess(CloseRequest::Session(session)).unwrap());
        assert_eq!(
            close.checked(true),
            Some(CloseDecision::Confirm(target.clone()))
        );
        assert!(close.can_cancel_confirmation());
        let (_, confirmed) = close.begin_commit(&target).unwrap();
        assert!(confirmed);
        // A Cancel or Escape drawn before Confirm now finds nothing to cancel.
        assert!(!close.can_cancel_confirmation());
        assert_eq!(close.current, Some(target));
    }

    #[test]
    fn close_commits_retry_only_when_the_runtime_refused_them() {
        assert!(close_commit_retries(&Err(MuxError::StaleClose)));
        assert!(close_commit_retries(&Err(MuxError::ConfirmationRequired)));
        // An accepted close detaches its attachment even when terminal
        // teardown then fails.
        assert!(!close_commit_retries(&Ok(())));
        assert!(!close_commit_retries(&Err(MuxError::Runtime(
            huterm_core::RuntimeError::ShutdownTimedOut
        ))));
    }

    #[test]
    fn the_window_title_names_the_active_tab_before_huterm() {
        assert_eq!(window_title(Some("cargo build")), "cargo build — Huterm");
        assert_eq!(window_title(Some("zsh · exited")), "zsh · exited — Huterm");
        assert_eq!(window_title(None), "Huterm");
        // Unchanged text must not reach `set_window_title` again; the sync
        // compares against the last title it set.
        let mut last = String::new();
        let mut sets = 0;
        for active in [Some("zsh"), Some("zsh"), Some("vim"), None, None] {
            let title = window_title(active);
            if title != last {
                sets += 1;
                last = title;
            }
        }
        assert_eq!(sets, 3);
    }

    #[test]
    fn copy_tab_directory_uses_the_reported_path_local_or_remote() {
        use huterm_protocol::{TerminalDirectory, TerminalMetadata};
        let local = TerminalMetadata::new(
            Some(TerminalDirectory::new(None, "/home/jim/src".into(), true)),
            None,
        );
        assert_eq!(
            tab_directory_path(&local).as_deref(),
            Some("/home/jim/src")
        );
        let remote = TerminalMetadata::new(
            Some(TerminalDirectory::new(
                Some("build-host".into()),
                "/srv/build".into(),
                false,
            )),
            Some("ssh".into()),
        );
        assert_eq!(tab_directory_path(&remote).as_deref(), Some("/srv/build"));
        assert_eq!(tab_directory_path(&TerminalMetadata::default()), None);
        let blank = TerminalMetadata::new(
            Some(TerminalDirectory::new(None, String::new(), true)),
            None,
        );
        assert_eq!(tab_directory_path(&blank), None, "a blank path is unknown");
    }

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

    fn messages(stack: &NoticeStack) -> Vec<String> {
        stack
            .contents()
            .map(|content| content.message.clone())
            .collect()
    }

    #[test]
    fn startup_diagnostics_become_separate_notices_that_reloads_replace() {
        let path = Path::new("/home/me/.config/huterm/huterm.toml");
        let diagnostics = config_diagnostics(
            path,
            Some("config error"),
            Some("keymap error"),
            &["binding conflict".to_owned()],
            Some(config::LEGACY_ALACRITTY_WARNING),
        );
        let sources: Vec<_> = diagnostics
            .iter()
            .map(|content| (content.severity, content.source.clone()))
            .collect();
        assert_eq!(
            sources,
            [
                (Severity::Error, NoticeSource::Config),
                (Severity::Error, NoticeSource::Keymap),
                (Severity::Warning, NoticeSource::Keymap),
                (Severity::Warning, NoticeSource::Config),
            ]
        );
        assert!(diagnostics.iter().all(|content| content.lifetime
            == Lifetime::Persistent
            && content.location.as_deref() == Some(path.to_str().unwrap())));
        let actions = |index: usize| {
            diagnostics[index]
                .actions
                .iter()
                .map(|action| (action.label.as_str(), action.command.id))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            actions(0),
            [
                ("Open Settings", ids::OPEN_SETTINGS),
                ("Reload", ids::RELOAD_CONFIG)
            ]
        );
        assert_eq!(actions(3), [("Open Settings", ids::OPEN_SETTINGS)]);

        let now = Instant::now();
        let mut stack = NoticeStack::default();
        stack.push(
            NoticeContent::command_failure("Close", "Close failed: busy"),
            now,
        );
        assert!(stack.replace_diagnostics(&diagnostics, now));
        assert_eq!(
            messages(&stack),
            [
                "config error",
                "keymap error",
                "binding conflict",
                config::LEGACY_ALACRITTY_WARNING,
                "Close failed: busy",
            ]
        );
        // Dismissing a diagnostic hides it until the next reload.
        let newest = stack.newest().unwrap();
        assert!(stack.dismiss(newest));
        assert_eq!(messages(&stack)[0], "keymap error");
        // A reload that still fails raises every diagnostic again.
        assert!(stack.replace_diagnostics(&diagnostics, now));
        assert_eq!(messages(&stack)[0], "config error");
        assert_eq!(stack.contents().len(), 5);
        // A fixed file clears them but leaves the command notice alone.
        let fixed = config_diagnostics(path, None, None, &[], None);
        assert!(fixed.is_empty());
        assert!(stack.replace_diagnostics(&fixed, now));
        assert_eq!(messages(&stack), ["Close failed: busy"]);
    }

    #[test]
    fn a_failed_reload_keeps_the_active_configs_own_diagnostics() {
        let path = Path::new("/tmp/huterm.toml");
        let active = config_diagnostics(
            path,
            None,
            None,
            &["binding conflict".to_owned()],
            Some(config::LEGACY_ALACRITTY_WARNING),
        );
        let failed = failed_reload_diagnostics("bad toml", &active);
        assert_eq!(
            failed
                .iter()
                .map(|content| content.message.as_str())
                .collect::<Vec<_>>(),
            [
                "Config reload failed: bad toml",
                "binding conflict",
                config::LEGACY_ALACRITTY_WARNING
            ]
        );
        assert_eq!(failed[0].source, NoticeSource::Config);
        assert_eq!(failed[0].severity, Severity::Error);
        assert_eq!(failed[0].actions.len(), 2);

        let now = Instant::now();
        let mut stack = NoticeStack::default();
        stack.replace_diagnostics(&active, now);
        stack.replace_diagnostics(&failed, now);
        assert_eq!(
            messages(&stack),
            [
                "Config reload failed: bad toml",
                "binding conflict",
                config::LEGACY_ALACRITTY_WARNING,
            ],
            "the keymap conflict from the active config stays"
        );
    }

    #[test]
    fn terminal_failures_are_routed_with_their_tab() {
        let tab = TabId::new(7);
        let notice = terminal_notice(
            tab,
            "~/project",
            TerminalFailure {
                severity: Severity::Warning,
                title: "Input rejected",
                message: "Input buffer full".to_owned(),
            },
        );
        assert_eq!(
            notice.source,
            NoticeSource::Terminal {
                tab,
                title: "~/project".to_owned()
            }
        );
        assert_eq!(notice.severity, Severity::Warning);
        assert_eq!(notice.title, "Input rejected");
        assert_eq!(notice.location.as_deref(), Some("~/project"));
        assert_eq!(notice.lifetime, Lifetime::Expiring);
        assert_eq!(
            notice.smoke_line(),
            "warning|terminal:~/project|Input buffer full"
        );
        let now = Instant::now();
        let mut stack = NoticeStack::default();
        stack.push(notice.clone(), now);
        stack.push(
            terminal_notice(
                TabId::new(8),
                "other",
                TerminalFailure {
                    severity: Severity::Error,
                    title: "Terminal error",
                    message: "runtime stopped".to_owned(),
                },
            ),
            now,
        );
        assert!(stack.replace_source(&notice.source, Vec::new(), now));
        assert_eq!(messages(&stack), ["runtime stopped"], "keyed by tab");
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
        close.begin_check(target.clone());
        assert_eq!(
            close.checked(true),
            Some(CloseDecision::Confirm(target.clone()))
        );
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
            let strip = TabStrip::new(
                layout.tabs,
                position.vertical(),
                TabExtents::Uniform(8),
                px(0.0),
                false,
            );
            let extent = strip.tab_extent(0);
            let pointer = if strip.vertical {
                strip.bounds.origin + point(px(20.0), extent * 1.1)
            } else {
                strip.bounds.origin + point(extent * 1.1, px(10.0))
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
                    strip.preview(beyond, 0),
                    strip.preview(perpendicular, 0),
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
            assert!(
                layout.terminal.origin.y >= px(48.5),
                "{position:?}: {:?}",
                layout.terminal
            );
            if position.vertical() {
                // The column spans the safe area; its rows do not.
                assert_eq!(layout.tabs.origin.y, px(0.0));
                let strip = layout.strip_bounds(huterm_config::TabsConfig {
                    position,
                    ..Default::default()
                });
                assert_eq!(strip.origin.y, px(48.5));
                assert_eq!(strip.bottom(), layout.tabs.bottom());
            } else {
                assert!(layout.tabs.origin.y >= px(48.5));
            }
            for bounds in [layout.tabs, layout.terminal] {
                assert!(bounds.bottom() <= px(600.0));
            }
            let column_extra = if position.vertical() {
                f32::from(layout.tabs.size.width) * 48.5
            } else {
                0.0
            };
            let area = f32::from(layout.tabs.size.width)
                * f32::from(layout.tabs.size.height)
                + f32::from(layout.terminal.size.width)
                    * f32::from(layout.terminal.size.height);
            assert!((area - column_extra - 800.0 * 551.5).abs() < 0.01);
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
                    let floor = if position.vertical() && bounds == layout.tabs
                    {
                        px(0.0)
                    } else {
                        px(48.5).min(viewport.height)
                    };
                    assert!(bounds.origin.y >= floor);
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
                    px(SIDEBAR_HANDLE_WIDTH).min(layout.tabs.size.width)
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
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), "printf READY; read value".into()],
            working_directory: std::env::current_dir().unwrap(),
            environment: Vec::new(),
            grid_size: GridSize::clamped(40, 8),
            cell_size: CellSize {
                width: 8,
                height: 16,
            },
            presentation: huterm_protocol::TerminalPresentation::default(),
        }
    }

    #[test]
    fn attachment_allocation_failure_rolls_back_published_pty() {
        let runtime = DesktopRuntime::default();
        runtime.mux.lock().unwrap().reserve_through(u64::MAX - 5);
        assert!(matches!(
            runtime
                .open_tab(None, None, &lifecycle_command(), true)
                .result,
            Err(MuxError::IdExhausted)
        ));
        let mux = runtime.mux.lock().unwrap();
        assert!(mux.sessions().is_empty());
        assert_eq!(mux.terminal_count(), 0);
    }

    #[test]
    fn open_tab_returns_registered_host_effect_recipient() {
        let runtime = DesktopRuntime::default();
        let mut command = lifecycle_command();
        command.arguments = vec![
            "-c".into(),
            "read value; printf '\\033]52;c;ZGVza3RvcABjbGlwYm9hcmQ=\\007'; read value"
                .into(),
        ];
        let (session, _, opened, _, authority) =
            runtime.open_tab(None, None, &command, true).result.unwrap();

        opened
            .client
            .send_input(TerminalInput::Text("go\n".into()))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let pending = loop {
            if let Some(pending) = authority.host_effects.try_next() {
                break pending;
            }
            assert!(
                Instant::now() < deadline,
                "clipboard effect not delivered"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(authority.host_effects.is_current(&pending));
        let HostEffect::ClipboardWrite(write) = pending.effect() else {
            panic!("unexpected host effect");
        };
        assert_eq!(write.text(), "desktop\0clipboard");

        drop(pending);
        runtime.mux.lock().unwrap().close_session(session).unwrap();
    }

    #[test]
    fn orphaned_publication_preserves_another_attachment_and_transferred_tab() {
        let runtime = DesktopRuntime::default();
        let (session, workspace, opened, attachment, _authority) = runtime
            .open_tab(None, None, &lifecycle_command(), true)
            .result
            .unwrap();
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
        let (source, source_workspace, transferred, initial, _authority) =
            runtime
                .open_tab(None, None, &lifecycle_command(), true)
                .result
                .unwrap();
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
        let (source, workspace, opened, attachment, _authority) = runtime
            .open_tab(None, None, &lifecycle_command(), true)
            .result
            .unwrap();
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
            quake_profile: None,
        }];
        let assessment = runtime.assess(CloseRequest::Application).unwrap();
        runtime
            .commit(&assessment, false, Some(windows))
            .result
            .unwrap();
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
            runtime.commit(&assessment, false, Some(Vec::new())).result,
            Err(MuxError::StaleClose)
        ));
        assert!(!runtime.terminating.load(Ordering::Acquire));
        assert!(runtime.restore.lock().unwrap().is_none());
        assert_eq!(runtime.mux.lock().unwrap().sessions().len(), 2);
        let assessment = runtime.assess(CloseRequest::Application).unwrap();
        runtime
            .commit(&assessment, false, Some(Vec::new()))
            .result
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
            program: "/huterm-nonexistent-shell".into(),
            arguments: vec!["-c".into(), "printf READY; read value".into()],
            working_directory: std::env::current_dir().unwrap(),
            environment: Vec::new(),
            grid_size: GridSize::clamped(40, 8),
            cell_size: CellSize {
                width: 8,
                height: 16,
            },
            presentation: huterm_protocol::TerminalPresentation::default(),
        };
        assert!(runtime.open_tab(None, None, &command, true).result.is_err());
        assert!(runtime.mux.lock().unwrap().sessions().is_empty());
        assert_eq!(runtime.mux.lock().unwrap().terminal_count(), 0);
        command.program = "/bin/sh".into();
        let (session, workspace, first, attachment, _first_authority) =
            runtime.open_tab(None, None, &command, true).result.unwrap();
        let (sibling, _, second, _, _second_authority) =
            runtime.open_tab(None, None, &command, true).result.unwrap();
        assert_ne!(session, sibling);
        command.program = "/huterm-nonexistent-shell".into();
        assert!(runtime.open_tab(None, None, &command, true).result.is_err());
        assert!(
            runtime
                .open_tab(Some(workspace), attachment, &command, true)
                .result
                .is_err()
        );
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
            runtime.open_tab(None, None, &command, true).result,
            Err(MuxError::IdExhausted)
        ));
        assert!(runtime.mux.lock().unwrap().sessions().is_empty());
    }

    #[test]
    fn native_termination_drains_existing_terminals_and_rejects_queued_spawns()
    {
        let runtime = Arc::new(DesktopRuntime::default());
        let command = TerminalCommand {
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), "printf READY; read value".into()],
            working_directory: std::env::current_dir().unwrap(),
            environment: Vec::new(),
            grid_size: GridSize::clamped(40, 8),
            cell_size: CellSize {
                width: 8,
                height: 16,
            },
            presentation: huterm_protocol::TerminalPresentation::default(),
        };
        let (_, _, opened, _, _authority) =
            runtime.open_tab(None, None, &command, true).result.unwrap();
        // Hold the structural lock as an already-running spawn would, then
        // queue another spawn and invoke the exact native-hook cleanup method.
        let guard = runtime.mux.lock().unwrap();
        let spawn_runtime = Arc::clone(&runtime);
        let spawn = std::thread::spawn(move || {
            spawn_runtime.open_tab(None, None, &command, true).result
        });
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
        runtime.commit(&assessment, false, None).result.unwrap();
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
            close.begin_check(current.clone());
            close.queue(later);
            assert_eq!(
                close.checked(true),
                Some(CloseDecision::Confirm(current.clone()))
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
        assert_eq!(merge_close(None, tab.clone()), tab);
        assert_eq!(
            merge_close(Some(tab.clone()), CloseTarget::Window),
            CloseTarget::Window
        );
        assert_eq!(
            merge_close(Some(CloseTarget::Window), tab.clone()),
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
    fn tab_sets_union_with_tab_targets_and_lose_to_wider_scopes() {
        let (first, second, third) =
            (TabId::new(1), TabId::new(2), TabId::new(3));
        let set = CloseTarget::Tabs(vec![first, second]);
        // A single tab still replaces a single tab.
        assert_eq!(
            merge_close(
                Some(CloseTarget::Tab(first)),
                CloseTarget::Tab(second)
            ),
            CloseTarget::Tab(second)
        );
        // Unions keep first-occurrence order; `request_close` restores
        // window order before assessing.
        assert_eq!(
            merge_close(Some(CloseTarget::Tab(third)), set.clone()),
            CloseTarget::Tabs(vec![third, first, second])
        );
        assert_eq!(
            merge_close(Some(set.clone()), CloseTarget::Tab(third)),
            CloseTarget::Tabs(vec![first, second, third])
        );
        assert_eq!(
            merge_close(Some(set.clone()), CloseTarget::Tab(second)),
            set
        );
        assert_eq!(
            merge_close(
                Some(set.clone()),
                CloseTarget::Tabs(vec![second, third])
            ),
            CloseTarget::Tabs(vec![first, second, third])
        );
        assert_eq!(
            merge_close(Some(set.clone()), CloseTarget::Window),
            CloseTarget::Window
        );
        assert_eq!(
            merge_close(Some(CloseTarget::Application), set.clone()),
            CloseTarget::Application
        );
        // A queued set drops removed tabs and collapses to a single tab.
        let mut close = CloseState::default();
        close.queue(set);
        assert_eq!(
            close.take_pending(|id| id == second),
            Some(CloseTarget::Tab(second))
        );
        close.queue(CloseTarget::Tabs(vec![first, second]));
        assert_eq!(close.take_pending(|_| false), None);
    }

    #[test]
    fn tab_closes_are_refused_while_a_confirmation_is_pending() {
        let mut close = CloseState::default();
        assert_eq!(close.check_tab_close_available(), Ok(()));
        close.begin_check(CloseTarget::Tab(TabId::new(1)));
        assert_eq!(
            close.checked(true),
            Some(CloseDecision::Confirm(CloseTarget::Tab(TabId::new(1))))
        );
        assert_eq!(close.dialog_focus, DialogFocus::Primary);
        assert_eq!(
            close.check_tab_close_available(),
            Err(CommandError::Unavailable(
                "close confirmation pending".to_owned()
            ))
        );
        close.cancel();
        assert_eq!(close.check_tab_close_available(), Ok(()));
    }

    #[test]
    fn tab_closes_are_refused_while_the_about_panel_shows() {
        let close = CloseState::default();
        assert_eq!(tab_close_availability(&close, false), Ok(()));
        assert_eq!(
            tab_close_availability(&close, true),
            Err(CommandError::Unavailable(
                "About panel is showing".to_owned()
            ))
        );
    }

    #[test]
    fn a_repeated_close_window_is_refused_while_its_confirmation_shows() {
        let pending = Err(CommandError::Unavailable(
            "close confirmation pending".to_owned(),
        ));
        let mut close = CloseState::default();
        assert_eq!(close.check_close_available(&CloseTarget::Window), Ok(()));
        close.begin_check(CloseTarget::Tab(TabId::new(1)));
        close.checked(true);
        assert_eq!(
            close.check_close_available(&CloseTarget::Window),
            Ok(()),
            "closing the window widens a tab confirmation"
        );
        close.cancel();
        close.begin_check(CloseTarget::Window);
        assert_eq!(
            close.checked(true),
            Some(CloseDecision::Confirm(CloseTarget::Window))
        );
        close.dialog_focus = DialogFocus::Cancel;
        assert_eq!(close.check_close_available(&CloseTarget::Window), pending);
        assert_eq!(
            close.check_close_available(&CloseTarget::Application),
            Ok(()),
            "Quit still widens a window confirmation"
        );
        assert_eq!(
            close.dialog_focus,
            DialogFocus::Cancel,
            "the refused repeat leaves the focused button alone"
        );
        close.cancel();
        close.begin_check(CloseTarget::Application);
        close.checked(true);
        assert_eq!(close.check_close_available(&CloseTarget::Window), pending);
    }

    #[test]
    fn a_replaced_focused_toast_releases_focus_and_resumes_expiry() {
        let now = Instant::now();
        let tab = TabId::new(1);
        let failure = |message: &str| {
            terminal_notice(
                tab,
                "shell",
                TerminalFailure {
                    severity: Severity::Error,
                    title: "Terminal error",
                    message: message.to_owned(),
                },
            )
        };
        let mut stack = NoticeStack::default();
        let first = stack.push(failure("first"), now);
        let mut focused = Some(first);
        let mut hovered = HashSet::from([first]);
        assert!(
            !reconcile_notice_state(
                &mut stack,
                &mut focused,
                &mut hovered,
                now
            ),
            "a live focused toast keeps focus"
        );
        assert_eq!(focused, Some(first));
        assert_eq!(stack.next_deadline(), None, "focus pauses expiry");

        // The tab's next failure replaces its toast, including the focused one.
        let source = NoticeSource::Terminal {
            tab,
            title: "shell".to_owned(),
        };
        assert!(stack.replace_source(&source, vec![failure("second")], now));
        assert!(
            reconcile_notice_state(&mut stack, &mut focused, &mut hovered, now),
            "the focused toast went away, so focus returns to the terminal"
        );
        assert_eq!(focused, None);
        assert!(
            hovered.is_empty(),
            "the hover id of the replaced toast goes"
        );
        assert!(
            stack.next_deadline().is_some(),
            "nothing live is focused or hovered, so expiry resumes"
        );
    }

    #[test]
    fn a_set_check_widens_to_a_tab_queued_during_the_assessment() {
        let (first, second, third) =
            (TabId::new(1), TabId::new(2), TabId::new(3));
        let mut close = CloseState::default();
        close.begin_check(CloseTarget::Tabs(vec![first, second]));
        close.queue(CloseTarget::Tab(third));
        assert_eq!(
            close.checked(true),
            Some(CloseDecision::Check(CloseTarget::Tabs(vec![
                first, second, third
            ])))
        );
        assert!(close.pending.is_none());
    }

    #[test]
    fn tab_set_resolution_follows_window_order() {
        let (first, second, third) =
            (TabId::new(1), TabId::new(2), TabId::new(3));
        let order = [first, second, third];
        assert_eq!(other_tabs(&order, first), vec![second, third]);
        assert_eq!(other_tabs(&order, second), vec![first, third]);
        assert_eq!(other_tabs(&order, third), vec![first, second]);
        assert_eq!(other_tabs(&[first], first), Vec::<TabId>::new());
        assert_eq!(tabs_after(&order, first), vec![second, third]);
        assert_eq!(tabs_after(&order, second), vec![third]);
        assert_eq!(tabs_after(&order, third), Vec::<TabId>::new());
        assert_eq!(tabs_after(&order, TabId::new(9)), Vec::<TabId>::new());
        assert_eq!(tabs_target(Vec::new()), None);
        assert_eq!(tabs_target(vec![third]), Some(CloseTarget::Tab(third)));
        assert_eq!(
            tabs_target(vec![second, third]),
            Some(CloseTarget::Tabs(vec![second, third]))
        );
    }

    #[test]
    fn close_dialog_input_maps_busy_terminals_to_tabs() {
        use huterm_core::{JobProcess, JobState};
        let (first, second, third) =
            (TabId::new(1), TabId::new(2), TabId::new(3));
        let titles = vec![
            TabTitle {
                tab: first,
                terminal: TerminalId::new(10),
                title: "build".to_owned(),
            },
            TabTitle {
                tab: second,
                terminal: TerminalId::new(20),
                title: "shell".to_owned(),
            },
            TabTitle {
                tab: third,
                terminal: TerminalId::new(30),
                title: "editor".to_owned(),
            },
        ];
        let cargo = JobProcess {
            pid: 41,
            group: 41,
            group_started: None,
            foreground: true,
            identity: "cargo".to_owned(),
            command: "cargo".to_owned(),
            command_line: Some("cargo build".to_owned()),
        };
        let jobs = [
            (TerminalId::new(10), JobState::Running(vec![cargo.clone()])),
            (TerminalId::new(20), JobState::Idle),
            (TerminalId::new(30), JobState::Unknown),
        ];
        let jobs = || jobs.iter().map(|(terminal, state)| (*terminal, state));
        let row = ProcessRow {
            command: "cargo".to_owned(),
            pid: 41,
            foreground: true,
            command_line: Some("cargo build".to_owned()),
        };

        let single =
            close_dialog_input(&CloseTarget::Tab(first), jobs(), &titles);
        assert_eq!(
            single.target,
            CloseDialogTarget::Tab {
                title: "build".to_owned()
            }
        );
        assert_eq!(
            single.groups,
            vec![
                ProcessGroup {
                    tab_title: None,
                    state: ProcessGroupState::Known(vec![row.clone()]),
                },
                ProcessGroup {
                    tab_title: None,
                    state: ProcessGroupState::Unknown,
                },
            ],
            "idle tabs are omitted and single-tab dialogs have no headings"
        );

        let several = close_dialog_input(
            &CloseTarget::Tabs(vec![first, second, third]),
            jobs(),
            &titles,
        );
        assert_eq!(several.target, CloseDialogTarget::Tabs { count: 3 });
        assert_eq!(
            several.groups,
            vec![
                ProcessGroup {
                    tab_title: Some("build".to_owned()),
                    state: ProcessGroupState::Known(vec![row.clone()]),
                },
                ProcessGroup {
                    tab_title: Some("editor".to_owned()),
                    state: ProcessGroupState::Unknown,
                },
            ]
        );

        let window = close_dialog_input(&CloseTarget::Window, jobs(), &titles);
        assert_eq!(window.target, CloseDialogTarget::Window);
        assert_eq!(window.groups, several.groups);

        let unviewed =
            [(TerminalId::new(99), JobState::Running(vec![cargo.clone()]))];
        let quit = close_dialog_input(
            &CloseTarget::Application,
            unviewed.iter().map(|(terminal, state)| (*terminal, state)),
            &titles,
        );
        assert_eq!(quit.target, CloseDialogTarget::Application);
        assert_eq!(
            quit.groups,
            vec![ProcessGroup {
                tab_title: Some("Detached terminal".to_owned()),
                state: ProcessGroupState::Known(vec![row]),
            }]
        );
    }

    #[test]
    fn a_one_tab_window_dialog_has_no_headings() {
        use huterm_core::{JobProcess, JobState};
        let titles = [TabTitle {
            tab: TabId::new(1),
            terminal: TerminalId::new(10),
            title: "build".to_owned(),
        }];
        let cargo = JobProcess {
            pid: 41,
            group: 41,
            group_started: None,
            foreground: true,
            identity: "cargo".to_owned(),
            command: "cargo".to_owned(),
            command_line: None,
        };
        let running = JobState::Running(vec![cargo]);
        let jobs = [(TerminalId::new(10), &running)];
        let window = close_dialog_input(&CloseTarget::Window, jobs, &titles);
        assert_eq!(
            window
                .groups
                .iter()
                .map(|group| &group.tab_title)
                .collect::<Vec<_>>(),
            [&None],
            "the dialog covers one tab, so its rows need no heading"
        );
        let quit = close_dialog_input(&CloseTarget::Application, jobs, &titles);
        assert_eq!(
            quit.groups[0].tab_title.as_deref(),
            Some("build"),
            "Quit may cover other windows, so it keeps headings"
        );
    }

    #[test]
    fn all_placements_share_nonoverlapping_terminal_and_tab_bounds() {
        for position in [
            TabPosition::Top,
            TabPosition::Bottom,
            TabPosition::Left,
            TabPosition::Right,
            TabPosition::Titlebar,
        ] {
            for titlebar in [px(0.0), px(32.0)] {
                let layout = ChromeLayout::new(
                    size(px(800.0), px(600.0)),
                    titlebar,
                    position,
                );
                // The merged row lives in the titlebar, so it and the
                // terminal partition the whole window instead.
                let shared = if position == TabPosition::Titlebar {
                    px(600.0)
                } else {
                    px(600.0) - titlebar
                };
                assert_eq!(
                    layout.terminal.size.width
                        * f32::from(layout.terminal.size.height)
                        + layout.tabs.size.width
                            * f32::from(layout.tabs.size.height),
                    px(800.0) * f32::from(shared)
                );
                match position {
                    TabPosition::Top | TabPosition::Titlebar => {
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
    fn merged_traffic_lights_centre_any_button_size_at_the_native_inset() {
        let native = |x: f64, size: f64| Bounds {
            origin: point(x, 5.0),
            size: gpui::size(size, size),
        };
        // AppKit's 12-point buttons keep the placement GPUI 0.2.2 needed.
        assert_eq!(
            merged_traffic_lights(native(7.0, 12.0)),
            point(px(7.0), px(10.0))
        );
        // Larger 14-point buttons move right with AppKit and stay centred.
        assert_eq!(
            merged_traffic_lights(native(9.0, 14.0)),
            point(px(9.0), px(9.0))
        );
        // Buttons taller than the strip start at its top edge.
        assert_eq!(
            merged_traffic_lights(native(9.0, 40.0)),
            point(px(9.0), px(0.0))
        );
    }

    #[test]
    fn the_title_bar_row_holds_tabs_after_the_traffic_lights() {
        let viewport = size(px(800.0), px(600.0));
        let strip = px(32.0);
        let top = ChromeLayout::new(viewport, strip, TabPosition::Top);
        let merged = ChromeLayout::new(viewport, strip, TabPosition::Titlebar);
        // The terminal gains the bar's height: it starts under the strip.
        assert_eq!(
            merged.terminal,
            Bounds::new(point(px(0.0), strip), size(px(800.0), px(568.0)))
        );
        assert_eq!(
            merged.terminal.size.height,
            top.terminal.size.height + top.tabs.size.height
        );
        // The row is the strip itself: full width at the strip's height.
        assert_eq!(
            merged.tabs,
            Bounds::new(point(px(0.0), px(0.0)), size(px(800.0), strip))
        );
        // Tabs start after the traffic lights; Pill adds its usual lead.
        let tabs = |style| TabsConfig {
            position: TabPosition::Titlebar,
            style,
            ..TabsConfig::default()
        };
        assert_eq!(
            merged.strip_bounds(tabs(TabStyle::Strip)),
            Bounds::new(
                point(TRAFFIC_LIGHT_INSET, px(0.0)),
                size(px(800.0) - TRAFFIC_LIGHT_INSET, strip)
            )
        );
        assert_eq!(
            merged.strip_bounds(tabs(TabStyle::Pill)).origin.x,
            TRAFFIC_LIGHT_INSET + PILL_INSET - PILL_MARGIN_LEFT
        );
        // The menu button keeps the plain strip's spot at the row's right
        // end, and the strip reserves that slot beside `+`.
        let bar_inset = (merged.tabs.size.height - CONTROL_SIZE) / 2.0;
        assert_eq!(
            point(
                merged.tabs.right() - CONTROL_SLOT + CONTROL_INSET,
                merged.tabs.origin.y + bar_inset
            ),
            point(px(800.0) - CONTROL_INSET - CONTROL_SIZE, CONTROL_INSET)
        );
        let strip_geometry = TabStrip::new(
            merged.strip_bounds(tabs(TabStyle::Strip)),
            false,
            TabExtents::Uniform(3),
            px(0.0),
            true,
        );
        assert_eq!(
            strip_geometry.available(),
            px(800.0) - TRAFFIC_LIGHT_INSET - CONTROL_SLOT * 2.0
        );
        // Hiding the bar frees no terminal space: the strip stays.
        let hidden = ChromeLayout::new(viewport, strip, TabPosition::Titlebar)
            .present(Presentation::Hidden, TabPosition::Titlebar, 0.0);
        assert_eq!(hidden.terminal, merged.terminal);
        // A tiny window clamps the row to the viewport.
        let tiny = ChromeLayout::new(
            size(px(20.0), px(10.0)),
            strip,
            TabPosition::Titlebar,
        );
        assert_eq!(tiny.tabs.size, size(px(20.0), px(10.0)));
        assert_eq!(tiny.terminal.size.height, px(0.0));
        assert_eq!(
            tiny.strip_bounds(tabs(TabStyle::Strip)).size.width,
            px(0.0)
        );
    }

    fn framed(state: FrameState) -> WindowFrame {
        framed_with(state, ButtonLayout::standard())
    }

    fn framed_with(state: FrameState, buttons: ButtonLayout) -> WindowFrame {
        WindowFrame::resolve(
            resolve_tab_position(
                TabPosition::Titlebar,
                TabHost {
                    platform: Platform::Linux,
                    fullscreen: false,
                    quake: false,
                    client_decorations: state.client_decorations(),
                },
            ),
            state,
            buttons,
        )
    }

    #[test]
    fn the_drawn_title_row_and_terminal_sit_inside_the_frame() {
        if Platform::current() != Platform::Linux {
            return;
        }
        let viewport = size(px(800.0), px(600.0));
        let state = FrameState {
            decorations: gpui::Decorations::Client {
                tiling: gpui::Tiling::default(),
            },
            maximized: false,
            fullscreen: false,
        };
        let frame = framed(state);
        assert!(frame.controls);
        assert!(frame.decorated());
        let inset = client_frame::CLIENT_INSET;
        assert_eq!(frame.inset, gpui::Edges::all(inset));
        let layout =
            ChromeLayout::with_frame(viewport, TabPosition::Titlebar, frame);
        // The content is the viewport less the border on every side.
        assert_eq!(
            layout.content,
            Bounds::new(
                point(inset, inset),
                size(px(800.0) - inset * 2.0, px(600.0) - inset * 2.0)
            )
        );
        // The row spans the content's top at the bar's height; the
        // terminal takes the rest, shrunk by the inset on every side.
        assert_eq!(
            layout.tabs,
            Bounds::new(
                point(inset, inset),
                size(px(800.0) - inset * 2.0, TAB_HEIGHT)
            )
        );
        assert_eq!(
            layout.terminal,
            Bounds::new(
                point(inset, inset + TAB_HEIGHT),
                size(
                    px(800.0) - inset * 2.0,
                    px(600.0) - inset * 2.0 - TAB_HEIGHT
                )
            )
        );
        assert_eq!(layout.tabs.bottom(), layout.terminal.top());
        // Tabs start after the small lead and stop before the window
        // controls; the menu button slot sits just before them.
        let tabs = TabsConfig {
            position: TabPosition::Titlebar,
            style: TabStyle::Strip,
            ..TabsConfig::default()
        };
        let strip = layout.strip_bounds(tabs);
        assert_eq!(strip.origin, point(inset + TITLE_ROW_LEAD, inset));
        assert_eq!(
            strip.right(),
            layout.tabs.right() - ButtonLayout::standard().trailing.width()
        );
        assert_eq!(
            layout.title_row_border(),
            Bounds::new(
                point(inset, inset + TAB_HEIGHT - px(1.0)),
                size(px(800.0) - inset * 2.0, px(1.0))
            )
        );
        // Hiding the bar frees nothing: the row is the title bar.
        let hidden =
            layout.present(Presentation::Hidden, TabPosition::Titlebar, 0.0);
        assert_eq!(hidden.terminal, layout.terminal);
        // The row is the title bar: it takes its height from the content
        // like AppKit's strip, not a reservation on top of it.
        assert_eq!(title_row_height(false, frame), TAB_HEIGHT);
        assert_eq!(
            ChromeLayout::bar_reservation(tabs, SIDEBAR_WIDTH),
            size(px(0.0), px(0.0))
        );
    }

    #[test]
    fn the_title_row_follows_the_desktop_button_layout() {
        if Platform::current() != Platform::Linux {
            return;
        }
        let viewport = size(px(800.0), px(600.0));
        let state = FrameState {
            decorations: gpui::Decorations::Client {
                tiling: gpui::Tiling::default(),
            },
            maximized: false,
            fullscreen: false,
        };
        let inset = client_frame::CLIENT_INSET;
        let tabs = TabsConfig {
            position: TabPosition::Titlebar,
            style: TabStyle::Strip,
            ..TabsConfig::default()
        };
        // Buttons at the start push the tabs after them; with none at the
        // end the strip runs to the row's end, as beside macOS's lights.
        let left = ButtonLayout::parse("close,minimize,maximize:");
        let layout = ChromeLayout::with_frame(
            viewport,
            TabPosition::Titlebar,
            framed_with(state, left),
        );
        let strip = layout.strip_bounds(tabs);
        assert_eq!(strip.left(), inset + left.leading.width());
        assert_eq!(strip.right(), layout.tabs.right());
        // Split buttons reserve both ends.
        let split = ButtonLayout::parse("close:maximize");
        let layout = ChromeLayout::with_frame(
            viewport,
            TabPosition::Titlebar,
            framed_with(state, split),
        );
        let strip = layout.strip_bounds(tabs);
        assert_eq!(strip.left(), inset + split.leading.width());
        assert_eq!(strip.right(), layout.tabs.right() - split.trailing.width());
        // No buttons keep the small lead and reserve nothing at the end.
        let layout = ChromeLayout::with_frame(
            viewport,
            TabPosition::Titlebar,
            framed_with(state, ButtonLayout::parse("appmenu:")),
        );
        let strip = layout.strip_bounds(tabs);
        assert_eq!(strip.left(), inset + TITLE_ROW_LEAD);
        assert_eq!(strip.right(), layout.tabs.right());
        // A window without the drawn row carries no buttons at all.
        let fallback = framed_with(FrameState::default(), left);
        assert_eq!(fallback, WindowFrame::default());
    }

    #[test]
    fn tiled_maximized_and_fullscreen_frames_keep_the_row_but_no_border() {
        if Platform::current() != Platform::Linux {
            return;
        }
        let viewport = size(px(800.0), px(600.0));
        let client = |tiling| gpui::Decorations::Client { tiling };
        for state in [
            FrameState {
                decorations: client(gpui::Tiling {
                    left: true,
                    ..gpui::Tiling::default()
                }),
                maximized: false,
                fullscreen: false,
            },
            FrameState {
                decorations: client(gpui::Tiling::default()),
                maximized: true,
                fullscreen: false,
            },
        ] {
            let frame = framed(state);
            assert!(frame.controls, "{state:?}");
            assert!(!frame.decorated(), "{state:?}");
            assert_eq!(frame.inset, gpui::Edges::default(), "{state:?}");
            let layout = ChromeLayout::with_frame(
                viewport,
                TabPosition::Titlebar,
                frame,
            );
            assert_eq!(
                layout.content,
                Bounds::new(point(px(0.0), px(0.0)), viewport)
            );
            assert_eq!(
                layout.tabs,
                Bounds::new(
                    point(px(0.0), px(0.0)),
                    size(px(800.0), TAB_HEIGHT)
                )
            );
            assert_eq!(layout.terminal.origin, point(px(0.0), TAB_HEIGHT));
            assert_eq!(layout.terminal.right(), px(800.0));
            assert_eq!(layout.terminal.bottom(), px(600.0));
        }
        // Fullscreen hides the row: the tabs become a top bar with no frame.
        let fullscreen = WindowFrame::resolve(
            resolve_tab_position(
                TabPosition::Titlebar,
                TabHost {
                    platform: Platform::Linux,
                    fullscreen: true,
                    quake: false,
                    client_decorations: true,
                },
            ),
            FrameState {
                decorations: client(gpui::Tiling::tiled()),
                maximized: false,
                fullscreen: true,
            },
            ButtonLayout::standard(),
        );
        assert_eq!(fullscreen, WindowFrame::default());
        assert_eq!(title_row_height(true, fullscreen), px(0.0));
        // Without a compositor the row is the window manager's.
        let fallback = framed(FrameState::default());
        assert_eq!(fallback, WindowFrame::default());
        assert_eq!(title_row_height(false, fallback), px(0.0));
        let layout =
            ChromeLayout::with_frame(viewport, TabPosition::Top, fallback);
        assert_eq!(layout.terminal.origin, point(px(0.0), TAB_HEIGHT));
    }

    #[test]
    fn initial_windows_reserve_no_height_for_the_merged_row() {
        let tabs = |position| TabsConfig {
            position,
            always_show: true,
            ..TabsConfig::default()
        };
        assert_eq!(
            ChromeLayout::bar_reservation(
                tabs(TabPosition::Titlebar),
                SIDEBAR_WIDTH
            ),
            size(px(0.0), px(0.0))
        );
        assert_eq!(
            ChromeLayout::bar_reservation(
                tabs(TabPosition::Top),
                SIDEBAR_WIDTH
            ),
            size(px(0.0), tab_bar_height(tabs(TabPosition::Top)))
        );
        assert_eq!(
            ChromeLayout::bar_reservation(
                tabs(TabPosition::Bottom),
                SIDEBAR_WIDTH
            ),
            size(px(0.0), tab_bar_height(tabs(TabPosition::Bottom)))
        );
        for column in [TabPosition::Left, TabPosition::Right] {
            assert_eq!(
                ChromeLayout::bar_reservation(tabs(column), SIDEBAR_WIDTH),
                size(SIDEBAR_WIDTH, px(0.0))
            );
        }
        // Without a title bar the configured row becomes a top bar and
        // takes its height again.
        let host = TabHost {
            platform: Platform::Linux,
            fullscreen: false,
            quake: false,
            client_decorations: false,
        };
        let resolved = layout_tabs(tabs(TabPosition::Titlebar), host);
        assert_eq!(resolved.position, TabPosition::Top);
        assert_eq!(
            ChromeLayout::bar_reservation(resolved, SIDEBAR_WIDTH).height,
            tab_bar_height(resolved)
        );
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
        use huterm_config::RefreshMode;

        let mut pacer = super::super::refresh::SnapshotPacer::default();
        let mut scroll = ScrollController::default();
        for _ in 0..100 {
            scroll.invalidate();
            assert!(
                pacer
                    .begin(&mut scroll, false, RefreshMode::Display)
                    .is_none()
            );
        }
        assert_eq!(scroll.diagnostics().requests_started, 0);
        assert!(
            pacer
                .begin(&mut scroll, true, RefreshMode::Display)
                .is_some()
        );
        assert_eq!(scroll.diagnostics().requests_started, 1);
    }

    #[test]
    fn tab_labels_degrade_across_all_metadata_modes() {
        use huterm_config::{TabDirectory, TabLabel};
        use huterm_protocol::{TerminalDirectory, TerminalMetadata};

        let empty = TerminalMetadata::default();
        assert_eq!(
            resolve_tab_label(
                TabLabel::Title,
                TabDirectory::Name,
                "shell",
                &empty,
                &[]
            ),
            "shell"
        );
        assert_eq!(
            resolve_tab_label(
                TabLabel::Process,
                TabDirectory::Name,
                "shell",
                &empty,
                &[]
            ),
            "shell"
        );
        assert_eq!(
            resolve_tab_label(
                TabLabel::Directory,
                TabDirectory::Name,
                "shell",
                &empty,
                &[]
            ),
            "shell"
        );

        let process = TerminalMetadata::new(None, Some("vim".into()));
        assert_eq!(
            resolve_tab_label(
                TabLabel::Process,
                TabDirectory::Name,
                "shell",
                &process,
                &[]
            ),
            "vim"
        );
        assert_eq!(
            resolve_tab_label(
                TabLabel::ProcessAndDirectory,
                TabDirectory::Name,
                "shell",
                &process,
                &[]
            ),
            "vim"
        );

        let directory = TerminalMetadata::new(
            Some(TerminalDirectory::new(None, "/tmp/世界".into(), false)),
            None,
        );
        assert_eq!(
            resolve_tab_label(
                TabLabel::Directory,
                TabDirectory::Name,
                "shell",
                &directory,
                &[]
            ),
            "世界"
        );
        assert_eq!(
            resolve_tab_label(
                TabLabel::ProcessAndDirectory,
                TabDirectory::Name,
                "shell",
                &directory,
                &[]
            ),
            "世界"
        );

        let both = TerminalMetadata::new(
            Some(TerminalDirectory::new(
                Some("remote".into()),
                "/work/project".into(),
                false,
            )),
            Some("cargo".into()),
        );
        assert_eq!(
            resolve_tab_label(
                TabLabel::ProcessAndDirectory,
                TabDirectory::Name,
                "shell",
                &both,
                &[]
            ),
            "cargo · project"
        );
    }

    #[test]
    fn smart_labels_follow_running_programs_and_idle_directories() {
        use huterm_config::{TabDirectory, TabLabel};
        use huterm_protocol::{TerminalDirectory, TerminalMetadata};

        let home = ["/Users/me".to_owned()];
        let project = Some(TerminalDirectory::new(
            None,
            "/Users/me/Projects/huterm".into(),
            true,
        ));
        let smart = |metadata: &TerminalMetadata, style| {
            resolve_tab_label(TabLabel::Smart, style, "shell", metadata, &home)
        };
        let idle = TerminalMetadata::new(project.clone(), None)
            .with_foreground_title(Some("me@host: ~/Projects/huterm".into()));
        assert_eq!(smart(&idle, TabDirectory::Name), "huterm");
        assert_eq!(smart(&idle, TabDirectory::Path), "~/Projects/huterm");
        let running =
            TerminalMetadata::new(project.clone(), Some("vim".into()));
        assert_eq!(smart(&running, TabDirectory::Name), "vim");
        let titled =
            running.with_foreground_title(Some("notes.txt - VIM".into()));
        assert_eq!(smart(&titled, TabDirectory::Name), "notes.txt - VIM");
        assert_eq!(
            smart(&TerminalMetadata::default(), TabDirectory::Name),
            "shell"
        );
    }

    #[test]
    fn directory_styles_format_home_relative_and_remote_paths() {
        use huterm_config::TabDirectory::{Name, Path, Short};
        use huterm_protocol::TerminalDirectory;

        let home = ["/Users/me".to_owned()];
        for (path, local, name, full, short) in [
            ("/Users/me", true, "~", "~", "~"),
            ("/Users/me/", true, "~", "~", "~"),
            (
                "/Users/me/Projects",
                true,
                "Projects",
                "~/Projects",
                "~/Projects",
            ),
            (
                "/Users/me/Projects/huterm",
                true,
                "huterm",
                "~/Projects/huterm",
                "~/P/huterm",
            ),
            (
                "/Users/me/.t3/worktrees/huterm/t3code",
                true,
                "t3code",
                "~/.t3/worktrees/huterm/t3code",
                "~/.t/w/h/t3code",
            ),
            (
                "/Users/me/Ünïcode/app",
                true,
                "app",
                "~/Ünïcode/app",
                "~/Ü/app",
            ),
            (
                "/Users/meadow",
                true,
                "meadow",
                "/Users/meadow",
                "/U/meadow",
            ),
            (
                "/usr/local/share/man",
                true,
                "man",
                "/usr/local/share/man",
                "/u/l/s/man",
            ),
            ("/", true, "/", "/", "/"),
            (
                "/Users/me/src/app",
                false,
                "app",
                "/Users/me/src/app",
                "/U/m/s/app",
            ),
        ] {
            let directory = TerminalDirectory::new(None, path.into(), local);
            let label =
                |style| directory_label(&directory, style, &home).unwrap();
            assert_eq!(
                [label(Name), label(Path), label(Short)],
                [name, full, short],
                "{path} local={local}"
            );
        }
    }

    #[test]
    fn tab_labels_show_the_local_home_directory_as_a_tilde() {
        use huterm_config::{TabDirectory, TabLabel};
        use huterm_protocol::{TerminalDirectory, TerminalMetadata};

        let home = ["/Users/me".to_owned()];
        let at_home = |path: &str, local: bool| {
            TerminalMetadata::new(
                Some(TerminalDirectory::new(None, path.into(), local)),
                Some("vim".into()),
            )
        };
        for path in ["/Users/me", "/Users/me/"] {
            assert_eq!(
                resolve_tab_label(
                    TabLabel::Directory,
                    TabDirectory::Name,
                    "shell",
                    &at_home(path, true),
                    &home
                ),
                "~"
            );
        }
        assert_eq!(
            resolve_tab_label(
                TabLabel::ProcessAndDirectory,
                TabDirectory::Name,
                "shell",
                &at_home("/Users/me", true),
                &home
            ),
            "vim · ~"
        );
        assert_eq!(
            resolve_tab_label(
                TabLabel::Directory,
                TabDirectory::Name,
                "shell",
                &at_home("/Users/me/src", true),
                &home
            ),
            "src"
        );
        assert_eq!(
            resolve_tab_label(
                TabLabel::Directory,
                TabDirectory::Name,
                "shell",
                &at_home("/Users/me", false),
                &home
            ),
            "me",
            "a remote home is not this machine's"
        );
    }

    #[test]
    fn new_tab_directory_inheritance_accepts_only_local_usable_directories() {
        use std::os::unix::fs::PermissionsExt as _;

        use huterm_config::NewTabDirectory;
        use huterm_protocol::{TerminalDirectory, TerminalMetadata};

        let root = std::env::temp_dir().join(format!(
            "huterm-directory-inheritance-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let local = TerminalMetadata::new(
            Some(TerminalDirectory::new(
                Some("localhost".into()),
                root.to_string_lossy().into_owned(),
                true,
            )),
            None,
        );
        let captured =
            inherited_directory(NewTabDirectory::Inherit, &local).unwrap();
        assert_eq!(captured, root);
        assert!(usable_launch_directory(&captured));
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o600))
            .unwrap();
        assert!(!usable_launch_directory(&captured));
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        assert_eq!(inherited_directory(NewTabDirectory::Default, &local), None);

        let remote = TerminalMetadata::new(
            Some(TerminalDirectory::new(
                Some("remote".into()),
                root.to_string_lossy().into_owned(),
                false,
            )),
            None,
        );
        assert_eq!(
            inherited_directory(NewTabDirectory::Inherit, &remote),
            None
        );
        std::fs::remove_dir(&root).unwrap();
        assert!(!usable_launch_directory(&captured));
    }
}
