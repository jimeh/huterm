//! Every libghostty-vt call that is not a callback.
//!
//! Each owned handle frees itself on drop. Borrowed data carries the
//! lifetime of the handle it came from, and cursors over render-state rows
//! and cells exist only while their render state is shared-borrowed, so safe
//! callers cannot read native memory after the call that invalidates it.
//! Headers cited below are under `include/ghostty/vt/` in the pinned source.
#![expect(
    unsafe_code,
    reason = "libghostty-vt is a C library; this module confines its calls"
)]
#![deny(
    clippy::as_pointer_underscore,
    clippy::as_ptr_cast_mut,
    clippy::as_underscore,
    clippy::fn_to_numeric_cast_any,
    clippy::mem_forget,
    clippy::missing_safety_doc,
    clippy::multiple_unsafe_ops_per_block,
    clippy::undocumented_unsafe_blocks,
    clippy::unnecessary_safety_comment,
    clippy::unnecessary_safety_doc
)]

use core::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::{self, NonNull};

use crate::error::{Error, Result};
use crate::ffi::{
    self, BuildInfo, CellData, MouseEncoderOption, RenderCellData,
    RenderRowData, RenderStateData, RowData, TerminalCallback, TerminalData,
    TerminalOption, TerminalPointer,
};
use crate::types::{
    Cell, CellContent, CellWidth, Fill, MouseAction, MouseButton, Point,
    PointSpace, Rgb, Row, Scroll, Style, StyleColor,
};

/// An allocator that outlives every object created with it. `None` selects
/// the library's default allocator.
pub(crate) type AllocatorRef = Option<&'static ffi::GhosttyAllocator>;

fn allocator_ptr(allocator: AllocatorRef) -> *const ffi::GhosttyAllocator {
    allocator.map_or(ptr::null(), ptr::from_ref)
}

/// Views a library-produced string as bytes.
///
/// # Safety
///
/// `string` must come from a libghostty-vt API whose documented lifetime
/// covers `'a`, and its bytes must not change during `'a`.
pub(crate) unsafe fn string_bytes<'a>(string: ffi::GhosttyString) -> &'a [u8] {
    if string.ptr.is_null() || string.len == 0 {
        return &[];
    }
    // SAFETY: types.h `GhosttyString` points to `len` bytes, valid for the
    // lifetime the producing API documents, which the caller guarantees.
    unsafe { std::slice::from_raw_parts(string.ptr, string.len) }
}

/// Converts a C buffer result into a fill count.
fn fill(code: ffi::GhosttyResult, written: usize) -> Result<Fill> {
    match Error::from_code(code) {
        Ok(()) => Ok(Fill::Written(written)),
        Err(Error::OutOfSpace) => Ok(Fill::TooSmall(written)),
        Err(error) => Err(error),
    }
}

fn buffer_ptr<T>(buffer: &mut [T]) -> *mut T {
    if buffer.is_empty() {
        ptr::null_mut()
    } else {
        buffer.as_mut_ptr()
    }
}

/// An owned `GhosttyTerminal`.
#[derive(Debug)]
pub(crate) struct NativeTerminal {
    ptr: NonNull<ffi::GhosttyTerminalImpl>,
}

impl NativeTerminal {
    pub(crate) fn new(
        allocator: AllocatorRef,
        columns: u16,
        rows: u16,
    ) -> Result<Self> {
        let mut raw: ffi::GhosttyTerminal = ptr::null_mut();
        // SAFETY: terminal.h `ghostty_terminal_new` accepts NULL or a live
        // allocator (ours is 'static) and writes the handle to `raw`.
        let code = unsafe {
            ffi::ghostty_terminal_new(
                allocator_ptr(allocator),
                &raw mut raw,
                columns,
                rows,
            )
        };
        Error::from_code(code)?;
        NonNull::new(raw)
            .map(|ptr| Self { ptr })
            .ok_or(Error::InvalidValue)
    }

