//! Native benchmark of production row preparation and painting, without PTYs.
//!
//! One process measures one scenario. Every sample is a whole `prepare` or
//! `paint` call against synthetic snapshots, so results do not depend on PTY
//! throughput or snapshot scheduling. Each frame prepares one step and paints
//! it, and a measured step records both timings.
//!
//! Sampling must finish early in the process. On macOS the same paint cost 2.2
//! to 2.7 times more once the process was about two seconds old, presumably
//! because the scheduler stops favoring a lightly loaded process. A scenario
//! that sampled paint only after a long prepare phase straddled that change.
//! `elapsed_ms` on the window line reports when sampling ended.
//!
//! Preparing every step inside one frame hid shaping cost: GPUI keeps line
//! layouts for the current and previous frame, so a layout the renderer had
//! evicted came back from that cache instead of the platform shaper. Repeated
//! paints inside one frame grow that frame's scene, and the allocation and
//! draw-order work for the growing scene inflated later samples threefold.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};
use gpui::{
    App, Bounds, Context, Render, Window, WindowBounds, WindowOptions, canvas,
    div, point, prelude::*, px, size,
};
use huterm_protocol::{
    BufferPoint, BufferRange, Cell, CellColor, CellStyle, Cursor, CursorShape,
    GridSize, TerminalId, TerminalModes, TerminalRow, TerminalSnapshot,
    Viewport,
};

use super::{GlyphContent, GridMetrics, PreparedRow, TerminalRenderer};
use crate::config::Theme;

const COLUMNS: u16 = 160;
const ROWS: u16 = 50;
const FONT_SIZE: f32 = 12.0;
const WARMUP_CYCLES: usize = 3;
const DEFAULT_ITERATIONS: usize = 30;
/// Each iteration builds up to three full-grid snapshots up front, so the
/// count is capped before any scenario allocates.
const MAX_ITERATIONS: usize = 10_000;
const FRAME_DEADLINE: Duration = Duration::from_secs(15);
/// Distinguishes missing frames, a display problem, from benchmark failures.
const FRAME_STALL_EXIT_CODE: i32 = 3;
const SCENARIOS: [&str; 6] =
    ["ascii", "blocks", "boxes", "churn", "scroll", "selection"];

pub(crate) fn run() -> anyhow::Result<()> {
    let name = std::env::var("HUTERM_RENDERER_BENCH_SCENARIO")
        .context("set HUTERM_RENDERER_BENCH_SCENARIO")?;
    let iterations = match std::env::var("HUTERM_RENDERER_BENCH_ITERATIONS") {
        Ok(value) => value
            .parse::<usize>()
            .ok()
            .filter(|iterations| (1..=MAX_ITERATIONS).contains(iterations))
            .with_context(|| {
                format!(
                    "HUTERM_RENDERER_BENCH_ITERATIONS must be between 1 and {MAX_ITERATIONS}"
                )
            })?,
        Err(_) => DEFAULT_ITERATIONS,
    };
    let Some(scenario) = Scenario::named(&name, iterations) else {
        bail!(
            "unknown scenario {name:?}; expected one of {}",
            SCENARIOS.join(", ")
        );
    };
    crate::assets::application().run(move |cx| {
        let display_id = crate::benchmark_display::selected(cx)
            .expect("select benchmark display");
        let metrics =
            GridMetrics::resolve(cx.text_system(), family(), px(FONT_SIZE))
                .expect("benchmark font");
        // Device-pixel rounding at the window's scale can widen each cell by
        // up to one logical pixel, so leave that much room for the whole grid.
        let grid = size(
            (metrics.cell_width + px(1.0)) * f32::from(COLUMNS),
            (metrics.cell_height + px(1.0)) * f32::from(ROWS),
        );
        cx.open_window(
            WindowOptions {
                display_id,
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(20.0), px(40.0)),
                    grid,
                ))),
                ..WindowOptions::default()
            },
            |window, cx| {
                if let Some(display) = display_id {
                    crate::benchmark_display::verify(window, cx, display);
                }
                window.set_window_title("Huterm renderer benchmark");
                let fixture = Fixture::new(scenario, window, cx);
                cx.new(|_| fixture)
            },
        )
        .expect("open renderer benchmark window");
        cx.activate(true);
        // Paint needs one frame per sample. Xvfb without a window manager
        // never reports the window visible, so GPUI stops after one frame.
        cx.spawn(async move |cx| {
            cx.background_executor().timer(FRAME_DEADLINE).await;
            eprintln!(
                "renderer benchmark received too few frames within {FRAME_DEADLINE:?}; on Xvfb, run a window manager"
            );
            std::process::exit(FRAME_STALL_EXIT_CODE);
        })
        .detach();
    });
    Ok(())
}

