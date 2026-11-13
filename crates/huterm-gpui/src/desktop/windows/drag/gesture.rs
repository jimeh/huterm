//! The window's side of a drag: a press that starts a [`Gesture`], the
//! capture listeners' motion and release, the one target resolver, spring
//! loading, autoscroll, and the end of a gesture by a drop, a refusal, or a
//! cancellation.

use super::super::pane_geometry::{Zone, drop_zone};
use super::super::tab_strip::LabelZone;
use super::super::workspace_panel::RowAction;
use super::super::*;
use super::{
    Destination, DropTarget, Gesture, Hover, Operation, PANE_DROP_DEPTH,
    Payload, RowKey, Shown, SpringBack, SpringKey, operation, row_slot,
};

/// Which window resolves a target: the gesture's own, or another window
/// previewing it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in super::super) enum Side {
    Source,
    Incoming,
}

impl DesktopRuntime {
    /// Commits a drop's operation under the Mux lock.
    pub(in super::super) fn commit_drop(
        &self,
        change: Operation,
    ) -> Committed<Result<(), MuxError>> {
        match change {
            Operation::ReorderTab {
                workspace,
                tab,
                before,
            } => self.reorder_tab(workspace, tab, before),
            Operation::MoveTab {
                workspace,
                tab,
                to,
                before,
            } => {
                let mut mux = self.lock();
                let result = if self.terminating.load(Ordering::Acquire) {
                    Err(RuntimeError::Stopped.into())
                } else {
                    mux.move_tab(workspace, tab, to, before)
                };
                Committed {
                    result,
                    seq: mux.hierarchy_seq(),
                }
            }
            Operation::Pane(change) => {
                let Committed { result, seq } = self.commit_pane_move(change);
                Committed {
                    result: result.map(|_| ()),
                    seq,
                }
            }
            Operation::MoveWorkspace {
                session,
                workspace,
                to,
                before,
            } => self.move_workspace(session, workspace, to, before),
            Operation::MoveSession { session, before } => {
                self.move_session(session, before)
            }
        }
    }
}

impl WorkspaceView {
    /// Starts tracking a press on tab `tab`'s label; motion past the
    /// threshold makes it a reorder, and a release before that selects it.
    pub(in super::super) fn begin_reorder(
        &mut self,
        tab: TabId,
        pointer: gpui::Point<Pixels>,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(workspace) = self.workspace_id(cx) else {
            return;
        };
        let payload = Payload::Tab { workspace, tab };
        if self.can_reorder(payload, cx) {
            self.start_drag(payload, pointer, false, window, cx);
        }
    }

    /// Starts tracking a press on `pane`'s title bar, or a modifier press
    /// in its terminal, in `tab`; motion past the threshold makes it a drag.
    /// `remapped` marks a macOS Control-left press that arrived as Right.
    pub(in super::super) fn begin_pane_drag(
        &mut self,
        tab: TabId,
        pane: PaneId,
        pointer: gpui::Point<Pixels>,
        remapped: bool,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        // A drag that started from the layout an arrangement is about to
        // replace would aim at stale zones; one already under way goes on.
        if !self.can_drag_panes(cx) || self.arrangements > 0 {
            return;
        }
        let Some(workspace) = self.workspace_id(cx) else {
            return;
        };
        let payload = Payload::Pane {
            workspace,
            tab,
            pane,
        };
        self.start_drag(payload, pointer, remapped, window, cx);
    }

    /// Starts tracking a press on a sidebar list row; it becomes a drag
    /// past the threshold, and a click that shows the row's item otherwise.
    pub(in super::super) fn begin_row_drag(
        &mut self,
        row: RowKey,
        pointer: gpui::Point<Pixels>,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        let payload = match row {
            RowKey::Workspace(workspace) => {
                let Some(session) = self.attached_session(cx) else {
                    return;
                };
                Payload::Workspace { session, workspace }
            }
            RowKey::Session(session) => Payload::Session(session),
        };
        self.start_drag(payload, pointer, false, window, cx);
    }

    fn start_drag(
        &mut self,
        payload: Payload,
        pointer: gpui::Point<Pixels>,
        remapped: bool,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        if payload.in_bar() {
            self.end_cross_drag(true, cx);
            self.scroll_target = None;
        }
        let hover = Hover::new(
            pointer,
            self.shown(cx),
            self.tab_scroll,
            self.tab_strip(window),
        );
        self.drag = Some(Gesture::new(payload, pointer, hover, remapped));
        // A tab or pane press shows the closed hand at once, even over a
        // pane that already had focus.
        cx.notify();
    }

    /// The drag of a tab, from its press on.
    pub(in super::super) fn reordering(&self) -> bool {
        self.drag
            .as_ref()
            .is_some_and(|drag| matches!(drag.payload, Payload::Tab { .. }))
    }