    pub(crate) const fn as_ref(&self) -> TerminalRef<'_> {
        TerminalRef {
            ptr: self.ptr,
            _terminal: PhantomData,
        }
    }

    /// Sets an option whose value the header passes by pointer; `None`
    /// passes NULL, whose meaning each option documents.
    pub(crate) fn set<K: TerminalOption>(
        &mut self,
        value: Option<&K::Value>,
    ) -> Result<()> {
        let value =
            value.map_or(ptr::null(), |value| ptr::from_ref(value).cast());
        // SAFETY: terminal.h `ghostty_terminal_set` reads `K::Value`, the
        // generated input type for `K::KEY`, through `value` or accepts NULL.
        let code = unsafe {
            ffi::ghostty_terminal_set(self.ptr.as_ptr(), K::KEY, value)
        };
        Error::from_code(code)
    }

    /// Installs or clears a callback option.
    pub(crate) fn set_callback<K: TerminalCallback>(
        &mut self,
        callback: K::Callback,
    ) -> Result<()>
    where
        K::Callback: ffi::CallbackPointer,
    {
        let value = ffi::CallbackPointer::as_pointer(callback);
        // SAFETY: terminal.h passes callback options directly as the value
        // pointer; `K::Callback` is the generated callback type for `K::KEY`.
        let code = unsafe {
            ffi::ghostty_terminal_set(self.ptr.as_ptr(), K::KEY, value)
        };
        Error::from_code(code)
    }

    /// Installs the userdata pointer passed to every callback.
    ///
    /// # Safety
    ///
    /// Every installed callback must interpret `pointer` correctly, and it
    /// must stay valid until this terminal is freed.
    pub(crate) unsafe fn set_pointer<K: TerminalPointer>(
        &mut self,
        pointer: *mut c_void,
    ) -> Result<()> {
        // SAFETY: terminal.h `GHOSTTY_TERMINAL_OPT_USERDATA` takes the
        // pointer directly; the caller guarantees its validity.
        let code = unsafe {
            ffi::ghostty_terminal_set(
                self.ptr.as_ptr(),
                K::KEY,
                pointer.cast_const(),
            )
        };
        Error::from_code(code)
    }

    pub(crate) fn vt_write(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        // SAFETY: terminal.h `ghostty_terminal_vt_write` reads `len` bytes
        // from `data`. Callbacks it invokes never re-enter the terminal.
        unsafe {
            ffi::ghostty_terminal_vt_write(
                self.ptr.as_ptr(),
                bytes.as_ptr(),
                bytes.len(),
            );
        }
    }

    pub(crate) fn reset(&mut self) {
        // SAFETY: terminal.h `ghostty_terminal_reset` takes a live handle.
        unsafe { ffi::ghostty_terminal_reset(self.ptr.as_ptr()) }
    }

    pub(crate) fn resize(
        &mut self,
        columns: u16,
        rows: u16,
        cell_width: u32,
        cell_height: u32,
    ) -> Result<()> {
        // SAFETY: terminal.h `ghostty_terminal_resize` takes a live handle
        // and plain dimensions; zero dimensions return an error.
        let code = unsafe {
            ffi::ghostty_terminal_resize(
                self.ptr.as_ptr(),
                columns,
                rows,
                cell_width,
                cell_height,
            )
        };
        Error::from_code(code)
    }

    pub(crate) fn scroll(&mut self, scroll: Scroll) {
        let (tag, value) = match scroll {
            Scroll::Top => (
                ffi::GHOSTTY_SCROLL_VIEWPORT_TOP,
                ffi::GhosttyTerminalScrollViewportValue { row: 0 },
            ),
            Scroll::Bottom => (
                ffi::GHOSTTY_SCROLL_VIEWPORT_BOTTOM,
                ffi::GhosttyTerminalScrollViewportValue { row: 0 },
            ),
            Scroll::Delta(delta) => (
                ffi::GHOSTTY_SCROLL_VIEWPORT_DELTA,
                ffi::GhosttyTerminalScrollViewportValue { delta },
            ),
            Scroll::Row(row) => (
                ffi::GHOSTTY_SCROLL_VIEWPORT_ROW,
                ffi::GhosttyTerminalScrollViewportValue { row },
            ),
        };
        let behavior = ffi::GhosttyTerminalScrollViewport { tag, value };
        // SAFETY: terminal.h `ghostty_terminal_scroll_viewport` takes a live
        // handle and a tagged union whose active field matches `tag`.
        unsafe {
            ffi::ghostty_terminal_scroll_viewport(self.ptr.as_ptr(), behavior);
        }
    }
}

impl Drop for NativeTerminal {
    fn drop(&mut self) {
        // SAFETY: terminal.h `ghostty_terminal_free` releases a handle from
        // `ghostty_terminal_new`; nothing uses it afterwards.
        unsafe { ffi::ghostty_terminal_free(self.ptr.as_ptr()) }
    }
}

/// A terminal handle usable for reads during `'a`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TerminalRef<'a> {
    ptr: NonNull<ffi::GhosttyTerminalImpl>,
    _terminal: PhantomData<&'a NativeTerminal>,
}

impl<'a> TerminalRef<'a> {
    /// Wraps the handle a callback received.
    ///
    /// # Safety
    ///
    /// `raw` must be a live terminal for `'a`, and nothing may mutate it
    /// during `'a` except the write that is invoking the callback.
    pub(crate) unsafe fn from_callback(
        raw: ffi::GhosttyTerminal,
    ) -> Option<Self> {
        NonNull::new(raw).map(|ptr| Self {
            ptr,
            _terminal: PhantomData,
        })
    }

