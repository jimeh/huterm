//! Overlay scrollbars shared by the terminal, list, and tab bar views.
//!
//! `Scrollbars` owns one optional scrollbar per axis: its fade, hover
//! expansion, hover flag, and drag grab. Callers supply a `ScrollbarGeometry`
//! per enabled axis each frame in their own scroll units, translate the
//! thumb positions this returns back through that geometry, and keep the
//! GPUI element wiring for their pointer strips.

use std::time::{Duration, Instant};

use gpui::{Bounds, Div, Hsla, Pixels, div, prelude::*, px};

use super::animation::AnimationSchedule;

const MIN_THUMB_SIZE: f32 = 24.0;
/// How long an indicator stays fully visible after activity before fading.
pub(crate) const INDICATOR_HOLD: Duration = Duration::from_secs(2);
const INDICATOR_FADE: Duration = Duration::from_millis(400);
const SCROLLBAR_EXPAND: Duration = Duration::from_millis(180);
const SCROLLBAR_EXPANDED_HOLD: Duration = Duration::from_secs(4);
/// Resting thumb thickness of a full-size scrollbar.
const THUMB_THICKNESS: f32 = 6.0;
/// Gap kept between the thumb and a label beside it.
const LABEL_GAP: f32 = 4.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Axis {
    Vertical,
    Horizontal,
}

/// The edge of the scrolled area a scrollbar sits on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Edge {
    Right,
    #[expect(dead_code, reason = "no view places a scrollbar on its left yet")]
    Left,
    Bottom,
    #[expect(dead_code, reason = "no view places a scrollbar on its top yet")]
    Top,
}

impl Edge {
    pub(crate) fn axis(self) -> Axis {
        match self {
            Self::Right | Self::Left => Axis::Vertical,
            Self::Bottom | Self::Top => Axis::Horizontal,
        }
    }
}

/// Where a scroll offset of zero sits: lists count from the start, terminal
/// history from the end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Origin {
    Start,
    End,
}

/// What a press on the track outside the thumb does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TrackPress {
    /// Center the thumb under the pointer and start dragging it.
    Jump,
    /// Move one page toward the pointer.
    Page,
}

/// Empty space before and after a scrollbar track along its axis, plus the
/// track's own padding around the thumb at either end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TrackMargins {
    pub(crate) start: f32,
    pub(crate) end: f32,
    pub(crate) padding: f32,
}

impl TrackMargins {
    /// Equal margins for lists inside a panel.
    pub(crate) const EVEN: Self = Self {
        start: 2.0,
        end: 2.0,
        padding: 2.0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollbarOptions {
    pub(crate) edge: Edge,
    pub(crate) origin: Origin,
    /// Widen the thumb and show the track while hovered or dragging.
    pub(crate) expand_on_hover: bool,
    pub(crate) track_press: TrackPress,
    pub(crate) margins: TrackMargins,
    /// Resting size across the axis; the expanded state always grows back
    /// to full size.
    pub(crate) thumb: ThumbSize,
    /// Distance from `edge` to the strip, leaving that band to other
    /// controls such as a resize handle.
    pub(crate) edge_inset: f32,
    /// How far the pointer target reaches beyond the thumb across the axis.
    pub(crate) hit: HitBand,
    /// Keep the strip live after the indicator fades so hovering it shows
    /// the scrollbar again.
    pub(crate) reveal_on_hover: bool,
    /// How long the indicator stays visible after activity before fading.
    pub(crate) hold: Duration,
}

/// Extra pointer target across the axis, beyond the thumb itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HitBand {
    /// Points beyond the strip's far side toward the edge, so a target can
    /// reach into the edge inset or stop short of another control there.
    pub(crate) outward: f32,
    /// Points beyond the thumb's inner side, away from the edge.
    pub(crate) inward: f32,
}

impl HitBand {
    /// Only the thumb itself.
    pub(crate) const THUMB: Self = Self {
        outward: 0.0,
        inward: 0.0,
    };
}

/// How thick a scrollbar rests across its axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ThumbSize {
    /// A 6-point thumb in a 12-point strip.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "every view currently rests slimmer")
    )]
    Full,
    /// Half of `Full`.
    Slim,
    /// An exact resting thumb thickness in points; the strip scales with it
    /// down to a 6-point minimum.
    Points(f32),
}

impl ThumbSize {
    /// The resting size as a fraction of `Full`.
    fn scale(self) -> f32 {
        match self {
            Self::Full => 1.0,
            Self::Slim => 0.5,
            Self::Points(points) => (points / THUMB_THICKNESS).max(0.0),
        }
    }
}

/// Thumb and track colors, usually the theme's overlay colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollbarColors {
    pub(crate) thumb: Hsla,
    pub(crate) track: Hsla,
}

/// The result of a press on a scrollbar strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Press {
    /// The thumb was grabbed; later pointer moves report through `drag_to`.
    Grabbed,
    /// The thumb jumped so its start is at this position and was grabbed.
    Jump(f32),
    /// Move one page, backward when the press was before the thumb.
    Page { backward: bool },
}

/// A thumb and track laid out along one axis, in caller-relative points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollbarGeometry {
    pub(crate) thumb_start: f32,
    pub(crate) thumb_size: f32,
    pub(crate) track_start: f32,
    track_padding: f32,
    travel: f32,
    /// Scrollable range in the caller's units.
    range: f32,
    origin: Origin,
}

impl ScrollbarGeometry {
    /// Geometry for a track of `length` points showing `viewport` of
    /// `content`, scrolled by `offset` from `origin`, all in the caller's
    /// units. `None` when nothing overflows.
    pub(crate) fn new(
        length: f32,
        content: f32,
        viewport: f32,
        offset: f32,
        origin: Origin,
        margins: TrackMargins,
    ) -> Option<Self> {
        if !length.is_finite()
            || length <= 0.0
            || content <= viewport
            || viewport <= 0.0
        {
            return None;
        }
        let margin_scale = (length
            / (margins.start
                + margins.end
                + margins.padding * 2.0
                + MIN_THUMB_SIZE))
            .min(1.0);
        let track_start = margins.start * margin_scale;
        let track_length = length - track_start - margins.end * margin_scale;
        let track_padding = margins.padding * margin_scale;
        let inner = (track_length - track_padding * 2.0).max(0.0);
        let thumb_size =
            (inner * viewport / content).max(MIN_THUMB_SIZE).min(inner);
        let travel = (inner - thumb_size).max(0.0);
        let range = content - viewport;
        let ratio = (offset / range).clamp(0.0, 1.0);
        let along = match origin {
            Origin::Start => ratio,
            Origin::End => 1.0 - ratio,
        };
        Some(Self {
            thumb_start: track_start + track_padding + travel * along,
            thumb_size,
            track_start,
            track_padding,
            travel,
            range,
            origin,
        })
    }

    /// The offset, in the caller's units from its origin, that puts the
    /// thumb's start at `position`.
    pub(crate) fn offset_for_thumb_start(self, position: f32) -> f32 {
        if self.travel <= 0.0 {
            return 0.0;
        }
        let ratio = ((position - self.track_start - self.track_padding)
            / self.travel)
            .clamp(0.0, 1.0);
        match self.origin {
            Origin::Start => ratio * self.range,
            Origin::End => (1.0 - ratio) * self.range,
        }
    }

