//! Cell-sized terminal graphics. Coverage follows Ghostty's sprite groups.
//!
//! Adapted from Ghostty, copyright (c) 2024 Mitchell Hashimoto, Ghostty
//! contributors, under MIT. See third-party/terminal-graphics/README.md.

mod block;
mod box_drawing;
mod geometric_shapes;
mod powerline;

use std::sync::Arc;

use gpui::{
    Bounds, Hsla, PathBuilder, Pixels, Point, Size, Window, fill, point, px,
    size,
};

use super::GridMetrics;

/// Only replace standalone supported scalars. Grapheme sequences must retain
/// their font shaping, including combining marks and variation selectors.
pub(super) fn character(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let ch = chars.next()?;
    (chars.next().is_none() && slot(ch).is_some()).then_some(ch)
}

const SLOTS: usize = 186;

/// Dense index of a supported scalar. This is the list of built-in glyphs.
fn slot(ch: char) -> Option<usize> {
    let code = ch as usize;
    Some(match ch {
        '\u{2500}'..='\u{259f}' => code - 0x2500,
        '\u{e0b0}'..='\u{e0bf}' => 160 + code - 0xe0b0,
        '\u{e0d2}' => 176,
        '\u{e0d4}' => 177,
        '\u{25e2}'..='\u{25e5}' => 178 + code - 0x25e2,
        '\u{25f8}'..='\u{25fa}' => 182 + code - 0x25f8,
        '\u{25ff}' => 185,
        _ => return None,
    })
}

/// Shared geometry per glyph and cell span, indexed without hashing because
/// graphics-heavy screens look one up for almost every cell.
pub(super) struct GeometryCache {
    slots: Vec<Option<Arc<Geometry>>>,
}

impl Default for GeometryCache {
    fn default() -> Self {
        Self {
            slots: vec![None; SLOTS * 2],
        }
    }
}

impl GeometryCache {
    fn index(ch: char, columns: u16) -> Option<usize> {
        Some(slot(ch)? * 2 + usize::from(columns > 1))
    }

    /// Returns the geometry and whether it was already cached. `ch` must come
    /// from [`character`]; any other scalar yields empty geometry.
    pub(super) fn get_or_insert(
        &mut self,
        ch: char,
        columns: u16,
        metrics: GridMetrics,
    ) -> (Arc<Geometry>, bool) {
        let Some(entry) = Self::index(ch, columns)
            .and_then(|index| self.slots.get_mut(index))
        else {
            return (Arc::new(Geometry::default()), false);
        };
        if let Some(geometry) = entry {
            return (Arc::clone(geometry), true);
        }
        let geometry = Arc::new(Geometry::new(ch, metrics, columns));
        *entry = Some(Arc::clone(&geometry));
        (geometry, false)
    }

    pub(super) fn get(&self, ch: char, columns: u16) -> Option<&Arc<Geometry>> {
        self.slots.get(Self::index(ch, columns)?)?.as_ref()
    }

    pub(super) fn len(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    pub(super) fn clear(&mut self) {
        self.slots.fill(None);
    }
}

#[derive(Default)]
pub(super) struct Geometry {
    size: Size<Pixels>,
    rectangles: Vec<Rectangle>,
    strokes: Vec<ShapePath>,
    fills: Vec<ShapePath>,
}

struct Rectangle {
    bounds: Bounds<Pixels>,
    opacity: f32,
}

struct ShapePath {
    width: Pixels,
    start: Point<Pixels>,
    segments: Vec<Segment>,
}

impl ShapePath {
    fn build(
        &self,
        origin: Point<Pixels>,
        filled: bool,
    ) -> anyhow::Result<gpui::Path<Pixels>> {
        let mut builder = if filled {
            PathBuilder::fill()
        } else {
            PathBuilder::stroke(self.width)
        };
        builder.move_to(origin + self.start);
        for segment in &self.segments {
            match segment {
                Segment::Line(to) => builder.line_to(origin + *to),
                Segment::Curve(to, a, b) => builder.cubic_bezier_to(
                    origin + *to,
                    origin + *a,
                    origin + *b,
                ),
            }
        }
        if filled {
            builder.close();
        }
        builder.build()
    }
}

enum Segment {
    Line(Point<Pixels>),
    Curve(Point<Pixels>, Point<Pixels>, Point<Pixels>),
}

/// Geometry is calculated in physical pixels, then stored in logical points.
/// Shared integer boundaries prevent seams at fractional cell subdivisions.
struct Canvas {
    width: f32,
    height: f32,
    thickness: f32,
    scale: f32,
    geometry: Geometry,
}

impl Canvas {
    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        self.shaded_rect(x0, y0, x1, y1, 1.0);
    }