    /// The pane a pane drag past its threshold drags.
    pub(in super::super) fn dragged_pane(&self) -> Option<PaneId> {
        self.drag
            .as_ref()
            .filter(|drag| drag.dragging())
            .and_then(|drag| drag.payload.pane())
    }

    /// A pane drag, from its press on.
    #[cfg(test)]
    pub(in super::super) fn pane_dragging(&self) -> bool {
        self.drag
            .as_ref()
            .is_some_and(|drag| drag.payload.pane().is_some())
    }

    /// What the window shows: its workspace and that workspace's active
    /// tab.
    pub(in super::super) fn shown(&self, cx: &App) -> Shown {
        Shown {
            workspace: self.workspace_id(cx),
            tab: self.active_tab(cx),
        }
    }

    /// Whether the window holds `tab`'s view in `workspace`, shown or
    /// hidden.
    fn holds_tab_in(
        &self,
        workspace: WorkspaceId,
        tab: TabId,
        cx: &App,
    ) -> Option<&TabView> {
        let views = if Some(workspace) == self.workspace_id(cx) {
            Some(&self.tabs)
        } else {
            self.hidden.get(&workspace)
        };
        views?.iter().find(|view| view.id == tab)
    }

    /// Whether a tab or pane gesture comes from another workspace than the
    /// one the window shows, after a row drag sprang it there.
    pub(in super::super) fn drag_away(
        &self,
        payload: Payload,
        cx: &App,
    ) -> bool {
        payload.in_bar() && payload.workspace() != self.workspace_id(cx)
    }

    /// Whether a tab drag may start or go on: the tab is still in its
    /// workspace here, shown or sprung away from, and nothing holds the
    /// window.
    fn can_reorder(&self, payload: Payload, cx: &App) -> bool {
        let Payload::Tab { workspace, tab } = payload else {
            return false;
        };
        self.holds_tab_in(workspace, tab, cx).is_some()
            && cx.global::<Desktop>().hierarchy.state().tab_workspace(tab)
                == Some(workspace)
            && !self.busy
            && self.arrangements == 0
            && !self.overlay_holds_focus()
            && self.close.current.is_none()
            && self.record(cx).is_some_and(|record| !record.closing)
            && self.close.confirmation.is_none()
            && !cx.global::<Desktop>().quitting
            && !cx.global::<Desktop>().quit_pending
    }

    fn can_drag_panes(&self, cx: &App) -> bool {
        let desktop = cx.global::<Desktop>();
        !self.busy
            && !self.overlay_holds_focus()
            && !desktop.quitting
            && !desktop.quit_pending
    }

    /// Whether a pane drag can go on: the window takes drags, the dragged
    /// pane is still in its tab here and in the projection, and the window
    /// shows what the drag expects. A tab drag the window sprang away from
    /// its workspace checks the same; one in its own workspace checks
    /// itself on motion. Other payloads are always valid here; a row drag
    /// checks itself when its list draws.
    pub(in super::super) fn pane_drag_valid(&self, cx: &App) -> bool {
        let Some(drag) = &self.drag else {
            return false;
        };
        match drag.payload {
            Payload::Pane {
                workspace,
                tab,
                pane,
            } => {
                self.can_drag_panes(cx)
                    && drag.expected().matches(self.shown(cx))
                    && self
                        .holds_tab_in(workspace, tab, cx)
                        .is_some_and(|view| view.pane(pane).is_some())
                    && cx.global::<Desktop>().hierarchy.state().pane_tab(pane)
                        == Some(tab)
            }
            payload @ Payload::Tab { .. } if self.drag_away(payload, cx) => {
                self.can_reorder(payload, cx)
                    && drag.expected().matches(self.shown(cx))
            }
            _ => true,
        }
    }

    /// A tab or pane drag the window must check on each frame: a pane
    /// drag, or a tab drag sprang away from its workspace.
    pub(in super::super) fn drag_checks_each_frame(&self, cx: &App) -> bool {
        self.drag.as_ref().is_some_and(|drag| {
            drag.payload.pane().is_some() || self.drag_away(drag.payload, cx)
        })
    }

