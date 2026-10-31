//! Scroll area: a vertically scrolled region with the fading overlay
//! scrollbar, as menus, the palette's results, and the Licenses lists use.
//!
//! Anatomy: a relative frame holding the scrolled content, tracked by the
//! owner's `ScrollHandle`, and the owner's [`ListScrollbar`] mounted over its
//! right edge. The owner keeps both, because the scrollbar animates: it
//! drives the scrollbar's schedule from its own frame clock.
//!
//! Behavior: the wheel scrolls the content and reveals the indicator, which
//! fades after a hold. The scrollbar widens on hover, jumps on a track
//! press, and drags its thumb, continuing outside the area. A content that
//! fits draws no scrollbar and leaves its right edge free for clicks.
//!
//! Holding a row: a [`ScrollArea::fill`] area given a [`RowAnchor`]
//! ([`ScrollArea::hold_rows`]) keeps a row the user changed where it painted
//! while rows above or below it appear or disappear, such as a switch that
//! hides the rows under it at the content's foot. The owner wraps each row
//! the user can change in [`RowAnchor::row`] and calls [`RowAnchor::hold`]
//! on the change. In each frame laid out after it, the area moves its offset
//! so the row paints at the top it painted at, capped at the content's top.
//! While it holds, the content stays a viewport taller than the offset
//! needs, so neither rows going below nor rows coming above, up to a
//! viewport's worth in one frame, clamp the offset; that space lies below
//! what shows, and goes when the hold ends, so scrolling never reaches it.
//! The scrollbar counts it while the hold lasts, so a content that fits can
//! show a thumb for that second. More than a viewport coming above clamps
//! one frame, and the next corrects. The hold is about layout only: it ends
//! when [`HOLD_QUIET`] passes without the row needing a correction, on the
//! wheel or any other change of the offset, such as a scrollbar drag or a
//! reveal the owner follows with [`RowAnchor::release`], and on
//! [`RowAnchor::release`] itself. Space that showed below the content then
//! stays until the user scrolls up through it, or until the rows grow
//! shorter on their own, such as when a search narrows them, which drops it
//! in that frame; [`RowAnchor::reset`] drops it too, such as when the owner
//! shows other content.
//!
//! Keyboard: the area takes no focus. When focus moves to a control inside
//! it, such as on Tab or Shift-Tab, the owner passes the control's
//! [`ContentBounds`], recorded as it painted, to [`ScrollArea::reveal`],
//! which scrolls the smallest distance that shows the control and its focus
//! ring fully, and nothing when they already show, even when called again
//! before the area repaints. A control it scrolls to stops
//! [`REVEAL_CLEARANCE`] from the edge, or as far as the area leaves room
//! for, so a window's edge or hairline cannot cover the ring. A control
//! taller than the area shows from its top.
//!
//! Accessibility: a `ScrollView` node; the content reports its own nodes. A
//! held row's wrapper reports none and takes no element id, so it changes
//! no id inside it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, Context, Div, Element, ElementId, Entity,
    GlobalElementId, InspectorElementId, LayoutId, ParentElement, Pixels,
    Point, ScrollEvent, ScrollHandle, Styled, Window, canvas, div, point,
    prelude::*,
};

use crate::list_scrollbar::ListScrollbar;
use crate::scrollbar::ScrollbarColors;
use crate::tokens::size;

/// A scroll area's content and options, rendered by [`ScrollArea::render`].
pub struct ScrollArea {
    id: ElementId,
    scroll: ScrollHandle,
    max_height: Option<Pixels>,
    fill: bool,
    anchor: Option<RowAnchor>,
    children: Vec<AnyElement>,
}

impl ScrollArea {
    /// An area whose content `scroll` tracks.
    #[must_use]
    pub fn new(id: impl Into<ElementId>, scroll: &ScrollHandle) -> Self {
        Self {
            id: id.into(),
            scroll: scroll.clone(),
            max_height: None,
            fill: false,
            anchor: None,
            children: Vec::new(),
        }
    }

    /// The height beyond which the content scrolls. Unset, the area grows
    /// with its content, so it scrolls only when the caller limits the
    /// returned frame's height.
    #[must_use]
    pub fn max_height(mut self, max_height: Option<Pixels>) -> Self {
        self.max_height = max_height;
        self
    }

    /// Makes the area fill the rest of a flex column and shrink with it,
    /// as in a dialog body, so its content scrolls there instead of
    /// stretching the area to its full height.
    #[must_use]
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    /// Keeps the row `anchor` holds where it painted while the content
    /// changes around it. The owner keeps `anchor` with the area's
    /// `ScrollHandle` and gives each area its own. Only a [`Self::fill`]
    /// area holds rows: in one that grows with its content, the space a
    /// hold keeps would grow the area too.
    #[must_use]
    pub fn hold_rows(mut self, anchor: &RowAnchor) -> Self {
        self.anchor = Some(anchor.clone());
        self
    }

    /// Scrolls the area `scroll` tracks the smallest distance that shows
    /// `target`, a control's bounds within its content, with its focus
    /// ring. Returns whether it scrolled; the owner then redraws, and may
    /// show its scrollbar's indicator.
    #[must_use = "the owner redraws when the area scrolled"]
    pub fn reveal(scroll: &ScrollHandle, target: ContentBounds) -> bool {
        let offset = scroll.offset();
        let y = reveal_offset(
            scroll.bounds(),
            target.0 + offset,
            offset.y,
            scroll.max_offset().y,
        );
        if y == offset.y {
            return false;
        }
        scroll.set_offset(point(offset.x, y));
        true
    }

