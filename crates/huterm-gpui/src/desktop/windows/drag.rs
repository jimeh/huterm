//! One drag model for tabs, panes, and sidebar rows. A [`Gesture`] carries
//! what it drags as a [`Payload`]; the window resolves the [`DropTarget`]
//! under the pointer (`WorkspaceView::resolve_drop`), and [`operation`]
//! maps the pair to one Mux operation, or to none, which refuses the drop.
//! The press threshold, spring-loading, and edge autoscroll are shared by
//! every payload. The window's side lives in `drag/gesture.rs`, drawing in
//! `drag/overlay.rs`, and cross-window previews and transfers in
//! `cross_window`.

// User input picks positions here, so a stale index must not panic.
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use huterm_core::{Beside, MoveDestination, PaneRef};

use super::pane_geometry::Zone;
use super::pane_moves::PaneMove;
use super::*;

mod gesture;
mod overlay;

pub(super) use gesture::Side;
pub(super) use overlay::{RowList, drop_label};

/// How far a press moves before it becomes a drag.
pub(super) const DRAG_THRESHOLD: f64 = 4.0;

/// How long a drag rests on a spring-loading target before it activates.
pub(super) const SPRING_LOAD_DELAY: Duration = Duration::from_millis(500);

/// How far past the tab bar's inner edge a tab drag must reach over a pane,
/// perpendicular to the bar, before a release nests the tab instead of
/// reordering it.
pub(super) const PANE_DROP_DEPTH: Pixels = px(32.0);

/// A dragged row this close to its list's edge scrolls the list, at this
/// speed in points per second, every frame while it rests there.
const LIST_SCROLL_EDGE: Pixels = px(16.0);
const LIST_SCROLL_SPEED: f32 = 480.0;

/// What a gesture drags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Payload {
    /// A tab, from its label in the tab bar.
    Tab { workspace: WorkspaceId, tab: TabId },
    /// A pane, from its title bar or the modifier grab.
    Pane {
        workspace: WorkspaceId,
        tab: TabId,
        pane: PaneId,
    },
    /// A row of `session`'s workspace list.
    Workspace {
        session: SessionId,
        workspace: WorkspaceId,
    },
    /// A row of the session list.
    Session(SessionId),
}

impl Payload {
    /// The tab a tab or pane payload comes from.
    pub(super) fn tab(self) -> Option<TabId> {
        match self {
            Self::Tab { tab, .. } | Self::Pane { tab, .. } => Some(tab),
            Self::Workspace { .. } | Self::Session(_) => None,
        }
    }

    /// The workspace a tab, pane, or workspace payload comes from.
    pub(super) fn workspace(self) -> Option<WorkspaceId> {
        match self {
            Self::Tab { workspace, .. }
            | Self::Pane { workspace, .. }
            | Self::Workspace { workspace, .. } => Some(workspace),
            Self::Session(_) => None,
        }
    }

    pub(super) fn pane(self) -> Option<PaneId> {
        match self {
            Self::Pane { pane, .. } => Some(pane),
            _ => None,
        }
    }

    /// The row a row payload drags.
    pub(super) fn row(self) -> Option<RowKey> {
        match self {
            Self::Workspace { workspace, .. } => {
                Some(RowKey::Workspace(workspace))
            }
            Self::Session(session) => Some(RowKey::Session(session)),
            Self::Tab { .. } | Self::Pane { .. } => None,
        }
    }

    /// A tab or pane, which the closed hand follows from the press on and
    /// which can leave the window.
    pub(super) fn in_bar(self) -> bool {
        matches!(self, Self::Tab { .. } | Self::Pane { .. })
    }
}

/// A sidebar list row.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum RowKey {
    Workspace(WorkspaceId),
    Session(SessionId),
}

/// What spring-loading activates once a drag rests on it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum SpringKey {
    Tab(TabId),
    /// A sidebar row's workspace, which the window then shows.
    Workspace(WorkspaceId),
}

/// What a window shows: its workspace, and that workspace's active tab.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Shown {
    pub(super) workspace: Option<WorkspaceId>,
    pub(super) tab: Option<TabId>,
}

impl Shown {
    /// Whether the window showing `now` shows this: the same workspace,
    /// and the same tab unless this names none, as while a sprung
    /// workspace has not picked its tab yet.
    pub(super) fn matches(self, now: Self) -> bool {
        self.workspace == now.workspace
            && self.tab.is_none_or(|tab| now.tab == Some(tab))
    }
}

/// Where a release would land.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum DropTarget {
    /// A zone of a pane in the active tab. A refused zone, whose result
    /// would leave a pane under the minimum size, takes no drop.
    Pane {
        tab: TabId,
        pane: PaneId,
        frame: Bounds<Pixels>,
        zone: Zone,
        refused: bool,
    },
    /// The tab bar insertion slot before tab `index`, or past the last.
    Slot(usize),
    /// Empty space in the title row while the tab bar is hidden, where a
    /// drop opens a new tab at the end of the workspace.
    TitleRow,
    /// A target that spring-loads, such as an inactive tab's label middle.
    Spring(SpringKey),
    /// The sidebar list slot before a row, or after the last.
    RowSlot(Option<RowKey>),
    /// A sidebar workspace row under a tab or pane, which a drop moves
    /// into; `spring` when the window does not show it, so resting there
    /// shows it.
    Workspace {
        workspace: WorkspaceId,
        spring: bool,
    },
    /// Another window's content under a dragged workspace, which a drop
    /// moves into that window's session, last.
    Window,
}