    pub(crate) const fn raw(self) -> ffi::GhosttyTerminal {
        self.ptr.as_ptr()
    }

    /// Reads a value into `out`, which in/out keys read first.
    pub(crate) fn get<K: TerminalData>(self, out: &mut K::Out) -> Result<()> {
        // SAFETY: terminal.h `ghostty_terminal_get` writes the documented
        // output type for `K::KEY`, which the generated table names
        // `K::Out`; `out` is exclusive and live.
        let code = unsafe {
            ffi::ghostty_terminal_get(
                self.ptr.as_ptr(),
                K::KEY,
                ptr::from_mut(out).cast(),
            )
        };
        Error::from_code(code)
    }

    pub(crate) fn value<K: TerminalData>(self) -> Result<K::Out>
    where
        K::Out: Default,
    {
        let mut out = K::Out::default();
        self.get::<K>(&mut out)?;
        Ok(out)
    }

    /// Reads a borrowed string, valid until the next mutating call.
    pub(crate) fn string<K>(self) -> Result<&'a [u8]>
    where
        K: TerminalData<Out = ffi::GhosttyString>,
    {
        let string = self.value::<K>()?;
        // SAFETY: terminal.h documents title and pwd strings as valid until
        // the next mutating terminal call, which `'a` excludes.
        Ok(unsafe { string_bytes(string) })
    }

    pub(crate) fn grid_ref(self, point: Point) -> Result<GridRef<'a>> {
        let tag = match point.space {
            PointSpace::Active => ffi::GHOSTTY_POINT_TAG_ACTIVE,
            PointSpace::Viewport => ffi::GHOSTTY_POINT_TAG_VIEWPORT,
            PointSpace::Screen => ffi::GHOSTTY_POINT_TAG_SCREEN,
        };
        let point = ffi::GhosttyPoint {
            tag,
            value: ffi::GhosttyPointValue {
                coordinate: ffi::GhosttyPointCoordinate {
                    x: point.x,
                    y: point.y,
                },
            },
        };
        let mut raw: ffi::GhosttyGridRef = ffi::sized();
        // SAFETY: terminal.h `ghostty_terminal_grid_ref` resolves a point
        // and writes a sized grid ref to the exclusive `raw`.
        let code = unsafe {
            ffi::ghostty_terminal_grid_ref(
                self.ptr.as_ptr(),
                point,
                &raw mut raw,
            )
        };
        Error::from_code(code)?;
        Ok(GridRef {
            raw,
            _terminal: PhantomData,
        })
    }

    /// Formats an inclusive, linear selection as unwrapped, trimmed text.
    pub(crate) fn format_plain(
        self,
        start: &GridRef<'a>,
        end: &GridRef<'a>,
        allocator: AllocatorRef,
    ) -> Result<NativeBytes> {
        let mut selection: ffi::GhosttySelection = ffi::sized();
        selection.start = start.raw;
        selection.end = end.raw;
        selection.rectangle = false;
        let mut options: ffi::GhosttyTerminalSelectionFormatOptions =
            ffi::sized();
        options.emit = ffi::GHOSTTY_FORMATTER_FORMAT_PLAIN;
        options.unwrap = true;
        options.trim = true;
        options.selection = &raw const selection;
        let mut out = ptr::null_mut();
        let mut len = 0;
        // SAFETY: selection.h `ghostty_terminal_selection_format_alloc`
        // reads the options and selection (whose grid refs are current
        // because `'a` excludes mutation) and writes an allocation from
        // `allocator` to `out`/`len`.
        let code = unsafe {
            ffi::ghostty_terminal_selection_format_alloc(
                self.ptr.as_ptr(),
                allocator_ptr(allocator),
                options,
                &raw mut out,
                &raw mut len,
            )
        };
        let bytes = NativeBytes {
            ptr: out,
            len,
            allocator,
        };
        Error::from_code(code)?;
        Ok(bytes)
    }
}

/// An untracked grid reference, valid while its terminal is unchanged.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GridRef<'a> {
    raw: ffi::GhosttyGridRef,
    _terminal: PhantomData<&'a NativeTerminal>,
}

