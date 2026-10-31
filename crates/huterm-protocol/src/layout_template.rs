//! Layout templates: a small language that describes how to arrange a
//! tab's existing panes for any pane count, the six built-in layouts
//! written in it, and the rules that step a tab through a cycle of named
//! layouts.
//!
//! ```text
//! template := split
//! split    := axis "(" child ("," child)* ")"
//!           | "grid" "(" "*" ")"
//! axis     := "columns" | "rows"
//! child    := node [share]
//! node     := split | "pane" | "*"
//! share    := integer "%"                        1 to 99
//! ```
//!
//! `pane` takes one pane and `*` a share of the panes left once every
//! `pane` slot is counted. Expansion hands panes to slots in the
//! template's own traversal order, so an arrangement keeps the panes in
//! the order they started in. Expansion is pure: it names panes, not
//! pixels, and every client gets the same weights.

use std::fmt;
use std::sync::LazyLock;

use crate::pane_layout::apportion;
use crate::{
    Arrangement, LayoutError, MAX_SPLIT_CHILDREN, PaneId, PaneLayout, SplitAxis,
};

/// The most bytes a template holds.
pub const MAX_TEMPLATE_LEN: usize = 1024;

/// The most levels a template's splits nest, the root counting as one.
pub const MAX_TEMPLATE_DEPTH: usize = 16;

/// The most characters a layout name holds.
pub const MAX_LAYOUT_NAME_LEN: usize = 32;

/// A parsed layout template.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutTemplate {
    source: String,
    root: Node,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Node {
    Pane,
    Star,
    Split {
        axis: SplitAxis,
        children: Vec<Child>,
    },
    Grid,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Child {
    node: Node,
    /// The child's explicit share of its split, in percent.
    share: Option<u32>,
}

/// Why a template failed to parse, and where.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TemplateError {
    /// Byte offset into the template text where the rule broke.
    pub offset: usize,
    /// The rule broken.
    pub kind: TemplateErrorKind,
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}", self.kind, self.offset)
    }
}

impl std::error::Error for TemplateError {}

/// The rule a template broke.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemplateErrorKind {
    /// The text is longer than [`MAX_TEMPLATE_LEN`] bytes.
    TooLong,
    /// Splits nest deeper than [`MAX_TEMPLATE_DEPTH`] levels.
    TooDeep,
    /// The root is not a split or a grid.
    ExpectedSplit,
    /// A child is not a split, a grid, `pane`, or `*`.
    ExpectedNode,
    /// A split keyword is not followed by `(`.
    ExpectedOpen,
    /// A grid holds something other than `*`.
    ExpectedStar,
    /// A grid's `*` is not followed by `)`.
    ExpectedClose,
    /// A child is not followed by `,` or `)`.
    ExpectedSeparator,
    /// A share's number is not followed by `%`.
    ExpectedPercent,
    /// Text follows the root split.
    ExpectedEnd,
    /// A share is outside 1 to 99.
    ShareOutOfRange,
    /// Explicit shares total 100 or more beside a child without one.
    SharesTooLarge,
    /// Every child has a share, and the shares do not total 100.
    SharesNotTotal,
    /// A split has one child, and it is not `*`.
    TooFewChildren,
}

impl fmt::Display for TemplateErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong => {
                write!(f, "a template holds at most {MAX_TEMPLATE_LEN} bytes")
            }
            Self::TooDeep => write!(
                f,
                "splits nest at most {MAX_TEMPLATE_DEPTH} levels deep"
            ),
            Self::ExpectedSplit => {
                f.write_str("expected `columns`, `rows`, or `grid`")
            }
            Self::ExpectedNode => f.write_str(
                "expected `columns`, `rows`, `grid`, `pane`, or `*`",
            ),
            Self::ExpectedOpen => f.write_str("expected `(`"),
            Self::ExpectedStar => f.write_str("expected `*`"),
            Self::ExpectedClose => f.write_str("expected `)`"),
            Self::ExpectedSeparator => f.write_str("expected `,` or `)`"),
            Self::ExpectedPercent => f.write_str("expected `%`"),
            Self::ExpectedEnd => {
                f.write_str("expected the end of the template")
            }
            Self::ShareOutOfRange => f.write_str("a share must be 1% to 99%"),
            Self::SharesTooLarge => f.write_str(
                "shares must total less than 100% when a child has none",
            ),
            Self::SharesNotTotal => {
                f.write_str("shares must total 100% when every child has one")
            }
            Self::TooFewChildren => {
                f.write_str("a split needs two children unless one is `*`")
            }
        }
    }
}

