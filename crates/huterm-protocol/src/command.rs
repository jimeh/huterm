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
    /// Tab identity, supplied from client context rather than configuration.
    Tab,
    /// Workspace identity, supplied from client context.
    Workspace,
    /// Session identity, supplied from client context.
    Session,
}

/// One declared argument of a command.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ArgumentSpec {
    /// Argument name used by callers and configuration.
    pub name: &'static str,
    /// Accepted value type.
    pub kind: ArgumentKind,
    /// Whether an invocation must supply the argument.
    pub required: bool,
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
    /// Declared arguments in positional order.
    pub args: &'static [ArgumentSpec],
}

impl CommandSpec {
    /// Finds a declared argument by name.
    #[must_use]
    pub fn argument(&self, name: &str) -> Option<&'static ArgumentSpec> {
        self.args.iter().find(|spec| spec.name == name)
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
                | (Self::Text(_), ArgumentKind::Text)
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
        ArgumentKind::Tab => "a tab",
        ArgumentKind::Workspace => "a workspace",
        ArgumentKind::Session => "a session",
    }
}

/// Checks an invocation against the catalog's static contract.
///
/// Names resolve against the declared arguments regardless of order; any
/// optional argument may be omitted. Executors report availability, stale
/// targets, and runtime failures separately.
///
/// # Errors
/// Reports an unknown command, unknown or duplicate argument names, missing
/// required arguments, mistyped values, and out-of-range integers.
pub fn validate(
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
    if let Some(missing) = spec.args.iter().find(|declared| {
        declared.required && invocation.argument(declared.name).is_none()
    }) {
        return Err(CommandError::MissingArgument {
            command,
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
    /// Hides the application.
    pub const HIDE: CommandId = CommandId::new("hide");
    /// Hides other applications.
    pub const HIDE_OTHERS: CommandId = CommandId::new("hide_others");
    /// Shows all application windows.
    pub const SHOW_ALL: CommandId = CommandId::new("show_all");
    /// Reloads the configuration file.
    pub const RELOAD_CONFIG: CommandId = CommandId::new("reload_config");
    /// Opens the configuration file for editing.
    pub const OPEN_SETTINGS: CommandId = CommandId::new("open_settings");
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
        args,
    }
}

const NAME: ArgumentSpec = ArgumentSpec {
    name: "name",
    kind: ArgumentKind::Text,
    required: true,
};

const CATALOG: &[CommandSpec] = &[
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
        "Activate a tab by position; 9 selects the last tab.",
        &[ArgumentSpec {
            name: "index",
            kind: ArgumentKind::Integer { min: 1, max: 9 },
            required: true,
        }],
    ),
    spec(
        ids::TOGGLE_FULLSCREEN,
        CommandScope::Window,
        "Toggle Fullscreen",
        "Enter or leave fullscreen for this window.",
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
        "Set a custom name for a tab.",
        &[
            NAME,
            ArgumentSpec {
                name: "tab",
                kind: ArgumentKind::Tab,
                required: false,
            },
        ],
    ),
    spec(
        ids::RENAME_WORKSPACE,
        CommandScope::Runtime,
        "Rename Workspace",
        "Set a custom name for a workspace.",
        &[
            NAME,
            ArgumentSpec {
                name: "workspace",
                kind: ArgumentKind::Workspace,
                required: false,
            },
        ],
    ),
    spec(
        ids::RENAME_SESSION,
        CommandScope::Runtime,
        "Rename Session",
        "Set a custom name for a session.",
        &[
            NAME,
            ArgumentSpec {
                name: "session",
                kind: ArgumentKind::Session,
                required: false,
            },
        ],
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
        let source: &dyn std::error::Error = &error;
        assert!(source.source().is_none());
    }
}
