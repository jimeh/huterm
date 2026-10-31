use gpui::Size;
use huterm_config::{SyncIndicatorConfig, ZoomIndicatorConfig};

use super::super::button_layout::ButtonLayout;
use super::super::tab_visibility::Presentation;
use super::super::{ChromeLayout, TabPosition, WindowFrame, title_row_height};
use super::*;

const VIEWPORT: Size<Pixels> = Size {
    width: px(800.0),
    height: px(600.0),
};
const TITLEBAR: Pixels = px(32.0);

/// What `AppKit` reports for a titled window on macOS 26 and later.
const MACOS: NativeRadii = NativeRadii {
    radii: WindowRadii {
        top: px(16.0),
        bottom: px(16.0),
    },
    source: RadiusSource::AppKit,
};

fn corners(
    top_left: f32,
    top_right: f32,
    bottom_right: f32,
    bottom_left: f32,
) -> Corners {
    Corners {
        top_left: px(top_left),
        top_right: px(top_right),
        bottom_right: px(bottom_right),
        bottom_left: px(bottom_left),
    }
}

/// The pane area's corners in an 800×600 macOS window with a titlebar
/// and a tab bar at `position` shown as `presentation`, and the rounded
/// terminal corner beside a vertical bar when `inner` holds.
fn macos_corners(
    position: TabPosition,
    presentation: Presentation,
    chrome_hidden: bool,
    inner: bool,
) -> Corners {
    let titlebar = if chrome_hidden { px(0.0) } else { TITLEBAR };
    let layout = ChromeLayout::new(VIEWPORT, titlebar, position).present(
        presentation,
        position,
        1.0,
    );
    let radii = window_radii(chrome_hidden, Platform::MacOs, MACOS, false);
    let terminal = inner.then(|| TerminalCorner {
        at: if position == TabPosition::Left {
            layout.terminal.origin
        } else {
            layout.terminal.top_right()
        },
        radius: px(6.0),
    });
    area_corners(layout.content, layout.terminal, radii, terminal)
}

#[test]
fn only_corners_on_a_window_corner_or_huterms_own_rounding_are_round() {
    use Presentation::{Hidden, Reserved};
    use TabPosition::{Bottom, Left, Right, Titlebar, Top};
    // The titlebar strip always sits above the panes, so only the bottom
    // corners can meet the window's.
    for position in [Top, Titlebar] {
        assert_eq!(
            macos_corners(position, Reserved, false, false),
            corners(0.0, 0.0, 16.0, 16.0),
            "{position:?}"
        );
    }
    assert_eq!(
        macos_corners(Bottom, Reserved, false, false),
        corners(0.0, 0.0, 0.0, 0.0)
    );
    assert_eq!(
        macos_corners(Left, Reserved, false, false),
        corners(0.0, 0.0, 16.0, 0.0)
    );
    assert_eq!(
        macos_corners(Right, Reserved, false, false),
        corners(0.0, 0.0, 0.0, 16.0)
    );
    // A hidden bar gives the panes its room, down to the window's corners.
    for position in [Bottom, Left, Right] {
        assert_eq!(
            macos_corners(position, Hidden, false, false),
            corners(0.0, 0.0, 16.0, 16.0),
            "{position:?}"
        );
    }
    // The corner Huterm rounds beside a vertical tab bar takes its own
    // radius; a flush active row leaves it square.
    assert_eq!(
        macos_corners(Left, Reserved, false, true),
        corners(6.0, 0.0, 16.0, 0.0)
    );
    assert_eq!(
        macos_corners(Right, Reserved, false, true),
        corners(0.0, 6.0, 0.0, 16.0)
    );
    // Fullscreen and a Quake presentation hide the chrome: every corner
    // is square, even where the panes reach the window's corners.
    assert_eq!(
        macos_corners(Top, Hidden, true, false),
        corners(0.0, 0.0, 0.0, 0.0)
    );
}

