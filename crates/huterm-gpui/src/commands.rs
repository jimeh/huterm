//! GPUI dispatch for catalog commands.
//!
//! Each catalog scope maps to one GPUI action type so type-routed dispatch
//! delivers a command to the handler that owns it: the application for
//! [`InvokeApp`], the window's `WorkspaceView` for [`InvokeWindow`], and the
//! active `TerminalView` for [`InvokeTerminal`]. Palette-scope commands travel
//! as [`InvokePalette`] to the window's open palette. Runtime-scope commands
//! travel as [`InvokeWindow`] because the window fills omitted targets from
//! its own context before forwarding to the structural worker.

use gpui::{Action, App, MenuItem};
use huterm_protocol::{
    CommandArgument, CommandError, CommandId, CommandInvocation, CommandScope,
    CommandValue, TabId, WorkspaceId, ids, lookup, validate_supplied,
};

/// Application-scope command carried through GPUI's global action handlers.
#[derive(Action, Clone, Debug, PartialEq)]
#[action(namespace = huterm, no_json)]
pub(crate) struct InvokeApp(pub(crate) CommandInvocation);

/// Window- or runtime-scope command handled by the window's root view.
#[derive(Action, Clone, Debug, PartialEq)]
#[action(namespace = huterm, no_json)]
pub(crate) struct InvokeWindow(pub(crate) CommandInvocation);

/// Terminal-scope command handled by the focused terminal view.
#[derive(Action, Clone, Debug, PartialEq)]
#[action(namespace = huterm, no_json)]
pub(crate) struct InvokeTerminal(pub(crate) CommandInvocation);

/// Palette-scope command handled by the window's open command palette.
#[derive(Action, Clone, Debug, PartialEq)]
#[action(namespace = huterm, no_json)]
pub(crate) struct InvokePalette(pub(crate) CommandInvocation);

/// A validated invocation wrapped in the action type for its scope.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CommandAction {
    App(InvokeApp),
    Window(InvokeWindow),
    Terminal(InvokeTerminal),
    Palette(InvokePalette),
}

impl CommandAction {
    /// Validates supplied arguments and wraps `invocation` for its catalog
    /// scope.
    ///
    /// Bindings and menus may carry partial invocations that the window
    /// completes or prompts for. Executors run full validation immediately
    /// before side effects.
    ///
    /// # Errors
    /// Reports catalog validation failures unchanged.
    pub(crate) fn new(
        invocation: CommandInvocation,
    ) -> Result<Self, CommandError> {
        let spec = validate_supplied(&invocation)?;
        Ok(match spec.scope {
            CommandScope::Application => Self::App(InvokeApp(invocation)),
            CommandScope::Window | CommandScope::Runtime => {
                Self::Window(InvokeWindow(invocation))
            }
            CommandScope::Terminal => {
                Self::Terminal(InvokeTerminal(invocation))
            }
            CommandScope::Palette => Self::Palette(InvokePalette(invocation)),
        })
    }

    pub(crate) fn invocation(&self) -> &CommandInvocation {
        match self {
            Self::App(action) => &action.0,
            Self::Window(action) => &action.0,
            Self::Terminal(action) => &action.0,
            Self::Palette(action) => &action.0,
        }
    }

    pub(crate) fn into_boxed(self) -> Box<dyn Action> {
        match self {
            Self::App(action) => Box::new(action),
            Self::Window(action) => Box::new(action),
            Self::Terminal(action) => Box::new(action),
            Self::Palette(action) => Box::new(action),
        }
    }

    /// Builds a menu item labelled with the command's catalog title.
    pub(crate) fn menu_item(&self) -> MenuItem {
        let title = lookup(self.invocation().id.as_str()).map_or_else(
            String::new,
            |spec| {
                let mut title = spec.title.to_owned();
                if !spec.missing_prompted(self.invocation()).is_empty() {
                    title.push('…');
                }
                title
            },
        );
        match self.clone() {
            Self::App(action) => MenuItem::action(title, action),
            Self::Window(action) => MenuItem::action(title, action),
            Self::Terminal(action) => MenuItem::action(title, action),
            Self::Palette(action) => MenuItem::action(title, action),
        }
    }

    /// Dispatches to the active window, or globally when none is active.
    pub(crate) fn dispatch(&self, cx: &mut App) {
        match self {
            Self::App(action) => cx.dispatch_action(action),
            Self::Window(action) => cx.dispatch_action(action),
            Self::Terminal(action) => cx.dispatch_action(action),
            Self::Palette(action) => cx.dispatch_action(action),
        }
    }
}

/// Wraps an argument-free catalog command.
///
/// # Panics
/// Panics when `id` is not a catalog command; call sites pass catalog
/// constants, so this is a programming error. Commands with missing arguments
/// remain valid partial invocations for an interactive caller to complete.
pub(crate) fn invoke(id: CommandId) -> CommandAction {
    invoke_with(id, [])
}