impl LayoutTemplate {
    /// Parses a template.
    ///
    /// # Errors
    /// Returns the byte offset and the rule broken: bad syntax, a share
    /// outside 1 to 99, shares that leave no room or do not total 100, a
    /// split of one child that is not `*`, and the length and depth limits.
    pub fn parse(source: &str) -> Result<Self, TemplateError> {
        if source.len() > MAX_TEMPLATE_LEN {
            // The first character that does not fit, so the offset can
            // slice the text.
            let mut offset = MAX_TEMPLATE_LEN;
            while !source.is_char_boundary(offset) {
                offset -= 1;
            }
            return Err(TemplateError {
                offset,
                kind: TemplateErrorKind::TooLong,
            });
        }
        let mut parser = Parser {
            text: source.as_bytes(),
            at: 0,
        };
        parser.skip_space();
        let start = parser.at;
        let root = match parser.keyword() {
            Some(Keyword::Axis(axis)) => parser.split(axis, start, 1)?,
            Some(Keyword::Grid) => parser.grid()?,
            _ => return Err(error(start, TemplateErrorKind::ExpectedSplit)),
        };
        parser.skip_space();
        if parser.at < parser.text.len() {
            return Err(parser.error(TemplateErrorKind::ExpectedEnd));
        }
        Ok(Self {
            source: source.to_owned(),
            root,
        })
    }

    /// The template's text as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Arranges `panes`, given in traversal order, as the template
    /// describes. The result keeps their order.
    ///
    /// # Errors
    /// The template is unavailable for this many panes when it has no `*`
    /// and fewer pane slots than panes ([`LayoutError::TooManyPanes`]), or
    /// when the expansion leaves a split with more than
    /// [`MAX_SPLIT_CHILDREN`] children ([`LayoutError::TooManyChildren`]).
    /// No layout holds zero panes, so an empty list is
    /// [`LayoutError::MissingPane`].
    pub fn expand(&self, panes: &[PaneId]) -> Result<Arrangement, LayoutError> {
        if panes.is_empty() {
            return Err(LayoutError::MissingPane);
        }
        let (slots, stars) = self.root.slots();
        let remaining = panes.len().saturating_sub(slots);
        if stars == 0 && remaining > 0 {
            return Err(LayoutError::TooManyPanes);
        }
        let mut fill = Fill {
            panes,
            next: 0,
            star: 0,
            each: remaining.checked_div(stars).unwrap_or(0),
            extra: remaining.checked_rem(stars).unwrap_or(0),
        };
        let shaped = match &self.root {
            Node::Split { axis, children } => fill.split(*axis, children)?,
            Node::Grid => {
                let count = fill.star_count();
                fill.grid(count)?
            }
            Node::Pane | Node::Star => None,
        };
        // Every template holds a slot, and the first takes a pane.
        Ok(shaped.map_or(Arrangement::Pane(panes[0]), Shaped::arrangement))
    }
}

impl fmt::Display for LayoutTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

/// A layout a user or a command names.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NamedLayout {
    /// The name bindings and the config use.
    pub name: String,
    /// The name menus and the palette show.
    pub title: String,
    /// What the layout arranges.
    pub template: LayoutTemplate,
}

impl NamedLayout {
    /// Whether `name` can name a layout: 1 to [`MAX_LAYOUT_NAME_LEN`]
    /// lowercase ASCII letters, digits, and underscores, starting with a
    /// letter. Built-in names are valid, and reserved; see
    /// [`Self::built_in`].
    #[must_use]
    pub fn valid_name(name: &str) -> bool {
        let bytes = name.as_bytes();
        (1..=MAX_LAYOUT_NAME_LEN).contains(&bytes.len())
            && bytes[0].is_ascii_lowercase()
            && bytes.iter().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || *byte == b'_'
            })
    }

    /// The built-in layout called `name`.
    #[must_use]
    pub fn built_in(name: &str) -> Option<&'static Self> {
        BUILT_IN_LAYOUTS.iter().find(|layout| layout.name == name)
    }
}

/// The built-in layouts, in the default cycle's order: equal columns,
/// equal rows, main left, main top, main centered, and tiled.
pub static BUILT_IN_LAYOUTS: LazyLock<[NamedLayout; 6]> = LazyLock::new(|| {
    [
        ("columns", "Equal Columns", "columns(*)"),
        ("rows", "Equal Rows", "rows(*)"),
        ("main_left", "Main Left", "columns(pane 60%, rows(*))"),
        ("main_top", "Main Top", "rows(pane 60%, columns(*))"),
        (
            "main_centered",
            "Main Centered",
            "columns(rows(*), pane 50%, rows(*))",
        ),
        ("tiled", "Tiled", "grid(*)"),
    ]
    .map(|(name, title, template)| NamedLayout {
        name: name.to_owned(),
        title: title.to_owned(),
        template: LayoutTemplate::parse(template)
            .expect("built-in templates parse"),
    })
});

