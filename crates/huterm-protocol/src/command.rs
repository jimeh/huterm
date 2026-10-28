//! Command catalog, invocation values, and static validation.
//!
//! The catalog is the single definition of every user command: its stable ID,
//! execution scope, and named arguments in a fixed order. Clients bind, list,
//! and dispatch commands through it rather than restating names or arguments.

use std::fmt;

use crate::{NamedLayout, PaneId, SessionId, TabId, WorkspaceId};

/// Stable identifier of a catalog command, such as `select_tab`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommandId(&'static str);

impl CommandId {
    /// Wraps a static identifier. Prefer the constants in [`ids`].
    #[must_use]
    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }

    /// Returns the identifier text.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// Which owner executes a command.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CommandScope {
    /// Executed by the core runtime against canonical structure.
    Runtime,
    /// Executed by the client application, independent of any window.
    Application,
    /// Executed by one client window.
    Window,
    /// Executed by one terminal view.
    Terminal,
    /// Executed by the window's open command palette.
    Palette,
}

/// Declared type of one command argument.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ArgumentKind {
    /// Boolean flag.
    Bool,
    /// Integer within an inclusive range.
    Integer {
        /// Smallest accepted value.
        min: i64,
        /// Largest accepted value.
        max: i64,
    },
    /// Free text.
    Text,
    /// Name of a configured quake profile, resolved from client configuration;
    /// carried as [`CommandValue::Text`].
    QuakeProfile,
    /// Tab identity, supplied from client context rather than configuration.
    Tab,
    /// Workspace identity, supplied from client context.
    Workspace,
    /// Session identity, supplied from client context.
    Session,
    /// Pane identity, supplied from client context.
    Pane,
    /// A side of a pane: `left`, `right`, `up`, or `down`; carried as
    /// [`CommandValue::Text`].
    Side,
    /// Name of a pane layout, built in or configured, resolved by the
    /// client when the command runs; carried as [`CommandValue::Text`].
    /// Validation checks only the name's syntax
    /// ([`NamedLayout::valid_name`]).
    Layout,
}

/// The names a [`ArgumentKind::Side`] value accepts, in picker order.
pub const SIDES: [&str; 4] = ["left", "right", "up", "down"];

/// Whether an invocation must supply one command argument.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Requirement {
    /// Must be supplied.
    Always,
    /// May be omitted; the executor applies its documented default.
    Optional,
    /// Exactly one argument in the named group must be supplied.
    OneOf(&'static str),
}

/// One declared argument of a command.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ArgumentSpec {
    /// Argument name used by callers and configuration.
    pub name: &'static str,
    /// Accepted value type.
    pub kind: ArgumentKind,
    /// Whether and how an invocation must supply the argument.
    pub required: Requirement,
    /// Whether an interactive client prompts for this argument. Unprompted
    /// arguments exist for keybindings and programmatic callers only.
    pub prompt: bool,
}

impl ArgumentSpec {
    /// Returns whether this argument must always be supplied.
    #[must_use]
    pub const fn is_required(&self) -> bool {
        matches!(self.required, Requirement::Always)
    }

    /// Returns the one-of group containing this argument, if any.
    #[must_use]
    pub const fn group(&self) -> Option<&'static str> {
        match self.required {
            Requirement::OneOf(group) => Some(group),
            Requirement::Always | Requirement::Optional => None,
        }
    }
}

/// Catalog definition of one command.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CommandSpec {
    /// Stable identifier.
    pub id: CommandId,
    /// Short human-readable title for menus and palettes.
    pub title: &'static str,
    /// One-sentence description of the command's effect.
    pub description: &'static str,
    /// Owner that executes the command.
    pub scope: CommandScope,
    /// Key context a client must have captured for this command to be listed
    /// interactively. `None` lists the command everywhere.
    pub context: Option<&'static str>,
    /// Declared arguments in positional order.
    pub args: &'static [ArgumentSpec],
}

impl CommandSpec {
    /// Finds a declared argument by name.
    #[must_use]
    pub fn argument(&self, name: &str) -> Option<&'static ArgumentSpec> {
        self.args.iter().find(|spec| spec.name == name)
    }

    /// Returns arguments an interactive client must still collect in catalog
    /// order.
    ///
    /// This includes missing [`Requirement::Always`] arguments and one
    /// prompted member of each unsatisfied [`Requirement::OneOf`] group.
    #[must_use]
    pub fn missing_prompted(
        &self,
        invocation: &CommandInvocation,
    ) -> Vec<&'static ArgumentSpec> {
        let mut missing = Vec::new();
        for (index, argument) in self.args.iter().enumerate() {
            match argument.required {
                Requirement::Always => {
                    if invocation.argument(argument.name).is_none() {
                        missing.push(argument);
                    }
                }
                Requirement::Optional => {}
                Requirement::OneOf(group) => {
                    let first_in_group = self.args[..index]
                        .iter()
                        .all(|earlier| earlier.group() != Some(group));
                    let group_is_missing = self.args.iter().all(|member| {
                        member.group() != Some(group)
                            || invocation.argument(member.name).is_none()
                    });
                    if first_in_group && group_is_missing {
                        let prompted = self
                            .args
                            .iter()
                            .find(|member| {
                                member.group() == Some(group) && member.prompt
                            })
                            .or_else(|| {
                                self.args.iter().find(|member| {
                                    member.group() == Some(group)
                                })
                            });
                        if let Some(prompted) = prompted {
                            missing.push(prompted);
                        }
                    }
                }
            }
        }
        missing
    }
}

/// Value supplied for one argument.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum CommandValue {
    /// Boolean flag.
    Bool(bool),
    /// Integer value.
    Integer(i64),
    /// Free text.
    Text(String),
    /// Tab identity.
    Tab(TabId),
    /// Workspace identity.
    Workspace(WorkspaceId),
    /// Session identity.
    Session(SessionId),
    /// Pane identity.
    Pane(PaneId),
}

impl CommandValue {
    fn matches(&self, kind: ArgumentKind) -> bool {
        matches!(
            (self, kind),
            (Self::Bool(_), ArgumentKind::Bool)
                | (Self::Integer(_), ArgumentKind::Integer { .. })
                | (
                    Self::Text(_),
                    ArgumentKind::Text | ArgumentKind::QuakeProfile
                )
                | (Self::Pane(_), ArgumentKind::Pane)
                | (Self::Tab(_), ArgumentKind::Tab)
                | (Self::Workspace(_), ArgumentKind::Workspace)
                | (Self::Session(_), ArgumentKind::Session)
        ) || matches!(
            (self, kind),
            (Self::Text(side), ArgumentKind::Side) if SIDES.contains(&side.as_str())
        ) || matches!(
            (self, kind),
            (Self::Text(name), ArgumentKind::Layout) if NamedLayout::valid_name(name)
        )
    }
}

/// A named argument value in an invocation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CommandArgument {
    /// Declared argument name.
    pub name: String,
    /// Supplied value.
    pub value: CommandValue,
}

impl CommandArgument {
    /// Pairs an argument name with its value.
    #[must_use]
    pub fn new(name: impl Into<String>, value: CommandValue) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }
}

/// A request to run one command with named arguments.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CommandInvocation {
    /// Command to run.
    pub id: CommandId,
    /// Supplied arguments in any order.
    pub args: Vec<CommandArgument>,
}

impl CommandInvocation {
    /// Builds an invocation from a command and its arguments.
    #[must_use]
    pub fn new(id: CommandId, args: Vec<CommandArgument>) -> Self {
        Self { id, args }
    }

    /// Returns the first argument with this name.
    #[must_use]
    pub fn argument(&self, name: &str) -> Option<&CommandValue> {
        self.args
            .iter()
            .find(|argument| argument.name == name)
            .map(|argument| &argument.value)
    }

