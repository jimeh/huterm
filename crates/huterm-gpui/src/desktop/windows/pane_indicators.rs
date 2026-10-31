//! The shape that marks a tab syncing input or showing a zoomed pane,
//! from `[panes.sync_indicator]` and `[panes.zoom_indicator]`: which look
//! applies, the radius of each corner of the pane area, the lines each
//! style draws, and the paths a frame paints. Lines never overlap, since
//! GPUI blends overlapping path coverage darker; only `glow`'s soft bands
//! blend where they meet, as light would. The indicator carries no
//! listeners, cursor, hover style, or occlusion, so it never takes input.
//! See `docs/plans/pane-indicators.md`.

// Layout feeds these positions, so a stale index must not panic.
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    AnyElement, App, Bounds, Hsla, IntoElement, ParentElement, PathBuilder,
    Pixels, Point, Styled, Window, canvas, div, point, px, size,
};
use huterm_config::{
    IndicatorLook, IndicatorStyle, LinePattern, PaneTitleBarMode,
    PaneTitleBarPosition, PanesConfig,
};
use huterm_protocol::{PaneId, SplitAxis};

use super::client_frame::FRAME_RADIUS;
use super::pane_geometry::{Divider, PaneRect, TabPanes};
use super::tab_bar::{
    self, TabColors, icon_element, pane_focus_cue, sync_mark,
};
use super::{TabView, WorkspaceView, input_sync, pane_focus};
use crate::assets::Icon;
use crate::keymap::Platform;

/// How far apart two edges may lie and still count as one.
const EDGE_TOLERANCE: f32 = 0.5;

/// How deep a glow reaches inside the edge, in multiples of the look's
/// width.
const GLOW_DEPTH: f32 = 4.0;
/// The opacity of a glow's outermost stroke; inner strokes fall off from
/// it.
const GLOW_PEAK: f32 = 0.6;

/// The share of the look's opacity a `title_bar` tint draws at.
pub(super) const TINT_SHARE: f32 = 0.25;
/// The opacity of a sync mark while a zoom shows one pane, so typing
/// reaches only that pane and sync has no visible effect.
pub(super) const DIMMED_SYNC_MARK: f32 = 0.4;

/// A state chip's distance from its pane's edges.
const CHIP_INSET: f32 = 4.0;
/// The space between a chip's border and its marks, and between marks.
const CHIP_PADDING: f32 = 2.0;
const CHIP_ICON: f32 = 12.0;

/// The most vertices one path's tessellation may hold. lyon indexes them
/// with `u16`, and this leaves room for the estimates below to run short.
const VERTEX_BUDGET: usize = 16_384;

/// A tab state an indicator marks.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum IndicatorState {
    /// Typing reaches every pane the tab shows.
    Sync,
    /// This window shows one pane of the tab across the whole tab.
    Zoom,
}

impl IndicatorState {
    /// The state's colour: the tab accent for sync, ANSI magenta for zoom.
    pub(super) fn color(self, colors: TabColors) -> Hsla {
        match self {
            Self::Sync => colors.accent,
            Self::Zoom => colors.zoom,
        }
    }
}

/// The look a frame draws for its active tab.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ActiveLook {
    pub(super) state: IndicatorState,
    pub(super) look: IndicatorLook,
}

/// Which state's look applies. `syncing` holds while typing reaches the
/// tab's panes through input sync, and `zoomed` while this window shows a
/// zoomed pane. With both, typing reaches only the zoomed pane, so the zoom
/// look wins. `off` still resolves: it draws no shape, but the marks stay.
pub(super) fn resolve_look(
    syncing: bool,
    zoomed: bool,
    panes: &PanesConfig,
) -> Option<ActiveLook> {
    if zoomed {
        Some(ActiveLook {
            state: IndicatorState::Zoom,
            look: panes.zoom_indicator.into(),
        })
    } else if syncing {
        Some(ActiveLook {
            state: IndicatorState::Sync,
            look: panes.sync_indicator.into(),
        })
    } else {
        None
    }
}

/// Whether `style` paints paths: every style but `title_bar` and `off`.
fn paints(style: IndicatorStyle) -> bool {
    matches!(
        style,
        IndicatorStyle::Outline
            | IndicatorStyle::Dividers
            | IndicatorStyle::Glow
            | IndicatorStyle::Edge
    )
}

/// Whether `active` draws the tab's dividers in place of their plain
/// lines: the sync look's `outline`, `dividers`, and `glow`.
pub(super) fn draws_dividers(active: Option<ActiveLook>) -> bool {
    active.is_some_and(|active| {
        active.state == IndicatorState::Sync
            && matches!(
                active.look.style,
                IndicatorStyle::Outline
                    | IndicatorStyle::Dividers
                    | IndicatorStyle::Glow
            )
    })
}

/// The radius of each corner of the pane area.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Corners {
    pub(super) top_left: Pixels,
    pub(super) top_right: Pixels,
    pub(super) bottom_right: Pixels,
    pub(super) bottom_left: Pixels,
}

/// The radii of the window's own top and bottom corners.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct WindowRadii {
    pub(super) top: Pixels,
    pub(super) bottom: Pixels,
}

/// Where the window's corner radii came from.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum RadiusSource {
    /// `NSWindow`'s private corner radius selectors.
    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "only macOS reads AppKit's radii")
    )]
    AppKit,
    /// The macOS version table, when those selectors are missing.
    #[cfg_attr(
        all(not(target_os = "macos"), not(test)),
        expect(dead_code, reason = "only macOS reads AppKit's radii")
    )]
    Table,
    /// No native window to ask, as off macOS and in GPUI tests.
    #[default]
    None,
}

impl RadiusSource {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::AppKit => "appkit",
            Self::Table => "table",
            Self::None => "none",
        }
    }
}

/// The corner radii a macOS window reported when it opened.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct NativeRadii {
    pub(super) radii: WindowRadii,
    pub(super) source: RadiusSource,
}

/// The radius macOS `major` gives a titled window's corners, for a release
/// whose `NSWindow` does not answer the private selectors.
#[cfg_attr(
    all(not(target_os = "macos"), not(test)),
    expect(dead_code, reason = "only macOS reads AppKit's radii")
)]
pub(super) fn table_radius(major: u32) -> Pixels {
    if major >= 26 { px(16.0) } else { px(10.0) }
}

