//! The commands that rearrange a tab's panes without moving any between
//! tabs: `arrange_panes` into a named layout, the next and previous layout
//! of the cycle, and `balance_panes`. Each fits the arrangement to this
//! window's minimum pane size, then commits it on the window's ordered
//! layout commit queue, which divider drags and resize commands share.
//!
//! An arrangement holds no `busy`: it changes no membership, so terminal
//! input and reconcile carry on. Instead, a per-window count of
//! arrangements in flight refuses structural commands and divider presses
//! until each completion, so none computes from the layout it replaces.

// User input picks positions here, so a stale index must not panic.
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use gpui::SharedString;
use huterm_protocol::{
    Arrangement, LayoutChange, LayoutError, NamedLayout, PaneNode,
};

use std::fmt::Write as _;

use super::pane_geometry::{Fit, PaneMetrics, fit};
use super::pane_moves::probe_splits;
use super::*;

/// One job of a window's layout commit queue.
#[derive(Clone, Debug)]
pub(super) enum LayoutCommit {
    /// A divider drag's or resize command's weights for one split.
    Resize(ResizePreview),
    /// A whole tab's new arrangement.
    Arrange(Arrange),
}

/// An arrangement a layout command committed, with what its completion
/// records.
#[derive(Clone, Debug)]
pub(super) struct Arrange {
    tab: TabId,
    /// The layout the arrangement was fitted against; the runtime refuses
    /// the commit once the tab's structure differs from it.
    expected: PaneLayout,
    arrangement: Arrangement,
    /// The undo entry's name.
    label: SharedString,
    /// The pane this window focused, which undo and redo focus again.
    focus: PaneId,
    /// The view's own zoom the commit cleared, which a refused or empty
    /// commit gives back.
    zoomed: Option<PaneId>,
}

impl DesktopRuntime {
    /// Commits one layout queue job under the Mux lock. A resize returns
    /// an empty change: the queue records no undo entry for it.
    fn commit_layout(
        &self,
        workspace: WorkspaceId,
        commit: &LayoutCommit,
    ) -> Committed<Result<LayoutChange, MuxError>> {
        match commit {
            LayoutCommit::Resize(preview) => {
                let Committed { result, seq } =
                    self.resize_split(workspace, preview.tab, preview);
                Committed {
                    result: result.map(|()| LayoutChange::default()),
                    seq,
                }
            }
            LayoutCommit::Arrange(arrange) => {
                let mut mux = self.lock();
                let result = if self.terminating.load(Ordering::Acquire) {
                    Err(RuntimeError::Stopped.into())
                } else {
                    mux.arrange_panes(
                        workspace,
                        arrange.tab,
                        &arrange.expected,
                        &arrange.arrangement,
                    )
                };
                Committed {
                    result,
                    seq: mux.hierarchy_seq(),
                }
            }
        }
    }
}

/// The layout commands.
pub(super) fn is_layout_command(command: huterm_protocol::CommandId) -> bool {
    matches!(
        command,
        ids::ARRANGE_PANES
            | ids::NEXT_PANE_LAYOUT
            | ids::PREVIOUS_PANE_LAYOUT
            | ids::BALANCE_PANES
    )
}

/// The undo entry's name for arranging into `layout`, such as "Arrange
/// Panes: Main Left" or "Arrange Panes: dev".
fn arrange_label(layout: &NamedLayout) -> SharedString {
    format!("Arrange Panes: {}", layout.title).into()
}

/// How a notice names `layout` in a sentence: its title in lower case,
/// such as "equal rows", so notices match the names the palette shows.
fn prose(layout: &NamedLayout) -> String {
    layout.title.to_lowercase()
}

/// The most panes `layout` holds below `count`, which it cannot hold: the
/// largest smaller count it expands for.
fn capacity(layout: &NamedLayout, count: usize) -> Option<usize> {
    let panes: Vec<PaneId> = (0..count)
        .map(|value| PaneId::in_runtime(RuntimeId::new(u64::MAX), value as u64))
        .collect();
    (1..count).rev().find(|held| {
        panes
            .get(..*held)
            .is_some_and(|panes| layout.template.expand(panes).is_ok())
    })
}

