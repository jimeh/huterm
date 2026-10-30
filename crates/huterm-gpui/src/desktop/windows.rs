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
use super::about::{
    AboutActions, AboutButton, AboutButtons, AboutDetails, BuildFacts,
    about_details, render_about,
};
use super::close_dialog::{
    CloseButtons, CloseDialogInput, CloseDialogTarget, ProcessGroup,
    ProcessGroupState, ProcessRow, build_close_dialog, render_close_dialog,
};
use super::config_watch::ReloadKind;
use super::licenses::{
    Done as LicensesDone, LicensesFocus, LicensesState, LicensesView,
};
use super::notices::{
    Lifetime, NoticeContent, NoticeId, NoticeSource, NoticeStack,
    ToastHandlers, render_notice_stack,
};
use super::overlay::{OverlayColors, overlay_colors, tokens};
use super::palette::{
    CommandFrequency, CommandPalette, HistoryView, OwnedHistory, PaletteEvent,
    PaletteHierarchy, PaletteOpen, PaletteScope, PaletteTarget,
    QuakeProfileRow, RecentCommands,
};
use super::*;
use crate::commands::{
    RenameContext, Route, fill_rename_target, route, select_tab_slot,
};
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
    HierarchySubscription, HostEffectRecipientOptions, HostEffectViewerOptions,
    MuxError, OpenedPane, OpenedTab, RestoreDirection, TerminalViewer,
    ViewerOptions, ViewerWake,
};
use huterm_protocol::{
    AttachmentId, CommandArgument, CommandScope, HierarchyState, NodeId,
    PaneId, PaneInfo, PaneLayout, RuntimeId, SessionId, SplitAxis,
    SplitDirection, SplitId, SplitNode, TerminalId, Touched,
    ViewerCapabilities, WorkspaceId, catalog, resolve_tab_name, validate,
    validate_supplied,
};
use huterm_ui::components::button::{Button, Colors as ButtonColors};
use huterm_ui::components::dialog::Modal;
use huterm_ui::components::input::{ContextMenu as FieldContextMenu, Input};
use huterm_ui::components::menu::{
    MIN_WIDTH as MENU_MIN_WIDTH, Menu as MenuView, MenuEvent,
};
use huterm_ui::components::popover::{Anchor, Popover, place};
use huterm_ui::listbox::{Model as MenuModel, Row as MenuRow};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const TAB_HEIGHT: Pixels = px(32.0);
/// The smallest pane a split or resize leaves, in cells.
const MIN_PANE_COLUMNS: u16 = 4;
const MIN_PANE_ROWS: u16 = 2;

/// A divider's grab zone element ID, unique within its tab.
fn grab_id(divider: &Divider) -> gpui::SharedString {
    format!("divider-{}-{}", divider.split.get(), divider.index).into()
}

/// Whether each pane of `after` keeps the minimum size it had in `before`
/// on each axis, or, where it was already smaller, does not shrink.
fn keeps_minimum(
    before: &[(PaneId, GridSize)],
    after: &[(PaneId, GridSize)],
) -> bool {
    after.iter().all(|(pane, grid)| {
        let start = before
            .iter()
            .find_map(|(id, grid)| (id == pane).then_some(*grid))
            .unwrap_or(*grid);
        let keeps = |now: u16, was: u16, minimum: u16| now >= minimum.min(was);
        keeps(grid.columns, start.columns, MIN_PANE_COLUMNS)
            && keeps(grid.rows, start.rows, MIN_PANE_ROWS)
    })
}

/// Whether a close decides only once the projection reaches the hierarchy
/// core assessed it at: a refused emptied close decides from batches up to
/// that hierarchy, so none that arrived during the assessment is lost, and a
/// pane close whose pane moved finds the pane's new tab.
fn close_waits_for_projection<T>(
    checking: &CloseTarget,
    result: &Result<T, MuxError>,
) -> bool {
    matches!(
        (checking, result),
        (CloseTarget::Emptied, Err(MuxError::ViewNotEmpty(_)))
            | (
                CloseTarget::Pane { .. },
                Err(MuxError::UnknownPane(_) | MuxError::UnknownTab(_))
            )
    )
}

/// Whether `session` holds a pane other than `spawned`, which another
/// client moved in while a spawn's publication was pending.
fn holds_other_panes(mux: &Mux, session: SessionId, spawned: PaneId) -> bool {
    mux.session(session).is_some_and(|record| {
        record.workspaces.iter().any(|workspace| {
            mux.workspace(*workspace).is_some_and(|workspace| {
                workspace.tabs.iter().any(|tab| {
                    tab.layout()
                        .panes()
                        .into_iter()
                        .any(|pane| pane.id != spawned)
                })
            })
        })
    })
}

/// Closes an unpublished spawn's own pane when it is still in `session`,
/// leaving the rest of the session to the panes moved into it.
fn close_spawned_pane(mux: &mut Mux, session: SessionId, pane: PaneId) {
    if let Ok(target) = mux.select_pane(pane)
        && target.session == session
        && let (Some(workspace), Some(tab)) = (target.workspace, target.tab)
    {
        let _ = mux.close_pane(workspace, tab, pane);
    }
}

/// Whether every grid is at least the minimum pane size.
fn fits_minimum(grids: &[(PaneId, GridSize)]) -> bool {
    grids.iter().all(|(_, grid)| {
        grid.columns >= MIN_PANE_COLUMNS && grid.rows >= MIN_PANE_ROWS
    })
}
pub(super) const SIDEBAR_WIDTH: Pixels = px(180.0);
#[cfg(target_os = "macos")]
mod appkit_menu;
#[cfg(target_os = "macos")]
pub(super) use appkit_menu::check_window_menu as check_native_window_menu;
mod button_layout;
mod client_frame;
mod exit_closes;
mod field_menu;
mod layout_history;
mod layout_undo;
mod menu_spec;
mod model;
mod native_menus;
mod pane_drag;
mod pane_geometry;
mod pane_layouts;
mod pane_menu;
mod pane_moves;
mod projection;
mod settings_window;
mod tab_bar;
mod tab_menu;
mod tab_position;
mod tab_strip;
pub(super) mod tab_visibility;
mod terminal_menu;
mod view_install;
mod views;
mod window_menu;
#[cfg(all(test, feature = "test-support"))]
mod worker_hooks;
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
use exit_closes::{CommitOutcome, ExitCloses};
use menu_spec::MenuSpec;
use model::{
    PaneTitle, QuakeRecord, TabEntry, TitleScope, WindowLayout, WindowModel,
    WindowRecord, WindowRestore, apply_tab_order,
};
use native_menus::{MenuAnchor, MenuPresenter, NativeMenuSession};
use pane_drag::{DropTarget, PaneDrag, PaneMode};
use pane_geometry::{
    Divider, PaneRect, TabPanes, boundary_position, dragged_weights,
    layout_panes, with_boundary,
};
use pane_menu::{PaneMenuInput, pane_menu};
use projection::{
    Drained, Install, PendingInstalls, Projection, ReconcileTarget, Resolution,
    TabNames, TitleConsumers, Wait, WindowChange, holds_no_tabs,
    installed_order, reactivation, reconcile_window, tab_names,
    windows_to_visit,
};
use tab_bar::{
    Activity, PILL_HEIGHT, PILL_INSET, PILL_MARGIN_LEFT, PILL_MARGIN_RIGHT,
    TabColors, TabItem, TabStatus, VERTICAL_ROW_MARGIN_X,
    VERTICAL_ROW_MARGIN_Y, icon_element, tab_bar_height, top_chrome_uses_bar,
};
use tab_menu::{TabMenuInput, tab_menu};
use tab_position::{TabHost, resolve_tab_position};
use tab_strip::{TabExtents, TabStrip};
use tab_visibility::{Presentation, Reveal};
use terminal_menu::{DirectoryState, TerminalMenuInput, terminal_menu};
use views::{Views, broadcast, update_windows};
use window_menu::{
    MenuButtonPlacement, WindowMenuInput, menu_button_placement,
    split_new_tab_row, window_menu,
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
    #[cfg(all(test, feature = "test-support"))]
    worker_hooks: worker_hooks::WorkerHooks,
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
    TerminalViewer,
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
                let viewer_attachment = created_attachment.or(attachment);
                let viewer = viewer_attachment
                    .ok_or(MuxError::Runtime(RuntimeError::Invariant(
                        "a spawn into an existing workspace needs its attachment",
                    )))
                    .and_then(|viewer_attachment| {
                        mux.subscribe_terminal(
                            viewer_attachment,
                            tab.tab.first_pane().terminal_id,
                            ViewerOptions {
                                geometry: Some((
                                    command.grid_size,
                                    command.cell_size,
                                )),
                                spawned: true,
                                ..self.viewer_options(
                                    clipboard_allowed,
                                    command.presentation.clone(),
                                )
                            },
                        )
                    });
                match viewer {
                    Ok(viewer) => {
                        Ok((session, id, tab, created_attachment, viewer))
                    }
                    Err(error) => {
                        if workspace.is_none() {
                            let _ = mux.close_session(session);
                        } else {
                            let _ = mux.close_tab(id, tab.tab.id);
                        }
                        Err(error)
                    }
                }
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

    /// Splits `pane` of `tab` toward `direction` with a new terminal running
    /// `command`, and subscribes the window's viewer of it under the same
    /// lock. A viewer that cannot subscribe closes the new pane again.
    #[expect(
        clippy::too_many_arguments,
        reason = "a split names its window, pane, side, and new terminal"
    )]
    fn split_pane(
        &self,
        workspace: WorkspaceId,
        attachment: AttachmentId,
        tab: TabId,
        pane: PaneId,
        direction: SplitDirection,
        command: &TerminalCommand,
        clipboard_allowed: bool,
    ) -> Committed<Result<(OpenedPane, TerminalViewer), MuxError>> {
        let mut mux = self.lock();
        let result = if self.terminating.load(Ordering::Acquire) {
            Err(RuntimeError::Stopped.into())
        } else {
            mux.split_pane(workspace, tab, pane, direction, command)
                .and_then(|opened| {
                    let viewer = mux.subscribe_terminal(
                        attachment,
                        opened.pane.terminal_id,
                        ViewerOptions {
                            geometry: Some((
                                command.grid_size,
                                command.cell_size,
                            )),
                            spawned: true,
                            ..self.viewer_options(
                                clipboard_allowed,
                                command.presentation.clone(),
                            )
                        },
                    );
                    match viewer {
                        Ok(viewer) => Ok((opened, viewer)),
                        Err(error) => {
                            let _ =
                                mux.close_pane(workspace, tab, opened.pane.id);
                            Err(error)
                        }
                    }
                })
        };
        Committed {
            result,
            seq: mux.hierarchy_seq(),
        }
    }

    /// Sets the weights of `split`'s children, which must still be
    /// `children`, as a divider drag or resize command commits them.
    fn resize_split(
        &self,
        workspace: WorkspaceId,
        tab: TabId,
        preview: &ResizePreview,
    ) -> Committed<Result<(), MuxError>> {
        let mut mux = self.lock();
        let result = mux.resize_split(
            workspace,
            tab,
            preview.split,
            &preview.children,
            &preview.weights,
        );
        Committed {
            result,
            seq: mux.hierarchy_seq(),
        }
    }

    /// Sets or clears `tab`'s shared zoom.
    fn set_pane_zoom(
        &self,
        workspace: WorkspaceId,
        tab: TabId,
        zoomed: Option<PaneId>,
    ) -> Result<(), MuxError> {
        self.lock().set_pane_zoom(workspace, tab, zoomed)
    }

    /// The options a desktop view's viewer starts with: full capabilities,
    /// host effects under the window's clipboard policy, and the view's
    /// presentation.
    fn viewer_options(
        &self,
        clipboard_allowed: bool,
        presentation: TerminalPresentation,
    ) -> ViewerOptions {
        ViewerOptions {
            host_effects: Some(HostEffectViewerOptions {
                process: self.host_effects.clone(),
                options: HostEffectRecipientOptions::local_desktop(
                    clipboard_allowed,
                ),
            }),
            presentation: Some(presentation),
            ..ViewerOptions::new(ViewerCapabilities::ALL)
        }
    }

    /// Subscribes a viewer for each of a window's pane targets under one
    /// lock, for views the window did not create and for revoked views it
    /// replaces. Each viewer starts with its target's [`ViewerStart`];
    /// results follow `targets` in order.
    fn subscribe_panes(
        &self,
        attachment: AttachmentId,
        targets: &[view_install::PaneTarget],
        clipboard_allowed: bool,
    ) -> Committed<Vec<Result<TerminalViewer, MuxError>>> {
        let mux = self.lock();
        let terminating = self.terminating.load(Ordering::Acquire);
        let result = targets
            .iter()
            .map(|target| {
                if terminating {
                    Err(RuntimeError::Stopped.into())
                } else {
                    mux.subscribe_terminal(
                        attachment,
                        target.terminal,
                        ViewerOptions {
                            geometry: target.start.geometry,
                            ..self.viewer_options(
                                clipboard_allowed,
                                target.start.presentation.clone(),
                            )
                        },
                    )
                }
            })
            .collect();
        Committed {
            result,
            seq: mux.hierarchy_seq(),
        }
    }

    /// Removes an unpublished spawn's resources; returns the sequence the
    /// cleanup committed at. `pane` is the pane the spawn created.
    fn cleanup_spawn(
        &self,
        original_session: SessionId,
        workspace: WorkspaceId,
        tab: TabId,
        pane: PaneId,
        attachment: Option<AttachmentId>,
    ) -> u64 {
        let mut mux = self.lock();
        // Never tear down a resource another attachment has adopted, including
        // a tab moved to another session while its UI publication was pending,
        // or a pane another client moved in.
        if let Some(attachment) = attachment {
            let _ = mux.detach_session(attachment);
            if mux.session(original_session).is_some() {
                let session = original_session;
                let snapshot = mux.capture_hierarchy();
                if !snapshot.attachments.iter().any(|(_, id)| *id == session) {
                    if holds_other_panes(&mux, session, pane) {
                        close_spawned_pane(&mut mux, session, pane);
                    } else {
                        let _ = mux.close_session(session);
                    }
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
                // A spawned pane that moved elsewhere stays; panes moved
                // into its tab survive the cleanup.
                let layout = mux.tab(tab).map(|tab| tab.layout().clone());
                match layout {
                    Some(layout) if layout.pane(pane).is_none() => {}
                    Some(layout) if layout.pane_count() > 1 => {
                        let _ = mux.close_pane(workspace, tab, pane);
                    }
                    _ => {
                        let _ = mux.close_tab(workspace, tab);
                    }
                }
            }
        }
        mux.hierarchy_seq()
    }

    /// Prepares `request` under the lock, then checks jobs outside it.
    fn assess(
        &self,
        request: CloseRequest,
    ) -> Committed<Result<CloseAssessment, MuxError>> {
        let (ticket, seq) = {
            let mux = self.lock();
            (mux.prepare_close(request), mux.hierarchy_seq())
        };
        Committed {
            result: ticket.map(|ticket| ticket.check_jobs()),
            seq,
        }
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
        let assessment = self.assess(CloseRequest::Application).result?;
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
    /// What is wrong with the active configuration. New windows raise its
    /// persistent notices, Settings spells it out, and every reload
    /// replaces it.
    config_report: ConfigReport,
    /// Failures reported while no window could show them, such as a startup
    /// hotkey conflict. The next window to open or become active takes and
    /// raises them; a reload clears the ones still waiting.
    latched: Vec<NoticeContent>,
    /// Client-owned facts about every window; read window facts here.
    windows: WindowModel,
    /// Exit-closes owed to tabs whose shell exited under `close_on_exit`,
    /// which the close pipeline of one window showing the tab takes.
    exit_closes: ExitCloses,
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
    /// The reload latch and its follow-up flag.
    reloads: config_watch::Reloads,
    /// The configuration watcher's worker; `None` until startup runs it.
    watch: Option<config_watch::Handle>,
    /// The watcher's last acknowledged state, for smoke state.
    #[cfg(target_os = "macos")]
    watch_status: WatchStatus,
    quitting: bool,
    /// Set once Quit is approved. A reload that finishes later applies
    /// nothing, so it cannot register global shortcuts again.
    quit_committed: bool,
    pending_spawns: usize,
    quit_pending: bool,
    /// The Settings window, which is not in `windows`: it shows no session.
    settings: Option<gpui::WindowHandle<settings_window::SettingsView>>,
    external_drag_window: Option<gpui::WindowId>,
    /// The window buttons the desktop asks title rows to draw.
    button_layout: ButtonLayout,
    /// Shows native menus; `None` where windows draw every menu themselves.
    menu_presenter: Option<Rc<dyn MenuPresenter>>,
    /// Close failures reported in any window, including windows since
    /// removed.
    #[cfg(test)]
    close_failures: usize,
    #[cfg(all(target_os = "macos", feature = "macos-updater"))]
    updater: native_updater::Updater,
}
impl Global for Desktop {}

/// What the configuration watcher last acknowledged. Only the macOS input
/// smoke reads it; the Linux smoke observes reloads through the terminal.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Default)]
struct WatchStatus {
    watching: bool,
    reconciliations: u64,
}

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

/// Raises `message` as a command failure in `view`'s window. Returns false
/// for a window that cannot show it.
fn show_failure_in(
    cx: &mut App,
    view: &Entity<WorkspaceView>,
    message: &str,
) -> bool {
    view.update(cx, |view, cx| {
        if !view.can_show_notices(cx) {
            return false;
        }
        view.report_failure("Command failed", message, cx);
        true
    })
}

/// Raises `message` as a command failure in the active window. Returns
/// whether a window showed it.
fn show_active_window_failure(cx: &mut App, message: &str) -> bool {
    if settings_window::show_failure(cx, message) {
        return true;
    }
    let Some(window) = cx.active_window() else {
        return false;
    };
    window
        .update(cx, |root, _, cx| {
            root.downcast::<WorkspaceView>()
                .is_ok_and(|view| show_failure_in(cx, &view, message))
        })
        .unwrap_or(false)
}

/// Raises `message` as a command failure in the reporter's window, or in the
/// active window when the reporter is absent, closed, or cannot show it.
/// Returns whether a window showed it.
fn show_failure(
    cx: &mut App,
    reporter: Option<WeakEntity<WorkspaceView>>,
    message: &str,
) -> bool {
    reporter
        .and_then(|reporter| reporter.upgrade())
        .is_some_and(|reporter| show_failure_in(cx, &reporter, message))
        || show_active_window_failure(cx, message)
}

/// Records a failure for the next window to open or become active. Repeats
/// of a waiting message collapse.
fn latch_failure(cx: &mut App, message: &str) {
    let latched = &mut cx.global_mut::<Desktop>().latched;
    if !latched.iter().any(|content| content.message == message) {
        latched.push(NoticeContent::command_failure("Command failed", message));
    }
}

/// Takes `message` back out of the latch. Returns false when it is no longer
/// waiting there.
fn unlatch_failure(cx: &mut App, message: &str) -> bool {
    let latched = &mut cx.global_mut::<Desktop>().latched;
    let before = latched.len();
    latched.retain(|content| content.message != message);
    latched.len() != before
}

fn report_deferred_failure(
    cx: &mut App,
    reporter: Option<WeakEntity<WorkspaceView>>,
    message: String,
) {
    eprintln!("Huterm {message}");
    cx.defer(move |cx| {
        show_failure(cx, reporter, &message);
    });
}

/// Reports a failure that one window must show. It waits in the desktop
/// latch, for the next window to open or become active, only while no
/// existing window can show it.
fn report_deferred_failure_with_global_latch(
    cx: &mut App,
    reporter: Option<WeakEntity<WorkspaceView>>,
    message: String,
) {
    eprintln!("Huterm {message}");
    // Without a reporter the failure waits in the latch now, so a window
    // created before the deferred step below still raises it.
    let waiting = reporter.is_none();
    if waiting {
        latch_failure(cx, &message);
    }
    cx.defer(move |cx| {
        if waiting && !unlatch_failure(cx, &message) {
            // A new window already took it.
            return;
        }
        if !show_failure(cx, reporter, &message) {
            latch_failure(cx, &message);
        }
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
        ids::RELOAD_CONFIG => Ok(reload(cx)),
        ids::OPEN_SETTINGS => {
            // Global actions run while the dispatching window is borrowed,
            // and opening or activating a window updates another.
            cx.defer(settings_window::open);
            Ok(CommandOutcome::Accepted)
        }
        ids::OPEN_CONFIG_FILE => {
            cx.defer(open_config_file);
            Ok(CommandOutcome::Accepted)
        }
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

/// Opens `config.toml` in the system editor, creating it from the default
/// template first.
fn open_config_file(cx: &mut App) {
    let path = cx.global::<Desktop>().config_path.clone();
    match config::create_default(&path) {
        Ok(()) => cx.open_with_system(&path),
        Err(error) => {
            show_active_window_failure(
                cx,
                &format!("Cannot open {}: {error}", path.display()),
            );
        }
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

/// Why this build cannot check for updates, if it cannot: Sparkle failed
/// to start, or the build has no updater.
#[cfg_attr(
    not(all(target_os = "macos", feature = "macos-updater")),
    expect(
        clippy::unnecessary_wraps,
        reason = "only updater-enabled builds can check, so only they return None"
    )
)]
pub(super) fn update_check_refusal(cx: &App) -> Option<String> {
    #[cfg(all(target_os = "macos", feature = "macos-updater"))]
    {
        cx.global::<Desktop>()
            .updater
            .startup_error()
            .map(str::to_owned)
    }
    #[cfg(not(all(target_os = "macos", feature = "macos-updater")))]
    {
        let _ = cx;
        match unsupported_update_error() {
            CommandError::Unavailable(reason) => Some(reason),
            error => Some(error.to_string()),
        }
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
        ids::QUIT if desktop.quitting => Err(CommandError::Unavailable(
            "application quit is already in progress".to_owned(),
        )),
        ids::NEW_WINDOW
        | ids::SHOW_QUAKE
        | ids::HIDE_QUAKE
        | ids::TOGGLE_QUAKE
        | ids::RELOAD_CONFIG
        | ids::OPEN_SETTINGS
        | ids::OPEN_CONFIG_FILE
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
    let prompt = spec.scope != CommandScope::Focused
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

/// What is wrong with the active configuration, kept in one place so each
/// terminal window's persistent notices and the Settings notice report the
/// same problems however late the window opens.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct ConfigReport {
    /// The file failed at startup, so the defaults apply. Settings reads
    /// the file itself and shows its own banner for this.
    error: Option<String>,
    /// A binding failed at startup, so the default bindings apply.
    keymap_error: Option<String>,
    /// One message per binding conflict.
    conflicts: Vec<String>,
    warning: Option<String>,
    /// One message per global shortcut the platform refused.
    refused: Vec<String>,
    /// The last reload failed, so the previous settings are still active.
    reload_failure: Option<String>,
}

impl ConfigReport {
    /// The report for a reload that applied `config`, with the global
    /// shortcuts the platform `refused`.
    fn reloaded(
        config: &Config,
        compiled: &CompiledKeymap,
        refused: Vec<String>,
    ) -> Self {
        if let Some(warning) = &config.warning {
            eprintln!("Huterm configuration warning: {warning}");
        }
        let mut report = Self {
            conflicts: compiled.conflicts.clone(),
            warning: config.warning.clone(),
            ..Self::default()
        };
        report.refuse(refused);
        report
    }

    /// Records the global shortcuts the platform refused.
    fn refuse(&mut self, refused: Vec<String>) {
        if !refused.is_empty() {
            eprintln!("Huterm global shortcut warning: {}", refused.join("\n"));
        }
        self.refused = refused;
    }

    /// The persistent notices in precedence order: config error, keymap
    /// error, conflicts, warning, then refused shortcuts. A reload failure
    /// is not among them: it shows in the windows open when it happened,
    /// through [`failed_reload_diagnostics`].
    pub(super) fn notices(&self, path: &Path) -> Vec<NoticeContent> {
        let mut notices = config_diagnostics(
            path,
            self.error.as_deref(),
            self.keymap_error.as_deref(),
            &self.conflicts,
            self.warning.as_deref(),
        );
        notices.extend(refused_shortcuts_diagnostic(path, &self.refused));
        notices
    }

    /// What Settings reports: why the settings did not apply, or each
    /// thing the applied file asks for that the desktop could not honour.
    pub(super) fn for_settings(&self) -> Result<Vec<String>, String> {
        match self.reload_failure.as_ref().or(self.keymap_error.as_ref()) {
            Some(error) => Err(error.clone()),
            None => Ok(self
                .conflicts
                .iter()
                .chain(&self.warning)
                .chain(&self.refused)
                .cloned()
                .collect()),
        }
    }
}

/// Binds the startup keymap and returns its reserved keys with the
/// startup configuration's report.
///
/// A broken binding never blocks startup: defaults apply and the report
/// carries it like any other non-fatal configuration error.
pub(super) fn install_startup_keymap(
    cx: &mut App,
    loaded: &config::LoadedConfig,
) -> (InstalledKeymap, ConfigReport) {
    let (compiled, keymap_error) = compile_keymap(&loaded.config);
    let report = ConfigReport {
        error: loaded.error.clone(),
        keymap_error,
        conflicts: compiled.conflicts.clone(),
        warning: loaded.warning.clone(),
        ..ConfigReport::default()
    };
    let keymap = bind_keymap(cx, compiled);
    (keymap, report)
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
    let open_file = || bare(ids::OPEN_CONFIG_FILE);
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
            .action("Open Config File", open_file())
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
            .action("Open Config File", open_file()),
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
            .action("Open Config File", open_file()),
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
            .action("Open Config File", open_file()),
        );
    }
    diagnostics
}

/// The notice for global shortcuts the platform refused, if any. Reload
/// tries them again, such as after another application releases one.
fn refused_shortcuts_diagnostic(
    path: &Path,
    refused: &[String],
) -> Option<NoticeContent> {
    if refused.is_empty() {
        return None;
    }
    let message = refused.join("\n");
    Some(
        NoticeContent::diagnostic(
            Severity::Warning,
            NoticeSource::Keymap,
            "Global shortcuts unavailable",
            message,
        )
        .location(path.display().to_string())
        .action("Open Config File", bare(ids::OPEN_CONFIG_FILE))
        .action("Reload", bare(ids::RELOAD_CONFIG)),
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
            format!(
                "Config reload failed: {}\nPrevious settings are still active.",
                error.trim_end()
            ),
        )
        .action("Open Config File", bare(ids::OPEN_CONFIG_FILE))
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
    let inputs = loaded.inputs.clone();
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
        install_desktop(cx, Arc::clone(&app_runtime), hierarchy, loaded);
        start_config_watch(cx, inputs);
        #[cfg(target_os = "linux")]
        follow_button_layout(cx);
        install_native_quit(cx);
        let refused = quake_windows::install(cx);
        cx.global_mut::<Desktop>().config_report.refuse(refused);
        cx.on_app_quit(move |cx| {
            commit_quit(cx);
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
                cx.defer(move |cx| {
                    show_active_window_failure(cx, &message);
                });
            }
        });
        open_window(cx);
        cx.activate(true);
        startup(cx);
    });
    // Backends whose event loop returns get the same idempotent cleanup.
    runtime.terminate()?;
    Ok(())
}

