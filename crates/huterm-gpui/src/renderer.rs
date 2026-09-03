use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    Bounds, FontStyle, FontWeight, Hsla, LineLayout, Pixels, Point,
    StrikethroughStyle, TextRun, UnderlineStyle, Window, fill, font, point, px,
    rgba, size,
};
use huterm_protocol::{Cell, Cursor, CursorShape, Rgb, TerminalSnapshot};

pub(super) const FONT_SIZE: Pixels = px(14.0);
pub(super) const CELL_WIDTH: Pixels = px(8.4);
pub(super) const CELL_HEIGHT: Pixels = px(18.0);

#[cfg(target_os = "macos")]
pub(super) const FONT_FAMILY: &str = "Menlo";
#[cfg(target_os = "linux")]
pub(super) const FONT_FAMILY: &str = "monospace";

pub(super) struct TerminalRenderer {
    snapshot: Option<Arc<TerminalSnapshot>>,
    rows: Vec<PreparedRow>,
    layouts: GlyphLayoutCache<Arc<LineLayout>>,
    metrics: Option<GridMetrics>,
    stats: Option<RendererStats>,
}

impl TerminalRenderer {
    pub(super) fn new() -> Self {
        let stats = renderer_stats_enabled().then(RendererStats::new);
        if stats.is_some() {
            eprintln!("huterm-render stats=enabled");
        }
        Self {
            snapshot: None,
            rows: Vec::new(),
            layouts: GlyphLayoutCache::default(),
            metrics: None,
            stats,
        }
    }

    pub(super) fn records_stats(&self) -> bool {
        self.stats.is_some()
    }

    pub(super) fn prepare(
        &mut self,
        snapshot: Option<&Arc<TerminalSnapshot>>,
        window: &mut Window,
    ) {
        let started = self.stats.as_ref().map(|_| Instant::now());
        let Some(snapshot) = snapshot else {
            let changed = usize::from(!self.rows.is_empty());
            self.snapshot = None;
            self.rows.clear();
            self.record_prepare(started, changed, CacheActivity::default());
            return;
        };
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, snapshot))
        {
            self.record_prepare(started, 0, CacheActivity::default());
            return;
        }

        self.ensure_metrics(window);
        let rebuilds = rows_to_rebuild(self.snapshot.as_deref(), snapshot);
        let rebuilt_rows = rebuilds.iter().filter(|rebuild| **rebuild).count();
        let rows = usize::from(snapshot.size.rows);
        let columns = usize::from(snapshot.size.columns);
        let expected_cells = rows.saturating_mul(columns);
        if self.rows.len() != rows || snapshot.cells.len() != expected_cells {
            self.rows.clear();
            self.rows.resize_with(rows, PreparedRow::default);
        }

        let mut cache_activity = CacheActivity::default();
        if rebuilt_rows > 0 {
            self.layouts.begin_generation();
            for (row, cells) in
                snapshot.cells.chunks_exact(columns).take(rows).enumerate()
            {
                if rebuilds[row] {
                    self.rows[row] = prepare_row(
                        cells,
                        &mut self.layouts,
                        window,
                        &mut cache_activity,
                    );
                }
            }
        }

        self.snapshot = Some(Arc::clone(snapshot));
        self.record_prepare(started, rebuilt_rows, cache_activity);
    }

    pub(super) fn paint(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
    ) {
        let started = self.stats.as_ref().map(|_| Instant::now());
        let Some(snapshot) = &self.snapshot else {
            self.record_paint(started);
            return;
        };

        for (row_index, row) in self.rows.iter().enumerate() {
            for background in &row.backgrounds {
                window.paint_quad(fill(
                    cell_bounds(
                        bounds.origin,
                        background.start,
                        row_index,
                        background.columns,
                    ),
                    background.color,
                ));
            }
        }

        let metrics = self.metrics.unwrap_or_default();
        let grid_width = CELL_WIDTH * f32::from(snapshot.size.columns);
        for (row_index, row) in self.rows.iter().enumerate() {
            let row_bounds = Bounds::new(
                cell_origin(bounds.origin, 0, row_index),
                size(grid_width, CELL_HEIGHT),
            );
            window.paint_layer(row_bounds, |window| {
                paint_row(row, row_index, bounds.origin, metrics, window);
            });
        }

        if let Some(cursor) = snapshot
            .cursor
            .filter(|cursor| cursor.shape != CursorShape::Hidden)
        {
            window.paint_quad(fill(
                cursor_bounds(bounds.origin, cursor),
                rgba(0xffff_ff66),
            ));
        }
        self.record_paint(started);
    }

    fn ensure_metrics(&mut self, window: &mut Window) {
        if self.metrics.is_some() {
            return;
        }
        let runs = [text_run("M", FontVariant::default())];
        let layout = window
            .text_system()
            .layout_line("M", FONT_SIZE, &runs, None);
        self.metrics = Some(GridMetrics::from_layout(&layout));
    }

    fn record_prepare(
        &mut self,
        started: Option<Instant>,
        rebuilt_rows: usize,
        cache: CacheActivity,
    ) {
        if let (Some(stats), Some(started)) = (&mut self.stats, started) {
            stats.record_prepare(started.elapsed(), rebuilt_rows, cache);
        }
    }

    fn record_paint(&mut self, started: Option<Instant>) {
        if let (Some(stats), Some(started)) = (&mut self.stats, started) {
            stats.record_paint(started.elapsed());
        }
    }
}