#[test]
fn window_radii_follow_the_current_presentation_and_platform() {
    let zero = WindowRadii::default();
    // Fullscreen, or a Quake window presented as a drop-down.
    assert_eq!(window_radii(true, Platform::MacOs, MACOS, false), zero);
    // A Quake window shown as a regular window keeps its rounded corners.
    assert_eq!(
        window_radii(false, Platform::MacOs, MACOS, false),
        MACOS.radii
    );
    // Linux under server decorations, or a client frame drawn without a
    // border, as when tiled.
    assert_eq!(window_radii(false, Platform::Linux, MACOS, false), zero);
    // A decorated client frame rounds only its top corners.
    let decorated = window_radii(false, Platform::Linux, MACOS, true);
    assert_eq!(
        decorated,
        WindowRadii {
            top: FRAME_RADIUS,
            bottom: px(0.0)
        }
    );
    assert_eq!(window_radii(true, Platform::Linux, MACOS, true), zero);
    // Its title row keeps the panes away from those corners.
    let frame = WindowFrame {
        inset: gpui::Edges::all(px(10.0)),
        controls: true,
        buttons: ButtonLayout::default(),
    };
    let layout =
        ChromeLayout::with_frame(VIEWPORT, TabPosition::Titlebar, frame);
    assert!(title_row_height(false, frame) > px(0.0));
    assert_eq!(
        area_corners(layout.content, layout.terminal, decorated, None),
        Corners::default()
    );
}

#[test]
fn macos_releases_without_appkits_selectors_use_the_version_table() {
    assert_eq!(table_radius(26), px(16.0));
    assert_eq!(table_radius(27), px(16.0));
    assert_eq!(table_radius(15), px(10.0));
    assert_eq!(table_radius(0), px(10.0));
    assert_eq!(RadiusSource::default(), RadiusSource::None);
    assert_eq!(
        [
            RadiusSource::AppKit,
            RadiusSource::Table,
            RadiusSource::None
        ]
        .map(RadiusSource::name),
        ["appkit", "table", "none"]
    );
}

#[test]
fn zoom_wins_over_sync_and_off_still_resolves() {
    let mut panes = PanesConfig::default();
    let state = |syncing, zoomed, panes: &PanesConfig| {
        resolve_look(syncing, zoomed, panes).map(|active| active.state)
    };
    assert_eq!(state(false, false, &panes), None);
    assert_eq!(state(true, false, &panes), Some(IndicatorState::Sync));
    assert_eq!(state(false, true, &panes), Some(IndicatorState::Zoom));
    // Typing reaches only the zoomed pane, so its look wins.
    assert_eq!(state(true, true, &panes), Some(IndicatorState::Zoom));
    // An exited focused pane takes no input, so the caller passes no sync.
    assert_eq!(
        resolve_look(true, false, &panes).map(|active| active.look),
        Some(SyncIndicatorConfig::default().into())
    );
    assert_eq!(
        resolve_look(true, true, &panes).map(|active| active.look),
        Some(ZoomIndicatorConfig::default().into())
    );
    panes.sync_indicator.style = IndicatorStyle::Off;
    let off = resolve_look(true, false, &panes).unwrap();
    assert_eq!(off.look.style, IndicatorStyle::Off);
    assert!(!paints(off.look.style) && !draws_dividers(Some(off)));
    panes.sync_indicator.style = IndicatorStyle::Edge;
    assert!(!draws_dividers(resolve_look(true, false, &panes)));
    for style in [
        IndicatorStyle::Outline,
        IndicatorStyle::Dividers,
        IndicatorStyle::Glow,
    ] {
        panes.sync_indicator.style = style;
        assert!(
            draws_dividers(resolve_look(true, false, &panes)),
            "{style:?}"
        );
        // A zoomed tab shows no dividers to draw.
        assert!(
            !draws_dividers(resolve_look(true, true, &panes)),
            "{style:?}"
        );
    }
}

