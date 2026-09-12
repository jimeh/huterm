//! Command catalog, invocation values, and static validation.
//!
//! The catalog is the single definition of every user command: its stable ID,
//! execution scope, and named arguments in a fixed order. Clients bind, list,
//! and dispatch commands through it rather than restating names or arguments.

use std::fmt;

use crate::{SessionId, TabId, WorkspaceId};

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
}

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
                | (Self::Tab(_), ArgumentKind::Tab)
                | (Self::Workspace(_), ArgumentKind::Workspace)
                | (Self::Session(_), ArgumentKind::Session)
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
    /// Opens the configuration file for editing.
    pub const OPEN_SETTINGS: CommandId = CommandId::new("open_settings");
    /// Opens the command palette in the current window.
    pub const OPEN_COMMAND_PALETTE: CommandId =
        CommandId::new("open_command_palette");
    /// Shows application information.
    pub const ABOUT: CommandId = CommandId::new("about");
    /// Opens a new tab in the window.
    pub const NEW_TAB: CommandId = CommandId::new("new_tab");
    /// Closes the active tab after assessment.
    pub const CLOSE_TAB: CommandId = CommandId::new("close_tab");
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
        CommandScope::Window,
        "Open Settings",
        "Open the configuration file for editing.",
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
        "Close the active tab after assessing its processes.",
        &[],
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
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn text(name: &str, value: &str) -> CommandArgument {
        CommandArgument::new(name, CommandValue::Text(value.into()))
    }

    #[test]
    fn catalog_entries_are_unique_and_described() {
        let mut seen = HashSet::new();
        for spec in catalog() {
            assert!(seen.insert(spec.id), "duplicate id {}", spec.id);
            assert!(!spec.title.trim().is_empty(), "{} lacks a title", spec.id);
            assert!(
                !spec.description.trim().is_empty(),
                "{} lacks a description",
                spec.id
            );
            assert_eq!(lookup(spec.id.as_str()), Some(spec));
            assert!(
                matches!(spec.context, None | Some("Palette")),
                "{} has unknown context {:?}",
                spec.id,
                spec.context
            );
            for argument in spec.args {
                let Some(group) = argument.group() else {
                    continue;
                };
                let members = spec
                    .args
                    .iter()
                    .filter(|member| member.group() == Some(group))
                    .collect::<Vec<_>>();
                assert!(
                    members.len() >= 2,
                    "{} group {group} has fewer than two members",
                    spec.id
                );
                assert!(
                    members.iter().any(|member| member.prompt),
                    "{} group {group} has no prompted member",
                    spec.id
                );
            }
        }
        assert_eq!(lookup("unbind"), None);
    }

    #[test]
    fn unknown_command_is_rejected() {
        let bogus = CommandId::new("bogus");
        assert_eq!(
            validate(&CommandInvocation::new(bogus, Vec::new())),
            Err(CommandError::UnknownCommand(bogus))
        );
    }

    #[test]
    fn open_command_palette_is_argument_free_and_window_scoped() {
        let invocation =
            CommandInvocation::new(ids::OPEN_COMMAND_PALETTE, Vec::new());
        let spec = validate(&invocation).unwrap();
        assert_eq!(spec.scope, CommandScope::Window);
        assert!(spec.args.is_empty());
    }

    #[test]
    fn argument_names_must_be_declared_and_unique() {
        assert_eq!(
            validate(&CommandInvocation::new(
                ids::NEW_TAB,
                vec![CommandArgument::new("index", CommandValue::Integer(1))]
            )),
            Err(CommandError::UnknownArgument {
                command: ids::NEW_TAB,
                name: "index".into()
            })
        );
        assert_eq!(
            validate(&CommandInvocation::new(
                ids::RENAME_TAB,
                vec![text("name", "one"), text("name", "two")]
            )),
            Err(CommandError::DuplicateArgument {
                command: ids::RENAME_TAB,
                name: "name".into()
            })
        );
    }

    #[test]
    fn required_arguments_are_enforced_and_optional_ones_may_be_omitted() {
        let tab = TabId::new(7);
        assert_eq!(
            validate(&CommandInvocation::new(
                ids::RENAME_TAB,
                vec![CommandArgument::new("tab", CommandValue::Tab(tab))]
            )),
            Err(CommandError::MissingArgument {
                command: ids::RENAME_TAB,
                name: "name"
            })
        );
        let trailing_omitted =
            CommandInvocation::new(ids::RENAME_TAB, vec![text("name", "work")]);
        assert_eq!(
            validate(&trailing_omitted).map(|s| s.id),
            Ok(ids::RENAME_TAB)
        );
        assert_eq!(trailing_omitted.text("name"), Some("work"));
        assert_eq!(trailing_omitted.tab("tab"), None);
        let reordered = CommandInvocation::new(
            ids::RENAME_TAB,
            vec![
                CommandArgument::new("tab", CommandValue::Tab(tab)),
                text("name", "work"),
            ],
        );
        assert!(validate(&reordered).is_ok());
        assert_eq!(reordered.tab("tab"), Some(tab));
        assert_eq!(reordered.text("tab"), None);

        let by_index = CommandInvocation::new(
            ids::SELECT_TAB,
            vec![CommandArgument::new("index", CommandValue::Integer(3))],
        );
        assert!(validate(&by_index).is_ok());
        let by_tab = CommandInvocation::new(
            ids::SELECT_TAB,
            vec![CommandArgument::new("tab", CommandValue::Tab(tab))],
        );
        assert!(validate(&by_tab).is_ok());
    }

    #[test]
    fn argument_types_and_integer_ranges_are_checked() {
        assert_eq!(
            validate(&CommandInvocation::new(
                ids::SELECT_TAB,
                vec![text("index", "1")]
            )),
            Err(CommandError::ArgumentType {
                command: ids::SELECT_TAB,
                name: "index",
                expected: ArgumentKind::Integer { min: 1, max: 9 }
            })
        );
        for (value, ok) in [(0, false), (1, true), (9, true), (10, false)] {
            let result = validate(&CommandInvocation::new(
                ids::SELECT_TAB,
                vec![CommandArgument::new(
                    "index",
                    CommandValue::Integer(value),
                )],
            ));
            if ok {
                assert!(result.is_ok(), "index {value}");
            } else {
                assert_eq!(
                    result,
                    Err(CommandError::ArgumentRange {
                        command: ids::SELECT_TAB,
                        name: "index",
                        value,
                        min: 1,
                        max: 9
                    })
                );
            }
        }
    }

    #[test]
    fn validate_supplied_accepts_partial_invocations_and_rejects_bad_values() {
        let bare = CommandInvocation::new(ids::SELECT_TAB, Vec::new());
        assert_eq!(
            validate_supplied(&bare).map(|spec| spec.id),
            Ok(ids::SELECT_TAB)
        );

        assert_eq!(
            validate_supplied(&CommandInvocation::new(
                ids::SELECT_TAB,
                vec![CommandArgument::new("index", CommandValue::Integer(10))]
            )),
            Err(CommandError::ArgumentRange {
                command: ids::SELECT_TAB,
                name: "index",
                value: 10,
                min: 1,
                max: 9,
            })
        );
        assert_eq!(
            validate_supplied(&CommandInvocation::new(
                ids::SELECT_TAB,
                vec![text("index", "1")]
            )),
            Err(CommandError::ArgumentType {
                command: ids::SELECT_TAB,
                name: "index",
                expected: ArgumentKind::Integer { min: 1, max: 9 },
            })
        );

        let conflicting = CommandInvocation::new(
            ids::SELECT_TAB,
            vec![
                CommandArgument::new("tab", CommandValue::Tab(TabId::new(2))),
                CommandArgument::new("index", CommandValue::Integer(1)),
            ],
        );
        let conflict = CommandError::ConflictingArguments {
            command: ids::SELECT_TAB,
            group: "target",
            names: vec!["index", "tab"],
        };
        assert_eq!(validate_supplied(&conflicting), Err(conflict.clone()));
        assert_eq!(validate(&conflicting), Err(conflict));

        assert_eq!(
            validate_supplied(&CommandInvocation::new(
                ids::SELECT_TAB,
                vec![CommandArgument::new(
                    "position",
                    CommandValue::Integer(1)
                )]
            )),
            Err(CommandError::UnknownArgument {
                command: ids::SELECT_TAB,
                name: "position".into(),
            })
        );
    }

    #[test]
    fn validate_requires_exactly_one_group_member() {
        assert_eq!(
            validate(&CommandInvocation::new(ids::SELECT_TAB, Vec::new())),
            Err(CommandError::MissingArgument {
                command: ids::SELECT_TAB,
                name: "tab",
            })
        );
        assert!(
            validate(&CommandInvocation::new(
                ids::SELECT_TAB,
                vec![CommandArgument::new("index", CommandValue::Integer(3))]
            ))
            .is_ok()
        );
        assert!(
            validate(&CommandInvocation::new(
                ids::SELECT_TAB,
                vec![CommandArgument::new(
                    "tab",
                    CommandValue::Tab(TabId::new(3))
                )]
            ))
            .is_ok()
        );
    }

    #[test]
    fn missing_prompted_lists_only_what_a_client_must_collect() {
        let select_tab = lookup(ids::SELECT_TAB.as_str()).unwrap();
        let bare_select_tab =
            CommandInvocation::new(ids::SELECT_TAB, Vec::new());
        assert_eq!(
            select_tab
                .missing_prompted(&bare_select_tab)
                .iter()
                .map(|argument| argument.name)
                .collect::<Vec<_>>(),
            vec!["tab"]
        );
        let select_by_index = CommandInvocation::new(
            ids::SELECT_TAB,
            vec![CommandArgument::new("index", CommandValue::Integer(2))],
        );
        assert!(select_tab.missing_prompted(&select_by_index).is_empty());

        let rename_tab = lookup(ids::RENAME_TAB.as_str()).unwrap();
        assert_eq!(
            rename_tab
                .missing_prompted(&CommandInvocation::new(
                    ids::RENAME_TAB,
                    Vec::new()
                ))
                .iter()
                .map(|argument| argument.name)
                .collect::<Vec<_>>(),
            vec!["name"]
        );

        let toggle_quake = lookup(ids::TOGGLE_QUAKE.as_str()).unwrap();
        assert!(
            toggle_quake
                .missing_prompted(&CommandInvocation::new(
                    ids::TOGGLE_QUAKE,
                    Vec::new()
                ))
                .is_empty()
        );
    }

    #[test]
    fn quake_profile_arguments_carry_text() {
        assert!(
            validate(&CommandInvocation::new(
                ids::TOGGLE_QUAKE,
                vec![text("profile", "logs")]
            ))
            .is_ok()
        );
        assert_eq!(
            validate(&CommandInvocation::new(
                ids::TOGGLE_QUAKE,
                vec![CommandArgument::new("profile", CommandValue::Integer(1))]
            )),
            Err(CommandError::ArgumentType {
                command: ids::TOGGLE_QUAKE,
                name: "profile",
                expected: ArgumentKind::QuakeProfile,
            })
        );
    }

    #[test]
    fn palette_commands_are_palette_scoped_and_context_bound() {
        let expected = [
            ids::PALETTE_SELECT_NEXT,
            ids::PALETTE_SELECT_PREVIOUS,
            ids::PALETTE_PAGE_DOWN,
            ids::PALETTE_PAGE_UP,
            ids::PALETTE_CONFIRM,
            ids::PALETTE_BACK,
            ids::PALETTE_POP,
            ids::PALETTE_EXPAND,
            ids::PALETTE_NEXT_SLOT,
            ids::PALETTE_PREVIOUS_SLOT,
            ids::TEXT_DELETE_BACKWARD,
            ids::TEXT_DELETE_FORWARD,
            ids::TEXT_DELETE_WORD_BACKWARD,
            ids::TEXT_DELETE_WORD_FORWARD,
            ids::TEXT_DELETE_LINE_START,
            ids::TEXT_MOVE_LEFT,
            ids::TEXT_MOVE_RIGHT,
            ids::TEXT_MOVE_WORD_LEFT,
            ids::TEXT_MOVE_WORD_RIGHT,
            ids::TEXT_LINE_START,
            ids::TEXT_LINE_END,
            ids::TEXT_SELECT_ALL,
            ids::TEXT_COPY,
            ids::TEXT_PASTE,
        ];
        for id in expected {
            let spec = lookup(id.as_str()).unwrap();
            assert_eq!(spec.scope, CommandScope::Palette, "{id}");
            assert_eq!(spec.context, Some("Palette"), "{id}");
            assert!(only_select_argument(spec), "{id}");
        }
        for spec in catalog().iter().filter(|spec| {
            spec.id.as_str().starts_with("palette_")
                || spec.id.as_str().starts_with("text_")
        }) {
            assert_eq!(spec.scope, CommandScope::Palette, "{}", spec.id);
            assert_eq!(spec.context, Some("Palette"), "{}", spec.id);
            assert!(only_select_argument(spec), "{}", spec.id);
        }
    }

    /// Palette commands take no prompted arguments; movement commands may
    /// take the unprompted `select` flag.
    fn only_select_argument(spec: &CommandSpec) -> bool {
        spec.args.iter().all(|argument| {
            argument.name == "select"
                && argument.kind == ArgumentKind::Bool
                && argument.required == Requirement::Optional
                && !argument.prompt
        })
    }

    #[test]
    fn non_palette_commands_with_arguments_offer_a_prompted_argument() {
        for spec in catalog().iter().filter(|spec| {
            spec.scope != CommandScope::Palette && !spec.args.is_empty()
        }) {
            assert!(
                spec.args.iter().any(|argument| argument.prompt),
                "command `{}` has arguments but no prompted argument",
                spec.id
            );
        }
    }

    #[test]
    fn recent_tab_command_validates() {
        let recent = CommandInvocation::new(ids::SELECT_RECENT_TAB, Vec::new());
        let spec = validate(&recent).unwrap();
        assert_eq!(spec.scope, CommandScope::Window);
        assert!(spec.args.is_empty());
        assert_eq!(
            validate(&CommandInvocation::new(
                ids::SELECT_RECENT_TAB,
                vec![CommandArgument::new(
                    "tab",
                    CommandValue::Tab(TabId::new(1))
                )]
            )),
            Err(CommandError::UnknownArgument {
                command: ids::SELECT_RECENT_TAB,
                name: "tab".into(),
            })
        );
    }

    #[test]
    fn errors_display_their_context() {
        let error = CommandError::ArgumentRange {
            command: ids::SELECT_TAB,
            name: "index",
            value: 12,
            min: 1,
            max: 9,
        };
        assert_eq!(
            error.to_string(),
            "command `select_tab` argument `index` is 12; expected 1 to 9"
        );
        let conflict = CommandError::ConflictingArguments {
            command: ids::SELECT_TAB,
            group: "target",
            names: vec!["index", "tab"],
        };
        assert_eq!(
            conflict.to_string(),
            "command `select_tab` accepts only one of `index`, `tab`"
        );
        let source: &dyn std::error::Error = &error;
        assert!(source.source().is_none());
    }
}
