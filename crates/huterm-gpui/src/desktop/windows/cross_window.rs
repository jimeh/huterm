//! Source-owned cross-window gestures and two-stage transfer completion.
//! Preview updates are deferred and tagged; release resolves again and reserves
//! both windows synchronously before any runtime worker can start.
use super::pane_drag::{DropTarget, SpringLoad};
use super::pane_geometry::{Zone, drop_zone};
use super::pane_moves::PaneMove;
use super::tab_strip::LabelZone;
use super::window_locator::{Located, NativeLocator, WindowLocator};
use super::*;
use huterm_core::{Beside, MoveDestination, PaneRef};

pub(super) struct CrossWindows {
    pub(super) locator: Rc<dyn WindowLocator>,
    /// Most recently active first. Native identities require no window reads.
    pub(super) windows: Vec<(gpui::WindowId, u64)>,
    generation: u64,
    previews: HashMap<gpui::WindowId, (u64, u64, String)>,
    pub(super) opening: Option<gpui::Size<Pixels>>,
    pub(super) drag: Option<Drag>,
    #[cfg(test)]
    pub(super) fail_window: bool,
}
impl Default for CrossWindows {
    fn default() -> Self {
        Self {
            locator: Rc::new(NativeLocator),
            windows: Vec::new(),
            generation: 0,
            previews: HashMap::new(),
            opening: None,
            drag: None,
            #[cfg(test)]
            fail_window: false,
        }
    }
}