    /// Reads a boolean argument; absent or mistyped values read as `None`.
    #[must_use]
    pub fn bool(&self, name: &str) -> Option<bool> {
        match self.argument(name)? {
            CommandValue::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// Reads an integer argument; absent or mistyped values read as `None`.
    #[must_use]
    pub fn integer(&self, name: &str) -> Option<i64> {
        match self.argument(name)? {
            CommandValue::Integer(value) => Some(*value),
            _ => None,
        }
    }

    /// Reads a text argument; absent or mistyped values read as `None`.
    #[must_use]
    pub fn text(&self, name: &str) -> Option<&str> {
        match self.argument(name)? {
            CommandValue::Text(value) => Some(value),
            _ => None,
        }
    }

    /// Reads a tab argument; absent or mistyped values read as `None`.
    #[must_use]
    pub fn tab(&self, name: &str) -> Option<TabId> {
        match self.argument(name)? {
            CommandValue::Tab(value) => Some(*value),
            _ => None,
        }
    }

    /// Reads a pane argument; absent or mistyped values read as `None`.
    #[must_use]
    pub fn pane(&self, name: &str) -> Option<PaneId> {
        match self.argument(name)? {
            CommandValue::Pane(value) => Some(*value),
            _ => None,
        }
    }

    /// Reads a workspace argument; absent or mistyped values read as `None`.
    #[must_use]
    pub fn workspace(&self, name: &str) -> Option<WorkspaceId> {
        match self.argument(name)? {
            CommandValue::Workspace(value) => Some(*value),
            _ => None,
        }
    }

    /// Reads a session argument; absent or mistyped values read as `None`.
    #[must_use]
    pub fn session(&self, name: &str) -> Option<SessionId> {
        match self.argument(name)? {
            CommandValue::Session(value) => Some(*value),
            _ => None,
        }
    }
}

/// Result of a command that an executor accepted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandOutcome {
    /// The command's effect is complete.
    Completed,
    /// The command continues asynchronously; later failures are reported
    /// through the executing client.
    Accepted,
}

/// Why a command could not be validated or executed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandError {
    /// No catalog command has this identifier.
    UnknownCommand(CommandId),
    /// The command declares no argument with this name.
    UnknownArgument {
        /// Invoked command.
        command: CommandId,
        /// Supplied name.
        name: String,
    },
    /// The same argument name was supplied more than once.
    DuplicateArgument {
        /// Invoked command.
        command: CommandId,
        /// Repeated name.
        name: String,
    },
    /// A required argument was not supplied.
    MissingArgument {
        /// Invoked command.
        command: CommandId,
        /// Declared name.
        name: &'static str,
    },
    /// More than one argument of a one-of group was supplied.
    ConflictingArguments {
        /// Invoked command.
        command: CommandId,
        /// Declared one-of group.
        group: &'static str,
        /// Group member names in catalog order.
        names: Vec<&'static str>,
    },
    /// An argument value has the wrong type.
    ArgumentType {
        /// Invoked command.
        command: CommandId,
        /// Declared name.
        name: &'static str,
        /// Declared kind.
        expected: ArgumentKind,
    },
    /// An integer argument lies outside its declared range.
    ArgumentRange {
        /// Invoked command.
        command: CommandId,
        /// Declared name.
        name: &'static str,
        /// Supplied value.
        value: i64,
        /// Smallest accepted value.
        min: i64,
        /// Largest accepted value.
        max: i64,
    },
    /// The executor refused the command in its current state.
    Unavailable(String),
    /// A target ID no longer resolves, or belongs to another runtime.
    StaleTarget,
    /// The command needs a client window or view that is not available.
    ClientRequired,
    /// The executor is busy with a conflicting operation.
    Busy,
    /// The runtime failed while executing the command.
    Runtime(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCommand(id) => write!(f, "unknown command `{id}`"),
            Self::UnknownArgument { command, name } => {
                write!(f, "command `{command}` has no argument `{name}`")
            }
            Self::DuplicateArgument { command, name } => {
                write!(
                    f,
                    "command `{command}` received argument `{name}` more \
                     than once"
                )
            }
            Self::MissingArgument { command, name } => {
                write!(f, "command `{command}` requires argument `{name}`")
            }
            Self::ConflictingArguments { command, names, .. } => write!(
                f,
                "command `{command}` accepts only one of {}",
                names
                    .iter()
                    .map(|name| format!("`{name}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::ArgumentType {
                command,
                name,
                expected,
            } => write!(
                f,
                "command `{command}` argument `{name}` must be {}",
                describe_kind(*expected)
            ),
            Self::ArgumentRange {
                command,
                name,
                value,
                min,
                max,
            } => write!(
                f,
                "command `{command}` argument `{name}` is {value}; expected \
                 {min} to {max}"
            ),
            Self::Unavailable(reason) => {
                write!(f, "command unavailable: {reason}")
            }
            Self::StaleTarget => f.write_str("command target no longer exists"),
            Self::ClientRequired => {
                f.write_str("command requires a client window")
            }
            Self::Busy => f.write_str("command target is busy"),
            Self::Runtime(reason) => write!(f, "runtime error: {reason}"),
        }
    }
}

impl std::error::Error for CommandError {}

fn describe_kind(kind: ArgumentKind) -> &'static str {
    match kind {
        ArgumentKind::Bool => "a boolean",
        ArgumentKind::Integer { .. } => "an integer",
        ArgumentKind::Text => "text",
        ArgumentKind::QuakeProfile => "a quake profile name",
        ArgumentKind::Tab => "a tab",
        ArgumentKind::Workspace => "a workspace",
        ArgumentKind::Session => "a session",
        ArgumentKind::Pane => "a pane",
        ArgumentKind::Side => "left, right, up, or down",
        ArgumentKind::Layout => "a pane layout name",
    }
}

/// Checks supplied arguments against the catalog's static contract.
///
/// Names resolve against the declared arguments regardless of order; any
/// argument may be omitted. Executors report availability, stale targets, and
/// runtime failures separately.
///
/// # Errors
/// Reports an unknown command, unknown or duplicate argument names, conflicting
/// one-of arguments, mistyped values, and out-of-range integers.
pub fn validate_supplied(
    invocation: &CommandInvocation,
) -> Result<&'static CommandSpec, CommandError> {
    let command = invocation.id;
    let spec = lookup(command.as_str())
        .ok_or(CommandError::UnknownCommand(command))?;
    for (index, argument) in invocation.args.iter().enumerate() {
        let declared = spec.argument(&argument.name).ok_or_else(|| {
            CommandError::UnknownArgument {
                command,
                name: argument.name.clone(),
            }
        })?;
        if invocation.args[..index]
            .iter()
            .any(|earlier| earlier.name == argument.name)
        {
            return Err(CommandError::DuplicateArgument {
                command,
                name: argument.name.clone(),
            });
        }
        if !argument.value.matches(declared.kind) {
            return Err(CommandError::ArgumentType {
                command,
                name: declared.name,
                expected: declared.kind,
            });
        }
        if let (
            CommandValue::Integer(value),
            ArgumentKind::Integer { min, max },
        ) = (&argument.value, declared.kind)
            && !(min..=max).contains(value)
        {
            return Err(CommandError::ArgumentRange {
                command,
                name: declared.name,
                value: *value,
                min,
                max,
            });
        }
    }
    for (index, argument) in spec.args.iter().enumerate() {
        let Some(group) = argument.group() else {
            continue;
        };
        if spec.args[..index]
            .iter()
            .any(|earlier| earlier.group() == Some(group))
        {
            continue;
        }
        let supplied_count = spec
            .args
            .iter()
            .filter(|member| {
                member.group() == Some(group)
                    && invocation.argument(member.name).is_some()
            })
            .count();
        if supplied_count > 1 {
            let names = spec
                .args
                .iter()
                .filter(|member| member.group() == Some(group))
                .map(|member| member.name)
                .collect();
            return Err(CommandError::ConflictingArguments {
                command,
                group,
                names,
            });
        }
    }
    Ok(spec)
}

