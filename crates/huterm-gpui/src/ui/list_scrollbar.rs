//! The fading overlay scrollbar on a vertically scrolled list tracked by a
//! [`ScrollHandle`], such as the palette's results and a menu's rows. It
//! owns the indicator's state and builds the pointer strip and drag capture
//! the list's owner mounts in a relative frame around the list.

use std::time::Instant;

use gpui::{
    AnyElement, Context, Entity, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ScrollHandle, canvas, div, point, prelude::*, px,
};

use super::animation::AnimationSchedule;
use super::scrollbar::{
    Axis, Edge, HitBand, INDICATOR_HOLD, Origin, Press, ScrollbarColors,
    ScrollbarGeometries, ScrollbarGeometry, ScrollbarOptions, Scrollbars,
    ThumbSize, TrackMargins, TrackPress,
};

/// A slim jump-to-pointer track that widens on hover, scrolled in pixels
/// from the top.
const OPTIONS: ScrollbarOptions = ScrollbarOptions {
    edge: Edge::Right,
    origin: Origin::Start,
    expand_on_hover: true,
    track_press: TrackPress::Jump,
    margins: TrackMargins::EVEN,
    thumb: ThumbSize::Slim,
    edge_inset: 2.0,
    hit: HitBand {
        outward: 2.0,
        inward: 6.0,
    },
    reveal_on_hover: true,
    hold: INDICATOR_HOLD,
};

pub(crate) struct ListScrollbar {
    scrollbars: Scrollbars,
    scroll: ScrollHandle,
}

impl ListScrollbar {
    /// A vertical scrollbar over the list `scroll` tracks.
    pub(crate) fn new(scroll: ScrollHandle) -> Self {
        Self {
            scrollbars: Scrollbars::vertical(OPTIONS),
            scroll,
        }
    }

    /// Reveals the indicator; [`Self::advance`] fades it later. It only
    /// draws while the list overflows.
    pub(crate) fn show(&mut self) {
        self.scrollbars.show(Axis::Vertical, Instant::now());
    }

    pub(crate) fn schedule(&self, now: Instant) -> AnimationSchedule {
        self.scrollbars.schedule(now)
    }

    /// Advances the fade and hover expansion; true when a redraw is needed.
    pub(crate) fn advance(&mut self, now: Instant) -> bool {
        self.scrollbars.advance(now)
    }

    /// Whether the indicator is drawn: showing over a list that overflows.
    pub(crate) fn indicator_visible(&self) -> bool {
        self.scrollbars.visible(Axis::Vertical) && self.geometry().is_some()
    }

    pub(crate) fn dragging(&self) -> bool {
        self.scrollbars.dragging()
    }

    /// The indicator geometry for the list's current overflow, if any.
    pub(crate) fn geometry(&self) -> Option<ScrollbarGeometry> {
        let viewport = f32::from(self.scroll.bounds().size.height);
        let overflow = f32::from(self.scroll.max_offset().y);
        ScrollbarGeometry::new(
            viewport,
            viewport + overflow,
            viewport,
            -f32::from(self.scroll.offset().y),
            OPTIONS.origin,
            OPTIONS.margins,
        )
    }

    fn geometries(&self) -> ScrollbarGeometries {
        ScrollbarGeometries::vertical(self.geometry())
    }

    fn pointer_moved(&mut self, position: gpui::Point<gpui::Pixels>) -> bool {
        let geometries = self.geometries();
        self.scrollbars.pointer_moved(
            &geometries,
            self.scroll.bounds(),
            position,
            Instant::now(),
        )
    }

    fn pointer_left(&mut self) -> bool {
        self.scrollbars.pointer_left(Instant::now())
    }

    /// A press on the strip: grab the thumb, or jump it under the pointer
    /// and grab it there. False when the press missed the strip.
    fn press(&mut self, position: gpui::Point<gpui::Pixels>) -> bool {
        let geometries = self.geometries();
        let Some((_, press)) = self.scrollbars.press(
            &geometries,
            self.scroll.bounds(),
            position,
            Instant::now(),
        ) else {
            return false;
        };
        match press {
            Press::Grabbed | Press::Page { .. } => {}
            Press::Jump(thumb_start) => {
                if let Some(geometry) = geometries.vertical {
                    self.seek(geometry, thumb_start);
                }
            }
        }
        true
    }

