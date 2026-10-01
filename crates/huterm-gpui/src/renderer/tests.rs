use huterm_protocol::{
    CellStyle, GridSize, TerminalId, TerminalModes, Viewport,
};

use super::*;

#[test]
fn reconfigure_invalidates_rows_but_preserves_snapshot_data() {
    let metrics = GridMetrics::from_measurements(
        px(14.0),
        px(8.0),
        px(12.0),
        px(-3.0),
        1.0,
    );
    let original = Arc::new(snapshot(1, 1, &["A"]));
    let mut renderer =
        TerminalRenderer::new("Menlo".into(), Theme::default(), metrics);
    renderer.snapshot = Some(Arc::clone(&original));
    renderer.rows.push(PreparedRow::default());
    renderer.reconfigure("Menlo".into(), Theme::default(), metrics);
    assert!(renderer.snapshot.is_some());
    assert_eq!(renderer.rows.len(), 1);
    let mut theme = Theme::default();
    theme.background = theme.foreground;
    renderer.reconfigure("Menlo".into(), theme.clone(), metrics);
    assert!(renderer.snapshot.is_none());
    assert!(renderer.rows.is_empty());
    assert_eq!(original.rows[0].cells[0].text, "A");
    renderer.snapshot = Some(Arc::clone(&original));
    let larger = GridMetrics::from_measurements(
        px(18.0),
        px(10.0),
        px(15.0),
        px(-4.0),
        1.0,
    );
    renderer.reconfigure("Menlo".into(), theme, larger);
    assert!(renderer.snapshot.is_none());
    assert_eq!(renderer.metrics.cell_height, px(19.0));
}

#[test]
fn selection_foreground_only_affects_selected_visible_cells() {
    let mut snapshot = snapshot(2, 1, &["A", "B"]);
    snapshot.viewport.bottom_offset = 10;
    let range = BufferRange::ordered(
        huterm_protocol::BufferPoint {
            rows_from_live_bottom: 10,
            column: 0,
        },
        huterm_protocol::BufferPoint {
            rows_from_live_bottom: 10,
            column: 0,
        },
    );
    let mut theme = Theme::default();
    assert_eq!(
        selected_foreground(&snapshot, Some(range), &theme, 0, 0),
        None
    );
    theme.selection_foreground = Some(theme.background);
    assert_eq!(
        selected_foreground(&snapshot, Some(range), &theme, 0, 0),
        Some(theme.background)
    );
    assert_eq!(
        selected_foreground(&snapshot, Some(range), &theme, 0, 1),
        None
    );
    assert_eq!(selected_foreground(&snapshot, None, &theme, 0, 0), None);
    snapshot.viewport.bottom_offset = 11;
    assert_eq!(
        selected_foreground(&snapshot, Some(range), &theme, 0, 0),
        None
    );
}

#[test]
fn font_changes_evict_layouts_while_palette_changes_reuse_them() {
    let metrics = GridMetrics::from_measurements(
        px(14.0),
        px(8.0),
        px(12.0),
        px(-3.0),
        1.0,
    );
    let mut renderer =
        TerminalRenderer::new("Menlo".into(), Theme::default(), metrics);
    let variant = FontVariant::default();
    renderer.layouts.get_or_insert_with(
        GlyphText::Scalar('A'),
        variant,
        || Arc::new(LineLayout::default()),
    );
    let mut theme = Theme::default();
    theme.background = theme.foreground;
    renderer.reconfigure("Menlo".into(), theme.clone(), metrics);
    assert!(
        renderer
            .layouts
            .get_or_insert_with(GlyphText::Scalar('A'), variant, || panic!(
                "palette change must reuse glyph layout"
            ))
            .1
    );
    renderer.reconfigure("Monaco".into(), theme, metrics);
    assert!(
        !renderer
            .layouts
            .get_or_insert_with(GlyphText::Scalar('A'), variant, || {
                Arc::new(LineLayout::default())
            })
            .1
    );
}

#[test]
fn block_cursor_overlay_preserves_underlying_glyph_visibility() {
    let color = Rgb {
        red: 255,
        green: 255,
        blue: 255,
    };
    assert!(
        (cursor_fill(color, CursorShape::Block).a - 0.4).abs() < f32::EPSILON
    );
    for shape in [CursorShape::Beam, CursorShape::Underline] {
        assert!((cursor_fill(color, shape).a - 1.0).abs() < f32::EPSILON);
    }
}