/// `columns(pane, rows(pane, columns(pane, pane)))` in an 800×600 area
/// at the origin: a vertical divider at x = 400, a horizontal one across
/// the right column at y = 300, and a vertical one below it at x = 600.
fn nested_dividers() -> Vec<DividerLine> {
    let line = |x: f32, y: f32, w: f32, h: f32| {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    };
    vec![
        DividerLine {
            axis: SplitAxis::Columns,
            line: line(400.0, 0.0, 1.0, 600.0),
        },
        DividerLine {
            axis: SplitAxis::Rows,
            line: line(401.0, 300.0, 399.0, 1.0),
        },
        DividerLine {
            axis: SplitAxis::Columns,
            line: line(600.0, 301.0, 1.0, 299.0),
        },
    ]
}

fn key(style: IndicatorStyle, width: u8, pattern: LinePattern) -> PathKey {
    PathKey {
        area: Bounds::new(point(px(0.0), px(0.0)), VIEWPORT),
        corners: corners(0.0, 0.0, 16.0, 16.0),
        dividers: nested_dividers(),
        scale: 2.0,
        style,
        width,
        pattern,
    }
}

/// Whether two rectangles share any area.
fn overlap(a: Bounds<Pixels>, b: Bounds<Pixels>) -> bool {
    let (a, b) = (Rect::of(a), Rect::of(b));
    a.left < b.right - 1e-3
        && b.left < a.right - 1e-3
        && a.top < b.bottom - 1e-3
        && b.top < a.bottom - 1e-3
}

#[test]
fn dividers_stop_at_the_outline_and_at_t_junctions_without_overlapping() {
    for width in [1, 2, 3, 6] {
        let shapes =
            shapes(&key(IndicatorStyle::Outline, width, LinePattern::Solid));
        let w = f32::from(width);
        let inside = Rect {
            left: w,
            top: w,
            right: 800.0 - w,
            bottom: 600.0 - w,
        };
        assert_eq!(shapes.dividers.len(), 3, "width {width}");
        for (index, band) in shapes.dividers.iter().enumerate() {
            let band = Rect::of(*band);
            // Inside the outline's inner edge, so the ring never doubles.
            assert!(
                band.left >= inside.left - 1e-3
                    && band.top >= inside.top - 1e-3
                    && band.right <= inside.right + 1e-3
                    && band.bottom <= inside.bottom + 1e-3,
                "width {width}: divider {index} {band:?} crosses the outline"
            );
            for (other_index, other) in
                shapes.dividers.iter().enumerate().skip(index + 1)
            {
                assert!(
                    !overlap(band.bounds(), *other),
                    "width {width}: dividers {index} and {other_index} overlap: {band:?} {other:?}"
                );
            }
        }
        // Each band is `width` across, centred on its line, and its stems
        // reach the band they meet.
        let [main, across, stem] = shapes.dividers[..] else {
            panic!("three dividers");
        };
        assert_eq!(main.size.width, px(w));
        assert_eq!(
            f32::from(main.origin.x) + w / 2.0,
            400.5,
            "width {width}: the main divider is centred on its line"
        );
        assert_eq!(across.origin.x, main.right(), "width {width}");
        assert_eq!(stem.origin.y, across.bottom(), "width {width}");
        assert_eq!(across.right(), px(800.0 - w), "width {width}");
    }
    // Without an outline, the dividers alone run to the area's edges.
    let shapes = shapes(&key(IndicatorStyle::Dividers, 2, LinePattern::Solid));
    let main = shapes.dividers[0];
    assert_eq!((main.origin.y, main.bottom()), (px(0.0), px(600.0)));
    assert_eq!(shapes.lines.len(), 3, "no outline sides");
}

