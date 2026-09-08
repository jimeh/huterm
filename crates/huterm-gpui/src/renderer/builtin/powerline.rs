//! Geometric Powerline separators, adapted from Ghostty's `powerline.zig`.

use super::{Canvas, Segment, ShapePath, box_drawing, geometric_shapes};
use gpui::px;

pub(super) fn draw(ch: char, canvas: &mut Canvas) {
    let (w, h) = (canvas.width, canvas.height);
    match ch {
        '\u{e0b0}' | '\u{e0b1}' | '\u{e0b2}' | '\u{e0b3}' => {
            let left = matches!(ch, '\u{e0b2}' | '\u{e0b3}');
            let (base, tip) = if left { (w, 0.0) } else { (0.0, w) };
            if matches!(ch, '\u{e0b0}' | '\u{e0b2}') {
                canvas.polygon(&[(base, 0.0), (tip, h / 2.0), (base, h)]);
            } else {
                canvas.geometry.strokes.push(ShapePath {
                    width: px(canvas.thickness / canvas.scale),
                    start: canvas.point(base, 0.0),
                    segments: vec![
                        Segment::Line(canvas.point(tip, h / 2.0)),
                        Segment::Line(canvas.point(base, h)),
                    ],
                });
            }
        }
        '\u{e0b4}'..='\u{e0b7}' => rounded(
            canvas,
            matches!(ch, '\u{e0b6}' | '\u{e0b7}'),
            matches!(ch, '\u{e0b5}' | '\u{e0b7}'),
        ),
        '\u{e0b8}' => geometric_shapes::triangle(canvas, false, true, false),
        '\u{e0ba}' => geometric_shapes::triangle(canvas, true, true, false),
        '\u{e0bc}' => geometric_shapes::triangle(canvas, false, false, false),
        '\u{e0be}' => geometric_shapes::triangle(canvas, true, false, false),
        '\u{e0b9}' | '\u{e0bf}' => box_drawing::draw('╲', canvas),
        '\u{e0bb}' | '\u{e0bd}' => box_drawing::draw('╱', canvas),
        '\u{e0d2}' | '\u{e0d4}' => {
            let gap = canvas.thickness.min(h) / 2.0;
            for y in [0.0, h] {
                let middle = if y < h / 2.0 {
                    h / 2.0 - gap
                } else {
                    h / 2.0 + gap
                };
                let points =
                    [(0.0, y), (w, y), (w / 2.0, middle), (0.0, middle)];
                canvas.polygon(&points.map(|(x, y)| {
                    (if ch == '\u{e0d4}' { w - x } else { x }, y)
                }));
            }
        }
        _ => {}
    }
}

#[expect(
    clippy::many_single_char_names,
    reason = "standard circular arc coordinates"
)]
fn rounded(canvas: &mut Canvas, left: bool, outline: bool) {
    let (w, h) = (canvas.width, canvas.height);
    let r = w.min(h / 2.0);
    let c = (std::f32::consts::SQRT_2 - 1.0) * 4.0 / 3.0;
    let p = |x, y| canvas.point(if left { w - x } else { x }, y);
    let mut segments = vec![
        Segment::Curve(p(r, r), p(r * c, 0.0), p(r, r * (1.0 - c))),
        Segment::Line(p(r, h - r)),
        Segment::Curve(p(0.0, h), p(r, h - r * (1.0 - c)), p(r * c, h)),
    ];
    if outline {
        // Trace the inner arc backwards to keep the full stroke inside the
        // cell, including its horizontal butt caps at the joining edge.
        let t = canvas.thickness.min(r);
        let inner = r - t;
        segments.extend([
            Segment::Line(p(0.0, h - t)),
            Segment::Curve(
                p(inner, h - r),
                p(inner * c, h - t),
                p(inner, h - r + inner * c),
            ),
            Segment::Line(p(inner, r)),
            Segment::Curve(p(0.0, t), p(inner, r - inner * c), p(inner * c, t)),
        ]);
    }
    canvas.geometry.fills.push(ShapePath {
        width: px(0.0),
        start: p(0.0, 0.0),
        segments,
    });
}