/// Reads `window`'s corner radii from `AppKit`, before a Quake attach
/// strips its style. A window without an `AppKit` handle, as in GPUI
/// tests, has none and logs nothing.
#[cfg(target_os = "macos")]
pub(super) fn read_native_radii(window: &Window) -> NativeRadii {
    match crate::native_titlebar::corner_radii(window) {
        Err(_) => NativeRadii::default(),
        #[expect(
            clippy::cast_possible_truncation,
            reason = "point-sized radii"
        )]
        Ok(Some((top, bottom))) => NativeRadii {
            radii: WindowRadii {
                top: px(top as f32),
                bottom: px(bottom as f32),
            },
            source: RadiusSource::AppKit,
        },
        Ok(None) => {
            let major = crate::native_titlebar::macos_major_version();
            let radius = table_radius(major);
            eprintln!(
                "Window corners: AppKit reports no corner radius; using {} \
                 points for macOS {major}",
                f32::from(radius)
            );
            NativeRadii {
                radii: WindowRadii {
                    top: radius,
                    bottom: radius,
                },
                source: RadiusSource::Table,
            }
        }
    }
}

/// The window's corner radii this frame. They are zero while the chrome
/// hides (fullscreen and a Quake presentation), on Linux with server
/// decorations, and wherever a Linux client frame is not decorated; a
/// decorated Linux frame rounds only its top corners.
pub(super) fn window_radii(
    chrome_hidden: bool,
    platform: Platform,
    native: NativeRadii,
    decorated: bool,
) -> WindowRadii {
    match platform {
        _ if chrome_hidden => WindowRadii::default(),
        Platform::MacOs => native.radii,
        Platform::Linux if decorated => WindowRadii {
            top: FRAME_RADIUS,
            bottom: px(0.0),
        },
        Platform::Linux => WindowRadii::default(),
    }
}

/// The terminal corner Huterm rounds itself beside a vertical tab bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TerminalCorner {
    pub(super) at: Point<Pixels>,
    pub(super) radius: Pixels,
}

fn near(a: Point<Pixels>, b: Point<Pixels>) -> bool {
    (a.x - b.x).abs() <= px(EDGE_TOLERANCE)
        && (a.y - b.y).abs() <= px(EDGE_TOLERANCE)
}

/// Each corner's radius of `area`, the pane area inside the window's
/// `content`: a corner on a window corner takes that corner's radius, the
/// corner Huterm rounds beside a vertical tab bar takes its radius, and
/// every other corner is square. No radius exceeds half the area's
/// shorter side.
pub(super) fn area_corners(
    content: Bounds<Pixels>,
    area: Bounds<Pixels>,
    window: WindowRadii,
    terminal: Option<TerminalCorner>,
) -> Corners {
    let limit = (area.size.width.min(area.size.height) / 2.0).max(px(0.0));
    let radius = |corner: Point<Pixels>, window_corner, window_radius| {
        let radius = if near(corner, window_corner) {
            window_radius
        } else if let Some(terminal) =
            terminal.filter(|terminal| near(corner, terminal.at))
        {
            terminal.radius
        } else {
            px(0.0)
        };
        radius.max(px(0.0)).min(limit)
    };
    Corners {
        top_left: radius(area.origin, content.origin, window.top),
        top_right: radius(area.top_right(), content.top_right(), window.top),
        bottom_right: radius(
            area.bottom_right(),
            content.bottom_right(),
            window.bottom,
        ),
        bottom_left: radius(
            area.bottom_left(),
            content.bottom_left(),
            window.bottom,
        ),
    }
}

/// A point in window coordinates, as plain floats for the arithmetic.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pt {
    x: f32,
    y: f32,
}

/// Shorthand for a [`Pt`].
const fn pt(x: f32, y: f32) -> Pt {
    Pt { x, y }
}

impl Pt {
    fn pixels(self) -> Point<Pixels> {
        point(px(self.x), px(self.y))
    }
}

/// A rectangle by its edges.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl Rect {
    fn of(bounds: Bounds<Pixels>) -> Self {
        Self {
            left: f32::from(bounds.origin.x),
            top: f32::from(bounds.origin.y),
            right: f32::from(bounds.right()),
            bottom: f32::from(bounds.bottom()),
        }
    }

    fn bounds(self) -> Bounds<Pixels> {
        Bounds::new(
            point(px(self.left), px(self.top)),
            size(
                px((self.right - self.left).max(0.0)),
                px((self.bottom - self.top).max(0.0)),
            ),
        )
    }

    fn inset(self, by: f32) -> Self {
        Self {
            left: self.left + by,
            top: self.top + by,
            right: self.right - by,
            bottom: self.bottom - by,
        }
    }

    fn empty(self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    /// Whether the two overlap or touch, within the edge tolerance.
    fn meets(self, other: Self) -> bool {
        self.left <= other.right + EDGE_TOLERANCE
            && other.left <= self.right + EDGE_TOLERANCE
            && self.top <= other.bottom + EDGE_TOLERANCE
            && other.top <= self.bottom + EDGE_TOLERANCE
    }

    fn horizontal(self) -> bool {
        self.right - self.left >= self.bottom - self.top
    }
}

/// Rounds a coordinate to whole physical pixels.
fn snap(value: f32, scale: f32) -> f32 {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    (value * scale).round() / scale
}

/// A line width of `points`, rounded to whole physical pixels and never
/// thinner than one.
fn line_width(points: u8, scale: f32) -> f32 {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    (f32::from(points) * scale).round().max(1.0) / scale
}

/// One piece of a stroke's centreline.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Piece {
    Line {
        from: Pt,
        to: Pt,
    },
    /// A circular arc around `center` from angle `start` through `sweep`
    /// radians, clockwise on screen for a positive sweep.
    Arc {
        center: Pt,
        radius: f32,
        start: f32,
        sweep: f32,
    },
}

impl Piece {
    fn length(self) -> f32 {
        match self {
            Self::Line { from, to } => (to.x - from.x).hypot(to.y - from.y),
            Self::Arc { radius, sweep, .. } => radius * sweep.abs(),
        }
    }

    /// The point `distance` along the piece.
    fn at(self, distance: f32) -> Pt {
        let length = self.length();
        let t = if length > 0.0 {
            (distance / length).clamp(0.0, 1.0)
        } else {
            0.0
        };
        match self {
            Self::Line { from, to } => Pt {
                x: from.x + (to.x - from.x) * t,
                y: from.y + (to.y - from.y) * t,
            },
            Self::Arc {
                center,
                radius,
                start,
                sweep,
            } => {
                let angle = start + sweep * t;
                Pt {
                    x: center.x + radius * angle.cos(),
                    y: center.y + radius * angle.sin(),
                }
            }
        }
    }