impl PaneLayout {
    /// The layout of `cycle` this layout matches: among the layouts
    /// available for this many panes whose ideal expansion has the same
    /// shape, ignoring split identities and weights, the one whose weights
    /// are nearest, by the sum of absolute weight differences over every
    /// split. Ties go to the first in the cycle. `None` when no layout has
    /// the same shape.
    #[must_use]
    pub fn matching_layout(&self, cycle: &[NamedLayout]) -> Option<usize> {
        let current = self.arrangement();
        let panes = current.panes();
        cycle
            .iter()
            .enumerate()
            .filter_map(|(index, layout)| {
                let ideal = layout.template.expand(&panes).ok()?;
                ideal
                    .same_shape(&current)
                    .then(|| (ideal.weight_distance(&current), index))
            })
            .min()
            .map(|(_, index)| index)
    }

    /// The next layout of `cycle` after the matching one, or before it
    /// when `forward` is false, that would change this layout, with its
    /// fitted arrangement.
    ///
    /// Each layout is tried at most once, in cycle order, wrapping, so the
    /// matched layout comes last. With no match, a forward step starts at
    /// the first layout and a backward step at the last. A layout is
    /// skipped when it is unavailable for this many panes, when `fit`
    /// returns `None` because it does not fit the caller's view, or when
    /// its fitted arrangement equals this layout, weights included. `None`
    /// when every layout is skipped; the caller's `fit` knows whether any
    /// was skipped for not fitting.
    pub fn step_layout(
        &self,
        cycle: &[NamedLayout],
        forward: bool,
        mut fit: impl FnMut(&NamedLayout, Arrangement) -> Option<Arrangement>,
    ) -> Option<(usize, Arrangement)> {
        let count = cycle.len();
        let last = count.checked_sub(1)?;
        let current = self.arrangement();
        let panes = current.panes();
        // With no match, a forward step starts after the last layout and a
        // backward step before the first.
        let matched = self.matching_layout(cycle).unwrap_or(if forward {
            last
        } else {
            0
        });
        (1..=count)
            .map(|step| {
                if forward {
                    (matched + step) % count
                } else {
                    (matched + count - step) % count
                }
            })
            .find_map(|index| {
                let layout = &cycle[index];
                let ideal = layout.template.expand(&panes).ok()?;
                let fitted = fit(layout, ideal)?;
                (fitted != current).then_some((index, fitted))
            })
    }
}

enum Keyword {
    Axis(SplitAxis),
    Grid,
    Pane,
}

