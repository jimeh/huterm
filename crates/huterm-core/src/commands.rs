//! Runtime-scope command execution against the canonical [`Mux`].
//!
//! Clients resolve omitted target arguments from their own context before
//! calling [`execute`]; the runtime never guesses a target.

use huterm_protocol::{
    CommandError, CommandInvocation, CommandOutcome, CommandScope, ids,
    validate,
};

use crate::{Mux, MuxError};

/// Validates an invocation and runs it against the runtime.
///
/// Only [`CommandScope::Runtime`] commands execute here; other scopes belong to
/// a client. Callers must already hold the structural owner: this never
/// blocks, spawns, or touches a UI thread by itself.
///
/// # Errors
/// Reports catalog validation failures, [`CommandError::ClientRequired`] for
/// client-scoped commands, a missing target argument, and Mux failures mapped
/// to [`CommandError::StaleTarget`] or [`CommandError::Runtime`].
pub fn execute(
    mux: &mut Mux,
    invocation: &CommandInvocation,
) -> Result<CommandOutcome, CommandError> {
    let spec = validate(invocation)?;
    if spec.scope != CommandScope::Runtime {
        return Err(CommandError::ClientRequired);
    }
    let missing = |name| CommandError::MissingArgument {
        command: spec.id,
        name,
    };
    let name = || invocation.text("name").ok_or_else(|| missing("name"));
    let result = match spec.id {
        ids::RENAME_TAB => {
            let tab = invocation.tab("tab").ok_or_else(|| missing("tab"))?;
            mux.rename_tab(tab, Some(name()?))
        }
        ids::RENAME_WORKSPACE => {
            let workspace = invocation
                .workspace("workspace")
                .ok_or_else(|| missing("workspace"))?;
            mux.rename_workspace(workspace, Some(name()?))
        }
        ids::RENAME_SESSION => {
            let session = invocation
                .session("session")
                .ok_or_else(|| missing("session"))?;
            mux.rename_session(session, Some(name()?))
        }
        ids::RESET_TAB_NAME => {
            let tab = invocation.tab("tab").ok_or_else(|| missing("tab"))?;
            mux.rename_tab(tab, None)
        }
        ids::RESET_WORKSPACE_NAME => {
            let workspace = invocation
                .workspace("workspace")
                .ok_or_else(|| missing("workspace"))?;
            mux.rename_workspace(workspace, None)
        }
        ids::RESET_SESSION_NAME => {
            let session = invocation
                .session("session")
                .ok_or_else(|| missing("session"))?;
            mux.rename_session(session, None)
        }
        other => {
            return Err(CommandError::Unavailable(format!(
                "runtime command `{other}` has no executor"
            )));
        }
    };
    result
        .map(|()| CommandOutcome::Completed)
        .map_err(map_error)
}

