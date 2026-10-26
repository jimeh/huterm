//! Where each pane of one tab sits: its frame, title bar, and terminal
//! bounds, and the dividers between panes. One pure function of the layout,
//! the tab's content bounds, and the display scale, so painting, hit
//! testing, and every pane's requested grid read the same rectangles.

use gpui::{Bounds, Pixels, Size, point, px, size};
use huterm_protocol::{
    PaneId, PaneLayout, PaneNode, SPLIT_TOTAL, SplitAxis, SplitId,
};

/// Height of a pane title bar.
pub(super) const TITLE_BAR_HEIGHT: Pixels = px(24.0);

/// Width of the line between two panes.
pub(super) const DIVIDER_WIDTH: Pixels = px(1.0);

/// One pane's rectangles in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PaneRect {
    pub(super) pane: PaneId,
    /// The pane's whole share of the tab.
    pub(super) frame: Bounds<Pixels>,
    /// The title bar along the frame's top, in a split tab.
    pub(super) title_bar: Option<Bounds<Pixels>>,
    /// Where the terminal view sits: the frame below its title bar.
    pub(super) terminal: Bounds<Pixels>,
}

/// The line between two neighbouring children of a split.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Divider {
    pub(super) split: SplitId,
    /// The boundary after this child index.
    pub(super) index: usize,
    pub(super) axis: SplitAxis,
    pub(super) line: Bounds<Pixels>,
}

/// Every pane rectangle and divider of one tab.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct TabPanes {
    /// In traversal order.
    pub(super) panes: Vec<PaneRect>,
    pub(super) dividers: Vec<Divider>,
}

impl TabPanes {
    pub(super) fn pane(&self, pane: PaneId) -> Option<&PaneRect> {
        self.panes.iter().find(|rect| rect.pane == pane)
    }
}

/// Lays out `layout` inside `content`. A tab of one pane fills `content`
/// with no title bar, as an unsplit tab always has. Boundaries round to
/// physical pixels at `scale`; space too small for the dividers leaves
/// panes empty rather than overlapping.
pub(super) fn layout_panes(
    layout: &PaneLayout,
    content: Bounds<Pixels>,
    scale: f32,
) -> TabPanes {
    let mut panes = TabPanes::default();
    let title_bars = layout.pane_count() > 1;
    place(layout.root(), content, scale, title_bars, &mut panes);
    panes
}

fn place(
    node: &PaneNode,
    bounds: Bounds<Pixels>,
    scale: f32,
    title_bars: bool,
    out: &mut TabPanes,
) {
    match node {
        PaneNode::Pane(pane) => {
            out.panes.push(pane_rect(pane.id, bounds, title_bars));
        }
        PaneNode::Split(split) => {
            let (start, extent) = match split.axis {
                SplitAxis::Columns => (bounds.origin.x, bounds.size.width),
                SplitAxis::Rows => (bounds.origin.y, bounds.size.height),
            };
            let last = split.children.len() - 1;
            let available = (extent - DIVIDER_WIDTH * count(last)).max(px(0.0));
            let mut prefix = 0_u32;
            let mut edge = start;
            for (index, child) in split.children.iter().enumerate() {
                prefix += child.weight;
                // The last child ends at the far edge itself, so rounding
                // never leaves a gap or overlap at the tab's edge.
                let far = if index == last {
                    start + extent
                } else {
                    start
                        + snap(available * fraction(prefix), scale)
                        + DIVIDER_WIDTH * count(index)
                };
                let child_bounds = along(bounds, split.axis, edge, far - edge);
                place(&child.node, child_bounds, scale, title_bars, out);
                if index + 1 < split.children.len() {
                    out.dividers.push(Divider {
                        split: split.id,
                        index,
                        axis: split.axis,
                        line: along(bounds, split.axis, far, DIVIDER_WIDTH),
                    });
                }
                edge = far + DIVIDER_WIDTH;
            }
        }
    }
}

fn pane_rect(
    pane: PaneId,
    frame: Bounds<Pixels>,
    title_bars: bool,
) -> PaneRect {
    if !title_bars {
        return PaneRect {
            pane,
            frame,
            title_bar: None,
            terminal: frame,
        };
    }
    let height = TITLE_BAR_HEIGHT.min(frame.size.height);
    PaneRect {
        pane,
        frame,
        title_bar: Some(Bounds::new(
            frame.origin,
            size(frame.size.width, height),
        )),
        terminal: Bounds::new(
            point(frame.origin.x, frame.origin.y + height),
            size(frame.size.width, frame.size.height - height),
        ),
    }
}

/// `bounds` cut to `[start, start + length)` along `axis`.
fn along(
    bounds: Bounds<Pixels>,
    axis: SplitAxis,
    start: Pixels,
    length: Pixels,
) -> Bounds<Pixels> {
    let length = length.max(px(0.0));
    match axis {
        SplitAxis::Columns => Bounds::new(
            point(start, bounds.origin.y),
            Size::new(length, bounds.size.height),
        ),
        SplitAxis::Rows => Bounds::new(
            point(bounds.origin.x, start),
            Size::new(bounds.size.width, length),
        ),
    }
}

/// `prefix` of [`SPLIT_TOTAL`] as a fraction. Both fit in an `f32`
/// mantissa, so the division is exact to one rounding.
#[expect(
    clippy::cast_precision_loss,
    reason = "weights are at most 1,000,000, below f32's 2^24 exact range"
)]
fn fraction(prefix: u32) -> f32 {
    prefix as f32 / SPLIT_TOTAL as f32
}

/// A small count as a multiplier.
fn count(value: usize) -> f32 {
    f32::from(u16::try_from(value).unwrap_or(u16::MAX))
}

/// Rounds a length to whole physical pixels.
fn snap(length: Pixels, scale: f32) -> Pixels {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    px((f32::from(length) * scale).round() / scale)
}

#[cfg(test)]
mod tests;
