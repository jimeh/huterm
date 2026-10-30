//! Where each pane of one tab sits: its frame, title bar, and terminal
//! bounds, and the dividers between panes. One pure function of the layout,
//! the tab's content bounds, the view's zoom, and the display scale, so
//! painting, hit testing, and every pane's requested grid read the same
//! rectangles. Divider drags move one boundary through [`dragged_weights`],
//! a pane or tab drag finds where it would land through [`drop_zone`], and
//! a named layout or a balance fits its weights to the minimum pane size
//! through [`fit`].

// User input picks positions here, so a stale index must not panic.
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use gpui::{Bounds, Pixels, Size, point, px, size};
use huterm_config::{PaneTitleBar, PaneTitleBarPosition, PanesConfig};
use huterm_protocol::{
    Arrangement, PaneId, PaneLayout, PaneNode, SPLIT_TOTAL, SplitAxis,
    SplitDirection, SplitId, SplitNode,
};

use super::super::TerminalLayout;
use super::pane_moves::probe_splits;
use super::{MIN_PANE_COLUMNS, MIN_PANE_ROWS, fits_minimum};
use crate::config::WindowConfig;

/// Height of a pane title bar.
pub(super) const TITLE_BAR_HEIGHT: Pixels = px(24.0);

/// Where a split tab's panes put their title bars, from `[panes]`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct TitleBars {
    pub(super) mode: PaneTitleBar,
    pub(super) position: PaneTitleBarPosition,
}

impl TitleBars {
    pub(super) fn new(panes: &PanesConfig) -> Self {
        Self {
            mode: panes.title_bar,
            position: panes.title_bar_position,
        }
    }

    /// The height a bar takes from its pane's frame.
    fn reserved(self) -> Pixels {
        match self.mode {
            PaneTitleBar::Visible => TITLE_BAR_HEIGHT,
            PaneTitleBar::Hidden => px(0.0),
        }
    }
}

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
    /// The title bar along the frame's top or bottom, in a split tab whose
    /// bars are not hidden.
    pub(super) title_bar: Option<Bounds<Pixels>>,
    /// Where the terminal view sits: the frame beside its title bar.
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

/// Lays out `layout` inside `content`, with title bars placed as `bars`
/// says. A tab of one pane fills `content` with no title bar, as an unsplit
/// tab always has. A `zoomed` pane of a split tab fills `content` alone,
/// with its title bar and no dividers. Boundaries round to physical pixels
/// at `scale`; space too small for the dividers leaves panes empty rather
/// than overlapping.
pub(super) fn layout_panes(
    layout: &PaneLayout,
    content: Bounds<Pixels>,
    scale: f32,
    zoomed: Option<PaneId>,
    bars: TitleBars,
) -> TabPanes {
    let mut panes = TabPanes::default();
    let bars = (layout.pane_count() > 1).then_some(bars);
    if let Some(pane) =
        zoomed.filter(|pane| bars.is_some() && layout.pane(*pane).is_some())
    {
        panes.panes.push(pane_rect(pane, content, bars));
        return panes;
    }
    place(layout.root(), content, scale, bars, &mut panes);
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
    let (start, extent) = axis_range(span, split.axis);
    let gaps = count(split.children.len().saturating_sub(1));
    let available = f32::from(extent - DIVIDER_WIDTH * gaps);
    if available <= 0.0 {
        return split.children.iter().map(|child| child.weight).collect();
    }
    // The boundary sits after `index` dividers, centred on its own line.
    let offset = f32::from(
        position - start - DIVIDER_WIDTH * count(index) - DIVIDER_WIDTH / 2.0,
    );
    with_boundary(split, index, units(offset / available))
}