    pub(crate) fn contains(self, position: f32) -> bool {
        (self.thumb_start..=self.thumb_start + self.thumb_size)
            .contains(&position)
    }

    pub(crate) fn track_size(self) -> f32 {
        self.travel + self.thumb_size + self.track_padding * 2.0
    }

    pub(crate) fn track_contains(self, position: f32) -> bool {
        (self.track_start..=self.track_start + self.track_size())
            .contains(&position)
    }
}

/// Per-frame geometry for each enabled axis; `None` when that axis has
/// nothing to scroll.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ScrollbarGeometries {
    pub(crate) vertical: Option<ScrollbarGeometry>,
    pub(crate) horizontal: Option<ScrollbarGeometry>,
}

impl ScrollbarGeometries {
    pub(crate) fn vertical(geometry: Option<ScrollbarGeometry>) -> Self {
        Self {
            vertical: geometry,
            horizontal: None,
        }
    }

    fn get(&self, axis: Axis) -> Option<ScrollbarGeometry> {
        match axis {
            Axis::Vertical => self.vertical,
            Axis::Horizontal => self.horizontal,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ScrollbarExpansion {
    started: Option<Instant>,
    collapse_at: Option<Instant>,
    from: f32,
    target: f32,
    pub(crate) progress: f32,
}

impl ScrollbarExpansion {
    pub(crate) fn activate(&mut self, now: Instant) {
        self.sample(now);
        if self.target < 1.0 {
            self.from = self.progress;
            self.target = 1.0;
            self.started = Some(now);
        }
        self.collapse_at = Some(now + SCROLLBAR_EXPANDED_HOLD);
    }

    #[cfg(test)]
    fn active(&self) -> bool {
        self.target > 0.0 || self.progress > 0.0
    }

    pub(crate) fn update(
        &mut self,
        now: Instant,
        visible: bool,
        interacting: bool,
    ) -> bool {
        let previous = self.progress;
        if visible {
            if interacting {
                self.activate(now);
            } else if let Some(deadline) = self.collapse_at
                && now >= deadline
            {
                self.sample(deadline);
                self.from = self.progress;
                self.target = 0.0;
                self.started = Some(deadline);
                self.collapse_at = None;
            }
            self.sample(now);
        } else {
            *self = Self::default();
        }
        (previous - self.progress).abs() > f32::EPSILON
    }

    fn schedule(&self, now: Instant, interacting: bool) -> AnimationSchedule {
        let mut next = AnimationSchedule::IDLE;
        if self
            .started
            .is_some_and(|start| now < start + SCROLLBAR_EXPAND)
            || (self.progress - self.target).abs() > f32::EPSILON
        {
            next = AnimationSchedule::FRAME;
        }
        if !interacting && let Some(at) = self.collapse_at {
            next = next.merge(AnimationSchedule::at(at));
        }
        next
    }

    fn sample(&mut self, now: Instant) {
        if let Some(started) = self.started {
            let elapsed =
                (now.saturating_duration_since(started).as_secs_f32()
                    / SCROLLBAR_EXPAND.as_secs_f32())
                .clamp(0.0, 1.0);
            let eased = 1.0 - (1.0 - elapsed).powi(3);
            self.progress = self.from + (self.target - self.from) * eased;
        }
    }
}

/// A hold-then-fade for overlay indicators.
#[derive(Debug)]
pub(crate) struct IndicatorVisibility {
    hold: Duration,
    fade_start: Option<Instant>,
    pub(crate) opacity: f32,
}

impl Default for IndicatorVisibility {
    fn default() -> Self {
        Self::with_hold(INDICATOR_HOLD)
    }
}

impl IndicatorVisibility {
    pub(crate) fn with_hold(hold: Duration) -> Self {
        Self {
            hold,
            fade_start: None,
            opacity: 0.0,
        }
    }

    pub(crate) fn activate(&mut self, now: Instant) {
        self.fade_start = Some(now + self.hold);
        self.opacity = 1.0;
    }

    pub(crate) fn schedule(&self, now: Instant) -> AnimationSchedule {
        match self.fade_start {
            Some(at) if now < at => AnimationSchedule::at(at),
            Some(_) if self.opacity > 0.0 => AnimationSchedule::FRAME,
            _ => AnimationSchedule::IDLE,
        }
    }

    pub(crate) fn update(&mut self, now: Instant, interacting: bool) -> bool {
        let previous = self.opacity;
        if interacting {
            self.activate(now);
        } else {
            self.opacity = self.fade_start.map_or(0.0, |start| {
                1.0 - (now.saturating_duration_since(start).as_secs_f32()
                    / INDICATOR_FADE.as_secs_f32())
                .clamp(0.0, 1.0)
            });
        }
        (self.opacity - previous).abs() > f32::EPSILON
    }
}

#[derive(Debug)]
struct AxisScrollbar {
    options: ScrollbarOptions,
    visibility: IndicatorVisibility,
    expansion: ScrollbarExpansion,
    hovering: bool,
    /// Pointer distance from the thumb's start while dragging it.
    drag: Option<f32>,
}

impl AxisScrollbar {
    fn new(options: ScrollbarOptions) -> Self {
        Self {
            options,
            visibility: IndicatorVisibility::with_hold(options.hold),
            expansion: ScrollbarExpansion::default(),
            hovering: false,
            drag: None,
        }
    }

    fn interacting(&self) -> bool {
        self.hovering || self.drag.is_some()
    }

    fn expand(&mut self, now: Instant) {
        if self.options.expand_on_hover {
            self.expansion.activate(now);
        }
    }

    /// Scale across the axis: the thumb rests at its configured size and
    /// grows back to full size as it expands.
    fn scale(&self) -> f32 {
        let resting = self.options.thumb.scale();
        resting + (1.0 - resting) * self.expansion.progress
    }

    /// The thumb's inset from the strip's far side and its thickness across
    /// the axis at the current expansion.
    fn thumb_cross(&self) -> (f32, f32) {
        let expansion = self.expansion.progress;
        let scale = self.scale();
        (
            (2.0 + 2.0 * expansion) * scale,
            (6.0 + 4.0 * expansion) * scale,
        )
    }

    /// From the edge to the inner end of the pointer target.
    fn extent(&self) -> f32 {
        let (inset, thick) = self.thumb_cross();
        self.options.edge_inset + inset + thick + self.options.hit.inward
    }

    /// Pointer position along the axis when `position` is over this
    /// scrollbar's strip and track within `bounds`.
    fn hit(
        &self,
        geometry: Option<ScrollbarGeometry>,
        bounds: Bounds<Pixels>,
        position: gpui::Point<Pixels>,
    ) -> Option<f32> {
        let geometry = geometry?;
        if self.visibility.opacity <= 0.0 && !self.options.reveal_on_hover {
            return None;
        }
        let x = f32::from(position.x - bounds.origin.x);
        let y = f32::from(position.y - bounds.origin.y);
        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
        let (cross, extent, along) = match self.options.edge {
            Edge::Right => (x, width, y),
            Edge::Left => (extent_flip(x, width), width, y),
            Edge::Bottom => (y, height, x),
            Edge::Top => (extent_flip(y, height), height, x),
        };
        let far = extent - self.options.edge_inset;
        let (inset, thick) = self.thumb_cross();
        let outer = (far + self.options.hit.outward).min(extent);
        let inner = (far - inset - thick - self.options.hit.inward).max(0.0);
        (cross >= inner && cross < outer && geometry.track_contains(along))
            .then_some(along)
    }

    fn advance(&mut self, now: Instant) -> bool {
        let interacting = self.interacting();
        let mut changed = self.visibility.update(now, interacting);
        // Interaction keeps the expansion open only where it may open at all.
        changed |= self.expansion.update(
            now,
            self.visibility.opacity > 0.0,
            interacting && self.options.expand_on_hover,
        );
        changed
    }

    fn schedule(&self, now: Instant) -> AnimationSchedule {
        let interacting = self.interacting();
        let visibility = if interacting {
            AnimationSchedule::IDLE
        } else {
            self.visibility.schedule(now)
        };
        visibility.merge(self.expansion.schedule(now, interacting))
    }

    fn layers(
        &self,
        geometry: ScrollbarGeometry,
        colors: ScrollbarColors,
    ) -> impl Iterator<Item = Div> {
        let opacity = self.visibility.opacity;
        let expansion = self.expansion.progress;
        let edge = self.options.edge;
        let inset = self.options.edge_inset;
        let scale = self.scale();
        let (thumb_inset, thumb_thickness) = self.thumb_cross();
        let track = (expansion > 0.0).then(|| {
            place(
                div(),
                edge,
                inset + 2.0 * scale,
                geometry.track_start,
                geometry.track_size(),
                (8.0 + 6.0 * expansion) * scale,
            )
            .rounded(px((4.0 + 3.0 * expansion) * scale))
            .bg(colors.track)
            .opacity(opacity * expansion)
        });
        let thumb = place(
            div(),
            edge,
            inset + thumb_inset,
            geometry.thumb_start,
            geometry.thumb_size,
            thumb_thickness,
        )
        .rounded(px((3.0 + 2.0 * expansion) * scale))
        .bg(colors.thumb)
        .opacity(opacity);
        track.into_iter().chain(std::iter::once(thumb))
    }
}

/// Distance from the far edge, so `Left` and `Top` strips can share the
/// `Right` and `Bottom` comparison.
fn extent_flip(cross: f32, extent: f32) -> f32 {
    extent - cross
}

/// Positions an absolute quad `inset` points from `edge`, covering `length`
/// points from `start` along the axis and `thickness` across it.
fn place(
    quad: Div,
    edge: Edge,
    inset: f32,
    start: f32,
    length: f32,
    thickness: f32,
) -> Div {
    let quad = quad.absolute();
    match edge {
        Edge::Right => quad
            .right(px(inset))
            .top(px(start))
            .w(px(thickness))
            .h(px(length)),
        Edge::Left => quad
            .left(px(inset))
            .top(px(start))
            .w(px(thickness))
            .h(px(length)),
        Edge::Bottom => quad
            .bottom(px(inset))
            .left(px(start))
            .h(px(thickness))
            .w(px(length)),
        Edge::Top => quad
            .top(px(inset))
            .left(px(start))
            .h(px(thickness))
            .w(px(length)),
    }
}

/// Up to one scrollbar per axis for one scrolled area.
#[derive(Debug, Default)]
pub(crate) struct Scrollbars {
    vertical: Option<AxisScrollbar>,
    horizontal: Option<AxisScrollbar>,
}

impl Scrollbars {
    /// A controller with only the vertical axis enabled.
    pub(crate) fn vertical(options: ScrollbarOptions) -> Self {
        let mut scrollbars = Self::default();
        scrollbars.set_axis(Axis::Vertical, Some(options));
        scrollbars
    }

    /// Enables `axis` with `options`, or disables it with `None`, ending any
    /// gesture on it.
    pub(crate) fn set_axis(
        &mut self,
        axis: Axis,
        options: Option<ScrollbarOptions>,
    ) {
        let slot = self.slot_mut(axis);
        match (slot.as_mut(), options) {
            (Some(current), Some(options)) if current.options == options => {}
            (_, Some(options)) => {
                debug_assert_eq!(options.edge.axis(), axis);
                *slot = Some(AxisScrollbar::new(options));
            }
            (_, None) => *slot = None,
        }
    }

    fn slot(&self, axis: Axis) -> Option<&AxisScrollbar> {
        match axis {
            Axis::Vertical => self.vertical.as_ref(),
            Axis::Horizontal => self.horizontal.as_ref(),
        }
    }

    fn slot_mut(&mut self, axis: Axis) -> &mut Option<AxisScrollbar> {
        match axis {
            Axis::Vertical => &mut self.vertical,
            Axis::Horizontal => &mut self.horizontal,
        }
    }

    fn enabled(&self) -> impl Iterator<Item = (Axis, &AxisScrollbar)> {
        [
            (Axis::Vertical, self.vertical.as_ref()),
            (Axis::Horizontal, self.horizontal.as_ref()),
        ]
        .into_iter()
        .filter_map(|(axis, scrollbar)| Some((axis, scrollbar?)))
    }

    fn enabled_mut(
        &mut self,
    ) -> impl Iterator<Item = (Axis, &mut AxisScrollbar)> {
        [
            (Axis::Vertical, self.vertical.as_mut()),
            (Axis::Horizontal, self.horizontal.as_mut()),
        ]
        .into_iter()
        .filter_map(|(axis, scrollbar)| Some((axis, scrollbar?)))
    }

    /// Reveals the indicator on `axis`; `advance` fades it later.
    pub(crate) fn show(&mut self, axis: Axis, now: Instant) {
        if let Some(scrollbar) = self.slot_mut(axis) {
            scrollbar.visibility.activate(now);
        }
    }

    pub(crate) fn schedule(&self, now: Instant) -> AnimationSchedule {
        self.enabled()
            .fold(AnimationSchedule::IDLE, |next, (_, scrollbar)| {
                next.merge(scrollbar.schedule(now))
            })
    }

    /// Advances fades and expansion; true when a redraw
    /// is needed.
    pub(crate) fn advance(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for (_, scrollbar) in self.enabled_mut() {
            changed |= scrollbar.advance(now);
        }
        changed
    }

    pub(crate) fn visible(&self, axis: Axis) -> bool {
        self.slot(axis)
            .is_some_and(|scrollbar| scrollbar.visibility.opacity > 0.0)
    }

    /// Whether the caller should mount the pointer strip on `axis`: while
    /// the indicator shows, or always when hovering may reveal it.
    pub(crate) fn wants_strip(&self, axis: Axis) -> bool {
        self.slot(axis).is_some_and(|scrollbar| {
            scrollbar.visibility.opacity > 0.0
                || scrollbar.options.reveal_on_hover
        })
    }

    pub(crate) fn opacity(&self, axis: Axis) -> f32 {
        self.slot(axis)
            .map_or(0.0, |scrollbar| scrollbar.visibility.opacity)
    }

    /// Space to keep clear of the strip on `axis` as its expansion animates,
    /// for labels beside the thumb.
    pub(crate) fn strip_inset(&self, axis: Axis) -> f32 {
        self.slot(axis).map_or(0.0, |scrollbar| {
            let (inset, thick) = scrollbar.thumb_cross();
            scrollbar.options.edge_inset + inset + thick + LABEL_GAP
        })
    }

    /// Distance from the edge to the inner end of the pointer target on
    /// `axis`: the size across the axis of an element that covers the
    /// target and the edge inset, so `layers` inside it line up with `hit`.
    pub(crate) fn strip_extent(&self, axis: Axis) -> f32 {
        self.slot(axis).map_or(0.0, AxisScrollbar::extent)
    }

    pub(crate) fn dragging(&self) -> bool {
        self.enabled()
            .any(|(_, scrollbar)| scrollbar.drag.is_some())
    }

    /// The axis whose strip and track contain `position`, with the position
    /// along it. Vertical wins where the strips overlap in a corner.
    pub(crate) fn hit(
        &self,
        geometries: &ScrollbarGeometries,
        bounds: Bounds<Pixels>,
        position: gpui::Point<Pixels>,
    ) -> Option<(Axis, f32)> {
        self.enabled().find_map(|(axis, scrollbar)| {
            scrollbar
                .hit(geometries.get(axis), bounds, position)
                .map(|along| (axis, along))
        })
    }

    /// Updates hover from a pointer move; true when the hover state changed.
    pub(crate) fn pointer_moved(
        &mut self,
        geometries: &ScrollbarGeometries,
        bounds: Bounds<Pixels>,
        position: gpui::Point<Pixels>,
        now: Instant,
    ) -> bool {
        let hit = self.hit(geometries, bounds, position).map(|(axis, _)| axis);
        let mut changed = false;
        for (axis, scrollbar) in self.enabled_mut() {
            let hovering = hit == Some(axis);
            if hovering {
                scrollbar.expand(now);
                scrollbar.visibility.activate(now);
            }
            if !hovering && scrollbar.hovering {
                scrollbar.visibility.activate(now);
                scrollbar.expand(now);
            }
            if hovering != scrollbar.hovering {
                scrollbar.hovering = hovering;
                changed = true;
            }
        }
        changed
    }

    /// Clears hover when the pointer leaves the strips; true when it was set.
    pub(crate) fn pointer_left(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for (_, scrollbar) in self.enabled_mut() {
            changed |= scrollbar.hovering;
            if scrollbar.hovering {
                scrollbar.visibility.activate(now);
                if scrollbar.options.expand_on_hover {
                    scrollbar.expansion.activate(now);
                }
            }
            scrollbar.hovering = false;
        }
        changed
    }

    /// Clears interaction and animation state when an axis's strip is unmounted.
    /// Returns whether state changed, preserving the axis's configured options.
    pub(crate) fn unmount(&mut self, axis: Axis) -> bool {
        let Some(scrollbar) = self.slot_mut(axis) else {
            return false;
        };
        let changed = scrollbar.interacting()
            || scrollbar.visibility.fade_start.is_some()
            || scrollbar.expansion.started.is_some();
        *scrollbar = AxisScrollbar::new(scrollbar.options);
        changed
    }

    /// A press at `position`: grabs the thumb, or applies the axis's track
    /// press behavior. `None` when no strip was hit.
    pub(crate) fn press(
        &mut self,
        geometries: &ScrollbarGeometries,
        bounds: Bounds<Pixels>,
        position: gpui::Point<Pixels>,
        now: Instant,
    ) -> Option<(Axis, Press)> {
        let (axis, along) = self.hit(geometries, bounds, position)?;
        let geometry = geometries.get(axis)?;
        let scrollbar = self.slot_mut(axis).as_mut()?;
        let press = if geometry.contains(along) {
            scrollbar.drag = Some(along - geometry.thumb_start);
            Press::Grabbed
        } else {
            match scrollbar.options.track_press {
                TrackPress::Jump => {
                    let grab = geometry.thumb_size / 2.0;
                    scrollbar.drag = Some(grab);
                    Press::Jump(along - grab)
                }
                TrackPress::Page => Press::Page {
                    backward: along < geometry.thumb_start,
                },
            }
        };
        scrollbar.expand(now);
        scrollbar.visibility.activate(now);
        Some((axis, press))
    }

    /// The thumb start the dragging axis should move to for `position`.
    pub(crate) fn drag_to(
        &mut self,
        bounds: Bounds<Pixels>,
        position: gpui::Point<Pixels>,
        now: Instant,
    ) -> Option<(Axis, f32)> {
        let (axis, scrollbar) = self
            .enabled_mut()
            .find(|(_, scrollbar)| scrollbar.drag.is_some())?;
        let grab = scrollbar.drag?;
        let along = match axis {
            Axis::Vertical => f32::from(position.y - bounds.origin.y),
            Axis::Horizontal => f32::from(position.x - bounds.origin.x),
        };
        scrollbar.visibility.activate(now);
        Some((axis, along - grab))
    }

    /// Ends any drag; true when one was in progress.
    pub(crate) fn release(&mut self, now: Instant) -> bool {
        let mut released = false;
        for (_, scrollbar) in self.enabled_mut() {
            if scrollbar.drag.take().is_some() {
                scrollbar.visibility.activate(now);
                scrollbar.expand(now);
                released = true;
            }
        }
        released
    }

    /// Track and thumb quads for every visible axis, positioned relative to
    /// the scrolled area.
    pub(crate) fn layers<'a>(
        &'a self,
        geometries: &'a ScrollbarGeometries,
        colors: ScrollbarColors,
    ) -> impl Iterator<Item = Div> + 'a {
        self.enabled().flat_map(move |(axis, scrollbar)| {
            let geometry = geometries
                .get(axis)
                .filter(|_| scrollbar.visibility.opacity > 0.0);
            geometry
                .into_iter()
                .flat_map(move |geometry| scrollbar.layers(geometry, colors))
        })
    }
}

#[cfg(test)]
mod tests {
    use gpui::{point, size};

