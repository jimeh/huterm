//! Dependency-neutral messages and snapshots shared by Huterm clients.

#![deny(missing_docs)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

mod capture;
mod cell_text;
mod command;
mod hierarchy;
mod layout_template;
mod pane_layout;
mod viewer;
pub use capture::{
    CaptureError, MAX_DIRECTORY_CANDIDATES, RuntimeCapture, SessionCapture,
    TerminalCapture, TerminalLaunch, WorkspaceCapture,
};
pub use cell_text::{CellText, MAX_CELL_TEXT_BYTES};
pub use command::{
    ArgumentKind, ArgumentSpec, CommandArgument, CommandError, CommandId,
    CommandInvocation, CommandOutcome, CommandScope, CommandSpec, CommandValue,
    Requirement, SIDES, catalog, ids, lookup, validate, validate_supplied,
};
pub use hierarchy::{
    ApplyOutcome, DigestProfile, HIERARCHY_DIGEST_INTERVAL, HierarchyDigester,
    HierarchyEnvelope, HierarchyEvent, HierarchyState, LayoutChange,
    MAX_HIERARCHY_ITEMS, MAX_NAME_BYTES, ResyncReason, SessionInfo, StreamId,
    TabChange, TabInfo, TabRecord, TabSnapshot, Touched, WorkspaceInfo,
    resolve_tab_name,
};
pub use layout_template::{
    BUILT_IN_LAYOUTS, LayoutTemplate, MAX_LAYOUT_NAME_LEN, MAX_TEMPLATE_DEPTH,
    MAX_TEMPLATE_LEN, NamedLayout, TemplateError, TemplateErrorKind,
};
pub use pane_layout::{
    Arrangement, LayoutError, MAX_LAYOUT_DEPTH, MAX_SPLIT_CHILDREN,
    NORMALIZED_EXTENT, NodeId, NormalizedRect, PaneInfo, PaneLayout, PaneNode,
    SPLIT_TOTAL, SplitAxis, SplitChild, SplitDirection, SplitNode,
};
pub use viewer::{
    COMMAND_RECORD_CAPACITY, CommandRecord, CommandState, InputStamp,
    MAX_STATUS_TEXT_BYTES, StatusBaseline, StatusChanges,
    TERMINAL_FAILURE_CAPACITY, TerminalFailure, TerminalLifecycle,
    TerminalProgress, TerminalStatus, ViewerCapabilities,
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

/// The next value [`RuntimeId::allocate`] hands out. Zero stays reserved for
/// unscoped identities.
static NEXT_RUNTIME: AtomicU64 = AtomicU64::new(1);

impl RuntimeId {
    /// Allocates a runtime identity no other allocation in this process
    /// shares, so an embedded runtime and remote connections in one process
    /// never alias. Returns `None` once the identity space is exhausted.
    #[must_use]
    pub fn allocate() -> Option<Self> {
        NEXT_RUNTIME
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                next.checked_add(1)
            })
            .ok()
            .map(Self)
    }
}

/// Names one lifetime of a server's terminals: 16 random bytes drawn when a
/// server starts without its predecessor's terminals.
///
/// A live handover keeps it, so every identity it scopes stays valid; a
/// crash, a stop, or a restore from saved state draws a new one. It never
/// derives from process identity.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ServerIncarnation([u8; 16]);

impl ServerIncarnation {
    /// Wraps the incarnation's bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Returns the incarnation's bytes.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 16] {
        self.0
    }
}

impl std::fmt::Debug for ServerIncarnation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ServerIncarnation(")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        formatter.write_str(")")
    }
}

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
scoped_id!(
    ViewerId,
    "Identifies one client registration that displays a terminal."
);
scoped_id!(PaneId, "Identifies a pane within a tab's layout.");
scoped_id!(SplitId, "Identifies a split within a tab's layout.");
scoped_id!(TerminalId, "Identifies a terminal runtime.");
opaque_id!(
    HostEffectRecipientId,
    "Identifies one registered recipient of terminal host effects."
);