impl GridRef<'_> {
    pub(crate) fn cell(&self) -> Result<Cell> {
        let mut cell: ffi::GhosttyCell = 0;
        // SAFETY: grid_ref.h `ghostty_grid_ref_cell` reads a current ref
        // and writes the cell value.
        let code = unsafe {
            ffi::ghostty_grid_ref_cell(&raw const self.raw, &raw mut cell)
        };
        Error::from_code(code).map(|()| Cell(cell))
    }

    pub(crate) fn row(&self) -> Result<Row> {
        let mut row: ffi::GhosttyRow = 0;
        // SAFETY: grid_ref.h `ghostty_grid_ref_row` reads a current ref and
        // writes the row value.
        let code = unsafe {
            ffi::ghostty_grid_ref_row(&raw const self.raw, &raw mut row)
        };
        Error::from_code(code).map(|()| Row(row))
    }

    /// Writes the cell's grapheme codepoints.
    pub(crate) fn graphemes(&self, buffer: &mut [u32]) -> Result<Fill> {
        let mut len = 0;
        // SAFETY: grid_ref.h `ghostty_grid_ref_graphemes` writes at most
        // `buf_len` codepoints to `buf` (NULL when empty) and the count or
        // required count to `out_len`.
        let code = unsafe {
            ffi::ghostty_grid_ref_graphemes(
                &raw const self.raw,
                buffer_ptr(buffer),
                buffer.len(),
                &raw mut len,
            )
        };
        fill(code, len)
    }

    /// Writes the cell's hyperlink URI; zero bytes means no hyperlink.
    pub(crate) fn hyperlink_uri(&self, buffer: &mut [u8]) -> Result<Fill> {
        let mut len = 0;
        // SAFETY: grid_ref.h `ghostty_grid_ref_hyperlink_uri` writes at most
        // `buf_len` bytes to `buf` (NULL when empty) and the count or
        // required count to `out_len`.
        let code = unsafe {
            ffi::ghostty_grid_ref_hyperlink_uri(
                &raw const self.raw,
                buffer_ptr(buffer),
                buffer.len(),
                &raw mut len,
            )
        };
        fill(code, len)
    }
}

/// Bytes allocated by libghostty-vt, freed with the allocator that made
/// them.
#[derive(Debug)]
pub struct NativeBytes {
    ptr: *mut u8,
    len: usize,
    allocator: AllocatorRef,
}

impl NativeBytes {
    #[cfg(test)]
    pub(crate) const fn is_null(&self) -> bool {
        self.ptr.is_null()
    }

    /// The bytes; NULL with zero length is empty.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        if self.ptr.is_null() || self.len == 0 {
            return &[];
        }
        // SAFETY: allocator.h: the producing API returned `len` bytes at
        // `ptr`, owned by this value until drop.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for NativeBytes {
    fn drop(&mut self) {
        // SAFETY: allocator.h `ghostty_free` releases an allocation with the
        // allocator and length that produced it; NULL is a no-op.
        unsafe {
            ffi::ghostty_free(
                allocator_ptr(self.allocator),
                self.ptr,
                self.len,
            );
        }
    }
}

/// Allocates `len` bytes through `ghostty_alloc`.
#[cfg(test)]
pub(crate) fn alloc_bytes(allocator: AllocatorRef, len: usize) -> NativeBytes {
    // SAFETY: allocator.h `ghostty_alloc` returns NULL or `len` bytes that
    // must be freed with the same allocator, which `NativeBytes` does.
    let ptr = unsafe { ffi::ghostty_alloc(allocator_ptr(allocator), len) };
    NativeBytes {
        ptr,
        len: if ptr.is_null() { 0 } else { len },
        allocator,
    }
}

/// The linked library's ABI manifest.
#[cfg(test)]
pub(crate) fn type_json() -> &'static str {
    // SAFETY: types.h `ghostty_type_json` takes no arguments.
    let raw = unsafe { ffi::ghostty_type_json() };
    // SAFETY: types.h: the result is a NUL-terminated string valid for the
    // life of the process.
    let json = unsafe { std::ffi::CStr::from_ptr(raw) };
    json.to_str().unwrap_or_default()
}

pub(crate) fn build_info<K: BuildInfo>() -> Result<K::Out>
where
    K::Out: Default,
{
    let mut out = K::Out::default();
    // SAFETY: build_info.h `ghostty_build_info` writes the documented
    // output type, which the generated table names `K::Out`.
    let code = unsafe {
        ffi::ghostty_build_info(K::KEY, ptr::from_mut(&mut out).cast())
    };
    Error::from_code(code).map(|()| out)
}

impl Cell {
    fn get<K: CellData>(self) -> Result<K::Out>
    where
        K::Out: Default,
    {
        let mut out = K::Out::default();
        // SAFETY: screen.h `ghostty_cell_get` decodes a cell value into the
        // documented output type, which the generated table names `K::Out`.
        let code = unsafe {
            ffi::ghostty_cell_get(
                self.0,
                K::KEY,
                ptr::from_mut(&mut out).cast(),
            )
        };
        Error::from_code(code).map(|()| out)
    }

    /// The codepoint; zero for empty and background-only cells.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the key.
    pub fn codepoint(self) -> Result<u32> {
        self.get::<ffi::keys::cell_data::Codepoint>()
    }

