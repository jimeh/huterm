#![expect(
    clippy::float_cmp,
    reason = "coverage values are exact binary fractions, not measured coordinates"
)]

use super::*;

fn metrics(width: f32, height: f32, scale: f32) -> GridMetrics {
    GridMetrics::from_measurements(
        px(12.0),
        px(width),
        px(height - 3.0),
        px(3.0),
        scale,
    )
}

fn coverage(geometry: &Geometry, x: f32, y: f32) -> f32 {
    geometry
        .rectangles
        .iter()
        .filter(|r| {
            x >= f32::from(r.bounds.origin.x)
                && x < f32::from(r.bounds.right())
                && y >= f32::from(r.bounds.origin.y)
                && y < f32::from(r.bounds.bottom())
        })
        .map(|r| r.opacity)
        .fold(0.0, f32::max)
}

fn bitmap(ch: char, w: u16, h: u16) -> String {
    let geometry =
        Geometry::new(ch, metrics(f32::from(w), f32::from(h), 1.0), 1);
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| {
                    if coverage(
                        &geometry,
                        f32::from(x) + 0.5,
                        f32::from(y) + 0.5,
                    ) > 0.0
                    {
                        '#'
                    } else {
                        '.'
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn coverage_is_exactly_the_supported_standalone_groups() {
    for cp in (0x2500..=0x259f)
        .chain(0xe0b0..=0xe0bf)
        .chain([0xe0d2, 0xe0d4])
        .chain(0x25e2..=0x25e5)
        .chain(0x25f8..=0x25fa)
        .chain([0x25ff])
    {
        let ch = char::from_u32(cp).unwrap();
        assert_eq!(character(&ch.to_string()), Some(ch));
        let g = Geometry::new(ch, metrics(9.0, 17.0, 1.0), 1);
        assert!(
            !g.rectangles.is_empty()
                || !g.strokes.is_empty()
                || !g.fills.is_empty(),
            "U+{cp:04X}"
        );
    }
    for text in [
        "",
        "A",
        "界",
        "😀",
        "\u{24ff}",
        "\u{25a0}",
        "\u{2801}",
        "\u{e0c0}",
        "\u{e0d3}",
        "◢\u{fe0f}",
        "\u{e0b0}\u{0301}",
        "▐\u{0301}",
        "▐\u{fe0f}",
        "▐▕",
    ] {
        assert_eq!(character(text), None, "{text:?}");
    }
}

#[test]
fn stacked_scrollbar_blocks_cover_every_row_to_the_right_edge() {
    for scale in [1.0, 1.25, 1.5, 2.0] {
        for width in [7.0, 7.5, 9.0] {
            let m = metrics(width, 14.0, scale);
            for ch in ['▐', '▕'] {
                let g = Geometry::new(ch, m, 1);
                assert_eq!(g.rectangles.len(), 1);
                let rect = &g.rectangles[0].bounds;
                assert_eq!(rect.origin.y, px(0.0));
                assert_eq!(rect.bottom(), m.cell_height);
                assert!(
                    (f32::from(rect.right() - m.cell_width) * scale).abs()
                        < 0.001
                );
                let fraction = if ch == '▐' { 0.5 } else { 0.125 };
                let expected =
                    (f32::from(m.cell_width) * scale * fraction).round();
                assert!(
                    (f32::from(rect.size.width) * scale - expected).abs()
                        < 0.001
                );
                // The next row begins exactly where the previous block ends.
                assert_eq!(rect.bottom(), rect.origin.y + m.cell_height);
            }
        }
    }
}

#[test]
fn quadrants_partition_odd_cells_without_holes_or_overlaps() {
    let m = metrics(7.0, 13.0, 1.0);
    let left = Geometry::new('▚', m, 1);
    let right = Geometry::new('▞', m, 1);
    for y in 0..13u16 {
        for x in 0..7u16 {
            assert_eq!(
                coverage(&left, f32::from(x) + 0.5, f32::from(y) + 0.5)
                    + coverage(&right, f32::from(x) + 0.5, f32::from(y) + 0.5),
                1.0
            );
        }
    }
}

#[test]
fn fractions_have_the_expected_orientation() {
    assert_eq!(bitmap('▀', 4, 4), "####\n####\n....\n....");
    assert_eq!(bitmap('▄', 4, 4), "....\n....\n####\n####");
    assert_eq!(bitmap('▌', 4, 4), "##..\n##..\n##..\n##..");
    assert_eq!(bitmap('▐', 4, 4), "..##\n..##\n..##\n..##");
    assert_eq!(bitmap('▕', 8, 4), ".......#\n.......#\n.......#\n.......#");
}

#[test]
fn double_corners_and_intersections_preserve_the_center_gap() {
    assert_eq!(
        bitmap('╔', 7, 7),
        ".......\n.......\n..#####\n..#....\n..#.###\n..#.#..\n..#.#.."
    );
    assert_eq!(
        bitmap('╬', 7, 7),
        "..#.#..\n..#.#..\n###.###\n.......\n###.###\n..#.#..\n..#.#.."
    );
    assert_eq!(
        bitmap('╪', 7, 7),
        "...#...\n...#...\n#######\n...#...\n#######\n...#...\n...#..."
    );
    assert_eq!(
        bitmap('┿', 7, 7),
        "...#...\n...#...\n#######\n#######\n...#...\n...#...\n...#..."
    );
}

#[test]
fn solid_lines_join_across_even_and_odd_cells() {
    for w in [7u16, 8, 15] {
        for h in [13u16, 14, 27] {
            let m = metrics(f32::from(w), f32::from(h), 1.0);
            for ch in ['│', '┃', '║'] {
                let g = Geometry::new(ch, m, 1);
                for x in 0..w {
                    let x = f32::from(x) + 0.5;
                    let top = coverage(&g, x, 0.5);
                    for y in 0..h {
                        assert_eq!(
                            coverage(&g, x, f32::from(y) + 0.5),
                            top,
                            "{ch}"
                        );
                    }
                }
            }
            for ch in ['─', '━', '═'] {
                let g = Geometry::new(ch, m, 1);
                for y in 0..h {
                    let y = f32::from(y) + 0.5;
                    let left = coverage(&g, 0.5, y);
                    for x in 0..w {
                        assert_eq!(
                            coverage(&g, f32::from(x) + 0.5, y),
                            left,
                            "{ch}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn dashes_have_separate_segments_and_degrade_to_solid_in_tiny_cells() {
    for (ch, count) in
        [('╌', 2), ('┄', 3), ('┈', 4), ('╎', 2), ('┆', 3), ('┊', 4)]
    {
        let g = Geometry::new(ch, metrics(16.0, 24.0, 1.0), 1);
        assert_eq!(g.rectangles.len(), count, "{ch}");
        let tiny = Geometry::new(ch, metrics(2.0, 3.0, 1.0), 1);
        assert_eq!(tiny.rectangles.len(), 1, "{ch}");
    }
}

#[test]
fn shade_levels_cover_the_whole_cell_and_wide_blocks_cover_both_columns() {
    for (ch, opacity) in [('░', 0.25), ('▒', 0.5), ('▓', 0.75)] {
        let g = Geometry::new(ch, metrics(7.0, 14.0, 2.0), 1);
        assert_eq!(coverage(&g, 0.25, 0.25), opacity);
        assert_eq!(coverage(&g, 6.75, 13.75), opacity);
    }
    let g = Geometry::new('█', metrics(7.0, 14.0, 2.0), 2);
    assert_eq!(g.rectangles[0].bounds.size, size(px(14.0), px(14.0)));
}

#[test]
fn curved_and_diagonal_paths_build_at_small_and_large_sizes() {
    for scale in [1.0, 1.25, 2.0] {
        for width in [2.0, 7.0, 20.0] {
            for ch in ['╭', '╮', '╯', '╰', '╱', '╲', '╳'] {
                let g =
                    Geometry::new(ch, metrics(width, width * 2.0, scale), 1);
                for stroke in &g.strokes {
                    let mut builder = PathBuilder::stroke(stroke.width);
                    builder.move_to(stroke.start);
                    for segment in &stroke.segments {
                        match segment {
                            Segment::Line(to) => builder.line_to(*to),
                            Segment::Curve(to, a, b) => {
                                builder.cubic_bezier_to(*to, *a, *b);
                            }
                        }
                    }
                    assert!(builder.build().is_ok(), "{ch} at {width}/{scale}");
                }
            }
        }
    }
}

#[test]
fn fractional_scales_preserve_integer_stroke_centers() {
    for hundredths in 101..250u16 {
        let scale = f32::from(hundredths) / 100.0;
        for width in 6..16u16 {
            let m = metrics(f32::from(width), 14.0, scale);
            let g = Geometry::new('│', m, 1);
            let physical_width = (f32::from(m.cell_width) * scale).round();
            let thickness = scale.round().max(1.0);
            let expected =
                ((physical_width - thickness).max(0.0) / 2.0).floor();
            let actual = f32::from(g.rectangles[0].bounds.origin.x) * scale;
            assert!(
                (actual - expected).abs() < 0.001,
                "width={width} scale={scale}: {actual} != {expected}"
            );
        }
    }
}

// Winding coverage tests the filled polygons, including transparent holes,
// independently of GPUI's tessellator. Curves are checked separately below.
fn polygon_contains(g: &Geometry, x: f32, y: f32) -> bool {
    g.fills.iter().any(|shape| {
        let points: Vec<_> = std::iter::once(shape.start)
            .chain(shape.segments.iter().map(|segment| match segment {
                Segment::Line(to) => *to,
                Segment::Curve(..) => panic!("polygon test received a curve"),
            }))
            .collect();
        let mut winding = 0;
        for (a, b) in points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .take(points.len())
        {
            let (ax, ay, bx, by) = (
                f32::from(a.x),
                f32::from(a.y),
                f32::from(b.x),
                f32::from(b.y),
            );
            let side = (bx - ax) * (y - ay) - (x - ax) * (by - ay);
            if ay <= y && by > y && side > 0.0 {
                winding += 1;
            }
            if ay > y && by <= y && side < 0.0 {
                winding -= 1;
            }
        }
        winding != 0
    })
}

#[test]
fn corner_triangles_fill_the_named_corner_and_keep_outlines_inside() {
    for scale in [1.0, 1.25, 2.0] {
        for (filled, outline, right, bottom) in [
            ('◤', '◸', false, false),
            ('◥', '◹', true, false),
            ('◣', '◺', false, true),
            ('◢', '◿', true, true),
        ] {
            let m = metrics(10.0, 20.0, scale);
            let (w, h) = (f32::from(m.cell_width), f32::from(m.cell_height));
            let sample = |g: &Geometry, x, y| {
                polygon_contains(
                    g,
                    if right { w - x } else { x },
                    if bottom { h - y } else { y },
                )
            };
            let solid = Geometry::new(filled, m, 1);
            let hollow = Geometry::new(outline, m, 1);
            assert!(sample(&solid, 2.0, 2.0), "{filled}");
            assert!(!sample(&solid, w - 0.1, h - 0.1), "{filled}");
            assert!(sample(&hollow, 0.1, 2.0), "{outline} outer edge");
            assert!(!sample(&hollow, 2.0, 2.0), "{outline} hollow center");
            assert!(!sample(&hollow, -0.1, 2.0), "{outline} outside");
        }
    }
}

#[test]
fn powerline_tips_and_split_separators_have_expected_orientation_and_gap() {
    let m = metrics(10.0, 20.0, 1.0);
    for (ch, mirrored) in [('\u{e0b0}', false), ('\u{e0b2}', true)] {
        let g = Geometry::new(ch, m, 1);
        let x = |v| if mirrored { 10.0 - v } else { v };
        assert!(polygon_contains(&g, x(0.1), 0.5));
        assert!(polygon_contains(&g, x(9.8), 10.0));
        assert!(!polygon_contains(&g, x(9.8), 0.5));
    }
    for ch in ['\u{e0d2}', '\u{e0d4}'] {
        let g = Geometry::new(ch, m, 1);
        for x in [0.1, 5.0, 9.9] {
            assert!(polygon_contains(&g, x, 0.01));
            assert!(polygon_contains(&g, x, 19.99));
            assert!(!polygon_contains(&g, x, 10.0));
        }
    }
}

#[test]
fn filled_paths_tessellate_at_tiny_wide_and_fractional_sizes() {
    for scale in [1.0, 1.11, 1.5, 2.0] {
        for width in [1.0, 7.0, 20.0] {
            for columns in [1, 2] {
                for ch in "\u{e0b0}\u{e0b2}\u{e0b4}\u{e0b5}\u{e0b6}\u{e0b7}\u{e0d2}\u{e0d4}◢◣◤◥◸◹◺◿".chars() {
                    let g = Geometry::new(ch, metrics(width, 14.0, scale), columns);
                    for shape in &g.fills {
                        let mut builder = PathBuilder::fill();
                        builder.move_to(shape.start);
                        for segment in &shape.segments {
                            match segment {
                                Segment::Line(to) => builder.line_to(*to),
                                Segment::Curve(to, a, b) => builder.cubic_bezier_to(*to, *a, *b),
                            }
                        }
                        builder.close();
                        assert!(builder.build().is_ok(), "{ch} width={width} scale={scale} columns={columns}");
                    }
                }
            }
        }
    }
}
