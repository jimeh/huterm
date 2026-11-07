//! Expected boxes come from Ghostty's "Constraints" test in src/font/Glyph.zig,
//! which measured them with CoreText at 12 pt and 96 DPI. Copyright (c) 2024
//! Mitchell Hashimoto, Ghostty contributors, under MIT; see
//! third-party/nerd-font-constraints/README.md.

use super::*;

/// The grid of the monospace font in Ghostty's test.
const METRICS: Metrics = Metrics {
    cell_width: 10.0,
    cell_height: 22.0,
    face_width: 9.6,
    face_height: 21.12,
    face_y: 0.2,
    icon_height: 21.12,
    icon_height_single: 44.48 / 3.0,
};

#[track_caller]
fn assert_box(expected: GlyphBox, actual: GlyphBox) {
    for (name, want, got) in [
        ("width", expected.width, actual.width),
        ("height", expected.height, actual.height),
        ("x", expected.x, actual.x),
        ("y", expected.y, actual.y),
    ] {
        assert!(
            (want - got).abs() < 1e-6,
            "{name}: expected {want}, got {got} ({actual:?})"
        );
    }
}

#[test]
fn fit_scales_a_wide_symbol_into_one_cell_and_leaves_two() {
    // U+25A0 from Iosevka, which spans two cells.
    let glyph = GlyphBox {
        width: 10.272,
        height: 10.272,
        x: 2.864,
        y: 5.304,
    };
    assert_box(
        GlyphBox {
            width: 9.6,
            height: 9.6,
            x: 0.0,
            y: 5.64,
        },
        Constraint::FIT.constrain(glyph, &METRICS, 1),
    );
    assert_box(glyph, Constraint::FIT.constrain(glyph, &METRICS, 2));
}

#[test]
fn grouped_icon_scales_its_group_to_one_cell_and_keeps_size_in_two() {
    // Nerd Fonts 3.4.0's rule for U+EA61 (Codicons lightbulb), from
    // Ghostty's generated table, and its box from that release.
    let constraint = Constraint {
        size: Size::FitCover1,
        height: Height::Icon,
        align_horizontal: Align::Center1,
        align_vertical: Align::Center1,
        relative_width: 0.751_302_083_333_333_4,
        relative_height: 0.929_157_345_264_727_8,
        relative_x: 0.084_635_416_666_666_7,
        relative_y: 0.070_842_654_735_272_2,
        ..Constraint::DEFAULT
    };
    let glyph = GlyphBox {
        width: 9.015_625,
        height: 13.015_625,
        x: 3.015_625,
        y: 3.765_25,
    };
    assert_box(
        GlyphBox {
            width: 7.2125,
            height: 10.4125,
            x: 0.8125,
            y: 5.950_695_224_719_102,
        },
        constraint.constrain(glyph, &METRICS, 1),
    );
    assert_box(
        GlyphBox {
            width: glyph.width,
            height: glyph.height,
            x: 1.015_625,
            y: 4.748_369_030_898_877_5,
        },
        constraint.constrain(glyph, &METRICS, 2),
    );
}

#[test]
fn stretch_falls_back_to_a_uniform_fit_against_the_grid() {
    let constraint = Constraint::for_char('\u{e0c0}');
    assert_eq!(constraint.size, Size::Stretch);
    assert_eq!(constraint.align_horizontal, Align::Start);
    // U+E0C0 (flame) from the symbols font; Ghostty stretches it to fill
    // the cell exactly.
    let glyph = GlyphBox {
        width: 16.796_875,
        height: 16.468_75,
        x: -0.796_875,
        y: 1.710_937_5,
    };
    let fitted = constraint.constrain(glyph, &METRICS, 1);
    // The width reaches the cell, the shape is kept, and the glyph starts
    // at the left edge, centred vertically in the cell.
    assert!((fitted.width - METRICS.cell_width).abs() < 1e-9);
    assert!(
        (fitted.width / fitted.height - glyph.width / glyph.height).abs()
            < 1e-9
    );
    assert!(fitted.x.abs() < 1e-9);
    let below = fitted.y;
    let above = METRICS.cell_height - fitted.y - fitted.height;
    assert!((below - above).abs() < 1e-9, "{below} {above}");
}

#[test]
fn table_ranges_are_sorted_disjoint_and_index_constraints() {
    let mut previous_end = None;
    for &(start, end, index) in &table::RANGES {
        assert!(start <= end, "{start:#X}..={end:#X}");
        if let Some(previous) = previous_end {
            assert!(start > previous, "{start:#X} overlaps {previous:#X}");
        }
        previous_end = Some(end);
        assert!(usize::from(index) < table::CONSTRAINTS.len());
    }
    // A Powerline symbol follows font-patcher 3.5.1's `^pa1` rule, and text
    // has no rule.
    let branch = Constraint::for_char('\u{e0a0}');
    assert_eq!(branch.size, Size::FitCover1);
    assert_eq!(branch.height, Height::Cell);
    assert_eq!(branch.max_constraint_width, 1);
    assert_eq!(Constraint::for_char('a'), Constraint::FIT);
}

#[test]
fn table_matches_the_bundled_font_release() {
    let version = super::super::face()
        .expect("bundled face")
        .names()
        .into_iter()
        .filter(|name| name.name_id == ttf_parser::name_id::VERSION)
        .find_map(|name| name.to_string())
        .expect("a Unicode version name");
    assert!(
        version.contains(&format!("Nerd Fonts {}", table::NERD_FONTS_VERSION)),
        "run `mise run nerd-fonts:constraints` for {version:?}"
    );
}