    /// The area with `scrollbar`, which `view` owns and `get` reaches, over
    /// its right edge.
    pub fn render<V: 'static>(
        self,
        scrollbar: &mut ListScrollbar,
        colors: ScrollbarColors,
        view: &Entity<V>,
        get: fn(&mut V) -> &mut ListScrollbar,
        cx: &mut Context<'_, V>,
    ) -> Div {
        let scrollbar_id = ElementId::NamedChild(
            std::sync::Arc::new(self.id.clone()),
            "scrollbar".into(),
        );
        let strip = scrollbar.elements(scrollbar_id, colors, view, get, cx);
        let wheel = view.clone();
        let fill = self.fill;
        let released = self.anchor.clone();
        let (corrector, children) = match &self.anchor {
            Some(anchor) => {
                debug_assert!(
                    fill,
                    "hold_rows needs a filled area, whose viewport the content \
                     does not size"
                );
                // A thumb drag or a track jump scrolls as the wheel does.
                if scrollbar.dragging() {
                    anchor.release();
                }
                let keep = anchor.begin_frame(&self.scroll);
                let correcting = anchor.clone();
                // Runs after layout and before the scrolled div's prepaint
                // clamps the offset.
                let corrector = canvas(
                    move |_, window, cx| {
                        let now = cx.background_executor().now();
                        correcting.correct(window, now);
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .size_0();
                // One content box, so a kept height reaches GPUI's measure of
                // the content; `flex_none`, or a minimum below the content
                // would let it shrink. The rows inside it lay out without
                // that minimum, so the anchor can tell their own height.
                let rows = div().flex().flex_col().children(self.children);
                let content = div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .when_some(keep, Styled::min_h)
                    .child(anchor.content(rows))
                    .into_any_element();
                (Some(corrector), vec![content])
            }
            None => (None, self.children),
        };
        div()
            .relative()
            .flex()
            .flex_col()
            .when(fill, |frame| frame.flex_1().min_h_0())
            .children(corrector)
            .child(
                div()
                    .id(self.id)
                    .role(gpui::Role::ScrollView)
                    .flex()
                    .flex_col()
                    .when(fill, |area| area.flex_1().min_h_0())
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .when_some(self.max_height, Styled::max_h)
                    .on_scroll(move |_: &ScrollEvent, _, cx| {
                        if let Some(anchor) = &released {
                            anchor.release();
                        }
                        wheel.update(cx, |view, cx| {
                            get(view).show();
                            cx.notify();
                        });
                    })
                    .children(children),
            )
            .children(strip)
    }
}

/// A control's bounds within a scroll area's content: where it painted in
/// the window, less the area's scroll offset at the time. Unlike painted
/// bounds, they stay true after the area scrolls and before it repaints.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContentBounds(Bounds<Pixels>);

impl ContentBounds {
    /// The content bounds of a control that painted at `painted`, in window
    /// coordinates, inside the area `scroll` tracks. Record them while the
    /// control paints, such as from a `canvas` in it, when the area's offset
    /// is the one its bounds were laid out with.
    #[must_use]
    pub fn record(scroll: &ScrollHandle, painted: Bounds<Pixels>) -> Self {
        let offset = scroll.offset();
        Self(Bounds {
            origin: painted.origin - offset,
            size: painted.size,
        })
    }
}

/// How far [`ScrollArea::reveal`] keeps a control it scrolls to, with its
/// focus ring, from the viewport's edge.
pub const REVEAL_CLEARANCE: Pixels = gpui::px(8.0);

/// The vertical offset, from the current `offset` (zero or less, down to
/// `-max`), that shows `target` with its focus ring inside `viewport` while
/// moving it the least: its top near the viewport's top when it sits above,
/// its bottom near the viewport's bottom when below, and its top to the top
/// when it is taller than the viewport. A target whose ring already shows
/// stays; one that moves keeps [`REVEAL_CLEARANCE`] from the edge, where a
/// window's edge or hairline would otherwise cover its ring.
#[must_use]
pub fn reveal_offset(
    viewport: Bounds<Pixels>,
    target: Bounds<Pixels>,
    offset: Pixels,
    max: Pixels,
) -> Pixels {
    // The ring and the gap inside it reach outside the control.
    let reach = size::FOCUS_RING + gpui::px(1.0);
    let top = target.top() - reach;
    let bottom = target.bottom() + reach;
    // No more than the viewport leaves around the target, split between
    // its edges, so the clearance never pushes the far edge's ring out.
    let clearance = REVEAL_CLEARANCE
        .min(((viewport.size.height - (bottom - top)) / 2.0).max(Pixels::ZERO));
    let shift = if top < viewport.top() || bottom - top > viewport.size.height {
        viewport.top() - (top - clearance)
    } else if bottom > viewport.bottom() {
        viewport.bottom() - (bottom + clearance)
    } else {
        Pixels::ZERO
    };
    (offset + shift).clamp(-max, Pixels::ZERO)
}

/// How long a held row stays held without needing a correction: long enough
/// for a change's own effects, such as a save and the reload it starts, to
/// lay out, and short enough that layout changes the user did not cause
/// soon move the content as they would without a hold.
pub const HOLD_QUIET: Duration = Duration::from_secs(1);

/// How many frames in a row a clamped hold asks for to correct again.
const CLAMP_RETRIES: u8 = 2;

/// Holds the row a user changed in a [`ScrollArea`] where it painted, while
/// rows above or below it appear or disappear. The owner keeps one next to
/// the area's `ScrollHandle`, passes it to [`ScrollArea::hold_rows`], wraps
/// each row the user can change in [`Self::row`], and calls [`Self::hold`]
/// when the user changes one. Clones share the anchor.
#[derive(Clone, Default)]
pub struct RowAnchor(Rc<RefCell<Anchor>>);

#[derive(Default)]
struct Anchor {
    /// The area's handle, as its last render gave it.
    scroll: Option<ScrollHandle>,
    /// The area's frame, so a layout id from an earlier frame is never read.
    frame: u64,
    /// Where each wrapped row painted, by key, this frame.
    tops: HashMap<ElementId, Pixels>,
    held: Option<Held>,
    /// The content's minimum height: a viewport more than the held offset
    /// needs while holding, and after a release no more than shows, so it
    /// only shrinks.
    keep: Option<Pixels>,
    /// The rows' own layout this frame, without the kept minimum, taken by
    /// the correction like a held row's.
    content_layout: Option<(u64, LayoutId)>,
    /// The rows' own height, as the correction last measured it.
    natural: Option<Pixels>,
    /// The rows' own height when the hold that left `keep` ended. Rows
    /// shorter than that shrank on their own, as a search narrows them, and
    /// the space kept below them goes.
    kept_for: Option<Pixels>,
    /// Every frame's `tops`, while a test records them.
    #[cfg(feature = "test-support")]
    frames: Option<Vec<HashMap<ElementId, Pixels>>>,
}

struct Held {
    key: ElementId,
    /// Where the row painted when the hold began.
    top: Pixels,
    /// The offset the anchor last set or saw, after GPUI's clamp.
    seen: Point<Pixels>,
    /// The row's layout this frame, written as it lays out and taken by
    /// the correction, so a stale id never reaches GPUI.
    layout: Option<(u64, LayoutId)>,
    /// When the row last needed a correction, from the first frame after
    /// the hold; [`HOLD_QUIET`] after it, the hold ends.
    moved: Option<Instant>,
    /// Frames in a row that GPUI clamped the corrected offset.
    clamps: u8,
}

impl RowAnchor {
    /// An anchor holding nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `row`, a row the user can change, under `key`, unique among the
    /// area's rows. The wrapper adds no layout box, element id, or
    /// accessibility node.
    #[must_use]
    pub fn row(
        &self,
        key: impl Into<ElementId>,
        row: impl IntoElement,
    ) -> AnchoredRow {
        AnchoredRow {
            anchor: self.clone(),
            key: key.into(),
            content: false,
            row: Some(row.into_any_element()),
        }
    }

    /// The area's rows, wrapped so the correction can read their own height.
    fn content(&self, rows: impl IntoElement) -> AnchoredRow {
        AnchoredRow {
            anchor: self.clone(),
            key: ElementId::Name("content".into()),
            content: true,
            row: Some(rows.into_any_element()),
        }
    }

    /// Holds the row under `key` at the top it painted at in the last
    /// frame, replacing any hold. Returns `false`, and holds nothing, when
    /// that frame drew no such row.
    pub fn hold(&self, key: impl Into<ElementId>) -> bool {
        let key = key.into();
        let mut anchor = self.0.borrow_mut();
        let (Some(scroll), Some(top)) =
            (anchor.scroll.clone(), anchor.tops.get(&key).copied())
        else {
            anchor.held = None;
            return false;
        };
        let offset = scroll.offset();
        anchor.keep = Some(reach(offset.y, scroll.bounds().size.height));
        anchor.held = Some(Held {
            key,
            top,
            seen: offset,
            layout: None,
            moved: None,
            clamps: 0,
        });
        true
    }

    /// Releases the hold now. The space it kept below the content that
    /// showed stays until the user scrolls up through it.
    pub fn release(&self) {
        self.0.borrow_mut().let_go();
    }

    /// Releases the hold and drops the space it kept and the rows it saw,
    /// for content the area no longer shows.
    pub fn reset(&self) {
        let mut anchor = self.0.borrow_mut();
        anchor.held = None;
        anchor.keep = None;
        anchor.kept_for = None;
        anchor.tops.clear();
    }

    /// Whether a row is held.
    #[must_use]
    pub fn is_held(&self) -> bool {
        self.0.borrow().held.is_some()
    }

    /// Starts a frame of the area `scroll` tracks: forgets the last frame's
    /// rows and returns the content's minimum height, shrunk to what shows
    /// when nothing is held.
    fn begin_frame(&self, scroll: &ScrollHandle) -> Option<Pixels> {
        let mut anchor = self.0.borrow_mut();
        anchor.scroll = Some(scroll.clone());
        anchor.frame += 1;
        let tops = std::mem::take(&mut anchor.tops);
        #[cfg(feature = "test-support")]
        if let Some(frames) = anchor.frames.as_mut() {
            frames.push(tops);
        }
        #[cfg(not(feature = "test-support"))]
        drop(tops);
        if anchor.held.is_none()
            && let Some(keep) = anchor.keep
        {
            let shown = -scroll.offset().y + scroll.bounds().size.height;
            anchor.keep = Some(keep.min(shown));
        }
        anchor.keep
    }

    /// Moves the offset so the held row paints at its held top, capped at
    /// the content's top, after layout and before the scrolled div clamps.
    /// An offset that changed since the anchor last saw it, or a row that
    /// needed no correction for [`HOLD_QUIET`], releases the hold instead;
    /// a held row this frame did not lay out stays where the offset puts it.
    fn correct(&self, window: &mut Window, now: Instant) {
        let mut anchor = self.0.borrow_mut();
        let frame = anchor.frame;
        let Some(scroll) = anchor.scroll.clone() else {
            return;
        };
        if let Some((_, layout)) =
            anchor.content_layout.take().filter(|(at, _)| *at == frame)
        {
            anchor.natural = Some(window.layout_bounds(layout).size.height);
        }
        if anchor.held.is_none() {
            anchor.drop_space_the_rows_left(&scroll);
            return;
        }
        let Some(held) = anchor.held.as_mut() else {
            return;
        };
        let offset = scroll.offset();
        let quiet = held.moved.is_some_and(|moved| {
            now.saturating_duration_since(moved) >= HOLD_QUIET
        });
        if !same_offset(offset, held.seen) || quiet {
            anchor.let_go();
            return;
        }
        held.moved.get_or_insert(now);
        let Some((_, layout)) =
            held.layout.take().filter(|(at, _)| *at == frame)
        else {
            return;
        };
        let y =
            (held.top - window.layout_bounds(layout).top()).min(Pixels::ZERO);
        let corrected = point(offset.x, y);
        if !same_offset(corrected, offset) {
            held.moved = Some(now);
        }
        scroll.set_offset(corrected);
        held.seen = corrected;
        // For the next frame: this one's minimum is already laid out.
        let needed = reach(y, scroll.bounds().size.height);
        anchor.keep = Some(anchor.keep.map_or(needed, |keep| keep.max(needed)));
    }

    /// Starts recording where every wrapped row painted in each frame.
    #[cfg(feature = "test-support")]
    pub fn record_frames(&self) {
        self.0.borrow_mut().frames = Some(Vec::new());
    }

    /// Where every wrapped row painted, by key, in each frame since
    /// [`Self::record_frames`], the frame drawn last included.
    #[cfg(feature = "test-support")]
    #[must_use]
    pub fn recorded_frames(&self) -> Vec<HashMap<ElementId, Pixels>> {
        let anchor = self.0.borrow();
        let mut frames = anchor.frames.clone().unwrap_or_default();
        frames.push(anchor.tops.clone());
        frames
    }
}

impl Anchor {
    /// Ends the hold. The viewport it kept below what showed goes, so a
    /// scroll that ends the hold stops where the content showed instead of
    /// moving into space nobody saw; space that showed stays.
    fn let_go(&mut self) {
        let Some(held) = self.held.take() else {
            return;
        };
        if let (Some(keep), Some(scroll)) = (self.keep, &self.scroll) {
            let shown = -held.seen.y + scroll.bounds().size.height;
            self.keep = Some(keep.min(shown));
        }
        self.kept_for = self.natural;
    }

    /// Drops the space a released hold kept once the rows grow shorter on
    /// their own, and in this frame moves the offset to the foot of the
    /// rows, so they never scroll out of a pane left blank.
    fn drop_space_the_rows_left(&mut self, scroll: &ScrollHandle) {
        let (Some(natural), Some(kept_for)) = (self.natural, self.kept_for)
        else {
            return;
        };
        if natural >= kept_for - gpui::px(0.5) {
            return;
        }
        self.keep = None;
        self.kept_for = None;
        let offset = scroll.offset();
        let max = (natural - scroll.bounds().size.height).max(Pixels::ZERO);
        if -offset.y > max {
            scroll.set_offset(point(offset.x, -max));
        }
    }
}

/// The content height a held `offset` keeps in an area `viewport` tall: what
/// shows, and a viewport more below it, so rows coming above the held row
/// can move the offset down by up to a viewport without GPUI clamping it.
fn reach(offset: Pixels, viewport: Pixels) -> Pixels {
    -offset + viewport * 2.0
}

impl std::fmt::Debug for RowAnchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let anchor = self.0.borrow();
        f.debug_struct("RowAnchor")
            .field("held", &anchor.held.as_ref().map(|held| &held.key))
            .field("keep", &anchor.keep)
            .finish_non_exhaustive()
    }
}