    /// Points along the piece from `from` to `to` (distances along it),
    /// after the start point, which the caller already holds. An arc
    /// steps at most 7.5 degrees, well inside a pixel of the true curve
    /// at the window radii.
    fn extend(self, from: f32, to: f32, out: &mut Vec<Pt>) {
        if let Self::Arc { radius, .. } = self
            && radius > 0.0
        {
            let angle = (to - from) / radius;
            let step = PI / 24.0;
            let steps = (angle.abs() / step).ceil().max(1.0);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "at most a quarter turn in 7.5-degree steps"
            )]
            let steps = steps as u32;
            for index in 1..=steps {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a handful of steps"
                )]
                let fraction = index as f32 / steps as f32;
                out.push(self.at(from + (to - from) * fraction));
            }
        } else {
            out.push(self.at(to));
        }
    }
}

/// A stroke's centreline: pieces joined end to end, `closed` when the last
/// ends where the first starts.
#[derive(Clone, Debug, PartialEq)]
struct Track {
    pieces: Vec<Piece>,
    closed: bool,
}

impl Track {
    fn line(from: Pt, to: Pt) -> Self {
        Self {
            pieces: vec![Piece::Line { from, to }],
            closed: false,
        }
    }

    /// The closed centreline around `rect` with each corner's `radii` (top
    /// left, top right, bottom right, bottom left), clockwise from the
    /// middle of the top edge, so a dash pattern's seam never lands on a
    /// corner.
    fn rounded(rect: Rect, radii: [f32; 4]) -> Self {
        let [top_left, top_right, bottom_right, bottom_left] = radii;
        let Rect {
            left,
            top,
            right,
            bottom,
        } = rect;
        let middle = pt(f32::midpoint(left, right), top);
        // Clockwise, each corner's radius, its arc's centre and starting
        // angle, and where the arc starts and ends.
        let corners = [
            (
                top_right,
                pt(right - top_right, top + top_right),
                -FRAC_PI_2,
                pt(right - top_right, top),
                pt(right, top + top_right),
            ),
            (
                bottom_right,
                pt(right - bottom_right, bottom - bottom_right),
                0.0,
                pt(right, bottom - bottom_right),
                pt(right - bottom_right, bottom),
            ),
            (
                bottom_left,
                pt(left + bottom_left, bottom - bottom_left),
                FRAC_PI_2,
                pt(left + bottom_left, bottom),
                pt(left, bottom - bottom_left),
            ),
            (
                top_left,
                pt(left + top_left, top + top_left),
                PI,
                pt(left, top + top_left),
                pt(left + top_left, top),
            ),
        ];
        let mut pieces = Vec::new();
        let mut at = middle;
        for (radius, center, start, from, to) in corners {
            if at != from {
                pieces.push(Piece::Line { from: at, to: from });
            }
            if radius > 0.0 {
                pieces.push(Piece::Arc {
                    center,
                    radius,
                    start,
                    sweep: FRAC_PI_2,
                });
            }
            at = to;
        }
        if at != middle {
            pieces.push(Piece::Line {
                from: at,
                to: middle,
            });
        }
        Self {
            pieces,
            closed: true,
        }
    }

    fn length(&self) -> f32 {
        self.pieces.iter().map(|piece| piece.length()).sum()
    }

    /// The point `distance` along the track.
    fn at(&self, distance: f32) -> Option<Pt> {
        let mut walked = 0.0;
        for piece in &self.pieces {
            let length = piece.length();
            if distance <= walked + length {
                return Some(piece.at(distance - walked));
            }
            walked += length;
        }
        self.pieces.last().map(|piece| piece.at(piece.length()))
    }

    /// The track's points from `from` to `to` along it.
    fn polyline(&self, from: f32, to: f32) -> Vec<Pt> {
        let mut points: Vec<Pt> = self.at(from).into_iter().collect();
        let mut walked = 0.0;
        for piece in &self.pieces {
            let length = piece.length();
            let (start, end) = (walked, walked + length);
            walked = end;
            if end <= from || start >= to {
                continue;
            }
            piece.extend(
                (from - start).max(0.0),
                (to - start).min(length),
                &mut points,
            );
        }
        points
    }
}

/// What a pattern turns a track into.
#[derive(Clone, Debug, PartialEq)]
enum Mark {
    /// A stroked run of points, closed around the whole track when solid.
    Run { points: Vec<Pt>, closed: bool },
    /// A filled round dot.
    Dot { center: Pt },
}

/// The dashes of a `dashed` line `width` thick: `3 × width` long with
/// `2 × width` gaps.
fn dash_lengths(width: f32) -> (f32, f32) {
    (width * 3.0, width * 2.0)
}

/// The distance between dot centres of a `dotted` line `width` thick.
fn dot_spacing(width: f32) -> f32 {
    width * 3.0
}

/// `track` drawn in `pattern` at `width`. A line shorter than one period
/// of its pattern draws solid.
fn pattern_marks(track: &Track, pattern: LinePattern, width: f32) -> Vec<Mark> {
    let length = track.length();
    let solid = || {
        vec![Mark::Run {
            points: track.polyline(0.0, length),
            closed: track.closed,
        }]
    };
    match pattern {
        LinePattern::Solid => solid(),
        LinePattern::Dashed => {
            let (dash, gap) = dash_lengths(width);
            if length < dash + gap {
                return solid();
            }
            dash_spans(length, dash, gap, track.closed)
                .into_iter()
                .map(|(start, end)| Mark::Run {
                    points: track.polyline(start, end),
                    closed: false,
                })
                .collect()
        }
        LinePattern::Dotted => {
            let spacing = dot_spacing(width);
            if length < spacing {
                return solid();
            }
            dot_positions(length, spacing, width, track.closed)
                .into_iter()
                .filter_map(|at| track.at(at))
                .map(|center| Mark::Dot { center })
                .collect()
        }
    }
}

/// The dashes along a line `length` long, as start and end distances, with
/// `dash` and `gap` scaled together so a whole number fit: barely on a long
/// line, by up to about a third on one only two dashes long. A
/// closed line's dashes run all the way round, so its seam's gap matches
/// the others. An open line starts and ends on a dash, so it never ends in
/// a sliver.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "a dash count along an on-screen line is small and positive"
)]
fn dash_spans(
    length: f32,
    dash: f32,
    gap: f32,
    closed: bool,
) -> Vec<(f32, f32)> {
    let period = dash + gap;
    let (dashes, fit) = if closed {
        let dashes = ((length / period).round() as usize).max(1);
        (dashes, length / (dashes as f32 * period))
    } else {
        let dashes = (((length + gap) / period).round() as usize).max(1);
        let span = dashes as f32 * dash + (dashes - 1) as f32 * gap;
        (dashes, length / span)
    };
    let (dash, period) = (dash * fit, period * fit);
    (0..dashes)
        .map(|index| {
            let start = period * index as f32;
            (start, (start + dash).min(length))
        })
        .collect()
}

