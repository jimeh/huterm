use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use super::EngineEffect;
use crate::terminal::RuntimeError;
use huterm_protocol::{
    BufferRange, Cell, CellColor, CellSize, CellStyle, Cursor, CursorShape,
    GridSize, MouseEncoding, MouseTracking, Rgb, ScrollCommand, TerminalId,
    TerminalModes, TerminalRow, TerminalSnapshot, Viewport,
};
use libghostty_vt::fmt::{Format, Formatter, FormatterOptions};
use libghostty_vt::render::{
    CellIterator, CursorVisualStyle, Dirty, RowIterator,
};
use libghostty_vt::screen::{CellContentTag, CellWide, Screen};
use libghostty_vt::selection::Selection;
use libghostty_vt::style::{RgbColor, StyleColor, Underline};
use libghostty_vt::terminal::{
    ConformanceLevel, DeviceAttributeFeature, DeviceAttributes, DeviceType,
    Mode, Point, PointCoordinate, PrimaryDeviceAttributes, ScrollViewport,
    SecondaryDeviceAttributes, TertiaryDeviceAttributes,
};
use libghostty_vt::{RenderState, Terminal, TerminalOptions};

impl From<libghostty_vt::Error> for RuntimeError {
    fn from(error: libghostty_vt::Error) -> Self {
        Self::Engine(error.to_string())
    }
}

#[derive(Debug)]
pub(crate) struct TerminalEngine {
    terminal_id: TerminalId,
    generation: u64,
    size: GridSize,
    terminal: Terminal<'static, 'static>,
    render: RenderState<'static>,
    row_iterator: RowIterator<'static>,
    cell_iterator: CellIterator<'static>,
    retained_rows: Vec<Arc<TerminalRow>>,
    colors: Option<(Option<RgbColor>, Option<RgbColor>, [RgbColor; 256])>,
    effects: Rc<RefCell<Vec<EngineEffect>>>,
    title_dirty: Rc<std::cell::Cell<bool>>,
    palette_overrides: [bool; 256],
    palette_dirty: bool,
    escape_hint: EscapeHint,
    mouse_probe: RefCell<(
        libghostty_vt::mouse::Encoder<'static>,
        libghostty_vt::mouse::Event<'static>,
    )>,
}

