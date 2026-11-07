//! Native fixture using production row preparation and painting, without PTYs.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Bounds, Context, Pixels, Render, Window, WindowBounds, WindowOptions,
    canvas, div, point, prelude::*, px, size,
};
use huterm_protocol::{
    BufferPoint, BufferRange, Cell, CellColor, CellStyle, Cursor, CursorShape,
    GridSize, TerminalId, TerminalModes, TerminalRow, TerminalSnapshot,
    Viewport,
};

use super::{GlyphContent, GridMetrics, TerminalRenderer};
use crate::config::Theme;

pub(crate) fn run() {
    let hold = std::env::var_os("HUTERM_RENDERER_HOLD").is_some();
    // Startup markers bound a hang before the first paint, which otherwise
    // leaves no output at all.
    println!("RENDERER_SMOKE launching");
    crate::assets::application().run(move |cx| {
        crate::terminal_font::register_bundled_fonts(cx);
        println!("RENDERER_SMOKE application started");
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(40.0), px(60.0)),
                    size(px(1120.0), px(840.0)),
                ))),
                ..WindowOptions::default()
            },
            |window, cx| {
                window.set_window_title("Huterm renderer smoke");
                {
                    let fixture = Fixture::new(window, cx);
                    cx.new(|_| fixture)
                }
            },
        )
        .expect("open renderer smoke window");
        println!("RENDERER_SMOKE window opened");
        if hold {
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
        }
        cx.activate(true);
    });
}

struct Panel {
    renderer: TerminalRenderer,
    snapshot: Arc<TerminalSnapshot>,
    checked: bool,
}

struct Fixture {
    panels: Vec<Rc<RefCell<Panel>>>,
    completion: Rc<RefCell<Completion>>,
}

#[derive(Default)]
struct Completion {
    prepared: [bool; 3],
    checked: [bool; 3],
    painted: [bool; 3],
    scheduled: bool,
}

impl Completion {
    fn prepared_and_checked(&mut self, index: usize) {
        self.prepared[index] = true;
        self.checked[index] = true;
    }

    fn painted(&mut self, index: usize) -> bool {
        self.painted[index] = true;
        if self.scheduled
            || !self.prepared.into_iter().all(std::convert::identity)
            || !self.checked.into_iter().all(std::convert::identity)
            || !self.painted.into_iter().all(std::convert::identity)
        {
            return false;
        }
        self.scheduled = true;
        true
    }
}

impl Fixture {
    fn new(window: &Window, cx: &App) -> Self {
        let family = if cfg!(target_os = "macos") {
            "Menlo"
        } else {
            "DejaVu Sans Mono"
        };
        let panels = [12.0, 16.0, 20.0]
            .into_iter()
            .enumerate()
            .map(|(index, font_size)| {
                // Exercise native matching and paint with Medium as well as
                // the regular benchmark/default face.
                let weight = if index == 1 {
                    huterm_config::FontWeight::parse(&toml::Value::Integer(500))
                        .expect("medium weight")
                } else {
                    huterm_config::FontWeight::default()
                };
                let terminal_font = crate::terminal_font::TerminalFont::new(
                    family.into(),
                    weight,
                );
                let metrics = GridMetrics::resolve(
                    cx.text_system(),
                    &terminal_font,
                    px(font_size),
                )
                .expect("fixture font")
                .at_scale(window.scale_factor());
                Rc::new(RefCell::new(Panel {
                    renderer: TerminalRenderer::new(
                        terminal_font,
                        Theme::default(),
                        metrics,
                    ),
                    snapshot: Arc::new(snapshot(font_size)),
                    checked: false,
                }))
            })
            .collect();
        Self {
            panels,
            completion: Rc::new(RefCell::new(Completion::default())),
        }
    }
}