/// Why a layout command changes nothing, as its notice says it.
#[derive(Debug, Eq, PartialEq)]
enum Refusal {
    UnknownLayout(String),
    TooManyPanes { layout: String, most: usize },
    TooSmall(String),
    NoneFits,
    NoneChanges,
}

impl Refusal {
    fn message(&self) -> String {
        match self {
            Self::UnknownLayout(name) => {
                format!("No pane layout named `{name}`")
            }
            Self::TooManyPanes { layout, most } => {
                format!("The {layout} layout holds at most {most} panes")
            }
            Self::TooSmall(layout) => {
                format!("Window too small for the {layout} layout")
            }
            Self::NoneFits => "No other pane layout fits this window".into(),
            Self::NoneChanges => {
                "No other pane layout in the cycle changes these panes".into()
            }
        }
    }
}

impl WorkspaceView {
    /// Whether a layout command can run now: nothing structural, no
    /// confirmation, and no arrangement in flight, on a tab of two or more
    /// panes.
    pub(super) fn pane_layout_availability(
        &self,
        cx: &App,
    ) -> Result<(), CommandError> {
        self.check_navigation_available(cx)?;
        if self
            .active_tab_view(cx)
            .is_some_and(|tab| tab.panes.len() > 1)
        {
            Ok(())
        } else {
            Err(CommandError::Unavailable("tab has one pane".to_owned()))
        }
    }

    /// Runs `arrange_panes`, `next_pane_layout`, `previous_pane_layout`,
    /// or `balance_panes` on the active tab. A press while this window's
    /// last arrangement is in flight does nothing, so a held key repeats
    /// quietly. A refusal raises one notice; a fitted arrangement equal to
    /// the committed layout changes nothing at all.
    pub(super) fn run_pane_layout(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        if self.arrangements > 0 {
            return Ok(CommandOutcome::Completed);
        }
        self.pane_layout_availability(cx)?;
        let (tab, focus) = self.focused_pane(cx)?;
        let Some(bounds) = self.terminal_geometry(window).bounds else {
            return Ok(CommandOutcome::Completed);
        };
        let committed = self
            .committed_layout(tab, cx)
            .ok_or(CommandError::StaleTarget)?
            .into_owned();
        let scale = window.scale_factor();
        let grid = self.metrics.at_scale(scale);
        let metrics = PaneMetrics {
            cell: size(grid.cell_width, grid.cell_height),
            window: self.config.window,
            bars: self.title_bars(),
        };
        let fitted = |arrangement: &Arrangement| {
            fit(&committed, arrangement, bounds, scale, metrics)
        };
        let planned = match invocation.id {
            ids::ARRANGE_PANES => {
                let name = invocation
                    .text("layout")
                    .ok_or(CommandError::StaleTarget)?;
                arrange(&committed, &self.config.panes, name, fitted)
            }
            ids::NEXT_PANE_LAYOUT | ids::PREVIOUS_PANE_LAYOUT => {
                let forward = invocation.id == ids::NEXT_PANE_LAYOUT;
                let cycle = self.config.panes.cycle();
                step(&committed, &cycle, forward, fitted)
            }
            ids::BALANCE_PANES => {
                // Balancing never refuses: a window too small for the
                // tab's tree takes the proportional fallback.
                let (Fit::Fits(arrangement) | Fit::Short(arrangement)) =
                    fitted(&committed.balanced());
                Ok(("Balance Panes".into(), arrangement))
            }
            other => return Err(CommandError::UnknownCommand(other)),
        };
        let (label, arrangement) = match planned {
            Ok(planned) => planned,
            Err(refusal) => {
                let title = huterm_protocol::lookup(invocation.id.as_str())
                    .map_or("Arrange Panes", |spec| spec.title);
                self.report_failure(title, refusal.message(), cx);
                return Ok(CommandOutcome::Completed);
            }
        };
        if committed
            .arranged(&arrangement, probe_splits())
            .is_ok_and(|placed| placed == committed)
        {
            return Ok(CommandOutcome::Completed);
        }
        // Behind a zoom the change would not show. A shared zoom clears in
        // the arrangement's own event, so undo restores it; the view's own
        // zoom comes back if the commit changes nothing.
        if self
            .divider_drag
            .as_ref()
            .is_some_and(|drag| drag.tab == tab)
        {
            self.cancel_divider_drag(cx);
        }
        let zoomed = self
            .tabs
            .iter_mut()
            .find(|view| view.id == tab)
            .and_then(|view| view.zoomed.take());
        self.sync_zoom(window, cx);
        self.arrangements += 1;
        self.queue_layout_commit(
            LayoutCommit::Arrange(Arrange {
                tab,
                expected: committed,
                arrangement,
                label,
                focus,
                zoomed,
            }),
            window,
            cx,
        );
        cx.notify();
        Ok(CommandOutcome::Accepted)
    }