#[derive(Default)]
struct PreparedRow {
    backgrounds: Vec<PreparedBackground>,
    glyphs: Vec<PreparedGlyph>,
    underlines: Vec<PreparedDecoration>,
    strikeouts: Vec<PreparedDecoration>,
}

struct PreparedBackground {
    start: u16,
    columns: u16,
    color: Hsla,
}

struct PreparedGlyph {
    column: u16,
    layout: Arc<LineLayout>,
    color: Hsla,
}

struct PreparedDecoration {
    start: u16,
    columns: u16,
    color: Hsla,
}

fn prepare_row(
    cells: &[Cell],
    layouts: &mut GlyphLayoutCache<Arc<LineLayout>>,
    window: &mut Window,
    cache_activity: &mut CacheActivity,
) -> PreparedRow {
    let mut row = PreparedRow {
        backgrounds: prepare_backgrounds(cells),
        underlines: prepare_decorations(cells, |cell| cell.style.underline),
        strikeouts: prepare_decorations(cells, |cell| cell.style.strikeout),
        ..PreparedRow::default()
    };

    for (column, cell) in cells.iter().enumerate() {
        if cell.style.wide_spacer
            || cell.style.hidden
            || cell.text.is_empty()
            || cell.text == " "
        {
            continue;
        }
        let variant = FontVariant {
            bold: cell.style.bold,
            italic: cell.style.italic,
        };
        let (layout, hit) =
            layouts.get_or_insert_with(&cell.text, variant, || {
                let runs = [text_run(&cell.text, variant)];
                window
                    .text_system()
                    .layout_line(&cell.text, FONT_SIZE, &runs, None)
            });
        cache_activity.record(hit);
        row.glyphs.push(PreparedGlyph {
            column: u16::try_from(column).unwrap_or(u16::MAX),
            layout,
            color: rgb_color(display_foreground(
                cell.foreground,
                cell.style.dim,
            )),
        });
    }
    row
}

fn prepare_backgrounds(cells: &[Cell]) -> Vec<PreparedBackground> {
    let mut backgrounds = Vec::new();
    let mut start = 0;
    while start < cells.len() {
        let background = cells[start].background;
        let mut end = start + 1;
        while end < cells.len() && cells[end].background == background {
            end += 1;
        }
        backgrounds.push(PreparedBackground {
            start: u16::try_from(start).unwrap_or(u16::MAX),
            columns: u16::try_from(end - start).unwrap_or(u16::MAX),
            color: rgb_color(background),
        });
        start = end;
    }
    backgrounds
}