/// Checks an invocation against the catalog's complete static contract.
///
/// This runs [`validate_supplied`] and then requires every
/// [`Requirement::Always`] argument and exactly one member of every
/// [`Requirement::OneOf`] group.
///
/// # Errors
/// Reports every error from [`validate_supplied`] and missing required
/// arguments.
pub fn validate(
    invocation: &CommandInvocation,
) -> Result<&'static CommandSpec, CommandError> {
    let spec = validate_supplied(invocation)?;
    if let Some(missing) = spec.args.iter().find(|declared| {
        declared.is_required() && invocation.argument(declared.name).is_none()
    }) {
        return Err(CommandError::MissingArgument {
            command: invocation.id,
            name: missing.name,
        });
    }
    if let Some(missing) =
        spec.missing_prompted(invocation)
            .into_iter()
            .find(|argument| {
                argument.group().is_some()
                    && invocation.argument(argument.name).is_none()
            })
    {
        return Err(CommandError::MissingArgument {
            command: invocation.id,
            name: missing.name,
        });
    }
    Ok(spec)
}

/// Returns every catalog command in presentation order.
#[must_use]
pub fn catalog() -> &'static [CommandSpec] {
    CATALOG
}

/// Finds a catalog command by identifier text.
#[must_use]
pub fn lookup(id: &str) -> Option<&'static CommandSpec> {
    CATALOG.iter().find(|spec| spec.id.as_str() == id)
}

/// Identifiers of every catalog command.
pub mod ids {
    use super::CommandId;

