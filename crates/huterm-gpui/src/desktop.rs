use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ClipboardItem, Context, DispatchPhase, EventEmitter,
    FocusHandle, Focusable, KeyContext, Keystroke, Menu, MenuItem,
    Modifiers as GpuiModifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Render, ScrollDelta, ScrollWheelEvent, Subscription,
    SystemMenuType, Task, TitlebarOptions, Window, WindowBounds,
    WindowControlArea, WindowOptions, canvas, div, point, prelude::*, px, size,
    svg,
};
use huterm_core::{
    HostEffectRecipient, Mux, PresentationController, RuntimeClient,
    RuntimeError,
};
use huterm_protocol::{
    BufferPoint, BufferRange, CellSize, CommandError, CommandInvocation,
    CommandOutcome, CommandValue, GridSize, HostEffect, Modifiers, TabId,
    TerminalCommand, TerminalEvent, TerminalInput, TerminalMetadata,
    TerminalPresentation, TerminalSnapshot, ids,
};

use crate::APP_ID;
use crate::assets::Icon;
use crate::commands::{
    InvokeApp, InvokePalette, InvokeTerminal, InvokeWindow, invoke,
};
use crate::config::{
    self, Config, LinkModifiersExt, TabsConfig, Theme, WindowConfig,
};
#[cfg(test)]
use crate::input_queue::buffered_input_bytes;
use crate::input_queue::{
    Admission, InputQueue, PENDING_INPUT_BYTE_CAPACITY, PENDING_INPUT_CAPACITY,
};
use crate::keymap::{
    self, CompiledKeymap, InstalledKeymap, Platform, ReservedKeys,
};
use crate::mouse::{MouseState, application_route};
use crate::renderer::{
    GridMetrics, TerminalRenderer, rgb_color as color, rgba_color,
};
use crate::scroll::ScrollController;
use crate::ui::animation::AnimationSchedule;
use crate::ui::scrollbar::{
    Axis, Edge, HitBand, INDICATOR_HOLD, IndicatorVisibility, Origin, Press,
    ScrollbarColors, ScrollbarGeometries, ScrollbarGeometry, ScrollbarOptions,
    Scrollbars, ThumbSize, TrackMargins, TrackPress,
};
use huterm_protocol::{
    MouseAction, MouseButton as ProtocolMouseButton, MouseInput, MousePosition,
};
use key_hint::KeyHint;
use notices::Severity;
use overlay::{Swatch, key_cap, mono_font_family, raised_panel};

const INITIAL_COLUMNS: u16 = 100;
const INITIAL_ROWS: u16 = 32;
/// The terminal's overlay scrollbar: history rows counted from the bottom,
/// a paging track, hover expansion, and room below the track for the scroll
/// label.
const TERMINAL_SCROLLBAR: ScrollbarOptions = ScrollbarOptions {
    edge: Edge::Right,
    origin: Origin::End,
    expand_on_hover: true,
    track_press: TrackPress::Page,
    margins: TrackMargins {
        start: 2.0,
        end: 8.0,
        padding: 2.0,
    },
    thumb: ThumbSize::Slim,
    edge_inset: 2.0,
    // Reaches the window edge so a pointer pinned there still grabs it.
    hit: HitBand {
        outward: 2.0,
        inward: 6.0,
    },
    reveal_on_hover: false,
    hold: INDICATOR_HOLD,
};
const TITLEBAR_HEIGHT: Pixels = px(32.0);
/// The `scroll_to_bottom` default on every platform, shown by the scroll
/// pill until the window supplies the compiled keymap's binding.
const DEFAULT_SCROLL_TO_BOTTOM_KEY: &str = "shift-end";
const VISUAL_BELL_DURATION: Duration = Duration::from_millis(150);

mod about;
mod close_dialog;
mod composition;
mod key_bench;
mod key_hint;
mod keyboard;
mod links;
mod menu;
mod notices;
mod overlay;
pub(crate) mod palette;
mod refresh;
pub(crate) use windows::{
    fullscreen_smoke, integration_smoke, palette_smoke,
    presentation_query_smoke, quake_smoke, refresh_smoke,
};
#[cfg(target_os = "macos")]
pub(crate) mod menus_smoke;
mod windows;
#[cfg(all(target_os = "macos", feature = "macos-updater"))]
pub(crate) use windows::updater_smoke;
#[cfg(target_os = "macos")]
pub(crate) use windows::{clipboard_smoke, idle_bench, input_smoke};

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
            disabled: false,
            items: application_items,
        },
        Menu {
            name: "File".into(),
            disabled: false,
            items: vec![
                item(ids::NEW_WINDOW),
                item(ids::NEW_TAB),
                item(ids::CLOSE_TAB),
                item(ids::CLOSE_WINDOW),
            ],
        },
        Menu {
            name: "Edit".into(),
            disabled: false,
            items: vec![
                item(ids::COPY),
                item(ids::PASTE),
                item(ids::SELECT_ALL),
                MenuItem::separator(),
                item(ids::CLEAR_SCROLLBACK),
                item(ids::RESET_TERMINAL),
            ],
        },
        Menu {
            name: "View".into(),
            disabled: false,
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
            disabled: false,
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

/// How long a right-click waits for the link under the pointer before its
/// context menu opens without link rows.
const CONTEXT_LINK_TIMEOUT: Duration = Duration::from_millis(250);

/// A right-click's link lookup, riding on the next snapshot request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ContextLookup {
    epoch: u64,
    cell: huterm_protocol::MousePosition,
    /// Where the menu opens, in window coordinates.
    position: gpui::Point<Pixels>,
}

/// Asks the window to open the terminal's context menu at `position`, in
/// window coordinates, with the destination of the link under it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ContextMenuRequest {
    pub(super) position: gpui::Point<Pixels>,
    pub(super) link: Option<String>,
}

impl EventEmitter<ContextMenuRequest> for TerminalView {}

