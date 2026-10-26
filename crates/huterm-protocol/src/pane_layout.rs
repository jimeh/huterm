//! A tab's split tree of panes: its invariants, the transforms that change
//! it, and the geometry navigation uses.
//!
//! A layout is an n-ary tree. Each split divides its space along one axis
//! among two or more children, whose integer weights sum to
//! [`SPLIT_TOTAL`]. Splitting along a parent's axis adds a sibling, and a
//! child split never shares its parent's axis, so equal columns stay one
//! node. A [`PaneLayout`] can only be built through [`PaneLayout::new`] or
//! the transforms, which all validate, so every value upholds the
//! invariants.

use std::collections::HashSet;
use std::fmt;

use crate::{PaneId, SplitId, TerminalId};

/// The sum of the weights of every split's children.
pub const SPLIT_TOTAL: u32 = 1_000_000;

/// The most children one split holds. The largest child of a full split
/// still holds `SPLIT_TOTAL / 256`, so it can absorb the rounding a flatten
/// leaves.
pub const MAX_SPLIT_CHILDREN: usize = 256;

/// The extent of the root on each axis in normalized geometry.
pub const NORMALIZED_EXTENT: u128 = 1 << 96;

/// How a split arranges its children.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SplitAxis {
    /// Side by side, left to right.
    Columns,
    /// Stacked, top to bottom.
    Rows,
}

/// A side of a pane: where a split puts the new pane, or which way
/// navigation moves.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SplitDirection {
    /// Toward the left edge.
    Left,
    /// Toward the right edge.
    Right,
    /// Toward the top edge.
    Up,
    /// Toward the bottom edge.
    Down,
}

impl SplitDirection {
    /// The axis a split toward this side divides.
    #[must_use]
    pub const fn axis(self) -> SplitAxis {
        match self {
            Self::Left | Self::Right => SplitAxis::Columns,
            Self::Up | Self::Down => SplitAxis::Rows,
        }
    }

    /// Whether this side comes first along its axis.
    const fn leading(self) -> bool {
        matches!(self, Self::Left | Self::Up)
    }
}

/// One pane and the terminal it shows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaneInfo {
    /// Stable pane identity.
    pub id: PaneId,
    /// Terminal displayed by the pane.
    pub terminal_id: TerminalId,
    /// Launched program name used when the terminal has no title.
    pub fallback_name: String,
}

/// A node of a layout: a pane, or a split of further nodes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PaneNode {
    /// A leaf showing one terminal.
    Pane(PaneInfo),
    /// A division of space among children.
    Split(SplitNode),
}

impl PaneNode {
    /// The node's identity.
    #[must_use]
    pub fn id(&self) -> NodeId {
        match self {
            Self::Pane(pane) => NodeId::Pane(pane.id),
            Self::Split(split) => NodeId::Split(split.id),
        }
    }
}

/// A split of space along one axis.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SplitNode {
    /// Stable split identity.
    pub id: SplitId,
    /// How the children are arranged.
    pub axis: SplitAxis,
    /// Children in order along the axis.
    pub children: Vec<SplitChild>,
}

/// One child of a split and its share of the split's space.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SplitChild {
    /// Share of the split, out of [`SPLIT_TOTAL`].
    pub weight: u32,
    /// The child node.
    pub node: PaneNode,
}

/// The identity of a split's child.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NodeId {
    /// A pane.
    Pane(PaneId),
    /// A nested split.
    Split(SplitId),
}

/// Why a layout or a transform was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutError {
    /// A split has fewer than two children.
    TooFewChildren,
    /// A split has more than [`MAX_SPLIT_CHILDREN`] children.
    TooManyChildren,
    /// A child split shares its parent's axis without reaching the child
    /// limit.
    SameAxisChild,
    /// A weight is zero, or a split's weights do not sum to
    /// [`SPLIT_TOTAL`], or their count differs from the children's.
    InvalidWeights,
    /// A pane, split, or terminal identity appears twice.
    DuplicateId,
    /// The pane is not in the layout.
    UnknownPane,
    /// The split is not in the layout.
    UnknownSplit,
    /// The split's children are not the ones expected.
    ChildrenChanged,
    /// The pane's share is too small to divide.
    PaneTooSmall,
    /// The pane is the layout's only pane.
    LastPane,
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TooFewChildren => "a split has fewer than two children",
            Self::TooManyChildren => "a split has too many children",
            Self::SameAxisChild => "a split has a child on its own axis",
            Self::InvalidWeights => "split weights are invalid",
            Self::DuplicateId => "a pane, split, or terminal repeats",
            Self::UnknownPane => "pane is not in the layout",
            Self::UnknownSplit => "split is not in the layout",
            Self::ChildrenChanged => "split children changed",
            Self::PaneTooSmall => "pane is too small to split",
            Self::LastPane => "pane is the layout's only pane",
        })
    }
}

