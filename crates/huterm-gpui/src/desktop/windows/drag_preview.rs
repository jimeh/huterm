//! The macOS floating drag preview: a borderless `PopUp` window that follows
//! the pointer while a tab or pane drag is outside its source window. It
//! opens per drag, never takes focus, key status, or mouse events, and
//! closes when the drag record ends. Every change to the drag record calls
//! [`sync`], which defers so native frame changes run outside GPUI borrows.
use super::cross_window::{self, Drag};
use super::tab_bar::TabColors;
use super::window_locator::Located;
use super::*;
use gpui::{WindowBackgroundAppearance, WindowHandle, WindowKind};

#[cfg(target_os = "macos")]
#[path = "drag_preview/macos.rs"]
mod native;

/// What the preview shows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum PreviewMode {
    /// No preview: the setting is off, the platform has none, there is no
    /// drag, or the pointer is inside the source window.
    #[default]
    None,
    /// The title chip, over another Huterm window that takes the drop.
    Chip,
    /// A miniature window: a release opens a new window.
    Window,
    /// The chip, dimmed: a release does nothing.
    Dimmed,
}

impl PreviewMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Chip => "chip",
            Self::Window => "window",
            Self::Dimmed => "dimmed",
        }
    }
}

/// Where a drag's pointer is, as the preview distinguishes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Spot {
    Source,
    /// Another terminal window that takes drops.
    Target,
    /// A Huterm window that takes no drop, or a failed native query.
    NoTarget,
    /// Outside every Huterm window; `tear_off` when a release opens one.
    Outside {
        tear_off: bool,
    },
}

/// The preview a drag at `spot` shows; `None` without a drag.
pub(super) fn preview_mode(
    spot: Option<Spot>,
    enabled: bool,
    supported: bool,
) -> PreviewMode {
    if !enabled || !supported {
        return PreviewMode::None;
    }
    match spot {
        None | Some(Spot::Source) => PreviewMode::None,
        Some(Spot::Target) => PreviewMode::Chip,
        Some(Spot::Outside { tear_off: true }) => PreviewMode::Window,
        Some(Spot::Outside { tear_off: false } | Spot::NoTarget) => {
            PreviewMode::Dimmed
        }
    }
}

/// The miniature window's size, without its shadow margin.
pub(super) const MINIATURE: gpui::Size<f64> = gpui::Size {
    width: 176.0,
    height: 112.0,
};
/// Room around the miniature for the shadow its view draws, since the
/// native shadow is off.
const SHADOW_MARGIN: f64 = 16.0;
/// The chip's offset from the pointer, as the in-window chip keeps.
const CHIP_OFFSET: f64 = 12.0;
/// The chip size assumed until the preview view has measured one.
const CHIP_ESTIMATE: gpui::Size<f64> = gpui::Size {
    width: 120.0,
    height: 22.0,
};

fn margin(mode: PreviewMode) -> f64 {
    if mode == PreviewMode::Window {
        SHADOW_MARGIN
    } else {
        0.0
    }
}

/// The preview's native frame for content of `content` size at the screen
/// `pointer`, both in `AppKit` screen coordinates (origin bottom-left, y
/// up). The miniature and the dimmed chip centre on the pointer. The chip
/// sits `CHIP_OFFSET` below and right of it, flipping across each edge of
/// `visible` (the pointer's screen's visible frame) it would overflow, then
/// clamping inside it.
pub(super) fn preview_frame(
    mode: PreviewMode,
    pointer: gpui::Point<f64>,
    content: gpui::Size<f64>,
    visible: Option<Bounds<f64>>,
) -> Bounds<f64> {
    let origin = if mode == PreviewMode::Chip {
        let mut x = pointer.x + CHIP_OFFSET;
        let mut y = pointer.y - CHIP_OFFSET - content.height;
        if let Some(visible) = visible {
            let (left, bottom) = (visible.origin.x, visible.origin.y);
            let right = left + visible.size.width;
            let top = bottom + visible.size.height;
            if x + content.width > right {
                x = pointer.x - CHIP_OFFSET - content.width;
            }
            if y < bottom {
                y = pointer.y + CHIP_OFFSET;
            }
            x = x.clamp(left, (right - content.width).max(left));
            y = y.clamp(bottom, (top - content.height).max(bottom));
        }
        point(x, y)
    } else {
        point(
            pointer.x - content.width / 2.0,
            pointer.y - content.height / 2.0,
        )
    };
    let margin = margin(mode);
    Bounds::new(
        point(origin.x - margin, origin.y - margin),
        size(content.width + margin * 2.0, content.height + margin * 2.0),
    )
}