#[test]
fn dashes_and_dots_scale_with_the_width() {
    for width in [1.0, 2.0, 6.0] {
        assert_eq!(dash_lengths(width), (3.0 * width, 2.0 * width));
        assert_eq!(dot_spacing(width), 3.0 * width);
        let length = 100.0 * width;
        let line = Track::line(Pt { x: 0.0, y: 0.0 }, Pt { x: length, y: 0.0 });
        let dashes = pattern_marks(&line, LinePattern::Dashed, width);
        // 100 widths hold 20 periods of 5.
        assert_eq!(dashes.len(), 20, "{width}");
        let Mark::Run { points, closed } = &dashes[1] else {
            panic!("a dash");
        };
        assert!(!closed);
        assert_eq!(
            (points[0].x, points[points.len() - 1].x),
            (5.0 * width, 8.0 * width),
            "{width}"
        );
        let dots = pattern_marks(&line, LinePattern::Dotted, width);
        let centres: Vec<f32> = dots
            .iter()
            .map(|mark| match mark {
                Mark::Dot { center } => center.x,
                Mark::Run { .. } => panic!("a dot"),
            })
            .collect();
        // Inside the line's ends, one spacing apart.
        assert_eq!(centres[0], width / 2.0);
        assert!(
            centres
                .windows(2)
                .all(|pair| (pair[1] - pair[0] - 3.0 * width).abs() < 1e-3)
        );
        assert!(*centres.last().unwrap() <= length - width / 2.0);
    }
}

#[test]
fn a_line_shorter_than_one_period_draws_solid() {
    let width = 2.0;
    let line = |length: f32| {
        Track::line(Pt { x: 0.0, y: 0.0 }, Pt { x: length, y: 0.0 })
    };
    // A dash and a gap are 10 points at this width, a dot spacing 6.
    for (pattern, short) in
        [(LinePattern::Dashed, 9.0), (LinePattern::Dotted, 5.0)]
    {
        let marks = pattern_marks(&line(short), pattern, width);
        assert_eq!(
            marks,
            [Mark::Run {
                points: vec![Pt { x: 0.0, y: 0.0 }, Pt { x: short, y: 0.0 }],
                closed: false
            }],
            "{pattern:?}"
        );
    }
    // One full period is long enough for the pattern.
    assert_eq!(
        pattern_marks(&line(10.0), LinePattern::Dashed, width),
        [Mark::Run {
            points: vec![Pt { x: 0.0, y: 0.0 }, Pt { x: 6.0, y: 0.0 }],
            closed: false
        }]
    );
    assert_eq!(
        pattern_marks(&line(6.0), LinePattern::Dotted, width).len(),
        1
    );
}

#[test]
fn a_closed_outline_starts_and_ends_on_the_middle_of_its_top_edge() {
    let rect = Rect {
        left: 1.0,
        top: 1.0,
        right: 799.0,
        bottom: 599.0,
    };
    let track = Track::rounded(rect, [0.0, 0.0, 15.0, 15.0]);
    assert!(track.closed);
    assert!(matches!(track.pieces.first(), Some(Piece::Line { .. })));
    assert!(matches!(track.pieces.last(), Some(Piece::Line { .. })));
    let seam = track.at(0.0).unwrap();
    assert_eq!(seam, Pt { x: 400.0, y: 1.0 });
    // The perimeter with two rounded corners.
    let expected = 2.0 * (798.0 + 598.0) - 4.0 * 15.0 + PI * 15.0;
    assert!(
        (track.length() - expected).abs() < 1e-2,
        "{}",
        track.length()
    );
    // The last dash ends on the top edge too, so the seam is straight.
    let dashes = pattern_marks(&track, LinePattern::Dashed, 2.0);
    let Some(Mark::Run { points, .. }) = dashes.last() else {
        panic!("a dash");
    };
    let end = points[points.len() - 1];
    assert!(
        end.y.to_bits() == 1.0_f32.to_bits() && end.x > 1.0 && end.x <= 400.0,
        "{end:?}"
    );
    // Arcs follow the corner: the bottom-right arc's middle sits on the
    // circle around its centre.
    let arc = track
        .pieces
        .iter()
        .find_map(|piece| match piece {
            Piece::Arc { center, radius, .. } => {
                Some((*piece, *center, *radius))
            }
            Piece::Line { .. } => None,
        })
        .unwrap();
    let middle = arc.0.at(arc.0.length() / 2.0);
    assert!(
        ((middle.x - arc.1.x).hypot(middle.y - arc.1.y) - arc.2).abs() < 1e-3
    );
}