/// Normalized clipboard destination requested by terminal content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipboardDestination {
    /// The platform's ordinary system clipboard.
    System,
}

/// A write-only clipboard request produced by terminal content.
pub struct ClipboardWrite {
    /// Normalized destination selected by the terminal engine.
    pub destination: ClipboardDestination,
    text: String,
}

impl ClipboardWrite {
    /// Creates a clipboard write from decoded UTF-8 text.
    #[must_use]
    pub fn new(destination: ClipboardDestination, text: String) -> Self {
        Self { destination, text }
    }

    /// Returns the exact text to write, including embedded NUL bytes.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the retained allocation capacity charged to delivery budgets.
    #[must_use]
    pub fn allocation_capacity(&self) -> usize {
        self.text.capacity()
    }
}

impl std::fmt::Debug for ClipboardWrite {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClipboardWrite")
            .field("destination", &self.destination)
            .field("text", &"<redacted>")
            .finish()
    }
}

/// Characters core keeps of a notification's title; it ends longer titles
/// in an ellipsis.
pub const NOTIFICATION_TITLE_CHARS: usize = 128;

/// Characters core keeps of a notification's body; it ends longer bodies
/// in an ellipsis.
pub const NOTIFICATION_BODY_CHARS: usize = 512;

/// The most bytes a notification's title holds: every kept character at
/// four bytes, and the ellipsis.
pub const MAX_NOTIFICATION_TITLE_BYTES: usize =
    4 * NOTIFICATION_TITLE_CHARS + '…'.len_utf8();

/// The most bytes a notification's body holds: every kept character at
/// four bytes, and the ellipsis.
pub const MAX_NOTIFICATION_BODY_BYTES: usize =
    4 * NOTIFICATION_BODY_CHARS + '…'.len_utf8();

/// A desktop notification a program requested (OSC 9 or OSC 777).
///
/// Core builds it from untrusted terminal text: control and bidirectional
/// formatting characters are removed, whitespace is collapsed, and both
/// fields are bounded by [`NOTIFICATION_TITLE_CHARS`] and
/// [`NOTIFICATION_BODY_CHARS`]. Clients still present it only as plain
/// text.
#[derive(Clone, Eq, PartialEq)]
pub struct TerminalNotification {
    title: String,
    body: String,
}

impl TerminalNotification {
    /// Creates a notification from text core already normalized.
    #[must_use]
    pub fn new(title: String, body: String) -> Self {
        Self { title, body }
    }

    /// The program's title; empty when it sent none, as OSC 9 never does.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The notification text.
    #[must_use]
    pub fn body(&self) -> &str {
        &self.body
    }

    /// Returns the retained allocation capacity charged to delivery budgets.
    #[must_use]
    pub fn allocation_capacity(&self) -> usize {
        self.title.capacity().saturating_add(self.body.capacity())
    }
}

impl std::fmt::Debug for TerminalNotification {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalNotification")
            .field("title", &"<redacted>")
            .field("body", &"<redacted>")
            .finish()
    }
}

/// A client-mediated effect requested by terminal content.
#[derive(Debug)]
#[non_exhaustive]
pub enum HostEffect {
    /// Replace the selected clipboard with decoded terminal text.
    ClipboardWrite(ClipboardWrite),
    /// Present a desktop notification.
    Notification(TerminalNotification),
}

impl HostEffect {
    /// The kind whose eligibility and permission govern this effect.
    #[must_use]
    pub fn kind(&self) -> HostEffectKind {
        match self {
            Self::ClipboardWrite(_) => HostEffectKind::Clipboard,
            Self::Notification(_) => HostEffectKind::Notification,
        }
    }

    /// Returns the retained allocation capacity charged to delivery budgets.
    #[must_use]
    pub fn allocation_capacity(&self) -> usize {
        match self {
            Self::ClipboardWrite(write) => write.allocation_capacity(),
            Self::Notification(notification) => {
                notification.allocation_capacity()
            }
        }
    }
}