/// The preview window of the current drag, if one is open.
#[derive(Default)]
pub(super) struct DragPreview {
    handle: Option<WindowHandle<DragPreviewView>>,
    /// The native window number, which the locator searches below.
    number: Option<u64>,
    /// The mode the view draws.
    mode: PreviewMode,
    /// Whether the native window is on screen, read back after each show.
    visible: bool,
    /// The drag generation whose preview failed to open; that drag draws
    /// the in-window cues instead.
    failed: Option<u64>,
    scheduled: bool,
    /// Orders native updates; only the latest one's readback counts.
    sequence: u64,
    #[cfg(test)]
    pub(super) hooks: Hooks,
}

/// Window tests have no `AppKit` window: these stand in for the platform
/// and its readback.
#[cfg(test)]
#[derive(Default)]
pub(super) struct Hooks {
    pub(super) supported: bool,
    pub(super) fail_open: bool,
    /// The native show succeeds, so the preview counts as visible.
    pub(super) visible: bool,
}

impl DragPreview {
    /// The preview's native number, for the locator to search below.
    pub(super) fn below(&self) -> Option<u64> {
        self.handle.and(self.number)
    }

    pub(super) fn window_id(&self) -> Option<gpui::WindowId> {
        self.handle.map(|handle| handle.window_id())
    }

    pub(super) fn mode(&self) -> PreviewMode {
        if self.handle.is_some() {
            self.mode
        } else {
            PreviewMode::None
        }
    }

    /// The preview is natively on screen, so in-window cues give way.
    pub(super) fn visible(&self) -> bool {
        self.handle.is_some() && self.visible && self.mode != PreviewMode::None
    }
}

/// Whether this platform has a floating preview. Window tests choose the
/// answer on every host.
fn supported(cx: &App) -> bool {
    #[cfg(test)]
    return cx.global::<Desktop>().cross.preview.hooks.supported;
    #[cfg(not(test))]
    {
        let _ = cx;
        cfg!(target_os = "macos")
    }
}

/// The preview is natively visible: the source draws no tear-off pills.
pub(super) fn visible(cx: &App) -> bool {
    cx.global::<Desktop>().cross.preview.visible()
}

/// The preview draws the chip on screen, so a destination draws none.
pub(super) fn draws_chip(cx: &App) -> bool {
    let preview = &cx.global::<Desktop>().cross.preview;
    preview.visible() && preview.mode == PreviewMode::Chip
}

/// Forgets a preview window that closed without [`close`].
pub(super) fn closed(id: gpui::WindowId, cx: &mut App) {
    let preview = &mut cx.global_mut::<Desktop>().cross.preview;
    if preview.window_id() == Some(id) {
        preview.handle = None;
        preview.number = None;
        preview.visible = false;
        preview.mode = PreviewMode::None;
    }
}

/// Brings the preview in line with the drag record on a later turn.
pub(super) fn sync(cx: &mut App) {
    let preview = &mut cx.global_mut::<Desktop>().cross.preview;
    if preview.scheduled {
        return;
    }
    preview.scheduled = true;
    cx.defer(|cx| {
        cx.global_mut::<Desktop>().cross.preview.scheduled = false;
        sync_now(cx);
    });
}

struct Wanted {
    mode: PreviewMode,
    generation: u64,
    title: String,
    colors: TabColors,
    screen: Option<gpui::Point<f64>>,
}

