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
    RenderCellPopulate, RenderRowData, RenderRowPopulate, RenderStateData,
    RenderStatePopulate, RowData, TerminalCallback, TerminalData,
    TerminalOption, TerminalPointer,
};
use crate::types::{
    Cell, CellContent, CellWidth, Fill, Point, PointSpace, Rgb, Row, Scroll,
    Style, StyleColor,
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
        type Key = ffi::keys::render_state_data::RowIterator;
        let mut handle: <Key as RenderStatePopulate>::Out =
            iterator.ptr.as_ptr();
        // SAFETY: render.h `GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR` reads
        // the handle from `out` and fills the live iterator it names, which
        // `'s` borrows exclusively.
        let code = unsafe {
            ffi::ghostty_render_state_get(
                self.ptr.as_ptr(),
                Key::KEY,
                ptr::from_mut(&mut handle).cast(),
            )
        };
        Error::from_code(code)?;
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
        type Key = ffi::keys::render_row_data::Cells;
        let mut handle: <Key as RenderRowPopulate>::Out = cells.ptr.as_ptr();
        // SAFETY: render.h `GHOSTTY_RENDER_STATE_ROW_DATA_CELLS` reads the
        // handle from `out` and fills the live row cells it names, which
        // `'c` borrows exclusively.
        let code = unsafe {
            ffi::ghostty_render_state_row_get(
                self.ptr.as_ptr(),
                Key::KEY,
                ptr::from_mut(&mut handle).cast(),
            )
        };
        Error::from_code(code)?;
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

    /// Writes the current cell's grapheme cluster as UTF-8 into `buffer`;
    /// `Written(0)` means the cell has no text.
    pub(crate) fn graphemes_utf8(&self, buffer: &mut [u8]) -> Result<Fill> {
        type Key = ffi::keys::render_cell_data::GraphemesUtf8;
        let mut out: <Key as RenderCellPopulate>::Out = ffi::GhosttyBuffer {
            ptr: buffer_ptr(buffer),
            cap: buffer.len(),
            len: 0,
        };
        // SAFETY: render.h `GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8`
        // writes at most `cap` bytes through `ptr` (NULL when empty), which
        // name the exclusive `buffer`, and the count or required size to
        // `len`; it returns INVALID_VALUE before the first `next`.
        let code = unsafe {
            ffi::ghostty_render_state_row_cells_get(
                self.ptr.as_ptr(),
                Key::KEY,
                ptr::from_mut(&mut out).cast(),
            )
        };
        fill(code, out.len)
    }
}

/// The synthetic geometry the mouse probe encodes against: a 200x200 surface
/// of 1x1 cells.
///
/// Ghostty converts encoder geometry and event positions with unchecked
/// float-to-integer casts: grid sizes into `u16` (`renderer/size.zig`) and
/// positions into `i32` pixels (`input/mouse_encode.zig`). A surface 65536
/// cells wide, or a huge or NaN position, is undefined behavior in release
/// builds, so the encoder only ever receives these constants.
const PROBE_GEOMETRY: (u32, u32) = (200, 1);
/// The probe position; (100, 100) is column 100, past the single-byte range
/// so UTF-8 encoding is visible.
const PROBE_POSITION: f32 = 100.0;