impl CrossWindows {
    pub(super) fn activated(&mut self, id: gpui::WindowId) {
        if let Some(index) =
            self.windows.iter().position(|(window, _)| *window == id)
        {
            let entry = self.windows.remove(index);
            self.windows.insert(0, entry);
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Payload {
    pub(super) source: gpui::WindowId,
    workspace: WorkspaceId,
    tab: TabId,
    pane: Option<PaneId>,
    panes: Vec<PaneId>,
    focus: PaneId,
    pub(super) title: String,
}

#[derive(Clone)]
pub(super) struct Drag {
    generation: u64,
    sample: u64,
    pub(super) payload: Payload,
    pub(super) located: Located,
}

pub(super) struct Incoming {
    generation: u64,
    sample: u64,
    pub(super) payload: Payload,
    pub(super) pointer: gpui::Point<Pixels>,
    pub(super) target: Option<DropTarget>,
    origin: Option<TabId>,
    sprung: Option<TabId>,
    pub(super) spring: SpringLoad,
    scroll: Pixels,
    strip: TabStrip,
    over_bar: bool,
}
impl Incoming {
    pub(super) fn scrolls(&self, scroll: Pixels) -> bool {
        self.over_bar
            && self
                .strip
                .autoscroll(self.pointer, Duration::from_millis(16))
                != scroll
    }
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

pub(super) struct PendingTransfer {
    panes: Vec<PaneId>,
    focus: PaneId,
    touched: Vec<TabId>,
    pub(super) installing: bool,
    created: bool,
    closes: Vec<PaneId>,
    preview: Option<PreviewRestore>,
}

struct PreviewRestore {
    origin: Option<TabId>,
    sprung: Option<TabId>,
    scroll: Pixels,
}

#[derive(Clone, Copy)]
enum Transfer {
    Tab {
        workspace: WorkspaceId,
        tab: TabId,
        to: WorkspaceId,
        before: Option<TabId>,
    },
    Pane(PaneMove),
}
impl DesktopRuntime {
    fn transfer(&self, change: Transfer) -> Committed<Result<(), MuxError>> {
        match change {
            Transfer::Tab {
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
            Transfer::Pane(change) => {
                let Committed { result, seq } = self.commit_pane_move(change);
                Committed {
                    result: result.map(|_| ()),
                    seq,
                }
            }
        }
    }
}

fn handle(cx: &App, id: gpui::WindowId) -> Option<AnyWindowHandle> {
    cx.windows()
        .into_iter()
        .find(|handle| handle.window_id() == id)
}

impl WorkspaceView {
    fn cross_payload(&self, cx: &App) -> Option<Payload> {
        let (tab, pane) = if let Some(drag) =
            self.reorder.as_ref().filter(|drag| drag.dragging)
        {
            (drag.source.tab, None)
        } else {
            let drag = self.pane_drag.as_ref().filter(|drag| drag.dragging)?;
            (drag.tab, Some(drag.pane))
        };
        self.payload(tab, pane, cx)
    }

    fn payload(
        &self,
        tab: TabId,
        pane: Option<PaneId>,
        cx: &App,
    ) -> Option<Payload> {
        let view = self.tabs.iter().find(|view| view.id == tab)?;
        let layout = &cx.global::<Desktop>().hierarchy.state().tab(tab)?.layout;
        if pane.is_some_and(|pane| layout.pane(pane).is_none()) {
            return None;
        }
        let focused = view.focused().pane;
        let focused = if layout.pane(focused).is_some() {
            focused
        } else {
            layout.first_pane().id
        };
        Some(Payload {
            source: self.window,
            workspace: self.workspace_id(cx)?,
            tab,
            pane,
            panes: pane.map_or_else(
                || layout.panes().into_iter().map(|pane| pane.id).collect(),
                |pane| vec![pane],
            ),
            focus: pane.unwrap_or(focused),
            title: pane.and_then(|pane| view.pane(pane)).map_or_else(
                || view.title(self.config.tabs, cx),
                |pane| view.pane_label(pane, self.config.tabs, cx),
            ),
        })
    }

    /// Returns true outside the source, where local targeting and scrolling
    /// must pause. Only the source writes the shared gesture record.
    pub(super) fn route_cross_drag(
        &mut self,
        pointer: gpui::Point<Pixels>,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Some(payload) = self.cross_payload(cx) else {
            return false;
        };
        let desktop = cx.global::<Desktop>();
        let located = desktop.cross.locator.locate(
            window,
            pointer,
            &desktop.cross.windows,
        );
        let desktop = cx.global_mut::<Desktop>();
        if desktop
            .cross
            .drag
            .as_ref()
            .is_none_or(|drag| drag.payload.source != self.window)
        {
            desktop.cross.generation = desktop.cross.generation.wrapping_add(1);
            desktop.cross.drag = Some(Drag {
                generation: desktop.cross.generation,
                sample: 0,
                payload: payload.clone(),
                located,
            });
        }
        let Some(drag) = desktop.cross.drag.as_mut() else {
            return false;
        };
        // Membership and focused pane may have changed since the press.
        // Release comes through this same sampler before it reserves either end.
        drag.payload = payload;
        let previous = drag.located;
        drag.sample = drag.sample.wrapping_add(1);
        drag.located = located;
        let snapshot = drag.clone();
        if let Located::Window(id, _) = previous
            && id != self.window
            && previous_window(previous) != previous_window(located)
        {
            clear_incoming_later(id, snapshot.generation, true, cx);
        }
        if let Located::Window(id, point) = located
            && id != self.window
        {
            cx.defer(move |cx| {
                if let Some(handle) = handle(cx, id) {
                    let _ = handle.update(cx, |root, window, cx| {
                        if let Ok(view) = root.downcast::<WorkspaceView>() {
                            view.update(cx, |view, cx| {
                                view.receive_incoming(
                                    snapshot, point, window, cx,
                                );
                            });
                        }
                    });
                }
            });
        }
        !matches!(located, Located::Window(id, _) if id == self.window)
    }

    pub(super) fn cross_outside(&self, cx: &App) -> bool {
        cx.global::<Desktop>().cross.drag.as_ref().is_some_and(|drag| drag.payload.source == self.window && !matches!(drag.located, Located::Window(id, _) if id == self.window))
    }

    pub(super) fn tear_off_cue(&self, cx: &App) -> bool {
        self.tabs.len() > 1
            && cx
                .global::<Desktop>()
                .cross
                .drag
                .as_ref()
                .is_some_and(|drag| {
                    drag.payload.source == self.window
                        && drag.payload.pane.is_none()
                        && drag.located == Located::Outside
                })
    }

    pub(super) fn end_cross_drag(
        &mut self,
        back: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let desktop = cx.global_mut::<Desktop>();
        if desktop
            .cross
            .drag
            .as_ref()
            .is_some_and(|drag| drag.payload.source == self.window)
        {
            if let Some(drag) = desktop.cross.drag.take()
                && let Located::Window(id, _) = drag.located
                && id != self.window
            {
                clear_incoming_later(id, drag.generation, back, cx);
            }
            cx.notify();
        }
    }

    fn cross_eligible(&self, cx: &App) -> bool {
        !self.busy
            && self.arrangements == 0
            && !self.overlay_holds_focus()
            && self.native_menu.is_none()
            && self.close.current.is_none()
            && self.record(cx).is_some_and(|record| {
                !record.closing && record.attachment.is_some()
            })
            && !cx.global::<Desktop>().quitting
            && !terminating(cx)
            && self.quake.as_ref().is_none_or(|quake| {
                window_locator::target_visible(
                    quake.visible(),
                    false,
                    quake.presenting(),
                )
            })
    }

    pub(super) fn receive_incoming(
        &mut self,
        drag: Drag,
        pointer: gpui::Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        // Validate at delivery, including callbacks queued before cancellation
        // or a newer sample from this same source window.
        let current =
            cx.global::<Desktop>()
                .cross
                .drag
                .as_ref()
                .is_some_and(|current| {
                    current.generation == drag.generation
                        && current.sample == drag.sample
                });
        if !current {
            return;
        }
        if !self.cross_eligible(cx) {
            self.clear_incoming(true, window, cx);
            return;
        }
        if self
            .incoming
            .as_ref()
            .is_none_or(|incoming| incoming.generation != drag.generation)
        {
            self.clear_incoming(true, window, cx);
            self.incoming = Some(Incoming {
                generation: drag.generation,
                sample: drag.sample,
                payload: drag.payload.clone(),
                pointer,
                target: None,
                origin: self.active_tab(cx),
                sprung: None,
                spring: SpringLoad::default(),
                scroll: self.tab_scroll,
                strip: self.tab_strip(window),
                over_bar: false,
            });
            self.last_scroll = Instant::now();
        }
        if let Some(incoming) = &mut self.incoming {
            if incoming.sample > drag.sample {
                return;
            }
            incoming.sample = drag.sample;
            incoming.payload = drag.payload;
            incoming.pointer = pointer;
        }
        self.refresh_tab_visibility(window, cx);
        self.retarget_incoming(Instant::now(), false, window, cx);
        cx.notify();
    }

    pub(super) fn clear_incoming(
        &mut self,
        back: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        cx.global_mut::<Desktop>()
            .cross
            .previews
            .remove(&self.window);
        if let Some(incoming) = self.incoming.take() {
            if back {
                self.tab_scroll = incoming.scroll;
                if incoming.sprung == self.active_tab(cx)
                    && let Some(origin) = incoming.origin
                {
                    self.select_tab(origin, false, window, cx);
                }
            }
            cx.notify();
        }
    }

    fn incoming_target(
        &self,
        payload: &Payload,
        pointer: gpui::Point<Pixels>,
        window: &Window,
        cx: &App,
    ) -> Option<DropTarget> {
        if !self.bar_drawn()
            && self.presentation() == Presentation::Hidden
            && self.title_row() > px(0.0)
            && self
                .title_row_layout(&self.chrome_layout(window))
                .drop_region
                .contains(&pointer)
        {
            return Some(DropTarget::TitleRow);
        }
        if self.over_tab_bar(pointer, window) {
            let active = self.active_tab(cx).and_then(|active| {
                self.tabs.iter().position(|tab| tab.id == active)
            });
            return match self.tab_strip(window).label_zone(pointer, active)? {
                LabelZone::Spring(index) => {
                    self.tabs.get(index).map(|tab| DropTarget::Spring(tab.id))
                }
                LabelZone::Slot(index) => Some(DropTarget::Slot(index)),
            };
        }
        let (tab, rect) = self.drawn_pane_at(pointer)?;
        if self.active_tab(cx) != Some(tab) {
            return None;
        }
        let Zone::Edge(side) = drop_zone(rect.frame, pointer)? else {
            return None;
        };
        let target = DropTarget::Pane {
            tab,
            pane: rect.pane,
            frame: rect.frame,
            zone: Zone::Edge(side),
            refused: false,
        };
        let change = self.cross_change(payload, target, cx)?;
        let refused = match change {
            Transfer::Pane(change) => {
                self.check_move_fits(change, window, cx).is_err()
            }
            Transfer::Tab { .. } => false,
        };
        Some(DropTarget::Pane {
            tab,
            pane: rect.pane,
            frame: rect.frame,
            zone: Zone::Edge(side),
            refused,
        })
    }

    fn cross_change(
        &self,
        payload: &Payload,
        target: DropTarget,
        cx: &App,
    ) -> Option<Transfer> {
        let workspace = self.workspace_id(cx)?;
        match target {
            DropTarget::Pane {
                tab,
                pane,
                zone: Zone::Edge(side),
                refused: false,
                ..
            } => {
                let to = Beside {
                    target: PaneRef {
                        workspace,
                        tab,
                        pane,
                    },
                    side,
                };
                Some(Transfer::Pane(if let Some(pane) = payload.pane {
                    PaneMove::Move {
                        from: PaneRef {
                            workspace: payload.workspace,
                            tab: payload.tab,
                            pane,
                        },
                        to: MoveDestination::Beside(to),
                    }
                } else {
                    PaneMove::TabInto {
                        workspace: payload.workspace,
                        tab: payload.tab,
                        to,
                    }
                }))
            }
            DropTarget::Slot(_) | DropTarget::TitleRow => {
                let before = match target {
                    DropTarget::Slot(index) => {
                        self.tabs.get(index).map(|tab| tab.id)
                    }
                    _ => None,
                };
                Some(if let Some(pane) = payload.pane {
                    Transfer::Pane(PaneMove::Move {
                        from: PaneRef {
                            workspace: payload.workspace,
                            tab: payload.tab,
                            pane,
                        },
                        to: MoveDestination::NewTab { workspace, before },
                    })
                } else {
                    Transfer::Tab {
                        workspace: payload.workspace,
                        tab: payload.tab,
                        to: workspace,
                        before,
                    }
                })
            }
            _ => None,
        }
    }

    pub(super) fn retarget_incoming(
        &mut self,
        now: Instant,
        rearm: bool,
        window: &Window,
        cx: &mut App,
    ) {
        let Some(incoming) = &self.incoming else {
            return;
        };
        let target = self.incoming_target(
            &incoming.payload,
            incoming.pointer,
            window,
            cx,
        );
        let over_bar = self.over_tab_bar(incoming.pointer, window);
        let strip = self.tab_strip(window);
        if let Some(incoming) = &mut self.incoming {
            incoming.target = target;
            incoming.over_bar = over_bar;
            incoming.strip = strip;
            let tab = match target {
                Some(DropTarget::Spring(tab)) => Some(tab),
                _ => None,
            };
            if rearm {
                incoming.spring.rearm(tab, now);
            } else {
                incoming.spring.hover(tab, now);
            }
            let label =
                super::pane_drag::drop_label(incoming.target, &self.tabs);
            cx.global_mut::<Desktop>().cross.previews.insert(
                self.window,
                (incoming.generation, incoming.sample, label),
            );
        }
    }

    pub(super) fn advance_incoming_spring(
        &mut self,
        now: Instant,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(armed) = self
            .incoming
            .as_ref()
            .and_then(|incoming| incoming.spring.due(now))
        else {
            return;
        };
        self.retarget_incoming(now, false, window, cx);
        if let Some(incoming) = &mut self.incoming
            && incoming.target == Some(DropTarget::Spring(armed))
        {
            incoming.spring = SpringLoad::default();
            incoming.sprung = Some(armed);
            self.select_tab(armed, false, window, cx);
            self.retarget_incoming(now, false, window, cx);
            cx.notify();
        }
    }

    /// The caller has already re-sampled the locator at release. A true
    /// result consumes an outside-source release, including a cancellation.
    pub(super) fn finish_cross_drag(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Some(drag) = cx
            .global::<Desktop>()
            .cross
            .drag
            .clone()
            .filter(|drag| drag.payload.source == self.window)
        else {
            return false;
        };
        match drag.located {
            Located::Window(id, _) if id == self.window => {
                self.end_cross_drag(true, cx);
                false
            }
            Located::Window(id, pointer) => {
                let payload = drag.payload.clone();
                let reserved = handle(cx, id).and_then(|handle| {
                    handle
                        .update(cx, |root, target_window, cx| {
                            let view = root.downcast::<WorkspaceView>().ok()?;
                            view.update(cx, |view, cx| {
                                if !view.cross_eligible(cx) {
                                    return None;
                                }
                                let workspace = view.workspace_id(cx)?;
                                if workspace.runtime()
                                    != payload.workspace.runtime()
                                {
                                    return Some(Err(()));
                                }
                                let target = view.incoming_target(
                                    &payload,
                                    pointer,
                                    target_window,
                                    cx,
                                )?;
                                let change =
                                    view.cross_change(&payload, target, cx)?;
                                let mut touched = vec![payload.tab];
                                if let DropTarget::Pane { tab, .. } = target {
                                    touched.push(tab);
                                }
                                let preview = view
                                    .incoming
                                    .as_ref()
                                    .map(Incoming::restore);
                                view.clear_incoming(false, target_window, cx);
                                view.busy = true;
                                view.transfer = Some(PendingTransfer {
                                    panes: payload.panes.clone(),
                                    focus: payload.focus,
                                    touched,
                                    installing: false,
                                    created: false,
                                    closes: Vec::new(),
                                    preview,
                                });
                                cx.notify();
                                Some(Ok(change))
                            })
                        })
                        .ok()
                        .flatten()
                });
                match reserved {
                    Some(Ok(change)) => {
                        self.end_cross_drag(false, cx);
                        self.dispatch_transfer(
                            drag.payload.source,
                            id,
                            change,
                            window,
                            cx,
                        );
                    }
                    Some(Err(())) => {
                        self.end_cross_drag(true, cx);
                        self.report_failure(
                            "Move terminal",
                            "Cannot move between different runtimes",
                            cx,
                        );
                    }
                    None => self.end_cross_drag(true, cx),
                }
                true
            }
            Located::Outside
                if drag.payload.pane.is_none() && self.tabs.len() > 1 =>
            {
                self.end_cross_drag(true, cx);
                self.tear_off(drag.payload, window, cx);
                true
            }
            _ => {
                self.end_cross_drag(true, cx);
                true
            }
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one completion settles both reserved endpoints in order"
    )]
    fn dispatch_transfer(
        &mut self,
        source: gpui::WindowId,
        destination: gpui::WindowId,
        change: Transfer,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let source_tab = match change {
            Transfer::Tab { tab, .. }
            | Transfer::Pane(
                PaneMove::TabInto { tab, .. } | PaneMove::Swap { tab, .. },
            ) => tab,
            Transfer::Pane(PaneMove::Move { from, .. }) => from.tab,
        };
        let moving: Vec<PaneId> = match change {
            Transfer::Pane(PaneMove::Move { from, .. }) => vec![from.pane],
            _ => self
                .tabs
                .iter()
                .find(|tab| tab.id == source_tab)
                .map(|tab| tab.panes.iter().map(|pane| pane.pane).collect())
                .unwrap_or_default(),
        };
        let scroll = self
            .reorder
            .as_ref()
            .map(|drag| drag.original_scroll)
            .or(self.transfer_scroll.take());
        let back = self.pane_drag.as_ref().and_then(PaneDrag::back);
        self.busy = true;
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let task = cx.background_executor().spawn(async move {
            #[cfg(all(test, feature = "test-support"))]
            if runtime
                .worker_hooks
                .enter(worker_hooks::Stage::Transfer)
                .await
            {
                return Committed {
                    result: Err(RuntimeError::Stopped.into()),
                    seq: runtime.lock().hierarchy_seq(),
                };
            }
            let committed = runtime.transfer(change);
            #[cfg(all(test, feature = "test-support"))]
            runtime
                .worker_hooks
                .enter(worker_hooks::Stage::LayoutCommitted)
                .await;
            committed
        });
        let app = cx.to_async();
        cx.spawn_in(window, async move |_, _cx| {
            let Committed { result, seq } = task.await;
            projected(&app, seq).await;
            app.update(|cx| {
                // Both endpoints settle even if either view was removed.
                let mut closes = Vec::new();
                if let Some(handle) = handle(cx, source) {
                    let _ = handle.update(cx, |root, window, cx| {
                        if let Ok(view) = root.downcast::<WorkspaceView>() {
                            view.update(cx, |view, cx| {
                                view.busy = false;
                                if result.is_ok()
                                    && let Some(CloseTarget::Pane {
                                        pane, ..
                                    }) = view.close.pending.as_ref()
                                    && moving.contains(pane)
                                {
                                    closes.push(*pane);
                                    view.close.pending = None;
                                }
                                if result.is_err() {
                                    if let Some(scroll) = scroll {
                                        view.tab_scroll = scroll;
                                    }
                                    if let Some(back) = back {
                                        view.switch_back(back, window, cx);
                                    }
                                }
                                if result.is_ok() {
                                    view.clear_transfer_zoom(&[source_tab]);
                                }
                                if let Err(error) = &result
                                    && !terminating(cx)
                                {
                                    view.report_failure(
                                        "Move terminal",
                                        format!(
                                            "Cannot move terminal: {error}"
                                        ),
                                        cx,
                                    );
                                }
                                view.reconcile_own(window, cx);
                                view.resume_close(window, cx);
                                cx.notify();
                            });
                        }
                    });
                }
                if let Some(handle) = handle(cx, destination) {
                    let _ = handle.update(cx, |root, window, cx| {
                        if let Ok(view) = root.downcast::<WorkspaceView>() {
                            view.update(cx, |view, cx| {
                                if result.is_ok() {
                                    let touched = view
                                        .transfer
                                        .as_mut()
                                        .map(|pending| {
                                            pending.installing = true;
                                            pending.closes = closes;
                                            pending.touched.clone()
                                        })
                                        .unwrap_or_default();
                                    view.clear_transfer_zoom(&touched);
                                    view.reconcile_own(window, cx);
                                    view.finish_transfer_install(window, cx);
                                } else {
                                    let pending = view.transfer.take();
                                    let created = pending
                                        .as_ref()
                                        .is_some_and(|pending| pending.created);
                                    if let Some(preview) = pending
                                        .and_then(|pending| pending.preview)
                                    {
                                        view.tab_scroll = preview.scroll;
                                        if preview.sprung == view.active_tab(cx)
                                            && let Some(origin) = preview.origin
                                        {
                                            view.select_tab(
                                                origin, false, window, cx,
                                            );
                                        }
                                    }
                                    view.busy = false;
                                    if created {
                                        view.request_close(
                                            CloseTarget::Window,
                                            window,
                                            cx,
                                        );
                                    } else {
                                        view.reconcile_own(window, cx);
                                        view.resume_close(window, cx);
                                    }
                                }
                                cx.notify();
                            });
                        }
                    });
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn clear_transfer_zoom(&mut self, tabs: &[TabId]) {
        for tab in &mut self.tabs {
            if tabs.contains(&tab.id) {
                tab.zoomed = None;
            }
        }
    }

    pub(super) fn projected_pane_tab(
        &self,
        pane: PaneId,
        cx: &App,
    ) -> Option<TabId> {
        let state = cx.global::<Desktop>().hierarchy.state();
        state
            .pane_tab(pane)
            .filter(|tab| state.tab_workspace(*tab) == self.workspace_id(cx))
    }

    pub(super) fn transfer_contains(&self, pane: PaneId) -> bool {
        self.transfer
            .as_ref()
            .is_some_and(|pending| pending.panes.contains(&pane))
    }

    pub(super) fn finish_transfer_install(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(pending) = &self.transfer else {
            return;
        };
        if !pending.installing {
            return;
        }
        let in_workspace = |pane| self.projected_pane_tab(pane, cx);
        let installed = |pane| {
            self.tabs.iter().any(|tab| {
                tab.pane(pane)
                    .is_some_and(|pane| !pane.view.read(cx).revoked)
            })
        };
        let failed = pending.panes.iter().any(|pane| {
            !installed(*pane)
                && in_workspace(*pane).is_some()
                && self.installs.failed(*pane)
        });
        if !failed
            && !pending
                .panes
                .iter()
                .all(|pane| in_workspace(*pane).is_none() || installed(*pane))
        {
            return;
        }
        let focus = pending.focus;
        let closes = pending.closes.clone();
        self.transfer = None;
        self.busy = false;
        if let Some(tab) = self.tab_showing(focus) {
            self.select(tab, window, cx);
            self.focus_pane(tab, focus, window, cx);
        }
        if !terminating(cx) {
            // Activation also shows a hidden macOS tear-off host, even when
            // it has no installed view to focus after a subscription failure.
            window.activate_window();
        }
        for pane in closes {
            if let Some(tab) = self.projected_pane_tab(pane, cx) {
                self.close.queue(CloseTarget::Pane { tab, pane });
            }
        }
        self.resume_close(window, cx);
        cx.notify();
    }
}

fn previous_window(located: Located) -> Option<gpui::WindowId> {
    match located {
        Located::Window(id, _) => Some(id),
        _ => None,
    }
}
fn clear_incoming_later(
    id: gpui::WindowId,
    generation: u64,
    back: bool,
    cx: &mut App,
) {
    cx.defer(move |cx| {
        if cx
            .global::<Desktop>()
            .cross
            .drag
            .as_ref()
            .is_some_and(|drag| {
                drag.generation == generation
                    && previous_window(drag.located) == Some(id)
            })
        {
            return;
        }
        if let Some(handle) = handle(cx, id) {
            let _ = handle.update(cx, |root, window, cx| {
                if let Ok(view) = root.downcast::<WorkspaceView>() {
                    view.update(cx, |view, cx| {
                        if view.incoming.as_ref().is_some_and(|incoming| {
                            incoming.generation == generation
                        }) {
                            view.clear_incoming(back, window, cx);
                        }
                    });
                }
            });
        }
    });
}

impl Incoming {
    fn restore(&self) -> PreviewRestore {
        PreviewRestore {
            origin: self.origin,
            sprung: self.sprung,
            scroll: self.scroll,
        }
    }
}

impl WorkspaceView {
    /// Open the empty destination before moving membership. Its reservation
    /// prevents an empty-close while the transfer has not committed yet.
    #[expect(
        clippy::too_many_lines,
        reason = "window creation owns dispatch and empty-session cleanup across awaits"
    )]
    fn tear_off(
        &mut self,
        payload: Payload,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.busy
            || self.close.confirmation.is_some()
            || cx.global::<Desktop>().quitting
            || terminating(cx)
        {
            return;
        }
        self.busy = true;
        self.transfer_scroll =
            self.reorder.as_ref().map(|drag| drag.original_scroll);
        let content_size = window.viewport_size();
        #[cfg(target_os = "macos")]
        let placement = self.reorder.as_ref().and_then(|drag| {
            window_locator::screen_point(window, drag.pointer)
                .map(|pointer| (pointer, drag.origin))
        });
        let runtime = Arc::clone(&cx.global::<Desktop>().runtime);
        let cleanup_runtime = Arc::clone(&runtime);
        let task = cx.background_executor().spawn(async move {
            let mut mux = runtime.lock();
            let result = if runtime.terminating.load(Ordering::Acquire) {
                Err(RuntimeError::Stopped.into())
            } else {
                mux.create_session(None).and_then(|session| {
                    let result = mux.create_workspace(session, None).and_then(
                        |workspace| {
                            mux.attach_session(session).map(|attachment| {
                                (session, workspace, attachment)
                            })
                        },
                    );
                    if result.is_err() {
                        let _ = mux.close_session(session);
                    }
                    result
                })
            };
            Committed {
                result,
                seq: mux.hierarchy_seq(),
            }
        });
        let app = cx.to_async();
        cx.spawn_in(window, async move |source, cx| {
            let Committed { result, seq } = task.await;
            projected(&app, seq).await;
            let Ok((session, workspace, attachment)) = result else {
                let _ = source.update_in(cx, |view, window, cx| {
                    view.busy = false;
                    if let Some(scroll) = view.transfer_scroll.take() {
                        view.tab_scroll = scroll;
                    }
                    if !terminating(cx) {
                        view.report_failure(
                            "Move tab",
                            "Cannot create a destination session",
                            cx,
                        );
                    }
                    view.resume_close(window, cx);
                    cx.notify();
                });
                return;
            };
            let destination = app.update(|cx| {
                if terminating(cx) || cx.global::<Desktop>().quitting {
                    return None;
                }
                #[cfg(test)]
                if std::mem::take(
                    &mut cx.global_mut::<Desktop>().cross.fail_window,
                ) {
                    return None;
                }
                let before = cx.windows();
                cx.global_mut::<Desktop>().cross.opening = Some(content_size);
                open_window_inner(cx, false);
                cx.global_mut::<Desktop>().cross.opening = None;
                let handle = cx
                    .windows()
                    .into_iter()
                    .find(|handle| !before.contains(handle))?;
                handle
                    .update(cx, |root, _, cx| {
                        root.downcast::<WorkspaceView>().ok()?.update(
                            cx,
                            |view, cx| {
                                view.busy = true;
                                view.transfer = Some(PendingTransfer {
                                    panes: payload.panes.clone(),
                                    focus: payload.focus,
                                    touched: vec![payload.tab],
                                    installing: false,
                                    created: true,
                                    closes: Vec::new(),
                                    preview: None,
                                });
                                view.adopt_attachment(
                                    attachment, session, workspace, cx,
                                );
                            },
                        );
                        Some(())
                    })
                    .ok()
                    .flatten()?;
                Some(handle)
            });
            if let Some(destination) = destination {
                #[cfg(target_os = "macos")]
                if let Some((pointer, grab)) = placement {
                    let number = app.update(|cx| {
                        cx.global::<Desktop>()
                            .cross
                            .windows
                            .iter()
                            .find(|(id, _)| *id == destination.window_id())
                            .map(|(_, native)| *native)
                    });
                    if let Some(number) = number {
                        window_locator::place(number, pointer, grab);
                    }
                }
                let accepted = source
                    .update_in(cx, |view, window, cx| {
                        view.busy = false;
                        if terminating(cx) || cx.global::<Desktop>().quitting {
                            return false;
                        }
                        view.dispatch_transfer(
                            payload.source,
                            destination.window_id(),
                            Transfer::Tab {
                                workspace: payload.workspace,
                                tab: payload.tab,
                                to: workspace,
                                before: None,
                            },
                            window,
                            cx,
                        );
                        true
                    })
                    .unwrap_or(false);
                if accepted {
                    return;
                }
                let _ = destination.update(cx, |root, window, cx| {
                    if let Ok(view) = root.downcast::<WorkspaceView>() {
                        view.update(cx, |view, cx| {
                            view.transfer = None;
                            view.busy = false;
                            view.remove_window(window, cx, false);
                        });
                    }
                });
            }
            // Only the empty session can reach this cleanup. A successful
            // move returns above and is never rolled back by stopping PTYs.
            cx.background_executor()
                .spawn(async move {
                    let _ = cleanup_runtime.lock().close_session(session);
                })
                .await;
            let _ = source.update_in(cx, |view, window, cx| {
                view.busy = false;
                if let Some(scroll) = view.transfer_scroll.take() {
                    view.tab_scroll = scroll;
                }
                if !terminating(cx) {
                    view.report_failure(
                        "Move tab",
                        "Cannot open the destination window",
                        cx,
                    );
                }
                view.resume_close(window, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The free bar after the labels and add control belongs to the pill.
    /// A horizontal bar too short for the labelled pill takes the compact
    /// one there; only when neither fits does it fall back over the strip's
    /// end, before the add control.
    pub(super) fn tear_off_pill_bounds(
        &self,
        strip: &TabStrip,
        layout: &ChromeLayout,
    ) -> Bounds<Pixels> {
        let mut area = layout.strip_bounds(self.layout_tabs());
        if !strip.vertical
            && self.bar_menu_placement() == MenuButtonPlacement::BarEnd
        {
            area.size.width = (area.size.width - CONTROL_SLOT).max(px(0.0));
        }
        let gap = px(6.0);
        let width =
            TEAR_OFF_PILL.min((area.size.width - gap * 2.0).max(px(0.0)));
        let height = px(24.0).min(area.size.height);
        let origin = if strip.vertical {
            let after = strip.bounds.bottom() + CONTROL_SLOT + gap;
            point(
                area.left() + gap,
                if after + height <= area.bottom() {
                    after
                } else {
                    (strip.bounds.bottom() - height).max(area.top())
                },
            )
        } else {
            let after = strip.bounds.right() + CONTROL_SLOT + gap;
            let top = area.top() + (area.size.height - height) / 2.0;
            if after + width <= area.right() {
                point(after, top)
            } else if after + TEAR_OFF_COMPACT <= area.right() {
                return Bounds::new(
                    point(after, top),
                    size(TEAR_OFF_COMPACT, height),
                );
            } else {
                point((strip.bounds.right() - width).max(area.left()), top)
            }
        };
        Bounds::new(origin, size(width, height))
    }

    pub(super) fn check_tab_tear_off_available(
        &self,
        cx: &App,
    ) -> Result<(), CommandError> {
        self.check_new_tab_available(cx)?;
        self.check_runtime_available(cx)?;
        tab_tear_off_availability(self.tabs.len())
    }

    pub(super) fn move_tab_to_new_window(
        &mut self,
        invocation: &CommandInvocation,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Result<CommandOutcome, CommandError> {
        self.check_tab_tear_off_available(cx)?;
        let tab = self.target_tab(invocation, cx)?;
        let payload = self
            .payload(tab, None, cx)
            .ok_or(CommandError::StaleTarget)?;
        self.tear_off(payload, window, cx);
        Ok(CommandOutcome::Accepted)
    }
}

/// The labelled tear-off pill's width, and the icon-only pill a short bar
/// uses instead of covering its tabs.
pub(super) const TEAR_OFF_PILL: Pixels = px(128.0);
pub(super) const TEAR_OFF_COMPACT: Pixels = px(28.0);

/// The tear-off pill's text: the arrow alone once the pill is narrower than
/// its label needs.
pub(super) fn tear_off_label(bounds: Bounds<Pixels>) -> &'static str {
    if bounds.size.width < TEAR_OFF_PILL {
        "↗"
    } else {
        "↗ New window"
    }
}

pub(super) fn tab_tear_off_availability(
    count: usize,
) -> Result<(), CommandError> {
    if count < 2 {
        Err(CommandError::Unavailable(
            "The tab is already alone in its window.".to_owned(),
        ))
    } else {
        Ok(())
    }
}

/// Per-window identities and geometry, shared by the native input fixtures.
/// Called in application context with no window borrowed.
pub(super) fn smoke_state(cx: &mut App) -> String {
    use std::fmt::Write as _;
    let handles = cx.windows();
    let mut output = format!(
        "windows={}\n",
        cx.global::<Desktop>().windows.records().count()
    );
    for (index, handle) in handles.into_iter().enumerate() {
        let _ = handle.update(cx, |root, window, cx| {
            let Ok(entity) = root.downcast::<WorkspaceView>() else { return };
            let view = entity.read(cx);
            let ids = view.tabs.iter().map(|tab| tab.id.get().to_string()).collect::<Vec<_>>().join(",");
            let drawn = view.drawn_tabs.borrow().iter().map(|tab| tab.get().to_string()).collect::<Vec<_>>().join(",");
            let drawn = if drawn.is_empty() { "none".to_owned() } else { drawn };
            let terminals = view.tabs.iter().flat_map(|tab| tab.panes.iter().map(|pane| pane.terminal.get().to_string())).collect::<Vec<_>>().join(",");
            let native = window_locator::identity(window).unwrap_or(0);
            let origin = window_locator::screen_point(window, point(px(0.0), px(0.0))).unwrap_or_default();
            let _ = writeln!(output, "w{index}.native={native} w{index}.active={} w{index}.origin={},{} w{index}.size={},{} w{index}.tabs={} w{index}.tab_ids={ids} w{index}.drawn_tab_ids={drawn} w{index}.terminals={terminals} w{index}.busy={} w{index}.tabs_rects={} {}",
                window.is_window_active(), origin.x, origin.y, f32::from(window.viewport_size().width), f32::from(window.viewport_size().height), view.tabs.len(), view.busy,
                view.tab_smoke_rects(window), view.pane_smoke_state(&format!("w{index}."), window, cx));
        });
    }
    output
}

pub(super) fn start_smoke_publisher(cx: &mut App) {
    let Some(directory) =
        std::env::var_os("HUTERM_INPUT_STATE").map(PathBuf::from)
    else {
        return;
    };
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(10))
                .await;
            let state = cx.update(smoke_state);
            let temporary = directory.join("cross-state.tmp");
            if std::fs::write(&temporary, state)
                .and_then(|()| {
                    std::fs::rename(&temporary, directory.join("cross-state"))
                })
                .is_err()
            {
                break;
            }
        }
    })
    .detach();
}

pub(super) fn closed(id: gpui::WindowId, cx: &mut App) {
    let drag = cx.global::<Desktop>().cross.drag.clone();
    if let Some(drag) = drag
        && (drag.payload.source == id
            || previous_window(drag.located) == Some(id))
    {
        cx.global_mut::<Desktop>().cross.drag = None;
        if let Located::Window(destination, _) = drag.located
            && destination != id
        {
            clear_incoming_later(destination, drag.generation, true, cx);
        }
        if drag.payload.source != id {
            cx.defer(move |cx| {
                if let Some(source) = handle(cx, drag.payload.source) {
                    let _ = source.update(cx, |root, window, cx| {
                        if let Ok(view) = root.downcast::<WorkspaceView>() {
                            view.update(cx, |view, cx| {
                                view.cancel_reorder(window, cx);
                                view.cancel_pane_drag(window, cx);
                            });
                        }
                    });
                }
            });
        }
    }
}

impl WorkspaceView {
    pub(super) fn cross_drop_label(&self, cx: &App) -> Option<String> {
        let cross = &cx.global::<Desktop>().cross;
        let drag = cross.drag.as_ref()?;
        let destination = match drag.located {
            Located::Outside if self.tear_off_cue(cx) => {
                return Some("tear_off".into());
            }
            Located::Window(id, _) => id,
            _ => return Some("none".into()),
        };
        if drag.payload.source != self.window && destination != self.window {
            return None;
        }
        if destination == drag.payload.source {
            return None;
        }
        let label = cross
            .previews
            .get(&destination)
            .filter(|(generation, sample, _)| {
                *generation == drag.generation && *sample == drag.sample
            })
            .map_or("none", |(_, _, label)| label.as_str());
        let index = cx
            .windows()
            .iter()
            .position(|handle| handle.window_id() == destination)?;
        Some(format!("window:{index}:{label}"))
    }
}
