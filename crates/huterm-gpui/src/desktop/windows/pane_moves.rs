//! The commands that swap and move panes, extract a pane into a new tab,
//! and move a whole tab into a pane. Each resolves its target from the
//! projection and this window's focus, refuses a result that would leave a
//! pane under the minimum size in this window, and commits one Mux
//! operation on a worker. Reconcile then moves the affected pane views
//! between tabs, so a moved pane keeps its view, viewer, and state.

// User input picks positions here, so a stale index must not panic.
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use huterm_core::{Beside, MoveDestination, PaneRef};
use huterm_protocol::{LayoutChange, PaneNode, SIDES};

use super::pane_drag::SpringBack;
use super::*;

/// One rearrangement a command or a drop commits.
#[derive(Clone, Copy, Debug)]
pub(super) enum PaneMove {
    /// Swaps two panes of one tab.
    Swap {
        workspace: WorkspaceId,
        tab: TabId,
        pane: PaneId,
        other: PaneId,
    },
    /// Moves a pane beside another or into a new tab.
    Move { from: PaneRef, to: MoveDestination },
    /// Moves a whole tab beside a pane of another tab.
    TabInto {
        workspace: WorkspaceId,
        tab: TabId,
        to: Beside,
    },
}

impl PaneMove {
    /// The notice title a failed commit raises.
    fn title(self) -> &'static str {
        match self {
            Self::Swap { .. } => "Swap panes",
            Self::Move { .. } => "Move pane",
            Self::TabInto { .. } => "Move tab",
        }
    }

    /// The name undo and redo show for the change.
    fn label(self) -> &'static str {
        match self {
            Self::Swap { .. } => "Swap Panes",
            Self::Move { .. } => "Move Pane",
            Self::TabInto { .. } => "Move Tab into Pane",
        }
    }
}

impl DesktopRuntime {
    /// Commits `change` under the Mux lock, refusing during teardown.
    fn commit_pane_move(
        &self,
        change: PaneMove,
    ) -> Committed<Result<LayoutChange, MuxError>> {
        let mut mux = self.lock();
        let result = if self.terminating.load(Ordering::Acquire) {
            Err(RuntimeError::Stopped.into())
        } else {
            match change {
                PaneMove::Swap {
                    workspace,
                    tab,
                    pane,
                    other,
                } => mux.swap_panes(workspace, tab, pane, other),
                PaneMove::Move { from, to } => mux.move_pane(from, to),
                PaneMove::TabInto { workspace, tab, to } => {
                    mux.move_tab_to_pane(workspace, tab, to)
                }
            }
        };
        Committed {
            result,
            seq: mux.hierarchy_seq(),
        }
    }
}

/// The swap, move, extract, and tab-into-pane commands.
pub(super) fn is_pane_move_command(
    command: huterm_protocol::CommandId,
) -> bool {
    matches!(
        command,
        ids::SWAP_PANE_NEXT
            | ids::SWAP_PANE_PREVIOUS
            | ids::SWAP_PANE_LEFT
            | ids::SWAP_PANE_RIGHT
            | ids::SWAP_PANE_UP
            | ids::SWAP_PANE_DOWN
            | ids::MOVE_PANE_NEXT
            | ids::MOVE_PANE_PREVIOUS
            | ids::MOVE_PANE_LEFT
            | ids::MOVE_PANE_RIGHT
            | ids::MOVE_PANE_UP
            | ids::MOVE_PANE_DOWN
            | ids::MOVE_PANE_TO_PANE
            | ids::MOVE_PANE_TO_NEW_TAB
            | ids::MOVE_TAB_TO_PANE
    )
}

/// The direction a directional swap or move command names.
fn command_direction(
    command: huterm_protocol::CommandId,
) -> Option<SplitDirection> {
    use SplitDirection::{Down, Left, Right, Up};
    match command {
        ids::SWAP_PANE_LEFT | ids::MOVE_PANE_LEFT => Some(Left),
        ids::SWAP_PANE_RIGHT | ids::MOVE_PANE_RIGHT => Some(Right),
        ids::SWAP_PANE_UP | ids::MOVE_PANE_UP => Some(Up),
        ids::SWAP_PANE_DOWN | ids::MOVE_PANE_DOWN => Some(Down),
        _ => None,
    }
}