/// A prepared sequence of snapshots plus the snapshot whose paint is timed.
struct Scenario {
    name: &'static str,
    /// Snapshots prepared in order. Only flagged steps are recorded, so a
    /// scenario can interleave setup work with the transition under test.
    steps: Vec<(Arc<TerminalSnapshot>, bool)>,
    paint: Arc<TerminalSnapshot>,
    selection: Option<BufferRange>,
    theme: Theme,
}

impl Scenario {
    fn named(name: &str, iterations: usize) -> Option<Self> {
        let cycles = WARMUP_CYCLES + iterations;
        Some(match name {
            // Dense styled text where every row changes between snapshots.
            "ascii" => Self::alternating("ascii", cycles, ascii_cell),
            // The PTY workload's colored half blocks: all builtin rectangles.
            "blocks" => Self::alternating("blocks", cycles, block_cell),
            // A bordered TUI with rounded corners, diagonals, and powerline
            // separators, which paint through paths instead of glyph sprites.
            "boxes" => Self::alternating("boxes", cycles, box_cell),
            "selection" => Self::selection(cycles),
            "scroll" => Self::scroll(cycles),
            // Three frames per cycle: fewer cycles keep sampling early, but
            // at least one measured cycle must remain.
            "churn" => Self::churn(WARMUP_CYCLES + (iterations * 2 / 3).max(1)),
            _ => return None,
        })
    }

    /// Alternates two variants that differ in every row, so each measured
    /// step rebuilds the whole grid.
    fn alternating(
        name: &'static str,
        cycles: usize,
        cell: fn(usize, usize, usize) -> Cell,
    ) -> Self {
        let [first, second] = [0, 1].map(|variant| {
            snapshot(grid(|row, column| cell(row, column, variant)), 0)
        });
        let mut steps = Vec::new();
        for cycle in 0..cycles {
            let measured = cycle >= WARMUP_CYCLES;
            steps.push((Arc::clone(&first), measured));
            steps.push((Arc::clone(&second), measured));
        }
        Self {
            name,
            steps,
            paint: first,
            selection: None,
            theme: Theme::default(),
        }
    }

    /// The `ascii` text painted under a full-screen selection.
    fn selection(cycles: usize) -> Self {
        let mut scenario = Self::alternating("selection", cycles, ascii_cell);
        scenario.selection = Some(BufferRange {
            start: BufferPoint {
                rows_from_live_bottom: usize::from(ROWS - 1),
                column: 0,
            },
            end: BufferPoint {
                rows_from_live_bottom: 0,
                column: COLUMNS - 1,
            },
        });
        scenario.theme.selection_foreground = Some(scenario.theme.background);
        scenario
    }

    /// Output scrolling by one row: retained rows shift, one is new.
    fn scroll(cycles: usize) -> Self {
        let ring: Vec<_> = (0..usize::from(ROWS) + 14)
            .map(|row| terminal_row(|column| ascii_cell(row, column, 0)))
            .collect();
        let step = |index: usize| {
            let rows = (0..usize::from(ROWS))
                .map(|row| Arc::clone(&ring[(index + row) % ring.len()]))
                .collect();
            snapshot(rows, index)
        };
        Self {
            name: "scroll",
            steps: (0..cycles)
                .map(|index| (step(index), index >= WARMUP_CYCLES))
                .collect(),
            paint: step(0),
            selection: None,
            theme: Theme::default(),
        }
    }