/// Where along a line `length` long the dots of a `dotted` pattern
/// `width` thick sit, evenly spread at least `spacing` apart. A closed
/// line's dots run all the way round, so its seam keeps the rhythm. An
/// open line keeps its first and last dot a spacing, less half a dot, from
/// its ends: a divider ends at the edge of the line it meets, whose own dot
/// may sit right there, so a dot at the end would crowd it.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "a dot count along an on-screen line is small and positive"
)]
fn dot_positions(
    length: f32,
    spacing: f32,
    width: f32,
    closed: bool,
) -> Vec<f32> {
    if closed {
        let dots = ((length / spacing).floor() as usize).max(1);
        let step = length / dots as f32;
        return (0..dots).map(|index| step * index as f32).collect();
    }
    let inset = spacing - width / 2.0;
    let span = length - 2.0 * inset;
    if span < spacing {
        // Too short to keep both ends clear: one dot in the middle.
        return vec![length / 2.0];
    }
    let gaps = (span / spacing).floor() as usize;
    let step = span / gaps as f32;
    (0..=gaps)
        .map(|index| inset + step * index as f32)
        .collect()
}

/// The corners of a dot's polygon: enough that its edge stays within a
/// fraction of a pixel of a circle at `scale`.
fn dot_sides(diameter: f32, scale: f32) -> usize {
    let pixels = (diameter * scale.max(1.0) * PI / 1.5).ceil();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to a small positive count"
    )]
    let sides = pixels.clamp(8.0, 32.0) as usize;
    sides
}

fn dot_polygon(center: Pt, diameter: f32, sides: usize) -> Vec<Point<Pixels>> {
    let radius = diameter / 2.0;
    (0..sides)
        .map(|index| {
            #[expect(clippy::cast_precision_loss, reason = "at most 32 sides")]
            let angle = TAU * index as f32 / sides as f32;
            point(
                px(center.x + radius * angle.cos()),
                px(center.y + radius * angle.sin()),
            )
        })
        .collect()
}

/// One centreline the look strokes.
#[derive(Clone, Debug, PartialEq)]
struct Stroke {
    track: Track,
    width: f32,
    /// A share of the look's opacity.
    alpha: f32,
    pattern: LinePattern,
}

/// One region the look fills.
#[derive(Clone, Debug, PartialEq)]
struct Fill {
    points: Vec<Pt>,
    alpha: f32,
}

/// One divider as the tab's layout draws it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DividerLine {
    pub(super) axis: SplitAxis,
    /// The 1-point line between the panes.
    pub(super) line: Bounds<Pixels>,
}

impl From<&Divider> for DividerLine {
    fn from(divider: &Divider) -> Self {
        Self {
            axis: divider.axis,
            line: divider.line,
        }
    }
}

/// A divider as the look draws it: a band `width` across centred on its
/// line, from `start` to `end` along it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Band {
    vertical: bool,
    centre: f32,
    width: f32,
    start: f32,
    end: f32,
}

impl Band {
    fn rect(self) -> Rect {
        let (low, high) = (
            self.centre - self.width / 2.0,
            self.centre + self.width / 2.0,
        );
        if self.vertical {
            Rect {
                left: low,
                top: self.start,
                right: high,
                bottom: self.end,
            }
        } else {
            Rect {
                left: self.start,
                top: low,
                right: self.end,
                bottom: high,
            }
        }
    }

    fn track(self, offset: f32) -> Track {
        let at = |along: f32| {
            if self.vertical {
                Pt {
                    x: self.centre + offset,
                    y: along,
                }
            } else {
                Pt {
                    x: along,
                    y: self.centre + offset,
                }
            }
        };
        Track::line(at(self.start), at(self.end))
    }
}

/// The tab's dividers as bands `width` across, each centred on its line
/// and snapped to physical pixels at `scale`, trimmed so no two overlap:
/// an end on the pane area's edge stops `edge_inset` inside it, at the
/// outline's inner edge, and an end that meets another divider at a
/// T-junction stops at that divider's band.
fn trim_dividers(
    area: Rect,
    dividers: &[DividerLine],
    width: f32,
    edge_inset: f32,
    scale: f32,
) -> Vec<Band> {
    let bands: Vec<Band> = dividers
        .iter()
        .map(|divider| {
            let line = Rect::of(divider.line);
            let vertical = divider.axis == SplitAxis::Columns;
            let (middle, start, end) = if vertical {
                (f32::midpoint(line.left, line.right), line.top, line.bottom)
            } else {
                (f32::midpoint(line.top, line.bottom), line.left, line.right)
            };
            Band {
                vertical,
                centre: snap(middle - width / 2.0, scale) + width / 2.0,
                width,
                start,
                end,
            }
        })
        .collect();
    let trim = |band: &Band, end: f32, forward: bool| -> f32 {
        let (edge_low, edge_high) = if band.vertical {
            (area.top, area.bottom)
        } else {
            (area.left, area.right)
        };
        let edge = if forward { edge_high } else { edge_low };
        if (end - edge).abs() <= EDGE_TOLERANCE {
            return if forward {
                edge - edge_inset
            } else {
                edge + edge_inset
            };
        }
        // A perpendicular divider whose line this end touches.
        let line_middle = band.centre;
        bands
            .iter()
            .filter(|other| other.vertical != band.vertical)
            .find(|other| {
                let line_low = other.centre - 0.5;
                let line_high = other.centre + 0.5;
                end >= line_low - 0.5 - EDGE_TOLERANCE
                    && end <= line_high + 0.5 + EDGE_TOLERANCE
                    && line_middle >= other.start - EDGE_TOLERANCE
                    && line_middle <= other.end + EDGE_TOLERANCE
            })
            .map_or(end, |other| {
                if forward {
                    other.centre - other.width / 2.0
                } else {
                    other.centre + other.width / 2.0
                }
            })
    };
    bands
        .iter()
        .map(|band| Band {
            start: trim(band, band.start, false),
            end: trim(band, band.end, true),
            ..*band
        })
        .filter(|band| band.end > band.start)
        .collect()
}

/// What a frame's indicator paints, keyed by everything its paths depend
/// on. The colour and opacity stay out: `paint_path` takes them.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PathKey {
    pub(super) area: Bounds<Pixels>,
    pub(super) corners: Corners,
    pub(super) dividers: Vec<DividerLine>,
    pub(super) scale: f32,
    pub(super) style: IndicatorStyle,
    pub(super) width: u8,
    pub(super) pattern: LinePattern,
}