    /// Follows the pointer from the capture listeners.
    pub(in super::super) fn update_drag(
        &mut self,
        pointer: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(payload) = self.drag.as_ref().map(|drag| drag.payload) else {
            return;
        };
        if payload.row().is_some() {
            let dragging = self.drag.as_mut().is_some_and(|drag| {
                drag.follow(pointer);
                drag.dragging()
            });
            // A workspace row can leave the window, for another one or a
            // window of its own; a session row stays in its list.
            if dragging && matches!(payload, Payload::Workspace { .. }) {
                let outside = self.route_cross_drag(pointer, window, cx);
                if let Some(drag) = &mut self.drag {
                    drag.outside = outside;
                    if outside {
                        drag.hover.leave();
                    }
                }
                if outside {
                    cx.notify();
                    return;
                }
            } else if let Some(drag) = &mut self.drag {
                // A session row stays in its window, but past its edge it
                // must not keep scrolling the list.
                drag.outside = !Bounds::new(
                    point(px(0.0), px(0.0)),
                    window.viewport_size(),
                )
                .contains(&pointer);
            }
            self.retarget_drag(Instant::now(), false, window, cx);
            cx.notify();
            return;
        }
        let valid = match payload {
            Payload::Tab { .. } => {
                self.can_reorder(payload, cx) && self.pane_drag_valid(cx)
            }
            _ => self.pane_drag_valid(cx),
        };
        if !valid {
            self.cancel_drag(window, cx);
            self.resizing_sidebar = false;
            return;
        }
        let strip = self.tab_strip(window);
        let over_bar = self.over_tab_bar(pointer, window);
        let scroll = self.tab_scroll;
        let Some(drag) = &mut self.drag else {
            return;
        };
        let was_scrolling = drag.scrolls(scroll);
        drag.follow(pointer);
        drag.hover.set_strip(strip);
        if payload.pane().is_some() {
            drag.hover.over_bar = over_bar;
        }
        if !drag.dragging() {
            return;
        }
        if !was_scrolling {
            self.last_scroll = Instant::now();
        }
        if self.route_cross_drag(pointer, window, cx) {
            if let Some(drag) = &mut self.drag {
                drag.outside = true;
                drag.hover.leave();
            }
            cx.notify();
            return;
        }
        if let Some(drag) = &mut self.drag {
            drag.outside = false;
        }
        self.retarget_drag(Instant::now(), false, window, cx);
        cx.notify();
    }

    /// Resolves the gesture's target at its pointer and arms spring-loading
    /// for it; `rearm` restarts the delay, as when labels move under a
    /// still pointer.
    fn retarget_drag(
        &mut self,
        now: Instant,
        rearm: bool,
        window: &Window,
        cx: &App,
    ) {
        let Some((payload, pointer)) = self
            .drag
            .as_ref()
            .filter(|drag| drag.dragging())
            .map(|drag| (drag.payload, drag.pointer()))
        else {
            return;
        };
        let target =
            self.resolve_drop(payload, pointer, Side::Source, window, cx);
        let Some(drag) = &mut self.drag else {
            return;
        };
        drag.hover.retarget(target, now, rearm);
        if matches!(payload, Payload::Tab { .. }) {
            // A tab held over a pane to nest it leaves the bar where it is.
            drag.hover.over_bar = !drag.nesting();
        }
    }

    /// Resolves the gesture's target again against the panes a frame just
    /// drew, redrawing only when it changed.
    pub(in super::super) fn retarget_after_frame(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.cross_outside(cx) {
            return;
        }
        let Some(before) = self
            .drag
            .as_ref()
            .filter(|drag| drag.dragging() && drag.payload.in_bar())
            .map(Gesture::target)
        else {
            return;
        };
        self.retarget_drag(Instant::now(), false, window, cx);
        if self.drag.as_ref().map(Gesture::target) != Some(before) {
            cx.notify();
        }
    }

    /// Whether `pointer` is over the drawn tab strip: the bar without the
    /// window buttons or title-row controls at either end.
    pub(in super::super) fn over_tab_bar(
        &self,
        pointer: gpui::Point<Pixels>,
        window: &Window,
    ) -> bool {
        self.bar_drawn()
            && self
                .chrome_layout(window)
                .strip_bounds(self.layout_tabs())
                .contains(&pointer)
    }

    /// The target a drag of `payload` has at `pointer`, from the last
    /// drawn frame, in this window as the gesture's `side`.
    pub(in super::super) fn resolve_drop(
        &self,
        payload: Payload,
        pointer: gpui::Point<Pixels>,
        side: Side,
        window: &Window,
        cx: &App,
    ) -> Option<DropTarget> {
        match payload {
            Payload::Workspace { .. } if side == Side::Incoming => {
                self.resolve_incoming_workspace(payload, pointer, window, cx)
            }
            Payload::Workspace { .. } | Payload::Session(_) => {
                self.resolve_row(payload, pointer, cx)
            }
            Payload::Tab { .. } | Payload::Pane { .. } => {
                if let Some(row) =
                    self.resolve_workspace_row(payload, pointer, window, cx)
                {
                    return Some(row);
                }
                // A gesture the window sprang away from its workspace
                // resolves as another window's would: no reorder, no swap,
                // no lone-pane rule.
                let away = self.drag_away(payload, cx);
                match payload {
                    Payload::Tab { tab, .. }
                        if side == Side::Source && !away =>
                    {
                        self.resolve_reorder(payload, tab, pointer, window, cx)
                    }
                    _ => {
                        let rules = if away { Side::Incoming } else { side };
                        self.resolve_bar(
                            payload, pointer, side, rules, window, cx,
                        )
                    }
                }
            }
        }
    }

    /// The bounds of the workspace list's visible part, when the sidebar or
    /// the switcher's popover draws it.
    fn workspace_list_at(
        &self,
        pointer: gpui::Point<Pixels>,
        window: &Window,
    ) -> bool {
        self.rows_drawn(&self.chrome_layout(window))
            && self.workspace_drag_scroll().bounds().contains(&pointer)
    }