impl DropTarget {
    /// The key a drag resting here spring-loads.
    pub(super) fn spring(self) -> Option<SpringKey> {
        match self {
            Self::Spring(key) => Some(key),
            Self::Workspace {
                workspace,
                spring: true,
            } => Some(SpringKey::Workspace(workspace)),
            _ => None,
        }
    }
}

/// The drag's target and the order it is resolved against: the window
/// holding the target, its workspace's tabs, and the dragged row's list.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Destination<'a> {
    pub(super) workspace: Option<WorkspaceId>,
    /// The session the target window attaches to.
    pub(super) session: Option<SessionId>,
    /// The target is in another window than the gesture's source.
    pub(super) cross: bool,
    pub(super) tabs: &'a [TabId],
    pub(super) rows: &'a [RowKey],
}

/// One Mux operation a drop commits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Operation {
    /// Moves a tab within its workspace.
    ReorderTab {
        workspace: WorkspaceId,
        tab: TabId,
        before: Option<TabId>,
    },
    /// Moves a tab into another window's workspace.
    MoveTab {
        workspace: WorkspaceId,
        tab: TabId,
        to: WorkspaceId,
        before: Option<TabId>,
    },
    /// Swaps or moves a pane, or nests a tab beside a pane.
    Pane(PaneMove),
    /// Moves a workspace within its session, or into session `to`.
    MoveWorkspace {
        session: SessionId,
        workspace: WorkspaceId,
        to: SessionId,
        before: Option<WorkspaceId>,
    },
    MoveSession {
        session: SessionId,
        before: Option<SessionId>,
    },
}

impl Operation {
    /// The tab the operation moves, or moves a pane out of.
    pub(super) fn source_tab(self) -> Option<TabId> {
        match self {
            Self::ReorderTab { tab, .. }
            | Self::MoveTab { tab, .. }
            | Self::Pane(
                PaneMove::TabInto { tab, .. } | PaneMove::Swap { tab, .. },
            ) => Some(tab),
            Self::Pane(PaneMove::Move { from, .. }) => Some(from.tab),
            Self::MoveWorkspace { .. } | Self::MoveSession { .. } => None,
        }
    }

    /// The workspace a tab or pane leaves, when it moves into another.
    pub(super) fn source_workspace(self) -> Option<WorkspaceId> {
        match self {
            Self::MoveTab { workspace, to, .. } => {
                (workspace != to).then_some(workspace)
            }
            Self::Pane(PaneMove::Move {
                from,
                to: MoveDestination::NewTab { workspace, .. },
            }) => (from.workspace != workspace).then_some(from.workspace),
            Self::Pane(PaneMove::Move {
                from,
                to: MoveDestination::Beside(beside),
            }) => (from.workspace != beside.target.workspace)
                .then_some(from.workspace),
            Self::Pane(PaneMove::TabInto { workspace, to, .. }) => {
                (workspace != to.target.workspace).then_some(workspace)
            }
            Self::ReorderTab { .. }
            | Self::Pane(PaneMove::Swap { .. })
            | Self::MoveWorkspace { .. }
            | Self::MoveSession { .. } => None,
        }
    }
}

/// The operation a drop of `payload` on `target` commits, or `None` when
/// the pair takes no drop: a refused zone, a spring-loading target, a
/// centre outside the dragged pane's own tab, the row of the dragged tab's
/// or pane's own workspace, or a row dropped where it already sits.
pub(super) fn operation(
    payload: Payload,
    target: DropTarget,
    to: &Destination<'_>,
) -> Option<Operation> {
    if matches!(
        target,
        DropTarget::Pane { refused: true, .. } | DropTarget::Spring(_)
    ) {
        return None;
    }
    match payload {
        Payload::Pane {
            workspace,
            tab,
            pane,
        } => pane_operation(
            PaneRef {
                workspace,
                tab,
                pane,
            },
            target,
            to,
        ),
        Payload::Tab { workspace, tab } => {
            tab_operation(workspace, tab, target, to)
        }
        Payload::Workspace { session, workspace } => {
            workspace_operation(session, workspace, target, to)
        }
        Payload::Session(session) => {
            let DropTarget::RowSlot(before) = target else {
                return None;
            };
            let before = match before {
                Some(RowKey::Session(before)) => Some(before),
                Some(RowKey::Workspace(_)) => return None,
                None => None,
            };
            moves(
                to.rows,
                RowKey::Session(session),
                before.map(RowKey::Session),
            )
            .then_some(Operation::MoveSession { session, before })
        }
    }
}

/// A workspace row dropped on `target`: a slot of its own session's list
/// reorders it, and a slot or the content of a window attached to
/// another session moves it into that session.
fn workspace_operation(
    session: SessionId,
    workspace: WorkspaceId,
    target: DropTarget,
    to: &Destination<'_>,
) -> Option<Operation> {
    let before = match target {
        DropTarget::RowSlot(Some(RowKey::Workspace(before))) => Some(before),
        DropTarget::RowSlot(None) | DropTarget::Window => None,
        _ => return None,
    };
    let into = to.session.filter(|into| to.cross && *into != session);
    if let Some(into) = into {
        return Some(Operation::MoveWorkspace {
            session,
            workspace,
            to: into,
            before,
        });
    }
    (target != DropTarget::Window
        && moves(
            to.rows,
            RowKey::Workspace(workspace),
            before.map(RowKey::Workspace),
        ))
    .then_some(Operation::MoveWorkspace {
        session,
        workspace,
        to: session,
        before,
    })
}

