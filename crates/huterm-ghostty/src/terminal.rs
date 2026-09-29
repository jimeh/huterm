//! The safe terminal wrapper.

use std::cell::{Cell, RefCell, RefMut};

use crate::callbacks::{Bound, Host, State};
use crate::error::{Error, Result};
use crate::ffi::{
    self,
    keys::{terminal_data as data, terminal_option as option},
};
use crate::native::{AllocatorRef, NativeBytes, NativeTerminal};
use crate::types::{
    Cell as GridCell, DeviceAttributes, Effect, Fill, Mode, Palette, Point,
    PointSpace, Rgb, Row, Screen, Scroll, Scrollbar, palette_from_ffi,
    palette_to_ffi,
};

/// Construction options for a [`Terminal`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Options {
    /// Width in cells; must be nonzero.
    pub columns: u16,
    /// Height in cells; must be nonzero.
    pub rows: u16,
    /// Cell width in pixels for size reports; zero keeps them silent.
    pub cell_width: u32,
    /// Cell height in pixels for size reports; zero keeps them silent.
    pub cell_height: u32,
    /// Device attribute answers; `None` leaves Ghostty's default.
    pub device_attributes: Option<DeviceAttributes>,
    /// XTVERSION name; `None` reports Ghostty's default.
    pub xtversion: Option<String>,
}

/// A libghostty-vt terminal with its effect callbacks.
///
/// Native handles belong to the thread that created them, so a terminal is
/// neither `Send` nor `Sync`:
///
/// ```compile_fail,E0277
/// fn assert_send<T: Send>() {}
/// assert_send::<huterm_ghostty::Terminal<()>>();
/// ```
///
/// Borrowed strings end at the next mutating call:
///
/// ```compile_fail,E0502
/// # fn demo(terminal: &mut huterm_ghostty::Terminal<()>) -> huterm_ghostty::Result<()> {
/// let title = terminal.title()?;
/// terminal.write(b"\x1b]2;next\x07")?;
/// assert!(!title.is_empty());
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Terminal<H> {
    bound: Bound<H>,
    allocator: AllocatorRef,
}

impl<H: Host> Terminal<H> {
    /// Creates a terminal whose callbacks consult `host`.
    ///
    /// # Errors
    ///
    /// Fails when the library cannot create or configure the terminal.
    pub fn new(options: Options, host: H) -> Result<Self> {
        Self::new_in(options, host, None)
    }

    pub(crate) fn new_in(
        options: Options,
        host: H,
        allocator: AllocatorRef,
    ) -> Result<Self> {
        let native =
            NativeTerminal::new(allocator, options.columns, options.rows)?;
        let state = State {
            host: RefCell::new(host),
            effects: RefCell::new(Vec::new()),
            poisoned: Cell::new(false),
            cell_size: Cell::new((options.cell_width, options.cell_height)),
            device_attributes: options
                .device_attributes
                .as_ref()
                .map(DeviceAttributes::to_ffi),
            xtversion: options
                .xtversion
                .map(|version| version.into_bytes().into_boxed_slice()),
        };
        Ok(Self {
            bound: Bound::new(native, state)?,
            allocator,
        })
    }
}

impl<H> Terminal<H> {
    fn state(&self) -> &State<H> {
        self.bound.state()
    }

    fn native(&self) -> Result<&NativeTerminal> {
        if self.state().poisoned.get() {
            return Err(Error::Poisoned);
        }
        Ok(self.bound.native())
    }

    fn native_mut(&mut self) -> Result<&mut NativeTerminal> {
        if self.state().poisoned.get() {
            return Err(Error::Poisoned);
        }
        Ok(self.bound.native_mut())
    }

    /// Fails with [`Error::Poisoned`] if a callback panicked.
    fn after_callbacks(&self) -> Result<()> {
        if self.state().poisoned.get() {
            Err(Error::Poisoned)
        } else {
            Ok(())
        }
    }

