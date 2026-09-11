use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    App, Application, Bounds, ClipboardItem, Context, DispatchPhase,
    FocusHandle, Focusable, KeyContext, Keystroke, Menu, MenuItem,
    Modifiers as GpuiModifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, PromptLevel, Render, ScrollDelta, ScrollWheelEvent,
    Subscription, SystemMenuType, TitlebarOptions, Window, WindowBounds,
    WindowControlArea, WindowOptions, canvas, div, point, prelude::*, px, size,
};
use huterm_core::{Mux, RuntimeClient, RuntimeError};
use huterm_protocol::{
    BufferPoint, BufferRange, CellSize, CommandError, CommandInvocation,
    CommandOutcome, CommandValue, GridSize, Modifiers, TabId, TerminalCommand,
    TerminalEvent, TerminalInput, TerminalSnapshot, ids,
};

use crate::APP_ID;
use crate::commands::{
    InvokeApp, InvokePalette, InvokeTerminal, InvokeWindow, invoke,
};
use crate::config::{self, Config, LinkModifiersExt, Theme, WindowConfig};
#[cfg(test)]
use crate::input_queue::buffered_input_bytes;
use crate::input_queue::{
    Admission, InputQueue, PENDING_INPUT_BYTE_CAPACITY, PENDING_INPUT_CAPACITY,
};
use crate::keymap::{
    self, CompiledKeymap, InstalledKeymap, Platform, ReservedKeys,
};
use crate::mouse::{MouseState, application_route};
use crate::renderer::{GridMetrics, TerminalRenderer, rgb_color as color};
use crate::scroll::{
    IndicatorVisibility, ScrollController, ScrollbarExpansion,
    ScrollbarGeometry,
};
use huterm_protocol::{
    MouseAction, MouseButton as ProtocolMouseButton, MouseInput, MousePosition,
};

const INITIAL_COLUMNS: u16 = 100;
const INITIAL_ROWS: u16 = 32;
const SCROLLBAR_WIDTH: Pixels = px(12.0);
const SCROLLBAR_EXPANDED_WIDTH: Pixels = px(18.0);
const TITLEBAR_HEIGHT: Pixels = px(32.0);

mod composition;
mod keyboard;
mod links;
pub(crate) mod palette;
pub(crate) use windows::{
    fullscreen_smoke, integration_smoke, palette_smoke, quake_smoke,
};
#[cfg(target_os = "macos")]
pub(crate) mod menus_smoke;
mod windows;
#[cfg(target_os = "macos")]
pub(crate) use windows::input_smoke;
#[cfg(all(target_os = "macos", feature = "macos-updater"))]
pub(crate) use windows::updater_smoke;

pub(crate) fn run() -> anyhow::Result<()> {
    windows::run()
}

/// Compiles the platform defaults plus `config`'s entries, falling back to
/// defaults alone (with the diagnostic) when the user entries fail.
fn compile_keymap(config: &Config) -> (CompiledKeymap, Option<String>) {
    let platform = Platform::current();
    match keymap::compile(platform, &config.keybindings) {
        Ok(compiled) => (compiled, None),
        Err(error) => {
            (keymap::compile_defaults(platform), Some(error.to_string()))
        }
    }
}

/// Replaces command, component, menu, and discovery bindings together.
fn bind_keymap(cx: &mut App, compiled: CompiledKeymap) -> InstalledKeymap {
    let (bindings, installed) = compiled.install_parts();
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
    install_menus(cx);
    installed
}

