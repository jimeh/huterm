//! Plain values exchanged with the terminal. None of them borrow native
//! memory.

use crate::ffi;

/// An RGB color.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Rgb {
    /// Red channel.
    pub red: u8,
    /// Green channel.
    pub green: u8,
    /// Blue channel.
    pub blue: u8,
}

impl Rgb {
    /// Creates a color from its channels.
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }
}

impl From<ffi::GhosttyColorRgb> for Rgb {
    fn from(color: ffi::GhosttyColorRgb) -> Self {
        Self::new(color.r, color.g, color.b)
    }
}

impl From<Rgb> for ffi::GhosttyColorRgb {
    fn from(color: Rgb) -> Self {
        Self {
            r: color.red,
            g: color.green,
            b: color.blue,
        }
    }
}

/// A 256-color palette.
pub type Palette = [Rgb; 256];

pub(crate) fn palette_from_ffi(
    palette: &[ffi::GhosttyColorRgb; 256],
) -> Palette {
    std::array::from_fn(|index| palette[index].into())
}

pub(crate) fn palette_to_ffi(palette: &Palette) -> [ffi::GhosttyColorRgb; 256] {
    std::array::from_fn(|index| palette[index].into())
}

/// Which screen buffer is active.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Screen {
    /// The primary screen with scrollback.
    Primary,
    /// The alternate screen.
    Alternate,
}

/// Viewport position within the scrollable area, in rows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Scrollbar {
    /// Rows in the scrollable area, including the viewport.
    pub total: u64,
    /// Top-based row of the viewport's first row.
    pub offset: u64,
    /// Rows in the viewport.
    pub len: u64,
}

/// A viewport movement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scroll {
    /// The top of history.
    Top,
    /// The live screen.
    Bottom,
    /// Rows to move; negative values move toward history.
    Delta(isize),
    /// Top-based row that becomes the viewport's first row.
    Row(usize),
}

/// A terminal mode identifier, packed like the header's `GhosttyMode`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mode(pub(crate) ffi::GhosttyMode);

impl Mode {
    /// DEC private mode 1, application cursor keys (DECCKM).
    pub const CURSOR_KEYS: Self = Self::dec(1);
    /// DEC private mode 25, cursor visible (DECTCEM).
    pub const CURSOR_VISIBLE: Self = Self::dec(25);
    /// DEC private mode 1000, button mouse tracking.
    pub const NORMAL_MOUSE: Self = Self::dec(1000);
    /// DEC private mode 1004, focus events.
    pub const FOCUS_EVENT: Self = Self::dec(1004);
    /// DEC private mode 1006, SGR mouse format.
    pub const SGR_MOUSE: Self = Self::dec(1006);
    /// DEC private mode 2004, bracketed paste.
    pub const BRACKETED_PASTE: Self = Self::dec(2004);

    /// A DEC private (`?`-prefixed) mode.
    #[must_use]
    pub const fn dec(value: u16) -> Self {
        Self(ffi::mode(value, false))
    }

    /// An ANSI mode.
    #[must_use]
    pub const fn ansi(value: u16) -> Self {
        Self(ffi::mode(value, true))
    }
}

/// The color scheme reported for `CSI ? 996 n`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorScheme {
    /// A light background.
    Light,
    /// A dark background.
    Dark,
}

/// Device attribute answers for `CSI c`, `CSI > c`, and `CSI = c`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceAttributes {
    /// DA1 conformance level, such as [`DeviceAttributes::VT220`].
    pub conformance_level: u16,
    /// DA1 feature codes; at most 64 are reported.
    pub features: Vec<u16>,
    /// DA2 terminal type.
    pub device_type: u16,
    /// DA2 firmware version.
    pub firmware_version: u16,
    /// DA2 ROM cartridge number.
    pub rom_cartridge: u16,
    /// DA3 unit ID.
    pub unit_id: u32,
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the header's DA constants are small preprocessor integers"
)]
impl DeviceAttributes {
    /// DA1 conformance level for a VT220.
    pub const VT220: u16 = ffi::GHOSTTY_DA_CONFORMANCE_VT220 as u16;
    /// DA1 feature code for ANSI color.
    pub const FEATURE_ANSI_COLOR: u16 =
        ffi::GHOSTTY_DA_FEATURE_ANSI_COLOR as u16;
    /// DA2 terminal type for a VT220.
    pub const DEVICE_TYPE_VT220: u16 = ffi::GHOSTTY_DA_DEVICE_TYPE_VT220 as u16;

