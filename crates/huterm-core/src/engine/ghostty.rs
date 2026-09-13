use std::cell::{Cell as SharedCell, OnceCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use super::EngineEffect;
use crate::host_effects::{HostEffectAdmission, HostEffectSink};
use crate::terminal::RuntimeError;
use huterm_protocol::{
    BufferRange, Cell, CellColor, CellSize, CellStyle, Cursor, CursorShape,
    GridSize, MouseEncoding, MouseTracking, Rgb, ScrollCommand, TerminalId,
    TerminalModes, TerminalPresentation, TerminalRow, TerminalSnapshot,
    Viewport, appearance_for_background,
};
use libghostty_vt::fmt::{Format, Formatter, FormatterOptions};
use libghostty_vt::render::{
    CellIterator, CursorVisualStyle, Dirty, RowIterator,
};
use libghostty_vt::screen::{CellContentTag, CellWide, Screen};
use libghostty_vt::selection::Selection;
use libghostty_vt::style::{Palette, RgbColor, StyleColor, Underline};
use libghostty_vt::terminal::{
    ClipboardLocation, ClipboardWriteError, ColorScheme, ConformanceLevel,
    DeviceAttributeFeature, DeviceAttributes, DeviceType, Mode, Point,
    PointCoordinate, PrimaryDeviceAttributes, ScrollViewport,
    SecondaryDeviceAttributes, SizeReportSize, TertiaryDeviceAttributes,
};
use libghostty_vt::{RenderState, Terminal};

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
    cell: Rc<SharedCell<CellSize>>,
    presentation: TerminalPresentation,
    terminal: Terminal<'static, 'static>,
    render: RenderState<'static>,
    row_iterator: RowIterator<'static>,
    cell_iterator: CellIterator<'static>,
    retained_rows: Vec<Arc<TerminalRow>>,
    colors: Option<(Option<RgbColor>, Option<RgbColor>, [RgbColor; 256])>,
    effects: Rc<RefCell<Vec<EngineEffect>>>,
    host_effect_sink: Rc<OnceCell<HostEffectSink>>,
    title_dirty: Rc<std::cell::Cell<bool>>,
    palette_overrides: [bool; 256],
    colors_dirty: bool,
    default_overrides: DefaultOverrides,
    escape_hint: EscapeHint,
    mouse_probe: RefCell<(
        libghostty_vt::mouse::Encoder<'static>,
        libghostty_vt::mouse::Event<'static>,
    )>,
}