fn install_menus(cx: &mut App) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let item = |id| invoke(id).menu_item();
    let mut application_items = vec![item(ids::ABOUT)];
    #[cfg(all(target_os = "macos", feature = "macos-updater"))]
    application_items.push(item(ids::CHECK_FOR_UPDATES));
    application_items.extend([
        item(ids::OPEN_SETTINGS),
        item(ids::RELOAD_CONFIG),
        MenuItem::os_submenu("Services", SystemMenuType::Services),
        MenuItem::separator(),
        item(ids::HIDE),
        item(ids::HIDE_OTHERS),
        item(ids::SHOW_ALL),
        MenuItem::separator(),
        item(ids::QUIT),
    ]);
    cx.set_menus(vec![
        Menu {
            name: "Huterm".into(),
            items: application_items,
        },
        Menu {
            name: "File".into(),
            items: vec![
                item(ids::NEW_WINDOW),
                item(ids::NEW_TAB),
                item(ids::CLOSE_TAB),
                item(ids::CLOSE_WINDOW),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![item(ids::COPY), item(ids::PASTE)],
        },
        Menu {
            name: "View".into(),
            items: vec![
                item(ids::OPEN_COMMAND_PALETTE),
                MenuItem::separator(),
                item(ids::SCROLL_PAGE_UP),
                item(ids::SCROLL_PAGE_DOWN),
                item(ids::SCROLL_TO_BOTTOM),
                MenuItem::separator(),
                item(ids::TOGGLE_FULLSCREEN),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                item(ids::MINIMIZE),
                item(ids::ZOOM),
                item(ids::NEXT_TAB),
                item(ids::PREVIOUS_TAB),
                item(ids::SELECT_RECENT_TAB),
                item(ids::SELECT_TAB),
                item(ids::RENAME_TAB),
            ],
        },
    ]);
    #[cfg(target_os = "macos")]
    if let Err(error) = crate::native_quit::normalize_menu_key_equivalents() {
        eprintln!("Menu shortcut normalization failed: {error:#}");
    }
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

fn copy_availability(selection: Option<Selection>) -> Result<(), CommandError> {
    if selection.and_then(Selection::range).is_none() {
        Err(CommandError::Unavailable("no selection".to_owned()))
    } else {
        Ok(())
    }
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "terminal visibility, lifecycle, and pointer states are independent"
)]
struct TerminalView {
    client: RuntimeClient,
    title: String,
    exited: bool,
    visible: bool,
    input_queue: InputQueue,
    option_as_alt: config::MacosOptionAsAlt,
    composition: composition::Composition,
    #[cfg(target_os = "macos")]
    option_composition: crate::native_quit::OptionComposition,
    #[cfg(target_os = "macos")]
    pending_shortcuts: keyboard::PendingShortcuts,
    #[cfg(target_os = "macos")]
    native_window: gpui::AnyWindowHandle,
    mouse: MouseState,
    external_drag: bool,
    links: links::Links,
    links_enabled: bool,
    link_modifiers: config::LinkModifiers,
    link_diagnostic_at: Option<Instant>,
    open_link: fn(&str, &mut App),
    link_requests: u64,
    link_completions: u64,
    link_max_lookup: Duration,
    link_max_latency: Duration,
    pending_resize: Option<(GridSize, CellSize)>,
    resize_requests: u64,
    snapshot: Option<Arc<TerminalSnapshot>>,
    renderer: Rc<RefCell<TerminalRenderer>>,
    focus: FocusHandle,
    _focus_subscriptions: Vec<Subscription>,
    scroll: ScrollController,
    last_grid_size: GridSize,
    last_cell_size: Option<CellSize>,
    metrics: GridMetrics,
    font_family: String,
    font_size: Pixels,
    window_config: WindowConfig,
    sidebar_width: Pixels,
    tab_presentation: windows::tab_visibility::Presentation,
    tab_overlay: Option<Bounds<Pixels>>,
    chrome_hidden: bool,
    fullscreen_insets: gpui::Edges<Pixels>,
    theme: Theme,
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

impl TerminalView {
    #[allow(clippy::too_many_lines)]
    fn new(
        client: RuntimeClient,
        config: &Config,
        font_family: String,
        metrics: GridMetrics,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let theme = config.theme.clone();
        let focus_subscription =
            cx.on_focus(&focus, window, |view: &mut TerminalView, _, cx| {
                if view.visible
                    && view.enqueue_input(TerminalInput::Focus(true))
                {
                    cx.notify();
                }
            });
        let blur_subscription =
            cx.on_blur(&focus, window, |view: &mut TerminalView, _, cx| {
                // GPUI cancels pending shortcuts when focus changes. Window
                // deactivation alone does not, so retain replay tracking there.
                #[cfg(target_os = "macos")]
                view.pending_shortcuts.clear();
                view.clear_composition(cx);
                view.blur_mouse(cx);
                if view.enqueue_input(TerminalInput::Focus(false)) {
                    cx.notify();
                }
            });
        let activation_subscription = cx.observe_window_activation(
            window,
            |view: &mut TerminalView, window, cx| {
                if view.visible && !window.is_window_active() {
                    view.clear_composition(cx);
                    view.blur_mouse(cx);
                    cx.notify();
                }
            },
        );
        let pending_input_subscription =
            cx.observe_pending_input(window, Self::pending_input_changed);
        #[cfg(target_os = "macos")]
        let layout_subscription = {
            let view = cx.entity().downgrade();
            cx.on_keyboard_layout_change(move |cx| {
                let _ =
                    view.update(cx, |view, _| view.clear_option_composition());
            })
        };
        TerminalView {
            client,
            input_queue: InputQueue::default(),
            option_as_alt: config.terminal.macos_option_as_alt,
            composition: composition::Composition::default(),
            #[cfg(target_os = "macos")]
            option_composition: crate::native_quit::OptionComposition::default(
            ),
            #[cfg(target_os = "macos")]
            native_window: window.window_handle(),
            #[cfg(target_os = "macos")]
            pending_shortcuts: keyboard::PendingShortcuts::default(),
            mouse: MouseState::default(),
            external_drag: false,
            links: links::Links::default(),
            links_enabled: config.terminal.links,
            link_modifiers: config.terminal.link_modifiers,
            link_diagnostic_at: None,
            open_link: |destination, cx| cx.open_url(destination),
            link_requests: 0,
            link_completions: 0,
            link_max_lookup: Duration::ZERO,
            link_max_latency: Duration::ZERO,
            pending_resize: None,
            resize_requests: 0,
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
                activation_subscription,
                pending_input_subscription,
                #[cfg(target_os = "macos")]
                layout_subscription,
            ],
            scroll: ScrollController::default(),
            last_grid_size: GridSize::clamped(INITIAL_COLUMNS, INITIAL_ROWS),
            metrics,
            font_family,
            last_cell_size: None,
            font_size: metrics.font_size,
            window_config: config.window,
            sidebar_width: windows::SIDEBAR_WIDTH,
            tab_presentation: windows::tab_visibility::Presentation::Hidden,
            tab_overlay: None,
            chrome_hidden: false,
            fullscreen_insets: gpui::Edges::default(),
            theme,
            status: None,
            title: String::new(),
            exited: false,
            visible: false,
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
            scroll_benchmark: ScrollBenchmark::from_environment(
                window.scale_factor(),
            ),
            snapshot_sequence: 0,
        }
    }

    fn start_initial_snapshot(&mut self, cx: &mut Context<'_, Self>) {
        if std::env::var_os("HUTERM_SCROLL_BENCH").is_some()
            || std::env::var_os("HUTERM_RENDER_BENCH").is_some()
        {
            eprintln!(
                "huterm-engine engine={} revision={} snapshot=shared-rows native_optimize=ReleaseFast compression=disabled",
                self.client.engine().name(),
                self.client.engine_revision()
            );
        }
        self.scroll.invalidate();
        self.start_snapshot_if_needed(cx);
    }