impl std::error::Error for LayoutError {}

/// A rectangle in normalized geometry: `[x0, x1)` by `[y0, y1)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NormalizedRect {
    /// Left edge.
    pub x0: u128,
    /// Top edge.
    pub y0: u128,
    /// Right edge, exclusive.
    pub x1: u128,
    /// Bottom edge, exclusive.
    pub y1: u128,
}

impl NormalizedRect {
    fn empty(self) -> bool {
        self.x0 == self.x1 || self.y0 == self.y1
    }
}

/// A tab's validated split tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaneLayout {
    root: PaneNode,
}

impl PaneLayout {
    /// A layout of one pane.
    #[must_use]
    pub fn single(pane: PaneInfo) -> Self {
        Self {
            root: PaneNode::Pane(pane),
        }
    }

    /// Validates a tree.
    ///
    /// # Errors
    /// Rejects a split with too few or too many children, invalid weights,
    /// a child split on its parent's axis below the child limit, or a
    /// repeated pane, split, or terminal identity.
    pub fn new(root: PaneNode) -> Result<Self, LayoutError> {
        let mut ids = Ids::default();
        validate(&root, None, &mut ids)?;
        Ok(Self { root })
    }

    /// The root node.
    #[must_use]
    pub fn root(&self) -> &PaneNode {
        &self.root
    }

    /// The panes in traversal order: depth first, children in order.
    #[must_use]
    pub fn panes(&self) -> Vec<&PaneInfo> {
        let mut panes = Vec::new();
        collect_panes(&self.root, &mut panes);
        panes
    }

    /// The first pane in traversal order.
    #[must_use]
    pub fn first_pane(&self) -> &PaneInfo {
        let mut node = &self.root;
        loop {
            match node {
                PaneNode::Pane(pane) => return pane,
                PaneNode::Split(split) => node = &split.children[0].node,
            }
        }
    }

    /// Looks up a pane.
    #[must_use]
    pub fn pane(&self, id: PaneId) -> Option<&PaneInfo> {
        self.panes().into_iter().find(|pane| pane.id == id)
    }

    /// The number of panes.
    #[must_use]
    pub fn pane_count(&self) -> usize {
        count_panes(&self.root)
    }

    /// Looks up a split.
    #[must_use]
    pub fn split_node(&self, id: SplitId) -> Option<&SplitNode> {
        find_split(&self.root, id)
    }

    /// Whether `target` can be split toward `direction`, checked without
    /// identities so a caller can refuse before allocating any.
    ///
    /// # Errors
    /// Rejects an unknown target, a target whose weight is 1, and a parent
    /// already at [`MAX_SPLIT_CHILDREN`].
    pub fn check_split(
        &self,
        target: PaneId,
        direction: SplitDirection,
    ) -> Result<(), LayoutError> {
        match &self.root {
            PaneNode::Pane(leaf) if leaf.id == target => Ok(()),
            PaneNode::Pane(_) => Err(LayoutError::UnknownPane),
            PaneNode::Split(node) => check_within(node, target, direction),
        }
    }

    /// Every split identity in traversal order.
    #[must_use]
    pub fn split_ids(&self) -> Vec<SplitId> {
        let mut ids = Vec::new();
        collect_splits(&self.root, &mut ids);
        ids
    }