/// The kinds of host effect. A recipient is eligible for an effect only
/// while it supports and permits that kind, and each kind has its own
/// permission generation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HostEffectKind {
    /// Clipboard writes (OSC 52, OSC 1337 Copy, Kitty OSC 5522).
    Clipboard,
    /// Desktop notifications (OSC 9, OSC 777).
    Notification,
}

impl HostEffectKind {
    /// Every kind, in a stable order.
    pub const ALL: [Self; 2] = [Self::Clipboard, Self::Notification];

    /// This kind's position in [`Self::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Clipboard => 0,
            Self::Notification => 1,
        }
    }
}

/// Runtime ordering information for one host effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostEffectOrigin {
    /// Terminal whose parser produced the request.
    pub terminal_id: TerminalId,
    /// Monotonic sequence within this terminal runtime.
    pub sequence: u64,
}

/// Recipient selection stamped when an effect is admitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostEffectMetadata {
    /// Runtime ordering information.
    pub origin: HostEffectOrigin,
    /// Recipient selected at admission time.
    pub recipient: HostEffectRecipientId,
    /// Revocable recipient generation selected at admission time.
    pub recipient_generation: u64,
}

/// The most cells a terminal grid holds. Core clamps every grid it is given
/// (spawn, split, resize, and geometry reports) to it, so every viewport
/// stays encodable for a remote client.
pub const MAX_GRID_CELLS: usize = 1 << 20;

/// Terminal grid size in character cells.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GridSize {
    /// Number of columns.
    pub columns: u16,
    /// Number of visible rows.
    pub rows: u16,
}

impl GridSize {
    /// Creates a grid size with at least one cell in each dimension and at
    /// most [`MAX_GRID_CELLS`] cells, shrinking a larger grid while keeping
    /// its aspect.
    #[must_use]
    pub fn clamped(columns: u16, rows: u16) -> Self {
        let mut columns = columns.max(1);
        let mut rows = rows.max(1);
        let cells = usize::from(columns) * usize::from(rows);
        if cells > MAX_GRID_CELLS {
            // Both dimensions scale by the same factor, so the aspect stays.
            #[expect(
                clippy::cast_precision_loss,
                reason = "cell counts are far below f64's exact integer range"
            )]
            let scale = (MAX_GRID_CELLS as f64 / cells as f64).sqrt();
            let scaled = |value: u16| {
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "scale is below one, so the result fits u16"
                )]
                let scaled = (f64::from(value) * scale).floor() as u16;
                scaled.max(1)
            };
            columns = scaled(columns);
            rows = scaled(rows);
            // Rounding can leave the product a cell over; trim the longer
            // side until it fits.
            while usize::from(columns) * usize::from(rows) > MAX_GRID_CELLS {
                if columns >= rows {
                    columns -= 1;
                } else {
                    rows -= 1;
                }
            }
        }
        Self { columns, rows }
    }

    /// The number of cells.
    #[must_use]
    pub const fn cells(self) -> usize {
        self.columns as usize * self.rows as usize
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

/// How a terminal shown by several viewers chooses its size. Viewers that
/// take part are those with the `size` capability that have reported
/// geometry, shown or hidden. A manual size set on the terminal overrides
/// every policy until it is cleared.
///
/// Under [`Self::Smallest`] and [`Self::Largest`] the cell size is the
/// widest and the tallest cell any viewer reports, so the PTY's pixel size
/// suits the densest display and moving focus between viewers never
/// changes it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SizingPolicy {
    /// The grid and cell size of the viewer that most recently gained focus
    /// or typed.
    #[default]
    Latest,
    /// The fewest columns and the fewest rows any viewer reports, each
    /// taken separately, so every viewer shows the whole grid.
    Smallest,
    /// The most columns and the most rows any viewer reports, each taken
    /// separately.
    Largest,
    /// Viewers never resize the terminal: it keeps its size until a manual
    /// size is set. A terminal no viewer has reported geometry for yet
    /// takes the first report, so it keeps its laid-out size rather than
    /// the estimate it was spawned with.
    Manual,
}