/// A pane `from` dropped on `target`: a centre swaps within its own tab
/// here, an edge moves it beside the pane, and a slot, the title row, or
/// another workspace's row opens a new tab.
fn pane_operation(
    from: PaneRef,
    target: DropTarget,
    to: &Destination<'_>,
) -> Option<Operation> {
    let change = match target {
        DropTarget::Pane {
            tab,
            pane: other,
            zone: Zone::Centre,
            ..
        } => {
            if to.cross
                || tab != from.tab
                || to.workspace != Some(from.workspace)
            {
                return None;
            }
            PaneMove::Swap {
                workspace: from.workspace,
                tab: from.tab,
                pane: from.pane,
                other,
            }
        }
        DropTarget::Pane {
            tab,
            pane,
            zone: Zone::Edge(side),
            ..
        } => PaneMove::Move {
            from,
            to: MoveDestination::Beside(Beside {
                target: PaneRef {
                    workspace: to.workspace?,
                    tab,
                    pane,
                },
                side,
            }),
        },
        DropTarget::Slot(_) | DropTarget::TitleRow => PaneMove::Move {
            from,
            to: MoveDestination::NewTab {
                workspace: to.workspace?,
                before: match target {
                    DropTarget::Slot(index) => to.tabs.get(index).copied(),
                    _ => None,
                },
            },
        },
        DropTarget::Workspace { workspace, .. } => {
            if workspace == from.workspace {
                return None;
            }
            PaneMove::Move {
                from,
                to: MoveDestination::NewTab {
                    workspace,
                    before: None,
                },
            }
        }
        DropTarget::Spring(_) | DropTarget::RowSlot(_) | DropTarget::Window => {
            return None;
        }
    };
    Some(Operation::Pane(change))
}

/// A tab dropped on `target`: an edge nests it beside the pane, a slot
/// reorders it in its own workspace or moves it into the one shown, as the
/// title row does, and another workspace's row moves it to that
/// workspace's end.
fn tab_operation(
    workspace: WorkspaceId,
    tab: TabId,
    target: DropTarget,
    to: &Destination<'_>,
) -> Option<Operation> {
    // The window shows another workspace than the tab's: another window's,
    // or one a row drag sprang to.
    let away = to.cross || to.workspace != Some(workspace);
    match target {
        DropTarget::Pane {
            tab: other_tab,
            pane,
            zone: Zone::Edge(side),
            ..
        } => Some(Operation::Pane(PaneMove::TabInto {
            workspace,
            tab,
            to: Beside {
                target: PaneRef {
                    workspace: to.workspace?,
                    tab: other_tab,
                    pane,
                },
                side,
            },
        })),
        DropTarget::Slot(index) => {
            let before = to.tabs.get(index).copied();
            Some(if away {
                Operation::MoveTab {
                    workspace,
                    tab,
                    to: to.workspace?,
                    before,
                }
            } else {
                Operation::ReorderTab {
                    workspace,
                    tab,
                    before,
                }
            })
        }
        DropTarget::TitleRow => away.then_some(Operation::MoveTab {
            workspace,
            tab,
            to: to.workspace?,
            before: None,
        }),
        DropTarget::Workspace {
            workspace: into, ..
        } => (into != workspace).then_some(Operation::MoveTab {
            workspace,
            tab,
            to: into,
            before: None,
        }),
        DropTarget::Pane {
            zone: Zone::Centre, ..
        }
        | DropTarget::Spring(_)
        | DropTarget::RowSlot(_)
        | DropTarget::Window => None,
    }
}

/// Whether dropping `row` before `before` moves it in `order`: not where it
/// already sits, and not once it left the list.
fn moves(order: &[RowKey], row: RowKey, before: Option<RowKey>) -> bool {
    let Some(at) = order.iter().position(|item| *item == row) else {
        return false;
    };
    before != order.get(at + 1).copied()
}

/// The slot a dragged row at `pointer` lands in: before the first other row
/// in `order` whose painted middle is below the pointer, else after the
/// last.
pub(super) fn row_slot(
    row: RowKey,
    pointer: gpui::Point<Pixels>,
    order: &[RowKey],
    painted: impl Fn(RowKey) -> Option<Bounds<Pixels>>,
) -> DropTarget {
    DropTarget::RowSlot(order.iter().copied().filter(|item| *item != row).find(
        |item| {
            painted(*item).is_some_and(|bounds| pointer.y < bounds.center().y)
        },
    ))
}

/// A press, which becomes a drag once the pointer moves past
/// [`DRAG_THRESHOLD`].
#[derive(Clone, Copy, Debug)]
pub(super) struct Press {
    pub(super) origin: gpui::Point<Pixels>,
    pub(super) pointer: gpui::Point<Pixels>,
    pub(super) dragging: bool,
}

