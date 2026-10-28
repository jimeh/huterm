//! Pane drags from a title bar, tab drags that nest a tab into a pane, and
//! spring-loaded tabs. A drag resolves its target from the last drawn frame
//! on each motion, draws the result it would have, and on release commits
//! one Mux operation through the same path as the pane move commands.

// User input picks positions here, so a stale index must not panic.
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use gpui::Hsla;
use huterm_core::{Beside, MoveDestination, PaneRef};

use super::pane_geometry::{PaneRect, Zone, drop_zone, zone_preview};
use super::pane_moves::PaneMove;
use super::tab_strip::LabelZone;
use super::*;

/// How long a pane drag rests on an inactive tab's label before that tab
/// activates.
pub(super) const SPRING_LOAD_DELAY: Duration = Duration::from_millis(500);

/// How far past the tab bar's inner edge a tab drag must reach over a pane,
/// perpendicular to the bar, before a release nests the tab instead of
/// reordering it.
pub(super) const PANE_DROP_DEPTH: Pixels = px(32.0);

/// The tab a pane drag rests on, and since when. Every instant comes from
/// the caller, so tests drive the deadline without sleeping.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct SpringLoad {
    armed: Option<(TabId, Instant)>,
}

impl SpringLoad {
    /// Arms for `tab` at `now` unless already armed for it; `None` disarms.
    pub(super) fn hover(&mut self, tab: Option<TabId>, now: Instant) {
        match tab {
            Some(tab) if self.tab() == Some(tab) => {}
            tab => self.rearm(tab, now),
        }
    }

    /// Arms afresh, as when labels move under a still pointer.
    pub(super) fn rearm(&mut self, tab: Option<TabId>, now: Instant) {
        self.armed = tab.map(|tab| (tab, now));
    }

    pub(super) fn tab(&self) -> Option<TabId> {
        self.armed.map(|(tab, _)| tab)
    }

    pub(super) fn armed_at(&self) -> Option<Instant> {
        self.armed.map(|(_, at)| at)
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.armed_at().map(|at| at + SPRING_LOAD_DELAY)
    }

    /// The armed tab once its deadline has come.
    pub(super) fn due(&self, now: Instant) -> Option<TabId> {
        self.armed
            .filter(|(_, at)| now >= *at + SPRING_LOAD_DELAY)
            .map(|(tab, _)| tab)
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
    /// The tab bar insertion slot before tab `index`, where a dropped pane
    /// opens a new tab.
    Slot(usize),
    /// An inactive tab's label middle, which spring-loads that tab.
    Spring(TabId),
}

/// A tab drag in pane mode: over a pane of the active tab, deep enough
/// past the bar that a release nests the tab instead of reordering it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PaneMode {
    /// Over a centre, which offers nothing.
    Centre,
    /// Over an edge, which nests the tab beside the pane.
    Edge(DropTarget),
}

/// The tabs a drag that spring-loaded switches between when it ends
/// without a drop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SpringBack {
    /// The tab the drag started in.
    pub(super) origin: TabId,
    /// The tab spring-loading activated.
    pub(super) sprung: TabId,
}

/// A press on a pane's title bar, which becomes a drag past the threshold.
pub(super) struct PaneDrag {
    /// The tab holding the pane, active when the press began.
    pub(super) tab: TabId,
    pub(super) pane: PaneId,
    origin: gpui::Point<Pixels>,
    pub(super) pointer: gpui::Point<Pixels>,
    pub(super) dragging: bool,
    pub(super) target: Option<DropTarget>,
    /// The last tab spring-loading activated, unless that was `tab`.
    pub(super) sprung: Option<TabId>,
    pub(super) spring: SpringLoad,
    /// Whether the pointer is over the tab bar, which autoscrolls.
    over_bar: bool,
    original_scroll: Pixels,
    /// The strip as of the last motion, for autoscroll between frames.
    strip: TabStrip,
}

impl PaneDrag {
    /// The tab the drag expects active: the one it spring-loaded, else its
    /// own. Any other active tab means something else switched tabs.
    pub(super) fn expected_active(&self) -> TabId {
        self.sprung.unwrap_or(self.tab)
    }