/// Whether two offsets are one, allowing for GPUI rounding the largest
/// offset to hundredths of a point as it clamps, which a corrected offset at
/// a fractional scale can fall between.
fn same_offset(a: Point<Pixels>, b: Point<Pixels>) -> bool {
    let near = |a: Pixels, b: Pixels| (a - b).abs() < gpui::px(0.01);
    near(a.x, b.x) && near(a.y, b.y)
}

/// A row [`RowAnchor::row`] wrapped: it lays out and paints as its row
/// does, and tells the anchor where the row went.
pub struct AnchoredRow {
    anchor: RowAnchor,
    key: ElementId,
    /// The area's rows as a whole rather than one of them.
    content: bool,
    row: Option<AnyElement>,
}

impl std::fmt::Debug for AnchoredRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnchoredRow")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl IntoElement for AnchoredRow {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for AnchoredRow {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    // An id would join the element id stack and change every id, and with
    // it every state and accessibility id, inside the row.
    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(
        &self,
    ) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut row = self.row.take().expect("a row lays out once");
        let layout = row.request_layout(window, cx);
        let mut anchor = self.anchor.0.borrow_mut();
        let frame = anchor.frame;
        if self.content {
            anchor.content_layout = Some((frame, layout));
        } else if let Some(held) = anchor.held.as_mut()
            && held.key == self.key
        {
            held.layout = Some((frame, layout));
        }
        (layout, row)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        row: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if !self.content {
            let mut anchor = self.anchor.0.borrow_mut();
            anchor.tops.insert(self.key.clone(), bounds.top());
            let offset = anchor.scroll.as_ref().map(ScrollHandle::offset);
            if let (Some(held), Some(offset)) = (anchor.held.as_mut(), offset)
                && held.key == self.key
            {
                if same_offset(offset, held.seen) {
                    held.clamps = 0;
                } else {
                    // GPUI clamped the offset: more came above than the
                    // height this frame kept. The next frame keeps enough
                    // and corrects again.
                    held.seen = offset;
                    held.clamps += 1;
                    if held.clamps <= CLAMP_RETRIES {
                        window.request_animation_frame();
                    }
                }
            }
        }
        row.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        row: &mut Self::RequestLayoutState,
        (): &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        row.paint(window, cx);
    }
}