/// Installs the `Desktop` global over `runtime` and its hierarchy
/// projection, starts the projection's drain, forgets windows as they
/// close, and routes keystrokes to the active terminal.
fn install_desktop(
    cx: &mut App,
    runtime: Arc<DesktopRuntime>,
    hierarchy: Projection,
    loaded: config::LoadedConfig,
) {
    let (keymap, config_report) = install_startup_keymap(cx, &loaded);
    #[cfg(all(target_os = "macos", feature = "macos-updater"))]
    let updater = native_updater::Updater::initialize(loaded.config.updates);
    cx.set_global(Desktop {
        quake: quake_windows::Registry::default(),
        runtime,
        config: loaded.config,
        config_report,
        latched: Vec::new(),
        config_path: loaded.path,
        windows: WindowModel::default(),
        exit_closes: ExitCloses::default(),
        hierarchy,
        title_consumers: TitleConsumers::default(),
        views: Views::default(),
        keymap,
        frequency: CommandFrequency::default(),
        reloads: config_watch::Reloads::default(),
        watch: None,
        #[cfg(target_os = "macos")]
        watch_status: WatchStatus::default(),
        quitting: false,
        quit_committed: false,
        pending_spawns: 0,
        settings: None,
        quit_pending: false,
        external_drag_window: None,
        button_layout: ButtonLayout::standard(),
        #[cfg(target_os = "macos")]
        menu_presenter: Some(Rc::new(appkit_menu::AppKitPresenter)),
        #[cfg(not(target_os = "macos"))]
        menu_presenter: None,
        #[cfg(test)]
        close_failures: 0,
        #[cfg(all(target_os = "macos", feature = "macos-updater"))]
        updater,
    });
    start_hierarchy_drain(cx);
    cx.on_window_closed(|cx, window| {
        let desktop = cx.global_mut::<Desktop>();
        if desktop
            .settings
            .is_some_and(|settings| settings.window_id() == window)
        {
            desktop.settings = None;
        }
        desktop.windows.remove(window);
        // A close the window took and had not started committing never
        // reached the runtime.
        if desktop.exit_closes.release(window).is_some() {
            offer_exit_closes(cx, Some(window));
        }
        let desktop = cx.global_mut::<Desktop>();
        desktop.title_consumers.set_host(window, false);
        desktop.views.prune();
        // The closed window's titles left the model; Quit dialogs and
        // palettes elsewhere still list them until they refresh.
        mark_titles_changed(window, cx);
        maybe_exit(cx);
    })
    .detach();
    cx.observe_keystrokes(observe_keystroke).detach();
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
/// refreshes title consumers. Views it installs arrive later, from each
/// window's install completion.
fn reconcile(cx: &mut App, touched: &Touched) {
    if touched.is_empty() {
        return;
    }
    let desktop = cx.global_mut::<Desktop>();
    let state = desktop.hierarchy.state();
    desktop
        .exit_closes
        .retain(|pane| state.pane_tab(pane).is_some());
    let pruned = desktop.windows.prune_layout_histories(state);
    let desktop = cx.global::<Desktop>();
    let terminating = desktop.runtime.terminating.load(Ordering::Acquire);
    let windows = windows_to_visit(&desktop.windows, touched);
    let mut retitled = HashSet::new();
    let mut activate = Vec::new();
    update_windows(cx, &windows, |view, cx| {
        let before = view.active_pane(cx);
        let change = view.reconcile(touched, terminating, cx);
        if let Some(tab) = reactivation(before, view.active_pane(cx)) {
            activate.push((view.window, tab, true));
        } else if change.adopted
            && let Some(tab) = view.active_tab(cx)
        {
            // A pane view that changed tabs takes its new tab's visibility;
            // keyboard focus stays where it is.
            activate.push((view.window, tab, false));
        }
        if change.titles {
            retitled.insert(view.window);
        }
        // A resize or shared zoom elsewhere changes what this window draws
        // and the grids its panes request.
        if change.changed
            || view.tabs.iter().any(|tab| touched.contains_layout(tab.id))
        {
            cx.notify();
        }
    });
    // A window whose history lost an entry refreshes its open palette and
    // menu, which may offer the entry.
    update_windows(cx, &pruned, |_, cx| cx.notify());
    if !activate.is_empty() {
        // Selecting needs the window, which reconcile does not have here;
        // a deferred update runs once no window is on GPUI's update stack.
        cx.defer(move |cx| activate_reconciled_tabs(cx, &activate));
    }
    refresh_title_consumers(cx, true, &retitled);
}

/// Shows the tabs reconcile made active by removing the active tab or its
/// focused pane, as `reconcile_own` does for a window's own completion, and
/// focuses them when `refocus` is set,
/// unless an overlay holds the window's focus. A window whose active tab
/// changed again meanwhile is left alone.
fn activate_reconciled_tabs(
    cx: &mut App,
    tabs: &[(gpui::WindowId, TabId, bool)],
) {
    for &(id, tab, refocus) in tabs {
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
                    let focus = refocus && !view.overlay_holds_focus();
                    view.measure_tab_widths(window, cx);
                    view.select_tab(tab, focus, window, cx);
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

/// The window that hosts Quit's assessment: the active window, else the
/// first open one, skipping windows that are `closing`. A closing window's
/// removal is already scheduled and would drop the Quit with it; the
/// platform can also still name a window removed this turn as active.
fn quit_host<H: Copy + PartialEq>(
    active: Option<H>,
    windows: &[H],
    closing: impl Fn(H) -> bool,
) -> Option<H> {
    let eligible = |handle: &H| windows.contains(handle) && !closing(*handle);
    active
        .filter(eligible)
        .or_else(|| windows.iter().copied().find(eligible))
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
    let desktop = cx.global::<Desktop>();
    // Only a terminal window can host the confirmation; the Settings
    // window has no record.
    let host = quit_host(cx.active_window(), &cx.windows(), |handle| {
        desktop
            .windows
            .record(handle.window_id())
            .is_none_or(|record| record.closing)
    });
    if let Some(handle) = host {
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
        // The Settings window cannot host, so it does not count.
        let desktop = cx.global::<Desktop>();
        let hosts = cx
            .windows()
            .iter()
            .any(|handle| desktop.windows.record(handle.window_id()).is_some());
        if hosts {
            cx.defer(request_quit);
        } else {
            #[cfg(target_os = "macos")]
            native_quit::cancel_request();
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

/// Marks Quit committed and stops the configuration watcher. Dropping the
/// handle only sends the worker a message; the worker's own thread drops
/// the platform watcher.
fn commit_quit(cx: &mut App) {
    let desktop = cx.global_mut::<Desktop>();
    desktop.quit_committed = true;
    desktop.watch = None;
}

fn approved_quit(cx: &mut App) {
    commit_quit(cx);
    cx.spawn(async move |cx| {
        let mut quake_presentations = Vec::new();
        #[cfg(target_os = "macos")]
        let mut adapters = Vec::new();
        cx.update(|cx| {
            settings_window::close(cx);
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
                host.platform,
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
                #[cfg(test)]
                decorations_override: None,
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
                divider_drag: None,
                resize_commits: Vec::new(),
                resize_commit_counter: 0,
                layout_queue: None,
                arrangements: 0,
                drawn_dividers: RefCell::new(Vec::new()),
                drawn_panes: RefCell::new(None),
                drawn_drop: RefCell::new(None),
                pane_drag: None,
                reveal: Reveal::default(),
                reveal_context: None,
                pointer_reveal: PointerReveal::default(),
                layout_pending: false,
                synced_presentation: None,
                reorder: None,
                config,
                family,
                metrics: scaled_metrics,
                focus: cx.focus_handle(),
                busy: false,
                installs: PendingInstalls::default(),
                restore_focus: None,
                close: CloseState::default(),
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
                native_menu: None,
                native_menu_token: 0,
                menu_button_bounds: Rc::new(Cell::new(None)),
                popover: Popover::new(),
                window_button_bounds: Rc::default(),
                title_row_press: Rc::new(Cell::new(false)),
                title_row_moves: Rc::new(Cell::new(0)),
                about: None,
                about_buttons: AboutButtons::new(cx),
                about_initial: AboutButton::default(),
                licenses: None,
                close_buttons: CloseButtons::new(cx),
                dialog: Modal::new(),
                dialog_shown: None,
                // The window opened with this title. Writing it again from
                // the first render costs GPUI's X11 backend a blocking round
                // trip that can queue the window's MapNotify where the event
                // loop never sees it, so no later frame is ever requested.
                window_title: window_title(None),
                drawn_dialog_groups: None,
            });
            view.update(cx, |view, cx| {
                view.frame_clock.observe(cx);
                // A field's menu over Licenses reaches past its panel.
                view.dialog.beneath(&view.popover);
                // A quake window whose setup failed is removed below while
                // still hidden, so it leaves the latched failures waiting.
                if !profile_requested || view.quake.is_some() {
                    view.raise_desktop_notices(cx);
                }
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
                    view.prune_divider_drag(cx);
                    view.reclamp_divider_drag(window, cx);
                    view.sync_zoom(window, cx);
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
                    if !window.is_window_active() {
                        view.cancel_divider_drag(cx);
                        view.cancel_reorder(window, cx);
                        view.cancel_pane_drag(window, cx);
                        // An open menu closes, as AppKit menus and GTK
                        // popovers do, and focus is on the terminal when
                        // the window comes back.
                        view.close_menu(MenuFocusReturn::Terminal, window, cx);
                    }
                    view.refresh_tab_visibility(window, cx);
                    // A failure latched while every window was hidden or
                    // inactive belongs to the first one the user returns to.
                    view.raise_latched_failures_if_active(window, cx);
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
                    && !drag.nesting
                    && drag
                        .strip
                        .autoscroll(drag.pointer, Duration::from_millis(16))
                        != self.tab_scroll
            })
            || self
                .pane_drag
                .as_ref()
                .is_some_and(|drag| drag.scrolls(self.tab_scroll))
        {
            next = next.merge(AnimationSchedule::FRAME);
        }
        // A pane drag resting on a label waits on one deadline and draws
        // no frames.
        if let Some(deadline) = self
            .pane_drag
            .as_ref()
            .and_then(|drag| drag.spring.deadline())
        {
            next = next.merge(AnimationSchedule::at(deadline));
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
        let scrolled = self.advance_tab_scroll(now, window);
        if scrolled {
            self.rearm_after_scroll(now, window, cx);
        }
        if scrolled | self.tab_scrollbars.advance(now) | self.reveal.tick(now) {
            self.sync_tab_layout(window, cx);
            cx.notify();
        }
        self.advance_spring(now, window, cx);
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

    /// Raises a close failure notice.
    fn report_close_failure(
        &mut self,
        message: String,
        cx: &mut Context<'_, Self>,
    ) {
        #[cfg(test)]
        {
            cx.global_mut::<Desktop>().close_failures += 1;
        }
        self.report_failure("Close", message, cx);
    }

    /// Raises `content` and repaints.
    fn notify(&mut self, content: NoticeContent, cx: &mut Context<'_, Self>) {
        self.notices.push(content, Instant::now());
        cx.notify();
    }

    /// Whether a notice raised here reaches the user. A closing window is
    /// being removed, and a hidden Quake window stays off screen until it is
    /// summoned. The platform can still name either as the active window.
    fn can_show_notices(&self, cx: &App) -> bool {
        let closing = cx
            .global::<Desktop>()
            .windows
            .record(self.window)
            .is_some_and(|record| record.closing);
        !closing && self.quake_visible()
    }

    /// Raises the desktop's configuration diagnostics and latched failures
    /// in a new window.
    fn raise_desktop_notices(&mut self, cx: &mut Context<'_, Self>) {
        let desktop = cx.global::<Desktop>();
        let diagnostics = desktop.config_report.notices(&desktop.config_path);
        self.notices
            .replace_diagnostics(&diagnostics, Instant::now());
        self.raise_latched_failures(cx);
        cx.notify();
    }

    /// Takes the failures latched while no window could show them and raises
    /// them here, so each is shown once.
    fn raise_latched_failures(&mut self, cx: &mut Context<'_, Self>) {
        let latched = std::mem::take(&mut cx.global_mut::<Desktop>().latched);
        if latched.is_empty() {
            return;
        }
        let now = Instant::now();
        for content in latched {
            self.notices.push(content, now);
        }
        cx.notify();
    }

    /// Raises the latched failures here when the user is looking at this
    /// window: it is active and can show notices.
    fn raise_latched_failures_if_active(
        &mut self,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        if window.is_window_active() && self.can_show_notices(cx) {
            self.raise_latched_failures(cx);
        }
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
                    tokens(&self.config.theme),
                    &handlers,
                )),
        )
    }
}

/// Weights this window shows for one split before the projection holds
/// them: a divider drag's, or a committed resize's until the projection
/// reaches the commit.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ResizePreview {
    tab: TabId,
    split: SplitId,
    /// The split's children the weights were computed for, in order.
    children: Vec<NodeId>,
    weights: Vec<u32>,
}

/// A divider this window is dragging. It holds the dragged boundary, not
/// the split's weights, so a change elsewhere to the split's other
/// boundaries carries through the drag.
#[derive(Clone, Debug)]
struct DividerDrag {
    tab: TabId,
    split: SplitId,
    /// The split's children when the drag began, in order.
    children: Vec<NodeId>,
    /// The boundary after this child index.
    index: usize,
    axis: SplitAxis,
    /// The split's bounds as last drawn.
    span: Bounds<Pixels>,
    /// Where the boundary sits: the sum of the weights through `index`.
    boundary: u32,
}

/// The identities of one pane: its tab, itself, and its terminal.
#[derive(Clone, Copy, Debug)]
struct PaneKey {
    tab: TabId,
    pane: PaneId,
    terminal: TerminalId,
}

/// One pane of a tab as this window shows it.
struct PaneView {
    pane: PaneId,
    terminal: TerminalId,
    /// The hierarchy sequence the pane's view was built at. Reconcile drops
    /// the view only once the projection has applied it and no longer holds
    /// the pane.
    committed: u64,
    view: Entity<TerminalView>,
    /// The pane's activity drain; dropping it cancels the drain.
    activity_task: Task<()>,
}

impl PaneView {
    /// Replaces the pane's activity drain, cancelling the old one.
    fn restart_activity(&mut self, task: Task<()>) {
        drop(std::mem::replace(&mut self.activity_task, task));
    }

    /// The pane's title without status text: its custom name, else what
    /// its tab would show if this were its only pane, through the tab label
    /// settings but without the tab's custom name.
    fn label(
        &self,
        custom: Option<&str>,
        fallback: &str,
        tabs: huterm_config::TabsConfig,
        cx: &App,
    ) -> String {
        if let Some(custom) = custom {
            return custom.to_owned();
        }
        let terminal = self.view.read(cx);
        resolve_tab_label(
            tabs.label,
            tabs.directory,
            resolve_tab_name(None, &terminal.title, fallback),
            &terminal.metadata,
            home_paths(),
        )
    }

    fn status(&self, cx: &App) -> TabStatus {
        let terminal = self.view.read(cx);
        TabStatus::new(
            terminal.exited,
            terminal.failed,
            terminal.bell.unseen,
            terminal.metadata.foreground_process(),
        )
    }
}

struct TabView {
    id: TabId,
    /// The hierarchy sequence the tab's first view was built at. Reconcile
    /// drops the tab only once the projection has applied it and no longer
    /// holds the tab.
    committed: u64,
    /// The names the projection last held for this tab, used while it no
    /// longer does, such as during a close commit.
    names: TabNames,
    /// The views of the tab's panes; never empty.
    panes: Vec<PaneView>,
    /// Panes this window focused, most recent first. The first one present
    /// has keyboard focus whenever the tab is active.
    focus_history: Vec<PaneId>,
    /// The pane this window zooms under `[panes] zoom = "view"`.
    zoomed: Option<PaneId>,
    /// The zoom this view last applied to focus and visibility, from either
    /// scope, so a change made elsewhere is noticed once.
    applied_zoom: Option<PaneId>,
}

impl TabView {
    fn new(
        id: TabId,
        committed: u64,
        names: TabNames,
        panes: Vec<PaneView>,
    ) -> Self {
        debug_assert!(!panes.is_empty(), "a tab view needs a pane");
        let focus_history =
            panes.first().map(|pane| pane.pane).into_iter().collect();
        Self {
            id,
            committed,
            names,
            panes,
            focus_history,
            zoomed: None,
            applied_zoom: None,
        }
    }

    /// The pane with keyboard focus: the most recently focused one still
    /// present, else the first.
    fn focused(&self) -> &PaneView {
        self.focus_history
            .iter()
            .find_map(|id| self.pane(*id))
            .unwrap_or(&self.panes[0])
    }

    fn focused_view(&self) -> &Entity<TerminalView> {
        &self.focused().view
    }

    fn pane(&self, id: PaneId) -> Option<&PaneView> {
        self.panes.iter().find(|pane| pane.pane == id)
    }

    fn pane_mut(&mut self, id: PaneId) -> Option<&mut PaneView> {
        self.panes.iter_mut().find(|pane| pane.pane == id)
    }

    /// Every pane's view.
    fn views(&self) -> impl Iterator<Item = &Entity<TerminalView>> {
        self.panes.iter().map(|pane| &pane.view)
    }

    /// The pane `focus_pane_*` toward `direction` moves to from `pane`, in
    /// the projected layout; ties go to the pane this window focused most
    /// recently. A target this window has no view of yet, such as a pane
    /// still installing, counts as none. The pane key contexts ask the same
    /// question, so a binding they enable always moves focus.
    fn neighbor(
        &self,
        pane: PaneId,
        direction: SplitDirection,
        cx: &App,
    ) -> Option<PaneId> {
        cx.global::<Desktop>()
            .hierarchy
            .state()
            .tab(self.id)?
            .layout
            .neighbor(pane, direction, &self.focus_history)
            .filter(|next| self.pane(*next).is_some())
    }

    /// Moves focus to `pane`; returns whether it changed.
    fn focus_pane(&mut self, pane: PaneId) -> bool {
        if self.pane(pane).is_none() || self.focused().pane == pane {
            return false;
        }
        self.focus_history.retain(|id| *id != pane);
        self.focus_history.insert(0, pane);
        true
    }

    /// Drops the views of `panes`. Focus falls back to the most recently
    /// focused pane still present, else the first in layout order, which
    /// joins the history so a pane added before it later cannot take focus.
    fn remove_panes(&mut self, panes: &[PaneId]) {
        self.panes.retain(|view| !panes.contains(&view.pane));
        self.focus_history.retain(|id| !panes.contains(id));
        if self.zoomed.is_some_and(|zoomed| panes.contains(&zoomed)) {
            self.zoomed = None;
        }
        if let Some(first) = self.panes.first()
            && !self.focus_history.iter().any(|id| self.pane(*id).is_some())
        {
            self.focus_history.push(first.pane);
        }
    }

    /// The tab's label with status text, for previews, dialogs, and the
    /// window title.
    fn title(&self, tabs: huterm_config::TabsConfig, cx: &App) -> String {
        let (title, status) = self.label(tabs, cx);
        with_status_suffix(title, status)
    }

    /// Each pane's own title for the window model, which close dialogs
    /// read for their headings.
    fn pane_titles(
        &self,
        tabs: huterm_config::TabsConfig,
        cx: &App,
    ) -> Vec<PaneTitle> {
        self.panes
            .iter()
            .map(|pane| PaneTitle {
                terminal: pane.terminal,
                title: self.pane_label(pane, tabs, cx),
            })
            .collect()
    }

    /// The custom and launched program names of `pane`, from the
    /// projection while it holds the tab, else no custom name and the tab's
    /// remembered fallback name.
    fn pane_names<'a>(
        &'a self,
        pane: PaneId,
        cx: &'a App,
    ) -> (Option<&'a str>, &'a str) {
        let state = cx.global::<Desktop>().hierarchy.state();
        state
            .tab(self.id)
            .and_then(|info| info.layout.pane(pane))
            .map_or_else(
                || (None, tab_names(state, self.id, &self.names).1),
                |pane| {
                    (pane.custom_name.as_deref(), pane.fallback_name.as_str())
                },
            )
    }

    /// The title of `pane`, one of this tab's, without status text.
    fn pane_label(
        &self,
        pane: &PaneView,
        tabs: huterm_config::TabsConfig,
        cx: &App,
    ) -> String {
        let (custom, fallback) = self.pane_names(pane.pane, cx);
        pane.label(custom, fallback, tabs, cx)
    }

    /// Returns the display name without status text, and the status its
    /// indicator reports. The name is the tab's custom name, else the
    /// focused pane's title, which is that pane's custom name when it has
    /// one; the status is the focused pane's, except that
    /// a failure or bell in any pane shows. Names come from the hierarchy
    /// projection by identity; a tab the projection no longer holds keeps
    /// the names it last held.
    fn label(
        &self,
        tabs: huterm_config::TabsConfig,
        cx: &App,
    ) -> (String, TabStatus) {
        let focused = self.focused();
        let (custom, _) = tab_names(
            cx.global::<Desktop>().hierarchy.state(),
            self.id,
            &self.names,
        );
        let label = custom
            .map_or_else(|| self.pane_label(focused, tabs, cx), str::to_owned);
        let status = focused.status(cx);
        let others = self
            .panes
            .iter()
            .filter(|pane| pane.pane != focused.pane)
            .map(|pane| pane.status(cx));
        (label, attention_status(status, others))
    }
}

/// A split tab's status: the focused pane's, raised to a failure or a bell
/// that any other pane shows, because both ask for attention.
fn attention_status(
    focused: TabStatus,
    others: impl Iterator<Item = TabStatus>,
) -> TabStatus {
    others.fold(focused, |status, other| match (status, other) {
        (TabStatus::Failed, _) | (_, TabStatus::Failed) => TabStatus::Failed,
        (TabStatus::Exited, _) => TabStatus::Exited,
        (_, TabStatus::Bell) => TabStatus::Bell,
        (status, _) => status,
    })
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
    for part in parts.iter_mut().take(last) {
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
    /// Decorations `sync_frame` takes in place of GPUI's: the test window
    /// always reports server-side ones. While set, the frame also resolves
    /// as on Linux, so the title row Huterm draws can be tested on macOS.
    #[cfg(test)]
    decorations_override: Option<gpui::Decorations>,
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
    divider_drag: Option<DividerDrag>,
    /// Resizes this window committed, shown until the projection holds
    /// them, keyed by a per-window counter.
    resize_commits: Vec<(u64, ResizePreview)>,
    resize_commit_counter: u64,
    /// Feeds this window's resize and arrangement commits to one worker in
    /// order, so a later command never lands before an earlier one.
    layout_queue: Option<
        async_channel::Sender<(u64, WorkspaceId, pane_layouts::LayoutCommit)>,
    >,
    /// Arrangements this window queued whose completion has not run. While
    /// any is in flight, structural commands and divider presses are
    /// refused, as `busy` refuses them, but terminal input still flows.
    arrangements: usize,
    /// The active tab's dividers as the last frame drew them, which
    /// presses hit-test against.
    drawn_dividers: RefCell<Vec<Divider>>,
    /// The active tab's pane rectangles as the last frame drew them, which
    /// pane and tab drags hit-test against.
    drawn_panes: RefCell<Option<(TabId, Vec<PaneRect>)>>,
    /// The drop target the last frame drew, for smoke state.
    drawn_drop: RefCell<Option<String>>,
    /// A press on a pane title bar, or the drag it became.
    pane_drag: Option<PaneDrag>,
    reveal: Reveal,
    reveal_context: Option<(TabPosition, bool, bool)>,
    pointer_reveal: PointerReveal,
    layout_pending: bool,
    /// The tab bar presentation the last layout sync pushed, so a sync can
    /// tell the bar appearing or hiding from a resize.
    synced_presentation: Option<Presentation>,
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
    /// Views this window is installing for tabs it did not spawn.
    installs: PendingInstalls,
    /// The pane an undo or redo focuses once its view installs, when the
    /// restore brought it back into this window without one.
    restore_focus: Option<PaneId>,
    close: CloseState,
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
    /// The open native menu, which shows instead of `menu` on macOS.
    native_menu: Option<NativeMenuSession>,
    /// Names each native menu, so a late pick from a closed one is ignored.
    native_menu_token: u64,
    /// Where the open menu draws, and the guard that closes it on a press
    /// outside its painted content. Its content bounds are smoke state's
    /// `menu_rect`.
    popover: Popover,
    /// Where the menu button was last painted, for anchoring the menu and
    /// for smoke state; `None` while the button is not drawn.
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
    /// The About panel's details while it is showing.
    about: Option<AboutDetails>,
    about_buttons: AboutButtons,
    /// The About panel button that takes focus when the panel shows: Close,
    /// or Licenses after returning from the Licenses view.
    about_initial: AboutButton,
    /// The Licenses view, open over the About panel it returns to.
    licenses: Option<Entity<LicensesView>>,
    close_buttons: CloseButtons,
    /// The window's dialog slot, which the top dialog draws in: the close
    /// confirmation, the Licenses view, or the About panel.
    dialog: Modal,
    /// The dialog the slot was last opened for, so a change of top dialog
    /// opens it again and moves focus into the new one.
    dialog_shown: Option<ShownDialog>,
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
    /// The context menu of a pane's title bar in `tab`.
    Pane { tab: TabId, pane: PaneId },
    /// A text field's context menu, in the palette or the Licenses view.
    Field(WeakEntity<Input>),
}

impl MenuKind {
    /// The tab the menu's tab-targeted picks act on.
    fn tab(&self) -> Option<TabId> {
        match self {
            Self::Window | Self::Field(_) => None,
            Self::Tab(tab)
            | Self::Terminal { tab, .. }
            | Self::Pane { tab, .. } => Some(*tab),
        }
    }

    /// The pane the menu's pane-targeted picks act on.
    fn pane(&self) -> Option<PaneId> {
        match self {
            Self::Pane { pane, .. } => Some(*pane),
            Self::Window
            | Self::Tab(_)
            | Self::Terminal { .. }
            | Self::Field(_) => None,
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
    /// Closes the menu when focus leaves it; dropping the menu drops it.
    _blur: gpui::Subscription,
}

/// The open menu's scroll indicator fades on the window's frame clock.
impl refresh::Animated for MenuView {
    fn animation_schedule(&self, now: Instant) -> AnimationSchedule {
        self.schedule(now)
    }

    fn advance_animation(
        &mut self,
        now: Instant,
        _frame: bool,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.advance(now, cx);
    }
}

/// The palette's and the Licenses view's lists fade their scroll
/// indicators on the window's frame clock too.
impl refresh::Animated for huterm_ui::components::command::Command {
    fn animation_schedule(&self, now: Instant) -> AnimationSchedule {
        self.schedule(now)
    }

    fn advance_animation(
        &mut self,
        now: Instant,
        _frame: bool,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.advance(now, cx);
    }
}

/// The dialog the window's dialog slot shows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShownDialog {
    /// The close confirmation of this [`CloseState::generation`]: a new
    /// confirmation moves the generation, so it opens on its primary
    /// button again.
    Close(u64),
    Licenses,
    About,
}

/// Where focus goes when the window menu closes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MenuFocusReturn {
    /// The active terminal, after Escape, a pick, an outside press, or a
    /// command. Returning to the menu button would leave terminal bindings
    /// inactive until the user clicked back into the terminal. A field's
    /// menu returns to its field instead.
    Terminal,
    /// Whatever takes focus next, such as a close confirmation.
    Keep,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PaletteRefreshState {
    tabs: usize,
    active: Option<TabId>,
    busy: bool,
    confirming: bool,
    reordering: bool,
    quake: Option<(bool, bool)>,
    /// What undo and redo would restore, which pruning can change while
    /// the palette is open.
    restorable: [Option<gpui::SharedString>; 2],
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
    /// Pane mode: the pointer is over a pane of the active tab, deep enough
    /// past the bar that a release nests the tab instead of reordering.
    nesting: bool,
    /// The pane zone a release in pane mode nests into; a centre offers
    /// none.
    nest: Option<DropTarget>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CloseTarget {
    Tab(TabId),
    /// One pane of a tab. The runtime closes the tab instead when the pane
    /// is its last.
    Pane {
        tab: TabId,
        pane: PaneId,
    },
    /// Several tabs of this window, distinct and in window order. Always
    /// two or more: [`tabs_target`] normalizes shorter lists.
    Tabs(Vec<TabId>),
    Window,
    Application,
    /// The automatic close of a window whose last tab view went away. It
    /// stops no terminal, so it never prompts.
    Emptied,
}

impl CloseTarget {
    fn tabs(&self) -> Option<Vec<TabId>> {
        match self {
            Self::Tab(id) => Some(vec![*id]),
            Self::Tabs(ids) => Some(ids.clone()),
            Self::Pane { .. }
            | Self::Window
            | Self::Application
            | Self::Emptied => None,
        }
    }

    /// Whether this target closes `pane` of `tab` by naming it or its tab.
    /// A window close may only detach, so it never counts.
    fn names_pane(&self, tab: TabId, pane: PaneId) -> bool {
        match self {
            Self::Pane { pane: closed, .. } => *closed == pane,
            target => target.tabs().is_some_and(|tabs| tabs.contains(&tab)),
        }
    }

    /// Whether committing this target also closes everything `other`
    /// would.
    fn covers(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Application, _) => true,
            (Self::Window, other) => *other != Self::Application,
            (_, Self::Pane { tab, .. }) => {
                self == other
                    || self.tabs().is_some_and(|tabs| tabs.contains(tab))
            }
            (_, other) => match (self.tabs(), other.tabs()) {
                (Some(wide), Some(narrow)) => {
                    narrow.iter().all(|tab| wide.contains(tab))
                }
                _ => self == other,
            },
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

/// How a commit ended for the exit-close of `pane`, in `tab`, it carried.
fn exit_commit_outcome(
    result: &Result<(), MuxError>,
    target: &CloseTarget,
    tab: TabId,
    pane: PaneId,
) -> CommitOutcome {
    if close_commit_retries(result) {
        CommitOutcome::Retry
    } else if result.is_err() {
        CommitOutcome::Failed
    } else if target.names_pane(tab, pane) {
        CommitOutcome::Closed
    } else {
        CommitOutcome::NotReached
    }
}

/// Records how the commit that carried `pane`'s exit-close ended, whether
/// or not `window`, which started it, is still open.
fn finish_exit_commit(
    cx: &mut App,
    window: gpui::WindowId,
    pane: PaneId,
    outcome: CommitOutcome,
) {
    let desktop = cx.global_mut::<Desktop>();
    // A closing window's view still assesses a refused commit again; its
    // removal returns the close to owed.
    let window_open = desktop.windows.record(window).is_some();
    if desktop
        .exit_closes
        .finish_commit(pane, outcome, window_open)
    {
        offer_exit_closes(cx, Some(window));
    }
}

/// Offers the owed exit-closes, once no window is being updated, to the
/// windows other than `except` that show their tabs; the first whose close
/// pipeline is idle takes each. Never reads another window's view from
/// inside an update.
fn offer_exit_closes(cx: &mut App, except: Option<gpui::WindowId>) {
    cx.defer(move |cx| {
        if terminating(cx) {
            return;
        }
        let desktop = cx.global::<Desktop>();
        let state = desktop.hierarchy.state();
        let owed: Vec<TabId> = desktop
            .exit_closes
            .owed()
            .filter_map(|pane| state.pane_tab(pane))
            .collect();
        let windows: Vec<gpui::WindowId> = desktop
            .windows
            .records()
            .filter(|record| {
                Some(record.id) != except
                    && record.tabs.iter().any(|entry| owed.contains(&entry.id))
            })
            .map(|record| record.id)
            .collect();
        for handle in cx.windows() {
            if !windows.contains(&handle.window_id()) {
                continue;
            }
            let _ = handle.update(cx, |root, window, cx| {
                if let Ok(view) = root.downcast::<WorkspaceView>() {
                    view.update(cx, |view, cx| {
                        view.resume_close(window, cx);
                    });
                }
            });
        }
    });
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
        // A pane close joins no union: a tab-level target that covers its
        // tab absorbs it, and otherwise the newer request runs while
        // `CloseState::checked` keeps the other queued.
        (Some(pending), requested)
            if matches!(pending, CloseTarget::Pane { .. })
                || matches!(requested, CloseTarget::Pane { .. }) =>
        {
            if pending.covers(&requested) {
                pending
            } else {
                requested
            }
        }
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
        .and_then(|index| order.get(index + 1..))
        .map(<[TabId]>::to_vec)
        .unwrap_or_default()
}

/// Whether a window owes the emptied close of [`CloseTarget::Emptied`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum EmptiedClose {
    #[default]
    None,
    /// The window's last view went away: the next idle pipeline requests
    /// the close, and it stays owed until the close reaches an outcome.
    Owed,
    /// The runtime refused the close while the window's workspace held
    /// tabs; a later batch or install may owe it again.
    Refused,
}

#[derive(Default)]
struct CloseState {
    current: Option<CloseTarget>,
    pending: Option<CloseTarget>,
    confirmation: Option<CloseTarget>,
    assessment: Option<CloseAssessment>,
    generation: u64,
    emptied: EmptiedClose,
    /// Clicks the Confirm button's listener received, stale or not.
    #[cfg(test)]
    confirm_clicks: usize,
}

#[derive(Debug, Eq, PartialEq)]
enum CloseDecision {
    Check(CloseTarget),
    Confirm(CloseTarget),
    Close(CloseTarget),
}

impl CloseState {
    /// The next close to start once no operation is busy and no
    /// confirmation shows: a queued request first, then an exit-close that
    /// `take_exit` takes, then an owed emptied close.
    fn next_request(
        &mut self,
        busy: bool,
        contains: impl Fn(TabId) -> bool,
        take_exit: impl FnOnce() -> Option<CloseTarget>,
    ) -> Option<CloseTarget> {
        if busy || self.confirmation.is_some() {
            return None;
        }
        self.take_pending(&contains).or_else(take_exit).or_else(|| {
            (self.emptied == EmptiedClose::Owed).then_some(CloseTarget::Emptied)
        })
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
        // A second, different tab or pane close follows the first; wider
        // requests subsume narrower requests and repeated closes of one tab
        // or pane coalesce.
        if let Some(queued) = &pending
            && matches!(
                (&target, queued),
                (CloseTarget::Tab(_) | CloseTarget::Pane { .. }, _)
                    | (_, CloseTarget::Pane { .. })
            )
            && !matches!(queued, CloseTarget::Emptied)
            && !target.covers(queued)
        {
            self.pending = pending;
        }
        if foreground {
            self.confirmation = Some(target.clone());
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
    /// Whether a Confirm button drawn for `target` at `generation` may
    /// commit: only while that same confirmation still shows. Every cancel
    /// and every new check moves the generation, so a click dispatched
    /// against an older frame reaches neither a cancelled dialog nor a
    /// replacement asking the same question.
    fn can_confirm(&self, target: &CloseTarget, generation: u64) -> bool {
        self.generation == generation
            && self.confirmation.as_ref() == Some(target)
    }
    /// Ends the showing confirmation and its check without committing,
    /// keeping queued closes.
    fn dismiss_confirmation(&mut self) {
        self.generation += 1;
        self.assessment = None;
        self.confirmation = None;
        self.current = None;
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
            // A pane close follows its pane, which may have moved to
            // another tab; the request resolves its current tab.
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

    /// The session of this window's attachment; `None` without one.
    fn attached_session(&self, cx: &App) -> Option<SessionId> {
        self.record(cx)
            .and_then(|record| record.attachment.and(record.session))
    }

    /// Whether an overlay holds keyboard focus: the palette, a close
    /// confirmation, a menu, or the About panel. A tab selected behind it
    /// is shown and snapshotted without taking focus.
    fn overlay_holds_focus(&self) -> bool {
        self.palette.is_some()
            || self.close.confirmation.is_some()
            || self.menu.is_some()
            || self.about.is_some()
    }

    /// Appends a tab's view and model entry, activating a spawned tab, then
    /// applies the installed order, so a tab the projection placed earlier
    /// lands in its position.
    fn push_tab_view(&mut self, tab: TabView, spawned: bool, cx: &mut App) {
        let entry = TabEntry {
            id: tab.id,
            panes: tab.pane_titles(self.config.tabs, cx),
            title: tab.title(self.config.tabs, cx),
        };
        self.tabs.push(tab);
        self.close.emptied = EmptiedClose::None;
        let windows = &mut cx.global_mut::<Desktop>().windows;
        if spawned {
            windows.open_tab(self.window, entry);
        } else {
            windows.add_tab(self.window, entry);
        }
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
        let active = self.active_pane(cx);
        let change = self.reconcile(&Touched::default(), terminating(cx), cx);
        if !change.changed {
            return;
        }
        if let Some(next) = reactivation(active, self.active_pane(cx)) {
            self.measure_tab_widths(window, cx);
            self.select(next, window, cx);
        } else if change.adopted
            && let Some(tab) = self.active_tab(cx)
        {
            // A pane view that changed tabs takes its new tab's visibility.
            self.measure_tab_widths(window, cx);
            self.select_tab(tab, false, window, cx);
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
        let change = reconcile_window(
            &mut WindowReconcile { view: self, cx },
            touched,
            terminating,
        );
        if !terminating {
            self.note_emptied(change.emptied, touched, cx);
        }
        change
    }

    /// Owes the emptied close when reconcile took the window's last view,
    /// or when a batch that touched the window's workspace found its
    /// attached session gone or left a refused emptied close nothing to
    /// install. It only marks the window: the idle close pipeline requests
    /// the close, so reconcile never removes a window itself.
    fn note_emptied(
        &mut self,
        emptied: bool,
        touched: &Touched,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(record) = self
            .record(cx)
            .filter(|record| !record.closing && record.attachment.is_some())
        else {
            return;
        };
        let batch = record
            .workspace
            .is_some_and(|workspace| touched.contains_workspace(workspace));
        if emptied
            || (batch
                && (self.attached_session_gone(cx)
                    || self.emptied_close_may_retry(cx)))
        {
            self.owe_emptied_close(cx);
        }
    }

    /// Whether a refused emptied close may run again.
    fn emptied_close_may_retry(&self, cx: &App) -> bool {
        self.close.emptied == EmptiedClose::Refused
            && self.has_nothing_to_install(cx)
    }

    /// Whether the window shows no view and the projection holds no tab it
    /// would install, so no install is in flight either.
    fn has_nothing_to_install(&self, cx: &App) -> bool {
        self.tabs.is_empty()
            && holds_no_tabs(
                cx.global::<Desktop>().hierarchy.state(),
                self.workspace_id(cx),
                self.attached_session(cx),
            )
    }

    /// Whether the projection no longer holds the attached session.
    fn attached_session_gone(&self, cx: &App) -> bool {
        self.attached_session(cx).is_some_and(|session| {
            cx.global::<Desktop>()
                .hierarchy
                .state()
                .session(session)
                .is_none()
        })
    }

    /// Marks the emptied close owed; the notification runs the close
    /// pipeline, which takes it once idle.
    fn owe_emptied_close(&mut self, cx: &mut Context<'_, Self>) {
        if self.close.emptied != EmptiedClose::Owed && !terminating(cx) {
            self.close.emptied = EmptiedClose::Owed;
            cx.notify();
        }
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

    /// Shows the session of `attachment`, which this window did not
    /// create, in `workspace`. Attaching emits no hierarchy event, so the
    /// window runs its own full reconcile, which installs the views.
    fn adopt_attachment(
        &mut self,
        attachment: AttachmentId,
        session: SessionId,
        workspace: WorkspaceId,
        cx: &mut Context<'_, Self>,
    ) {
        cx.global_mut::<Desktop>().windows.attach(
            self.window,
            Some(attachment),
            session,
            workspace,
        );
        if self
            .reconcile(&Touched::everything(), terminating(cx), cx)
            .changed
        {
            cx.notify();
        }
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

    /// Each divider's line and the grab zone over it, which shows the resize
    /// cursor and claims a press to start a drag.
    fn divider_elements(
        dividers: &[Divider],
        colors: TabColors,
        cx: &mut Context<'_, Self>,
    ) -> Vec<gpui::AnyElement> {
        let mut elements = Vec::new();
        for divider in dividers {
            let line = divider.line;
            elements.push(
                div()
                    .absolute()
                    .left(line.origin.x)
                    .top(line.origin.y)
                    .w(line.size.width)
                    .h(line.size.height)
                    .bg(colors.border)
                    .into_any_element(),
            );
            // Occludes the panes beneath, so no terminal sees a press here
            // and starts a selection or mouse report, while overlays drawn
            // later keep theirs.
            let grab = divider.grab;
            let grabbed = *divider;
            elements.push(
                div()
                    .id(grab_id(divider))
                    .absolute()
                    .left(grab.origin.x)
                    .top(grab.origin.y)
                    .w(grab.size.width)
                    .h(grab.size.height)
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _, cx| {
                            cx.stop_propagation();
                            if view.divider_drag.is_none()
                                && view.divider_press_allowed()
                            {
                                view.start_divider_drag(grabbed, cx);
                            }
                        }),
                    )
                    .cursor(match divider.axis {
                        SplitAxis::Columns => {
                            gpui::CursorStyle::ResizeLeftRight
                        }
                        SplitAxis::Rows => gpui::CursorStyle::ResizeUpDown,
                    })
                    .into_any_element(),
            );
        }
        elements
    }

    /// The active tab's panes: each pane's terminal view at its rectangle
    /// inside `bounds`, a title bar per pane in a split tab, and the
    /// dividers between panes.
    fn pane_elements(
        &self,
        bounds: Bounds<Pixels>,
        colors: TabColors,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> Vec<gpui::AnyElement> {
        let Some(tab) = self.active_tab_view(cx) else {
            self.drawn_dividers.borrow_mut().clear();
            *self.drawn_panes.borrow_mut() = None;
            return Vec::new();
        };
        let tab_id = tab.id;
        // A layout this window shows none of yet, as when a busy window
        // defers removals, is drawn as if the projection held no layout.
        let zoomed = self.shown_zoom(tab, cx);
        let rects = self.tab_panes(tab_id, bounds, zoomed, window, cx).filter(
            |rects| {
                tab.panes.iter().any(|pane| rects.pane(pane.pane).is_some())
            },
        );
        let focused = tab.focused().pane;
        *self.drawn_dividers.borrow_mut() = rects
            .as_ref()
            .map(|rects| rects.dividers.clone())
            .unwrap_or_default();
        *self.drawn_panes.borrow_mut() =
            rects.as_ref().map(|rects| (tab_id, rects.panes.clone()));
        let mut elements = Vec::new();
        for pane in &tab.panes {
            // A view the layout no longer holds, such as one still closing,
            // is not drawn. A tab without a layout to draw shows only its
            // focused pane, across the whole bounds.
            let rect = match &rects {
                Some(rects) => match rects.pane(pane.pane) {
                    Some(rect) => Some(rect),
                    None => continue,
                },
                None if pane.pane == focused => None,
                None => continue,
            };
            let terminal = rect.map_or(bounds, |rect| rect.terminal);
            let id = pane.pane;
            elements.push(
                div()
                    .id(("pane", id.get()))
                    .absolute()
                    .left(terminal.origin.x)
                    .top(terminal.origin.y)
                    .w(terminal.size.width)
                    .h(terminal.size.height)
                    .overflow_hidden()
                    // Before the terminal's own press handling, which keeps
                    // GPUI's focus tracking from focusing it.
                    .capture_any_mouse_down(cx.listener(
                        move |view, _: &gpui::MouseDownEvent, window, cx| {
                            view.focus_pane(tab_id, id, window, cx);
                        },
                    ))
                    .child(pane.view.clone())
                    .into_any_element(),
            );
            if let Some(title_bar) = rect.and_then(|rect| rect.title_bar) {
                let title = tab.pane_label(pane, self.config.tabs, cx);
                elements.push(
                    Self::pane_title_bar(
                        tab_bar::PaneTitleBar {
                            tab: tab_id,
                            pane: id,
                            bounds: title_bar,
                            title,
                            status: pane.status(cx),
                            focused: id == focused,
                            zoomed: zoomed == Some(id),
                        },
                        colors,
                        cx,
                    )
                    .into_any_element(),
                );
            }
        }
        if let Some(rects) = &rects {
            elements.extend(Self::divider_elements(
                &rects.dividers,
                colors,
                cx,
            ));
        }
        elements
    }

    /// Gives `pane` of `tab` keyboard focus in this window: the tab's
    /// focused pane becomes `pane`, and GPUI focus moves to its view when
    /// the tab is active and no overlay holds focus.
    fn focus_pane(
        &mut self,
        tab: TabId,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(view) = self.tabs.iter_mut().find(|view| view.id == tab)
        else {
            return;
        };
        let changed = view.focus_pane(pane);
        if self.active_tab(cx) == Some(tab) && !self.overlay_holds_focus() {
            self.restore_tab_focus(window, cx);
        }
        if changed {
            self.publish_active_title(tab, cx);
            cx.notify();
        }
    }

    /// Applies zoom changes from either scope once: a newly zoomed pane
    /// takes its tab's focus, since a hidden pane cannot hold it, and the
    /// active tab's panes show or hide. Moving GPUI focus inside an
    /// inactive window reports no focus to the terminal, so a zoom set
    /// elsewhere never takes size control from the active window.
    fn sync_zoom(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let zooms: Vec<(TabId, Option<PaneId>)> = self
            .tabs
            .iter()
            .map(|tab| (tab.id, self.shown_zoom(tab, cx)))
            .collect();
        let active = self.active_tab(cx);
        let mut reselect = false;
        for (id, zoomed) in zooms {
            let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id)
            else {
                continue;
            };
            if tab.applied_zoom == zoomed {
                continue;
            }
            tab.applied_zoom = zoomed;
            if let Some(zoomed) = zoomed {
                tab.focus_pane(zoomed);
            }
            reselect |= active == Some(id);
            if self
                .divider_drag
                .as_ref()
                .is_some_and(|drag| drag.tab == id)
            {
                self.divider_drag = None;
                cx.notify();
            }
        }
        if let Some(active) = active.filter(|_| reselect) {
            let focus = !self.overlay_holds_focus();
            self.select_tab(active, focus, window, cx);
            self.publish_active_title(active, cx);
            cx.notify();
        }
    }

    /// Zooms `pane` of `tab`, or unzooms it when it is the zoomed pane,
    /// as the title bar's toggle does.
    fn toggle_zoom_of(
        &mut self,
        tab: TabId,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        // A zoom set while an arrangement is in flight would hide the
        // layout it lands, or, shared, could land first and be cleared.
        if self.arrangements > 0 {
            return;
        }
        let zoomed = self
            .tabs
            .iter()
            .find(|view| view.id == tab)
            .and_then(|view| self.shown_zoom(view, cx));
        let next = (zoomed != Some(pane)).then_some(pane);
        if let Some(view) = self.tabs.iter_mut().find(|view| view.id == tab) {
            view.focus_pane(pane);
        }
        self.set_zoom(tab, next, window, cx);
    }

    /// Sets `tab`'s zoom in the configured scope. A shared zoom reaches
    /// this window through the hierarchy, like any other window's.
    fn set_zoom(
        &mut self,
        tab: TabId,
        zoomed: Option<PaneId>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.cancel_divider_drag(cx);
        match self.config.panes.zoom {
            huterm_config::PaneZoomScope::View => {
                if let Some(view) =
                    self.tabs.iter_mut().find(|view| view.id == tab)
                {
                    view.zoomed = zoomed;
                }
                self.sync_zoom(window, cx);
            }
            huterm_config::PaneZoomScope::Shared => {
                let Some(workspace) = self.workspace_id(cx) else {
                    return;
                };
                let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
                let work = cx.background_executor().spawn(async move {
                    runtime.set_pane_zoom(workspace, tab, zoomed)
                });
                cx.spawn_in(window, async move |view, cx| {
                    if let Err(error) = work.await {
                        let _ = view.update(cx, |view, cx| {
                            if !terminating(cx) {
                                view.report_failure(
                                    "Zoom pane",
                                    format!("Cannot zoom pane: {error}"),
                                    cx,
                                );
                            }
                        });
                    }
                })
                .detach();
            }
        }
    }

    /// Unzooms `tab` before navigation or a split, as tmux does.
    fn unzoom(
        &mut self,
        tab: TabId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let zoomed = self
            .tabs
            .iter()
            .find(|view| view.id == tab)
            .and_then(|view| self.shown_zoom(view, cx));
        if zoomed.is_some() {
            self.set_zoom(tab, None, window, cx);
        }
    }

    /// Whether a press may grab a divider: no overlay or modal claims it,
    /// and no arrangement is about to replace the weights it would drag.
    fn divider_press_allowed(&self) -> bool {
        self.arrangements == 0
            && self.menu.is_none()
            && self.palette.is_none()
            && self.close.confirmation.is_none()
            && self.about.is_none()
            && self.reorder.is_none()
            && !self.resizing_sidebar
    }

    /// Starts dragging `divider` of the active tab from the weights this
    /// window shows.
    fn start_divider_drag(
        &mut self,
        divider: Divider,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(tab) = self.active_tab(cx) else {
            return;
        };
        let Some(split) = self
            .shown_layout(tab, cx)
            .and_then(|layout| layout.split_node(divider.split).cloned())
        else {
            return;
        };
        self.divider_drag = Some(DividerDrag {
            tab,
            split: split.id,
            children: split
                .children
                .iter()
                .map(|child| child.node.id())
                .collect(),
            index: divider.index,
            axis: divider.axis,
            span: divider.span,
            boundary: split
                .children
                .iter()
                .take(divider.index + 1)
                .map(|child| child.weight)
                .sum(),
        });
        cx.notify();
    }

    /// Moves the dragged boundary toward `pointer`, as far as the minimum
    /// pane size allows, over the split as other changes left it. The
    /// previewed panes resize their terminals as a window resize does.
    fn drag_divider_to(
        &mut self,
        pointer: gpui::Point<Pixels>,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(drag) = self.divider_drag.clone() else {
            return;
        };
        let Some(split) = self
            .committed_layout(drag.tab, cx)
            .and_then(|layout| layout.split_node(drag.split).cloned())
        else {
            self.cancel_divider_drag(cx);
            return;
        };
        let span = self
            .drawn_dividers
            .borrow()
            .iter()
            .find(|divider| {
                divider.split == drag.split && divider.index == drag.index
            })
            .map_or(drag.span, |divider| divider.span);
        let position = match drag.axis {
            SplitAxis::Columns => pointer.x,
            SplitAxis::Rows => pointer.y,
        };
        // From the split as other changes left it, which met the minimum,
        // not from the carried boundary, which may not.
        let from = ResizePreview {
            tab: drag.tab,
            split: drag.split,
            children: drag.children.clone(),
            weights: split.children.iter().map(|child| child.weight).collect(),
        };
        let target = dragged_weights(&split, drag.index, span, position);
        let weights = self.clamp_resize(&from, drag.index, target, window, cx);
        let boundary = weights.iter().take(drag.index + 1).sum();
        if let Some(current) = &mut self.divider_drag {
            current.span = span;
            if current.boundary != boundary {
                current.boundary = boundary;
                cx.notify();
            }
        }
    }

    /// The weights between `preview`'s and `target`, which differ only
    /// beside the boundary after child `index`, that come nearest `target`
    /// while every pane keeps the minimum size in this window. A pane
    /// already under the minimum, after the window shrank, may not shrink
    /// further, so it cannot freeze the tab's other boundaries.
    fn clamp_resize(
        &self,
        preview: &ResizePreview,
        index: usize,
        target: Vec<u32>,
        window: &Window,
        cx: &App,
    ) -> Vec<u32> {
        let Some(layout) = self.shown_layout(preview.tab, cx) else {
            return preview.weights.clone();
        };
        let grids = |weights: &[u32]| {
            layout
                .with_weights(preview.split, &preview.children, weights)
                .ok()
                .and_then(|candidate| self.pane_grids(&candidate, window))
        };
        let Some(start) = grids(&preview.weights) else {
            return preview.weights.clone();
        };
        let fits = |weights: &[u32]| {
            grids(weights).is_some_and(|grids| keeps_minimum(&start, &grids))
        };
        if fits(&target) {
            return target;
        }
        let from = preview.weights.clone();
        // `target` comes from the same split, so it has the same children.
        let (Some(&first), Some(&second), Some(&aim)) =
            (from.get(index), from.get(index + 1), target.get(index))
        else {
            return from;
        };
        let pair = first + second;
        let with = |first: u32| {
            let mut weights = from.clone();
            if let Some([left, right]) = weights.get_mut(index..=index + 1) {
                *left = first;
                *right = pair - first;
            }
            weights
        };
        // The fitting end stays at `good`; the boundary moves toward `bad`.
        let (mut good, mut bad) = (first, aim);
        while good.abs_diff(bad) > 1 {
            let middle = good.midpoint(bad);
            if fits(&with(middle)) {
                good = middle;
            } else {
                bad = middle;
            }
        }
        with(good)
    }

    /// Commits the dragged boundary once, on release, over the split as
    /// other changes left it.
    fn finish_divider_drag(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(drag) = self.divider_drag.take() else {
            return;
        };
        // Against the layout with this window's queued commits, so a drag
        // back to the projection's position still lands after them.
        let Some(split) = self
            .committed_layout(drag.tab, cx)
            .and_then(|layout| layout.split_node(drag.split).cloned())
        else {
            return;
        };
        let weights = self.drag_weights(&drag, &split, window, cx);
        if !split
            .children
            .iter()
            .map(|child| child.weight)
            .eq(weights.iter().copied())
        {
            let preview = ResizePreview {
                tab: drag.tab,
                split: drag.split,
                children: drag.children,
                weights,
            };
            self.commit_resize(preview, window, cx);
        }
        cx.notify();
    }

    /// Cancels a divider drag whose split another change removed or whose
    /// children changed, or whose tab is no longer active.
    fn prune_divider_drag(&mut self, cx: &mut Context<'_, Self>) {
        let Some(drag) = &self.divider_drag else {
            return;
        };
        let applies = self.active_tab(cx) == Some(drag.tab)
            && cx
                .global::<Desktop>()
                .hierarchy
                .state()
                .tab(drag.tab)
                .and_then(|info| info.layout.split_node(drag.split))
                .is_some_and(|split| {
                    split
                        .children
                        .iter()
                        .map(|child| child.node.id())
                        .eq(drag.children.iter().copied())
                });
        if !applies {
            self.cancel_divider_drag(cx);
        }
    }

    /// The weights a drag shows over `split` as other changes left it: its
    /// boundary carried over `split`, moved back as far as the minimum pane
    /// size requires against `split`'s own weights.
    fn drag_weights(
        &self,
        drag: &DividerDrag,
        split: &SplitNode,
        window: &Window,
        cx: &App,
    ) -> Vec<u32> {
        let base = ResizePreview {
            tab: drag.tab,
            split: drag.split,
            children: drag.children.clone(),
            weights: split.children.iter().map(|child| child.weight).collect(),
        };
        let carried = with_boundary(split, drag.index, drag.boundary);
        self.clamp_resize(&base, drag.index, carried, window, cx)
    }

    /// Clamps a drag's carried boundary after another change to its split,
    /// so a neighbour that change shrank is never drawn or committed under
    /// the minimum pane size.
    fn reclamp_divider_drag(
        &mut self,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(drag) = self.divider_drag.clone() else {
            return;
        };
        let Some(split) = self
            .committed_layout(drag.tab, cx)
            .and_then(|layout| layout.split_node(drag.split).cloned())
        else {
            return;
        };
        let boundary: u32 = self
            .drag_weights(&drag, &split, window, cx)
            .iter()
            .take(drag.index + 1)
            .sum();
        if let Some(current) = &mut self.divider_drag
            && current.boundary != boundary
        {
            current.boundary = boundary;
            cx.notify();
        }
    }

    /// Drops a divider drag and its preview.
    fn cancel_divider_drag(&mut self, cx: &mut Context<'_, Self>) {
        if self.divider_drag.take().is_some() {
            cx.notify();
        }
    }

    /// Commits `preview` with `resize_split` on the layout commit queue.
    /// The window shows the committed weights until the projection holds
    /// the commit, so the divider never snaps back; a rejected commit drops
    /// them silently and the projection's layout shows.
    fn commit_resize(
        &mut self,
        preview: ResizePreview,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.queue_layout_commit(
            pane_layouts::LayoutCommit::Resize(preview),
            window,
            cx,
        );
    }

    /// Moves the focused pane's edge toward `direction` by `amount` cells,
    /// through the nearest split where the pane has a neighbour on that
    /// side, and commits it as a divider drag does.
    fn resize_pane(
        &mut self,
        direction: SplitDirection,
        amount: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        // Its weights would come from the layout the arrangement replaces
        // and land after it.
        if self.arrangements > 0 {
            return Ok(CommandOutcome::Completed);
        }
        let (tab, pane) = self.focused_pane(cx)?;
        self.cancel_divider_drag(cx);
        let (axis, leading) = match direction {
            SplitDirection::Left => (SplitAxis::Columns, true),
            SplitDirection::Right => (SplitAxis::Columns, false),
            SplitDirection::Up => (SplitAxis::Rows, true),
            SplitDirection::Down => (SplitAxis::Rows, false),
        };
        let Some(bounds) = self.terminal_geometry(window).bounds else {
            return Ok(CommandOutcome::Completed);
        };
        let Some((split, index, span)) =
            self.shown_layout(tab, cx).and_then(|layout| {
                let (split, index) = pane_geometry::resize_boundary(
                    &layout, pane, axis, leading,
                )?;
                let divider =
                    layout_panes(&layout, bounds, window.scale_factor(), None)
                        .dividers
                        .into_iter()
                        .find(|divider| {
                            divider.split == split.id && divider.index == index
                        })?;
                Some((split.clone(), index, divider.span))
            })
        else {
            return Ok(CommandOutcome::Completed);
        };
        // Resizing a zoomed tab shows its panes again, as navigation does.
        self.unzoom(tab, window, cx);
        let metrics = self.metrics.at_scale(window.scale_factor());
        let cell = match axis {
            SplitAxis::Columns => metrics.cell_width,
            SplitAxis::Rows => metrics.cell_height,
        };
        let from = boundary_position(&split, index, span);
        let to = if leading {
            from - cell * amount
        } else {
            from + cell * amount
        };
        let preview = ResizePreview {
            tab,
            split: split.id,
            children: split
                .children
                .iter()
                .map(|child| child.node.id())
                .collect(),
            weights: split.children.iter().map(|child| child.weight).collect(),
        };
        let target = dragged_weights(&split, index, span, to);
        let weights = self.clamp_resize(&preview, index, target, window, cx);
        if weights != preview.weights {
            self.commit_resize(
                ResizePreview { weights, ..preview },
                window,
                cx,
            );
            cx.notify();
        }
        Ok(CommandOutcome::Completed)
    }

    /// Republishes `tab`'s title, which follows its focused pane.
    fn publish_active_title(&self, tab: TabId, cx: &mut App) {
        let Some(view) = self.tabs.iter().find(|view| view.id == tab) else {
            return;
        };
        let title = view.title(self.config.tabs, cx);
        if cx.global_mut::<Desktop>().windows.set_tab_title(
            self.window,
            tab,
            &title,
        ) {
            mark_titles_changed(self.window, cx);
        }
    }

    /// Moves the view of `pane` from tab `from` into tab `to`, keeping its
    /// terminal view, viewer, scroll, selection, and exit state. The pane
    /// joins the end of `to`'s focus history and takes its place in `to`'s
    /// traversal order; a `to` this window had no view of gets one, and a
    /// `from` left empty is dropped, handing the window's activation to
    /// `to` when `from` was active.
    fn adopt_pane_view(
        &mut self,
        pane: PaneId,
        from: TabId,
        to: TabId,
        cx: &mut App,
    ) {
        let Some(source) = self.tabs.iter_mut().find(|view| view.id == from)
        else {
            return;
        };
        let Some(index) =
            source.panes.iter().position(|view| view.pane == pane)
        else {
            return;
        };
        let moved = source.panes.remove(index);
        let committed = moved.committed;
        let emptied = source.panes.is_empty();
        if !emptied {
            // The pane left, so focus falls back as for a closed pane.
            source.remove_panes(&[pane]);
            let titles = source.pane_titles(self.config.tabs, cx);
            let title = source.title(self.config.tabs, cx);
            let windows = &mut cx.global_mut::<Desktop>().windows;
            if windows.set_pane_titles(self.window, from, titles)
                | windows.set_tab_title(self.window, from, &title)
            {
                mark_titles_changed(self.window, cx);
            }
        }
        if let Some(target) = self.tabs.iter_mut().find(|view| view.id == to) {
            target.panes.push(moved);
            target.focus_history.push(pane);
            view_install::in_layout_order(&mut target.panes, to, cx);
            let titles = target.pane_titles(self.config.tabs, cx);
            let title = target.title(self.config.tabs, cx);
            let windows = &mut cx.global_mut::<Desktop>().windows;
            if windows.set_pane_titles(self.window, to, titles)
                | windows.set_tab_title(self.window, to, &title)
            {
                mark_titles_changed(self.window, cx);
            }
        } else {
            let names = cx
                .global::<Desktop>()
                .hierarchy
                .state()
                .tab(to)
                .map(TabNames::from_info)
                .unwrap_or_default();
            self.push_tab_view(
                TabView::new(to, committed, names, vec![moved]),
                false,
                cx,
            );
        }
        if emptied {
            // The tab that took the last pane takes over as active.
            if self.active_tab(cx) == Some(from) {
                cx.global_mut::<Desktop>().windows.select(self.window, to);
            }
            self.drop_tab_views(&[from], cx);
        }
    }

    /// Drops pane views of `tab` without closing anything, and republishes
    /// the tab's pane titles. A tab left without a view of any pane goes
    /// too. Returns whether anything was dropped.
    fn remove_pane_views(
        &mut self,
        tab: TabId,
        panes: &[PaneId],
        cx: &mut App,
    ) -> bool {
        let Some(view) = self.tabs.iter_mut().find(|view| view.id == tab)
        else {
            return false;
        };
        let before = view.panes.len();
        view.remove_panes(panes);
        if view.panes.len() == before {
            return false;
        }
        if view.panes.is_empty() {
            self.drop_tab_views(&[tab], cx);
            return true;
        }
        let titles = view.pane_titles(self.config.tabs, cx);
        let title = view.title(self.config.tabs, cx);
        let windows = &mut cx.global_mut::<Desktop>().windows;
        let changed = windows.set_pane_titles(self.window, tab, titles)
            | windows.set_tab_title(self.window, tab, &title);
        if changed {
            mark_titles_changed(self.window, cx);
        }
        true
    }

    /// Drops pane views after this window closed a pane or reconcile saw it
    /// go, then shows the tab's remaining panes and focuses the one that
    /// took focus.
    fn drop_pane_views(
        &mut self,
        tab: TabId,
        panes: &[PaneId],
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.remove_pane_views(tab, panes, cx)
            && self.active_tab(cx) == Some(tab)
        {
            self.sync_tab_layout(window, cx);
            if !self.overlay_holds_focus() {
                self.restore_tab_focus(window, cx);
            }
        }
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

    fn session(&self) -> Option<SessionId> {
        self.view.attached_session(self.cx)
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
        // A pane rename touches only its tab, and may leave the tab's own
        // title as it was.
        let panes = self
            .view
            .tabs
            .iter()
            .find(|view| view.id == tab)
            .map(|view| view.pane_titles(self.view.config.tabs, self.cx));
        let windows = &mut self.cx.global_mut::<Desktop>().windows;
        let tab_changed = windows.set_tab_title(self.view.window, tab, title);
        let panes_changed = panes.is_some_and(|panes| {
            windows.set_pane_titles(self.view.window, tab, panes)
        });
        tab_changed || panes_changed
    }

    fn pending_installs(&mut self) -> &mut PendingInstalls {
        &mut self.view.installs
    }

    fn install(&mut self, tabs: &[TabId]) {
        self.view.install_tabs(tabs, self.cx);
    }

    fn revoked(&self) -> Vec<TabId> {
        self.view
            .tabs
            .iter()
            .filter(|tab| tab.views().any(|view| view.read(self.cx).revoked))
            .map(|tab| tab.id)
            .collect()
    }

    fn replace(&mut self, tabs: &[TabId]) {
        self.view.replace_tabs(tabs, self.cx);
    }

    fn installed_panes(&self, tab: TabId) -> Vec<(PaneId, u64)> {
        self.view
            .tabs
            .iter()
            .find(|view| view.id == tab)
            .map(|view| {
                view.panes
                    .iter()
                    .map(|pane| (pane.pane, pane.committed))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn drop_panes(&mut self, tab: TabId, panes: &[PaneId]) {
        self.view.remove_pane_views(tab, panes, self.cx);
    }

    fn adopt_pane(&mut self, pane: PaneId, from: TabId, to: TabId) {
        self.view.adopt_pane_view(pane, from, to, self.cx);
    }

    fn order_panes(&mut self, tab: TabId) -> bool {
        let Some(view) = self.view.tabs.iter_mut().find(|view| view.id == tab)
        else {
            return false;
        };
        let before: Vec<PaneId> =
            view.panes.iter().map(|pane| pane.pane).collect();
        view_install::in_layout_order(&mut view.panes, tab, self.cx);
        if view.panes.iter().map(|pane| pane.pane).eq(before) {
            return false;
        }
        let titles = view.pane_titles(self.view.config.tabs, self.cx);
        if self.cx.global_mut::<Desktop>().windows.set_pane_titles(
            self.view.window,
            tab,
            titles,
        ) {
            mark_titles_changed(self.view.window, self.cx);
        }
        true
    }
}

impl WorkspaceView {
    /// The platform whose window frame this window draws.
    #[cfg_attr(
        not(test),
        expect(clippy::unused_self, reason = "only tests override it")
    )]
    fn frame_platform(&self) -> Platform {
        #[cfg(test)]
        if self.decorations_override.is_some() {
            return Platform::Linux;
        }
        Platform::current()
    }

    /// The facts that decide whether this window has a title-bar row.
    fn tab_host(&self) -> TabHost {
        TabHost {
            platform: self.frame_platform(),
            fullscreen: self.fullscreen.chrome_hidden,
            quake: self.quake.is_some(),
            client_decorations: self.frame_state.client_decorations(),
        }
    }

    /// The frame this window's chrome sits in, for its resolved position
    /// and the decorations GPUI last reported.
    fn window_frame(&self) -> WindowFrame {
        WindowFrame::resolve(
            self.frame_platform(),
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
        #[cfg(test)]
        let state = FrameState {
            decorations: self.decorations_override.unwrap_or(state.decorations),
            ..state
        };
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

    /// The geometry every terminal view in the window takes: the terminal
    /// bounds with the tab bar shown or hidden, never mid-reveal, and the
    /// scrollbar track margins beside that chrome.
    fn terminal_geometry(&self, window: &Window) -> TerminalGeometry {
        let tabs = self.layout_tabs();
        let presentation = self.presentation();
        let bounds = ChromeLayout::for_tabs(
            window.viewport_size(),
            self.title_row(),
            tabs,
            self.sidebar_width,
            self.fullscreen_insets,
            self.notch_shelf(),
            self.window_frame(),
        )
        .present(presentation, tabs.position, 0.0)
        .terminal;
        TerminalGeometry {
            bounds: Some(bounds),
            track_margins: terminal_track_margins(
                presentation,
                tabs.position,
                self.title_row(),
                self.config.window,
            ),
        }
    }

    /// The rectangles of `tab`'s panes inside `base`, from the projected
    /// layout; `None` for a tab the projection no longer holds.
    fn tab_panes(
        &self,
        tab: TabId,
        base: Bounds<Pixels>,
        zoomed: Option<PaneId>,
        window: &Window,
        cx: &App,
    ) -> Option<TabPanes> {
        let layout = self.shown_layout(tab, cx)?;
        Some(layout_panes(&layout, base, window.scale_factor(), zoomed))
    }

    /// `tab`'s layout as this window shows it: the projection's, with the
    /// resizes this window committed but the projection does not hold yet
    /// and its divider drag applied. A preview whose split changed no
    /// longer applies and is skipped.
    fn shown_layout<'a>(
        &self,
        tab: TabId,
        cx: &'a App,
    ) -> Option<Cow<'a, PaneLayout>> {
        let layout = self.committed_layout(tab, cx)?;
        let Some(drag) =
            self.divider_drag.as_ref().filter(|drag| drag.tab == tab)
        else {
            return Some(layout);
        };
        let Some(weights) = layout
            .split_node(drag.split)
            .map(|split| with_boundary(split, drag.index, drag.boundary))
        else {
            return Some(layout);
        };
        Some(
            layout
                .with_weights(drag.split, &drag.children, &weights)
                .map_or(layout, Cow::Owned),
        )
    }

    /// `tab`'s layout with this window's committed resizes applied but not
    /// its divider drag, which carries its boundary over this layout.
    fn committed_layout<'a>(
        &self,
        tab: TabId,
        cx: &'a App,
    ) -> Option<Cow<'a, PaneLayout>> {
        let mut layout = Cow::Borrowed(
            &cx.global::<Desktop>().hierarchy.state().tab(tab)?.layout,
        );
        for (_, preview) in self
            .resize_commits
            .iter()
            .filter(|(_, preview)| preview.tab == tab)
        {
            if let Ok(previewed) = layout.with_weights(
                preview.split,
                &preview.children,
                &preview.weights,
            ) {
                layout = Cow::Owned(previewed);
            }
        }
        Some(layout)
    }

    /// Whether `pane` of `tab` shows in this window when the tab is active:
    /// a zoomed pane hides the others, as a hidden tab does.
    fn zoom_shows(&self, tab: &TabView, pane: PaneId, cx: &App) -> bool {
        self.shown_zoom(tab, cx).is_none_or(|zoomed| zoomed == pane)
    }

    /// The pane `tab` shows across the whole tab in this window, from the
    /// window's own zoom or, under `[panes] zoom = "shared"`, the tab's.
    fn shown_zoom(&self, tab: &TabView, cx: &App) -> Option<PaneId> {
        let zoomed = match self.config.panes.zoom {
            huterm_config::PaneZoomScope::View => tab.zoomed,
            huterm_config::PaneZoomScope::Shared => {
                cx.global::<Desktop>().hierarchy.state().tab(tab.id)?.zoomed
            }
        };
        zoomed.filter(|pane| tab.pane(*pane).is_some() && tab.panes.len() > 1)
    }

    /// Each of `tab`'s pane views' geometry, in `tab.panes` order: its
    /// terminal rectangle inside the window's terminal bounds. A pane at
    /// the bounds' top right keeps the window's scrollbar track margins,
    /// which avoid the chrome corner there. A view the layout no longer
    /// holds, such as one still closing, keeps the geometry it has, and so
    /// does every view of a tab the projection no longer holds.
    fn pane_geometries(
        &self,
        tab: &TabView,
        base: TerminalGeometry,
        window: &Window,
        cx: &App,
    ) -> Vec<TerminalGeometry> {
        let Some(bounds) = base.bounds else {
            return vec![base; tab.panes.len()];
        };
        // A zoomed pane takes the whole tab; the panes it hides keep the
        // geometry of the split, as hidden tabs keep theirs.
        let rects = self.tab_panes(tab.id, bounds, None, window, cx);
        let zoomed = self.shown_zoom(tab, cx).and_then(|zoomed| {
            self.tab_panes(tab.id, bounds, Some(zoomed), window, cx)
        });
        tab.panes
            .iter()
            .map(|pane| {
                let Some(rect) = zoomed
                    .as_ref()
                    .and_then(|rects| rects.pane(pane.pane))
                    .or_else(|| {
                        rects.as_ref().and_then(|rects| rects.pane(pane.pane))
                    })
                else {
                    return pane.view.read(cx).geometry;
                };
                let corner = rect.terminal.origin.y == bounds.origin.y
                    && rect.terminal.right() == bounds.right();
                TerminalGeometry {
                    bounds: Some(rect.terminal),
                    track_margins: if corner {
                        base.track_margins
                    } else {
                        TerminalGeometry::default().track_margins
                    },
                }
            })
            .collect()
    }

    /// Every pane view of every tab with the geometry it takes now.
    fn pane_views_with_geometry(
        &self,
        window: &Window,
        cx: &App,
    ) -> Vec<(Entity<TerminalView>, TerminalGeometry)> {
        self.pane_views_with_layout(window, cx)
            .into_iter()
            .map(|(view, geometry, _)| (view, geometry))
            .collect()
    }

    /// Every pane view of every tab with its geometry and its place in its
    /// tab.
    fn pane_views_with_layout(
        &self,
        window: &Window,
        cx: &App,
    ) -> Vec<(Entity<TerminalView>, TerminalGeometry, PanePlacement)> {
        let base = self.terminal_geometry(window);
        self.tabs
            .iter()
            .flat_map(|tab| {
                let geometries = self.pane_geometries(tab, base, window, cx);
                let split = tab.panes.len() > 1;
                let zoomed = self.shown_zoom(tab, cx);
                tab.panes
                    .iter()
                    .zip(geometries)
                    .map(|(pane, geometry)| {
                        let has_pane = |direction| {
                            tab.neighbor(pane.pane, direction, cx).is_some()
                        };
                        let placement = PanePlacement {
                            split,
                            zoomed: zoomed == Some(pane.pane),
                            has_pane_left: has_pane(SplitDirection::Left),
                            has_pane_right: has_pane(SplitDirection::Right),
                            has_pane_up: has_pane(SplitDirection::Up),
                            has_pane_down: has_pane(SplitDirection::Down),
                        };
                        (pane.view.clone(), geometry, placement)
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn sync_tab_layout(&mut self, window: &Window, cx: &mut Context<'_, Self>) {
        // Config and bounds changes must reach hidden or frame-blocked PTYs.
        // Consume this once; pointer events do not require scanning every tab.
        let geometry_changed = std::mem::take(&mut self.layout_pending);
        let presentation = self.presentation();
        // The tab bar appearing or hiding is not a resize to report.
        let bar_toggled = self
            .synced_presentation
            .replace(presentation)
            .is_some_and(|synced| synced != presentation);
        let views = self.pane_views_with_layout(window, cx);
        let quake_presenting = self.quake_presenting();
        let overlay = (presentation == Presentation::Overlay
            && self.reveal.progress > 0.0)
            .then(|| {
                let layout = self.chrome_layout(window);
                layout.tabs.intersect(&layout.terminal)
            });
        let mut changed_any = false;
        for (view, geometry, placement) in views {
            view.update(cx, |terminal, cx| {
                // Key contexts render with the view, so a layout or zoom
                // change notifies every pane it moves, even one whose
                // terminal and geometry stay the same.
                if terminal.pane != placement {
                    terminal.pane = placement;
                    cx.notify();
                }
                let scale_changed =
                    terminal.metrics.at_scale(window.scale_factor())
                        != terminal.metrics;
                let cell_changed = terminal.last_cell_size
                    != Some(terminal.physical_cell_size());
                let changed = bar_toggled || terminal.geometry != geometry;
                changed_any |= geometry_changed
                    || changed
                    || scale_changed
                    || cell_changed;
                terminal.tab_overlay = overlay;
                terminal.geometry = geometry;
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

    /// Whether the window draws its tab bar: reserved, or an overlay bar
    /// part way or fully revealed.
    fn bar_drawn(&self) -> bool {
        self.presentation() == Presentation::Reserved
            || self.presentation() == Presentation::Overlay
                && self.reveal.progress > 0.0
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
        // Hidden views take the new width now and resize when shown.
        for (view, geometry) in self.pane_views_with_geometry(window, cx) {
            view.update(cx, |view, cx| {
                view.geometry = geometry;
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
            nesting: false,
            nest: None,
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
        let mode = self.reorder_pane_mode(window, cx);
        if let Some(drag) = &mut self.reorder {
            drag.nesting = mode.is_some();
            drag.nest = match mode {
                Some(PaneMode::Edge(target)) => Some(target),
                Some(PaneMode::Centre) | None => None,
            };
        }
        cx.notify();
    }

    fn advance_tab_scroll(&mut self, now: Instant, window: &Window) -> bool {
        let elapsed = now.saturating_duration_since(self.last_scroll);
        self.last_scroll = now;
        let strip = self.tab_strip(window);
        let next = if let Some(drag) = &mut self.reorder {
            drag.strip = strip;
            // A tab held over a pane to nest it leaves the bar where it is.
            if !drag.dragging || drag.nesting {
                return false;
            }
            drag.strip.autoscroll(drag.pointer, elapsed)
        } else if let Some(next) = self
            .pane_drag
            .as_mut()
            .and_then(|drag| drag.autoscroll(strip.clone(), elapsed))
        {
            next
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

    /// Focuses the active terminal, or the showing dialog.
    fn restore_tab_focus(&self, window: &mut Window, cx: &mut App) {
        if self.dialog_showing() {
            self.focus_dialog(window, cx);
        } else if let Some(tab) = self.active_view(cx) {
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
        if drag.nesting {
            self.finish_nest(drag.source, drag.nest, window, cx);
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

    /// The window's terminal: the active tab's focused pane.
    fn active_view(&self, cx: &App) -> Option<Entity<TerminalView>> {
        self.active_tab_view(cx)
            .map(|tab| tab.focused_view().clone())
    }

    /// The active tab and the pane this window focuses in it.
    fn active_pane(&self, cx: &App) -> Option<(TabId, PaneId)> {
        self.active_tab_view(cx)
            .map(|tab| (tab.id, tab.focused().pane))
    }

    fn active_tab_view(&self, cx: &App) -> Option<&TabView> {
        let active = self.active_tab(cx)?;
        self.tabs.iter().find(|tab| tab.id == active)
    }

    /// Drains `pane_id`'s viewer and publishes what changed. `tab_id` is
    /// the tab the drain started in; a pane that has since moved to another
    /// tab of this window refreshes there, so its drain needs no restart.
    fn refresh_pane(
        &mut self,
        tab_id: TabId,
        pane_id: PaneId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<bool> {
        let tab = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id && tab.pane(pane_id).is_some())
            .or_else(|| {
                self.tabs.iter().find(|tab| tab.pane(pane_id).is_some())
            })?;
        let tab_id = tab.id;
        let pane = tab.pane(pane_id)?;
        let result = pane
            .view
            .update(cx, |terminal, cx| terminal.refresh(window, cx));
        let (title, status) = tab.label(self.config.tabs, cx);
        // An exit notice names the pane in a split tab.
        let exit_title = if tab.panes.len() > 1 {
            tab.pane_label(pane, self.config.tabs, cx)
        } else {
            title.clone()
        };
        let pane_titles = tab.pane_titles(self.config.tabs, cx);
        // Title, metadata, and exit changes all arrive through this drain;
        // the model only records a title that actually changed.
        let windows = &mut cx.global_mut::<Desktop>().windows;
        let tab_changed = windows.set_tab_title(
            self.window,
            tab_id,
            &with_status_suffix(title.clone(), status),
        );
        let panes_changed =
            windows.set_pane_titles(self.window, tab_id, pane_titles);
        if tab_changed || panes_changed {
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
                exit_notice(tab_id, &exit_title, result.exit_code),
                Instant::now(),
            );
            cx.notify();
        }
        if result.exited {
            let desktop = cx.global_mut::<Desktop>();
            let holds = desktop.hierarchy.state().pane_tab(pane_id).is_some();
            desktop.exit_closes.observe(
                pane_id,
                self.config.terminal.close_on_exit,
                holds,
            );
        }
        if result.changed {
            cx.notify();
        }
        // A revocation emits no hierarchy event; reconcile replaces the
        // viewer, or removes the view once its pane has left.
        if self
            .tabs
            .iter()
            .filter(|tab| tab.id == tab_id)
            .filter_map(|tab| tab.pane(pane_id))
            .any(|pane| pane.view.read(cx).revoked)
        {
            self.reconcile_own(window, cx);
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
        self.cancel_pane_drag(window, cx);
        self.resizing_sidebar = false;
        self.check_new_tab_available(cx)?;
        let mut command = shell_command(
            self.metrics.at_scale(window.scale_factor()),
            &self.config.theme,
            self.config.terminal,
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
                            opened.tab.first_pane().id,
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

    /// The active tab and its focused pane.
    fn focused_pane(&self, cx: &App) -> Result<(TabId, PaneId), CommandError> {
        self.active_tab_view(cx)
            .map(|tab| (tab.id, tab.focused().pane))
            .ok_or_else(|| {
                CommandError::Unavailable("window has no active tab".to_owned())
            })
    }

    /// Asks to close the active tab's focused pane, as its title bar's
    /// close button does.
    fn close_focused_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.check_tab_close_available()?;
        let (tab, pane) = self.focused_pane(cx)?;
        self.request_close(CloseTarget::Pane { tab, pane }, window, cx);
        Ok(CommandOutcome::Accepted)
    }

    /// Focuses the pane after or before the focused one in traversal order,
    /// wrapping.
    fn cycle_pane(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        let (tab, pane) = self.focused_pane(cx)?;
        let next = cx
            .global::<Desktop>()
            .hierarchy
            .state()
            .tab(tab)
            .and_then(|info| info.layout.cycle(pane, forward));
        if let Some(next) = next {
            self.unzoom(tab, window, cx);
            self.focus_pane(tab, next, window, cx);
        }
        Ok(CommandOutcome::Completed)
    }

    /// Focuses the pane beside the focused one toward `direction`, without
    /// wrapping; ties go to the pane this window focused most recently.
    fn focus_neighbor(
        &mut self,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        let (tab, pane) = self.focused_pane(cx)?;
        let next = self
            .tabs
            .iter()
            .find(|view| view.id == tab)
            .and_then(|view| view.neighbor(pane, direction, cx));
        if let Some(next) = next {
            self.unzoom(tab, window, cx);
            self.focus_pane(tab, next, window, cx);
        }
        Ok(CommandOutcome::Completed)
    }

    /// The grid a new pane gets when `pane` of `tab` splits toward
    /// `direction`, if every pane of the resulting layout, title bars,
    /// dividers, and padding included, keeps at least the minimum pane size
    /// in this window.
    fn split_grid(
        &self,
        tab: TabId,
        pane: PaneId,
        direction: SplitDirection,
        window: &Window,
        cx: &App,
    ) -> Result<GridSize, CommandError> {
        let too_small = || {
            CommandError::Unavailable("pane is too small to split".to_owned())
        };
        let layout = &cx
            .global::<Desktop>()
            .hierarchy
            .state()
            .tab(tab)
            .ok_or(CommandError::StaleTarget)?
            .layout;
        // Placeholder identities no runtime allocates.
        let probe = RuntimeId::new(u64::MAX);
        let added = PaneId::in_runtime(probe, u64::MAX);
        let candidate = layout
            .with_split(
                pane,
                direction,
                PaneInfo {
                    id: added,
                    terminal_id: TerminalId::new(u64::MAX),
                    fallback_name: String::new(),
                    custom_name: None,
                },
                SplitId::in_runtime(probe, u64::MAX),
            )
            .map_err(|_| too_small())?;
        let grids =
            self.pane_grids(&candidate, window).ok_or_else(too_small)?;
        if !fits_minimum(&grids) {
            return Err(too_small());
        }
        grids
            .into_iter()
            .find_map(|(pane, grid)| (pane == added).then_some(grid))
            .ok_or_else(too_small)
    }

    /// The grid each pane of `layout` would get in this window, unzoomed,
    /// with title bars, dividers, and padding; `None` for a window that
    /// hosts no terminal bounds.
    fn pane_grids(
        &self,
        layout: &PaneLayout,
        window: &Window,
    ) -> Option<Vec<(PaneId, GridSize)>> {
        let bounds = self.terminal_geometry(window).bounds?;
        let metrics = self.metrics.at_scale(window.scale_factor());
        let cell = size(metrics.cell_width, metrics.cell_height);
        let rects = layout_panes(layout, bounds, window.scale_factor(), None);
        Some(
            rects
                .panes
                .iter()
                .map(|rect| {
                    let layout = TerminalLayout::new(
                        rect.terminal.size,
                        cell,
                        self.config.window,
                    );
                    (rect.pane, layout.grid)
                })
                .collect(),
        )
    }

    /// Splits the active tab's focused pane toward `direction`. The new pane
    /// starts in the focused pane's directory under `new_tab_directory`, at
    /// the grid it will show, and takes focus.
    fn split_pane(
        &mut self,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.check_new_tab_available(cx)?;
        let (tab, pane) = self.focused_pane(cx)?;
        let grid = self.split_grid(tab, pane, direction, window, cx)?;
        // Splitting a zoomed tab unzooms it; the runtime clears a shared
        // zoom with the split itself.
        if let Some(view) = self.tabs.iter_mut().find(|view| view.id == tab) {
            view.zoomed = None;
        }
        self.sync_zoom(window, cx);
        let (Some(workspace), Some(attachment)) =
            (self.workspace_id(cx), self.attachment_id(cx))
        else {
            return Err(CommandError::StaleTarget);
        };
        let mut command = shell_command(
            self.metrics.at_scale(window.scale_factor()),
            &self.config.theme,
            self.config.terminal,
        )
        .map_err(|error| CommandError::Runtime(error.to_string()))?;
        command.grid_size = grid;
        let inherited_directory = self.active_view(cx).and_then(|view| {
            inherited_directory(
                self.config.terminal.new_tab_directory,
                &view.read(cx).metadata,
            )
        });
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
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
            runtime.split_pane(
                workspace,
                attachment,
                tab,
                pane,
                direction,
                &command,
                clipboard_allowed,
            )
        });
        let app = cx.to_async();
        cx.spawn_in(window, async move |view, cx| {
            let Committed { result, seq } = task.await;
            let resolution = projected(&app, seq).await;
            let _ = view.update_in(cx, |view, window, cx| {
                view.busy = false;
                let terminating = terminating(cx);
                match result {
                    Ok((opened, viewer))
                        if resolution == Resolution::Ready && !terminating =>
                    {
                        let key = PaneKey {
                            tab,
                            pane: opened.pane.id,
                            terminal: opened.pane.terminal_id,
                        };
                        view.publish_split(key, viewer, seq, window, cx);
                    }
                    // Reconcile installs the pane like any other once the
                    // projection holds it.
                    Ok(_) => {}
                    Err(error) => {
                        if !terminating {
                            view.notify(
                                NoticeContent::command_failure(
                                    "Cannot split pane",
                                    format!("Cannot split pane: {error}"),
                                ),
                                cx,
                            );
                        }
                    }
                }
                view.reconcile_own(window, cx);
                view.resume_close(window, cx);
                cx.notify();
            });
            // The split stays pending through its wait, so Quit requested
            // meanwhile resumes only after it settles.
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

    /// Adds a split's new pane view to its tab and focuses it.
    fn publish_split(
        &mut self,
        key: PaneKey,
        viewer: TerminalViewer,
        committed: u64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let tab = key.tab;
        if !self.tabs.iter().any(|view| view.id == tab) {
            return;
        }
        let Some(pane) = self.pane_view(key, viewer, committed, window, cx)
        else {
            return;
        };
        self.add_pane_views(tab, vec![pane], committed, cx);
        if let Some(view) = self.tabs.iter_mut().find(|view| view.id == tab) {
            view.focus_pane(key.pane);
        }
        self.publish_active_title(tab, cx);
        if self.active_tab(cx) == Some(tab) {
            self.select(tab, window, cx);
        }
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
        let (session, id, opened, attachment, viewer) = spawned;
        self.startup_reporter = None;
        cx.global_mut::<Desktop>().windows.attach(
            self.window,
            attachment,
            session,
            id,
        );
        let tab_id = opened.tab.id;
        // A viewer revoked already leaves the tab without a view, which
        // reconcile installs again if the window still shows its workspace.
        let first = opened.tab.first_pane();
        let key = PaneKey {
            tab: tab_id,
            pane: first.id,
            terminal: first.terminal_id,
        };
        let Some(tab) = self.tab_view(key, viewer, committed, window, cx)
        else {
            return;
        };
        self.push_tab_view(tab, true, cx);
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

    /// Builds the view of `key`'s tab around the viewer of its pane;
    /// `None` when the viewer was already revoked.
    fn tab_view(
        &mut self,
        key: PaneKey,
        viewer: TerminalViewer,
        committed: u64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<TabView> {
        let tab_id = key.tab;
        let pane = self.pane_view(key, viewer, committed, window, cx)?;
        // The completion drained first, so the projection holds the tab.
        let names = cx
            .global::<Desktop>()
            .hierarchy
            .state()
            .tab(tab_id)
            .map(TabNames::from_info)
            .unwrap_or_default();
        Some(TabView::new(tab_id, committed, names, vec![pane]))
    }

    /// Builds one pane's view around its viewer, with the pane's activity
    /// drain; `None` when the viewer was already revoked.
    fn pane_view(
        &mut self,
        key: PaneKey,
        viewer: TerminalViewer,
        committed: u64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<PaneView> {
        if let Some(recipient) = viewer.host_effects() {
            recipient
                .set_allowed(self.config.terminal.clipboard_write.is_allowed());
        }
        let activity = viewer.wake_handle();
        let scroll_key = scroll_to_bottom_key(&cx.global::<Desktop>().keymap);
        let geometry = self.terminal_geometry(window);
        let terminal = cx.new(|cx| {
            let mut terminal = TerminalView::new(
                viewer,
                Rc::clone(&self.frame_clock),
                &self.config,
                self.family.clone(),
                self.metrics.at_scale(window.scale_factor()),
                window,
                cx,
            );
            terminal.set_scroll_to_bottom_key(scroll_key, cx);
            terminal.geometry = geometry;
            terminal
        });
        if terminal.read(cx).revoked {
            return None;
        }
        cx.subscribe_in(&terminal, window, Self::handle_context_menu_request)
            .detach();
        let failure_wakes = terminal.read(cx).failure_wakes.clone();
        let activity_task = Self::pane_activity_task(
            key.tab,
            key.pane,
            activity,
            failure_wakes,
            window,
            cx,
        );
        Some(PaneView {
            pane: key.pane,
            terminal: key.terminal,
            committed,
            view: terminal,
            activity_task,
        })
    }

    /// A pane's activity drain: waits for terminal events or failures and
    /// refreshes the pane in bounded batches.
    fn pane_activity_task(
        tab_id: TabId,
        pane_id: PaneId,
        activity: ViewerWake,
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
                let stopped =
                    TerminalView::wait_for_activity(&activity, &failure_wakes)
                        .await
                        .is_err();
                if let Some(probe) = &activity_probe {
                    probe.waiting(false);
                }
                loop {
                    let more = view.update_in(cx, |view, window, cx| {
                        view.refresh_pane(tab_id, pane_id, window, cx)
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
        let reporter = self.startup_reporter.take();
        if self.quake.is_some() && self.tabs.is_empty() {
            // This window is about to close, so another must show the
            // failure: the reporter's, the active one, or the next to open
            // or become active.
            report_deferred_failure_with_global_latch(
                cx,
                reporter,
                message.clone(),
            );
            if let Some(state) = &self.quake {
                cx.global_mut::<Desktop>().quake.failed_spawn =
                    Some((state.name.clone(), message));
            }
            eprintln!("Cannot start quake shell: {error}");
            self.remove_window(window, cx, false);
            return true;
        }
        if reporter.is_some() {
            report_deferred_failure(cx, reporter, message);
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
            CommandScope::Focused => {
                if let Some(licenses) = self.licenses.clone() {
                    return licenses.update(cx, |licenses, cx| {
                        licenses.run(invocation, window, cx)
                    });
                }
                self.palette
                    .clone()
                    .ok_or_else(|| {
                        CommandError::Unavailable(
                            "command palette is not open".to_owned(),
                        )
                    })?
                    .update(cx, |palette, cx| palette.run(invocation, cx))
            }
            CommandScope::Terminal => Err(CommandError::ClientRequired),
        }
    }

    /// Runs a focused-scope command from the palette or the Licenses view.
    /// Neither gives the field verbs a meaning of its own, so they go on
    /// unhandled and GPUI tries the next binding for the key, the palette's
    /// own under `Palette`.
    fn invoke_focused(
        &mut self,
        action: &InvokeFocused,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if crate::ui::input::is_field_verb(action.0.id) {
            cx.propagate();
            return;
        }
        cx.stop_propagation();
        if self.licenses.is_some() {
            // The view has no error line; a refused command changes nothing.
            let _ = self.invoke_interactive(&action.0, window, cx);
            return;
        }
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
        self.cancel_pane_drag(window, cx);

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
                .map(|tab| tab.focused().terminal),
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
            titles: self.palette_titles(cx),
            colors: self.palette_colors(),
            placement: self.config.palette.placement,
            history,
            profiles: quake_profile_rows(cx),
            layouts: self.config.panes.layouts(),
            request,
            retained_query,
            hierarchy,
            hierarchy_seq,
            clock: Rc::clone(&self.frame_clock),
        };
        let palette = cx.new(|cx| CommandPalette::open(open, cx));
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
        let pane_titles: HashMap<TerminalId, &str> =
            desktop.windows.published_pane_titles().collect();
        let state = desktop.hierarchy.state();
        let hierarchy = PaletteHierarchy::from_projection_with(
            state,
            |tab| titles.get(&tab).copied(),
            |terminal| pane_titles.get(&terminal).copied(),
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
                .map(|tab| tab.focused().terminal),
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
                    Ok(CommandScope::Focused) => {
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

    /// Palette titles that name what `undo` and `redo` would restore.
    fn palette_titles(
        &self,
        cx: &App,
    ) -> std::collections::HashMap<huterm_protocol::CommandId, String> {
        [
            (ids::UNDO, RestoreDirection::Undo),
            (ids::REDO, RestoreDirection::Redo),
        ]
        .into_iter()
        .filter_map(|(id, direction)| {
            let label = self.layout_restore_label(direction, cx)?;
            Some((id, format!("{} {label}", window_menu::label(id))))
        })
        .collect()
    }

    fn current_palette_refresh_state(&self, cx: &App) -> PaletteRefreshState {
        PaletteRefreshState {
            tabs: self.tabs.len(),
            active: self.active_tab(cx),
            busy: self.busy || self.arrangements > 0,
            confirming: self.dialog_showing(),
            reordering: self.reorder.is_some(),
            quake: self
                .quake
                .as_ref()
                .map(|state| (state.visible(), state.fullscreen_context())),
            restorable: [RestoreDirection::Undo, RestoreDirection::Redo]
                .map(|direction| self.layout_restore_label(direction, cx)),
        }
    }

    fn refresh_palette(&mut self, cx: &mut Context<'_, Self>) {
        let Some(palette) = self.palette.clone() else {
            self.palette_refresh_state = None;
            return;
        };
        let state = self.current_palette_refresh_state(cx);
        if self.palette_refresh_state.as_ref() == Some(&state) {
            return;
        }
        self.palette_refresh_state = Some(state);
        let target = palette.read(cx).target.clone();
        let availability = self.palette_availability(&target, cx);
        let keymap = cx.global::<Desktop>().keymap.clone();
        let titles = self.palette_titles(cx);
        palette.update(cx, |palette, _| {
            palette.set_availability(availability, titles);
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
        overlay_colors(&self.config.theme)
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
                ids::SPLIT_RIGHT
                | ids::SPLIT_DOWN
                | ids::SPLIT_LEFT
                | ids::SPLIT_UP => {
                    self.check_new_tab_available(cx)?;
                    self.active_tab_id(cx).map(|_| ())
                }
                ids::CLOSE_PANE => {
                    self.check_tab_close_available()?;
                    self.active_tab_id(cx).map(|_| ())
                }
                ids::FOCUS_NEXT_PANE
                | ids::FOCUS_PREVIOUS_PANE
                | ids::FOCUS_PANE_LEFT
                | ids::FOCUS_PANE_RIGHT
                | ids::FOCUS_PANE_UP
                | ids::FOCUS_PANE_DOWN
                | ids::RESIZE_PANE_LEFT
                | ids::RESIZE_PANE_RIGHT
                | ids::RESIZE_PANE_UP
                | ids::RESIZE_PANE_DOWN
                | ids::TOGGLE_PANE_ZOOM => {
                    self.check_navigation_available(cx)?;
                    if self
                        .active_tab_view(cx)
                        .is_some_and(|tab| tab.panes.len() > 1)
                    {
                        Ok(())
                    } else {
                        Err(CommandError::Unavailable(
                            "tab has one pane".to_owned(),
                        ))
                    }
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
                ids::ABOUT => Ok(()),
                other if pane_moves::is_pane_move_command(other) => {
                    self.pane_move_availability(other, cx)
                }
                other if pane_layouts::is_layout_command(other) => {
                    self.pane_layout_availability(cx)
                }
                other => match layout_undo::restore_direction(other) {
                    Some(direction) => {
                        self.layout_restore_availability(direction, cx)
                    }
                    None => Err(CommandError::UnknownCommand(other)),
                },
            },
            CommandScope::Runtime => {
                self.check_runtime_available(cx)?;
                match command {
                    ids::RENAME_TAB if target.tab.is_none() => Err(
                        CommandError::Unavailable("window has no tab".into()),
                    ),
                    ids::RENAME_PANE if target.terminal.is_none() => Err(
                        CommandError::Unavailable("window has no pane".into()),
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
                if Some(terminal.viewer.terminal_id()) != target.terminal {
                    return Err(CommandError::StaleTarget);
                }
                terminal.command_availability(command)
            }
            CommandScope::Focused => Ok(()),
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
        if let Some(licenses) = self.licenses.clone() {
            let (colors, hints) =
                (self.palette_colors(), self.config.window.shortcut_hints);
            licenses.update(cx, |licenses, cx| {
                licenses.set_presentation(colors, hints, cx);
            });
        }
        if let Some(palette) = self.palette.clone() {
            let colors = self.palette_colors();
            let profiles = quake_profile_rows(cx);
            let layouts = self.config.panes.layouts();
            palette.update(cx, |palette, cx| {
                palette.set_presentation(
                    colors,
                    self.config.palette.placement,
                    profiles,
                    &layouts,
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
        } else if self.arrangements > 0 {
            "pane arrangement in progress"
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
        tab_directory_path(&tab.focused_view().read(cx).metadata).ok_or_else(
            || CommandError::Unavailable("directory unknown".to_owned()),
        )
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
        match tab.focused_view().read(cx).metadata.directory() {
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

    /// Runs a `dialog_*` command against the top dialog. The focus
    /// commands cycle through its buttons in the window's dialog slot, and
    /// confirm presses the focused button, so Enter on Cancel cancels. In
    /// the close confirmation, cancel cancels; in the About panel, it
    /// closes the panel. The Licenses view takes its keys as `Palette`
    /// commands, so these reach it only through a binding the user moved:
    /// confirm presses its focused button and cancel steps back as Escape
    /// does there.
    fn run_dialog_command(
        &mut self,
        command: huterm_protocol::CommandId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        if !self.dialog_showing() {
            self.confirming_target()?;
        }
        match command {
            ids::DIALOG_FOCUS_NEXT => self.dialog.focus_next(window, cx),
            ids::DIALOG_FOCUS_PREVIOUS => {
                self.dialog.focus_previous(window, cx);
            }
            ids::DIALOG_CONFIRM | ids::DIALOG_CANCEL => {
                self.answer_dialog(command == ids::DIALOG_CONFIRM, window, cx)?;
            }
            other => return Err(CommandError::UnknownCommand(other)),
        }
        cx.notify();
        Ok(CommandOutcome::Completed)
    }

    /// Presses the top dialog's focused button for `dialog_confirm`, or
    /// answers `dialog_cancel`, as [`Self::run_dialog_command`] describes.
    fn answer_dialog(
        &mut self,
        confirm: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<(), CommandError> {
        if let Some(licenses) = self.licenses.clone() {
            let id = if confirm {
                ids::PALETTE_CONFIRM
            } else {
                ids::PALETTE_BACK
            };
            let invocation = CommandInvocation::new(id, vec![]);
            licenses.update(cx, |licenses, cx| {
                licenses.run(&invocation, window, cx)
            })?;
            return Ok(());
        }
        if self.about.is_some() {
            match self.about_buttons.focused(window) {
                _ if !confirm => self.close_about(window, cx),
                Some(AboutButton::Licenses) => self.show_licenses(window, cx),
                Some(AboutButton::Copy) => self.copy_about_details(cx),
                Some(AboutButton::Close) => self.close_about(window, cx),
                None => {}
            }
            return Ok(());
        }
        let target = self.confirming_target()?;
        if !confirm || self.close_buttons.cancel.is_focused(window) {
            self.cancel_confirmation(window, cx);
        } else if self.close_buttons.primary.is_focused(window) {
            self.finish_close(target, window, cx);
        }
        Ok(())
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

    fn run_pane_command(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        use SplitDirection::{Down, Left, Right, Up};
        // Validated to 1..=100 cells.
        let amount = invocation
            .integer("amount")
            .and_then(|amount| u16::try_from(amount).ok())
            .map_or(5.0, f32::from);
        // Focus, resize, and zoom would act on the layout an arrangement of
        // this window is about to replace. A key pressed meanwhile does
        // nothing, as a layout key does, so a held key stays quiet.
        let follows_layout = matches!(
            invocation.id,
            ids::FOCUS_NEXT_PANE
                | ids::FOCUS_PREVIOUS_PANE
                | ids::FOCUS_PANE_LEFT
                | ids::FOCUS_PANE_RIGHT
                | ids::FOCUS_PANE_UP
                | ids::FOCUS_PANE_DOWN
                | ids::RESIZE_PANE_LEFT
                | ids::RESIZE_PANE_RIGHT
                | ids::RESIZE_PANE_UP
                | ids::RESIZE_PANE_DOWN
                | ids::TOGGLE_PANE_ZOOM
        );
        if follows_layout && self.arrangements > 0 {
            return Ok(CommandOutcome::Completed);
        }
        match invocation.id {
            ids::SPLIT_RIGHT => self.split_pane(Right, window, cx),
            ids::SPLIT_DOWN => self.split_pane(Down, window, cx),
            ids::SPLIT_LEFT => self.split_pane(Left, window, cx),
            ids::SPLIT_UP => self.split_pane(Up, window, cx),
            ids::CLOSE_PANE => self.close_focused_pane(window, cx),
            ids::FOCUS_NEXT_PANE => self.cycle_pane(true, window, cx),
            ids::FOCUS_PREVIOUS_PANE => self.cycle_pane(false, window, cx),
            ids::FOCUS_PANE_LEFT => self.focus_neighbor(Left, window, cx),
            ids::FOCUS_PANE_RIGHT => self.focus_neighbor(Right, window, cx),
            ids::FOCUS_PANE_UP => self.focus_neighbor(Up, window, cx),
            ids::FOCUS_PANE_DOWN => self.focus_neighbor(Down, window, cx),
            ids::RESIZE_PANE_LEFT => self.resize_pane(Left, amount, window, cx),
            ids::RESIZE_PANE_RIGHT => {
                self.resize_pane(Right, amount, window, cx)
            }
            ids::RESIZE_PANE_UP => self.resize_pane(Up, amount, window, cx),
            ids::RESIZE_PANE_DOWN => self.resize_pane(Down, amount, window, cx),
            ids::TOGGLE_PANE_ZOOM => {
                let (tab, pane) = self.focused_pane(cx)?;
                self.toggle_zoom_of(tab, pane, window, cx);
                Ok(CommandOutcome::Completed)
            }
            other => Err(CommandError::UnknownCommand(other)),
        }
    }

    fn run_command(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        validate(invocation)?;
        // A menu open over the palette is its field's, and takes its keys.
        if self.palette.is_some()
            && invocation.id != ids::OPEN_COMMAND_PALETTE
            && !is_menu_command(invocation.id)
        {
            // The palette's search field has no undo, so its undo keys do
            // nothing rather than raise a notice.
            if layout_undo::restore_direction(invocation.id).is_some() {
                return Ok(CommandOutcome::Completed);
            }
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
            id if is_pane_command(id) => {
                self.run_pane_command(invocation, window, cx)
            }
            id if pane_moves::is_pane_move_command(id) => {
                self.run_pane_move(invocation, window, cx)
            }
            id if pane_layouts::is_layout_command(id) => {
                self.run_pane_layout(invocation, window, cx)
            }
            ids::UNDO => {
                self.run_layout_restore(RestoreDirection::Undo, window, cx)
            }
            ids::REDO => {
                self.run_layout_restore(RestoreDirection::Redo, window, cx)
            }
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
            ids::OPEN_COMMAND_PALETTE => self.open_palette(window, cx),
            ids::RENAME_TAB
            | ids::RENAME_PANE
            | ids::RENAME_WORKSPACE
            | ids::RENAME_SESSION => {
                let invocation =
                    fill_rename_target(invocation, self.rename_context(cx))?;
                Ok(run_on_runtime(invocation, cx))
            }
            other => Err(CommandError::UnknownCommand(other)),
        }
    }
}

impl WorkspaceView {
    /// The targets a rename that omits its own takes from this window.
    fn rename_context(&self, cx: &App) -> RenameContext {
        RenameContext {
            tab: self.active_tab(cx),
            pane: self.active_tab_view(cx).map(|tab| tab.focused().pane),
            workspace: self.workspace_id(cx),
        }
    }

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
            .flat_map(|entry| {
                let split = entry.panes.len() > 1;
                entry.panes.iter().map(move |pane| TabTitle {
                    tab: entry.id,
                    terminal: pane.terminal,
                    title: entry.title.clone(),
                    pane: split.then(|| pane.title.clone()),
                })
            })
            .collect();
        let jobs = self
            .close
            .assessment
            .iter()
            .flat_map(CloseAssessment::terminal_jobs);
        // A lone pane's close reads as its tab's, as before splits existed;
        // the runtime decides from the layout, not this window's views.
        let pane_title = match target {
            CloseTarget::Pane { tab, pane } => self
                .tabs
                .iter()
                .find(|view| {
                    view.id == *tab
                        && cx
                            .global::<Desktop>()
                            .hierarchy
                            .state()
                            .tab(*tab)
                            .is_some_and(|info| info.layout.pane_count() > 1)
                })
                .and_then(|view| {
                    let pane = view.pane(*pane)?;
                    Some(view.pane_label(pane, self.config.tabs, cx))
                }),
            _ => None,
        };
        close_dialog_input(target, jobs, &titles, pane_title.as_deref())
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
    /// The pane's own title, in a split tab.
    pane: Option<String>,
}

impl TabTitle {
    /// The heading of this terminal's jobs: the tab title, followed by the
    /// pane's in a split tab.
    fn heading(&self) -> String {
        match &self.pane {
            Some(pane) => format!("{} › {pane}", self.title),
            None => self.title.clone(),
        }
    }
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
/// a tab set, the application, or a window with more than one tab. A pane
/// close with no `pane_title`, the close of a tab's only pane, reads as the
/// tab's close.
fn close_dialog_input<'a>(
    target: &CloseTarget,
    jobs: impl IntoIterator<Item = (TerminalId, &'a huterm_core::JobState)>,
    titles: &[TabTitle],
    pane_title: Option<&str>,
) -> CloseDialogInput {
    let dialog_target = match target {
        CloseTarget::Tab(id) => CloseDialogTarget::Tab {
            title: titles
                .iter()
                .find(|entry| entry.tab == *id)
                .map(|entry| entry.title.clone())
                .unwrap_or_default(),
        },
        CloseTarget::Pane { tab, .. } => match pane_title {
            Some(title) => CloseDialogTarget::Pane {
                title: title.to_owned(),
            },
            None => CloseDialogTarget::Tab {
                title: titles
                    .iter()
                    .find(|entry| entry.tab == *tab)
                    .map(|entry| entry.title.clone())
                    .unwrap_or_default(),
            },
        },
        CloseTarget::Tabs(ids) => CloseDialogTarget::Tabs { count: ids.len() },
        CloseTarget::Window | CloseTarget::Emptied => CloseDialogTarget::Window,
        CloseTarget::Application => CloseDialogTarget::Application,
    };
    // A split tab's jobs show under their panes' headings, so two busy
    // panes stay distinguishable.
    let headings = match (&dialog_target, target) {
        (CloseDialogTarget::Pane { .. }, _)
        | (CloseDialogTarget::Tab { .. }, CloseTarget::Pane { .. }) => false,
        (CloseDialogTarget::Tab { .. }, CloseTarget::Tab(id)) => titles
            .iter()
            .any(|entry| entry.tab == *id && entry.pane.is_some()),
        (CloseDialogTarget::Window, _) => titles.len() > 1,
        _ => true,
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
                        TabTitle::heading,
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
    /// Shows the About panel, or refocuses it, or the Licenses view over it,
    /// when it is already showing. Its buttons hold focus, so the
    /// `confirming` bindings reach it and terminal input is blocked while
    /// it is up.
    fn show_about(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        if self.about.is_some() {
            self.focus_dialog(window, cx);
            cx.notify();
            return Ok(CommandOutcome::Completed);
        }
        self.check_available(false)?;
        self.cancel_reorder(window, cx);
        self.cancel_pane_drag(window, cx);
        self.resizing_sidebar = false;
        if let Some(terminal) = self.active_view(cx) {
            terminal.update(cx, TerminalView::clear_composition);
        }
        let facts = BuildFacts::current(display_backend(window));
        self.about = Some(about_details(&facts));
        self.about_initial = AboutButton::default();
        self.sync_dialog(window, cx);
        cx.notify();
        Ok(CommandOutcome::Completed)
    }

    /// The top dialog, which the window's dialog slot shows.
    fn top_dialog(&self) -> Option<ShownDialog> {
        if self.close.confirmation.is_some() {
            Some(ShownDialog::Close(self.close.generation))
        } else if self.licenses.is_some() {
            Some(ShownDialog::Licenses)
        } else {
            self.about.is_some().then_some(ShownDialog::About)
        }
    }

    /// Opens the dialog slot for the top dialog, which moves focus into it
    /// on its first frame, or closes it when none shows. Focus left in the
    /// closed dialog returns to the window's focus handle, and the caller
    /// then focuses the terminal where it should; focus already elsewhere,
    /// such as on a terminal, stays. Every change of top dialog comes here,
    /// and each frame checks it.
    fn sync_dialog(&mut self, window: &mut Window, cx: &mut App) {
        let top = self.top_dialog();
        if top == self.dialog_shown {
            return;
        }
        self.dialog_shown = top;
        if top.is_some() {
            self.dialog.open(&self.focus);
            return;
        }
        let elsewhere = window.focused(cx).filter(|focus| {
            self.close_buttons.focused(window) == "none"
                && self.about_buttons.focused(window).is_none()
                && *focus != self.focus
        });
        let _ = self.dialog.close(window, cx);
        if let Some(focus) = elsewhere {
            focus.focus(window, cx);
        }
    }

    /// The control focus returns to in the showing dialog: the one that
    /// last held focus while it still shows, or else the one it opens on.
    fn dialog_focus_target(&self, cx: &App) -> Option<FocusHandle> {
        let top = self.top_dialog()?;
        if let Some(last) = self.dialog.last_focused() {
            return Some(last);
        }
        match top {
            ShownDialog::Close(_) => Some(self.close_buttons.primary.clone()),
            ShownDialog::Licenses => self
                .licenses
                .as_ref()
                .map(|licenses| licenses.read(cx).focus_handle(cx)),
            ShownDialog::About => {
                Some(self.about_buttons.handle(self.about_initial).clone())
            }
        }
    }

    /// Whether one of the showing dialog's controls has focus.
    fn dialog_holds_focus(&self, window: &Window, cx: &App) -> bool {
        match self.top_dialog() {
            Some(ShownDialog::Close(_)) => {
                self.close_buttons.focused(window) != "none"
            }
            Some(ShownDialog::Licenses) => {
                self.licenses.as_ref().is_some_and(|licenses| {
                    licenses.read(cx).focus(window, cx).is_some()
                })
            }
            Some(ShownDialog::About) => {
                self.about_buttons.focused(window).is_some()
            }
            None => false,
        }
    }

    /// Returns focus to the showing dialog unless one of its controls has
    /// it: to the control that last held it, or else to the one it opened
    /// on, or to the search as the Licenses view replaces the About panel.
    fn focus_dialog(&self, window: &mut Window, cx: &mut App) {
        if self.dialog_holds_focus(window, cx) {
            return;
        }
        if let Some(target) = self.dialog_focus_target(cx) {
            target.focus(window, cx);
        }
    }

    /// Closes the About panel, and the Licenses view over it, and returns
    /// focus to the terminal.
    fn close_about(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.licenses = None;
        if self.about.take().is_none() {
            return;
        }
        self.sync_dialog(window, cx);
        self.focus_terminal(window, cx);
        cx.notify();
    }

    /// Opens the Licenses view over the About panel, its search focused.
    fn show_licenses(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.open_licenses(crate::licenses::load, window, cx);
    }

    /// [`Self::show_licenses`] with the notices read by `load`.
    fn open_licenses(
        &mut self,
        load: impl FnOnce() -> Result<
            crate::licenses::Catalog,
            crate::licenses::LoadError,
        > + Send
        + 'static,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.about.is_none() {
            return;
        }
        if self.licenses.is_some() {
            self.focus_dialog(window, cx);
            return;
        }
        let colors = self.palette_colors();
        let hints = self.config.window.shortcut_hints;
        let modal = self.dialog.clone();
        let licenses = cx.new(|cx| {
            self.frame_clock.observe(cx);
            LicensesView::new(
                modal,
                colors,
                hints,
                &self.frame_clock,
                load,
                window,
                cx,
            )
        });
        cx.subscribe_in(
            &licenses,
            window,
            |view, _, _: &LicensesDone, window, cx| {
                view.close_licenses(window, cx);
            },
        )
        .detach();
        self.licenses = Some(licenses);
        self.sync_dialog(window, cx);
        cx.notify();
    }

    /// Returns from the Licenses view to the About panel, with focus on its
    /// Licenses button.
    fn close_licenses(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.licenses.take().is_none() {
            return;
        }
        self.about_initial = AboutButton::Licenses;
        self.sync_dialog(window, cx);
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
            let views = self.pane_views_with_geometry(window, cx);
            let quake_presenting = self.quake_presenting();
            for (view, geometry) in views {
                view.update(cx, |terminal, cx| {
                    terminal.quiet_resize = quake_presenting;
                    terminal.geometry = geometry;
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

    /// Removes the window. With `quit_after`, a queued Quit continues
    /// through [`request_quit`] once the window is gone; the `quit` command
    /// would be refused while `quitting` is set.
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
                        cx.defer(request_quit);
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
                cx.defer(request_quit);
            }
        }
    }

    fn select(
        &mut self,
        id: TabId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.select_tab(id, true, window, cx);
    }

    /// Activates `id`, showing it and hiding the others; `focus` moves
    /// keyboard focus to its terminal.
    fn select_tab(
        &mut self,
        id: TabId,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !cx.global_mut::<Desktop>().windows.select(self.window, id) {
            return;
        }
        // Only a pane drag's own spring load switches tabs under it.
        self.interrupt_pane_drag(id);
        if self
            .divider_drag
            .as_ref()
            .is_some_and(|drag| drag.tab != id)
        {
            self.cancel_divider_drag(cx);
        }
        self.sync_tab_layout(window, cx);
        self.reveal_active(window, cx);
        let visible = self.quake_visible();
        let zooms: Vec<Option<PaneId>> = self
            .tabs
            .iter()
            .map(|tab| self.shown_zoom(tab, cx))
            .collect();
        for (tab, zoomed) in self.tabs.iter().zip(zooms) {
            let focused = tab.focused().pane;
            for pane in &tab.panes {
                // A zoomed pane hides the others, as a hidden tab does.
                let shown = zoomed.is_none_or(|zoomed| zoomed == pane.pane);
                pane.view.update(cx, |view, cx| {
                    let was_visible = view.visible;
                    view.visible = visible && tab.id == id && shown;
                    if view.visible {
                        view.bell.viewed(window.is_window_active());
                        view.resize_if_needed(window);
                        if focus && pane.pane == focused {
                            view.focus.focus(window, cx);
                        }
                        view.start_initial_snapshot(cx);
                    } else {
                        view.hide(cx);
                    }
                    if was_visible != view.visible {
                        cx.notify();
                    }
                });
            }
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
        let count = self.tabs.len();
        let next = if forward {
            index + 1
        } else {
            index + count.saturating_sub(1)
        };
        // The model and the views agree on a window's tabs, but a window
        // without tab views has nothing to cycle through.
        if let Some(tab) =
            next.checked_rem(count).and_then(|next| self.tabs.get(next))
        {
            self.select_from_command(tab.id, window, cx);
        }
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
        self.cancel_pane_drag(window, cx);
        self.cancel_divider_drag(cx);
        self.resizing_sidebar = false;
        let Some(target) = self.current_close_target(target) else {
            self.release_exit_close_if_idle(cx);
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
        self.sync_dialog(window, cx);
        let Some(request) = self.close_request(&target, window, cx) else {
            self.release_exit_close_if_idle(cx);
            return;
        };
        let generation = self.close.generation;
        let checking = target.clone();
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let task = cx.background_executor().spawn(async move {
            #[cfg(all(test, feature = "test-support"))]
            if runtime
                .worker_hooks
                .enter(worker_hooks::Stage::Assess)
                .await
            {
                return Committed {
                    result: Err(RuntimeError::Stopped.into()),
                    seq: runtime.lock().hierarchy_seq(),
                };
            }
            let assessed = runtime.assess(request);
            #[cfg(all(test, feature = "test-support"))]
            runtime
                .worker_hooks
                .enter(worker_hooks::Stage::Assessed)
                .await;
            assessed
        });
        self.busy = true;
        let app = cx.to_async();
        cx.spawn_in(window, async move |view, cx| {
            let Committed { result, seq } = task.await;
            let resolution = if close_waits_for_projection(&checking, &result) {
                projected(&app, seq).await
            } else {
                Resolution::Ready
            };
            let _ = view.update_in(cx, |view, window, cx| {
                if view.close.generation != generation {
                    return;
                }
                view.busy = false;
                if resolution == Resolution::Cancelled {
                    // Teardown owns removal; decide nothing.
                    view.close.current = None;
                    return;
                }
                view.reconcile_own(window, cx);
                let assessment = match result {
                    Ok(assessment) => assessment,
                    Err(error) => {
                        view.close_assessment_failed(
                            &checking, &error, window, cx,
                        );
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
                        view.licenses = None;
                        view.focus.focus(window, cx);
                        view.sync_dialog(window, cx);
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

    /// Settles a close of `checking` the runtime would not assess. Each
    /// failure is reported at most once and never retried.
    ///
    /// - A window or emptied close whose attachment core invalidated
    ///   removes the window without a notice: its session closed elsewhere,
    ///   and nothing is left to close.
    /// - A refused emptied close keeps the window, as
    ///   [`Self::emptied_close_refused`] describes.
    /// - Another emptied failure raises one notice and clears the mark;
    ///   queued closes, including Quit, run next.
    /// - Any other failure raises one notice and cancels the close.
    fn close_assessment_failed(
        &mut self,
        checking: &CloseTarget,
        error: &MuxError,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match (checking, error) {
            (
                CloseTarget::Window | CloseTarget::Emptied,
                MuxError::UnknownAttachment(_),
            ) => self.remove_detached_window(window, cx),
            (CloseTarget::Emptied, MuxError::ViewNotEmpty(_)) => {
                self.emptied_close_refused(window, cx);
            }
            // The pane moved to another tab this window shows between the
            // close's request and its assessment; assess it there.
            (
                CloseTarget::Pane { tab, pane },
                MuxError::UnknownPane(_) | MuxError::UnknownTab(_),
            ) if self
                .tab_showing(*pane)
                .is_some_and(|current| current != *tab) =>
            {
                let target = CloseTarget::Pane {
                    tab: self.tab_showing(*pane).unwrap_or(*tab),
                    pane: *pane,
                };
                self.request_close(target, window, cx);
            }
            // The pane left this window before its close was assessed; the
            // close never reached Mux, so an exit-close returns to owed for
            // the window that shows the pane now. Queued closes still run.
            (
                CloseTarget::Pane { pane, .. },
                MuxError::UnknownPane(_) | MuxError::UnknownTab(_),
            ) if self.tab_showing(*pane).is_none() => {
                self.close.current = None;
                self.release_exit_close_if_idle(cx);
                self.restore_tab_focus(window, cx);
                self.resume_close(window, cx);
                cx.notify();
            }
            (CloseTarget::Emptied, _) => {
                self.report_close_failure(
                    format!("Cannot assess close: {error}"),
                    cx,
                );
                self.close.current = None;
                self.close.emptied = EmptiedClose::None;
                self.resume_close(window, cx);
                cx.notify();
            }
            _ => {
                self.report_close_failure(
                    format!("Cannot assess close: {error}"),
                    cx,
                );
                cx.global_mut::<Desktop>().exit_closes.settle(self.window);
                self.cancel_close(window, cx);
            }
        }
    }

    /// Settles an emptied close the runtime refused while the window's
    /// workspace held tabs, once the projection reached the hierarchy it
    /// was refused at. A session closed meanwhile removes the window
    /// without a notice; a workspace left with nothing to install asks
    /// again; otherwise the window stays, marked refused while it shows no
    /// view, and installs the tabs.
    fn emptied_close_refused(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.close.current = None;
        if self.attached_session_gone(cx) {
            self.remove_detached_window(window, cx);
            return;
        }
        self.close.emptied = if !self.tabs.is_empty() {
            EmptiedClose::None
        } else if self.has_nothing_to_install(cx) {
            EmptiedClose::Owed
        } else {
            EmptiedClose::Refused
        };
        // Queued user closes run first.
        cx.notify();
    }

    /// Removes the window after the runtime detached its attachment or
    /// deleted its session; a queued Quit continues elsewhere.
    fn remove_detached_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let quit_after =
            matches!(self.close.pending, Some(CloseTarget::Application));
        cx.global_mut::<Desktop>()
            .windows
            .detach_attachment(self.window);
        self.remove_window(window, cx, quit_after);
    }

    /// The runtime request for `target`; `None` when this window has
    /// nothing to send. A window or emptied close without an attachment
    /// removes the window at once.
    fn close_request(
        &mut self,
        target: &CloseTarget,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<CloseRequest> {
        Some(match target {
            CloseTarget::Application => CloseRequest::Application,
            CloseTarget::Window | CloseTarget::Emptied => {
                let Some(attachment) = self.attachment_id(cx) else {
                    self.remove_window(window, cx, false);
                    return None;
                };
                if *target == CloseTarget::Window {
                    CloseRequest::Window(attachment)
                } else {
                    CloseRequest::EmptiedWindow {
                        attachment,
                        workspace: self.workspace_id(cx)?,
                    }
                }
            }
            CloseTarget::Tab(tab) => CloseRequest::Tab {
                workspace: self.workspace_id(cx)?,
                tab: *tab,
            },
            CloseTarget::Pane { tab, pane } => CloseRequest::Pane {
                workspace: self.workspace_id(cx)?,
                tab: self.tab_showing(*pane).unwrap_or(*tab),
                pane: *pane,
            },
            CloseTarget::Tabs(tabs) => CloseRequest::Tabs {
                workspace: self.workspace_id(cx)?,
                tabs: tabs.clone(),
            },
        })
    }

    /// Returns the exit-close this window took to owed when no operation
    /// of the window still holds it, and offers it to the other windows
    /// showing its tab.
    fn release_exit_close_if_idle(&self, cx: &mut App) {
        if self.busy || self.close.confirmation.is_some() {
            return;
        }
        if cx
            .global_mut::<Desktop>()
            .exit_closes
            .release(self.window)
            .is_some()
        {
            offer_exit_closes(cx, Some(self.window));
        }
    }

    /// The tab whose view in this window shows `pane`, wherever the pane
    /// moved.
    fn tab_showing(&self, pane: PaneId) -> Option<TabId> {
        self.tabs
            .iter()
            .find(|view| view.pane(pane).is_some())
            .map(|view| view.id)
    }

    /// Restricts tab targets to current tabs, in window order, and points a
    /// pane target at the tab now showing its pane; `None` when nothing
    /// remains to close.
    fn current_close_target(&self, target: CloseTarget) -> Option<CloseTarget> {
        match target {
            CloseTarget::Tab(id) => self
                .tabs
                .iter()
                .any(|tab| tab.id == id)
                .then_some(CloseTarget::Tab(id)),
            CloseTarget::Pane { pane, .. } => self
                .tab_showing(pane)
                .map(|tab| CloseTarget::Pane { tab, pane }),
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

    /// Commits the confirmation of `target` from its Confirm button, drawn
    /// at `generation`. A stale click, dispatched against a frame drawn
    /// before Cancel or before a new check replaced the dialog, does
    /// nothing; `finish_close` would otherwise assess it again.
    fn confirm_confirmation(
        &mut self,
        target: CloseTarget,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        #[cfg(test)]
        {
            self.close.confirm_clicks += 1;
        }
        if self.close.can_confirm(&target, generation) {
            self.finish_close(target, window, cx);
        }
    }

    fn cancel_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.busy = false;
        self.reconcile_own(window, cx);
        let cancelled = self.close.cancel();
        // A cancelled close never reached the runtime.
        self.release_exit_close_if_idle(cx);
        if matches!(cancelled, Some(CloseTarget::Application)) {
            #[cfg(target_os = "macos")]
            native_quit::cancel_request();
            cx.global_mut::<Desktop>().quitting = false;
        }
        self.sync_dialog(window, cx);
        if let Some(tab) = self.active_view(cx) {
            let focus = tab.read(cx).focus.clone();
            focus.focus(window, cx);
        }
        cx.notify();
    }

    /// Whether the window no longer shows any tab or pane `target` names;
    /// window-wide targets always apply.
    fn shows_none_of(&self, target: &CloseTarget) -> bool {
        match target {
            CloseTarget::Pane { pane, .. } => self.tab_showing(*pane).is_none(),
            target => target.tabs().is_some_and(|tabs| {
                !tabs
                    .iter()
                    .any(|tab| self.tabs.iter().any(|v| v.id == *tab))
            }),
        }
    }

    fn resume_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if self
            .close
            .confirmation
            .as_ref()
            .is_some_and(|target| self.shows_none_of(target))
        {
            // Every tab or pane it asks about left the window, so its commit
            // could only go stale. Queued closes stay queued and run below.
            self.close.dismiss_confirmation();
            self.sync_dialog(window, cx);
            self.release_exit_close_if_idle(cx);
            self.restore_tab_focus(window, cx);
            cx.notify();
        }
        let id = self.window;
        let tabs = &self.tabs;
        let shows = |tab: TabId| tabs.iter().any(|view| view.id == tab);
        // A closing window or teardown takes no automatic close.
        let can_take = !terminating(cx)
            && self.record(cx).is_some_and(|record| !record.closing);
        let target = self
            .close
            .next_request(self.busy, shows, || {
                if !can_take {
                    return None;
                }
                let desktop = cx.global_mut::<Desktop>();
                let state = desktop.hierarchy.state();
                let pane_tab = |pane: PaneId| {
                    state.pane_tab(pane).filter(|tab| {
                        tabs.iter().any(|view| {
                            view.id == *tab && view.pane(pane).is_some()
                        })
                    })
                };
                let owed: Vec<(PaneId, TabId)> = desktop
                    .exit_closes
                    .owed()
                    .filter_map(|pane| Some((pane, pane_tab(pane)?)))
                    .collect();
                let pane = desktop.exit_closes.take(id, |pane| {
                    owed.iter().any(|(owed, _)| *owed == pane)
                })?;
                let (_, tab) =
                    owed.into_iter().find(|(owed, _)| *owed == pane)?;
                Some(CloseTarget::Pane { tab, pane })
            })
            .filter(|target| can_take || *target != CloseTarget::Emptied);
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
        self.sync_dialog(window, cx);
        self.busy = true;
        let generation = self.close.generation;
        // From here the commit's result decides the exit-close, even if
        // this window closes first.
        let exit = {
            let desktop = cx.global_mut::<Desktop>();
            desktop
                .exit_closes
                .begin_commit(self.window)
                .and_then(|pane| {
                    Some((desktop.hierarchy.state().pane_tab(pane)?, pane))
                })
        };
        let id = self.window;
        self.publish_restorable_bounds(cx);
        let windows = if matches!(target, CloseTarget::Application) {
            cx.global_mut::<Desktop>().quitting = true;
            Some(capture_windows(cx))
        } else {
            None
        };
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let task = cx.background_executor().spawn(async move {
            #[cfg(all(test, feature = "test-support"))]
            if runtime
                .worker_hooks
                .enter(worker_hooks::Stage::Commit)
                .await
            {
                // A teardown failure after the runtime accepted the close.
                let mut committed =
                    runtime.commit(&assessment, confirmed, windows);
                committed.result = Err(RuntimeError::Stopped.into());
                return committed;
            }
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
            if let Some((tab, pane)) = exit {
                let outcome = exit_commit_outcome(&result, &target, tab, pane);
                app.update(|cx| finish_exit_commit(cx, id, pane, outcome));
            }
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
                    view.report_close_failure(message, cx);
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
                    CloseTarget::Window | CloseTarget::Emptied => {
                        // The runtime detached the attachment or deleted its
                        // session, even when terminal teardown then failed.
                        view.remove_detached_window(window, cx);
                    }
                    CloseTarget::Tab(id) => {
                        view.remove_closed_tabs(&[id], window, cx);
                    }
                    CloseTarget::Pane { tab, pane } => {
                        view.remove_closed_pane(tab, pane, window, cx);
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

    /// Drops the view of a committed pane close. When the pane was its
    /// tab's last, the runtime closed the tab, which is removed as a tab
    /// close is.
    fn remove_closed_pane(
        &mut self,
        tab: TabId,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if cx.global::<Desktop>().hierarchy.state().tab(tab).is_none() {
            self.remove_closed_tabs(&[tab], window, cx);
            return;
        }
        self.drop_pane_views(tab, &[pane], window, cx);
        self.reconcile(&Touched::default(), terminating(cx), cx);
        if self.active_tab(cx) == Some(tab) {
            self.select(tab, window, cx);
        }
        if self.resume_close(window, cx) {
            return;
        }
        cx.notify();
    }

    /// Drops the records of committed tab closes, then resumes queued
    /// closes, which include the emptied close once no tab remains.
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
        if self.tabs.is_empty() {
            self.owe_emptied_close(cx);
        }
        if self.resume_close(window, cx) {
            return;
        }
        cx.notify();
    }
}

/// Starts the configuration watcher and hands it what the startup load
/// consumed. Its reports reach the desktop on the UI thread.
fn start_config_watch(cx: &mut App, inputs: config::Inputs) {
    let (sender, receiver) = async_channel::unbounded();
    let handle = config_watch::spawn(move |report| {
        let _ = sender.send_blocking(report);
    });
    cx.global_mut::<Desktop>().watch = Some(handle);
    configure_watch(cx, inputs);
    cx.spawn(async move |cx| {
        while let Ok(report) = receiver.recv().await {
            cx.update(|cx| handle_watch_report(cx, report));
        }
    })
    .detach();
}

/// Tells the watcher what the last load or reload consumed and whether the
/// effective configuration wants watching. A failed reload keeps the
/// previous configuration, and with it the previous setting.
fn configure_watch(cx: &App, inputs: config::Inputs) {
    let desktop = cx.global::<Desktop>();
    if let Some(watch) = &desktop.watch {
        watch.configure(config_watch::Settings {
            enabled: desktop.config.general.watch_config,
            config: desktop.config_path.clone(),
            inputs,
        });
    }
}

fn handle_watch_report(cx: &mut App, report: config_watch::Report) {
    match report {
        config_watch::Report::Changed { differs, .. } => {
            if cx.global_mut::<Desktop>().reloads.changed(differs) {
                run_reload(cx, ReloadKind::Automatic);
            }
        }
        config_watch::Report::Status {
            watching,
            reconciliations,
        } => {
            if watching {
                clear_watch_unavailable(cx);
            }
            #[cfg(target_os = "macos")]
            {
                cx.global_mut::<Desktop>().watch_status = WatchStatus {
                    watching,
                    reconciliations,
                };
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (watching, reconciliations);
        }
        config_watch::Report::Unavailable(error) => {
            show_watch_unavailable(cx, &error);
        }
    }
}

const WATCH_UNAVAILABLE: &str = "Automatic reload unavailable";

/// Raises the warning that changes in a directory the watcher cannot watch
/// will not reload on their own, in the active window or, without one, the
/// next window to show. The watcher raises it again only after a reconcile
/// ends with every watch held.
fn show_watch_unavailable(cx: &mut App, error: &str) {
    eprintln!("Huterm configuration watcher unavailable: {error}");
    let content = NoticeContent {
        severity: Severity::Warning,
        source: NoticeSource::Command,
        title: WATCH_UNAVAILABLE.into(),
        message: format!(
            "Some configuration files will not reload automatically: {error}. Reload still works."
        ),
        location: Some(path_for_status(cx)),
        actions: vec![super::notices::NoticeAction {
            label: "Reload".into(),
            command: bare(ids::RELOAD_CONFIG),
        }],
        lifetime: super::notices::Lifetime::Persistent,
    };
    let shown = cx.active_window().is_some_and(|window| {
        window
            .update(cx, |root, _, cx| {
                root.downcast::<WorkspaceView>().is_ok_and(|view| {
                    view.update(cx, |view, cx| {
                        if !view.can_show_notices(cx) {
                            return false;
                        }
                        view.notify(content.clone(), cx);
                        true
                    })
                })
            })
            .unwrap_or(false)
    });
    if !shown {
        cx.global_mut::<Desktop>().latched.push(content);
    }
}

/// Takes the watch warning back once every watch holds again, from the
/// latch and from every window, since it no longer describes the watcher.
fn clear_watch_unavailable(cx: &mut App) {
    let warning = |content: &NoticeContent| {
        content.source == NoticeSource::Command
            && content.title == WATCH_UNAVAILABLE
    };
    cx.global_mut::<Desktop>()
        .latched
        .retain(|content| !warning(content));
    for handle in cx.windows() {
        let _ = handle.update(cx, |root, _, cx| {
            if let Ok(view) = root.downcast::<WorkspaceView>() {
                view.update(cx, |view, cx| {
                    if view.notices.dismiss_where(warning) {
                        cx.notify();
                    }
                });
            }
        });
    }
}

/// Reloads on request, whatever the fingerprints say. During another
/// reload it runs right after that one.
fn reload(cx: &mut App) -> CommandOutcome {
    if cx.global_mut::<Desktop>().reloads.manual() {
        run_reload(cx, ReloadKind::Manual);
    }
    CommandOutcome::Accepted
}

/// Rereads the configuration on the background executor and applies it.
/// The caller has latched `Desktop::reloads`. An automatic reload does not
/// start once Quit is committed. Until then one may start, both while Quit
/// awaits confirmation, which can still be cancelled, and while its teardown
/// runs: Quit unregisters the global shortcuts only once approved, and
/// `finish_reload` applies nothing after.
fn run_reload(cx: &mut App, kind: ReloadKind) {
    if kind == ReloadKind::Automatic && cx.global::<Desktop>().quit_committed {
        follow_up_reload(cx);
        return;
    }
    let path = cx.global::<Desktop>().config_path.clone();
    let reload_path = path.clone();
    // Taken before the read, so every rename it counts is in the file read.
    let renames = cx.global::<Desktop>().windows.quake_renames_noted();
    let task = cx
        .background_executor()
        .spawn(async move { config::reload(&reload_path) });
    cx.spawn(async move |cx| {
        let reloaded = task.await;
        cx.update(|cx| finish_reload(cx, kind, &path, reloaded, renames));
    })
    .detach();
}

/// Applies a reload that read the file once `renames` quake profile
/// renames were noted.
fn finish_reload(
    cx: &mut App,
    kind: ReloadKind,
    path: &Path,
    reloaded: config::Reloaded,
    renames: u64,
) {
    // Quit already unregistered the global shortcuts and is closing the
    // windows; applying now would register the shortcuts again.
    if cx.global::<Desktop>().quit_committed {
        return;
    }
    let config::Reloaded {
        config: result,
        inputs,
        missing,
    } = reloaded;
    // An editor saving by renaming leaves no file for a moment. Skip the
    // round quietly; the event that brings the file back reloads again.
    if kind == ReloadKind::Automatic && missing {
        follow_up_reload(cx);
        return;
    }
    let result = result.and_then(|config| {
        let (family, metrics) =
            resolve_metrics(&config, cx).map_err(|error| error.to_string())?;
        let compiled =
            keymap::compile(Platform::current(), &config.keybindings)
                .map_err(|error| format!("{}: {error}", path_for_status(cx)))?;
        let shortcuts = quake_windows::compile_shortcuts(&config, &compiled)?;
        Ok((config, family, metrics, compiled, shortcuts))
    });
    let result =
        result.map(|(config, family, metrics, compiled, shortcuts)| {
            // A shortcut the platform refuses, such as one another application
            // holds, warns without blocking the rest of the configuration.
            let refused = quake_windows::replace_registrations(cx, shortcuts);
            #[cfg(all(target_os = "macos", feature = "macos-updater"))]
            apply_update_config(cx, &config);
            let desktop = cx.global_mut::<Desktop>();
            desktop.config = config.clone();
            desktop.config_report =
                ConfigReport::reloaded(&config, &compiled, refused);
            desktop.latched.clear();
            quake_windows::reconcile(cx, renames);
            let keymap = bind_keymap(cx, compiled);
            cx.global_mut::<Desktop>().keymap = keymap;
            maybe_exit(cx);
            (config, family, metrics)
        });
    report_config_reload_error(&result);
    if let Err(error) = &result {
        cx.global_mut::<Desktop>().config_report.reload_failure =
            Some(error.clone());
    }
    settings_window::reloaded(cx, kind == ReloadKind::Automatic);
    let desktop = cx.global::<Desktop>();
    let active = desktop.config_report.notices(path);
    let diagnostics = match &result {
        Ok(_) => active,
        Err(error) => failed_reload_diagnostics(error, &active),
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
    configure_watch(cx, inputs);
    follow_up_reload(cx);
}

/// Releases the reload latch, running one more reload when something
/// changed while this one ran.
fn follow_up_reload(cx: &mut App) {
    if let Some(kind) = cx.global_mut::<Desktop>().reloads.finished() {
        run_reload(cx, kind);
    }
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
            // `layout_pending` resizes the views at the next sync.
            for (view, geometry) in self.pane_views_with_geometry(window, cx) {
                view.update(cx, |view, cx| {
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
                    view.geometry = geometry;
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
    /// The frame for a `platform` window whose tabs resolved to
    /// `position`. Only a Linux window drawing the merged row owns a
    /// border; its inset follows the tiling, maximized, and fullscreen
    /// state GPUI reports, and its row draws the desktop's `buttons`.
    fn resolve(
        platform: Platform,
        position: TabPosition,
        state: FrameState,
        buttons: ButtonLayout,
    ) -> Self {
        let controls =
            platform == Platform::Linux && position == TabPosition::Titlebar;
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
        // A pane drag whose pane left its tab, or under a tab switch it did
        // not make, ends even while the pointer rests.
        if self.pane_drag.is_some() && !self.pane_drag_valid(cx) {
            cx.defer_in(window, |view, window, cx| {
                if view.pane_drag.is_some() && !view.pane_drag_valid(cx) {
                    view.cancel_pane_drag(window, cx);
                }
            });
        }
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
        let swatch = tokens(&self.config.theme);
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
        // A dialog change that skipped `sync_dialog` still opens or closes
        // the slot before its guard reads it.
        self.sync_dialog(window, cx);
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
                    if view.divider_drag.is_some()
                        && event.keystroke.key == "escape"
                    {
                        view.cancel_divider_drag(cx);
                        cx.stop_propagation();
                    } else if view.reorder.is_some()
                        && event.keystroke.key == "escape"
                    {
                        view.cancel_reorder(window, cx);
                        // GPUI skips raw keystroke observers after propagation
                        // stops, so this Escape cannot also reach the terminal.
                        cx.stop_propagation();
                    } else if view
                        .pane_drag
                        .as_ref()
                        .is_some_and(|drag| drag.dragging)
                        && event.keystroke.key == "escape"
                    {
                        view.cancel_pane_drag(window, cx);
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
            // Only the palette and the Licenses view send focused-scope
            // commands, so without them the window takes none, and the Edit
            // menu's Cut, which is `text_cut`, is disabled over a terminal.
            .when(self.palette.is_some() || self.licenses.is_some(), |root| {
                root.on_action(cx.listener(Self::invoke_focused))
            })
            .on_action(cx.listener(
                |view, request: &FieldContextMenu, window, cx| {
                    view.open_field_menu(request, window, cx);
                },
            ))
            .child(huterm_ui::focus::tracker());
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
                                    if view.divider_drag.is_some() {
                                        view.drag_divider_to(
                                            event.position,
                                            window,
                                            cx,
                                        );
                                        cx.stop_propagation();
                                    } else if view.resizing_sidebar {
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
                                    } else if view.pane_drag.is_some() {
                                        view.update_pane_drag(
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
                        move |_: &gpui::MouseDownEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = press_view.update(cx, |view, cx| {
                                    view.pointer_reveal.outside = false;
                                    // Only a press the title row itself
                                    // accepts (in its bubble handler) may
                                    // become a window move.
                                    view.title_row_press.set(false);
                                    view.defer_pointer_refresh(window, cx);
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
                                    if view.divider_drag.is_some() {
                                        view.drag_divider_to(
                                            event.position,
                                            window,
                                            cx,
                                        );
                                        view.finish_divider_drag(window, cx);
                                        cx.stop_propagation();
                                    } else if view.resizing_sidebar {
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
                                    } else if view.pane_drag.is_some() {
                                        view.finish_pane_drag(
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
        // A press outside the open menu, or on its button, closes it and
        // stops there, even before the menu's first frame: nothing beneath
        // takes focus, starts a selection, or sends a mouse report. It
        // follows the listeners above, which every press must reach.
        let guard_view = cx.entity().downgrade();
        root = root.child(self.popover.guard(move |_, window, cx| {
            let _ = guard_view.update(cx, |view, cx| {
                view.close_menu(MenuFocusReturn::Terminal, window, cx);
            });
        }));
        // A dialog's guard stops every press and wheel event on its scrim
        // outside its panel, and anywhere before its first frame. None of
        // the window's dialogs closes on an outside press, so the reason it
        // reports is ignored.
        root = root.child(self.dialog.guard(|_, _, _| {}));
        let titlebar = self.title_row();
        let top_chrome = titlebar + self.fullscreen_insets.top.max(px(0.0));
        let bar_drawn = self.bar_drawn();
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
        root = root.children(self.pane_elements(
            layout.terminal,
            colors,
            window,
            cx,
        ));
        // A drag resolves against the drawn panes, which this frame may have
        // just replaced, as after a spring load; a resting pointer must not
        // keep the target it found before.
        if self.pane_drag.as_ref().is_some_and(|drag| drag.dragging) {
            cx.defer_in(window, |view, window, cx| {
                view.retarget_after_frame(window, cx);
            });
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
                .on_scroll(cx.listener(
                    move |view, event: &ScrollEvent, window, cx| {
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
                    targeted: self.menu_targets_tab(tab.id)
                        || self.pane_drag.as_ref().is_some_and(|drag| {
                            drag.spring.tab() == Some(tab.id)
                        }),
                };
                bar = bar.child(Self::tab_element(
                    &item, bounds, tabs, colors, swatch, cx,
                ));
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
                        Button::new(
                            if forward {
                                "scroll-tabs-forward"
                            } else {
                                "scroll-tabs-backward"
                            },
                            if forward {
                                "Scroll Tabs Right"
                            } else {
                                "Scroll Tabs Left"
                            },
                            swatch,
                        )
                        .colors(ButtonColors {
                            fill: Some(colors.bar),
                            ..colors.control()
                        })
                        .icon(
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
                        )
                        .icon_only()
                        .group("scroll-tabs")
                        .stop_presses()
                        .absolute()
                        .left(if vertical {
                            (strip.bounds.size.width - CONTROL_SIZE)
                                .max(px(0.0))
                                / 2.0
                        } else {
                            edge
                        })
                        .top(if vertical { edge } else { bar_inset })
                        .size(CONTROL_SIZE)
                        .rounded_md()
                        .on_click(Self::button_action(
                            cx,
                            move |view, window, cx| {
                                let amount = view.tab_strip(window).available()
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
                        )),
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
                Button::new("new-tab", "New Tab", swatch)
                    .colors(colors.control())
                    .icon(
                        icon_element(Icon::Plus, colors.inactive)
                            .group_hover("new-tab", |style| {
                                style.text_color(colors.foreground)
                            }),
                    )
                    .icon_only()
                    .group("new-tab")
                    .occlude()
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
                    .on_click(Self::button_action(cx, |view, window, cx| {
                        if let Err(error) = view.new_tab(window, cx) {
                            view.report_failure(
                                "Cannot open tab",
                                error.to_string(),
                                cx,
                            );
                        }
                    })),
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
                                view.cancel_pane_drag(window, cx);
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
            // In pane mode the preview returns to the tab's slot, dimmed.
            let preview = if drag.nesting {
                strip.label(source)
            } else {
                strip.preview(drag.pointer, source)
            };
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
                    .opacity(if drag.nesting { 0.45 } else { 0.9 })
                    .child(title),
            );
            if !drag.nesting {
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
        }
        root = root.children(self.drag_overlay(colors, window, cx));
        if let Some(notices) = self.render_notices(layout.terminal, cx) {
            root = root.child(notices);
        }
        if let Some(menu) = self.render_menu(&layout, window, cx) {
            root = root.child(menu);
        }
        // The window's one dialog slot shows the top dialog.
        self.drawn_dialog_groups = None;
        let scrim_corners = modal_corners(&layout);
        if let Some(target) = self.close.confirmation.clone() {
            let generation = self.close.generation;
            let input = self.close_dialog_input(&target, cx);
            self.drawn_dialog_groups = Some(dialog_group_titles(&input));
            let model = build_close_dialog(&input);
            let dialog = render_close_dialog(
                &model,
                &self.dialog,
                &self.close_buttons,
                content.size,
                tokens(&self.config.theme),
                self.config.window.shortcut_hints,
                Self::button_action(cx, |view, window, cx| {
                    view.cancel_confirmation(window, cx);
                }),
                Self::button_action(cx, move |view, window, cx| {
                    view.confirm_confirmation(
                        target.clone(),
                        generation,
                        window,
                        cx,
                    );
                }),
            )
            .scrim_corners(scrim_corners);
            root = root.child(modal_layer(dialog, &layout));
        } else if let Some(licenses) = &self.licenses {
            licenses.update(cx, |licenses, _| {
                licenses.set_viewport(content.size, scrim_corners);
            });
            root = root.child(modal_layer(
                div().absolute().inset_0().child(licenses.clone()),
                &layout,
            ));
        } else if let Some(details) = &self.about {
            let about = render_about(
                details,
                &self.dialog,
                &self.about_buttons,
                self.about_initial,
                content.size,
                tokens(&self.config.theme),
                self.config.window.shortcut_hints,
                AboutActions {
                    licenses: Self::button_action(cx, |view, window, cx| {
                        view.show_licenses(window, cx);
                    }),
                    copy: Self::button_action(cx, |view, _, cx| {
                        view.copy_about_details(cx);
                    }),
                    close: Self::button_action(cx, |view, window, cx| {
                        view.close_about(window, cx);
                    }),
                },
            )
            .scrim_corners(scrim_corners);
            root = root.child(modal_layer(about, &layout));
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

/// The corners a dialog's scrim rounds: a drawn client frame's rounded top
/// corners, or none.
fn modal_corners(layout: &ChromeLayout) -> gpui::Corners<Pixels> {
    if !layout.frame.decorated() {
        return gpui::Corners::default();
    }
    gpui::Corners {
        top_left: FRAME_RADIUS,
        top_right: FRAME_RADIUS,
        ..gpui::Corners::default()
    }
}

/// Hosts a dialog, whose scrim fills its parent: the whole window, or the
/// content inside a drawn client frame, so the transparent border around
/// the frame's shadow stays untinted and the frame's resize edges, outside
/// the scrim, still take presses. The dialog rounds its scrim with
/// [`modal_corners`].
fn modal_layer(
    dialog: impl IntoElement,
    layout: &ChromeLayout,
) -> gpui::AnyElement {
    if !layout.frame.decorated() {
        return dialog.into_any_element();
    }
    let content = layout.content;
    div()
        .absolute()
        .left(content.origin.x)
        .top(content.origin.y)
        .w(content.size.width)
        .h(content.size.height)
        .child(dialog)
        .into_any_element()
}

/// The split, close, and focus commands for panes.
fn is_pane_command(command: huterm_protocol::CommandId) -> bool {
    matches!(
        command,
        ids::SPLIT_RIGHT
            | ids::SPLIT_DOWN
            | ids::SPLIT_LEFT
            | ids::SPLIT_UP
            | ids::CLOSE_PANE
            | ids::FOCUS_NEXT_PANE
            | ids::FOCUS_PREVIOUS_PANE
            | ids::FOCUS_PANE_LEFT
            | ids::FOCUS_PANE_RIGHT
            | ids::FOCUS_PANE_UP
            | ids::FOCUS_PANE_DOWN
            | ids::RESIZE_PANE_LEFT
            | ids::RESIZE_PANE_RIGHT
            | ids::RESIZE_PANE_UP
            | ids::RESIZE_PANE_DOWN
            | ids::TOGGLE_PANE_ZOOM
    )
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
        let has =
            |direction| self.layout_restore_label(direction, cx).is_some();
        let restorable = window_menu::Restorable {
            undo: has(RestoreDirection::Undo),
            redo: has(RestoreDirection::Redo),
        };
        WindowMenuInput::for_build(
            selection,
            self.notices.contents().len(),
            restorable,
        )
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
        self.open_window_menu(from_keyboard, None, window, cx)
    }

    /// [`Self::open_menu`], at `pointer` for a context menu.
    fn open_window_menu(
        &mut self,
        from_keyboard: bool,
        pointer: Option<gpui::Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.check_available(false)?;
        if self.native_menu.is_some() {
            return Err(CommandError::Unavailable("a menu is open".to_owned()));
        }
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
        let spec = window_menu(
            &self.window_menu_input(cx),
            &cx.global::<Desktop>().keymap,
            &contexts,
        );
        let anchor = pointer.map_or_else(
            || MenuAnchor::Below(self.menu_anchor(&self.chrome_layout(window))),
            MenuAnchor::Pointer,
        );
        if self.open_native_menu(MenuKind::Window, &spec, anchor, window, cx) {
            return Ok(CommandOutcome::Completed);
        }
        self.mount_menu(
            spec.to_listbox(Platform::current()),
            MenuKind::Window,
            pointer,
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
        if let Err(error) =
            self.open_window_menu(false, Some(pointer), window, cx)
        {
            self.report_failure("Open Menu", error.to_string(), cx);
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
            || self.native_menu.is_some()
            || !self.tabs.iter().any(|record| record.id == tab)
        {
            return;
        }
        self.close_menu(MenuFocusReturn::Keep, window, cx);
        if let Some(terminal) = self.active_view(cx) {
            terminal.update(cx, TerminalView::clear_composition);
        }
        let contexts = window.context_stack();
        let Some(spec) = self.tab_menu_spec(tab, &contexts, cx) else {
            return;
        };
        let anchor = MenuAnchor::Pointer(pointer);
        if self.open_native_menu(MenuKind::Tab(tab), &spec, anchor, window, cx)
        {
            return;
        }
        self.mount_menu(
            spec.to_listbox(Platform::current()),
            MenuKind::Tab(tab),
            Some(pointer),
            contexts,
            false,
            window,
            cx,
        );
    }

    /// Opens the context menu of `pane`'s title bar at `pointer`, replacing
    /// any open menu. The pane takes focus first, as a left press gives it,
    /// so the menu's focused-pane commands act on it. It refuses where the
    /// tab menu does, and for a pane the active tab does not show.
    fn open_pane_menu(
        &mut self,
        tab: TabId,
        pane: PaneId,
        pointer: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.check_available(true).is_err()
            || self.palette.is_some()
            || self.native_menu.is_some()
            || self.active_tab(cx) != Some(tab)
            || self
                .active_tab_view(cx)
                .is_none_or(|view| view.pane(pane).is_none())
        {
            return;
        }
        self.close_menu(MenuFocusReturn::Keep, window, cx);
        // The pane typing went to, before the press moves focus.
        if let Some(terminal) = self.active_view(cx) {
            terminal.update(cx, TerminalView::clear_composition);
        }
        self.focus_pane(tab, pane, window, cx);
        let contexts = window.context_stack();
        let kind = MenuKind::Pane { tab, pane };
        let Some(spec) = self.pane_menu_spec(&kind, &contexts, cx) else {
            return;
        };
        let anchor = MenuAnchor::Pointer(pointer);
        if self.open_native_menu(kind.clone(), &spec, anchor, window, cx) {
            return;
        }
        self.mount_menu(
            spec.to_listbox(Platform::current()),
            kind,
            Some(pointer),
            contexts,
            false,
            window,
            cx,
        );
    }

    /// The pane menu's rows, or `None` once its pane is no longer the
    /// active tab's focused pane, which its picks act on.
    fn pane_menu_spec(
        &self,
        kind: &MenuKind,
        contexts: &[KeyContext],
        cx: &App,
    ) -> Option<MenuSpec> {
        let MenuKind::Pane { tab, pane } = *kind else {
            return None;
        };
        let view = self.active_tab_view(cx).filter(|view| view.id == tab)?;
        let focused = view.focused();
        if focused.pane != pane {
            return None;
        }
        let input = PaneMenuInput {
            platform: Platform::current(),
            zoomed: self.shown_zoom(view, cx) == Some(pane),
            directory_known: tab_directory_path(
                &focused.view.read(cx).metadata,
            )
            .is_some(),
        };
        Some(pane_menu(input, &cx.global::<Desktop>().keymap, contexts))
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
        if self.check_available(true).is_err()
            || self.palette.is_some()
            || self.native_menu.is_some()
        {
            return;
        }
        self.close_menu(MenuFocusReturn::Keep, window, cx);
        if let Some(terminal) = self.active_view(cx) {
            terminal.update(cx, TerminalView::clear_composition);
        }
        let contexts = window.context_stack();
        let kind = MenuKind::Terminal { tab, link };
        let Some(spec) = self.terminal_menu_spec(&kind, &contexts, cx) else {
            return;
        };
        let anchor = MenuAnchor::Pointer(pointer);
        if self.open_native_menu(kind.clone(), &spec, anchor, window, cx) {
            return;
        }
        self.mount_menu(
            spec.to_listbox(Platform::current()),
            kind,
            Some(pointer),
            contexts,
            false,
            window,
            cx,
        );
    }

    /// Opens a text field's context menu where its right press landed,
    /// replacing any open menu. Only the palette's and Licenses' fields
    /// ask, and editing a field needs nothing from the window, so it opens
    /// whenever the field is still there.
    fn open_field_menu(
        &mut self,
        request: &FieldContextMenu,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.close_menu(MenuFocusReturn::Keep, window, cx);
        let contexts = window.context_stack();
        let kind = MenuKind::Field(request.field.clone());
        let Some(model) = Self::field_menu_model(&kind, &contexts, cx) else {
            return;
        };
        self.mount_menu(
            model,
            kind,
            Some(request.position),
            contexts,
            false,
            window,
            cx,
        );
    }

    /// A field menu's rows, or `None` once its field is gone.
    fn field_menu_model(
        kind: &MenuKind,
        contexts: &[KeyContext],
        cx: &App,
    ) -> Option<MenuModel> {
        let MenuKind::Field(field) = kind else {
            return None;
        };
        let field = field.upgrade()?;
        Some(field_menu::field_menu_model(
            field_menu::FieldMenuInput::of(field.read(cx)),
            &cx.global::<Desktop>().keymap,
            contexts,
        ))
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
        // A right-click while a menu is open only closes it, before the
        // terminal sees it, so an open menu here was opened after the
        // click; a lookup that finished late must not replace it.
        if self.active_view(cx).as_ref() == Some(terminal)
            && self.menu.is_none()
            && self.native_menu.is_none()
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
    fn terminal_menu_spec(
        &self,
        kind: &MenuKind,
        contexts: &[KeyContext],
        cx: &App,
    ) -> Option<MenuSpec> {
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
            split: self
                .active_tab_view(cx)
                .is_some_and(|tab| tab.panes.len() > 1),
            directory,
        };
        Some(terminal_menu(
            input,
            &cx.global::<Desktop>().keymap,
            contexts,
        ))
    }

    /// The tab menu's rows for `tab`, or `None` once the tab is gone.
    fn tab_menu_spec(
        &self,
        tab: TabId,
        contexts: &[KeyContext],
        cx: &App,
    ) -> Option<MenuSpec> {
        let (index, view) = self
            .tabs
            .iter()
            .enumerate()
            .find(|(_, record)| record.id == tab)?;
        let directory_known =
            tab_directory_path(&view.focused_view().read(cx).metadata)
                .is_some();
        let input = TabMenuInput {
            platform: Platform::current(),
            index,
            count: self.tabs.len(),
            active: self.active_tab(cx) == Some(tab),
            vertical: self.layout_tabs().position.vertical(),
            directory_known,
        };
        Some(tab_menu(&input, &cx.global::<Desktop>().keymap, contexts))
    }

    /// The rows for whichever menu is open, or `None` once its target is
    /// gone.
    fn current_menu_model(&self, cx: &App) -> Option<MenuModel> {
        let menu = self.menu.as_ref()?;
        let spec = match &menu.kind {
            MenuKind::Window => Some(window_menu(
                &self.window_menu_input(cx),
                &cx.global::<Desktop>().keymap,
                &menu.contexts,
            )),
            MenuKind::Tab(tab) => self.tab_menu_spec(*tab, &menu.contexts, cx),
            kind @ MenuKind::Terminal { .. } => {
                self.terminal_menu_spec(kind, &menu.contexts, cx)
            }
            kind @ MenuKind::Pane { .. } => {
                self.pane_menu_spec(kind, &menu.contexts, cx)
            }
            kind @ MenuKind::Field(_) => {
                Self::field_menu_spec(kind, &menu.contexts, cx)
            }
        };
        spec.map(|spec| spec.to_listbox(Platform::current()))
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
        // An opening menu ends a tab or pane drag; the menu then takes focus.
        self.cancel_reorder(window, cx);
        self.cancel_pane_drag(window, cx);
        let swatch = tokens(&self.config.theme);
        let clock = Rc::clone(&self.frame_clock);
        let label = match kind {
            MenuKind::Window => "Menu",
            MenuKind::Tab(_) => "Tab",
            MenuKind::Terminal { .. } => "Terminal",
            MenuKind::Pane { .. } => "Pane",
            MenuKind::Field(_) => "Edit",
        };
        let view = cx.new(|cx| {
            clock.observe(cx);
            MenuView::new(label, model, swatch, from_keyboard, cx)
        });
        cx.subscribe_in(&view, window, Self::handle_menu_event)
            .detach();
        // A menu supersedes a right-click still waiting for its link
        // lookup, as any terminal press does: the press that dismisses this
        // menu stops at the popover and never reaches the terminal to
        // cancel it.
        if let Some(terminal) = self.active_view(cx) {
            terminal.update(cx, |terminal, _| terminal.cancel_context_lookup());
        }
        // Focus leaving the menu closes it where focus went. The closes
        // that move focus themselves drop the menu first, so this finds
        // nothing to close then. Blur runs in the draw's focus phase, which
        // drops a notify, so the close waits for the draw to end. The
        // activation observer closes a menu whose window deactivates before
        // this runs; skipping an inactive window only guards that order.
        let menu_id = view.entity_id();
        let menu_focus = view.read(cx).focus_handle(cx);
        let blur = cx.on_blur(&menu_focus, window, move |_, window, cx| {
            cx.defer_in(window, move |view, window, cx| {
                let current =
                    view.menu.as_ref().map(|menu| menu.view.entity_id());
                if current == Some(menu_id) && window.is_window_active() {
                    view.close_menu(MenuFocusReturn::Keep, window, cx);
                }
            });
        });
        menu_focus.focus(window, cx);
        // The button's anchor follows its painted bounds at every render.
        self.popover.open(
            pointer.map_or_else(
                || {
                    Anchor::Trigger(
                        self.menu_button_bounds.get().unwrap_or_default(),
                    )
                },
                Anchor::Pointer,
            ),
            true,
        );
        self.menu = Some(OpenMenu {
            view,
            kind,
            pointer,
            contexts,
            _blur: blur,
        });
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
        let _ = self.popover.close();
        let Some(menu) = self.menu.take() else {
            return false;
        };
        match (focus, menu.kind) {
            (MenuFocusReturn::Terminal, MenuKind::Field(field))
                if let Some(field) = field.upgrade() =>
            {
                field.read(cx).focus_handle(cx).focus(window, cx);
            }
            (MenuFocusReturn::Terminal, _) => self.focus_terminal(window, cx),
            (MenuFocusReturn::Keep, _) => {}
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
        self.refresh_native_menu(cx);
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
                self.run_menu_pick(&kind, id, window, cx);
            }
        }
    }

    /// Runs the item `id` picked from a menu of `kind`.
    fn run_menu_pick(
        &mut self,
        kind: &MenuKind,
        id: &str,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let MenuKind::Terminal {
            link: Some(link), ..
        } = kind
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
        // Context menu picks that take a tab act on the menu's tab, and a
        // pane menu's picks that take an optional pane act on its pane. A
        // required pane is a move's destination.
        let takes_tab = spec.args.iter().any(|arg| arg.name == "tab");
        let takes_pane = spec.args.iter().any(|arg| {
            arg.name == "pane"
                && arg.required == huterm_protocol::Requirement::Optional
        });
        let mut args = Vec::new();
        if let Some(tab) = kind.tab().filter(|_| takes_tab) {
            args.push(CommandArgument::new("tab", CommandValue::Tab(tab)));
        }
        if let Some(pane) = kind.pane().filter(|_| takes_pane) {
            args.push(CommandArgument::new("pane", CommandValue::Pane(pane)));
        }
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
                self.report_failure("Command failed", error.to_string(), cx);
            }
        }
        cx.notify();
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

    /// `run` on this view as a [`Button`]'s click action.
    pub(super) fn button_action(
        cx: &Context<'_, Self>,
        run: impl Fn(&mut Self, &mut Window, &mut Context<'_, Self>) + 'static,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        let view = cx.entity().downgrade();
        move |window, cx| {
            let _ = view.update(cx, |view, cx| run(view, window, cx));
        }
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
        }) || self
            .native_menu
            .as_ref()
            .is_some_and(|session| session.from_button)
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
    ) -> Button {
        let bounds_cell = Rc::clone(&self.menu_button_bounds);
        let open = self.menu_button_open();
        let dot = !open && !self.notices.is_empty();
        let swatch = tokens(&self.config.theme);
        Button::new("window-menu", "Menu", swatch)
            .colors(colors.control())
            .icon(
                icon_element(Icon::Menu, colors.inactive)
                    .w(px(14.0))
                    .h(px(14.0))
                    .group_hover("window-menu", |style| {
                        style.text_color(colors.foreground)
                    }),
            )
            .icon_only()
            .expanded(open)
            .group("window-menu")
            .occlude()
            .stop_presses()
            .size(CONTROL_SIZE)
            .rounded_md()
            .when(open, |button| button.bg(colors.control_hover))
            .on_click(Self::button_action(cx, |view, window, cx| {
                view.toggle_menu(window, cx);
                cx.stop_propagation();
            }))
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
            .child(self.popover.trigger_probe())
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
        let swatch = tokens(&self.config.theme);
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
            let element = Button::new(id, label, swatch)
                .colors(ButtonColors {
                    fill: Some(colors.hover),
                    ..colors.control()
                })
                .icon(
                    icon_element(icon, colors.inactive)
                        .group_hover(id, |style| {
                            style.text_color(colors.foreground)
                        }),
                )
                .icon_only()
                .group(id)
                .occlude()
                .stop_presses()
                .relative()
                .size(WINDOW_CONTROL_SIZE)
                .rounded_full()
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
                WindowButton::Minimize => element.on_click(|window, cx| {
                    window.minimize_window();
                    cx.stop_propagation();
                }),
                WindowButton::Maximize => element.on_click(|window, cx| {
                    window.zoom_window();
                    cx.stop_propagation();
                }),
                WindowButton::Close => element.on_click(Self::button_action(
                    cx,
                    |view, window, cx| {
                        view.request_close(CloseTarget::Window, window, cx);
                        cx.stop_propagation();
                    },
                )),
            });
        }
        controls
    }

    /// The open menu in its popover: below its button, or above it from
    /// the lower half; right-aligned from the right half; at the pointer
    /// for a context menu; scrolling within the space left.
    fn render_menu(
        &self,
        layout: &ChromeLayout,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<gpui::AnyElement> {
        let open = self.menu.as_ref()?;
        let menu = open.view.clone();
        let viewport = window.viewport_size();
        let height = MenuView::content_height(menu.read(cx).model());
        let anchor = match open.pointer {
            Some(pointer) => Anchor::Pointer(pointer),
            None => Anchor::Trigger(self.menu_anchor(layout)),
        };
        let placement = place(anchor, size(MENU_MIN_WIDTH, height), viewport);
        menu.update(cx, |menu, cx| {
            menu.set_max_height(Some(placement.max_height), cx);
        });
        self.popover.render(
            placement,
            viewport,
            div().w(MENU_MIN_WIDTH).child(menu),
        )
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
                    .unwrap_or_else(|| "none".into());
                let kind = match open.kind {
                    MenuKind::Window => "window",
                    MenuKind::Tab(_) => "tab",
                    MenuKind::Terminal { .. } => "terminal",
                    MenuKind::Pane { .. } => "pane",
                    MenuKind::Field(_) => "field",
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
        let native = match self.native_menu_kind() {
            Some(MenuKind::Window) => "window",
            Some(MenuKind::Tab(_)) => "tab",
            Some(MenuKind::Terminal { .. }) => "terminal",
            Some(MenuKind::Pane { .. }) => "pane",
            None => "none",
        };
        format!(
            "{menu} {prefix}native_menu={native} {prefix}menu_button_open={} {prefix}menu_rect={}",
            self.menu_button_open(),
            self.popover.content_bounds().map_or_else(
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

    /// The active tab's panes for smoke fixtures, as `<prefix>` fields:
    /// `panes` counts them, `pane_focus` is the focused pane's index, and
    /// `pane_zoomed` the zoomed one's, or `none`. In split layout order and
    /// `;`-separated, `pane_grids` holds each pane's requested grid as
    /// `columns,rows`, `pane_rects` its drawn terminal bounds in window
    /// points (a pane a zoom hides keeps its split bounds), and `pane_close`
    /// and `pane_zoom` its title bar buttons' centres as `x,y`, or `none`
    /// while the tab is not split. `pane_dividers` lists the drawn
    /// dividers' grab zones as `x,y,w,h`. `pane_layout` is the projected
    /// layout in canonical form, and `arranging` counts this window's
    /// arrangements in flight.
    pub(super) fn pane_smoke_state(
        &self,
        prefix: &str,
        window: &Window,
        cx: &App,
    ) -> String {
        let tab = self.active_tab_view(cx);
        let bounds = self.terminal_geometry(window).bounds;
        let zoomed = tab.and_then(|tab| self.shown_zoom(tab, cx));
        let panes_of = |zoom| {
            tab.zip(bounds).and_then(|(tab, bounds)| {
                self.tab_panes(tab.id, bounds, zoom, window, cx)
            })
        };
        let split = panes_of(None);
        let drawn = zoomed.and_then(|zoomed| panes_of(Some(zoomed)));
        let panes: Vec<_> = split
            .iter()
            .flat_map(|rects| &rects.panes)
            .filter_map(|rect| {
                let rect = drawn
                    .as_ref()
                    .and_then(|drawn| drawn.pane(rect.pane))
                    .unwrap_or(rect);
                Some((rect, tab?.pane(rect.pane)?))
            })
            .collect();
        let index_of = |pane: Option<PaneId>| {
            pane.and_then(|pane| {
                panes.iter().position(|(rect, _)| rect.pane == pane)
            })
            .map_or_else(|| "none".to_owned(), |index| index.to_string())
        };
        let focus = index_of(tab.map(|tab| tab.focused().pane));
        let zoom_index = index_of(zoomed);
        let joined =
            |field: &dyn Fn(&pane_geometry::PaneRect, &PaneView) -> String| {
                panes
                    .iter()
                    .map(|(rect, pane)| field(rect, pane))
                    .collect::<Vec<_>>()
                    .join(";")
            };
        let point_of = |point: gpui::Point<Pixels>| {
            format!("{},{}", f32::from(point.x), f32::from(point.y))
        };
        let rect_of = |bounds: Bounds<Pixels>| {
            format!(
                "{},{},{},{}",
                f32::from(bounds.origin.x),
                f32::from(bounds.origin.y),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height)
            )
        };
        let grids = joined(&|_, pane| {
            let grid = pane.view.read(cx).requested_grid;
            format!("{},{}", grid.columns, grid.rows)
        });
        let terminals = joined(&|rect, _| rect_of(rect.terminal));
        let button = |center: fn(Bounds<Pixels>) -> gpui::Point<Pixels>| {
            joined(&|rect, _| {
                rect.title_bar.map_or_else(
                    || "none".to_owned(),
                    |bar| point_of(center(bar)),
                )
            })
        };
        let closes = button(Self::pane_close_center);
        let zooms = button(Self::pane_zoom_center);
        // Where a press grabs each title bar for a drag: its title text.
        let grabs =
            button(|bar| gpui::point(bar.origin.x + px(40.0), bar.center().y));
        let drop = self
            .drawn_drop
            .borrow()
            .clone()
            .unwrap_or_else(|| "none".to_owned());
        let spring = self
            .pane_drag
            .as_ref()
            .and_then(|drag| drag.spring.tab())
            .and_then(|armed| self.tabs.iter().position(|tab| tab.id == armed))
            .map_or_else(|| "none".to_owned(), |index| index.to_string());
        let dividers = self
            .drawn_dividers
            .borrow()
            .iter()
            .map(|divider| rect_of(divider.grab))
            .collect::<Vec<_>>()
            .join(";");
        format!(
            "{prefix}panes={} {prefix}pane_focus={focus} {prefix}pane_zoomed={zoom_index} {prefix}pane_grids={grids} {prefix}pane_rects={terminals} {prefix}pane_close={closes} {prefix}pane_zoom={zooms} {prefix}pane_grab={grabs} {prefix}drop={drop} {prefix}spring={spring} {} {prefix}pane_dividers={}",
            panes.len(),
            self.layout_smoke_state(prefix, cx),
            if dividers.is_empty() {
                "none"
            } else {
                &dividers
            },
        )
    }

    /// Smoke output for the Licenses view: `closed`, `loading`,
    /// `unavailable`, or its match count, the Debug-quoted selected name,
    /// and its keyboard focus.
    fn licenses_smoke_state(
        &self,
        prefix: &str,
        window: &Window,
        cx: &Context<'_, Self>,
    ) -> String {
        let licenses_focus = self
            .licenses
            .as_ref()
            .and_then(|view| view.read(cx).focus(window, cx))
            .map_or("none", LicensesFocus::name);
        let (licenses, licenses_selected) =
            match self.licenses.as_ref().map(|view| view.read(cx).state(cx)) {
                None => ("closed".to_owned(), None),
                Some(LicensesState::Loading) => ("loading".to_owned(), None),
                Some(LicensesState::Unavailable) => {
                    ("unavailable".to_owned(), None)
                }
                Some(LicensesState::Ready { matches, selected }) => {
                    (matches.to_string(), selected)
                }
            };
        format!(
            "{prefix}licenses={licenses} {prefix}licenses_selected={licenses_selected:?} {prefix}licenses_focus={licenses_focus}"
        )
    }

    /// Smoke output for the transient UI, each field named after `prefix`:
    /// the menu (open, focused, selected item, targeted tab index), the
    /// About panel and its focused button, the Licenses view, the close
    /// confirmation (showing, focused button,
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
                MenuKind::Window
                | MenuKind::Terminal { .. }
                | MenuKind::Pane { .. }
                | MenuKind::Field(_) => None,
            })
            .and_then(|tab| {
                self.tabs.iter().position(|record| record.id == tab)
            })
            .map_or_else(|| "none".to_owned(), |index| index.to_string());
        let about = self.about.is_some();
        let about_focus = self
            .about_buttons
            .focused(window)
            .map_or("none", AboutButton::name);
        let licenses = self.licenses_smoke_state(prefix, window, cx);
        let menu = self.menu_smoke_state(prefix, window, cx);
        let dialog_focus = self.close_buttons.focused(window);
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
            "{menu} {prefix}menu_target={target} {prefix}about={about} {prefix}about_focus={about_focus} {licenses} {prefix}confirming={} {prefix}dialog_focus={dialog_focus} {prefix}dialog_title={dialog_title} {prefix}dialog_groups={dialog_groups:?} {prefix}dialog_drawn_groups={:?} {prefix}notice_focus={} {prefix}window_title={:?} {prefix}menu_button={button} {prefix}window_buttons={controls} {prefix}traffic_lights={traffic_lights} {prefix}title_row_moves={} {prefix}scale={} {prefix}client_decorations={} {prefix}frame_inset={} {prefix}maximized={} {prefix}fullscreen={:?} {prefix}content={},{},{},{}",
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

/// The control a showing dialog or About panel returns focus to, for
/// handing focus back when something else takes it; `None` while neither
/// shows.
pub(super) fn modal_focus(window: &Window, cx: &App) -> Option<FocusHandle> {
    let root = window.root::<WorkspaceView>().flatten()?;
    root.read(cx).dialog_focus_target(cx)
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
mod tests;
#[cfg(all(test, feature = "test-support"))]
mod window_tests;