#[test]
fn a_dotted_closed_outline_keeps_a_dot_apart_at_its_seam() {
    let rect = Rect {
        left: 0.5,
        top: 0.5,
        right: 99.5,
        bottom: 49.5,
    };
    let track = Track::rounded(rect, [0.0; 4]);
    let dots = pattern_marks(&track, LinePattern::Dotted, 1.0);
    let Some(Mark::Dot { center: last }) = dots.last() else {
        panic!("dots");
    };
    let Some(Mark::Dot { center: first }) = dots.first() else {
        panic!("dots");
    };
    assert!((last.x - first.x).hypot(last.y - first.y) >= 1.0 - 1e-3);
}

#[test]
fn a_dotted_hairline_around_a_4k_area_splits_into_paths_that_all_build() {
    for scale in [1.0, 2.0] {
        let key = PathKey {
            area: Bounds::new(
                point(px(0.0), px(0.0)),
                size(px(3840.0), px(2160.0)),
            ),
            corners: corners(0.0, 0.0, 16.0, 16.0),
            dividers: Vec::new(),
            scale,
            style: IndicatorStyle::Outline,
            width: 1,
            pattern: LinePattern::Dotted,
        };
        let painted = paint(&key);
        assert_eq!(painted.failed, 0, "scale {scale}");
        assert!(
            painted.paths.len() > 1,
            "scale {scale}: one path of every dot"
        );
        assert!(
            painted
                .paths
                .iter()
                .all(|path| !path.path.vertices.is_empty())
        );
    }
}

#[test]
fn one_path_past_lyons_vertex_index_fails_to_build() {
    // The limit the chunking keeps under: 9,000 octagons hold 72,000
    // vertices, past `u16`.
    let mut builder = PathBuilder::fill();
    for index in 0..9_000_u16 {
        let center = Pt {
            x: f32::from(index % 100) * 4.0,
            y: f32::from(index / 100) * 4.0,
        };
        builder.add_polygon(&dot_polygon(center, 1.0, 8), true);
    }
    assert!(builder.build().is_err());
}

#[test]
fn glow_strokes_fall_off_inward_and_dividers_glow_both_ways() {
    let shapes = shapes(&key(IndicatorStyle::Glow, 2, LinePattern::Dashed));
    // `width × 4` one-point strokes around the edge.
    let edge: Vec<&Stroke> = shapes
        .strokes
        .iter()
        .filter(|stroke| stroke.track.closed)
        .collect();
    assert_eq!(edge.len(), 8);
    assert!(edge.windows(2).all(|pair| pair[1].alpha < pair[0].alpha));
    assert!((edge[0].alpha - GLOW_PEAK).abs() < 1e-6);
    // Glow ignores the pattern.
    assert!(
        shapes
            .strokes
            .iter()
            .all(|stroke| stroke.pattern == LinePattern::Solid
                && (stroke.width - 1.0).abs() < f32::EPSILON)
    );
    // Each divider: its own line plus eight strokes on each side.
    assert_eq!(shapes.strokes.len(), 8 + 3 * (1 + 16));
    assert_eq!(shapes.dividers.len(), 3);
    // A glow is no line the focus cue must clear.
    assert_eq!(shapes.lines, []);
}