    /// Splits `target` toward `direction`, adding `pane`.
    ///
    /// When the target's parent splits on that direction's axis, the new
    /// pane becomes the target's sibling and the two divide the target's
    /// weight, the target keeping the larger half. Otherwise the target is
    /// replaced by a new split, identified by `split`, of two equal
    /// children.
    ///
    /// # Errors
    /// Rejects an unknown target, a target whose weight is 1, a parent
    /// already at [`MAX_SPLIT_CHILDREN`], and a repeated identity.
    pub fn with_split(
        &self,
        target: PaneId,
        direction: SplitDirection,
        pane: PaneInfo,
        split: SplitId,
    ) -> Result<Self, LayoutError> {
        self.check_split(target, direction)?;
        let pane = PaneNode::Pane(pane);
        let root = match self.root.clone() {
            PaneNode::Pane(leaf) if leaf.id == target => {
                new_split(split, direction, PaneNode::Pane(leaf), pane)
            }
            PaneNode::Pane(_) => return Err(LayoutError::UnknownPane),
            PaneNode::Split(mut node) => {
                if !split_within(&mut node, target, direction, pane, split)? {
                    return Err(LayoutError::UnknownPane);
                }
                PaneNode::Split(node)
            }
        };
        Self::new(root)
    }

    /// Removes `target`. Its weight goes to its previous sibling, or the
    /// next one when it was first. A split left with one child is replaced
    /// by that child, and a child split that then shares its new parent's
    /// axis is flattened into it.
    ///
    /// # Errors
    /// Rejects an unknown target and the layout's only pane.
    pub fn without_pane(&self, target: PaneId) -> Result<Self, LayoutError> {
        let root = match self.root.clone() {
            PaneNode::Pane(leaf) if leaf.id == target => {
                return Err(LayoutError::LastPane);
            }
            PaneNode::Pane(_) => return Err(LayoutError::UnknownPane),
            PaneNode::Split(mut node) => {
                if !remove_within(&mut node, target) {
                    return Err(LayoutError::UnknownPane);
                }
                if node.children.len() == 1 {
                    node.children.remove(0).node
                } else {
                    PaneNode::Split(node)
                }
            }
        };
        Self::new(root)
    }

    /// Replaces the weights of `split`'s children, which must be exactly
    /// `children`, in order.
    ///
    /// # Errors
    /// Rejects an unknown split, different children, and invalid weights.
    pub fn with_weights(
        &self,
        split: SplitId,
        children: &[NodeId],
        weights: &[u32],
    ) -> Result<Self, LayoutError> {
        let mut root = self.root.clone();
        let node = find_split_mut(&mut root, split)
            .ok_or(LayoutError::UnknownSplit)?;
        if node.children.len() != children.len()
            || node
                .children
                .iter()
                .zip(children)
                .any(|(child, id)| child.node.id() != *id)
        {
            return Err(LayoutError::ChildrenChanged);
        }
        if weights.len() != children.len() {
            return Err(LayoutError::InvalidWeights);
        }
        for (child, &weight) in node.children.iter_mut().zip(weights) {
            child.weight = weight;
        }
        Self::new(root)
    }

    /// Each pane's rectangle in normalized geometry, in traversal order.
    ///
    /// The root spans [`NORMALIZED_EXTENT`] on each axis. A split's child
    /// `i` ends at `start + floor(extent × prefix / SPLIT_TOTAL)`, where
    /// `prefix` sums the weights through child `i`, so neighbours share
    /// their boundary exactly. Extreme nesting can leave a pane with zero
    /// extent.
    #[must_use]
    pub fn normalized_rects(&self) -> Vec<(PaneId, NormalizedRect)> {
        let mut rects = Vec::new();
        collect_rects(
            &self.root,
            NormalizedRect {
                x0: 0,
                y0: 0,
                x1: NORMALIZED_EXTENT,
                y1: NORMALIZED_EXTENT,
            },
            &mut rects,
        );
        rects
    }