/// The side a validated `side` argument names.
fn side(name: &str) -> Option<SplitDirection> {
    match SIDES.iter().position(|side| *side == name)? {
        0 => Some(SplitDirection::Left),
        1 => Some(SplitDirection::Right),
        2 => Some(SplitDirection::Up),
        3 => Some(SplitDirection::Down),
        _ => None,
    }
}

/// The workspace argument as a new-tab destination, defaulting to the
/// source pane's workspace. In its own workspace the new tab goes right
/// after the source tab.
fn new_tab_argument(
    invocation: &CommandInvocation,
    from: PaneRef,
    state: &HierarchyState,
) -> Result<MoveDestination, CommandError> {
    let workspace = invocation.workspace("workspace").unwrap_or(from.workspace);
    if state.workspace(workspace).is_none() {
        return Err(CommandError::StaleTarget);
    }
    let before = (workspace == from.workspace)
        .then(|| {
            let order = state.workspace_tabs(workspace)?;
            let index = order.iter().position(|id| *id == from.tab)?;
            order.get(index + 1).copied()
        })
        .flatten();
    Ok(MoveDestination::NewTab { workspace, before })
}

/// The pane and side arguments as a destination, with the pane resolved to
/// where the projection shows it now.
fn beside_argument(
    invocation: &CommandInvocation,
    cx: &App,
) -> Result<Beside, CommandError> {
    let pane = invocation.pane("pane").ok_or(CommandError::StaleTarget)?;
    let side = invocation
        .text("side")
        .and_then(side)
        .ok_or(CommandError::StaleTarget)?;
    let state = cx.global::<Desktop>().hierarchy.state();
    let tab = state.pane_tab(pane).ok_or(CommandError::StaleTarget)?;
    let workspace =
        state.tab_workspace(tab).ok_or(CommandError::StaleTarget)?;
    Ok(Beside {
        target: PaneRef {
            workspace,
            tab,
            pane,
        },
        side,
    })
}

/// A layout's panes with placeholder identities no runtime allocates, for
/// sizing a candidate layout before the runtime builds the real one.
fn probe_split() -> SplitId {
    SplitId::in_runtime(RuntimeId::new(u64::MAX), u64::MAX)
}

/// Distinct placeholder split identities in [`probe_split`]'s runtime,
/// counting down from its value, for a candidate whose splits all take
/// new identities: a layout rejects a repeated one.
pub(super) fn probe_splits() -> impl FnMut() -> SplitId {
    let mut next = u64::MAX;
    move || {
        let id = SplitId::in_runtime(RuntimeId::new(u64::MAX), next);
        next = next.saturating_sub(1);
        id
    }
}

impl WorkspaceView {
    /// Whether a pane move command can run now: no structural operation,
    /// confirmation, or quit in progress, and an active tab; the commands
    /// that step through the focused tab need it split.
    pub(super) fn pane_move_availability(
        &self,
        command: huterm_protocol::CommandId,
        cx: &App,
    ) -> Result<(), CommandError> {
        self.check_new_tab_available(cx)?;
        let tab = self.active_tab_view(cx).ok_or_else(|| {
            CommandError::Unavailable("window has no active tab".to_owned())
        })?;
        let within_tab = !matches!(
            command,
            ids::MOVE_PANE_TO_PANE
                | ids::MOVE_PANE_TO_NEW_TAB
                | ids::MOVE_TAB_TO_PANE
        );
        if within_tab && tab.panes.len() < 2 {
            return Err(CommandError::Unavailable(
                "tab has one pane".to_owned(),
            ));
        }
        Ok(())
    }

    /// Runs a swap, move, extract, or tab-into-pane command.
    pub(super) fn run_pane_move(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.pane_move_availability(invocation.id, cx)?;
        let workspace =
            self.workspace_id(cx).ok_or(CommandError::StaleTarget)?;
        let (tab, pane) = self.focused_pane(cx)?;
        let from = PaneRef {
            workspace,
            tab,
            pane,
        };
        // Wrapping and directional commands with nowhere to go do nothing.
        let Some(change) = self.plan_pane_move(invocation, from, cx)? else {
            return Ok(CommandOutcome::Completed);
        };
        self.check_move_fits(change, window, cx)?;
        let focus = match change {
            PaneMove::TabInto { tab, .. } => self
                .tabs
                .iter()
                .find(|view| view.id == tab)
                .map(|view| view.focused().pane),
            _ => Some(pane),
        };
        self.commit_pane_move(change, focus, None, window, cx);
        Ok(CommandOutcome::Accepted)
    }

