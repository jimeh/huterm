//! Cell-sized terminal graphics. Coverage follows Ghostty's sprite groups.
//!
//! Adapted from Ghostty, copyright (c) 2024 Mitchell Hashimoto, Ghostty
//! contributors, under MIT. See third-party/terminal-graphics/README.md.

mod block;
mod box_drawing;

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
    (chars.next().is_none() && matches!(ch, '\u{2500}'..='\u{259f}'))
        .then_some(ch)
}

#[derive(Default)]
pub(super) struct Geometry {
    size: Size<Pixels>,
    rectangles: Vec<Rectangle>,
    strokes: Vec<Stroke>,
}

struct Rectangle {
    bounds: Bounds<Pixels>,
    opacity: f32,
}

struct Stroke {
    width: Pixels,
    start: Point<Pixels>,
    segments: Vec<Segment>,
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
            _ => {}
        }
        canvas.geometry
    }

    pub(super) fn paint(
        &self,
        origin: Point<Pixels>,
        color: Hsla,
        window: &mut Window,
    ) {
        window.with_content_mask(
            Some(gpui::ContentMask {
                bounds: Bounds::new(origin, self.size),
            }),
            |window| {
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
                for stroke in &self.strokes {
                    let mut builder = PathBuilder::stroke(stroke.width);
                    builder.move_to(origin + stroke.start);
                    for segment in &stroke.segments {
                        match segment {
                            Segment::Line(to) => builder.line_to(origin + *to),
                            Segment::Curve(to, a, b) => builder
                                .cubic_bezier_to(
                                    origin + *to,
                                    origin + *a,
                                    origin + *b,
                                ),
                        }
                    }
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, color);
                    }
                }
            },
        );
    }
}

#[cfg(test)]
mod tests;
