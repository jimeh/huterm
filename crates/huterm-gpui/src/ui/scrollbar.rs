//! Overlay scrollbars shared by the terminal, list, and tab bar views.
//!
//! `Scrollbars` owns one optional scrollbar per axis: its fade, hover
//! expansion, hover flag, and drag grab. Callers supply a `ScrollbarGeometry`
//! per enabled axis each frame in their own scroll units, translate the
//! thumb positions this returns back through that geometry, and keep the
//! GPUI element wiring for their pointer strips.

use std::time::{Duration, Instant};

use gpui::{Bounds, Div, Hsla, Pixels, div, prelude::*, px};

const MIN_THUMB_SIZE: f32 = 24.0;
const TRACK_PADDING: f32 = 2.0;
const INDICATOR_HOLD: Duration = Duration::from_secs(2);
const INDICATOR_FADE: Duration = Duration::from_millis(400);
const SCROLLBAR_EXPAND: Duration = Duration::from_millis(180);
const SCROLLBAR_EXPANDED_HOLD: Duration = Duration::from_secs(4);
/// Pointer strip along the scrollbar's edge that owns its gestures.
const STRIP_THICKNESS: f32 = 12.0;
const STRIP_EXPANDED_THICKNESS: f32 = 18.0;

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
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "horizontal tab bars adopt this later")
    )]
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

/// Empty space before and after a scrollbar track along its axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TrackMargins {
    pub(crate) start: f32,
    pub(crate) end: f32,
}

impl TrackMargins {
    /// Equal margins for lists inside a panel.
    pub(crate) const EVEN: Self = Self {
        start: 2.0,
        end: 2.0,
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
                + TRACK_PADDING * 2.0
                + MIN_THUMB_SIZE))
            .min(1.0);
        let track_start = margins.start * margin_scale;
        let track_length = length - track_start - margins.end * margin_scale;
        let track_padding = TRACK_PADDING * margin_scale;
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

    pub(crate) fn active(&self) -> bool {
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
#[derive(Debug, Default)]
pub(crate) struct IndicatorVisibility {
    fade_start: Option<Instant>,
    pub(crate) opacity: f32,
}

impl IndicatorVisibility {
    pub(crate) fn activate(&mut self, now: Instant) {
        self.fade_start = Some(now + INDICATOR_HOLD);
        self.opacity = 1.0;
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
            visibility: IndicatorVisibility::default(),
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

    fn thickness(&self) -> f32 {
        if self.expansion.active() {
            STRIP_EXPANDED_THICKNESS
        } else {
            STRIP_THICKNESS
        }
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
        if self.visibility.opacity <= 0.0 {
            return None;
        }
        let x = f32::from(position.x - bounds.origin.x);
        let y = f32::from(position.y - bounds.origin.y);
        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
        let thickness = self.thickness();
        let (cross, extent, along) = match self.options.edge {
            Edge::Right => (x, width, y),
            Edge::Left => (extent_flip(x, width), width, y),
            Edge::Bottom => (y, height, x),
            Edge::Top => (extent_flip(y, height), height, x),
        };
        (cross >= (extent - thickness).max(0.0)
            && cross < extent
            && geometry.track_contains(along))
        .then_some(along)
    }

    fn advance(&mut self, now: Instant) -> bool {
        let interacting = self.interacting();
        let mut changed = self.visibility.update(now, interacting);
        changed |= self.expansion.update(
            now,
            self.visibility.opacity > 0.0,
            interacting,
        );
        changed
    }

    fn layers(
        &self,
        geometry: ScrollbarGeometry,
        foreground: Hsla,
    ) -> impl Iterator<Item = Div> {
        let opacity = self.visibility.opacity;
        let expansion = self.expansion.progress;
        let edge = self.options.edge;
        let track = (expansion > 0.0).then(|| {
            place(
                div(),
                edge,
                2.0,
                geometry.track_start,
                geometry.track_size(),
                8.0 + 6.0 * expansion,
            )
            .rounded(px(4.0 + 3.0 * expansion))
            .bg(foreground.opacity(20.0 / 255.0))
            .opacity(opacity * expansion)
        });
        let thumb = place(
            div(),
            edge,
            2.0 + 2.0 * expansion,
            geometry.thumb_start,
            geometry.thumb_size,
            6.0 + 4.0 * expansion,
        )
        .rounded(px(3.0 + 2.0 * expansion))
        .bg(foreground.opacity(187.0 / 255.0))
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

    /// Advances fades and expansion from a refresh pump; true when a redraw
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

    pub(crate) fn opacity(&self, axis: Axis) -> f32 {
        self.slot(axis)
            .map_or(0.0, |scrollbar| scrollbar.visibility.opacity)
    }

    /// Space to keep clear of the strip on `axis` as its expansion animates,
    /// for labels beside the thumb.
    pub(crate) fn strip_inset(&self, axis: Axis) -> f32 {
        self.slot(axis).map_or(0.0, |scrollbar| {
            STRIP_THICKNESS
                + (STRIP_EXPANDED_THICKNESS - STRIP_THICKNESS)
                    * scrollbar.expansion.progress
        })
    }

    /// Thickness of the pointer strip on `axis` for the caller's element.
    pub(crate) fn strip_thickness(&self, axis: Axis) -> f32 {
        self.slot(axis).map_or(0.0, AxisScrollbar::thickness)
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
            if hovering != scrollbar.hovering {
                scrollbar.hovering = hovering;
                changed = true;
            }
        }
        changed
    }

    /// Clears hover when the pointer leaves the strips; true when it was set.
    pub(crate) fn pointer_left(&mut self) -> bool {
        let mut changed = false;
        for (_, scrollbar) in self.enabled_mut() {
            changed |= scrollbar.hovering;
            scrollbar.hovering = false;
        }
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
        foreground: Hsla,
    ) -> impl Iterator<Item = Div> + 'a {
        self.enabled().flat_map(move |(axis, scrollbar)| {
            let geometry = geometries
                .get(axis)
                .filter(|_| scrollbar.visibility.opacity > 0.0);
            geometry.into_iter().flat_map(move |geometry| {
                scrollbar.layers(geometry, foreground)
            })
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

    fn vertical(expand_on_hover: bool, track_press: TrackPress) -> Scrollbars {
        Scrollbars::vertical(ScrollbarOptions {
            edge: Edge::Right,
            origin: Origin::End,
            expand_on_hover,
            track_press,
            margins: TERMINAL_MARGINS,
        })
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
        assert!(
            (scrollbars.strip_thickness(Axis::Vertical)
                - STRIP_EXPANDED_THICKNESS)
                .abs()
                < f32::EPSILON
        );
        assert_eq!(
            scrollbars.hit(&geometries, bounds, position),
            Some((Axis::Vertical, 200.0))
        );
        assert!(scrollbars.pointer_left());
        assert!(!scrollbars.pointer_left());
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
        assert!(
            (jumping.strip_thickness(Axis::Vertical) - STRIP_THICKNESS).abs()
                < f32::EPSILON
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
        assert_eq!(scrollbars.layers(&geometries, Hsla::default()).count(), 2);
        scrollbars.set_axis(Axis::Horizontal, None);
        assert!(
            scrollbars
                .hit(&geometries, bounds, point(px(200.0), px(395.0)))
                .is_none()
        );
    }
}