    /// Two single-row updates followed by a full redraw of many distinct
    /// characters. Only the redraw is recorded: it shows what the earlier
    /// small updates evicted from the glyph layout cache.
    fn churn(cycles: usize) -> Self {
        let full = [0, 1].map(|variant| {
            grid(|row, column| varied_cell(row, column, variant))
        });
        let mut steps = Vec::new();
        for cycle in 0..cycles {
            let rows = &full[cycle % 2];
            steps.push((snapshot(rows.clone(), 0), cycle >= WARMUP_CYCLES));
            // Two updates in their own frames are enough for a cache that
            // rotates on every prepare, and for GPUI's two-frame layout cache,
            // to drop the redraw's text.
            for update in 0..2 {
                let mut rows = rows.clone();
                rows[usize::from(ROWS) - 1] = terminal_row(|column| {
                    prompt_cell(column, cycle * 2 + update)
                });
                steps.push((snapshot(rows, 0), false));
            }
        }
        Self {
            name: "churn",
            steps,
            paint: snapshot(full[0].clone(), 0),
            selection: None,
            theme: Theme::default(),
        }
    }
}

fn grid(cell: impl Fn(usize, usize) -> Cell) -> Vec<Arc<TerminalRow>> {
    (0..usize::from(ROWS))
        .map(|row| terminal_row(|column| cell(row, column)))
        .collect()
}

fn terminal_row(cell: impl Fn(usize) -> Cell) -> Arc<TerminalRow> {
    Arc::new(TerminalRow {
        cells: (0..usize::from(COLUMNS)).map(cell).collect(),
    })
}

fn snapshot(
    rows: Vec<Arc<TerminalRow>>,
    history_size: usize,
) -> Arc<TerminalSnapshot> {
    Arc::new(TerminalSnapshot {
        terminal_id: TerminalId::new(1),
        generation: 1,
        size: GridSize::clamped(COLUMNS, ROWS),
        rows,
        cursor: Some(Cursor {
            row: ROWS - 1,
            column: 2,
            shape: CursorShape::Block,
        }),
        modes: TerminalModes::default(),
        viewport: Viewport::default(),
        history_size,
        cursor_color: None,
    })
}

fn plain(text: impl Into<String>) -> Cell {
    Cell {
        text: text.into(),
        foreground: CellColor::DefaultForeground,
        background: CellColor::DefaultBackground,
        style: CellStyle::default(),
    }
}

fn indexed(index: usize) -> CellColor {
    CellColor::Indexed(u8::try_from(index % 256).unwrap_or_default())
}

const SOURCE: &str = "fn prepare(&mut self, snapshot: &Snapshot) -> Result<usize, Error> { let rows = snapshot.rows.iter().filter(|row| row.dirty).count(); // TODO: reuse 0x7f [a..=z] ";

fn ascii_cell(row: usize, column: usize, variant: usize) -> Cell {
    let source = SOURCE.as_bytes();
    let byte = source[(row * 31 + column + variant * 7) % source.len()];
    let mut cell = plain(char::from(byte));
    match (column / 8 + row + variant) % 6 {
        1 => {
            cell.style.bold = true;
            cell.foreground = indexed(2);
        }
        2 => {
            cell.style.italic = true;
            cell.foreground = indexed(4);
        }
        3 => cell.style.underline = true,
        4 => {
            cell.background = indexed(row % 6 + 1);
            cell.foreground = indexed(0);
        }
        5 => cell.style.dim = true,
        _ => {}
    }
    cell
}

fn block_cell(row: usize, column: usize, phase: usize) -> Cell {
    let mut cell = plain('▀');
    cell.foreground = indexed(16 + (phase + row * 3 + column) % 216);
    cell
}

