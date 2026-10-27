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
//! Accessibility: a `ScrollView` node; the content reports its own nodes.

use gpui::{
    AnyElement, Context, Div, ElementId, Entity, ParentElement, Pixels,
    ScrollHandle, ScrollWheelEvent, Styled, div, prelude::*,
};

use crate::list_scrollbar::ListScrollbar;
use crate::scrollbar::ScrollbarColors;

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

    /// The height beyond which the content scrolls; unset, the area takes
    /// the height its parent gives it.
    #[must_use]
    pub fn max_height(mut self, max_height: Option<Pixels>) -> Self {
        self.max_height = max_height;
        self
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
        Context, IntoElement, Modifiers, Render, ScrollDelta, ScrollHandle,
        ScrollWheelEvent, TestAppContext, TouchPhase, Window, div, point,
        prelude::*, px,
    };

    use super::ScrollArea;
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
                .children(
                    (0..20)
                        .map(|row| div().h(px(20.0)).child(format!("{row}"))),
                )
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
}