    /// The change `invocation` asks of the focused pane `from`, or `None`
    /// when a wrapping or directional command has nowhere to go.
    fn plan_pane_move(
        &self,
        invocation: &CommandInvocation,
        from: PaneRef,
        cx: &App,
    ) -> Result<Option<PaneMove>, CommandError> {
        let PaneRef {
            workspace,
            tab,
            pane,
        } = from;
        let state = cx.global::<Desktop>().hierarchy.state();
        let layout = state
            .tab(tab)
            .ok_or(CommandError::StaleTarget)?
            .layout
            .clone();
        let change = match invocation.id {
            ids::SWAP_PANE_NEXT | ids::SWAP_PANE_PREVIOUS => {
                let forward = invocation.id == ids::SWAP_PANE_NEXT;
                layout.cycle(pane, forward).map(|other| PaneMove::Swap {
                    workspace,
                    tab,
                    pane,
                    other,
                })
            }
            ids::MOVE_PANE_NEXT | ids::MOVE_PANE_PREVIOUS => {
                let forward = invocation.id == ids::MOVE_PANE_NEXT;
                layout.step_target(pane, forward).map(|(target, side)| {
                    PaneMove::Move {
                        from,
                        to: MoveDestination::Beside(Beside {
                            target: PaneRef {
                                pane: target,
                                ..from
                            },
                            side,
                        }),
                    }
                })
            }
            ids::MOVE_PANE_TO_PANE => Some(PaneMove::Move {
                from,
                to: MoveDestination::Beside(beside_argument(invocation, cx)?),
            }),
            ids::MOVE_PANE_TO_NEW_TAB => Some(PaneMove::Move {
                from,
                to: new_tab_argument(invocation, from, state)?,
            }),
            ids::MOVE_TAB_TO_PANE => Some(PaneMove::TabInto {
                workspace,
                tab: self.target_tab(invocation, cx)?,
                to: beside_argument(invocation, cx)?,
            }),
            command => {
                let direction = command_direction(command)
                    .ok_or(CommandError::UnknownCommand(command))?;
                let neighbor = self
                    .tabs
                    .iter()
                    .find(|view| view.id == tab)
                    .and_then(|view| view.neighbor(pane, direction, cx));
                neighbor.map(|other| {
                    if matches!(
                        command,
                        ids::SWAP_PANE_LEFT
                            | ids::SWAP_PANE_RIGHT
                            | ids::SWAP_PANE_UP
                            | ids::SWAP_PANE_DOWN
                    ) {
                        PaneMove::Swap {
                            workspace,
                            tab,
                            pane,
                            other,
                        }
                    } else {
                        PaneMove::Move {
                            from,
                            to: MoveDestination::Beside(Beside {
                                target: PaneRef {
                                    pane: other,
                                    ..from
                                },
                                side: direction,
                            }),
                        }
                    }
                })
            }
        };
        Ok(change)
    }