impl std::fmt::Debug for ScrollArea {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScrollArea")
            .field("id", &self.id)
            .field("max_height", &self.max_height)
            .field("children", &self.children.len())
            .finish_non_exhaustive()
    }
}

impl ParentElement for ScrollArea {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use gpui::{
        Bounds, Context, IntoElement, Modifiers, Render, ScrollDelta,
        ScrollEvent, ScrollHandle, TestAppContext, TouchPhase, Window, div,
        point, prelude::*, px, size,
    };

    use std::time::Duration;

    use super::{
        ContentBounds, HOLD_QUIET, REVEAL_CLEARANCE, RowAnchor, ScrollArea,
        reveal_offset,
    };
    use crate::list_scrollbar::ListScrollbar;
    use crate::testing::{a11y_node, tokens};

    struct Scrolled {
        scroll: ScrollHandle,
        scrollbar: ListScrollbar,
    }

    impl Render for Scrolled {
        fn render(
            &mut self,
            _: &mut Window,
            cx: &mut Context<'_, Self>,
        ) -> impl IntoElement {
            let view = cx.entity();
            ScrollArea::new("area", &self.scroll)
                .max_height(Some(px(100.0)))
                .children((0..20).map(|row| {
                    div()
                        .h(px(20.0))
                        .debug_selector(move || format!("row-{row}"))
                        .child(format!("{row}"))
                }))
                .render(
                    &mut self.scrollbar,
                    tokens().scrollbar,
                    &view,
                    |view| &mut view.scrollbar,
                    cx,
                )
        }
    }