    /// What the cell holds.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the key.
    pub fn content(self) -> Result<CellContent> {
        Ok(match self.get::<ffi::keys::cell_data::ContentTag>()? {
            ffi::GHOSTTY_CELL_CONTENT_CODEPOINT => CellContent::Codepoint,
            ffi::GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME => {
                CellContent::Grapheme
            }
            ffi::GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE => {
                CellContent::BackgroundPalette
            }
            ffi::GHOSTTY_CELL_CONTENT_BG_COLOR_RGB => {
                CellContent::BackgroundRgb
            }
            other => CellContent::Unknown(other),
        })
    }

    /// The cell's width role.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the key.
    pub fn width(self) -> Result<CellWidth> {
        Ok(match self.get::<ffi::keys::cell_data::Wide>()? {
            ffi::GHOSTTY_CELL_WIDE_NARROW => CellWidth::Narrow,
            ffi::GHOSTTY_CELL_WIDE_WIDE => CellWidth::Wide,
            ffi::GHOSTTY_CELL_WIDE_SPACER_TAIL => CellWidth::SpacerTail,
            ffi::GHOSTTY_CELL_WIDE_SPACER_HEAD => CellWidth::SpacerHead,
            other => CellWidth::Unknown(other),
        })
    }

    /// Whether the cell has text.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the key.
    pub fn has_text(self) -> Result<bool> {
        self.get::<ffi::keys::cell_data::HasText>()
    }

    /// Whether the cell has a non-default style.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the key.
    pub fn has_styling(self) -> Result<bool> {
        self.get::<ffi::keys::cell_data::HasStyling>()
    }

    /// Whether the cell has an OSC 8 hyperlink.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the key.
    pub fn has_hyperlink(self) -> Result<bool> {
        self.get::<ffi::keys::cell_data::HasHyperlink>()
    }

    /// The background palette index of a background-only cell.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the key.
    pub fn background_palette(self) -> Result<u8> {
        self.get::<ffi::keys::cell_data::ColorPalette>()
    }

    /// The background color of an RGB background-only cell.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the key.
    pub fn background_rgb(self) -> Result<Rgb> {
        self.get::<ffi::keys::cell_data::ColorRgb>().map(Rgb::from)
    }
}

impl Row {
    fn get<K: RowData>(self) -> Result<K::Out>
    where
        K::Out: Default,
    {
        let mut out = K::Out::default();
        // SAFETY: screen.h `ghostty_row_get` decodes a row value into the
        // documented output type, which the generated table names `K::Out`.
        let code = unsafe {
            ffi::ghostty_row_get(self.0, K::KEY, ptr::from_mut(&mut out).cast())
        };
        Error::from_code(code).map(|()| out)
    }

    /// Whether the row soft-wraps into the next.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the key.
    pub fn wrapped(self) -> Result<bool> {
        self.get::<ffi::keys::row_data::Wrap>()
    }

    /// Whether any cell may carry grapheme clusters.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the key.
    pub fn has_graphemes(self) -> Result<bool> {
        self.get::<ffi::keys::row_data::Grapheme>()
    }

    /// Whether any cell may be styled.
    ///
    /// # Errors
    ///
    /// Fails if the library rejects the key.
    pub fn has_styles(self) -> Result<bool> {
        self.get::<ffi::keys::row_data::Styled>()
    }
}

fn style_color(color: &ffi::GhosttyStyleColor) -> StyleColor {
    match color.tag {
        ffi::GHOSTTY_STYLE_COLOR_PALETTE => {
            // SAFETY: style.h `GhosttyStyleColor`: `palette` is the active
            // field when `tag` is `GHOSTTY_STYLE_COLOR_PALETTE`.
            let palette = unsafe { color.value.palette };
            StyleColor::Palette(palette)
        }
        ffi::GHOSTTY_STYLE_COLOR_RGB => {
            // SAFETY: `rgb` is the active field when `tag` is
            // `GHOSTTY_STYLE_COLOR_RGB`.
            let rgb = unsafe { color.value.rgb };
            StyleColor::Rgb(rgb.into())
        }
        _ => StyleColor::None,
    }
}

pub(crate) fn style_from_ffi(style: &ffi::GhosttyStyle) -> Style {
    Style {
        foreground: style_color(&style.fg_color),
        background: style_color(&style.bg_color),
        bold: style.bold,
        italic: style.italic,
        faint: style.faint,
        inverse: style.inverse,
        invisible: style.invisible,
        strikethrough: style.strikethrough,
        underline: style.underline != ffi::GHOSTTY_SGR_UNDERLINE_NONE,
    }
}