    /// Opens a new desktop window.
    pub const NEW_WINDOW: CommandId = CommandId::new("new_window");
    /// Quits the application after assessed close.
    pub const QUIT: CommandId = CommandId::new("quit");
    /// Opens or focuses the platform update checker.
    pub const CHECK_FOR_UPDATES: CommandId =
        CommandId::new("check_for_updates");
    /// Show a named quake profile's window.
    pub const SHOW_QUAKE: CommandId = CommandId::new("show_quake");
    /// Hide a named quake profile's window without closing its terminals.
    pub const HIDE_QUAKE: CommandId = CommandId::new("hide_quake");
    /// Toggle a named quake profile's window.
    pub const TOGGLE_QUAKE: CommandId = CommandId::new("toggle_quake");
    /// Hide the application.
    pub const HIDE: CommandId = CommandId::new("hide");
    /// Hides other applications.
    pub const HIDE_OTHERS: CommandId = CommandId::new("hide_others");
    /// Shows all application windows.
    pub const SHOW_ALL: CommandId = CommandId::new("show_all");
    /// Reloads the configuration file.
    pub const RELOAD_CONFIG: CommandId = CommandId::new("reload_config");
    /// Opens the Settings window, or focuses it when it is open.
    pub const OPEN_SETTINGS: CommandId = CommandId::new("open_settings");
    /// Opens the configuration file in the system editor.
    pub const OPEN_CONFIG_FILE: CommandId = CommandId::new("open_config_file");
    /// Opens the command palette in the current window.
    pub const OPEN_COMMAND_PALETTE: CommandId =
        CommandId::new("open_command_palette");
    /// Shows application information.
    pub const ABOUT: CommandId = CommandId::new("about");
    /// Opens a new tab in the window.
    pub const NEW_TAB: CommandId = CommandId::new("new_tab");
    /// Closes a tab after assessment, defaulting to the active tab.
    pub const CLOSE_TAB: CommandId = CommandId::new("close_tab");
    /// Closes every other tab in the window after assessment.
    pub const CLOSE_OTHER_TABS: CommandId = CommandId::new("close_other_tabs");
    /// Closes the tabs after a tab after assessment.
    pub const CLOSE_TABS_AFTER: CommandId = CommandId::new("close_tabs_after");
    /// Copies a tab's working directory to the clipboard.
    pub const COPY_TAB_DIRECTORY: CommandId =
        CommandId::new("copy_tab_directory");
    /// Closes the window after assessment.
    pub const CLOSE_WINDOW: CommandId = CommandId::new("close_window");
    /// Activates the next tab.
    pub const NEXT_TAB: CommandId = CommandId::new("next_tab");
    /// Activates the previous tab.
    pub const PREVIOUS_TAB: CommandId = CommandId::new("previous_tab");
    /// Activates a tab by position.
    pub const SELECT_TAB: CommandId = CommandId::new("select_tab");
    /// Toggles window fullscreen.
    pub const TOGGLE_FULLSCREEN: CommandId =
        CommandId::new("toggle_fullscreen");
    /// Toggles platform-native fullscreen.
    pub const TOGGLE_NATIVE_FULLSCREEN: CommandId =
        CommandId::new("toggle_native_fullscreen");
    /// Toggles current-Space fullscreen on macOS.
    pub const TOGGLE_NON_NATIVE_FULLSCREEN: CommandId =
        CommandId::new("toggle_non_native_fullscreen");
    /// Minimizes the window.
    pub const MINIMIZE: CommandId = CommandId::new("minimize");
    /// Zooms the window.
    pub const ZOOM: CommandId = CommandId::new("zoom");
    /// Copies the terminal selection.
    pub const COPY: CommandId = CommandId::new("copy");
    /// Pastes the clipboard into the terminal.
    pub const PASTE: CommandId = CommandId::new("paste");
    /// Scrolls the terminal one page toward history.
    pub const SCROLL_PAGE_UP: CommandId = CommandId::new("scroll_page_up");
    /// Scrolls the terminal one page toward live output.
    pub const SCROLL_PAGE_DOWN: CommandId = CommandId::new("scroll_page_down");
    /// Scrolls the terminal to live output.
    pub const SCROLL_TO_BOTTOM: CommandId = CommandId::new("scroll_to_bottom");
    /// Renames a tab.
    pub const RENAME_TAB: CommandId = CommandId::new("rename_tab");
    /// Renames a workspace.
    pub const RENAME_WORKSPACE: CommandId = CommandId::new("rename_workspace");
    /// Renames a session.
    pub const RENAME_SESSION: CommandId = CommandId::new("rename_session");
    /// Activates the most recently used tab.
    pub const SELECT_RECENT_TAB: CommandId =
        CommandId::new("select_recent_tab");
    /// Splits the focused pane, putting a new pane to its right.
    pub const SPLIT_RIGHT: CommandId = CommandId::new("split_right");
    /// Splits the focused pane, putting a new pane below it.
    pub const SPLIT_DOWN: CommandId = CommandId::new("split_down");
    /// Splits the focused pane, putting a new pane to its left.
    pub const SPLIT_LEFT: CommandId = CommandId::new("split_left");
    /// Splits the focused pane, putting a new pane above it.
    pub const SPLIT_UP: CommandId = CommandId::new("split_up");
    /// Closes the focused pane after assessment; a lone pane closes its tab.
    pub const CLOSE_PANE: CommandId = CommandId::new("close_pane");
    /// Focuses the next pane of the active tab, wrapping.
    pub const FOCUS_NEXT_PANE: CommandId = CommandId::new("focus_next_pane");
    /// Focuses the previous pane of the active tab, wrapping.
    pub const FOCUS_PREVIOUS_PANE: CommandId =
        CommandId::new("focus_previous_pane");
    /// Focuses the pane to the left of the focused one.
    pub const FOCUS_PANE_LEFT: CommandId = CommandId::new("focus_pane_left");
    /// Focuses the pane to the right of the focused one.
    pub const FOCUS_PANE_RIGHT: CommandId = CommandId::new("focus_pane_right");
    /// Focuses the pane above the focused one.
    pub const FOCUS_PANE_UP: CommandId = CommandId::new("focus_pane_up");
    /// Focuses the pane below the focused one.
    pub const FOCUS_PANE_DOWN: CommandId = CommandId::new("focus_pane_down");
    /// Moves the focused pane's left edge to the left.
    pub const RESIZE_PANE_LEFT: CommandId = CommandId::new("resize_pane_left");
    /// Moves the focused pane's right edge to the right.
    pub const RESIZE_PANE_RIGHT: CommandId =
        CommandId::new("resize_pane_right");
    /// Moves the focused pane's top edge up.
    pub const RESIZE_PANE_UP: CommandId = CommandId::new("resize_pane_up");
    /// Moves the focused pane's bottom edge down.
    pub const RESIZE_PANE_DOWN: CommandId = CommandId::new("resize_pane_down");
    /// Zooms the focused pane across its tab, or unzooms it.
    pub const TOGGLE_PANE_ZOOM: CommandId = CommandId::new("toggle_pane_zoom");
    /// Swaps the focused pane with the next one in its tab, wrapping.
    pub const SWAP_PANE_NEXT: CommandId = CommandId::new("swap_pane_next");
    /// Swaps the focused pane with the previous one in its tab, wrapping.
    pub const SWAP_PANE_PREVIOUS: CommandId =
        CommandId::new("swap_pane_previous");
    /// Swaps the focused pane with the pane to its left.
    pub const SWAP_PANE_LEFT: CommandId = CommandId::new("swap_pane_left");
    /// Swaps the focused pane with the pane to its right.
    pub const SWAP_PANE_RIGHT: CommandId = CommandId::new("swap_pane_right");
    /// Swaps the focused pane with the pane above it.
    pub const SWAP_PANE_UP: CommandId = CommandId::new("swap_pane_up");
    /// Swaps the focused pane with the pane below it.
    pub const SWAP_PANE_DOWN: CommandId = CommandId::new("swap_pane_down");
    /// Moves the focused pane one place forward in its tab, wrapping.
    pub const MOVE_PANE_NEXT: CommandId = CommandId::new("move_pane_next");
    /// Moves the focused pane one place back in its tab, wrapping.
    pub const MOVE_PANE_PREVIOUS: CommandId =
        CommandId::new("move_pane_previous");
    /// Moves the focused pane past the pane to its left.
    pub const MOVE_PANE_LEFT: CommandId = CommandId::new("move_pane_left");
    /// Moves the focused pane past the pane to its right.
    pub const MOVE_PANE_RIGHT: CommandId = CommandId::new("move_pane_right");
    /// Moves the focused pane past the pane above it.
    pub const MOVE_PANE_UP: CommandId = CommandId::new("move_pane_up");
    /// Moves the focused pane past the pane below it.
    pub const MOVE_PANE_DOWN: CommandId = CommandId::new("move_pane_down");
    /// Moves the focused pane beside a chosen pane in any tab.
    pub const MOVE_PANE_TO_PANE: CommandId =
        CommandId::new("move_pane_to_pane");
    /// Moves the focused pane into a new tab.
    pub const MOVE_PANE_TO_NEW_TAB: CommandId =
        CommandId::new("move_pane_to_new_tab");
    /// Moves a whole tab beside a chosen pane of another tab.
    pub const MOVE_TAB_TO_PANE: CommandId = CommandId::new("move_tab_to_pane");
    /// Arranges the active tab's panes into a named layout.
    pub const ARRANGE_PANES: CommandId = CommandId::new("arrange_panes");
    /// Arranges the active tab's panes into the next layout of the cycle.
    pub const NEXT_PANE_LAYOUT: CommandId = CommandId::new("next_pane_layout");
    /// Arranges the active tab's panes into the previous layout of the
    /// cycle.
    pub const PREVIOUS_PANE_LAYOUT: CommandId =
        CommandId::new("previous_pane_layout");
    /// Gives every split of the active tab equal weights, keeping its shape.
    pub const BALANCE_PANES: CommandId = CommandId::new("balance_panes");
    /// Undoes the last layout change this window made.
    pub const UNDO: CommandId = CommandId::new("undo");
    /// Redoes the layout change this window undid last.
    pub const REDO: CommandId = CommandId::new("redo");
    /// Opens the window's application menu.
    pub const OPEN_MENU: CommandId = CommandId::new("open_menu");
    /// Focuses the window's notice list.
    pub const FOCUS_NOTICES: CommandId = CommandId::new("focus_notices");
    /// Dismisses every notice in the window.
    pub const DISMISS_ALL_NOTICES: CommandId =
        CommandId::new("dismiss_all_notices");
    /// Selects the terminal's whole scrollback and screen.
    pub const SELECT_ALL: CommandId = CommandId::new("select_all");
    /// Erases the terminal's scrollback, keeping the screen.
    pub const CLEAR_SCROLLBACK: CommandId = CommandId::new("clear_scrollback");
    /// Resets the terminal emulator's state.
    pub const RESET_TERMINAL: CommandId = CommandId::new("reset_terminal");
    /// Opens a tab's working directory in the file manager.
    pub const OPEN_TAB_DIRECTORY: CommandId =
        CommandId::new("open_tab_directory");
    /// Opens the active terminal's context menu at its cursor.
    pub const OPEN_CONTEXT_MENU: CommandId =
        CommandId::new("open_context_menu");
    /// Activates the focused dialog button.
    pub const DIALOG_CONFIRM: CommandId = CommandId::new("dialog_confirm");
    /// Cancels the open dialog.
    pub const DIALOG_CANCEL: CommandId = CommandId::new("dialog_cancel");
    /// Moves dialog focus to the next button.
    pub const DIALOG_FOCUS_NEXT: CommandId =
        CommandId::new("dialog_focus_next");
    /// Moves dialog focus to the previous button.
    pub const DIALOG_FOCUS_PREVIOUS: CommandId =
        CommandId::new("dialog_focus_previous");
    /// Moves the menu selection down.
    pub const MENU_SELECT_NEXT: CommandId = CommandId::new("menu_select_next");
    /// Moves the menu selection up.
    pub const MENU_SELECT_PREVIOUS: CommandId =
        CommandId::new("menu_select_previous");
    /// Moves the menu selection to the first item.
    pub const MENU_SELECT_FIRST: CommandId =
        CommandId::new("menu_select_first");
    /// Moves the menu selection to the last item.
    pub const MENU_SELECT_LAST: CommandId = CommandId::new("menu_select_last");
    /// Opens the next menu button.
    pub const MENU_SELECT_RIGHT: CommandId =
        CommandId::new("menu_select_right");
    /// Opens the previous menu button.
    pub const MENU_SELECT_LEFT: CommandId = CommandId::new("menu_select_left");
    /// Runs the selected menu item.
    pub const MENU_CONFIRM: CommandId = CommandId::new("menu_confirm");
    /// Closes the open menu.
    pub const MENU_CLOSE: CommandId = CommandId::new("menu_close");
    /// Moves the notice selection down.
    pub const NOTICE_NEXT: CommandId = CommandId::new("notice_next");
    /// Moves the notice selection up.
    pub const NOTICE_PREVIOUS: CommandId = CommandId::new("notice_previous");
    /// Runs the selected notice's action.
    pub const NOTICE_RUN_ACTION: CommandId =
        CommandId::new("notice_run_action");
    /// Dismisses the selected notice.
    pub const NOTICE_DISMISS: CommandId = CommandId::new("notice_dismiss");
    /// Focuses the Settings search field.
    pub const SETTINGS_SEARCH: CommandId = CommandId::new("settings_search");
    /// Moves Settings focus to the next control.
    pub const SETTINGS_FOCUS_NEXT: CommandId =
        CommandId::new("settings_focus_next");
    /// Moves Settings focus to the previous control.
    pub const SETTINGS_FOCUS_PREVIOUS: CommandId =
        CommandId::new("settings_focus_previous");
    /// Toggles the focused switch, or opens the focused select.
    pub const SETTINGS_TOGGLE: CommandId = CommandId::new("settings_toggle");
    /// Steps the focused control up or to its next option.
    pub const SETTINGS_INCREASE: CommandId =
        CommandId::new("settings_increase");
    /// Steps the focused control down or to its previous option.
    pub const SETTINGS_DECREASE: CommandId =
        CommandId::new("settings_decrease");
    /// Steps the focused slider up one page.
    pub const SETTINGS_PAGE_UP: CommandId = CommandId::new("settings_page_up");
    /// Steps the focused slider down one page.
    pub const SETTINGS_PAGE_DOWN: CommandId =
        CommandId::new("settings_page_down");
    /// Moves the focused slider to its minimum.
    pub const SETTINGS_FIRST: CommandId = CommandId::new("settings_first");
    /// Moves the focused slider to its maximum.
    pub const SETTINGS_LAST: CommandId = CommandId::new("settings_last");
    /// Resets the focused setting to its default.
    pub const SETTINGS_RESET: CommandId = CommandId::new("settings_reset");
    /// Moves the palette selection down.
    pub const PALETTE_SELECT_NEXT: CommandId =
        CommandId::new("palette_select_next");
    /// Moves the palette selection up.
    pub const PALETTE_SELECT_PREVIOUS: CommandId =
        CommandId::new("palette_select_previous");
    /// Moves the palette selection down one page.
    pub const PALETTE_PAGE_DOWN: CommandId =
        CommandId::new("palette_page_down");
    /// Moves the palette selection up one page.
    pub const PALETTE_PAGE_UP: CommandId = CommandId::new("palette_page_up");
    /// Runs the selected command or commits the active argument.
    pub const PALETTE_CONFIRM: CommandId = CommandId::new("palette_confirm");
    /// Returns to command search or closes the palette.
    pub const PALETTE_BACK: CommandId = CommandId::new("palette_back");
    /// Reopens the previous argument or returns to command search.
    pub const PALETTE_POP: CommandId = CommandId::new("palette_pop");
    /// Opens the selected command's arguments without running it.
    pub const PALETTE_EXPAND: CommandId = CommandId::new("palette_expand");
    /// Moves to the next command argument.
    pub const PALETTE_NEXT_SLOT: CommandId =
        CommandId::new("palette_next_slot");
    /// Moves to the previous command argument.
    pub const PALETTE_PREVIOUS_SLOT: CommandId =
        CommandId::new("palette_previous_slot");
    /// Deletes the character before the text cursor.
    pub const TEXT_DELETE_BACKWARD: CommandId =
        CommandId::new("text_delete_backward");
    /// Deletes the character after the text cursor.
    pub const TEXT_DELETE_FORWARD: CommandId =
        CommandId::new("text_delete_forward");
    /// Deletes the word before the text cursor.
    pub const TEXT_DELETE_WORD_BACKWARD: CommandId =
        CommandId::new("text_delete_word_backward");
    /// Deletes the word after the cursor.
    pub const TEXT_DELETE_WORD_FORWARD: CommandId =
        CommandId::new("text_delete_word_forward");
    /// Deletes from the line start to the text cursor.
    pub const TEXT_DELETE_LINE_START: CommandId =
        CommandId::new("text_delete_line_start");
    /// Moves the text cursor left.
    pub const TEXT_MOVE_LEFT: CommandId = CommandId::new("text_move_left");
    /// Moves the text cursor right.
    pub const TEXT_MOVE_RIGHT: CommandId = CommandId::new("text_move_right");
    /// Moves the text cursor left one word.
    pub const TEXT_MOVE_WORD_LEFT: CommandId =
        CommandId::new("text_move_word_left");
    /// Moves the text cursor right one word.
    pub const TEXT_MOVE_WORD_RIGHT: CommandId =
        CommandId::new("text_move_word_right");
    /// Moves the text cursor to the line start.
    pub const TEXT_LINE_START: CommandId = CommandId::new("text_line_start");
    /// Moves the text cursor to the line end.
    pub const TEXT_LINE_END: CommandId = CommandId::new("text_line_end");
    /// Selects all text.
    pub const TEXT_SELECT_ALL: CommandId = CommandId::new("text_select_all");
    /// Cuts the text selection.
    pub const TEXT_CUT: CommandId = CommandId::new("text_cut");
    /// Copies the text selection.
    pub const TEXT_COPY: CommandId = CommandId::new("text_copy");
    /// Pastes text from the clipboard.
    pub const TEXT_PASTE: CommandId = CommandId::new("text_paste");
}

