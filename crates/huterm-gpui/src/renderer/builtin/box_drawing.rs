//! Box Drawing, U+2500–U+257F.
// Codepoint mapping, intersection extents, dashes and curves adapted from
// Ghostty's sprite/draw/box.zig; MIT, see the parent module.

use super::{Canvas, Segment, Stroke};
use gpui::px;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Weight {
    None,
    Light,
    Heavy,
    Double,
}
use Weight::{Double as D, Heavy as H, Light as L, None as N};

#[expect(
    clippy::too_many_lines,
    reason = "keep the complete Unicode mapping together for upstream comparison"
)]
pub(super) fn draw(ch: char, canvas: &mut Canvas) {
    let cp = u32::from(ch);
    // Arms are ordered up, right, down, left.
    let lines = match cp {
        0x2500 => [N, L, N, L], // ─
        0x2501 => [N, H, N, H], // ━
        0x2502 => [L, N, L, N], // │
        0x2503 => [H, N, H, N], // ┃
        0x250c => [N, L, L, N], // ┌
        0x250d => [N, H, L, N], // ┍
        0x250e => [N, L, H, N], // ┎
        0x250f => [N, H, H, N], // ┏
        0x2510 => [N, N, L, L], // ┐
        0x2511 => [N, N, L, H], // ┑
        0x2512 => [N, N, H, L], // ┒
        0x2513 => [N, N, H, H], // ┓
        0x2514 => [L, L, N, N], // └
        0x2515 => [L, H, N, N], // ┕
        0x2516 => [H, L, N, N], // ┖
        0x2517 => [H, H, N, N], // ┗
        0x2518 => [L, N, N, L], // ┘
        0x2519 => [L, N, N, H], // ┙
        0x251a => [H, N, N, L], // ┚
        0x251b => [H, N, N, H], // ┛
        0x251c => [L, L, L, N], // ├
        0x251d => [L, H, L, N], // ┝
        0x251e => [H, L, L, N], // ┞
        0x251f => [L, L, H, N], // ┟
        0x2520 => [H, L, H, N], // ┠
        0x2521 => [H, H, L, N], // ┡
        0x2522 => [L, H, H, N], // ┢
        0x2523 => [H, H, H, N], // ┣
        0x2524 => [L, N, L, L], // ┤
        0x2525 => [L, N, L, H], // ┥
        0x2526 => [H, N, L, L], // ┦
        0x2527 => [L, N, H, L], // ┧
        0x2528 => [H, N, H, L], // ┨
        0x2529 => [H, N, L, H], // ┩
        0x252a => [L, N, H, H], // ┪
        0x252b => [H, N, H, H], // ┫
        0x252c => [N, L, L, L], // ┬
        0x252d => [N, L, L, H], // ┭
        0x252e => [N, H, L, L], // ┮
        0x252f => [N, H, L, H], // ┯
        0x2530 => [N, L, H, L], // ┰
        0x2531 => [N, L, H, H], // ┱
        0x2532 => [N, H, H, L], // ┲
        0x2533 => [N, H, H, H], // ┳
        0x2534 => [L, L, N, L], // ┴
        0x2535 => [L, L, N, H], // ┵
        0x2536 => [L, H, N, L], // ┶
        0x2537 => [L, H, N, H], // ┷
        0x2538 => [H, L, N, L], // ┸
        0x2539 => [H, L, N, H], // ┹
        0x253a => [H, H, N, L], // ┺
        0x253b => [H, H, N, H], // ┻
        0x253c => [L, L, L, L], // ┼
        0x253d => [L, L, L, H], // ┽
        0x253e => [L, H, L, L], // ┾
        0x253f => [L, H, L, H], // ┿
        0x2540 => [H, L, L, L], // ╀
        0x2541 => [L, L, H, L], // ╁
        0x2542 => [H, L, H, L], // ╂
        0x2543 => [H, L, L, H], // ╃
        0x2544 => [H, H, L, L], // ╄
        0x2545 => [L, L, H, H], // ╅
        0x2546 => [L, H, H, L], // ╆
        0x2547 => [H, H, L, H], // ╇
        0x2548 => [L, H, H, H], // ╈
        0x2549 => [H, L, H, H], // ╉
        0x254a => [H, H, H, L], // ╊
        0x254b => [H, H, H, H], // ╋
        0x2550 => [N, D, N, D], // ═
        0x2551 => [D, N, D, N], // ║
        0x2552 => [N, D, L, N], // ╒
        0x2553 => [N, L, D, N], // ╓
        0x2554 => [N, D, D, N], // ╔
        0x2555 => [N, N, L, D], // ╕
        0x2556 => [N, N, D, L], // ╖
        0x2557 => [N, N, D, D], // ╗
        0x2558 => [L, D, N, N], // ╘
        0x2559 => [D, L, N, N], // ╙
        0x255a => [D, D, N, N], // ╚
        0x255b => [L, N, N, D], // ╛
        0x255c => [D, N, N, L], // ╜
        0x255d => [D, N, N, D], // ╝
        0x255e => [L, D, L, N], // ╞
        0x255f => [D, L, D, N], // ╟
        0x2560 => [D, D, D, N], // ╠
        0x2561 => [L, N, L, D], // ╡
        0x2562 => [D, N, D, L], // ╢
        0x2563 => [D, N, D, D], // ╣
        0x2564 => [N, D, L, D], // ╤
        0x2565 => [N, L, D, L], // ╥
        0x2566 => [N, D, D, D], // ╦
        0x2567 => [L, D, N, D], // ╧
        0x2568 => [D, L, N, L], // ╨
        0x2569 => [D, D, N, D], // ╩
        0x256a => [L, D, L, D], // ╪
        0x256b => [D, L, D, L], // ╫
        0x256c => [D, D, D, D], // ╬
        0x2574 => [N, N, N, L], // ╴
        0x2575 => [L, N, N, N], // ╵
        0x2576 => [N, L, N, N], // ╶
        0x2577 => [N, N, L, N], // ╷
        0x2578 => [N, N, N, H], // ╸
        0x2579 => [H, N, N, N], // ╹
        0x257a => [N, H, N, N], // ╺
        0x257b => [N, N, H, N], // ╻
        0x257c => [N, H, N, L], // ╼
        0x257d => [L, N, H, N], // ╽
        0x257e => [N, L, N, H], // ╾
        0x257f => [H, N, L, N], // ╿
        0x2504..=0x250b | 0x254c..=0x254f => {
            let count = if cp >= 0x254c {
                2.0
            } else if cp >= 0x2508 {
                4.0
            } else {
                3.0
            };
            dashed(canvas, cp & 2 != 0, cp & 1 != 0, count);
            return;
        }
        0x256d..=0x2570 => {
            arc(canvas, cp);
            return;
        }
        0x2571..=0x2573 => {
            diagonal(canvas, cp);
            return;
        }
        _ => return,
    };
    // Rotating each arm into the same local frame keeps all four directions
    // governed by the same junction rules.
    for direction in 0..4 {
        arm(canvas, lines, direction);
    }
}