impl TerminalEngine {
    pub(super) fn new(
        terminal_id: TerminalId,
        size: GridSize,
        cell: CellSize,
    ) -> Result<Self, RuntimeError> {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: size.columns,
            rows: size.rows,
            max_scrollback: 16 * 1024 * 1024,
        })?;
        terminal.set_glyph_protocol_enabled(false)?;
        terminal.set_apc_max_bytes(Some(0))?;
        let effects = Rc::new(RefCell::new(Vec::new()));
        let write_effects = Rc::clone(&effects);
        terminal.on_pty_write(move |_, bytes| {
            if bytes.starts_with(b"\x1b_G")
                || (bytes.starts_with(b"\x1b[?") && bytes.ends_with(b"u"))
            {
                return;
            }
            write_effects
                .borrow_mut()
                .push(EngineEffect::PtyWrite(bytes.to_vec()));
        })?;
        let bell_effects = Rc::clone(&effects);
        terminal.on_bell(move |_| {
            bell_effects.borrow_mut().push(EngineEffect::Bell);
        })?;
        let title_dirty = Rc::new(std::cell::Cell::new(false));
        let title_changed = Rc::clone(&title_dirty);
        terminal.on_title_changed(move |_| title_changed.set(true))?;
        // Huterm does not implement Ghostty's image or keyboard extensions.
        terminal.on_device_attributes(|_| {
            Some(DeviceAttributes {
                primary: PrimaryDeviceAttributes::new(
                    ConformanceLevel::VT220,
                    &[DeviceAttributeFeature::ANSI_COLOR],
                ),
                secondary: SecondaryDeviceAttributes {
                    device_type: DeviceType::VT220,
                    firmware_version: 0,
                    rom_cartridge: 0,
                },
                tertiary: TertiaryDeviceAttributes { unit_id: 0 },
            })
        })?;
        terminal.on_xtversion(|_| Some("Huterm"))?;
        terminal.resize(
            size.columns,
            size.rows,
            u32::from(cell.width),
            u32::from(cell.height),
        )?;
        Ok(Self {
            terminal_id,
            generation: 0,
            size,
            terminal,
            render: RenderState::new()?,
            row_iterator: RowIterator::new()?,
            cell_iterator: CellIterator::new()?,
            retained_rows: Vec::new(),
            colors: None,
            effects,
            title_dirty,
            palette_overrides: [false; 256],
            palette_dirty: false,
            escape_hint: EscapeHint::Ground,
            mouse_probe: RefCell::new((
                libghostty_vt::mouse::Encoder::new()?,
                libghostty_vt::mouse::Event::new()?,
            )),
        })
    }

    pub(super) fn process(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<EngineEffect>, RuntimeError> {
        self.palette_dirty |= self.escape_hint.observe(bytes);
        self.terminal.vt_write(bytes);
        self.generation = self.generation.saturating_add(1);
        self.drain_effects()
    }

    fn drain_effects(&self) -> Result<Vec<EngineEffect>, RuntimeError> {
        let mut effects = self.effects.borrow_mut();
        if self.title_dirty.replace(false) {
            effects
                .push(EngineEffect::Title(self.terminal.title()?.to_owned()));
        }
        Ok(std::mem::take(&mut *effects))
    }

    pub(super) fn resize(
        &mut self,
        size: GridSize,
        cell: CellSize,
    ) -> Result<Vec<EngineEffect>, RuntimeError> {
        self.terminal.resize(
            size.columns,
            size.rows,
            u32::from(cell.width),
            u32::from(cell.height),
        )?;
        self.size = size;
        self.generation = self.generation.saturating_add(1);
        self.drain_effects()
    }
    pub(super) fn size(&self) -> GridSize {
        self.size
    }
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn modes(&self) -> Result<TerminalModes, RuntimeError> {
        let mode = |mode| self.terminal.mode(mode);
        let (tracking, encoding) = self.mouse_modes()?;
        Ok(TerminalModes {
            application_cursor: mode(Mode::DECCKM)?,
            alternate_screen: self.terminal.active_screen()?
                == Screen::Alternate,
            bracketed_paste: mode(Mode::BRACKETED_PASTE)?,
            focus_reporting: mode(Mode::FOCUS_EVENT)?,
            mouse_tracking: tracking,
            mouse_encoding: encoding,
        })
    }

    fn mouse_modes(
        &self,
    ) -> Result<(MouseTracking, MouseEncoding), RuntimeError> {
        use libghostty_vt::mouse::{
            Action, Button, EncoderSize, Position, TrackingMode,
        };
        let mut probe = self.mouse_probe.borrow_mut();
        let (encoder, event) = &mut *probe;
        // Native mode bits are independent; only the mouse encoder sees the
        // actual last-selected format/tracking flags. Probe locally with fixed
        // geometry so UTF-8 remains distinguishable even in a 1x1 terminal.
        encoder
            .set_options_from_terminal(&self.terminal)
            .set_size(EncoderSize {
                screen_width: 200,
                screen_height: 200,
                cell_width: 1,
                cell_height: 1,
                padding_top: 0,
                padding_bottom: 0,
                padding_left: 0,
                padding_right: 0,
            })
            .set_track_last_cell(false)
            .set_any_button_pressed(false);
        event
            .set_position(Position { x: 100.0, y: 100.0 })
            .set_action(Action::Motion)
            .set_button(None);
        let mut buffer = [0; 64];
        let tracking = if encoder.encode(event, &mut buffer)? > 0 {
            MouseTracking::AllMotion
        } else {
            encoder.set_any_button_pressed(true);
            event.set_button(Some(Button::Left));
            if encoder.encode(event, &mut buffer)? > 0 {
                MouseTracking::ButtonMotion
            } else if self.terminal.is_mouse_tracking()? {
                MouseTracking::Buttons
            } else {
                MouseTracking::Disabled
            }
        };
        encoder
            .set_tracking_mode(TrackingMode::Any)
            .set_any_button_pressed(false);
        event
            .set_action(Action::Press)
            .set_button(Some(Button::Left));
        let count = encoder.encode(event, &mut buffer)?;
        let encoding = if buffer[..count].starts_with(b"\x1b[<") {
            MouseEncoding::Sgr
        } else if count > 6 && buffer[..count].starts_with(b"\x1b[M") {
            MouseEncoding::Utf8
        } else {
            MouseEncoding::Legacy
        };
        Ok((tracking, encoding))
    }

    pub(super) fn scroll(
        &mut self,
        command: ScrollCommand,
    ) -> Result<(), RuntimeError> {
        let scroll =
            match command {
                ScrollCommand::Relative(rows) => ScrollViewport::Delta(
                    isize::try_from(rows.saturating_neg()).unwrap_or(
                        if rows < 0 { isize::MAX } else { isize::MIN },
                    ),
                ),
                ScrollCommand::Absolute(offset) => ScrollViewport::Row(
                    self.terminal.scrollback_rows()?.saturating_sub(offset),
                ),
                ScrollCommand::Live => ScrollViewport::Bottom,
            };
        self.terminal.scroll_viewport(scroll);
        Ok(())
    }

    fn refresh_palette_overrides(&mut self) -> Result<(), RuntimeError> {
        if !self.palette_dirty {
            return Ok(());
        }
        let original = self.terminal.default_color_palette()?;
        let before = self.terminal.color_palette()?;
        let mut probe = original;
        for color in &mut probe.0 {
            color.r ^= 1;
        }
        self.terminal.set_default_color_palette(Some(probe))?;
        let probed = self.terminal.color_palette();
        // Always restore the palette, including a failed effective-color read.
        self.terminal.set_default_color_palette(Some(original))?;
        let probed = probed?;
        for (index, overridden) in self.palette_overrides.iter_mut().enumerate()
        {
            *overridden = probed.0[index] == before.0[index];
        }
        self.palette_dirty = false;
        self.colors = None;
        Ok(())
    }

    pub(super) fn viewport_state(
        &self,
    ) -> Result<(usize, usize), RuntimeError> {
        let scrollbar = self.terminal.scrollbar()?;
        let history_size = self.terminal.scrollback_rows()?;
        let bottom_offset = usize::try_from(
            scrollbar
                .total
                .saturating_sub(scrollbar.len)
                .saturating_sub(scrollbar.offset),
        )
        .unwrap_or(usize::MAX);
        Ok((bottom_offset, history_size))
    }

    pub(super) fn snapshot(
        &mut self,
    ) -> Result<TerminalSnapshot, RuntimeError> {
        self.refresh_palette_overrides()?;
        let modes = self.modes()?;
        let (bottom_offset, history_size) = self.viewport_state()?;
        let fg = self.terminal.fg_color()?;
        let bg = self.terminal.bg_color()?;
        let palette = self.terminal.color_palette()?;

        let state = self.render.update(&self.terminal)?;
        let colors = (fg, bg, palette.0);
        let full = self.colors.as_ref() != Some(&colors)
            || state.dirty()? == Dirty::Full
            || self.retained_rows.len() != usize::from(self.size.rows)
            || self.retained_rows.first().is_some_and(|row| {
                row.cells.len() != usize::from(self.size.columns)
            });
        let mut rows = self.row_iterator.update(&state)?;
        let mut index = 0;
        while let Some(row) = rows.next() {
            if full || row.dirty()? {
                let mut cells = Vec::new();
                cells
                    .try_reserve_exact(usize::from(self.size.columns))
                    .map_err(|error| RuntimeError::Engine(error.to_string()))?;
                let mut iter = self.cell_iterator.update(row)?;
                while let Some(cell) = iter.next() {
                    cells.push(snapshot_cell(
                        cell,
                        fg,
                        bg,
                        &palette,
                        &self.palette_overrides,
                    )?);
                }
                let owned = Arc::new(TerminalRow { cells });
                if index < self.retained_rows.len() {
                    self.retained_rows[index] = owned;
                } else {
                    self.retained_rows.push(owned);
                }
            }
            index += 1;
        }
        self.retained_rows.truncate(index);
        let visible = state.cursor_visible()?;
        let shape = if visible {
            match state.cursor_visual_style()? {
                CursorVisualStyle::Bar => CursorShape::Beam,
                CursorVisualStyle::Underline => CursorShape::Underline,
                _ => CursorShape::Block,
            }
        } else {
            CursorShape::Hidden
        };
        let cursor = state.cursor_viewport()?.map(|cursor| Cursor {
            row: cursor.y,
            column: cursor.x,
            shape,
        });
        let snapshot = TerminalSnapshot {
            terminal_id: self.terminal_id,
            generation: self.generation,
            size: self.size,
            rows: self.retained_rows.clone(),
            cursor,
            modes,
            viewport: Viewport { bottom_offset },
            history_size,
            cursor_color: state.cursor_color()?.map(rgb),
        };
        let mut rows = self.row_iterator.update(&state)?;
        while let Some(row) = rows.next() {
            row.set_dirty(false)?;
        }
        state.set_dirty(Dirty::Clean)?;
        self.colors = Some(colors);
        Ok(snapshot)
    }

    pub(super) fn extract_text(
        &self,
        generation: u64,
        range: BufferRange,
    ) -> Result<Option<String>, RuntimeError> {
        if generation != self.generation
            || BufferRange::ordered(range.start, range.end) != range
        {
            return Ok(None);
        }
        let total = self.terminal.total_rows()?;
        let point = |point: huterm_protocol::BufferPoint| {
            if usize::from(point.column) >= usize::from(self.size.columns) {
                return None;
            }
            Some(Point::Screen(PointCoordinate {
                x: point.column,
                y: u32::try_from(
                    total
                        .checked_sub(1)?
                        .checked_sub(point.rows_from_live_bottom)?,
                )
                .ok()?,
            }))
        };
        let (Some(start), Some(end)) = (point(range.start), point(range.end))
        else {
            return Ok(None);
        };
        let selection = Selection::new(
            self.terminal.grid_ref(start)?,
            self.terminal.grid_ref(end)?,
            false,
        );
        let mut formatter = Formatter::new(
            &self.terminal,
            FormatterOptions::new()
                .with_format(Format::Plain)
                .with_unwrap(true)
                .with_trim(true)
                .with_selection(&selection),
        )?;
        let bytes = formatter.format_alloc(None)?;
        Ok(Some(String::from_utf8(bytes.to_vec()).map_err(
            |error| RuntimeError::Engine(error.to_string()),
        )?))
    }
}