    /// Refuses a move whose destination tab would leave a pane under the
    /// minimum pane size in this window, as a split refuses: a pane that
    /// met the minimum must keep it, and a pane that arrives must meet it.
    pub(super) fn check_move_fits(
        &self,
        change: PaneMove,
        window: &Window,
        cx: &App,
    ) -> Result<(), CommandError> {
        let too_small = || {
            CommandError::Unavailable(
                "the destination pane is too small to take it".to_owned(),
            )
        };
        let state = cx.global::<Desktop>().hierarchy.state();
        let layout_of = |tab: TabId| {
            state
                .tab(tab)
                .map(|info| info.layout.clone())
                .ok_or(CommandError::StaleTarget)
        };
        let (current, candidate) = match change {
            // Slots keep their sizes, so a swap never shrinks one.
            PaneMove::Swap { .. }
            | PaneMove::Move {
                to: MoveDestination::NewTab { .. },
                ..
            } => return Ok(()),
            PaneMove::Move {
                from,
                to: MoveDestination::Beside(beside),
            } => {
                let source = layout_of(from.tab)?;
                let moved = source
                    .pane(from.pane)
                    .cloned()
                    .ok_or(CommandError::StaleTarget)?;
                let current = layout_of(beside.target.tab)?;
                let base = if beside.target.tab == from.tab {
                    current
                        .without_pane(from.pane)
                        .map_err(|_| CommandError::StaleTarget)?
                } else {
                    current.clone()
                };
                let candidate = base
                    .with_inserted(
                        beside.target.pane,
                        beside.side,
                        PaneNode::Pane(moved),
                        probe_split(),
                    )
                    .map_err(|_| too_small())?;
                (current, candidate)
            }
            PaneMove::TabInto { tab, to, .. } => {
                if tab == to.target.tab {
                    return Err(CommandError::Unavailable(
                        "a tab cannot move into its own pane".to_owned(),
                    ));
                }
                let source = layout_of(tab)?;
                let current = layout_of(to.target.tab)?;
                let candidate = current
                    .with_inserted(
                        to.target.pane,
                        to.side,
                        source.root().clone(),
                        probe_split(),
                    )
                    .map_err(|_| too_small())?;
                (current, candidate)
            }
        };
        let (Some(before), Some(after)) = (
            self.pane_grids(&current, window),
            self.pane_grids(&candidate, window),
        ) else {
            return Ok(());
        };
        let arrived: Vec<_> = after
            .iter()
            .filter(|(pane, _)| !before.iter().any(|(id, _)| id == pane))
            .copied()
            .collect();
        if keeps_minimum(&before, &after) && fits_minimum(&arrived) {
            Ok(())
        } else {
            Err(too_small())
        }
    }

    /// Commits `change` on a worker. Once the projection holds the result,
    /// reconcile has moved the pane views; the window then shows the tab
    /// that holds `focus` and focuses it there. A refused drop that
    /// spring-loaded switches `back`.
    pub(super) fn commit_pane_move(
        &mut self,
        change: PaneMove,
        focus: Option<PaneId>,
        back: Option<SpringBack>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        // A move unzooms the tabs it touches in this window, as a split
        // does; a swap keeps the zoom on its pane.
        if !matches!(change, PaneMove::Swap { .. }) {
            let touched = match change {
                PaneMove::Move { from, to } => vec![
                    Some(from.tab),
                    match to {
                        MoveDestination::Beside(beside) => {
                            Some(beside.target.tab)
                        }
                        MoveDestination::NewTab { .. } => None,
                    },
                ],
                PaneMove::TabInto { tab, to, .. } => {
                    vec![Some(tab), Some(to.target.tab)]
                }
                PaneMove::Swap { .. } => Vec::new(),
            };
            for view in &mut self.tabs {
                if touched.contains(&Some(view.id)) {
                    view.zoomed = None;
                }
            }
            self.sync_zoom(window, cx);
        }
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        self.busy = true;
        let task = cx
            .background_executor()
            .spawn(async move { runtime.commit_pane_move(change) });
        let app = cx.to_async();
        cx.spawn_in(window, async move |view, cx| {
            let Committed { result, seq } = task.await;
            projected(&app, seq).await;
            let _ = view.update_in(cx, |view, window, cx| {
                view.busy = false;
                if let Err(error) = &result
                    && !terminating(cx)
                {
                    view.report_failure(
                        change.title(),
                        format!(
                            "Cannot {}: {error}",
                            change.title().to_lowercase()
                        ),
                        cx,
                    );
                }
                view.reconcile_own(window, cx);
                if let Ok(made) = &result {
                    view.record_layout_change(
                        change.label(),
                        focus,
                        made.clone(),
                        seq,
                        cx,
                    );
                }
                if result.is_err()
                    && let Some(back) = back
                {
                    view.switch_back(back, window, cx);
                }
                if result.is_ok()
                    && let Some(pane) = focus
                    && let Some(tab) = view.tab_showing(pane)
                {
                    if view.active_tab(cx) != Some(tab) {
                        view.select(tab, window, cx);
                    }
                    view.focus_pane(tab, pane, window, cx);
                }
                view.restore_tab_focus(window, cx);
                view.resume_close(window, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
