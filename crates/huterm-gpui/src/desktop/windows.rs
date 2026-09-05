use super::*;
use crate::config::TabPosition;
#[cfg(target_os = "macos")]
use crate::native_quit;
use gpui::{Entity, Global, WeakEntity};
use huterm_core::{
    CloseAssessment, CloseRequest, HierarchySnapshot, MuxError, OpenedTab,
};
use huterm_protocol::{AttachmentId, SessionId, WorkspaceId};
use std::sync::atomic::{AtomicBool, Ordering};

const TAB_HEIGHT: Pixels = px(32.0);
pub(super) const SIDEBAR_WIDTH: Pixels = px(180.0);
mod tab_strip;
use tab_strip::TabStrip;
const CONTROL_SIZE: Pixels = px(28.0);
const TAB_DRAG_THRESHOLD: f64 = 4.0;

#[derive(Default)]
struct DesktopRuntime {
    mux: Mutex<Mux>,
    terminating: AtomicBool,
    restore: Mutex<Option<RestoreSnapshot>>,
}

impl DesktopRuntime {
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
        mux.validate_close(assessment, &current, confirmed)?;
        if let Some(windows) = windows {
            self.terminating.store(true, Ordering::Release);
            self.capture(&mux, windows);
        }
        mux.commit_close(assessment, &current, confirmed)
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
    runtime: Arc<DesktopRuntime>,
    config: Config,
    config_path: PathBuf,
    config_error: Option<String>,
    windows: Vec<WeakEntity<WorkspaceView>>,
    reloading: bool,
    quitting: bool,
    pending_spawns: usize,
    quit_pending: bool,
}
impl Global for Desktop {}

