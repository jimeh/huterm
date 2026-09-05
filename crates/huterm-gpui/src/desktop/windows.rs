use super::*;
use crate::config::TabPosition;
use gpui::{Entity, Global, WeakEntity};
use huterm_core::{MuxError, OpenedTab};
use huterm_protocol::WorkspaceId;
use std::sync::atomic::{AtomicBool, Ordering};

const TAB_HEIGHT: Pixels = px(32.0);
const SIDEBAR_WIDTH: Pixels = px(180.0);
const TAB_WIDTH: Pixels = px(160.0);
const CONTROL_SIZE: Pixels = px(28.0);
const TAB_DRAG_THRESHOLD: f64 = 4.0;
const TAB_PAGE_DELAY: Duration = Duration::from_millis(400);

#[derive(Default)]
struct DesktopRuntime {
    mux: Mutex<Mux>,
    terminating: AtomicBool,
}

impl DesktopRuntime {
    fn open_tab(
        &self,
        workspace: Option<WorkspaceId>,
        command: &TerminalCommand,
    ) -> Result<(WorkspaceId, OpenedTab), MuxError> {
        let mut mux = self
            .mux
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.terminating.load(Ordering::Acquire) {
            return Err(RuntimeError::Stopped.into());
        }
        let id = match workspace {
            Some(id) => id,
            None => mux.create_workspace()?,
        };
        match mux.open_tab(id, command) {
            Ok(tab) => Ok((id, tab)),
            Err(error) => {
                if workspace.is_none() {
                    let _ = mux.close_workspace(id);
                }
                Err(error)
            }
        }
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
        // A queued spawn must observe termination after it acquires the same
        // mutex, even when it had not started when the native quit hook ran.
        self.terminating.store(true, Ordering::Release);
        self.mux
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shutdown()
    }
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
    Application::new().run(move |cx| {
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
        cx.on_app_quit(move |_| {
            // AppKit terminate: does not return from Application::run. GPUI
            // allows only 100 ms for quit futures, so this terminal hook must
            // finish synchronous cleanup before returning its empty future.
            if let Err(error) = app_runtime.terminate() {
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
            if cx.windows().is_empty() {
                cx.quit();
            }
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
        cx.quit();
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

#[expect(
    clippy::too_many_lines,
    reason = "native window creation installs its lifecycle and event pump"
)]
fn open_window(cx: &mut App) {
    if cx.global::<Desktop>().quitting || cx.global::<Desktop>().quit_pending {
        return;
    }
    let config = cx.global::<Desktop>().config.clone();
    let (family, metrics) = match resolve_metrics(&config, cx) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("Cannot open window: {error}");
            if cx.windows().is_empty() {
                cx.quit();
            }
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
                workspace: None,
                tabs: Vec::new(),
                active: None,
                first_visible: 0,
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
                view.new_tab(window, cx);
            });
            let pump_view = view.downgrade();
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                    if pump_view
                        .update(cx, |view, cx| {
                            if view.advance_drag_page(Instant::now()) {
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
        if cx.windows().is_empty() {
            cx.quit();
        }
    }
}

struct TabView {
    id: TabId,
    fallback_title: String,
    view: Entity<TerminalView>,
}

struct WorkspaceView {
    workspace: Option<WorkspaceId>,
    tabs: Vec<TabView>,
    active: Option<TabId>,
    first_visible: usize,
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
    original_first: usize,
    page: Option<(bool, Instant)>,
    strip: TabStrip,
}

#[derive(Clone, Debug)]
struct TabStrip {
    bounds: Bounds<Pixels>,
    vertical: bool,
    visible: std::ops::Range<usize>,
    capacity: usize,
    count: usize,
    extent: Pixels,
    leading: Pixels,
}

impl TabStrip {
    fn new(
        bounds: Bounds<Pixels>,
        vertical: bool,
        count: usize,
        active: usize,
        first: usize,
        pin_active: bool,
    ) -> Self {
        let available = if vertical {
            bounds.size.height
        } else {
            bounds.size.width
        };
        let nominal = if vertical { TAB_HEIGHT } else { TAB_WIDTH };
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "bounded nonnegative window dimension"
        )]
        let capacity = ((available - CONTROL_SIZE * 3.0).max(nominal) / nominal)
            .floor() as usize;
        let visible = if pin_active {
            visible_tabs(count, active, first, capacity)
        } else {
            let first = first.min(count.saturating_sub(capacity));
            first..(first + capacity).min(count)
        };
        Self {
            bounds,
            vertical,
            visible,
            capacity,
            count,
            extent: if vertical {
                TAB_HEIGHT
            } else {
                TAB_WIDTH.min((available - CONTROL_SIZE * 3.0).max(px(1.0)))
            },
            leading: if count > capacity {
                CONTROL_SIZE * 2.0
            } else {
                px(0.0)
            },
        }
    }
    fn axis(&self, pointer: gpui::Point<Pixels>) -> Pixels {
        if self.vertical {
            pointer.y - self.bounds.origin.y
        } else {
            pointer.x - self.bounds.origin.x
        }
    }
    fn available(&self) -> Pixels {
        if self.vertical {
            self.bounds.size.height
        } else {
            self.bounds.size.width
        }
    }
    fn slot(&self, pointer: gpui::Point<Pixels>) -> usize {
        let axis = self.axis(pointer);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped insertion slot"
        )]
        let offset = ((axis - self.leading).max(px(0.0)) / self.extent + 0.5)
            .floor() as usize;
        self.visible.start + offset.min(self.visible.len())
    }
    fn page_direction(&self, pointer: gpui::Point<Pixels>) -> Option<bool> {
        if self.count <= self.capacity {
            return None;
        }
        let axis = self.axis(pointer);
        if axis < CONTROL_SIZE && self.visible.start > 0 {
            Some(false)
        } else if (axis < self.leading
            || axis >= self.available() - CONTROL_SIZE)
            && axis >= CONTROL_SIZE
            && self.visible.end < self.count
        {
            Some(true)
        } else {
            None
        }
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "visible slot count is bounded by window dimensions"
    )]
    fn marker(&self, slot: usize) -> Bounds<Pixels> {
        let offset = self.leading
            + self.extent
                * (slot
                    .saturating_sub(self.visible.start)
                    .min(self.visible.len()) as f32);
        let offset = offset.min((self.available() - px(2.0)).max(px(0.0)));
        if self.vertical {
            Bounds::new(
                self.bounds.origin + point(px(0.0), offset),
                size(self.bounds.size.width, px(2.0)),
            )
        } else {
            Bounds::new(
                self.bounds.origin + point(offset, px(0.0)),
                size(px(2.0), self.bounds.size.height),
            )
        }
    }
    fn preview(&self, pointer: gpui::Point<Pixels>) -> Bounds<Pixels> {
        let extent = self.extent.min(self.available());
        let offset = (self.axis(pointer) - extent / 2.0)
            .clamp(px(0.0), (self.available() - extent).max(px(0.0)));
        if self.vertical {
            Bounds::new(
                self.bounds.origin + point(px(0.0), offset),
                size(self.bounds.size.width, extent),
            )
        } else {
            Bounds::new(
                self.bounds.origin + point(offset, px(0.0)),
                size(extent, self.bounds.size.height),
            )
        }
    }
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
}

