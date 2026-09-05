use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    App, Application, Bounds, ClipboardItem, Context, DispatchPhase,
    FocusHandle, Focusable, KeyBinding, Keystroke, Menu, MenuItem,
    Modifiers as GpuiModifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, PromptLevel, Render, ScrollDelta, ScrollWheelEvent,
    Subscription, SystemMenuType, TitlebarOptions, Window, WindowBounds,
    WindowControlArea, WindowOptions, actions, canvas, div, point, prelude::*,
    px, size,
};
use huterm_core::{
    Mux, RuntimeClient, RuntimeError, TerminalOwner, TerminalRuntime,
};
use huterm_protocol::{
    BufferPoint, BufferRange, CellSize, GridSize, Modifiers, PaneId, SessionId,
    TabId, TerminalCommand, TerminalEvent, TerminalId, TerminalInput,
    TerminalKey, TerminalSnapshot,
};

use crate::APP_ID;
use crate::config::{self, Config, Theme, WindowConfig};
use crate::renderer::{GridMetrics, TerminalRenderer, rgb_color as color};
use crate::scroll::{
    IndicatorVisibility, ScrollController, ScrollbarExpansion,
    ScrollbarGeometry,
};

const INITIAL_COLUMNS: u16 = 100;
const INITIAL_ROWS: u16 = 32;
const PENDING_INPUT_CAPACITY: usize = 256;
const PENDING_INPUT_BYTE_CAPACITY: usize = 1024 * 1024;
const SCROLLBAR_WIDTH: Pixels = px(12.0);
const SCROLLBAR_EXPANDED_WIDTH: Pixels = px(18.0);
const TITLEBAR_HEIGHT: Pixels = px(32.0);

actions!(
    huterm,
    [
        About,
        Copy,
        Hide,
        HideOthers,
        Minimize,
        Paste,
        Quit,
        ScrollPageDown,
        ScrollPageUp,
        ScrollToBottom,
        Settings,
        ReloadConfiguration,
        ShowAll,
        ToggleFullscreen,
        Zoom
    ]
);

#[expect(
    clippy::too_many_lines,
    reason = "desktop startup binds the runtime and one-window application lifetime"
)]
pub(crate) fn run() -> anyhow::Result<()> {
    let loaded = config::load();
    if let Some(error) = &loaded.error {
        eprintln!("Huterm configuration error: {error}");
    }
    let config_path = loaded.path;
    let config = loaded.config;
    let config_error = loaded.error;
    let runtime_owner = Arc::new(Mutex::new(None));
    let startup_runtime_owner = Arc::clone(&runtime_owner);
    let app_runtime_owner = Arc::clone(&runtime_owner);
    let startup_error = Arc::new(Mutex::new(None));
    let app_startup_error = Arc::clone(&startup_error);

    Application::new().run(move |cx: &mut App| {
        install_bindings(cx);
        install_menus(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &Hide, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
        cx.on_app_quit(move |_| {
            if let Err(error) = shutdown_runtime(&app_runtime_owner) {
                eprintln!("failed to stop the terminal runtime: {error}");
            }
            async {}
        })
        .detach();
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let (font_family, metrics) = match resolve_metrics(&config, cx) {
            Ok(resolved) => resolved,
            Err(error) => {
                *app_startup_error
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(error);
                cx.quit();
                return;
            }
        };
        let command = match shell_command(metrics) {
            Ok(command) => command,
            Err(error) => {
                *app_startup_error
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(error);
                cx.quit();
                return;
            }
        };
        let terminal_id = TerminalId::new(1);
        let runtime = match TerminalRuntime::spawn(terminal_id, &command) {
            Ok(runtime) => runtime,
            Err(error) => {
                *app_startup_error
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(error.into());
                cx.quit();
                return;
            }
        };
        let client = runtime.client();
        *startup_runtime_owner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(runtime);
        let mut mux = Mux::default();
        if let Err(error) = mux.insert(
            terminal_id,
            TerminalOwner {
                session_id: SessionId::new(1),
                tab_id: TabId::new(1),
                pane_id: PaneId::new(1),
            },
        ) {
            *app_startup_error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(error.into());
            cx.quit();
            return;
        }

        let bounds = Bounds::centered(
            None,
            size(
                metrics.cell_width * f32::from(INITIAL_COLUMNS)
                    + px(config.window.padding_x * 2.0),
                metrics.cell_height * f32::from(INITIAL_ROWS)
                    + px(config.window.padding_y * 2.0)
                    + titlebar_inset(cfg!(target_os = "macos"), false),
            ),
            cx,
        );
        let theme = config.theme.clone();
        let window = match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Huterm".into()),
                    appears_transparent: cfg!(target_os = "macos"),
                    ..TitlebarOptions::default()
                }),
                app_id: Some(APP_ID.into()),
                ..WindowOptions::default()
            },
            move |window, cx| {
                let scaled_metrics = metrics.at_scale(window.scale_factor());
                if scaled_metrics != metrics {
                    window.resize(size(
                        scaled_metrics.cell_width * f32::from(INITIAL_COLUMNS)
                            + px(config.window.padding_x * 2.0),
                        scaled_metrics.cell_height * f32::from(INITIAL_ROWS)
                            + px(config.window.padding_y * 2.0)
                            + titlebar_inset(cfg!(target_os = "macos"), false),
                    ));
                }
                let metrics = scaled_metrics;
                let focus = cx.focus_handle();
                let view = cx.new(|cx| {
                    let focus_subscription = cx.on_focus(
                        &focus,
                        window,
                        |view: &mut TerminalView, _, cx| {
                            if view.enqueue_input(TerminalInput::Focus(true)) {
                                cx.notify();
                            }
                        },
                    );
                    let blur_subscription = cx.on_blur(
                        &focus,
                        window,
                        |view: &mut TerminalView, _, cx| {
                            if view.enqueue_input(TerminalInput::Focus(false)) {
                                cx.notify();
                            }
                        },
                    );
                    let mut view = TerminalView {
                        mux,
                        client,
                        pending_inputs: VecDeque::new(),
                        pending_input_bytes: 0,
                        pending_resize: None,
                        snapshot: None,
                        renderer: Rc::new(RefCell::new(TerminalRenderer::new(
                            font_family.clone(),
                            theme.clone(),
                            metrics,
                        ))),
                        focus,
                        _focus_subscriptions: vec![
                            focus_subscription,
                            blur_subscription,
                        ],
                        scroll: ScrollController::default(),
                        last_grid_size: GridSize::clamped(
                            INITIAL_COLUMNS,
                            INITIAL_ROWS,
                        ),
                        metrics,
                        font_family,
                        last_cell_size: None,
                        reload_task: None,
                        font_size: metrics.font_size,
                        window_config: config.window,
                        theme,
                        config_path,
                        status: config_error,
                        selection: None,
                        selected_text: None,
                        selecting: false,
                        scrollbar_dragging: false,
                        scrollbar_drag_offset: px(0.0),
                        scrollbar_hovering: false,
                        scrollbar_visibility: IndicatorVisibility::default(),
                        scrollbar_expansion: ScrollbarExpansion::default(),
                        resize_visibility: IndicatorVisibility::default(),
                        last_viewport: None,
                        selection_edge_direction: 0,
                        scroll_benchmark: ScrollBenchmark::from_environment(),
                        snapshot_sequence: 0,
                    };
                    view.start_initial_snapshot(cx);
                    TerminalView::start_event_pump(cx);
                    view
                });
                view.read(cx).focus.focus(window);
                view
            },
        ) {
            Ok(window) => window,
            Err(error) => {
                *app_startup_error
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(anyhow::anyhow!(
                        "failed to open the Huterm window: {error}"
                    ));
                cx.quit();
                return;
            }
        };
        let view = match window.update(cx, |_view, _, cx| cx.entity()) {
            Ok(view) => view,
            Err(error) => {
                *app_startup_error
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(anyhow::anyhow!(
                        "failed to initialize the Huterm window: {error}"
                    ));
                cx.quit();
                return;
            }
        };
        cx.observe_keystrokes(move |event, _, cx| {
            if event.action.is_some() || reserved_keystroke(&event.keystroke) {
                return;
            }
            view.update(cx, |view, cx| {
                if view.handle_keystroke(&event.keystroke) {
                    cx.notify();
                }
                view.start_snapshot_if_needed(cx);
            });
        })
        .detach();
        cx.activate(true);
    });

    if let Some(error) = startup_error
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    {
        return Err(error);
    }
    shutdown_runtime(&runtime_owner).map_err(Into::into)
}