const fn spec(
    id: CommandId,
    scope: CommandScope,
    title: &'static str,
    description: &'static str,
    args: &'static [ArgumentSpec],
) -> CommandSpec {
    CommandSpec {
        id,
        title,
        description,
        scope,
        context: None,
        args,
    }
}

const fn spec_in(
    context: &'static str,
    id: CommandId,
    scope: CommandScope,
    title: &'static str,
    description: &'static str,
    args: &'static [ArgumentSpec],
) -> CommandSpec {
    CommandSpec {
        id,
        title,
        description,
        scope,
        context: Some(context),
        args,
    }
}

const NAME: ArgumentSpec = ArgumentSpec {
    name: "name",
    kind: ArgumentKind::Text,
    required: Requirement::Always,
    prompt: true,
};

/// A pane resize moves its edge by `amount` cells.
const RESIZE: &[ArgumentSpec] = &[ArgumentSpec {
    name: "amount",
    kind: ArgumentKind::Integer { min: 1, max: 100 },
    required: Requirement::Optional,
    prompt: false,
}];

/// Movement commands extend the selection instead when `select` is true.
const SELECT: &[ArgumentSpec] = &[ArgumentSpec {
    name: "select",
    kind: ArgumentKind::Bool,
    required: Requirement::Optional,
    prompt: false,
}];

const PROFILE: &[ArgumentSpec] = &[ArgumentSpec {
    name: "profile",
    kind: ArgumentKind::QuakeProfile,
    required: Requirement::Optional,
    prompt: true,
}];

/// The pane a move lands beside, chosen in the palette.
const TARGET_PANE: ArgumentSpec = ArgumentSpec {
    name: "pane",
    kind: ArgumentKind::Pane,
    required: Requirement::Always,
    prompt: true,
};

/// The side of the target pane a move takes.
const SIDE: ArgumentSpec = ArgumentSpec {
    name: "side",
    kind: ArgumentKind::Side,
    required: Requirement::Always,
    prompt: true,
};

/// Tab-targeted commands default to the active tab. The argument is not
/// prompted so the palette runs them immediately; tab-strip menus and
/// programmatic callers supply another target.
const TAB: &[ArgumentSpec] = &[ArgumentSpec {
    name: "tab",
    kind: ArgumentKind::Tab,
    required: Requirement::Optional,
    prompt: false,
}];