pub(super) fn run() -> anyhow::Result<()> {
    let loaded = config::load();
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
        cx.set_global(Desktop {
            runtime: Arc::clone(&app_runtime),
            config: loaded.config,
            config_path: loaded.path,
            config_error: loaded.error,
            windows: Vec::new(),
            reloading: false,
            quitting: false,
            pending_spawns: 0,
            quit_pending: false,
        });
        install_native_quit(cx);
        cx.on_app_quit(move |cx| {
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
        install_bindings(cx);
        install_menus(cx);
        cx.on_action(|_: &NewWindow, cx| open_window(cx));
        cx.on_action(|_: &ReloadConfiguration, cx| reload(cx));
        // Global actions run while the dispatching window is borrowed. Route
        // quit after that window has returned to App's window map.
        cx.on_action(|_: &Quit, cx| cx.defer(request_quit));
        cx.on_action(|_: &Hide, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
        cx.on_window_closed(|cx| {
            cx.global_mut::<Desktop>()
                .windows
                .retain(|view| view.upgrade().is_some());
            maybe_exit(cx);
        })
        .detach();
        cx.observe_keystrokes(|event, window, cx| {
            if event.action.is_some() || reserved_keystroke(&event.keystroke) {
                return;
            }
            if let Some(root) = window.root::<WorkspaceView>().flatten() {
                root.update(cx, |view, cx| {
                    if view.busy
                        || view.close.confirmation.is_some()
                        || view.reorder.is_some()
                    {
                        return;
                    }
                    if let Some(tab) = view.active_view() {
                        tab.update(cx, |tab, cx| {
                            if tab.handle_keystroke(&event.keystroke) {
                                cx.notify();
                            }
                            tab.start_snapshot_if_needed(cx);
                        });
                    }
                });
            }
        })
        .detach();
        open_window(cx);
        cx.activate(true);
    });
    // Backends whose event loop returns get the same idempotent cleanup.
    runtime.terminate()?;
    Ok(())
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
    #[cfg(target_os = "macos")]
    native_quit::allow_termination();
    cx.quit();
}

fn install_native_quit(_cx: &mut App) {
    #[cfg(target_os = "macos")]
    {
        let requests = native_quit::install()
            .expect("cannot install cancellable native termination hook");
        _cx.spawn(async move |cx| {
            while requests.recv().await.is_ok() {
                let _ = cx.update(|cx| cx.defer(request_quit));
            }
        })
        .detach();
    }
}

fn initial_window_size(
    config: &Config,
    metrics: GridMetrics,
) -> gpui::Size<Pixels> {
    size(
        metrics.cell_width * f32::from(INITIAL_COLUMNS)
            + px(config.window.padding_x * 2.0)
            + if config.window.tab_position.vertical() {
                SIDEBAR_WIDTH
            } else {
                px(0.0)
            },
        metrics.cell_height * f32::from(INITIAL_ROWS)
            + px(config.window.padding_y * 2.0)
            + titlebar_inset(cfg!(target_os = "macos"), false)
            + if config.window.tab_position.vertical() {
                px(0.0)
            } else {
                TAB_HEIGHT
            },
    )
}

fn open_window(cx: &mut App) {
    open_window_inner(cx, true);
}

#[expect(
    clippy::too_many_lines,
    reason = "native window creation installs lifecycle and event pump"
)]
fn open_window_inner(cx: &mut App, launch_shell: bool) {
    if cx.global::<Desktop>().quitting || cx.global::<Desktop>().quit_pending {
        return;
    }
    let config = cx.global::<Desktop>().config.clone();
    let (family, metrics) = match resolve_metrics(&config, cx) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("Cannot open window: {error}");
            maybe_exit(cx);
            return;
        }
    };
    let bounds =
        Bounds::centered(None, initial_window_size(&config, metrics), cx);
    let result = cx.open_window(
        WindowOptions {
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
            let view = cx.new(|cx| WorkspaceView {
                attachment: None,
                bounds: window.window_bounds(),
                workspace: None,
                tabs: Vec::new(),
                active: None,
                tab_scroll: px(0.0),
                scroll_target: None,
                last_scroll: Instant::now(),
                sidebar_width: SIDEBAR_WIDTH,
                resizing_sidebar: false,
                reorder: None,
                config,
                family,
                metrics: scaled_metrics,
                focus: cx.focus_handle(),
                busy: false,
                close: CloseState::default(),
                status: cx.global::<Desktop>().config_error.clone(),
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
                if launch_shell {
                    view.new_tab(window, cx);
                }
            });
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
                                view.bounds = window.window_bounds();
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
                                        metadata_changed |= previous
                                            != (
                                                terminal.title.clone(),
                                                terminal.exited,
                                            );
                                    });
                                }
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
        eprintln!("Cannot open window: {error}");
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
    attachment: Option<AttachmentId>,
    bounds: WindowBounds,
    workspace: Option<WorkspaceId>,
    tabs: Vec<TabView>,
    active: Option<TabId>,
    tab_scroll: Pixels,
    scroll_target: Option<Pixels>,
    last_scroll: Instant,
    sidebar_width: Pixels,
    resizing_sidebar: bool,
    reorder: Option<TabReorder>,
    config: Config,
    family: String,
    metrics: GridMetrics,
    focus: FocusHandle,
    busy: bool,
    close: CloseState,
    status: Option<String>,
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