fn install_bindings(cx: &mut App) {
    let mut bindings = vec![
        KeyBinding::new("shift-pageup", ScrollPageUp, None),
        KeyBinding::new("shift-pagedown", ScrollPageDown, None),
        KeyBinding::new("shift-end", ScrollToBottom, None),
        KeyBinding::new("f11", ToggleFullscreen, None),
    ];
    if cfg!(target_os = "macos") {
        bindings.extend([
            KeyBinding::new("cmd-c", Copy, None),
            KeyBinding::new("cmd-v", Paste, None),
            KeyBinding::new("cmd-,", Settings, None),
            reload_binding(true),
            KeyBinding::new("ctrl-cmd-f", ToggleFullscreen, None),
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-m", Minimize, None),
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("cmd-alt-h", HideOthers, None),
        ]);
    } else {
        bindings.extend([
            KeyBinding::new("ctrl-shift-c", Copy, None),
            KeyBinding::new("ctrl-shift-v", Paste, None),
            reload_binding(false),
        ]);
    }
    cx.bind_keys(bindings);
}

fn reload_binding(is_macos: bool) -> KeyBinding {
    // GPUI folds Shift+comma into '<' and clears Shift on both backends.
    KeyBinding::new(
        if is_macos { "cmd-<" } else { "ctrl-<" },
        ReloadConfiguration,
        None,
    )
}

fn install_menus(cx: &mut App) {
    if !cfg!(target_os = "macos") {
        return;
    }
    cx.set_menus(vec![
        Menu {
            name: "Huterm".into(),
            items: vec![
                MenuItem::action("About Huterm", About),
                MenuItem::action("Settings...", Settings),
                MenuItem::action("Reload Configuration", ReloadConfiguration),
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Hide Huterm", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::action("Show All", ShowAll),
                MenuItem::separator(),
                MenuItem::action("Quit Huterm", Quit),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Copy", Copy),
                MenuItem::action("Paste", Paste),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Scroll Page Up", ScrollPageUp),
                MenuItem::action("Scroll Page Down", ScrollPageDown),
                MenuItem::action("Scroll to Bottom", ScrollToBottom),
                MenuItem::separator(),
                MenuItem::action("Toggle Full Screen", ToggleFullscreen),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimize", Minimize),
                MenuItem::action("Zoom", Zoom),
            ],
        },
    ]);
}

fn resolve_metrics(
    config: &Config,
    cx: &App,
) -> anyhow::Result<(String, GridMetrics)> {
    let text_system = cx.text_system();
    let mut family = config.font.family.clone();
    if family != "monospace"
        && !text_system
            .all_font_names()
            .iter()
            .any(|name| name == &family)
    {
        let fallback = if cfg!(target_os = "macos") {
            "Menlo"
        } else {
            "monospace"
        };
        eprintln!("Huterm font {family:?} is unavailable; using {fallback}");
        family = fallback.into();
    }
    let font_size = px(config.font.size);
    let metrics = GridMetrics::resolve(text_system, &family, font_size)?;
    Ok((family, metrics))
}

fn shutdown_runtime(
    runtime_owner: &Mutex<Option<TerminalRuntime>>,
) -> Result<(), RuntimeError> {
    let runtime = runtime_owner
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    runtime.map_or(Ok(()), TerminalRuntime::shutdown)
}

#[derive(Clone, Copy, Debug)]
struct Selection {
    generation: u64,
    anchor: BufferPoint,
    head: BufferPoint,
}
impl Selection {
    fn range(self) -> Option<BufferRange> {
        // Buffer ranges include both endpoints, so equal cells would select
        // one character even though the pointer has not dragged across a cell.
        (self.anchor != self.head)
            .then(|| BufferRange::ordered(self.anchor, self.head))
    }
}

struct TerminalView {
    mux: Mux,
    client: RuntimeClient,
    pending_inputs: VecDeque<TerminalInput>,
    pending_input_bytes: usize,
    pending_resize: Option<(GridSize, CellSize)>,
    snapshot: Option<Arc<TerminalSnapshot>>,
    renderer: Rc<RefCell<TerminalRenderer>>,
    focus: FocusHandle,
    _focus_subscriptions: Vec<Subscription>,
    scroll: ScrollController,
    last_grid_size: GridSize,
    last_cell_size: Option<CellSize>,
    reload_task: Option<gpui::Task<()>>,
    metrics: GridMetrics,
    font_family: String,
    font_size: Pixels,
    window_config: WindowConfig,
    theme: Theme,
    config_path: PathBuf,
    status: Option<String>,
    selection: Option<Selection>,
    selected_text: Option<String>,
    selecting: bool,
    scrollbar_dragging: bool,
    scrollbar_drag_offset: Pixels,
    scrollbar_hovering: bool,
    scrollbar_visibility: IndicatorVisibility,
    scrollbar_expansion: ScrollbarExpansion,
    resize_visibility: IndicatorVisibility,
    last_viewport: Option<gpui::Size<Pixels>>,
    selection_edge_direction: i64,
    scroll_benchmark: Option<ScrollBenchmark>,
    snapshot_sequence: u64,
}

impl Drop for TerminalView {
    fn drop(&mut self) {
        let _ = self.mux.close(self.client.terminal_id());
        let _ = self.client.close();
    }
}