/// Whether the presented terminal background is predominantly light or dark.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalAppearance {
    /// Light text area suitable for dark foreground content.
    Light,
    /// Dark text area suitable for light foreground content.
    Dark,
}

/// Classifies a terminal background using relative luminance in sRGB space.
#[must_use]
pub fn appearance_for_background(background: Rgb) -> TerminalAppearance {
    let linear = |channel: u8| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(background.red)
        + 0.7152 * linear(background.green)
        + 0.0722 * linear(background.blue);
    if luminance > 0.5 {
        TerminalAppearance::Light
    } else {
        TerminalAppearance::Dark
    }
}

/// Theme-controlled terminal colors before application OSC overrides.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalPresentation {
    /// Default foreground color.
    pub foreground: Rgb,
    /// Default background color.
    pub background: Rgb,
    /// Default cursor color.
    pub cursor: Rgb,
    /// Default indexed color palette.
    pub palette: [Rgb; 256],
}

impl Default for TerminalPresentation {
    fn default() -> Self {
        let rgb = |value: u32| {
            let [_, red, green, blue] = value.to_be_bytes();
            Rgb { red, green, blue }
        };
        let ansi = [
            rgb(0x0000_0000),
            rgb(0x00cd_0000),
            rgb(0x0000_cd00),
            rgb(0x00cd_cd00),
            rgb(0x0000_00ee),
            rgb(0x00cd_00cd),
            rgb(0x0000_cdcd),
            rgb(0x00e5_e5e5),
            rgb(0x007f_7f7f),
            rgb(0x00ff_0000),
            rgb(0x0000_ff00),
            rgb(0x00ff_ff00),
            rgb(0x005c_5cff),
            rgb(0x00ff_00ff),
            rgb(0x0000_ffff),
            rgb(0x00ff_ffff),
        ];
        let palette = std::array::from_fn(|index| {
            if let Some(color) = ansi.get(index) {
                return *color;
            }
            let index = u8::try_from(index).unwrap_or(u8::MAX);
            if index >= 232 {
                let level =
                    8_u8.saturating_add((index - 232).saturating_mul(10));
                return Rgb {
                    red: level,
                    green: level,
                    blue: level,
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
            Rgb {
                red: channel(value / 36),
                green: channel((value / 6) % 6),
                blue: channel(value % 6),
            }
        });
        Self {
            foreground: Rgb {
                red: 0xe5,
                green: 0xe5,
                blue: 0xe5,
            },
            background: Rgb {
                red: 0,
                green: 0,
                blue: 0,
            },
            cursor: Rgb {
                red: 0xff,
                green: 0xff,
                blue: 0xff,
            },
            palette,
        }
    }
}

/// Shared terminal viewport owned by the runtime.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Viewport {
    /// Number of rows above the live screen bottom.
    pub bottom_offset: usize,
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
    /// Insert.
    Insert,
    /// Function key 1.
    F1,
    /// Function key 2.
    F2,
    /// Function key 3.
    F3,
    /// Function key 4.
    F4,
    /// Function key 5.
    F5,
    /// Function key 6.
    F6,
    /// Function key 7.
    F7,
    /// Function key 8.
    F8,
    /// Function key 9.
    F9,
    /// Function key 10.
    F10,
    /// Function key 11.
    F11,
    /// Function key 12.
    F12,
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

/// Layout identity retained before platform shortcut normalization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CharacterKey {
    /// Unshifted character on the active layout.
    pub unshifted: char,
    /// Layout text with Shift, before Control or Meta transformations.
    pub text: String,
    /// Physical key's US-layout character, when known.
    pub base_layout: Option<char>,
    /// Modifiers, including raw Shift and the client's Meta policy.
    pub modifiers: Modifiers,
}

/// Effective Kitty keyboard flags. Clients use this only as a routing hint.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KittyKeyboardFlags(u8);
impl KittyKeyboardFlags {
    /// Retains only disambiguation (1) and alternate keys (4).
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits & 5)
    }
    /// Supported flags currently active.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }
    /// Whether disambiguation enables Kitty encoding and Kitty-only routing.
    #[must_use]
    pub const fn disambiguates(self) -> bool {
        self.0 & 1 != 0
    }
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
        /// Native key identity for dequeue-time Kitty encoding.
        key: Option<Box<CharacterKey>>,
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
    /// Application mouse action.
    Mouse(MouseInput),
}

