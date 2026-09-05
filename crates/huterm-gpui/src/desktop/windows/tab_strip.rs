use super::{Bounds, CONTROL_SIZE, Pixels, TAB_HEIGHT, point, px, size};
use std::time::Duration;

/// Pixel geometry shared by rendering, wheel scrolling and drag hit testing.
#[derive(Clone, Debug)]
pub(super) struct TabStrip {
    pub(super) bounds: Bounds<Pixels>,
    pub(super) vertical: bool,
    pub(super) count: usize,
    pub(super) extent: Pixels,
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
        count: usize,
        offset: Pixels,
    ) -> Self {
        let available = if vertical {
            bounds.size.height
        } else {
            bounds.size.width
        };
        let viewport = (available - CONTROL_SIZE).max(px(0.0));
        let extent = if vertical {
            TAB_HEIGHT
        } else {
            (viewport / count.max(1) as f32).max(px(120.0))
        };
        if vertical {
            bounds.size.height = viewport.min(extent * count as f32);
        } else {
            bounds.size.width = viewport;
        }
        let mut strip = Self {
            bounds,
            vertical,
            count,
            extent,
            offset: px(0.0),
        };
        strip.offset = offset.clamp(px(0.0), strip.max_offset());
        strip
    }
    pub(super) fn available(&self) -> Pixels {
        if self.vertical {
            self.bounds.size.height
        } else {
            self.bounds.size.width
        }
    }
    pub(super) fn max_offset(&self) -> Pixels {
        (self.extent * self.count as f32 - self.available()).max(px(0.0))
    }
    fn axis(&self, pointer: gpui::Point<Pixels>) -> Pixels {
        if self.vertical {
            pointer.y - self.bounds.origin.y
        } else {
            pointer.x - self.bounds.origin.x
        }
    }
    pub(super) fn reveal(&self, index: usize) -> Pixels {
        let start = self.extent * index as f32;
        let end = start + self.extent;
        let offset = if start < self.offset {
            start
        } else if end > self.offset + self.available() {
            end - self.available()
        } else {
            self.offset
        };
        offset.clamp(px(0.0), self.max_offset())
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped nonnegative insertion slot"
    )]
    pub(super) fn slot(&self, pointer: gpui::Point<Pixels>) -> usize {
        (((self.axis(pointer).clamp(px(0.0), self.available()) + self.offset)
            / self.extent
            + 0.5)
            .floor() as usize)
            .min(self.count)
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
        let offset = (self.extent * slot.min(self.count) as f32 - self.offset)
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
    pub(super) fn preview(
        &self,
        pointer: gpui::Point<Pixels>,
    ) -> Bounds<Pixels> {
        let extent = self.extent.min(self.available());
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

    fn strip(vertical: bool, count: usize, offset: f32) -> TabStrip {
        TabStrip::new(
            Bounds::new(point(px(20.0), px(40.0)), size(px(628.0), px(220.0))),
            vertical,
            count,
            px(offset),
        )
    }

    #[test]
    fn horizontal_tabs_share_width_until_minimum_then_scroll() {
        for (count, width) in
            [(1, 600.0), (2, 300.0), (3, 200.0), (5, 120.0), (6, 120.0)]
        {
            let tabs = strip(false, count, 0.0);
            assert_eq!(tabs.extent, px(width));
        }
        assert_eq!(strip(false, 5, 0.0).max_offset(), px(0.0));
        assert_eq!(strip(false, 6, 0.0).max_offset(), px(120.0));
    }

    #[test]
    fn vertical_rows_stay_fixed_and_leave_new_button_visible() {
        let fitting = strip(true, 3, 0.0);
        assert_eq!(fitting.extent, px(32.0));
        assert_eq!(fitting.available(), px(96.0));
        assert_eq!(fitting.bounds.size.width, px(628.0));
        let overflowing = strip(true, 20, 0.0);
        assert_eq!(overflowing.available(), px(192.0));
        assert_eq!(overflowing.max_offset(), px(448.0));
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
            tabs.offset = tabs.extent * 3.0;
            let along = tabs.extent * 0.6;
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
                        point(px(0.0), tabs.extent)
                    } else {
                        point(tabs.extent, px(0.0))
                    }
            );
        }
    }
}