fn wanted(cx: &App) -> Option<Wanted> {
    let desktop = cx.global::<Desktop>();
    let drag: &Drag = desktop.cross.drag.as_ref()?;
    let source = cross_window::handle(cx, drag.payload.source)?
        .downcast::<WorkspaceView>()?
        .read(cx)
        .ok()?;
    let spot = match drag.located {
        Located::Window(id, _) if id == drag.payload.source => Spot::Source,
        Located::Window(id, _) => {
            let eligible = cross_window::handle(cx, id)
                .and_then(|handle| handle.downcast::<WorkspaceView>())
                .and_then(|handle| handle.read(cx).ok())
                .is_some_and(|view| view.cross_eligible(cx));
            if eligible {
                Spot::Target
            } else {
                Spot::NoTarget
            }
        }
        Located::Outside => Spot::Outside {
            tear_off: source.tears_off(&drag.payload),
        },
        Located::Blocked => Spot::NoTarget,
    };
    Some(Wanted {
        mode: preview_mode(
            Some(spot),
            source.config.window.macos_drag_preview,
            supported(cx),
        ),
        generation: drag.generation(),
        title: drag.payload.title.clone(),
        colors: source.floating_colors(),
        screen: drag.screen,
    })
}

fn sync_now(cx: &mut App) {
    let wanted = wanted(cx);
    let was_visible = visible(cx);
    let was_mode = cx.global::<Desktop>().cross.preview.mode();
    match wanted {
        None => close(cx),
        Some(wanted) if wanted.mode == PreviewMode::None => hide(cx),
        Some(wanted) => show(&wanted, cx),
    }
    if visible(cx) != was_visible
        || cx.global::<Desktop>().cross.preview.mode() != was_mode
    {
        cx.refresh_windows();
    }
}