/// Whether core may inject shell integration at launch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ShellIntegrationMode {
    /// Load integration for supported interactive shells.
    #[default]
    Detect,
    /// Start the original command without injection.
    Off,
}

/// Optional shell integration capabilities.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum ShellFeature {
    /// Append Huterm's executable directory to the shell's search path.
    Path,
}

/// A set of optional shell integration capabilities.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShellFeatures(std::collections::BTreeSet<ShellFeature>);

impl ShellFeatures {
    /// Whether a feature is enabled.
    #[must_use]
    pub fn contains(&self, feature: ShellFeature) -> bool {
        self.0.contains(&feature)
    }

    /// The enabled features, in order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = ShellFeature> + '_ {
        self.0.iter().copied()
    }
}

impl FromIterator<ShellFeature> for ShellFeatures {
    fn from_iter<T: IntoIterator<Item = ShellFeature>>(features: T) -> Self {
        Self(features.into_iter().collect())
    }
}

/// Integration requested by the client for a newly started terminal.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ShellIntegration {
    /// Launch policy.
    pub mode: ShellIntegrationMode,
    /// Optional capabilities.
    pub features: ShellFeatures,
}

impl ShellIntegration {
    /// No shell injection, used by command fixtures and explicit opt-out.
    pub const OFF: Self = Self {
        mode: ShellIntegrationMode::Off,
        features: ShellFeatures(std::collections::BTreeSet::new()),
    };
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
    /// Shell integration requested for this launch.
    pub shell_integration: ShellIntegration,
    /// Initial character grid.
    pub grid_size: GridSize,
    /// Initial cell pixel size.
    pub cell_size: CellSize,
    /// Initial presentation published before the child process starts.
    pub presentation: TerminalPresentation,
    /// History limit applied before the child process starts.
    pub scrollback: ScrollbackLimit,
    /// Whether the runtime compresses history while the terminal is idle,
    /// which releases most of its memory. Compressed history reads exactly
    /// like uncompressed history.
    pub scrollback_compression: bool,
}

/// How much live history a terminal keeps above its screen: a row limit,
/// counted in physical rows so a line that soft-wraps counts once for each
/// screen row it fills, and an optional limit on the bytes the engine
/// allocates for the screen and its history. Whichever is reached first
/// applies.
///
/// Neither is a memory budget. The runtime frees history in blocks of rows,
/// so retention can fall somewhat short of either limit; the configuration
/// reference states the guarantees.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ScrollbackLimit {
    rows: u32,
    bytes: Option<u64>,
}

impl ScrollbackLimit {
    /// Keeps no history.
    pub const NONE: Self = Self::rows_only(0);
    /// The default limit, 10,000 rows.
    pub const DEFAULT: Self = Self::rows_only(10_000);
    /// The largest row limit, 100,000 rows.
    pub const MAX: Self = Self::rows_only(100_000);
    /// The largest byte limit, 16 GiB.
    pub const MAX_BYTES: u64 = 16 * 1024 * 1024 * 1024;

    const fn rows_only(rows: u32) -> Self {
        Self { rows, bytes: None }
    }