fn rgb(color: RgbColor) -> Rgb {
    Rgb {
        red: color.r,
        green: color.g,
        blue: color.b,
    }
}

fn snapshot_cell(
    cell: &libghostty_vt::render::CellIteration<'_, '_>,
    fg: Option<RgbColor>,
    bg: Option<RgbColor>,
    palette: &libghostty_vt::style::Palette,
    overrides: &[bool; 256],
) -> Result<Cell, RuntimeError> {
    let raw = cell.raw_cell()?;
    let style = cell.style()?;
    let resolve =
        |color, default, override_color: Option<RgbColor>| match color {
            StyleColor::None => override_color
                .map_or(default, |color| CellColor::Rgb(rgb(color))),
            StyleColor::Rgb(color) => CellColor::Rgb(rgb(color)),
            StyleColor::Palette(index) => {
                if overrides[usize::from(index.0)] {
                    CellColor::Rgb(rgb(palette.get(index)))
                } else {
                    CellColor::Indexed(index.0)
                }
            }
        };
    let mut foreground =
        resolve(style.fg_color, CellColor::DefaultForeground, fg);
    let background_style = match raw.content_tag()? {
        CellContentTag::BgColorPalette => {
            StyleColor::Palette(raw.bg_color_palette()?)
        }
        CellContentTag::BgColorRgb => StyleColor::Rgb(raw.bg_color_rgb()?),
        _ => style.bg_color,
    };
    let mut background =
        resolve(background_style, CellColor::DefaultBackground, bg);
    if style.inverse {
        std::mem::swap(&mut foreground, &mut background);
    }
    let mut text = String::new();
    cell.graphemes_utf8(&mut text)?;
    if text.is_empty() {
        text.push(' ');
    }
    let wide = raw.wide()?;
    Ok(Cell {
        text,
        foreground,
        background,
        style: CellStyle {
            bold: style.bold,
            dim: style.faint,
            italic: style.italic,
            underline: style.underline != Underline::None,
            strikeout: style.strikethrough,
            hidden: style.invisible,
            wide: wide == CellWide::Wide,
            wide_spacer: matches!(
                wide,
                CellWide::SpacerTail | CellWide::SpacerHead
            ),
        },
    })
}