fn close(cx: &mut App) {
    let preview = &mut cx.global_mut::<Desktop>().cross.preview;
    preview.failed = None;
    preview.visible = false;
    preview.mode = PreviewMode::None;
    preview.number = None;
    preview.sequence = preview.sequence.wrapping_add(1);
    if let Some(handle) = preview.handle.take() {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

/// Keeps the window for the rest of the drag but orders it out, even
/// while an earlier show is still queued.
fn hide(cx: &mut App) {
    let preview = &mut cx.global_mut::<Desktop>().cross.preview;
    let shown = preview.mode != PreviewMode::None || preview.visible;
    preview.mode = PreviewMode::None;
    preview.visible = false;
    if preview.handle.is_none() || !shown {
        return;
    }
    preview.sequence = preview.sequence.wrapping_add(1);
    let (sequence, number) = (preview.sequence, preview.number);
    if let Some(number) = number {
        apply_native(sequence, number, None, cx);
    }
}

fn show(wanted: &Wanted, cx: &mut App) {
    let preview = &cx.global::<Desktop>().cross.preview;
    if preview.failed == Some(wanted.generation) {
        return;
    }
    let handle = if let Some(handle) = preview.handle {
        handle
    } else if let Some(handle) = open(wanted, cx) {
        handle
    } else {
        cx.global_mut::<Desktop>().cross.preview.failed =
            Some(wanted.generation);
        return;
    };
    let measured = handle
        .update(cx, |view, _, cx| {
            // A theme reload mid-drag shows with the next mode or title.
            view.colors = wanted.colors;
            if view.mode != wanted.mode || view.title != wanted.title {
                view.mode = wanted.mode;
                view.title.clone_from(&wanted.title);
                cx.notify();
            }
            view.measured
                .borrow()
                .as_ref()
                .filter(|(title, _)| *title == wanted.title)
                .map(|(_, size)| *size)
        })
        .ok()
        .flatten();
    let preview = &mut cx.global_mut::<Desktop>().cross.preview;
    preview.mode = wanted.mode;
    preview.sequence = preview.sequence.wrapping_add(1);
    let sequence = preview.sequence;
    #[cfg(test)]
    if preview.number.is_none() {
        preview.visible = preview.hooks.visible;
        return;
    }
    let (Some(number), Some(pointer)) = (preview.number, wanted.screen) else {
        return;
    };
    let content = if wanted.mode == PreviewMode::Window {
        MINIATURE
    } else {
        measured.map_or(CHIP_ESTIMATE, |size| {
            gpui::size(
                f64::from(f32::from(size.width)),
                f64::from(f32::from(size.height)),
            )
        })
    };
    let frame = preview_frame(
        wanted.mode,
        pointer,
        content,
        native_visible_frame(pointer),
    );
    apply_native(sequence, number, Some(frame), cx);
}

fn open(
    wanted: &Wanted,
    cx: &mut App,
) -> Option<WindowHandle<DragPreviewView>> {
    #[cfg(test)]
    if cx.global::<Desktop>().cross.preview.hooks.fail_open {
        eprintln!("Drag preview: injected open failure");
        return None;
    }
    let extent = MINIATURE.width.max(MINIATURE.height) + SHADOW_MARGIN * 2.0;
    #[expect(clippy::cast_possible_truncation, reason = "a small point size")]
    let extent = px(extent as f32);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.0), px(0.0)),
            size(extent, extent),
        ))),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        inactive_frame_interval: None,
        window_background: WindowBackgroundAppearance::Transparent,
        ..WindowOptions::default()
    };
    let (mode, title, colors) =
        (wanted.mode, wanted.title.clone(), wanted.colors);
    let handle = match cx.open_window(options, move |_, cx| {
        cx.new(|_| DragPreviewView {
            mode,
            title,
            colors,
            measured: Rc::new(RefCell::new(None)),
        })
    }) {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("Drag preview: {error:#}");
            return None;
        }
    };
    let number = handle
        .update(cx, |_, window, _| window_locator::identity(window))
        .ok()
        .flatten();
    let preview = &mut cx.global_mut::<Desktop>().cross.preview;
    preview.handle = Some(handle);
    preview.number = number;
    preview.visible = false;
    if let Some(number) = number {
        configure_native(number, cx);
    }
    Some(handle)
}

/// Styles the fresh native window on a later turn, outside GPUI borrows:
/// `AppKit` reports the style mask's resize synchronously.
fn configure_native(number: u64, cx: &mut App) {
    #[cfg(target_os = "macos")]
    cx.spawn(async move |_| {
        if let Err(error) = native::configure(number) {
            eprintln!("Drag preview style: {error:#}");
        }
    })
    .detach();
    #[cfg(not(target_os = "macos"))]
    let _ = (number, cx);
}

/// Moves and shows the native window at `frame`, or hides it, on a later
/// turn outside GPUI borrows, then records whether it is on screen. A
/// newer update queued meanwhile supersedes this one.
fn apply_native(
    sequence: u64,
    number: u64,
    frame: Option<Bounds<f64>>,
    cx: &mut App,
) {
    #[cfg(target_os = "macos")]
    cx.spawn(async move |cx| {
        let current = cx.update(|cx| {
            let preview = &cx.global::<Desktop>().cross.preview;
            preview.sequence == sequence && preview.number == Some(number)
        });
        if !current {
            return;
        }
        let shown = match native::apply(number, frame) {
            Ok(shown) => shown,
            Err(error) => {
                eprintln!("Drag preview: {error:#}");
                false
            }
        };
        cx.update(|cx| {
            let preview = &mut cx.global_mut::<Desktop>().cross.preview;
            if preview.sequence != sequence || preview.number != Some(number) {
                return;
            }
            if preview.visible != shown {
                preview.visible = shown;
                cx.refresh_windows();
            }
        });
    })
    .detach();
    #[cfg(not(target_os = "macos"))]
    let _ = (sequence, number, frame, cx);
}