    /// A workspace row under a tab or pane: one of another workspace than
    /// the dragged item's, which spring-loads unless the window shows it.
    fn resolve_workspace_row(
        &self,
        payload: Payload,
        pointer: gpui::Point<Pixels>,
        window: &Window,
        cx: &App,
    ) -> Option<DropTarget> {
        if !self.workspace_list_at(pointer, window) {
            return None;
        }
        let painted = self.workspace_row_bounds.borrow();
        let workspace =
            self.session_workspaces(cx).into_iter().find(|row| {
                painted
                    .get(row)
                    .is_some_and(|bounds| bounds.contains(&pointer))
            })?;
        (payload.workspace() != Some(workspace)).then(|| {
            DropTarget::Workspace {
                workspace,
                spring: self.workspace_id(cx) != Some(workspace),
            }
        })
    }

    /// Another window's workspace over this one: a slot of the workspace
    /// list where it draws, else the window's content.
    fn resolve_incoming_workspace(
        &self,
        payload: Payload,
        pointer: gpui::Point<Pixels>,
        window: &Window,
        cx: &App,
    ) -> Option<DropTarget> {
        let row = payload.row()?;
        if !self.workspace_list_at(pointer, window) {
            // A window on the workspace's own session takes it only in its
            // list, as a reorder.
            let Payload::Workspace { session, .. } = payload else {
                return None;
            };
            return (self.attached_session(cx) != Some(session))
                .then_some(DropTarget::Window);
        }
        let order: Vec<RowKey> = self
            .session_workspaces(cx)
            .into_iter()
            .map(RowKey::Workspace)
            .collect();
        let painted = self.workspace_row_bounds.borrow();
        Some(row_slot(row, pointer, &order, |item| match item {
            RowKey::Workspace(workspace) => painted.get(&workspace).copied(),
            RowKey::Session(_) => None,
        }))
    }

    /// A row's slot in its list, from the rows' painted midpoints.
    fn resolve_row(
        &self,
        payload: Payload,
        pointer: gpui::Point<Pixels>,
        cx: &App,
    ) -> Option<DropTarget> {
        let row = payload.row()?;
        let order = self.row_order(payload, cx);
        let workspaces = self.workspace_row_bounds.borrow();
        let sessions = self.session_row_bounds.borrow();
        Some(row_slot(row, pointer, &order, |item| match item {
            RowKey::Workspace(workspace) => workspaces.get(&workspace).copied(),
            RowKey::Session(session) => sessions.get(&session).copied(),
        }))
    }

    /// The list a row payload's row sits in, in order.
    fn row_order(&self, payload: Payload, cx: &App) -> Vec<RowKey> {
        match payload {
            Payload::Workspace { .. } => self
                .workspace_order(cx)
                .into_iter()
                .map(RowKey::Workspace)
                .collect(),
            Payload::Session(_) => self
                .session_rows(cx)
                .iter()
                .map(|row| RowKey::Session(row.session))
                .collect(),
            Payload::Tab { .. } | Payload::Pane { .. } => Vec::new(),
        }
    }

    /// A tab drag in its own window: in pane mode, over a pane of the
    /// active tab, which is not the dragged tab, at least
    /// [`PANE_DROP_DEPTH`] past the bar's inner edge, an edge nests the tab
    /// and a centre offers nothing; anywhere else the drag projects onto
    /// the bar's slots.
    fn resolve_reorder(
        &self,
        payload: Payload,
        tab: TabId,
        pointer: gpui::Point<Pixels>,
        window: &Window,
        cx: &App,
    ) -> Option<DropTarget> {
        let pane_mode = || {
            let active = self.active_tab(cx)?;
            if tab == active || !self.bar_drawn() {
                return None;
            }
            let (drawn, rect) = self.drawn_pane_at(pointer)?;
            if drawn != active
                || self.depth_past_bar(pointer, window) < PANE_DROP_DEPTH
            {
                return None;
            }
            let Some(Zone::Edge(side)) = drop_zone(rect.frame, pointer) else {
                return Some(None);
            };
            Some(self.refuse_small(
                payload,
                DropTarget::Pane {
                    tab: drawn,
                    pane: rect.pane,
                    frame: rect.frame,
                    zone: Zone::Edge(side),
                    refused: false,
                },
                Side::Source,
                window,
                cx,
            ))
        };
        pane_mode().unwrap_or_else(|| {
            Some(DropTarget::Slot(self.tab_strip(window).slot(pointer)))
        })
    }

