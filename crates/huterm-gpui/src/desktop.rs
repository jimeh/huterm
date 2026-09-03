use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    App, Application, Bounds, ClipboardItem, Context, FocusHandle, Focusable,
    KeyBinding, Keystroke, Menu, MenuItem, Modifiers as GpuiModifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    PromptLevel, Render, ScrollDelta, ScrollWheelEvent, Subscription,
    SystemMenuType, TitlebarOptions, Window, WindowBounds, WindowOptions,
    actions, canvas, div, font, prelude::*, px, rgba, size,
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
use crate::config::{self, Config, Theme};
use crate::renderer::{GridMetrics, TerminalRenderer, rgb_color as color};
use crate::scroll::{ScrollController, ScrollbarGeometry};

const INITIAL_COLUMNS: u16 = 100;
const INITIAL_ROWS: u16 = 32;
const PENDING_INPUT_CAPACITY: usize = 256;
const PENDING_INPUT_BYTE_CAPACITY: usize = 1024 * 1024;
const SCROLLBAR_WIDTH: Pixels = px(12.0);

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
                metrics.cell_width * f32::from(INITIAL_COLUMNS),
                metrics.cell_height * f32::from(INITIAL_ROWS),
            ),
            cx,
        );
        let theme = config.theme.clone();
        let window = match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Huterm".into()),
                    ..TitlebarOptions::default()
                }),
                app_id: Some(APP_ID.into()),
                ..WindowOptions::default()
            },
            move |window, cx| {
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
                        font_size: metrics.font_size,
                        theme,
                        config_path,
                        status: config_error,
                        selection: None,
                        selected_text: None,
                        selecting: false,
                        scrollbar_dragging: false,
                        scrollbar_drag_offset: px(0.0),
                        scrollbar_hovering: false,
                        scrollbar_active_until: None,
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
            KeyBinding::new("ctrl-cmd-f", ToggleFullscreen, None),
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-m", Minimize, None),
        ]);
    } else {
        bindings.extend([
            KeyBinding::new("ctrl-shift-c", Copy, None),
            KeyBinding::new("ctrl-shift-v", Paste, None),
        ]);
    }
    cx.bind_keys(bindings);
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
    let font_id = text_system.resolve_font(&font(family.clone()));
    let cell_width = text_system.advance(font_id, font_size, 'M')?.width;
    let ascent = text_system.ascent(font_id, font_size);
    let descent = text_system.descent(font_id, font_size);
    Ok((
        family,
        GridMetrics::from_measurements(font_size, cell_width, ascent, descent),
    ))
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
    fn range(self) -> BufferRange {
        BufferRange::ordered(self.anchor, self.head)
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
    metrics: GridMetrics,
    font_family: String,
    font_size: Pixels,
    theme: Theme,
    config_path: PathBuf,
    status: Option<String>,
    selection: Option<Selection>,
    selected_text: Option<String>,
    selecting: bool,
    scrollbar_dragging: bool,
    scrollbar_drag_offset: Pixels,
    scrollbar_hovering: bool,
    scrollbar_active_until: Option<Instant>,
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
            let injected_at = self
                .scroll_benchmark
                .as_mut()
                .and_then(|benchmark| {
                    benchmark.take_injection(viewport.bottom_offset)
                })
                .unwrap_or_else(Instant::now);
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
        if changed {
            self.activate_scrollbar();
            self.start_snapshot_if_needed(cx);
            cx.notify();
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
        if event.position.x >= window.viewport_size().width - SCROLLBAR_WIDTH
            && self.scroll.history() > 0
        {
            let geometry = self.scrollbar_geometry(window);
            if geometry.is_some_and(|geometry| {
                geometry.contains(f32::from(event.position.y))
            }) {
                let geometry = geometry.expect("checked above");
                self.scrollbar_dragging = true;
                self.scrollbar_drag_offset =
                    event.position.y - px(geometry.thumb_start);
                self.activate_scrollbar();
            } else if let Some(geometry) = geometry {
                let upward = f32::from(event.position.y) < geometry.thumb_start;
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
        let point = point_for_position(event.position, snapshot, self.metrics);
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
        let was_hovering = self.scrollbar_hovering;
        self.scrollbar_hovering =
            event.position.x >= window.viewport_size().width - SCROLLBAR_WIDTH;
        if self.scrollbar_hovering != was_hovering {
            cx.notify();
        }
        if self.scrollbar_dragging {
            self.scrollbar_seek(
                event.position.y - self.scrollbar_drag_offset,
                window,
                cx,
            );
            return;
        }
        if !self.selecting {
            return;
        }
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        if let Some(selection) = &mut self.selection {
            selection.head =
                point_for_position(event.position, snapshot, self.metrics);
            self.selected_text = None;
            self.update_renderer_selection();
            let direction = edge_scroll_direction(
                f32::from(event.position.y),
                f32::from(window.viewport_size().height),
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
        self.scrollbar_dragging = false;
        self.selection_edge_direction = 0;
        if !self.selecting {
            return;
        }
        self.selecting = false;
        let Some(selection) = self.selection else {
            return;
        };
        let range = selection.range();
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
                match result {
                    Ok(Some(text))
                        if view.selection.is_some_and(|current| {
                            current.generation == selection.generation
                                && current.range() == range
                        }) =>
                    {
                        view.selected_text = Some(text);
                    }
                    Ok(_) => view.clear_selection(),
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
            f32::from(window.viewport_size().height),
            self.last_grid_size.rows,
            self.scroll.history(),
            self.scroll.displayed(),
        )
    }
    fn clear_selection(&mut self) {
        self.selection = None;
        self.selected_text = None;
        self.selecting = false;
        self.selection_edge_direction = 0;
        self.update_renderer_selection();
    }
    fn activate_scrollbar(&mut self) {
        self.scrollbar_active_until =
            Some(Instant::now() + Duration::from_millis(750));
    }
    fn update_renderer_selection(&self) {
        self.renderer
            .borrow_mut()
            .set_selection(self.selection.map(Selection::range));
    }

    fn resize_if_needed(&mut self, window: &Window) {
        let viewport = window.viewport_size();
        let size = GridSize::clamped(
            cell_count(viewport.width, self.metrics.cell_width),
            cell_count(viewport.height, self.metrics.cell_height),
        );
        if size == self.last_grid_size {
            return;
        }
        self.last_grid_size = size;
        let cell = CellSize {
            width: pixel_count(self.metrics.cell_width),
            height: pixel_count(self.metrics.cell_height),
        };
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
                benchmark_environment("HUTERM_SCROLL_DISPLAY_SCALE"),
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
        reason = "the terminal canvas and its overlays share one render tree"
    )]
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        self.resize_if_needed(window);
        if self.renderer.borrow().records_stats() {
            window.request_animation_frame();
        }
        let scrollbar_active = self.scrollbar_dragging
            || self.scrollbar_hovering
            || self
                .scrollbar_active_until
                .is_some_and(|deadline| deadline > Instant::now());
        if scrollbar_active {
            window.request_animation_frame();
        }
        let snapshot = self.snapshot.clone();
        let status = self.status.clone();
        let prepare_renderer = Rc::clone(&self.renderer);
        let paint_renderer = Rc::clone(&self.renderer);
        let mut root = div()
            .key_context("Huterm")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::about))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::settings))
            .on_action(cx.listener(Self::toggle_fullscreen))
            .on_action(cx.listener(Self::minimize))
            .on_action(cx.listener(Self::zoom))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::scroll_to_bottom))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .size_full()
            .bg(color(self.theme.background))
            .text_size(self.font_size)
            .font_family(self.font_family.clone())
            .child(canvas(
                move |_, window, _| {
                    prepare_renderer
                        .borrow_mut()
                        .prepare(snapshot.as_ref(), window);
                },
                move |bounds, (), window, _| {
                    paint_renderer.borrow_mut().paint(bounds, window);
                },
            ));
        if self.scroll.history() > 0 {
            let geometry = self
                .scrollbar_geometry(window)
                .expect("history should produce scrollbar geometry");
            root = root
                .child(
                    div()
                        .absolute()
                        .right(px(2.0))
                        .top(px(geometry.thumb_start))
                        .w(px(6.0))
                        .h(px(geometry.thumb_size))
                        .rounded(px(3.0))
                        .bg(rgba(if scrollbar_active {
                            0xffff_ffbb
                        } else {
                            0xffff_ff66
                        })),
                )
                .child(
                    div()
                        .absolute()
                        .right(px(12.0))
                        .bottom(px(4.0))
                        .px_2()
                        .py_1()
                        .rounded(px(3.0))
                        .bg(rgba(0x0000_0099))
                        .text_color(rgba(0xffff_ffcc))
                        .child(
                            if self.scroll.desired() == self.scroll.displayed()
                            {
                                format!("{} lines up", self.scroll.displayed())
                            } else {
                                format!(
                                    "{} requested, {} shown",
                                    self.scroll.desired(),
                                    self.scroll.displayed()
                                )
                            },
                        ),
                );
        }
        root.when_some(status, |view, status| {
            view.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .px_2()
                    .py_1()
                    .bg(rgba(0x0000_00cc))
                    .text_color(rgba(0xffff_ffcc))
                    .child(status),
            )
        })
    }
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
            width: pixel_count(metrics.cell_width),
            height: pixel_count(metrics.cell_height),
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
    (cfg!(target_os = "macos")
        && modifiers.platform
        && (matches!(key, "c" | "v" | "," | "q" | "m")
            || (modifiers.control && key == "f")))
        || (!cfg!(target_os = "macos")
            && modifiers.control
            && modifiers.shift
            && matches!(key, "c" | "v"))
        || (modifiers.shift && matches!(key, "pageup" | "pagedown" | "end"))
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
        let plain_control = GpuiModifiers {
            control: true,
            ..GpuiModifiers::default()
        };
        assert!(!reserved_chord(plain_control, "c"));

        let scroll = GpuiModifiers {
            shift: true,
            ..GpuiModifiers::default()
        };
        assert!(reserved_chord(scroll, "pageup"));

        let clipboard = if cfg!(target_os = "macos") {
            GpuiModifiers {
                platform: true,
                ..GpuiModifiers::default()
            }
        } else {
            GpuiModifiers {
                control: true,
                shift: true,
                ..GpuiModifiers::default()
            }
        };
        assert!(reserved_chord(clipboard, "c"));
        assert!(reserved_chord(clipboard, "v"));
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
}