#[test]
fn menlo_12_matches_reference_terminal_grid_width() {
    // Measured with CoreText for Menlo Nerd Font Mono Regular at 12pt.
    let metrics = GridMetrics::from_measurements(
        px(12.0),
        px(7.224_609_4),
        px(11.138_672),
        px(-2.830_078_1),
        2.0,
    );
    assert_eq!(metrics.cell_width * 120.0, px(840.0));
    assert_eq!(metrics.cell_height * 40.0, px(560.0));
}

#[test]
fn display_scale_round_trips_preserve_unrounded_font_measurements() {
    let original = GridMetrics::from_measurements(
        px(14.0),
        px(8.429),
        px(12.995),
        px(-3.302),
        1.0,
    );
    let retina = original.at_scale(2.0);
    assert_eq!(
        size(original.cell_width, original.cell_height),
        size(px(8.0), px(16.0))
    );
    assert_eq!(
        size(retina.cell_width, retina.cell_height),
        size(px(8.5), px(16.5))
    );
    assert_eq!(retina.baseline * 2.0, (retina.baseline * 2.0).round());
    assert_eq!(retina.at_scale(1.0), original);
    assert_eq!(retina.at_scale(1.5).at_scale(2.0), retina);
}

#[test]
fn rounded_grid_shares_cell_edges_and_centers_glyph_advance() {
    let metrics = GridMetrics::from_measurements(
        px(14.0),
        px(8.429),
        px(12.995),
        px(3.302),
        2.0,
    );
    let origin = point(px(4.0), px(36.0));
    let next = cell_origin(origin, 120, 40, metrics);
    assert_eq!(next, origin + point(px(1020.0), px(660.0)));
    assert!(
        (f32::from(
            metrics.glyph_offset_x * 2.0 + metrics.advance - metrics.cell_width
        ))
        .abs()
            < 0.0001
    );
    assert_eq!(
        metrics,
        GridMetrics::from_measurements(
            px(14.0),
            px(8.429),
            px(12.995),
            px(-3.302),
            2.0,
        )
    );
}

#[test]
fn tiny_font_metrics_keep_at_least_one_device_pixel() {
    let metrics = GridMetrics::from_measurements(
        px(0.1),
        px(0.01),
        px(0.02),
        px(-0.01),
        2.0,
    );
    assert_eq!(
        size(metrics.cell_width, metrics.cell_height),
        size(px(0.5), px(0.5))
    );
}

#[test]
fn grid_metrics_round_height_with_signed_descent() {
    let metrics = GridMetrics::from_measurements(
        px(14.0),
        px(8.429),
        px(12.995),
        px(-3.302),
        1.0,
    );

    assert_eq!(metrics.cell_height, px(16.0));
}

#[test]
fn grid_metrics_should_order_vertical_positions_with_signed_descent() {
    let metrics = GridMetrics::from_measurements(
        px(14.0),
        px(8.429),
        px(12.995),
        px(-3.302),
        1.0,
    );

    assert!(
        metrics.strikeout < metrics.baseline
            && metrics.baseline < metrics.underline
            && metrics.underline < metrics.cell_height
    );
}

#[test]
fn cache_reuses_layouts_without_calling_factory() {
    let mut cache = GlyphLayoutCache::default();
    let mut calls = 0;
    let variant = FontVariant::default();

    let first =
        cache.get_or_insert_with(GlyphText::Scalar('A'), variant, || {
            calls += 1;
            42
        });
    let second =
        cache.get_or_insert_with(GlyphText::Scalar('A'), variant, || {
            calls += 1;
            99
        });

    assert_eq!(first, (42, false));
    assert_eq!(second, (42, true));
    assert_eq!(calls, 1);
}

#[test]
fn cache_evicts_layouts_unused_for_two_generations() {
    let mut cache = GlyphLayoutCache::with_capacity(1);
    let variant = FontVariant::default();
    let _ = cache.get_or_insert_with(GlyphText::Scalar('A'), variant, || 1);
    cache.begin_generation();
    let _ = cache.get_or_insert_with(GlyphText::Scalar('B'), variant, || 2);
    cache.begin_generation();

    let (value, hit) =
        cache.get_or_insert_with(GlyphText::Scalar('A'), variant, || 3);

    assert_eq!(value, 3);
    assert!(!hit);
}

