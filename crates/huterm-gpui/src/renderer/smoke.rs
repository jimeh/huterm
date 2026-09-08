//! Native fixture using production row preparation and painting, without PTYs.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Application, Bounds, Context, Render, Window, WindowBounds,
    WindowOptions, canvas, div, point, prelude::*, px, size,
};
use huterm_protocol::{
    BufferPoint, BufferRange, Cell, CellColor, CellStyle, GridSize, TerminalId,
    TerminalModes, TerminalRow, TerminalSnapshot, Viewport,
};

use super::{GlyphContent, GridMetrics, TerminalRenderer};
use crate::config::Theme;

pub(crate) fn run() {
    let hold = std::env::var_os("HUTERM_RENDERER_HOLD").is_some();
    Application::new().run(move |cx| {
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
        if hold {
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
        }
        cx.activate(true);
        if !hold {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                cx.update(|cx| {
                    // A successful process must have exercised prepare and paint.
                    assert!(PAINTED.load(std::sync::atomic::Ordering::Relaxed));
                    println!("RENDERER_SMOKE passed");
                    cx.quit();
                })
                .expect("finish renderer smoke");
            })
            .detach();
        }
    });
}

static PAINTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

struct Panel {
    renderer: TerminalRenderer,
    snapshot: Arc<TerminalSnapshot>,
    checked: bool,
}

struct Fixture {
    panels: Vec<Rc<RefCell<Panel>>>,
}

impl Fixture {
    fn new(window: &Window, cx: &App) -> Self {
        let family = if cfg!(target_os = "macos") {
            "Menlo"
        } else {
            "monospace"
        };
        let panels = [12.0, 16.0, 20.0]
            .into_iter()
            .map(|font_size| {
                let metrics = GridMetrics::resolve(
                    cx.text_system(),
                    family,
                    px(font_size),
                )
                .expect("fixture font")
                .at_scale(window.scale_factor());
                Rc::new(RefCell::new(Panel {
                    renderer: TerminalRenderer::new(
                        family.into(),
                        Theme::default(),
                        metrics,
                    ),
                    snapshot: Arc::new(snapshot(font_size)),
                    checked: false,
                }))
            })
            .collect();
        Self { panels }
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
                let family = panel.renderer.font_family.clone();
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
        for panel in &self.panels {
            let prepare = Rc::clone(panel);
            let paint = Rc::clone(panel);
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
                    },
                    move |bounds, (), window, _| {
                        paint.borrow_mut().renderer.paint(bounds, window);
                        PAINTED
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    },
                )
                .w(m.cell_width * 32.0)
                .h(m.cell_height * 33.0)
                .flex_shrink_0(),
            );
        }
        root
    }
}

fn check(panel: &mut Panel, window: &mut Window) {
    let renderer = &mut panel.renderer;
    assert_eq!(
        renderer.graphics.len(),
        190,
        "186 standalone and four wide builtins must bypass font shaping"
    );
    check_wide(panel);
    let renderer = &mut panel.renderer;
    let first = Arc::clone(&renderer.graphics[&('▐', 1)]);
    // An unchanged snapshot reuses prepared geometry.
    renderer.prepare(Some(&panel.snapshot), window);
    assert!(Arc::ptr_eq(&first, &renderer.graphics[&('▐', 1)]));

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
    renderer.reconfigure(renderer.font_family.clone(), theme, renderer.metrics);
    renderer.prepare(Some(&panel.snapshot), window);
    assert!(
        Arc::ptr_eq(&first, &renderer.graphics[&('▐', 1)]),
        "theme reload reuses color-free geometry"
    );
    let original_metrics = renderer.metrics;
    renderer.reconfigure(
        renderer.font_family.clone(),
        renderer.theme.clone(),
        original_metrics.at_scale(original_metrics.scale_factor + 0.5),
    );
    renderer.prepare(Some(&panel.snapshot), window);
    assert!(
        !Arc::ptr_eq(&first, &renderer.graphics[&('▐', 1)]),
        "display scale invalidates geometry"
    );
    renderer.reconfigure(
        renderer.font_family.clone(),
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
    lines.extend(["\u{e0b0} \u{e0b1} \u{e0b2} \u{e0b3} \u{e0b4} \u{e0b5} \u{e0b6} \u{e0b7} \u{e0b8} \u{e0b9} \u{e0ba} \u{e0bb} \u{e0bc} \u{e0bd} \u{e0be} \u{e0bf}", "\u{e0d2} \u{e0d4} ◢ ◣ ◤ ◥ ◸ ◹ ◺ ◿", "██\u{e0b0}██\u{e0b4}██\u{e0b8}██\u{e0bc}██\u{e0d2}", "██\u{e0b1}██\u{e0b5}██◸██◿██\u{e0d4}"].map(str::to_owned));
    let rows = lines
        .into_iter()
        .enumerate()
        .map(|(row, line)| {
            let mut cells: Vec<_> = line
                .chars()
                .map(|ch| Cell {
                    text: ch.to_string(),
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
            if row == 27 {
                for cell in &mut cells {
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
                populate_wide(&mut cells);
            }
            style_row(row, &mut cells);
            Arc::new(TerminalRow { cells })
        })
        .collect();
    TerminalSnapshot {
        terminal_id: TerminalId::new(1),
        generation: 1,
        size: GridSize::clamped(32, 33),
        rows,
        cursor: None,
        modes: TerminalModes::default(),
        viewport: Viewport::default(),
        history_size: 0,
        cursor_color: None,
    }
}

fn style_row(row: usize, cells: &mut [Cell]) {
    if row >= 31 {
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
            Arc::ptr_eq(geometry, &renderer.graphics[&(ch, columns)]),
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
        cells[column].text = ch.to_string();
        cells[column].style.wide = true;
        if column < 31 {
            cells[column + 1].text = "█".into();
            cells[column + 1].style.wide_spacer = true;
        }
    }
}
