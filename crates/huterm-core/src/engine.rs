use std::sync::mpsc::{self, Receiver, Sender};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{
    Color, CursorShape as AlacrittyCursorShape, NamedColor,
};
use alacritty_terminal::vte::ansi::{Processor, Rgb as AlacrittyRgb};
use huterm_protocol::{
    Cell, CellStyle, Cursor, CursorShape, GridSize, Rgb, TerminalId,
    TerminalModes, TerminalSnapshot, Viewport,
};

#[derive(Clone, Debug)]
struct EventProxy(Sender<Event>);

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

#[derive(Debug)]
pub(crate) enum EngineEffect {
    PtyWrite(Vec<u8>),
    Title(String),
    Bell,
}

pub(crate) struct TerminalEngine {
    terminal_id: TerminalId,
    generation: u64,
    parser: Processor,
    term: Term<EventProxy>,
    events: Receiver<Event>,
}

impl std::fmt::Debug for TerminalEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalEngine")
            .field("terminal_id", &self.terminal_id)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl TerminalEngine {
    pub(crate) fn new(terminal_id: TerminalId, size: GridSize) -> Self {
        let (sender, events) = mpsc::channel();
        let dimensions = EngineDimensions::from(size);
        let config = Config {
            scrolling_history: 10_000,
            ..Config::default()
        };
        Self {
            terminal_id,
            generation: 0,
            parser: Processor::new(),
            term: Term::new(config, &dimensions, EventProxy(sender)),
            events,
        }
    }

    pub(crate) fn process(&mut self, bytes: &[u8]) -> Vec<EngineEffect> {
        self.parser.advance(&mut self.term, bytes);
        self.generation = self.generation.saturating_add(1);
        self.drain_effects()
    }

    pub(crate) fn resize(&mut self, size: GridSize) {
        self.term.resize(EngineDimensions::from(size));
        self.generation = self.generation.saturating_add(1);
    }

    pub(crate) fn modes(&self) -> TerminalModes {
        modes(*self.term.mode())
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn snapshot(&self, viewport: Viewport) -> TerminalSnapshot {
        let columns = self.term.columns();
        let rows = self.term.screen_lines();
        let history_size = self.term.history_size();
        let bottom_offset = viewport.bottom_offset.min(history_size);
        let top_line = -i32::try_from(bottom_offset).unwrap_or(i32::MAX);
        let mut cells = Vec::with_capacity(rows.saturating_mul(columns));

        for row in 0..rows {
            let line = Line(top_line + i32::try_from(row).unwrap_or(i32::MAX));
            for column in 0..columns {
                cells.push(snapshot_cell(
                    &self.term.grid()[line][Column(column)],
                    self.term.renderable_content().colors,
                ));
            }
        }

        let renderable = self.term.renderable_content();
        let cursor = (bottom_offset == 0).then(|| Cursor {
            row: u16::try_from(renderable.cursor.point.line.0)
                .unwrap_or_default(),
            column: u16::try_from(renderable.cursor.point.column.0)
                .unwrap_or_default(),
            shape: cursor_shape(renderable.cursor.shape),
        });

        TerminalSnapshot {
            terminal_id: self.terminal_id,
            generation: self.generation,
            size: GridSize::clamped(
                u16::try_from(columns).unwrap_or(u16::MAX),
                u16::try_from(rows).unwrap_or(u16::MAX),
            ),
            cells,
            cursor,
            modes: modes(renderable.mode),
            history_size,
        }
    }

    fn drain_effects(&self) -> Vec<EngineEffect> {
        self.events
            .try_iter()
            .filter_map(|event| match event {
                Event::PtyWrite(text) => {
                    Some(EngineEffect::PtyWrite(text.into_bytes()))
                }
                Event::Title(title) => Some(EngineEffect::Title(title)),
                Event::ResetTitle => Some(EngineEffect::Title("HUTerm".into())),
                Event::Bell => Some(EngineEffect::Bell),
                _ => None,
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug)]
struct EngineDimensions {
    columns: usize,
    rows: usize,
}

impl From<GridSize> for EngineDimensions {
    fn from(size: GridSize) -> Self {
        let size = GridSize::clamped(size.columns, size.rows);
        Self {
            columns: usize::from(size.columns),
            rows: usize::from(size.rows),
        }
    }
}

impl Dimensions for EngineDimensions {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

fn snapshot_cell(
    cell: &alacritty_terminal::term::cell::Cell,
    colors: &alacritty_terminal::term::color::Colors,
) -> Cell {
    let mut text = String::from(cell.c);
    if let Some(combining) = cell.zerowidth() {
        text.extend(combining);
    }

    let mut foreground = resolve_color(cell.fg, colors, true);
    let mut background = resolve_color(cell.bg, colors, false);
    if cell.flags.contains(Flags::INVERSE) {
        std::mem::swap(&mut foreground, &mut background);
    }

    Cell {
        text,
        foreground,
        background,
        style: CellStyle {
            bold: cell.flags.contains(Flags::BOLD),
            dim: cell.flags.contains(Flags::DIM),
            italic: cell.flags.intersects(Flags::ITALIC | Flags::BOLD_ITALIC),
            underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
            strikeout: cell.flags.contains(Flags::STRIKEOUT),
            hidden: cell.flags.contains(Flags::HIDDEN),
            wide: cell.flags.contains(Flags::WIDE_CHAR),
            wide_spacer: cell.flags.contains(Flags::WIDE_CHAR_SPACER),
        },
    }
}

fn resolve_color(
    color: Color,
    colors: &alacritty_terminal::term::color::Colors,
    foreground: bool,
) -> Rgb {
    let rgb = match color {
        Color::Spec(rgb) => rgb,
        Color::Indexed(index) => indexed_color(index),
        Color::Named(named) => {
            colors[named].unwrap_or_else(|| named_color(named, foreground))
        }
    };
    Rgb {
        red: rgb.r,
        green: rgb.g,
        blue: rgb.b,
    }
}

fn named_color(named: NamedColor, foreground: bool) -> AlacrittyRgb {
    const ANSI: [AlacrittyRgb; 16] = [
        AlacrittyRgb {
            r: 0x1d,
            g: 0x1f,
            b: 0x21,
        },
        AlacrittyRgb {
            r: 0xcc,
            g: 0x66,
            b: 0x66,
        },
        AlacrittyRgb {
            r: 0xb5,
            g: 0xbd,
            b: 0x68,
        },
        AlacrittyRgb {
            r: 0xf0,
            g: 0xc6,
            b: 0x74,
        },
        AlacrittyRgb {
            r: 0x81,
            g: 0xa2,
            b: 0xbe,
        },
        AlacrittyRgb {
            r: 0xb2,
            g: 0x94,
            b: 0xbb,
        },
        AlacrittyRgb {
            r: 0x8a,
            g: 0xbe,
            b: 0xb7,
        },
        AlacrittyRgb {
            r: 0xc5,
            g: 0xc8,
            b: 0xc6,
        },
        AlacrittyRgb {
            r: 0x66,
            g: 0x66,
            b: 0x66,
        },
        AlacrittyRgb {
            r: 0xd5,
            g: 0x4e,
            b: 0x53,
        },
        AlacrittyRgb {
            r: 0xb9,
            g: 0xca,
            b: 0x4a,
        },
        AlacrittyRgb {
            r: 0xe7,
            g: 0xc5,
            b: 0x47,
        },
        AlacrittyRgb {
            r: 0x7a,
            g: 0xa6,
            b: 0xda,
        },
        AlacrittyRgb {
            r: 0xc3,
            g: 0x97,
            b: 0xd8,
        },
        AlacrittyRgb {
            r: 0x70,
            g: 0xc0,
            b: 0xb1,
        },
        AlacrittyRgb {
            r: 0xea,
            g: 0xea,
            b: 0xea,
        },
    ];

    let index = named as usize;
    if index < ANSI.len() {
        ANSI[index]
    } else if foreground {
        AlacrittyRgb {
            r: 0xc5,
            g: 0xc8,
            b: 0xc6,
        }
    } else {
        AlacrittyRgb {
            r: 0x1d,
            g: 0x1f,
            b: 0x21,
        }
    }
}

fn indexed_color(index: u8) -> AlacrittyRgb {
    if index < 16 {
        return named_color(
            match index {
                0 => NamedColor::Black,
                1 => NamedColor::Red,
                2 => NamedColor::Green,
                3 => NamedColor::Yellow,
                4 => NamedColor::Blue,
                5 => NamedColor::Magenta,
                6 => NamedColor::Cyan,
                7 => NamedColor::White,
                8 => NamedColor::BrightBlack,
                9 => NamedColor::BrightRed,
                10 => NamedColor::BrightGreen,
                11 => NamedColor::BrightYellow,
                12 => NamedColor::BrightBlue,
                13 => NamedColor::BrightMagenta,
                14 => NamedColor::BrightCyan,
                _ => NamedColor::BrightWhite,
            },
            true,
        );
    }
    if index >= 232 {
        let level =
            8_u8.saturating_add(index.saturating_sub(232).saturating_mul(10));
        return AlacrittyRgb {
            r: level,
            g: level,
            b: level,
        };
    }
    let value = index - 16;
    let channel = |component: u8| {
        if component == 0 {
            0
        } else {
            55 + component * 40
        }
    };
    AlacrittyRgb {
        r: channel(value / 36),
        g: channel((value / 6) % 6),
        b: channel(value % 6),
    }
}

fn modes(mode: TermMode) -> TerminalModes {
    TerminalModes {
        application_cursor: mode.contains(TermMode::APP_CURSOR),
        alternate_screen: mode.contains(TermMode::ALT_SCREEN),
        bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
        focus_reporting: mode.contains(TermMode::FOCUS_IN_OUT),
    }
}

fn cursor_shape(shape: AlacrittyCursorShape) -> CursorShape {
    match shape {
        AlacrittyCursorShape::Block | AlacrittyCursorShape::HollowBlock => {
            CursorShape::Block
        }
        AlacrittyCursorShape::Underline => CursorShape::Underline,
        AlacrittyCursorShape::Beam => CursorShape::Beam,
        AlacrittyCursorShape::Hidden => CursorShape::Hidden,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> TerminalEngine {
        TerminalEngine::new(TerminalId::new(1), GridSize::clamped(8, 3))
    }

    #[test]
    fn snapshot_should_parse_color_style_and_wide_cells() {
        let mut engine = engine();
        engine.process("\x1b[31;1;4mA界".as_bytes());

        let snapshot = engine.snapshot(Viewport::default());
        let first = &snapshot.cells[0];
        let wide = &snapshot.cells[1];
        let spacer = &snapshot.cells[2];

        assert_eq!(
            (
                first.text.as_str(),
                first.style.bold,
                first.style.underline,
                first.foreground,
                wide.text.as_str(),
                wide.style.wide,
                spacer.style.wide_spacer,
            ),
            (
                "A",
                true,
                true,
                Rgb {
                    red: 0xcc,
                    green: 0x66,
                    blue: 0x66
                },
                "界",
                true,
                true,
            )
        );
    }

    #[test]
    fn snapshot_should_report_alternate_screen_mode() {
        let mut engine = engine();
        engine.process(b"primary\x1b[?1049halternate");

        let alternate = engine.snapshot(Viewport::default());
        engine.process(b"\x1b[?1049l");
        let primary = engine.snapshot(Viewport::default());

        assert_eq!(
            (
                alternate.modes.alternate_screen,
                primary.modes.alternate_screen
            ),
            (true, false)
        );
    }

    #[test]
    fn snapshot_should_read_scrollback_without_mutating_live_view() {
        let mut engine = engine();
        engine.process(b"one\r\ntwo\r\nthree\r\nfour");

        let live = engine.snapshot(Viewport::default());
        let scrolled = engine.snapshot(Viewport { bottom_offset: 1 });
        let live_again = engine.snapshot(Viewport::default());

        assert!(live.history_size > 0);
        assert_ne!(scrolled.cells, live.cells);
        assert_eq!(live_again, live);
        assert!(live.cursor.is_some());
        assert!(scrolled.cursor.is_none());
    }

    #[test]
    fn resize_should_advance_generation_and_change_snapshot_size() {
        let mut engine = engine();
        engine.process(b"hello");
        let before = engine.generation();

        engine.resize(GridSize::clamped(12, 4));
        let snapshot = engine.snapshot(Viewport::default());

        assert_eq!(
            (snapshot.generation, snapshot.size),
            (before + 1, GridSize::clamped(12, 4))
        );
    }
}