    fn shaded_rect(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        opacity: f32,
    ) {
        let x0 = x0.clamp(0.0, self.width);
        let y0 = y0.clamp(0.0, self.height);
        let x1 = x1.clamp(x0, self.width);
        let y1 = y1.clamp(y0, self.height);
        if x1 > x0 && y1 > y0 {
            self.geometry.rectangles.push(Rectangle {
                bounds: Bounds::new(
                    self.point(x0, y0),
                    size(
                        px((x1 - x0) / self.scale),
                        px((y1 - y0) / self.scale),
                    ),
                ),
                opacity,
            });
        }
    }

    fn polygon(&mut self, points: &[(f32, f32)]) {
        self.geometry.fills.push(ShapePath {
            width: px(0.0),
            start: self.point(points[0].0, points[0].1),
            segments: points[1..]
                .iter()
                .map(|&(x, y)| Segment::Line(self.point(x, y)))
                .collect(),
        });
    }

    fn point(&self, x: f32, y: f32) -> Point<Pixels> {
        point(px(x / self.scale), px(y / self.scale))
    }
}

impl Geometry {
    pub(super) fn new(ch: char, metrics: GridMetrics, columns: u16) -> Self {
        let mut canvas = Canvas {
            width: f32::from(metrics.cell_width * metrics.scale_factor).round()
                * f32::from(columns),
            height: f32::from(metrics.cell_height * metrics.scale_factor)
                .round(),
            // A light stroke grows with the font, but remains at least one
            // physical pixel. Bold text does not change Unicode line weights.
            thickness: (f32::from(metrics.font_size) * metrics.scale_factor
                / 12.0)
                .round()
                .max(1.0),
            scale: metrics.scale_factor,
            geometry: Self {
                size: size(
                    metrics.cell_width * f32::from(columns),
                    metrics.cell_height,
                ),
                ..Self::default()
            },
        };
        match ch {
            '\u{2500}'..='\u{257f}' => box_drawing::draw(ch, &mut canvas),
            '\u{2580}'..='\u{259f}' => block::draw(ch, &mut canvas),
            '\u{e0b0}'..='\u{e0bf}' | '\u{e0d2}' | '\u{e0d4}' => {
                powerline::draw(ch, &mut canvas);
            }
            '\u{25e2}'..='\u{25e5}' | '\u{25f8}'..='\u{25fa}' | '\u{25ff}' => {
                geometric_shapes::draw(ch, &mut canvas);
            }
            _ => {}
        }
        canvas.geometry
    }

    /// Rectangle and path primitives emitted by every paint of this glyph.
    pub(super) fn primitive_counts(&self) -> (usize, usize) {
        (self.rectangles.len(), self.strokes.len() + self.fills.len())
    }

    pub(super) fn paint(
        &self,
        origin: Point<Pixels>,
        color: Hsla,
        window: &mut Window,
    ) {
        // `Canvas::shaded_rect` clamps rectangles to the cell.
        for rectangle in &self.rectangles {
            let mut tint = color;
            tint.a *= rectangle.opacity;
            window.paint_quad(fill(
                Bounds::new(
                    origin + rectangle.bounds.origin,
                    rectangle.bounds.size,
                ),
                tint,
            ));
        }
        if self.strokes.is_empty() && self.fills.is_empty() {
            return;
        }
        // Diagonal strokes overshoot the cell, so only paths need the mask.
        window.with_content_mask(
            Some(gpui::ContentMask {
                bounds: Bounds::new(origin, self.size),
            }),
            |window| {
                for (stroke, filled) in self
                    .strokes
                    .iter()
                    .map(|s| (s, false))
                    .chain(self.fills.iter().map(|s| (s, true)))
                {
                    if let Ok(path) = stroke.build(origin, filled) {
                        window.paint_path(path, color);
                    }
                }
            },
        );
    }
}

#[cfg(test)]
mod tests;