fn prepare_decorations(
    cells: &[Cell],
    decorated: impl Fn(&Cell) -> bool,
) -> Vec<PreparedDecoration> {
    let mut decorations: Vec<PreparedDecoration> = Vec::new();
    for (column, cell) in cells.iter().enumerate() {
        if cell.style.wide_spacer || !decorated(cell) {
            continue;
        }
        let start = u16::try_from(column).unwrap_or(u16::MAX);
        let available = cells.len().saturating_sub(column);
        let columns = if cell.style.wide && available >= 2 {
            2
        } else {
            1
        };
        let columns = u16::try_from(columns).unwrap_or(1);
        let color =
            rgb_color(display_foreground(cell.foreground, cell.style.dim));
        if let Some(previous) = decorations.last_mut()
            && previous.start.saturating_add(previous.columns) == start
            && previous.color == color
        {
            previous.columns = previous.columns.saturating_add(columns);
        } else {
            decorations.push(PreparedDecoration {
                start,
                columns,
                color,
            });
        }
    }
    decorations
}

fn paint_row(
    row: &PreparedRow,
    row_index: usize,
    grid_origin: Point<Pixels>,
    metrics: GridMetrics,
    window: &mut Window,
) {
    for glyph in &row.glyphs {
        let origin =
            cell_origin(grid_origin, usize::from(glyph.column), row_index);
        for run in &glyph.layout.runs {
            for shaped in &run.glyphs {
                let glyph_origin = point(
                    origin.x + shaped.position.x,
                    origin.y + metrics.baseline,
                );
                let result = if shaped.is_emoji {
                    window.paint_emoji(
                        glyph_origin,
                        run.font_id,
                        shaped.id,
                        glyph.layout.font_size,
                    )
                } else {
                    window.paint_glyph(
                        glyph_origin,
                        run.font_id,
                        shaped.id,
                        glyph.layout.font_size,
                        glyph.color,
                    )
                };
                let _ = result;
            }
        }
    }

    for decoration in &row.underlines {
        let origin =
            cell_origin(grid_origin, usize::from(decoration.start), row_index);
        window.paint_underline(
            point(origin.x, origin.y + metrics.underline),
            CELL_WIDTH * f32::from(decoration.columns),
            &UnderlineStyle {
                color: Some(decoration.color),
                thickness: px(1.0),
                wavy: false,
            },
        );
    }
    for decoration in &row.strikeouts {
        let origin =
            cell_origin(grid_origin, usize::from(decoration.start), row_index);
        window.paint_strikethrough(
            point(origin.x, origin.y + metrics.strikeout),
            CELL_WIDTH * f32::from(decoration.columns),
            &StrikethroughStyle {
                color: Some(decoration.color),
                thickness: px(1.0),
            },
        );
    }
}

fn rows_to_rebuild(
    previous: Option<&TerminalSnapshot>,
    current: &TerminalSnapshot,
) -> Vec<bool> {
    let rows = usize::from(current.size.rows);
    let columns = usize::from(current.size.columns);
    let expected = rows.saturating_mul(columns);
    let Some(previous) = previous.filter(|previous| {
        previous.size == current.size
            && previous.cells.len() == expected
            && current.cells.len() == expected
    }) else {
        return vec![true; rows];
    };

    previous
        .cells
        .chunks_exact(columns)
        .zip(current.cells.chunks_exact(columns))
        .map(|(before, after)| before != after)
        .collect()
}

#[derive(Clone, Copy, Default, Eq, Hash, PartialEq)]
struct FontVariant {
    bold: bool,
    italic: bool,
}

impl FontVariant {
    fn index(self) -> usize {
        usize::from(self.bold) | (usize::from(self.italic) << 1)
    }
}

struct GlyphLayoutCache<T> {
    current: [VariantLayouts<T>; 4],
    previous: [VariantLayouts<T>; 4],
}