#[derive(Debug, Eq, PartialEq)]
enum CloseDecision {
    Check(CloseTarget),
    Confirm(CloseTarget),
    Close(CloseTarget),
}

impl CloseState {
    fn begin_check(&mut self, target: CloseTarget) -> CloseTarget {
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
        self.pending = None;
        self.current = None;
        self.confirmation.take()
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
        let layout = ChromeLayout::new(
            window.viewport_size(),
            terminal_top(window),
            position,
        );
        let active = self
            .tabs
            .iter()
            .position(|tab| Some(tab.id) == self.active)
            .unwrap_or(0);
        TabStrip::new(
            layout.tabs,
            position.vertical(),
            self.tabs.len(),
            active,
            self.first_visible,
            !self.reorder.as_ref().is_some_and(|drag| drag.dragging),
        )
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
        self.reorder = Some(TabReorder {
            source,
            origin: pointer,
            pointer,
            dragging: false,
            original_first: self.first_visible,
            page: None,
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
            return;
        }
        let strip = self.tab_strip(window);
        if let Some(drag) = &mut self.reorder {
            drag.pointer = pointer;
            if !drag.dragging
                && (pointer - drag.origin).magnitude() > TAB_DRAG_THRESHOLD
            {
                drag.dragging = true;
                self.focus.focus(window);
            }
            drag.strip = strip;
            let direction =
                drag.strip.page_direction(pointer).filter(|_| drag.dragging);
            if direction != drag.page.map(|(forward, _)| forward) {
                drag.page = direction
                    .map(|forward| (forward, Instant::now() + TAB_PAGE_DELAY));
            }
        }
        cx.notify();
    }

    fn advance_drag_page(&mut self, now: Instant) -> bool {
        let Some(drag) = &mut self.reorder else {
            return false;
        };
        let Some((forward, deadline)) = drag.page else {
            return false;
        };
        if !drag.dragging || now < deadline {
            return false;
        }
        let previous = self.first_visible;
        self.first_visible = if forward {
            self.first_visible
                .saturating_add(drag.strip.capacity)
                .min(drag.strip.count.saturating_sub(drag.strip.capacity))
        } else {
            self.first_visible.saturating_sub(drag.strip.capacity)
        };
        drag.page = (self.first_visible != previous)
            .then_some((forward, now + TAB_PAGE_DELAY));
        self.first_visible != previous
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
            self.first_visible = drag.original_first;
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
        reason = "spawn completion publishes a tab or cleans up an orphaned workspace"
    )]
    fn new_tab(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.cancel_reorder(window, cx);
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
        let fallback = command
            .program
            .file_name()
            .unwrap_or(command.program.as_os_str())
            .to_string_lossy()
            .into_owned();
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
                (WorkspaceId, OpenedTab),
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
                        Ok((id, opened)) => {
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
                                fallback_title: fallback,
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
                && let Some(Ok((id, _))) = result
            {
                cx.background_executor()
                    .spawn(async move {
                        let _ = cleanup_runtime
                            .mux
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .close_workspace(id);
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
        for tab in &self.tabs {
            tab.view.update(cx, |view, cx| {
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

    fn request_close(
        &mut self,
        target: CloseTarget,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.cancel_reorder(window, cx);
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
        let clients: Vec<_> = if matches!(target, CloseTarget::Application) {
            cx.global::<Desktop>()
                .windows
                .clone()
                .into_iter()
                .filter_map(|view| view.upgrade())
                .filter(|view| view.entity_id() != cx.entity_id())
                .flat_map(|view| {
                    view.read(cx)
                        .tabs
                        .iter()
                        .map(|tab| tab.view.read(cx).client.clone())
                        .collect::<Vec<_>>()
                })
                .collect()
        } else {
            self.tabs
                .iter()
                .filter(|tab| match target {
                    CloseTarget::Tab(id) => tab.id == id,
                    _ => true,
                })
                .map(|tab| tab.view.read(cx).client.clone())
                .collect()
        };
        let mut clients = clients;
        if matches!(target, CloseTarget::Application) {
            clients.extend(
                self.tabs.iter().map(|tab| tab.view.read(cx).client.clone()),
            );
        }
        self.busy = true;
        cx.spawn_in(window, async move |view, cx| {
            let mut foreground = false;
            for client in clients {
                foreground |= client.has_foreground_job().await.unwrap_or(true);
            }
            let _ = view.update_in(cx, |view, window, cx| {
                view.busy = false;
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
        if matches!(self.close.cancel(), Some(CloseTarget::Application)) {
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
        self.close.confirmation = None;
        self.close.current = Some(target);
        self.busy = true;
        if matches!(target, CloseTarget::Application) {
            cx.global_mut::<Desktop>().quitting = true;
        }
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let workspace = self.workspace;
        let task = cx.background_executor().spawn(async move {
            if matches!(target, CloseTarget::Application) {
                return runtime.terminate();
            }
            let mut mux = runtime
                .mux
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match (target, workspace) {
                (CloseTarget::Application, _) => mux.shutdown(),
                (CloseTarget::Tab(id), Some(workspace)) => {
                    mux.close_tab(workspace, id)
                }
                (CloseTarget::Window, Some(workspace)) => {
                    mux.close_workspace(workspace)
                }
                _ => Ok(()),
            }
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = task.await;
            let _ = view.update_in(cx, |view, window, cx| {
                view.busy = false;
                view.close.current = None;
                if let Err(error) = result {
                    view.status = Some(format!("Close failed: {error}"));
                    eprintln!(
                        "{}",
                        view.status.as_deref().unwrap_or("Close failed")
                    );
                }
                match target {
                    CloseTarget::Application => cx.quit(),
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
                            view.finish_close(CloseTarget::Window, window, cx);
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
    pub(super) fn new(
        viewport: gpui::Size<Pixels>,
        titlebar: Pixels,
        position: TabPosition,
    ) -> Self {
        let top = titlebar.min(viewport.height);
        let available = size(
            viewport.width.max(px(0.0)),
            (viewport.height - top).max(px(0.0)),
        );
        let mut terminal = Bounds::new(point(px(0.0), top), available);
        let mut tabs = terminal;
        if position.vertical() {
            tabs.size.width = SIDEBAR_WIDTH.min(available.width * 0.5);
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

// Paging keeps the selected tab visible without depending on GPUI scroll offsets.
fn visible_tabs(
    count: usize,
    active: usize,
    first: usize,
    capacity: usize,
) -> std::ops::Range<usize> {
    let capacity = capacity.max(1);
    let mut first = first.min(count.saturating_sub(capacity));
    if active < first {
        first = active;
    }
    if active >= first + capacity {
        first = active + 1 - capacity;
    }
    first..(first + capacity).min(count)
}

impl Render for WorkspaceView {
    #[expect(
        clippy::too_many_lines,
        reason = "window chrome composes tab controls and close confirmation"
    )]
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let position = self.config.window.tab_position;
        let layout = ChromeLayout::new(
            window.viewport_size(),
            terminal_top(window),
            position,
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
                        // Retain the gesture through keystroke observation, so
                        // Escape cannot also reach the active terminal.
                        cx.defer_in(window, |view, window, cx| {
                            view.cancel_reorder(window, cx);
                        });
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
                                    if view.reorder.is_some() {
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
                                    if view.reorder.is_some() {
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
        let capacity = strip.capacity;
        let visible = strip.visible.clone();
        self.first_visible = visible.start;
        if let Some(drag) = &mut self.reorder {
            drag.strip = strip.clone();
        }
        let mut bar = div()
            .absolute()
            .left(layout.tabs.origin.x)
            .top(layout.tabs.origin.y)
            .w(layout.tabs.size.width)
            .h(layout.tabs.size.height)
            .overflow_hidden()
            .flex()
            .when(vertical, Styled::flex_col)
            .bg(foreground.opacity(0.06));
        if self.tabs.len() > capacity {
            for (label, forward) in [("‹", false), ("›", true)] {
                bar = bar.child(
                    div()
                        .id(if forward {
                            "next-tabs"
                        } else {
                            "previous-tabs"
                        })
                        .flex_shrink_0()
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
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.navigate(forward, window, cx);
                        }))
                        .child(label),
                );
            }
        }
        for index in visible {
            let tab = &self.tabs[index];
            let id = tab.id;
            let terminal = tab.view.read(cx);
            let title = if terminal.title.trim().is_empty() {
                tab.fallback_title.clone()
            } else {
                terminal.title.clone()
            };
            let title = if terminal.exited {
                format!("{title} · exited")
            } else {
                title
            };
            bar = bar.child(
                div()
                    .id(("tab", id.get()))
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
        bar = bar.child(
            div()
                .id("new-tab")
                .flex_shrink_0()
                .w(CONTROL_SIZE)
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
        root = root.child(bar);
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
                .map(|tab| tab.fallback_title.clone())
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
            root = root.child(div().absolute().inset_0().bg(background.opacity(0.9)).flex().items_center().justify_center().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(div().w_full().max_w(px(460.0)).mx_4().p_4().bg(background).border_1().border_color(foreground.opacity(0.3)).flex().flex_col().gap_3()
                    .child("A foreground job is running. Close and terminate it?")
                    .child(div().flex().gap_3()
                        .child(div().id("cancel-close").px_3().py_1().cursor_pointer().on_click(cx.listener(|view, _, window, cx| view.cancel_close(window, cx))).child("Cancel"))
                        .child(div().id("confirm-close").px_3().py_1().bg(foreground.opacity(0.15)).cursor_pointer().on_click(cx.listener(move |view, _, window, cx| view.finish_close(target, window, cx))).child("Close")))));
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
                TabStrip::new(layout.tabs, position.vertical(), 8, 0, 0, true);
            let pointer = if strip.vertical {
                strip.bounds.origin
                    + point(px(20.0), strip.leading + strip.extent * 1.1)
            } else {
                strip.bounds.origin
                    + point(strip.leading + strip.extent * 1.1, px(10.0))
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
                        strip.visible.start
                    } else {
                        strip.visible.end
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
    fn overflow_drag_pages_are_not_pinned_to_active_tab() {
        for vertical in [false, true] {
            let bounds = Bounds::new(
                point(px(20.0), px(40.0)),
                size(px(600.0), px(220.0)),
            );
            let initial = TabStrip::new(bounds, vertical, 15, 0, 0, true);
            let next =
                TabStrip::new(bounds, vertical, 15, 0, initial.capacity, false);
            assert_eq!(next.visible.start, initial.capacity);
            let previous_control = bounds.origin + point(px(2.0), px(2.0));
            assert_eq!(next.page_direction(previous_control), Some(false));
            let end = bounds.origin + point(px(10_000.0), px(10_000.0));
            assert_eq!(next.page_direction(end), Some(true));
            let last = TabStrip::new(bounds, vertical, 15, 0, 15, false);
            assert_eq!(last.visible.end, 15);
            assert_eq!(last.slot(end), 15);
            assert_eq!(last.page_direction(end), None);
            let canceled = TabStrip::new(
                bounds,
                vertical,
                15,
                0,
                last.visible.start,
                true,
            );
            assert_eq!(canceled.visible.start, 0);
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
        let (_, opened) = runtime.open_tab(None, &command).unwrap();
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
    fn tab_overflow_keeps_active_tab_visible_after_navigation_and_removal() {
        assert_eq!(visible_tabs(12, 11, 0, 3), 9..12);
        assert_eq!(visible_tabs(12, 0, 9, 3), 0..3);
        assert_eq!(visible_tabs(2, 1, 9, 3), 0..2);
        assert_eq!(visible_tabs(12, 7, 0, 0), 7..8);
        assert_eq!(visible_tabs(0, 0, 0, 3), 0..0);
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