#[test]
fn the_edge_bar_runs_along_the_top_and_follows_its_corners() {
    let mut edge = key(IndicatorStyle::Edge, 3, LinePattern::Solid);
    edge.corners = corners(6.0, 0.0, 16.0, 16.0);
    let shapes = shapes(&edge);
    assert!(shapes.strokes.is_empty() && shapes.dividers.is_empty());
    assert_eq!(
        shapes.lines,
        [Bounds::new(
            point(px(0.0), px(0.0)),
            size(px(800.0), px(3.0))
        )]
    );
    let [fill] = &shapes.fills[..] else {
        panic!("one bar");
    };
    // The top-left end starts where the arc does and widens downward; the
    // square top-right end stays at the edge.
    assert_eq!(fill.points[0], Pt { x: 6.0, y: 0.0 });
    assert!(fill.points[8].x < 6.0 && fill.points[8].x > 0.0);
    assert!(
        fill.points[9..]
            .iter()
            .all(|at| (at.x - 800.0).abs() < f32::EPSILON)
    );
    // A patterned bar stays inside the arcs.
    edge.pattern = LinePattern::Dashed;
    let shapes = super::shapes(&edge);
    let [stroke] = &shapes.strokes[..] else {
        panic!("one bar");
    };
    assert_eq!(
        stroke.track.pieces,
        [Piece::Line {
            from: Pt { x: 6.0, y: 1.5 },
            to: Pt { x: 800.0, y: 1.5 }
        }]
    );
}

#[test]
fn the_focus_cue_keeps_a_lines_width_clear_of_every_line_it_meets() {
    let shapes = shapes(&key(IndicatorStyle::Outline, 2, LinePattern::Dashed));
    let area = Bounds::new(point(px(0.0), px(0.0)), VIEWPORT);
    let cue = |frame: Bounds<Pixels>, bottom: bool| {
        inset_cue(
            tab_bar::pane_focus_cue_bounds(frame, bottom),
            bottom,
            &shapes.lines,
            2.0,
            area,
            corners(0.0, 0.0, 16.0, 16.0),
        )
    };
    let rect = |x: f32, y: f32, w: f32, h: f32| {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    };
    // The bottom-right pane: below the horizontal divider, right of the
    // stem. Its top cue clears the divider's band below it, and its ends
    // clear the stem and the outline.
    // The bands run from 299.5 to 301.5 down and 599.5 to 601.5 across.
    let frame = rect(601.0, 301.0, 199.0, 299.0);
    assert_eq!(cue(frame, false), rect(603.5, 303.5, 192.5, 2.0));
    // The left pane's top cue clears the outline's top and left side and
    // the main divider's band, from 399.5 to 401.5.
    let frame = rect(0.0, 0.0, 400.0, 600.0);
    assert_eq!(cue(frame, false), rect(4.0, 4.0, 393.5, 2.0));
    // A bottom cue rises over the outline and stays clear of the rounded
    // bottom-left corner.
    let bottom = cue(frame, true);
    assert_eq!(bottom.bottom(), px(596.0));
    assert_eq!(bottom.origin.x, px(16.0));
    // No lines leave the cue where the bar would be.
    assert_eq!(
        inset_cue(
            tab_bar::pane_focus_cue_bounds(frame, false),
            false,
            &[],
            0.0,
            area,
            Corners::default(),
        ),
        rect(0.0, 0.0, 400.0, 2.0)
    );
}

#[test]
fn a_chip_sits_inside_its_bars_trailing_corner_and_clears_a_rounded_one() {
    let area = Bounds::new(point(px(0.0), px(0.0)), VIEWPORT);
    let frame =
        Bounds::new(point(px(400.0), px(0.0)), size(px(400.0), px(600.0)));
    let top = chip_bounds(frame, false, 1, area, corners(0.0, 0.0, 16.0, 0.0));
    assert_eq!(top.right(), px(796.0));
    assert_eq!(top.origin.y, px(4.0));
    assert_eq!(top.size, size(px(18.0), px(18.0)));
    // Two marks widen it; a bottom bar's sits clear of a 16-point corner.
    let bottom =
        chip_bounds(frame, true, 2, area, corners(0.0, 0.0, 16.0, 0.0));
    assert_eq!(bottom.size.width, px(32.0));
    assert!(bottom.right() < px(795.0) && bottom.bottom() < px(595.0));
}