    use super::*;

    const TERMINAL_MARGINS: TrackMargins = TrackMargins {
        start: 2.0,
        end: 8.0,
        padding: 2.0,
    };

    fn rows(
        length: f32,
        visible: f32,
        history: f32,
        offset: f32,
    ) -> Option<ScrollbarGeometry> {
        ScrollbarGeometry::new(
            length,
            visible + history,
            visible,
            offset,
            Origin::End,
            TERMINAL_MARGINS,
        )
    }

    fn options(
        expand_on_hover: bool,
        track_press: TrackPress,
    ) -> ScrollbarOptions {
        ScrollbarOptions {
            edge: Edge::Right,
            origin: Origin::End,
            expand_on_hover,
            track_press,
            margins: TERMINAL_MARGINS,
            thumb: ThumbSize::Full,
            edge_inset: 0.0,
            hit: HitBand {
                outward: 0.0,
                inward: 4.0,
            },
            reveal_on_hover: false,
            hold: INDICATOR_HOLD,
        }
    }

    fn vertical(expand_on_hover: bool, track_press: TrackPress) -> Scrollbars {
        Scrollbars::vertical(options(expand_on_hover, track_press))
    }

    #[test]
    fn settled_drag_release_restarts_the_expanded_hold() {
        let now = Instant::now();
        let bounds =
            Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(400.0)));
        let geometry = rows(400.0, 32.0, 100.0, 50.0).unwrap();
        let geometries = ScrollbarGeometries::vertical(Some(geometry));
        let mut scrollbars = Scrollbars::vertical(ScrollbarOptions {
            hold: Duration::from_secs(10),
            ..options(true, TrackPress::Jump)
        });
        scrollbars.show(Axis::Vertical, now);
        assert_eq!(
            scrollbars.press(
                &geometries,
                bounds,
                point(px(95.0), px(geometry.thumb_start + 5.0)),
                now
            ),
            Some((Axis::Vertical, Press::Grabbed))
        );
        scrollbars.advance(now + SCROLLBAR_EXPAND);
        assert_eq!(
            scrollbars.schedule(now + SCROLLBAR_EXPAND),
            AnimationSchedule::IDLE
        );
        // The idle scheduler does not advance a settled off-strip drag.
        let released = now + Duration::from_secs(30);
        assert!(scrollbars.release(released));
        scrollbars.advance(released);
        assert!(
            (scrollbars.vertical.as_ref().unwrap().expansion.progress - 1.0)
                .abs()
                < f32::EPSILON
        );
        let deadline = released + SCROLLBAR_EXPANDED_HOLD;
        assert_eq!(
            scrollbars.schedule(released),
            AnimationSchedule::at(deadline)
        );
        scrollbars.advance(deadline + SCROLLBAR_EXPAND / 2);
        let progress = scrollbars.vertical.as_ref().unwrap().expansion.progress;
        assert!(progress > 0.0 && progress < 1.0);
    }

    #[test]
    fn animation_schedule_preserves_hold_extension_and_finishes_fade() {
        let now = Instant::now();
        let mut indicator =
            IndicatorVisibility::with_hold(Duration::from_secs(1));
        indicator.activate(now);
        assert!(!indicator.update(now + Duration::from_millis(100), false));
        assert_eq!(
            indicator.schedule(now),
            AnimationSchedule::at(now + Duration::from_secs(1))
        );
        indicator.activate(now + Duration::from_millis(500));
        assert_eq!(
            indicator.schedule(now),
            AnimationSchedule::at(now + Duration::from_millis(1500))
        );
        let fade = now + Duration::from_millis(1600);
        assert!(indicator.update(fade, false));
        assert_eq!(indicator.schedule(fade), AnimationSchedule::FRAME);
        assert!(indicator.update(now + Duration::from_secs(3), false));
        assert_eq!(
            indicator.schedule(now + Duration::from_secs(3)),
            AnimationSchedule::IDLE
        );
    }

    #[test]
    fn settled_hover_does_not_need_frames_and_leave_restarts_hold() {
        let now = Instant::now();
        let mut bar = AxisScrollbar::new(options(true, TrackPress::Jump));
        bar.hovering = true;
        bar.visibility.activate(now);
        bar.expand(now);
        bar.advance(now + SCROLLBAR_EXPAND);
        assert_eq!(
            bar.schedule(now + SCROLLBAR_EXPAND),
            AnimationSchedule::IDLE
        );
        let mut bars = Scrollbars::vertical(options(true, TrackPress::Jump));
        bars.vertical = Some(bar);
        let left = now + Duration::from_secs(10);
        assert!(bars.pointer_left(left));
        assert_eq!(
            bars.schedule(left).deadline,
            Some(left + bars.vertical.as_ref().unwrap().options.hold)
        );
        assert!(!bars.schedule(left).frame);
    }

    #[test]
    fn unmounted_strip_cancels_hover_drag_and_animation_until_reactivated() {
        let now = Instant::now();
        let config = ScrollbarOptions {
            hold: Duration::from_secs(7),
            reveal_on_hover: true,
            ..options(true, TrackPress::Jump)
        };
        for elapsed in [Duration::ZERO, SCROLLBAR_EXPAND] {
            let mut bars = Scrollbars::vertical(config);
            let bar = bars.vertical.as_mut().unwrap();
            bar.hovering = true;
            bar.drag = Some(5.0);
            bar.visibility.activate(now);
            bar.expand(now);
            bars.advance(now + elapsed);
            assert!(bars.visible(Axis::Vertical));
            assert!(bars.dragging());

            assert!(bars.unmount(Axis::Vertical));
            assert_eq!(bars.schedule(now + elapsed), AnimationSchedule::IDLE);
            assert!(!bars.visible(Axis::Vertical));
            assert!(!bars.dragging());
            assert!(!bars.pointer_left(now + elapsed));
            assert!(!bars.advance(now + Duration::from_secs(30)));
            assert!(!bars.unmount(Axis::Vertical));

            // The strip can be mounted and revealed again with its original hold.
            assert!(bars.wants_strip(Axis::Vertical));
            let remounted = now + Duration::from_secs(60);
            bars.show(Axis::Vertical, remounted);
            assert!(bars.visible(Axis::Vertical));
            assert_eq!(
                bars.schedule(remounted),
                AnimationSchedule::at(remounted + config.hold)
            );
            let bounds = Bounds::new(
                point(px(0.0), px(0.0)),
                size(px(100.0), px(400.0)),
            );
            let geometries =
                ScrollbarGeometries::vertical(rows(400.0, 32.0, 100.0, 0.0));
            assert!(bars.pointer_moved(
                &geometries,
                bounds,
                point(px(95.0), px(200.0)),
                remounted,
            ));
            assert!(bars.schedule(remounted).frame);
            let resting_extent = bars.strip_extent(Axis::Vertical);
            bars.advance(remounted + SCROLLBAR_EXPAND);
            assert!(bars.strip_extent(Axis::Vertical) > resting_extent);
        }
    }

    #[test]
    fn unmount_cancels_an_existing_fade_only_on_its_own_axis() {
        let now = Instant::now();
        let mut bars = vertical(true, TrackPress::Jump);
        bars.set_axis(
            Axis::Horizontal,
            Some(ScrollbarOptions {
                edge: Edge::Bottom,
                ..options(true, TrackPress::Jump)
            }),
        );
        bars.show(Axis::Vertical, now);
        let fading = now + INDICATOR_HOLD + INDICATOR_FADE / 2;
        bars.advance(fading);
        assert_eq!(bars.schedule(fading), AnimationSchedule::FRAME);
        bars.show(Axis::Horizontal, fading);

        assert!(bars.unmount(Axis::Vertical));
        assert!(!bars.visible(Axis::Vertical));
        assert!(bars.visible(Axis::Horizontal));
        assert_eq!(
            bars.schedule(fading),
            AnimationSchedule::at(fading + INDICATOR_HOLD)
        );
        assert!(bars.unmount(Axis::Horizontal));
        assert_eq!(bars.schedule(fading), AnimationSchedule::IDLE);
        assert!(!Scrollbars::default().unmount(Axis::Vertical));
    }

    #[test]
    fn slim_halves_the_strip_and_edge_inset_moves_it_inboard() {
        let now = Instant::now();
        let bounds =
            Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(400.0)));
        let geometries =
            ScrollbarGeometries::vertical(rows(400.0, 32.0, 100.0, 0.0));
        let mut slim = Scrollbars::vertical(ScrollbarOptions {
            thumb: ThumbSize::Slim,
            ..options(false, TrackPress::Jump)
        });
        slim.show(Axis::Vertical, now);
        // Resting slim: 1 inset, 3 thumb, 4 inward.
        assert!((slim.strip_extent(Axis::Vertical) - 8.0).abs() < 0.01);
        assert!(
            slim.hit(&geometries, bounds, point(px(91.0), px(200.0)))
                .is_none()
        );
        assert!(
            slim.hit(&geometries, bounds, point(px(95.0), px(200.0)))
                .is_some()
        );
        let mut growing = Scrollbars::vertical(ScrollbarOptions {
            thumb: ThumbSize::Slim,
            ..options(true, TrackPress::Jump)
        });
        growing.show(Axis::Vertical, now);
        assert!(growing.pointer_moved(
            &geometries,
            bounds,
            point(px(97.0), px(200.0)),
            now
        ));
        growing.advance(now + SCROLLBAR_EXPAND);
        assert!((growing.strip_extent(Axis::Vertical) - 18.0).abs() < 0.001);
        assert!((growing.strip_inset(Axis::Vertical) - 18.0).abs() < 0.001);
        assert_eq!(
            growing
                .layers(
                    &geometries,
                    ScrollbarColors {
                        thumb: Hsla::default(),
                        track: Hsla::default()
                    }
                )
                .count(),
            2
        );
    }

    #[test]
    fn exact_thumb_sizes_scale_the_strip_down_to_a_grabbable_minimum() {
        let now = Instant::now();
        let bounds =
            Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(400.0)));
        let geometries =
            ScrollbarGeometries::vertical(rows(400.0, 32.0, 100.0, 0.0));
        let mut hairline = Scrollbars::vertical(ScrollbarOptions {
            thumb: ThumbSize::Points(2.0),
            hit: HitBand::THUMB,
            ..options(false, TrackPress::Jump)
        });
        hairline.show(Axis::Vertical, now);
        // A hairline with a thumb-only band: 2/3 inset plus a 2-point thumb.
        assert!(
            (hairline.strip_extent(Axis::Vertical) - 2.0 - 2.0 / 3.0).abs()
                < 0.01
        );
        assert!(
            hairline
                .hit(&geometries, bounds, point(px(98.5), px(200.0)))
                .is_some()
        );
        assert!(
            hairline
                .hit(&geometries, bounds, point(px(100.0), px(200.0)))
                .is_none()
        );
        assert!(
            hairline
                .hit(&geometries, bounds, point(px(97.0), px(200.0)))
                .is_none()
        );
        // Reaching outward covers the edge; inward extends the inner side.
        let mut reaching = Scrollbars::vertical(ScrollbarOptions {
            edge_inset: 2.0,
            hit: HitBand {
                outward: 2.0,
                inward: 6.0,
            },
            ..options(false, TrackPress::Jump)
        });
        reaching.show(Axis::Vertical, now);
        assert!(
            reaching
                .hit(&geometries, bounds, point(px(99.5), px(200.0)))
                .is_some()
        );
        assert!(
            reaching
                .hit(&geometries, bounds, point(px(84.5), px(200.0)))
                .is_some()
        );
        assert!(
            reaching
                .hit(&geometries, bounds, point(px(83.0), px(200.0)))
                .is_none()
        );

        let mut inset = Scrollbars::vertical(ScrollbarOptions {
            edge_inset: 6.0,
            ..options(true, TrackPress::Jump)
        });
        inset.show(Axis::Vertical, now);
        assert!((inset.strip_extent(Axis::Vertical) - 18.0).abs() < 0.001);
        // The band nearest the edge belongs to other controls.
        assert!(
            inset
                .hit(&geometries, bounds, point(px(97.0), px(200.0)))
                .is_none()
        );
        assert!(
            inset
                .hit(&geometries, bounds, point(px(90.0), px(200.0)))
                .is_some()
        );
        assert!(
            inset
                .hit(&geometries, bounds, point(px(81.0), px(200.0)))
                .is_none()
        );
        assert!((inset.strip_inset(Axis::Vertical) - 18.0).abs() < 0.001);
    }

    #[test]
    fn set_axis_keeps_state_for_equal_options_and_resets_for_new_ones() {
        let now = Instant::now();
        let bounds =
            Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(400.0)));
        let geometries =
            ScrollbarGeometries::vertical(rows(400.0, 32.0, 100.0, 0.0));
        let mut scrollbars = vertical(true, TrackPress::Jump);
        scrollbars.show(Axis::Vertical, now);
        assert!(scrollbars.pointer_moved(
            &geometries,
            bounds,
            point(px(95.0), px(200.0)),
            now
        ));
        // Render re-applies the same options every frame; nothing resets.
        scrollbars
            .set_axis(Axis::Vertical, Some(options(true, TrackPress::Jump)));
        assert!(scrollbars.visible(Axis::Vertical));
        // Hover survived, so leaving now reports a change.
        assert!(scrollbars.pointer_left(now));
        // Different options rebuild the axis from scratch.
        scrollbars.set_axis(
            Axis::Vertical,
            Some(ScrollbarOptions {
                hold: Duration::from_millis(300),
                ..options(true, TrackPress::Jump)
            }),
        );
        assert!(!scrollbars.visible(Axis::Vertical));
        assert!(!scrollbars.pointer_left(now));
    }

    #[test]
    fn reveal_on_hover_keeps_the_strip_live_after_the_indicator_fades() {
        let now = Instant::now();
        let bounds =
            Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(400.0)));
        let geometries =
            ScrollbarGeometries::vertical(rows(400.0, 32.0, 100.0, 0.0));
        let position = point(px(95.0), px(200.0));
        let mut hidden = vertical(true, TrackPress::Jump);
        assert!(!hidden.wants_strip(Axis::Vertical));
        assert!(hidden.hit(&geometries, bounds, position).is_none());
        let mut revealing = Scrollbars::vertical(ScrollbarOptions {
            reveal_on_hover: true,
            ..options(true, TrackPress::Jump)
        });
        assert!(revealing.wants_strip(Axis::Vertical));
        assert!(!revealing.visible(Axis::Vertical));
        assert!(revealing.hit(&geometries, bounds, position).is_some());
        assert!(revealing.pointer_moved(&geometries, bounds, position, now));
        assert!(revealing.visible(Axis::Vertical));
        hidden.show(Axis::Vertical, now);
        assert!(hidden.wants_strip(Axis::Vertical));
    }

    #[test]
    fn hold_sets_how_long_the_indicator_stays_before_fading() {
        let now = Instant::now();
        let mut quick =
            IndicatorVisibility::with_hold(Duration::from_millis(600));
        quick.activate(now);
        assert!(!quick.update(now + Duration::from_millis(600), false));
        assert!(quick.update(now + Duration::from_millis(800), false));
        assert!((quick.opacity - 0.5).abs() < f32::EPSILON);
        let mut standard = IndicatorVisibility::default();
        standard.activate(now);
        assert!(!standard.update(now + Duration::from_millis(800), false));
        assert!((standard.opacity - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn expansion_animates_then_collapses_after_four_seconds_without_hover() {
        let now = Instant::now();
        let mut expansion = ScrollbarExpansion::default();
        expansion.activate(now);
        assert!(expansion.active());
        assert!(expansion.update(now + SCROLLBAR_EXPAND / 2, true, false));
        assert!((expansion.progress - 0.875).abs() < f32::EPSILON);
        expansion.update(now + SCROLLBAR_EXPAND, true, false);
        assert!((expansion.progress - 1.0).abs() < f32::EPSILON);
        assert!(!expansion.update(now + SCROLLBAR_EXPANDED_HOLD, true, false));
        assert!(expansion.active());
        expansion.update(
            now + SCROLLBAR_EXPANDED_HOLD + SCROLLBAR_EXPAND / 2,
            true,
            false,
        );
        assert!((expansion.progress - 0.125).abs() < f32::EPSILON);
        expansion.update(
            now + SCROLLBAR_EXPANDED_HOLD + SCROLLBAR_EXPAND,
            true,
            false,
        );
        assert!(!expansion.active());
        assert!(expansion.progress.abs() < f32::EPSILON);
        assert!(!expansion.update(now + Duration::from_secs(12), true, false));
        assert!(!expansion.active());
    }

    #[test]
    fn expansion_resets_when_indicator_fades_before_collapse() {
        let now = Instant::now();
        let mut visibility = IndicatorVisibility::default();
        let mut expansion = ScrollbarExpansion::default();
        visibility.activate(now);
        expansion.activate(now);
        let fading = now + INDICATOR_HOLD + INDICATOR_FADE / 2;
        visibility.update(fading, false);
        expansion.update(fading, visibility.opacity > 0.0, false);
        assert!(expansion.active());
        assert!((expansion.progress - 1.0).abs() < f32::EPSILON);
        let hidden = now + INDICATOR_HOLD + INDICATOR_FADE;
        visibility.update(hidden, false);
        expansion.update(hidden, visibility.opacity > 0.0, false);
        assert!(!expansion.active());
    }

    #[test]
    fn hovering_or_dragging_holds_expansion_and_reentry_reverses_smoothly() {
        let now = Instant::now();
        let mut expansion = ScrollbarExpansion::default();
        expansion.activate(now);
        let release = now + Duration::from_secs(30);
        expansion.update(release, true, true);
        assert!((expansion.progress - 1.0).abs() < f32::EPSILON);
        let midway = release + SCROLLBAR_EXPANDED_HOLD + SCROLLBAR_EXPAND / 2;
        expansion.update(midway, true, false);
        let progress = expansion.progress;
        expansion.activate(midway);
        assert!((expansion.progress - progress).abs() < f32::EPSILON);
        expansion.update(midway + SCROLLBAR_EXPAND, true, true);
        assert!((expansion.progress - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn indicator_holds_then_fades_and_stops_updating() {
        let now = Instant::now();
        let mut visibility = IndicatorVisibility::default();
        assert!(!visibility.update(now, false));
        assert!(visibility.opacity.abs() < f32::EPSILON);
        visibility.activate(now);
        assert!(!visibility.update(now + Duration::from_secs(2), false));
        assert!((visibility.opacity - 1.0).abs() < f32::EPSILON);
        assert!(visibility.update(now + Duration::from_millis(2200), false));
        assert!((visibility.opacity - 0.5).abs() < f32::EPSILON);
        assert!(visibility.update(now + Duration::from_millis(2400), false));
        assert!(visibility.opacity.abs() < f32::EPSILON);
        assert!(!visibility.update(now + Duration::from_secs(3), false));
    }

    #[test]
    fn interaction_and_new_scroll_activity_restart_the_fade_delay() {
        let now = Instant::now();
        let mut visibility = IndicatorVisibility::default();
        assert!(visibility.update(now, true));
        assert!(!visibility.update(now + Duration::from_secs(5), true));
        assert!(!visibility.update(now + Duration::from_secs(6), false));
        assert!(visibility.update(now + Duration::from_millis(7200), false));
        assert!((visibility.opacity - 0.5).abs() < f32::EPSILON);
        visibility.activate(now + Duration::from_millis(7200));
        assert!((visibility.opacity - 1.0).abs() < f32::EPSILON);
        assert!(!visibility.update(now + Duration::from_secs(9), false));
    }

    #[test]
    fn inset_track_keeps_tiny_windows_bounded() {
        let geometry = rows(12.0, 32.0, 100.0, 50.0).unwrap();
        assert!(geometry.thumb_start > 0.0);
        assert!(geometry.thumb_start + geometry.thumb_size < 12.0);
        assert!(geometry.offset_for_thumb_start(0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn dragging_beyond_the_window_clamps_to_history_endpoints() {
        let geometry = rows(400.0, 32.0, 1000.0, 500.0).unwrap();
        assert!(
            (geometry.offset_for_thumb_start(-200.0) - 1000.0).abs() < 0.01
        );
        assert!(geometry.offset_for_thumb_start(600.0).abs() < 0.01);
        assert!(
            (geometry.offset_for_thumb_start(geometry.thumb_start) - 500.0)
                .abs()
                < 0.01
        );
    }

    #[test]
    fn end_origin_geometry_tracks_bottom_middle_top_and_minimum_thumb() {
        assert!(rows(400.0, 32.0, 0.0, 0.0).is_none());
        let bottom = rows(400.0, 32.0, 100.0, 0.0).unwrap();
        let middle = rows(400.0, 32.0, 100.0, 50.0).unwrap();
        let top = rows(400.0, 32.0, 100.0, 100.0).unwrap();
        assert!((top.thumb_start - 4.0).abs() < f32::EPSILON);
        assert!((bottom.track_size() - 390.0).abs() < f32::EPSILON);
        assert!((top.thumb_start - top.track_start - 2.0).abs() < f32::EPSILON);
        assert!(
            (bottom.track_start + bottom.track_size()
                - bottom.thumb_start
                - bottom.thumb_size
                - 2.0)
                .abs()
                < 0.0001
        );
        assert!(
            (middle.thumb_start - top.thumb_start.midpoint(bottom.thumb_start))
                .abs()
                < f32::EPSILON
        );
        assert!(
            (bottom.thumb_start - (390.0 - bottom.thumb_size)).abs()
                < f32::EPSILON
        );
        for (geometry, offset) in [(top, 100.0), (middle, 50.0), (bottom, 0.0)]
        {
            assert!(
                (geometry.offset_for_thumb_start(geometry.thumb_start)
                    - offset)
                    .abs()
                    < 0.01
            );
        }
        let deep = rows(400.0, 32.0, 10_000.0, 0.0).unwrap();
        assert!((deep.thumb_size - MIN_THUMB_SIZE).abs() < f32::EPSILON);
        assert!((deep.offset_for_thumb_start(0.0) - 10_000.0).abs() < 0.01);
        assert!(
            deep.offset_for_thumb_start(400.0 - deep.thumb_size).abs() < 0.01
        );
    }

    #[test]
    fn start_origin_geometry_maps_offset_from_top_and_needs_overflow() {
        let even = TrackMargins::EVEN;
        let pixels = |offset| {
            ScrollbarGeometry::new(
                336.0,
                672.0,
                336.0,
                offset,
                Origin::Start,
                even,
            )
        };
        assert!(
            ScrollbarGeometry::new(
                336.0,
                336.0,
                336.0,
                0.0,
                Origin::Start,
                even
            )
            .is_none()
        );
        let top = pixels(0.0).unwrap();
        let bottom = pixels(336.0).unwrap();
        let middle = pixels(168.0).unwrap();
        assert!((top.track_start - 2.0).abs() < f32::EPSILON);
        assert!(
            (bottom.track_start + bottom.track_size() - 334.0).abs()
                < f32::EPSILON
        );
        assert!(
            (middle.offset_for_thumb_start(middle.thumb_start) - 168.0).abs()
                < 0.01
        );
        assert!((top.thumb_start - top.track_start - 2.0).abs() < f32::EPSILON);
        assert!(
            (bottom.track_start + bottom.track_size()
                - bottom.thumb_start
                - bottom.thumb_size
                - 2.0)
                .abs()
                < 0.0001
        );
        assert!(
            (middle.thumb_start - top.thumb_start.midpoint(bottom.thumb_start))
                .abs()
                < 0.0001
        );
        assert!(
            (top.thumb_size * 2.0 - (top.travel + top.thumb_size)).abs()
                < 0.0001
        );
        // Flush margins let the thumb touch both ends of the track.
        let flush = ScrollbarGeometry::new(
            336.0,
            672.0,
            336.0,
            336.0,
            Origin::Start,
            TrackMargins {
                start: 0.0,
                end: 0.0,
                padding: 0.0,
            },
        )
        .unwrap();
        assert!(flush.track_start.abs() < f32::EPSILON);
        assert!((flush.thumb_start + flush.thumb_size - 336.0).abs() < 0.0001);
    }

    #[test]
    fn hover_target_expands_only_while_visible() {
        let now = Instant::now();
        let bounds =
            Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(400.0)));
        let geometries =
            ScrollbarGeometries::vertical(rows(400.0, 32.0, 100.0, 0.0));
        let position = point(px(85.0), px(200.0));
        let mut scrollbars = vertical(true, TrackPress::Page);
        assert!(scrollbars.hit(&geometries, bounds, position).is_none());
        scrollbars.show(Axis::Vertical, now);
        assert!(scrollbars.hit(&geometries, bounds, position).is_none());
        assert_eq!(
            scrollbars.hit(&geometries, bounds, point(px(95.0), px(200.0))),
            Some((Axis::Vertical, 200.0))
        );
        assert!(scrollbars.pointer_moved(
            &geometries,
            bounds,
            point(px(95.0), px(200.0)),
            now
        ));
        // Expanding widens the target: 4 inset, 10 thumb, 4 inward.
        scrollbars.advance(now + SCROLLBAR_EXPAND);
        assert!((scrollbars.strip_extent(Axis::Vertical) - 18.0).abs() < 0.01);
        assert_eq!(
            scrollbars.hit(&geometries, bounds, position),
            Some((Axis::Vertical, 200.0))
        );
        assert!(scrollbars.pointer_left(now));
        assert!(!scrollbars.pointer_left(now));
    }

    #[test]
    fn hit_testing_excludes_insets_and_outside_bounds() {
        let now = Instant::now();
        let bounds =
            Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(400.0)));
        let geometries =
            ScrollbarGeometries::vertical(rows(400.0, 32.0, 100.0, 0.0));
        let mut scrollbars = vertical(true, TrackPress::Page);
        scrollbars.show(Axis::Vertical, now);
        for position in [
            point(px(99.0), px(1.0)),
            point(px(99.0), px(393.0)),
            point(px(100.0), px(200.0)),
            point(px(-1.0), px(200.0)),
        ] {
            assert!(
                scrollbars.hit(&geometries, bounds, position).is_none(),
                "{position:?}"
            );
        }
        assert!(
            scrollbars
                .hit(&geometries, bounds, point(px(99.0), px(392.0)))
                .is_some()
        );
    }

    #[test]
    fn press_grabs_jumps_or_pages_by_option_and_drag_follows_the_grab() {
        let now = Instant::now();
        let bounds =
            Bounds::new(point(px(10.0), px(20.0)), size(px(100.0), px(400.0)));
        let geometry = rows(400.0, 32.0, 100.0, 50.0).unwrap();
        let geometries = ScrollbarGeometries::vertical(Some(geometry));
        let on_thumb = point(px(105.0), px(20.0 + geometry.thumb_start + 5.0));
        let above = point(px(105.0), px(20.0 + geometry.track_start + 1.0));

        let mut paging = vertical(true, TrackPress::Page);
        paging.show(Axis::Vertical, now);
        assert_eq!(
            paging.press(&geometries, bounds, above, now),
            Some((Axis::Vertical, Press::Page { backward: true }))
        );
        assert!(!paging.dragging());
        assert_eq!(
            paging.press(&geometries, bounds, on_thumb, now),
            Some((Axis::Vertical, Press::Grabbed))
        );
        assert!(paging.dragging());
        assert_eq!(
            paging.drag_to(bounds, point(px(0.0), px(20.0 + 100.0)), now),
            Some((Axis::Vertical, 100.0 - 5.0))
        );
        assert!(paging.release(now));
        assert!(!paging.release(now));

        let mut jumping = vertical(false, TrackPress::Jump);
        jumping.show(Axis::Vertical, now);
        assert_eq!(
            jumping.press(&geometries, bounds, above, now),
            Some((
                Axis::Vertical,
                Press::Jump(
                    geometry.track_start + 1.0 - geometry.thumb_size / 2.0
                )
            ))
        );
        assert!(jumping.dragging());
        // Neither the press nor a later pump tick may open the expansion.
        jumping.advance(now + SCROLLBAR_EXPAND);
        assert!(
            (jumping.strip_extent(Axis::Vertical) - 12.0).abs() < f32::EPSILON
        );
        assert_eq!(
            jumping
                .layers(
                    &geometries,
                    ScrollbarColors {
                        thumb: Hsla::default(),
                        track: Hsla::default()
                    }
                )
                .count(),
            1
        );
    }

    #[test]
    fn horizontal_and_vertical_share_bounds_with_vertical_winning_corners() {
        let now = Instant::now();
        let bounds =
            Bounds::new(point(px(0.0), px(0.0)), size(px(400.0), px(400.0)));
        let mut scrollbars = vertical(true, TrackPress::Jump);
        scrollbars.set_axis(
            Axis::Horizontal,
            Some(ScrollbarOptions {
                edge: Edge::Bottom,
                origin: Origin::Start,
                expand_on_hover: false,
                track_press: TrackPress::Jump,
                margins: TrackMargins::EVEN,
                thumb: ThumbSize::Full,
                edge_inset: 0.0,
                hit: HitBand {
                    outward: 0.0,
                    inward: 4.0,
                },
                reveal_on_hover: false,
                hold: INDICATOR_HOLD,
            }),
        );
        let geometries = ScrollbarGeometries {
            vertical: rows(400.0, 32.0, 100.0, 0.0),
            horizontal: ScrollbarGeometry::new(
                400.0,
                800.0,
                400.0,
                0.0,
                Origin::Start,
                TrackMargins::EVEN,
            ),
        };
        scrollbars.show(Axis::Vertical, now);
        scrollbars.show(Axis::Horizontal, now);
        assert_eq!(
            scrollbars
                .hit(&geometries, bounds, point(px(200.0), px(395.0)))
                .map(|(axis, _)| axis),
            Some(Axis::Horizontal)
        );
        assert_eq!(
            scrollbars
                .hit(&geometries, bounds, point(px(395.0), px(200.0)))
                .map(|(axis, _)| axis),
            Some(Axis::Vertical)
        );
        // The vertical track ends 8 points up, so only horizontal owns the
        // very corner.
        assert_eq!(
            scrollbars
                .hit(&geometries, bounds, point(px(395.0), px(395.0)))
                .map(|(axis, _)| axis),
            Some(Axis::Horizontal)
        );
        assert_eq!(
            scrollbars
                .layers(
                    &geometries,
                    ScrollbarColors {
                        thumb: Hsla::default(),
                        track: Hsla::default()
                    }
                )
                .count(),
            2
        );
        scrollbars.set_axis(Axis::Horizontal, None);
        assert!(
            scrollbars
                .hit(&geometries, bounds, point(px(200.0), px(395.0)))
                .is_none()
        );
    }
}