fn box_cell(row: usize, column: usize, variant: usize) -> Cell {
    const PANEL_ROWS: usize = 10;
    const PANEL_COLUMNS: usize = 20;
    let (panel_row, panel_column) =
        (row % PANEL_ROWS, (column + variant) % PANEL_COLUMNS);
    let (last_row, last_column) = (PANEL_ROWS - 1, PANEL_COLUMNS - 1);
    let edge_row = panel_row == 0 || panel_row == last_row;
    let edge_column = panel_column == 0 || panel_column == last_column;
    let text = match (panel_row, panel_column) {
        (0, 0) => '╭',
        (0, column) if column == last_column => '╮',
        (row, 0) if row == last_row => '╰',
        (row, column) if row == last_row && column == last_column => '╯',
        _ if edge_row => '─',
        _ if edge_column => '│',
        (5, column) => {
            if column % 2 == 0 {
                '╱'
            } else {
                '╲'
            }
        }
        (7, column) if column % 6 == 0 => '\u{e0b0}',
        (3, column) => ['░', '▒', '▓', '█'][column % 4],
        (_, column) => char::from(b"status ok "[column % 10]),
    };
    let mut cell = plain(text);
    if edge_row || edge_column {
        cell.foreground = indexed(8);
    } else if panel_row == 7 {
        // Powerline segments: the separator takes the previous segment's
        // background as its foreground.
        let segment = panel_column / 6;
        cell.background = indexed(segment + 1);
        cell.foreground = if text == '\u{e0b0}' {
            indexed(segment)
        } else {
            indexed(0)
        };
    }
    cell
}

/// Roughly 480 distinct single-scalar characters across common scripts.
fn varied_characters() -> Vec<char> {
    [
        0x21..=0x7e_u32,
        0xa1..=0xac,
        0xae..=0xff,
        0x100..=0x17f,
        0x391..=0x3a1,
        0x3a3..=0x3c9,
        0x410..=0x44f,
    ]
    .into_iter()
    .flatten()
    .filter_map(char::from_u32)
    .collect()
}

fn varied_cell(row: usize, column: usize, variant: usize) -> Cell {
    thread_local! {
        static CHARACTERS: Vec<char> = varied_characters();
    }
    CHARACTERS.with(|characters| {
        plain(characters[(row * 53 + column + variant * 11) % characters.len()])
    })
}

fn prompt_cell(column: usize, update: usize) -> Cell {
    let digits = b"0123456789";
    plain(char::from(digits[(column + update) % digits.len()]))
}

fn family() -> &'static str {
    if cfg!(target_os = "macos") {
        "Menlo"
    } else {
        "monospace"
    }
}

struct State {
    renderer: TerminalRenderer,
    scenario: Scenario,
    started: Instant,
    next_step: usize,
    /// Whether the step prepared in this frame records its timings.
    frame_measured: bool,
    prepares: Prepares,
    paints: Vec<Duration>,
    finished: bool,
    frames: crate::benchmark_display::FrameIntervals,
}

struct Fixture {
    state: Rc<RefCell<State>>,
}

impl Fixture {
    fn new(scenario: Scenario, window: &Window, cx: &App) -> Self {
        let family = family();
        let metrics =
            GridMetrics::resolve(cx.text_system(), family, px(FONT_SIZE))
                .expect("benchmark font")
                .at_scale(window.scale_factor());
        let mut renderer = TerminalRenderer::new(
            family.into(),
            scenario.theme.clone(),
            metrics,
        );
        renderer.set_selection(scenario.selection);
        Self {
            state: Rc::new(RefCell::new(State {
                renderer,
                scenario,
                started: Instant::now(),
                next_step: 0,
                frame_measured: false,
                prepares: Prepares::default(),
                paints: Vec::new(),
                finished: false,
                frames: crate::benchmark_display::FrameIntervals::default(),
            })),
        }
    }
}

