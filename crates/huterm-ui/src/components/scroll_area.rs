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
//! Accessibility: a `ScrollView` node; the content reports its own nodes.

use gpui::{
    AnyElement, Bounds, Context, Div, ElementId, Entity, ParentElement, Pixels,
    ScrollHandle, ScrollWheelEvent, Styled, div, point, prelude::*,
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
        div()
            .relative()
            .flex()
            .flex_col()
            .when(fill, |frame| frame.flex_1().min_h_0())
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
                    .on_scroll_wheel(move |_: &ScrollWheelEvent, _, cx| {
                        wheel.update(cx, |view, cx| {
                            get(view).show();
                            cx.notify();
                        });
                    })
                    .children(self.children),
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
        ScrollHandle, ScrollWheelEvent, TestAppContext, TouchPhase, Window,
        div, point, prelude::*, px, size,
    };

    use super::{ContentBounds, REVEAL_CLEARANCE, ScrollArea, reveal_offset};
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

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(20.0), px(20.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(-40.0))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
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
}