#[test]
fn cache_promotes_layouts_from_previous_generation() {
    let mut cache = GlyphLayoutCache::with_capacity(1);
    let variant = FontVariant::default();
    let _ = cache.get_or_insert_with(GlyphText::Scalar('A'), variant, || 1);
    cache.begin_generation();
    assert_eq!(
        cache.get_or_insert_with(GlyphText::Scalar('A'), variant, || 2),
        (1, true)
    );
    cache.begin_generation();

    assert_eq!(
        cache.get_or_insert_with(GlyphText::Scalar('A'), variant, || 3),
        (1, true)
    );
}

#[test]
fn backgrounds_merge_runs_and_omit_the_theme_background() {
    let theme = Theme::default();
    let cell = |background| Cell {
        text: " ".into(),
        foreground: CellColor::DefaultForeground,
        background,
        style: CellStyle::default(),
    };
    let red = CellColor::Rgb(Rgb {
        red: 255,
        green: 0,
        blue: 0,
    });
    let cells = [
        cell(CellColor::DefaultBackground),
        cell(red),
        cell(red),
        // An explicit color equal to the theme background needs no quad.
        cell(CellColor::Rgb(theme.background)),
        cell(CellColor::Indexed(4)),
    ];

    let backgrounds = prepare_backgrounds(&cells, &theme);

    let spans: Vec<_> = backgrounds
        .iter()
        .map(|background| (background.start, background.columns))
        .collect();
    assert_eq!(spans, [(1, 2), (4, 1)]);
    assert_eq!(backgrounds[1].color, rgb_color(theme.indexed(4)));
}

fn key(text: &str) -> GlyphText<'_> {
    let mut characters = text.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None) => GlyphText::Scalar(character),
        _ => GlyphText::Sequence(text),
    }
}

#[test]
fn cache_keys_ascii_scalars_and_sequences_separately() {
    let mut cache = GlyphLayoutCache::with_capacity(4);
    let variant = FontVariant::default();
    let bold = FontVariant {
        bold: true,
        italic: false,
    };
    for (value, text) in ["e", "é", "e\u{301}"].into_iter().enumerate() {
        assert_eq!(
            cache.get_or_insert_with(key(text), variant, || value),
            (value, false)
        );
    }
    assert_eq!(
        cache.get_or_insert_with(GlyphText::Scalar('e'), bold, || 9),
        (9, false)
    );
    // Every storage path survives promotion out of the older generation.
    cache.begin_generation();
    assert_eq!(cache.current_len, 0);
    for (value, text) in ["e", "é", "e\u{301}"].into_iter().enumerate() {
        assert_eq!(
            cache.get_or_insert_with(key(text), variant, || 7),
            (value, true)
        );
    }
}

#[test]
fn cache_keeps_layouts_until_a_generation_fills() {
    let mut cache = GlyphLayoutCache::with_capacity(2);
    let variant = FontVariant::default();
    let _ = cache.get_or_insert_with(GlyphText::Scalar('A'), variant, || 1);
    // Small updates must not evict text used by rows they left alone.
    for _ in 0..3 {
        cache.begin_generation();
        let _ = cache.get_or_insert_with(GlyphText::Scalar('B'), variant, || 2);
    }
    assert_eq!(
        cache.get_or_insert_with(GlyphText::Scalar('A'), variant, || 3),
        (1, true)
    );

    // Two full generations later an unused layout is gone.
    for text in ["C", "D", "E", "F"] {
        cache.begin_generation();
        let _ = cache.get_or_insert_with(key(text), variant, || 4);
    }
    cache.begin_generation();
    assert_eq!(
        cache.get_or_insert_with(GlyphText::Scalar('A'), variant, || 5),
        (5, false)
    );
}

#[test]
fn row_diff_rebuilds_only_changed_content() {
    let previous = snapshot(2, 2, &["A", "B", "C", "D"]);
    let current = snapshot(2, 2, &["A", "B", "X", "D"]);

    assert_eq!(row_sources(Some(&previous), &current), vec![Some(0), None]);
}

