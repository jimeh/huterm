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
//! it, such as on Tab or Shift-Tab, the owner passes the control's painted
//! bounds to [`ScrollArea::reveal`], which scrolls the smallest distance
//! that shows the control and its focus ring fully, and nothing when they
//! already show. A control taller than the area shows from its top.
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

    /// Scrolls the area `scroll` tracks the smallest distance that shows
    /// `bounds`, a control's bounds as last painted in window coordinates,
    /// with its focus ring. Returns whether it scrolled; the owner then
    /// redraws, and may show its scrollbar's indicator.
    #[must_use = "the owner redraws when the area scrolled"]
    pub fn reveal(scroll: &ScrollHandle, bounds: Bounds<Pixels>) -> bool {
        let offset = scroll.offset();
        let y = reveal_offset(
            scroll.bounds(),
            bounds,
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
        div()
            .relative()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(self.id)
                    .role(gpui::Role::ScrollView)
                    .flex()
                    .flex_col()
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

/// The vertical offset, from the current `offset` (zero or less, down to
/// `-max`), that shows `target` with its focus ring inside `viewport` while
/// moving it the least: its top to the viewport's top when it sits above,
/// its bottom to the viewport's bottom when below, and its top to the top
/// when it is taller than the viewport.
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
    let shift = if top < viewport.top() || bottom - top > viewport.size.height {
        viewport.top() - top
    } else if bottom > viewport.bottom() {
        viewport.bottom() - bottom
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

    use super::{ScrollArea, reveal_offset};
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
        // Inside, with room for the three-point ring: nothing moves.
        assert_eq!(reveal(60.0, 20.0, -40.0, 300.0), -40.0);
        // Below: its bottom and ring meet the viewport's bottom.
        assert_eq!(reveal(140.0, 20.0, -40.0, 300.0), -53.0);
        // Above: its top and ring meet the viewport's top.
        assert_eq!(reveal(40.0, 20.0, -40.0, 300.0), -27.0);
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
        let row = cx.debug_bounds("row-10").expect("row 10 painted");
        assert!(ScrollArea::reveal(&scroll, row), "row 10 sits below");
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        let row = cx.debug_bounds("row-10").expect("row 10 painted");
        let viewport = scroll.bounds();
        // Its bottom and ring meet the viewport's bottom.
        assert_eq!(row.bottom() + px(3.0), viewport.bottom());
        assert!(
            !ScrollArea::reveal(&scroll, row),
            "a shown row moves nothing"
        );
    }
}