    /// Queues `commit` behind this window's earlier layout commits. One
    /// worker runs them in order; each completion waits for the projection
    /// to reach its commit.
    pub(super) fn queue_layout_commit(
        &mut self,
        commit: LayoutCommit,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(workspace) = self.workspace_id(cx) else {
            if let LayoutCommit::Arrange(arrange) = commit {
                self.arrangements = self.arrangements.saturating_sub(1);
                self.restore_zoom(arrange.tab, arrange.zoomed, window, cx);
            }
            return;
        };
        self.resize_commit_counter += 1;
        let id = self.resize_commit_counter;
        if let LayoutCommit::Resize(preview) = &commit {
            self.resize_commits.push((id, preview.clone()));
        }
        let queue = self.layout_queue.get_or_insert_with(|| {
            let (sender, receiver) =
                async_channel::unbounded::<(u64, WorkspaceId, LayoutCommit)>();
            let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
            let app = cx.to_async();
            cx.spawn_in(window, async move |view, cx| {
                while let Ok((id, workspace, commit)) = receiver.recv().await {
                    let runtime = Arc::clone(&runtime);
                    let job = commit.clone();
                    let Committed { result, seq } = cx
                        .background_executor()
                        .spawn(async move {
                            #[cfg(all(test, feature = "test-support"))]
                            runtime
                                .worker_hooks
                                .enter(worker_hooks::Stage::Layout)
                                .await;
                            let committed =
                                runtime.commit_layout(workspace, &job);
                            #[cfg(all(test, feature = "test-support"))]
                            runtime
                                .worker_hooks
                                .enter(worker_hooks::Stage::LayoutCommitted)
                                .await;
                            committed
                        })
                        .await;
                    if result.is_ok()
                        || matches!(commit, LayoutCommit::Arrange(_))
                    {
                        projected(&app, seq).await;
                    }
                    // A closed window still sends the commits it queued; the
                    // loop ends once its sender is dropped.
                    let _ = view.update_in(cx, |view, window, cx| {
                        view.finish_layout_commit(
                            id, commit, result, seq, window, cx,
                        );
                    });
                }
            })
            .detach();
            sender
        });
        // The receiver lives as long as the view, so sending cannot fail
        // while the view exists.
        let _ = queue.try_send((id, workspace, commit));
    }

    /// Settles one layout queue job once the projection holds it: a resize
    /// stops showing its preview, and an arrangement leaves the in-flight
    /// count and records its undo entry or raises its refusal. A refused or
    /// empty arrangement changed nothing, so it gives the view back the
    /// zoom it cleared.
    fn finish_layout_commit(
        &mut self,
        id: u64,
        commit: LayoutCommit,
        result: Result<LayoutChange, MuxError>,
        seq: u64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match commit {
            LayoutCommit::Resize(_) => {
                self.resize_commits.retain(|(commit, _)| *commit != id);
            }
            LayoutCommit::Arrange(arrange) => {
                self.arrangements = self.arrangements.saturating_sub(1);
                if result.as_ref().is_ok_and(LayoutChange::is_empty)
                    || (result.is_err() && !terminating(cx))
                {
                    self.restore_zoom(arrange.tab, arrange.zoomed, window, cx);
                }
                match result {
                    Ok(change) => self.record_layout_change(
                        arrange.label,
                        Some(arrange.focus),
                        change,
                        seq,
                        cx,
                    ),
                    Err(error) if !terminating(cx) => {
                        let message = match error {
                            MuxError::InvalidLayout(
                                LayoutError::ChildrenChanged,
                            ) => "The panes changed; try again".to_owned(),
                            error => format!("Cannot arrange panes: {error}"),
                        };
                        self.report_failure("Arrange Panes", message, cx);
                    }
                    Err(_) => {}
                }
            }
        }
        cx.notify();
    }
}

