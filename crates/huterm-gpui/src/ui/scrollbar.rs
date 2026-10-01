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
mod tests;
