//! How the Nerd Fonts font patcher sizes and aligns each icon in a cell.
//!
//! Adapted from Ghostty's `RenderOptions.Constraint` in src/font/Glyph.zig,
//! copyright (c) 2024 Mitchell Hashimoto, Ghostty contributors, under MIT.
//! See third-party/nerd-font-constraints/README.md. Positions use Ghostty's
//! coordinates: logical pixels, x from the cell's left edge and y up from
//! its bottom edge.

#[expect(
    clippy::unreadable_literal,
    reason = "the generator writes floats as Python prints them"
)]
mod table;

/// Sizing rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Size {
    /// Don't change the glyph's size.
    None,
    /// Scale down to fit within the bounds, preserving aspect ratio.
    Fit,
    /// Scale up or down to exactly match the bounds, preserving aspect ratio.
    Cover,
    /// Scale down to fit, or up to cover at least one cell (font-patcher's
    /// `pa`).
    FitCover1,
    /// Stretch to the bounds in both directions. GPUI scales glyphs only
    /// uniformly, so Huterm sizes these like `Fit` against the grid.
    Stretch,
}

/// Alignment along one axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Align {
    /// Don't move the glyph, beyond keeping it inside the padded bounds.
    None,
    /// Align the leading (left or bottom) edge.
    Start,
    /// Align the trailing (right or top) edge.
    End,
    /// Centre in the first cell even for a multi-cell constraint. The
    /// patcher has no plain centring across every cell.
    Center1,
}

/// The height metric a one-cell constraint scales against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Height {
    /// The primary face's line height.
    Cell,
    /// The icon height, which is shorter than the line for one cell.
    Icon,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Constraint {
    pub(super) size: Size,
    pub(super) align_vertical: Align,
    pub(super) align_horizontal: Align,
    pad_top: f64,
    pad_left: f64,
    pad_right: f64,
    pad_bottom: f64,
    /// The glyph's size and bearings relative to its scale group's box.
    relative_width: f64,
    relative_height: f64,
    relative_x: f64,
    relative_y: f64,
    /// The glyph's width relative to its scale group's, for the scale factor
    /// alone, when the group's glyphs have different advances: font-patcher
    /// scales the whole group by its combined box but aligns each glyph on
    /// its own, while `relative_width` covers both. `None` uses
    /// `relative_width`.
    relative_scale_width: Option<f64>,
    /// The widest aspect ratio (width / height) a stretch may produce.
    pub(super) max_xy_ratio: Option<f64>,
    /// The most cells the glyph may span.
    max_constraint_width: u8,
    pub(super) height: Height,
}

impl Constraint {
    const DEFAULT: Self = Self {
        size: Size::None,
        align_vertical: Align::None,
        align_horizontal: Align::None,
        pad_top: 0.0,
        pad_left: 0.0,
        pad_right: 0.0,
        pad_bottom: 0.0,
        relative_width: 1.0,
        relative_height: 1.0,
        relative_x: 0.0,
        relative_y: 0.0,
        relative_scale_width: None,
        max_xy_ratio: None,
        max_constraint_width: 2,
        height: Height::Cell,
    };

    /// What Ghostty applies to a symbol that has no Nerd Font rule.
    const FIT: Self = Self {
        size: Size::Fit,
        ..Self::DEFAULT
    };