#[test]
fn row_diff_rebuilds_every_row_after_resize() {
    let previous = snapshot(2, 2, &["A", "B", "C", "D"]);
    let current = snapshot(3, 2, &["A", "B", "C", "D", "E", "F"]);

    assert_eq!(row_sources(Some(&previous), &current), vec![None, None]);
}

#[test]
fn row_diff_ignores_cursor_only_changes() {
    let previous = snapshot(2, 2, &["A", "B", "C", "D"]);
    let mut current = previous.clone();
    current.cursor = Some(Cursor {
        row: 1,
        column: 1,
        shape: CursorShape::Beam,
    });

    assert_eq!(
        row_sources(Some(&previous), &current),
        vec![Some(0), Some(1)]
    );
}

#[test]
fn row_diff_reuses_shifted_rows_during_static_scrolling() {
    let mut previous = snapshot(1, 3, &["B", "C", "D"]);
    previous.history_size = 10;
    let mut current = snapshot(1, 3, &["A", "B", "C"]);
    current.history_size = 10;
    current.viewport.bottom_offset = 1;

    assert_eq!(
        row_sources(Some(&previous), &current),
        vec![None, Some(0), Some(1)]
    );
}

#[test]
fn row_diff_reuses_every_row_when_pinned_history_grows() {
    let mut previous = snapshot(1, 3, &["A", "B", "C"]);
    previous.history_size = 100;
    previous.viewport.bottom_offset = 10;
    let mut current = previous.clone();
    current.history_size = 101;
    current.viewport.bottom_offset = 11;
    current.generation += 1;

    assert_eq!(
        row_sources(Some(&previous), &current),
        vec![Some(0), Some(1), Some(2)]
    );
}

#[test]
fn row_sources_follow_reused_arcs_when_history_cannot_report_a_shift() {
    let previous = snapshot(1, 3, &[" ", "A", "B"]);
    let mut current = snapshot(1, 3, &[" ", " ", "C"]);
    // A full scrollback holds history constant while content moves up;
    // the runtime reuses the blank row twice and `A`, `B` once each.
    current.rows = vec![
        Arc::clone(&previous.rows[0]),
        Arc::clone(&previous.rows[2]),
        Arc::clone(&current.rows[2]),
    ];
    assert_eq!(current.history_size, previous.history_size);
    assert_eq!(
        row_sources(Some(&previous), &current),
        vec![Some(0), Some(2), None]
    );
    current.rows[1] = Arc::clone(&previous.rows[0]);
    assert_eq!(
        row_sources(Some(&previous), &current),
        vec![Some(0), Some(0), None]
    );
}

#[test]
fn realign_moves_last_uses_and_clones_shared_sources() {
    let previous = vec!["blank".to_owned(), "A".to_owned(), "B".to_owned()];
    assert_eq!(
        realign(previous.clone(), &[Some(1), Some(2), None]),
        ["A", "B", ""]
    );
    assert_eq!(
        realign(previous.clone(), &[Some(0), Some(0), Some(2)]),
        ["blank", "blank", "B"]
    );
    assert_eq!(realign(previous, &[None, Some(7)]), ["", ""]);
}

#[test]
fn decorations_merge_across_a_wide_cell_and_its_spacer() {
    let mut cells = snapshot(3, 1, &["界", " ", "A"]).rows[0].cells.clone();
    cells[0].style.wide = true;
    cells[0].style.underline = true;
    cells[1].style.wide_spacer = true;
    cells[2].style.underline = true;

    let decorations = prepare_decorations(&cells, &Theme::default(), |cell| {
        cell.style.underline
    });

    assert_eq!(decorations.len(), 1);
    assert_eq!(decorations[0].start, 0);
    assert_eq!(decorations[0].columns, 3);
}

#[test]
fn selection_expands_over_both_halves_of_a_wide_character() {
    let mut cells = snapshot(2, 1, &["界", " "]).rows[0].cells.clone();
    cells[0].style.wide = true;
    cells[1].style.wide_spacer = true;

    for selected_column in 0..=1 {
        let point = huterm_protocol::BufferPoint {
            rows_from_live_bottom: 0,
            column: selected_column,
        };
        let selection = BufferRange::ordered(point, point);

        assert!(selection_covers_column(selection, 0, 0, &cells));
        assert!(selection_covers_column(selection, 0, 1, &cells));
    }
}