impl WorkspaceView {
    /// Zooms `pane` in `tab` again after an arrangement that changed
    /// nothing, unless the pane left the tab, the view zoomed meanwhile, or
    /// focus moved to another pane, which the zoom would pull it back from.
    fn restore_zoom(
        &mut self,
        tab: TabId,
        pane: Option<PaneId>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(pane) = pane else {
            return;
        };
        let Some(view) = self.tabs.iter_mut().find(|view| view.id == tab)
        else {
            return;
        };
        if view.zoomed.is_none()
            && view.panes.len() > 1
            && view.pane(pane).is_some()
            && view.focused().pane == pane
        {
            view.zoomed = Some(pane);
            self.sync_zoom(window, cx);
        }
    }

    /// The active tab's projected layout and this window's arrangements in
    /// flight, as `<prefix>pane_layout` and `<prefix>arranging` smoke
    /// fields.
    pub(super) fn layout_smoke_state(&self, prefix: &str, cx: &App) -> String {
        let layout = self
            .active_tab(cx)
            .and_then(|tab| cx.global::<Desktop>().hierarchy.state().tab(tab))
            .map_or_else(
                || "none".to_owned(),
                |info| smoke_layout(&info.layout),
            );
        format!(
            "{prefix}pane_layout={layout} {prefix}arranging={}",
            self.arrangements
        )
    }
}

/// `layout` as smoke state shows it: each pane by its traversal index,
/// split identities left out, and every split's axis (`C` or `R`) and
/// `weight:child` list, such as `C[600000:0,400000:R[500000:1,500000:2]]`.
fn smoke_layout(layout: &PaneLayout) -> String {
    fn walk(node: &PaneNode, order: &[PaneId], out: &mut String) {
        match node {
            PaneNode::Pane(pane) => {
                let index = order.iter().position(|id| *id == pane.id);
                out.push_str(
                    &index.map_or_else(|| "?".into(), |i| i.to_string()),
                );
            }
            PaneNode::Split(split) => {
                out.push(match split.axis {
                    SplitAxis::Columns => 'C',
                    SplitAxis::Rows => 'R',
                });
                out.push('[');
                for (index, child) in split.children.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    let _ = write!(out, "{}:", child.weight);
                    walk(&child.node, order, out);
                }
                out.push(']');
            }
        }
    }
    let order: Vec<PaneId> =
        layout.panes().into_iter().map(|pane| pane.id).collect();
    let mut out = String::new();
    walk(layout.root(), &order, &mut out);
    out
}

/// `arrange_panes` into the layout `layouts` calls `name`, fitted by
/// `fitted`.
fn arrange(
    committed: &PaneLayout,
    layouts: &huterm_config::PanesConfig,
    name: &str,
    fitted: impl Fn(&Arrangement) -> Fit,
) -> Result<(SharedString, Arrangement), Refusal> {
    let layout = layouts
        .layout(name)
        .ok_or_else(|| Refusal::UnknownLayout(name.to_owned()))?;
    let panes: Vec<PaneId> =
        committed.panes().into_iter().map(|pane| pane.id).collect();
    let ideal =
        layout
            .template
            .expand(&panes)
            .map_err(|_| Refusal::TooManyPanes {
                layout: prose(&layout),
                most: capacity(&layout, panes.len()).unwrap_or(1),
            })?;
    match fitted(&ideal) {
        Fit::Fits(arrangement) => Ok((arrange_label(&layout), arrangement)),
        Fit::Short(_) => Err(Refusal::TooSmall(prose(&layout))),
    }
}

