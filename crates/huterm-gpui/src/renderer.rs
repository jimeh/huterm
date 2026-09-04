use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    Bounds, FontStyle, FontWeight, Hsla, LineLayout, Pixels, Point,
    StrikethroughStyle, TextRun, UnderlineStyle, Window, fill, font, point, px,
    rgba, size,
};
use huterm_protocol::{
    BufferRange, Cell, CellColor, Cursor, CursorShape, Rgb, TerminalSnapshot,
};

use crate::config::Theme;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct GridMetrics {
    pub(super) cell_width: Pixels,
    pub(super) cell_height: Pixels,
    pub(super) font_size: Pixels,
    baseline: Pixels,
    underline: Pixels,
    strikeout: Pixels,
}

impl GridMetrics {
    pub(super) fn from_measurements(
        font_size: Pixels,
        cell_width: Pixels,
        ascent: Pixels,
        descent: Pixels,
    ) -> Self {
        let cell_height = (ascent + descent).ceil().max(px(1.0));
        let cell_width = cell_width.ceil().max(px(1.0));
        let baseline = ascent;
        Self {
            cell_width,
            cell_height,
            font_size,
            baseline,
            underline: baseline + descent * 0.618,
            strikeout: ascent * 0.5,
        }
    }
}

pub(super) struct TerminalRenderer {
    snapshot: Option<Arc<TerminalSnapshot>>,
    rows: Vec<PreparedRow>,
    layouts: GlyphLayoutCache<Arc<LineLayout>>,
    metrics: GridMetrics,
    font_family: String,
    theme: Theme,
    selection: Option<BufferRange>,
    stats: Option<RendererStats>,
    scroll_benchmark: Option<ScrollBenchmarkStats>,
}

impl TerminalRenderer {
    pub(super) fn new(
        font_family: String,
        theme: Theme,
        metrics: GridMetrics,
    ) -> Self {
        let stats = renderer_stats_enabled().then(RendererStats::new);
        let scroll_benchmark =
            scroll_benchmark_enabled().then(ScrollBenchmarkStats::default);
        if stats.is_some() {
            eprintln!("huterm-render stats=enabled");
        }
        Self {
            snapshot: None,
            rows: Vec::new(),
            layouts: GlyphLayoutCache::default(),
            metrics,
            font_family,
            theme,
            selection: None,
            stats,
            scroll_benchmark,
        }
    }

    pub(super) fn records_stats(&self) -> bool {
        timing_enabled(self.stats.is_some(), self.scroll_benchmark.is_some())
    }

    pub(super) fn begin_scroll_sample(
        &mut self,
        sequence: u64,
        requested_offset: usize,
        injected_at: Option<Instant>,
    ) {
        if let Some(benchmark) = &mut self.scroll_benchmark {
            benchmark.begin(sequence, requested_offset, injected_at);
        }
    }

    pub(super) fn complete_scroll_snapshot(
        &mut self,
        duration: Duration,
        returned_offset: usize,
        wakeup_delay: Duration,
    ) {
        if let Some(benchmark) = &mut self.scroll_benchmark {
            benchmark.complete_snapshot(
                duration,
                returned_offset,
                wakeup_delay,
            );
        }
    }

    pub(super) fn set_selection(&mut self, selection: Option<BufferRange>) {
        self.selection = selection;
    }