impl Render for Fixture {
    fn render(
        &mut self,
        window: &mut Window,
        _: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        for panel in &self.panels {
            let mut panel = panel.borrow_mut();
            let metrics =
                panel.renderer.metrics.at_scale(window.scale_factor());
            if metrics != panel.renderer.metrics {
                let family = panel.renderer.terminal_font.clone();
                let theme = panel.renderer.theme.clone();
                panel.renderer.reconfigure(family, theme, metrics);
                panel.checked = false;
            }
        }
        let mut root = div()
            .flex()
            .gap(px(20.0))
            .p(px(20.0))
            .size_full()
            .bg(gpui::rgb(0x001d_1f21));
        for (index, panel) in self.panels.iter().enumerate() {
            let prepare = Rc::clone(panel);
            let paint = Rc::clone(panel);
            let prepare_completion = Rc::clone(&self.completion);
            let paint_completion = Rc::clone(&self.completion);
            let m = panel.borrow().renderer.metrics;
            root = root.child(
                canvas(
                    move |_, window, _| {
                        let mut panel = prepare.borrow_mut();
                        let snapshot = Arc::clone(&panel.snapshot);
                        panel.renderer.prepare(Some(&snapshot), window);
                        if !panel.checked {
                            check(&mut panel, window);
                            panel.checked = true;
                        }
                        prepare_completion
                            .borrow_mut()
                            .prepared_and_checked(index);
                    },
                    move |bounds, (), window, cx| {
                        paint.borrow_mut().renderer.paint(bounds, window);
                        if paint_completion.borrow_mut().painted(index)
                            && std::env::var_os("HUTERM_RENDERER_HOLD").is_none()
                        {
                            cx.defer(|cx| {
                                cx.spawn(async move |cx| {
                                    // On Linux the initial paint can finish before the
                                    // native event loop starts. Cross that boundary before
                                    // asking the platform loop to stop.
                                    cx.background_executor()
                                        .timer(Duration::from_millis(50))
                                        .await;
                                    cx.update(|cx| {
                                        println!("RENDERER_SMOKE all-panels prepared=3 checked=3 painted=3");
                                        println!("RENDERER_SMOKE passed");
                                        cx.quit();
                                    });
                                })
                                .detach();
                            });
                        }
                    },
                )
                .w(m.cell_width * 32.0)
                .h(m.cell_height * f32::from(ROWS))
                .flex_shrink_0(),
            );
        }
        root
    }
}

fn cached_geometry(
    renderer: &TerminalRenderer,
    ch: char,
    columns: u16,
) -> &Arc<super::builtin::Geometry> {
    renderer
        .graphics
        .get(ch, columns)
        .expect("prepared builtin geometry")
}

const ROWS: u16 = 34;
/// Nerd Font icons that neither platform's default terminal font covers,
/// then text that stays on the font path.
const ICONS: &str =
    "\u{e0a0} \u{f07b} \u{f115} \u{f0001} \u{23fb} \u{e300} \u{f31a}  a";
const ICON_ROW: usize = 29;
const ICON_WIDE: usize = 12;

fn check(panel: &mut Panel, window: &mut Window) {
    check_icons(panel, window);
    let renderer = &mut panel.renderer;
    assert_eq!(
        renderer.graphics.len(),
        190,
        "186 standalone and four wide builtins must bypass font shaping"
    );
    check_wide(panel);
    let renderer = &mut panel.renderer;
    let first = Arc::clone(cached_geometry(renderer, '▐', 1));
    // An unchanged snapshot reuses prepared geometry.
    renderer.prepare(Some(&panel.snapshot), window);
    assert!(Arc::ptr_eq(&first, cached_geometry(renderer, '▐', 1)));

    let row = &renderer.rows[25];
    assert!(
        row.glyphs.iter().all(|glyph| glyph.column < 16),
        "hidden cells must not paint"
    );
    assert!(matches!(&row.glyphs[0].content, GlyphContent::Font(_)));
    assert!(
        row.glyphs
            .iter()
            .any(|glyph| matches!(&glyph.content, GlyphContent::Builtin(_)))
    );

    for column in [5, 7] {
        let glyph = renderer.rows[27]
            .glyphs
            .iter()
            .find(|glyph| glyph.column == column)
            .expect("grapheme glyph");
        assert!(
            matches!(&glyph.content, GlyphContent::Font(_)),
            "combining sequences must use font shaping"
        );
    }

    let mut theme = renderer.theme.clone();
    theme.selection_foreground = Some(huterm_protocol::Rgb {
        red: 255,
        green: 220,
        blue: 0,
    });
    renderer.reconfigure(
        renderer.terminal_font.clone(),
        theme,
        renderer.metrics,
    );
    renderer.prepare(Some(&panel.snapshot), window);
    assert!(
        Arc::ptr_eq(&first, cached_geometry(renderer, '▐', 1)),
        "theme reload reuses color-free geometry"
    );
    let original_metrics = renderer.metrics;
    renderer.reconfigure(
        renderer.terminal_font.clone(),
        renderer.theme.clone(),
        original_metrics.at_scale(original_metrics.scale_factor + 0.5),
    );
    renderer.prepare(Some(&panel.snapshot), window);
    assert!(
        !Arc::ptr_eq(&first, cached_geometry(renderer, '▐', 1)),
        "display scale invalidates geometry"
    );
    renderer.reconfigure(
        renderer.terminal_font.clone(),
        renderer.theme.clone(),
        original_metrics,
    );
    renderer.prepare(Some(&panel.snapshot), window);
    renderer.set_selection(Some(BufferRange {
        start: BufferPoint {
            rows_from_live_bottom: 6,
            column: 0,
        },
        end: BufferPoint {
            rows_from_live_bottom: 6,
            column: 31,
        },
    }));
    println!(
        "RENDERER_SMOKE prepared size={} scale={} builtin=186",
        f32::from(renderer.metrics.font_size),
        renderer.metrics.scale_factor
    );
}