fn copy_availability(selection: Option<Selection>) -> Result<(), CommandError> {
    if selection.and_then(Selection::range).is_none() {
        Err(CommandError::Unavailable("no selection".to_owned()))
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct BellPresentation {
    unseen: bool,
    flash_until: Option<Instant>,
}

impl BellPresentation {
    fn ring(&mut self, now: Instant, active: bool, enabled: bool) -> bool {
        if !enabled {
            return false;
        }
        let previous = *self;
        if active {
            self.flash_until = Some(now + VISUAL_BELL_DURATION);
        } else {
            self.unseen = true;
        }
        *self != previous
    }

    fn viewed(&mut self, active: bool) -> bool {
        if !active || !self.unseen {
            return false;
        }
        self.unseen = false;
        true
    }

    fn advance(&mut self, now: Instant) -> bool {
        if self.flash_until.is_some_and(|deadline| now >= deadline) {
            self.flash_until = None;
            true
        } else {
            false
        }
    }

    fn flashing(self, now: Instant) -> bool {
        self.flash_until.is_some_and(|deadline| now < deadline)
    }

    fn clear(&mut self) -> bool {
        let changed = self.unseen || self.flash_until.is_some();
        *self = Self::default();
        changed
    }
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "terminal visibility, lifecycle, and pointer states are independent"
)]
struct TerminalView {
    client: RuntimeClient,
    presentation: PresentationController,
    pending_presentation: Option<TerminalPresentation>,
    host_effects: HostEffectRecipient,
    title: String,
    metadata: TerminalMetadata,
    metadata_revision: u64,
    bell: BellPresentation,
    bell_flash_count: u64,
    visual_bell: bool,
    exited: bool,
    /// The root process's exit status, once it has exited with one.
    exit_code: Option<u32>,
    failed: bool,
    visible: bool,
    input_queue: InputQueue,
    pending_work: async_channel::Sender<()>,
    _pending_work_task: Task<()>,
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
    right_click: config::RightClickAction,
    /// A right-click waiting for its link lookup before its menu opens.
    context_lookup: Option<ContextLookup>,
    context_lookup_epoch: u64,
    link_requests: u64,
    link_completions: u64,
    link_max_lookup: Duration,
    link_max_latency: Duration,
    pending_resize: Option<(GridSize, CellSize)>,
    resize_requests: u64,
    /// Times the size panel was raised; smoke state reports it.
    resize_indicators: u64,
    /// Layout changes keep the size panel hidden while a Quake window is
    /// still presenting: showing, settling, or entering fullscreen.
    quiet_resize: bool,
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
    /// The tab configuration with its position resolved for the window;
    /// the workspace keeps it current, so `titlebar` never reaches layout
    /// without a title bar.
    tabs_config: TabsConfig,
    sidebar_width: Pixels,
    tab_presentation: windows::tab_visibility::Presentation,
    tab_overlay: Option<Bounds<Pixels>>,
    chrome_hidden: bool,
    fullscreen_insets: gpui::Edges<Pixels>,
    /// Shelf beside a display notch that holds the top tab bar, if any.
    notch_shelf: Option<Bounds<Pixels>>,
    /// The window frame the chrome sits in; the workspace keeps it current
    /// with the Linux client-side decorations GPUI reports.
    window_frame: windows::WindowFrame,
    theme: Theme,
    /// Failures waiting for the window's notice stack, oldest first. The
    /// tab's activity drain collects them with [`TerminalView::refresh`].
    failures: Vec<TerminalFailure>,
    /// Wakes the tab's activity drain when a failure is queued outside it.
    failure_wake: async_channel::Sender<()>,
    failure_wakes: async_channel::Receiver<()>,
    selection: Option<Selection>,
    selected_text: Option<String>,
    selecting: bool,
    scrollbars: Scrollbars,
    resize_visibility: IndicatorVisibility,
    /// Key cap shown beside "Jump to live" in the scroll pill.
    scroll_to_bottom_key: String,
    last_viewport: Option<gpui::Size<Pixels>>,
    selection_edge_direction: i64,
    scroll_benchmark: Option<ScrollBenchmark>,
    snapshot_sequence: u64,
    snapshot_pacer: refresh::SnapshotPacer,
    refresh_mode: huterm_config::RefreshMode,
    frame_clock: Rc<refresh::FrameClock>,
}

#[derive(Default)]
struct RefreshResult {
    changed: bool,
    exited: bool,
    /// The exit status when `exited` is set and the OS reported one.
    exit_code: Option<u32>,
    more: bool,
    /// Failures queued since the last drain, oldest first.
    failures: Vec<TerminalFailure>,
}

/// Failures the terminal queues at most: later ones are dropped so a stuck
/// runtime cannot grow the queue without bound.
const FAILURE_CAPACITY: usize = 8;

/// A terminal failure on its way to the window's notices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TerminalFailure {
    pub(super) severity: Severity,
    pub(super) title: &'static str,
    pub(super) message: String,
}

struct TerminalViewAuthority {
    presentation: PresentationController,
    host_effects: HostEffectRecipient,
}