    /// A tab or pane drag over the title row, the tab bar, or a pane of
    /// the active tab. A pane alone in its tab offers no slot or title
    /// row in its own window, and only the dragged pane's own tab offers a
    /// centre. `rules` is the side whose rules apply: a gesture sprang
    /// away from its workspace follows another window's.
    fn resolve_bar(
        &self,
        payload: Payload,
        pointer: gpui::Point<Pixels>,
        side: Side,
        rules: Side,
        window: &Window,
        cx: &App,
    ) -> Option<DropTarget> {
        let source = rules == Side::Source;
        let lone = source
            && payload.pane().is_some()
            && payload.tab().is_none_or(|tab| self.pane_count(tab) < 2);
        if !self.bar_drawn()
            && self.presentation() == Presentation::Hidden
            && self.title_row() > px(0.0)
            && self
                .title_row_layout(&self.chrome_layout(window))
                .drop_region
                .contains(&pointer)
        {
            return (!lone).then_some(DropTarget::TitleRow);
        }
        if self.over_tab_bar(pointer, window) {
            let active = self.active_tab(cx).and_then(|active| {
                self.tabs.iter().position(|tab| tab.id == active)
            });
            return match self.tab_strip(window).label_zone(pointer, active)? {
                LabelZone::Spring(index) => Some(DropTarget::Spring(
                    SpringKey::Tab(self.tabs.get(index)?.id),
                )),
                LabelZone::Slot(index) => {
                    (!lone).then_some(DropTarget::Slot(index))
                }
            };
        }
        let (tab, rect) = self.drawn_pane_at(pointer)?;
        // Until the next frame after a spring load, the drawn panes are the
        // previous tab's.
        if self.active_tab(cx) != Some(tab)
            || (source && payload.pane() == Some(rect.pane))
        {
            return None;
        }
        let zone = drop_zone(rect.frame, pointer)?;
        // Another window takes only edges.
        if zone == Zone::Centre && !source {
            return None;
        }
        self.refuse_small(
            payload,
            DropTarget::Pane {
                tab,
                pane: rect.pane,
                frame: rect.frame,
                zone,
                refused: false,
            },
            side,
            window,
            cx,
        )
    }

    /// `target` when it takes a drop of `payload`, marked refused when the
    /// result would leave a pane under the minimum size here.
    fn refuse_small(
        &self,
        payload: Payload,
        target: DropTarget,
        side: Side,
        window: &Window,
        cx: &App,
    ) -> Option<DropTarget> {
        let change = self.drop_operation(payload, target, side, cx)?;
        let refused = match change {
            Operation::Pane(change) => {
                self.check_move_fits(change, window, cx).is_err()
            }
            _ => false,
        };
        match target {
            DropTarget::Pane {
                tab,
                pane,
                frame,
                zone,
                ..
            } => Some(DropTarget::Pane {
                tab,
                pane,
                frame,
                zone,
                refused,
            }),
            target => Some(target),
        }
    }

    /// The operation a drop of `payload` on `target` in this window
    /// commits, or `None` when it takes no drop.
    pub(in super::super) fn drop_operation(
        &self,
        payload: Payload,
        target: DropTarget,
        side: Side,
        cx: &App,
    ) -> Option<Operation> {
        let tabs: Vec<TabId> = self.tabs.iter().map(|tab| tab.id).collect();
        let rows = match payload {
            // Another window's workspace slots into this window's list.
            Payload::Workspace { .. } if side == Side::Incoming => self
                .session_workspaces(cx)
                .into_iter()
                .map(RowKey::Workspace)
                .collect(),
            _ => self.row_order(payload, cx),
        };
        operation(
            payload,
            target,
            &Destination {
                workspace: self.workspace_id(cx),
                session: self.attached_session(cx),
                cross: side == Side::Incoming,
                tabs: &tabs,
                rows: &rows,
            },
        )
    }

    pub(in super::super) fn pane_count(&self, tab: TabId) -> usize {
        self.tabs
            .iter()
            .find(|view| view.id == tab)
            .map_or(0, |view| view.panes.len())
    }

    /// The drawn pane of the active tab under `pointer`, with its tab.
    pub(in super::super) fn drawn_pane_at(
        &self,
        pointer: gpui::Point<Pixels>,
    ) -> Option<(TabId, PaneRect)> {
        let drawn = self.drawn_panes.borrow();
        let (tab, rects) = drawn.as_ref()?;
        rects
            .iter()
            .find(|rect| rect.frame.contains(&pointer))
            .map(|rect| (*tab, *rect))
    }

    /// The drawn frame of `pane`, when the active tab holds it.
    pub(in super::super) fn drawn_frame(
        &self,
        pane: PaneId,
    ) -> Option<Bounds<Pixels>> {
        let drawn = self.drawn_panes.borrow();
        let (_, rects) = drawn.as_ref()?;
        rects
            .iter()
            .find(|rect| rect.pane == pane)
            .map(|rect| rect.frame)
    }

