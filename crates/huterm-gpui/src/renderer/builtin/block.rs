//! Block Elements, U+2580–U+259F.
// Adapted from Ghostty's sprite/draw/block.zig; MIT, see the parent module.

use super::Canvas;

pub(super) fn draw(ch: char, canvas: &mut Canvas) {
    let w = canvas.width;
    let h = canvas.height;
    let cp = u32::from(ch);
    match cp {
        0x2580 => canvas.rect(0.0, 0.0, w, (h / 2.0).round()),
        0x2581..=0x2588 => {
            let eighths =
                f32::from(u16::try_from(cp - 0x2580).unwrap_or_default());
            canvas.rect(0.0, h - (h * eighths / 8.0).round(), w, h);
        }
        0x2589..=0x258f => {
            let eighths =
                f32::from(u16::try_from(0x2590 - cp).unwrap_or_default());
            canvas.rect(0.0, 0.0, (w * eighths / 8.0).round(), h);
        }
        0x2590 => canvas.rect(w - (w / 2.0).round(), 0.0, w, h),
        0x2591..=0x2593 => {
            let shade =
                f32::from(u16::try_from(cp - 0x2590).unwrap_or_default()) / 4.0;
            canvas.shaded_rect(0.0, 0.0, w, h, shade);
        }
        0x2594 => canvas.rect(0.0, 0.0, w, (h / 8.0).round()),
        0x2595 => canvas.rect(w - (w / 8.0).round(), 0.0, w, h),
        0x2596..=0x259f => {
            // Bits are top-left, top-right, bottom-left, bottom-right.
            let mask = match cp {
                0x2596 => 0b0100,
                0x2597 => 0b1000,
                0x2598 => 0b0001,
                0x2599 => 0b1101,
                0x259a => 0b1001,
                0x259b => 0b0111,
                0x259c => 0b1011,
                0x259d => 0b0010,
                0x259e => 0b0110,
                _ => 0b1110,
            };
            let x = (w / 2.0).round();
            let y = (h / 2.0).round();
            for (bit, bounds) in [
                (1, [0.0, 0.0, x, y]),
                (2, [x, 0.0, w, y]),
                (4, [0.0, y, x, h]),
                (8, [x, y, w, h]),
            ] {
                if mask & bit != 0 {
                    canvas.rect(bounds[0], bounds[1], bounds[2], bounds[3]);
                }
            }
        }
        _ => {}
    }
}