#[derive(Clone, Copy)]
struct Bands {
    light0: f32,
    light1: f32,
    heavy0: f32,
    heavy1: f32,
    double0: f32,
    double1: f32,
}
impl Bands {
    fn new(length: f32, thickness: f32) -> Self {
        let light0 = ((length - thickness).max(0.0) / 2.0).floor();
        let light1 = light0 + thickness;
        let heavy0 = ((length - 2.0 * thickness).max(0.0) / 2.0).floor();
        Self {
            light0,
            light1,
            heavy0,
            heavy1: heavy0 + 2.0 * thickness,
            double0: (light0 - thickness).max(0.0),
            double1: light1 + thickness,
        }
    }
}

fn arm(canvas: &mut Canvas, lines: [Weight; 4], direction: usize) {
    // Use native axis coordinates instead of geometric rotation: flipping an
    // odd-width cell would move its one-pixel center line by one pixel.
    let vertical = direction.is_multiple_of(2);
    let from_start = direction == 0 || direction == 3;
    let own = lines[direction];
    let opposite = lines[(direction + 2) % 4];
    let (negative, positive) = if vertical {
        (lines[3], lines[1])
    } else {
        (lines[0], lines[2])
    };
    let (across, along) = if vertical {
        (canvas.width, canvas.height)
    } else {
        (canvas.height, canvas.width)
    };
    let a = Bands::new(across, canvas.thickness);
    let b = Bands::new(along, canvas.thickness);
    let extent = if negative == H || positive == H {
        if from_start { b.heavy1 } else { b.heavy0 }
    } else if negative != positive || own == opposite {
        if negative == D || positive == D {
            if from_start { b.double1 } else { b.double0 }
        } else if from_start {
            b.light1
        } else {
            b.light0
        }
    } else if negative == N && positive == N {
        if from_start { b.light1 } else { b.light0 }
    } else if from_start {
        b.light0
    } else {
        b.light1
    };
    let mut rectangle = |a0, a1, end| {
        let (b0, b1) = if from_start { (0.0, end) } else { (end, along) };
        if vertical {
            canvas.rect(a0, b0, a1, b1);
        } else {
            canvas.rect(b0, a0, b1, a1);
        }
    };
    match own {
        N => {}
        L => rectangle(a.light0, a.light1, extent),
        H => rectangle(a.heavy0, a.heavy1, extent),
        D => {
            let junction = if from_start { b.light0 } else { b.light1 };
            rectangle(
                a.double0,
                a.light0,
                if negative == D { junction } else { extent },
            );
            rectangle(
                a.light1,
                a.double1,
                if positive == D { junction } else { extent },
            );
        }
    }
}