/// Wraps a catalog command with named arguments.
///
/// # Panics
/// Panics when the supplied invocation arguments fail catalog validation; call
/// sites pass catalog constants and literal arguments, so this is a
/// programming error.
pub(crate) fn invoke_with(
    id: CommandId,
    args: impl IntoIterator<Item = (&'static str, CommandValue)>,
) -> CommandAction {
    let args = args
        .into_iter()
        .map(|(name, value)| CommandArgument::new(name, value))
        .collect();
    CommandAction::new(CommandInvocation::new(id, args))
        .unwrap_or_else(|error| panic!("invalid catalog invocation: {error}"))
}

/// Where a validated command executes for a programmatic caller.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Route<W> {
    Application,
    Window(W),
    Terminal(W),
}

/// Chooses the executing view for `scope`, requiring a window for every
/// scope except application.
///
/// # Errors
/// Returns [`CommandError::ClientRequired`] when the scope needs a window and
/// none was supplied.
pub(crate) fn route<W>(
    scope: CommandScope,
    window: Option<W>,
) -> Result<Route<W>, CommandError> {
    match scope {
        CommandScope::Application => Ok(Route::Application),
        CommandScope::Window
        | CommandScope::Runtime
        | CommandScope::Palette => window
            .map(Route::Window)
            .ok_or(CommandError::ClientRequired),
        CommandScope::Terminal => window
            .map(Route::Terminal)
            .ok_or(CommandError::ClientRequired),
    }
}

/// Supplies a window-resolved target when the invocation omits it.
///
/// # Errors
/// Returns [`CommandError::Unavailable`] when neither the invocation nor the
/// window provides the target.
pub(crate) fn fill_target(
    invocation: &CommandInvocation,
    name: &'static str,
    resolved: Option<CommandValue>,
) -> Result<CommandInvocation, CommandError> {
    if invocation.argument(name).is_some() {
        return Ok(invocation.clone());
    }
    let Some(value) = resolved else {
        return Err(CommandError::Unavailable(format!(
            "window has no {name} to target"
        )));
    };
    let mut filled = invocation.clone();
    filled.args.push(CommandArgument::new(name, value));
    Ok(filled)
}

/// Fills the target a rename command omits from the invoking window.
///
/// An explicit target always wins. `rename_session` without a session carries
/// the window's workspace instead, because the session owning a workspace is
/// canonical runtime state and is resolved under the Mux lock on the worker.
///
/// # Errors
/// Returns [`CommandError::Unavailable`] when the window cannot supply the
/// omitted target, and [`CommandError::UnknownCommand`] for other commands.
pub(crate) fn fill_rename_target(
    invocation: &CommandInvocation,
    active_tab: Option<TabId>,
    workspace: Option<WorkspaceId>,
) -> Result<CommandInvocation, CommandError> {
    match invocation.id {
        ids::RENAME_TAB => {
            fill_target(invocation, "tab", active_tab.map(CommandValue::Tab))
        }
        ids::RENAME_SESSION if invocation.argument("session").is_some() => {
            Ok(invocation.clone())
        }
        ids::RENAME_WORKSPACE | ids::RENAME_SESSION => fill_target(
            invocation,
            "workspace",
            workspace.map(CommandValue::Workspace),
        ),
        other => Err(CommandError::UnknownCommand(other)),
    }
}