// A conservative invalidation hint, not a second terminal parser. Native
// Ghostty still interprets every color and reset. This tracks OSC boundaries
// across PTY chunks so palette probing is absent from ordinary text/CSI updates.
#[derive(Debug)]
enum EscapeHint {
    Ground,
    Escape,
    Osc,
    OscEscape,
}
impl EscapeHint {
    fn observe(&mut self, bytes: &[u8]) -> bool {
        let mut changed = false;
        for byte in bytes {
            *self = match (&*self, byte) {
                (Self::Osc | Self::OscEscape, 0x07 | 0x9c)
                | (Self::OscEscape, b'\\')
                | (Self::Escape, b'c') => {
                    changed = true;
                    Self::Ground
                }
                (Self::Osc | Self::OscEscape, 0x18 | 0x1a) => Self::Ground,
                (Self::Osc | Self::OscEscape, 0x1b) => Self::OscEscape,
                (Self::Osc | Self::OscEscape, _)
                | (_, 0x9d)
                | (Self::Escape, b']') => Self::Osc,
                (_, 0x1b) => Self::Escape,
                _ => Self::Ground,
            };
        }
        changed
    }
}

impl super::links::LinkBuffer for TerminalEngine {
    fn total_rows(&self) -> Result<usize, huterm_protocol::LinkLookup> {
        self.terminal
            .total_rows()
            .map_err(|_| huterm_protocol::LinkLookup::Unavailable)
    }