fn dashed(canvas: &mut Canvas, vertical: bool, heavy: bool, count: f32) {
    let length = if vertical {
        canvas.height
    } else {
        canvas.width
    };
    let across = if vertical {
        canvas.width
    } else {
        canvas.height
    };
    let thick = canvas.thickness * if heavy { 2.0 } else { 1.0 };
    let center = ((across - thick).max(0.0) / 2.0).floor();
    let desired_gap = if count > 2.0 {
        canvas.thickness.max(4.0)
    } else {
        canvas.thickness * if vertical { 2.0 } else { 1.0 }
    };
    let mut rectangle = |start, end| {
        if vertical {
            canvas.rect(center, start, center + thick, end);
        } else {
            canvas.rect(start, center, end, center + thick);
        }
    };
    if length < 2.0 * count {
        rectangle(0.0, length);
        return;
    }

    let gap = desired_gap.min((length / (2.0 * count)).floor());
    let available = length - gap * count;
    let dash = (available / count).floor();
    let mut remaining = available % count;
    let mut position = if vertical { 0.0 } else { (gap / 2.0).floor() };
    let mut index = 0.0;
    while index < count {
        let extra = if remaining > 0.0 { 1.0 } else { 0.0 };
        remaining -= extra;
        let end = position + dash + extra;
        rectangle(position, end);
        position = end + gap;
        index += 1.0;
    }
}

fn diagonal(canvas: &mut Canvas, cp: u32) {
    let w = canvas.width;
    let h = canvas.height;
    let dx = 0.5 * (w / h).min(1.0);
    let dy = 0.5 * (h / w).min(1.0);
    for rising in [true, false] {
        if (rising && cp == 0x2572) || (!rising && cp == 0x2571) {
            continue;
        }
        let (x0, x1) = if rising { (w + dx, -dx) } else { (-dx, w + dx) };
        canvas.geometry.strokes.push(Stroke {
            width: px(canvas.thickness / canvas.scale),
            start: canvas.point(x0, -dy),
            segments: vec![Segment::Line(canvas.point(x1, h + dy))],
        });
    }
}

fn arc(canvas: &mut Canvas, cp: u32) {
    let w = canvas.width;
    let h = canvas.height;
    let t = canvas.thickness;
    let cx = ((w - t).max(0.0) / 2.0).floor() + t / 2.0;
    let cy = ((h - t).max(0.0) / 2.0).floor() + t / 2.0;
    // Clamp to the available center-to-edge distance for very small cells.
    let radius = cx.min(w - cx).min(cy).min(h - cy).max(0.0);
    let right = cp == 0x256d || cp == 0x2570;
    let down = cp == 0x256d || cp == 0x256e;
    let sx = if right { 1.0 } else { -1.0 };
    let sy = if down { 1.0 } else { -1.0 };
    canvas.geometry.strokes.push(Stroke {
        width: px(t / canvas.scale),
        start: canvas.point(cx, if down { h } else { 0.0 }),
        segments: vec![
            Segment::Line(canvas.point(cx, cy + sy * radius)),
            Segment::Curve(
                canvas.point(cx + sx * radius, cy),
                canvas.point(cx, cy + sy * radius * 0.25),
                canvas.point(cx + sx * radius * 0.25, cy),
            ),
            Segment::Line(canvas.point(if right { w } else { 0.0 }, cy)),
        ],
    });
}