/// Icons lay out in the bundled symbols font, one em fitted to the cell and
/// centred in it, while text keeps the terminal font.
fn check_icons(panel: &Panel, window: &Window) {
    let renderer = &panel.renderer;
    let metrics = renderer.metrics;
    let text_system = window.text_system();
    let symbols = text_system
        .resolve_font(&gpui::font(crate::terminal_font::SYMBOLS_FAMILY));
    let glyphs = &renderer.rows[ICON_ROW].glyphs;
    assert_eq!(glyphs.len(), 8, "seven icons and one letter");
    let size = super::symbols::font_size(metrics);
    let ascent = text_system.ascent(symbols, size);
    let descent = text_system.descent(symbols, size).abs();
    let whole = |value: Pixels| {
        let device = value * metrics.scale_factor;
        (device - device.round()).abs() < px(0.001)
    };
    for glyph in &glyphs[..7] {
        let wide = usize::from(glyph.column) == ICON_WIDE;
        assert_eq!(glyph.wide, wide, "wide flag at column {}", glyph.column);
        let GlyphContent::Symbol(symbol) = &glyph.content else {
            panic!("icon at column {} must use the symbols font", glyph.column)
        };
        let x = if wide { symbol.wide_x } else { symbol.origin.x };
        let (layout, y) = (&symbol.layout, symbol.origin.y);
        assert_eq!(layout.runs.len(), 1);
        assert_eq!(layout.runs[0].font_id, symbols);
        assert_eq!(layout.font_size, size);
        let span = metrics.cell_width * if wide { 2.0 } else { 1.0 };
        let right = x + layout.width;
        assert!(
            x >= px(0.0) && right <= span + px(0.5),
            "icon at column {} spans {x:?}..{right:?} of {span:?}",
            glyph.column
        );
        // Equal space on both sides and above and below, within the device
        // pixel that alignment may move each origin.
        let pixel = px(1.0 / metrics.scale_factor);
        assert!(
            (x - (span - right)).abs() <= pixel,
            "icon at column {} must centre horizontally",
            glyph.column
        );
        assert!(
            ((y - ascent) - (metrics.cell_height - y - descent)).abs() <= pixel,
            "icon at column {} must centre vertically",
            glyph.column
        );
        assert!(
            whole(x) && whole(y),
            "icon at column {} must sit on whole device pixels",
            glyph.column
        );
    }
    assert!(matches!(&glyphs[7].content, GlyphContent::Font(_)));
}

fn snapshot(font_size: f32) -> TerminalSnapshot {
    let mut lines = vec![format!("Font size {font_size}"), String::new()];
    for start in (0x2500..=0x2590).step_by(16) {
        let mut line = String::new();
        for cp in start..start + 16 {
            line.push(char::from_u32(cp).expect("fixture codepoint"));
            line.push(' ');
        }
        lines.push(line);
    }
    lines.extend(
        [
            "",
            "┌───┬───┐ ╔═══╦═══╗",
            "│ ▐▕│ ▐▕│ ║ ▐▕║ ▐▕║",
            "│ ▐▕│ ▐▕│ ║ ▐▕║ ▐▕║",
            "│ ▐▕│ ▐▕│ ║ ▐▕║ ▐▕║",
            "├───┼───┤ ╠═══╬═══╣",
            "│ ▐▕│ ▐▕│ ║ ▐▕║ ▐▕║",
            "└───┴───┘ ╚═══╩═══╝",
            "╭───────╮ ╱╲╱╲╱╲",
            "│░▒▓█▐▕ │ ╲╱╲╱╲╱",
            "╰───────╯",
            "",
            "normal ▐▕ bold ▐▕",
            "dim    ▐▕ hidden ▐▕",
            "selected ▐▕ ░▒▓█",
            "text M界  ▐\u{301}  ▕\u{fe0e}",
            "",
        ]
        .map(str::to_owned),
    );
    lines.push(ICONS.to_owned());
    lines.extend(["\u{e0b0} \u{e0b1} \u{e0b2} \u{e0b3} \u{e0b4} \u{e0b5} \u{e0b6} \u{e0b7} \u{e0b8} \u{e0b9} \u{e0ba} \u{e0bb} \u{e0bc} \u{e0bd} \u{e0be} \u{e0bf}", "\u{e0d2} \u{e0d4} ◢ ◣ ◤ ◥ ◸ ◹ ◺ ◿", "██\u{e0b0}██\u{e0b4}██\u{e0b8}██\u{e0bc}██\u{e0d2}", "██\u{e0b1}██\u{e0b5}██◸██◿██\u{e0d4}"].map(str::to_owned));
    let rows = lines
        .into_iter()
        .enumerate()
        .map(|(row, line)| {
            let mut cells: Vec<_> = line
                .chars()
                .map(|ch| Cell {
                    text: ch.into(),
                    foreground: CellColor::Rgb(Theme::default().foreground),
                    background: CellColor::Rgb(Theme::default().background),
                    style: CellStyle::default(),
                })
                .collect();
            cells.resize_with(32, || Cell {
                text: " ".into(),
                foreground: CellColor::DefaultForeground,
                background: CellColor::DefaultBackground,
                style: CellStyle::default(),
            });
            cells.truncate(32);
            populate_row(row, &mut cells);
            style_row(row, &mut cells);
            Arc::new(TerminalRow {
                cells,
                ..TerminalRow::default()
            })
        })
        .collect();
    TerminalSnapshot {
        terminal_id: TerminalId::new(1),
        generation: 1,
        revision: 0,
        geometry_revision: 0,
        size: GridSize::clamped(32, ROWS),
        rows,
        // The translucent block cursor must paint over the "n" of "normal".
        cursor: Some(Cursor {
            row: 24,
            column: 0,
            shape: CursorShape::Block,
        }),
        modes: TerminalModes::default(),
        viewport: Viewport::default(),
        history_size: 0,
        cursor_color: None,
        default_background: None,
    }
}

