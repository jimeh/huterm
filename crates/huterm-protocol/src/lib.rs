//! Dependency-neutral messages and snapshots shared by Huterm clients.

#![deny(missing_docs)]

use std::path::PathBuf;
use std::sync::Arc;

mod command;
pub use command::{
    ArgumentKind, ArgumentSpec, CommandArgument, CommandError, CommandId,
    CommandInvocation, CommandOutcome, CommandScope, CommandSpec, CommandValue,
    catalog, ids, lookup, validate,
};

macro_rules! opaque_id {
    ($name:ident, $docs:literal) => {
        #[doc = $docs]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(u64);

        impl $name {
            #[doc = "Creates an identifier from its process-local numeric value."]
            #[must_use]
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            #[doc = "Returns the process-local numeric value."]
            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }
        }
    };
}

opaque_id!(
    RuntimeId,
    "Identifies one runtime incarnation within the current process."
);

macro_rules! scoped_id {
    ($name:ident, $docs:literal) => {
        #[doc = $docs]
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name {
            runtime: RuntimeId,
            value: u64,
        }
        impl $name {
            /// Creates an unscoped identifier for fixtures or restoration input.
            /// Structural runtime APIs reject it until assigned the correct scope.
            #[must_use]
            pub const fn new(value: u64) -> Self {
                Self::in_runtime(RuntimeId::new(0), value)
            }
            /// Creates an identifier in a runtime incarnation.
            #[must_use]
            pub const fn in_runtime(runtime: RuntimeId, value: u64) -> Self {
                Self { runtime, value }
            }
            /// Returns the numeric identity within the runtime.
            #[must_use]
            pub const fn get(self) -> u64 {
                self.value
            }
            /// Returns the owning runtime incarnation.
            #[must_use]
            pub const fn runtime(self) -> RuntimeId {
                self.runtime
            }
        }
    };
}

scoped_id!(AttachmentId, "Identifies one session view attachment.");
scoped_id!(SessionId, "Identifies a runtime-owned session.");
scoped_id!(WorkspaceId, "Identifies a runtime-owned workspace.");
scoped_id!(TabId, "Identifies a tab within a workspace.");
opaque_id!(PaneId, "Identifies a pane within a tab.");
opaque_id!(TerminalId, "Identifies a terminal runtime.");

/// Terminal grid size in character cells.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GridSize {
    /// Number of columns.
    pub columns: u16,
    /// Number of visible rows.
    pub rows: u16,
}

impl GridSize {
    /// Creates a grid size, clamping both dimensions to at least one cell.
    #[must_use]
    pub fn clamped(columns: u16, rows: u16) -> Self {
        Self {
            columns: columns.max(1),
            rows: rows.max(1),
        }
    }
}

/// Terminal cell pixel size used to resize the PTY.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CellSize {
    /// Cell width in pixels.
    pub width: u16,
    /// Cell height in pixels.
    pub height: u16,
}

/// Shared terminal viewport owned by the runtime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Viewport {
    /// Number of rows above the live screen bottom.
    pub bottom_offset: usize,
}

/// Emulator selected once when a terminal starts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TerminalEngineKind {
    /// Alacritty 0.26.0, available in every build.
    #[default]
    Alacritty,
    /// Ghostty via libghostty-vt, available in every Huterm build.
    Ghostty,
}

impl TerminalEngineKind {
    /// Stable configuration and benchmark name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Alacritty => "alacritty",
            Self::Ghostty => "ghostty",
        }
    }
}

/// Ordered movement of the shared terminal viewport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScrollCommand {
    /// Move toward history for positive values, toward live output for negative.
    Relative(i64),
    /// Set a bottom-relative offset, clamped to retained history.
    Absolute(usize),
    /// Follow live output.
    Live,
}

/// A keyboard key whose terminal encoding depends on emulator modes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TerminalKey {
    /// Enter or return.
    Enter,
    /// Horizontal tab.
    Tab,
    /// Backspace.
    Backspace,
    /// Escape.
    Escape,
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Delete.
    Delete,
}

/// Keyboard modifiers attached to terminal input.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers {
    /// Control modifier.
    pub control: bool,
    /// Alt or Option modifier.
    pub alt: bool,
    /// Shift modifier.
    pub shift: bool,
}

/// Application mouse tracking requested by the terminal.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MouseTracking {
    /// No mouse reporting.
    #[default]
    Disabled,
    /// Button and wheel events, mode 1000.
    Buttons,
    /// Also report motion with a held button, mode 1002.
    ButtonMotion,
    /// Also report hover motion, mode 1003.
    AllMotion,
}

/// Wire format for mouse reports, independent of tracking.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MouseEncoding {
    /// Byte coordinates.
    #[default]
    Legacy,
    /// UTF-8 coordinates, mode 1005.
    Utf8,
    /// Decimal coordinates, mode 1006.
    Sgr,
}

/// Zero-based position in the live terminal grid.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MousePosition {
    /// Column from the left edge.
    pub column: u32,
    /// Row from the top edge.
    pub row: u32,
}