/// The shapes a key draws, before tessellation.
#[derive(Clone, Debug, Default, PartialEq)]
struct Shapes {
    strokes: Vec<Stroke>,
    fills: Vec<Fill>,
    /// What each drawn line covers: the outline's sides, the edge bar, and
    /// each divider's band, which the focus cue keeps clear of.
    lines: Vec<Bounds<Pixels>>,
    /// The divider segments drawn.
    dividers: Vec<Bounds<Pixels>>,
}

/// The x offset of a rounded corner of `radius` at `depth` below the edge
/// it rounds: how far in from the side the area starts there.
fn corner_offset(radius: f32, depth: f32) -> f32 {
    if radius <= 0.0 || depth >= radius {
        0.0
    } else {
        let rise = radius - depth;
        radius - (radius * radius - rise * rise).max(0.0).sqrt()
    }
}

fn shapes(key: &PathKey) -> Shapes {
    let area = Rect::of(key.area);
    let area = Rect {
        left: snap(area.left, key.scale),
        top: snap(area.top, key.scale),
        right: snap(area.right, key.scale),
        bottom: snap(area.bottom, key.scale),
    };
    let mut shapes = Shapes::default();
    if area.empty() {
        return shapes;
    }
    let radii = [
        key.corners.top_left,
        key.corners.top_right,
        key.corners.bottom_right,
        key.corners.bottom_left,
    ]
    .map(f32::from);
    match key.style {
        IndicatorStyle::Outline | IndicatorStyle::Dividers => {
            lines(key, area, radii, &mut shapes);
        }
        IndicatorStyle::Glow => glow(key, area, radii, &mut shapes),
        IndicatorStyle::Edge => edge(key, area, radii, &mut shapes),
        IndicatorStyle::TitleBar | IndicatorStyle::Off => {}
    }
    shapes
}

/// `radii` less `by`, for a line inset `by` from the area's edge.
fn inset_radii(radii: [f32; 4], by: f32) -> [f32; 4] {
    radii.map(|radius| (radius - by).max(0.0))
}

/// The `outline` style's line around `area` with its corner `radii`, and
/// the dividers in the same style; or the `dividers` style's alone.
fn lines(key: &PathKey, area: Rect, radii: [f32; 4], shapes: &mut Shapes) {
    let width = line_width(key.width, key.scale);
    let outline = key.style == IndicatorStyle::Outline;
    if outline {
        let rect = area.inset(width / 2.0);
        if !rect.empty() {
            shapes.strokes.push(Stroke {
                track: Track::rounded(rect, inset_radii(radii, width / 2.0)),
                width,
                alpha: 1.0,
                pattern: key.pattern,
            });
        }
        for side in [
            Rect {
                bottom: area.top + width,
                ..area
            },
            Rect {
                top: area.bottom - width,
                ..area
            },
            Rect {
                right: area.left + width,
                ..area
            },
            Rect {
                left: area.right - width,
                ..area
            },
        ] {
            shapes.lines.push(side.bounds());
        }
    }
    let inset = if outline { width } else { 0.0 };
    for band in trim_dividers(area, &key.dividers, width, inset, key.scale) {
        shapes.strokes.push(Stroke {
            track: band.track(0.0),
            width,
            alpha: 1.0,
            pattern: key.pattern,
        });
        shapes.lines.push(band.rect().bounds());
        shapes.dividers.push(band.rect().bounds());
    }
}

/// The `glow` style: stacked 1-point strokes inside `area`'s edge, `width
/// × 4` deep and fading inward, and each divider glowing on both sides.
/// It ignores the pattern.
fn glow(key: &PathKey, area: Rect, radii: [f32; 4], shapes: &mut Shapes) {
    let solid = |track, alpha| Stroke {
        track,
        width: 1.0,
        alpha,
        pattern: LinePattern::Solid,
    };
    let depth = (GLOW_DEPTH * f32::from(key.width)).round().max(1.0);
    let falloff = |step: f32| GLOW_PEAK * (1.0 - step / depth).powi(2);
    let mut step = 0.0;
    while step < depth {
        let rect = area.inset(step + 0.5);
        if rect.empty() {
            break;
        }
        shapes.strokes.push(solid(
            Track::rounded(rect, inset_radii(radii, step + 0.5)),
            falloff(step),
        ));
        step += 1.0;
    }
    // Each divider's own line at full strength, glowing on both sides as
    // the edge does inward.
    for band in trim_dividers(area, &key.dividers, 1.0, 1.0, key.scale) {
        shapes.strokes.push(solid(band.track(0.0), 1.0));
        let mut step = 0.0;
        while step < depth {
            for side in [-1.0, 1.0] {
                shapes.strokes.push(solid(
                    band.track(side * (step + 1.0)),
                    falloff(step),
                ));
            }
            step += 1.0;
        }
        shapes.dividers.push(band.rect().bounds());
    }
}

/// The `edge` style: one bar along `area`'s top, its ends following the
/// top corners' `radii`.
fn edge(key: &PathKey, area: Rect, radii: [f32; 4], shapes: &mut Shapes) {
    let width = line_width(key.width, key.scale);
    let [top_left, top_right, ..] = radii;
    let bar = Rect {
        bottom: area.top + width,
        ..area
    };
    shapes.lines.push(bar.bounds());
    if key.pattern == LinePattern::Solid {
        let samples = 8_u8;
        let mut left = Vec::new();
        let mut right = Vec::new();
        for index in 0..=samples {
            let depth = width * f32::from(index) / f32::from(samples);
            let y = area.top + depth;
            left.push(pt(area.left + corner_offset(top_left, depth), y));
            right.push(pt(area.right - corner_offset(top_right, depth), y));
        }
        right.reverse();
        left.extend(right);
        shapes.fills.push(Fill {
            points: left,
            alpha: 1.0,
        });
    } else {
        // A patterned bar keeps inside the corners' arcs.
        let y = area.top + width / 2.0;
        shapes.strokes.push(Stroke {
            track: Track::line(
                pt(area.left + top_left, y),
                pt(area.right - top_right, y),
            ),
            width,
            alpha: 1.0,
            pattern: key.pattern,
        });
    }
}

/// One tessellated path and its share of the look's opacity.
#[derive(Clone, Debug)]
pub(super) struct IndicatorPath {
    pub(super) path: gpui::Path<Pixels>,
    pub(super) alpha: f32,
}