    fn start_snapshot_if_needed(&mut self, cx: &mut Context<'_, Self>) {
        if self.scroll.displayed() > 0 || self.scroll.desired() > 0 {
            self.cancel_mouse();
        }
        let Some(viewport) =
            begin_visible_snapshot(&mut self.scroll, self.visible)
        else {
            return;
        };
        let link_intent = self.links.intent();
        let link_started = Instant::now();
        self.link_requests += u64::from(link_intent.is_some());
        let requested = self.client.request_snapshot_with_link(
            self.scroll.submitted_scroll(),
            link_intent.map(|intent| intent.point),
        );
        let request = match requested {
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
                        if link_intent.is_some() {
                            view.link_completions += 1;
                            view.link_max_lookup = view.link_max_lookup.max(reply.lookup_duration);
                            view.link_max_latency = view.link_max_latency.max(link_started.elapsed());
                        }
                        view.renderer.borrow_mut().complete_scroll_snapshot(
                            reply.snapshot_duration,
                            reply.requested_viewport.bottom_offset,
                            reply.snapshot.viewport.bottom_offset,
                            reply.completed_at.elapsed(),
                        );
                        if matches!(reply.link, Some(huterm_protocol::LinkLookup::Unavailable | huterm_protocol::LinkLookup::ScanLimit))
                            && view.link_diagnostic_at.is_none_or(|previous| previous.elapsed() >= Duration::from_secs(5)) {
                            view.link_diagnostic_at = Some(Instant::now());
                            eprintln!("Link lookup unavailable or exceeded its bounded scan; terminal remains usable");
                        }
                        view.links.publish(link_intent, reply.link);
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
        if self.mouse.observe_modes(snapshot.modes) {
            self.input_queue.cancel_motion();
            self.scroll.reset_wheel();
        }
        if snapshot.viewport.bottom_offset > 0 || self.scroll.desired() > 0 {
            self.cancel_mouse();
        }
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
        if self.scroll.displayed() > 0 || self.scroll.desired() > 0 {
            self.cancel_mouse();
        }
        let mut changed = false;
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
        for _ in 0..64 {
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
                Ok(Some(TerminalEvent::TitleChanged { title, .. })) => {
                    self.title = title;
                    changed = true;
                }
                Ok(Some(TerminalEvent::Exited { status, .. })) => {
                    self.exited = true;
                    self.clear_composition(cx);
                    self.input_queue.close();
                    self.mouse = MouseState::default();
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
        changed |= self.retry_client_messages();
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

    fn pending_input_changed(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        #[cfg(not(target_os = "macos"))]
        let _ = cx;
        if self.visible {
            #[cfg(target_os = "macos")]
            {
                self.pending_shortcuts
                    .update(window.pending_input_keystrokes(), Vec::new());
                if !window.has_pending_keystrokes() {
                    // Timeout announces None after resolving the current context,
                    // before replaying actions. Mismatch announces it afterward.
                    self.capture_shortcut_resolution(window, cx);
                }
            }
            if window.has_pending_keystrokes() {
                self.clear_option_composition();
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn capture_shortcut_resolution(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        // Menu/programmatic actions can run while GPUI still holds the sequence.
        // Keyboard resolution takes it before dispatching any replay action.
        if window.has_pending_keystrokes() {
            return;
        }
        let Some(strokes) = self.pending_shortcuts.resolution_strokes() else {
            return;
        };
        let mut bindings = Vec::<gpui::KeyBinding>::new();
        // GPUI resolves every replay action before invoking any handler. Freeze
        // the current eligibility once, before a handler can change it.
        for start in 0..strokes.len() {
            for end in start + 1..=strokes.len() {
                for candidate in cx.all_bindings_for_input(&strokes[start..end])
                {
                    if bindings.iter().any(|binding| {
                        binding.action().partial_eq(candidate.action())
                    }) {
                        continue;
                    }
                    bindings.extend(window.bindings_for_action_in(
                        candidate.action(),
                        &self.focus,
                    ));
                }
            }
        }
        self.pending_shortcuts.capture_resolution(bindings);
    }

    #[cfg_attr(
        not(target_os = "macos"),
        expect(
            clippy::unused_self,
            reason = "shared lifecycle callbacks cancel macOS-only composition"
        )
    )]
    fn clear_option_composition(&mut self) {
        #[cfg(target_os = "macos")]
        self.option_composition.clear();
    }

    fn handle_keystroke(
        &mut self,
        keystroke: &Keystroke,
        reserved: &ReservedKeys,
        window: &Window,
    ) -> bool {
        if self.exited
            || keystroke.modifiers.platform
            || reserved.is_reserved(keystroke)
        {
            self.clear_option_composition();
            return false;
        }
        #[cfg(target_os = "macos")]
        if let Err(error) = self.option_composition.refresh_source() {
            self.clear_option_composition();
            self.set_status(format!("Option text input failed: {error}"));
        }
        #[cfg(target_os = "macos")]
        if self.option_composition.is_pending()
            && matches!(keystroke.key.as_str(), "escape" | "backspace")
            && keystroke.modifiers == gpui::Modifiers::default()
        {
            self.clear_option_composition();
            return true;
        }
        let input = keyboard::translate(
            keystroke,
            Platform::current(),
            self.option_as_alt,
        );
        #[cfg(target_os = "macos")]
        let input = {
            if input.is_some()
                || self.option_as_alt != config::MacosOptionAsAlt::Off
            {
                self.clear_option_composition();
                input
            } else if self.composition.is_empty()
                && keystroke.key_char.is_some()
                && (keystroke.modifiers.alt
                    || self.option_composition.is_pending())
            {
                match self.option_composition.translate_current(window) {
                    Ok(Some(text)) if text.is_empty() => return true,
                    Ok(Some(text)) => Some(TerminalInput::Text(text)),
                    Ok(None) => None,
                    Err(error) => {
                        self.clear_option_composition();
                        self.set_status(format!(
                            "Option text input failed: {error}"
                        ));
                        return true;
                    }
                }
            } else {
                self.clear_option_composition();
                input
            }
        };
        #[cfg(not(target_os = "macos"))]
        let _ = window;
        let Some(input) = input else {
            return false;
        };
        self.enqueue_input(input);
        self.scroll.bottom();
        self.scroll.invalidate();
        // Recognized chords stay consumed even when the bounded queue rejects
        // them. Falling through would send Option text through AppKit instead.
        true
    }

    fn scroll(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.visible
            || self
                .tab_overlay
                .is_some_and(|bounds| bounds.contains(&event.position))
        {
            return;
        }
        self.links.invalidate();
        let (application, cell) =
            self.application_mouse(event.position, event.modifiers, window);
        if self.mouse.wheel_route(application) {
            self.scroll.reset_wheel();
            self.input_queue.boundary();
        }
        if application {
            let delta = match event.delta {
                ScrollDelta::Pixels(delta) => {
                    let width = f64::from(f32::from(self.metrics.cell_width));
                    let height = f64::from(f32::from(self.metrics.cell_height));
                    if !width.is_finite()
                        || !height.is_finite()
                        || width <= 0.0
                        || height <= 0.0
                    {
                        return;
                    }
                    (
                        f64::from(f32::from(delta.x)) / width,
                        f64::from(f32::from(delta.y)) / height,
                    )
                }
                ScrollDelta::Lines(delta) => {
                    (f64::from(delta.x), f64::from(delta.y))
                }
            };
            self.input_queue.boundary();
            for direction in self.mouse.wheel(delta.0, delta.1) {
                self.admit_input(
                    TerminalInput::Mouse(MouseInput {
                        position: cell,
                        action: MouseAction::Wheel(direction),
                        modifiers: protocol_modifiers(event.modifiers),
                    }),
                    false,
                    true,
                );
            }
            return;
        }
        let changed = local_scroll(
            &mut self.scroll,
            event,
            f32::from(self.metrics.cell_height),
        );
        if self.scroll.history() > 0 {
            self.activate_scrollbar();
            cx.notify();
        }
        if changed {
            self.start_snapshot_if_needed(cx);
        }
    }

    fn invoke_terminal(
        &mut self,
        action: &InvokeTerminal,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        cx.stop_propagation();
        if let Err(error) = self.run_command(&action.0, window, cx)
            && self.set_status(error.to_string())
        {
            cx.notify();
        }
    }

    /// Runs a terminal-scope catalog command against this view.
    ///
    /// # Errors
    /// Reports refused commands as [`CommandError::Unavailable`] and commands
    /// this view does not own as [`CommandError::UnknownCommand`].
    fn command_availability(
        &self,
        command: huterm_protocol::CommandId,
    ) -> Result<(), CommandError> {
        match command {
            ids::COPY => copy_availability(self.selection),
            ids::PASTE if self.exited => {
                Err(CommandError::Unavailable("terminal has exited".to_owned()))
            }
            ids::PASTE
            | ids::SCROLL_PAGE_UP
            | ids::SCROLL_PAGE_DOWN
            | ids::SCROLL_TO_BOTTOM => Ok(()),
            other => Err(CommandError::UnknownCommand(other)),
        }
    }

    fn run_command(
        &mut self,
        invocation: &CommandInvocation,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.clear_option_composition();
        self.command_availability(invocation.id)?;
        match invocation.id {
            ids::COPY => {
                if let Some(text) = &self.selected_text {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        text.clone(),
                    ));
                }
            }
            ids::PASTE => {
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
            ids::SCROLL_PAGE_UP => {
                self.scroll_command(cx, |scroll, rows| scroll.page(rows, true));
            }
            ids::SCROLL_PAGE_DOWN => {
                self.scroll_command(cx, |scroll, rows| {
                    scroll.page(rows, false)
                });
            }
            ids::SCROLL_TO_BOTTOM => {
                self.scroll_command(cx, |scroll, _| scroll.bottom());
            }
            other => return Err(CommandError::UnknownCommand(other)),
        }
        Ok(CommandOutcome::Completed)
    }

    fn scroll_command(
        &mut self,
        cx: &mut Context<'_, Self>,
        apply: impl FnOnce(&mut ScrollController, u16) -> bool,
    ) {
        self.links.invalidate();
        if apply(&mut self.scroll, self.last_grid_size.rows) {
            self.activate_scrollbar();
            self.start_snapshot_if_needed(cx);
            cx.notify();
        }
    }

    fn link_cell(
        &self,
        position: gpui::Point<Pixels>,
        window: &Window,
    ) -> Option<MousePosition> {
        let (inside, cell) = application_mouse_geometry(
            position,
            self.content_bounds(window).origin,
            self.terminal_layout(window),
            size(self.metrics.cell_width, self.metrics.cell_height),
        );
        inside.then_some(cell)
    }

    fn effective_link_modifiers(
        &self,
        modifiers: GpuiModifiers,
        window: &Window,
    ) -> bool {
        self.links_enabled
            && self.visible
            && window.is_window_active()
            && self.focus.is_focused(window)
            && self.link_modifiers.matches(
                modifiers,
                !self.exited
                    && self.snapshot.as_ref().is_some_and(|snapshot| {
                        snapshot.modes.mouse_tracking
                            != huterm_protocol::MouseTracking::Disabled
                    }),
            )
    }

    fn update_link_pointer(
        &mut self,
        position: gpui::Point<Pixels>,
        modifiers: GpuiModifiers,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let point = self.link_cell(position, window);
        let enabled = self.effective_link_modifiers(modifiers, window)
            && !self
                .tab_overlay
                .is_some_and(|bounds| bounds.contains(&position))
            && !self.external_drag
            && !self.selecting
            && !self.scrollbar_dragging
            && !self.mouse.held(ProtocolMouseButton::Left);
        let was_hovered = self.links.hover().is_some();
        if self.links.update(
            point,
            enabled,
            (f32::from(position.x), f32::from(position.y)),
        ) {
            self.scroll.invalidate();
            self.start_snapshot_if_needed(cx);
        }
        if was_hovered != self.links.hover().is_some() {
            cx.notify();
        }
    }

    fn can_drop_paths(&self, window: &Window) -> bool {
        !self
            .tab_overlay
            .is_some_and(|bounds| bounds.contains(&window.mouse_position()))
            && self.visible
            && !self.exited
            && self.focus.is_focused(window)
            && self
                .content_bounds(window)
                .contains(&window.mouse_position())
    }

    fn drop_paths(
        &mut self,
        paths: &gpui::ExternalPaths,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.external_drag = false;
        self.blur_mouse(cx);
        if !self.can_drop_paths(window) {
            return;
        }
        match crate::file_drop::format_paths(
            paths.paths(),
            PENDING_INPUT_BYTE_CAPACITY,
        ) {
            Ok(text) => {
                let (accepted, _) =
                    self.admit_input(TerminalInput::Paste(text), false, false);
                if accepted {
                    self.focus.focus(window);
                    self.scroll.bottom();
                    self.scroll.invalidate();
                    self.start_snapshot_if_needed(cx);
                }
            }
            Err(error) => {
                self.set_status(error.to_owned());
            }
        }
        cx.notify();
    }

    fn application_mouse(
        &self,
        position: gpui::Point<Pixels>,
        modifiers: GpuiModifiers,
        window: &Window,
    ) -> (bool, MousePosition) {
        let (in_grid, cell) = application_mouse_geometry(
            position,
            self.content_bounds(window).origin,
            self.terminal_layout(window),
            size(self.metrics.cell_width, self.metrics.cell_height),
        );
        let in_grid = in_grid
            && window.is_window_active()
            && self.focus.is_focused(window);
        let position = position - self.content_bounds(window).origin;
        let route = self.snapshot.as_ref().is_some_and(|snapshot| {
            application_route(
                in_grid,
                self.scrollbar_at(position, window).is_some(),
                modifiers.shift,
                (!self.exited).then_some(snapshot.modes.mouse_tracking),
                self.scroll.displayed(),
                self.scroll.desired(),
            )
        });
        (route, cell)
    }

    fn mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.visible
            || self
                .tab_overlay
                .is_some_and(|bounds| bounds.contains(&event.position))
        {
            return;
        }
        self.external_drag = false;
        self.focus.focus(window);
        let Some(button) = protocol_mouse_button(event.button) else {
            return;
        };
        let (application, cell) =
            self.application_mouse(event.position, event.modifiers, window);
        if self.mouse.held(button) {
            return;
        }
        self.update_link_pointer(event.position, event.modifiers, window, cx);
        if event.button == MouseButton::Left
            && self.link_cell(event.position, window).is_some()
            && self
                .scrollbar_at(
                    event.position - self.content_bounds(window).origin,
                    window,
                )
                .is_none()
            && self.links.press(
                cell,
                (f32::from(event.position.x), f32::from(event.position.y)),
            )
        {
            self.clear_selection();
            cx.stop_propagation();
            cx.notify();
            return;
        }
        self.input_queue.boundary();
        if self.mouse.down(button, application) {
            let (accepted, _) = self.admit_input(
                TerminalInput::Mouse(MouseInput {
                    position: cell,
                    action: MouseAction::Press(button),
                    modifiers: protocol_modifiers(event.modifiers),
                }),
                false,
                false,
            );
            if accepted {
                self.mouse.accepted(button, cell);
            }
            cx.notify();
            return;
        }
        if application || event.button != MouseButton::Left {
            return;
        }
        let position = event.position - self.content_bounds(window).origin;
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
        if !self.visible
            || overlay_blocks_pointer(
                self.tab_overlay,
                event.position,
                self.owns_pointer_gesture(),
            )
        {
            return;
        }
        if self.external_drag && cx.has_active_drag() {
            return;
        }
        self.external_drag = false;
        self.update_link_pointer(event.position, event.modifiers, window, cx);
        if self.links.owns_press() {
            cx.notify();
            return;
        }
        let position = event.position - self.content_bounds(window).origin;
        if self.scroll.displayed() > 0 || self.scroll.desired() > 0 {
            self.cancel_mouse();
        }
        let (application, cell) =
            self.application_mouse(event.position, event.modifiers, window);
        if let Some(motion) = self.mouse.motion(
            cell,
            protocol_modifiers(event.modifiers),
            application,
        ) {
            self.admit_input(TerminalInput::Mouse(motion), false, true);
        }
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
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.visible
            || overlay_blocks_pointer(
                self.tab_overlay,
                event.position,
                self.owns_pointer_gesture(),
            )
        {
            return;
        }
        if self.external_drag {
            self.external_drag = false;
            self.cancel_mouse();
            return;
        }
        // AppKit can remap a Control-left release to Right and erase Control.
        // A separately held Right button still owns its own release.
        let remapped_link_release = cfg!(target_os = "macos")
            && event.button == MouseButton::Right
            && !self.mouse.held(ProtocolMouseButton::Right);
        if self.links.owns_press()
            && (event.button == MouseButton::Left || remapped_link_release)
        {
            self.update_link_pointer(
                event.position,
                event.modifiers,
                window,
                cx,
            );
            let point = self.link_cell(event.position, window);
            let enabled = !remapped_link_release
                && self.effective_link_modifiers(event.modifiers, window);
            let (_, destination) = self.links.release(point, enabled);
            if let Some(destination) = destination {
                (self.open_link)(&destination, cx);
            }
            self.scroll.invalidate();
            self.start_snapshot_if_needed(cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let Some(button) = protocol_mouse_button(event.button) else {
            return;
        };
        let button = self
            .mouse
            .release_button(button, cfg!(target_os = "macos"))
            .unwrap_or(button);
        let (_, cell) =
            self.application_mouse(event.position, event.modifiers, window);
        self.input_queue.boundary();
        if let Some(release) = self.mouse.release(
            button,
            cell,
            protocol_modifiers(event.modifiers),
        ) {
            self.admit_input(TerminalInput::Mouse(release), true, false);
            cx.notify();
        }
        if button != ProtocolMouseButton::Left {
            return;
        }
        if self.scrollbar_dragging {
            self.scrollbar_dragging = false;
            self.activate_scrollbar();
            cx.notify();
        }
        self.finish_selection(cx);
    }

    fn finish_selection(&mut self, cx: &mut Context<'_, Self>) {
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
            f32::from(self.viewport(window).height),
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
            self.viewport(window).width,
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

    fn owns_pointer_gesture(&self) -> bool {
        self.selecting
            || self.scrollbar_dragging
            || self.external_drag
            || self.links.owns_press()
            || [
                ProtocolMouseButton::Left,
                ProtocolMouseButton::Middle,
                ProtocolMouseButton::Right,
            ]
            .into_iter()
            .any(|button| self.mouse.held(button))
    }

    fn content_bounds(&self, window: &Window) -> Bounds<Pixels> {
        windows::ChromeLayout::with_safe_area(
            window.viewport_size(),
            terminal_top(self.chrome_hidden),
            self.window_config.tab_position,
            self.sidebar_width,
            self.fullscreen_insets,
        )
        .present(self.tab_presentation, self.window_config.tab_position, 0.0)
        .terminal
    }
    fn viewport(&self, window: &Window) -> gpui::Size<Pixels> {
        self.content_bounds(window).size
    }

    fn terminal_layout(&self, window: &Window) -> TerminalLayout {
        TerminalLayout::new(
            self.viewport(window),
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
        let viewport = self.viewport(window);
        if self
            .last_viewport
            .replace(viewport)
            .is_some_and(|previous| previous != viewport)
        {
            self.links.invalidate();
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
        self.links.invalidate();
        self.last_grid_size = size;
        self.last_cell_size = Some(cell);
        self.resize_requests += 1;
        match self.client.resize(size, cell) {
            Ok(()) => self.pending_resize = None,
            Err(RuntimeError::Busy) => self.pending_resize = Some((size, cell)),
            Err(error) => self.status = Some(error.to_string()),
        }
    }

    fn enqueue_input(&mut self, input: TerminalInput) -> bool {
        self.mouse.boundary();
        let (_, changed) = self.admit_input(input, false, false);
        changed
    }

    fn admit_input(
        &mut self,
        input: TerminalInput,
        release: bool,
        quiet: bool,
    ) -> (bool, bool) {
        if self.exited {
            return (false, false);
        }
        match self.input_queue.enqueue(input, release, |input| self.client.send_input(input)) {
            Ok(Admission::Accepted) => (true, false),
            Ok(Admission::Closed) => (false, false),
            Ok(Admission::Full) if quiet => (false, false),
            Ok(Admission::Full) => (false, self.set_status(format!("Input buffer full ({PENDING_INPUT_CAPACITY} events or {PENDING_INPUT_BYTE_CAPACITY} bytes); input rejected"))),
            Err(error) => {
                self.mouse = MouseState::default();
                (false, self.set_status(error.to_string()))
            }
        }
    }

    fn cancel_mouse(&mut self) {
        self.input_queue.cancel_motion();
        for release in self.mouse.cancel() {
            self.admit_input(TerminalInput::Mouse(release), true, false);
        }
    }

    fn hide(&mut self, cx: &mut Context<'_, Self>) {
        self.clear_composition(cx);
        self.blur_mouse(cx);
        self.mouse.forget_released_buttons();
        self.links.forget_press();
        self.scrollbar_hovering = false;
        self.visible = false;
    }

    fn blur_mouse(&mut self, cx: &mut Context<'_, Self>) {
        self.links.disable();
        self.cancel_mouse();
        self.finish_selection(cx);
        self.scrollbar_dragging = false;
        self.selection_edge_direction = 0;
        self.scroll.reset_wheel();
    }

    fn retry_client_messages(&mut self) -> bool {
        if self.exited {
            self.input_queue.close();
        }
        if let Err(error) = self
            .input_queue
            .retry(|input| self.client.send_input(input))
        {
            self.mouse = MouseState::default();
            return self.set_status(error.to_string());
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
    /// Key context for binding predicates: `Terminal`, plus `selection`
    /// while a range is selected and `exited` after the root shell exits.
    fn key_context(&self) -> KeyContext {
        let mut context = KeyContext::default();
        context.add("Terminal");
        // A mouse-down anchor is not a selection until the drag reaches
        // another cell, so a plain click must not enable `selection`.
        if self.selection.and_then(Selection::range).is_some() {
            context.add("selection");
        }
        if self.exited {
            context.add("exited");
        }
        context
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
    display_scale: f32,
}

impl ScrollBenchmark {
    fn from_environment(display_scale: f32) -> Option<Self> {
        std::env::var("HUTERM_SCROLL_BENCH")
            .is_ok_and(|value| {
                value == "1" || value.eq_ignore_ascii_case("true")
            })
            .then(|| Self::new(display_scale))
    }

    fn new(display_scale: f32) -> Self {
        Self {
            step: 0,
            started: false,
            queue_next: false,
            pending_injection: None,
            last_report: Instant::now(),
            display_scale,
        }
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
        let display_scale = self.display_scale;
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

fn overlay_blocks_pointer(
    overlay: Option<Bounds<Pixels>>,
    pointer: gpui::Point<Pixels>,
    owns_gesture: bool,
) -> bool {
    // The release can precede the window refresh that hides an overlay after
    // a terminal press. Existing terminal ownership wins during that interval.
    !owns_gesture && overlay.is_some_and(|bounds| bounds.contains(&pointer))
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
        self.update_link_pointer(
            window.mouse_position(),
            window.modifiers(),
            window,
            cx,
        );
        if let Some(benchmark) = &mut self.scroll_benchmark {
            benchmark.display_scale = window.scale_factor();
        }
        if self.renderer.borrow().records_stats() {
            window.request_animation_frame();
        }
        let snapshot = self.snapshot.clone();
        let status = self.status.clone();
        let prepare_renderer = Rc::clone(&self.renderer);
        let paint_renderer = Rc::clone(&self.renderer);
        let mouse_view = cx.entity().downgrade();
        #[cfg(target_os = "macos")]
        let input_view = cx.entity();
        #[cfg(target_os = "macos")]
        let input_focus = self.focus.clone();
        let layout = self.terminal_layout(window);
        let hovered_link = self.links.hover().cloned();
        let link_metrics = self.metrics;
        let underline = color(self.theme.foreground);
        let paint_link = hovered_link.clone();
        let mut root = div()
            .id("terminal")
            .on_hover(cx.listener(|view, hovering, _, cx| {
                if !hovering && view.scrollbar_hovering {
                    view.scrollbar_hovering = false;
                    view.activate_scrollbar();
                    cx.notify();
                }
            }))
            .key_context(self.key_context())
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::invoke_terminal))
            .capture_key_down(cx.listener(
                |view, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape"
                        && view.links.cancel_press()
                    {
                        cx.stop_propagation();
                        cx.notify();
                    }
                },
            ))
            .on_modifiers_changed(cx.listener(
                |view, event: &gpui::ModifiersChangedEvent, window, cx| {
                    view.update_link_pointer(
                        window.mouse_position(),
                        event.modifiers,
                        window,
                        cx,
                    );
                    cx.notify();
                },
            ))
            .on_drag_move::<gpui::ExternalPaths>(cx.listener(
                |view, _, _, cx| {
                    view.external_drag = true;
                    view.blur_mouse(cx);
                    view.links.forget_press();
                },
            ))
            .can_drop({
                let view = cx.entity().downgrade();
                move |value, window, cx| {
                    value.is::<gpui::ExternalPaths>()
                        && view.upgrade().is_some_and(|view| {
                            view.read(cx).can_drop_paths(window)
                        })
                }
            })
            .on_drop(cx.listener(Self::drop_paths))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::mouse_up))
            .relative()
            .w_full()
            .h(self.viewport(window).height)
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
                    move |bounds, (), window, cx| {
                        let bounds = Bounds::new(
                            bounds.origin + layout.bounds.origin,
                            layout.bounds.size,
                        );
                        #[cfg(target_os = "macos")]
                        window.handle_input(
                            &input_focus,
                            gpui::ElementInputHandler::new(bounds, input_view),
                            cx,
                        );
                        #[cfg(not(target_os = "macos"))]
                        let _ = cx;
                        window.with_content_mask(
                            Some(gpui::ContentMask { bounds }),
                            |window| {
                                paint_renderer
                                    .borrow_mut()
                                    .paint(bounds, window);
                                if let Some(link) = &paint_link {
                                    for cell in &link.cells {
                                        let origin = bounds.origin
                                            + point(
                                                link_metrics.cell_width
                                                    * f32::from(
                                                        u16::try_from(
                                                            cell.position
                                                                .column,
                                                        )
                                                        .unwrap_or(u16::MAX),
                                                    ),
                                                link_metrics.cell_height
                                                    * (f32::from(
                                                        u16::try_from(
                                                            cell.position.row,
                                                        )
                                                        .unwrap_or(u16::MAX),
                                                    ) + 1.0)
                                                    - px(1.0),
                                            );
                                        window.paint_quad(gpui::fill(
                                            Bounds::new(
                                                origin,
                                                size(
                                                    link_metrics.cell_width,
                                                    px(1.0),
                                                ),
                                            ),
                                            underline,
                                        ));
                                    }
                                }
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
        #[cfg(target_os = "macos")]
        {
            root = root
                .capture_action(cx.listener(
                    |view, _: &InvokeApp, window, cx| {
                        view.capture_shortcut_resolution(window, cx);
                    },
                ))
                .capture_action(cx.listener(
                    |view, _: &InvokeWindow, window, cx| {
                        view.capture_shortcut_resolution(window, cx);
                    },
                ))
                .capture_action(cx.listener(
                    |view, _: &InvokeTerminal, window, cx| {
                        view.capture_shortcut_resolution(window, cx);
                    },
                ))
                .on_key_down(cx.listener(
                    |view, event: &gpui::KeyDownEvent, _, cx| {
                        // GPUI sends unmatched chord replays here before dispatch_input,
                        // which otherwise looks identical to a native text commit.
                        if view
                            .pending_shortcuts
                            .consume_replay(&event.keystroke)
                        {
                            cx.stop_propagation();
                        }
                    },
                ));
        }
        if let Some(link) = hovered_link {
            root = root.cursor(gpui::CursorStyle::PointingHand).child(
                div()
                    .absolute()
                    .left(px(4.0))
                    .bottom(px(4.0))
                    .w((self.viewport(window).width - px(8.0)).max(px(0.0)))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .bg(color(self.theme.background))
                    .text_color(color(self.theme.foreground))
                    .child(link.destination),
            );
        }
        let displayed_offset = self.scroll.displayed();
        if self.scrollbar_visibility.opacity > 0.0
            && let Some(geometry) = self.scrollbar_geometry(window)
        {
            let expansion = self.scrollbar_expansion.progress;
            root = root.children(crate::ui::scrollbar::layers(
                geometry,
                self.scrollbar_visibility.opacity,
                expansion,
                color(self.theme.foreground),
            ));
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
        root.when_some(status, |view, status| {
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
        })
    }
}

fn begin_visible_snapshot(
    scroll: &mut ScrollController,
    visible: bool,
) -> Option<huterm_protocol::Viewport> {
    if visible {
        scroll.begin_request()
    } else {
        None
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

fn terminal_top(chrome_hidden: bool) -> Pixels {
    titlebar_inset(cfg!(target_os = "macos"), chrome_hidden)
}

fn shell_command(
    metrics: GridMetrics,
    engine: huterm_protocol::TerminalEngineKind,
) -> anyhow::Result<TerminalCommand> {
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
        engine,
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
fn local_scroll(
    controller: &mut ScrollController,
    event: &ScrollWheelEvent,
    row_height: f32,
) -> bool {
    match event.delta {
        ScrollDelta::Pixels(delta) => {
            controller.scroll_pixels(f32::from(delta.y), row_height)
        }
        ScrollDelta::Lines(delta) => {
            // GPUI's X11 backend moves Shift-wheel vertical lines into x.
            let lines = if cfg!(target_os = "linux")
                && event.modifiers.shift
                && delta.y == 0.0
            {
                delta.x
            } else {
                delta.y
            };
            controller.scroll_lines(lines)
        }
    }
}

fn application_mouse_geometry(
    position: gpui::Point<Pixels>,
    origin: gpui::Point<Pixels>,
    layout: TerminalLayout,
    cell: gpui::Size<Pixels>,
) -> (bool, MousePosition) {
    let relative = position - origin - layout.bounds.origin;
    let inside = relative.x >= px(0.0)
        && relative.y >= px(0.0)
        && relative.x < layout.bounds.size.width
        && relative.y < layout.bounds.size.height;
    (inside, mouse_position(relative, layout.grid, cell))
}

fn protocol_mouse_button(button: MouseButton) -> Option<ProtocolMouseButton> {
    match button {
        MouseButton::Left => Some(ProtocolMouseButton::Left),
        MouseButton::Middle => Some(ProtocolMouseButton::Middle),
        MouseButton::Right => Some(ProtocolMouseButton::Right),
        MouseButton::Navigate(_) => None,
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "finite coordinates clamp to the u16 grid before conversion"
)]
fn mouse_position(
    position: gpui::Point<Pixels>,
    grid: GridSize,
    cell: gpui::Size<Pixels>,
) -> MousePosition {
    let coordinate = |value: Pixels, dimension: Pixels, count: u16| {
        let value = f32::from(value);
        let dimension = f32::from(dimension);
        if !value.is_finite() || !dimension.is_finite() || dimension <= 0.0 {
            return 0;
        }
        (value / dimension)
            .floor()
            .clamp(0.0, f32::from(count.saturating_sub(1))) as u32
    };
    MousePosition {
        column: coordinate(position.x, cell.width, grid.columns),
        row: coordinate(position.y, cell.height, grid.rows),
    }
}

fn protocol_modifiers(modifiers: GpuiModifiers) -> Modifiers {
    Modifiers {
        control: modifiers.control,
        alt: modifiers.alt,
        shift: modifiers.shift,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_gesture_retains_motion_and_release_under_an_overlay() {
        let overlay = Some(Bounds::new(
            point(px(0.0), px(0.0)),
            size(px(800.0), px(32.0)),
        ));
        let on_bar = point(px(100.0), px(12.0));
        assert!(overlay_blocks_pointer(overlay, on_bar, false));
        assert!(!overlay_blocks_pointer(overlay, on_bar, true));
        assert!(!overlay_blocks_pointer(
            overlay,
            point(px(100.0), px(100.0)),
            false
        ));
        assert!(!overlay_blocks_pointer(None, on_bar, false));
    }

    #[test]
    fn benchmark_starts_from_attached_window_scale_without_a_render() {
        let mut benchmark = ScrollBenchmark::new(2.0);
        let mut scroll = ScrollController::default();
        scroll.complete(huterm_protocol::Viewport::default(), 10_000);
        assert!(benchmark.drive(&mut scroll, INITIAL_ROWS, 16.0));
        assert!(benchmark.is_started());
        assert_eq!(scroll.desired(), 1);
        assert!(benchmark.take_injection(1).is_some());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn x11_shift_wheel_uses_remapped_lines_for_local_scrollback() {
        let mut scroll = ScrollController::default();
        scroll.complete(huterm_protocol::Viewport::default(), 100);
        let mut event = ScrollWheelEvent {
            position: point(px(40.0), px(40.0)),
            delta: ScrollDelta::Lines(point(3.0, 0.0)),
            modifiers: GpuiModifiers {
                shift: true,
                ..GpuiModifiers::default()
            },
            touch_phase: gpui::TouchPhase::default(),
        };
        assert!(local_scroll(&mut scroll, &event, 16.0));
        assert_eq!(scroll.desired(), 3);
        event.modifiers.shift = false;
        assert!(!local_scroll(&mut scroll, &event, 16.0));
        assert_eq!(scroll.desired(), 3);
        event.modifiers.shift = true;
        event.delta = ScrollDelta::Lines(point(9.0, -2.0));
        assert!(local_scroll(&mut scroll, &event, 16.0));
        assert_eq!(scroll.desired(), 1);
    }

    #[test]
    fn application_coordinates_exclude_titlebar_padding_and_grid_endpoints() {
        let cell = size(px(8.0), px(16.0));
        let layout = TerminalLayout::new(
            size(px(105.0), px(59.0)),
            cell,
            WindowConfig::default(),
        );
        let geometry = |x, y| {
            application_mouse_geometry(
                point(px(x), px(y)),
                point(px(0.0), px(32.0)),
                layout,
                cell,
            )
        };
        assert_eq!(
            geometry(4.0, 36.0),
            (true, MousePosition { column: 0, row: 0 })
        );
        assert_eq!(
            geometry(99.9, 83.9),
            (true, MousePosition { column: 11, row: 2 })
        );
        for (x, y) in [
            (4.0, 31.0),
            (3.9, 40.0),
            (10.0, 35.9),
            (100.0, 40.0),
            (10.0, 84.0),
            (-100.0, -100.0),
            (1000.0, 1000.0),
        ] {
            assert!(!geometry(x, y).0, "{x} {y}");
        }
        assert_eq!(
            geometry(1000.0, 1000.0).1,
            MousePosition { column: 11, row: 2 }
        );
        assert_eq!(geometry(-100.0, -100.0).1, MousePosition::default());
        for dimension in [0.0, -1.0, f32::INFINITY, f32::NAN] {
            assert_eq!(
                mouse_position(
                    point(px(50.0), px(50.0)),
                    layout.grid,
                    size(px(dimension), px(dimension))
                ),
                MousePosition::default()
            );
        }
    }

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
            display_scale: 1.0,
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