/// Converts a validated one-based `select_tab` index to a tab slot.
///
/// # Errors
/// Returns [`CommandError::MissingArgument`] when the invocation omits
/// `index`; catalog validation rejects out-of-range values earlier.
pub(crate) fn select_tab_slot(
    invocation: &CommandInvocation,
) -> Result<usize, CommandError> {
    let index =
        invocation
            .integer("index")
            .ok_or(CommandError::MissingArgument {
                command: invocation.id,
                name: "index",
            })?;
    usize::try_from(index.saturating_sub(1)).map_err(|_| {
        CommandError::ArgumentRange {
            command: invocation.id,
            name: "index",
            value: index,
            min: 1,
            max: 9,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use huterm_protocol::{SessionId, validate};

    fn invocation(
        id: CommandId,
        args: &[(&'static str, CommandValue)],
    ) -> CommandInvocation {
        CommandInvocation::new(
            id,
            args.iter()
                .map(|(name, value)| CommandArgument::new(*name, value.clone()))
                .collect(),
        )
    }

    #[test]
    fn wrapper_follows_catalog_scope_and_rejects_unknown_commands() {
        assert!(matches!(invoke(ids::QUIT), CommandAction::App(_)));
        assert!(matches!(
            invoke(ids::CHECK_FOR_UPDATES),
            CommandAction::App(_)
        ));
        assert!(matches!(invoke(ids::NEW_TAB), CommandAction::Window(_)));
        assert!(matches!(invoke(ids::ABOUT), CommandAction::Window(_)));
        assert!(matches!(invoke(ids::COPY), CommandAction::Terminal(_)));
        assert!(matches!(
            invoke(ids::PALETTE_CONFIRM),
            CommandAction::Palette(_)
        ));
        assert!(matches!(
            invoke_with(
                ids::RENAME_TAB,
                [("name", CommandValue::Text("work".into()))]
            ),
            CommandAction::Window(_)
        ));
        assert_eq!(
            CommandAction::new(invocation(CommandId::new("missing"), &[])),
            Err(CommandError::UnknownCommand(CommandId::new("missing")))
        );
    }

    #[test]
    fn command_action_accepts_partial_invocations() {
        assert!(matches!(
            CommandAction::new(invocation(ids::SELECT_TAB, &[])),
            Ok(CommandAction::Window(_))
        ));
        assert!(matches!(
            CommandAction::new(invocation(ids::PALETTE_CONFIRM, &[])),
            Ok(CommandAction::Palette(_))
        ));
        assert!(matches!(
            CommandAction::new(invocation(
                ids::SELECT_TAB,
                &[
                    ("index", CommandValue::Integer(1)),
                    ("tab", CommandValue::Tab(TabId::new(1))),
                ],
            )),
            Err(CommandError::ConflictingArguments { .. })
        ));
    }

    #[test]
    fn menu_items_mark_commands_that_need_interactive_arguments() {
        let menu_name = |command| match invoke(command).menu_item() {
            MenuItem::Action { name, .. } => name.to_string(),
            _ => panic!("catalog command did not produce an action item"),
        };

        assert_eq!(menu_name(ids::SELECT_TAB), "Select Tab…");
        assert_eq!(menu_name(ids::RENAME_TAB), "Rename Tab…");
        assert_eq!(menu_name(ids::NEW_TAB), "New Tab");
    }

    #[test]
    fn select_tab_index_is_validated_before_dispatch_and_maps_to_slots() {
        for (index, slot) in [(1, 0), (5, 4), (9, 8)] {
            let action = invoke_with(
                ids::SELECT_TAB,
                [("index", CommandValue::Integer(index))],
            );
            assert_eq!(select_tab_slot(action.invocation()), Ok(slot));
        }
        for index in [0, 10] {
            let result = CommandAction::new(invocation(
                ids::SELECT_TAB,
                &[("index", CommandValue::Integer(index))],
            ));
            assert!(
                matches!(result, Err(CommandError::ArgumentRange { value, .. }) if value == index),
                "index {index}: {result:?}"
            );
        }
        assert!(matches!(
            select_tab_slot(&invocation(ids::SELECT_TAB, &[])),
            Err(CommandError::MissingArgument { name: "index", .. })
        ));
    }

    #[test]
    fn routing_requires_a_window_outside_application_scope() {
        assert_eq!(
            route::<u8>(CommandScope::Application, None),
            Ok(Route::Application)
        );
        assert_eq!(
            route::<u8>(CommandScope::Window, None),
            Err(CommandError::ClientRequired)
        );
        assert_eq!(
            route::<u8>(CommandScope::Runtime, None),
            Err(CommandError::ClientRequired)
        );
        assert_eq!(
            route::<u8>(CommandScope::Palette, None),
            Err(CommandError::ClientRequired)
        );
        assert_eq!(
            route(CommandScope::Terminal, Some(7)),
            Ok(Route::Terminal(7))
        );
        assert_eq!(route(CommandScope::Runtime, Some(7)), Ok(Route::Window(7)));
        assert_eq!(route(CommandScope::Palette, Some(7)), Ok(Route::Window(7)));
    }

    #[test]
    fn omitted_rename_target_uses_window_context_or_reports_unavailable() {
        let rename = invocation(
            ids::RENAME_TAB,
            &[("name", CommandValue::Text("work".into()))],
        );
        let missing = fill_target(&rename, "tab", None);
        assert!(
            matches!(missing, Err(CommandError::Unavailable(_))),
            "{missing:?}"
        );
        let filled =
            fill_target(&rename, "tab", Some(CommandValue::Tab(TabId::new(3))))
                .unwrap();
        assert_eq!(filled.tab("tab"), Some(TabId::new(3)));
        assert!(validate(&filled).is_ok());

        let explicit = invocation(
            ids::RENAME_TAB,
            &[
                ("name", CommandValue::Text("work".into())),
                ("tab", CommandValue::Tab(TabId::new(1))),
            ],
        );
        let kept = fill_target(
            &explicit,
            "tab",
            Some(CommandValue::Tab(TabId::new(3))),
        )
        .unwrap();
        assert_eq!(kept.tab("tab"), Some(TabId::new(1)));
    }

    #[test]
    fn explicit_session_target_skips_the_workspace_fill() {
        let name = ("name", CommandValue::Text("work".into()));
        let explicit = invocation(
            ids::RENAME_SESSION,
            &[
                name.clone(),
                ("session", CommandValue::Session(SessionId::new(2))),
            ],
        );
        assert_eq!(
            fill_rename_target(&explicit, None, None),
            Ok(explicit.clone())
        );
        assert!(validate(&explicit).is_ok());

        let omitted = invocation(ids::RENAME_SESSION, &[name]);
        assert!(matches!(
            fill_rename_target(&omitted, None, None),
            Err(CommandError::Unavailable(_))
        ));
        let filled =
            fill_rename_target(&omitted, None, Some(WorkspaceId::new(4)))
                .unwrap();
        assert_eq!(filled.workspace("workspace"), Some(WorkspaceId::new(4)));
        assert_eq!(filled.session("session"), None);
    }
}