/// Rows whose cells differ from their fixture text: graphemes and wide
/// cells.
fn populate_row(row: usize, cells: &mut [Cell]) {
    if row == 27 {
        for cell in cells.iter_mut() {
            cell.text = " ".into();
        }
        for (column, text) in [
            (0, "M"),
            (2, "界"),
            (5, "▐\u{0301}"),
            (7, "▕\u{fe0e}"),
            (9, "😀"),
        ] {
            cells[column].text = text.into();
        }
        for column in [2, 9] {
            cells[column].style.wide = true;
            cells[column + 1].style.wide_spacer = true;
        }
    }
    if row == 28 {
        populate_wide(cells);
    }
    if row == ICON_ROW {
        cells[ICON_WIDE].style.wide = true;
        cells[ICON_WIDE + 1].text = " ".into();
        cells[ICON_WIDE + 1].style.wide_spacer = true;
    }
}

fn style_row(row: usize, cells: &mut [Cell]) {
    if row >= 32 {
        for (column, cell) in cells.iter_mut().enumerate() {
            cell.foreground =
                CellColor::Indexed(if column < 6 { 4 } else { 2 });
            cell.background =
                CellColor::Indexed(if column < 6 { 3 } else { 5 });
        }
    }
    if row == 24 {
        for cell in &mut cells[10..] {
            cell.style.bold = true;
        }
    }
    if row == 25 {
        for cell in &mut cells[..10] {
            cell.style.dim = true;
        }
        for cell in &mut cells[16..] {
            cell.style.hidden = true;
        }
    }
}

fn check_wide(panel: &Panel) {
    let renderer = &panel.renderer;
    let glyphs = &renderer.rows[28].glyphs;
    assert_eq!(glyphs.len(), 5, "wide spacer cells must not paint");
    for (column, ch, columns) in [
        (0, '▐', 2),
        (3, '\u{e0b0}', 2),
        (6, '◢', 2),
        (9, '\u{e0b4}', 2),
        (31, '\u{e0b0}', 1),
    ] {
        let glyph = glyphs
            .iter()
            .find(|glyph| glyph.column == column)
            .expect("wide fixture glyph");
        let GlyphContent::Builtin(geometry) = &glyph.content else {
            panic!("wide builtins must bypass font shaping")
        };
        assert!(
            Arc::ptr_eq(geometry, cached_geometry(renderer, ch, columns)),
            "wrong width at column {column}"
        );
    }
}

fn populate_wide(cells: &mut [Cell]) {
    for (column, ch) in [
        (0, '▐'),
        (3, '\u{e0b0}'),
        (6, '◢'),
        (9, '\u{e0b4}'),
        (31, '\u{e0b0}'),
    ] {
        cells[column].text = ch.into();
        cells[column].style.wide = true;
        if column < 31 {
            cells[column + 1].text = "█".into();
            cells[column + 1].style.wide_spacer = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Completion;

    #[test]
    fn completion_fires_once_after_every_panel_finishes() {
        let mut completion = Completion::default();
        for index in 0..3 {
            completion.prepared_and_checked(index);
        }
        assert!(!completion.painted(2));
        assert!(!completion.painted(0));
        assert!(completion.painted(1));
        assert!(!completion.painted(1));
    }
}