    pub(super) fn prepare(
        &mut self,
        snapshot: Option<&Arc<TerminalSnapshot>>,
        window: &mut Window,
    ) {
        let started = self.records_stats().then(Instant::now);
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

        self.align_rows(snapshot);
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
                        &self.font_family,
                        self.metrics.font_size,
                        &self.theme,
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
        let started = self.records_stats().then(Instant::now);
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
                        self.metrics,
                    ),
                    background.color,
                ));
            }
        }

        if let Some(selection) = self.selection {
            paint_selection(
                snapshot,
                selection,
                bounds.origin,
                self.metrics,
                &self.theme,
                window,
            );
        }

        let metrics = self.metrics;
        let grid_width = metrics.cell_width * f32::from(snapshot.size.columns);
        for (row_index, row) in self.rows.iter().enumerate() {
            let row_bounds = Bounds::new(
                cell_origin(bounds.origin, 0, row_index, metrics),
                size(grid_width, metrics.cell_height),
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
                cursor_bounds(bounds.origin, cursor, metrics),
                rgb_color(snapshot.cursor_color.unwrap_or(self.theme.cursor)),
            ));
        }
        self.record_paint(started);
    }

    fn align_rows(&mut self, current: &TerminalSnapshot) {
        let Some(previous) = self.snapshot.as_deref() else {
            return;
        };
        if previous.size != current.size
            || self.rows.len() != usize::from(current.size.rows)
        {
            return;
        }
        let offset_delta = current.viewport.bottom_offset as i128
            - previous.viewport.bottom_offset as i128;
        let history_delta =
            current.history_size as i128 - previous.history_size as i128;
        let shift = offset_delta - history_delta;
        let rows = i128::from(current.size.rows);
        if shift == 0 || shift.unsigned_abs() >= rows.unsigned_abs() {
            return;
        }
        let mut old: Vec<Option<PreparedRow>> = std::mem::take(&mut self.rows)
            .into_iter()
            .map(Some)
            .collect();
        self.rows = (0..usize::from(current.size.rows))
            .map(|new_row| {
                let old_row = new_row as i128 - shift;
                if (0..rows).contains(&old_row) {
                    old[usize::try_from(old_row).unwrap_or_default()]
                        .take()
                        .unwrap_or_default()
                } else {
                    PreparedRow::default()
                }
            })
            .collect();
    }

    fn record_prepare(
        &mut self,
        started: Option<Instant>,
        rebuilt_rows: usize,
        cache: CacheActivity,
    ) {
        let duration = started.map(|started| started.elapsed());
        if let (Some(stats), Some(duration)) = (&mut self.stats, duration) {
            stats.record_prepare(duration, rebuilt_rows, cache);
        }
        if let (Some(benchmark), Some(duration)) =
            (&mut self.scroll_benchmark, duration)
        {
            benchmark.complete_prepare(duration, rebuilt_rows, self.rows.len());
        }
    }

    fn record_paint(&mut self, started: Option<Instant>) {
        let duration = started.map(|started| started.elapsed());
        if let (Some(stats), Some(duration)) = (&mut self.stats, duration) {
            stats.record_paint(duration);
        }
        if let (Some(benchmark), Some(duration)) =
            (&mut self.scroll_benchmark, duration)
        {
            benchmark.complete_paint(duration);
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
    font_family: &str,
    font_size: Pixels,
    theme: &Theme,
) -> PreparedRow {
    let mut row = PreparedRow {
        backgrounds: prepare_backgrounds(cells, theme),
        underlines: prepare_decorations(cells, theme, |cell| {
            cell.style.underline
        }),
        strikeouts: prepare_decorations(cells, theme, |cell| {
            cell.style.strikeout
        }),
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
                let runs = [text_run(&cell.text, variant, font_family)];
                window
                    .text_system()
                    .layout_line(&cell.text, font_size, &runs, None)
            });
        cache_activity.record(hit);
        row.glyphs.push(PreparedGlyph {
            column: u16::try_from(column).unwrap_or(u16::MAX),
            layout,
            color: rgb_color(display_foreground(
                resolve_color(cell.foreground, theme),
                cell.style.dim,
            )),
        });
    }
    row
}