impl Press {
    pub(super) fn new(pointer: gpui::Point<Pixels>) -> Self {
        Self {
            origin: pointer,
            pointer,
            dragging: false,
        }
    }

    /// Follows the pointer. Returns whether the press just became a drag.
    pub(super) fn follow(&mut self, pointer: gpui::Point<Pixels>) -> bool {
        self.pointer = pointer;
        let starts = !self.dragging
            && (pointer - self.origin).magnitude() > DRAG_THRESHOLD;
        self.dragging |= starts;
        starts
    }
}

/// The key a drag rests on, and since when. Every instant comes from the
/// caller, so tests drive the deadline without sleeping.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct SpringLoad {
    armed: Option<(SpringKey, Instant)>,
}

impl SpringLoad {
    /// Arms for `key` at `now` unless already armed for it; `None` disarms.
    pub(super) fn hover(&mut self, key: Option<SpringKey>, now: Instant) {
        match key {
            Some(key) if self.key() == Some(key) => {}
            key => self.rearm(key, now),
        }
    }

    /// Arms afresh, as when targets move under a still pointer.
    pub(super) fn rearm(&mut self, key: Option<SpringKey>, now: Instant) {
        self.armed = key.map(|key| (key, now));
    }

    pub(super) fn key(&self) -> Option<SpringKey> {
        self.armed.map(|(key, _)| key)
    }

    /// The armed tab, when the key is a tab.
    pub(super) fn tab(&self) -> Option<TabId> {
        match self.key()? {
            SpringKey::Tab(tab) => Some(tab),
            SpringKey::Workspace(_) => None,
        }
    }

    pub(super) fn armed_at(&self) -> Option<Instant> {
        self.armed.map(|(_, at)| at)
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.armed_at().map(|at| at + SPRING_LOAD_DELAY)
    }

    /// The armed key once its deadline has come.
    pub(super) fn due(&self, now: Instant) -> Option<SpringKey> {
        self.armed
            .filter(|(_, at)| now >= *at + SPRING_LOAD_DELAY)
            .map(|(key, _)| key)
    }
}

/// What a drag that spring-loaded switches between when it ends without a
/// drop: the workspace and tab shown when it began, and those shown after
/// its last spring load.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SpringBack {
    pub(super) origin: Shown,
    pub(super) sprung: Shown,
}

/// A drag over one window: the source's own gesture, or a destination's
/// preview of another window's. It holds the resolved target, the
/// spring-load and what it switched, and the tab bar's autoscroll.
#[derive(Clone, Debug)]
pub(super) struct Hover {
    pub(super) pointer: gpui::Point<Pixels>,
    pub(super) target: Option<DropTarget>,
    pub(super) spring: SpringLoad,
    /// What the window showed when the drag began over it.
    origin: Shown,
    /// What the window showed after the last spring load, unless that was
    /// `origin`.
    sprung: Option<Shown>,
    /// The bar scroll when the drag began, which an ending without a drop
    /// puts back.
    pub(super) scroll: Pixels,
    /// The strip as of the last motion, for autoscroll between frames.
    strip: TabStrip,
    /// Whether the strip autoscrolls under the pointer.
    pub(super) over_bar: bool,
}

impl Hover {
    pub(super) fn new(
        pointer: gpui::Point<Pixels>,
        origin: Shown,
        scroll: Pixels,
        strip: TabStrip,
    ) -> Self {
        Self {
            pointer,
            target: None,
            spring: SpringLoad::default(),
            origin,
            sprung: None,
            scroll,
            strip,
            over_bar: false,
        }
    }

    /// Takes the resolved `target` and arms spring-loading for it; `rearm`
    /// restarts the delay, as when labels move under a still pointer.
    pub(super) fn retarget(
        &mut self,
        target: Option<DropTarget>,
        now: Instant,
        rearm: bool,
    ) {
        self.target = target;
        let key = target.and_then(DropTarget::spring);
        if rearm {
            self.spring.rearm(key, now);
        } else {
            self.spring.hover(key, now);
        }
    }

    /// Forgets the target and the bar, as when the pointer leaves the
    /// window.
    pub(super) fn leave(&mut self) {
        self.target = None;
        self.over_bar = false;
        self.spring = SpringLoad::default();
    }

    /// Records that spring-loading made the window show `shown`, which
    /// names no tab while a sprung workspace has not picked one yet.
    pub(super) fn sprang(&mut self, shown: Shown) {
        self.sprung = (shown != self.origin).then_some(shown);
        self.spring = SpringLoad::default();
    }

    /// What the drag expects the window to show: what it spring-loaded,
    /// else what it began over.
    pub(super) fn expected(&self) -> Shown {
        self.sprung.unwrap_or(self.origin)
    }

    pub(super) fn back(&self) -> Option<SpringBack> {
        Some(SpringBack {
            origin: self.origin,
            sprung: self.sprung?,
        })
    }

    pub(super) fn set_strip(&mut self, strip: TabStrip) {
        self.strip = strip;
    }

    /// Whether autoscroll would move the bar from `scroll` now.
    pub(super) fn scrolls(&self, scroll: Pixels) -> bool {
        self.over_bar
            && self
                .strip
                .autoscroll(self.pointer, Duration::from_millis(16))
                != scroll
    }

