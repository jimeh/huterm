use huterm_protocol::{MouseEncoding, MouseTracking};
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
    BufferPoint, BufferRange, Cell, CellColor, CellStyle, Cursor, CursorShape,
    GridSize, Rgb, TerminalId, TerminalModes, TerminalSnapshot, Viewport,
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

    pub(crate) fn size(&self) -> GridSize {
        GridSize::clamped(
            u16::try_from(self.term.columns()).unwrap_or(u16::MAX),
            u16::try_from(self.term.screen_lines()).unwrap_or(u16::MAX),
        )
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
        let renderable = self.term.renderable_content();

        for row in 0..rows {
            let line = Line(top_line + i32::try_from(row).unwrap_or(i32::MAX));
            for column in 0..columns {
                cells.push(snapshot_cell(
                    &self.term.grid()[line][Column(column)],
                    renderable.colors,
                ));
            }
        }

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
            viewport: Viewport { bottom_offset },
            history_size,
            cursor_color: renderable.colors[NamedColor::Cursor].map(rgb),
        }
    }

    pub(crate) fn extract_text(
        &self,
        generation: u64,
        range: BufferRange,
    ) -> Option<String> {
        if generation != self.generation {
            return None;
        }
        let start = self.buffer_point(range.start)?;
        let end = self.buffer_point(range.end)?;
        (start <= end).then(|| self.term.bounds_to_string(start, end))
    }

    fn buffer_point(
        &self,
        point: BufferPoint,
    ) -> Option<alacritty_terminal::index::Point> {
        let rows = self.term.screen_lines();
        let max_row = self
            .term
            .history_size()
            .saturating_add(rows.saturating_sub(1));
        if point.rows_from_live_bottom > max_row
            || usize::from(point.column) >= self.term.columns()
        {
            return None;
        }
        let bottom = i32::try_from(rows.saturating_sub(1)).ok()?;
        let distance = i32::try_from(point.rows_from_live_bottom).ok()?;
        Some(alacritty_terminal::index::Point::new(
            Line(bottom.saturating_sub(distance)),
            Column(usize::from(point.column)),
        ))
    }

    fn drain_effects(&self) -> Vec<EngineEffect> {
        self.events
            .try_iter()
            .filter_map(|event| match event {
                Event::PtyWrite(text) => {
                    Some(EngineEffect::PtyWrite(text.into_bytes()))
                }
                Event::Title(title) => Some(EngineEffect::Title(title)),
                Event::ResetTitle => Some(EngineEffect::Title("Huterm".into())),
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

    let mut foreground = resolve_color(cell.fg, colors);
    let mut background = resolve_color(cell.bg, colors);
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
            italic: cell.flags.contains(Flags::ITALIC),
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
) -> CellColor {
    match color {
        Color::Spec(value) => CellColor::Rgb(rgb(value)),
        Color::Indexed(index) => colors[usize::from(index)]
            .map(rgb)
            .map_or(CellColor::Indexed(index), CellColor::Rgb),
        Color::Named(named) => colors[named]
            .map(rgb)
            .map_or_else(|| named_color(named), CellColor::Rgb),
    }
}

fn rgb(value: AlacrittyRgb) -> Rgb {
    Rgb {
        red: value.r,
        green: value.g,
        blue: value.b,
    }
}

fn named_color(named: NamedColor) -> CellColor {
    match named {
        NamedColor::Black | NamedColor::DimBlack => CellColor::Indexed(0),
        NamedColor::Red | NamedColor::DimRed => CellColor::Indexed(1),
        NamedColor::Green | NamedColor::DimGreen => CellColor::Indexed(2),
        NamedColor::Yellow | NamedColor::DimYellow => CellColor::Indexed(3),
        NamedColor::Blue | NamedColor::DimBlue => CellColor::Indexed(4),
        NamedColor::Magenta | NamedColor::DimMagenta => CellColor::Indexed(5),
        NamedColor::Cyan | NamedColor::DimCyan => CellColor::Indexed(6),
        NamedColor::White | NamedColor::DimWhite => CellColor::Indexed(7),
        NamedColor::BrightBlack => CellColor::Indexed(8),
        NamedColor::BrightRed => CellColor::Indexed(9),
        NamedColor::BrightGreen => CellColor::Indexed(10),
        NamedColor::BrightYellow => CellColor::Indexed(11),
        NamedColor::BrightBlue => CellColor::Indexed(12),
        NamedColor::BrightMagenta => CellColor::Indexed(13),
        NamedColor::BrightCyan => CellColor::Indexed(14),
        NamedColor::BrightWhite => CellColor::Indexed(15),
        NamedColor::Foreground
        | NamedColor::BrightForeground
        | NamedColor::DimForeground => CellColor::DefaultForeground,
        NamedColor::Background => CellColor::DefaultBackground,
        NamedColor::Cursor => CellColor::Cursor,
    }
}

fn modes(mode: TermMode) -> TerminalModes {
    TerminalModes {
        mouse_tracking: if mode.contains(TermMode::MOUSE_MOTION) {
            MouseTracking::AllMotion
        } else if mode.contains(TermMode::MOUSE_DRAG) {
            MouseTracking::ButtonMotion
        } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseTracking::Buttons
        } else {
            MouseTracking::Disabled
        },
        mouse_encoding: if mode.contains(TermMode::SGR_MOUSE) {
            MouseEncoding::Sgr
        } else if mode.contains(TermMode::UTF8_MOUSE) {
            MouseEncoding::Utf8
        } else {
            MouseEncoding::Legacy
        },
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
    fn mouse_modes_follow_parser_including_inactive_reset_and_terminal_reset() {
        let mut engine = engine();
        let mut generation = 0;
        for (sequence, tracking, encoding) in [
            ("\x1b[?1006h", MouseTracking::Disabled, MouseEncoding::Sgr),
            ("\x1b[?1000h", MouseTracking::Buttons, MouseEncoding::Sgr),
            (
                "\x1b[?1002h",
                MouseTracking::ButtonMotion,
                MouseEncoding::Sgr,
            ),
            ("\x1b[?1003h", MouseTracking::AllMotion, MouseEncoding::Sgr),
            ("\x1b[?1000l", MouseTracking::AllMotion, MouseEncoding::Sgr),
            ("\x1b[?1002l", MouseTracking::AllMotion, MouseEncoding::Sgr),
            ("\x1b[?1005h", MouseTracking::AllMotion, MouseEncoding::Utf8),
            ("\x1b[?1006l", MouseTracking::AllMotion, MouseEncoding::Utf8),
            (
                "\x1b[?1005l",
                MouseTracking::AllMotion,
                MouseEncoding::Legacy,
            ),
            (
                "\x1b[?1003l",
                MouseTracking::Disabled,
                MouseEncoding::Legacy,
            ),
            (
                "\x1b[?1002h\x1b[?1006h\x1bc",
                MouseTracking::Disabled,
                MouseEncoding::Legacy,
            ),
        ] {
            engine.process(sequence.as_bytes());
            let snapshot = engine.snapshot(Viewport::default());
            assert!(snapshot.generation > generation);
            generation = snapshot.generation;
            assert_eq!(
                (snapshot.modes.mouse_tracking, snapshot.modes.mouse_encoding),
                (tracking, encoding),
                "{sequence:?}"
            );
        }
    }

    #[test]
    fn queued_mouse_uses_current_modes_dimensions_and_format_for_each_event() {
        use huterm_protocol::{
            Modifiers, MouseAction, MouseButton, MouseInput, MousePosition,
            TerminalInput,
        };
        let mut engine = engine();
        let input = |action| {
            TerminalInput::Mouse(MouseInput {
                action,
                position: MousePosition { column: 7, row: 2 },
                modifiers: Modifiers::default(),
            })
        };
        let encode = |engine: &TerminalEngine, action| {
            crate::input::encode_input(
                &input(action),
                engine.modes(),
                engine.size(),
            )
        };
        engine.process(b"\x1b[?1002h\x1b[?1006h");
        assert_eq!(
            encode(&engine, MouseAction::Press(MouseButton::Right)),
            b"\x1b[<2;8;3M"
        );
        engine.resize(GridSize::clamped(2, 1));
        engine.process(b"\x1b[?1006l");
        assert_eq!(
            encode(&engine, MouseAction::Release(MouseButton::Right)),
            b"\x1b[M#\"!"
        );
        engine.process(b"\x1b[?1002l");
        assert!(
            encode(&engine, MouseAction::Motion(Some(MouseButton::Right)))
                .is_empty()
        );
        assert!(
            encode(&engine, MouseAction::Release(MouseButton::Right))
                .is_empty()
        );
        engine.process(b"\x1b[?1002h\x1b[?1006h");
        assert_eq!(
            encode(&engine, MouseAction::Motion(Some(MouseButton::Right))),
            b"\x1b[<34;2;1M"
        );
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
                first.style.italic,
                first.style.underline,
                first.foreground,
                wide.text.as_str(),
                wide.style.wide,
                spacer.style.wide_spacer,
            ),
            (
                "A",
                true,
                false,
                true,
                CellColor::Indexed(1),
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

        let clamped = engine.snapshot(Viewport {
            bottom_offset: usize::MAX,
        });
        assert_eq!(clamped.viewport.bottom_offset, live.history_size);
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

    #[test]
    fn dynamic_default_color_overrides_remain_explicit() {
        let mut engine = engine();
        engine.process(b"\x1b]10;#112233\x07A");

        let snapshot = engine.snapshot(Viewport::default());
        assert_eq!(
            snapshot.cells[0].foreground,
            CellColor::Rgb(Rgb {
                red: 0x11,
                green: 0x22,
                blue: 0x33,
            })
        );
    }

    #[test]
    fn snapshot_preserves_defaults_true_color_indexed_and_reverse_video() {
        let mut engine = engine();
        engine.process(b"D\x1b[38;2;1;2;3;48;5;4;7mR");

        let snapshot = engine.snapshot(Viewport::default());
        assert_eq!(snapshot.cells[0].foreground, CellColor::DefaultForeground);
        assert_eq!(snapshot.cells[0].background, CellColor::DefaultBackground);
        assert_eq!(snapshot.cells[1].foreground, CellColor::Indexed(4));
        assert_eq!(
            snapshot.cells[1].background,
            CellColor::Rgb(Rgb {
                red: 1,
                green: 2,
                blue: 3,
            })
        );
    }

    #[test]
    fn selection_extracts_hard_broken_lines_and_rejects_stale_ranges() {
        let mut engine = engine();
        engine.process(b"alpha\r\nbeta");
        let generation = engine.generation();
        let range = BufferRange::ordered(
            BufferPoint {
                rows_from_live_bottom: 2,
                column: 0,
            },
            BufferPoint {
                rows_from_live_bottom: 1,
                column: 3,
            },
        );

        assert_eq!(
            engine.extract_text(generation, range).as_deref(),
            Some("alpha\nbeta")
        );
        assert_eq!(engine.extract_text(generation + 1, range), None);
        assert_eq!(
            engine.extract_text(
                generation,
                BufferRange::ordered(
                    BufferPoint {
                        rows_from_live_bottom: usize::MAX,
                        column: 0,
                    },
                    range.end,
                ),
            ),
            None
        );
    }

    #[test]
    fn selection_preserves_soft_wraps_combining_marks_and_wide_cells() {
        let mut wrapped = engine();
        wrapped.process("abcdefghij".as_bytes());
        let wrapped_range = BufferRange::ordered(
            BufferPoint {
                rows_from_live_bottom: 2,
                column: 0,
            },
            BufferPoint {
                rows_from_live_bottom: 1,
                column: 1,
            },
        );
        assert_eq!(
            wrapped
                .extract_text(wrapped.generation(), wrapped_range)
                .as_deref(),
            Some("abcdefghij")
        );

        let mut unicode = engine();
        unicode.process("e\u{301}界".as_bytes());
        let unicode_range = BufferRange::ordered(
            BufferPoint {
                rows_from_live_bottom: 2,
                column: 0,
            },
            BufferPoint {
                rows_from_live_bottom: 2,
                column: 2,
            },
        );
        assert_eq!(
            unicode
                .extract_text(unicode.generation(), unicode_range)
                .as_deref(),
            Some("e\u{301}界")
        );
    }
}