/// An owned `GhosttyRenderState`.
#[derive(Debug)]
pub(crate) struct NativeRenderState {
    ptr: NonNull<ffi::GhosttyRenderStateImpl>,
}

impl NativeRenderState {
    pub(crate) fn new(allocator: AllocatorRef) -> Result<Self> {
        let mut raw: ffi::GhosttyRenderState = ptr::null_mut();
        // SAFETY: render.h `ghostty_render_state_new` accepts NULL or a live
        // allocator and writes the handle to `raw`.
        let code = unsafe {
            ffi::ghostty_render_state_new(
                allocator_ptr(allocator),
                &raw mut raw,
            )
        };
        Error::from_code(code)?;
        NonNull::new(raw)
            .map(|ptr| Self { ptr })
            .ok_or(Error::InvalidValue)
    }

    /// Captures the terminal. Exclusive access to both ensures no row or
    /// cell cursor from the previous capture survives.
    pub(crate) fn update(
        &mut self,
        terminal: &mut NativeTerminal,
    ) -> Result<()> {
        // SAFETY: render.h `ghostty_render_state_update` needs both live
        // handles and exclusive terminal access, which `&mut` provides.
        let code = unsafe {
            ffi::ghostty_render_state_update(
                self.ptr.as_ptr(),
                terminal.ptr.as_ptr(),
            )
        };
        Error::from_code(code)
    }

    pub(crate) fn get<K: RenderStateData>(
        &self,
        out: &mut K::Out,
    ) -> Result<()> {
        // SAFETY: render.h `ghostty_render_state_get` writes the documented
        // output type for `K::KEY`, which the generated table names
        // `K::Out`; sized outputs arrive initialized by the caller.
        let code = unsafe {
            ffi::ghostty_render_state_get(
                self.ptr.as_ptr(),
                K::KEY,
                ptr::from_mut(out).cast(),
            )
        };
        Error::from_code(code)
    }

    /// Marks the global and every row's dirty state consumed.
    pub(crate) fn clean(&mut self) -> Result<()> {
        // SAFETY: render.h `ghostty_render_state_clean` takes a live handle.
        let code =
            unsafe { ffi::ghostty_render_state_clean(self.ptr.as_ptr()) };
        Error::from_code(code)
    }

    /// Points `iterator` at this capture's rows.
    pub(crate) fn rows<'s>(
        &'s self,
        iterator: &'s mut NativeRowIterator,
    ) -> Result<RowCursor<'s>> {
        // render.h: `GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR` fills the
        // pre-allocated iterator named by the handle.
        let mut handle = iterator.ptr.as_ptr();
        self.get::<ffi::keys::render_state_data::RowIterator>(&mut handle)?;
        Ok(RowCursor {
            ptr: iterator.ptr,
            _state: PhantomData,
        })
    }
}

impl Drop for NativeRenderState {
    fn drop(&mut self) {
        // SAFETY: render.h `ghostty_render_state_free` releases a handle
        // from `ghostty_render_state_new`.
        unsafe { ffi::ghostty_render_state_free(self.ptr.as_ptr()) }
    }
}

/// An owned, reusable render-state row iterator.
#[derive(Debug)]
pub(crate) struct NativeRowIterator {
    ptr: NonNull<ffi::GhosttyRenderStateRowIteratorImpl>,
}

impl NativeRowIterator {
    pub(crate) fn new(allocator: AllocatorRef) -> Result<Self> {
        let mut raw: ffi::GhosttyRenderStateRowIterator = ptr::null_mut();
        // SAFETY: render.h `ghostty_render_state_row_iterator_new` accepts
        // NULL or a live allocator and writes the handle to `raw`.
        let code = unsafe {
            ffi::ghostty_render_state_row_iterator_new(
                allocator_ptr(allocator),
                &raw mut raw,
            )
        };
        Error::from_code(code)?;
        NonNull::new(raw)
            .map(|ptr| Self { ptr })
            .ok_or(Error::InvalidValue)
    }
}

impl Drop for NativeRowIterator {
    fn drop(&mut self) {
        // SAFETY: render.h `ghostty_render_state_row_iterator_free` releases
        // a handle from `ghostty_render_state_row_iterator_new`.
        unsafe {
            ffi::ghostty_render_state_row_iterator_free(self.ptr.as_ptr());
        }
    }
}

/// A populated row iterator, valid while its render state is unchanged.
#[derive(Debug)]
pub(crate) struct RowCursor<'s> {
    ptr: NonNull<ffi::GhosttyRenderStateRowIteratorImpl>,
    _state: PhantomData<(&'s NativeRenderState, &'s mut NativeRowIterator)>,
}