    /// A filled area under a header in a short flex column, as in a
    /// dialog body.
    struct Filled {
        scroll: ScrollHandle,
        scrollbar: ListScrollbar,
    }

    impl Render for Filled {
        fn render(
            &mut self,
            _: &mut Window,
            cx: &mut Context<'_, Self>,
        ) -> impl IntoElement {
            let view = cx.entity();
            div()
                .h(px(100.0))
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(div().h(px(20.0)).flex_none())
                .child(
                    ScrollArea::new("area", &self.scroll)
                        .fill()
                        .children(
                            (0..20).map(|_| div().h(px(20.0)).flex_none()),
                        )
                        .render(
                            &mut self.scrollbar,
                            tokens().scrollbar,
                            &view,
                            |view| &mut view.scrollbar,
                            cx,
                        ),
                )
        }
    }

    #[gpui::test]
    fn a_filled_area_scrolls_in_the_space_left(cx: &mut TestAppContext) {
        let scroll = ScrollHandle::new();
        let handle = scroll.clone();
        let (_, cx) = cx.add_window_view(move |_, _| Filled {
            scrollbar: ListScrollbar::new(scroll.clone()),
            scroll,
        });
        cx.run_until_parked();
        // 400 points of rows in the 80 left under the header.
        assert_eq!(handle.max_offset().y, px(320.0));
    }

    #[gpui::test]
    fn the_wheel_scrolls_and_reveals_the_indicator(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| {
            let scroll = ScrollHandle::new();
            Scrolled {
                scrollbar: ListScrollbar::new(scroll.clone()),
                scroll,
            }
        });
        cx.run_until_parked();
        cx.update(|window, _| {
            assert_eq!(a11y_node(window, "area")["aria"]["role"], "ScrollView");
        });
        assert!(
            !view.read_with(cx, |view, _| view.scrollbar.indicator_visible())
        );