    fn back(&self) -> Option<SpringBack> {
        self.sprung.map(|sprung| SpringBack {
            origin: self.tab,
            sprung,
        })
    }

    /// Whether autoscroll would move the bar under the pointer now.
    pub(super) fn scrolls(&self, scroll: Pixels) -> bool {
        self.dragging
            && self.over_bar
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
        (self.dragging && self.over_bar)
            .then(|| self.strip.autoscroll(self.pointer, elapsed))
    }
}

impl WorkspaceView {
    /// Starts tracking a press on `pane`'s title bar in `tab`; motion past
    /// the threshold makes it a drag.
    pub(super) fn begin_pane_drag(
        &mut self,
        tab: TabId,
        pane: PaneId,
        pointer: gpui::Point<Pixels>,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        if !self.can_drag_panes(cx) {
            return;
        }
        self.scroll_target = None;
        self.pane_drag = Some(PaneDrag {
            tab,
            pane,
            origin: pointer,
            pointer,
            dragging: false,
            target: None,
            sprung: None,
            spring: SpringLoad::default(),
            over_bar: false,
            original_scroll: self.tab_scroll,
            strip: self.tab_strip(window),
        });
    }

    fn can_drag_panes(&self, cx: &App) -> bool {
        let desktop = cx.global::<Desktop>();
        !self.busy
            && self.arrangements == 0
            && !self.overlay_holds_focus()
            && !desktop.quitting
            && !desktop.quit_pending
    }

    /// Whether the drag can go on: the window takes drags, the dragged pane
    /// is still in its tab here and in the projection, and the active tab
    /// is the one the drag expects.
    pub(super) fn pane_drag_valid(&self, cx: &App) -> bool {
        let Some(drag) = &self.pane_drag else {
            return false;
        };
        self.can_drag_panes(cx)
            && self.active_tab(cx) == Some(drag.expected_active())
            && self.tabs.iter().any(|view| {
                view.id == drag.tab && view.pane(drag.pane).is_some()
            })
            && cx.global::<Desktop>().hierarchy.state().pane_tab(drag.pane)
                == Some(drag.tab)
    }

    pub(super) fn update_pane_drag(
        &mut self,
        pointer: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.pane_drag.is_none() {
            return;
        }
        if !self.pane_drag_valid(cx) {
            self.cancel_pane_drag(window, cx);
            return;
        }
        let strip = self.tab_strip(window);
        let over_bar = self.over_tab_bar(pointer, window);
        let Some(drag) = &mut self.pane_drag else {
            return;
        };
        let was_scrolling = drag.scrolls(self.tab_scroll);
        drag.pointer = pointer;
        drag.strip = strip;
        drag.over_bar = over_bar;
        if !drag.dragging
            && (pointer - drag.origin).magnitude() > TAB_DRAG_THRESHOLD
        {
            drag.dragging = true;
        }
        if !drag.dragging {
            return;
        }
        if !was_scrolling {
            self.last_scroll = Instant::now();
        }
        self.retarget_pane_drag(Instant::now(), false, window, cx);
        cx.notify();
    }

    /// Resolves the drag's target at its pointer and arms spring-loading
    /// for the label under it; `rearm` restarts the delay, as when labels
    /// move under a still pointer.
    fn retarget_pane_drag(
        &mut self,
        now: Instant,
        rearm: bool,
        window: &Window,
        cx: &App,
    ) {
        let Some(pointer) = self.pane_drag.as_ref().map(|drag| drag.pointer)
        else {
            return;
        };
        let target = self.pane_drop_target(pointer, window, cx);
        let Some(drag) = &mut self.pane_drag else {
            return;
        };
        drag.target = target;
        let spring = match target {
            Some(DropTarget::Spring(tab)) => Some(tab),
            _ => None,
        };
        if rearm {
            drag.spring.rearm(spring, now);
        } else {
            drag.spring.hover(spring, now);
        }
    }

    /// Resolves the drag's target again against the panes a frame just
    /// drew, redrawing only when it changed.
    pub(super) fn retarget_after_frame(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(before) = self
            .pane_drag
            .as_ref()
            .filter(|drag| drag.dragging)
            .map(|drag| drag.target)
        else {
            return;
        };
        self.retarget_pane_drag(Instant::now(), false, window, cx);
        if self.pane_drag.as_ref().map(|drag| drag.target) != Some(before) {
            cx.notify();
        }
    }