    /// The bar offset autoscroll reaches after `elapsed`, over `strip`.
    pub(super) fn autoscroll(
        &mut self,
        strip: TabStrip,
        elapsed: Duration,
    ) -> Option<Pixels> {
        self.strip = strip;
        self.over_bar
            .then(|| self.strip.autoscroll(self.pointer, elapsed))
    }
}

/// A sidebar list's edge autoscroll under a dragged row.
#[derive(Clone, Copy, Debug)]
pub(super) struct ListScroll {
    /// When the edge scroll last moved the list.
    scrolled_at: Instant,
}

impl ListScroll {
    pub(super) fn new(now: Instant) -> Self {
        Self { scrolled_at: now }
    }

    /// The direction a dragged row at `pointer` scrolls the list `scroll`
    /// moves: positive towards the top, negative towards the bottom, `None`
    /// away from an edge or where the list cannot move further.
    pub(super) fn direction(
        pointer: gpui::Point<Pixels>,
        scroll: &gpui::ScrollHandle,
    ) -> Option<f32> {
        let list = scroll.bounds();
        let offset = scroll.offset().y;
        if pointer.y < list.top() + LIST_SCROLL_EDGE && offset < px(0.0) {
            Some(1.0)
        } else if pointer.y > list.bottom() - LIST_SCROLL_EDGE
            && offset > -scroll.max_offset().y
        {
            Some(-1.0)
        } else {
            None
        }
    }

    /// Scrolls the list under a row resting at its edge by the time since
    /// the last frame. Returns whether it scrolled.
    pub(super) fn advance(
        &mut self,
        now: Instant,
        pointer: gpui::Point<Pixels>,
        scroll: &gpui::ScrollHandle,
    ) -> bool {
        let direction = Self::direction(pointer, scroll);
        let elapsed = now.saturating_duration_since(self.scrolled_at);
        self.scrolled_at = now;
        let Some(direction) = direction else {
            return false;
        };
        // A long gap, such as the first frame, moves no more than a frame.
        let seconds = elapsed.as_secs_f32().min(0.05);
        let offset = scroll.offset();
        let y = (offset.y + px(LIST_SCROLL_SPEED * seconds * direction))
            .clamp(-scroll.max_offset().y, px(0.0));
        scroll.set_offset(point(offset.x, y));
        true
    }
}

/// One drag in its source window, from the press on: what it drags, the
/// press and its threshold, and the hover it shares with a destination's
/// preview.
pub(super) struct Gesture {
    pub(super) payload: Payload,
    press: Press,
    pub(super) hover: Hover,
    /// The pointer left the source window; the cross-window record owns
    /// where it is.
    pub(super) outside: bool,
    /// The press was a macOS Control-left that GPUI turned into Right.
    remapped: bool,
    list: ListScroll,
}

impl Gesture {
    pub(super) fn new(
        payload: Payload,
        pointer: gpui::Point<Pixels>,
        hover: Hover,
        remapped: bool,
    ) -> Self {
        Self {
            payload,
            press: Press::new(pointer),
            hover,
            outside: false,
            remapped,
            list: ListScroll::new(Instant::now()),
        }
    }

    pub(super) fn dragging(&self) -> bool {
        self.press.dragging
    }

    pub(super) fn pointer(&self) -> gpui::Point<Pixels> {
        self.press.pointer
    }

    /// Where the press began, in window coordinates.
    #[cfg(target_os = "macos")]
    pub(super) fn origin(&self) -> gpui::Point<Pixels> {
        self.press.origin
    }

    pub(super) fn target(&self) -> Option<DropTarget> {
        self.hover.target
    }

    pub(super) fn remapped(&self) -> bool {
        self.remapped
    }

    /// Follows the pointer. Returns whether the press just became a drag.
    pub(super) fn follow(&mut self, pointer: gpui::Point<Pixels>) -> bool {
        self.hover.pointer = pointer;
        let starts = self.press.follow(pointer);
        if starts {
            self.list = ListScroll::new(Instant::now());
        }
        starts
    }

    /// What the drag expects the window to show: what it spring-loaded,
    /// else what it started over.
    pub(super) fn expected(&self) -> Shown {
        self.hover.expected()
    }

    pub(super) fn back(&self) -> Option<SpringBack> {
        self.hover.back()
    }

    /// A tab drag in pane mode: over a pane of the active tab, deep enough
    /// past the bar that a release nests the tab instead of reordering it.
    pub(super) fn nesting(&self) -> bool {
        matches!(self.payload, Payload::Tab { .. })
            && self.dragging()
            && !self.outside
            && !matches!(self.target(), Some(DropTarget::Slot(_)))
    }

    /// Whether Escape ends the gesture: a tab press at once, every other
    /// press only once it drags, so before that Escape reaches the
    /// terminal.
    pub(super) fn escape_cancels(&self) -> bool {
        matches!(self.payload, Payload::Tab { .. }) || self.dragging()
    }

    /// Whether the tab bar autoscrolls from `scroll` now.
    pub(super) fn scrolls(&self, scroll: Pixels) -> bool {
        self.dragging() && self.hover.scrolls(scroll)
    }

    /// The bar offset autoscroll reaches after `elapsed`, over `strip`.
    pub(super) fn autoscroll(
        &mut self,
        strip: TabStrip,
        elapsed: Duration,
    ) -> Option<Pixels> {
        if self.dragging() {
            self.hover.autoscroll(strip, elapsed)
        } else {
            self.hover.set_strip(strip);
            None
        }
    }