/// The weights of `split`'s children with the boundary after child `index`
/// at `boundary`, the sum of the weights through that child. Only the two
/// children beside it change, and each keeps at least one weight unit, so
/// a boundary carried across another change to the split keeps the other
/// children's weights.
pub(super) fn with_boundary(
    split: &SplitNode,
    index: usize,
    boundary: u32,
) -> Vec<u32> {
    let mut weights: Vec<u32> =
        split.children.iter().map(|child| child.weight).collect();
    let before: u32 = weights.iter().take(index).sum();
    if let Some([left, right]) = weights.get_mut(index..=index + 1) {
        // Validated layouts give every child at least one weight unit.
        let pair = *left + *right;
        let first = boundary.saturating_sub(before).clamp(1, pair.max(2) - 1);
        *left = first;
        *right = pair.saturating_sub(first);
    }
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
    bars: Option<TitleBars>,
    out: &mut TabPanes,
) {
    match node {
        PaneNode::Pane(pane) => {
            out.panes.push(pane_rect(pane.id, bounds, bars));
        }
        PaneNode::Split(split) => {
            let weights: Vec<u32> =
                split.children.iter().map(|child| child.weight).collect();
            let spans = child_spans(&weights, bounds, split.axis, scale);
            for (index, (child, child_bounds)) in
                split.children.iter().zip(&spans).enumerate()
            {
                place(&child.node, *child_bounds, scale, bars, out);
                if index + 1 < split.children.len() {
                    let far = axis_range(*child_bounds, split.axis);
                    let far = far.0 + far.1;
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
            }
        }
    }
}

/// The bounds of each child of a split with `weights` along `axis` inside
/// `bounds`, divider lines between them. Every boundary but the far edge
/// snaps to physical pixels at `scale`; the last child ends at the far
/// edge itself, so rounding never leaves a gap or overlap at the tab's
/// edge.
fn child_spans(
    weights: &[u32],
    bounds: Bounds<Pixels>,
    axis: SplitAxis,
    scale: f32,
) -> Vec<Bounds<Pixels>> {
    let (start, extent) = axis_range(bounds, axis);
    let last = weights.len().saturating_sub(1);
    let available = (extent - DIVIDER_WIDTH * count(last)).max(px(0.0));
    let mut prefix = 0_u32;
    let mut edge = start;
    weights
        .iter()
        .enumerate()
        .map(|(index, weight)| {
            prefix += weight;
            let far = if index == last {
                start + extent
            } else {
                start
                    + snap(available * fraction(prefix), scale)
                    + DIVIDER_WIDTH * count(index)
            };
            let child = along(bounds, axis, edge, far - edge);
            edge = far + DIVIDER_WIDTH;
            child
        })
        .collect()
}

/// One pane's rectangles in `frame`; `bars` is `None` in a tab of one
/// pane, which never shows a title bar.
fn pane_rect(
    pane: PaneId,
    frame: Bounds<Pixels>,
    bars: Option<TitleBars>,
) -> PaneRect {
    let height = bars.map_or(px(0.0), TitleBars::reserved);
    if height <= px(0.0) {
        return PaneRect {
            pane,
            frame,
            title_bar: None,
            terminal: frame,
        };
    }
    let height = height.min(frame.size.height);
    let rest = size(frame.size.width, frame.size.height - height);
    let bar = size(frame.size.width, height);
    let (title_bar, terminal) = match bars.map(|bars| bars.position) {
        Some(PaneTitleBarPosition::Bottom) => (
            Bounds::new(point(frame.origin.x, frame.bottom() - height), bar),
            Bounds::new(frame.origin, rest),
        ),
        _ => (
            Bounds::new(frame.origin, bar),
            Bounds::new(point(frame.origin.x, frame.origin.y + height), rest),
        ),
    };
    PaneRect {
        pane,
        frame,
        title_bar: Some(title_bar),
        terminal,
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

/// An arrangement fitted to one view's minimum pane size.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Fit {
    /// Every pane keeps at least the minimum size.
    Fits(Arrangement),
    /// No weights keep every pane at the minimum size in this view; the
    /// arrangement shares the space in proportion to the minimums.
    Short(Arrangement),
}

/// What a view's panes need to hold the minimum grid: its cell size and
/// window padding, through [`TerminalLayout`], and the space its title bars
/// take.
#[derive(Clone, Copy, Debug)]
pub(super) struct PaneMetrics {
    pub(super) cell: Size<Pixels>,
    pub(super) window: WindowConfig,
    pub(super) bars: TitleBars,
}

impl PaneMetrics {
    /// The grid a pane terminal of `terminal` size requests.
    fn grid(self, terminal: Size<Pixels>) -> huterm_protocol::GridSize {
        TerminalLayout::new(terminal, self.cell, self.window).grid
    }
}

/// Fits `arrangement` of `layout`'s panes inside `content`, unzoomed, with
/// title bars and dividers, so every pane keeps the minimum pane size.
///
/// Top down, each split keeps its ideal weights while every child fits
/// them. Otherwise, when the children's minimums fit, a child whose share
/// falls under its minimum takes exactly its minimum and the others divide
/// the rest in proportion to their ideal weights, repeated until none is
/// short. A child that boundary snapping leaves a physical pixel short
/// moves its boundary one physical pixel toward the neighbour with the
/// most room, at most once per boundary. When the minimums do not fit,
/// the children share the space in proportion to them. The real geometry
/// decides: an arrangement whose grids pass the check a split preflight
/// uses is [`Fit::Fits`], the ideal one unchanged when it already passes,
/// and any other is [`Fit::Short`].
pub(super) fn fit(
    layout: &PaneLayout,
    arrangement: &Arrangement,
    content: Bounds<Pixels>,
    scale: f32,
    metrics: PaneMetrics,
) -> Fit {
    let holds = |arrangement: &Arrangement| {
        layout
            .arranged(arrangement, probe_splits())
            .is_ok_and(|placed| {
                let grids: Vec<_> =
                    layout_panes(&placed, content, scale, None, metrics.bars)
                        .panes
                        .iter()
                        .map(|rect| {
                            (rect.pane, metrics.grid(rect.terminal.size))
                        })
                        .collect();
                fits_minimum(&grids)
            })
    };
    if holds(arrangement) {
        return Fit::Fits(arrangement.clone());
    }
    let minimums = Minimums::new(metrics);
    let fitted = fit_node(arrangement, content, scale, &minimums);
    if holds(&fitted) {
        Fit::Fits(fitted)
    } else {
        Fit::Short(fitted)
    }
}

/// The smallest pane frame that holds the minimum grid on each axis, a
/// reserved title bar included, exactly rather than rounded to physical
/// pixels: at a fractional scale the dividers leave child extents off the
/// pixel grid.
struct Minimums {
    width: Pixels,
    height: Pixels,
}

impl Minimums {
    fn new(metrics: PaneMetrics) -> Self {
        let large = px(1_000_000.0);
        let columns = |width: Pixels| {
            metrics.grid(size(width, large)).columns >= MIN_PANE_COLUMNS
        };
        let bar = metrics.bars.reserved();
        let rows = |height: Pixels| {
            let terminal = height - bar.min(height);
            metrics.grid(size(large, terminal)).rows >= MIN_PANE_ROWS
        };
        let padding = |value: f32| px(value) * 2.0;
        Self {
            width: smallest(
                padding(metrics.window.padding_x)
                    + metrics.cell.width * f32::from(MIN_PANE_COLUMNS),
                columns,
            ),
            height: smallest(
                bar + padding(metrics.window.padding_y)
                    + metrics.cell.height * f32::from(MIN_PANE_ROWS),
                rows,
            ),
        }
    }

    /// The smallest extent of `node` along `axis`: a pane's frame, the sum
    /// of a same-axis split's children and dividers, or the largest child
    /// of a split across the axis.
    fn of(&self, node: &Arrangement, axis: SplitAxis) -> Pixels {
        match node {
            Arrangement::Pane(_) => match axis {
                SplitAxis::Columns => self.width,
                SplitAxis::Rows => self.height,
            },
            Arrangement::Split {
                axis: inner,
                children,
            } => {
                let each =
                    children.iter().map(|(_, child)| self.of(child, axis));
                if *inner == axis {
                    each.fold(px(0.0), |sum, extent| sum + extent)
                        + DIVIDER_WIDTH
                            * count(children.len().saturating_sub(1))
                } else {
                    each.fold(px(0.0), Pixels::max)
                }
            }
        }
    }
}

/// The smallest extent for which `holds` is true, found by bisection
/// against the real grid from `estimate` rather than trusted.
fn smallest(estimate: Pixels, holds: impl Fn(Pixels) -> bool) -> Pixels {
    let mut high = estimate.max(px(1.0));
    while !holds(high) {
        high *= 2.0;
    }
    let mut low = px(0.0);
    for _ in 0..32 {
        let middle = (low + high) / 2.0;
        if holds(middle) {
            high = middle;
        } else {
            low = middle;
        }
    }
    high
}

fn fit_node(
    node: &Arrangement,
    bounds: Bounds<Pixels>,
    scale: f32,
    minimums: &Minimums,
) -> Arrangement {
    let Arrangement::Split { axis, children } = node else {
        return node.clone();
    };
    let axis = *axis;
    let (_, extent) = axis_range(bounds, axis);
    let available = (extent
        - DIVIDER_WIDTH * count(children.len().saturating_sub(1)))
    .max(px(0.0));
    let needs: Vec<Pixels> = children
        .iter()
        .map(|(_, child)| minimums.of(child, axis))
        .collect();
    let ideal: Vec<u32> = children.iter().map(|(weight, _)| *weight).collect();
    let extents = |weights: &[u32]| -> Vec<Pixels> {
        child_spans(weights, bounds, axis, scale)
            .into_iter()
            .map(|span| axis_range(span, axis).1)
            .collect()
    };
    let roomy = |weights: &[u32]| {
        extents(weights)
            .iter()
            .zip(&needs)
            .all(|(have, need)| have >= need)
    };
    let weights = if roomy(&ideal) {
        ideal
    } else if needs.iter().fold(px(0.0), |sum, need| sum + *need) <= available {
        let weights = water_fill(&ideal, &needs, available);
        nudge(weights, &needs, scale, available, extents)
    } else {
        let parts: Vec<f64> = needs
            .iter()
            .map(|need| f64::from(f32::from(*need)))
            .collect();
        apportion(&parts)
    };
    let spans = child_spans(&weights, bounds, axis, scale);
    Arrangement::Split {
        axis,
        children: weights
            .into_iter()
            .zip(children.iter().zip(spans))
            .map(|(weight, ((_, child), span))| {
                (weight, fit_node(child, span, scale, minimums))
            })
            .collect(),
    }
}

/// Weights where each child whose share of `available` would fall under
/// its minimum takes exactly the minimum, and the rest divide what is left
/// in proportion to their `ideal` weights. Each round fixes at least one
/// more child, so it ends within the child count. The minimums must fit.
fn water_fill(ideal: &[u32], needs: &[Pixels], available: Pixels) -> Vec<u32> {
    let available = f64::from(f32::from(available));
    // Each child's ideal weight, minimum, and whether it takes the minimum.
    let mut children: Vec<(f64, f64, bool)> = ideal
        .iter()
        .zip(needs)
        .map(|(weight, need)| {
            (f64::from(*weight), f64::from(f32::from(*need)), false)
        })
        .collect();
    loop {
        let taken: f64 = children
            .iter()
            .filter(|child| child.2)
            .map(|child| child.1)
            .sum();
        let free: f64 = children
            .iter()
            .filter(|child| !child.2)
            .map(|child| child.0)
            .sum();
        let rest = (available - taken).max(0.0);
        let mut changed = false;
        for child in children.iter_mut().filter(|child| !child.2) {
            if rest * child.0 / free < child.1 {
                child.2 = true;
                changed = true;
            }
        }
        if !changed || children.iter().all(|child| child.2) {
            let parts: Vec<f64> = children
                .iter()
                .map(
                    |(weight, need, fixed)| {
                        if *fixed { *need } else { rest * weight / free }
                    },
                )
                .collect();
            return apportion(&parts);
        }
    }
}

/// Moves the boundary beside each child that snapping left under its
/// minimum toward the neighbour with the most room above its own, in
/// quarter physical pixels and at most three pixels, until the child holds
/// its minimum. A move that would leave the neighbour under its own
/// minimum is not made, and each boundary moves for one child only.
fn nudge(
    mut weights: Vec<u32>,
    needs: &[Pixels],
    scale: f32,
    available: Pixels,
    extents: impl Fn(&[u32]) -> Vec<Pixels>,
) -> Vec<u32> {
    if available <= px(0.0) {
        return weights;
    }
    let pixel = 1.0 / f64::from(scale.max(1e-3));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a quarter physical pixel of a positive extent is a small share"
    )]
    let step = ((pixel / 4.0) / f64::from(f32::from(available))
        * f64::from(SPLIT_TOTAL))
    .ceil()
    .max(1.0) as u32;
    let room = |weights: &[u32], index: usize| {
        extents(weights)
            .get(index)
            .zip(needs.get(index))
            .map(|(have, need)| f32::from(*have - *need))
    };
    let mut moved = vec![false; weights.len().saturating_sub(1)];
    for child in 0..weights.len() {
        if room(&weights, child).is_none_or(|room| room >= 0.0) {
            continue;
        }
        // The boundary before the child gives way toward its previous
        // sibling, the one after it toward its next.
        let open = |boundary: usize| moved.get(boundary) == Some(&false);
        let before = child
            .checked_sub(1)
            .filter(|boundary| open(*boundary))
            .and_then(|boundary| {
                Some((boundary, boundary, room(&weights, boundary)?))
            });
        let after = Some(child).filter(|boundary| open(*boundary)).and_then(
            |boundary| {
                Some((boundary, boundary + 1, room(&weights, boundary + 1)?))
            },
        );
        let Some((boundary, giver, _)) = [before, after]
            .into_iter()
            .flatten()
            .max_by(|a, b| a.2.total_cmp(&b.2))
        else {
            continue;
        };
        if let Some(slot) = moved.get_mut(boundary) {
            *slot = true;
        }
        let mut trial = weights.clone();
        for _ in 0..12 {
            let spare = trial.get(giver).is_some_and(|weight| *weight > step);
            if !spare {
                break;
            }
            if let Some(weight) = trial.get_mut(giver) {
                *weight -= step;
            }
            if let Some(weight) = trial.get_mut(child) {
                *weight += step;
            }
            if room(&trial, giver).is_some_and(|room| room < 0.0) {
                break;
            }
            if room(&trial, child).is_some_and(|room| room >= 0.0) {
                weights = trial;
                break;
            }
        }
    }
    weights
}

