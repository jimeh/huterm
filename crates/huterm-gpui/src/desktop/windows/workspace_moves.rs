//! The keyboard and palette equivalents of the workspace drags: a whole
//! tab to another workspace, a workspace to another session, and a
//! workspace or session one place up or down its list. Each commits the
//! operation a drop would, through the same completion, so stale
//! destinations, busy windows, Quit, and the emptied-workspace rules
//! behave as they do for a drag.

use super::drag::Operation;
use super::{
    App, CommandError, CommandInvocation, CommandOutcome, Context, Desktop,
    HierarchyState, SessionId, TabId, Window, WorkspaceId, WorkspaceView,
};

/// Where a one-place move puts an item in its list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Step<T> {
    /// Before this item.
    Before(T),
    /// After every other item.
    Last,
}

impl<T> Step<T> {
    /// The `before` argument the Mux's moves take.
    pub(super) fn before(self) -> Option<T> {
        match self {
            Self::Before(item) => Some(item),
            Self::Last => None,
        }
    }
}

/// Where moving `item` one place up or down `order` puts it, or `None`
/// when it is already at that end or not in `order`.
pub(super) fn step<T: Copy + Eq>(
    order: &[T],
    item: T,
    up: bool,
) -> Option<Step<T>> {
    let at = order.iter().position(|id| *id == item)?;
    if up {
        let previous = at.checked_sub(1)?;
        order.get(previous).copied().map(Step::Before)
    } else {
        // Past the next item: before the one after it, else last.
        order.get(at + 1)?;
        Some(order.get(at + 2).copied().map_or(Step::Last, Step::Before))
    }
}

/// Whether a workspace other than `workspace` exists, in any session.
pub(super) fn other_workspace(
    state: &HierarchyState,
    workspace: WorkspaceId,
) -> bool {
    state
        .sessions()
        .iter()
        .filter_map(|session| state.session_workspaces(*session))
        .flatten()
        .any(|other| *other != workspace)
}

/// A tab moves to another workspace only while one exists, in any session.
pub(super) fn tab_to_workspace_availability(
    other_workspace: bool,
) -> Result<(), CommandError> {
    if other_workspace {
        Ok(())
    } else {
        Err(CommandError::Unavailable(
            "There is no other workspace.".to_owned(),
        ))
    }
}

impl WorkspaceView {
    /// Refuses Move Tab to Workspace as a drop on a row would, and returns
    /// the tab, its workspace, and the destination: `workspace`, when it
    /// names another live workspace. The palette checks availability with
    /// no destination yet, which only needs another workspace to exist.
    pub(super) fn check_tab_to_workspace_available(
        &self,
        tab: Option<TabId>,
        destination: Option<WorkspaceId>,
        cx: &App,
    ) -> Result<(TabId, WorkspaceId, Option<WorkspaceId>), CommandError> {
        self.check_new_tab_available(cx)?;
        self.check_runtime_available(cx)?;
        let tab = match tab {
            Some(tab) => self.existing_tab(tab)?,
            None => self.active_tab_id(cx)?,
        };
        let state = cx.global::<Desktop>().hierarchy.state();
        let from = state.tab_workspace(tab).ok_or(CommandError::StaleTarget)?;
        let Some(to) = destination else {
            tab_to_workspace_availability(other_workspace(state, from))?;
            return Ok((tab, from, None));
        };
        if state.workspace(to).is_none() {
            return Err(CommandError::StaleTarget);
        }
        if to == from {
            return Err(CommandError::Unavailable(
                "the tab is already in that workspace".to_owned(),
            ));
        }
        Ok((tab, from, Some(to)))
    }

    /// Moves a whole tab to the end of another workspace, as a drop on its
    /// row does: the tab keeps its layout and terminals, and the window
    /// keeps showing its workspace.
    pub(super) fn move_tab_to_workspace(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        let destination = invocation
            .workspace("workspace")
            .ok_or(CommandError::StaleTarget)?;
        let (tab, workspace, to) = self.check_tab_to_workspace_available(
            invocation.tab("tab"),
            Some(destination),
            cx,
        )?;
        let to = to.ok_or(CommandError::StaleTarget)?;
        self.cancel_drag(window, cx);
        self.commit_tab_move(
            Operation::MoveTab {
                workspace,
                tab,
                to,
                before: None,
            },
            None,
            window,
            cx,
        );
        Ok(CommandOutcome::Accepted)
    }

    /// The window's session and `workspace`, else the window's workspace,
    /// while it belongs to that session.
    fn own_session_workspace(
        &self,
        workspace: Option<WorkspaceId>,
        cx: &App,
    ) -> Result<(SessionId, WorkspaceId), CommandError> {
        let session = self.attached_session(cx).ok_or_else(|| {
            CommandError::Unavailable("window has no session".to_owned())
        })?;
        let workspace = workspace
            .or_else(|| self.workspace_id(cx))
            .filter(|workspace| {
                cx.global::<Desktop>()
                    .hierarchy
                    .state()
                    .workspace_session(*workspace)
                    == Some(session)
            })
            .ok_or(CommandError::StaleTarget)?;
        Ok((session, workspace))
    }