    /// Whether the drag scrolls the list `scroll`, always inside its
    /// window: a dragged row anywhere there, a tab or pane only over the
    /// list.
    fn in_list(&self, scroll: &gpui::ScrollHandle) -> bool {
        !self.outside
            && (self.payload.row().is_some()
                || (self.payload.in_bar()
                    && scroll.bounds().contains(&self.pointer())))
    }

    /// Whether the drag rests at the edge of the list `scroll`.
    pub(super) fn list_scrolls(&self, scroll: &gpui::ScrollHandle) -> bool {
        self.dragging()
            && self.in_list(scroll)
            && ListScroll::direction(self.pointer(), scroll).is_some()
    }

    /// Scrolls the list under a drag resting at its edge. Returns whether
    /// it scrolled.
    pub(super) fn advance_list(
        &mut self,
        now: Instant,
        scroll: &gpui::ScrollHandle,
    ) -> bool {
        self.dragging()
            && self.in_list(scroll)
            && self.list.advance(now, self.press.pointer, scroll)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> WorkspaceId {
        WorkspaceId::new(1)
    }

    #[test]
    fn a_press_becomes_a_drag_only_past_the_threshold() {
        let mut press = Press::new(point(px(10.0), px(10.0)));
        assert!(!press.follow(point(px(13.0), px(12.0))));
        assert!(!press.dragging);
        assert!(press.follow(point(px(15.0), px(10.0))));
        assert!(press.dragging);
        // Moving back within the threshold keeps the drag.
        assert!(!press.follow(point(px(10.0), px(10.0))));
        assert!(press.dragging);
        assert_eq!(press.pointer, point(px(10.0), px(10.0)));
    }

    #[test]
    fn a_spring_load_arms_rearms_and_comes_due_at_its_deadline() {
        let (first, second) =
            (SpringKey::Tab(TabId::new(1)), SpringKey::Tab(TabId::new(2)));
        let start = Instant::now();
        let mut spring = SpringLoad::default();
        assert_eq!(spring.deadline(), None);
        spring.hover(Some(first), start);
        assert_eq!(spring.key(), Some(first));
        assert_eq!(spring.tab(), Some(TabId::new(1)));
        assert_eq!(spring.deadline(), Some(start + SPRING_LOAD_DELAY));
        // Hovering the same target keeps the first arming.
        spring.hover(Some(first), start + Duration::from_millis(300));
        assert_eq!(spring.armed_at(), Some(start));
        let deadline = start + SPRING_LOAD_DELAY;
        let early = deadline.checked_sub(Duration::from_millis(1)).unwrap();
        assert_eq!(spring.due(early), None);
        assert_eq!(spring.due(deadline), Some(first));
        // Another target re-arms with a fresh time; leaving disarms.
        let later = start + Duration::from_millis(400);
        spring.hover(Some(second), later);
        assert_eq!(spring.key(), Some(second));
        assert_eq!(spring.due(deadline), None);
        assert_eq!(spring.deadline(), Some(later + SPRING_LOAD_DELAY));
        spring.rearm(Some(second), later + Duration::from_millis(100));
        assert_eq!(spring.armed_at(), Some(later + Duration::from_millis(100)));
        spring.hover(None, later);
        assert_eq!(spring, SpringLoad::default());
        // A workspace row arms the same way and names its workspace.
        let row = SpringKey::Workspace(workspace());
        spring.hover(Some(row), later);
        assert_eq!(spring.key(), Some(row));
        assert_eq!(spring.tab(), None);
        assert_eq!(spring.due(later + SPRING_LOAD_DELAY), Some(row));
    }

    #[test]
    fn a_row_slot_follows_the_painted_midpoints_and_skips_the_dragged_row() {
        let rows = [
            RowKey::Workspace(WorkspaceId::new(1)),
            RowKey::Workspace(WorkspaceId::new(2)),
            RowKey::Workspace(WorkspaceId::new(3)),
        ];
        let painted = |row: RowKey| {
            let index = rows.iter().position(|item| *item == row)?;
            #[expect(clippy::cast_precision_loss, reason = "three rows")]
            let top = 40.0 * index as f32;
            Some(Bounds::new(
                point(px(0.0), px(top)),
                size(px(100.0), px(40.0)),
            ))
        };
        let at = |y: f32, dragged: RowKey| {
            row_slot(dragged, point(px(5.0), px(y)), &rows, painted)
        };
        let [first, second, third] = rows;
        assert_eq!(at(10.0, third), DropTarget::RowSlot(Some(first)));
        assert_eq!(at(50.0, third), DropTarget::RowSlot(Some(second)));
        // Over the dragged row's own middle, the next row is the slot.
        assert_eq!(at(10.0, first), DropTarget::RowSlot(Some(second)));
        assert_eq!(at(119.0, first), DropTarget::RowSlot(None));
    }

    #[test]
    fn a_row_drop_moves_only_when_it_changes_the_order() {
        let [one, two, three] = [1, 2, 3].map(WorkspaceId::new);
        let rows = [one, two, three].map(RowKey::Workspace);
        let to = Destination {
            rows: &rows,
            ..Destination::default()
        };
        let drag = Payload::Workspace {
            session: SessionId::new(1),
            workspace: one,
        };
        let drop = |before: Option<WorkspaceId>| {
            operation(
                drag,
                DropTarget::RowSlot(before.map(RowKey::Workspace)),
                &to,
            )
        };
        // Before its successor, it stays where it is.
        assert_eq!(drop(Some(two)), None);
        assert_eq!(
            drop(Some(three)),
            Some(Operation::MoveWorkspace {
                session: SessionId::new(1),
                workspace: one,
                to: SessionId::new(1),
                before: Some(three),
            })
        );
        assert_eq!(
            drop(None),
            Some(Operation::MoveWorkspace {
                session: SessionId::new(1),
                workspace: one,
                to: SessionId::new(1),
                before: None,
            })
        );
        // A row that left the list moves nothing.
        let gone = Destination {
            rows: &rows[1..],
            ..Destination::default()
        };
        assert_eq!(operation(drag, DropTarget::RowSlot(None), &gone), None);
        // A session slot takes no workspace.
        assert_eq!(
            operation(
                drag,
                DropTarget::RowSlot(Some(RowKey::Session(SessionId::new(1)))),
                &to
            ),
            None
        );
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "one table, read top to bottom")]
    fn a_tab_or_pane_drop_maps_to_one_operation_or_a_refusal() {
        let (tab, other_tab) = (TabId::new(1), TabId::new(2));
        let (pane, other) = (PaneId::new(1), PaneId::new(2));
        let tabs = [tab, other_tab];
        let local = Destination {
            workspace: Some(workspace()),
            tabs: &tabs,
            ..Destination::default()
        };
        let cross = Destination {
            workspace: Some(WorkspaceId::new(2)),
            cross: true,
            tabs: &tabs,
            ..Destination::default()
        };
        let frame = Bounds::default();
        let zone = |tab, zone, refused| DropTarget::Pane {
            tab,
            pane: other,
            frame,
            zone,
            refused,
        };
        let pane_drag = Payload::Pane {
            workspace: workspace(),
            tab,
            pane,
        };
        let tab_drag = Payload::Tab {
            workspace: workspace(),
            tab,
        };
        let right = Zone::Edge(SplitDirection::Right);
        // A centre swaps only within the dragged pane's own tab, here.
        assert_eq!(
            operation(pane_drag, zone(tab, Zone::Centre, false), &local),
            Some(Operation::Pane(PaneMove::Swap {
                workspace: workspace(),
                tab,
                pane,
                other,
            }))
        );
        for (target, to) in [
            (zone(other_tab, Zone::Centre, false), &local),
            (zone(tab, Zone::Centre, false), &cross),
            (zone(tab, right, true), &local),
            (DropTarget::Spring(SpringKey::Tab(other_tab)), &local),
        ] {
            assert_eq!(operation(pane_drag, target, to), None, "{target:?}");
        }
        assert_eq!(
            operation(pane_drag, DropTarget::Slot(1), &cross),
            Some(Operation::Pane(PaneMove::Move {
                from: PaneRef {
                    workspace: workspace(),
                    tab,
                    pane,
                },
                to: MoveDestination::NewTab {
                    workspace: WorkspaceId::new(2),
                    before: Some(other_tab),
                },
            }))
        );
        assert_eq!(
            operation(pane_drag, DropTarget::TitleRow, &local),
            Some(Operation::Pane(PaneMove::Move {
                from: PaneRef {
                    workspace: workspace(),
                    tab,
                    pane,
                },
                to: MoveDestination::NewTab {
                    workspace: workspace(),
                    before: None,
                },
            }))
        );
        // A tab reorders here and moves across windows.
        assert_eq!(
            operation(tab_drag, DropTarget::Slot(2), &local),
            Some(Operation::ReorderTab {
                workspace: workspace(),
                tab,
                before: None,
            })
        );
        assert_eq!(
            operation(tab_drag, DropTarget::TitleRow, &cross),
            Some(Operation::MoveTab {
                workspace: workspace(),
                tab,
                to: WorkspaceId::new(2),
                before: None,
            })
        );
        assert_eq!(operation(tab_drag, DropTarget::TitleRow, &local), None);
        // A tab nests beside a pane's edge and never into a centre.
        assert!(matches!(
            operation(tab_drag, zone(other_tab, right, false), &cross),
            Some(Operation::Pane(PaneMove::TabInto { .. }))
        ));
        assert_eq!(
            operation(tab_drag, zone(other_tab, Zone::Centre, false), &local),
            None
        );
    }