    /// A limit of `rows` with no byte limit, or `None` when it exceeds
    /// [`Self::MAX`].
    #[must_use]
    pub const fn new(rows: u32) -> Option<Self> {
        if rows > Self::MAX.rows {
            None
        } else {
            Some(Self::rows_only(rows))
        }
    }

    /// This limit with a byte limit as well, or `None` when `bytes` exceeds
    /// [`Self::MAX_BYTES`].
    #[must_use]
    pub const fn with_bytes(self, bytes: u64) -> Option<Self> {
        if bytes > Self::MAX_BYTES {
            None
        } else {
            Some(Self {
                rows: self.rows,
                bytes: Some(bytes),
            })
        }
    }

    /// The limit in physical rows.
    #[must_use]
    pub const fn rows(self) -> u32 {
        self.rows
    }

    /// The byte limit, when one is set.
    #[must_use]
    pub const fn bytes(self) -> Option<u64> {
        self.bytes
    }
}

impl Default for ScrollbackLimit {
    fn default() -> Self {
        Self::DEFAULT
    }
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
    /// Cell text, including combining characters; [`CellText::EMPTY`] for a
    /// cell that holds none, spacers included.
    pub text: CellText,
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

    /// The smallest range that covers both ranges and everything between
    /// them.
    #[must_use]
    pub fn cover(self, other: Self) -> Self {
        let start = if buffer_point_precedes(self.start, other.start) {
            self.start
        } else {
            other.start
        };
        let end = if buffer_point_precedes(self.end, other.end) {
            other.end
        } else {
            self.end
        };
        Self { start, end }
    }
}

/// The unit a multi-click selection grows by: a double-click selects
/// words and a triple-click selects logical lines, which span soft wraps.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SelectionUnit {
    /// A run of word characters, a run of word boundaries, or a link.
    Word,
    /// A logical line across its soft wraps.
    Line,
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
    /// Supported Kitty keyboard flags, for client routing only.
    pub kitty_keyboard: KittyKeyboardFlags,
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
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TerminalRow {
    /// Cells in display-column order.
    pub cells: Vec<Cell>,
    /// The row soft-wraps into the next row: its text continues there
    /// without a line break, so copying joins the two rows.
    pub wrapped: bool,
    /// The row continues a soft-wrapped row. It usually matches the
    /// previous row's `wrapped`, but inserting or deleting lines clears it
    /// on the rows that move, and Copy reads it.
    pub wrap_continuation: bool,
    /// The shell integration (OSC 133) prompt mark of the row.
    pub prompt: RowPrompt,
}

/// Whether a row holds a shell prompt, as shell integration (OSC 133)
/// marks it. The mark covers the row, not where its prompt text ends.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum RowPrompt {
    /// The row holds no prompt.
    #[default]
    None,
    /// The row starts a prompt: the first row of the prompt before a
    /// command.
    Primary,
    /// The row continues a prompt, such as a later row of a multiline
    /// prompt or the prompt of a continued command line.
    Continuation,
}

/// Immutable viewport snapshot produced by the terminal runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalSnapshot {
    /// Terminal this snapshot belongs to.
    pub terminal_id: TerminalId,
    /// Monotonic terminal generation. Content changes advance it; scrolling
    /// and presentation changes do not.
    pub generation: u64,
    /// Publication revision: advances on every change that can alter a
    /// snapshot, including viewport moves and presentation changes.
    pub revision: u64,
    /// Advances on every canonical resize. Mouse reports carry it so the
    /// runtime can discard coordinates computed against an older grid.
    pub geometry_revision: u64,
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
    /// Effective OSC 11 default background override, cleared by OSC 111.
    pub default_background: Option<Rgb>,
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

/// A terminal-reported working directory, independent of an operating-system path type.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct TerminalDirectory {
    host: Option<String>,
    path: String,
    local: bool,
}

impl TerminalDirectory {
    /// Creates a bounded, normalized directory value owned by the runtime.
    #[must_use]
    pub fn new(host: Option<String>, path: String, local: bool) -> Self {
        Self { host, path, local }
    }