fn native_visible_frame(pointer: gpui::Point<f64>) -> Option<Bounds<f64>> {
    #[cfg(target_os = "macos")]
    return native::visible_frame(pointer);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pointer;
        None
    }
}

/// Smoke fields for the preview: its mode everywhere, and on macOS the
/// native readback of its visibility and frame and the key window.
pub(super) fn smoke_state(cx: &App) -> String {
    let preview = &cx.global::<Desktop>().cross.preview;
    let mode = preview.mode().label();
    #[cfg(target_os = "macos")]
    {
        let (shown, frame) =
            preview.below().and_then(native::readback).map_or_else(
                || (false, "none".to_owned()),
                |(shown, frame)| {
                    (
                        shown,
                        format!(
                            "{},{},{},{}",
                            frame.origin.x,
                            frame.origin.y,
                            frame.size.width,
                            frame.size.height
                        ),
                    )
                },
            );
        format!(
            "preview={mode} preview_visible={shown} preview_frame={frame} key_window={}\n",
            native::key_window().unwrap_or(0)
        )
    }
    #[cfg(not(target_os = "macos"))]
    format!("preview={mode}\n")
}

/// The preview window's root: the title chip, or the miniature window.
pub(super) struct DragPreviewView {
    mode: PreviewMode,
    title: String,
    colors: TabColors,
    measured: Measured,
}

/// The chip as last measured, with the title it was measured for.
type Measured = Rc<RefCell<Option<(String, gpui::Size<Pixels>)>>>;

impl Render for DragPreviewView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let typography = Typography::of(cx);
        typography.apply(window);
        let root = div()
            .size_full()
            .relative()
            .font_family(typography.family)
            .text_size(tab_bar::TAB_TEXT_SIZE)
            .text_color(self.colors.foreground);
        match self.mode {
            PreviewMode::None => root,
            PreviewMode::Window => root.child(self.miniature()),
            PreviewMode::Chip | PreviewMode::Dimmed => root.child(self.chip()),
        }
    }
}

