//! Cell-filling corner triangles, adapted from Ghostty's `geometric_shapes.zig`.

use super::Canvas;

pub(super) fn draw(ch: char, canvas: &mut Canvas) {
    let (right, bottom, outline) = match ch {
        '◢' => (true, true, false),
        '◣' => (false, true, false),
        '◤' => (false, false, false),
        '◥' => (true, false, false),
        '◸' => (false, false, true),
        '◹' => (true, false, true),
        '◺' => (false, true, true),
        '◿' => (true, true, true),
        _ => return,
    };
    triangle(canvas, right, bottom, outline);
}

pub(super) fn triangle(
    canvas: &mut Canvas,
    right: bool,
    bottom: bool,
    outline: bool,
) {
    let (w, h) = (canvas.width, canvas.height);
    let transform = |(x, y): (f32, f32)| {
        (
            if right { w - x } else { x },
            if bottom { h - y } else { y },
        )
    };
    let mut points = vec![(0.0, 0.0), (w, 0.0), (0.0, h)];
    if outline {
        // Offset all three edges inward by the same perpendicular distance.
        let t = canvas.thickness;
        let diagonal = w.hypot(h);
        let inner_x = w - t * (w + diagonal) / h;
        let inner_y = h - t * (h + diagonal) / w;
        if inner_x > t && inner_y > t {
            // Opposite winding leaves the inset triangle transparent. The
            // doubled bridge joins the contours without a visible edge.
            points.extend([
                (0.0, 0.0),
                (t, t),
                (t, inner_y),
                (inner_x, t),
                (t, t),
                (0.0, 0.0),
            ]);
        }
    }
    canvas.polygon(&points.into_iter().map(transform).collect::<Vec<_>>());
}
