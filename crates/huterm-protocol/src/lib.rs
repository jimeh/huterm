//! Dependency-neutral messages and snapshots shared by `HUTerm` clients.

#![deny(missing_docs)]

use std::path::PathBuf;

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

opaque_id!(SessionId, "Identifies a server-owned session.");
opaque_id!(TabId, "Identifies a tab within a session.");
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

/// Client-owned viewport selection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Viewport {
    /// Number of rows above the live screen bottom.
    pub bottom_offset: usize,
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

/// Keyboard modifiers attached to a terminal key.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers {
    /// Control modifier.
    pub control: bool,
    /// Alt or Option modifier.
    pub alt: bool,
    /// Shift modifier.
    pub shift: bool,
}

/// Structured terminal input from a client.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TerminalInput {
    /// Text composed by the platform input system.
    Text(String),
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
}

/// A command used to start a terminal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalCommand {
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
    /// Resolved foreground color.
    pub foreground: Rgb,
    /// Resolved background color.
    pub background: Rgb,
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
}

/// Immutable viewport snapshot produced by the terminal runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalSnapshot {
    /// Terminal this snapshot belongs to.
    pub terminal_id: TerminalId,
    /// Monotonic terminal generation.
    pub generation: u64,
    /// Grid size represented by `cells`.
    pub size: GridSize,
    /// Row-major cells, exactly `size.rows * size.columns` entries.
    pub cells: Vec<Cell>,
    /// Cursor when it falls within this viewport.
    pub cursor: Option<Cursor>,
    /// Modes current at this generation.
    pub modes: TerminalModes,
    /// Maximum valid client scroll offset.
    pub history_size: usize,
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