impl Render for Fixture {
    fn render(
        &mut self,
        window: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let metrics = {
            let mut state = self.state.borrow_mut();
            state.frames.tick();
            let metrics =
                state.renderer.metrics.at_scale(window.scale_factor());
            if metrics != state.renderer.metrics {
                let family = state.renderer.font_family.clone();
                let theme = state.renderer.theme.clone();
                state.renderer.reconfigure(family, theme, metrics);
            }
            metrics
        };
        let grid = size(
            metrics.cell_width * f32::from(COLUMNS),
            metrics.cell_height * f32::from(ROWS),
        );
        let viewport = window.viewport_size();
        // Clipped glyphs skip scene insertion and would understate paint cost.
        let fits =
            grid.width <= viewport.width && grid.height <= viewport.height;
        if !self.state.borrow().finished {
            window.request_animation_frame();
        }
        let prepare = Rc::clone(&self.state);
        let paint = Rc::clone(&self.state);
        div()
            .size_full()
            .bg(super::rgb_color(Theme::default().background))
            .child(
                canvas(
                    move |_, window, _| {
                        let mut state = prepare.borrow_mut();
                        if !prepare_next_step(&mut state, window) {
                            let snapshot = Arc::clone(&state.scenario.paint);
                            state.renderer.prepare(Some(&snapshot), window);
                        }
                    },
                    move |bounds, (), window, cx| {
                        let mut state = paint.borrow_mut();
                        let started = Instant::now();
                        state.renderer.paint(bounds, window);
                        let elapsed = started.elapsed();
                        if state.finished {
                            return;
                        }
                        if state.frame_measured {
                            state.paints.push(elapsed);
                        }
                        // The frame after the last step prepared the paint
                        // snapshot, whose primitives the report counts.
                        if state.next_step <= state.scenario.steps.len() {
                            return;
                        }
                        state.finished = true;
                        if let Some(display) = window.display(cx) {
                            state.frames.report("renderer", display.id());
                        }
                        report_paint(&state);
                        println!(
                            "RENDERER_BENCH scenario={} phase=window elapsed_ms={} grid={COLUMNS}x{ROWS} grid_px={}x{} viewport_px={}x{} scale={} fits={fits}",
                            state.scenario.name,
                            state.started.elapsed().as_millis(),
                            f32::from(grid.width),
                            f32::from(grid.height),
                            f32::from(viewport.width),
                            f32::from(viewport.height),
                            window.scale_factor(),
                        );
                        cx.defer(|cx| {
                            cx.spawn(async move |cx| {
                                // On Linux the initial paint can finish before the
                                // native event loop starts. Cross that boundary before
                                // asking the platform loop to stop.
                                cx.background_executor()
                                    .timer(Duration::from_millis(50))
                                    .await;
                                cx.update(|cx| {
                                    println!("RENDERER_BENCH passed");
                                    cx.quit();
                                })
                                .expect("finish renderer benchmark");
                            })
                            .detach();
                        });
                    },
                )
                .w(grid.width)
                .h(grid.height)
                .flex_shrink_0(),
            )
    }
}

#[derive(Default)]
struct Prepares {
    samples: Vec<Duration>,
    rebuilt_rows: usize,
    hits: usize,
    misses: usize,
}

/// Prepares this frame's step. Returns false once every step has run, after
/// reporting them; `next_step` then moves past the end, and the caller
/// prepares the paint snapshot for the final frame.
fn prepare_next_step(state: &mut State, window: &mut Window) -> bool {
    let Some((snapshot, measured)) =
        state.scenario.steps.get(state.next_step).cloned()
    else {
        if state.next_step == state.scenario.steps.len() {
            report_prepare(state);
        }
        state.next_step = state.scenario.steps.len() + 1;
        state.frame_measured = false;
        return false;
    };
    state.next_step += 1;
    state.frame_measured = measured;
    let started = Instant::now();
    state.renderer.prepare(Some(&snapshot), window);
    let elapsed = started.elapsed();
    if measured {
        let outcome = state.renderer.last_prepare;
        state.prepares.samples.push(elapsed);
        state.prepares.rebuilt_rows += outcome.rebuilt_rows;
        state.prepares.hits += outcome.cache.hits;
        state.prepares.misses += outcome.cache.misses;
    }
    true
}

