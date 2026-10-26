//! Where each pane of one tab sits: its frame, title bar, and terminal
//! bounds, and the dividers between panes. One pure function of the layout,
//! the tab's content bounds, the view's zoom, and the display scale, so
//! painting, hit testing, and every pane's requested grid read the same
//! rectangles. Divider drags move one boundary through [`dragged_weights`].

use gpui::{Bounds, Pixels, Size, point, px, size};
use huterm_protocol::{
    PaneId, PaneLayout, PaneNode, SPLIT_TOTAL, SplitAxis, SplitId, SplitNode,
};

/// Height of a pane title bar.
pub(super) const TITLE_BAR_HEIGHT: Pixels = px(24.0);

/// Width of the line between two panes.
pub(super) const DIVIDER_WIDTH: Pixels = px(1.0);

/// Width of the zone across a divider that a press grabs.
pub(super) const GRAB_WIDTH: Pixels = px(8.0);

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
    /// Where a press grabs the divider: [`GRAB_WIDTH`] across its line,
    /// inside the split's bounds.
    pub(super) grab: Bounds<Pixels>,
    /// The split's whole bounds, across which a drag moves the boundary.
    pub(super) span: Bounds<Pixels>,
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
/// with no title bar, as an unsplit tab always has. A `zoomed` pane of a
/// split tab fills `content` alone, with its title bar and no dividers.
/// Boundaries round to physical pixels at `scale`; space too small for the
/// dividers leaves panes empty rather than overlapping.
pub(super) fn layout_panes(
    layout: &PaneLayout,
    content: Bounds<Pixels>,
    scale: f32,
    zoomed: Option<PaneId>,
) -> TabPanes {
    let mut panes = TabPanes::default();
    let title_bars = layout.pane_count() > 1;
    if let Some(pane) =
        zoomed.filter(|pane| title_bars && layout.pane(*pane).is_some())
    {
        panes.panes.push(pane_rect(pane, content, true));
        return panes;
    }
    place(layout.root(), content, scale, title_bars, &mut panes);
    panes
}

/// The weights of `split`'s children once the boundary after child
/// `index` moves to `position`, along the split's axis inside `span`, the
/// split's bounds. Only the two children beside the boundary change, and
/// each keeps at least one weight unit.
pub(super) fn dragged_weights(
    split: &SplitNode,
    index: usize,
    span: Bounds<Pixels>,
    position: Pixels,
) -> Vec<u32> {
    let mut weights: Vec<u32> =
        split.children.iter().map(|child| child.weight).collect();
    if index + 1 >= weights.len() {
        return weights;
    }
    let (start, extent) = axis_range(span, split.axis);
    let gaps = count(weights.len() - 1);
    let available = f32::from(extent - DIVIDER_WIDTH * gaps);
    if available <= 0.0 {
        return weights;
    }
    let before: u32 = weights[..index].iter().sum();
    let pair = weights[index] + weights[index + 1];
    // The boundary sits after `index` dividers, centred on its own line.
    let offset = f32::from(
        position - start - DIVIDER_WIDTH * count(index) - DIVIDER_WIDTH / 2.0,
    );
    let wanted = units(offset / available).saturating_sub(before);
    let first = wanted.clamp(1, pair - 1);
    weights[index] = first;
    weights[index + 1] = pair - first;
    weights
}

/// Where the boundary after child `index` of `split` sits along its axis,
/// at the centre of its divider line, inside `span`.
pub(super) fn boundary_position(
    split: &SplitNode,
    index: usize,
    span: Bounds<Pixels>,
) -> Pixels {
    let (start, extent) = axis_range(span, split.axis);
    let gaps = count(split.children.len().saturating_sub(1));
    let available = (extent - DIVIDER_WIDTH * gaps).max(px(0.0));
    let prefix: u32 = split
        .children
        .iter()
        .take(index + 1)
        .map(|child| child.weight)
        .sum();
    start
        + available * fraction(prefix)
        + DIVIDER_WIDTH * count(index)
        + DIVIDER_WIDTH / 2.0
}

/// The boundary a resize of `pane` toward the leading or trailing side of
/// `axis` moves: in the nearest ancestor split on that axis where the pane
/// has a neighbour on that side, the split and the index of the child
/// before the boundary.
pub(super) fn resize_boundary(
    layout: &PaneLayout,
    pane: PaneId,
    axis: SplitAxis,
    leading: bool,
) -> Option<(&SplitNode, usize)> {
    let mut path = Vec::new();
    if !path_to(layout.root(), pane, &mut path) {
        return None;
    }
    path.into_iter().rev().find_map(|(split, child)| {
        if split.axis != axis {
            return None;
        }
        if leading {
            (child > 0).then(|| (split, child - 1))
        } else {
            (child + 1 < split.children.len()).then_some((split, child))
        }
    })
}

/// Collects the splits from `node` down to `pane`, each with the index of
/// the child that holds it; returns whether `pane` is under `node`.
fn path_to<'a>(
    node: &'a PaneNode,
    pane: PaneId,
    path: &mut Vec<(&'a SplitNode, usize)>,
) -> bool {
    match node {
        PaneNode::Pane(info) => info.id == pane,
        PaneNode::Split(split) => {
            for (index, child) in split.children.iter().enumerate() {
                path.push((split, index));
                if path_to(&child.node, pane, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
    }
}

/// `bounds`' start and extent along `axis`.
fn axis_range(bounds: Bounds<Pixels>, axis: SplitAxis) -> (Pixels, Pixels) {
    match axis {
        SplitAxis::Columns => (bounds.origin.x, bounds.size.width),
        SplitAxis::Rows => (bounds.origin.y, bounds.size.height),
    }
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
            let (start, extent) = axis_range(bounds, split.axis);
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
                    let reach = (GRAB_WIDTH - DIVIDER_WIDTH) / 2.0;
                    let grab =
                        along(bounds, split.axis, far - reach, GRAB_WIDTH)
                            .intersect(&bounds);
                    out.dividers.push(Divider {
                        split: split.id,
                        index,
                        axis: split.axis,
                        line: along(bounds, split.axis, far, DIVIDER_WIDTH),
                        grab,
                        span: bounds,
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

/// A fraction of the split as weight units, rounded and clamped to
/// [`SPLIT_TOTAL`].
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=SPLIT_TOTAL before the cast"
)]
fn units(fraction: f32) -> u32 {
    let total = f64::from(SPLIT_TOTAL);
    (f64::from(fraction) * total).round().clamp(0.0, total) as u32
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
