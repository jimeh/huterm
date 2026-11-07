//! Compares Huterm's icon placement with the Nerd Fonts project's own
//! patched "Nerd Font Mono" builds of three families. Each fixture, written by
//! `mise run nerd-fonts:reference`, holds a family's cell metrics and the ink
//! box of every icon the regular family lacks, as the patched font draws it.

use gpui::px;

use super::constraint::{Constraint, Size};
use super::{GridMetrics, grid, ink_box, place};

const FIXTURES: [(&str, &str); 3] = [
    (
        "DejaVu Sans Mono",
        include_str!("reference/dejavu-sans-mono.txt"),
    ),
    ("Meslo LG M", include_str!("reference/meslo-lg-m.txt")),
    (
        "JetBrains Mono",
        include_str!("reference/jetbrains-mono.txt"),
    ),
];

struct Fixture {
    units_per_em: f64,
    advance: f64,
    ascent: f64,
    descent: f64,
    cap_height: f64,
    /// Code point and ink box: x min, y min, x max, y max in font units.
    icons: Vec<(char, [f64; 4])>,
}

fn parse(text: &str) -> Fixture {
    let mut fields = std::collections::HashMap::new();
    let mut icons = Vec::new();
    for line in text.lines().filter(|line| !line.starts_with('#')) {
        let mut words = line.split_whitespace();
        let Some(key) = words.next() else { continue };
        if let Ok(code) = u32::from_str_radix(key, 16)
            && key.len() == 5
        {
            let values: Vec<f64> =
                words.map(|word| word.parse().expect("number")).collect();
            let ch = char::from_u32(code).expect("scalar");
            icons.push((ch, [values[0], values[1], values[2], values[3]]));
        } else if key != "family" {
            fields.insert(
                key,
                words.next().expect("value").parse::<f64>().expect("number"),
            );
        }
    }
    Fixture {
        units_per_em: fields["units_per_em"],
        advance: fields["advance"],
        ascent: fields["ascent"],
        descent: fields["descent"],
        cap_height: fields["cap_height"],
        icons,
    }
}

/// The furthest any ink edge may sit from the patched font's, as a share of
/// the cell height. The patcher rounds glyphs to font units and Huterm rounds
/// origins to device pixels; the rest is measurement noise, which peaked at
/// 4.4% when this was written. Fitting the em square to the cell, the first
/// approach, missed by up to 40%.
const TOLERANCE: f64 = 0.05;

/// GPUI scales glyphs only uniformly, so Huterm sizes stretch rules and
/// aspect-ratio limits like fit, where the patcher distorts the glyph.
fn approximated(ch: char) -> bool {
    let constraint = Constraint::for_char(ch);
    constraint.size == Size::Stretch || constraint.max_xy_ratio.is_some()
}

#[test]
#[expect(
    clippy::cast_possible_truncation,
    reason = "cell-sized logical pixels fit f32"
)]
fn icons_match_patched_nerd_font_mono_families() {
    let mut compared = 0;
    let mut failures = Vec::new();
    for (family, text) in FIXTURES {
        let fixture = parse(text);
        assert!(
            fixture.icons.len() > 10_000,
            "{family} has {} icons",
            fixture.icons.len()
        );
        // A Retina size and a 1x size, which round origins differently.
        for (font_size, scale) in [(13.0_f32, 2.0_f32), (16.0, 1.0)] {
            let unit = f64::from(font_size) / fixture.units_per_em;
            let metrics = GridMetrics::from_measurements(
                px(font_size),
                px((fixture.advance * unit) as f32),
                px((fixture.ascent * unit) as f32),
                px((fixture.descent * unit) as f32),
                scale,
            );
            let grid = grid(metrics, px((fixture.cap_height * unit) as f32));
            let cell_height = f64::from(f32::from(metrics.cell_height));
            let baseline = f64::from(f32::from(metrics.baseline));
            let offset_x = f64::from(f32::from(metrics.glyph_offset_x));
            for &(ch, [x_min, y_min, x_max, y_max]) in &fixture.icons {
                if approximated(ch) {
                    continue;
                }
                let at = place(ch, metrics, &grid, 1).expect("placement");
                let ink = ink_box(ch, at.font_size).expect("ink");
                let left = f64::from(f32::from(at.origin.x)) + ink.x;
                let bottom = f64::from(f32::from(at.origin.y)) - ink.y;
                let ours =
                    [left, bottom - ink.height, left + ink.width, bottom];
                // The patched glyph draws like text: from the grid's glyph
                // offset and baseline.
                let theirs = [
                    offset_x + x_min * unit,
                    baseline - y_max * unit,
                    offset_x + x_max * unit,
                    baseline - y_min * unit,
                ];
                let deviation = ours
                    .iter()
                    .zip(theirs)
                    .map(|(ours, theirs)| (ours - theirs).abs())
                    .fold(0.0, f64::max)
                    / cell_height;
                if deviation > TOLERANCE {
                    failures.push(format!(
                        "{family} {font_size}px @{scale}x U+{:04X}: {:.1}% off; \
                         Huterm {ours:.2?}, patched {theirs:.2?}",
                        u32::from(ch),
                        deviation * 100.0
                    ));
                }
                compared += 1;
            }
        }
    }
    assert!(compared > 60_000, "compared only {compared} icons");
    assert!(
        failures.is_empty(),
        "{} of {compared} icons differ from the patched fonts by more than \
         {:.0}% of a cell; the first:\n{}",
        failures.len(),
        TOLERANCE * 100.0,
        failures[..failures.len().min(10)].join("\n")
    );
}
