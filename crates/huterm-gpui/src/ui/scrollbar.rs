//! Overlay scrollbar presentation shared by the terminal and list views.
//!
//! Callers own the geometry, fade, and expansion state; this only builds the
//! track and thumb elements so both views look the same.

use gpui::{Div, Hsla, div, prelude::*, px};

use crate::scroll::ScrollbarGeometry;

/// Track and thumb layers for a scrollbar at `opacity`, widened by
/// `expansion` in `0.0..=1.0`. The track appears only while expanding.
pub(crate) fn layers(
    geometry: ScrollbarGeometry,
    opacity: f32,
    expansion: f32,
    foreground: Hsla,
) -> impl Iterator<Item = Div> {
    let track = (expansion > 0.0).then(|| {
        div()
            .absolute()
            .right(px(2.0))
            .top(px(geometry.track_start))
            .w(px(8.0 + 6.0 * expansion))
            .h(px(geometry.track_size()))
            .rounded(px(4.0 + 3.0 * expansion))
            .bg(foreground.opacity(20.0 / 255.0))
            .opacity(opacity * expansion)
    });
    let thumb = div()
        .absolute()
        .right(px(2.0 + 2.0 * expansion))
        .top(px(geometry.thumb_start))
        .w(px(6.0 + 4.0 * expansion))
        .h(px(geometry.thumb_size))
        .rounded(px(3.0 + 2.0 * expansion))
        .bg(foreground.opacity(187.0 / 255.0))
        .opacity(opacity);
    track.into_iter().chain(std::iter::once(thumb))
}