struct Parser<'a> {
    text: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    /// An error at the cursor.
    fn error(&self, kind: TemplateErrorKind) -> TemplateError {
        error(self.at, kind)
    }

    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    /// Consumes `byte` after any whitespace; false when it is not next.
    fn eat(&mut self, byte: u8) -> bool {
        self.skip_space();
        if self.peek() == Some(byte) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    /// Consumes a keyword at the cursor, leaving the cursor in place when
    /// none is there.
    fn keyword(&mut self) -> Option<Keyword> {
        let start = self.at;
        let mut end = start;
        while self.text.get(end).is_some_and(u8::is_ascii_lowercase) {
            end += 1;
        }
        let keyword = match &self.text[start..end] {
            b"columns" => Keyword::Axis(SplitAxis::Columns),
            b"rows" => Keyword::Axis(SplitAxis::Rows),
            b"grid" => Keyword::Grid,
            b"pane" => Keyword::Pane,
            _ => return None,
        };
        self.at = end;
        Some(keyword)
    }

    /// The children of a split whose keyword began at `start`, through its
    /// closing `)`, at `depth` levels deep.
    fn split(
        &mut self,
        axis: SplitAxis,
        start: usize,
        depth: usize,
    ) -> Result<Node, TemplateError> {
        if !self.eat(b'(') {
            return Err(self.error(TemplateErrorKind::ExpectedOpen));
        }
        let mut children = Vec::new();
        loop {
            self.skip_space();
            let child_start = self.at;
            let node = if self.eat(b'*') {
                Node::Star
            } else {
                match self.keyword() {
                    Some(Keyword::Pane) => Node::Pane,
                    Some(Keyword::Grid) => {
                        nested(child_start, depth)?;
                        self.grid()?
                    }
                    Some(Keyword::Axis(axis)) => {
                        nested(child_start, depth)?;
                        self.split(axis, child_start, depth + 1)?
                    }
                    None => {
                        return Err(self.error(TemplateErrorKind::ExpectedNode));
                    }
                }
            };
            let share = self.share()?;
            children.push(Child { node, share });
            if self.eat(b')') {
                break;
            }
            if !self.eat(b',') {
                return Err(self.error(TemplateErrorKind::ExpectedSeparator));
            }
        }
        check_shares(&children).map_err(|kind| error(start, kind))?;
        Ok(Node::Split { axis, children })
    }

    /// A grid's `(*)`, after its keyword.
    fn grid(&mut self) -> Result<Node, TemplateError> {
        if !self.eat(b'(') {
            return Err(self.error(TemplateErrorKind::ExpectedOpen));
        }
        if !self.eat(b'*') {
            return Err(self.error(TemplateErrorKind::ExpectedStar));
        }
        if !self.eat(b')') {
            return Err(self.error(TemplateErrorKind::ExpectedClose));
        }
        Ok(Node::Grid)
    }

    /// An optional share after a child: an integer and `%`.
    fn share(&mut self) -> Result<Option<u32>, TemplateError> {
        self.skip_space();
        let start = self.at;
        let mut value = Some(0_u32);
        while let Some(byte) = self.peek().filter(u8::is_ascii_digit) {
            value = value
                .and_then(|value| value.checked_mul(10))
                .and_then(|value| value.checked_add(u32::from(byte - b'0')));
            self.at += 1;
        }
        if self.at == start {
            return Ok(None);
        }
        if !self.eat(b'%') {
            return Err(self.error(TemplateErrorKind::ExpectedPercent));
        }
        match value {
            Some(share @ 1..=99) => Ok(Some(share)),
            _ => Err(error(start, TemplateErrorKind::ShareOutOfRange)),
        }
    }
}

/// Refuses a split that begins at `start` inside one `depth` levels deep
/// when it would pass the depth limit.
fn nested(start: usize, depth: usize) -> Result<(), TemplateError> {
    if depth >= MAX_TEMPLATE_DEPTH {
        return Err(error(start, TemplateErrorKind::TooDeep));
    }
    Ok(())
}

fn error(offset: usize, kind: TemplateErrorKind) -> TemplateError {
    TemplateError { offset, kind }
}

/// Checks a split's children: at least two unless one is `*`, explicit
/// shares below 100 beside a child without one, and exactly 100 when every
/// child has one.
fn check_shares(children: &[Child]) -> Result<(), TemplateErrorKind> {
    if children.len() < 2 && !children.iter().any(|c| c.node == Node::Star) {
        return Err(TemplateErrorKind::TooFewChildren);
    }
    let total: u32 = children.iter().filter_map(|child| child.share).sum();
    let all = children.iter().all(|child| child.share.is_some());
    if all && total != 100 {
        return Err(TemplateErrorKind::SharesNotTotal);
    }
    if !all && total >= 100 {
        return Err(TemplateErrorKind::SharesTooLarge);
    }
    Ok(())
}

impl Node {
    /// The pane slots and stars under this node.
    fn slots(&self) -> (usize, usize) {
        match self {
            Self::Pane => (1, 0),
            Self::Star | Self::Grid => (0, 1),
            Self::Split { children, .. } => {
                children.iter().fold((0, 0), |(panes, stars), child| {
                    let (more_panes, more_stars) = child.node.slots();
                    (panes + more_panes, stars + more_stars)
                })
            }
        }
    }
}

/// A template's tree during expansion: fractions stay unrounded until the
/// final tree converts them to weights.
enum Shaped {
    Pane(PaneId),
    Split {
        axis: SplitAxis,
        children: Vec<(f64, Shaped)>,
    },
}

impl Shaped {
    fn arrangement(self) -> Arrangement {
        match self {
            Self::Pane(pane) => Arrangement::Pane(pane),
            Self::Split { axis, children } => {
                let fractions: Vec<f64> =
                    children.iter().map(|(fraction, _)| *fraction).collect();
                Arrangement::Split {
                    axis,
                    children: apportion(&fractions)
                        .into_iter()
                        .zip(children)
                        .map(|(weight, (_, child))| {
                            (weight, child.arrangement())
                        })
                        .collect(),
                }
            }
        }
    }
}

/// Hands panes to a template's slots in template order.
struct Fill<'a> {
    panes: &'a [PaneId],
    next: usize,
    /// Stars filled so far.
    star: usize,
    /// Panes every star takes.
    each: usize,
    /// How many of the first stars take one more.
    extra: usize,
}