    /// The pane after or before `from` in traversal order, wrapping. `None`
    /// for an unknown pane or a layout of one pane.
    #[must_use]
    pub fn cycle(&self, from: PaneId, forward: bool) -> Option<PaneId> {
        let panes = self.panes();
        let count = panes.len();
        let index = panes.iter().position(|pane| pane.id == from)?;
        if count < 2 {
            return None;
        }
        let next = if forward {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        Some(panes[next].id)
    }

    /// The pane `direction` moves to from `from`, without wrapping.
    ///
    /// Candidates lie wholly beyond `from`'s edge on that side in
    /// normalized geometry and have nonzero extent. The nearest wins; among
    /// equal distances, a candidate overlapping `from` on the
    /// perpendicular axis by a positive length beats one that only touches
    /// or misses it, then the longer overlap wins, then the pane earlier in
    /// `history` (most recent first), then traversal order.
    #[must_use]
    pub fn neighbor(
        &self,
        from: PaneId,
        direction: SplitDirection,
        history: &[PaneId],
    ) -> Option<PaneId> {
        let rects = self.normalized_rects();
        let source = rects.iter().find(|(id, _)| *id == from)?.1;
        let recency = |id: PaneId| {
            history
                .iter()
                .position(|entry| *entry == id)
                .unwrap_or(usize::MAX)
        };
        rects
            .iter()
            .enumerate()
            .filter(|(_, (id, rect))| *id != from && !rect.empty())
            .filter_map(|(order, (id, rect))| {
                let (distance, overlap) = reach(source, *rect, direction)?;
                Some((
                    distance,
                    overlap == 0,
                    std::cmp::Reverse(overlap),
                    recency(*id),
                    order,
                    *id,
                ))
            })
            .min()
            .map(|candidate| candidate.5)
    }
}

/// The distance from `source`'s edge on `direction`'s side to `candidate`
/// and their overlap on the perpendicular axis, or `None` when `candidate`
/// is not wholly beyond that edge.
fn reach(
    source: NormalizedRect,
    candidate: NormalizedRect,
    direction: SplitDirection,
) -> Option<(u128, u128)> {
    let overlap = |a0: u128, a1: u128, b0: u128, b1: u128| {
        a1.min(b1).saturating_sub(a0.max(b0))
    };
    match direction {
        SplitDirection::Right => (candidate.x0 >= source.x1).then(|| {
            (
                candidate.x0 - source.x1,
                overlap(source.y0, source.y1, candidate.y0, candidate.y1),
            )
        }),
        SplitDirection::Left => (candidate.x1 <= source.x0).then(|| {
            (
                source.x0 - candidate.x1,
                overlap(source.y0, source.y1, candidate.y0, candidate.y1),
            )
        }),
        SplitDirection::Down => (candidate.y0 >= source.y1).then(|| {
            (
                candidate.y0 - source.y1,
                overlap(source.x0, source.x1, candidate.x0, candidate.x1),
            )
        }),
        SplitDirection::Up => (candidate.y1 <= source.y0).then(|| {
            (
                source.y0 - candidate.y1,
                overlap(source.x0, source.x1, candidate.x0, candidate.x1),
            )
        }),
    }
}

#[derive(Default)]
struct Ids {
    panes: HashSet<PaneId>,
    splits: HashSet<SplitId>,
    terminals: HashSet<TerminalId>,
}

fn validate(
    node: &PaneNode,
    parent: Option<(SplitAxis, usize)>,
    ids: &mut Ids,
) -> Result<(), LayoutError> {
    match node {
        PaneNode::Pane(pane) => {
            if !ids.panes.insert(pane.id)
                || !ids.terminals.insert(pane.terminal_id)
            {
                return Err(LayoutError::DuplicateId);
            }
        }
        PaneNode::Split(split) => {
            let count = split.children.len();
            if count < 2 {
                return Err(LayoutError::TooFewChildren);
            }
            if count > MAX_SPLIT_CHILDREN {
                return Err(LayoutError::TooManyChildren);
            }
            if let Some((axis, siblings)) = parent
                && axis == split.axis
                && siblings + count - 1 <= MAX_SPLIT_CHILDREN
            {
                return Err(LayoutError::SameAxisChild);
            }
            if !ids.splits.insert(split.id) {
                return Err(LayoutError::DuplicateId);
            }
            let sum: u64 =
                split.children.iter().map(|c| u64::from(c.weight)).sum();
            if split.children.iter().any(|child| child.weight == 0)
                || sum != u64::from(SPLIT_TOTAL)
            {
                return Err(LayoutError::InvalidWeights);
            }
            for child in &split.children {
                validate(&child.node, Some((split.axis, count)), ids)?;
            }
        }
    }
    Ok(())
}

fn collect_panes<'a>(node: &'a PaneNode, panes: &mut Vec<&'a PaneInfo>) {
    match node {
        PaneNode::Pane(pane) => panes.push(pane),
        PaneNode::Split(split) => {
            for child in &split.children {
                collect_panes(&child.node, panes);
            }
        }
    }
}