    fn wrapped(&self, row: usize) -> Result<bool, huterm_protocol::LinkLookup> {
        let row = u32::try_from(row)
            .map_err(|_| huterm_protocol::LinkLookup::Unavailable)?;
        self.terminal
            .grid_ref(Point::Screen(PointCoordinate { x: 0, y: row }))
            .and_then(|reference| reference.row())
            .and_then(libghostty_vt::screen::Row::is_wrapped)
            .map_err(|_| huterm_protocol::LinkLookup::Unavailable)
    }

    fn cell(
        &self,
        row: usize,
        column: u16,
    ) -> Result<super::links::TextCell, huterm_protocol::LinkLookup> {
        use super::links::TextCell;
        use huterm_protocol::LinkLookup;
        let error = |error| match error {
            libghostty_vt::Error::OutOfSpace { .. } => LinkLookup::ScanLimit,
            _ => LinkLookup::Unavailable,
        };
        let row = u32::try_from(row).map_err(|_| LinkLookup::Unavailable)?;
        let reference = self
            .terminal
            .grid_ref(Point::Screen(PointCoordinate { x: column, y: row }))
            .map_err(error)?;
        let cell = reference.cell().map_err(error)?;
        let mut text = String::new();
        if !matches!(
            cell.wide().map_err(error)?,
            libghostty_vt::screen::CellWide::SpacerTail
                | libghostty_vt::screen::CellWide::SpacerHead
        ) {
            let mut graphemes = ['\0'; 256];
            let count = reference.graphemes(&mut graphemes).map_err(error)?;
            if count == 0 {
                text.push(' ');
            } else {
                text.extend(&graphemes[..count]);
            }
        }
        Ok(TextCell { text })
    }