/// An event the mouse probe encodes, always at the probe position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProbeEvent {
    /// Motion without a button.
    Motion,
    /// Motion with the left button held.
    LeftDrag,
    /// A left button press.
    LeftPress,
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

    fn set<K: MouseEncoderOption>(&mut self, value: &K::Value) {
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

    /// Reports every event kind, overriding the synced tracking mode.
    pub(crate) fn track_any_event(&mut self) {
        self.set::<ffi::keys::mouse_encoder_option::Event>(
            &ffi::GHOSTTY_MOUSE_TRACKING_ANY,
        );
    }

    /// Sets the probe's fixed geometry, without padding.
    pub(crate) fn set_probe_geometry(&mut self) {
        let (screen, cell) = PROBE_GEOMETRY;
        let mut size: ffi::GhosttyMouseEncoderSize = ffi::sized();
        size.screen_width = screen;
        size.screen_height = screen;
        size.cell_width = cell;
        size.cell_height = cell;
        self.set::<ffi::keys::mouse_encoder_option::Size>(&size);
    }

    pub(crate) fn set_any_button_pressed(&mut self, pressed: bool) {
        self.set::<ffi::keys::mouse_encoder_option::AnyButtonPressed>(&pressed);
    }

    pub(crate) fn set_track_last_cell(&mut self, track: bool) {
        self.set::<ffi::keys::mouse_encoder_option::TrackLastCell>(&track);
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

    /// Describes `event` at the probe position.
    pub(crate) fn set(&mut self, event: ProbeEvent) {
        let (action, left) = match event {
            ProbeEvent::Motion => (ffi::GHOSTTY_MOUSE_ACTION_MOTION, false),
            ProbeEvent::LeftDrag => (ffi::GHOSTTY_MOUSE_ACTION_MOTION, true),
            ProbeEvent::LeftPress => (ffi::GHOSTTY_MOUSE_ACTION_PRESS, true),
        };
        let event = self.ptr.as_ptr();
        // SAFETY: mouse/event.h `ghostty_mouse_event_set_action` takes a
        // live event.
        unsafe { ffi::ghostty_mouse_event_set_action(event, action) };
        if left {
            // SAFETY: mouse/event.h `ghostty_mouse_event_set_button` takes a
            // live event.
            unsafe {
                ffi::ghostty_mouse_event_set_button(
                    event,
                    ffi::GHOSTTY_MOUSE_BUTTON_LEFT,
                );
            }
        } else {
            // SAFETY: mouse/event.h `ghostty_mouse_event_clear_button` takes
            // a live event.
            unsafe { ffi::ghostty_mouse_event_clear_button(event) };
        }
        let position = ffi::GhosttyMousePosition {
            x: PROBE_POSITION,
            y: PROBE_POSITION,
        };
        // SAFETY: mouse/event.h `ghostty_mouse_event_set_position` takes a
        // live event.
        unsafe { ffi::ghostty_mouse_event_set_position(event, position) };
    }
}

impl Drop for NativeMouseEvent {
    fn drop(&mut self) {
        // SAFETY: mouse/event.h `ghostty_mouse_event_free` releases a handle
        // from `ghostty_mouse_event_new`.
        unsafe { ffi::ghostty_mouse_event_free(self.ptr.as_ptr()) }
    }
}

/// Measures how many bytes getters write and setters read, for the key
/// contract tests.
#[cfg(test)]
pub(crate) mod key_probe {
    use super::{
        BuildInfo, CellCursor, CellData, Error, MouseEncoderOption,
        NativeMouseEncoder, NativeRenderState, NativeRowCells,
        NativeRowIterator, NativeTerminal, RenderCellData, RenderRowData,
        RenderRowPopulate, RenderStateData, RenderStatePopulate, Result,
        RowCursor, RowData, TerminalData, TerminalOption, TerminalRef, c_void,
        ffi, ptr,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use std::mem::MaybeUninit;

    use crate::test_layout::{self, Part};
    use crate::types::{Cell, Row};

    /// Guard bytes probed after each value.
    const GUARD: usize = 64;

    /// What a getter wrote into its output.
    #[derive(Debug, Eq, PartialEq)]
    pub(crate) struct Written {
        /// The output's Rust type.
        pub(crate) type_name: &'static str,
        /// Declared bytes at or past the kept input that the call left
        /// untouched.
        pub(crate) untouched: Vec<usize>,
        /// Whether any byte after the output changed.
        pub(crate) overran: bool,
    }

    /// 16-byte-aligned bytes that a foreign call may write, including
    /// uninitialized padding, so they are read only where a field lives.
    struct Storage {
        words: Vec<MaybeUninit<u128>>,
        len: usize,
    }

    impl Storage {
        /// `len` bytes, all set to `byte`.
        fn new(len: usize, byte: u8) -> Self {
            let mut words = vec![MaybeUninit::uninit(); len.div_ceil(16)];
            // SAFETY: `words` holds at least `len` writable bytes.
            unsafe {
                words.as_mut_ptr().cast::<u8>().write_bytes(byte, len);
            }
            Self { words, len }
        }

        fn as_mut_ptr(&mut self) -> *mut c_void {
            self.words.as_mut_ptr().cast()
        }

        /// Copies `value`'s first `len` bytes to the start.
        fn place<T>(&mut self, value: &T, len: usize) {
            assert!(len <= size_of::<T>() && len <= self.len);
            // SAFETY: `value` has `size_of::<T>()` readable bytes and the
            // storage at least `len` writable ones; an untyped copy may
            // carry `value`'s padding.
            unsafe {
                ptr::copy_nonoverlapping(
                    ptr::from_ref(value).cast::<u8>(),
                    self.words.as_mut_ptr().cast::<u8>(),
                    len,
                );
            }
        }

        /// Reads one byte.
        ///
        /// # Safety
        ///
        /// `offset` must lie in a declared field of the output or in the
        /// guard, which this storage initialized and the library either
        /// left alone or wrote as part of a value.
        unsafe fn read(&self, offset: usize) -> u8 {
            // SAFETY: the words hold `self.len` bytes; viewing them as
            // possibly uninitialized bytes reads nothing.
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    self.words.as_ptr().cast::<MaybeUninit<u8>>(),
                    self.len,
                )
            };
            // SAFETY: initialized per the caller's contract.
            unsafe { bytes[offset].assume_init() }
        }

        /// Reads the bytes of every declared part, following each tag to
        /// its active union member.
        fn declared(&self, parts: &[Part], out: &mut BTreeMap<usize, u8>) {
            for part in parts {
                match part {
                    Part::Bytes(range) => {
                        for offset in range.clone() {
                            // SAFETY: `range` is a declared field.
                            out.insert(offset, unsafe { self.read(offset) });
                        }
                    }
                    Part::Tagged { tag, arms } => {
                        let mut bytes = [0; 8];
                        for (index, offset) in tag.clone().enumerate() {
                            // SAFETY: the tag is a declared field.
                            bytes[index] = unsafe { self.read(offset) };
                            out.insert(offset, bytes[index]);
                        }
                        let value = tag_value(&bytes[..tag.len()]);
                        let (_, active) = arms
                            .iter()
                            .find(|(arm, _)| *arm == value)
                            .unwrap_or_else(|| {
                                panic!(
                                    "tag at {tag:?} holds {value}, which \
                                     names no union member"
                                )
                            });
                        self.declared(active, out);
                    }
                }
            }
        }
    }

    /// A native-endian signed tag of one, two, four, or eight bytes.
    fn tag_value(bytes: &[u8]) -> i64 {
        match bytes.len() {
            1 => i64::from(i8::from_ne_bytes([bytes[0]])),
            2 => i64::from(i16::from_ne_bytes(bytes.try_into().unwrap())),
            4 => i64::from(i32::from_ne_bytes(bytes.try_into().unwrap())),
            8 => i64::from_ne_bytes(bytes.try_into().unwrap()),
            width => panic!("unsupported tag width {width}"),
        }
    }

    /// Calls `call` on an output of `T`'s size whose first `keep` bytes come
    /// from `input`, with the rest and a guard prefilled with 0x00 and then
    /// 0xFF. A declared byte that differs between the runs was never
    /// written; padding is never read.
    fn probe<T>(
        input: &T,
        keep: usize,
        mut call: impl FnMut(*mut c_void) -> ffi::GhosttyResult,
    ) -> Result<Written> {
        let size = size_of::<T>();
        let parts = test_layout::of::<T>();
        let mut runs = Vec::new();
        for pattern in [0x00, 0xFF] {
            let mut out = Storage::new(size + GUARD, pattern);
            out.place(input, keep);
            Error::from_code(call(out.as_mut_ptr()))?;
            let mut declared = BTreeMap::new();
            out.declared(&parts, &mut declared);
            let overran = (size..size + GUARD)
                // SAFETY: the guard is storage this probe initialized.
                .any(|offset| unsafe { out.read(offset) } != pattern);
            runs.push((declared, overran));
        }
        let [(first, low), (second, high)] = &runs[..] else {
            unreachable!("two runs");
        };
        Ok(Written {
            type_name: std::any::type_name::<T>(),
            untouched: first
                .keys()
                .chain(second.keys())
                .copied()
                .filter(|&offset| offset >= keep)
                .filter(|offset| first.get(offset) != second.get(offset))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            overran: *low || *high,
        })
    }

    pub(crate) fn terminal<K: TerminalData>(
        terminal: TerminalRef<'_>,
        input: &K::Out,
        keep: usize,
    ) -> Result<Written> {
        probe(input, keep, |out| {
            // SAFETY: terminal.h `ghostty_terminal_get`; `out` holds
            // `size_of::<K::Out>()` bytes plus a guard, with any input the
            // key reads kept from `input`.
            unsafe { ffi::ghostty_terminal_get(terminal.raw(), K::KEY, out) }
        })
    }

    pub(crate) fn render_state<K: RenderStateData>(
        state: &NativeRenderState,
        input: &K::Out,
        keep: usize,
    ) -> Result<Written> {
        probe(input, keep, |out| {
            // SAFETY: render.h `ghostty_render_state_get`; `out` holds
            // `size_of::<K::Out>()` bytes plus a guard.
            unsafe {
                ffi::ghostty_render_state_get(state.ptr.as_ptr(), K::KEY, out)
            }
        })
    }

    /// Probes the row-iterator key, which reads a handle from `out`.
    pub(crate) fn row_iterator(
        state: &NativeRenderState,
        iterator: &mut NativeRowIterator,
    ) -> Result<Written> {
        type Key = ffi::keys::render_state_data::RowIterator;
        let handle: <Key as RenderStatePopulate>::Out = iterator.ptr.as_ptr();
        probe(&handle, size_of_val(&handle), |out| {
            // SAFETY: render.h `GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR` reads
            // the kept handle to the exclusively borrowed iterator and fills
            // it.
            unsafe {
                ffi::ghostty_render_state_get(state.ptr.as_ptr(), Key::KEY, out)
            }
        })
    }

    pub(crate) fn render_row<K: RenderRowData>(
        row: &RowCursor<'_>,
        input: &K::Out,
        keep: usize,
    ) -> Result<Written> {
        probe(input, keep, |out| {
            // SAFETY: render.h `ghostty_render_state_row_get` on a cursor
            // positioned on a row; `out` holds the output plus a guard.
            unsafe {
                ffi::ghostty_render_state_row_get(row.ptr.as_ptr(), K::KEY, out)
            }
        })
    }

    /// Probes the row-cells key, which reads a handle from `out`.
    pub(crate) fn row_cells(
        row: &RowCursor<'_>,
        cells: &mut NativeRowCells,
    ) -> Result<Written> {
        type Key = ffi::keys::render_row_data::Cells;
        let handle: <Key as RenderRowPopulate>::Out = cells.ptr.as_ptr();
        probe(&handle, size_of_val(&handle), |out| {
            // SAFETY: render.h `GHOSTTY_RENDER_STATE_ROW_DATA_CELLS` reads
            // the kept handle to the exclusively borrowed row cells and
            // fills them.
            unsafe {
                ffi::ghostty_render_state_row_get(
                    row.ptr.as_ptr(),
                    Key::KEY,
                    out,
                )
            }
        })
    }

    pub(crate) fn render_cell<K: RenderCellData>(
        cells: &CellCursor<'_>,
        input: &K::Out,
        keep: usize,
    ) -> Result<Written> {
        probe(input, keep, |out| {
            // SAFETY: render.h `ghostty_render_state_row_cells_get` on a
            // cursor positioned on a cell; `out` holds the output plus a
            // guard.
            unsafe {
                ffi::ghostty_render_state_row_cells_get(
                    cells.ptr.as_ptr(),
                    K::KEY,
                    out,
                )
            }
        })
    }

    pub(crate) fn cell<K: CellData>(
        cell: Cell,
        input: &K::Out,
    ) -> Result<Written> {
        probe(input, 0, |out| {
            // SAFETY: screen.h `ghostty_cell_get` decodes into `out`, which
            // holds the output plus a guard.
            unsafe { ffi::ghostty_cell_get(cell.0, K::KEY, out) }
        })
    }

    pub(crate) fn row<K: RowData>(row: Row, input: &K::Out) -> Result<Written> {
        probe(input, 0, |out| {
            // SAFETY: screen.h `ghostty_row_get` decodes into `out`, which
            // holds the output plus a guard.
            unsafe { ffi::ghostty_row_get(row.0, K::KEY, out) }
        })
    }

    pub(crate) fn build_info<K: BuildInfo>(input: &K::Out) -> Result<Written> {
        probe(input, 0, |out| {
            // SAFETY: build_info.h `ghostty_build_info` writes into `out`,
            // which holds the output plus a guard.
            unsafe { ffi::ghostty_build_info(K::KEY, out) }
        })
    }

    /// Sets a terminal option from `value` followed by `guard` bytes, so a
    /// library that reads past the value sees them.
    pub(crate) fn set_terminal<K: TerminalOption>(
        terminal: &mut NativeTerminal,
        value: &K::Value,
        guard: u8,
    ) -> Result<()> {
        let size = size_of::<K::Value>();
        let mut input = Storage::new(size + GUARD, guard);
        input.place(value, size);
        // SAFETY: terminal.h `ghostty_terminal_set` reads `K::Value` from
        // `input`, which holds it plus a guard.
        let code = unsafe {
            ffi::ghostty_terminal_set(
                terminal.ptr.as_ptr(),
                K::KEY,
                input.as_mut_ptr().cast_const(),
            )
        };
        Error::from_code(code)
    }

    /// Sets a mouse encoder option from `value` followed by `guard` bytes.
    pub(crate) fn set_mouse<K: MouseEncoderOption>(
        encoder: &mut NativeMouseEncoder,
        value: &K::Value,
        guard: u8,
    ) {
        let size = size_of::<K::Value>();
        let mut input = Storage::new(size + GUARD, guard);
        input.place(value, size);
        // SAFETY: mouse/encoder.h `ghostty_mouse_encoder_setopt` reads
        // `K::Value` from `input`, which holds it plus a guard.
        unsafe {
            ffi::ghostty_mouse_encoder_setopt(
                encoder.ptr.as_ptr(),
                K::KEY,
                input.as_mut_ptr().cast_const(),
            );
        }
    }
}