impl TerminalEngine {
    #[expect(
        clippy::too_many_lines,
        reason = "native callbacks are installed together before ownership starts"
    )]
    pub(super) fn new(
        terminal_id: TerminalId,
        size: GridSize,
        cell: CellSize,
        presentation: TerminalPresentation,
    ) -> Result<Self, RuntimeError> {
        let mut terminal = Terminal::new(size.columns, size.rows)?;
        terminal.set_scrollback_max_bytes(Some(16 * 1024 * 1024))?;
        terminal.set_glyph_protocol_enabled(false)?;
        terminal.set_apc_max_bytes(Some(0))?;
        let host_effect_sink = Rc::new(OnceCell::new());
        let clipboard_sink = Rc::clone(&host_effect_sink);
        terminal.on_clipboard_write(move |_, write| {
            if write.location() != ClipboardLocation::Standard {
                return Err(ClipboardWriteError::Unsupported);
            }
            let mut contents = write.contents();
            let Some(content) = contents.next() else {
                return admit_clipboard(&clipboard_sink, "");
            };
            if content.mime != "text/plain" || contents.next().is_some() {
                return Err(ClipboardWriteError::Unsupported);
            }
            let text = std::str::from_utf8(content.data)
                .map_err(|_| ClipboardWriteError::InvalidData)?;
            admit_clipboard(&clipboard_sink, text)
        })?;
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
        let reported_cell = Rc::new(SharedCell::new(cell));
        let size_cell = Rc::clone(&reported_cell);
        terminal.on_size(move |terminal| {
            let cell = size_cell.get();
            if cell.width == 0 || cell.height == 0 {
                return None;
            }
            Some(SizeReportSize {
                rows: terminal.rows().ok()?,
                columns: terminal.cols().ok()?,
                cell_width: u32::from(cell.width),
                cell_height: u32::from(cell.height),
            })
        })?;
        terminal.on_color_scheme(|terminal| {
            let background = terminal.bg_color().ok()??;
            Some(match appearance_for_background(rgb(background)) {
                huterm_protocol::TerminalAppearance::Light => {
                    ColorScheme::Light
                }
                huterm_protocol::TerminalAppearance::Dark => ColorScheme::Dark,
            })
        })?;
        apply_presentation(&mut terminal, &presentation)?;
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
            cell: reported_cell,
            presentation,
            terminal,
            render: RenderState::new()?,
            row_iterator: RowIterator::new()?,
            cell_iterator: CellIterator::new()?,
            retained_rows: Vec::new(),
            colors: None,
            effects,
            host_effect_sink,
            title_dirty,
            palette_overrides: [false; 256],
            colors_dirty: false,
            default_overrides: DefaultOverrides::default(),
            escape_hint: EscapeHint::Ground,
            mouse_probe: RefCell::new((
                libghostty_vt::mouse::Encoder::new()?,
                libghostty_vt::mouse::Event::new()?,
            )),
        })
    }

    pub(super) fn set_host_effect_sink(&self, sink: HostEffectSink) {
        let result = self.host_effect_sink.set(sink);
        debug_assert!(result.is_ok());
    }

    pub(super) fn process(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<EngineEffect>, RuntimeError> {
        self.colors_dirty |= self.escape_hint.observe(bytes);
        self.terminal.vt_write(bytes);
        self.generation = self.generation.saturating_add(1);
        self.drain_effects()
    }

    pub(super) fn update_presentation(
        &mut self,
        presentation: TerminalPresentation,
    ) -> Result<(), RuntimeError> {
        apply_presentation(&mut self.terminal, &presentation)?;
        self.presentation = presentation;
        self.colors = None;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn presentation(&self) -> &TerminalPresentation {
        &self.presentation
    }

    #[cfg(test)]
    pub(super) fn cell_size(&self) -> CellSize {
        self.cell.get()
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
        self.cell.set(cell);
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

    fn refresh_color_overrides(&mut self) -> Result<(), RuntimeError> {
        if !self.colors_dirty {
            return Ok(());
        }
        self.default_overrides.foreground = probe_default_override(
            &mut self.terminal,
            DefaultColor::Foreground,
        )?;
        self.default_overrides.background = probe_default_override(
            &mut self.terminal,
            DefaultColor::Background,
        )?;
        self.default_overrides.cursor =
            probe_default_override(&mut self.terminal, DefaultColor::Cursor)?;
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
        self.colors_dirty = false;
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
        self.refresh_color_overrides()?;
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
                        self.default_overrides,
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
            cursor_color: if self.default_overrides.cursor {
                state.cursor_color()?.map(rgb)
            } else {
                None
            },
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

fn admit_clipboard(
    sink: &OnceCell<HostEffectSink>,
    text: &str,
) -> Result<(), ClipboardWriteError> {
    let Some(sink) = sink.get() else {
        return Ok(());
    };
    match sink.admit_borrowed(text) {
        HostEffectAdmission::Accepted => Ok(()),
        HostEffectAdmission::Full | HostEffectAdmission::Contended => {
            Err(ClipboardWriteError::Busy)
        }
        HostEffectAdmission::NoRecipient
        | HostEffectAdmission::Denied
        | HostEffectAdmission::Closed => Err(ClipboardWriteError::Denied),
    }
}

fn rgb(color: RgbColor) -> Rgb {
    Rgb {
        red: color.r,
        green: color.g,
        blue: color.b,
    }
}

fn ghostty_rgb(color: Rgb) -> RgbColor {
    RgbColor {
        r: color.red,
        g: color.green,
        b: color.blue,
    }
}

fn apply_presentation(
    terminal: &mut Terminal<'_, '_>,
    presentation: &TerminalPresentation,
) -> Result<(), RuntimeError> {
    terminal
        .set_default_fg_color(Some(ghostty_rgb(presentation.foreground)))?
        .set_default_bg_color(Some(ghostty_rgb(presentation.background)))?
        .set_default_cursor_color(Some(ghostty_rgb(presentation.cursor)))?
        .set_default_color_palette(Some(Palette(
            presentation.palette.map(ghostty_rgb),
        )))?;
    Ok(())
}

#[derive(Clone, Copy)]
enum DefaultColor {
    Foreground,
    Background,
    Cursor,
}

#[derive(Clone, Copy, Debug, Default)]
struct DefaultOverrides {
    foreground: bool,
    background: bool,
    cursor: bool,
}

fn probe_default_override(
    terminal: &mut Terminal<'_, '_>,
    color: DefaultColor,
) -> Result<bool, RuntimeError> {
    let (original, before) = match color {
        DefaultColor::Foreground => {
            (terminal.default_fg_color()?, terminal.fg_color()?)
        }
        DefaultColor::Background => {
            (terminal.default_bg_color()?, terminal.bg_color()?)
        }
        DefaultColor::Cursor => {
            (terminal.default_cursor_color()?, terminal.cursor_color()?)
        }
    };
    let mut probe = original.unwrap_or(RgbColor { r: 0, g: 0, b: 0 });
    probe.r ^= 1;
    match color {
        DefaultColor::Foreground => {
            terminal.set_default_fg_color(Some(probe))?;
        }
        DefaultColor::Background => {
            terminal.set_default_bg_color(Some(probe))?;
        }
        DefaultColor::Cursor => {
            terminal.set_default_cursor_color(Some(probe))?;
        }
    }
    let probed = match color {
        DefaultColor::Foreground => terminal.fg_color(),
        DefaultColor::Background => terminal.bg_color(),
        DefaultColor::Cursor => terminal.cursor_color(),
    };
    match color {
        DefaultColor::Foreground => {
            terminal.set_default_fg_color(original)?;
        }
        DefaultColor::Background => {
            terminal.set_default_bg_color(original)?;
        }
        DefaultColor::Cursor => {
            terminal.set_default_cursor_color(original)?;
        }
    }
    Ok(probed? == before)
}

fn snapshot_cell(
    cell: &libghostty_vt::render::CellIteration<'_, '_>,
    fg: Option<RgbColor>,
    bg: Option<RgbColor>,
    palette: &libghostty_vt::style::Palette,
    overrides: &[bool; 256],
    default_overrides: DefaultOverrides,
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
    let mut foreground = resolve(
        style.fg_color,
        CellColor::DefaultForeground,
        default_overrides.foreground.then_some(fg).flatten(),
    );
    let background_style = match raw.content_tag()? {
        CellContentTag::BgColorPalette => {
            StyleColor::Palette(raw.bg_color_palette()?)
        }
        CellContentTag::BgColorRgb => StyleColor::Rgb(raw.bg_color_rgb()?),
        _ => style.bg_color,
    };
    let mut background = resolve(
        background_style,
        CellColor::DefaultBackground,
        default_overrides.background.then_some(bg).flatten(),
    );
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
}
impl EscapeHint {
    fn observe(&mut self, bytes: &[u8]) -> bool {
        let mut changed = false;
        for byte in bytes {
            *self = match (&*self, byte) {
                (Self::Osc, 0x07 | 0x18 | 0x1a | 0x9c)
                | (Self::Escape, b'c') => {
                    changed = true;
                    Self::Ground
                }
                (Self::Osc, 0x1b) => {
                    changed = true;
                    Self::Escape
                }
                (Self::Osc, _) | (_, 0x9d) | (Self::Escape, b']') => Self::Osc,
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

    fn presentation() -> TerminalPresentation {
        let mut palette = TerminalPresentation::default().palette;
        palette[1] = Rgb {
            red: 0xaa,
            green: 0xbb,
            blue: 0xcc,
        };
        TerminalPresentation {
            foreground: Rgb {
                red: 0x11,
                green: 0x22,
                blue: 0x33,
            },
            background: Rgb {
                red: 0x44,
                green: 0x55,
                blue: 0x66,
            },
            cursor: Rgb {
                red: 0x77,
                green: 0x88,
                blue: 0x99,
            },
            palette,
        }
    }

    fn engine() -> TerminalEngine {
        TerminalEngine::new(
            TerminalId::new(1),
            GridSize::clamped(8, 3),
            CellSize {
                width: 9,
                height: 17,
            },
            presentation(),
        )
        .unwrap()
    }

    fn replies(effects: Vec<EngineEffect>) -> Vec<Vec<u8>> {
        effects
            .into_iter()
            .filter_map(|effect| match effect {
                EngineEffect::PtyWrite(bytes) => Some(bytes),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn native_queries_report_seeded_colors_size_and_appearance() {
        let mut engine = engine();
        let effects = engine
            .process(
                b"\x1b]4;1;?\x1b\\\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b]12;?\x1b\\\x1b[14t\x1b[16t\x1b[18t\x1b[?996n",
            )
            .unwrap();
        assert_eq!(
            replies(effects),
            vec![
                b"\x1b]4;1;rgb:aaaa/bbbb/cccc\x1b\\".to_vec(),
                b"\x1b]10;rgb:1111/2222/3333\x1b\\".to_vec(),
                b"\x1b]11;rgb:4444/5555/6666\x1b\\".to_vec(),
                b"\x1b]12;rgb:7777/8888/9999\x1b\\".to_vec(),
                b"\x1b[4;51;72t".to_vec(),
                b"\x1b[6;17;9t".to_vec(),
                b"\x1b[8;3;8t".to_vec(),
                b"\x1b[?997;1n".to_vec(),
            ]
        );
    }

    #[test]
    fn size_queries_follow_the_existing_ordered_resize_state() {
        let mut engine = engine();
        engine
            .resize(
                GridSize::clamped(5, 4),
                CellSize {
                    width: 11,
                    height: 19,
                },
            )
            .unwrap();
        assert_eq!(
            replies(engine.process(b"\x1b[14t\x1b[16t\x1b[18t").unwrap()),
            vec![
                b"\x1b[4;76;55t".to_vec(),
                b"\x1b[6;19;11t".to_vec(),
                b"\x1b[8;4;5t".to_vec(),
            ]
        );
    }

    #[test]
    fn size_queries_are_silent_until_cell_geometry_is_known() {
        let mut engine = TerminalEngine::new(
            TerminalId::new(1),
            GridSize::clamped(8, 3),
            CellSize {
                width: 0,
                height: 0,
            },
            presentation(),
        )
        .unwrap();
        assert!(
            replies(engine.process(b"\x1b[14t\x1b[16t\x1b[18t").unwrap())
                .is_empty()
        );

        engine
            .resize(
                GridSize::clamped(5, 4),
                CellSize {
                    width: 11,
                    height: 19,
                },
            )
            .unwrap();
        assert_eq!(
            replies(engine.process(b"\x1b[14t\x1b[16t\x1b[18t").unwrap()),
            vec![
                b"\x1b[4;76;55t".to_vec(),
                b"\x1b[6;19;11t".to_vec(),
                b"\x1b[8;4;5t".to_vec(),
            ]
        );
    }

    #[test]
    fn mutations_queries_and_resets_are_ordered_within_one_write() {
        let mut engine = engine();
        let effects = engine
            .process(
                b"\x1b]4;1;#010203\x1b\\\x1b]4;1;?\x1b\\\x1b]104;1\x1b\\\x1b]4;1;?\x1b\\\x1b]10;#abcdef\x1b\\\x1b]10;?\x1b\\\x1b]110\x1b\\\x1b]10;?\x1b\\\x1b]11;#ffffff\x1b\\\x1b]11;?\x1b\\\x1b[?996n\x1b]111\x1b\\\x1b]11;?\x1b\\\x1b[?996n\x1b]12;#0a0b0c\x1b\\\x1b]12;?\x1b\\\x1b]112\x1b\\\x1b]12;?\x1b\\",
            )
            .unwrap();
        assert_eq!(
            replies(effects),
            vec![
                b"\x1b]4;1;rgb:0101/0202/0303\x1b\\".to_vec(),
                b"\x1b]4;1;rgb:aaaa/bbbb/cccc\x1b\\".to_vec(),
                b"\x1b]10;rgb:abab/cdcd/efef\x1b\\".to_vec(),
                b"\x1b]10;rgb:1111/2222/3333\x1b\\".to_vec(),
                b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\".to_vec(),
                b"\x1b[?997;2n".to_vec(),
                b"\x1b]11;rgb:4444/5555/6666\x1b\\".to_vec(),
                b"\x1b[?997;1n".to_vec(),
                b"\x1b]12;rgb:0a0a/0b0b/0c0c\x1b\\".to_vec(),
                b"\x1b]12;rgb:7777/8888/9999\x1b\\".to_vec(),
            ]
        );
    }

    #[test]
    fn osc_dispatch_at_escape_updates_override_state_before_the_next_byte() {
        let mut engine = engine();
        engine.process(b"A\x1b[31mB").unwrap();
        engine
            .process(
                b"\x1b]10;#010203\x1b]11;#040506\x1b]12;#070809\x1b]4;1;#0a0b0c\x1b",
            )
            .unwrap();
        let overridden = engine.snapshot().unwrap();
        assert_eq!(
            (
                overridden.rows[0].cells[0].foreground,
                overridden.rows[0].cells[0].background,
                overridden.rows[0].cells[1].foreground,
                overridden.cursor_color,
            ),
            (
                CellColor::Rgb(Rgb {
                    red: 1,
                    green: 2,
                    blue: 3,
                }),
                CellColor::Rgb(Rgb {
                    red: 4,
                    green: 5,
                    blue: 6,
                }),
                CellColor::Rgb(Rgb {
                    red: 10,
                    green: 11,
                    blue: 12,
                }),
                Some(Rgb {
                    red: 7,
                    green: 8,
                    blue: 9,
                }),
            )
        );

        engine
            .process(b"]110\x1b]111\x1b]112\x1b]104;1\x1b")
            .unwrap();
        let reset = engine.snapshot().unwrap();
        assert_eq!(
            (
                reset.rows[0].cells[0].foreground,
                reset.rows[0].cells[0].background,
                reset.rows[0].cells[1].foreground,
                reset.cursor_color,
            ),
            (
                CellColor::DefaultForeground,
                CellColor::DefaultBackground,
                CellColor::Indexed(1),
                None,
            )
        );
    }

    #[test]
    fn osc_dispatch_at_can_or_sub_updates_override_state_immediately() {
        for terminator in [0x18, 0x1a] {
            let mut engine = engine();
            engine.process(b"A").unwrap();
            let mut mutation = b"\x1b]10;#010203".to_vec();
            mutation.push(terminator);
            engine.process(&mutation).unwrap();
            assert_eq!(
                engine.snapshot().unwrap().rows[0].cells[0].foreground,
                CellColor::Rgb(Rgb {
                    red: 1,
                    green: 2,
                    blue: 3,
                }),
                "terminator={terminator:#x}"
            );

            let mut reset = b"\x1b]110".to_vec();
            reset.push(terminator);
            engine.process(&reset).unwrap();
            assert_eq!(
                engine.snapshot().unwrap().rows[0].cells[0].foreground,
                CellColor::DefaultForeground,
                "terminator={terminator:#x}"
            );
        }
    }

    #[test]
    fn default_and_equal_osc_overrides_survive_theme_updates_and_resets() {
        let mut engine = engine();
        engine.process(b"A\x1b[31mB").unwrap();
        let initial = engine.snapshot().unwrap();
        assert_eq!(
            initial.rows[0].cells[0].foreground,
            CellColor::DefaultForeground
        );
        assert_eq!(initial.rows[0].cells[1].foreground, CellColor::Indexed(1));

        engine
            .process(
                b"\r\x1b[0m\x1b]10;#112233\x1b\\\x1b]11;#ffffff\x1b\\\x1b]12;#778899\x1b\\\x1b]4;1;#aabbcc\x1b\\A",
            )
            .unwrap();
        let overridden = engine.snapshot().unwrap();
        assert_eq!(
            overridden.rows[0].cells[0].foreground,
            CellColor::Rgb(presentation().foreground)
        );
        assert_eq!(
            overridden.rows[0].cells[0].background,
            CellColor::Rgb(Rgb {
                red: 0xff,
                green: 0xff,
                blue: 0xff,
            })
        );
        assert_eq!(
            overridden.rows[0].cells[1].foreground,
            CellColor::Rgb(presentation().palette[1])
        );
        assert_eq!(overridden.cursor_color, Some(presentation().cursor));

        let mut changed = presentation();
        changed.foreground.red = 0xfe;
        changed.background = Rgb {
            red: 0x10,
            green: 0x20,
            blue: 0x30,
        };
        changed.cursor.blue = 0xdc;
        changed.palette[1].red = 0xcb;
        let generation = engine.generation();
        engine.update_presentation(changed.clone()).unwrap();
        let themed = engine.snapshot().unwrap();
        assert_eq!(themed.generation, generation);
        assert!(!Arc::ptr_eq(&overridden.rows[0], &themed.rows[0]));
        assert_eq!(
            themed.rows[0].cells[0].foreground,
            CellColor::Rgb(presentation().foreground)
        );
        assert_eq!(
            replies(
                engine
                    .process(
                        b"\x1b]4;1;?\x1b\\\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b]12;?\x1b\\\x1b[?996n"
                    )
                    .unwrap()
            ),
            vec![
                b"\x1b]4;1;rgb:aaaa/bbbb/cccc\x1b\\".to_vec(),
                b"\x1b]10;rgb:1111/2222/3333\x1b\\".to_vec(),
                b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\".to_vec(),
                b"\x1b]12;rgb:7777/8888/9999\x1b\\".to_vec(),
                b"\x1b[?997;2n".to_vec(),
            ]
        );

        assert_eq!(
            replies(
                engine
                    .process(
                        b"\x1b]104;1\x1b\\\x1b]110\x1b\\\x1b]111\x1b\\\x1b]112\x1b\\\x1b]4;1;?\x1b\\\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b]12;?\x1b\\\x1b[?996n"
                    )
                    .unwrap()
            ),
            vec![
                b"\x1b]4;1;rgb:cbcb/bbbb/cccc\x1b\\".to_vec(),
                b"\x1b]10;rgb:fefe/2222/3333\x1b\\".to_vec(),
                b"\x1b]11;rgb:1010/2020/3030\x1b\\".to_vec(),
                b"\x1b]12;rgb:7777/8888/dcdc\x1b\\".to_vec(),
                b"\x1b[?997;1n".to_vec(),
            ]
        );
        let reset = engine.snapshot().unwrap();
        assert_eq!(
            reset.rows[0].cells[0].foreground,
            CellColor::DefaultForeground
        );
        assert_eq!(
            reset.rows[0].cells[0].background,
            CellColor::DefaultBackground
        );
        assert_eq!(reset.rows[0].cells[1].foreground, CellColor::Indexed(1));
        assert_eq!(reset.cursor_color, None);
        assert_eq!(engine.presentation(), &changed);
    }

    #[test]
    fn split_queries_reply_once_after_completion() {
        for (query, dispatch_at, expected) in [
            (
                b"\x1b]10;?\x1b\\".as_slice(),
                b"\x1b]10;?\x1b\\".len() - 1,
                b"\x1b]10;rgb:1111/2222/3333\x1b\\".as_slice(),
            ),
            (
                b"\x1b[14t".as_slice(),
                b"\x1b[14t".len(),
                b"\x1b[4;51;72t".as_slice(),
            ),
            (
                b"\x1b[?996n".as_slice(),
                b"\x1b[?996n".len(),
                b"\x1b[?997;1n".as_slice(),
            ),
        ] {
            for split in 0..=query.len() {
                let mut engine = engine();
                let mut actual =
                    replies(engine.process(&query[..split]).unwrap());
                if split < dispatch_at {
                    assert!(actual.is_empty());
                }
                actual
                    .extend(replies(engine.process(&query[split..]).unwrap()));
                assert_eq!(
                    actual,
                    vec![expected.to_vec()],
                    "query={query:?} split={split}"
                );
            }
        }
    }

    #[test]
    fn malformed_and_unsupported_queries_are_silent() {
        let mut engine = engine();
        assert!(
            replies(
                engine
                    .process(
                        b"\x1b]4;999;?\x1b\\\x1b]10;bogus\x1b\\\x1b[15t\x1b[?996;1n"
                    )
                    .unwrap()
            )
            .is_empty()
        );
    }

    #[test]
    fn osc_104_without_indices_resets_the_complete_palette() {
        let mut engine = engine();
        assert_eq!(
            replies(
                engine
                    .process(
                        b"\x1b]4;1;#010203;2;#040506\x1b\\\x1b]104\x1b\\\x1b]4;1;?;2;?\x1b\\"
                    )
                    .unwrap()
            ),
            vec![concat!(
                "\x1b]4;1;rgb:aaaa/bbbb/cccc\x1b\\",
                "\x1b]4;2;rgb:0000/cdcd/0000\x1b\\"
            )
            .as_bytes()
            .to_vec()]
        );
    }

    #[test]
    fn clipboard_callbacks_preserve_empty_and_binary_contents() {
        let writes = Rc::new(RefCell::new(Vec::new()));
        let captured = Rc::clone(&writes);
        let mut terminal = Terminal::new(8, 3).unwrap();
        terminal
            .on_clipboard_write(move |_, write| {
                captured.borrow_mut().push(
                    write
                        .contents()
                        .map(|content| content.data.to_vec())
                        .collect::<Vec<_>>(),
                );
                Ok(())
            })
            .unwrap();
        terminal.vt_write(b"\x1b]52;c;\x07");
        terminal.vt_write(b"\x1b]52;c;/w==\x07");
        terminal.vt_write(b"\x1b]1337;Copy=:Zg==\x1b\\");
        assert_eq!(
            *writes.borrow(),
            vec![vec![], vec![vec![255]], vec![b"f".to_vec()]]
        );
    }

    #[test]
    fn oversized_clipboard_capture_is_dropped_and_parser_recovers() {
        let writes = Rc::new(RefCell::new(Vec::new()));
        let captured = Rc::clone(&writes);
        let mut terminal = Terminal::new(8, 3).unwrap();
        terminal
            .on_clipboard_write(move |_, write| {
                captured.borrow_mut().push(
                    write
                        .contents()
                        .map(|content| content.data.to_vec())
                        .collect::<Vec<_>>(),
                );
                Ok(())
            })
            .unwrap();
        // Feed bounded chunks so the test itself never retains the huge OSC.
        // Cancellation and reset must not dispatch the rejected prefix.
        for (prefix, ending) in [
            (b"\x1b]52;c;".as_slice(), b"\x07".as_slice()),
            (b"\x1b]1337;Copy=:", b"\x07"),
            (b"\x1b]52;c;".as_slice(), b"\x18".as_slice()),
            (b"\x1b]1337;Copy=:", b"\x1bc"),
        ] {
            terminal.vt_write(prefix);
            for _ in 0..=8192 {
                terminal.vt_write(&[b'A'; 1024]);
            }
            terminal.vt_write(ending);
            assert!(writes.borrow().is_empty());
        }
        terminal.vt_write(b"\x1b]52;c;Zg==\x07");
        assert_eq!(*writes.borrow(), vec![vec![b"f".to_vec()]]);
    }

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
                TerminalPresentation::default(),
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