impl<'s> RowCursor<'s> {
    pub(crate) fn next(&mut self) -> bool {
        // SAFETY: render.h `ghostty_render_state_row_iterator_next` advances
        // an iterator populated from a render state that `'s` keeps intact.
        unsafe {
            ffi::ghostty_render_state_row_iterator_next(self.ptr.as_ptr())
        }
    }

    pub(crate) fn get<K: RenderRowData>(&self, out: &mut K::Out) -> Result<()> {
        // SAFETY: render.h `ghostty_render_state_row_get` writes the
        // documented output type for `K::KEY` (`K::Out`), or returns
        // INVALID_VALUE before the first `next`.
        let code = unsafe {
            ffi::ghostty_render_state_row_get(
                self.ptr.as_ptr(),
                K::KEY,
                ptr::from_mut(out).cast(),
            )
        };
        Error::from_code(code)
    }

    /// Points `cells` at the current row's cells.
    pub(crate) fn cells<'c>(
        &'c self,
        cells: &'c mut NativeRowCells,
    ) -> Result<CellCursor<'c>>
    where
        's: 'c,
    {
        // render.h: `GHOSTTY_RENDER_STATE_ROW_DATA_CELLS` fills the
        // pre-allocated row cells named by the handle.
        let mut handle = cells.ptr.as_ptr();
        self.get::<ffi::keys::render_row_data::Cells>(&mut handle)?;
        Ok(CellCursor {
            ptr: cells.ptr,
            _row: PhantomData,
        })
    }
}

/// An owned, reusable render-state row cells container.
#[derive(Debug)]
pub(crate) struct NativeRowCells {
    ptr: NonNull<ffi::GhosttyRenderStateRowCellsImpl>,
}

impl NativeRowCells {
    pub(crate) fn new(allocator: AllocatorRef) -> Result<Self> {
        let mut raw: ffi::GhosttyRenderStateRowCells = ptr::null_mut();
        // SAFETY: render.h `ghostty_render_state_row_cells_new` accepts NULL
        // or a live allocator and writes the handle to `raw`.
        let code = unsafe {
            ffi::ghostty_render_state_row_cells_new(
                allocator_ptr(allocator),
                &raw mut raw,
            )
        };
        Error::from_code(code)?;
        NonNull::new(raw)
            .map(|ptr| Self { ptr })
            .ok_or(Error::InvalidValue)
    }
}

impl Drop for NativeRowCells {
    fn drop(&mut self) {
        // SAFETY: render.h `ghostty_render_state_row_cells_free` releases a
        // handle from `ghostty_render_state_row_cells_new`.
        unsafe { ffi::ghostty_render_state_row_cells_free(self.ptr.as_ptr()) }
    }
}

/// Populated row cells, valid while their row cursor stays on the row.
#[derive(Debug)]
pub(crate) struct CellCursor<'c> {
    ptr: NonNull<ffi::GhosttyRenderStateRowCellsImpl>,
    _row: PhantomData<&'c mut NativeRowCells>,
}

impl CellCursor<'_> {
    pub(crate) fn next(&mut self) -> bool {
        // SAFETY: render.h `ghostty_render_state_row_cells_next` advances
        // cells populated from a row that the cursor lifetime keeps intact.
        unsafe { ffi::ghostty_render_state_row_cells_next(self.ptr.as_ptr()) }
    }

    pub(crate) fn get<K: RenderCellData>(
        &self,
        out: &mut K::Out,
    ) -> Result<()> {
        // SAFETY: render.h `ghostty_render_state_row_cells_get` writes the
        // documented output type for `K::KEY` (`K::Out`), or returns
        // INVALID_VALUE before the first `next`.
        let code = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.ptr.as_ptr(),
                K::KEY,
                ptr::from_mut(out).cast(),
            )
        };
        Error::from_code(code)
    }
}

/// An owned mouse encoder.
#[derive(Debug)]
pub(crate) struct NativeMouseEncoder {
    ptr: NonNull<ffi::GhosttyMouseEncoderImpl>,
}

impl NativeMouseEncoder {
    pub(crate) fn new(allocator: AllocatorRef) -> Result<Self> {
        let mut raw: ffi::GhosttyMouseEncoder = ptr::null_mut();
        // SAFETY: mouse/encoder.h `ghostty_mouse_encoder_new` accepts NULL or
        // a live allocator and writes the handle to `raw`.
        let code = unsafe {
            ffi::ghostty_mouse_encoder_new(
                allocator_ptr(allocator),
                &raw mut raw,
            )
        };
        Error::from_code(code)?;
        NonNull::new(raw)
            .map(|ptr| Self { ptr })
            .ok_or(Error::InvalidValue)
    }