fn report_prepare(state: &mut State) {
    let prepares = std::mem::take(&mut state.prepares);
    let count = prepares.samples.len().max(1);
    println!(
        "RENDERER_BENCH scenario={} phase=prepare {} rebuilt_rows={} cache_hits={} cache_misses={}",
        state.scenario.name,
        Summary::new(prepares.samples),
        prepares.rebuilt_rows / count,
        prepares.hits / count,
        prepares.misses / count,
    );
}

fn report_paint(state: &State) {
    let samples = state.paints.clone();
    if std::env::var_os("HUTERM_RENDERER_BENCH_SAMPLES").is_some() {
        // Unsorted, so a trend across frames stays visible.
        let samples: Vec<_> = samples
            .iter()
            .map(|sample| sample.as_nanos().to_string())
            .collect();
        println!("RENDERER_BENCH paint_samples_ns={}", samples.join(","));
    }
    println!(
        "RENDERER_BENCH scenario={} phase=paint {} {}",
        state.scenario.name,
        Summary::new(samples),
        Primitives::count(&state.renderer.rows),
    );
}

/// Deterministic per-paint primitive counts for the prepared rows.
#[derive(Default)]
struct Primitives {
    backgrounds: usize,
    glyphs: usize,
    builtins: usize,
    rectangles: usize,
    paths: usize,
    decorations: usize,
}

impl Primitives {
    fn count(rows: &[PreparedRow]) -> Self {
        let mut counts = Self::default();
        for row in rows {
            counts.backgrounds += row.backgrounds.len();
            counts.decorations += row.underlines.len() + row.strikeouts.len();
            for glyph in &row.glyphs {
                match &glyph.content {
                    GlyphContent::Font(_) => counts.glyphs += 1,
                    GlyphContent::Builtin(geometry) => {
                        let (rectangles, paths) = geometry.primitive_counts();
                        counts.builtins += 1;
                        counts.rectangles += rectangles;
                        counts.paths += paths;
                    }
                }
            }
        }
        counts
    }
}

impl std::fmt::Display for Primitives {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "backgrounds={} glyphs={} builtins={} rectangles={} paths={} decorations={}",
            self.backgrounds,
            self.glyphs,
            self.builtins,
            self.rectangles,
            self.paths,
            self.decorations,
        )
    }
}

/// Elapsed-time distribution. `Instant` samples include scheduler preemption,
/// so comparisons should use the median and minimum rather than the maximum.
struct Summary {
    samples: Vec<Duration>,
}

impl Summary {
    fn new(mut samples: Vec<Duration>) -> Self {
        samples.sort_unstable();
        Self { samples }
    }

    fn quantile(&self, numerator: usize, denominator: usize) -> u128 {
        let last = self.samples.len().saturating_sub(1);
        let index = (last * numerator).div_ceil(denominator);
        self.samples.get(index).map_or(0, Duration::as_nanos)
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "samples={} min_ns={} median_ns={} p95_ns={} max_ns={}",
            self.samples.len(),
            self.quantile(0, 1),
            self.quantile(1, 2),
            self.quantile(19, 20),
            self.quantile(1, 1),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_reports_nearest_rank_quantiles() {
        let summary = Summary::new(
            [5, 1, 4, 2, 3]
                .map(Duration::from_nanos)
                .into_iter()
                .collect(),
        );
        assert_eq!(
            summary.to_string(),
            "samples=5 min_ns=1 median_ns=3 p95_ns=5 max_ns=5"
        );
    }

    #[test]
    fn every_listed_scenario_builds_full_grids() {
        for name in SCENARIOS {
            let scenario = Scenario::named(name, 2).expect(name);
            assert!(scenario.steps.iter().any(|(_, measured)| *measured));
            for (snapshot, _) in &scenario.steps {
                assert_eq!(snapshot.rows.len(), usize::from(ROWS));
                assert!(
                    snapshot
                        .rows
                        .iter()
                        .all(|row| { row.cells.len() == usize::from(COLUMNS) })
                );
            }
        }
        assert!(Scenario::named("missing", 2).is_none());
    }
}