fn prepare_backgrounds(
    cells: &[Cell],
    theme: &Theme,
) -> Vec<PreparedBackground> {
    let mut backgrounds = Vec::new();
    let mut start = 0;
    while start < cells.len() {
        let background = resolve_color(cells[start].background, theme);
        let mut end = start + 1;
        while end < cells.len()
            && resolve_color(cells[end].background, theme) == background
        {
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
    theme: &Theme,
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
        let color = rgb_color(display_foreground(
            resolve_color(cell.foreground, theme),
            cell.style.dim,
        ));
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
        let origin = cell_origin(
            grid_origin,
            usize::from(glyph.column),
            row_index,
            metrics,
        );
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
        let origin = cell_origin(
            grid_origin,
            usize::from(decoration.start),
            row_index,
            metrics,
        );
        window.paint_underline(
            point(origin.x, origin.y + metrics.underline),
            metrics.cell_width * f32::from(decoration.columns),
            &UnderlineStyle {
                color: Some(decoration.color),
                thickness: px(1.0),
                wavy: false,
            },
        );
    }
    for decoration in &row.strikeouts {
        let origin = cell_origin(
            grid_origin,
            usize::from(decoration.start),
            row_index,
            metrics,
        );
        window.paint_strikethrough(
            point(origin.x, origin.y + metrics.strikeout),
            metrics.cell_width * f32::from(decoration.columns),
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

    let offset_delta = current.viewport.bottom_offset as i128
        - previous.viewport.bottom_offset as i128;
    let history_delta =
        current.history_size as i128 - previous.history_size as i128;
    let shift = offset_delta - history_delta;
    let previous_rows: Vec<&[Cell]> =
        previous.cells.chunks_exact(columns).collect();
    current
        .cells
        .chunks_exact(columns)
        .enumerate()
        .map(|(new_row, after)| {
            let old_row = new_row as i128 - shift;
            old_row < 0
                || old_row >= rows as i128
                || previous_rows[usize::try_from(old_row).unwrap_or_default()]
                    != after
        })
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

fn text_run(text: &str, variant: FontVariant, font_family: &str) -> TextRun {
    let mut cell_font = font(font_family.to_owned());
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

fn cell_origin(
    grid_origin: Point<Pixels>,
    column: usize,
    row: usize,
    metrics: GridMetrics,
) -> Point<Pixels> {
    let column = u16::try_from(column).unwrap_or(u16::MAX);
    let row = u16::try_from(row).unwrap_or(u16::MAX);
    point(
        grid_origin.x + metrics.cell_width * f32::from(column),
        grid_origin.y + metrics.cell_height * f32::from(row),
    )
}

fn cell_bounds(
    grid_origin: Point<Pixels>,
    column: u16,
    row: usize,
    columns: u16,
    metrics: GridMetrics,
) -> Bounds<Pixels> {
    Bounds::new(
        cell_origin(grid_origin, usize::from(column), row, metrics),
        size(metrics.cell_width * f32::from(columns), metrics.cell_height),
    )
}

fn cursor_bounds(
    grid_origin: Point<Pixels>,
    cursor: Cursor,
    metrics: GridMetrics,
) -> Bounds<Pixels> {
    let origin = point(
        grid_origin.x + metrics.cell_width * f32::from(cursor.column),
        grid_origin.y + metrics.cell_height * f32::from(cursor.row),
    );
    match cursor.shape {
        CursorShape::Block => {
            Bounds::new(origin, size(metrics.cell_width, metrics.cell_height))
        }
        CursorShape::Underline => Bounds::new(
            point(origin.x, origin.y + metrics.cell_height - px(2.0)),
            size(metrics.cell_width, px(2.0)),
        ),
        CursorShape::Beam => {
            Bounds::new(origin, size(px(2.0), metrics.cell_height))
        }
        CursorShape::Hidden => Bounds::new(origin, size(px(0.0), px(0.0))),
    }
}

fn resolve_color(color: CellColor, theme: &Theme) -> Rgb {
    match color {
        CellColor::DefaultForeground => theme.foreground,
        CellColor::DefaultBackground => theme.background,
        CellColor::Cursor => theme.cursor,
        CellColor::Indexed(index) => theme.indexed(index),
        CellColor::Rgb(color) => color,
    }
}

fn paint_selection(
    snapshot: &TerminalSnapshot,
    selection: BufferRange,
    origin: Point<Pixels>,
    metrics: GridMetrics,
    theme: &Theme,
    window: &mut Window,
) {
    let rows = usize::from(snapshot.size.rows);
    let columns = usize::from(snapshot.size.columns);
    for row in 0..rows {
        let rows_from_live_bottom = snapshot
            .viewport
            .bottom_offset
            .saturating_add(rows.saturating_sub(1).saturating_sub(row));
        let mut start = None;
        let mut end = 0_u16;
        let row_start = row.saturating_mul(columns);
        let row_cells = snapshot
            .cells
            .get(row_start..row_start.saturating_add(columns))
            .unwrap_or_default();
        for column in 0..snapshot.size.columns {
            if selection_covers_column(
                selection,
                rows_from_live_bottom,
                column,
                row_cells,
            ) {
                start.get_or_insert(column);
                end = column.saturating_add(1);
            }
        }
        if let Some(start) = start {
            window.paint_quad(fill(
                cell_bounds(
                    origin,
                    start,
                    row,
                    end.saturating_sub(start),
                    metrics,
                ),
                rgb_color(theme.selection),
            ));
        }
    }
}

fn selection_covers_column(
    selection: BufferRange,
    rows_from_live_bottom: usize,
    column: u16,
    row: &[Cell],
) -> bool {
    let contains = |column| {
        range_contains(
            selection,
            huterm_protocol::BufferPoint {
                rows_from_live_bottom,
                column,
            },
        )
    };
    if contains(column) {
        return true;
    }
    let index = usize::from(column);
    if row.get(index).is_some_and(|cell| cell.style.wide) {
        return column.checked_add(1).is_some_and(contains);
    }
    row.get(index).is_some_and(|cell| cell.style.wide_spacer)
        && column.checked_sub(1).is_some_and(contains)
}

fn range_contains(
    range: BufferRange,
    point: huterm_protocol::BufferPoint,
) -> bool {
    let after_start = point.rows_from_live_bottom
        < range.start.rows_from_live_bottom
        || (point.rows_from_live_bottom == range.start.rows_from_live_bottom
            && point.column >= range.start.column);
    let before_end = point.rows_from_live_bottom
        > range.end.rows_from_live_bottom
        || (point.rows_from_live_bottom == range.end.rows_from_live_bottom
            && point.column <= range.end.column);
    after_start && before_end
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

fn timing_enabled(renderer_stats: bool, scroll_benchmark: bool) -> bool {
    renderer_stats || scroll_benchmark
}

fn scroll_benchmark_enabled() -> bool {
    std::env::var("HUTERM_SCROLL_BENCH")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

#[derive(Default)]
struct ScrollBenchmarkStats {
    in_flight: Option<ScrollBenchmarkSample>,
    ready_to_paint: Option<ScrollBenchmarkSample>,
    dropped_before_paint: usize,
}

impl ScrollBenchmarkStats {
    fn begin(
        &mut self,
        sequence: u64,
        requested_offset: usize,
        injected_at: Option<Instant>,
    ) {
        if self
            .in_flight
            .replace(ScrollBenchmarkSample {
                sequence,
                requested_offset,
                injected_at,
                ..ScrollBenchmarkSample::default()
            })
            .is_some()
        {
            self.dropped_before_paint += 1;
        }
    }

    fn complete_snapshot(
        &mut self,
        duration: Duration,
        returned_offset: usize,
        wakeup_delay: Duration,
    ) {
        let Some(mut sample) = self.in_flight.take() else {
            return;
        };
        sample.snapshot = duration;
        sample.returned_offset = returned_offset;
        sample.wakeup_delay = wakeup_delay;
        let matched_input = usize::from(sample.injected_at.is_some());
        let latency = sample
            .injected_at
            .map_or(Duration::ZERO, |injected_at| injected_at.elapsed());
        eprintln!(
            "huterm-scroll snapshot sequence={} requested={} returned={} snapshot_us={} input={} latency_us={} timer_wait_us={}",
            sample.sequence,
            sample.requested_offset,
            sample.returned_offset,
            sample.snapshot.as_micros(),
            matched_input,
            latency.as_micros(),
            sample.wakeup_delay.as_micros(),
        );
        if self.ready_to_paint.replace(sample).is_some() {
            self.dropped_before_paint += 1;
        }
    }

    fn complete_prepare(
        &mut self,
        duration: Duration,
        rebuilt_rows: usize,
        total_rows: usize,
    ) {
        if let Some(sample) = &mut self.ready_to_paint {
            sample.prepare = duration;
            sample.rebuilt_rows = rebuilt_rows;
            sample.total_rows = total_rows;
            sample.prepared = true;
        }
    }

    fn complete_paint(&mut self, duration: Duration) {
        if !self
            .ready_to_paint
            .as_ref()
            .is_some_and(|sample| sample.prepared)
        {
            return;
        }
        let Some(mut sample) = self.ready_to_paint.take() else {
            return;
        };
        sample.paint = duration;
        let matched_input = usize::from(sample.injected_at.is_some());
        let latency = sample
            .injected_at
            .map_or(Duration::ZERO, |injected_at| injected_at.elapsed());
        eprintln!(
            "huterm-scroll sample sequence={} requested={} returned={} snapshot_us={} prepare_us={} paint_us={} input={} latency_us={} timer_wait_us={} rebuilt_rows={} reused_rows={} dropped={}",
            sample.sequence,
            sample.requested_offset,
            sample.returned_offset,
            sample.snapshot.as_micros(),
            sample.prepare.as_micros(),
            sample.paint.as_micros(),
            matched_input,
            latency.as_micros(),
            sample.wakeup_delay.as_micros(),
            sample.rebuilt_rows,
            sample.total_rows.saturating_sub(sample.rebuilt_rows),
            self.dropped_before_paint,
        );
    }
}

struct ScrollBenchmarkSample {
    sequence: u64,
    requested_offset: usize,
    returned_offset: usize,
    injected_at: Option<Instant>,
    snapshot: Duration,
    prepare: Duration,
    paint: Duration,
    wakeup_delay: Duration,
    rebuilt_rows: usize,
    total_rows: usize,
    prepared: bool,
}

impl Default for ScrollBenchmarkSample {
    fn default() -> Self {
        Self {
            sequence: 0,
            requested_offset: 0,
            returned_offset: 0,
            injected_at: None,
            snapshot: Duration::ZERO,
            prepare: Duration::ZERO,
            paint: Duration::ZERO,
            wakeup_delay: Duration::ZERO,
            rebuilt_rows: 0,
            total_rows: 0,
            prepared: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use huterm_protocol::{
        CellStyle, GridSize, TerminalId, TerminalModes, Viewport,
    };

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
    fn row_diff_reuses_shifted_rows_during_static_scrolling() {
        let mut previous = snapshot(1, 3, &["B", "C", "D"]);
        previous.history_size = 10;
        let mut current = snapshot(1, 3, &["A", "B", "C"]);
        current.history_size = 10;
        current.viewport.bottom_offset = 1;

        assert_eq!(
            rows_to_rebuild(Some(&previous), &current),
            vec![true, false, false]
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
            rows_to_rebuild(Some(&previous), &current),
            vec![false, false, false]
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
            prepare_decorations(&cells, &Theme::default(), |cell| {
                cell.style.underline
            });

        assert_eq!(decorations.len(), 1);
        assert_eq!(decorations[0].start, 0);
        assert_eq!(decorations[0].columns, 3);
    }

    #[test]
    fn selection_expands_over_both_halves_of_a_wide_character() {
        let mut cells = snapshot(2, 1, &["界", " "]).cells;
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
    fn next_scroll_request_does_not_replace_snapshot_awaiting_paint() {
        let mut benchmark = ScrollBenchmarkStats::default();
        benchmark.begin(1, 10, Some(Instant::now()));
        benchmark.complete_snapshot(
            Duration::from_micros(10),
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
            cursor: None,
            modes: TerminalModes::default(),
            viewport: Viewport::default(),
            history_size: 0,
            cursor_color: None,
        }
    }
}