impl TerminalView {
    #[allow(clippy::too_many_lines)]
    #[expect(
        clippy::too_many_arguments,
        reason = "terminal construction needs its window-owned frame clock"
    )]
    fn new(
        client: RuntimeClient,
        authority: TerminalViewAuthority,
        frame_clock: Rc<refresh::FrameClock>,
        config: &Config,
        font_family: String,
        metrics: GridMetrics,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let TerminalViewAuthority {
            presentation,
            host_effects,
        } = authority;
        frame_clock.observe(cx);
        let focus = cx.focus_handle();
        let theme = config.theme.clone();
        let initial_presentation = terminal_presentation(&theme);
        let (pending_presentation, presentation_failure) =
            match presentation.update(initial_presentation.clone()) {
                Ok(()) => (None, None),
                Err(RuntimeError::Busy) => (Some(initial_presentation), None),
                Err(error) => (None, Some(error.to_string())),
            };
        let (failure_wake, failure_wakes) = async_channel::bounded(1);
        let focus_subscription = cx.on_focus(
            &focus,
            window,
            |view: &mut TerminalView, window, cx| {
                // Focus that reaches the terminal while a dialog shows goes
                // back to the dialog; the program is told nothing. GPUI calls
                // this only when drawn frames' focus changes, so it misses
                // focus that returns to the terminal before the dialog's
                // first frame. Terminal presses suppress that focus-on-click.
                if let Some(dialog) = windows::modal_focus(window, cx) {
                    dialog.focus(window, cx);
                    return;
                }
                view.host_effects.note_focus();
                if view.visible
                    && view.enqueue_input(TerminalInput::Focus(true))
                {
                    cx.notify();
                }
            },
        );
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
                if view.visible {
                    if window.is_window_active() {
                        if view.bell.viewed(true) {
                            cx.notify();
                        }
                    } else {
                        view.clear_composition(cx);
                        view.blur_mouse(cx);
                        cx.notify();
                    }
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
        let (pending_work, wakes) = async_channel::bounded(1);
        let pending_work_task = cx.spawn(async move |view, cx| {
            while wakes.recv().await.is_ok() {
                loop {
                    while wakes.try_recv().is_ok() {}
                    let Ok(pending) = view.update(cx, |view, cx| {
                        view.refresh_pending_work(cx);
                        view.has_pending_work()
                    }) else {
                        return;
                    };
                    if !pending {
                        break;
                    }
                    // Busy runtimes need a bounded retry even without activity
                    // or display frames. Idle terminals never arm this timer.
                    cx.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                }
            }
        });
        let mut view = TerminalView {
            client,
            presentation,
            pending_presentation,
            host_effects,
            input_queue: InputQueue::default(),
            pending_work,
            _pending_work_task: pending_work_task,
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
            right_click: config.terminal.right_click,
            context_lookup: None,
            context_lookup_epoch: 0,
            link_requests: 0,
            link_completions: 0,
            link_max_lookup: Duration::ZERO,
            link_max_latency: Duration::ZERO,
            pending_resize: None,
            resize_requests: 0,
            resize_indicators: 0,
            quiet_resize: false,
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
            tabs_config: config.tabs,
            sidebar_width: windows::SIDEBAR_WIDTH,
            tab_presentation: windows::tab_visibility::Presentation::Hidden,
            tab_overlay: None,
            chrome_hidden: false,
            fullscreen_insets: gpui::Edges::default(),
            notch_shelf: None,
            window_frame: windows::WindowFrame::default(),
            theme,
            failures: Vec::new(),
            failure_wake,
            failure_wakes,
            title: String::new(),
            metadata: TerminalMetadata::default(),
            metadata_revision: 0,
            bell: BellPresentation::default(),
            bell_flash_count: 0,
            visual_bell: config.terminal.bell.visual,
            exited: false,
            exit_code: None,
            failed: false,
            visible: false,
            selection: None,
            selected_text: None,
            selecting: false,
            scrollbars: Scrollbars::vertical(TERMINAL_SCROLLBAR),
            resize_visibility: IndicatorVisibility::default(),
            scroll_to_bottom_key: DEFAULT_SCROLL_TO_BOTTOM_KEY.to_owned(),
            last_viewport: None,
            selection_edge_direction: 0,
            scroll_benchmark: ScrollBenchmark::from_environment(
                window.scale_factor(),
            ),
            snapshot_sequence: 0,
            snapshot_pacer: refresh::SnapshotPacer::default(),
            refresh_mode: config.terminal.refresh,
            frame_clock,
        };
        if let Some(message) = presentation_failure {
            view.report_failure(Severity::Error, "Terminal error", message);
        }
        view.wake_pending_work();
        key_bench::start(window.window_handle(), cx);
        view
    }

    fn start_initial_snapshot(&mut self, cx: &mut Context<'_, Self>) {
        if std::env::var_os("HUTERM_SCROLL_BENCH").is_some()
            || std::env::var_os("HUTERM_RENDER_BENCH").is_some()
            || std::env::var_os("HUTERM_ENGINE_DIAGNOSTIC").is_some()
        {
            eprintln!(
                "huterm-engine engine=ghostty revision={} snapshot=shared-rows native_optimize=ReleaseFast compression=disabled",
                self.client.engine_revision()
            );
        }
        self.scroll.invalidate();
        self.start_snapshot_if_needed(cx);
    }

    #[expect(
        clippy::too_many_lines,
        reason = "admission, link lookups, and benchmark samples share one request"
    )]
    fn start_snapshot_if_needed(&mut self, cx: &mut Context<'_, Self>) {
        if self.scroll.displayed() > 0 || self.scroll.desired() > 0 {
            self.cancel_mouse();
        }
        let Some(viewport) = self.snapshot_pacer.begin(
            &mut self.scroll,
            self.visible,
            self.refresh_mode,
        ) else {
            return;
        };
        self.frame_clock.schedule(cx.entity().downgrade(), cx);
        let link_intent = self.links.intent();
        let (context_lookup, link_point) = self.link_lookup_point(link_intent);
        let link_started = Instant::now();
        self.link_requests += u64::from(link_intent.is_some());
        let requested = self.client.request_snapshot_with_link(
            self.scroll.submitted_scroll(),
            link_point,
        );
        let request = match requested {
            Ok(request) => request,
            Err(error) => {
                self.scroll.fail();
                self.report_failure(
                    Severity::Error,
                    "Terminal error",
                    error.to_string(),
                );
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
                let changed = match result {
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
                        view.complete_context_lookup(context_lookup, reply.link.as_ref(), cx);
                        let hover = view.links.hover().cloned();
                        view.links.publish(link_intent, reply.link);
                        let hover_changed = view.links.hover() != hover.as_ref();
                        let changed =
                            view.apply_snapshot(reply.snapshot) || hover_changed;
                        view.renderer
                            .borrow_mut()
                            .record_output_applied(reply.invalidated_at, changed);
                        changed
                    }
                    Err(error) => {
                        view.scroll.fail();
                        // The window's notice stack draws the failure; the
                        // terminal itself has nothing new to render.
                        view.report_failure(
                            Severity::Error,
                            "Terminal error",
                            error.to_string(),
                        );
                        false
                    }
                };
                view.start_snapshot_if_needed(cx);
                if changed {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Applies a snapshot and reports whether the view must render again.
    fn apply_snapshot(&mut self, snapshot: TerminalSnapshot) -> bool {
        let mut changed = self
            .snapshot
            .as_deref()
            .is_none_or(|shown| !same_presentation(shown, &snapshot));
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
            changed = true;
        }
        if self.selecting
            && self.selection_edge_direction != 0
            && self.selection.is_some()
        {
            changed = true;
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
        changed
    }

    fn refresh(
        &mut self,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> RefreshResult {
        let previously_exited = self.exited;
        let mut host_count = 0;
        let mut event_count = 0;
        for _ in 0..8 {
            let Some(pending) = self.host_effects.try_next() else {
                break;
            };
            host_count += 1;
            if let HostEffect::ClipboardWrite(write) = pending.effect() {
                let item = ClipboardItem::new_string(write.text().to_owned());
                if self.host_effects.is_current(&pending) {
                    cx.write_to_clipboard(item);
                }
            }
        }
        if self.scroll.displayed() > 0 || self.scroll.desired() > 0 {
            self.cancel_mouse();
        }
        let mut changed = self.retry_client_messages();
        for _ in 0..64 {
            let event = self.client.try_recv_event();
            if matches!(event, Ok(Some(_))) {
                event_count += 1;
            }
            match event {
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
                // Applications often resend an unchanged title; only a new
                // title needs to re-render the tab and window.
                Ok(Some(TerminalEvent::TitleChanged { title, .. }))
                    if title != self.title =>
                {
                    self.title = title;
                    changed = true;
                }
                Ok(Some(TerminalEvent::MetadataChanged {
                    revision,
                    metadata,
                    ..
                })) => {
                    if revision > self.metadata_revision {
                        self.metadata_revision = revision;
                        self.metadata = metadata;
                        changed = true;
                    }
                }
                Ok(Some(TerminalEvent::Bell(_))) => {
                    let active = self.visible && window.is_window_active();
                    if active && self.visual_bell {
                        self.bell_flash_count =
                            self.bell_flash_count.wrapping_add(1);
                    }
                    changed |= self.bell.ring(
                        Instant::now(),
                        active,
                        self.visual_bell,
                    );
                }
                Ok(Some(TerminalEvent::Exited { status, .. })) => {
                    self.observe_exit(status.code, cx);
                    changed = true;
                }
                Ok(Some(TerminalEvent::Failed { message, .. })) => {
                    self.failed = true;
                    self.report_failure(
                        Severity::Error,
                        "Terminal failed",
                        message,
                    );
                    self.scroll.invalidate();
                    changed = true;
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }
        self.start_snapshot_if_needed(cx);
        if changed {
            cx.notify();
        }
        RefreshResult {
            changed,
            exited: !previously_exited && self.exited,
            exit_code: self.exit_code,
            more: host_count == 8 || event_count == 64,
            failures: std::mem::take(&mut self.failures),
        }
    }

    /// Root-shell exit: input closes and mouse ownership resets. Exit is
    /// not a failure; the window announces it only for a tab it keeps.
    fn observe_exit(&mut self, code: Option<u32>, cx: &mut Context<'_, Self>) {
        self.exited = true;
        self.exit_code = code;
        self.clear_composition(cx);
        self.input_queue.close();
        self.mouse = MouseState::default();
        self.scroll.invalidate();
    }

    /// Queues a failure for the window's notices and wakes the tab's
    /// activity drain. Returns whether it was queued: repeats of a waiting
    /// message and failures beyond [`FAILURE_CAPACITY`] are dropped.
    fn report_failure(
        &mut self,
        severity: Severity,
        title: &'static str,
        message: String,
    ) -> bool {
        if self.failures.len() >= FAILURE_CAPACITY
            || self
                .failures
                .iter()
                .any(|failure| failure.message == message)
        {
            return false;
        }
        self.failures.push(TerminalFailure {
            severity,
            title,
            message,
        });
        let _ = self.failure_wake.try_send(());
        true
    }

    /// Completes on runtime activity or a queued failure. `Err` means the
    /// runtime stopped; the failure channel outlives the view's drain, so it
    /// never closes first.
    pub(super) async fn wait_for_activity(
        client: &RuntimeClient,
        failure_wakes: &async_channel::Receiver<()>,
    ) -> Result<(), RuntimeError> {
        let mut activity = std::pin::pin!(client.wait_for_activity());
        let mut failure = std::pin::pin!(failure_wakes.recv());
        std::future::poll_fn(|context| {
            if let Poll::Ready(result) = activity.as_mut().poll(context) {
                return Poll::Ready(result);
            }
            match failure.as_mut().poll(context) {
                Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
                Poll::Ready(Err(_)) => Poll::Ready(Err(RuntimeError::Stopped)),
                Poll::Pending => Poll::Pending,
            }
        })
        .await
    }

    /// Whether the scroll pill is drawn, so the window can raise its
    /// notices above it.
    pub(super) fn scroll_pill_visible(&self) -> bool {
        self.scroll.displayed() > 0
            && self.scrollbars.opacity(Axis::Vertical) > 0.0
    }

    fn has_pending_work(&self) -> bool {
        !self.input_queue.is_empty()
            || self.pending_resize.is_some()
            || self.pending_presentation.is_some()
            || self.scroll_benchmark.is_some()
    }

    fn wake_pending_work(&self) {
        if self.has_pending_work() {
            let _ = self.pending_work.try_send(());
        }
    }

    fn refresh_pending_work(&mut self, cx: &mut Context<'_, Self>) {
        let mut changed = self.retry_client_messages();
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

    fn reload_terminal_config(
        &mut self,
        terminal: config::TerminalConfig,
        cx: &mut Context<'_, Self>,
    ) {
        self.host_effects
            .set_allowed(terminal.clipboard_write.is_allowed());
        self.refresh_mode = terminal.refresh;
        self.start_snapshot_if_needed(cx);
        self.links.disable();
        self.links_enabled = terminal.links;
        self.link_modifiers = terminal.link_modifiers;
        self.right_click = terminal.right_click;
        self.visual_bell = terminal.bell.visual;
        if !self.visual_bell && self.bell.clear() {
            cx.notify();
        }
        if self.option_as_alt != terminal.macos_option_as_alt {
            self.clear_composition(cx);
            self.option_as_alt = terminal.macos_option_as_alt;
        }
    }

    fn publish_presentation(&mut self, theme: &Theme) {
        let state = terminal_presentation(theme);
        match self.presentation.update(state.clone()) {
            Ok(()) => self.pending_presentation = None,
            Err(RuntimeError::Busy) => self.pending_presentation = Some(state),
            Err(error) => {
                self.pending_presentation = None;
                self.report_failure(
                    Severity::Error,
                    "Terminal error",
                    error.to_string(),
                );
            }
        }
        self.wake_pending_work();
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
        cx: &mut Context<'_, Self>,
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
            self.report_failure(
                Severity::Error,
                "Option input",
                format!("Option text input failed: {error}"),
            );
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
                        self.report_failure(
                            Severity::Error,
                            "Option input",
                            format!("Option text input failed: {error}"),
                        );
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
        if self.enqueue_input(input) {
            cx.notify();
        }
        self.return_to_live_output();
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
            || windows::modal_showing(window, cx)
        {
            return;
        }
        let hovered = self.links.hover().is_some();
        self.links.invalidate();
        if hovered {
            // A wheel event that moves no rows brings no snapshot, so the
            // cleared hover's underline and pointer need their own render.
            cx.notify();
        }
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
            // Rows and the thumb move when their snapshot arrives. Until then
            // only revealing a hidden or fading indicator changes the view;
            // an armed animation deadline picks up an extended hold.
            let revealed = self.scrollbars.opacity(Axis::Vertical) < 1.0;
            self.activate_scrollbar();
            if revealed {
                cx.notify();
            }
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
        if let Err(error) = self.run_command(&action.0, window, cx) {
            self.report_failure(
                Severity::Error,
                "Command failed",
                error.to_string(),
            );
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
            | ids::SCROLL_TO_BOTTOM
            | ids::SELECT_ALL
            | ids::CLEAR_SCROLLBACK
            | ids::RESET_TERMINAL => Ok(()),
            other => Err(CommandError::UnknownCommand(other)),
        }
    }

    /// Whether the view shows history rather than live output.
    fn scrolled_back(&self) -> bool {
        self.scroll.displayed() > 0
    }

    /// Where a keyboard-opened context menu anchors, in window
    /// coordinates: below the cursor cell, or the grid's top-left corner
    /// when the cursor is out of view.
    fn context_menu_anchor(&self, window: &Window) -> gpui::Point<Pixels> {
        let origin = self.content_bounds(window).origin
            + self.terminal_layout(window).bounds.origin;
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.cursor)
            .map_or(origin, |cursor| {
                origin
                    + point(
                        self.metrics.cell_width * f32::from(cursor.column),
                        self.metrics.cell_height * f32::from(cursor.row + 1),
                    )
            })
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
            ids::COPY => match (&self.selected_text, self.selection) {
                (Some(text), _) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        text.clone(),
                    ));
                }
                // The selection's text is still being extracted: copy it
                // when it arrives rather than dropping the request.
                (None, Some(selection)) => {
                    if let Some(range) = selection.range() {
                        self.extract_selection(selection, range, true, cx);
                    }
                }
                (None, None) => {}
            },
            ids::SELECT_ALL => self.select_all(cx),
            ids::CLEAR_SCROLLBACK | ids::RESET_TERMINAL => {
                let edited = if invocation.id == ids::CLEAR_SCROLLBACK {
                    self.client.clear_history()
                } else {
                    self.client.reset()
                };
                edited.map_err(|error| {
                    CommandError::Runtime(error.to_string())
                })?;
                // Both move history, so the selection's cells are gone. The
                // runtime invalidates the view once it applies the edit; a
                // snapshot requested now could run first and spend the
                // frame's allowance on the old content.
                self.clear_selection();
                self.links.invalidate();
                cx.notify();
            }
            ids::PASTE => {
                if let Some(text) =
                    cx.read_from_clipboard().and_then(|item| item.text())
                {
                    if self.enqueue_input(TerminalInput::Paste(text)) {
                        cx.notify();
                    }
                    self.return_to_live_output();
                    self.start_snapshot_if_needed(cx);
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
            && !self.scrollbars.dragging()
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
                    self.focus.focus(window, cx);
                    self.return_to_live_output();
                    self.start_snapshot_if_needed(cx);
                }
            }
            Err(error) => {
                self.report_failure(
                    Severity::Error,
                    "File drop",
                    error.to_owned(),
                );
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
                self.over_scrollbar(position, window),
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
        if windows::modal_showing(window, cx) {
            // A press dispatched against a frame drawn before the dialog's
            // scrim reaches this hitbox. Bubble listeners run in reverse
            // registration order, so this suppresses the `track_focus`
            // focus-on-click registered before it.
            window.prevent_default();
            return;
        }
        if !self.visible
            || self
                .tab_overlay
                .is_some_and(|bounds| bounds.contains(&event.position))
        {
            return;
        }
        self.external_drag = false;
        // Any later press supersedes a right-click still waiting for its
        // link lookup; a new right-click starts its own below.
        self.context_lookup = None;
        self.focus.focus(window, cx);
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
            && !self.over_scrollbar(
                event.position - self.content_bounds(window).origin,
                window,
            )
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
        // A right press during another gesture, such as a held link press
        // or a selection drag, opens nothing.
        let gesture = self.owns_pointer_gesture();
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
        if !application && event.button == MouseButton::Right {
            if !gesture {
                self.right_click(event.position, window, cx);
            }
            return;
        }
        if application || event.button != MouseButton::Left {
            return;
        }
        let position = event.position - self.content_bounds(window).origin;
        let geometries = self.scrollbar_geometries(window);
        if let Some((_, press)) = self.scrollbars.press(
            &geometries,
            self.viewport_bounds(window),
            position,
            Instant::now(),
        ) {
            match press {
                Press::Grabbed | Press::Jump(_) => cx.notify(),
                Press::Page { backward } => {
                    if self.scroll.page(self.last_grid_size.rows, backward) {
                        self.start_snapshot_if_needed(cx);
                        cx.notify();
                    }
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

    /// The cell a snapshot request looks up, and the right-click it answers.
    /// A held link chord owns the lookup; a right-click that finds one
    /// pending has already opened its menu without waiting.
    fn link_lookup_point(
        &self,
        link_intent: Option<links::Intent>,
    ) -> (
        Option<ContextLookup>,
        Option<huterm_protocol::MousePosition>,
    ) {
        let context_lookup =
            self.context_lookup.filter(|_| link_intent.is_none());
        let point = link_intent
            .map(|intent| intent.point)
            .or(context_lookup.map(|lookup| lookup.cell));
        (context_lookup, point)
    }

    /// Opens the menu of a right-click whose lookup `outcome` answers, unless
    /// a newer right-click or the timeout replaced it.
    fn complete_context_lookup(
        &mut self,
        lookup: Option<ContextLookup>,
        outcome: Option<&huterm_protocol::LinkLookup>,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(lookup) =
            lookup.filter(|lookup| self.context_lookup == Some(*lookup))
        else {
            return;
        };
        self.context_lookup = None;
        let link = match outcome {
            Some(huterm_protocol::LinkLookup::Match(link)) => {
                Some(link.destination.clone())
            }
            _ => None,
        };
        cx.emit(ContextMenuRequest {
            position: lookup.position,
            link,
        });
    }

    /// A right press no application took: the configured action.
    fn right_click(
        &mut self,
        position: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let command = match self.right_click {
            config::RightClickAction::Menu => {
                let cell = self.link_cell(position, window);
                self.request_context_menu(position, cell, cx);
                return;
            }
            config::RightClickAction::Ignore => return,
            config::RightClickAction::Paste => ids::PASTE,
            config::RightClickAction::CopyOrPaste => {
                if copy_availability(self.selection).is_ok() {
                    ids::COPY
                } else {
                    ids::PASTE
                }
            }
        };
        let result = self.run_command(
            &CommandInvocation::new(command, Vec::new()),
            window,
            cx,
        );
        if let Err(error) = result {
            self.report_failure(
                Severity::Error,
                "Command failed",
                error.to_string(),
            );
        } else if command == ids::COPY {
            self.clear_selection();
            cx.notify();
        }
    }

    /// Asks the window for the context menu at `position`, first looking
    /// up the link at `cell` unless its result is already known. The
    /// lookup rides on a snapshot request and is bounded by
    /// [`CONTEXT_LINK_TIMEOUT`], after which the menu opens without it.
    fn request_context_menu(
        &mut self,
        position: gpui::Point<Pixels>,
        cell: Option<huterm_protocol::MousePosition>,
        cx: &mut Context<'_, Self>,
    ) {
        self.context_lookup = None;
        let hovered = self
            .links
            .hover()
            .filter(|link| {
                cell.is_some_and(|cell| {
                    link.cells.iter().any(|linked| linked.position == cell)
                })
            })
            .map(|link| link.destination.clone());
        let cell = cell.filter(|_| {
            hovered.is_none()
                && self.links_enabled
                && self.links.intent().is_none()
        });
        let Some(cell) = cell else {
            cx.emit(ContextMenuRequest {
                position,
                link: hovered,
            });
            return;
        };
        self.context_lookup_epoch = self.context_lookup_epoch.wrapping_add(1);
        let lookup = ContextLookup {
            epoch: self.context_lookup_epoch,
            cell,
            position,
        };
        self.context_lookup = Some(lookup);
        self.scroll.invalidate();
        self.start_snapshot_if_needed(cx);
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(CONTEXT_LINK_TIMEOUT).await;
            let _ = view.update(cx, |view, cx| {
                if view.context_lookup == Some(lookup) {
                    view.context_lookup = None;
                    cx.emit(ContextMenuRequest {
                        position: lookup.position,
                        link: None,
                    });
                }
            });
        })
        .detach();
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
        let now = Instant::now();
        let geometries = self.scrollbar_geometries(window);
        let bounds = self.viewport_bounds(window);
        if self
            .scrollbars
            .pointer_moved(&geometries, bounds, position, now)
        {
            self.activate_scrollbar();
            cx.notify();
        }
        if let Some((_, thumb_start)) =
            self.scrollbars.drag_to(bounds, position, now)
        {
            self.scrollbar_seek(px(thumb_start), window, cx);
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
            let head = point_for_position(position, snapshot, self.metrics);
            let direction = edge_scroll_direction(
                f32::from(position.y),
                f32::from(layout.bounds.size.height),
            );
            // Movement within one cell changes nothing. Past an edge, each
            // move still scrolls a row, so only the idle case returns early.
            if selection.head == head
                && direction == 0
                && self.selection_edge_direction == 0
            {
                return;
            }
            selection.head = head;
            self.selected_text = None;
            self.update_renderer_selection();
            if self.selection.and_then(Selection::range).is_none() {
                self.selection_edge_direction = 0;
                cx.notify();
                return;
            }
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
        if self.scrollbars.release(Instant::now()) {
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
        self.extract_selection(selection, range, false, cx);
    }

    /// Selects every row of history and the screen.
    fn select_all(&mut self, cx: &mut Context<'_, Self>) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let rows = usize::from(snapshot.size.rows);
        let columns = snapshot.size.columns;
        if rows == 0 || columns == 0 {
            return;
        }
        let selection = Selection {
            generation: snapshot.generation,
            anchor: BufferPoint {
                rows_from_live_bottom: snapshot.history_size + rows - 1,
                column: 0,
            },
            head: BufferPoint {
                rows_from_live_bottom: 0,
                column: columns - 1,
            },
        };
        self.selection = Some(selection);
        self.selected_text = None;
        self.selecting = false;
        self.update_renderer_selection();
        if let Some(range) = selection.range() {
            self.extract_selection(selection, range, false, cx);
        }
        cx.notify();
    }

    /// Asks the runtime for `selection`'s text and keeps it while the
    /// selection stays current. With `copy`, the text also goes to the
    /// clipboard when it arrives.
    fn extract_selection(
        &mut self,
        selection: Selection,
        range: BufferRange,
        copy: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let request =
            match self.client.request_selection(selection.generation, range) {
                Ok(request) => request,
                Err(error) => {
                    self.report_failure(
                        Severity::Error,
                        "Terminal error",
                        error.to_string(),
                    );
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
                    // A requested copy writes the text it asked for even if
                    // the selection changed since; only caching needs it
                    // current.
                    Ok(Some(text)) => {
                        if copy {
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                text.clone(),
                            ));
                        }
                        if current {
                            view.selected_text = Some(text);
                        }
                    }
                    Ok(None) if current => view.clear_selection(),
                    Ok(_) => {}
                    Err(error) => {
                        view.report_failure(
                            Severity::Error,
                            "Terminal error",
                            error.to_string(),
                        );
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
        let offset = rows_for_offset(
            geometry.offset_for_thumb_start(f32::from(thumb_start)),
        );
        if self.scroll.set_desired(offset) {
            self.activate_scrollbar();
            self.start_snapshot_if_needed(cx);
            cx.notify();
        }
    }

    fn scrollbar_geometry(&self, window: &Window) -> Option<ScrollbarGeometry> {
        terminal_scrollbar_geometry(
            f32::from(self.viewport(window).height),
            self.last_grid_size.rows,
            self.scroll.history(),
            self.scroll.displayed(),
            self.scrollbar_options().margins,
        )
    }

    /// The terminal scrollbar options for the current chrome: with a right
    /// tab bar under top chrome, the track starts below the rounded corner
    /// so the patch never covers the thumb.
    fn scrollbar_options(&self) -> ScrollbarOptions {
        ScrollbarOptions {
            margins: terminal_track_margins(
                self.tab_presentation,
                self.tabs_config.position,
                windows::title_row_height(
                    self.chrome_hidden,
                    self.window_frame,
                ),
                self.window_config,
            ),
            ..TERMINAL_SCROLLBAR
        }
    }

    fn scrollbar_geometries(&self, window: &Window) -> ScrollbarGeometries {
        ScrollbarGeometries::vertical(self.scrollbar_geometry(window))
    }

    /// The viewport in the content-relative coordinates mouse handlers use.
    fn viewport_bounds(&self, window: &Window) -> Bounds<Pixels> {
        Bounds::new(point(px(0.0), px(0.0)), self.viewport(window))
    }

    /// Whether the content-relative `position` is over the visible
    /// scrollbar strip.
    fn over_scrollbar(
        &self,
        position: gpui::Point<Pixels>,
        window: &Window,
    ) -> bool {
        self.scrollbars
            .hit(
                &self.scrollbar_geometries(window),
                self.viewport_bounds(window),
                position,
            )
            .is_some()
    }
    fn clear_selection(&mut self) {
        self.selection = None;
        self.selected_text = None;
        self.selecting = false;
        self.selection_edge_direction = 0;
        self.update_renderer_selection();
    }
    fn activate_scrollbar(&mut self) {
        self.scrollbars.show(Axis::Vertical, Instant::now());
    }
    fn update_renderer_selection(&self) {
        self.renderer
            .borrow_mut()
            .set_selection(self.selection.and_then(Selection::range));
    }

    fn owns_pointer_gesture(&self) -> bool {
        self.selecting
            || self.scrollbars.dragging()
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
        windows::ChromeLayout::for_tabs(
            window.viewport_size(),
            windows::title_row_height(self.chrome_hidden, self.window_frame),
            self.tabs_config,
            self.sidebar_width,
            self.fullscreen_insets,
            self.notch_shelf,
            self.window_frame,
        )
        .present(self.tab_presentation, self.tabs_config.position, 0.0)
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

    fn physical_cell_size(&self) -> CellSize {
        CellSize {
            width: pixel_count(
                self.metrics.cell_width * self.metrics.scale_factor,
            ),
            height: pixel_count(
                self.metrics.cell_height * self.metrics.scale_factor,
            ),
        }
    }

    fn resize_if_needed(&mut self, window: &Window) {
        self.resize_to_layout(window, false);
    }

    /// Resizes to the current layout. A `quiet` change keeps the size panel
    /// hidden: the tab bar appeared or hid, which is not a resize the user
    /// made.
    fn resize_to_layout(&mut self, window: &Window, quiet: bool) {
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
            if !quiet && !self.quiet_resize {
                self.resize_visibility.activate(Instant::now());
                self.resize_indicators += 1;
            }
        }
        let size = self.terminal_layout(window).grid;
        let cell = self.physical_cell_size();
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
            Err(error) => {
                self.pending_resize = None;
                self.report_failure(
                    Severity::Error,
                    "Terminal error",
                    error.to_string(),
                );
            }
        }
        self.wake_pending_work();
    }

    /// Scrolls back to live output after queueing input. The caller starts the
    /// snapshot, which is needed only when this moved the viewport: the
    /// input's echo invalidates the terminal itself, and a snapshot requested
    /// before the echo exists would spend the frame's allowance and delay it.
    fn return_to_live_output(&mut self) {
        self.scroll.bottom();
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
        let result = match self.input_queue.enqueue(input, release, |input| self.client.offer_input(input)) {
            Ok(Admission::Accepted) => (true, false),
            Ok(Admission::Closed) => (false, false),
            Ok(Admission::Full) if quiet => (false, false),
            Ok(Admission::Full) => (false, self.report_failure(Severity::Warning, "Input rejected", format!("Input buffer full ({PENDING_INPUT_CAPACITY} events or {PENDING_INPUT_BYTE_CAPACITY} bytes); input rejected"))),
            Err(error) => {
                self.mouse = MouseState::default();
                (false, self.report_failure(Severity::Error, "Terminal error", error.to_string()))
            }
        };
        self.wake_pending_work();
        result
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
        self.scrollbars.pointer_left(Instant::now());
        self.visible = false;
    }

    fn blur_mouse(&mut self, cx: &mut Context<'_, Self>) {
        // A right-click still waiting for its link lookup must not open its
        // menu once focus or visibility has moved on.
        self.context_lookup = None;
        self.links.disable();
        self.cancel_mouse();
        self.finish_selection(cx);
        self.scrollbars.release(Instant::now());
        self.selection_edge_direction = 0;
        self.scroll.reset_wheel();
    }

    fn retry_client_messages(&mut self) -> bool {
        if self.exited {
            self.input_queue.close();
        }
        if let Err(error) = self
            .input_queue
            .retry(|input| self.client.offer_input(input))
        {
            self.mouse = MouseState::default();
            return self.report_failure(
                Severity::Error,
                "Terminal error",
                error.to_string(),
            );
        }
        if let Some((grid, cell)) = self.pending_resize {
            match self.client.resize(grid, cell) {
                Ok(()) => self.pending_resize = None,
                Err(RuntimeError::Busy) => {}
                Err(error) => {
                    self.pending_resize = None;
                    return self.report_failure(
                        Severity::Error,
                        "Terminal error",
                        error.to_string(),
                    );
                }
            }
        }
        if let Some(presentation) = self.pending_presentation.clone() {
            match self.presentation.update(presentation) {
                Ok(()) => self.pending_presentation = None,
                Err(RuntimeError::Busy) => {}
                Err(error) => {
                    self.pending_presentation = None;
                    return self.report_failure(
                        Severity::Error,
                        "Terminal error",
                        error.to_string(),
                    );
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

    /// Shows `key` (the compiled keymap's first `scroll_to_bottom` binding
    /// in the Terminal context) in the scroll pill; `None` restores the
    /// platform default.
    pub(super) fn set_scroll_to_bottom_key(
        &mut self,
        key: Option<String>,
        cx: &mut Context<'_, Self>,
    ) {
        let key =
            key.unwrap_or_else(|| DEFAULT_SCROLL_TO_BOTTOM_KEY.to_owned());
        if self.scroll_to_bottom_key != key {
            self.scroll_to_bottom_key = key;
            cx.notify();
        }
    }
}

impl refresh::Animated for TerminalView {
    fn animation_schedule(&self, now: Instant) -> AnimationSchedule {
        let mut next = self
            .scrollbars
            .schedule(now)
            .merge(self.resize_visibility.schedule(now));
        if let Some(at) = self.bell.flash_until {
            next = next.merge(AnimationSchedule::at(at));
        }
        if !self.visible {
            next.frame = false;
        }
        next
    }

    fn advance_animation(
        &mut self,
        now: Instant,
        _frame: bool,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.scrollbars.advance(now)
            | self.resize_visibility.update(now, false)
            | self.bell.advance(now)
        {
            cx.notify();
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
                "huterm-scroll environment revision={} profile=release os={} arch={} hardware={} display_scale={} gpui=gpui-pre-0.3.6 viewport={}x{} history={}",
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

impl TerminalView {
    /// The hovered link's destination on a small raised panel at the
    /// terminal's bottom-left corner, truncated to the terminal's width.
    fn render_link_status(
        &self,
        destination: String,
        terminal_bounds: Bounds<Pixels>,
        viewport_height: Pixels,
    ) -> impl IntoElement {
        let swatch = Swatch::from_theme(&self.theme);
        let inset = px(8.0);
        div()
            .absolute()
            .left(terminal_bounds.origin.x + inset)
            .bottom(viewport_height - terminal_bounds.bottom() + inset)
            .max_w((terminal_bounds.size.width - inset * 2.0).max(px(0.0)))
            .flex()
            .child(
                raised_panel(swatch, 8.0)
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .h(px(26.0))
                    .px(px(10.0))
                    .text_size(px(12.0))
                    .text_color(swatch.fg)
                    .child(
                        svg()
                            .path(Icon::Link.asset_path())
                            .size(px(13.0))
                            .flex_none()
                            .text_color(swatch.muted),
                    )
                    .child(
                        div()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(destination),
                    ),
            )
    }

    /// The pill at the bottom centre of the terminal bounds while scrolled
    /// back. It consumes presses and releases so a click cannot start a
    /// selection or application mouse input, and fades with the indicator.
    fn render_scroll_pill(
        &self,
        pill: ScrollPill,
        terminal_bounds: Bounds<Pixels>,
        viewport_height: Pixels,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let swatch = Swatch::from_theme(&self.theme);
        let hints = self.window_config.shortcut_hints;
        let bottom = viewport_height
            - (terminal_bounds.origin.y + terminal_bounds.size.height)
            + px(12.0);
        // Presses stay on the pill. Releases pass through: a selection or
        // application gesture that started in the terminal must still end
        // there, and the terminal ignores releases it does not own.
        let stop = |_: &MouseDownEvent, _: &mut Window, cx: &mut App| {
            cx.stop_propagation();
        };
        div()
            .absolute()
            .left(terminal_bounds.origin.x)
            .w(terminal_bounds.size.width)
            .bottom(bottom)
            .flex()
            .justify_center()
            .child(
                raised_panel(swatch, 15.0)
                    .id("scroll-pill")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(10.0))
                    .h(px(30.0))
                    .pl(px(12.0))
                    .pr(px(if hints { 6.0 } else { 12.0 }))
                    .text_size(px(12.0))
                    .text_color(swatch.fg)
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(|pill| pill.bg(swatch.surface.blend(swatch.hover)))
                    .active(|pill| {
                        pill.bg(swatch.surface.blend(swatch.pressed()))
                    })
                    .opacity(self.scrollbars.opacity(Axis::Vertical))
                    .on_mouse_down(MouseButton::Left, stop)
                    .on_mouse_down(MouseButton::Right, stop)
                    .on_mouse_down(MouseButton::Middle, stop)
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.scroll_command(cx, |scroll, _| scroll.bottom());
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child("↑")
                            .child(
                                div()
                                    .font_family(mono_font_family())
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(pill.offset),
                            )
                            .child(format!("of {} lines", pill.total)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .h(px(18.0))
                            .pl(px(10.0))
                            .when(hints, |jump| jump.pr(px(4.0)))
                            .border_l_1()
                            .border_color(swatch.line)
                            .text_color(swatch.muted)
                            .child("Jump to live")
                            .when(hints, |jump| {
                                jump.child(key_cap(
                                    &KeyHint::parse(
                                        &self.scroll_to_bottom_key,
                                        keymap::Platform::current(),
                                    ),
                                    swatch,
                                ))
                            }),
                    ),
            )
    }

    /// The centred `columns × rows` panel while the grid size changes. The
    /// surface stays opaque; the fade applies to the whole panel.
    fn render_size_panel(&self) -> impl IntoElement {
        let swatch = Swatch::from_theme(&self.theme);
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .child(
                raised_panel(swatch, 12.0)
                    .opacity(self.resize_visibility.opacity)
                    .pt(px(14.0))
                    .pb(px(12.0))
                    .px(px(22.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .font_family(mono_font_family())
                            .text_size(px(26.0))
                            .line_height(px(26.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(swatch.fg)
                            .child(self.last_grid_size.columns.to_string())
                            .child(
                                div()
                                    .mx(px(6.0))
                                    .font_weight(gpui::FontWeight::NORMAL)
                                    .text_color(swatch.dim)
                                    .child("×"),
                            )
                            .child(self.last_grid_size.rows.to_string()),
                    )
                    .child(
                        div()
                            .mt(px(6.0))
                            .text_size(px(11.0))
                            .text_color(swatch.muted)
                            .child("columns × rows"),
                    ),
            )
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
        // Native layout can activate an indicator without a terminal event or
        // entity notification (for example, a resize within the same grid cell).
        self.frame_clock.animate(
            cx.entity().downgrade(),
            refresh::Animated::animation_schedule(self, Instant::now()),
            cx,
        );
        self.update_link_pointer(
            window.mouse_position(),
            window.modifiers(),
            window,
            cx,
        );
        if let Some(benchmark) = &mut self.scroll_benchmark {
            benchmark.display_scale = window.scale_factor();
        }
        if self.renderer.borrow().requests_continuous_frames() {
            window.request_animation_frame();
        }
        let snapshot = self.snapshot.clone();
        let bell_flash = self.bell.flashing(Instant::now());
        let prepare_renderer = Rc::clone(&self.renderer);
        let paint_renderer = Rc::clone(&self.renderer);
        let mouse_view = cx.entity().downgrade();
        #[cfg(target_os = "macos")]
        let input_view = cx.entity();
        #[cfg(target_os = "macos")]
        let input_focus = self.focus.clone();
        self.scrollbars
            .set_axis(Axis::Vertical, Some(self.scrollbar_options()));
        let layout = self.terminal_layout(window);
        let terminal_bounds = layout.bounds;
        let hovered_link = self.links.hover().cloned();
        let link_metrics = self.metrics;
        let underline = color(self.theme.foreground);
        let paint_link = hovered_link.clone();
        let mut root = div()
            .id("terminal")
            .on_hover(cx.listener(|view, hovering, _, cx| {
                if !hovering && view.scrollbars.pointer_left(Instant::now()) {
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
                    // Notifies only when link hover changes. Modifier presses
                    // while typing must not re-render the window.
                    view.update_link_pointer(
                        window.mouse_position(),
                        event.modifiers,
                        window,
                        cx,
                    );
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
                                    paint_link_underline(
                                        link,
                                        bounds,
                                        link_metrics,
                                        underline,
                                        window,
                                    );
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
        if bell_flash {
            root = root.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(color(self.theme.foreground).opacity(0.14)),
            );
        }
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
                self.render_link_status(
                    link.destination,
                    terminal_bounds,
                    self.viewport(window).height,
                ),
            );
        }
        let displayed_offset = self.scroll.displayed();
        let geometries = self.scrollbar_geometries(window);
        if self.scrollbars.visible(Axis::Vertical)
            && geometries.vertical.is_some()
        {
            root = root.children(
                self.scrollbars
                    .layers(&geometries, scrollbar_colors(&self.theme))
                    .collect::<Vec<_>>(),
            );
            if let Some(pill) =
                ScrollPill::new(displayed_offset, self.scroll.history())
            {
                root = root.child(self.render_scroll_pill(
                    pill,
                    terminal_bounds,
                    self.viewport(window).height,
                    cx,
                ));
            }
        }
        if self.resize_visibility.opacity > 0.0 {
            root = root.child(self.render_size_panel());
        }
        root
    }
}

/// Underlines a hovered link with one quad per row segment, in a single layer
/// so GPUI orders them with one bounds-tree insertion.
fn paint_link_underline(
    link: &huterm_protocol::TerminalLink,
    grid: Bounds<Pixels>,
    metrics: GridMetrics,
    color: gpui::Hsla,
    window: &mut Window,
) {
    let cells =
        |value: u32| f32::from(u16::try_from(value).unwrap_or(u16::MAX));
    window.paint_layer(grid, |window| {
        for (start, columns) in link_segments(&link.cells) {
            let origin = grid.origin
                + point(
                    metrics.cell_width * cells(start.column),
                    metrics.cell_height * (cells(start.row) + 1.0) - px(1.0),
                );
            window.paint_quad(gpui::fill(
                Bounds::new(
                    origin,
                    size(metrics.cell_width * cells(columns), px(1.0)),
                ),
                color,
            ));
        }
    });
}

/// Merges a link's ordered cells into runs of adjacent cells on one row.
fn link_segments(
    cells: &[huterm_protocol::LinkCell],
) -> Vec<(MousePosition, u32)> {
    let mut segments: Vec<(MousePosition, u32)> = Vec::new();
    for cell in cells {
        let position = cell.position;
        if let Some((start, columns)) = segments.last_mut()
            && start.row == position.row
            && start.column.checked_add(*columns) == Some(position.column)
        {
            *columns += 1;
        } else {
            segments.push((position, 1));
        }
    }
    segments
}

/// Terminal scrollbar geometry in rows: `history` rows above `visible_rows`,
/// with `displayed_offset` counted from the bottom.
#[expect(
    clippy::cast_precision_loss,
    reason = "terminal scrollback is capped at 10000 rows"
)]
fn terminal_scrollbar_geometry(
    height: f32,
    visible_rows: u16,
    history: usize,
    displayed_offset: usize,
    margins: TrackMargins,
) -> Option<ScrollbarGeometry> {
    let visible = f32::from(visible_rows.max(1));
    ScrollbarGeometry::new(
        height,
        visible + history as f32,
        visible,
        displayed_offset.min(history) as f32,
        TERMINAL_SCROLLBAR.origin,
        margins,
    )
}

/// The terminal scrollbar's track margins: with a right tab bar shown
/// under a titlebar, the track starts below the rounded corner patch so the
/// patch never covers the thumb.
fn terminal_track_margins(
    presentation: windows::tab_visibility::Presentation,
    position: huterm_config::TabPosition,
    titlebar: Pixels,
    window: WindowConfig,
) -> TrackMargins {
    let mut margins = TERMINAL_SCROLLBAR.margins;
    if presentation == windows::tab_visibility::Presentation::Reserved
        && position == huterm_config::TabPosition::Right
        && titlebar > px(0.0)
    {
        let radius = f32::from(windows::terminal_corner_radius(window));
        margins.start = margins.start.max(radius + 1.0);
    }
    margins
}

/// Rounds a scrollbar offset in rows back to a history offset.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a bounded scrollbar ratio maps to at most 10000 rows"
)]
fn rows_for_offset(offset: f32) -> usize {
    offset.round() as usize
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

/// The theme's overlay scrollbar colors as GPUI colors.
pub(super) fn scrollbar_colors(theme: &Theme) -> ScrollbarColors {
    let ui = theme.ui();
    ScrollbarColors {
        thumb: rgba_color(ui.scrollbar_thumb),
        track: rgba_color(ui.scrollbar_track),
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
    theme: &Theme,
    identity: huterm_config::TerminalIdentity,
) -> anyhow::Result<TerminalCommand> {
    let shell =
        std::env::var_os("SHELL").map_or_else(default_shell, PathBuf::from);
    let is_macos = cfg!(target_os = "macos");
    let arguments = shell_arguments(is_macos);
    let mut environment = locale_environment(
        is_macos && is_packaged_macos(),
        ["LANG", "LC_CTYPE", "LC_ALL"]
            .into_iter()
            .any(|name| std::env::var_os(name).is_some()),
    );
    environment.extend(crate::terminfo::environment(identity));
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
        presentation: terminal_presentation(theme),
    })
}

fn terminal_presentation(theme: &Theme) -> TerminalPresentation {
    let palette = std::array::from_fn(|index| {
        theme.indexed(u8::try_from(index).expect("palette index fits in u8"))
    });
    TerminalPresentation {
        foreground: theme.foreground,
        background: theme.background,
        cursor: theme.cursor,
        palette,
    }
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

/// Whether two snapshots draw the same view. Unchanged rows share their `Arc`
/// across snapshots. The generation is not compared: presentation updates
/// change colors without advancing it.
fn same_presentation(
    shown: &TerminalSnapshot,
    next: &TerminalSnapshot,
) -> bool {
    shown.size == next.size
        && shown.cursor == next.cursor
        && shown.cursor_color == next.cursor_color
        && shown.modes == next.modes
        && shown.viewport == next.viewport
        && shown.history_size == next.history_size
        && shown.rows.len() == next.rows.len()
        && shown
            .rows
            .iter()
            .zip(&next.rows)
            .all(|(shown, next)| Arc::ptr_eq(shown, next))
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

/// Text for the scroll pill: the displayed snapshot's offset within its
/// retained history, both with thousands separators.
struct ScrollPill {
    offset: String,
    total: String,
}

impl ScrollPill {
    /// `None` at the live bottom, where the pill hides while the indicator
    /// fades.
    fn new(displayed_offset: usize, history: usize) -> Option<Self> {
        (displayed_offset > 0).then(|| Self {
            offset: format_line_count(displayed_offset),
            total: format_line_count(history.max(displayed_offset)),
        })
    }

    #[cfg(test)]
    fn text(&self) -> String {
        format!("↑ {} of {} lines", self.offset, self.total)
    }
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
    fn terminal_presentation_uses_the_resolved_theme() {
        let theme = Theme {
            background: huterm_protocol::Rgb {
                red: 0xff,
                green: 0xff,
                blue: 0xff,
            },
            ..Theme::default()
        };
        let presentation = terminal_presentation(&theme);
        assert_eq!(presentation.foreground, theme.foreground);
        assert_eq!(presentation.background, theme.background);
        assert_eq!(presentation.cursor, theme.cursor);
        assert_eq!(presentation.palette[0], theme.indexed(0));
        assert_eq!(presentation.palette[255], theme.indexed(255));
    }

    #[test]
    fn link_segments_split_at_row_wraps_and_gaps() {
        let cell = |row, column| huterm_protocol::LinkCell {
            position: MousePosition { column, row },
            text: huterm_protocol::CellText::from('x'),
        };
        let cells =
            [cell(0, 78), cell(0, 79), cell(1, 0), cell(1, 1), cell(1, 3)];
        assert_eq!(
            link_segments(&cells),
            [
                (MousePosition { column: 78, row: 0 }, 2),
                (MousePosition { column: 0, row: 1 }, 2),
                (MousePosition { column: 3, row: 1 }, 1),
            ]
        );
    }

    #[test]
    fn snapshot_comparison_ignores_generation_but_not_rows_or_viewport() {
        let row =
            || Arc::new(huterm_protocol::TerminalRow { cells: Vec::new() });
        let shown = TerminalSnapshot {
            terminal_id: huterm_protocol::TerminalId::new(1),
            generation: 1,
            size: GridSize::clamped(1, 2),
            rows: vec![row(), row()],
            cursor: None,
            modes: huterm_protocol::TerminalModes::default(),
            viewport: huterm_protocol::Viewport { bottom_offset: 0 },
            history_size: 0,
            cursor_color: None,
        };
        // Presentation updates can change colors without a new generation,
        // and an unchanged generation must not hide new rows.
        let recolored = TerminalSnapshot {
            generation: 2,
            ..shown.clone()
        };
        assert!(same_presentation(&shown, &recolored));
        let mut new_row = shown.clone();
        new_row.rows[1] = row();
        assert!(!same_presentation(&shown, &new_row));
        let scrolled = TerminalSnapshot {
            viewport: huterm_protocol::Viewport { bottom_offset: 1 },
            history_size: 1,
            ..shown.clone()
        };
        assert!(!same_presentation(&shown, &scrolled));
        let grown = TerminalSnapshot {
            history_size: 1,
            ..shown.clone()
        };
        assert!(!same_presentation(&shown, &grown), "scrollbar size changed");
    }

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
    fn terminal_track_starts_below_the_corner_only_beside_a_right_bar() {
        use huterm_config::TabPosition;
        use windows::tab_visibility::Presentation;
        let base = TERMINAL_SCROLLBAR.margins;
        let window = WindowConfig::default();
        let margins = |presentation, position, titlebar| {
            terminal_track_margins(presentation, position, px(titlebar), window)
        };
        // Default padding 4 gives a 4-point corner, so the track starts at 5.
        assert!(
            (margins(Presentation::Reserved, TabPosition::Right, 32.0).start
                - 5.0)
                .abs()
                < f32::EPSILON
        );
        for (presentation, position, titlebar) in [
            (Presentation::Reserved, TabPosition::Left, 32.0),
            (Presentation::Reserved, TabPosition::Top, 32.0),
            (Presentation::Hidden, TabPosition::Right, 32.0),
            (Presentation::Reserved, TabPosition::Right, 0.0),
        ] {
            assert_eq!(margins(presentation, position, titlebar), base);
        }
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
    fn live_bottom_hides_the_scroll_pill_while_the_indicator_fades() {
        assert!(ScrollPill::new(0, 9_870).is_none());
    }

    #[test]
    fn scroll_pill_uses_the_displayed_snapshot_offset_and_history() {
        assert_eq!(
            ScrollPill::new(1_284, 9_870).map(|pill| pill.text()),
            Some("↑ 1,284 of 9,870 lines".to_owned())
        );
        assert_eq!(
            ScrollPill::new(12, 12).map(|pill| pill.text()),
            Some("↑ 12 of 12 lines".to_owned())
        );
        // A snapshot from before history shrank can exceed the new total.
        assert_eq!(
            ScrollPill::new(5, 3).map(|pill| pill.text()),
            Some("↑ 5 of 5 lines".to_owned())
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

    #[test]
    fn visual_bell_flashes_active_views_and_marks_inactive_views() {
        let now = Instant::now();
        let mut active = BellPresentation::default();
        assert!(active.ring(now, true, true));
        assert!(active.flashing(now));
        assert!(!active.unseen);
        assert!(!active.advance(now + VISUAL_BELL_DURATION / 2));
        assert!(active.ring(now + VISUAL_BELL_DURATION / 2, true, true));
        assert!(active.flashing(now + VISUAL_BELL_DURATION));
        assert!(active.advance(now + VISUAL_BELL_DURATION * 2));

        let mut inactive = BellPresentation::default();
        assert!(inactive.ring(now, false, true));
        assert!(inactive.unseen);
        assert!(!inactive.viewed(false));
        assert!(inactive.viewed(true));
        assert!(!inactive.unseen);
    }

    #[test]
    fn disabling_visual_bells_clears_transient_presentation() {
        let now = Instant::now();
        let mut bell = BellPresentation::default();
        bell.ring(now, false, true);
        bell.ring(now, true, true);
        assert!(bell.clear());
        assert_eq!(bell, BellPresentation::default());
        assert!(!bell.ring(now, false, false));
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