fn count_panes(node: &PaneNode) -> usize {
    match node {
        PaneNode::Pane(_) => 1,
        PaneNode::Split(split) => {
            split.children.iter().map(|c| count_panes(&c.node)).sum()
        }
    }
}

fn find_split(node: &PaneNode, id: SplitId) -> Option<&SplitNode> {
    match node {
        PaneNode::Pane(_) => None,
        PaneNode::Split(split) if split.id == id => Some(split),
        PaneNode::Split(split) => split
            .children
            .iter()
            .find_map(|child| find_split(&child.node, id)),
    }
}

fn find_split_mut(node: &mut PaneNode, id: SplitId) -> Option<&mut SplitNode> {
    let PaneNode::Split(split) = node else {
        return None;
    };
    if split.id == id {
        return Some(split);
    }
    split
        .children
        .iter_mut()
        .find_map(|child| find_split_mut(&mut child.node, id))
}

/// A split of `original` and `pane` toward `direction`, each holding half.
fn new_split(
    id: SplitId,
    direction: SplitDirection,
    original: PaneNode,
    pane: PaneNode,
) -> PaneNode {
    let half = SPLIT_TOTAL / 2;
    let (first, second) = if direction.leading() {
        (pane, original)
    } else {
        (original, pane)
    };
    PaneNode::Split(SplitNode {
        id,
        axis: direction.axis(),
        children: vec![
            SplitChild {
                weight: half,
                node: first,
            },
            SplitChild {
                weight: SPLIT_TOTAL - half,
                node: second,
            },
        ],
    })
}

/// Splits `target` somewhere under `node`; returns whether it was found.
fn split_within(
    node: &mut SplitNode,
    target: PaneId,
    direction: SplitDirection,
    pane: PaneNode,
    split: SplitId,
) -> Result<bool, LayoutError> {
    let Some(index) = node.children.iter().position(
        |child| matches!(&child.node, PaneNode::Pane(leaf) if leaf.id == target),
    ) else {
        for child in &mut node.children {
            if let PaneNode::Split(nested) = &mut child.node
                && contains_pane(nested, target)
            {
                return split_within(nested, target, direction, pane, split);
            }
        }
        return Ok(false);
    };
    if node.axis == direction.axis() {
        // `check_split` already refused a full parent and a weight of 1.
        let weight = node.children[index].weight;
        let added = weight / 2;
        node.children[index].weight = weight - added;
        let at = if direction.leading() {
            index
        } else {
            index + 1
        };
        node.children.insert(
            at,
            SplitChild {
                weight: added,
                node: pane,
            },
        );
    } else {
        let child = node.children.remove(index);
        node.children.insert(
            index,
            SplitChild {
                weight: child.weight,
                node: new_split(split, direction, child.node, pane),
            },
        );
    }
    Ok(true)
}

fn check_within(
    node: &SplitNode,
    target: PaneId,
    direction: SplitDirection,
) -> Result<(), LayoutError> {
    for child in &node.children {
        match &child.node {
            PaneNode::Pane(leaf) if leaf.id == target => {
                return if node.axis != direction.axis() {
                    Ok(())
                } else if node.children.len() >= MAX_SPLIT_CHILDREN {
                    Err(LayoutError::TooManyChildren)
                } else if child.weight < 2 {
                    Err(LayoutError::PaneTooSmall)
                } else {
                    Ok(())
                };
            }
            PaneNode::Split(nested) if contains_pane(nested, target) => {
                return check_within(nested, target, direction);
            }
            _ => {}
        }
    }
    Err(LayoutError::UnknownPane)
}

fn collect_splits(node: &PaneNode, ids: &mut Vec<SplitId>) {
    if let PaneNode::Split(split) = node {
        ids.push(split.id);
        for child in &split.children {
            collect_splits(&child.node, ids);
        }
    }
}

fn contains_pane(node: &SplitNode, target: PaneId) -> bool {
    node.children.iter().any(|child| match &child.node {
        PaneNode::Pane(pane) => pane.id == target,
        PaneNode::Split(nested) => contains_pane(nested, target),
    })
}

