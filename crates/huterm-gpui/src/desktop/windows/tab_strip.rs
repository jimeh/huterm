use super::{Bounds, CONTROL_SIZE, Pixels, TAB_HEIGHT, point, px, size};
use std::time::Duration;

/// How tab extents along the strip axis are chosen.
#[derive(Clone, Debug)]
pub(super) enum TabExtents {
    /// Vertical rows of `TAB_HEIGHT`, or horizontal tabs sharing the width
    /// equally down to a 120-point minimum.
    Uniform(usize),
    /// Horizontal tabs sized to their titles, already clamped by the caller.
    /// Vertical strips ignore the widths and keep uniform rows.
    Fit(Vec<Pixels>),
}

/// Pixel geometry shared by rendering, wheel scrolling and drag hit testing.
#[derive(Clone, Debug)]
pub(super) struct TabStrip {
    pub(super) bounds: Bounds<Pixels>,
    pub(super) vertical: bool,
    pub(super) count: usize,
    /// `count + 1` ascending tab boundaries along the axis, starting at zero.
    edges: Vec<Pixels>,
    pub(super) offset: Pixels,
}

#[expect(
    clippy::cast_precision_loss,
    reason = "tab counts fit practical pixel geometry"
)]
impl TabStrip {
    pub(super) fn new(
        mut bounds: Bounds<Pixels>,
        vertical: bool,
        extents: TabExtents,
        offset: Pixels,
    ) -> Self {
        let available = if vertical {
            bounds.size.height
        } else {
            bounds.size.width
        };
        let viewport = (available - CONTROL_SIZE).max(px(0.0));
        let fit = matches!(extents, TabExtents::Fit(_)) && !vertical;
        let extents = match extents {
            TabExtents::Fit(widths) if !vertical => widths,
            TabExtents::Fit(widths) => vec![TAB_HEIGHT; widths.len()],
            TabExtents::Uniform(count) if vertical => vec![TAB_HEIGHT; count],
            TabExtents::Uniform(count) => {
                vec![(viewport / count.max(1) as f32).max(px(120.0)); count]
            }
        };
        let count = extents.len();
        let mut edges = Vec::with_capacity(count + 1);
        let mut end = px(0.0);
        edges.push(end);
        for extent in extents {
            end += extent;
            edges.push(end);
        }
        if vertical {
            bounds.size.height = viewport.min(end);
        } else if fit {
            // The new-tab button follows the last tab until the bar overflows.
            bounds.size.width = viewport.min(end);
        } else {
            bounds.size.width = viewport;
        }
        let mut strip = Self {
            bounds,
            vertical,
            count,
            edges,
            offset: px(0.0),
        };
        strip.offset = offset.clamp(px(0.0), strip.max_offset());
        strip
    }
    /// Where tab `index` begins along the axis, before scrolling.
    pub(super) fn start(&self, index: usize) -> Pixels {
        self.edges[index.min(self.count)]
    }
    pub(super) fn tab_extent(&self, index: usize) -> Pixels {
        self.start(index + 1) - self.start(index)
    }
    pub(super) fn available(&self) -> Pixels {
        if self.vertical {
            self.bounds.size.height
        } else {
            self.bounds.size.width
        }
    }
    pub(super) fn max_offset(&self) -> Pixels {
        (self.start(self.count) - self.available()).max(px(0.0))
    }
    fn axis(&self, pointer: gpui::Point<Pixels>) -> Pixels {
        if self.vertical {
            pointer.y - self.bounds.origin.y
        } else {
            pointer.x - self.bounds.origin.x
        }
    }
    pub(super) fn reveal(&self, index: usize) -> Pixels {
        let start = self.start(index);
        let end = self.start(index + 1);
        let offset = if start < self.offset {
            start
        } else if end > self.offset + self.available() {
            end - self.available()
        } else {
            self.offset
        };
        offset.clamp(px(0.0), self.max_offset())
    }
    /// The insertion slot at the tab boundary nearest the pointer.
    pub(super) fn slot(&self, pointer: gpui::Point<Pixels>) -> usize {
        let position =
            self.axis(pointer).clamp(px(0.0), self.available()) + self.offset;
        let next = self
            .edges
            .partition_point(|edge| *edge < position)
            .min(self.count);
        if next == 0 {
            return 0;
        }
        let previous = next - 1;
        if position - self.edges[previous] < self.edges[next] - position {
            previous
        } else {
            next
        }
    }
    pub(super) fn autoscroll(
        &self,
        pointer: gpui::Point<Pixels>,
        elapsed: Duration,
    ) -> Pixels {
        let axis = self.axis(pointer);
        let edge = CONTROL_SIZE.min(self.available() / 2.0);
        let direction = if axis < edge {
            -1.0
        } else if axis > self.available() - edge {
            1.0
        } else {
            0.0
        };
        (self.offset
            + px(direction
                * 360.0
                * elapsed.min(Duration::from_millis(50)).as_secs_f32()))
        .clamp(px(0.0), self.max_offset())
    }
    pub(super) fn toward(&self, target: Pixels, elapsed: Duration) -> Pixels {
        let remaining = target - self.offset;
        if remaining.abs() < px(0.5) {
            return target;
        }
        self.offset
            + remaining
                * (elapsed.min(Duration::from_millis(50)).as_secs_f32() / 0.1)
    }
    pub(super) fn marker(&self, slot: usize) -> Bounds<Pixels> {
        let offset = (self.start(slot) - self.offset)
            .clamp(px(0.0), (self.available() - px(2.0)).max(px(0.0)));
        if self.vertical {
            Bounds::new(
                self.bounds.origin + point(px(0.0), offset),
                size(self.bounds.size.width, px(2.0)),
            )
        } else {
            Bounds::new(
                self.bounds.origin + point(offset, px(0.0)),
                size(px(2.0), self.bounds.size.height),
            )
        }
    }
    /// A drag preview the size of tab `index`, centered on the pointer.
    pub(super) fn preview(
        &self,
        pointer: gpui::Point<Pixels>,
        index: usize,
    ) -> Bounds<Pixels> {
        let extent = self.tab_extent(index).min(self.available());
        let offset = (self.axis(pointer) - extent / 2.0)
            .clamp(px(0.0), (self.available() - extent).max(px(0.0)));
        if self.vertical {
            Bounds::new(
                self.bounds.origin + point(px(0.0), offset),
                size(self.bounds.size.width, extent),
            )
        } else {
            Bounds::new(
                self.bounds.origin + point(offset, px(0.0)),
                size(extent, self.bounds.size.height),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Bounds<Pixels> {
        Bounds::new(point(px(20.0), px(40.0)), size(px(628.0), px(220.0)))
    }

    fn strip(vertical: bool, count: usize, offset: f32) -> TabStrip {
        TabStrip::new(
            bounds(),
            vertical,
            TabExtents::Uniform(count),
            px(offset),
        )
    }

    fn fit(widths: &[f32], offset: f32) -> TabStrip {
        TabStrip::new(
            bounds(),
            false,
            TabExtents::Fit(widths.iter().map(|width| px(*width)).collect()),
            px(offset),
        )
    }

    #[test]
    fn horizontal_tabs_share_width_until_minimum_then_scroll() {
        for (count, width) in
            [(1, 600.0), (2, 300.0), (3, 200.0), (5, 120.0), (6, 120.0)]
        {
            let tabs = strip(false, count, 0.0);
            assert_eq!(tabs.tab_extent(0), px(width));
            assert_eq!(tabs.available(), px(600.0));
        }
        assert_eq!(strip(false, 5, 0.0).max_offset(), px(0.0));
        assert_eq!(strip(false, 6, 0.0).max_offset(), px(120.0));
    }

    #[test]
    fn vertical_rows_stay_fixed_and_leave_new_button_visible() {
        let fitting = strip(true, 3, 0.0);
        assert_eq!(fitting.tab_extent(0), px(32.0));
        assert_eq!(fitting.available(), px(96.0));
        assert_eq!(fitting.bounds.size.width, px(628.0));
        let overflowing = strip(true, 20, 0.0);
        assert_eq!(overflowing.available(), px(192.0));
        assert_eq!(overflowing.max_offset(), px(448.0));
        let ignored = TabStrip::new(
            bounds(),
            true,
            TabExtents::Fit(vec![px(300.0); 2]),
            px(0.0),
        );
        assert_eq!(ignored.tab_extent(1), TAB_HEIGHT);
    }

    #[test]
    fn fit_tabs_keep_their_widths_and_shrink_the_bar_to_content() {
        let tabs = fit(&[100.0, 240.0, 96.0], 0.0);
        assert_eq!(tabs.start(1), px(100.0));
        assert_eq!(tabs.start(2), px(340.0));
        assert_eq!(tabs.tab_extent(1), px(240.0));
        assert_eq!(tabs.available(), px(436.0));
        assert_eq!(tabs.max_offset(), px(0.0));
        let overflowing = fit(&[240.0; 4], 0.0);
        assert_eq!(overflowing.available(), px(600.0));
        assert_eq!(overflowing.max_offset(), px(360.0));
    }

    #[test]
    fn fit_slots_markers_previews_and_reveal_follow_unequal_widths() {
        let tabs = fit(&[100.0, 240.0, 96.0], 0.0);
        let at = |x: f32| tabs.bounds.origin + point(px(x), px(5.0));
        assert_eq!(tabs.slot(at(49.0)), 0);
        assert_eq!(tabs.slot(at(51.0)), 1);
        assert_eq!(tabs.slot(at(219.0)), 1);
        assert_eq!(tabs.slot(at(221.0)), 2);
        assert_eq!(tabs.slot(at(10_000.0)), 3);
        assert_eq!(tabs.slot(at(-500.0)), 0);
        assert_eq!(tabs.marker(2).origin.x, tabs.bounds.origin.x + px(340.0));
        assert_eq!(tabs.preview(at(200.0), 1).size.width, px(240.0));
        let wide = fit(&[240.0; 4], 0.0);
        assert_eq!(wide.reveal(1), px(0.0));
        assert_eq!(wide.reveal(3), px(360.0));
        let scrolled = fit(&[240.0; 4], 300.0);
        assert_eq!(scrolled.reveal(0), px(0.0));
        assert_eq!(scrolled.reveal(2), px(300.0));
    }

    #[test]
    fn user_scroll_is_preserved_until_explicit_reveal_and_clamped_after_close()
    {
        for vertical in [false, true] {
            let tabs = strip(vertical, 20, 155.5);
            assert_eq!(tabs.offset, px(155.5));
            assert_eq!(tabs.reveal(0), px(0.0));
            assert_eq!(tabs.reveal(19), tabs.max_offset());
            assert_eq!(strip(vertical, 1, 155.5).offset, px(0.0));
            assert_eq!(strip(vertical, 20, -20.0).offset, px(0.0));
        }
    }

    #[test]
    fn stationary_edge_drag_scrolls_pixels_and_caps_elapsed_time() {
        for vertical in [false, true] {
            let mut tabs = strip(vertical, 20, 0.0);
            let end = tabs.bounds.origin + point(px(10_000.0), px(10_000.0));
            let tick = Duration::from_millis(16);
            let first = tabs.autoscroll(end, tick);
            assert_eq!(first, px(5.76));
            tabs.offset = first;
            assert_eq!(tabs.autoscroll(end, tick), px(11.52));
            assert_eq!(
                tabs.autoscroll(end, Duration::from_secs(10)),
                first + px(18.0)
            );
            tabs.offset = tabs.max_offset() - px(1.0);
            assert_eq!(tabs.autoscroll(end, tick), tabs.max_offset());
            assert!(tabs.autoscroll(tabs.bounds.origin, tick) < tabs.offset);
        }
    }

    #[test]
    fn arrow_scroll_animation_advances_without_overshoot() {
        let tabs = strip(false, 20, 100.0);
        assert_eq!(
            tabs.toward(px(200.0), Duration::from_millis(16)),
            px(116.0)
        );
        assert_eq!(tabs.toward(px(200.0), Duration::from_secs(10)), px(150.0));
        assert_eq!(
            tabs.toward(px(100.25), Duration::from_millis(16)),
            px(100.25)
        );
    }

    #[test]
    fn drop_slots_include_pixel_offset_and_ignore_perpendicular_excursion() {
        for vertical in [false, true] {
            let mut tabs = strip(vertical, 20, 0.0);
            let extent = tabs.tab_extent(0);
            tabs.offset = extent * 3.0;
            let along = extent * 0.6;
            let pointer = tabs.bounds.origin
                + if vertical {
                    point(px(-10_000.0), along)
                } else {
                    point(along, px(-10_000.0))
                };
            assert_eq!(tabs.slot(pointer), 4);
            assert_eq!(
                tabs.slot(
                    tabs.bounds.origin - point(px(10_000.0), px(10_000.0))
                ),
                3
            );
            assert_eq!(
                tabs.marker(4).origin,
                tabs.bounds.origin
                    + if vertical {
                        point(px(0.0), extent)
                    } else {
                        point(extent, px(0.0))
                    }
            );
        }
    }
}