        cx.simulate_event(ScrollEvent {
            position: point(px(20.0), px(20.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-40.0))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
            ..Default::default()
        });
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.scroll.offset().y < px(0.0), "the content scrolled");
            assert!(view.scrollbar.indicator_visible(), "the indicator shows");
        });
    }

    /// A 100-point viewport from y 50, and a row of `height` at `top`.
    fn reveal(top: f32, height: f32, offset: f32, max: f32) -> f32 {
        let viewport =
            Bounds::new(point(px(0.0), px(50.0)), size(px(200.0), px(100.0)));
        let target =
            Bounds::new(point(px(0.0), px(top)), size(px(200.0), px(height)));
        reveal_offset(viewport, target, px(offset), px(max)).as_f32()
    }

    #[test]
    fn reveal_moves_the_least_that_shows_the_ring_too() {
        // Inside, with room for the three-point ring: nothing moves, even
        // nearer the edge than the clearance.
        assert_eq!(reveal(60.0, 20.0, -40.0, 300.0), -40.0);
        assert_eq!(reveal(53.0, 20.0, -40.0, 300.0), -40.0);
        // Below: its bottom and ring stop eight points above the
        // viewport's bottom.
        assert_eq!(reveal(140.0, 20.0, -40.0, 300.0), -61.0);
        // Above: its top and ring stop eight points below the top.
        assert_eq!(reveal(40.0, 20.0, -40.0, 300.0), -19.0);
        // Nearly the viewport's height: the room left is split between
        // the edges, so neither ring leaves the viewport.
        assert_eq!(reveal(140.0, 90.0, -40.0, 300.0), -125.0);
        // Taller than the viewport: its top shows.
        assert_eq!(reveal(140.0, 150.0, -40.0, 300.0), -127.0);
        // The offset stays within the content.
        assert_eq!(reveal(40.0, 20.0, -5.0, 300.0), 0.0);
        assert_eq!(reveal(400.0, 20.0, -40.0, 300.0), -300.0);
    }

    #[gpui::test]
    fn reveal_scrolls_a_painted_row_into_view_once(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| {
            let scroll = ScrollHandle::new();
            Scrolled {
                scrollbar: ListScrollbar::new(scroll.clone()),
                scroll,
            }
        });
        cx.run_until_parked();
        let scroll = view.read_with(cx, |view, _| view.scroll.clone());
        let painted = cx.debug_bounds("row-10").expect("row 10 painted");
        let row = ContentBounds::record(&scroll, painted);
        assert!(ScrollArea::reveal(&scroll, row), "row 10 sits below");
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let painted = cx.debug_bounds("row-10").expect("row 10 painted");
        let viewport = scroll.bounds();
        // Its bottom and ring stop the clearance above the viewport's
        // bottom.
        assert_eq!(
            painted.bottom() + px(3.0) + REVEAL_CLEARANCE,
            viewport.bottom()
        );
        let row = ContentBounds::record(&scroll, painted);
        assert!(
            !ScrollArea::reveal(&scroll, row),
            "a shown row moves nothing"
        );
    }

    #[gpui::test]
    fn a_second_reveal_before_a_repaint_moves_nothing(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| {
            let scroll = ScrollHandle::new();
            Scrolled {
                scrollbar: ListScrollbar::new(scroll.clone()),
                scroll,
            }
        });
        cx.run_until_parked();
        let scroll = view.read_with(cx, |view, _| view.scroll.clone());
        let painted = cx.debug_bounds("row-10").expect("row 10 painted");
        let row = ContentBounds::record(&scroll, painted);
        assert!(ScrollArea::reveal(&scroll, row));
        assert!(!ScrollArea::reveal(&scroll, row), "already revealed");
        assert_eq!(scroll.offset().y, px(-131.0));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let painted = cx.debug_bounds("row-10").expect("row 10 painted");
        let viewport = scroll.bounds();
        assert!(painted.top() >= viewport.top());
        assert!(painted.bottom() <= viewport.bottom());
    }

    /// A 100-point area of 20-point rows: `above` rows, the row `anchor`
    /// can hold (`held`, a button, while `shown`), and `below` rows, each
    /// wrapped under its own key. The area fills a 100-point column, as in
    /// Settings.
    struct Anchored {
        scroll: ScrollHandle,
        scrollbar: ListScrollbar,
        anchor: RowAnchor,
        above: usize,
        shown: bool,
        below: usize,
    }

    impl Render for Anchored {
        fn render(
            &mut self,
            _: &mut Window,
            cx: &mut Context<'_, Self>,
        ) -> impl IntoElement {
            let view = cx.entity();
            let row = |key: String| {
                self.anchor.row(key.clone(), div().h(px(20.0)).child(key))
            };
            let above = (0..self.above).map(|at| row(format!("above-{at}")));
            let held = self.shown.then(|| {
                self.anchor.row(
                    "held",
                    div()
                        .id("held-button")
                        .role(gpui::Role::Button)
                        .aria_label("Held")
                        .h(px(20.0)),
                )
            });
            let below = (0..self.below).map(|at| row(format!("below-{at}")));
            let area = ScrollArea::new("area", &self.scroll)
                .fill()
                .hold_rows(&self.anchor)
                .children(above)
                .children(held)
                .children(below)
                .render(
                    &mut self.scrollbar,
                    tokens().scrollbar,
                    &view,
                    |view| &mut view.scrollbar,
                    cx,
                );
            div().flex().flex_col().h(px(100.0)).child(area)
        }
    }

    /// An anchored area drawn at `scale`, scrolled down `scrolled` points.
    fn anchored(
        cx: &mut TestAppContext,
        scale: f32,
        above: usize,
        below: usize,
        scrolled: f32,
    ) -> (gpui::Entity<Anchored>, &mut gpui::VisualTestContext) {
        let (view, cx) = cx.add_window_view(|_, _| {
            let scroll = ScrollHandle::new();
            Anchored {
                scrollbar: ListScrollbar::new(scroll.clone()),
                scroll,
                anchor: RowAnchor::new(),
                above,
                shown: true,
                below,
            }
        });
        cx.simulate_scale_factor_change(scale);
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            view.scroll.set_offset(point(px(0.0), px(-scrolled)));
            view.anchor.record_frames();
            cx.notify();
        });
        cx.run_until_parked();
        (view, cx)
    }

    /// Where the row under `key` painted, against the area's top, in each
    /// frame recorded; `None` in a frame that drew no such row.
    fn tops(
        view: &gpui::Entity<Anchored>,
        cx: &mut gpui::VisualTestContext,
        key: &str,
    ) -> Vec<Option<f32>> {
        view.read_with(cx, |view, _| {
            let top = view.scroll.bounds().top();
            let key = gpui::ElementId::Name(key.to_owned().into());
            view.anchor
                .recorded_frames()
                .iter()
                .map(|frame| frame.get(&key).map(|at| (*at - top).as_f32()))
                .collect()
        })
    }

    /// The held row's top, against the area's, in the first frame recorded
    /// whose top for the row under `key` is `changed`.
    fn first_changed_top(
        view: &gpui::Entity<Anchored>,
        cx: &mut gpui::VisualTestContext,
        changed: impl Fn(Option<f32>) -> bool,
        key: &str,
    ) -> f32 {
        let marker = tops(view, cx, key);
        let held = tops(view, cx, "held");
        let frame = marker
            .iter()
            .position(|top| changed(*top))
            .unwrap_or_else(|| panic!("no frame changed {key}: {marker:?}"));
        held[frame].expect("the held row drew")
    }

    fn offset(
        view: &gpui::Entity<Anchored>,
        cx: &mut gpui::VisualTestContext,
    ) -> f32 {
        view.read_with(cx, |view, _| view.scroll.offset().y.as_f32())
    }

    fn rows_going_below_a_held_row_at_the_foot_leave_it_in_place(
        cx: &mut TestAppContext,
        scale: f32,
    ) {
        // 260 points of rows in 100, scrolled to the foot: the held row
        // paints 40 points down.
        let (view, cx) = anchored(cx, scale, 10, 2, 160.0);
        assert_eq!(tops(&view, cx, "held").last(), Some(&Some(40.0)));
        view.update(cx, |view, cx| {
            assert!(view.anchor.hold("held"));
            view.below = 0;
            cx.notify();
        });
        cx.run_until_parked();
        let top = first_changed_top(&view, cx, |top| top.is_none(), "below-0");
        assert!((top - 40.0).abs() <= 0.5, "the row moved to {top}");
        assert_eq!(offset(&view, cx), -160.0);
    }

    #[gpui::test]
    fn rows_going_below_a_held_row_leave_it_in_place_at_scale_1(
        cx: &mut TestAppContext,
    ) {
        rows_going_below_a_held_row_at_the_foot_leave_it_in_place(cx, 1.0);
    }

    #[gpui::test]
    fn rows_going_below_a_held_row_leave_it_in_place_at_scale_2(
        cx: &mut TestAppContext,
    ) {
        rows_going_below_a_held_row_at_the_foot_leave_it_in_place(cx, 2.0);
    }

    fn rows_coming_above_a_held_row_leave_it_in_place(
        cx: &mut TestAppContext,
        scale: f32,
    ) {
        // Rows 5 to 9 show, the held row 30.5 points down.
        let (view, cx) = anchored(cx, scale, 6, 6, 89.5);
        let before = tops(&view, cx, "held").last().copied().flatten();
        let before = before.expect("the held row drew");
        view.update(cx, |view, cx| {
            assert!(view.anchor.hold("held"));
            view.above = 9;
            cx.notify();
        });
        cx.run_until_parked();
        let top = first_changed_top(&view, cx, |top| top.is_some(), "above-8");
        assert!((top - before).abs() <= 0.5, "{before} moved to {top}");
        assert!((offset(&view, cx) + 149.5).abs() <= 0.5);
    }

    #[gpui::test]
    fn rows_coming_above_a_held_row_leave_it_in_place_at_scale_1(
        cx: &mut TestAppContext,
    ) {
        rows_coming_above_a_held_row_leave_it_in_place(cx, 1.0);
    }

    #[gpui::test]
    fn rows_coming_above_a_held_row_leave_it_in_place_at_scale_2(
        cx: &mut TestAppContext,
    ) {
        rows_coming_above_a_held_row_leave_it_in_place(cx, 2.0);
    }

    #[gpui::test]
    fn rows_going_above_a_held_row_clamp_at_the_top(cx: &mut TestAppContext) {
        // Scrolled 40 of the three rows above, the held row 20 points down.
        let (view, cx) = anchored(cx, 2.0, 3, 10, 40.0);
        view.update(cx, |view, cx| {
            assert!(view.anchor.hold("held"));
            view.above = 0;
            cx.notify();
        });
        cx.run_until_parked();
        let top = first_changed_top(&view, cx, |top| top.is_none(), "above-0");
        assert_eq!(top, 0.0, "the content's top shows");
        assert_eq!(offset(&view, cx), 0.0);
    }

    #[gpui::test]
    fn a_held_row_that_goes_keeps_the_offset(cx: &mut TestAppContext) {
        let (view, cx) = anchored(cx, 2.0, 10, 2, 160.0);
        view.update(cx, |view, cx| {
            assert!(view.anchor.hold("held"));
            view.shown = false;
            view.below = 0;
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(tops(&view, cx, "held").last(), Some(&None));
        // The rows above stay put: the content keeps its height.
        assert_eq!(tops(&view, cx, "above-9").last(), Some(&Some(20.0)));
        assert_eq!(offset(&view, cx), -160.0);
    }

    fn wheel(cx: &mut gpui::VisualTestContext, y: f32) {
        cx.simulate_event(ScrollEvent {
            position: point(px(20.0), px(20.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(y))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
            ..Default::default()
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn the_wheel_the_scrollbar_and_a_set_offset_release_the_hold(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = anchored(cx, 2.0, 6, 6, 60.0);
        view.update(cx, |view, _| assert!(view.anchor.hold("held")));
        wheel(cx, -20.0);
        assert!(!view.read_with(cx, |view, _| view.anchor.is_held()));
        // Released, rows coming above push the row down.
        let before = offset(&view, cx);
        view.update(cx, |view, cx| {
            view.above = 8;
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(offset(&view, cx), before);
        // An offset set elsewhere, as a reveal sets it, releases it too.
        view.update(cx, |view, cx| {
            assert!(view.anchor.hold("held"));
            view.scroll.set_offset(point(px(0.0), px(-10.0)));
            cx.notify();
        });
        cx.run_until_parked();
        assert!(!view.read_with(cx, |view, _| view.anchor.is_held()));
        assert_eq!(offset(&view, cx), -10.0);
        // So does a press on the scrollbar's track.
        let area = view.read_with(cx, |view, _| view.scroll.bounds());
        view.update(cx, |view, _| assert!(view.anchor.hold("held")));
        cx.simulate_mouse_down(
            point(area.right() - px(2.0), area.bottom() - px(4.0)),
            gpui::MouseButton::Left,
            Modifiers::none(),
        );
        cx.run_until_parked();
        assert!(!view.read_with(cx, |view, _| view.anchor.is_held()));
    }

    /// The viewport a hold keeps below the content is never shown: a wheel
    /// that ends the hold stops at the content's foot.
    #[gpui::test]
    fn a_wheel_that_ends_a_hold_stops_at_the_foot(cx: &mut TestAppContext) {
        let (view, cx) = anchored(cx, 2.0, 10, 2, 160.0);
        view.update(cx, |view, cx| {
            assert!(view.anchor.hold("held"));
            cx.notify();
        });
        cx.run_until_parked();
        wheel(cx, -40.0);
        assert!(!view.read_with(cx, |view, _| view.anchor.is_held()));
        assert_eq!(offset(&view, cx), -160.0);
    }

    #[gpui::test]
    fn kept_space_shrinks_as_the_user_scrolls_up_and_goes_on_reset(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = anchored(cx, 2.0, 10, 2, 160.0);
        view.update(cx, |view, cx| {
            assert!(view.anchor.hold("held"));
            view.below = 0;
            cx.notify();
        });
        cx.run_until_parked();
        let held = |cx: &mut gpui::VisualTestContext| {
            view.read_with(cx, |view, _| view.anchor.is_held())
        };
        // Quiet ends the hold, and the next render drops what lies below.
        cx.executor().advance_clock(HOLD_QUIET);
        rerender(&view, cx);
        assert!(!held(cx), "a quiet row stays held");
        rerender(&view, cx);
        let max = |cx: &mut gpui::VisualTestContext| {
            view.read_with(cx, |view, _| view.scroll.max_offset().y.as_f32())
        };
        assert_eq!(max(cx), 160.0, "the content keeps the offset's height");
        wheel(cx, 20.0);
        assert_eq!(offset(&view, cx), -140.0);
        assert_eq!(max(cx), 140.0, "the space scrolled past is gone");
        wheel(cx, -40.0);
        assert_eq!(offset(&view, cx), -140.0, "it never grows back");
        view.update(cx, |view, cx| {
            view.anchor.reset();
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(max(cx), 120.0);
        assert_eq!(offset(&view, cx), -120.0);
    }

    /// Draws the frame the area asked for, as the display does.
    fn next_frame(cx: &mut gpui::VisualTestContext) {
        cx.update(Window::simulate_next_frame);
        cx.run_until_parked();
    }

    /// Renders the area again, as any change to the owner does.
    fn rerender(
        view: &gpui::Entity<Anchored>,
        cx: &mut gpui::VisualTestContext,
    ) {
        view.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
    }

    #[gpui::test]
    fn rows_coming_above_as_rows_go_below_keep_the_row_in_place(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = anchored(cx, 2.0, 10, 2, 160.0);
        view.update(cx, |view, cx| {
            assert!(view.anchor.hold("held"));
            view.above = 12;
            view.below = 0;
            cx.notify();
        });
        cx.run_until_parked();
        // The viewport the hold keeps below covers the rows that came above.
        let top = first_changed_top(&view, cx, |top| top.is_some(), "above-11");
        assert_eq!(top, 40.0);
        assert_eq!(offset(&view, cx), -200.0);
    }

    /// Content shorter than the area has no room to scroll, so rows coming
    /// above a held row needed the space the hold keeps below.
    #[gpui::test]
    fn rows_coming_above_a_held_row_in_content_that_fits_leave_it_in_place(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = anchored(cx, 2.0, 1, 1, 0.0);
        view.update(cx, |view, cx| {
            assert!(view.anchor.hold("held"));
            view.above = 3;
            cx.notify();
        });
        cx.run_until_parked();
        let top = first_changed_top(&view, cx, |top| top.is_some(), "above-2");
        assert_eq!(top, 20.0);
        // So do rows coming above over the space a hold at the foot kept.
        let (view, cx) = anchored(cx, 2.0, 10, 2, 160.0);
        view.update(cx, |view, cx| {
            assert!(view.anchor.hold("held"));
            view.below = 0;
            cx.notify();
        });
        cx.run_until_parked();
        view.update(cx, |view, cx| {
            assert!(view.anchor.hold("held"));
            view.above = 12;
            cx.notify();
        });
        cx.run_until_parked();
        let top = first_changed_top(&view, cx, |top| top.is_some(), "above-11");
        assert_eq!(top, 40.0);
    }

    /// More than a viewport coming above content shorter than the area
    /// clamps the frame it lands in, and the frame the hold asks for
    /// corrects, each time it happens while the row stays held.
    #[gpui::test]
    fn every_clamp_under_one_hold_is_corrected_by_the_frame_it_asks_for(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = anchored(cx, 2.0, 1, 1, 0.0);
        view.update(cx, |view, _| assert!(view.anchor.hold("held")));
        for above in [10, 20, 30] {
            view.update(cx, |view, cx| {
                view.above = above;
                cx.notify();
            });
            cx.run_until_parked();
            assert_ne!(
                tops(&view, cx, "held").last(),
                Some(&Some(20.0)),
                "{above} rows above: no clamp to correct"
            );
            next_frame(cx);
            assert_eq!(
                tops(&view, cx, "held").last(),
                Some(&Some(20.0)),
                "{above} rows above"
            );
        }
    }

    /// Space a hold left below the rows goes once they grow shorter on
    /// their own, in that frame, so they never scroll out of a blank pane.
    #[gpui::test]
    fn kept_space_goes_once_the_rows_shrink_on_their_own(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = anchored(cx, 2.0, 10, 2, 160.0);
        view.update(cx, |view, cx| {
            assert!(view.anchor.hold("held"));
            view.below = 0;
            cx.notify();
        });
        cx.run_until_parked();
        cx.executor().advance_clock(HOLD_QUIET);
        rerender(&view, cx);
        assert!(!view.read_with(cx, |view, _| view.anchor.is_held()));
        // Forty points of kept space show below the held row.
        assert_eq!(tops(&view, cx, "held").last(), Some(&Some(40.0)));
        view.update(cx, |view, cx| {
            view.above = 1;
            cx.notify();
        });
        cx.run_until_parked();
        // The two rows left fit, and show from the top at once.
        let top = first_changed_top(&view, cx, |top| top.is_none(), "above-1");
        assert_eq!(top, 20.0);
        assert_eq!(offset(&view, cx), 0.0);
    }

    /// A hold ends once [`HOLD_QUIET`] passes without the row needing a
    /// correction; each correction starts the wait again.
    #[gpui::test]
    fn a_hold_ends_once_its_row_stays_put_for_hold_quiet(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = anchored(cx, 2.0, 6, 6, 60.0);
        let held = |cx: &mut gpui::VisualTestContext| {
            view.read_with(cx, |view, _| view.anchor.is_held())
        };
        let more_above = |cx: &mut gpui::VisualTestContext| {
            view.update(cx, |view, cx| {
                view.above += 1;
                cx.notify();
            });
            cx.run_until_parked();
        };
        view.update(cx, |view, _| assert!(view.anchor.hold("held")));
        rerender(&view, cx);
        let half = HOLD_QUIET / 2;
        cx.executor().advance_clock(half);
        more_above(cx);
        cx.executor().advance_clock(half + Duration::from_millis(1));
        rerender(&view, cx);
        assert!(held(cx), "the correction started the wait again");
        let before = tops(&view, cx, "held").last().copied().flatten();
        cx.executor().advance_clock(HOLD_QUIET);
        more_above(cx);
        assert!(!held(cx));
        // Released, the row moves with the rows that came above it.
        let after = tops(&view, cx, "held").last().copied().flatten();
        assert_eq!(after, before.map(|top| top + 20.0));
    }

    #[gpui::test]
    fn the_row_wrapper_reports_no_node_of_its_own(cx: &mut TestAppContext) {
        let (_, cx) = anchored(cx, 2.0, 1, 1, 0.0);
        cx.update(|window, _| {
            let button = a11y_node(window, "held-button");
            assert_eq!(button["aria"]["role"], "Button");
            assert_eq!(button["aria"]["label"], "Held");
        });
    }
}