    /// The font patcher's rule for `ch`, or a plain fit for a symbol it
    /// does not list.
    pub(super) fn for_char(ch: char) -> Self {
        let code = u32::from(ch);
        table::RANGES
            .binary_search_by(|&(start, end, _)| {
                if end < code {
                    std::cmp::Ordering::Less
                } else if start > code {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()
            .and_then(|index| table::RANGES.get(index))
            .and_then(|&(_, _, index)| {
                table::CONSTRAINTS.get(usize::from(index))
            })
            .copied()
            .unwrap_or(Self::FIT)
    }

    /// The glyph box after applying this constraint within `columns` cells.
    /// The width and height keep the glyph's aspect ratio.
    pub(super) fn constrain(
        self,
        glyph: GlyphBox,
        metrics: &Metrics,
        columns: u8,
    ) -> GlyphBox {
        if self.size == Size::None
            && self.align_horizontal == Align::None
            && self.align_vertical == Align::None
        {
            return glyph;
        }
        if self.size != Size::Stretch {
            return self.constrain_inner(glyph, metrics, columns);
        }
        // Stretched glyphs align to the grid rather than the face, and need
        // no negative padding against rounding gaps.
        let grid = Metrics {
            face_width: metrics.cell_width,
            face_height: metrics.cell_height,
            face_y: 0.0,
            ..*metrics
        };
        let constraint = Self {
            pad_bottom: self.pad_bottom.max(0.0),
            pad_top: self.pad_top.max(0.0),
            pad_left: self.pad_left.max(0.0),
            pad_right: self.pad_right.max(0.0),
            ..self
        };
        constraint.constrain_inner(glyph, &grid, columns)
    }

    fn constrain_inner(
        self,
        glyph: GlyphBox,
        metrics: &Metrics,
        columns: u8,
    ) -> GlyphBox {
        // Like font-patcher, never stretch across two cells for a very wide
        // face.
        let columns = if self.size == Size::Stretch
            && metrics.face_width > 0.9 * metrics.face_height
        {
            1
        } else {
            self.max_constraint_width.min(columns).max(1)
        };

        // Scaling and alignment apply to the glyph's scale group.
        let mut group = {
            let width = glyph.width / self.relative_width;
            let height = glyph.height / self.relative_height;
            GlyphBox {
                width,
                height,
                x: glyph.x - width * self.relative_x,
                y: glyph.y - height * self.relative_y,
            }
        };

        let factor = self.scale_factor(
            &GlyphBox {
                width: glyph.width
                    / self.relative_scale_width.unwrap_or(self.relative_width),
                ..group
            },
            metrics,
            columns,
        );
        let center_x = group.x + group.width / 2.0;
        let center_y = group.y + group.height / 2.0;
        group.width *= factor;
        group.height *= factor;
        group.x = center_x - group.width / 2.0;
        group.y = center_y - group.height / 2.0;

        group.y = self.aligned_y(&group, metrics);
        group.x = self.aligned_x(&group, metrics, columns);

        GlyphBox {
            width: factor * glyph.width,
            height: factor * glyph.height,
            x: group.x + group.width * self.relative_x,
            y: group.y + group.height * self.relative_y,
        }
    }

    /// The uniform scale for this group. Ghostty scales stretched glyphs and
    /// over-wide aspect ratios per axis; GPUI cannot, so those take the
    /// smaller factor and keep the glyph's shape.
    fn scale_factor(
        self,
        group: &GlyphBox,
        metrics: &Metrics,
        columns: u8,
    ) -> f64 {
        if self.size == Size::None {
            return 1.0;
        }
        let multi_cell = columns > 1;
        let target_width = (f64::from(columns)
            - (self.pad_left + self.pad_right))
            * metrics.face_width;
        let target_height = (1.0 - (self.pad_bottom + self.pad_top))
            * match self.height {
                Height::Cell => metrics.face_height,
                Height::Icon if multi_cell => metrics.icon_height,
                Height::Icon => metrics.icon_height_single,
            };
        let width_factor = target_width / group.width;
        let height_factor = target_height / group.height;

        let mut factor = match self.size {
            Size::None => 1.0,
            Size::Fit | Size::Stretch => {
                1.0_f64.min(width_factor).min(height_factor)
            }
            Size::Cover => width_factor.min(height_factor),
            Size::FitCover1 => {
                let factor = width_factor.min(height_factor);
                // Upscale a multi-cell icon only as far as one cell would,
                // so an icon keeps its size when a space follows it.
                if multi_cell && factor > 1.0 {
                    self.scale_factor(group, metrics, 1).max(1.0)
                } else {
                    factor
                }
            }
        };
        if let Some(ratio) = self.max_xy_ratio
            && group.width * factor > group.height * factor * ratio
        {
            factor = factor.min(group.height * factor * ratio / group.width);
        }
        factor
    }

    fn aligned_y(self, group: &GlyphBox, metrics: &Metrics) -> f64 {
        if self.size == Size::None && self.align_vertical == Align::None {
            return group.y;
        }
        // face_y offsets the face within the pixel-rounded cell, whose
        // baseline sits on a pixel boundary rather than centred.
        let start = metrics.face_y + self.pad_bottom * metrics.face_height;
        let end = metrics.face_y
            + (metrics.face_height
                - group.height
                - self.pad_top * metrics.face_height);
        let center = f64::midpoint(start, end);
        match self.align_vertical {
            Align::None if end < start => center,
            Align::None => group.y.min(end).max(start),
            Align::Start => start,
            Align::End => end,
            Align::Center1 => center,
        }
    }

    fn aligned_x(
        self,
        group: &GlyphBox,
        metrics: &Metrics,
        columns: u8,
    ) -> f64 {
        if self.size == Size::None && self.align_horizontal == Align::None {
            return group.x;
        }
        let span =
            metrics.face_width + f64::from(columns - 1) * metrics.cell_width;
        let start = self.pad_left * metrics.face_width;
        let end = span - group.width - self.pad_right * metrics.face_width;
        match self.align_horizontal {
            Align::None => group.x.min(end).max(start),
            Align::Start => start,
            Align::End => end.max(start),
            // The first cell's centre, which left-aligns a glyph wider than
            // one cell.
            Align::Center1 => {
                let end1 = metrics.face_width
                    - group.width
                    - self.pad_right * metrics.face_width;
                start.max(f64::midpoint(start, end1))
            }
        }
    }
}

/// A glyph's ink box in logical pixels, y up from the cell's bottom edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::renderer) struct GlyphBox {
    pub(in crate::renderer) width: f64,
    pub(in crate::renderer) height: f64,
    pub(in crate::renderer) x: f64,
    pub(in crate::renderer) y: f64,
}

/// The grid measurements constraints use, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::renderer) struct Metrics {
    /// The rounded cell.
    pub(super) cell_width: f64,
    pub(super) cell_height: f64,
    /// The primary face's advance and line height before rounding.
    pub(super) face_width: f64,
    pub(super) face_height: f64,
    /// The face's bottom edge above the cell's bottom edge.
    pub(super) face_y: f64,
    /// Icon heights for multi-cell and one-cell constraints. Like
    /// font-patcher, a one-cell icon uses (2 * cap height + line height) / 3.
    pub(super) icon_height: f64,
    pub(super) icon_height_single: f64,
}

#[cfg(test)]
mod tests;