    /// How far `pointer` is past the tab bar's inner edge, toward the
    /// terminal, perpendicular to the bar.
    fn depth_past_bar(
        &self,
        pointer: gpui::Point<Pixels>,
        window: &Window,
    ) -> Pixels {
        let bar = self.chrome_layout(window).tabs;
        match self.layout_tabs().position {
            TabPosition::Top | TabPosition::Titlebar => {
                pointer.y - bar.bottom()
            }
            TabPosition::Bottom => bar.top() - pointer.y,
            TabPosition::Left => pointer.x - bar.right(),
            TabPosition::Right => bar.left() - pointer.x,
        }
    }

    /// Ends the gesture at `pointer` from the capture listeners: a drop
    /// commits its operation, a release before the threshold clicks, and a
    /// release without a target ends it as a cancellation does.
    pub(in super::super) fn finish_drag(
        &mut self,
        pointer: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.update_drag(pointer, window, cx);
        if self.finish_cross_drag(window, cx) {
            if let Some(drag) = self.take_drag(cx)
                && !self.busy
            {
                self.end_without_drop(&drag, window, cx);
            }
            cx.notify();
            return;
        }
        let Some(drag) = self.take_drag(cx) else {
            return;
        };
        cx.notify();
        let payload = drag.payload;
        if let Payload::Tab { .. } = payload
            && !self.can_reorder(payload, cx)
        {
            self.restore_tab_focus(window, cx);
            return;
        }
        if !drag.dragging() {
            self.click(payload, window, cx);
            return;
        }
        if let Payload::Workspace { session, .. } = payload
            && self.attached_session(cx) != Some(session)
        {
            return;
        }
        let change = drag.target().and_then(|target| {
            self.drop_operation(payload, target, Side::Source, cx)
        });
        let Some(change) = change else {
            self.end_without_drop(&drag, window, cx);
            return;
        };
        let focus = match payload {
            Payload::Pane { pane, .. } => Some(pane),
            // A nested tab keeps the pane it focused.
            Payload::Tab { tab, .. } => self
                .all_tab_views()
                .find(|view| view.id == tab)
                .map(|view| view.focused().pane),
            Payload::Workspace { .. } | Payload::Session(_) => None,
        };
        self.commit_drop(change, focus, drag.back(), window, cx);
    }

    /// A press released before the threshold: a tab label selects its
    /// tab, a row shows its item, and a pane title only focused its pane.
    fn click(
        &mut self,
        payload: Payload,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match payload {
            Payload::Tab { tab, .. } => self.select(tab, window, cx),
            Payload::Pane { .. } => {}
            Payload::Workspace { workspace, .. } => {
                self.run_workspace_row(workspace, RowAction::Show, window, cx);
            }
            Payload::Session(session) => {
                self.run_session_row(session, RowAction::Show, window, cx);
            }
        }
    }

    /// Commits a local drop on a worker. A pane move and a nested tab
    /// focus `focus` afterwards and switch `back` when refused; a tab
    /// reorder restores focus; row moves only report failure.
    fn commit_drop(
        &mut self,
        change: Operation,
        focus: Option<PaneId>,
        back: Option<SpringBack>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        match change {
            Operation::Pane(change) => {
                self.commit_pane_move(change, focus, back, window, cx);
            }
            Operation::ReorderTab { .. } | Operation::MoveTab { .. } => {
                self.commit_tab_move(change, back, window, cx);
            }
            Operation::MoveWorkspace {
                session,
                workspace,
                before,
                ..
            } => {
                self.move_workspace(
                    session, workspace, session, before, window, cx,
                );
            }
            Operation::MoveSession { session, before } => {
                Self::move_session(session, before, cx);
            }
        }
    }