/// Weights out of [`SPLIT_TOTAL`] in proportion to `parts`: each child
/// first takes one unit, and the rest go by largest remainder, ties to
/// the earlier child, so every weight is positive and they sum exactly.
fn apportion(parts: &[f64]) -> Vec<u32> {
    let total: f64 = parts.iter().sum();
    let units = u32::try_from(parts.len()).unwrap_or(u32::MAX);
    let spare = f64::from(SPLIT_TOTAL.saturating_sub(units));
    let shares: Vec<f64> = parts
        .iter()
        .map(|part| {
            if total > 0.0 {
                part / total * spare
            } else {
                spare / f64::from(units.max(1))
            }
        })
        .collect();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "each share lies in 0..=SPLIT_TOTAL"
    )]
    let mut weights: Vec<u32> = shares
        .iter()
        .map(|share| 1 + share.floor() as u32)
        .collect();
    let left = SPLIT_TOTAL.saturating_sub(weights.iter().sum());
    let mut order: Vec<(usize, f64)> = shares
        .iter()
        .map(|share| share - share.floor())
        .enumerate()
        .collect();
    order.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let extra = usize::try_from(left).unwrap_or(usize::MAX);
    for (index, _) in order.into_iter().cycle().take(extra) {
        if let Some(weight) = weights.get_mut(index) {
            *weight += 1;
        }
    }
    weights
}