#[test]
fn scroll_benchmark_alone_enables_renderer_timing() {
    assert!(!timing_enabled(false, false));
    assert!(timing_enabled(true, false));
    assert!(timing_enabled(false, true));
}

#[test]
fn stats_mode_selects_continuous_frames_only_for_the_boolean_form() {
    assert_eq!(stats_mode_from("1"), Some(StatsMode::ContinuousFrames));
    assert_eq!(stats_mode_from("TRUE"), Some(StatsMode::ContinuousFrames));
    assert_eq!(stats_mode_from("events"), Some(StatsMode::Events));
    assert_eq!(stats_mode_from("0"), None);
    assert_eq!(stats_mode_from(""), None);
}

#[test]
fn next_scroll_request_does_not_replace_snapshot_awaiting_paint() {
    let mut benchmark = ScrollBenchmarkStats::default();
    benchmark.begin(1, 10, Some(Instant::now()));
    benchmark.complete_snapshot(
        Duration::from_micros(10),
        10,
        10,
        Duration::from_micros(2),
    );

    benchmark.begin(2, 11, Some(Instant::now()));
    benchmark.complete_prepare(Duration::from_micros(20), 1, 32);

    assert_eq!(
        benchmark.in_flight.as_ref().map(|sample| sample.sequence),
        Some(2)
    );
    assert!(
        benchmark
            .ready_to_paint
            .as_ref()
            .is_some_and(|sample| sample.sequence == 1)
    );

    benchmark.complete_paint(Duration::from_micros(30));

    assert!(benchmark.ready_to_paint.is_none());
    assert_eq!(
        benchmark.in_flight.as_ref().map(|sample| sample.sequence),
        Some(2)
    );
}

#[test]
fn semantic_colors_and_dim_are_resolved_only_for_paint() {
    let theme = Theme::default();
    assert_eq!(
        resolve_color(CellColor::DefaultForeground, &theme),
        theme.foreground
    );
    assert_eq!(
        resolve_color(CellColor::DefaultBackground, &theme),
        theme.background
    );
    assert_eq!(resolve_color(CellColor::Cursor, &theme), theme.cursor);
    assert_eq!(resolve_color(CellColor::Indexed(9), &theme), theme.ansi[9]);
    let explicit = Rgb {
        red: 30,
        green: 60,
        blue: 90,
    };
    assert_eq!(resolve_color(CellColor::Rgb(explicit), &theme), explicit);
    assert_eq!(
        display_foreground(explicit, true),
        Rgb {
            red: 20,
            green: 40,
            blue: 60,
        }
    );
}

#[test]
fn selection_range_includes_ordered_multiline_endpoints() {
    let range = BufferRange::ordered(
        huterm_protocol::BufferPoint {
            rows_from_live_bottom: 4,
            column: 3,
        },
        huterm_protocol::BufferPoint {
            rows_from_live_bottom: 2,
            column: 1,
        },
    );
    assert!(range_contains(range, range.start));
    assert!(range_contains(
        range,
        huterm_protocol::BufferPoint {
            rows_from_live_bottom: 3,
            column: 0,
        }
    ));
    assert!(range_contains(range, range.end));
    assert!(!range_contains(
        range,
        huterm_protocol::BufferPoint {
            rows_from_live_bottom: 4,
            column: 2,
        }
    ));
}

fn snapshot(columns: u16, rows: u16, contents: &[&str]) -> TerminalSnapshot {
    TerminalSnapshot {
        terminal_id: TerminalId::new(1),
        generation: 1,
        size: GridSize::clamped(columns, rows),
        rows: contents
            .chunks(usize::from(columns))
            .map(|contents| {
                Arc::new(huterm_protocol::TerminalRow {
                    cells: contents
                        .iter()
                        .map(|text| Cell {
                            text: (*text).into(),
                            foreground: CellColor::Rgb(Rgb {
                                red: 255,
                                green: 255,
                                blue: 255,
                            }),
                            background: CellColor::Rgb(Rgb {
                                red: 0,
                                green: 0,
                                blue: 0,
                            }),
                            style: CellStyle::default(),
                        })
                        .collect(),
                })
            })
            .collect(),
        cursor: None,
        modes: TerminalModes::default(),
        viewport: Viewport::default(),
        history_size: 0,
        cursor_color: None,
    }
}