/// The paths of one key and what its lines cover.
#[derive(Debug, Default)]
pub(super) struct Painted {
    pub(super) paths: Vec<IndicatorPath>,
    pub(super) lines: Vec<Bounds<Pixels>>,
    pub(super) dividers: Vec<Bounds<Pixels>>,
    /// Paths whose tessellation failed and were skipped.
    pub(super) failed: usize,
}

/// Whether a failed tessellation has been logged, so one bad geometry
/// does not log every frame.
static LOGGED_FAILURE: AtomicBool = AtomicBool::new(false);

/// Paths of one alpha and kind, filled up to the vertex budget at a time.
struct Chunk {
    alpha: f32,
    /// The stroke width, or `None` for fills.
    width: Option<f32>,
    builder: Option<PathBuilder>,
    vertices: usize,
}

impl Chunk {
    fn builder(&mut self) -> &mut PathBuilder {
        let width = self.width;
        self.builder.get_or_insert_with(|| {
            width.map_or_else(PathBuilder::fill, |width| {
                PathBuilder::stroke(px(width))
            })
        })
    }

    fn finish(&mut self, painted: &mut Painted) {
        self.vertices = 0;
        let Some(builder) = self.builder.take() else {
            return;
        };
        match builder.build() {
            Ok(path) => painted.paths.push(IndicatorPath {
                path,
                alpha: self.alpha,
            }),
            Err(error) => {
                painted.failed += 1;
                if !LOGGED_FAILURE.swap(true, Ordering::Relaxed) {
                    eprintln!("Pane indicator: skipped a path: {error}");
                }
            }
        }
    }

    /// Adds a shape of about `vertices`, starting a new path first when
    /// this one would pass the budget.
    fn add(
        &mut self,
        vertices: usize,
        painted: &mut Painted,
        draw: impl FnOnce(&mut PathBuilder),
    ) {
        if self.vertices > 0 && self.vertices + vertices > VERTEX_BUDGET {
            self.finish(painted);
        }
        self.vertices += vertices;
        draw(self.builder());
    }
}

/// Tessellates `key`'s shapes into paths, each under GPUI's vertex limit.
pub(super) fn paint(key: &PathKey) -> Painted {
    let shapes = shapes(key);
    let mut painted = Painted {
        lines: shapes.lines.clone(),
        dividers: shapes.dividers.clone(),
        ..Painted::default()
    };
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut chunk = |alpha: f32, width: Option<f32>| -> usize {
        if let Some(index) = chunks.iter().position(|chunk| {
            chunk.alpha.to_bits() == alpha.to_bits()
                && chunk.width.map(f32::to_bits) == width.map(f32::to_bits)
        }) {
            return index;
        }
        chunks.push(Chunk {
            alpha,
            width,
            builder: None,
            vertices: 0,
        });
        chunks.len() - 1
    };
    // Indices into `chunks`, resolved before borrowing it mutably.
    let mut work: Vec<(usize, Mark, f32)> = Vec::new();
    let mut fills: Vec<(usize, Vec<Point<Pixels>>)> = Vec::new();
    for stroke in &shapes.strokes {
        for mark in pattern_marks(&stroke.track, stroke.pattern, stroke.width) {
            let index = match mark {
                Mark::Run { .. } => chunk(stroke.alpha, Some(stroke.width)),
                Mark::Dot { .. } => chunk(stroke.alpha, None),
            };
            work.push((index, mark, stroke.width));
        }
    }
    for fill in &shapes.fills {
        let index = chunk(fill.alpha, None);
        fills.push((index, fill.points.iter().map(|at| at.pixels()).collect()));
    }
    let sides = |width: f32| dot_sides(width, key.scale);
    for (index, mark, width) in work {
        let Some(chunk) = chunks.get_mut(index) else {
            continue;
        };
        match mark {
            Mark::Run { points, closed } => {
                // A stroke's tessellation keeps about two vertices per
                // point on each side, plus its caps.
                chunk.add(points.len() * 4 + 4, &mut painted, |builder| {
                    let mut points = points.iter();
                    if let Some(first) = points.next() {
                        builder.move_to(first.pixels());
                    }
                    for point in points {
                        builder.line_to(point.pixels());
                    }
                    if closed {
                        builder.close();
                    }
                });
            }
            Mark::Dot { center } => {
                let sides = sides(width);
                chunk.add(sides, &mut painted, |builder| {
                    builder
                        .add_polygon(&dot_polygon(center, width, sides), true);
                });
            }
        }
    }
    for (index, points) in fills {
        if let Some(chunk) = chunks.get_mut(index) {
            chunk.add(points.len(), &mut painted, |builder| {
                builder.add_polygon(&points, true);
            });
        }
    }
    for chunk in &mut chunks {
        chunk.finish(&mut painted);
    }
    painted
}

/// The view's tessellated indicator, kept for the geometry and look it was
/// built for, so paths are built once per change rather than per paint.
#[derive(Debug, Default)]
pub(super) struct PathCache {
    entry: Option<(PathKey, Rc<Painted>)>,
}

impl PathCache {
    pub(super) fn get(&mut self, key: &PathKey) -> Rc<Painted> {
        if let Some((cached, painted)) = &self.entry
            && cached == key
        {
            return Rc::clone(painted);
        }
        let painted = Rc::new(paint(key));
        self.entry = Some((key.clone(), Rc::clone(&painted)));
        painted
    }

    /// The key of the paths held, which window tests compare with the
    /// frame's record so a stale cache shows.
    #[cfg(test)]
    pub(super) fn key(&self) -> Option<&PathKey> {
        self.entry.as_ref().map(|(key, _)| key)
    }
}

/// What the indicator layer drew in a frame, for tests and smoke state.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct IndicatorRecord {
    /// The state and style drawn, if a look applied.
    pub(super) look: Option<(IndicatorState, IndicatorStyle)>,
    /// The pane area's corner radii.
    pub(super) corners: Corners,
    /// The key of the painted paths, while the style paints any.
    pub(super) key: Option<PathKey>,
    /// The divider segments drawn.
    pub(super) dividers: Vec<Bounds<Pixels>>,
    /// The drawn panes typing reaches through input sync, whatever the
    /// style.
    pub(super) receiving: Vec<PaneId>,
    /// Panes whose title bar the `title_bar` style tints.
    pub(super) tinted: Vec<PaneId>,
    /// Panes that show a state chip because their bar is off screen.
    pub(super) chips: Vec<PaneId>,
}