impl<T> Default for GlyphLayoutCache<T> {
    fn default() -> Self {
        Self {
            current: std::array::from_fn(|_| VariantLayouts::default()),
            previous: std::array::from_fn(|_| VariantLayouts::default()),
        }
    }
}

impl<T: Clone> GlyphLayoutCache<T> {
    fn begin_generation(&mut self) {
        self.previous = std::mem::take(&mut self.current);
    }

    fn get_or_insert_with(
        &mut self,
        text: &str,
        variant: FontVariant,
        create: impl FnOnce() -> T,
    ) -> (T, bool) {
        let index = variant.index();
        if let Some(value) = self.current[index].get(text) {
            return (value, true);
        }
        if let Some(value) = self.previous[index].get(text) {
            self.current[index].insert(text, value.clone());
            return (value, true);
        }
        let value = create();
        self.current[index].insert(text, value.clone());
        (value, false)
    }
}

struct VariantLayouts<T> {
    scalars: HashMap<char, T>,
    sequences: HashMap<String, T>,
}

impl<T> Default for VariantLayouts<T> {
    fn default() -> Self {
        Self {
            scalars: HashMap::new(),
            sequences: HashMap::new(),
        }
    }
}

impl<T: Clone> VariantLayouts<T> {
    fn get(&self, text: &str) -> Option<T> {
        match single_scalar(text) {
            Some(character) => self.scalars.get(&character).cloned(),
            None => self.sequences.get(text).cloned(),
        }
    }

    fn insert(&mut self, text: &str, value: T) {
        match single_scalar(text) {
            Some(character) => {
                self.scalars.insert(character, value);
            }
            None => {
                self.sequences.insert(text.to_owned(), value);
            }
        }
    }
}

fn single_scalar(text: &str) -> Option<char> {
    let mut characters = text.chars();
    let character = characters.next()?;
    characters.next().is_none().then_some(character)
}