    pub(crate) fn to_ffi(&self) -> ffi::GhosttyDeviceAttributes {
        let mut attributes = ffi::GhosttyDeviceAttributes::default();
        let count = self.features.len().min(attributes.primary.features.len());
        attributes.primary.conformance_level = self.conformance_level;
        attributes.primary.features[..count]
            .copy_from_slice(&self.features[..count]);
        attributes.primary.num_features = count;
        attributes.secondary.device_type = self.device_type;
        attributes.secondary.firmware_version = self.firmware_version;
        attributes.secondary.rom_cartridge = self.rom_cartridge;
        attributes.tertiary.unit_id = self.unit_id;
        attributes
    }
}

/// Where a clipboard write goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ClipboardLocation {
    /// The standard system clipboard.
    Standard,
    /// The selection clipboard.
    Selection,
    /// The primary selection.
    Primary,
    /// A location this binding does not know.
    Unknown(i32),
}

impl ClipboardLocation {
    pub(crate) const fn from_ffi(
        location: ffi::GhosttyClipboardLocation,
    ) -> Self {
        match location {
            ffi::GHOSTTY_CLIPBOARD_LOCATION_STANDARD => Self::Standard,
            ffi::GHOSTTY_CLIPBOARD_LOCATION_SELECTION => Self::Selection,
            ffi::GHOSTTY_CLIPBOARD_LOCATION_PRIMARY => Self::Primary,
            other => Self::Unknown(other),
        }
    }
}

/// One MIME representation in a clipboard write, borrowed for the
/// callback's duration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClipboardContent<'a> {
    /// MIME type bytes.
    pub mime: &'a [u8],
    /// Decoded, binary-safe data.
    pub data: &'a [u8],
}

/// A program's request to write a clipboard, borrowed for the callback's
/// duration. OSC 52, iTerm2 OSC 1337 `Copy`, and Kitty OSC 5522 all arrive
/// in this shape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipboardWrite<'a> {
    /// Destination clipboard.
    pub location: ClipboardLocation,
    /// Representations of one value; empty requests clearing.
    pub contents: Vec<ClipboardContent<'a>>,
    /// Program name from the Kitty protocol; empty otherwise.
    pub name: &'a [u8],
    /// Whether a Kitty session grant already covers this request.
    pub granted: bool,
    /// Whether the Kitty request carried a session password.
    pub can_remember: bool,
}

/// The answer to a clipboard write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ClipboardWriteResult {
    /// The write completed.
    Success,
    /// Policy or the user denied the write.
    Denied,
    /// The destination or a representation is unsupported.
    Unsupported,
    /// The clipboard is temporarily unavailable.
    Busy,
    /// A representation contains invalid data.
    InvalidData,
    /// Writing failed.
    IoError,
}

impl ClipboardWriteResult {
    pub(crate) const fn to_ffi(self) -> ffi::GhosttyClipboardWriteResult {
        match self {
            Self::Success => ffi::GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS,
            Self::Denied => ffi::GHOSTTY_CLIPBOARD_WRITE_RESULT_DENIED,
            Self::Unsupported => {
                ffi::GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED
            }
            Self::Busy => ffi::GHOSTTY_CLIPBOARD_WRITE_RESULT_BUSY,
            Self::InvalidData => {
                ffi::GHOSTTY_CLIPBOARD_WRITE_RESULT_INVALID_DATA
            }
            Self::IoError => ffi::GHOSTTY_CLIPBOARD_WRITE_RESULT_IO_ERROR,
        }
    }
}

/// A side effect a write produced, in callback order.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Effect {
    /// Bytes the terminal answers to the program.
    PtyWrite(Vec<u8>),
    /// A BEL character.
    Bell,
    /// The title changed; read it with `Terminal::title`.
    TitleChanged,
    /// The working directory changed to these raw bytes, as the program
    /// sent them (an OSC 7 URI or a bare path).
    PwdChanged(Vec<u8>),
}