    #[test]
    fn a_workspace_row_takes_tabs_and_panes_from_other_workspaces_only() {
        let (own, other) = (workspace(), WorkspaceId::new(2));
        let (tab, pane) = (TabId::new(1), PaneId::new(1));
        let tabs = [tab];
        let to = Destination {
            workspace: Some(own),
            tabs: &tabs,
            ..Destination::default()
        };
        let row =
            |workspace, spring| DropTarget::Workspace { workspace, spring };
        let tab_drag = Payload::Tab {
            workspace: own,
            tab,
        };
        let pane_drag = Payload::Pane {
            workspace: own,
            tab,
            pane,
        };
        assert_eq!(
            operation(tab_drag, row(other, true), &to),
            Some(Operation::MoveTab {
                workspace: own,
                tab,
                to: other,
                before: None,
            })
        );
        assert_eq!(
            operation(pane_drag, row(other, false), &to),
            Some(Operation::Pane(PaneMove::Move {
                from: PaneRef {
                    workspace: own,
                    tab,
                    pane,
                },
                to: MoveDestination::NewTab {
                    workspace: other,
                    before: None,
                },
            }))
        );
        // The item's own workspace takes no drop, shown or not.
        for spring in [false, true] {
            assert_eq!(operation(tab_drag, row(own, spring), &to), None);
            assert_eq!(operation(pane_drag, row(own, spring), &to), None);
        }
        // Only a row that is not shown spring-loads.
        assert_eq!(
            row(other, true).spring(),
            Some(SpringKey::Workspace(other))
        );
        assert_eq!(row(other, false).spring(), None);
        // Rows and windows take no row drags this way.
        let row_drag = Payload::Workspace {
            session: SessionId::new(1),
            workspace: own,
        };
        assert_eq!(operation(row_drag, row(other, true), &to), None);
        assert_eq!(operation(tab_drag, DropTarget::Window, &to), None);
    }