    fn drag_to(&mut self, position: gpui::Point<gpui::Pixels>) -> bool {
        let Some(geometry) = self.geometry() else {
            return false;
        };
        let Some((_, thumb_start)) = self.scrollbars.drag_to(
            self.scroll.bounds(),
            position,
            Instant::now(),
        ) else {
            return false;
        };
        self.seek(geometry, thumb_start);
        true
    }

    fn release(&mut self) -> bool {
        self.scrollbars.release(Instant::now())
    }

    fn seek(&self, geometry: ScrollbarGeometry, thumb_start: f32) {
        let offset = geometry.offset_for_thumb_start(thumb_start);
        self.scroll.set_offset(point(px(0.0), px(-offset)));
    }

    /// The pointer strip at the list's right edge and a window-level drag
    /// capture, both absolute: mount them in a relative frame that shares
    /// the list's bounds. `view` owns this scrollbar, reached through `get`.
    pub(crate) fn elements<V: 'static>(
        &mut self,
        id: &'static str,
        colors: ScrollbarColors,
        view: &Entity<V>,
        get: fn(&mut V) -> &mut Self,
        cx: &mut Context<'_, V>,
    ) -> Vec<AnyElement> {
        let geometries = self.geometries();
        // Before the list's first layout its scroll bounds are empty, so
        // geometry cannot say whether it overflows; mount the strip then
        // too, because nothing else repaints before the pointer arrives on
        // a headless X server. Once laid out, a list that fits keeps its
        // right edge free for row clicks.
        let unmeasured = self.scroll.bounds().size.height <= px(0.0);
        let show_strip = self.scrollbars.wants_strip(Axis::Vertical)
            && (geometries.vertical.is_some() || unmeasured);
        // No strip remains to paint its leave animation.
        if !show_strip && self.scrollbars.unmount(Axis::Vertical) {
            cx.notify();
        }
        let mut elements = Vec::with_capacity(2);
        if show_strip {
            elements.push(self.strip(id, colors, &geometries, view, get));
        }
        elements.push(drag_capture(view, get));
        elements
    }

    fn strip<V: 'static>(
        &self,
        id: &'static str,
        colors: ScrollbarColors,
        geometries: &ScrollbarGeometries,
        view: &Entity<V>,
        get: fn(&mut V) -> &mut Self,
    ) -> AnyElement {
        let width = self.scrollbars.strip_extent(Axis::Vertical);
        let (on_move, on_leave, on_press, on_release) =
            (view.clone(), view.clone(), view.clone(), view.clone());
        div()
            .id(id)
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(px(width))
            .block_mouse_except_scroll()
            .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                on_move.update(cx, |view, cx| {
                    if get(view).pointer_moved(event.position) {
                        cx.notify();
                    }
                });
            })
            .on_hover(move |hovering: &bool, _, cx| {
                if !hovering {
                    on_leave.update(cx, |view, cx| {
                        if get(view).pointer_left() {
                            cx.notify();
                        }
                    });
                }
            })
            .on_mouse_down(
                MouseButton::Left,
                move |event: &MouseDownEvent, _, cx| {
                    let hit = on_press.update(cx, |view, cx| {
                        let hit = get(view).press(event.position);
                        if hit {
                            cx.notify();
                        }
                        hit
                    });
                    if hit {
                        cx.stop_propagation();
                    }
                },
            )
            // A press and release within one frame ends before the
            // window-level release listener exists.
            .on_mouse_up(MouseButton::Left, move |_: &MouseUpEvent, _, cx| {
                on_release.update(cx, |view, cx| {
                    if get(view).release() {
                        cx.notify();
                    }
                });
            })
            .children(
                self.scrollbars
                    .layers(geometries, colors)
                    .collect::<Vec<_>>(),
            )
            .into_any_element()
    }
}

/// Window-level move and release listeners, so a thumb drag continues
/// outside the strip and the list.
fn drag_capture<V: 'static>(
    view: &Entity<V>,
    get: fn(&mut V) -> &mut ListScrollbar,
) -> AnyElement {
    let (on_move, on_release) = (view.clone(), view.clone());
    canvas(
        |_, _, _| {},
        move |_, (), window, _| {
            let on_move = on_move.clone();
            window.on_mouse_event(
                move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase.bubble() {
                        on_move.update(cx, |view, cx| {
                            if get(view).drag_to(event.position) {
                                cx.notify();
                            }
                        });
                    }
                },
            );
            let on_release = on_release.clone();
            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                if phase.bubble() {
                    on_release.update(cx, |view, cx| {
                        if get(view).release() {
                            cx.notify();
                        }
                    });
                }
            });
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}