impl WorkspaceView {
    fn tab_strip(&self, window: &Window) -> TabStrip {
        let position = self.config.window.tab_position;
        let layout = ChromeLayout::with_sidebar(
            window.viewport_size(),
            terminal_top(window),
            position,
            self.sidebar_width,
        );
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
        reason = "publishes a spawned tab and cleans orphaned publication"
    )]
    fn new_tab(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.cancel_reorder(window, cx);
        self.resizing_sidebar = false;
        if self.busy
            || self.close.confirmation.is_some()
            || cx.global::<Desktop>().quitting
            || cx.global::<Desktop>().quit_pending
        {
            return;
        }
        let command =
            match shell_command(self.metrics.at_scale(window.scale_factor())) {
                Ok(command) => command,
                Err(error) => {
                    self.status = Some(error.to_string());
                    cx.notify();
                    return;
                }
            };
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
                    cx.defer(|cx| cx.dispatch_action(&Quit));
                }
            });
            let mut result = Some(result);
            let update = view.update_in(cx, |view, window, cx| {
                view.busy = false;
                if let Some(result) = result.take() {
                    match result {
                        Ok((_, id, opened, attachment)) => {
                            if let Some(attachment) = attachment {
                                view.attachment = Some(attachment);
                            }
                            view.workspace = Some(id);
                            let config_path =
                                cx.global::<Desktop>().config_path.clone();
                            let terminal = cx.new(|cx| {
                                TerminalView::new(
                                    opened.client,
                                    &view.config,
                                    config_path,
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
                            view.status =
                                cx.global::<Desktop>().config_error.clone();
                        }
                        Err(error) => {
                            view.status =
                                Some(format!("Cannot open tab: {error}"));
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
        self.reveal_active(window);
        for tab in &self.tabs {
            tab.view.update(cx, |view, cx| {
                view.sidebar_width = self.sidebar_width;
                view.visible = tab.id == id;
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

    fn navigate(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.busy
            || self.reorder.is_some()
            || self.close.confirmation.is_some()
            || self.tabs.is_empty()
        {
            return;
        }
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
        self.select(self.tabs[next].id, window, cx);
    }

    fn select_index(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.busy
            || self.reorder.is_some()
            || self.close.confirmation.is_some()
        {
            return;
        }
        let tab = if index == 8 {
            self.tabs.last()
        } else {
            self.tabs.get(index)
        };
        if let Some(tab) = tab {
            self.select(tab.id, window, cx);
        }
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
                    window.remove_window();
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
        if let Some(target) = self
            .close
            .take_pending(|id| self.tabs.iter().any(|tab| tab.id == id))
        {
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
        self.bounds = window.window_bounds();
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
                        if matches!(
                            view.close.pending,
                            Some(CloseTarget::Application)
                        ) {
                            cx.defer(|cx| cx.dispatch_action(&Quit));
                        }
                        window.remove_window();
                    }
                    CloseTarget::Tab(id) => {
                        remove_tab(
                            &mut view.tabs,
                            &mut view.active,
                            id,
                            |tab| tab.id,
                        );
                        if let Some(active) = view.active {
                            view.select(active, window, cx);
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

fn reload(cx: &mut App) {
    if cx.global::<Desktop>().reloading {
        return;
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
                resolve_metrics(&config, cx)
                    .map(|(family, metrics)| (config, family, metrics))
                    .map_err(|error| error.to_string())
            });
            if let Ok((config, _, _)) = &result {
                cx.global_mut::<Desktop>().config = config.clone();
                cx.global_mut::<Desktop>().config_error = None;
            }
            let windows = cx.global::<Desktop>().windows.clone();
            for window in windows {
                let _ = window.update(cx, |view, cx| {
                    match &result {
                        Ok((config, family, metrics)) => {
                            view.resizing_sidebar = false;
                            view.scroll_target = None;
                            view.config = config.clone();
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
                                    view.theme = config.theme.clone();
                                    cx.notify();
                                });
                            }
                        }
                        Err(error) => {
                            view.status =
                                Some(format!("Config reload failed: {error}"));
                        }
                    }
                    cx.notify();
                });
            }
        });
    })
    .detach();
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

    pub(super) fn with_sidebar(
        viewport: gpui::Size<Pixels>,
        titlebar: Pixels,
        position: TabPosition,
        sidebar_width: Pixels,
    ) -> Self {
        let top = titlebar.min(viewport.height);
        let available = size(
            viewport.width.max(px(0.0)),
            (viewport.height - top).max(px(0.0)),
        );
        let mut terminal = Bounds::new(point(px(0.0), top), available);
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
        let position = self.config.window.tab_position;
        let layout = ChromeLayout::with_sidebar(
            window.viewport_size(),
            terminal_top(window),
            position,
            self.sidebar_width,
        );
        let foreground = color(self.config.theme.foreground);
        let background = color(self.config.theme.background);
        let mut root = div()
            .size_full()
            .relative()
            .bg(background)
            .text_color(foreground)
            .text_size(px(13.0))
            .track_focus(&self.focus)
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
            .on_action(cx.listener(|view, _: &NewTab, window, cx| {
                view.new_tab(window, cx);
            }))
            .on_action(cx.listener(|view, _: &CloseTab, window, cx| {
                if let Some(id) = view.active {
                    view.request_close(CloseTarget::Tab(id), window, cx);
                }
            }))
            .on_action(cx.listener(|view, _: &CloseWindow, window, cx| {
                view.request_close(CloseTarget::Window, window, cx);
            }))
            .on_action(cx.listener(|view, _: &NextTab, window, cx| {
                view.navigate(true, window, cx);
            }))
            .on_action(cx.listener(|view, _: &PreviousTab, window, cx| {
                view.navigate(false, window, cx);
            }))
            .on_action(cx.listener(|view, _: &Tab1, window, cx| {
                view.select_index(0, window, cx);
            }))
            .on_action(cx.listener(|view, _: &Tab2, window, cx| {
                view.select_index(1, window, cx);
            }));
        root = root
            .on_action(cx.listener(|view, _: &Tab3, window, cx| {
                view.select_index(2, window, cx);
            }))
            .on_action(cx.listener(|view, _: &Tab4, window, cx| {
                view.select_index(3, window, cx);
            }))
            .on_action(cx.listener(|view, _: &Tab5, window, cx| {
                view.select_index(4, window, cx);
            }))
            .on_action(cx.listener(|view, _: &Tab6, window, cx| {
                view.select_index(5, window, cx);
            }))
            .on_action(cx.listener(|view, _: &Tab7, window, cx| {
                view.select_index(6, window, cx);
            }))
            .on_action(cx.listener(|view, _: &Tab8, window, cx| {
                view.select_index(7, window, cx);
            }))
            .on_action(cx.listener(|view, _: &Tab9, window, cx| {
                view.select_index(8, window, cx);
            }));
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
        if terminal_top(window) > px(0.0) {
            root = root.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(terminal_top(window))
                    .pl(px(84.0))
                    .flex()
                    .items_center()
                    .window_control_area(WindowControlArea::Drag)
                    .child("Huterm"),
            );
        }
        let strip = self.tab_strip(window);
        let vertical = strip.vertical;
        self.tab_scroll = strip.offset;
        if let Some(drag) = &mut self.reorder {
            drag.strip = strip.clone();
        }
        let mut bar = div()
            .id("tab-strip")
            .absolute()
            .left(strip.bounds.origin.x)
            .top(strip.bounds.origin.y)
            .w(strip.bounds.size.width)
            .h(strip.bounds.size.height)
            .overflow_hidden()
            .bg(foreground.opacity(0.06))
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
                            move |view, event: &MouseDownEvent, window, cx| {
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
                        div().flex_1().min_w_0().text_ellipsis().child(title),
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
                        .on_click(cx.listener(move |view, _, window, cx| {
                            let amount = view.tab_strip(window).available()
                                * if forward { 0.75 } else { -0.75 };
                            let strip = view.tab_strip(window);
                            view.scroll_target = Some(
                                (view.scroll_target.unwrap_or(strip.offset)
                                    + amount)
                                    .clamp(px(0.0), strip.max_offset()),
                            );
                            cx.notify();
                            cx.stop_propagation();
                        }))
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
        root = root.child(bar).child(
            div()
                .id("new-tab")
                .absolute()
                .left(
                    layout.tabs.origin.x
                        + if vertical { px(0.0) } else { strip.available() },
                )
                .top(
                    layout.tabs.origin.y
                        + if vertical { strip.available() } else { px(0.0) },
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
                .on_click(
                    cx.listener(|view, _, window, cx| view.new_tab(window, cx)),
                )
                .child("+"),
        );
        if vertical {
            let handle = layout.sidebar_resize_handle(position);
            root = root.child(
                div()
                    .id("sidebar-resize")
                    .absolute()
                    .left(handle.origin.x)
                    .top(handle.origin.y)
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
            let message = match (target, unknown) {
                (CloseTarget::Application, true) => {
                    "Some process state is unavailable. Quit Huterm and terminate all sessions?"
                }
                (CloseTarget::Application, false) => {
                    "Quit Huterm and terminate running jobs in all sessions?"
                }
                (CloseTarget::Window, true) => {
                    "Some process state is unavailable. Close this final view and terminate its session?"
                }
                (CloseTarget::Window, false) => {
                    "Close this final view and terminate running jobs in its session?"
                }
                (CloseTarget::Tab(_), true) => {
                    "Process state is unavailable. Close this tab and terminate its terminal?"
                }
                (CloseTarget::Tab(_), false) => {
                    "Close this tab and terminate its running jobs?"
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
                            .w_full()
                            .max_w(px(460.0))
                            .mx_4()
                            .p_4()
                            .bg(background)
                            .border_1()
                            .border_color(foreground.opacity(0.3))
                            .flex()
                            .flex_col()
                            .gap_3()
                            .child(message)
                            .child(
                                div()
                                    .flex()
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
        root
    }
}

pub(super) fn tab_bindings(macos: bool) -> Vec<KeyBinding> {
    let modifier = if macos { "cmd" } else { "ctrl-shift" };
    let mut bindings = vec![
        KeyBinding::new(&format!("{modifier}-n"), NewWindow, None),
        KeyBinding::new(&format!("{modifier}-t"), NewTab, None),
        KeyBinding::new(&format!("{modifier}-w"), CloseTab, None),
        KeyBinding::new(
            if macos { "cmd-shift-w" } else { "ctrl-shift-q" },
            CloseWindow,
            None,
        ),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
    ];
    let modifier = if macos { "cmd" } else { "alt" };
    bindings.extend([
        KeyBinding::new(&format!("{modifier}-1"), Tab1, None),
        KeyBinding::new(&format!("{modifier}-2"), Tab2, None),
        KeyBinding::new(&format!("{modifier}-3"), Tab3, None),
        KeyBinding::new(&format!("{modifier}-4"), Tab4, None),
        KeyBinding::new(&format!("{modifier}-5"), Tab5, None),
        KeyBinding::new(&format!("{modifier}-6"), Tab6, None),
        KeyBinding::new(&format!("{modifier}-7"), Tab7, None),
        KeyBinding::new(&format!("{modifier}-8"), Tab8, None),
        KeyBinding::new(&format!("{modifier}-9"), Tab9, None),
    ]);
    bindings
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for placement in [
            TabPosition::Top,
            TabPosition::Bottom,
            TabPosition::Left,
            TabPosition::Right,
        ] {
            let chrome = ChromeLayout::new(
                size(px(800.0), px(600.0)),
                px(32.0),
                placement,
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
            first
                .client
                .read_snapshot(huterm_protocol::Viewport::default()),
            Err(RuntimeError::Stopped)
        ));
        assert!(
            second
                .client
                .read_snapshot(huterm_protocol::Viewport::default())
                .is_ok()
        );
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
            opened
                .client
                .read_snapshot(huterm_protocol::Viewport::default()),
            Err(RuntimeError::Stopped)
        ));
        runtime.terminate().unwrap();
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
        for macos in [true, false] {
            let prefix = if macos { "cmd" } else { "ctrl-shift" };
            for chord in [
                format!("{prefix}-n"),
                format!("{prefix}-t"),
                format!("{prefix}-w"),
                "ctrl-tab".into(),
                "ctrl-shift-tab".into(),
                format!("{}-9", if macos { "cmd" } else { "alt" }),
            ] {
                let key = Keystroke::parse(&chord).unwrap();
                assert!(
                    tab_bindings(macos).iter().any(|binding| binding
                        .match_keystrokes(std::slice::from_ref(&key))
                        == Some(false)),
                    "{chord}"
                );
                assert!(
                    reserved_chord_for_platform(macos, key.modifiers, &key.key),
                    "{chord}"
                );
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