    /// Cuts the host and path to at most [`MAX_STATUS_TEXT_BYTES`] each.
    #[must_use]
    pub fn bounded(self) -> Self {
        let Self { host, path, local } = self;
        Self {
            host: host.map(bound_status_text),
            path: bound_status_text(path),
            local,
        }
    }

    /// Returns the reported URI host, when the report carried one.
    #[must_use]
    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    /// Returns the decoded absolute display path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns whether core classified the directory as local to this runtime host.
    #[must_use]
    pub fn is_local(&self) -> bool {
        self.local
    }
}

/// Cuts `text` to at most [`MAX_STATUS_TEXT_BYTES`] at a character
/// boundary.
#[must_use]
pub fn bound_status_text(mut text: String) -> String {
    text.truncate(text.floor_char_boundary(MAX_STATUS_TEXT_BYTES));
    text
}

/// Current non-cell terminal metadata.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct TerminalMetadata {
    directory: Option<TerminalDirectory>,
    foreground_process: Option<String>,
    foreground_title: Option<String>,
    shell_directory: Option<TerminalDirectory>,
}

impl TerminalMetadata {
    /// Creates a complete metadata replacement.
    #[must_use]
    pub fn new(
        directory: Option<TerminalDirectory>,
        foreground_process: Option<String>,
    ) -> Self {
        Self {
            directory,
            foreground_process,
            foreground_title: None,
            shell_directory: None,
        }
    }

    /// Cuts every string to at most [`MAX_STATUS_TEXT_BYTES`] at a
    /// character boundary, as core does when it publishes status.
    #[must_use]
    pub fn bounded(self) -> Self {
        let Self {
            directory,
            foreground_process,
            foreground_title,
            shell_directory,
        } = self;
        Self {
            directory: directory.map(TerminalDirectory::bounded),
            foreground_process: foreground_process.map(bound_status_text),
            foreground_title: foreground_title.map(bound_status_text),
            shell_directory: shell_directory.map(TerminalDirectory::bounded),
        }
    }

    /// Adds the root shell's last local directory report.
    #[must_use]
    pub fn with_shell_directory(
        mut self,
        directory: Option<TerminalDirectory>,
    ) -> Self {
        self.shell_directory = directory;
        self
    }

    /// Adds the title set by the process group that holds the foreground.
    #[must_use]
    pub fn with_foreground_title(mut self, title: Option<String>) -> Self {
        self.foreground_title = title;
        self
    }

    /// Returns the latest accepted directory report.
    #[must_use]
    pub fn directory(&self) -> Option<&TerminalDirectory> {
        self.directory.as_ref()
    }

    /// Returns the current best-effort foreground-process display name.
    #[must_use]
    pub fn foreground_process(&self) -> Option<&str> {
        self.foreground_process.as_deref()
    }

    /// Returns the title set by the process group that holds the foreground,
    /// if it set one. Unlike the terminal title, a job's title stops applying
    /// once the job leaves the foreground.
    #[must_use]
    pub fn foreground_title(&self) -> Option<&str> {
        self.foreground_title.as_deref()
    }