    pub(crate) fn set<K: MouseEncoderOption>(&mut self, value: &K::Value) {
        // SAFETY: mouse/encoder.h `ghostty_mouse_encoder_setopt` reads the
        // documented value type for `K::KEY`, which the generated table
        // names `K::Value`.
        unsafe {
            ffi::ghostty_mouse_encoder_setopt(
                self.ptr.as_ptr(),
                K::KEY,
                ptr::from_ref(value).cast(),
            );
        }
    }

    pub(crate) fn sync(&mut self, terminal: TerminalRef<'_>) {
        // SAFETY: mouse/encoder.h `ghostty_mouse_encoder_setopt_from_terminal`
        // reads tracking and format from a live terminal.
        unsafe {
            ffi::ghostty_mouse_encoder_setopt_from_terminal(
                self.ptr.as_ptr(),
                terminal.raw(),
            );
        }
    }

    pub(crate) fn encode(
        &mut self,
        event: &NativeMouseEvent,
        buffer: &mut [u8],
    ) -> Result<Fill> {
        let mut len = 0;
        // SAFETY: mouse/encoder.h `ghostty_mouse_encoder_encode` writes at
        // most `out_buf_size` bytes to `out_buf` and the count to `out_len`.
        let code = unsafe {
            ffi::ghostty_mouse_encoder_encode(
                self.ptr.as_ptr(),
                event.ptr.as_ptr(),
                buffer_ptr(buffer).cast(),
                buffer.len(),
                &raw mut len,
            )
        };
        fill(code, len)
    }
}

impl Drop for NativeMouseEncoder {
    fn drop(&mut self) {
        // SAFETY: mouse/encoder.h `ghostty_mouse_encoder_free` releases a
        // handle from `ghostty_mouse_encoder_new`.
        unsafe { ffi::ghostty_mouse_encoder_free(self.ptr.as_ptr()) }
    }
}

/// An owned mouse event.
#[derive(Debug)]
pub(crate) struct NativeMouseEvent {
    ptr: NonNull<ffi::GhosttyMouseEventImpl>,
}

impl NativeMouseEvent {
    pub(crate) fn new(allocator: AllocatorRef) -> Result<Self> {
        let mut raw: ffi::GhosttyMouseEvent = ptr::null_mut();
        // SAFETY: mouse/event.h `ghostty_mouse_event_new` accepts NULL or a
        // live allocator and writes the handle to `raw`.
        let code = unsafe {
            ffi::ghostty_mouse_event_new(allocator_ptr(allocator), &raw mut raw)
        };
        Error::from_code(code)?;
        NonNull::new(raw)
            .map(|ptr| Self { ptr })
            .ok_or(Error::InvalidValue)
    }

    pub(crate) fn set_action(&mut self, action: MouseAction) {
        let action = match action {
            MouseAction::Press => ffi::GHOSTTY_MOUSE_ACTION_PRESS,
            MouseAction::Release => ffi::GHOSTTY_MOUSE_ACTION_RELEASE,
            MouseAction::Motion => ffi::GHOSTTY_MOUSE_ACTION_MOTION,
        };
        // SAFETY: mouse/event.h `ghostty_mouse_event_set_action` takes a
        // live event.
        unsafe {
            ffi::ghostty_mouse_event_set_action(self.ptr.as_ptr(), action);
        }
    }

    pub(crate) fn set_button(&mut self, button: Option<MouseButton>) {
        let Some(button) = button else {
            // SAFETY: mouse/event.h `ghostty_mouse_event_clear_button` takes
            // a live event.
            unsafe { ffi::ghostty_mouse_event_clear_button(self.ptr.as_ptr()) };
            return;
        };
        let button = match button {
            MouseButton::Left => ffi::GHOSTTY_MOUSE_BUTTON_LEFT,
            MouseButton::Right => ffi::GHOSTTY_MOUSE_BUTTON_RIGHT,
            MouseButton::Middle => ffi::GHOSTTY_MOUSE_BUTTON_MIDDLE,
        };
        // SAFETY: mouse/event.h `ghostty_mouse_event_set_button` takes a live
        // event.
        unsafe {
            ffi::ghostty_mouse_event_set_button(self.ptr.as_ptr(), button);
        }
    }

    pub(crate) fn set_position(&mut self, x: f32, y: f32) {
        // SAFETY: mouse/event.h `ghostty_mouse_event_set_position` takes a
        // live event.
        unsafe {
            ffi::ghostty_mouse_event_set_position(
                self.ptr.as_ptr(),
                ffi::GhosttyMousePosition { x, y },
            );
        }
    }
}

impl Drop for NativeMouseEvent {
    fn drop(&mut self) {
        // SAFETY: mouse/event.h `ghostty_mouse_event_free` releases a handle
        // from `ghostty_mouse_event_new`.
        unsafe { ffi::ghostty_mouse_event_free(self.ptr.as_ptr()) }
    }
}