    /// Whether a host callback has panicked.
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.state().poisoned.get()
    }

    /// The host, for updating its policy between writes.
    ///
    /// # Panics
    ///
    /// Never: callbacks borrow the host only during terminal calls, which
    /// `&mut self` excludes.
    pub fn host_mut(&mut self) -> RefMut<'_, H> {
        self.state().host.borrow_mut()
    }

    /// Parses PTY output. Effects queue until [`Terminal::take_effects`].
    ///
    /// # Errors
    ///
    /// [`Error::Poisoned`] if a host callback panicked, now or earlier.
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.native_mut()?.vt_write(bytes);
        self.after_callbacks()
    }

    /// Moves queued effects, in callback order, to the end of `out`.
    pub fn take_effects(&mut self, out: &mut Vec<Effect>) {
        out.append(&mut self.state().effects.borrow_mut());
    }

    /// Performs a full reset (RIS).
    ///
    /// # Errors
    ///
    /// [`Error::Poisoned`] if a host callback panicked.
    pub fn reset(&mut self) -> Result<()> {
        self.native_mut()?.reset();
        self.after_callbacks()
    }

    /// Resizes the grid and records the cell pixel size for size reports.
    ///
    /// # Errors
    ///
    /// Fails for zero dimensions or when reflow cannot allocate.
    pub fn resize(
        &mut self,
        columns: u16,
        rows: u16,
        cell_width: u32,
        cell_height: u32,
    ) -> Result<()> {
        self.native_mut()?
            .resize(columns, rows, cell_width, cell_height)?;
        self.state().cell_size.set((cell_width, cell_height));
        self.after_callbacks()
    }

    /// Moves the viewport.
    ///
    /// # Errors
    ///
    /// [`Error::Poisoned`] if a host callback panicked.
    pub fn scroll(&mut self, scroll: Scroll) -> Result<()> {
        self.native_mut()?.scroll(scroll);
        Ok(())
    }

    /// Sets the default foreground; `None` unsets it.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the option.
    pub fn set_default_foreground(&mut self, color: Option<Rgb>) -> Result<()> {
        let color = color.map(ffi::GhosttyColorRgb::from);
        self.native_mut()?
            .set::<option::ColorForeground>(color.as_ref())
    }

    /// Sets the default background; `None` unsets it.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the option.
    pub fn set_default_background(&mut self, color: Option<Rgb>) -> Result<()> {
        let color = color.map(ffi::GhosttyColorRgb::from);
        self.native_mut()?
            .set::<option::ColorBackground>(color.as_ref())
    }

    /// Sets the default cursor color; `None` unsets it.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the option.
    pub fn set_default_cursor(&mut self, color: Option<Rgb>) -> Result<()> {
        let color = color.map(ffi::GhosttyColorRgb::from);
        self.native_mut()?
            .set::<option::ColorCursor>(color.as_ref())
    }

    /// Sets the default palette. Entries an OSC sequence overrode keep
    /// their override.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the option.
    pub fn set_default_palette(&mut self, palette: &Palette) -> Result<()> {
        let palette = palette_to_ffi(palette);
        self.native_mut()?
            .set::<option::ColorPalette>(Some(&palette))
    }

    /// Sets the scrollback byte budget; `None` removes it and zero disables
    /// scrollback.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the option.
    pub fn set_scrollback_bytes(&mut self, bytes: Option<usize>) -> Result<()> {
        self.native_mut()?
            .set::<option::ScrollbackMaxBytes>(bytes.as_ref())
    }

    /// Turns off the APC protocols, Kitty graphics and the Glyph protocol,
    /// and stops buffering APC payloads. Kitty clipboard (OSC 5522) is an
    /// OSC protocol and stays enabled; see
    /// [`Terminal::set_clipboard_write_limit`].
    ///
    /// # Errors
    ///
    /// Fails if the library rejects an option.
    pub fn disable_apc_protocols(&mut self) -> Result<()> {
        let native = self.native_mut()?;
        native.set::<option::KittyImageStorageLimit>(Some(&0))?;
        native.set::<option::GlyphProtocol>(Some(&false))?;
        native.set::<option::ApcMaxBytes>(Some(&0))
    }

    /// Grid width and height in cells.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn size(&self) -> Result<(u16, u16)> {
        let terminal = self.native()?.as_ref();
        Ok((
            terminal.value::<data::Cols>()?,
            terminal.value::<data::Rows>()?,
        ))
    }

    /// The title, valid until the next mutating call.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn title(&self) -> Result<&[u8]> {
        self.native()?.as_ref().string::<data::Title>()
    }

    /// The working directory, valid until the next mutating call.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn pwd(&self) -> Result<&[u8]> {
        self.native()?.as_ref().string::<data::Pwd>()
    }

    /// Whether a mode is set.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidValue`] for a mode the library does not know.
    pub fn mode(&self, mode: Mode) -> Result<bool> {
        let mut config = ffi::GhosttyTerminalModeConfig {
            mode: mode.0,
            value: false,
        };
        self.native()?.as_ref().get::<data::Mode>(&mut config)?;
        Ok(config.value)
    }

    /// The active screen.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn screen(&self) -> Result<Screen> {
        Ok(
            match self.native()?.as_ref().value::<data::ActiveScreen>()? {
                ffi::GHOSTTY_TERMINAL_SCREEN_ALTERNATE => Screen::Alternate,
                _ => Screen::Primary,
            },
        )
    }

    /// Whether any mouse tracking mode is set.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn mouse_tracking(&self) -> Result<bool> {
        self.native()?.as_ref().value::<data::MouseTracking>()
    }

    /// Whether the parser is at ground, outside any escape sequence, OSC,
    /// or UTF-8 codepoint.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn is_ground(&self) -> Result<bool> {
        self.native()?.as_ref().value::<data::VtGround>()
    }

    /// The viewport's position within the scrollable area.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn scrollbar(&self) -> Result<Scrollbar> {
        let bar = self.native()?.as_ref().value::<data::Scrollbar>()?;
        Ok(Scrollbar {
            total: bar.total,
            offset: bar.offset,
            len: bar.len,
        })
    }

    /// Rows in the active screen, including scrollback.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn total_rows(&self) -> Result<usize> {
        self.native()?.as_ref().value::<data::TotalRows>()
    }

    /// Scrollback rows above the active area.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn scrollback_rows(&self) -> Result<usize> {
        self.native()?.as_ref().value::<data::ScrollbackRows>()
    }

    /// The configured scrollback byte budget, `None` when unlimited.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn scrollback_bytes(&self) -> Result<Option<usize>> {
        optional(self.native()?.as_ref().value::<data::ScrollbackMaxBytes>())
    }

    /// The Kitty image storage limit, `None` when Kitty graphics are
    /// compiled out.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn kitty_image_storage_limit(&self) -> Result<Option<u64>> {
        optional(
            self.native()?
                .as_ref()
                .value::<data::KittyImageStorageLimit>(),
        )
    }

    /// The most decoded bytes one Kitty clipboard (OSC 5522) write may
    /// carry; a larger write fails with `EFBIG` before reaching the host.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn clipboard_write_limit(&self) -> Result<usize> {
        self.native()?
            .as_ref()
            .value::<data::ClipboardWriteMaxBytes>()
    }

    /// Sets the most decoded bytes one Kitty clipboard (OSC 5522) write may
    /// carry, counted across all its representations; `None` restores
    /// Ghostty's 64 MiB default. Every value is valid: 0 refuses any write
    /// that carries data, and `usize::MAX` removes the limit.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn set_clipboard_write_limit(
        &mut self,
        bytes: Option<usize>,
    ) -> Result<()> {
        self.native_mut()?
            .set::<option::ClipboardWriteMaxBytes>(bytes.as_ref())
    }

    /// Effective foreground: the OSC override, else the default.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn foreground(&self) -> Result<Option<Rgb>> {
        self.color::<data::ColorForeground>()
    }

    /// Effective background.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn background(&self) -> Result<Option<Rgb>> {
        self.color::<data::ColorBackground>()
    }

    /// Effective cursor color.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn cursor_color(&self) -> Result<Option<Rgb>> {
        self.color::<data::ColorCursor>()
    }

    /// Default foreground, ignoring OSC overrides.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn default_foreground(&self) -> Result<Option<Rgb>> {
        self.color::<data::ColorForegroundDefault>()
    }

    /// Default background, ignoring OSC overrides.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn default_background(&self) -> Result<Option<Rgb>> {
        self.color::<data::ColorBackgroundDefault>()
    }

    /// Default cursor color, ignoring OSC overrides.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn default_cursor_color(&self) -> Result<Option<Rgb>> {
        self.color::<data::ColorCursorDefault>()
    }

    fn color<K>(&self) -> Result<Option<Rgb>>
    where
        K: ffi::TerminalData<Out = ffi::GhosttyColorRgb>,
    {
        optional(self.native()?.as_ref().value::<K>())
            .map(|color| color.map(Rgb::from))
    }

    /// Effective palette, including OSC overrides.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn palette(&self) -> Result<Palette> {
        self.palette_of::<data::ColorPalette>()
    }

    /// Default palette, ignoring OSC overrides.
    ///
    /// # Errors
    ///
    /// Fails if the terminal is poisoned.
    pub fn default_palette(&self) -> Result<Palette> {
        self.palette_of::<data::ColorPaletteDefault>()
    }

    fn palette_of<K>(&self) -> Result<Palette>
    where
        K: ffi::TerminalData<Out = [ffi::GhosttyColorRgb; 256]>,
    {
        let mut palette = [ffi::GhosttyColorRgb::default(); 256];
        self.native()?.as_ref().get::<K>(&mut palette)?;
        Ok(palette_from_ffi(&palette))
    }

    /// Resolves a point, borrowing the terminal until the reference drops.
    ///
    /// ```compile_fail,E0502
    /// # use huterm_ghostty::{Point, PointSpace};
    /// # fn demo(terminal: &mut huterm_ghostty::Terminal<()>) -> huterm_ghostty::Result<()> {
    /// let cell = terminal.grid_ref(Point { space: PointSpace::Active, x: 0, y: 0 })?;
    /// terminal.write(b"x")?;
    /// cell.cell()?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`Error::InvalidValue`] for a point outside its coordinate space.
    pub fn grid_ref(&self, point: Point) -> Result<GridRef<'_>> {
        Ok(GridRef(self.native()?.as_ref().grid_ref(point)?))
    }

    /// Formats the inclusive, linear selection between two screen points as
    /// plain text with soft wraps joined and trailing whitespace trimmed.
    ///
    /// # Errors
    ///
    /// Fails for points outside the screen or when allocation fails.
    pub fn format_plain(
        &self,
        start: Point,
        end: Point,
    ) -> Result<NativeBytes> {
        let terminal = self.native()?.as_ref();
        let start = terminal.grid_ref(start)?;
        let end = terminal.grid_ref(end)?;
        terminal.format_plain(&start, &end, self.allocator)
    }

    pub(crate) fn native_parts(&mut self) -> Result<&mut NativeTerminal> {
        self.native_mut()
    }

    pub(crate) fn native_ref(&self) -> Result<&NativeTerminal> {
        self.native()
    }
}