/// The focus cue at `cue`, the bar edge of its pane, moved `gap` clear of
/// each line in `lines` it meets: down from a line along its top (or up
/// from one along its bottom when `bottom`), and in from one at either
/// end. Its ends also stay clear of the pane area's rounded corners.
fn inset_cue(
    cue: Bounds<Pixels>,
    bottom: bool,
    lines: &[Bounds<Pixels>],
    gap: f32,
    area: Bounds<Pixels>,
    corners: Corners,
) -> Bounds<Pixels> {
    let mut cue = Rect::of(cue);
    let height = cue.bottom - cue.top;
    let lines: Vec<Rect> = lines.iter().map(|line| Rect::of(*line)).collect();
    // A line it meets along its edge moves it; repeat while one does, since
    // a move can bring it to the next.
    for _ in 0..=lines.len() {
        let mut moved = false;
        for line in lines.iter().filter(|line| line.horizontal()) {
            if !cue.meets(*line) {
                continue;
            }
            if bottom && line.top - gap < cue.bottom {
                cue.bottom = line.top - gap;
                cue.top = cue.bottom - height;
                moved = true;
            } else if !bottom && line.bottom + gap > cue.top {
                cue.top = line.bottom + gap;
                cue.bottom = cue.top + height;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    let middle = f32::midpoint(cue.left, cue.right);
    for line in lines.iter().filter(|line| !line.horizontal()) {
        if !cue.meets(*line) {
            continue;
        }
        if f32::midpoint(line.left, line.right) < middle {
            cue.left = cue.left.max(line.right + gap);
        } else {
            cue.right = cue.right.min(line.left - gap);
        }
    }
    let area = Rect::of(area);
    let (left_radius, right_radius, edge) = if bottom {
        (corners.bottom_left, corners.bottom_right, area.bottom)
    } else {
        (corners.top_left, corners.top_right, area.top)
    };
    let within = |radius: Pixels| {
        let radius = f32::from(radius);
        let depth = if bottom {
            edge - cue.bottom
        } else {
            cue.top - edge
        };
        radius > 0.0 && depth < radius
    };
    if within(left_radius) && cue.left < area.left + f32::from(left_radius) {
        cue.left = area.left + f32::from(left_radius);
    }
    if within(right_radius) && cue.right > area.right - f32::from(right_radius)
    {
        cue.right = area.right - f32::from(right_radius);
    }
    cue.right = cue.right.max(cue.left);
    cue.bounds()
}

/// The state chip at the trailing corner of `frame`'s bar edge, `count`
/// marks wide, clear of the focus cue and of a rounded corner there.
fn chip_bounds(
    frame: Bounds<Pixels>,
    bottom: bool,
    count: usize,
    area: Bounds<Pixels>,
    corners: Corners,
) -> Bounds<Pixels> {
    #[expect(clippy::cast_precision_loss, reason = "one or two marks")]
    let count = count.max(1) as f32;
    let width = CHIP_ICON * count + CHIP_PADDING * (count + 1.0) + 2.0;
    let height = CHIP_ICON + CHIP_PADDING * 2.0 + 2.0;
    let corner = if bottom {
        frame.bottom_right()
    } else {
        frame.top_right()
    };
    let radius = if near(corner, area.bottom_right()) {
        corners.bottom_right
    } else if near(corner, area.top_right()) {
        corners.top_right
    } else {
        px(0.0)
    };
    // A box this far in from both sides of a corner clears its arc.
    let inset = CHIP_INSET.max(f32::from(radius) * 0.3 + 1.0);
    let right = f32::from(frame.right()) - inset;
    let top = if bottom {
        f32::from(frame.bottom()) - inset - height
    } else {
        f32::from(frame.origin.y) + inset
    };
    Bounds::new(
        point(px(right - width), px(top)),
        size(px(width), px(height)),
    )
}

/// An element that paints `painted`'s paths in `color`, each at its own
/// share of it. It moves clones of the cached paths into the scene and
/// takes no input.
fn painted_paths(painted: Rc<Painted>, color: Hsla) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |_, (), window, _| {
            for path in &painted.paths {
                window.paint_path(path.path.clone(), color.opacity(path.alpha));
            }
        },
    )
    .absolute()
    .size_full()
    .into_any_element()
}

impl WorkspaceView {
    /// The look `tab` shows in this window, from whether typing reaches
    /// its panes through input sync and whether this window shows one of
    /// them zoomed.
    pub(super) fn active_look(
        &self,
        tab: &TabView,
        cx: &App,
    ) -> Option<ActiveLook> {
        resolve_look(
            input_sync::receives(tab, tab.focused(), cx),
            self.shown_zoom(tab, cx).is_some(),
            &self.config.panes,
        )
    }

    /// The window's corner radii this frame, from its current fullscreen,
    /// Quake, and frame state rather than the state it opened in.
    pub(super) fn window_radii(&self) -> WindowRadii {
        window_radii(
            self.chrome_hidden(),
            self.frame_platform(),
            self.native_radii,
            self.window_frame().decorated(),
        )
    }

    /// The marks drawn over the active tab's panes, after the dividers and
    /// the rounded terminal corner beside a vertical tab bar: the sync or
    /// zoom indicator, any state chips, then the focused pane's accent
    /// line, so the cue stays readable. `area` is the pane area inside
    /// the window's `content`.
    pub(super) fn pane_marks(
        &self,
        area: Bounds<Pixels>,
        content: Bounds<Pixels>,
        terminal_corner: Option<TerminalCorner>,
        colors: TabColors,
        window: &Window,
        cx: &App,
    ) -> Vec<AnyElement> {
        let Some(tab) = self.active_tab_view(cx) else {
            return Vec::new();
        };
        // What `pane_elements` drew this frame.
        let rects = self
            .drawn_panes
            .borrow()
            .as_ref()
            .filter(|(drawn, _)| *drawn == tab.id)
            .map(|(_, panes)| TabPanes {
                panes: panes.clone(),
                dividers: self.drawn_dividers.borrow().clone(),
            });
        let focused = tab.focused().pane;
        let drawn: Vec<PaneRect> = rects.as_ref().map_or_else(
            || {
                vec![PaneRect {
                    pane: focused,
                    frame: area,
                    title_bar: None,
                    terminal: area,
                }]
            },
            |rects| rects.panes.clone(),
        );
        let active = self.active_look(tab, cx);
        let corners =
            area_corners(content, area, self.window_radii(), terminal_corner);
        let mut record = IndicatorRecord {
            look: active.map(|active| (active.state, active.look.style)),
            corners,
            receiving: drawn
                .iter()
                .filter(|rect| {
                    tab.pane(rect.pane)
                        .is_some_and(|pane| input_sync::receives(tab, pane, cx))
                })
                .map(|rect| rect.pane)
                .collect(),
            tinted: std::mem::take(
                &mut self.drawn_focus.borrow_mut().indicator.tinted,
            ),
            ..IndicatorRecord::default()
        };
        let mut elements = Vec::new();
        let mut lines = Vec::new();
        if let Some(active) = active.filter(|active| paints(active.look.style))
        {
            let dividers = match (active.state, &rects) {
                (IndicatorState::Sync, Some(rects)) => {
                    rects.dividers.iter().map(DividerLine::from).collect()
                }
                _ => Vec::new(),
            };
            let key = PathKey {
                area,
                corners,
                dividers,
                scale: window.scale_factor(),
                style: active.look.style,
                width: active.look.width,
                pattern: active.look.pattern,
            };
            let painted = self.indicator_paths.borrow_mut().get(&key);
            record.key = Some(key);
            record.dividers.clone_from(&painted.dividers);
            lines.clone_from(&painted.lines);
            let color = active.state.color(colors).opacity(active.look.opacity);
            elements.push(painted_paths(painted, color));
        }
        if let Some(active) = active
            .filter(|active| active.look.style == IndicatorStyle::TitleBar)
        {
            for rect in &drawn {
                if let Some(chip) = self
                    .state_chip(tab, rect, active, area, corners, colors, cx)
                {
                    record.chips.push(rect.pane);
                    elements.push(chip);
                }
            }
        }
        // Only a look that draws lines moves the cue clear of them.
        let gap = active.filter(|_| !lines.is_empty()).map_or(0.0, |active| {
            line_width(active.look.width, window.scale_factor())
        });
        // Panes whose views are installed, as the dim layer counts them: a
        // layout can place a pane before its view arrives.
        let shown = Self::panes_shown(tab, rects.as_ref());
        let cue = self.focus_cue(
            rects.as_ref(),
            focused,
            shown,
            &lines,
            gap,
            area,
            corners,
            colors,
        );
        self.drawn_focus.borrow_mut().indicator = record;
        elements.extend(cue);
        elements
    }

    /// The focused pane's accent line, while `[panes] focus_accent` draws
    /// one. It draws only while the pane's bar is off screen, so no bar
    /// covers it, and moves clear of the indicator's `lines` by `gap`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the frame's geometry and the indicator's lines"
    )]
    fn focus_cue(
        &self,
        rects: Option<&TabPanes>,
        focused: PaneId,
        shown: usize,
        lines: &[Bounds<Pixels>],
        gap: f32,
        area: Bounds<Pixels>,
        corners: Corners,
        colors: TabColors,
    ) -> Option<AnyElement> {
        let bars = self.title_bars();
        let alpha = pane_focus::focus_accent(
            bars.mode,
            self.pane_bars.progress(focused),
            shown,
            self.config.panes.focus_accent,
        )?;
        let frame = rects?.pane(focused)?.frame;
        let bottom = bars.position == PaneTitleBarPosition::Bottom;
        let cue = inset_cue(
            tab_bar::pane_focus_cue_bounds(frame, bottom),
            bottom,
            lines,
            gap,
            area,
            corners,
        );
        let mut marks = self.drawn_focus.borrow_mut();
        marks.accent = Some((focused, alpha));
        marks.cue = Some(cue);
        Some(pane_focus_cue(cue, colors, alpha).into_any_element())
    }

    /// The tint over `pane`'s title bar under the `title_bar` style: the
    /// state's colour at the look's opacity, scaled to [`TINT_SHARE`]. The
    /// zoom look marks the zoomed pane, the only one with a bar on screen,
    /// and the sync look each pane typing reaches (`input_sync`). A tinted
    /// pane joins the frame's record.
    pub(super) fn bar_tint(
        &self,
        active: Option<ActiveLook>,
        pane: PaneId,
        input_sync: bool,
        colors: TabColors,
    ) -> Option<Hsla> {
        let marked = active.is_some_and(|active| match active.state {
            IndicatorState::Zoom => true,
            IndicatorState::Sync => input_sync,
        });
        let active = active.filter(|active| {
            marked && active.look.style == IndicatorStyle::TitleBar
        })?;
        self.drawn_focus.borrow_mut().indicator.tinted.push(pane);
        Some(
            active
                .state
                .color(colors)
                .opacity(active.look.opacity * TINT_SHARE),
        )
    }

    /// The chip that marks `rect`'s pane under the `title_bar` style while
    /// its bar is off screen: hidden, not revealed yet, or absent, as in a
    /// tab of one pane. It carries no listeners or tooltip.
    #[expect(
        clippy::too_many_arguments,
        reason = "the pane, the look, and the area's geometry"
    )]
    fn state_chip(
        &self,
        tab: &TabView,
        rect: &PaneRect,
        active: ActiveLook,
        area: Bounds<Pixels>,
        corners: Corners,
        colors: TabColors,
        cx: &App,
    ) -> Option<AnyElement> {
        let pane = tab.pane(rect.pane)?;
        let syncs = input_sync::receives(tab, pane, cx);
        let marked = match active.state {
            IndicatorState::Sync => syncs,
            IndicatorState::Zoom => true,
        };
        let bars = self.title_bars();
        let bar_shown = rect.title_bar.is_some()
            && match bars.mode {
                PaneTitleBarMode::Visible => true,
                PaneTitleBarMode::Hover => {
                    self.pane_bars.progress(rect.pane) > 0.0
                }
                PaneTitleBarMode::Hidden => false,
            };
        if !marked || bar_shown {
            return None;
        }
        let zoomed = active.state == IndicatorState::Zoom;
        let mut marks = Vec::new();
        if zoomed {
            marks.push(
                icon_element(Icon::Maximize, colors.zoom).into_any_element(),
            );
        }
        if syncs {
            marks.push(sync_mark(colors, zoomed).into_any_element());
        }
        let bottom = bars.position == PaneTitleBarPosition::Bottom;
        let bounds =
            chip_bounds(rect.frame, bottom, marks.len(), area, corners);
        let color = active.state.color(colors);
        let opacity = active.look.opacity;
        Some(
            div()
                .absolute()
                .left(bounds.origin.x)
                .top(bounds.origin.y)
                .w(bounds.size.width)
                .h(bounds.size.height)
                .flex()
                .items_center()
                .justify_center()
                .gap(px(CHIP_PADDING))
                .rounded(px(4.0))
                .border_1()
                .border_color(color.opacity(0.6 * opacity))
                .bg(color.opacity(0.18 * opacity))
                .children(marks)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests;