    /// Returns the root shell's last local OSC 7 report. Reports from other
    /// process groups, such as a remote shell under `ssh`, never change it,
    /// so it can stand in when [`Self::directory`] is remote.
    #[must_use]
    pub fn shell_directory(&self) -> Option<&TerminalDirectory> {
        self.shell_directory.as_ref()
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
    pub text: CellText,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollback_limits_accept_zero_through_the_maximum() {
        assert_eq!(ScrollbackLimit::new(0), Some(ScrollbackLimit::NONE));
        assert_eq!(ScrollbackLimit::new(100_000), Some(ScrollbackLimit::MAX));
        assert_eq!(ScrollbackLimit::new(100_001), None);
        assert_eq!(ScrollbackLimit::default().rows(), 10_000);
        assert_eq!(ScrollbackLimit::default().bytes(), None);
    }

    #[test]
    fn scrollback_byte_limits_accept_zero_through_16_gib() {
        let rows = ScrollbackLimit::DEFAULT;
        assert_eq!(
            rows.with_bytes(0).and_then(ScrollbackLimit::bytes),
            Some(0)
        );
        let largest = rows.with_bytes(16 << 30).unwrap();
        assert_eq!(largest.bytes(), Some(16 << 30));
        assert_eq!(largest.rows(), 10_000);
        assert_eq!(rows.with_bytes((16 << 30) + 1), None);
    }

    #[test]
    fn clamped_grids_hold_at_most_the_cell_cap_and_keep_their_aspect() {
        assert_eq!(
            GridSize::clamped(0, 0),
            GridSize {
                columns: 1,
                rows: 1
            }
        );
        assert_eq!(
            GridSize::clamped(80, 24),
            GridSize {
                columns: 80,
                rows: 24
            }
        );
        // Exactly at the cap stays.
        assert_eq!(GridSize::clamped(1024, 1024).cells(), MAX_GRID_CELLS);
        let square = GridSize::clamped(u16::MAX, u16::MAX);
        assert_eq!(
            square,
            GridSize {
                columns: 1024,
                rows: 1024
            }
        );
        let wide = GridSize::clamped(u16::MAX, 100);
        assert!(wide.cells() <= MAX_GRID_CELLS, "{wide:?}");
        assert!(wide.cells() > MAX_GRID_CELLS - usize::from(wide.columns));
        let ratio = f64::from(wide.columns) / f64::from(wide.rows);
        assert!((ratio - 655.35).abs() < 655.35 * 0.03, "{wide:?}");
        // One long dimension under the cap is untouched.
        assert_eq!(
            GridSize::clamped(u16::MAX, 1),
            GridSize {
                columns: u16::MAX,
                rows: 1
            }
        );
    }

    #[test]
    fn bounded_metadata_cuts_every_string_at_a_character_boundary() {
        let long = format!("{}\u{e9}", "x".repeat(MAX_STATUS_TEXT_BYTES - 1));
        let cut = "x".repeat(MAX_STATUS_TEXT_BYTES - 1);
        let directory =
            TerminalDirectory::new(Some(long.clone()), long.clone(), true);
        let metadata =
            TerminalMetadata::new(Some(directory.clone()), Some(long.clone()))
                .with_foreground_title(Some(long.clone()))
                .with_shell_directory(Some(directory))
                .bounded();
        for text in [
            metadata.directory().unwrap().host().unwrap(),
            metadata.directory().unwrap().path(),
            metadata.foreground_process().unwrap(),
            metadata.foreground_title().unwrap(),
            metadata.shell_directory().unwrap().host().unwrap(),
            metadata.shell_directory().unwrap().path(),
        ] {
            assert_eq!(text, cut);
        }
        assert_eq!(bound_status_text("short".into()), "short");
    }

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

    #[test]
    fn allocated_runtime_identities_are_distinct_and_never_unscoped() {
        let first = RuntimeId::allocate().unwrap();
        let second = RuntimeId::allocate().unwrap();
        assert_ne!(first, second);
        assert_ne!(first.get(), 0);
        assert_ne!(second.get(), 0);
    }

    #[test]
    fn server_incarnation_debug_prints_its_bytes_in_hex() {
        let mut bytes = [0; 16];
        bytes[0] = 0xab;
        bytes[15] = 0x01;
        assert_eq!(
            format!("{:?}", ServerIncarnation::from_bytes(bytes)),
            "ServerIncarnation(ab000000000000000000000000000001)"
        );
    }

    #[test]
    fn clipboard_write_debug_redacts_payload() {
        let write = ClipboardWrite::new(
            ClipboardDestination::System,
            "secret clipboard text".to_owned(),
        );
        let debug = format!("{write:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("secret clipboard text"));
    }
}