const CATALOG: &[CommandSpec] = &[
    spec(
        ids::SHOW_QUAKE,
        CommandScope::Application,
        "Show Quake",
        "Show a named quake window, defaulting to default.",
        PROFILE,
    ),
    spec(
        ids::HIDE_QUAKE,
        CommandScope::Application,
        "Hide Quake",
        "Hide a named quake window and retain its jobs.",
        PROFILE,
    ),
    spec(
        ids::TOGGLE_QUAKE,
        CommandScope::Application,
        "Toggle Quake",
        "Show or hide a named quake window.",
        PROFILE,
    ),
    spec(
        ids::NEW_WINDOW,
        CommandScope::Application,
        "New Window",
        "Open a new window with its own session and workspace.",
        &[],
    ),
    spec(
        ids::QUIT,
        CommandScope::Application,
        "Quit",
        "Quit after assessing every open terminal.",
        &[],
    ),
    spec(
        ids::CHECK_FOR_UPDATES,
        CommandScope::Application,
        "Check for Updates...",
        "Open or focus the platform update checker.",
        &[],
    ),
    spec(
        ids::HIDE,
        CommandScope::Application,
        "Hide",
        "Hide the application.",
        &[],
    ),
    spec(
        ids::HIDE_OTHERS,
        CommandScope::Application,
        "Hide Others",
        "Hide every other application.",
        &[],
    ),
    spec(
        ids::SHOW_ALL,
        CommandScope::Application,
        "Show All",
        "Show every application window.",
        &[],
    ),
    spec(
        ids::RELOAD_CONFIG,
        CommandScope::Application,
        "Reload Configuration",
        "Reload the configuration file, keeping the last working settings on \
         error.",
        &[],
    ),
    spec(
        ids::OPEN_SETTINGS,
        CommandScope::Application,
        "Open Settings",
        "Open the Settings window.",
        &[],
    ),
    spec(
        ids::OPEN_CONFIG_FILE,
        CommandScope::Application,
        "Open Config File",
        "Open the configuration file in the system editor.",
        &[],
    ),
    spec(
        ids::OPEN_COMMAND_PALETTE,
        CommandScope::Window,
        "Open Command Palette",
        "Search and run commands in this window.",
        &[],
    ),
    spec(
        ids::ABOUT,
        CommandScope::Window,
        "About Huterm",
        "Show application version and license information.",
        &[],
    ),
    spec(
        ids::NEW_TAB,
        CommandScope::Window,
        "New Tab",
        "Open a new terminal tab in this window.",
        &[],
    ),
    spec(
        ids::CLOSE_TAB,
        CommandScope::Window,
        "Close Tab",
        "Close a tab after assessing its processes; defaults to the active tab.",
        TAB,
    ),
    spec(
        ids::CLOSE_OTHER_TABS,
        CommandScope::Window,
        "Close Other Tabs",
        "Close every tab except one after assessing their processes; defaults \
         to the active tab.",
        TAB,
    ),
    spec(
        ids::CLOSE_TABS_AFTER,
        CommandScope::Window,
        "Close Tabs After",
        "Close every tab after one after assessing their processes; defaults \
         to the active tab.",
        TAB,
    ),
    spec(
        ids::CLOSE_WINDOW,
        CommandScope::Window,
        "Close Window",
        "Close this window after assessing its terminals.",
        &[],
    ),
    spec(
        ids::NEXT_TAB,
        CommandScope::Window,
        "Next Tab",
        "Activate the tab after the active one.",
        &[],
    ),
    spec(
        ids::PREVIOUS_TAB,
        CommandScope::Window,
        "Previous Tab",
        "Activate the tab before the active one.",
        &[],
    ),
    spec(
        ids::SELECT_TAB,
        CommandScope::Window,
        "Select Tab",
        "Activate a tab by position from a keybinding, or choose one.",
        &[
            ArgumentSpec {
                name: "index",
                kind: ArgumentKind::Integer { min: 1, max: 9 },
                required: Requirement::OneOf("target"),
                prompt: false,
            },
            ArgumentSpec {
                name: "tab",
                kind: ArgumentKind::Tab,
                required: Requirement::OneOf("target"),
                prompt: true,
            },
        ],
    ),
    spec(
        ids::TOGGLE_FULLSCREEN,
        CommandScope::Window,
        "Toggle Fullscreen",
        "Enter or leave fullscreen for this window.",
        &[],
    ),
    spec(
        ids::TOGGLE_NATIVE_FULLSCREEN,
        CommandScope::Window,
        "Toggle Native Fullscreen",
        "Enter native fullscreen or leave the current fullscreen mode.",
        &[],
    ),
    spec(
        ids::TOGGLE_NON_NATIVE_FULLSCREEN,
        CommandScope::Window,
        "Toggle Non-Native Fullscreen",
        "Enter current-Space fullscreen on macOS or leave fullscreen.",
        &[],
    ),
    spec(
        ids::MINIMIZE,
        CommandScope::Window,
        "Minimize",
        "Minimize this window.",
        &[],
    ),
    spec(
        ids::ZOOM,
        CommandScope::Window,
        "Zoom",
        "Zoom this window.",
        &[],
    ),
    spec(
        ids::COPY,
        CommandScope::Terminal,
        "Copy",
        "Copy the terminal selection to the clipboard.",
        &[],
    ),
    spec(
        ids::PASTE,
        CommandScope::Terminal,
        "Paste",
        "Paste the clipboard into the terminal.",
        &[],
    ),
    spec(
        ids::COPY_TAB_DIRECTORY,
        CommandScope::Window,
        "Copy Tab Directory",
        "Copy a tab's working directory to the clipboard; defaults to the \
         active tab.",
        TAB,
    ),
    spec(
        ids::SCROLL_PAGE_UP,
        CommandScope::Terminal,
        "Scroll Page Up",
        "Scroll the terminal one page toward history.",
        &[],
    ),
    spec(
        ids::SCROLL_PAGE_DOWN,
        CommandScope::Terminal,
        "Scroll Page Down",
        "Scroll the terminal one page toward live output.",
        &[],
    ),
    spec(
        ids::SCROLL_TO_BOTTOM,
        CommandScope::Terminal,
        "Scroll to Bottom",
        "Scroll the terminal to live output.",
        &[],
    ),
    spec(
        ids::RENAME_TAB,
        CommandScope::Runtime,
        "Rename Tab",
        "Set a custom name for a tab; a blank name restores the default.",
        &[
            NAME,
            ArgumentSpec {
                name: "tab",
                kind: ArgumentKind::Tab,
                required: Requirement::Optional,
                prompt: true,
            },
        ],
    ),
    spec(
        ids::RENAME_WORKSPACE,
        CommandScope::Runtime,
        "Rename Workspace",
        "Set a custom name for a workspace; a blank name restores the default.",
        &[
            NAME,
            ArgumentSpec {
                name: "workspace",
                kind: ArgumentKind::Workspace,
                required: Requirement::Optional,
                prompt: true,
            },
        ],
    ),
    spec(
        ids::RENAME_SESSION,
        CommandScope::Runtime,
        "Rename Session",
        "Set a custom name for a session; a blank name restores the default.",
        &[
            NAME,
            ArgumentSpec {
                name: "session",
                kind: ArgumentKind::Session,
                required: Requirement::Optional,
                prompt: true,
            },
        ],
    ),
    spec(
        ids::SELECT_RECENT_TAB,
        CommandScope::Window,
        "Switch to Last Tab",
        "Activate the most recently used tab; repeat to toggle between two tabs.",
        &[],
    ),
    spec(
        ids::SPLIT_RIGHT,
        CommandScope::Window,
        "Split Right",
        "Split the focused pane, opening a new pane to its right.",
        &[],
    ),
    spec(
        ids::SPLIT_DOWN,
        CommandScope::Window,
        "Split Down",
        "Split the focused pane, opening a new pane below it.",
        &[],
    ),
    spec(
        ids::SPLIT_LEFT,
        CommandScope::Window,
        "Split Left",
        "Split the focused pane, opening a new pane to its left.",
        &[],
    ),
    spec(
        ids::SPLIT_UP,
        CommandScope::Window,
        "Split Up",
        "Split the focused pane, opening a new pane above it.",
        &[],
    ),
    spec(
        ids::CLOSE_PANE,
        CommandScope::Window,
        "Close Pane",
        "Close the focused pane after assessing its processes; closing a \
         tab's only pane closes the tab.",
        &[],
    ),
    spec(
        ids::FOCUS_NEXT_PANE,
        CommandScope::Window,
        "Focus Next Pane",
        "Focus the pane after the focused one in the active tab, wrapping.",
        &[],
    ),
    spec(
        ids::FOCUS_PREVIOUS_PANE,
        CommandScope::Window,
        "Focus Previous Pane",
        "Focus the pane before the focused one in the active tab, wrapping.",
        &[],
    ),
    spec(
        ids::FOCUS_PANE_LEFT,
        CommandScope::Window,
        "Focus Pane Left",
        "Focus the pane to the left of the focused one.",
        &[],
    ),
    spec(
        ids::FOCUS_PANE_RIGHT,
        CommandScope::Window,
        "Focus Pane Right",
        "Focus the pane to the right of the focused one.",
        &[],
    ),
    spec(
        ids::FOCUS_PANE_UP,
        CommandScope::Window,
        "Focus Pane Up",
        "Focus the pane above the focused one.",
        &[],
    ),
    spec(
        ids::FOCUS_PANE_DOWN,
        CommandScope::Window,
        "Focus Pane Down",
        "Focus the pane below the focused one.",
        &[],
    ),
    spec(
        ids::RESIZE_PANE_LEFT,
        CommandScope::Window,
        "Resize Pane Left",
        "Move the focused pane's left edge left by `amount` cells, 5 by \
         default.",
        RESIZE,
    ),
    spec(
        ids::RESIZE_PANE_RIGHT,
        CommandScope::Window,
        "Resize Pane Right",
        "Move the focused pane's right edge right by `amount` cells, 5 by \
         default.",
        RESIZE,
    ),
    spec(
        ids::RESIZE_PANE_UP,
        CommandScope::Window,
        "Resize Pane Up",
        "Move the focused pane's top edge up by `amount` cells, 5 by default.",
        RESIZE,
    ),
    spec(
        ids::RESIZE_PANE_DOWN,
        CommandScope::Window,
        "Resize Pane Down",
        "Move the focused pane's bottom edge down by `amount` cells, 5 by \
         default.",
        RESIZE,
    ),
    spec(
        ids::TOGGLE_PANE_ZOOM,
        CommandScope::Window,
        "Toggle Pane Zoom",
        "Show the focused pane across its whole tab, or show every pane again.",
        &[],
    ),
    spec(
        ids::SWAP_PANE_NEXT,
        CommandScope::Window,
        "Swap Pane with Next",
        "Swap the focused pane with the next pane in its tab, wrapping.",
        &[],
    ),
    spec(
        ids::SWAP_PANE_PREVIOUS,
        CommandScope::Window,
        "Swap Pane with Previous",
        "Swap the focused pane with the previous pane in its tab, wrapping.",
        &[],
    ),
    spec(
        ids::SWAP_PANE_LEFT,
        CommandScope::Window,
        "Swap Pane Left",
        "Swap the focused pane with the pane to its left.",
        &[],
    ),
    spec(
        ids::SWAP_PANE_RIGHT,
        CommandScope::Window,
        "Swap Pane Right",
        "Swap the focused pane with the pane to its right.",
        &[],
    ),
    spec(
        ids::SWAP_PANE_UP,
        CommandScope::Window,
        "Swap Pane Up",
        "Swap the focused pane with the pane above it.",
        &[],
    ),
    spec(
        ids::SWAP_PANE_DOWN,
        CommandScope::Window,
        "Swap Pane Down",
        "Swap the focused pane with the pane below it.",
        &[],
    ),
    spec(
        ids::MOVE_PANE_NEXT,
        CommandScope::Window,
        "Move Pane Forward",
        "Move the focused pane one place forward in its tab, wrapping.",
        &[],
    ),
    spec(
        ids::MOVE_PANE_PREVIOUS,
        CommandScope::Window,
        "Move Pane Back",
        "Move the focused pane one place back in its tab, wrapping.",
        &[],
    ),
    spec(
        ids::MOVE_PANE_LEFT,
        CommandScope::Window,
        "Move Pane Left",
        "Move the focused pane past the pane to its left.",
        &[],
    ),
    spec(
        ids::MOVE_PANE_RIGHT,
        CommandScope::Window,
        "Move Pane Right",
        "Move the focused pane past the pane to its right.",
        &[],
    ),
    spec(
        ids::MOVE_PANE_UP,
        CommandScope::Window,
        "Move Pane Up",
        "Move the focused pane past the pane above it.",
        &[],
    ),
    spec(
        ids::MOVE_PANE_DOWN,
        CommandScope::Window,
        "Move Pane Down",
        "Move the focused pane past the pane below it.",
        &[],
    ),
    spec(
        ids::MOVE_PANE_TO_PANE,
        CommandScope::Window,
        "Move Pane to Another Pane",
        "Move the focused pane beside a pane in any tab, keeping its \
         terminal running.",
        &[TARGET_PANE, SIDE],
    ),
    spec(
        ids::MOVE_PANE_TO_NEW_TAB,
        CommandScope::Window,
        "Move Pane to New Tab",
        "Move the focused pane into a new tab, in this workspace or another.",
        &[ArgumentSpec {
            name: "workspace",
            kind: ArgumentKind::Workspace,
            required: Requirement::Optional,
            prompt: true,
        }],
    ),
    spec(
        ids::MOVE_TAB_TO_PANE,
        CommandScope::Window,
        "Move Tab into Pane",
        "Move a whole tab beside a pane of another tab, keeping its layout.",
        &[
            ArgumentSpec {
                name: "tab",
                kind: ArgumentKind::Tab,
                required: Requirement::Optional,
                prompt: false,
            },
            TARGET_PANE,
            SIDE,
        ],
    ),
    spec(
        ids::ARRANGE_PANES,
        CommandScope::Window,
        "Arrange Panes",
        "Arrange the active tab's panes into a named layout, keeping every \
         terminal running.",
        &[ArgumentSpec {
            name: "layout",
            kind: ArgumentKind::Layout,
            required: Requirement::Always,
            prompt: true,
        }],
    ),
    spec(
        ids::NEXT_PANE_LAYOUT,
        CommandScope::Window,
        "Next Pane Layout",
        "Arrange the active tab's panes into the next layout of the cycle.",
        &[],
    ),
    spec(
        ids::PREVIOUS_PANE_LAYOUT,
        CommandScope::Window,
        "Previous Pane Layout",
        "Arrange the active tab's panes into the previous layout of the \
         cycle.",
        &[],
    ),
    spec(
        ids::BALANCE_PANES,
        CommandScope::Window,
        "Balance Panes",
        "Give every split of the active tab equal sizes, keeping its shape.",
        &[],
    ),
    spec(
        ids::UNDO,
        CommandScope::Window,
        "Undo",
        "Undo this window's last pane move, swap, tab drop, or layout change.",
        &[],
    ),
    spec(
        ids::REDO,
        CommandScope::Window,
        "Redo",
        "Redo the layout change this window undid last.",
        &[],
    ),
    spec(
        ids::OPEN_MENU,
        CommandScope::Window,
        "Open Menu",
        "Open this window's application menu.",
        &[],
    ),
    spec(
        ids::FOCUS_NOTICES,
        CommandScope::Window,
        "Focus Notices",
        "Move keyboard focus to this window's notices.",
        &[],
    ),
    spec(
        ids::DISMISS_ALL_NOTICES,
        CommandScope::Window,
        "Dismiss All Notices",
        "Dismiss every notice in this window.",
        &[],
    ),
    spec(
        ids::SELECT_ALL,
        CommandScope::Terminal,
        "Select All",
        "Select the terminal's scrollback and screen.",
        &[],
    ),
    spec(
        ids::CLEAR_SCROLLBACK,
        CommandScope::Terminal,
        "Clear Scrollback",
        "Erase the terminal's scrollback, keeping the screen.",
        &[],
    ),
    spec(
        ids::RESET_TERMINAL,
        CommandScope::Terminal,
        "Reset Terminal",
        "Reset the terminal's modes, screen, and scrollback, as after a \
         garbled program exits. Running programs are unaffected.",
        &[],
    ),
    spec(
        ids::OPEN_TAB_DIRECTORY,
        CommandScope::Window,
        "Open Tab Directory",
        "Open a tab's working directory in the file manager; defaults to \
         the active tab.",
        TAB,
    ),
    spec(
        ids::OPEN_CONTEXT_MENU,
        CommandScope::Window,
        "Open Context Menu",
        "Open the active terminal's context menu at its cursor.",
        &[],
    ),
    spec_in(
        "confirming",
        ids::DIALOG_CONFIRM,
        CommandScope::Window,
        "Dialog: Confirm",
        "Activate the focused dialog button.",
        &[],
    ),
    spec_in(
        "confirming",
        ids::DIALOG_CANCEL,
        CommandScope::Window,
        "Dialog: Cancel",
        "Close the dialog without confirming.",
        &[],
    ),
    spec_in(
        "confirming",
        ids::DIALOG_FOCUS_NEXT,
        CommandScope::Window,
        "Dialog: Next Button",
        "Move dialog focus to the next button.",
        &[],
    ),
    spec_in(
        "confirming",
        ids::DIALOG_FOCUS_PREVIOUS,
        CommandScope::Window,
        "Dialog: Previous Button",
        "Move dialog focus to the previous button.",
        &[],
    ),
    spec_in(
        "menu",
        ids::MENU_SELECT_NEXT,
        CommandScope::Window,
        "Menu: Next Item",
        "Move the menu selection down.",
        &[],
    ),
    spec_in(
        "menu",
        ids::MENU_SELECT_PREVIOUS,
        CommandScope::Window,
        "Menu: Previous Item",
        "Move the menu selection up.",
        &[],
    ),
    spec_in(
        "menu",
        ids::MENU_SELECT_FIRST,
        CommandScope::Window,
        "Menu: First Item",
        "Move the menu selection to the first item.",
        &[],
    ),
    spec_in(
        "menu",
        ids::MENU_SELECT_LAST,
        CommandScope::Window,
        "Menu: Last Item",
        "Move the menu selection to the last item.",
        &[],
    ),
    spec_in(
        "menu",
        ids::MENU_SELECT_RIGHT,
        CommandScope::Window,
        "Menu: Next Button",
        "Open the menu to the right of the open one.",
        &[],
    ),
    spec_in(
        "menu",
        ids::MENU_SELECT_LEFT,
        CommandScope::Window,
        "Menu: Previous Button",
        "Open the menu to the left of the open one.",
        &[],
    ),
    spec_in(
        "menu",
        ids::MENU_CONFIRM,
        CommandScope::Window,
        "Menu: Run Item",
        "Run the selected menu item.",
        &[],
    ),
    spec_in(
        "menu",
        ids::MENU_CLOSE,
        CommandScope::Window,
        "Menu: Close",
        "Close the open menu.",
        &[],
    ),
    spec_in(
        "notices",
        ids::NOTICE_NEXT,
        CommandScope::Window,
        "Notices: Next",
        "Move the notice selection down.",
        &[],
    ),
    spec_in(
        "notices",
        ids::NOTICE_PREVIOUS,
        CommandScope::Window,
        "Notices: Previous",
        "Move the notice selection up.",
        &[],
    ),
    spec_in(
        "notices",
        ids::NOTICE_RUN_ACTION,
        CommandScope::Window,
        "Notices: Run Action",
        "Run the selected notice's action.",
        &[],
    ),
    spec_in(
        "notices",
        ids::NOTICE_DISMISS,
        CommandScope::Window,
        "Notices: Dismiss",
        "Dismiss the selected notice.",
        &[],
    ),
    spec_in(
        "Settings",
        ids::SETTINGS_SEARCH,
        CommandScope::Window,
        "Settings: Search",
        "Focus the search field.",
        &[],
    ),
    spec_in(
        "Settings",
        ids::SETTINGS_FOCUS_NEXT,
        CommandScope::Window,
        "Settings: Next Control",
        "Move focus to the next control.",
        &[],
    ),
    spec_in(
        "Settings",
        ids::SETTINGS_FOCUS_PREVIOUS,
        CommandScope::Window,
        "Settings: Previous Control",
        "Move focus to the previous control.",
        &[],
    ),
    spec_in(
        "SettingsControl",
        ids::SETTINGS_TOGGLE,
        CommandScope::Window,
        "Settings: Toggle",
        "Toggle the focused switch, or open the focused select.",
        &[],
    ),
    spec_in(
        "SettingsControl",
        ids::SETTINGS_INCREASE,
        CommandScope::Window,
        "Settings: Increase",
        "Step the focused control up, or to its next option.",
        &[],
    ),
    spec_in(
        "SettingsControl",
        ids::SETTINGS_DECREASE,
        CommandScope::Window,
        "Settings: Decrease",
        "Step the focused control down, or to its previous option.",
        &[],
    ),
    spec_in(
        "SettingsControl",
        ids::SETTINGS_PAGE_UP,
        CommandScope::Window,
        "Settings: Page Up",
        "Step the focused slider up one page.",
        &[],
    ),
    spec_in(
        "SettingsControl",
        ids::SETTINGS_PAGE_DOWN,
        CommandScope::Window,
        "Settings: Page Down",
        "Step the focused slider down one page.",
        &[],
    ),
    spec_in(
        "SettingsControl",
        ids::SETTINGS_FIRST,
        CommandScope::Window,
        "Settings: Minimum",
        "Move the focused slider to its minimum.",
        &[],
    ),
    spec_in(
        "SettingsControl",
        ids::SETTINGS_LAST,
        CommandScope::Window,
        "Settings: Maximum",
        "Move the focused slider to its maximum.",
        &[],
    ),
    spec_in(
        "SettingsControl",
        ids::SETTINGS_RESET,
        CommandScope::Window,
        "Settings: Reset",
        "Reset the focused setting to its default.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::PALETTE_SELECT_NEXT,
        CommandScope::Palette,
        "Palette: Next Item",
        "Move the palette selection down.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::PALETTE_SELECT_PREVIOUS,
        CommandScope::Palette,
        "Palette: Previous Item",
        "Move the palette selection up.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::PALETTE_PAGE_DOWN,
        CommandScope::Palette,
        "Palette: Page Down",
        "Move the palette selection down one page.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::PALETTE_PAGE_UP,
        CommandScope::Palette,
        "Palette: Page Up",
        "Move the palette selection up one page.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::PALETTE_CONFIRM,
        CommandScope::Palette,
        "Palette: Confirm",
        "Run the selected command or commit the active argument.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::PALETTE_BACK,
        CommandScope::Palette,
        "Palette: Back",
        "Return to command search, or close the palette.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::PALETTE_POP,
        CommandScope::Palette,
        "Palette: Pop Argument",
        "Reopen the previous argument, or return to search.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::PALETTE_EXPAND,
        CommandScope::Palette,
        "Palette: Edit Arguments",
        "Open the selected command's arguments without running it.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::PALETTE_NEXT_SLOT,
        CommandScope::Palette,
        "Palette: Next Argument",
        "Move to the next argument.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::PALETTE_PREVIOUS_SLOT,
        CommandScope::Palette,
        "Palette: Previous Argument",
        "Move to the previous argument.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::TEXT_DELETE_BACKWARD,
        CommandScope::Palette,
        "Text: Delete Backward",
        "Delete the character before the cursor.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::TEXT_DELETE_FORWARD,
        CommandScope::Palette,
        "Text: Delete Forward",
        "Delete the character after the cursor.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::TEXT_DELETE_WORD_BACKWARD,
        CommandScope::Palette,
        "Text: Delete Word Backward",
        "Delete the word before the cursor.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::TEXT_DELETE_WORD_FORWARD,
        CommandScope::Palette,
        "Text: Delete Word Forward",
        "Delete the word after the cursor.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::TEXT_DELETE_LINE_START,
        CommandScope::Palette,
        "Text: Delete to Line Start",
        "Delete from the line start to the cursor.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::TEXT_MOVE_LEFT,
        CommandScope::Palette,
        "Text: Move Left",
        "Move the cursor left, or extend the selection with `select`.",
        SELECT,
    ),
    spec_in(
        "Palette",
        ids::TEXT_MOVE_RIGHT,
        CommandScope::Palette,
        "Text: Move Right",
        "Move the cursor right, or extend the selection with `select`.",
        SELECT,
    ),
    spec_in(
        "Palette",
        ids::TEXT_MOVE_WORD_LEFT,
        CommandScope::Palette,
        "Text: Move Word Left",
        "Move the cursor one word left, or extend the selection with `select`.",
        SELECT,
    ),
    spec_in(
        "Palette",
        ids::TEXT_MOVE_WORD_RIGHT,
        CommandScope::Palette,
        "Text: Move Word Right",
        "Move the cursor one word right, or extend the selection with `select`.",
        SELECT,
    ),
    spec_in(
        "Palette",
        ids::TEXT_LINE_START,
        CommandScope::Palette,
        "Text: Line Start",
        "Move the cursor to the line start, or extend the selection with `select`.",
        SELECT,
    ),
    spec_in(
        "Palette",
        ids::TEXT_LINE_END,
        CommandScope::Palette,
        "Text: Line End",
        "Move the cursor to the line end, or extend the selection with `select`.",
        SELECT,
    ),
    spec_in(
        "Palette",
        ids::TEXT_SELECT_ALL,
        CommandScope::Palette,
        "Text: Select All",
        "Select all text.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::TEXT_CUT,
        CommandScope::Palette,
        "Text: Cut",
        "Cut the selection to the clipboard.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::TEXT_COPY,
        CommandScope::Palette,
        "Text: Copy",
        "Copy the selection.",
        &[],
    ),
    spec_in(
        "Palette",
        ids::TEXT_PASTE,
        CommandScope::Palette,
        "Text: Paste",
        "Paste from the clipboard.",
        &[],
    ),
];

#[cfg(test)]
mod tests;