fn map_error(error: MuxError) -> CommandError {
    match error {
        MuxError::ForeignRuntime(_)
        | MuxError::UnknownSession(_)
        | MuxError::UnknownWorkspace(_)
        | MuxError::UnknownTab(_) => CommandError::StaleTarget,
        other => CommandError::Runtime(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use huterm_protocol::{
        CellSize, CommandArgument, CommandValue, GridSize, TabId,
        TerminalCommand, TerminalEngineKind,
    };

    fn command() -> TerminalCommand {
        TerminalCommand {
            engine: TerminalEngineKind::Alacritty,
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), "read value".into()],
            working_directory: std::env::current_dir().unwrap(),
            environment: Vec::new(),
            grid_size: GridSize::clamped(40, 8),
            cell_size: CellSize {
                width: 8,
                height: 16,
            },
        }
    }

    fn rename_tab(name: &str, tab: Option<TabId>) -> CommandInvocation {
        let mut args = vec![CommandArgument::new(
            "name",
            CommandValue::Text(name.into()),
        )];
        if let Some(tab) = tab {
            args.push(CommandArgument::new("tab", CommandValue::Tab(tab)));
        }
        CommandInvocation::new(ids::RENAME_TAB, args)
    }

    #[test]
    fn renames_through_runtime_and_rejects_bad_targets_without_change() {
        let mut mux = Mux::default();
        let session = mux.create_session(None).unwrap();
        let workspace = mux.create_workspace(session, None).unwrap();
        let tab = mux.open_tab(workspace, &command()).unwrap().tab;
        assert_eq!(
            execute(&mut mux, &rename_tab("custom", Some(tab.id))),
            Ok(CommandOutcome::Completed)
        );
        assert_eq!(mux.tab(tab.id).unwrap().custom_name(), Some("custom"));

        assert_eq!(
            execute(&mut mux, &rename_tab("later", None)),
            Err(CommandError::MissingArgument {
                command: ids::RENAME_TAB,
                name: "tab"
            })
        );
        assert!(matches!(
            execute(&mut mux, &rename_tab(" ", Some(tab.id))),
            Err(CommandError::Runtime(_))
        ));
        let foreign =
            TabId::in_runtime(Mux::default().runtime_id(), tab.id.get());
        assert_eq!(
            execute(&mut mux, &rename_tab("foreign", Some(foreign))),
            Err(CommandError::StaleTarget)
        );
        assert_eq!(mux.tab(tab.id).unwrap().custom_name(), Some("custom"));

        assert_eq!(
            execute(
                &mut mux,
                &CommandInvocation::new(ids::NEW_TAB, Vec::new())
            ),
            Err(CommandError::ClientRequired)
        );

        mux.close_tab(workspace, tab.id).unwrap();
        assert_eq!(
            execute(&mut mux, &rename_tab("gone", Some(tab.id))),
            Err(CommandError::StaleTarget)
        );
        assert_eq!(mux.workspace(workspace).unwrap().tabs, Vec::new());
        mux.close_session(session).unwrap();
    }

    #[test]
    fn reset_clears_custom_names_and_is_idempotent() {
        let mut mux = Mux::default();
        let session = mux.create_session(Some("custom session")).unwrap();
        let workspace = mux
            .create_workspace(session, Some("custom workspace"))
            .unwrap();
        let tab = mux.open_tab(workspace, &command()).unwrap().tab;
        mux.rename_tab(tab.id, Some("custom tab")).unwrap();

        let resets = [
            CommandInvocation::new(
                ids::RESET_TAB_NAME,
                vec![CommandArgument::new("tab", CommandValue::Tab(tab.id))],
            ),
            CommandInvocation::new(
                ids::RESET_WORKSPACE_NAME,
                vec![CommandArgument::new(
                    "workspace",
                    CommandValue::Workspace(workspace),
                )],
            ),
            CommandInvocation::new(
                ids::RESET_SESSION_NAME,
                vec![CommandArgument::new(
                    "session",
                    CommandValue::Session(session),
                )],
            ),
        ];

        for reset in &resets {
            assert_eq!(execute(&mut mux, reset), Ok(CommandOutcome::Completed));
        }
        assert_eq!(mux.tab(tab.id).unwrap().custom_name(), None);
        assert_eq!(mux.workspace(workspace).unwrap().custom_name(), None);
        assert_eq!(mux.session(session).unwrap().custom_name(), None);

        for reset in &resets {
            assert_eq!(execute(&mut mux, reset), Ok(CommandOutcome::Completed));
        }

        let foreign =
            TabId::in_runtime(Mux::default().runtime_id(), tab.id.get());
        assert_eq!(
            execute(
                &mut mux,
                &CommandInvocation::new(
                    ids::RESET_TAB_NAME,
                    vec![CommandArgument::new(
                        "tab",
                        CommandValue::Tab(foreign),
                    )],
                ),
            ),
            Err(CommandError::StaleTarget)
        );
        assert_eq!(mux.tab(tab.id).unwrap().custom_name(), None);

        mux.close_session(session).unwrap();
    }
}