/// Supported physical mouse buttons.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseButton {
    /// Primary button.
    Left,
    /// Middle button.
    Middle,
    /// Secondary button.
    Right,
}

/// Direction of one application wheel step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WheelDirection {
    /// Upward scroll.
    Up,
    /// Downward scroll.
    Down,
    /// Leftward scroll.
    Left,
    /// Rightward scroll.
    Right,
}

/// Mouse action, with wheel releases excluded by construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseAction {
    /// A physical button was pressed.
    Press(MouseButton),
    /// A physical button was released.
    Release(MouseButton),
    /// Motion with the most recently pressed held button, or hover.
    Motion(Option<MouseButton>),
    /// One wheel step.
    Wheel(WheelDirection),
}

/// Structured mouse input interpreted using current runtime modes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MouseInput {
    /// Live-grid coordinates, clamped by the runtime before encoding.
    pub position: MousePosition,
    /// Physical action.
    pub action: MouseAction,
    /// Modifiers at the time of the event.
    pub modifiers: Modifiers,
}

/// Structured terminal input from a client.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TerminalInput {
    /// Text composed by the platform input system.
    Text(String),
    /// A character chord, interpreted by the client before enqueueing.
    Character {
        /// Character text, including any Control transformation.
        text: String,
        /// Prefix the character with ESC for terminal Meta input.
        meta: bool,
    },
    /// A terminal key and its modifiers.
    Key {
        /// Logical key.
        key: TerminalKey,
        /// Active keyboard modifiers.
        modifiers: Modifiers,
    },
    /// Explicit paste content.
    Paste(String),
    /// Focus state for terminal focus-reporting mode.
    Focus(bool),
    /// Application mouse action.
    Mouse(MouseInput),
}

/// A command used to start a terminal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalCommand {
    /// Engine captured at creation; existing terminals retain their engine.
    pub engine: TerminalEngineKind,
    /// Executable path.
    pub program: PathBuf,
    /// Arguments excluding the executable itself.
    pub arguments: Vec<String>,
    /// Initial working directory.
    pub working_directory: PathBuf,
    /// Environment overrides.
    pub environment: Vec<(String, String)>,
    /// Initial character grid.
    pub grid_size: GridSize,
    /// Initial cell pixel size.
    pub cell_size: CellSize,
}

/// RGB color independent of a renderer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgb {
    /// Red channel.
    pub red: u8,
    /// Green channel.
    pub green: u8,
    /// Blue channel.
    pub blue: u8,
}

/// A terminal color which clients resolve through their active theme.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CellColor {
    /// Client theme foreground.
    DefaultForeground,
    /// Client theme background.
    DefaultBackground,
    /// Client theme cursor color.
    Cursor,
    /// Entry in the terminal's 256-color palette.
    Indexed(u8),
    /// Explicit RGB color supplied by terminal content.
    Rgb(Rgb),
}

/// Renderable terminal cell style.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "terminal cell flags are independent"
)]
pub struct CellStyle {
    /// Bold text.
    pub bold: bool,
    /// Dim text.
    pub dim: bool,
    /// Italic text.
    pub italic: bool,
    /// Underlined text.
    pub underline: bool,
    /// Struck-through text.
    pub strikeout: bool,
    /// Hidden text.
    pub hidden: bool,
    /// Cell occupies the leading half of a wide glyph.
    pub wide: bool,
    /// Cell is the spacer after a wide glyph.
    pub wide_spacer: bool,
}

/// One cell in a terminal snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cell {
    /// Cell text, including combining characters.
    pub text: String,
    /// Semantic foreground color.
    pub foreground: CellColor,
    /// Semantic background color.
    pub background: CellColor,
    /// Text and width attributes.
    pub style: CellStyle,
}

/// Cursor shape rendered by a client.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorShape {
    /// Filled rectangular cursor.
    Block,
    /// Underline cursor.
    Underline,
    /// Vertical bar cursor.
    Beam,
    /// Hidden cursor.
    Hidden,
}

/// Cursor state within a snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cursor {
    /// Zero-based row in the snapshot.
    pub row: u16,
    /// Zero-based column.
    pub column: u16,
    /// Rendered cursor shape.
    pub shape: CursorShape,
}

/// A point in canonical terminal scrollback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferPoint {
    /// Number of rows above the live screen bottom.
    pub rows_from_live_bottom: usize,
    /// Zero-based terminal column.
    pub column: u16,
}

/// An inclusive ordered range in canonical terminal scrollback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferRange {
    /// Earlier endpoint in terminal reading order.
    pub start: BufferPoint,
    /// Later endpoint in terminal reading order.
    pub end: BufferPoint,
}

impl BufferRange {
    /// Creates a range with endpoints ordered in terminal reading order.
    #[must_use]
    pub fn ordered(first: BufferPoint, second: BufferPoint) -> Self {
        if buffer_point_precedes(first, second) {
            Self {
                start: first,
                end: second,
            }
        } else {
            Self {
                start: second,
                end: first,
            }
        }
    }
}