fn optional<T>(value: Result<T>) -> Result<Option<T>> {
    match value {
        Ok(value) => Ok(Some(value)),
        Err(Error::NoValue) => Ok(None),
        Err(error) => Err(error),
    }
}

/// A resolved cell, valid while its terminal is borrowed.
#[derive(Clone, Copy, Debug)]
pub struct GridRef<'a>(crate::native::GridRef<'a>);

impl GridRef<'_> {
    /// The cell value.
    ///
    /// # Errors
    ///
    /// Fails if the reference is invalid.
    pub fn cell(&self) -> Result<GridCell> {
        self.0.cell()
    }

    /// The row value.
    ///
    /// # Errors
    ///
    /// Fails if the reference is invalid.
    pub fn row(&self) -> Result<Row> {
        self.0.row()
    }

    /// Writes the cell's grapheme codepoints.
    ///
    /// # Errors
    ///
    /// Fails if the reference is invalid.
    pub fn graphemes(&self, buffer: &mut [u32]) -> Result<Fill> {
        self.0.graphemes(buffer)
    }

    /// Writes the cell's OSC 8 URI; `Written(0)` means no hyperlink.
    ///
    /// # Errors
    ///
    /// Fails if the reference is invalid.
    pub fn hyperlink_uri(&self, buffer: &mut [u8]) -> Result<Fill> {
        self.0.hyperlink_uri(buffer)
    }
}

impl Point {
    /// A point in the active area.
    #[must_use]
    pub const fn active(x: u16, y: u32) -> Self {
        Self {
            space: PointSpace::Active,
            x,
            y,
        }
    }

    /// A point in the viewport.
    #[must_use]
    pub const fn viewport(x: u16, y: u32) -> Self {
        Self {
            space: PointSpace::Viewport,
            x,
            y,
        }
    }

    /// A point counted from the top of history.
    #[must_use]
    pub const fn screen(x: u16, y: u32) -> Self {
        Self {
            space: PointSpace::Screen,
            x,
            y,
        }
    }
}

impl Host for () {}