    fn hyperlink(
        &self,
        row: usize,
        column: u16,
    ) -> Result<Option<String>, huterm_protocol::LinkLookup> {
        use huterm_protocol::LinkLookup;
        let error = |error| match error {
            libghostty_vt::Error::OutOfSpace { .. } => LinkLookup::ScanLimit,
            _ => LinkLookup::Unavailable,
        };
        let row = u32::try_from(row).map_err(|_| LinkLookup::Unavailable)?;
        let reference = self
            .terminal
            .grid_ref(Point::Screen(PointCoordinate { x: column, y: row }))
            .map_err(error)?;
        if !reference
            .cell()
            .map_err(error)?
            .has_hyperlink()
            .map_err(error)?
        {
            return Ok(None);
        }
        let mut bytes = vec![0; super::links::MAX_LINK_BYTES];
        let count = reference.hyperlink_uri(&mut bytes).map_err(error)?;
        bytes.truncate(count);
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| LinkLookup::Unavailable)
    }

    fn same_hyperlink(
        &self,
        row: usize,
        column: u16,
        destination: &str,
        scratch: &mut [u8],
    ) -> Result<bool, huterm_protocol::LinkLookup> {
        use huterm_protocol::LinkLookup;
        let row = u32::try_from(row).map_err(|_| LinkLookup::Unavailable)?;
        let reference = self
            .terminal
            .grid_ref(Point::Screen(PointCoordinate { x: column, y: row }))
            .map_err(|_| LinkLookup::Unavailable)?;
        match reference.hyperlink_uri(scratch) {
            Ok(count) => Ok(scratch[..count] == *destination.as_bytes()),
            Err(libghostty_vt::Error::OutOfSpace { .. }) => Ok(false),
            Err(_) => Err(LinkLookup::Unavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_native_memset_preserves_rust_byte_fills() {
        // Ghostty's exported memset once treated C's int fill as a u8.
        // Rust may pass -1 for 0xff, corrupting hash-table control bytes.
        for size in [4, 8, 16, 17, 31, 32, 64, 256] {
            let bytes = vec![u8::MAX; std::hint::black_box(size)];
            assert!(
                bytes.iter().all(|byte| *byte == u8::MAX),
                "native memset corrupted a {size}-byte fill: {bytes:?}"
            );
        }
    }

    #[test]
    fn split_osc_palette_override_equal_to_default_stays_explicit_until_reset()
    {
        let original = libghostty_vt::style::Palette::default().0[1];
        let sequence = format!(
            "\x1b]4;1;#{:02x}{:02x}{:02x}\x1b\\",
            original.r, original.g, original.b
        );
        for split in 0..=sequence.len() {
            let mut engine = TerminalEngine::new(
                TerminalId::new(1),
                GridSize::clamped(8, 3),
                CellSize {
                    width: 8,
                    height: 16,
                },
            )
            .unwrap();
            engine.process(b"\x1b[31mA").unwrap();
            assert_eq!(
                engine.snapshot().unwrap().rows[0].cells[0].foreground,
                CellColor::Indexed(1)
            );
            engine.process(&sequence.as_bytes()[..split]).unwrap();
            let _ = engine.snapshot().unwrap();
            engine.process(&sequence.as_bytes()[split..]).unwrap();
            assert_eq!(
                engine.snapshot().unwrap().rows[0].cells[0].foreground,
                CellColor::Rgb(rgb(original)),
                "split={split}"
            );
            engine.process(b"\x1b]104;1\x07").unwrap();
            assert_eq!(
                engine.snapshot().unwrap().rows[0].cells[0].foreground,
                CellColor::Indexed(1)
            );
        }
    }
}