/// The render state's global damage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Dirty {
    /// Nothing changed.
    Clean,
    /// Some rows changed; check each row's flag.
    Partial,
    /// Every row must be redrawn.
    Full,
}

/// The cursor's visual style.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CursorStyle {
    /// A vertical bar.
    Bar,
    /// A filled block.
    Block,
    /// An underline.
    Underline,
    /// A hollow block.
    BlockHollow,
}

/// Cursor state from the render state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cursor {
    /// Whether terminal modes make the cursor visible.
    pub visible: bool,
    /// Visual style.
    pub style: CursorStyle,
    /// Column and row within the viewport, when the cursor is inside it.
    pub position: Option<(u16, u16)>,
}

/// Colors from the render state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderColors {
    /// Effective background.
    pub background: Rgb,
    /// Effective foreground.
    pub foreground: Rgb,
    /// Cursor color when terminal state sets one explicitly.
    pub cursor: Option<Rgb>,
    /// Effective palette.
    pub palette: Palette,
}

/// A cell value copied out of the grid. Reading its fields needs no
/// terminal, so it stays valid after the terminal changes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cell(pub(crate) ffi::GhosttyCell);

/// A row value copied out of the grid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Row(pub(crate) ffi::GhosttyRow);

/// What a cell holds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CellContent {
    /// One codepoint, zero when empty.
    Codepoint,
    /// A codepoint followed by more grapheme codepoints.
    Grapheme,
    /// No text; a background palette index.
    BackgroundPalette,
    /// No text; a background RGB color.
    BackgroundRgb,
    /// A tag this binding does not know.
    Unknown(i32),
}

/// A cell's width role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CellWidth {
    /// One column.
    Narrow,
    /// The leading column of a wide character.
    Wide,
    /// The column after a wide character.
    SpacerTail,
    /// Padding at a soft-wrapped row's end before a wide character.
    SpacerHead,
    /// A value this binding does not know.
    Unknown(i32),
}

impl CellWidth {
    /// Whether the cell is a spacer rather than a character.
    #[must_use]
    pub const fn is_spacer(self) -> bool {
        matches!(self, Self::SpacerTail | Self::SpacerHead)
    }
}

/// A style color.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StyleColor {
    /// No color; the default applies.
    None,
    /// A palette index.
    Palette(u8),
    /// An RGB value.
    Rgb(Rgb),
}

/// A cell's style.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "SGR attributes are independent flags"
)]
pub struct Style {
    /// Foreground color.
    pub foreground: StyleColor,
    /// Background color.
    pub background: StyleColor,
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Faint (dim).
    pub faint: bool,
    /// Reverse video.
    pub inverse: bool,
    /// Invisible (hidden).
    pub invisible: bool,
    /// Struck through.
    pub strikethrough: bool,
    /// Any underline style.
    pub underline: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            foreground: StyleColor::None,
            background: StyleColor::None,
            bold: false,
            italic: false,
            faint: false,
            inverse: false,
            invisible: false,
            strikethrough: false,
            underline: false,
        }
    }
}

/// A grid coordinate system.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PointSpace {
    /// The active area where the cursor moves.
    Active,
    /// The visible viewport, which may be scrolled into history.
    Viewport,
    /// History and the active area; resolving it walks the scrollback.
    Screen,
}

/// A cell position in one coordinate system.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Point {
    /// Coordinate system.
    pub space: PointSpace,
    /// Column.
    pub x: u16,
    /// Row.
    pub y: u32,
}

/// Outcome of filling a caller buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fill {
    /// This many elements were written.
    Written(usize),
    /// The buffer was too small; this many elements are required.
    TooSmall(usize),
}

/// Build configuration of the linked library.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Optimize {
    /// Zig `Debug`.
    Debug,
    /// Zig `ReleaseSafe`.
    ReleaseSafe,
    /// Zig `ReleaseSmall`.
    ReleaseSmall,
    /// Zig `ReleaseFast`.
    ReleaseFast,
    /// A value this binding does not know.
    Unknown(i32),
}