fn buffer_point_precedes(first: BufferPoint, second: BufferPoint) -> bool {
    first.rows_from_live_bottom > second.rows_from_live_bottom
        || (first.rows_from_live_bottom == second.rows_from_live_bottom
            && first.column <= second.column)
}

/// Emulator modes that clients need for input and presentation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "terminal modes are independent protocol flags"
)]
pub struct TerminalModes {
    /// Application cursor-key mode.
    pub application_cursor: bool,
    /// Alternate screen is active.
    pub alternate_screen: bool,
    /// Bracketed paste mode.
    pub bracketed_paste: bool,
    /// Focus reporting mode.
    pub focus_reporting: bool,
    /// Requested application mouse events.
    pub mouse_tracking: MouseTracking,
    /// Mouse report wire format.
    pub mouse_encoding: MouseEncoding,
}

/// One immutable row shared between complete snapshot generations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalRow {
    /// Cells in display-column order.
    pub cells: Vec<Cell>,
}

/// Immutable viewport snapshot produced by the terminal runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalSnapshot {
    /// Terminal this snapshot belongs to.
    pub terminal_id: TerminalId,
    /// Monotonic terminal generation.
    pub generation: u64,
    /// Grid size represented by `rows`.
    pub size: GridSize,
    /// Complete viewport rows, each containing `size.columns` cells.
    pub rows: Vec<Arc<TerminalRow>>,
    /// Cursor when it falls within this viewport.
    pub cursor: Option<Cursor>,
    /// Modes current at this generation.
    pub modes: TerminalModes,
    /// Actual viewport used after clamping the requested offset.
    pub viewport: Viewport,
    /// Maximum valid shared scroll offset.
    pub history_size: usize,
    /// Effective cursor color override, when terminal content defines one.
    pub cursor_color: Option<Rgb>,
}

impl TerminalSnapshot {
    /// Iterates the viewport cells without copying their contents.
    pub fn cells(&self) -> impl Iterator<Item = &Cell> {
        self.rows.iter().flat_map(|row| row.cells.iter())
    }
}

/// Child-process exit status independent of a platform process type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExitStatus {
    /// Exit code when the platform supplied one.
    pub code: Option<u32>,
    /// Whether the process reported success.
    pub success: bool,
}

/// Asynchronous runtime event delivered to clients.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TerminalEvent {
    /// A new terminal is ready.
    Ready(TerminalId),
    /// Snapshot data changed. Clients may coalesce these notifications.
    Invalidated {
        /// Changed terminal.
        terminal_id: TerminalId,
        /// Latest available generation.
        generation: u64,
    },
    /// Terminal title changed.
    TitleChanged {
        /// Changed terminal.
        terminal_id: TerminalId,
        /// New title.
        title: String,
    },
    /// Terminal bell rang.
    Bell(TerminalId),
    /// Child process exited.
    Exited {
        /// Exited terminal.
        terminal_id: TerminalId,
        /// Child exit status.
        status: ExitStatus,
    },
    /// Startup or runtime failure.
    Failed {
        /// Failed terminal.
        terminal_id: TerminalId,
        /// Human-readable failure.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_ranges_are_ordered_by_reading_position() {
        let upper = BufferPoint {
            rows_from_live_bottom: 8,
            column: 4,
        };
        let lower = BufferPoint {
            rows_from_live_bottom: 3,
            column: 1,
        };
        assert_eq!(
            BufferRange::ordered(lower, upper),
            BufferRange {
                start: upper,
                end: lower
            }
        );

        let left = BufferPoint {
            rows_from_live_bottom: 3,
            column: 1,
        };
        let right = BufferPoint {
            rows_from_live_bottom: 3,
            column: 7,
        };
        assert_eq!(BufferRange::ordered(right, left).start, left);
    }
}

/// Origin of an activatable terminal hyperlink.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkSource {
    /// A detected HTTP or HTTPS URL in terminal text.
    PlainText,
    /// An explicit OSC 8 destination.
    Osc8,
}

/// One visible cell belonging to a link, retained for activation validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkCell {
    /// Cell position in the paired snapshot's viewport.
    pub position: MousePosition,
    /// Visible text, including combining characters and empty wide spacers.
    pub text: String,
}

/// Complete destination and visible identity resolved from canonical state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalLink {
    /// Validated HTTP or HTTPS destination, without truncation.
    pub destination: String,
    /// Whether the destination came from OSC 8 or plain text.
    pub source: LinkSource,
    /// Ordered visible cells belonging to this link.
    pub cells: Vec<LinkCell>,
}

/// Nonfatal outcome of an optional snapshot-paired link lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinkLookup {
    /// A complete activatable target.
    Match(TerminalLink),
    /// The requested cell has no supported destination.
    NoMatch,
    /// A bounded scan could not prove a complete destination.
    ScanLimit,
    /// Buffer inspection failed; the paired snapshot remains usable.
    Unavailable,
}