/// Removes `target` somewhere under `node` and normalizes the split it
/// shrank; returns whether it was found. `node` itself may be left with one
/// child, which its parent replaces, keeping that child's identity.
fn remove_within(node: &mut SplitNode, target: PaneId) -> bool {
    if let Some(index) = node.children.iter().position(
        |child| matches!(&child.node, PaneNode::Pane(leaf) if leaf.id == target),
    ) {
        let removed = node.children.remove(index);
        let heir = index.saturating_sub(1);
        node.children[heir].weight += removed.weight;
        // A same-axis child kept because flattening it would have passed
        // the child limit may fit now that `node` holds one fewer child. A
        // lone survivor is left for the parent to collapse instead, so it
        // keeps its identity.
        let mut index = 0;
        while node.children.len() > 1 && index < node.children.len() {
            let before = node.children.len();
            normalize_child(node, index);
            index += 1 + (node.children.len() - before);
        }
        return true;
    }
    let Some(index) = node.children.iter().position(|child| {
        matches!(&child.node, PaneNode::Split(nested) if contains_pane(nested, target))
    }) else {
        return false;
    };
    if let PaneNode::Split(nested) = &mut node.children[index].node {
        remove_within(nested, target);
    }
    normalize_child(node, index);
    true
}

/// Replaces a child split left with one child by that child, then flattens
/// the result into `node` when it shares `node`'s axis and the merged split
/// stays within the child limit.
fn normalize_child(node: &mut SplitNode, index: usize) {
    if let PaneNode::Split(nested) = &mut node.children[index].node
        && nested.children.len() == 1
    {
        let only = nested.children.remove(0).node;
        node.children[index].node = only;
    }
    let flatten = matches!(
        &node.children[index].node,
        PaneNode::Split(nested)
            if nested.axis == node.axis
                && node.children.len() + nested.children.len() - 1
                    <= MAX_SPLIT_CHILDREN
    );
    if !flatten {
        return;
    }
    let slot = node.children.remove(index);
    let PaneNode::Split(nested) = slot.node else {
        return;
    };
    let scaled = nested.children.into_iter().map(|child| SplitChild {
        weight: scale(child.weight, slot.weight),
        node: child.node,
    });
    node.children.splice(index..index, scaled);
    rebalance(node);
}

/// `weight × slot / SPLIT_TOTAL`, rounded down, and at least 1.
fn scale(weight: u32, slot: u32) -> u32 {
    let scaled = u64::from(weight) * u64::from(slot) / u64::from(SPLIT_TOTAL);
    u32::try_from(scaled).unwrap_or(SPLIT_TOTAL).max(1)
}

/// Adds the difference between `SPLIT_TOTAL` and the children's weights to,
/// or takes it from, the largest child, the first among equals.
fn rebalance(node: &mut SplitNode) {
    let sum: i64 = node.children.iter().map(|c| i64::from(c.weight)).sum();
    let difference = i64::from(SPLIT_TOTAL) - sum;
    let largest = node
        .children
        .iter()
        .enumerate()
        .max_by(|(a_index, a), (b_index, b)| {
            a.weight.cmp(&b.weight).then(b_index.cmp(a_index))
        })
        .map_or(0, |(index, _)| index);
    let child = &mut node.children[largest];
    let adjusted = i64::from(child.weight) + difference;
    child.weight = u32::try_from(adjusted).unwrap_or(1).max(1);
}

fn collect_rects(
    node: &PaneNode,
    rect: NormalizedRect,
    rects: &mut Vec<(PaneId, NormalizedRect)>,
) {
    match node {
        PaneNode::Pane(pane) => rects.push((pane.id, rect)),
        PaneNode::Split(split) => {
            let (start, end) = match split.axis {
                SplitAxis::Columns => (rect.x0, rect.x1),
                SplitAxis::Rows => (rect.y0, rect.y1),
            };
            let extent = end - start;
            let mut prefix = 0_u128;
            let mut edge = start;
            for child in &split.children {
                prefix += u128::from(child.weight);
                let far = start + extent * prefix / u128::from(SPLIT_TOTAL);
                let child_rect = match split.axis {
                    SplitAxis::Columns => NormalizedRect {
                        x0: edge,
                        x1: far,
                        ..rect
                    },
                    SplitAxis::Rows => NormalizedRect {
                        y0: edge,
                        y1: far,
                        ..rect
                    },
                };
                collect_rects(&child.node, child_rect, rects);
                edge = far;
            }
        }
    }
}

#[cfg(test)]
mod tests;