impl TerminalView {
    fn start_event_pump(cx: &mut Context<'_, Self>) {
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                if view.update(cx, TerminalView::refresh).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn start_initial_snapshot(&mut self, cx: &mut Context<'_, Self>) {
        self.scroll.invalidate();
        self.start_snapshot_if_needed(cx);
    }

    fn start_snapshot_if_needed(&mut self, cx: &mut Context<'_, Self>) {
        let Some(viewport) = self.scroll.begin_request() else {
            return;
        };
        let request = match self.client.request_snapshot(viewport) {
            Ok(request) => request,
            Err(error) => {
                self.scroll.fail();
                self.set_status(error.to_string());
                return;
            }
        };
        self.snapshot_sequence = self.snapshot_sequence.saturating_add(1);
        if self
            .scroll_benchmark
            .as_ref()
            .is_some_and(ScrollBenchmark::is_started)
        {
            let injected_at =
                self.scroll_benchmark.as_mut().and_then(|benchmark| {
                    benchmark.take_injection(viewport.bottom_offset)
                });
            self.renderer.borrow_mut().begin_scroll_sample(
                self.snapshot_sequence,
                viewport.bottom_offset,
                injected_at,
            );
        }
        if self
            .scroll_benchmark
            .as_mut()
            .is_some_and(ScrollBenchmark::queue_during_inflight)
        {
            if self.scroll.scroll_rows(1)
                && let Some(benchmark) = &mut self.scroll_benchmark
            {
                benchmark.record_injection(self.scroll.desired());
            }
            let _ = self.scroll.begin_request();
        }
        cx.spawn(async move |view, cx| {
            let result = request.recv().await;
            let _ = view.update(cx, |view, cx| {
                match result {
                    Ok(reply) => {
                        view.renderer.borrow_mut().complete_scroll_snapshot(
                            reply.snapshot_duration,
                            reply.snapshot.viewport.bottom_offset,
                            reply.completed_at.elapsed(),
                        );
                        view.apply_snapshot(reply.snapshot);
                    }
                    Err(error) => {
                        view.scroll.fail();
                        view.set_status(error.to_string());
                    }
                }
                view.start_snapshot_if_needed(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_snapshot(&mut self, snapshot: TerminalSnapshot) {
        self.scroll
            .complete(snapshot.viewport, snapshot.history_size);
        if self.selection.is_some_and(|selection| {
            selection.generation != snapshot.generation
        }) {
            self.clear_selection();
        }
        if self.selecting
            && self.selection_edge_direction != 0
            && self.selection.is_some()
        {
            let row = if self.selection_edge_direction > 0 {
                usize::from(snapshot.size.rows.saturating_sub(1))
            } else {
                0
            };
            if let Some(selection) = &mut self.selection {
                selection.head.rows_from_live_bottom =
                    snapshot.viewport.bottom_offset.saturating_add(row);
            }
            self.scroll.scroll_rows(self.selection_edge_direction);
            self.update_renderer_selection();
        }
        self.snapshot = Some(Arc::new(snapshot));
    }

    fn refresh(&mut self, cx: &mut Context<'_, Self>) {
        let mut changed = self.retry_client_messages();
        changed |= self.scrollbar_visibility.update(
            Instant::now(),
            self.scrollbar_dragging || self.scrollbar_hovering,
        );
        changed |= self.resize_visibility.update(Instant::now(), false);
        changed |= self.scrollbar_expansion.update(
            Instant::now(),
            self.scrollbar_visibility.opacity > 0.0,
            self.scrollbar_hovering || self.scrollbar_dragging,
        );
        loop {
            match self.client.try_recv_event() {
                Ok(Some(TerminalEvent::Invalidated { generation, .. })) => {
                    if self.selection.is_some_and(|selection| {
                        selection.generation != generation
                    }) {
                        self.clear_selection();
                        changed = true;
                    }
                    self.scroll.invalidate();
                }
                Ok(Some(TerminalEvent::Ready(_))) => self.scroll.invalidate(),
                Ok(Some(TerminalEvent::Exited { status, .. })) => {
                    changed |= self.set_status(status.code.map_or_else(
                        || "Process exited".into(),
                        |code| format!("Process exited with status {code}"),
                    ));
                    self.scroll.invalidate();
                }
                Ok(Some(TerminalEvent::Failed { message, .. })) => {
                    changed |= self.set_status(message);
                    self.scroll.invalidate();
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
        if let Some(benchmark) = &mut self.scroll_benchmark {
            changed |= benchmark.drive(
                &mut self.scroll,
                self.last_grid_size.rows,
                f32::from(self.metrics.cell_height),
            );
            benchmark.report(self.scroll.diagnostics());
        }
        self.start_snapshot_if_needed(cx);
        if changed {
            cx.notify();
        }
    }

    fn handle_keystroke(&mut self, keystroke: &Keystroke) -> bool {
        if keystroke.modifiers.platform || reserved_keystroke(keystroke) {
            return false;
        }
        let modifiers = protocol_modifiers(keystroke.modifiers);
        let key = match keystroke.key.as_str() {
            "enter" => Some(TerminalKey::Enter),
            "tab" => Some(TerminalKey::Tab),
            "backspace" => Some(TerminalKey::Backspace),
            "escape" => Some(TerminalKey::Escape),
            "up" => Some(TerminalKey::Up),
            "down" => Some(TerminalKey::Down),
            "left" => Some(TerminalKey::Left),
            "right" => Some(TerminalKey::Right),
            "home" => Some(TerminalKey::Home),
            "end" => Some(TerminalKey::End),
            "pageup" => Some(TerminalKey::PageUp),
            "pagedown" => Some(TerminalKey::PageDown),
            "delete" => Some(TerminalKey::Delete),
            _ => None,
        };
        let input = if let Some(key) = key {
            TerminalInput::Key { key, modifiers }
        } else if keystroke.modifiers.control {
            let Some(byte) = control_byte(&keystroke.key) else {
                return false;
            };
            TerminalInput::Text(String::from(char::from(byte)))
        } else if let Some(text) = &keystroke.key_char {
            TerminalInput::Text(text.clone())
        } else {
            return false;
        };
        let changed = self.enqueue_input(input);
        self.scroll.bottom();
        self.scroll.invalidate();
        changed
    }

    fn scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let changed = match event.delta {
            ScrollDelta::Pixels(delta) => self.scroll.scroll_pixels(
                f32::from(delta.y),
                f32::from(self.metrics.cell_height),
            ),
            ScrollDelta::Lines(delta) => self.scroll.scroll_lines(delta.y),
        };
        if self.scroll.history() > 0 {
            self.activate_scrollbar();
            cx.notify();
        }
        if changed {
            self.start_snapshot_if_needed(cx);
        }
    }

    fn page_up(
        &mut self,
        _: &ScrollPageUp,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        cx.stop_propagation();
        if self.scroll.page(self.last_grid_size.rows, true) {
            self.activate_scrollbar();
            self.start_snapshot_if_needed(cx);
            cx.notify();
        }
    }
    fn page_down(
        &mut self,
        _: &ScrollPageDown,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        cx.stop_propagation();
        if self.scroll.page(self.last_grid_size.rows, false) {
            self.activate_scrollbar();
            self.start_snapshot_if_needed(cx);
            cx.notify();
        }
    }
    fn scroll_to_bottom(
        &mut self,
        _: &ScrollToBottom,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        cx.stop_propagation();
        if self.scroll.bottom() {
            self.activate_scrollbar();
            self.start_snapshot_if_needed(cx);
            cx.notify();
        }
    }
    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<'_, Self>) {
        cx.stop_propagation();
        if let Some(text) =
            cx.read_from_clipboard().and_then(|item| item.text())
        {
            self.enqueue_input(TerminalInput::Paste(text));
            self.scroll.bottom();
            self.scroll.invalidate();
            self.start_snapshot_if_needed(cx);
            cx.notify();
        }
    }
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<'_, Self>) {
        cx.stop_propagation();
        if let Some(text) = &self.selected_text {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        }
    }
    fn settings(
        &mut self,
        _: &Settings,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        cx.stop_propagation();
        match config::create_default(&self.config_path) {
            Ok(()) => cx.open_with_system(&self.config_path),
            Err(error) => {
                self.set_status(format!("failed to open settings: {error}"));
                cx.notify();
            }
        }
    }
    fn reload_configuration(
        &mut self,
        _: &ReloadConfiguration,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        cx.stop_propagation();
        if self.reload_task.is_some() {
            return;
        }
        let config_path = self.config_path.clone();
        let task = cx
            .background_executor()
            .spawn(async move { config::reload(&config_path) });
        self.reload_task = Some(cx.spawn(async move |view, cx| {
            let result = task.await;
            let _ = view.update(cx, |view, cx| {
                view.reload_task = None;
                let result = result.and_then(|config| {
                    resolve_metrics(&config, cx)
                        .map(|(family, metrics)| (config, family, metrics))
                        .map_err(|error| error.to_string())
                });
                match result {
                    Ok((config, family, metrics)) => {
                        let metrics =
                            metrics.at_scale(view.metrics.scale_factor);
                        view.renderer.borrow_mut().reconfigure(
                            family.clone(),
                            config.theme.clone(),
                            metrics,
                        );
                        view.font_family = family;
                        view.font_size = metrics.font_size;
                        view.metrics = metrics;
                        view.window_config = config.window;
                        view.theme = config.theme;
                        view.status = None;
                    }
                    Err(error) => {
                        view.set_status(format!(
                            "Config reload failed: {error}"
                        ));
                    }
                }
                cx.notify();
            });
        }));
    }
    #[expect(clippy::unused_self, reason = "GPUI actions receive the view")]
    fn about(
        &mut self,
        _: &About,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        cx.stop_propagation();
        let detail = format!("Version {}\n{APP_ID}", env!("CARGO_PKG_VERSION"));
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
    }
    #[expect(clippy::unused_self, reason = "GPUI actions receive the view")]
    fn toggle_fullscreen(
        &mut self,
        _: &ToggleFullscreen,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        cx.stop_propagation();
        window.toggle_fullscreen();
    }
    #[expect(clippy::unused_self, reason = "GPUI actions receive the view")]
    fn minimize(
        &mut self,
        _: &Minimize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        cx.stop_propagation();
        window.minimize_window();
    }
    #[expect(clippy::unused_self, reason = "GPUI actions receive the view")]
    fn zoom(
        &mut self,
        _: &Zoom,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        cx.stop_propagation();
        window.zoom_window();
    }

    fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.focus.focus(window);
        let position = event.position - point(px(0.0), terminal_top(window));
        if let Some(geometry) = self.scrollbar_at(position, window) {
            if geometry.contains(f32::from(position.y)) {
                self.scrollbar_dragging = true;
                self.scrollbar_expansion.activate(Instant::now());
                self.scrollbar_drag_offset =
                    position.y - px(geometry.thumb_start);
                self.activate_scrollbar();
                cx.notify();
            } else {
                let upward = f32::from(position.y) < geometry.thumb_start;
                if self.scroll.page(self.last_grid_size.rows, upward) {
                    self.activate_scrollbar();
                    self.start_snapshot_if_needed(cx);
                    cx.notify();
                }
            }
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let position = position - self.terminal_layout(window).bounds.origin;
        let point = point_for_position(position, snapshot, self.metrics);
        self.selection = Some(Selection {
            generation: snapshot.generation,
            anchor: point,
            head: point,
        });
        self.selected_text = None;
        self.selecting = true;
        self.update_renderer_selection();
        cx.notify();
    }

    fn mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let position = event.position - point(px(0.0), terminal_top(window));
        let was_hovering = self.scrollbar_hovering;
        self.scrollbar_hovering = self.scrollbar_at(position, window).is_some();
        if self.scrollbar_hovering {
            self.scrollbar_expansion.activate(Instant::now());
        }
        if self.scrollbar_hovering != was_hovering {
            self.activate_scrollbar();
            cx.notify();
        }
        if self.scrollbar_dragging {
            self.scrollbar_seek(
                position.y - self.scrollbar_drag_offset,
                window,
                cx,
            );
            return;
        }
        if !self.selecting {
            return;
        }
        let layout = self.terminal_layout(window);
        let position = position - layout.bounds.origin;
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if let Some(selection) = &mut self.selection {
            selection.head =
                point_for_position(position, snapshot, self.metrics);
            self.selected_text = None;
            self.update_renderer_selection();
            if self.selection.and_then(Selection::range).is_none() {
                self.selection_edge_direction = 0;
                cx.notify();
                return;
            }
            let direction = edge_scroll_direction(
                f32::from(position.y),
                f32::from(layout.bounds.size.height),
            );
            self.selection_edge_direction = direction;
            if direction != 0 && self.scroll.scroll_rows(direction) {
                self.activate_scrollbar();
                self.start_snapshot_if_needed(cx);
            }
            cx.notify();
        }
    }

    fn mouse_up(
        &mut self,
        _: &MouseUpEvent,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.scrollbar_dragging {
            self.scrollbar_dragging = false;
            self.activate_scrollbar();
            cx.notify();
        }
        self.selection_edge_direction = 0;
        if !self.selecting {
            return;
        }
        self.selecting = false;
        let Some(selection) = self.selection else {
            return;
        };
        let Some(range) = selection.range() else {
            self.clear_selection();
            cx.notify();
            return;
        };
        let request =
            match self.client.request_selection(selection.generation, range) {
                Ok(request) => request,
                Err(error) => {
                    self.set_status(error.to_string());
                    return;
                }
            };
        cx.spawn(async move |view, cx| {
            let result = request.recv().await;
            let _ = view.update(cx, |view, cx| {
                let current = selection_request_is_current(
                    view.selection,
                    selection,
                    range,
                );
                match result {
                    Ok(Some(text)) if current => {
                        view.selected_text = Some(text);
                    }
                    Ok(None) if current => view.clear_selection(),
                    Ok(_) => {}
                    Err(error) => {
                        view.set_status(error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn scrollbar_seek(
        &mut self,
        thumb_start: Pixels,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(geometry) = self.scrollbar_geometry(window) else {
            return;
        };
        let offset = geometry.offset_for_thumb_start(f32::from(thumb_start));
        if self.scroll.set_desired(offset) {
            self.activate_scrollbar();
            self.start_snapshot_if_needed(cx);
            cx.notify();
        }
    }

    fn scrollbar_geometry(&self, window: &Window) -> Option<ScrollbarGeometry> {
        ScrollbarGeometry::new(
            f32::from(terminal_viewport(window).height),
            self.last_grid_size.rows,
            self.scroll.history(),
            self.scroll.displayed(),
        )
    }

    fn scrollbar_expanded(&self) -> bool {
        self.scrollbar_expansion.active()
    }

    fn scrollbar_at(
        &self,
        position: gpui::Point<Pixels>,
        window: &Window,
    ) -> Option<ScrollbarGeometry> {
        let geometry = self.scrollbar_geometry(window)?;
        scrollbar_hit_test(
            position,
            terminal_viewport(window).width,
            geometry,
            self.scrollbar_visibility.opacity,
            self.scrollbar_expanded(),
        )
        .then_some(geometry)
    }
    fn clear_selection(&mut self) {
        self.selection = None;
        self.selected_text = None;
        self.selecting = false;
        self.selection_edge_direction = 0;
        self.update_renderer_selection();
    }
    fn activate_scrollbar(&mut self) {
        self.scrollbar_visibility.activate(Instant::now());
    }
    fn update_renderer_selection(&self) {
        self.renderer
            .borrow_mut()
            .set_selection(self.selection.and_then(Selection::range));
    }

    fn terminal_layout(&self, window: &Window) -> TerminalLayout {
        TerminalLayout::new(
            terminal_viewport(window),
            size(self.metrics.cell_width, self.metrics.cell_height),
            self.window_config,
        )
    }

    fn resize_if_needed(&mut self, window: &Window) {
        let metrics = self.metrics.at_scale(window.scale_factor());
        if metrics != self.metrics {
            self.metrics = metrics;
            self.renderer.borrow_mut().reconfigure(
                self.font_family.clone(),
                self.theme.clone(),
                metrics,
            );
        }
        let viewport = terminal_viewport(window);
        if self
            .last_viewport
            .replace(viewport)
            .is_some_and(|previous| previous != viewport)
        {
            self.resize_visibility.activate(Instant::now());
        }
        let size = self.terminal_layout(window).grid;
        let cell = CellSize {
            width: pixel_count(
                self.metrics.cell_width * self.metrics.scale_factor,
            ),
            height: pixel_count(
                self.metrics.cell_height * self.metrics.scale_factor,
            ),
        };
        if size == self.last_grid_size && self.last_cell_size == Some(cell) {
            return;
        }
        self.last_grid_size = size;
        self.last_cell_size = Some(cell);
        match self.client.resize(size, cell) {
            Ok(()) => self.pending_resize = None,
            Err(RuntimeError::Busy) => self.pending_resize = Some((size, cell)),
            Err(error) => self.status = Some(error.to_string()),
        }
    }

    fn enqueue_input(&mut self, input: TerminalInput) -> bool {
        if self.pending_inputs.is_empty() {
            match self.client.send_input(input.clone()) {
                Ok(()) => return false,
                Err(RuntimeError::Busy) => {}
                Err(error) => return self.set_status(error.to_string()),
            }
        }
        let input_bytes = buffered_input_bytes(&input);
        if self.pending_inputs.len() == PENDING_INPUT_CAPACITY
            || self.pending_input_bytes.saturating_add(input_bytes)
                > PENDING_INPUT_BYTE_CAPACITY
        {
            return self.set_status(format!("Input buffer full ({PENDING_INPUT_CAPACITY} events or {PENDING_INPUT_BYTE_CAPACITY} bytes); input rejected"));
        }
        self.pending_inputs.push_back(input);
        self.pending_input_bytes += input_bytes;
        false
    }

    fn retry_client_messages(&mut self) -> bool {
        while let Some(input) = self.pending_inputs.front().cloned() {
            match self.client.send_input(input) {
                Ok(()) => {
                    if let Some(sent) = self.pending_inputs.pop_front() {
                        self.pending_input_bytes = self
                            .pending_input_bytes
                            .saturating_sub(buffered_input_bytes(&sent));
                    }
                }
                Err(RuntimeError::Busy) => break,
                Err(error) => {
                    self.pending_inputs.clear();
                    self.pending_input_bytes = 0;
                    return self.set_status(error.to_string());
                }
            }
        }
        if let Some((grid, cell)) = self.pending_resize {
            match self.client.resize(grid, cell) {
                Ok(()) => self.pending_resize = None,
                Err(RuntimeError::Busy) => {}
                Err(error) => {
                    self.pending_resize = None;
                    return self.set_status(error.to_string());
                }
            }
        }
        false
    }
    fn set_status(&mut self, status: String) -> bool {
        if self.status.as_ref() == Some(&status) {
            false
        } else {
            self.status = Some(status);
            true
        }
    }
}

struct ScrollBenchmark {
    step: u64,
    started: bool,
    queue_next: bool,
    pending_injection: Option<(usize, Instant)>,
    last_report: Instant,
    display_scale: Option<f32>,
}

impl ScrollBenchmark {
    fn from_environment() -> Option<Self> {
        std::env::var("HUTERM_SCROLL_BENCH")
            .is_ok_and(|value| {
                value == "1" || value.eq_ignore_ascii_case("true")
            })
            .then(|| Self {
                step: 0,
                started: false,
                queue_next: false,
                pending_injection: None,
                last_report: Instant::now(),
                display_scale: None,
            })
    }

    fn is_started(&self) -> bool {
        self.started
    }

    fn drive(
        &mut self,
        scroll: &mut ScrollController,
        visible_rows: u16,
        row_height: f32,
    ) -> bool {
        let Some(display_scale) = self.display_scale else {
            return false;
        };
        if scroll.history() < 10_000 {
            return false;
        }
        if !self.started {
            self.started = true;
            eprintln!(
                "huterm-scroll environment revision={} profile=release os={} arch={} hardware={} display_scale={} gpui=0.2.2 viewport={}x{} history={}",
                benchmark_environment("HUTERM_SCROLL_REVISION"),
                std::env::consts::OS,
                std::env::consts::ARCH,
                benchmark_environment("HUTERM_SCROLL_HARDWARE"),
                display_scale,
                INITIAL_COLUMNS,
                visible_rows,
                scroll.history(),
            );
        }
        let changed = match self.step % 10 {
            0 => scroll.scroll_rows(1),
            1 => scroll.scroll_pixels(row_height * 0.4, row_height),
            2 => scroll.scroll_pixels(row_height * 0.7, row_height),
            3 => scroll.page(visible_rows, true),
            4 => scroll.page(visible_rows, false),
            5 => scroll.set_desired(scroll.history() / 2),
            6 => scroll.set_desired(scroll.history()),
            7 => scroll.scroll_rows(-1),
            8 => scroll.bottom(),
            _ => scroll.set_desired(scroll.history().saturating_sub(1)),
        };
        self.step = self.step.saturating_add(1);
        self.queue_next = self.step.is_multiple_of(19);
        if changed {
            self.record_injection(scroll.desired());
        }
        changed
    }

    fn queue_during_inflight(&mut self) -> bool {
        std::mem::take(&mut self.queue_next)
    }

    fn record_injection(&mut self, offset: usize) {
        self.pending_injection = Some((offset, Instant::now()));
    }

    fn take_injection(&mut self, offset: usize) -> Option<Instant> {
        self.pending_injection
            .take_if(|(requested, _)| *requested == offset)
            .map(|(_, instant)| instant)
    }

    fn report(&mut self, diagnostics: crate::scroll::ScrollDiagnostics) {
        if self.last_report.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_report = Instant::now();
        eprintln!(
            "huterm-scroll queue requests_started={} requests_completed={} requests_coalesced={} maximum_concurrent={} queued_updates={} maximum_queued={}",
            diagnostics.requests_started,
            diagnostics.requests_completed,
            diagnostics.requests_coalesced,
            diagnostics.maximum_concurrent,
            diagnostics.queued_updates,
            diagnostics.maximum_queued,
        );
    }
}

fn benchmark_environment(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| "unknown".into())
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TerminalView {
    #[expect(
        clippy::too_many_lines,
        reason = "terminal content and window chrome share one render tree"
    )]
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        self.resize_if_needed(window);
        if let Some(benchmark) = &mut self.scroll_benchmark {
            benchmark.display_scale = Some(window.scale_factor());
        }
        if self.renderer.borrow().records_stats() {
            window.request_animation_frame();
        }
        let snapshot = self.snapshot.clone();
        let status = self.status.clone();
        let prepare_renderer = Rc::clone(&self.renderer);
        let paint_renderer = Rc::clone(&self.renderer);
        let mouse_view = cx.entity().downgrade();
        let layout = self.terminal_layout(window);
        let mut root = div()
            .id("terminal")
            .on_hover(cx.listener(|view, hovering, _, cx| {
                if !hovering && view.scrollbar_hovering {
                    view.scrollbar_hovering = false;
                    view.activate_scrollbar();
                    cx.notify();
                }
            }))
            .key_context("Huterm")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::about))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::settings))
            .on_action(cx.listener(Self::reload_configuration))
            .on_action(cx.listener(Self::toggle_fullscreen))
            .on_action(cx.listener(Self::minimize))
            .on_action(cx.listener(Self::zoom))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::scroll_to_bottom))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .relative()
            .w_full()
            .h(terminal_viewport(window).height)
            .bg(color(self.theme.background))
            .text_size(self.font_size)
            .font_family(self.font_family.clone())
            .child(
                canvas(
                    move |_, window, _| {
                        prepare_renderer
                            .borrow_mut()
                            .prepare(snapshot.as_ref(), window);
                    },
                    move |bounds, (), window, _| {
                        let bounds = Bounds::new(
                            bounds.origin + layout.bounds.origin,
                            layout.bounds.size,
                        );
                        window.with_content_mask(
                            Some(gpui::ContentMask { bounds }),
                            |window| {
                                paint_renderer
                                    .borrow_mut()
                                    .paint(bounds, window);
                            },
                        );
                        window.on_mouse_event(
                            move |event: &MouseMoveEvent, phase, window, cx| {
                                if phase == DispatchPhase::Bubble {
                                    let _ =
                                        mouse_view.update(cx, |view, cx| {
                                            view.mouse_move(event, window, cx);
                                        });
                                }
                            },
                        );
                    },
                )
                .size_full(),
            );
        let displayed_offset = self.scroll.displayed();
        if self.scrollbar_visibility.opacity > 0.0
            && let Some(geometry) = self.scrollbar_geometry(window)
        {
            let expansion = self.scrollbar_expansion.progress;
            if expansion > 0.0 {
                root = root.child(
                    div()
                        .absolute()
                        .right(px(2.0))
                        .top(px(geometry.track_start))
                        .w(px(8.0 + 6.0 * expansion))
                        .h(px(geometry.track_size()))
                        .rounded(px(4.0 + 3.0 * expansion))
                        .bg(color(self.theme.foreground).opacity(20.0 / 255.0))
                        .opacity(self.scrollbar_visibility.opacity * expansion),
                );
            }
            root = root.child(
                div()
                    .absolute()
                    .right(px(2.0 + 2.0 * expansion))
                    .top(px(geometry.thumb_start))
                    .w(px(6.0 + 4.0 * expansion))
                    .h(px(geometry.thumb_size))
                    .rounded(px(3.0 + 2.0 * expansion))
                    .bg(color(self.theme.foreground).opacity(187.0 / 255.0))
                    .opacity(self.scrollbar_visibility.opacity),
            );
            if let Some(label) = scroll_position_label(displayed_offset) {
                root = root.child(
                    div()
                        .absolute()
                        .right(
                            SCROLLBAR_WIDTH
                                + (SCROLLBAR_EXPANDED_WIDTH - SCROLLBAR_WIDTH)
                                    * expansion,
                        )
                        .bottom(px(12.0))
                        .px_2()
                        .py_1()
                        .rounded(px(3.0))
                        .bg(color(self.theme.background))
                        .text_color(color(self.theme.foreground))
                        .opacity(self.scrollbar_visibility.opacity)
                        .child(label),
                );
            }
        }
        if self.resize_visibility.opacity > 0.0 {
            root = root.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded(px(3.0))
                            .bg(color(self.theme.background))
                            .text_color(color(self.theme.foreground))
                            .opacity(self.resize_visibility.opacity)
                            .child(format!(
                                "{} x {}",
                                self.last_grid_size.columns,
                                self.last_grid_size.rows
                            )),
                    ),
            );
        }
        let root = root.when_some(status, |view, status| {
            view.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .px_2()
                    .py_1()
                    .bg(color(self.theme.background))
                    .text_color(color(self.theme.foreground))
                    .child(status),
            )
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(color(self.theme.background))
            .when(terminal_top(window) > px(0.0), |view| {
                view.child(
                    div()
                        .h(terminal_top(window))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .pl(px(84.0))
                        .text_size(px(13.0))
                        .text_color(color(self.theme.foreground))
                        .window_control_area(WindowControlArea::Drag)
                        .child("Huterm"),
                )
            })
            .child(root)
    }
}

fn scrollbar_hit_test(
    position: gpui::Point<Pixels>,
    viewport_width: Pixels,
    geometry: ScrollbarGeometry,
    opacity: f32,
    expanded: bool,
) -> bool {
    let width = if expanded {
        SCROLLBAR_EXPANDED_WIDTH
    } else {
        SCROLLBAR_WIDTH
    };
    opacity > 0.0
        && position.x >= (viewport_width - width).max(px(0.0))
        && position.x < viewport_width
        && geometry.track_contains(f32::from(position.y))
}

#[derive(Clone, Copy, Debug)]
struct TerminalLayout {
    bounds: Bounds<Pixels>,
    grid: GridSize,
}

impl TerminalLayout {
    fn new(
        viewport: gpui::Size<Pixels>,
        cell: gpui::Size<Pixels>,
        config: WindowConfig,
    ) -> Self {
        let padding_x = px(config.padding_x).min(viewport.width / 2.0);
        let padding_y = px(config.padding_y).min(viewport.height / 2.0);
        let available = size(
            (viewport.width - padding_x * 2.0).max(px(0.0)),
            (viewport.height - padding_y * 2.0).max(px(0.0)),
        );
        let grid = GridSize::clamped(
            cell_count(available.width, cell.width),
            cell_count(available.height, cell.height),
        );
        let width = (cell.width * f32::from(grid.columns)).min(available.width);
        let height = (cell.height * f32::from(grid.rows)).min(available.height);
        let extra_left = if config.padding_balance {
            (available.width - width) / 2.0
        } else {
            px(0.0)
        };
        Self {
            bounds: Bounds::new(
                point(padding_x + extra_left, padding_y),
                size(width, height),
            ),
            grid,
        }
    }
}

fn titlebar_inset(macos: bool, fullscreen: bool) -> Pixels {
    if macos && !fullscreen {
        TITLEBAR_HEIGHT
    } else {
        px(0.0)
    }
}

fn terminal_top(window: &Window) -> Pixels {
    titlebar_inset(cfg!(target_os = "macos"), window.is_fullscreen())
}

fn terminal_viewport(window: &Window) -> gpui::Size<Pixels> {
    let viewport = window.viewport_size();
    size(
        viewport.width,
        (viewport.height - terminal_top(window)).max(px(0.0)),
    )
}

fn shell_command(metrics: GridMetrics) -> anyhow::Result<TerminalCommand> {
    let shell =
        std::env::var_os("SHELL").map_or_else(default_shell, PathBuf::from);
    let is_macos = cfg!(target_os = "macos");
    let arguments = shell_arguments(is_macos);
    let environment = locale_environment(
        is_macos && is_packaged_macos(),
        ["LANG", "LC_CTYPE", "LC_ALL"]
            .into_iter()
            .any(|name| std::env::var_os(name).is_some()),
    );
    let working_directory = if is_macos {
        config::home_directory()
    } else {
        std::env::current_dir()?
    };
    Ok(TerminalCommand {
        program: shell,
        arguments,
        working_directory,
        environment,
        grid_size: GridSize::clamped(INITIAL_COLUMNS, INITIAL_ROWS),
        cell_size: CellSize {
            width: pixel_count(metrics.cell_width * metrics.scale_factor),
            height: pixel_count(metrics.cell_height * metrics.scale_factor),
        },
    })
}
fn shell_arguments(is_macos: bool) -> Vec<String> {
    if is_macos {
        vec!["-l".into()]
    } else {
        Vec::new()
    }
}
fn locale_environment(
    is_packaged_macos: bool,
    inherited_locale: bool,
) -> Vec<(String, String)> {
    if is_packaged_macos && !inherited_locale {
        vec![("LANG".into(), "en_US.UTF-8".into())]
    } else {
        Vec::new()
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "mouse coordinates are clamped to the visible grid"
)]
fn point_for_position(
    position: gpui::Point<Pixels>,
    snapshot: &TerminalSnapshot,
    metrics: GridMetrics,
) -> BufferPoint {
    let column = ((position.x / metrics.cell_width).floor().max(0.0) as u16)
        .min(snapshot.size.columns.saturating_sub(1));
    let row = ((position.y / metrics.cell_height).floor().max(0.0) as usize)
        .min(usize::from(snapshot.size.rows.saturating_sub(1)));
    BufferPoint {
        rows_from_live_bottom: snapshot.viewport.bottom_offset.saturating_add(
            usize::from(snapshot.size.rows)
                .saturating_sub(1)
                .saturating_sub(row),
        ),
        column,
    }
}
fn protocol_modifiers(modifiers: GpuiModifiers) -> Modifiers {
    Modifiers {
        control: modifiers.control,
        alt: modifiers.alt,
        shift: modifiers.shift,
    }
}
fn reserved_keystroke(keystroke: &Keystroke) -> bool {
    reserved_chord(keystroke.modifiers, &keystroke.key)
}
fn reserved_chord(modifiers: GpuiModifiers, key: &str) -> bool {
    reserved_chord_for_platform(cfg!(target_os = "macos"), modifiers, key)
}
fn reserved_chord_for_platform(
    is_macos: bool,
    modifiers: GpuiModifiers,
    key: &str,
) -> bool {
    if exact_modifiers(modifiers, ModifierChord::Shift) {
        return matches!(key, "pageup" | "pagedown" | "end");
    }
    if is_macos {
        (exact_modifiers(modifiers, ModifierChord::Command)
            && matches!(key, "c" | "v" | "," | "<" | "q" | "m" | "h"))
            || (exact_modifiers(modifiers, ModifierChord::CommandAlt)
                && key == "h")
            || (exact_modifiers(modifiers, ModifierChord::CommandControl)
                && key == "f")
    } else {
        (exact_modifiers(modifiers, ModifierChord::ControlShift)
            && matches!(key, "c" | "v"))
            || (exact_modifiers(modifiers, ModifierChord::Control)
                && key == "<")
    }
}
#[derive(Clone, Copy)]
enum ModifierChord {
    Shift,
    Command,
    CommandAlt,
    CommandControl,
    Control,
    ControlShift,
}
fn exact_modifiers(modifiers: GpuiModifiers, chord: ModifierChord) -> bool {
    let matches = match chord {
        ModifierChord::Shift => modifiers.shift,
        ModifierChord::Command => modifiers.platform,
        ModifierChord::Control => modifiers.control,
        ModifierChord::CommandAlt => modifiers.platform && modifiers.alt,
        ModifierChord::CommandControl => {
            modifiers.platform && modifiers.control
        }
        ModifierChord::ControlShift => modifiers.control && modifiers.shift,
    };
    let count = usize::from(modifiers.control)
        + usize::from(modifiers.alt)
        + usize::from(modifiers.shift)
        + usize::from(modifiers.platform)
        + usize::from(modifiers.function);
    let expected_count = match chord {
        ModifierChord::Shift
        | ModifierChord::Command
        | ModifierChord::Control => 1,
        ModifierChord::CommandAlt
        | ModifierChord::CommandControl
        | ModifierChord::ControlShift => 2,
    };
    matches && count == expected_count
}
fn selection_request_is_current(
    current: Option<Selection>,
    requested: Selection,
    range: BufferRange,
) -> bool {
    current.is_some_and(|current| {
        current.generation == requested.generation
            && current.range() == Some(range)
    })
}
fn format_line_count(value: usize) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.char_indices() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(character);
    }
    formatted
}
fn scroll_position_label(displayed_offset: usize) -> Option<String> {
    (displayed_offset > 0)
        .then(|| format!("{} lines up", format_line_count(displayed_offset)))
}

fn edge_scroll_direction(position: f32, viewport_height: f32) -> i64 {
    if position < 0.0 {
        1
    } else if position >= viewport_height {
        -1
    } else {
        0
    }
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "viewport coordinates are clamped to protocol dimensions"
)]
fn cell_count(viewport: Pixels, cell: Pixels) -> u16 {
    (viewport / cell).floor().clamp(1.0, f32::from(u16::MAX)) as u16
}
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "positive font metrics are clamped to PTY fields"
)]
fn pixel_count(value: Pixels) -> u16 {
    f32::from(value).ceil().clamp(1.0, f32::from(u16::MAX)) as u16
}
fn default_shell() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/bin/zsh")
    } else {
        PathBuf::from("/bin/sh")
    }
}
fn is_packaged_macos() -> bool {
    cfg!(target_os = "macos")
        && std::env::current_exe().is_ok_and(|executable| {
            executable.components().any(|component| {
                component.as_os_str().to_string_lossy().ends_with(".app")
            })
        })
}
fn control_byte(key: &str) -> Option<u8> {
    if key.eq_ignore_ascii_case("space") {
        return Some(0);
    }
    let [byte] = key.as_bytes() else {
        return None;
    };
    let byte = byte.to_ascii_uppercase();
    matches!(byte, b'@'..=b'_').then_some(byte & 0x1f)
}
fn buffered_input_bytes(input: &TerminalInput) -> usize {
    match input {
        TerminalInput::Text(text) | TerminalInput::Paste(text) => text.len(),
        TerminalInput::Key { .. } | TerminalInput::Focus(_) => {
            std::mem::size_of::<TerminalInput>()
        }
        _ => std::mem::size_of::<TerminalInput>(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollbar_hover_target_expands_only_while_visible() {
        let geometry = ScrollbarGeometry::new(400.0, 32, 100, 0).unwrap();
        let position = point(px(85.0), px(200.0));
        assert!(!scrollbar_hit_test(
            position,
            px(100.0),
            geometry,
            1.0,
            false
        ));
        assert!(scrollbar_hit_test(position, px(100.0), geometry, 1.0, true));
        assert!(!scrollbar_hit_test(
            position,
            px(100.0),
            geometry,
            0.0,
            true
        ));
        assert!(scrollbar_hit_test(
            point(px(95.0), px(200.0)),
            px(100.0),
            geometry,
            0.5,
            false
        ));
    }

    #[test]
    fn scrollbar_hit_testing_excludes_insets_and_outside_window() {
        let geometry = ScrollbarGeometry::new(400.0, 32, 100, 0).unwrap();
        for position in [
            point(px(99.0), px(1.0)),
            point(px(99.0), px(393.0)),
            point(px(100.0), px(200.0)),
            point(px(-1.0), px(200.0)),
        ] {
            assert!(!scrollbar_hit_test(
                position,
                px(100.0),
                geometry,
                1.0,
                true
            ));
        }
        assert!(scrollbar_hit_test(
            point(px(99.0), px(392.0)),
            px(100.0),
            geometry,
            1.0,
            true
        ));
    }

    #[test]
    fn terminal_padding_keeps_remainder_on_right_and_bottom_by_default() {
        let layout = TerminalLayout::new(
            size(px(105.0), px(59.0)),
            size(px(8.0), px(16.0)),
            WindowConfig::default(),
        );
        assert_eq!(layout.grid, GridSize::clamped(12, 3));
        assert_eq!(
            layout.bounds,
            Bounds::new(point(px(4.0), px(4.0)), size(px(96.0), px(48.0)))
        );
    }

    #[test]
    fn balanced_padding_centers_columns_without_changing_rows_or_grid_size() {
        let layout = TerminalLayout::new(
            size(px(111.0), px(59.0)),
            size(px(8.0), px(16.0)),
            WindowConfig {
                padding_balance: true,
                ..WindowConfig::default()
            },
        );
        assert_eq!(layout.grid, GridSize::clamped(12, 3));
        assert_eq!(layout.bounds.origin, point(px(7.5), px(4.0)));
        assert_eq!(px(111.0) - layout.bounds.right(), layout.bounds.origin.x);
        let grid_position = point(px(15.5), px(20.0)) - layout.bounds.origin;
        assert_eq!(grid_position, point(px(8.0), px(16.0)));
    }

    #[test]
    fn balanced_padding_leaves_exact_cell_fit_unchanged() {
        let layout = TerminalLayout::new(
            size(px(104.0), px(56.0)),
            size(px(8.0), px(16.0)),
            WindowConfig {
                padding_balance: true,
                ..WindowConfig::default()
            },
        );
        assert_eq!(layout.bounds.origin, point(px(4.0), px(4.0)));
        assert_eq!(layout.grid, GridSize::clamped(12, 3));
    }

    #[test]
    fn excessive_padding_clips_tiny_viewports_without_negative_dimensions() {
        let layout = TerminalLayout::new(
            size(px(3.0), px(2.0)),
            size(px(8.0), px(16.0)),
            WindowConfig {
                padding_balance: true,
                ..WindowConfig::default()
            },
        );
        assert_eq!(layout.grid, GridSize::clamped(1, 1));
        assert_eq!(
            layout.bounds,
            Bounds::new(point(px(1.5), px(1.0)), size(px(0.0), px(0.0)))
        );
    }
    #[test]
    fn control_byte_should_only_accept_ascii_control_keys_and_named_space() {
        assert_eq!(control_byte("a"), Some(1));
        assert_eq!(control_byte("["), Some(27));
        assert_eq!(control_byte("space"), Some(0));
        assert_eq!(control_byte("enter"), None);
        assert_eq!(control_byte("é"), None);
    }
    #[test]
    fn buffered_input_bytes_should_include_owned_text() {
        assert_eq!(buffered_input_bytes(&TerminalInput::Text("abc".into())), 3);
        assert!(
            buffered_input_bytes(&TerminalInput::Focus(true))
                >= std::mem::size_of::<bool>()
        );
    }
    #[test]
    fn selection_drag_edges_scroll_in_reading_direction() {
        assert_eq!(edge_scroll_direction(-1.0, 400.0), 1);
        assert_eq!(edge_scroll_direction(200.0, 400.0), 0);
        assert_eq!(edge_scroll_direction(400.0, 400.0), -1);
    }
    #[test]
    fn application_shortcuts_are_reserved_but_plain_control_c_is_not() {
        assert!(!reserved_chord(modifiers(&[TestModifier::Control]), "c"));
        assert!(reserved_chord(modifiers(&[TestModifier::Shift]), "pageup"));

        let clipboard = if cfg!(target_os = "macos") {
            modifiers(&[TestModifier::Platform])
        } else {
            modifiers(&[TestModifier::Control, TestModifier::Shift])
        };
        assert!(reserved_chord(clipboard, "c"));
        assert!(reserved_chord(clipboard, "v"));
    }
    #[test]
    fn reload_binding_matches_gpui_shifted_punctuation() {
        // Both native backends fold Shift+comma into '<' without Shift.
        for (is_macos, modifier) in [
            (true, TestModifier::Platform),
            (false, TestModifier::Control),
        ] {
            let event = Keystroke {
                modifiers: modifiers(&[modifier]),
                key: "<".into(),
                key_char: None,
            };
            assert_eq!(
                reload_binding(is_macos)
                    .match_keystrokes(std::slice::from_ref(&event)),
                Some(false) // Complete match, not a pending chord prefix.
            );
            assert!(reserved_chord_for_platform(
                is_macos,
                event.modifiers,
                &event.key
            ));
        }
    }

    #[test]
    fn macos_shortcuts_require_exact_modifiers() {
        assert!(reserved_chord_for_platform(
            true,
            modifiers(&[TestModifier::Platform]),
            "<",
        ));
        let command = modifiers(&[TestModifier::Platform]);
        assert!(reserved_chord_for_platform(true, command, "h"));
        assert!(reserved_chord_for_platform(
            true,
            modifiers(&[TestModifier::Alt, TestModifier::Platform]),
            "h"
        ));
        assert!(reserved_chord_for_platform(
            true,
            modifiers(&[TestModifier::Control, TestModifier::Platform]),
            "f"
        ));
        assert!(!reserved_chord_for_platform(
            true,
            modifiers(&[TestModifier::Alt, TestModifier::Platform]),
            "c"
        ));
        let mut with_function = command;
        with_function.function = true;
        assert!(!reserved_chord_for_platform(true, with_function, "c"));
    }
    #[test]
    fn linux_shortcuts_require_exact_modifiers() {
        assert!(reserved_chord_for_platform(
            false,
            modifiers(&[TestModifier::Control]),
            "<",
        ));
        let clipboard =
            modifiers(&[TestModifier::Control, TestModifier::Shift]);
        assert!(reserved_chord_for_platform(false, clipboard, "c"));
        assert!(reserved_chord_for_platform(false, clipboard, "v"));
        assert!(!reserved_chord_for_platform(
            false,
            modifiers(&[
                TestModifier::Control,
                TestModifier::Alt,
                TestModifier::Shift,
            ]),
            "c"
        ));
        assert!(!reserved_chord_for_platform(
            false,
            modifiers(&[
                TestModifier::Control,
                TestModifier::Shift,
                TestModifier::Platform,
            ]),
            "c"
        ));
        let mut with_function = clipboard;
        with_function.function = true;
        assert!(!reserved_chord_for_platform(false, with_function, "c"));
    }
    #[test]
    fn stale_selection_replies_do_not_match_newer_selection() {
        let requested = selection(1, 2, 4);
        assert!(!selection_request_is_current(
            Some(selection(1, 2, 2)),
            requested,
            requested.range().unwrap()
        ));
        assert!(selection_request_is_current(
            Some(requested),
            requested,
            requested.range().unwrap()
        ));
        assert!(!selection_request_is_current(
            Some(selection(2, 2, 4)),
            requested,
            requested.range().unwrap()
        ));
        assert!(!selection_request_is_current(
            Some(selection(1, 3, 4)),
            requested,
            requested.range().unwrap()
        ));
    }
    #[test]
    fn line_counts_use_thousands_separators() {
        assert_eq!(format_line_count(0), "0");
        assert_eq!(format_line_count(999), "999");
        assert_eq!(format_line_count(1_284), "1,284");
        assert_eq!(format_line_count(10_000), "10,000");
        assert_eq!(format_line_count(1_000_000), "1,000,000");
    }

    #[test]
    fn selection_requires_dragging_out_of_the_initial_cell() {
        let mut candidate = selection(1, 2, 2);
        assert_eq!(candidate.range(), None, "a click must not select a cell");
        candidate.head.column = 3;
        assert_eq!(
            candidate.range(),
            Some(BufferRange::ordered(candidate.anchor, candidate.head))
        );
        candidate.head.column = 1;
        assert_eq!(
            candidate.range(),
            Some(BufferRange::ordered(candidate.head, candidate.anchor))
        );
        candidate.head = candidate.anchor;
        assert_eq!(candidate.range(), None);
        candidate.head.rows_from_live_bottom = 1;
        assert!(candidate.range().is_some(), "vertical dragging must select");
    }
    #[test]
    fn live_bottom_hides_its_label_while_the_indicator_fades() {
        assert_eq!(scroll_position_label(0), None);
    }

    #[test]
    fn scroll_position_label_uses_the_displayed_snapshot_offset() {
        assert_eq!(
            scroll_position_label(1_284).as_deref(),
            Some("1,284 lines up")
        );
    }
    #[test]
    fn unmatched_benchmark_request_preserves_the_pending_input() {
        let mut benchmark = ScrollBenchmark {
            step: 0,
            started: true,
            queue_next: false,
            pending_injection: Some((7, Instant::now())),
            last_report: Instant::now(),
            display_scale: Some(1.0),
        };

        assert!(benchmark.take_injection(8).is_none());
        assert!(benchmark.take_injection(7).is_some());
        assert!(benchmark.take_injection(7).is_none());
    }
    #[test]
    fn macos_shell_is_login_and_only_packaged_launches_fill_missing_locale() {
        assert_eq!(shell_arguments(true), vec!["-l"]);
        assert!(shell_arguments(false).is_empty());
        assert_eq!(
            locale_environment(true, false),
            vec![("LANG".into(), "en_US.UTF-8".into())]
        );
        assert!(locale_environment(true, true).is_empty());
        assert!(locale_environment(false, false).is_empty());
    }

    #[derive(Clone, Copy)]
    enum TestModifier {
        Control,
        Alt,
        Shift,
        Platform,
    }

    fn modifiers(active: &[TestModifier]) -> GpuiModifiers {
        let mut modifiers = GpuiModifiers::default();
        for modifier in active {
            match modifier {
                TestModifier::Control => modifiers.control = true,
                TestModifier::Alt => modifiers.alt = true,
                TestModifier::Shift => modifiers.shift = true,
                TestModifier::Platform => modifiers.platform = true,
            }
        }
        modifiers
    }

    fn selection(generation: u64, anchor: u16, head: u16) -> Selection {
        Selection {
            generation,
            anchor: BufferPoint {
                rows_from_live_bottom: 0,
                column: anchor,
            },
            head: BufferPoint {
                rows_from_live_bottom: 0,
                column: head,
            },
        }
    }
}