    /// Whether `pointer` is over the drawn tab strip: the bar without the
    /// window buttons or title-row controls at either end.
    fn over_tab_bar(
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

    /// The target of a pane drag at `pointer`, from the last drawn frame.
    fn pane_drop_target(
        &self,
        pointer: gpui::Point<Pixels>,
        window: &Window,
        cx: &App,
    ) -> Option<DropTarget> {
        let drag = self.pane_drag.as_ref()?;
        if self.over_tab_bar(pointer, window) {
            let active = self.active_tab(cx).and_then(|active| {
                self.tabs.iter().position(|tab| tab.id == active)
            });
            return match self.tab_strip(window).label_zone(pointer, active)? {
                LabelZone::Spring(index) => {
                    Some(DropTarget::Spring(self.tabs.get(index)?.id))
                }
                LabelZone::Slot(index) => Some(DropTarget::Slot(index)),
            };
        }
        let (tab, rect) = self.drawn_pane_at(pointer)?;
        // Until the next frame after a spring load, the drawn panes are the
        // previous tab's.
        if rect.pane == drag.pane || self.active_tab(cx) != Some(tab) {
            return None;
        }
        let zone = drop_zone(rect.frame, pointer)?;
        // A swap exchanges panes within one tab.
        if zone == Zone::Centre && tab != drag.tab {
            return None;
        }
        let target = DropTarget::Pane {
            tab,
            pane: rect.pane,
            frame: rect.frame,
            zone,
            refused: false,
        };
        let change = self.pane_drop_change(drag.tab, drag.pane, target, cx)?;
        let refused = self.check_move_fits(change, window, cx).is_err();
        Some(DropTarget::Pane {
            tab,
            pane: rect.pane,
            frame: rect.frame,
            zone,
            refused,
        })
    }

    /// The drawn pane of the active tab under `pointer`, with its tab.
    fn drawn_pane_at(
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
    pub(super) fn drawn_frame(&self, pane: PaneId) -> Option<Bounds<Pixels>> {
        let drawn = self.drawn_panes.borrow();
        let (_, rects) = drawn.as_ref()?;
        rects
            .iter()
            .find(|rect| rect.pane == pane)
            .map(|rect| rect.frame)
    }

    /// The operation a pane drag of `pane` from `tab` commits at `target`.
    fn pane_drop_change(
        &self,
        tab: TabId,
        pane: PaneId,
        target: DropTarget,
        cx: &App,
    ) -> Option<PaneMove> {
        let workspace = self.workspace_id(cx)?;
        let from = PaneRef {
            workspace,
            tab,
            pane,
        };
        Some(match target {
            DropTarget::Pane {
                pane: other,
                zone: Zone::Centre,
                ..
            } => PaneMove::Swap {
                workspace,
                tab,
                pane,
                other,
            },
            DropTarget::Pane {
                tab: destination,
                pane: other,
                zone: Zone::Edge(side),
                ..
            } => PaneMove::Move {
                from,
                to: MoveDestination::Beside(Beside {
                    target: PaneRef {
                        workspace,
                        tab: destination,
                        pane: other,
                    },
                    side,
                }),
            },
            DropTarget::Slot(index) => PaneMove::Move {
                from,
                to: MoveDestination::NewTab {
                    workspace,
                    before: self.tabs.get(index).map(|view| view.id),
                },
            },
            DropTarget::Spring(_) => return None,
        })
    }

    /// Ends the drag at `pointer`: a drop commits its operation, and a
    /// release without a target cancels.
    pub(super) fn finish_pane_drag(
        &mut self,
        pointer: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.update_pane_drag(pointer, window, cx);
        let Some(drag) = self.pane_drag.take() else {
            return;
        };
        cx.notify();
        if !drag.dragging {
            // A plain press only focused the pane.
            return;
        }
        let change = drag
            .target
            .filter(|target| {
                !matches!(target, DropTarget::Pane { refused: true, .. })
            })
            .and_then(|target| {
                self.pane_drop_change(drag.tab, drag.pane, target, cx)
            });
        match change {
            Some(change) => self.commit_pane_move(
                change,
                Some(drag.pane),
                drag.back(),
                window,
                cx,
            ),
            None => self.end_pane_drag(&drag, window, cx),
        }
    }

    /// Cancels a pane drag, switching back to the tab it started in when it
    /// spring-loaded another.
    pub(super) fn cancel_pane_drag(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(drag) = self.pane_drag.take() {
            self.end_pane_drag(&drag, window, cx);
        }
    }

    fn end_pane_drag(
        &mut self,
        drag: &PaneDrag,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.tab_scroll = drag.original_scroll;
        if let Some(back) = drag.back() {
            self.switch_back(back, window, cx);
        }
        if !self.overlay_holds_focus() {
            self.restore_tab_focus(window, cx);
        }
        cx.notify();
    }

    /// Reactivates the tab a drag started in when the tab it spring-loaded
    /// is still active, without taking focus from an overlay. A tab switch
    /// the drag did not make leaves the window where it is.
    pub(super) fn switch_back(
        &mut self,
        back: SpringBack,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.active_tab(cx) == Some(back.sprung)
            && self.tabs.iter().any(|view| view.id == back.origin)
        {
            let focus = !self.overlay_holds_focus();
            self.select_tab(back.origin, focus, window, cx);
        }
    }

    /// Drops a pane drag that a tab switch it did not make interrupted;
    /// nothing switches back.
    pub(super) fn interrupt_pane_drag(&mut self, selected: TabId) {
        if self
            .pane_drag
            .as_ref()
            .is_some_and(|drag| drag.expected_active() != selected)
        {
            self.pane_drag = None;
        }
    }

    /// Fires a spring load whose deadline has come. The pointer is resolved
    /// against the strip as drawn now: still over the armed tab's spring
    /// zone, the tab activates; over another tab's, the delay restarts for
    /// it; anywhere else, spring-loading disarms.
    pub(super) fn advance_spring(
        &mut self,
        now: Instant,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(armed) = self
            .pane_drag
            .as_ref()
            .and_then(|drag| drag.spring.due(now))
        else {
            return;
        };
        if !self.pane_drag_valid(cx) {
            self.cancel_pane_drag(window, cx);
            return;
        }
        self.retarget_pane_drag(now, false, window, cx);
        let Some(drag) = &mut self.pane_drag else {
            return;
        };
        if drag.target != Some(DropTarget::Spring(armed)) {
            // `retarget` re-armed for another label or disarmed.
            cx.notify();
            return;
        }
        drag.sprung = (armed != drag.tab).then_some(armed);
        drag.spring = SpringLoad::default();
        let focus = !self.overlay_holds_focus();
        self.select_tab(armed, focus, window, cx);
        self.retarget_pane_drag(now, false, window, cx);
        cx.notify();
    }

    /// Restarts spring-loading after autoscroll moved the labels under a
    /// still pointer, so a scrolling bar never springs.
    pub(super) fn rearm_after_scroll(
        &mut self,
        now: Instant,
        window: &Window,
        cx: &App,
    ) {
        if self.pane_drag.as_ref().is_some_and(|drag| drag.dragging) {
            self.retarget_pane_drag(now, true, window, cx);
        }
    }

    /// Where a tab drag is in pane mode, or `None` when it reorders. Pane
    /// mode needs the pointer over a pane of the active tab, which is not
    /// the dragged tab, at least [`PANE_DROP_DEPTH`] past the bar's inner
    /// edge.
    pub(super) fn reorder_pane_mode(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<PaneMode> {
        let drag = self.reorder.as_ref().filter(|drag| drag.dragging)?;
        let active = self.active_tab(cx)?;
        if drag.source.tab == active || !self.bar_drawn() {
            return None;
        }
        let (tab, rect) = self.drawn_pane_at(drag.pointer)?;
        if tab != active
            || self.depth_past_bar(drag.pointer, window) < PANE_DROP_DEPTH
        {
            return None;
        }
        let Some(Zone::Edge(side)) = drop_zone(rect.frame, drag.pointer) else {
            return Some(PaneMode::Centre);
        };
        let workspace = self.workspace_id(cx)?;
        let change = PaneMove::TabInto {
            workspace,
            tab: drag.source.tab,
            to: Beside {
                target: PaneRef {
                    workspace,
                    tab,
                    pane: rect.pane,
                },
                side,
            },
        };
        Some(PaneMode::Edge(DropTarget::Pane {
            tab,
            pane: rect.pane,
            frame: rect.frame,
            zone: Zone::Edge(side),
            refused: self.check_move_fits(change, window, cx).is_err(),
        }))
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

    /// Commits a tab drag released in pane mode: an edge zone nests the
    /// tab beside the pane, and a centre or a refused zone does nothing.
    pub(super) fn finish_nest(
        &mut self,
        source: TabDragSource,
        target: Option<DropTarget>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(DropTarget::Pane {
            tab,
            pane,
            zone: Zone::Edge(side),
            refused: false,
            ..
        }) = target
        else {
            self.restore_tab_focus(window, cx);
            return;
        };
        let focus = self
            .tabs
            .iter()
            .find(|view| view.id == source.tab)
            .map(|view| view.focused().pane);
        let change = PaneMove::TabInto {
            workspace: source.workspace,
            tab: source.tab,
            to: Beside {
                target: PaneRef {
                    workspace: source.workspace,
                    tab,
                    pane,
                },
                side,
            },
        };
        self.commit_pane_move(change, focus, None, window, cx);
    }

    /// What a drag shows over the window: the result region of its target,
    /// the insertion marker of a tab bar slot, and a chip with the dragged
    /// pane's or tab's title at the pointer. It also records the drawn
    /// target for smoke state.
    pub(super) fn drag_overlay(
        &self,
        colors: TabColors,
        window: &Window,
        cx: &App,
    ) -> Vec<gpui::AnyElement> {
        let mut elements = Vec::new();
        let (target, pointer, title, source) = if let Some(drag) =
            self.pane_drag.as_ref().filter(|drag| drag.dragging)
        {
            let tab = self.tabs.iter().find(|view| view.id == drag.tab);
            let title = tab
                .and_then(|tab| {
                    let pane = tab.pane(drag.pane)?;
                    Some(pane.label(
                        tab.fallback_name(drag.pane, cx),
                        self.config.tabs,
                        cx,
                    ))
                })
                .unwrap_or_default();
            (
                drag.target,
                drag.pointer,
                title,
                self.drawn_frame(drag.pane),
            )
        } else if let Some(drag) = self
            .reorder
            .as_ref()
            .filter(|drag| drag.dragging && drag.nesting)
        {
            let title = self
                .tabs
                .iter()
                .find(|tab| tab.id == drag.source.tab)
                .map(|tab| tab.title(self.config.tabs, cx))
                .unwrap_or_default();
            (drag.nest, drag.pointer, title, None)
        } else {
            *self.drawn_drop.borrow_mut() = None;
            return elements;
        };
        *self.drawn_drop.borrow_mut() = Some(drop_label(target, &self.tabs));
        elements.extend(self.target_elements(target, source, colors, window));
        elements.push(
            div()
                .absolute()
                .left(pointer.x + px(12.0))
                .top(pointer.y + px(12.0))
                .max_w(px(240.0))
                .overflow_hidden()
                .px(px(8.0))
                .py(px(2.0))
                .rounded(px(6.0))
                .bg(colors.active)
                .border_1()
                .border_color(colors.border)
                .text_color(colors.foreground)
                .opacity(0.9)
                .whitespace_nowrap()
                .child(title)
                .into_any_element(),
        );
        elements
    }

    /// The result region of a drag `target`, with the dragged pane's
    /// `source` frame outlined for a swap, or a tab bar slot's marker.
    fn target_elements(
        &self,
        target: Option<DropTarget>,
        source: Option<Bounds<Pixels>>,
        colors: TabColors,
        window: &Window,
    ) -> Vec<gpui::AnyElement> {
        let mut elements = Vec::new();
        let outline = |bounds: Bounds<Pixels>, color: Hsla| {
            div()
                .absolute()
                .left(bounds.origin.x)
                .top(bounds.origin.y)
                .w(bounds.size.width)
                .h(bounds.size.height)
                .border_2()
                .border_color(color)
                .into_any_element()
        };
        match target {
            Some(DropTarget::Pane {
                frame,
                zone,
                refused,
                ..
            }) => {
                let color = if refused { colors.error } else { colors.accent };
                let region = zone_preview(frame, zone);
                elements.push(
                    div()
                        .absolute()
                        .left(region.origin.x)
                        .top(region.origin.y)
                        .w(region.size.width)
                        .h(region.size.height)
                        .bg(color.opacity(0.22))
                        .border_2()
                        .border_color(color.opacity(0.8))
                        .into_any_element(),
                );
                if zone == Zone::Centre
                    && let Some(source) = source
                {
                    elements.push(outline(source, color.opacity(0.8)));
                }
            }
            Some(DropTarget::Slot(index)) => {
                let marker = self.tab_strip(window).marker(index);
                elements.push(
                    div()
                        .absolute()
                        .left(marker.origin.x)
                        .top(marker.origin.y)
                        .w(marker.size.width)
                        .h(marker.size.height)
                        .bg(colors.accent)
                        .into_any_element(),
                );
            }
            // The armed label draws its own outline.
            Some(DropTarget::Spring(_)) | None => {}
        }
        elements
    }
}

/// A drop target as smoke state names it: `pane:<index>:<zone>` with the
/// target pane's index in its tab's traversal order, `slot:<index>`,
/// `spring:<tab index>`, or `none`. A refused zone ends in `:refused`.
fn drop_label(target: Option<DropTarget>, tabs: &[TabView]) -> String {
    match target {
        None => "none".to_owned(),
        Some(DropTarget::Slot(index)) => format!("slot:{index}"),
        Some(DropTarget::Spring(tab)) => format!(
            "spring:{}",
            tabs.iter()
                .position(|view| view.id == tab)
                .map_or_else(|| "?".to_owned(), |index| index.to_string())
        ),
        Some(DropTarget::Pane {
            tab,
            pane,
            zone,
            refused,
            ..
        }) => {
            let index = tabs
                .iter()
                .find(|view| view.id == tab)
                .and_then(|view| view.panes.iter().position(|p| p.pane == pane))
                .map_or_else(|| "?".to_owned(), |index| index.to_string());
            let zone = match zone {
                Zone::Centre => "centre",
                Zone::Edge(SplitDirection::Left) => "left",
                Zone::Edge(SplitDirection::Right) => "right",
                Zone::Edge(SplitDirection::Up) => "up",
                Zone::Edge(SplitDirection::Down) => "down",
            };
            let refused = if refused { ":refused" } else { "" };
            format!("pane:{index}:{zone}{refused}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spring_load_arms_rearms_and_comes_due_at_its_deadline() {
        let (first, second) = (TabId::new(1), TabId::new(2));
        let start = Instant::now();
        let mut spring = SpringLoad::default();
        assert_eq!(spring.deadline(), None);
        spring.hover(Some(first), start);
        assert_eq!(spring.tab(), Some(first));
        assert_eq!(spring.deadline(), Some(start + SPRING_LOAD_DELAY));
        // Hovering the same label keeps the first arming.
        spring.hover(Some(first), start + Duration::from_millis(300));
        assert_eq!(spring.armed_at(), Some(start));
        let deadline = start + SPRING_LOAD_DELAY;
        let early = deadline.checked_sub(Duration::from_millis(1)).unwrap();
        assert_eq!(spring.due(early), None);
        assert_eq!(spring.due(deadline), Some(first));
        // Another label re-arms with a fresh time; leaving disarms.
        let later = start + Duration::from_millis(400);
        spring.hover(Some(second), later);
        assert_eq!(spring.tab(), Some(second));
        assert_eq!(spring.due(deadline), None);
        assert_eq!(spring.deadline(), Some(later + SPRING_LOAD_DELAY));
        spring.rearm(Some(second), later + Duration::from_millis(100));
        assert_eq!(spring.armed_at(), Some(later + Duration::from_millis(100)));
        spring.hover(None, later);
        assert_eq!(spring, SpringLoad::default());
    }
}