    /// Refuses Move Workspace to Session as a drop on another window
    /// would, and returns the workspace's session, the workspace, and the
    /// destination: `destination`, when it names another live session.
    /// A session's only workspace may move, as by drag; its emptied window
    /// then closes.
    pub(super) fn check_workspace_to_session_available(
        &self,
        workspace: Option<WorkspaceId>,
        destination: Option<SessionId>,
        cx: &App,
    ) -> Result<(SessionId, WorkspaceId, Option<SessionId>), CommandError> {
        self.check_new_tab_available(cx)?;
        self.check_runtime_available(cx)?;
        let (session, workspace) = self.own_session_workspace(workspace, cx)?;
        let state = cx.global::<Desktop>().hierarchy.state();
        let Some(to) = destination else {
            if !state.sessions().iter().any(|other| *other != session) {
                return Err(CommandError::Unavailable(
                    "There is no other session.".to_owned(),
                ));
            }
            return Ok((session, workspace, None));
        };
        if state.session(to).is_none() {
            return Err(CommandError::StaleTarget);
        }
        if to == session {
            return Err(CommandError::Unavailable(
                "the workspace is already in that session".to_owned(),
            ));
        }
        Ok((session, workspace, Some(to)))
    }

    /// Moves a workspace to the end of another session. Windows that showed
    /// it follow through their emptied close, as after a drop on another
    /// window, which closes a window whose session it emptied; windows on
    /// the destination hold it hidden.
    pub(super) fn move_workspace_to_session(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        let destination = invocation
            .session("session")
            .ok_or(CommandError::StaleTarget)?;
        let (session, workspace, to) = self
            .check_workspace_to_session_available(
                invocation.workspace("workspace"),
                Some(destination),
                cx,
            )?;
        let to = to.ok_or(CommandError::StaleTarget)?;
        self.cancel_drag(window, cx);
        self.move_workspace(session, workspace, to, None, window, cx);
        Ok(CommandOutcome::Accepted)
    }

    /// Refuses Move Workspace Up or Down at its end of the session, and
    /// returns the session, the workspace, and the slot it moves before.
    pub(super) fn check_workspace_step_available(
        &self,
        workspace: Option<WorkspaceId>,
        up: bool,
        cx: &App,
    ) -> Result<(SessionId, WorkspaceId, Option<WorkspaceId>), CommandError>
    {
        self.check_new_tab_available(cx)?;
        self.check_runtime_available(cx)?;
        let (session, workspace) = self.own_session_workspace(workspace, cx)?;
        let before = step(&self.session_workspaces(cx), workspace, up)
            .map(Step::before)
            .ok_or_else(|| {
                CommandError::Unavailable(
                    if up {
                        "workspace is first"
                    } else {
                        "workspace is last"
                    }
                    .to_owned(),
                )
            })?;
        Ok((session, workspace, before))
    }

    /// Moves a workspace one place up or down its session, as a row drag
    /// does.
    pub(super) fn move_workspace_step(
        &mut self,
        workspace: Option<WorkspaceId>,
        up: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        let (session, workspace, before) =
            self.check_workspace_step_available(workspace, up, cx)?;
        self.cancel_drag(window, cx);
        self.move_workspace(session, workspace, session, before, window, cx);
        Ok(CommandOutcome::Accepted)
    }

    /// Moves `session` one place up or down the session order, as a session
    /// row drag does; does nothing at its end.
    pub(super) fn move_session_step(
        &mut self,
        session: SessionId,
        up: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.check_new_tab_available(cx)?;
        self.check_runtime_available(cx)?;
        let order =
            cx.global::<Desktop>().hierarchy.state().sessions().to_vec();
        let Some(before) = step(&order, session, up).map(Step::before) else {
            return Ok(CommandOutcome::Completed);
        };
        self.cancel_drag(window, cx);
        Self::move_session(session, before, cx);
        Ok(CommandOutcome::Accepted)
    }
}

#[cfg(test)]
mod tests {
    use super::{Step, step};

    #[test]
    fn a_step_moves_one_place_and_stops_at_the_ends() {
        let order = [1, 2, 3];
        assert_eq!(step(&order, 1, true), None);
        assert_eq!(step(&order, 2, true), Some(Step::Before(1)));
        assert_eq!(step(&order, 3, true), Some(Step::Before(2)));
        assert_eq!(step(&order, 1, false), Some(Step::Before(3)));
        // The second-to-last moves last.
        assert_eq!(step(&order, 2, false), Some(Step::Last));
        assert_eq!(step(&order, 3, false), None);
        assert_eq!(step(&order, 4, false), None);
        assert_eq!(step(&[1], 1, true), None);
    }
}