/// The next or previous layout of `cycle` that fits this window and
/// changes the tab.
fn step(
    committed: &PaneLayout,
    cycle: &[NamedLayout],
    forward: bool,
    fitted: impl Fn(&Arrangement) -> Fit,
) -> Result<(SharedString, Arrangement), Refusal> {
    let mut short = false;
    let stepped =
        committed.step_layout(cycle, forward, |_, ideal| {
            match fitted(&ideal) {
                Fit::Fits(arrangement) => Some(arrangement),
                Fit::Short(_) => {
                    short = true;
                    None
                }
            }
        });
    match stepped {
        Some((index, arrangement)) => cycle
            .get(index)
            .map(|layout| (arrange_label(layout), arrangement))
            .ok_or(Refusal::NoneChanges),
        None if short => Err(Refusal::NoneFits),
        None => Err(Refusal::NoneChanges),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use huterm_protocol::{PaneInfo, PaneNode, SplitChild, SplitId, SplitNode};

    fn pane(value: u64) -> PaneId {
        PaneId::in_runtime(RuntimeId::new(3), value)
    }

    /// The layout `arrangement` describes, with fresh split identities.
    fn layout_of(arrangement: &Arrangement) -> PaneLayout {
        fn node(arrangement: &Arrangement, next: &mut u64) -> PaneNode {
            match arrangement {
                Arrangement::Pane(id) => PaneNode::Pane(PaneInfo {
                    id: *id,
                    terminal_id: TerminalId::new(id.get()),
                    fallback_name: "sh".into(),
                    custom_name: None,
                }),
                Arrangement::Split { axis, children } => {
                    *next += 1;
                    let id = SplitId::in_runtime(RuntimeId::new(3), *next);
                    PaneNode::Split(SplitNode {
                        id,
                        axis: *axis,
                        children: children
                            .iter()
                            .map(|(weight, child)| SplitChild {
                                weight: *weight,
                                node: node(child, next),
                            })
                            .collect(),
                    })
                }
            }
        }
        PaneLayout::new(node(arrangement, &mut 1000)).unwrap()
    }

    /// `name` over `count` panes.
    fn built_in(name: &str, count: u64) -> PaneLayout {
        let panes: Vec<PaneId> = (1..=count).map(pane).collect();
        layout_of(
            &NamedLayout::built_in(name)
                .unwrap()
                .template
                .expand(&panes)
                .unwrap(),
        )
    }

    fn fits(arrangement: &Arrangement) -> Fit {
        Fit::Fits(arrangement.clone())
    }

    fn short(arrangement: &Arrangement) -> Fit {
        Fit::Short(arrangement.clone())
    }

    /// The defaults plus a `pair` layout of two panes.
    fn with_pair() -> huterm_config::PanesConfig {
        huterm_config::PanesConfig {
            layouts: [("pair".to_owned(), "columns(pane, pane)".to_owned())]
                .into(),
            ..huterm_config::PanesConfig::default()
        }
    }

    #[test]
    fn arranging_names_layouts_in_prose_and_their_limits() {
        let layouts = with_pair();
        let three = built_in("columns", 3);
        assert_eq!(
            arrange(&three, &layouts, "main_left", short)
                .unwrap_err()
                .message(),
            "Window too small for the main left layout"
        );
        assert_eq!(
            arrange(&three, &layouts, "dev", fits)
                .unwrap_err()
                .message(),
            "No pane layout named `dev`"
        );
        assert_eq!(
            arrange(&three, &layouts, "pair", fits)
                .unwrap_err()
                .message(),
            "The pair layout holds at most 2 panes"
        );
        let many = built_in("tiled", 258);
        assert_eq!(
            arrange(&many, &layouts, "columns", fits)
                .unwrap_err()
                .message(),
            "The equal columns layout holds at most 256 panes"
        );
        assert_eq!(
            arrange(&many, &layouts, "main_top", fits)
                .unwrap_err()
                .message(),
            "The main top layout holds at most 257 panes"
        );
        let (label, _) = arrange(&three, &layouts, "columns", fits).unwrap();
        assert_eq!(label, "Arrange Panes: Equal Columns");
        let (label, _) =
            arrange(&built_in("rows", 2), &layouts, "pair", fits).unwrap();
        assert_eq!(label, "Arrange Panes: pair");
    }

    #[test]
    fn a_step_names_why_no_layout_applies() {
        let cycle = huterm_config::PanesConfig::default().cycle();
        let three = built_in("columns", 3);
        assert_eq!(
            step(&three, &cycle, true, short).unwrap_err().message(),
            "No other pane layout fits this window"
        );
        // At 514 panes only tiled is available, and the tab is tiled.
        assert_eq!(
            step(&built_in("tiled", 514), &cycle, false, fits)
                .unwrap_err()
                .message(),
            "No other pane layout in the cycle changes these panes"
        );
        let (label, _) = step(&three, &cycle, true, fits).unwrap();
        assert_eq!(label, "Arrange Panes: Equal Rows");
    }
}