    /// Commits a tab reorder or move on a worker; a refused move that
    /// spring-loaded switches `back`.
    pub(in super::super) fn commit_tab_move(
        &mut self,
        change: Operation,
        back: Option<SpringBack>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        self.busy = true;
        let task = cx.background_executor().spawn(async move {
            #[cfg(all(test, feature = "test-support"))]
            if runtime.worker_hooks.enter(worker_hooks::Stage::Move).await {
                return Committed {
                    result: Err(RuntimeError::Stopped.into()),
                    seq: runtime.lock().hierarchy_seq(),
                };
            }
            runtime.commit_drop(change)
        });
        let app = cx.to_async();
        let (title, verb) = match change {
            Operation::MoveTab { .. } => ("Move tab", "move"),
            _ => ("Reorder tab", "reorder"),
        };
        cx.spawn_in(window, async move |view, cx| {
            let Committed { result, seq } = task.await;
            // Reconcile applies the committed order while draining; this
            // completion only restores focus and reports failure.
            projected(&app, seq).await;
            let _ = view.update_in(cx, |view, window, cx| {
                view.busy = false;
                let moved = result.is_ok();
                if let Err(error) = result
                    && !terminating(cx)
                {
                    view.report_failure(
                        title,
                        format!("Cannot {verb} tab: {error}"),
                        cx,
                    );
                }
                view.reconcile_own(window, cx);
                if moved {
                    view.settle_moved(change, window, cx);
                } else if let Some(back) = back {
                    view.switch_back(back, window, cx);
                }
                view.restore_tab_focus(window, cx);
                view.resume_close(window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// After a committed move between this window's workspaces: a tab that
    /// landed in the shown workspace shows, and a hidden workspace the move
    /// emptied closes unless pinned, as an exit-close's does.
    pub(in super::super) fn settle_moved(
        &mut self,
        change: Operation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Operation::MoveTab { tab, to, .. } = change
            && Some(to) == self.workspace_id(cx)
            && self.tabs.iter().any(|view| view.id == tab)
        {
            self.select(tab, window, cx);
        }
        if let Some(source) = change.source_workspace()
            && Some(source) != self.workspace_id(cx)
        {
            self.close_emptied_hidden(source, cx);
        }
    }

    fn take_drag(&mut self, cx: &mut Context<'_, Self>) -> Option<Gesture> {
        self.end_cross_drag(true, cx);
        let drag = self.drag.take();
        if drag.is_some() {
            cx.notify();
        }
        drag
    }

    /// Cancels the gesture: nothing moves, the bar scroll a tab or pane
    /// drag began with comes back, and a spring-loaded tab switches back.
    /// `true` when one was under way.
    pub(in super::super) fn cancel_drag(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Some(drag) = self.take_drag(cx) else {
            return false;
        };
        self.end_without_drop(&drag, window, cx);
        true
    }

    /// Drops a row drag, as when its list closes or its row goes; nothing
    /// moves, and a workspace row that left the window ends its
    /// cross-window record too.
    pub(in super::super) fn drop_row_drag(
        &mut self,
        row: impl FnOnce(Payload) -> bool,
        cx: &mut Context<'_, Self>,
    ) {
        if self.drag.as_ref().is_some_and(|drag| row(drag.payload)) {
            self.drag = None;
            self.end_cross_drag(true, cx);
        }
    }

    fn end_without_drop(
        &mut self,
        drag: &Gesture,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !drag.payload.in_bar() {
            return;
        }
        self.tab_scroll = drag.hover.scroll;
        if let Some(back) = drag.back() {
            self.switch_back(back, window, cx);
        }
        if !self.overlay_holds_focus() {
            self.restore_tab_focus(window, cx);
        }
        cx.notify();
    }

    /// Reactivates the tab a drag started over when the tab it
    /// spring-loaded is still active, without taking focus from an
    /// overlay. A tab switch the drag did not make leaves the window where
    /// it is.
    pub(in super::super) fn switch_back(
        &mut self,
        back: SpringBack,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let focus = !self.overlay_holds_focus();
        self.spring_back(back, focus, window, cx);
    }

    /// Shows `back.origin` again while the window still shows
    /// `back.sprung`: its workspace, when the drag sprang a row and the
    /// window may switch, then its tab.
    pub(in super::super) fn spring_back(
        &mut self,
        back: SpringBack,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !back.sprung.matches(self.shown(cx)) {
            return;
        }
        if back.origin.workspace != self.workspace_id(cx) {
            let Some(origin) = back.origin.workspace else {
                return;
            };
            if !self.spring_workspace(origin, focus, window, cx) {
                return;
            }
        }
        if let Some(tab) = back.origin.tab
            && self.active_tab(cx) != Some(tab)
            && self.tabs.iter().any(|view| view.id == tab)
        {
            self.select_tab(tab, focus, window, cx);
        }
    }

    /// Whether spring-loading may activate `key` now: a tab always, a
    /// workspace of the attached session while the window could switch to
    /// it.
    fn can_spring(&self, key: SpringKey, cx: &App) -> bool {
        match key {
            SpringKey::Tab(_) => true,
            SpringKey::Workspace(workspace) => {
                self.check_switch_guard(cx).is_ok()
                    && self.attached_session(cx).is_some_and(|session| {
                        cx.global::<Desktop>()
                            .hierarchy
                            .state()
                            .workspace_session(workspace)
                            == Some(session)
                    })
            }
        }
    }

    /// Shows `workspace` of the attached session under a drag, which goes
    /// on, unlike a workspace switch, moving keyboard focus only when
    /// `focus`. Returns whether it shows.
    fn spring_workspace(
        &mut self,
        workspace: WorkspaceId,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if !self.can_spring(SpringKey::Workspace(workspace), cx) {
            return false;
        }
        let Some(session) = self.attached_session(cx) else {
            return false;
        };
        self.show_workspace_focused(
            session, workspace, false, focus, window, cx,
        );
        self.workspace_id(cx) == Some(workspace)
    }

    /// What the window shows once spring-loading activates `key`, as far
    /// as known before it does: a workspace's tab is not.
    fn springs_to(&self, key: SpringKey, cx: &App) -> Shown {
        match key {
            SpringKey::Tab(tab) => Shown {
                workspace: self.workspace_id(cx),
                tab: Some(tab),
            },
            SpringKey::Workspace(workspace) => Shown {
                workspace: Some(workspace),
                tab: None,
            },
        }
    }

    /// Drops a pane drag that a tab switch it did not make interrupted;
    /// nothing switches back.
    pub(in super::super) fn interrupt_pane_drag(
        &mut self,
        selected: TabId,
        cx: &mut Context<'_, Self>,
    ) {
        let now = Shown {
            workspace: self.workspace_id(cx),
            tab: Some(selected),
        };
        if self.drag.as_ref().is_some_and(|drag| {
            (drag.payload.pane().is_some() || self.drag_away(drag.payload, cx))
                && !drag.expected().matches(now)
        }) {
            self.take_drag(cx);
        }
    }

    /// Activates what a spring load came due on: a tab, with keyboard focus
    /// when `focus`, or a workspace row's workspace. `hover` records what
    /// the window will show first, so the switch is the drag's own, then
    /// what it shows once it did.
    pub(in super::super) fn spring_load(
        &mut self,
        key: SpringKey,
        focus: bool,
        hover: fn(&mut Self) -> Option<&mut Hover>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.can_spring(key, cx) {
            if let Some(hover) = hover(self) {
                hover.spring = super::SpringLoad::default();
            }
            return;
        }
        let springs_to = self.springs_to(key, cx);
        if let Some(hover) = hover(self) {
            hover.sprang(springs_to);
        }
        match key {
            SpringKey::Tab(tab) => self.select_tab(tab, focus, window, cx),
            SpringKey::Workspace(workspace) => {
                self.spring_workspace(workspace, focus, window, cx);
            }
        }
        let shown = self.shown(cx);
        if let Some(hover) = hover(self) {
            hover.sprang(shown);
        }
    }

    /// Fires a spring load whose deadline has come. The pointer is resolved
    /// against the window as drawn now: still over the armed target, it
    /// activates; over another, the delay restarts for it; anywhere else,
    /// spring-loading disarms.
    pub(in super::super) fn advance_spring(
        &mut self,
        now: Instant,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(armed) = self
            .drag
            .as_ref()
            .and_then(|drag| drag.hover.spring.due(now))
        else {
            return;
        };
        if !self.pane_drag_valid(cx) {
            self.cancel_drag(window, cx);
            return;
        }
        self.retarget_drag(now, false, window, cx);
        let Some(drag) = &mut self.drag else {
            return;
        };
        if drag.target().and_then(DropTarget::spring) != Some(armed) {
            // `retarget` re-armed for another target or disarmed.
            cx.notify();
            return;
        }
        let focus = !self.overlay_holds_focus();
        self.spring_load(
            armed,
            focus,
            |view| view.drag.as_mut().map(|drag| &mut drag.hover),
            window,
            cx,
        );
        self.retarget_drag(now, false, window, cx);
        cx.notify();
    }

    /// Restarts spring-loading after autoscroll moved the labels under a
    /// still pointer, so a scrolling bar never springs.
    pub(in super::super) fn rearm_after_scroll(
        &mut self,
        now: Instant,
        window: &Window,
        cx: &mut App,
    ) {
        self.retarget_incoming(now, true, window, cx);
        if self
            .drag
            .as_ref()
            .is_some_and(|drag| drag.dragging() && drag.payload.in_bar())
            && !self.cross_outside(cx)
        {
            self.retarget_drag(now, true, window, cx);
        }
    }

    /// The list a dragged row scrolls: the workspace switcher's or the
    /// sidebar's, or the session list.
    fn drag_list_scroll(&self) -> Option<gpui::ScrollHandle> {
        match self.drag.as_ref()?.payload {
            Payload::Workspace { .. } => Some(self.workspace_drag_scroll()),
            Payload::Session(_) => Some(self.session_list_scroll.clone()),
            // A tab or pane reaching for a row scrolls the workspace list
            // it is over, while one draws.
            Payload::Tab { .. } | Payload::Pane { .. } => {
                let chrome = self.workspace_chrome_now.get();
                (chrome.docked() || self.switcher_mode.is_some())
                    .then(|| self.workspace_drag_scroll())
            }
        }
    }

    /// Whether a dragged row rests at its list's edge, so frames keep
    /// scrolling it.
    pub(in super::super) fn drag_list_scrolls(&self) -> bool {
        self.drag_list_scroll().is_some_and(|scroll| {
            self.drag
                .as_ref()
                .is_some_and(|drag| drag.list_scrolls(&scroll))
        })
    }

    /// Scrolls the list under a dragged row resting at its edge by the time
    /// since the last frame, and moves the drop target with the rows.
    /// Returns whether it scrolled.
    pub(in super::super) fn advance_drag_list(
        &mut self,
        now: Instant,
        window: &Window,
        cx: &App,
    ) -> bool {
        let Some(scroll) = self.drag_list_scroll() else {
            return false;
        };
        let scrolled = self
            .drag
            .as_mut()
            .is_some_and(|drag| drag.advance_list(now, &scroll));
        if scrolled {
            self.retarget_drag(now, false, window, cx);
        }
        scrolled
    }
}