    #[test]
    fn a_tab_in_a_window_showing_another_workspace_moves_there() {
        let (own, shown) = (workspace(), WorkspaceId::new(2));
        let (tab, there) = (TabId::new(1), TabId::new(5));
        let tabs = [there];
        // A row drag sprang the window to `shown`: the tab is not in its
        // bar, so a slot or the title row moves the tab instead of
        // reordering it.
        let sprung = Destination {
            workspace: Some(shown),
            tabs: &tabs,
            ..Destination::default()
        };
        let drag = Payload::Tab {
            workspace: own,
            tab,
        };
        assert_eq!(
            operation(drag, DropTarget::Slot(0), &sprung),
            Some(Operation::MoveTab {
                workspace: own,
                tab,
                to: shown,
                before: Some(there),
            })
        );
        assert_eq!(
            operation(drag, DropTarget::TitleRow, &sprung),
            Some(Operation::MoveTab {
                workspace: own,
                tab,
                to: shown,
                before: None,
            })
        );
    }

    #[test]
    fn a_workspace_dropped_on_another_session_moves_there() {
        let (session, other) = (SessionId::new(1), SessionId::new(2));
        let [one, two, three] = [1, 2, 3].map(WorkspaceId::new);
        let rows = [two, three].map(RowKey::Workspace);
        let drag = Payload::Workspace {
            session,
            workspace: one,
        };
        let there = Destination {
            session: Some(other),
            cross: true,
            rows: &rows,
            ..Destination::default()
        };
        for (target, before) in [
            (
                DropTarget::RowSlot(Some(RowKey::Workspace(three))),
                Some(three),
            ),
            (DropTarget::RowSlot(None), None),
            (DropTarget::Window, None),
        ] {
            assert_eq!(
                operation(drag, target, &there),
                Some(Operation::MoveWorkspace {
                    session,
                    workspace: one,
                    to: other,
                    before,
                }),
                "{target:?}"
            );
        }
        // Another window of the same session reorders it, and its
        // content takes no drop.
        let own_rows = [one, two, three].map(RowKey::Workspace);
        let same = Destination {
            session: Some(session),
            cross: true,
            rows: &own_rows,
            ..Destination::default()
        };
        assert_eq!(
            operation(
                drag,
                DropTarget::RowSlot(Some(RowKey::Workspace(three))),
                &same
            ),
            Some(Operation::MoveWorkspace {
                session,
                workspace: one,
                to: session,
                before: Some(three),
            })
        );
        assert_eq!(operation(drag, DropTarget::Window, &same), None);
        // The source's own content takes no drop either.
        let local = Destination {
            session: Some(session),
            rows: &own_rows,
            ..Destination::default()
        };
        assert_eq!(operation(drag, DropTarget::Window, &local), None);
    }

    #[test]
    fn a_spring_back_returns_to_the_workspace_and_tab_the_drag_began_over() {
        let (own, other) = (workspace(), WorkspaceId::new(2));
        let origin = Shown {
            workspace: Some(own),
            tab: Some(TabId::new(1)),
        };
        let mut hover = Hover::new(
            point(px(0.0), px(0.0)),
            origin,
            px(0.0),
            TabStrip::new(
                Bounds::default(),
                false,
                super::super::tab_strip::TabExtents::Uniform(0),
                px(0.0),
                px(0.0),
            ),
        );
        assert_eq!(hover.back(), None);
        assert_eq!(hover.expected(), origin);
        // A row spring names its workspace before the tab is known; any
        // tab of it matches until the drag records the one it shows.
        let pending = Shown {
            workspace: Some(other),
            tab: None,
        };
        hover.sprang(pending);
        assert!(hover.expected().matches(Shown {
            workspace: Some(other),
            tab: Some(TabId::new(7)),
        }));
        assert!(!hover.expected().matches(origin));
        let sprung = Shown {
            workspace: Some(other),
            tab: Some(TabId::new(7)),
        };
        hover.sprang(sprung);
        assert_eq!(hover.back(), Some(SpringBack { origin, sprung }));
        // Springing back onto the origin leaves nothing to switch back.
        hover.sprang(origin);
        assert_eq!(hover.back(), None);
    }
}