impl DragPreviewView {
    /// The in-window chip, measured as drawn. A new size syncs the native
    /// frame again.
    fn chip(&self) -> gpui::AnyElement {
        let colors = self.colors;
        let title = self.title.clone();
        let opacity = if self.mode == PreviewMode::Dimmed {
            0.45
        } else {
            0.9
        };
        let measured = Rc::clone(&self.measured);
        gpui::canvas(
            move |_, window, cx| {
                let mut chip = drag_chip(&title, colors, px(240.0))
                    .opacity(opacity)
                    .into_any_element();
                let size = chip.layout_as_root(
                    gpui::size(
                        gpui::AvailableSpace::MaxContent,
                        gpui::AvailableSpace::MaxContent,
                    ),
                    window,
                    cx,
                );
                chip.prepaint_at(point(px(0.0), px(0.0)), window, cx);
                let changed =
                    measured.replace(Some((title.clone(), size))).is_none_or(
                        |(old, old_size)| old != title || old_size != size,
                    );
                if changed {
                    sync(cx);
                }
                chip
            },
            |_, mut chip, window, cx| chip.paint(window, cx),
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }

    /// Traffic lights, a bar whose only tab is the dragged title, and an
    /// empty terminal inside an accent border.
    fn miniature(&self) -> gpui::AnyElement {
        let colors = self.colors;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "small point sizes"
        )]
        let (margin, width, height) = (
            px(SHADOW_MARGIN as f32),
            px(MINIATURE.width as f32),
            px(MINIATURE.height as f32),
        );
        let light = |color: u32| {
            div().size(px(9.0)).rounded_full().bg(gpui::rgb(color))
        };
        div()
            .debug_selector(|| "drag-preview-miniature".to_owned())
            .absolute()
            .left(margin)
            .top(margin)
            .w(width)
            .h(height)
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(7.0))
            .border_1()
            .border_color(colors.accent)
            .bg(colors.terminal)
            .shadow(vec![gpui::BoxShadow {
                color: gpui::black().opacity(0.45),
                offset: point(px(0.0), px(6.0)),
                blur_radius: px(12.0),
                spread_radius: px(0.0),
                inset: false,
            }])
            .child(
                div()
                    .h(px(22.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .px(px(8.0))
                    .bg(colors.bar)
                    .border_b_1()
                    .border_color(colors.border)
                    .child(light(0x00ff_5f57))
                    .child(light(0x00fe_bc2e))
                    .child(light(0x0028_c840).mr(px(6.0)))
                    .child(
                        div()
                            .h(px(16.0))
                            .min_w_0()
                            .flex()
                            .items_center()
                            .px(px(10.0))
                            .rounded(px(6.0))
                            .bg(colors.active)
                            .text_color(colors.foreground)
                            .text_size(huterm_ui::typography::ui(11.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(self.title.clone()),
                    ),
            )
            .into_any_element()
    }
}

/// The drag title chip that the in-window drags and the preview draw.
pub(super) fn drag_chip(
    title: &str,
    colors: TabColors,
    max_width: Pixels,
) -> gpui::Div {
    div()
        .debug_selector(|| "drag-title-chip".to_owned())
        .max_w(max_width)
        .overflow_hidden()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(6.0))
        .bg(colors.active)
        .border_1()
        .border_color(colors.border)
        .text_color(colors.foreground)
        .whitespace_nowrap()
        .child(title.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mode_follows_the_spot_the_setting_and_the_platform() {
        for (spot, mode) in [
            (None, PreviewMode::None),
            (Some(Spot::Source), PreviewMode::None),
            (Some(Spot::Target), PreviewMode::Chip),
            (Some(Spot::NoTarget), PreviewMode::Dimmed),
            (Some(Spot::Outside { tear_off: true }), PreviewMode::Window),
            (Some(Spot::Outside { tear_off: false }), PreviewMode::Dimmed),
        ] {
            assert_eq!(preview_mode(spot, true, true), mode, "{spot:?}");
            assert_eq!(
                preview_mode(spot, false, true),
                PreviewMode::None,
                "setting off at {spot:?}"
            );
            assert_eq!(
                preview_mode(spot, true, false),
                PreviewMode::None,
                "no platform preview at {spot:?}"
            );
        }
    }

    #[test]
    fn the_miniature_and_dimmed_chip_centre_on_the_pointer() {
        let pointer = point(500.0, 300.0);
        let frame =
            preview_frame(PreviewMode::Window, pointer, MINIATURE, None);
        assert_eq!(frame, Bounds::new(point(396.0, 228.0), size(208.0, 144.0)));
        let chip = size(80.0, 22.0);
        // Centring ignores the screen edge: the preview stays on the pointer.
        let visible = Some(Bounds::new(point(0.0, 0.0), size(520.0, 800.0)));
        assert_eq!(
            preview_frame(PreviewMode::Dimmed, pointer, chip, visible),
            Bounds::new(point(460.0, 289.0), chip)
        );
    }

    #[test]
    fn the_chip_sits_below_right_and_flips_at_each_screen_edge() {
        let chip = size(80.0, 22.0);
        let visible = Bounds::new(point(0.0, 0.0), size(1000.0, 800.0));
        let frame = |x: f64, y: f64| {
            preview_frame(PreviewMode::Chip, point(x, y), chip, Some(visible))
                .origin
        };
        assert_eq!(frame(500.0, 400.0), point(512.0, 366.0));
        assert_eq!(frame(950.0, 400.0), point(858.0, 366.0), "right edge");
        assert_eq!(frame(500.0, 20.0), point(512.0, 32.0), "bottom edge");
        assert_eq!(frame(950.0, 20.0), point(858.0, 32.0), "corner");
        assert_eq!(frame(500.0, 795.0), point(512.0, 761.0));
        assert_eq!(
            preview_frame(PreviewMode::Chip, point(5.0, 5.0), chip, None)
                .origin,
            point(17.0, -29.0),
            "no screen leaves the chip unclamped"
        );
    }
}