impl Fill<'_> {
    /// The next pane for a `pane` slot, or `None` once every pane is
    /// placed.
    fn pane(&mut self) -> Option<PaneId> {
        let pane = self.panes.get(self.next).copied()?;
        self.next += 1;
        Some(pane)
    }

    /// The number of panes the next star takes.
    fn star_count(&mut self) -> usize {
        let count = self.each + usize::from(self.star < self.extra);
        self.star += 1;
        count
    }

    /// The next `count` panes, for a star.
    fn star_panes(&mut self, count: usize) -> Vec<PaneId> {
        let panes = self.panes[self.next..self.next + count].to_vec();
        self.next += count;
        panes
    }

    /// A split's expansion: `None` when no child holds a pane, the only
    /// child when one does, and otherwise a split whose same-axis child
    /// splits are flattened into it while the child limit allows.
    fn split(
        &mut self,
        axis: SplitAxis,
        children: &[Child],
    ) -> Result<Option<Shaped>, LayoutError> {
        let explicit: u32 = children.iter().filter_map(|c| c.share).sum();
        let implicit = children.iter().filter(|c| c.share.is_none()).count();
        let open = f64::from(100 - explicit.min(100))
            / (100.0 * count_f64(implicit.max(1)));
        let mut shaped = Vec::new();
        for child in children {
            let share =
                child.share.map_or(open, |share| f64::from(share) / 100.0);
            match &child.node {
                Node::Pane => {
                    if let Some(pane) = self.pane() {
                        shaped.push((share, Shaped::Pane(pane)));
                    }
                }
                Node::Star => {
                    let count = self.star_count();
                    for pane in self.star_panes(count) {
                        shaped.push((
                            share / count_f64(count),
                            Shaped::Pane(pane),
                        ));
                    }
                }
                Node::Grid => {
                    let count = self.star_count();
                    if let Some(grid) = self.grid(count)? {
                        shaped.push((share, grid));
                    }
                }
                Node::Split { axis, children } => {
                    if let Some(split) = self.split(*axis, children)? {
                        shaped.push((share, split));
                    }
                }
            }
        }
        joined(axis, shaped)
    }

    /// A grid of the next `count` panes: rows of `ceil(sqrt(count))`
    /// panes, the last row holding the rest, every row and every pane in
    /// a row at equal shares.
    fn grid(&mut self, count: usize) -> Result<Option<Shaped>, LayoutError> {
        let panes = self.star_panes(count);
        let columns = (1..=count).find(|c| c * c >= count).unwrap_or(1);
        let mut rows = Vec::new();
        for row in panes.chunks(columns) {
            let cells =
                row.iter().map(|pane| (1.0, Shaped::Pane(*pane))).collect();
            if let Some(row) = joined(SplitAxis::Columns, cells)? {
                rows.push((1.0, row));
            }
        }
        joined(SplitAxis::Rows, rows)
    }
}

/// Joins the children that hold panes into a split along `axis`: their
/// shares scale up to fill it, a lone child stands for the split, and a
/// child split on the same axis flattens into it, its fractions multiplied
/// by its own, unless that passes the child limit.
fn joined(
    axis: SplitAxis,
    mut children: Vec<(f64, Shaped)>,
) -> Result<Option<Shaped>, LayoutError> {
    if children.len() < 2 {
        return Ok(children.pop().map(|(_, child)| child));
    }
    let sum: f64 = children.iter().map(|(fraction, _)| fraction).sum();
    for (fraction, _) in &mut children {
        *fraction /= sum;
    }
    let mut index = 0;
    while index < children.len() {
        let flatten = matches!(
            &children[index].1,
            Shaped::Split { axis: nested, children: inner }
                if *nested == axis
                    && children.len() + inner.len() - 1 <= MAX_SPLIT_CHILDREN
        );
        if !flatten {
            index += 1;
            continue;
        }
        let (
            share,
            Shaped::Split {
                children: inner, ..
            },
        ) = children.remove(index)
        else {
            unreachable!("only a split flattens");
        };
        let added = inner.len();
        children.splice(
            index..index,
            inner
                .into_iter()
                .map(|(fraction, child)| (fraction * share, child)),
        );
        index += added;
    }
    if children.len() > MAX_SPLIT_CHILDREN {
        return Err(LayoutError::TooManyChildren);
    }
    Ok(Some(Shaped::Split { axis, children }))
}

#[expect(
    clippy::cast_precision_loss,
    reason = "a star's pane count is far below 2^52"
)]
fn count_f64(count: usize) -> f64 {
    count as f64
}

#[cfg(test)]
mod tests;