/// Where a dragged pane or tab lands on a pane frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Zone {
    /// Beside the target pane, on this side.
    Edge(SplitDirection),
    /// In the target pane's place, swapping with it.
    Centre,
}

/// The zone of `frame`, title bar included, under `point`. The outer
/// quarter on each side is that edge's zone. Where two edge zones overlap
/// near a corner, the edge nearer the pointer as a fraction of the frame's
/// width or height wins, ties going to the top or bottom edge. The rest is
/// the centre, and a point outside the frame has no zone.
pub(super) fn drop_zone(
    frame: Bounds<Pixels>,
    point: gpui::Point<Pixels>,
) -> Option<Zone> {
    if !frame.contains(&point)
        || frame.size.width <= px(0.0)
        || frame.size.height <= px(0.0)
    {
        return None;
    }
    let across =
        f32::from(point.x - frame.origin.x) / f32::from(frame.size.width);
    let down =
        f32::from(point.y - frame.origin.y) / f32::from(frame.size.height);
    let edge = |fraction: f32, low: SplitDirection, high: SplitDirection| {
        if fraction < 0.25 {
            Some((low, fraction))
        } else if fraction > 0.75 {
            Some((high, 1.0 - fraction))
        } else {
            None
        }
    };
    let horizontal = edge(across, SplitDirection::Left, SplitDirection::Right);
    let vertical = edge(down, SplitDirection::Up, SplitDirection::Down);
    Some(match (horizontal, vertical) {
        (None, None) => Zone::Centre,
        (Some((side, near)), Some((_, other))) if near < other => {
            Zone::Edge(side)
        }
        (Some((side, _)), None) | (_, Some((side, _))) => Zone::Edge(side),
    })
}

/// The part of `frame` a drop in `zone` previews: the half an edge drop
/// would take, or the whole frame for a swap.
pub(super) fn zone_preview(
    frame: Bounds<Pixels>,
    zone: Zone,
) -> Bounds<Pixels> {
    let half = size(frame.size.width / 2.0, frame.size.height / 2.0);
    match zone {
        Zone::Centre => frame,
        Zone::Edge(SplitDirection::Left) => {
            Bounds::new(frame.origin, size(half.width, frame.size.height))
        }
        Zone::Edge(SplitDirection::Right) => Bounds::new(
            point(frame.origin.x + half.width, frame.origin.y),
            size(frame.size.width - half.width, frame.size.height),
        ),
        Zone::Edge(SplitDirection::Up) => {
            Bounds::new(frame.origin, size(frame.size.width, half.height))
        }
        Zone::Edge(SplitDirection::Down) => Bounds::new(
            point(frame.origin.x, frame.origin.y + half.height),
            size(frame.size.width, frame.size.height - half.height),
        ),
    }
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