fn text_run(text: &str, variant: FontVariant) -> TextRun {
    let mut cell_font = font(FONT_FAMILY);
    cell_font.weight = if variant.bold {
        FontWeight::BOLD
    } else {
        FontWeight::NORMAL
    };
    cell_font.style = if variant.italic {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    };
    TextRun {
        len: text.len(),
        font: cell_font,
        color: rgba(0xffff_ffff).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

#[derive(Clone, Copy, Default)]
struct GridMetrics {
    baseline: Pixels,
    underline: Pixels,
    strikeout: Pixels,
}

impl GridMetrics {
    fn from_layout(layout: &LineLayout) -> Self {
        let padding_top = (CELL_HEIGHT - layout.ascent - layout.descent) / 2.0;
        let baseline = padding_top + layout.ascent;
        Self {
            baseline,
            underline: baseline + layout.descent * 0.618,
            strikeout: (layout.ascent * 0.5 + baseline) * 0.5,
        }
    }
}

fn cell_origin(
    grid_origin: Point<Pixels>,
    column: usize,
    row: usize,
) -> Point<Pixels> {
    let column = u16::try_from(column).unwrap_or(u16::MAX);
    let row = u16::try_from(row).unwrap_or(u16::MAX);
    point(
        grid_origin.x + CELL_WIDTH * f32::from(column),
        grid_origin.y + CELL_HEIGHT * f32::from(row),
    )
}

fn cell_bounds(
    grid_origin: Point<Pixels>,
    column: u16,
    row: usize,
    columns: u16,
) -> Bounds<Pixels> {
    Bounds::new(
        cell_origin(grid_origin, usize::from(column), row),
        size(CELL_WIDTH * f32::from(columns), CELL_HEIGHT),
    )
}

fn cursor_bounds(grid_origin: Point<Pixels>, cursor: Cursor) -> Bounds<Pixels> {
    let origin = point(
        grid_origin.x + CELL_WIDTH * f32::from(cursor.column),
        grid_origin.y + CELL_HEIGHT * f32::from(cursor.row),
    );
    match cursor.shape {
        CursorShape::Block => {
            Bounds::new(origin, size(CELL_WIDTH, CELL_HEIGHT))
        }
        CursorShape::Underline => Bounds::new(
            point(origin.x, origin.y + CELL_HEIGHT - px(2.0)),
            size(CELL_WIDTH, px(2.0)),
        ),
        CursorShape::Beam => Bounds::new(origin, size(px(2.0), CELL_HEIGHT)),
        CursorShape::Hidden => Bounds::new(origin, size(px(0.0), px(0.0))),
    }
}

pub(super) fn display_foreground(foreground: Rgb, dim: bool) -> Rgb {
    if !dim {
        return foreground;
    }
    let dim = |channel: u8| {
        u8::try_from(u16::from(channel) * 2 / 3).unwrap_or(u8::MAX)
    };
    Rgb {
        red: dim(foreground.red),
        green: dim(foreground.green),
        blue: dim(foreground.blue),
    }
}

pub(super) fn rgb_color(rgb: Rgb) -> Hsla {
    gpui::rgb(
        (u32::from(rgb.red) << 16)
            | (u32::from(rgb.green) << 8)
            | u32::from(rgb.blue),
    )
    .into()
}

#[derive(Clone, Copy, Default)]
struct CacheActivity {
    hits: usize,
    misses: usize,
}

impl CacheActivity {
    fn record(&mut self, hit: bool) {
        if hit {
            self.hits += 1;
        } else {
            self.misses += 1;
        }
    }
}

struct RendererStats {
    interval_started: Instant,
    prepare_calls: u64,
    paint_calls: u64,
    prepare_total: Duration,
    prepare_max: Duration,
    paint_total: Duration,
    paint_max: Duration,
    rebuilt_rows: usize,
    cache_hits: usize,
    cache_misses: usize,
}

impl RendererStats {
    fn new() -> Self {
        Self {
            interval_started: Instant::now(),
            prepare_calls: 0,
            paint_calls: 0,
            prepare_total: Duration::ZERO,
            prepare_max: Duration::ZERO,
            paint_total: Duration::ZERO,
            paint_max: Duration::ZERO,
            rebuilt_rows: 0,
            cache_hits: 0,
            cache_misses: 0,
        }
    }

    fn record_prepare(
        &mut self,
        duration: Duration,
        rebuilt_rows: usize,
        cache: CacheActivity,
    ) {
        if self.prepare_calls == 0 {
            eprintln!(
                "huterm-render first_prepare_us={} rows={} cache_hits={} cache_misses={}",
                duration.as_micros(),
                rebuilt_rows,
                cache.hits,
                cache.misses,
            );
        }
        self.prepare_calls += 1;
        self.prepare_total += duration;
        self.prepare_max = self.prepare_max.max(duration);
        self.rebuilt_rows += rebuilt_rows;
        self.cache_hits += cache.hits;
        self.cache_misses += cache.misses;
    }

    fn record_paint(&mut self, duration: Duration) {
        if self.paint_calls == 0 {
            eprintln!("huterm-render first_paint_us={}", duration.as_micros());
        }
        self.paint_calls += 1;
        self.paint_total += duration;
        self.paint_max = self.paint_max.max(duration);
        if self.interval_started.elapsed() < Duration::from_secs(1) {
            return;
        }
        let prepare_average =
            average_micros(self.prepare_total, self.prepare_calls);
        let paint_average = average_micros(self.paint_total, self.paint_calls);
        eprintln!(
            "huterm-render frames={} prepare_us_avg={} prepare_us_max={} paint_us_avg={} paint_us_max={} rows={} cache_hits={} cache_misses={}",
            self.paint_calls,
            prepare_average,
            self.prepare_max.as_micros(),
            paint_average,
            self.paint_max.as_micros(),
            self.rebuilt_rows,
            self.cache_hits,
            self.cache_misses,
        );
        *self = Self::new();
    }
}

fn average_micros(total: Duration, count: u64) -> u128 {
    total.as_micros() / u128::from(count.max(1))
}

fn renderer_stats_enabled() -> bool {
    std::env::var("HUTERM_RENDER_STATS")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

#[cfg(test)]
mod tests {
    use huterm_protocol::{CellStyle, GridSize, TerminalId, TerminalModes};

    use super::*;

    #[test]
    fn cache_reuses_layouts_without_calling_factory() {
        let mut cache = GlyphLayoutCache::default();
        let mut calls = 0;
        let variant = FontVariant::default();

        let first = cache.get_or_insert_with("A", variant, || {
            calls += 1;
            42
        });
        let second = cache.get_or_insert_with("A", variant, || {
            calls += 1;
            99
        });

        assert_eq!(first, (42, false));
        assert_eq!(second, (42, true));
        assert_eq!(calls, 1);
    }

    #[test]
    fn cache_evicts_layouts_unused_for_two_generations() {
        let mut cache = GlyphLayoutCache::default();
        let variant = FontVariant::default();
        let _ = cache.get_or_insert_with("A", variant, || 1);
        cache.begin_generation();
        let _ = cache.get_or_insert_with("B", variant, || 2);
        cache.begin_generation();

        let (value, hit) = cache.get_or_insert_with("A", variant, || 3);

        assert_eq!(value, 3);
        assert!(!hit);
    }

    #[test]
    fn cache_promotes_layouts_from_previous_generation() {
        let mut cache = GlyphLayoutCache::default();
        let variant = FontVariant::default();
        let _ = cache.get_or_insert_with("A", variant, || 1);
        cache.begin_generation();
        assert_eq!(cache.get_or_insert_with("A", variant, || 2), (1, true));
        cache.begin_generation();

        assert_eq!(cache.get_or_insert_with("A", variant, || 3), (1, true));
    }

    #[test]
    fn row_diff_rebuilds_only_changed_content() {
        let previous = snapshot(2, 2, &["A", "B", "C", "D"]);
        let current = snapshot(2, 2, &["A", "B", "X", "D"]);

        assert_eq!(
            rows_to_rebuild(Some(&previous), &current),
            vec![false, true]
        );
    }

    #[test]
    fn row_diff_rebuilds_every_row_after_resize() {
        let previous = snapshot(2, 2, &["A", "B", "C", "D"]);
        let current = snapshot(3, 2, &["A", "B", "C", "D", "E", "F"]);

        assert_eq!(
            rows_to_rebuild(Some(&previous), &current),
            vec![true, true]
        );
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
            rows_to_rebuild(Some(&previous), &current),
            vec![false, false]
        );
    }

    #[test]
    fn decorations_merge_across_a_wide_cell_and_its_spacer() {
        let mut cells = snapshot(3, 1, &["界", " ", "A"]).cells;
        cells[0].style.wide = true;
        cells[0].style.underline = true;
        cells[1].style.wide_spacer = true;
        cells[2].style.underline = true;

        let decorations =
            prepare_decorations(&cells, |cell| cell.style.underline);

        assert_eq!(decorations.len(), 1);
        assert_eq!(decorations[0].start, 0);
        assert_eq!(decorations[0].columns, 3);
    }

    fn snapshot(
        columns: u16,
        rows: u16,
        contents: &[&str],
    ) -> TerminalSnapshot {
        TerminalSnapshot {
            terminal_id: TerminalId::new(1),
            generation: 1,
            size: GridSize::clamped(columns, rows),
            cells: contents
                .iter()
                .map(|text| Cell {
                    text: (*text).to_owned(),
                    foreground: Rgb {
                        red: 255,
                        green: 255,
                        blue: 255,
                    },
                    background: Rgb {
                        red: 0,
                        green: 0,
                        blue: 0,
                    },
                    style: CellStyle::default(),
                })
                .collect(),
            cursor: None,
            modes: TerminalModes::default(),
            history_size: 0,
        }
    }
}
